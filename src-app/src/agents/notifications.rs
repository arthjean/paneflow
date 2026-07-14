//! Desktop notification routing for agent lifecycle events.
//!
//! This module owns both sides of the notification gate:
//! - the process-wide focus flag updated by the GPUI app;
//! - a single cross-platform `notify-rust` firing path used by `ai.*` handlers.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::BackgroundExecutor;
use paneflow_config::schema::{AgentPanelConfig, NotifyWhenAgentWaiting, PaneFlowConfig};

use crate::agent_launcher::TerminalAgent;
#[cfg(target_os = "windows")]
use crate::windows_app_identity::PANEFLOW_WINDOWS_AUMID;

const NOTIFICATION_DETAIL_CAP_CHARS: usize = 512;

#[cfg(target_os = "windows")]
const PANEFLOW_WINDOWS_NOTIFICATION_ICON_ASSET: &str = "icons/paneflow.png";
#[cfg(target_os = "windows")]
const PANEFLOW_WINDOWS_NOTIFICATION_ICON_FILE: &str = "paneflow-notification.png";

/// Window-active gate updated by `cx.observe_window_activation`.
/// `true` while the OS reports the Paneflow window as the focused one.
static WINDOW_ACTIVE: AtomicBool = AtomicBool::new(true);

/// Update the window-active flag. Called from
/// `cx.observe_window_activation` and from the initial activation
/// tick that GPUI fires when the observer registers.
pub fn set_window_active(active: bool) {
    WINDOW_ACTIVE.store(active, Ordering::Relaxed);
}

