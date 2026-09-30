use gpui::{AppContext, Context, KeyDownEvent, ScrollHandle, Window};

use crate::{PaneFlowApp, SettingsSection, config_writer, keybindings};

impl PaneFlowApp {
    pub(crate) fn open_settings_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_at(SettingsSection::General, window, cx);
    }

    pub(crate) fn open_settings_at(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace_menu_open = None;
        self.settings_section = Some(section);
        self.reset_settings_scroll();
        self.terminal_dropdown = None;
        self.general_dropdown = None;
        self.workspace_template_dropdown = None;
        self.workspace_template_detail_open = false;
        self.agent_profile_editor = None;
        self.font_dropdown_open = false;
        self.font_search.clear();
        self.theme_dropdown_open = false;
        self.clear_settings_search(cx);
        if section == SettingsSection::Shortcuts {
            self.rebuild_shortcut_rows(cx);
        }
        self.refresh_mcp_status(cx);
        if section == SettingsSection::Agents {
            self.probe_agent_versions(cx);
            self.refresh_integration_status(cx);
        }
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn set_shortcut_capture(&mut self, active: bool, cx: &mut Context<Self>) {
        let changed = self.shortcut_capture_active != active;
        self.shortcut_capture_active = active;
        if active {
            self.shortcut_search_input.update(cx, |input, cx| {
                input.clear(cx);
            });
            self.cancel_shortcut_recording();
        } else if changed {
            self.rebuild_shortcut_rows(cx);
        }
    }

    pub(crate) fn clear_shortcut_filters(&mut self, cx: &mut Context<Self>) {
        self.shortcut_capture_active = false;
        self.shortcut_search_input.update(cx, |input, cx| {
            input.clear(cx);
        });
        self.rebuild_shortcut_rows(cx);
    }

    pub(crate) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_section = None;
        self.clear_shortcut_filters(cx);
        self.shortcut_reset_armed_at = None;
        self.font_dropdown_open = false;
        self.font_search.clear();
        self.theme_dropdown_open = false;
        self.terminal_dropdown = None;
        self.general_dropdown = None;
        self.workspace_template_dropdown = None;
        self.workspace_template_detail_open = false;
        self.agent_profile_editor = None;
        self.clear_settings_search(cx);
        if self.recording_shortcut.is_some() {
            self.cancel_shortcut_recording();
            keybindings::apply_keybindings(cx, &self.cached_config.shortcuts);
        }
    }

    pub(crate) fn cancel_shortcut_recording(&mut self) {
        self.recording_shortcut = None;
        self.shortcut_conflict = None;
    }

    pub(crate) fn start_shortcut_recording(
        &mut self,
        action_name: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.set_shortcut_capture(false, cx);
        self.shortcut_reset_armed_at = None;
        self.recording_shortcut = Some(action_name);
        self.shortcut_conflict = None;
    }

    pub(crate) fn arm_shortcut_reset(&mut self) {
        self.cancel_shortcut_recording();
        self.shortcut_reset_armed_at = Some(std::time::Instant::now());
    }

    pub(crate) fn confirm_shortcut_reset(&mut self, cx: &mut Context<Self>) {
        if !reset_confirmation_accepted(self.shortcut_reset_armed_at, std::time::Instant::now()) {
            return;
        }
        self.shortcut_reset_armed_at = None;
        self.persist_shortcut_change(config_writer::reset_shortcuts_checked, cx);
    }

    pub(crate) fn persist_shortcut_change(
        &mut self,
        write: impl FnOnce() -> bool + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.cancel_shortcut_recording();
        cx.spawn(async move |this, cx| {
            let saved =
                smol::unblock(move || write().then(paneflow_config::loader::load_config)).await;
            let _ = this.update(cx, |app, cx| {
                match saved {
                    Some(config) => app.apply_saved_shortcuts(config, cx),
                    None => app.show_toast("Could not save shortcut", cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply_saved_shortcuts(
        &mut self,
        config: paneflow_config::schema::PaneFlowConfig,
        cx: &mut Context<Self>,
    ) {
        keybindings::apply_keybindings(cx, &config.shortcuts);
        self.effective_shortcuts = keybindings::effective_shortcuts(&config.shortcuts);
        self.cached_config.shortcuts = config.shortcuts;
        crate::config_snapshot::publish(&self.cached_config, cx);
        self.rebuild_shortcut_rows(cx);
    }

    pub(crate) fn reset_settings_scroll(&mut self) {
        self.settings_scroll = ScrollHandle::new();
        self.settings_drag = None;
    }

    fn clear_settings_search(&mut self, cx: &mut Context<Self>) {
        self.settings_search_input.update(cx, |inp, cx| {
            inp.clear(cx);
        });
    }
}

pub(crate) struct PendingSetting {
    sequence: u64,
    nested: bool,
    key: &'static str,
    value: serde_json::Value,
}

fn pending_setting_slot(nested: bool, key: &str) -> String {
    if nested {
        format!("terminal.{key}")
    } else {
        key.to_string()
    }
}

pub(crate) fn overlay_pending_settings<'a>(
    config: paneflow_config::schema::PaneFlowConfig,
    pending: impl IntoIterator<Item = &'a PendingSetting>,
) -> paneflow_config::schema::PaneFlowConfig {
    let mut pending: Vec<&PendingSetting> = pending.into_iter().collect();
    pending.sort_by_key(|setting| setting.sequence);
    pending.into_iter().fold(config, |config, setting| {
        config_writer::with_field(&config, setting.nested, setting.key, setting.value.clone())
            .unwrap_or(config)
    })
}

const RESET_CONFIRM_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

pub(crate) fn reset_confirmation_accepted(
    armed_at: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    armed_at.is_some_and(|armed| now.saturating_duration_since(armed) >= RESET_CONFIRM_DELAY)
}

impl PaneFlowApp {
    pub(crate) fn persist_setting(
        &mut self,
        nested: bool,
        key: &'static str,
        value: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let default_shell_changed = !nested
            && key == "default_shell"
            && normalized_shell_setting(self.cached_config.default_shell.as_deref())
                != normalized_shell_setting(value.as_str());
        let next = match config_writer::with_field(&self.cached_config, nested, key, value.clone())
        {
            Ok(next) => next,
            Err(_) => {
                self.show_toast(format!("Could not save setting: {key}"), cx);
                return;
            }
        };
        self.cached_config = next;
        crate::config_snapshot::publish(&self.cached_config, cx);
        if crate::terminal::element::apply_font_config(&self.cached_config) {
            for ws in &self.workspaces {
                ws.propagate_config(&self.cached_config, cx);
            }
        }
        if !nested
            && matches!(
                key,
                "windows_terminal_material"
                    | "windows_chrome_material"
                    | "macos_chrome_material"
                    | "linux_terminal_material"
                    | "linux_chrome_material"
            )
        {
            for ws in &self.workspaces {
                ws.propagate_config(&self.cached_config, cx);
            }
        }
        if nested && terminal_key_repaints_open_terminals(key) {
            for ws in &self.workspaces {
                ws.propagate_config(&self.cached_config, cx);
            }
        }
        if !nested && key == "reduce_motion" {
            crate::ui_primitives::set_reduce_motion(self.cached_config.reduce_motion_enabled());
        }
        if !nested && key == "editor" {
            self.apply_editor_display(cx);
        }
        if !nested && key == "worktrees" {
            crate::workspace::worktree::set_worktrees_root(self.cached_config.worktrees.dir_path());
        }
        if default_shell_changed {
            self.handle_default_shell_changed(cx);
        }
        cx.notify();
        let sequence = config_writer::next_setting_sequence();
        let slot = pending_setting_slot(nested, key);
        self.pending_settings.insert(
            slot.clone(),
            PendingSetting {
                sequence,
                nested,
                key,
                value: value.clone(),
            },
        );
        cx.spawn(async move |this, cx| {
            let outcome = smol::unblock(move || {
                config_writer::save_setting_ordered(nested, key, value, sequence)
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .pending_settings
                    .get(&slot)
                    .is_some_and(|pending| pending.sequence == sequence)
                {
                    this.pending_settings.remove(&slot);
                }
                if outcome == config_writer::SettingWrite::Failed {
                    log::warn!(
                        "settings: failed to persist {key}; choice is in-memory only this session"
                    );
                    this.show_toast(format!("Could not save setting: {key}"), cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn handle_default_shell_changed(&mut self, cx: &mut Context<Self>) {
        self.show_toast("Shell updated. New terminals will use it.", cx);
    }

    pub(crate) fn persist_agent_panel_setting(
        &mut self,
        key: &'static str,
        value: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        match config_writer::with_agent_panel_field(&self.cached_config, key, value.clone()) {
            Ok(next) => {
                self.cached_config = next;
                crate::config_snapshot::publish(&self.cached_config, cx);
            }
            Err(_) => {
                self.show_toast(format!("Could not save setting: agent_panel.{key}"), cx);
                return;
            }
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let ok =
                smol::unblock(move || config_writer::save_agent_panel_field_checked(key, value))
                    .await;
            if !ok {
                log::warn!(
                    "settings: failed to persist agent_panel.{key}; choice is in-memory only this session"
                );
                let _ = this.update(cx, |this, cx| {
                    this.show_toast(format!("Could not save agent panel setting: {key}"), cx);
                });
            }
        })
        .detach();
    }

    pub(crate) fn handle_settings_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.font_dropdown_open {
            let key = event.keystroke.key.as_str();
            match key {
                "escape" => {
                    self.font_dropdown_open = false;
                    self.font_search.clear();
                    cx.notify();
                }
                "backspace" => {
                    self.font_search.pop();
                    cx.notify();
                }
                _ => {
                    if let Some(ch) = &event.keystroke.key_char
                        && !ch.is_empty()
                        && !event.keystroke.modifiers.control
                        && !event.keystroke.modifiers.platform
                    {
                        self.font_search.push_str(ch);
                        cx.notify();
                    }
                }
            }
            return;
        }

        if event.keystroke.key == "escape" && self.recording_shortcut.is_none() {
            if self.terminal_dropdown.is_some() {
                self.terminal_dropdown = None;
            } else if self.general_dropdown.is_some() {
                self.general_dropdown = None;
            } else if self.workspace_template_dropdown.is_some() {
                self.workspace_template_dropdown = None;
            } else if self.agent_profile_editor.is_some() {
                self.close_agent_profile_editor(cx);
            } else {
                self.close_settings(cx);
            }
            cx.notify();
        }
    }

    pub(crate) fn intercept_shortcut_keystroke(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.settings_section != Some(SettingsSection::Shortcuts) {
            return false;
        }
        if keybindings::is_bare_modifier(keystroke) {
            return false;
        }

        if self.recording_shortcut.is_some() {
            if self
                .shortcut_search_input
                .read(cx)
                .focus_handle
                .is_focused(window)
            {
                self.cancel_shortcut_recording();
                cx.notify();
                return false;
            }
            self.handle_shortcut_recording(keystroke, window, cx);
            cx.notify();
            return true;
        }

        if !self.shortcut_capture_active {
            return false;
        }

        if keystroke.key == "escape" {
            self.set_shortcut_capture(false, cx);
            cx.notify();
            return true;
        }

        let formatted = keybindings::format_keystroke(&keystroke.unparse());
        self.shortcut_search_input.update(cx, |input, cx| {
            input.set_value(formatted, cx);
        });
        cx.notify();
        true
    }

    pub(crate) fn handle_shortcut_recording(
        &mut self,
        keystroke: &gpui::Keystroke,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(action_name) = self.recording_shortcut else {
            return;
        };

        if keybindings::is_bare_modifier(keystroke) {
            return;
        }

        if keystroke.key == "escape" {
            self.cancel_shortcut_recording();
            cx.notify();
            return;
        }

        if !self
            .effective_shortcuts
            .iter()
            .any(|entry| entry.action_name == action_name)
        {
            self.cancel_shortcut_recording();
            cx.notify();
            return;
        }

        let unassigns = matches!(keystroke.key.as_str(), "backspace" | "delete")
            && !keystroke.modifiers.modified();
        if unassigns {
            self.persist_shortcut_change(move || config_writer::unassign_shortcut(action_name), cx);
            return;
        }

        let new_key = keystroke.unparse();
        let confirmed = self
            .shortcut_conflict
            .as_ref()
            .is_some_and(|conflict| keybindings::keystrokes_conflict(&conflict.key, &new_key));
        if !confirmed {
            let owner = self
                .effective_shortcuts
                .iter()
                .filter(|entry| entry.action_name != action_name)
                .find(|entry| {
                    entry
                        .raw_key
                        .as_deref()
                        .is_some_and(|raw| keybindings::keystrokes_conflict(raw, &new_key))
                });
            if let Some(owner) = owner {
                self.shortcut_conflict = Some(crate::settings::tabs::shortcuts::ShortcutConflict {
                    label: keybindings::format_keystroke(&new_key),
                    key: new_key,
                    owner: owner.description.clone(),
                });
                cx.notify();
                return;
            }
        }

        self.persist_shortcut_change(
            move || config_writer::save_shortcut_checked(&new_key, action_name),
            cx,
        );
    }

    pub(crate) fn process_config_changes(&mut self, cx: &mut Context<Self>) {
        let new_config = self
            .pending_config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(config) = new_config {
            let config = overlay_pending_settings(config, self.pending_settings.values());
            if self.recording_shortcut.is_some() {
                self.cancel_shortcut_recording();
            }
            crate::terminal::element::apply_font_config(&config);
            let default_shell_changed =
                super::settings::normalized_shell_setting(
                    self.cached_config.default_shell.as_deref(),
                ) != super::settings::normalized_shell_setting(config.default_shell.as_deref());
            let theme_mode = crate::ThemeMode::from_config(
                config.theme_mode.as_deref(),
                config.theme.as_deref(),
            );
            keybindings::apply_keybindings(cx, &config.shortcuts);
            self.effective_shortcuts = keybindings::effective_shortcuts(&config.shortcuts);
            crate::theme::set_active_theme(config.theme.as_deref());
            self.reconcile_telemetry_consent(&config, cx);
            crate::workspace::worktree::set_worktrees_root(config.worktrees.dir_path());
            self.cached_config = config;
            crate::config_snapshot::publish(&self.cached_config, cx);
            self.theme_mode = theme_mode;
            crate::ui_primitives::set_reduce_motion(self.cached_config.reduce_motion_enabled());
            self.apply_editor_display(cx);
            if default_shell_changed {
                self.handle_default_shell_changed(cx);
            }
            for ws in &self.workspaces {
                ws.propagate_config(&self.cached_config, cx);
            }
            cx.notify();
        }

        if self
            .theme_changed
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            cx.notify();
        }

        crate::theme::publish_theme_generation(cx);
    }

    fn reconcile_telemetry_consent(
        &mut self,
        config: &paneflow_config::schema::PaneFlowConfig,
        cx: &mut Context<Self>,
    ) {
        let new_enabled = config.telemetry.as_ref().and_then(|t| t.enabled);
        let decision = reconcile_telemetry(self.telemetry_enabled_last, new_enabled);
        if !decision.rebuild {
            return;
        }

        let consent = crate::telemetry::client::TelemetryConsent::from_config(new_enabled);
        let (api_key, host) = super::telemetry_events::posthog_endpoint();
        let deactivating_telemetry = std::sync::Arc::clone(&self.telemetry);
        deactivating_telemetry.disable();
        cx.background_spawn(async move {
            smol::unblock(move || deactivating_telemetry.deactivate()).await;
        })
        .detach();
        let (telemetry_client, _) =
            crate::telemetry::client::TelemetryClient::from_consent(consent, api_key, host, || {
                (crate::telemetry::id::telemetry_id(), false)
            });
        let telemetry = std::sync::Arc::new(telemetry_client);
        self.telemetry = std::sync::Arc::clone(&telemetry);
        Self::spawn_telemetry_flusher(telemetry, cx);

        if decision.reenabled {
            self.telemetry
                .capture(crate::telemetry::event::TelemetryEvent::telemetry_reenabled());
        }

        self.telemetry_enabled_last = new_enabled;

        if let Some(msg) = decision.toast_msg {
            self.show_toast(msg, cx);
        }
    }
}

pub(crate) fn normalized_shell_setting(shell: Option<&str>) -> &str {
    shell.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("")
}

fn terminal_key_repaints_open_terminals(key: &str) -> bool {
    matches!(
        key,
        "integrated_glyphs" | "color_emoji" | "cursor_color" | "minimum_contrast"
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TelemetryReconciliation {
    pub rebuild: bool,
    pub reenabled: bool,
    pub toast_msg: Option<&'static str>,
}

pub(crate) fn reconcile_telemetry(old: Option<bool>, new: Option<bool>) -> TelemetryReconciliation {
    if old == new {
        return TelemetryReconciliation {
            rebuild: false,
            reenabled: false,
            toast_msg: None,
        };
    }
    let toast_msg = Some(match new {
        Some(true) => "Télémétrie activée",
        Some(false) => "Télémétrie désactivée",
        None => "Télémétrie : la demande réapparaîtra au prochain lancement",
    });
    TelemetryReconciliation {
        rebuild: true,
        reenabled: old == Some(false) && new == Some(true),
        toast_msg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hot_reloading_terminal_key_reaches_the_open_terminals() {
        for key in [
            "integrated_glyphs",
            "color_emoji",
            "cursor_color",
            "minimum_contrast",
        ] {
            assert!(terminal_key_repaints_open_terminals(key), "{key}");
        }
        assert!(!terminal_key_repaints_open_terminals("cursor_shape"));
        assert!(!terminal_key_repaints_open_terminals("scrollback_lines"));
    }

    #[test]
    fn the_settings_ladder_rewrites_the_resolved_threshold_through_the_config_writer() {
        let config = paneflow_config::schema::PaneFlowConfig::default();
        let off = config_writer::with_field(
            &config,
            true,
            "minimum_contrast",
            crate::settings::tabs::terminal::minimum_contrast_setting(1),
        )
        .expect("a valid contrast step");
        let terminal = off.terminal.clone().unwrap_or_default();
        assert_eq!(terminal.resolved_minimum_contrast(), 0.0);

        let auto = config_writer::with_field(
            &off,
            true,
            "minimum_contrast",
            crate::settings::tabs::terminal::minimum_contrast_setting(0),
        )
        .expect("auto is a valid step");
        assert_eq!(
            auto.terminal
                .clone()
                .unwrap_or_default()
                .resolved_minimum_contrast(),
            paneflow_config::schema::TerminalConfig::DEFAULT_MINIMUM_CONTRAST
        );
    }

    #[test]
    fn a_reload_from_an_older_write_keeps_the_newer_value_in_memory() {
        let on_disk = config_writer::with_field(
            &paneflow_config::schema::PaneFlowConfig::default(),
            false,
            "font_size",
            serde_json::json!(14.0),
        )
        .expect("a font size");
        let pending = [
            PendingSetting {
                sequence: 8,
                nested: false,
                key: "font_size",
                value: serde_json::json!(16.0),
            },
            PendingSetting {
                sequence: 7,
                nested: false,
                key: "font_size",
                value: serde_json::json!(15.0),
            },
        ];

        let merged = overlay_pending_settings(on_disk, pending.iter());

        assert_eq!(
            merged.font_size,
            Some(16.0),
            "the second stepper click still shows after the first write reloads"
        );
    }

    #[test]
    fn a_reset_confirmation_needs_a_deliberate_pause() {
        let armed = std::time::Instant::now();
        let at = |ms: u64| armed + std::time::Duration::from_millis(ms);

        assert!(!reset_confirmation_accepted(None, at(1000)));
        assert!(!reset_confirmation_accepted(Some(armed), at(0)));
        assert!(!reset_confirmation_accepted(Some(armed), at(399)));
        assert!(reset_confirmation_accepted(Some(armed), at(400)));
    }

    #[test]
    fn identical_state_is_a_noop() {
        for state in [None, Some(false), Some(true)] {
            let r = reconcile_telemetry(state, state);
            assert!(!r.rebuild, "no rebuild for identical {state:?}");
            assert!(!r.reenabled);
            assert!(r.toast_msg.is_none());
        }
    }

    #[test]
    fn none_to_some_true_rebuilds_but_does_not_flag_reenabled() {
        let r = reconcile_telemetry(None, Some(true));
        assert!(r.rebuild);
        assert!(
            !r.reenabled,
            "first-ever consent (None → true) is not a re-enable"
        );
        assert_eq!(r.toast_msg, Some("Télémétrie activée"));
    }

    #[test]
    fn none_to_some_false_rebuilds() {
        let r = reconcile_telemetry(None, Some(false));
        assert!(r.rebuild);
        assert!(!r.reenabled);
        assert_eq!(r.toast_msg, Some("Télémétrie désactivée"));
    }

    #[test]
    fn some_false_to_some_true_flags_reenabled() {
        let r = reconcile_telemetry(Some(false), Some(true));
        assert!(r.rebuild);
        assert!(
            r.reenabled,
            "opted-out → opted-in is the only transition that emits telemetry_reenabled"
        );
        assert_eq!(r.toast_msg, Some("Télémétrie activée"));
    }

    #[test]
    fn some_true_to_some_false_rebuilds_no_reenabled() {
        let r = reconcile_telemetry(Some(true), Some(false));
        assert!(r.rebuild);
        assert!(!r.reenabled);
        assert_eq!(r.toast_msg, Some("Télémétrie désactivée"));
    }

    #[test]
    fn some_true_to_none_rebuilds() {
        let r = reconcile_telemetry(Some(true), None);
        assert!(r.rebuild);
        assert!(!r.reenabled);
        assert_eq!(
            r.toast_msg,
            Some("Télémétrie : la demande réapparaîtra au prochain lancement")
        );
    }

    #[test]
    fn some_false_to_none_rebuilds_no_reenabled() {
        let r = reconcile_telemetry(Some(false), None);
        assert!(r.rebuild);
        assert!(!r.reenabled);
        assert_eq!(
            r.toast_msg,
            Some("Télémétrie : la demande réapparaîtra au prochain lancement")
        );
    }
}