/// Is the Paneflow window currently the focused surface?
pub fn window_active() -> bool {
    WINDOW_ACTIVE.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopNotificationUrgency {
    Normal,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DesktopNotification {
    summary: String,
    body: String,
    urgency: DesktopNotificationUrgency,
}

impl DesktopNotification {
    pub(crate) fn turn_finished(
        agent: TerminalAgent,
        workspace_title: &str,
        session_summary: Option<&str>,
    ) -> Self {
        Self {
            summary: format!("{} finished", agent.display_name()),
            body: notification_context_body(workspace_title, session_summary),
            urgency: DesktopNotificationUrgency::Normal,
        }
    }

    pub(crate) fn needs_input(
        agent: TerminalAgent,
        workspace_title: &str,
        message: Option<&str>,
    ) -> Self {
        Self {
            summary: format!("{} needs input", agent.display_name()),
            body: attention_notification_body(workspace_title, message),
            urgency: DesktopNotificationUrgency::Critical,
        }
    }

    pub(crate) fn agent_exited(
        agent: TerminalAgent,
        workspace_title: &str,
        exit_code: i32,
    ) -> Self {
        Self {
            summary: format!("{} exited unexpectedly", agent.display_name()),
            body: agent_exit_notification_body(workspace_title, exit_code),
            urgency: DesktopNotificationUrgency::Critical,
        }
    }

    pub(crate) fn stalled(agent: TerminalAgent, workspace_title: &str, silent_secs: u64) -> Self {
        Self {
            summary: format!("{} may be stuck", agent.display_name()),
            body: stalled_notification_body(workspace_title, silent_secs),
            urgency: DesktopNotificationUrgency::Critical,
        }
    }
}

/// Fire a best-effort desktop notification without blocking the GPUI thread.
pub(crate) fn fire_desktop_notification(
    notification: DesktopNotification,
    config: &PaneFlowConfig,
    executor: BackgroundExecutor,
) {
    let gate = config.agent_panel.as_ref().map_or(
        NotifyWhenAgentWaiting::Never,
        AgentPanelConfig::resolved_notify_when_agent_waiting,
    );
    if !should_fire_desktop_notification(gate, window_active()) {
        return;
    }

    executor
        .spawn(async move {
            let _ = smol::unblock(move || show_desktop_notification(notification)).await;
        })
        .detach();
}

pub(crate) fn should_fire_desktop_notification(
    gate: NotifyWhenAgentWaiting,
    window_active: bool,
) -> bool {
    match gate {
        NotifyWhenAgentWaiting::Never => false,
        NotifyWhenAgentWaiting::PrimaryScreen | NotifyWhenAgentWaiting::AllScreens => {
            !window_active
        }
    }
}

/// Bound + sanitize an agent question before it is stored on the session
/// and mirrored to notifications.
pub(crate) fn sanitize_notification_message(raw: &str) -> String {
    crate::markdown::strip_bidi_zero_width(raw.chars().take(512).collect())
}

fn notification_detail(raw: &str) -> Option<String> {
    let clean: String = crate::markdown::strip_bidi_zero_width(
        raw.chars().take(NOTIFICATION_DETAIL_CAP_CHARS).collect(),
    )
    .trim()
    .to_string();
    (!clean.is_empty()).then_some(clean)
}

fn notification_context_body(workspace_title: &str, session_summary: Option<&str>) -> String {
    session_summary
        .and_then(notification_detail)
        .or_else(|| notification_detail(workspace_title))
        .unwrap_or_else(|| "Paneflow".to_string())
}

pub(crate) fn attention_notification_body(workspace_title: &str, message: Option<&str>) -> String {
    notification_context_body(workspace_title, message)
}

pub(crate) fn agent_exit_notification_body(workspace_title: &str, exit_code: i32) -> String {
    format!(
        "{}: exited with code {exit_code}",
        notification_context_body(workspace_title, None)
    )
}

pub(crate) fn stalled_notification_body(workspace_title: &str, silent_secs: u64) -> String {
    format!(
        "{}: no activity for {silent_secs} s",
        notification_context_body(workspace_title, None)
    )
}

fn show_desktop_notification(notification: DesktopNotification) -> Result<(), String> {
    let mut builder = notify_rust::Notification::new();
    builder
        .summary(&notification.summary)
        .body(&notification.body)
        .appname("Paneflow")
        .icon("paneflow")
        .timeout(std::time::Duration::from_secs(8));

    #[cfg(any(all(unix, not(target_os = "macos")), target_os = "windows"))]
    builder.urgency(notification_urgency_for_platform(notification.urgency));

    #[cfg(all(unix, not(target_os = "macos")))]
    builder.hint(notify_rust::Hint::DesktopEntry("paneflow".to_string()));

    #[cfg(target_os = "windows")]
    {
        let _ = crate::windows_app_identity::ensure_process_app_user_model_id();
        let _ = ensure_windows_app_user_model_id_registered();
        builder.app_id(PANEFLOW_WINDOWS_AUMID);
    }

    builder.show().map(|_| ()).map_err(|err| err.to_string())
}

#[cfg(any(all(unix, not(target_os = "macos")), target_os = "windows"))]
fn notification_urgency_for_platform(urgency: DesktopNotificationUrgency) -> notify_rust::Urgency {
    #[cfg(target_os = "windows")]
    {
        match urgency {
            DesktopNotificationUrgency::Normal => notify_rust::Urgency::Normal,
            DesktopNotificationUrgency::Critical => notify_rust::Urgency::Critical,
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        match urgency {
            DesktopNotificationUrgency::Normal => notify_rust::Urgency::Normal,
            DesktopNotificationUrgency::Critical => notify_rust::Urgency::Critical,
        }
    }
}

#[cfg(target_os = "windows")]
fn ensure_windows_app_user_model_id_registered() -> Result<(), String> {
    let key_path = format!(r"SOFTWARE\Classes\AppUserModelId\{PANEFLOW_WINDOWS_AUMID}");
    let key = windows_registry::CURRENT_USER
        .create(&key_path)
        .map_err(|err| format!("create HKCU\\{key_path}: {err}"))?;
    key.set_string("DisplayName", "Paneflow")
        .map_err(|err| format!("set DisplayName: {err}"))?;
    key.set_string("IconBackgroundColor", "0")
        .map_err(|err| format!("set IconBackgroundColor: {err}"))?;
    let icon_path = ensure_windows_notification_icon()?;
    key.set_hstring("IconUri", &icon_path.as_path().into())
        .map_err(|err| format!("set IconUri: {err}"))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn ensure_windows_notification_icon() -> Result<std::path::PathBuf, String> {
    let data = crate::assets::Assets::get(PANEFLOW_WINDOWS_NOTIFICATION_ICON_ASSET)
        .ok_or_else(|| {
            format!(
                "embedded notification icon {PANEFLOW_WINDOWS_NOTIFICATION_ICON_ASSET} not found"
            )
        })?
        .data;
    let icon_dir = crate::runtime_paths::data_dir()
        .ok_or_else(|| "Paneflow data dir is unavailable for notification icon".to_string())?
        .join("icons");
    std::fs::create_dir_all(&icon_dir)
        .map_err(|err| format!("create notification icon dir {}: {err}", icon_dir.display()))?;

    let icon_path = icon_dir.join(PANEFLOW_WINDOWS_NOTIFICATION_ICON_FILE);
    let needs_write = match std::fs::read(&icon_path) {
        Ok(existing) => existing != data.as_ref(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
        Err(err) => {
            return Err(format!(
                "read notification icon {}: {err}",
                icon_path.display()
            ));
        }
    };
    if needs_write {
        std::fs::write(&icon_path, data.as_ref())
            .map_err(|err| format!("write notification icon {}: {err}", icon_path.display()))?;
    }
    Ok(icon_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_gate_honors_never_and_window_focus() {
        assert!(!should_fire_desktop_notification(
            NotifyWhenAgentWaiting::Never,
            false
        ));
        assert!(
            !should_fire_desktop_notification(NotifyWhenAgentWaiting::PrimaryScreen, true),
            "active Paneflow window suppresses OS notifications"
        );
        assert!(
            should_fire_desktop_notification(NotifyWhenAgentWaiting::PrimaryScreen, false),
            "inactive Paneflow window notifies"
        );
        assert!(should_fire_desktop_notification(
            NotifyWhenAgentWaiting::AllScreens,
            false
        ));
    }

    #[test]
    fn notification_message_is_bounded_and_bidi_stripped() {
        let spoofed = "Allow \u{202E}?fr- mr\u{202C} ?";
        let clean = sanitize_notification_message(spoofed);
        assert!(!clean.contains('\u{202E}'), "RLO stripped");
        assert!(!clean.contains('\u{202C}'), "PDF stripped");
        assert!(clean.contains("Allow"), "visible text kept: {clean}");

        let long = "é".repeat(600);
        assert_eq!(
            sanitize_notification_message(&long).chars().count(),
            512,
            "char-bounded, multibyte-safe"
        );
    }

    #[test]
    fn notification_bodies_are_specific_and_non_empty() {
        assert_eq!(
            attention_notification_body("backend", Some("Allow `cargo test`?")),
            "Allow `cargo test`?"
        );
        assert_eq!(attention_notification_body("backend", None), "backend");
        assert_eq!(
            attention_notification_body("backend", Some("   ")),
            "backend"
        );
        assert_eq!(
            agent_exit_notification_body("api", 1),
            "api: exited with code 1"
        );
        assert_eq!(
            stalled_notification_body("api", 300),
            "api: no activity for 300 s"
        );
        assert_eq!(
            notification_context_body("workspace", Some("Finished the release draft")),
            "Finished the release draft"
        );
    }

    #[test]
    fn desktop_notification_constructors_set_title_body_and_urgency() {
        let finished =
            DesktopNotification::turn_finished(TerminalAgent::Codex, "backend", Some("Tests pass"));
        assert_eq!(finished.summary, "Codex finished");
        assert_eq!(finished.body, "Tests pass");
        assert_eq!(finished.urgency, DesktopNotificationUrgency::Normal);

        let finished_without_summary =
            DesktopNotification::turn_finished(TerminalAgent::Codex, "backend", None);
        assert_eq!(finished_without_summary.summary, "Codex finished");
        assert_eq!(finished_without_summary.body, "backend");

        let attention = DesktopNotification::needs_input(
            TerminalAgent::ClaudeCode,
            "backend",
            Some("Approve edit?"),
        );
        assert_eq!(attention.summary, "Claude Code needs input");
        assert_eq!(attention.body, "Approve edit?");
        assert_eq!(attention.urgency, DesktopNotificationUrgency::Critical);
    }
}
