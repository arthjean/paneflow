use std::borrow::Cow;
use std::collections::VecDeque;
use std::io;
use std::sync::Arc;

use futures::channel::mpsc::UnboundedReceiver;

use super::clipboard_gate::ClipboardGate;
#[cfg(test)]
use super::ghostty_session::SpawnedGhostty;
use super::ghostty_session::{
    GhosttyInputSendResult, GhosttyRuntimePending, GhosttySession, GhosttyUiEvent,
    ProgramNotification,
};
use super::host_link::{HostLinkState, HostedAttachment, HostedSession};
use super::marks::SharedMarkRing;
use super::service_detector::{ServiceInfo, detect_framework, parse_service_line};
use super::shell::{resolve_shell_for_spawn, setup_shell_integration};
use super::types::{
    Content, GridLineText, GridMetrics, HyperlinkZone, Line, Modes, Point, SelectionGeometry,
    SelectionKind, SelectionRange, ShellQuoting, TerminalWindowSize,
};
use crate::limits::MAX_OSC52_BYTES;
#[cfg(test)]
use paneflow_config::schema::TerminalConfig;
use paneflow_config::schema::{SessionGeneration, SessionId, TerminalSurfaceProfile};
pub(crate) use paneflow_host::env::INHERITED_AGENT_SESSION_ENV;
#[cfg(test)]
pub(super) use paneflow_host::env::inherited_env_keys_to_strip;
use paneflow_host::env::{
    is_forbidden_child_env_key, is_inherited_agent_session_env_key, is_valid_env_name,
};
#[cfg(all(test, windows))]
pub(super) use paneflow_host::process::{
    WINDOWS_PROCESS_TREE_TERMINATION_BUDGET, terminate_windows_process_tree,
};
use paneflow_terminal_ghostty::Scroll as GhosttyScroll;

mod diagnostics;
mod session_backend;
mod spawn_env;

pub use diagnostics::*;
pub(crate) use session_backend::*;
pub(super) use spawn_env::*;

#[cfg(test)]
const DEFAULT_SCROLLBACK_LINES: usize = TerminalConfig::DEFAULT_SCROLLBACK_LINES;
const MAX_PENDING_CLIPBOARD_OPS: usize = 8;
const MAX_PENDING_NOTIFICATIONS: usize = 8;

#[cfg(test)]
fn resolved_scrollback_lines(profile: TerminalSurfaceProfile) -> usize {
    paneflow_config::loader::load_config()
        .terminal
        .unwrap_or(TerminalConfig {
            scrollback_lines: Some(DEFAULT_SCROLLBACK_LINES),
            ..Default::default()
        })
        .resolved_scrollback_lines_for_profile(profile)
}

#[derive(Clone)]
enum PendingTerminalInput {
    Raw(Cow<'static, [u8]>),
    Key(paneflow_terminal_ghostty::KeyInput),
    Mouse {
        input: paneflow_terminal_ghostty::MouseInput,
        repeat: usize,
    },
    Focus(paneflow_terminal_ghostty::FocusEvent),
    Paste {
        text: String,
        allow_unsafe: bool,
        location: paneflow_terminal_ghostty::ClipboardLocation,
    },
}

impl PendingTerminalInput {
    fn queued_bytes(&self) -> usize {
        match self {
            Self::Raw(bytes) => bytes.len(),
            Self::Key(input) => std::mem::size_of::<paneflow_terminal_ghostty::KeyInput>()
                .saturating_add(input.text.len()),
            Self::Mouse { repeat, .. } => {
                std::mem::size_of::<paneflow_terminal_ghostty::MouseInput>().saturating_add(*repeat)
            }
            Self::Focus(_) => std::mem::size_of::<paneflow_terminal_ghostty::FocusEvent>(),
            Self::Paste { text, .. } => text.len(),
        }
    }

    fn queue_limit(&self) -> usize {
        match self {
            Self::Raw(_) | Self::Paste { .. } => {
                MAX_PENDING_INPUT_BYTES - INPUT_CONTROL_RESERVE_BYTES
            }
            Self::Key(input) if input.action == paneflow_terminal_ghostty::KeyAction::Release => {
                MAX_PENDING_INPUT_BYTES
            }
            Self::Mouse { input, .. }
                if input.action == paneflow_terminal_ghostty::MouseAction::Release =>
            {
                MAX_PENDING_INPUT_BYTES
            }
            Self::Focus(_) => MAX_PENDING_INPUT_BYTES,
            Self::Key(_) | Self::Mouse { .. } => {
                MAX_PENDING_INPUT_BYTES - INPUT_CONTROL_RESERVE_BYTES
            }
        }
    }

    fn fits_after(&self, queued_bytes: usize) -> bool {
        queued_bytes.saturating_add(self.queued_bytes()) <= self.queue_limit()
    }

    fn try_send(&self, ghostty: &GhosttySession) -> GhosttyInputSendResult {
        match self {
            Self::Raw(bytes) => ghostty.write(bytes.clone().into_owned()),
            Self::Key(input) => ghostty.write_key(input.clone()),
            Self::Mouse { input, repeat } => ghostty.write_mouse(*input, *repeat),
            Self::Focus(event) => ghostty.write_focus(*event),
            Self::Paste {
                text,
                allow_unsafe,
                location,
            } => ghostty.write_paste(text.clone(), *allow_unsafe, *location),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackendInputResult {
    Accepted,
    Rejected,
}

pub struct TerminalState {
    pub session_id: SessionId,
    pub(crate) hosted: Option<HostedSession>,
    pub(crate) host_link: HostLinkState,
    ghostty: GhosttySession,
    ghostty_events_rx: Option<UnboundedReceiver<GhosttyUiEvent>>,
    backend_failure: Option<TerminalBackendFailureDiagnostics>,
    pub(crate) marks: SharedMarkRing,
    last_prompt_seq: u64,
    pub exited: Option<i32>,
    resolved_hover_link: Option<(Point, Option<HyperlinkZone>)>,
    keyboard_input_sent: std::sync::atomic::AtomicBool,
    pub exit_signal: Option<String>,
    pub child_pid: u32,
    pub title: String,
    pub current_cwd: Option<String>,
    pub progress: Option<paneflow_terminal_ghostty::ProgressReport>,
    pub custom_name: Option<String>,
    pub detected_agent: Option<crate::agent_launcher::TerminalAgent>,
    pub agent_confirmed: bool,
    pub agent_declared_until: Option<std::time::Instant>,
    pub detected_ports: Vec<(u16, Option<String>)>,
    pub port_conflicts: Vec<(u16, String)>,
    pub announced_ports: Vec<u16>,
    pub font_size_override: Option<f32>,
    pub osc52_mode: Osc52Mode,
    terminal_focused: bool,
    clipboard_gate: Arc<ClipboardGate>,
    pub(super) shell_quoting: ShellQuoting,
    pub(super) pending_clipboard_ops: Vec<String>,
    pub(super) pending_notifications: Vec<ProgramNotification>,
    pub(super) pending_host_notices: Vec<String>,
    pub cached_foreground_command: Option<String>,
    pub cursor_blinking: bool,
    pub dirty: bool,
    pub output_generation: u64,
    pub(super) last_activity_burst: Option<std::time::Instant>,
    cwd_poll_ticks: u32,
    reported_ports: std::collections::HashSet<u16>,
    #[cfg(debug_assertions)]
    pub(crate) last_keystroke_at: Option<std::time::Instant>,
    pending_input: std::sync::Mutex<VecDeque<PendingTerminalInput>>,
    pub(super) osc52_policy: paneflow_config::schema::Osc52ClipboardConfig,
}

const MAX_PENDING_INPUT_BYTES: usize = 1024 * 1024;
const INPUT_CONTROL_RESERVE_BYTES: usize = 64 * 1024;

const SPAWN_FAILURE_SCROLLBACK_LINES: usize = 256;

impl TerminalState {
    pub(crate) fn session_backend(&self) -> TerminalSessionBackend {
        TerminalSessionBackend::new(self.ghostty.clone())
    }

    pub(super) fn ghostty_session(&self) -> GhosttySession {
        self.ghostty.clone()
    }

    pub(crate) fn take_backend_events(&mut self) -> TerminalBackendEvents {
        TerminalBackendEvents(self.ghostty_events_rx.take())
    }

    pub(crate) fn has_backend_events(&self) -> bool {
        self.ghostty_events_rx.is_some()
    }

    pub(crate) fn process_backend_event(&mut self, event: TerminalBackendEvent) {
        self.process_ghostty_event(event.0);
    }

    pub(crate) fn process_backend_wakeup(&mut self) {
        self.dirty = true;
        self.output_generation = self.output_generation.saturating_add(1);
        self.flush_ghostty_pending_input();
        self.ghostty.retry_backpressured_commands();
    }

    pub(crate) fn notify_window_size(&self, size: TerminalWindowSize) {
        self.ghostty.resize(size);
    }

    #[cfg(test)]
    pub(super) fn promote_ghostty(&mut self, spawned: SpawnedGhostty) {
        self.ghostty.promote();
        self.child_pid = spawned.child_pid;
        self.current_cwd = Some(spawned.cwd.to_string_lossy().into_owned());
        self.host_link = HostLinkState::Attached;
        self.set_osc52_mode(self.promoted_osc52_mode());
        self.cursor_blinking = true;
        self.dirty = true;
        self.flush_ghostty_pending_input();
    }

    fn promoted_osc52_mode(&self) -> Osc52Mode {
        match self.osc52_policy {
            paneflow_config::schema::Osc52ClipboardConfig::Copy => Osc52Mode::CopyOnly,
            paneflow_config::schema::Osc52ClipboardConfig::Off => Osc52Mode::Disabled,
        }
    }

    #[cfg(test)]
    pub(super) fn queued_pastes(&self) -> Vec<(String, bool)> {
        self.pending_input
            .lock()
            .map(|pending| {
                pending
                    .iter()
                    .filter_map(|input| match input {
                        PendingTerminalInput::Paste {
                            text, allow_unsafe, ..
                        } => Some((text.clone(), *allow_unsafe)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn promote_hosted(&mut self, hosted: HostedAttachment) {
        self.ghostty.promote();
        self.child_pid = hosted.pid;
        self.current_cwd = Some(hosted.cwd);
        self.hosted = Some(HostedSession {
            endpoint: hosted.attachment.endpoint,
            generation: hosted.attachment.generation,
        });
        self.host_link = HostLinkState::Attached;
        self.set_osc52_mode(self.promoted_osc52_mode());
        self.cursor_blinking = true;
        self.dirty = true;
        self.flush_ghostty_pending_input();
    }

    pub(super) fn mark_host_link(&mut self, state: HostLinkState) {
        if !state.accepts_input()
            && let Ok(mut pending) = self.pending_input.lock()
        {
            pending.clear();
        }
        if matches!(
            state,
            HostLinkState::Ended(_) | HostLinkState::Unavailable(_)
        ) {
            self.cursor_blinking = false;
            self.progress = None;
        }
        self.host_link = state;
        self.dirty = true;
    }

    pub(crate) fn hosted_stop_target(
        &self,
    ) -> Option<(std::path::PathBuf, SessionId, SessionGeneration)> {
        let hosted = self.hosted.as_ref()?;
        Some((
            hosted.endpoint.clone(),
            self.session_id.clone(),
            hosted.generation,
        ))
    }

    fn flush_ghostty_pending_input(&self) {
        if !self.ghostty.is_promoted() {
            return;
        }
        let Ok(mut pending) = self.pending_input.lock() else {
            return;
        };
        while let Some(input) = pending.front().cloned() {
            match input.try_send(&self.ghostty) {
                GhosttyInputSendResult::Sent => {
                    pending.pop_front();
                }
                GhosttyInputSendResult::Full => break,
                GhosttyInputSendResult::Closed => {
                    let discarded = pending.len();
                    pending.clear();
                    log::warn!(
                        target: "paneflow::terminal::ghostty",
                        "Ghostty input closed with {discarded} deferred events"
                    );
                    break;
                }
            }
        }
    }

    pub(super) fn report_spawn_failure(
        &mut self,
        failure: TerminalBackendFailureDiagnostics,
        message: &str,
    ) {
        self.backend_failure = Some(failure);
        self.ghostty.shutdown();

        let size = self.ghostty.requested_window_size();
        let (session, pending, events_rx) =
            GhosttySession::pending_with_clipboard_gate(size, self.clipboard_gate.clone());
        if let Err(error) = session.start_display(pending, SPAWN_FAILURE_SCROLLBACK_LINES) {
            log::error!(
                target: "paneflow::terminal::ghostty",
                "could not open the spawn-failure pane: {error}"
            );
            return;
        }

        self.marks = session.marks();
        self.ghostty = session;
        self.ghostty_events_rx = Some(events_rx);
        self.write_output(message.as_bytes());
        self.dirty = true;
    }

    pub fn backend_diagnostics(&self) -> TerminalBackendDiagnostics {
        let identity = paneflow_terminal_ghostty::build_identity();
        TerminalBackendDiagnostics {
            failure: self.backend_failure.clone(),
            target_triple: env!("PANEFLOW_TARGET_TRIPLE"),
            ghostty: GhosttyBuildDiagnostics {
                version: paneflow_terminal_ghostty::GHOSTTY_APP_VERSION,
                source_sha: identity.source_sha,
                api_version: identity.api_version,
                zig_version: identity.zig_version,
                optimization: identity.optimization,
                simd: identity.simd,
            },
        }
    }

    #[cfg(test)]
    pub fn new(
        working_directory: Option<std::path::PathBuf>,
        workspace_id: u64,
        surface_id: u64,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
    ) -> anyhow::Result<Self> {
        Self::new_with_profile(
            working_directory,
            workspace_id,
            surface_id,
            initial_size,
            user_env,
            TerminalSurfaceProfile::Normal,
        )
    }

    #[cfg(test)]
    pub fn new_with_profile(
        working_directory: Option<std::path::PathBuf>,
        workspace_id: u64,
        surface_id: u64,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
        profile: TerminalSurfaceProfile,
    ) -> anyhow::Result<Self> {
        let params = Self::resolve_spawn_params_with_profile(
            working_directory,
            workspace_id,
            surface_id,
            initial_size,
            user_env,
            profile,
        );
        let max_scrollback = resolved_scrollback_lines(params.profile);
        let (mut state, pending) =
            Self::new_pending_with_shell_quoting(params.cols, params.rows, params.shell_quoting);
        let spawned = state
            .ghostty_session()
            .start(pending.ghostty, params, max_scrollback)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        state.promote_ghostty(spawned);
        Ok(state)
    }

    #[cfg(test)]
    pub(super) fn new_pending(cols: usize, rows: usize) -> (Self, PendingTerminalBackend) {
        Self::new_pending_with_shell_quoting(cols, rows, ShellQuoting::default_for_platform())
    }

    pub(super) fn new_pending_with_shell_quoting(
        cols: usize,
        rows: usize,
        shell_quoting: ShellQuoting,
    ) -> (Self, PendingTerminalBackend) {
        Self::build_display_only(cols, rows, shell_quoting)
    }

    #[cfg(test)]
    pub fn new_display_only(rows: usize, cols: usize) -> Self {
        Self::new_display_only_with_profile(rows, cols, TerminalSurfaceProfile::Normal)
    }

    #[cfg(test)]
    pub fn new_display_only_with_profile(
        rows: usize,
        cols: usize,
        profile: TerminalSurfaceProfile,
    ) -> Self {
        let (state, pending) =
            Self::build_display_only(cols, rows, ShellQuoting::default_for_platform());
        if let Err(error) = state
            .ghostty
            .start_display(pending.ghostty, resolved_scrollback_lines(profile))
        {
            log::error!(
                target: "paneflow::terminal::ghostty",
                "could not start the display-only runtime: {error}"
            );
        }
        state
    }

    fn build_display_only(
        cols: usize,
        rows: usize,
        shell_quoting: ShellQuoting,
    ) -> (Self, PendingTerminalBackend) {
        let clipboard_gate = Arc::new(ClipboardGate::default());
        let (ghostty, runtime_pending, events_rx) = GhosttySession::pending_with_clipboard_gate(
            TerminalWindowSize::new(cols, rows, 0, 0),
            clipboard_gate.clone(),
        );
        let marks = ghostty.marks();
        let state = Self {
            session_id: SessionId::new(),
            hosted: None,
            host_link: HostLinkState::Attaching,
            ghostty,
            ghostty_events_rx: Some(events_rx),
            backend_failure: None,
            marks,
            last_prompt_seq: 0,
            exited: None,
            keyboard_input_sent: std::sync::atomic::AtomicBool::new(false),
            exit_signal: None,
            resolved_hover_link: None,
            child_pid: 0,
            current_cwd: None,
            progress: None,
            custom_name: None,
            detected_agent: None,
            agent_confirmed: false,
            agent_declared_until: None,
            detected_ports: Vec::new(),
            port_conflicts: Vec::new(),
            announced_ports: Vec::new(),
            font_size_override: None,
            osc52_mode: Osc52Mode::Disabled,
            terminal_focused: false,
            clipboard_gate,
            shell_quoting,
            pending_clipboard_ops: Vec::new(),
            pending_notifications: Vec::new(),
            pending_host_notices: Vec::new(),
            cached_foreground_command: None,
            cursor_blinking: false,
            title: String::from("Terminal"),
            dirty: true,
            output_generation: 0,
            last_activity_burst: None,
            cwd_poll_ticks: 0,
            reported_ports: std::collections::HashSet::new(),
            #[cfg(debug_assertions)]
            last_keystroke_at: None,
            pending_input: std::sync::Mutex::new(VecDeque::new()),
            osc52_policy: paneflow_config::schema::Osc52ClipboardConfig::Copy,
        };
        (
            state,
            PendingTerminalBackend {
                ghostty: runtime_pending,
            },
        )
    }

    pub fn write_output(&self, bytes: &[u8]) {
        let mut converted = Vec::with_capacity(bytes.len());
        let mut prev = 0u8;
        for &b in bytes {
            if b == b'\n' && prev != b'\r' {
                converted.push(b'\r');
            }
            converted.push(b);
            prev = b;
        }
        self.ghostty.write_output(&converted);
    }

    #[cfg(test)]
    pub fn sync(&mut self) {
        self.sync_channels();
        if let Some(mut rx) = self.ghostty_events_rx.take() {
            while let Ok(event) = rx.try_recv() {
                self.process_ghostty_event(event);
            }
            self.ghostty_events_rx = Some(rx);
        }
    }

    pub fn sync_channels(&mut self) {
        self.cwd_poll_ticks = self.cwd_poll_ticks.wrapping_add(1);
        if self.cwd_poll_ticks.is_multiple_of(25)
            && let Some(cwd) = self.cwd_now()
        {
            self.current_cwd = Some(cwd.to_string_lossy().into_owned());
        }
    }

    pub(crate) fn take_resolved_hover_link(&mut self) -> Option<(Point, Option<HyperlinkZone>)> {
        self.resolved_hover_link.take()
    }

    #[cfg(test)]
    pub(super) fn processed_output_bytes_for_test(&self) -> usize {
        self.ghostty.processed_output_bytes_for_test()
    }

    pub(crate) fn take_shell_prompt_ready(&mut self) -> bool {
        let seq = self
            .marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .prompt_start_seq();
        let fired = seq != self.last_prompt_seq;
        self.last_prompt_seq = seq;
        fired
    }

    fn process_ghostty_event(&mut self, event: GhosttyUiEvent) {
        match event {
            GhosttyUiEvent::Wakeup(events) => {
                events.acknowledge_wakeup();
                self.dirty = true;
                self.output_generation = self.output_generation.saturating_add(1);
            }
            GhosttyUiEvent::Title(events) => {
                if let Some(title) = events.take_title()
                    && !is_executable_path_title(&title)
                {
                    self.title = title;
                }
            }
            GhosttyUiEvent::WorkingDirectory(events) => {
                if let Some(cwd) = events.take_working_directory() {
                    self.current_cwd = Some(cwd);
                }
            }
            GhosttyUiEvent::Progress(events) => {
                if let Some(report) = events.take_progress() {
                    self.progress = match report.state {
                        paneflow_terminal_ghostty::ProgressState::Remove => None,
                        _ => Some(report),
                    };
                }
            }
            GhosttyUiEvent::Notification(events) => {
                for notification in events.take_notifications() {
                    if self.pending_notifications.len() >= MAX_PENDING_NOTIFICATIONS {
                        self.pending_notifications.remove(0);
                    }
                    self.pending_notifications.push(notification);
                }
            }
            GhosttyUiEvent::Clipboard(events) => {
                for text in events.take_clipboard() {
                    self.deliver_clipboard_text(text);
                }
            }
            GhosttyUiEvent::ServiceOutputReady(events) => {
                events.acknowledge_service_output();
                self.last_activity_burst = None;
                self.dirty = true;
            }
            GhosttyUiEvent::ChildExited { code, signal } => {
                if self.exited.is_none() {
                    self.exited = Some(code);
                    self.exit_signal = signal;
                }
                self.dirty = true;
                self.progress = None;
                self.cached_foreground_command = None;
                self.reported_ports.clear();
            }
            GhosttyUiEvent::HyperlinkResolved { point, link } => {
                self.resolved_hover_link = Some((point, link));
            }
            GhosttyUiEvent::InputRejected(error) => {
                log::warn!(target: "paneflow::terminal::ghostty", "{error}");
            }
            GhosttyUiEvent::RuntimeFailed(error) => {
                log::error!(target: "paneflow::terminal::ghostty", "{error}");
                if self.exited.is_none() {
                    self.exited = Some(-1);
                }
                self.dirty = true;
            }
            GhosttyUiEvent::HostLink(state) => {
                self.mark_host_link(state);
            }
            GhosttyUiEvent::HostNotice(message) => {
                self.pending_host_notices.push(message);
            }
        }
    }

    fn deliver_clipboard_text(&mut self, text: String) {
        if self.terminal_focused
            && self.osc52_mode != Osc52Mode::Disabled
            && text.len() <= MAX_OSC52_BYTES
        {
            self.queue_clipboard_op(text);
        }
    }

    fn queue_clipboard_op(&mut self, text: String) {
        if self.pending_clipboard_ops.len() >= MAX_PENDING_CLIPBOARD_OPS {
            self.pending_clipboard_ops.remove(0);
        }
        self.pending_clipboard_ops.push(text);
    }

    #[cfg(target_os = "linux")]
    pub fn cwd_now(&self) -> Option<std::path::PathBuf> {
        if self.exited.is_some() {
            return None;
        }
        if self.child_pid == 0 {
            return None;
        }
        let proc_path = format!("/proc/{}/cwd", self.child_pid);
        std::fs::read_link(&proc_path).ok()
    }

    #[cfg(target_os = "macos")]
    pub fn cwd_now(&self) -> Option<std::path::PathBuf> {
        use std::ffi::CStr;
        use std::mem::MaybeUninit;
        use std::os::raw::c_void;

        if self.exited.is_some() {
            return None;
        }

        if self.child_pid == 0 {
            return None;
        }

        let pid = self.child_pid as libc::c_int;
        let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;

        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                info.as_mut_ptr() as *mut c_void,
                size,
            )
        };

        if written <= 0 {
            let err = std::io::Error::last_os_error();
            log::warn!(
                "cwd_now: proc_pidinfo(pid={pid}) returned {written} ({err}) - shell may have exited or SIP / sandbox is denying the read"
            );
            return None;
        }

        if written < size {
            log::warn!(
                "cwd_now: proc_pidinfo(pid={pid}) wrote {written} of {size} bytes - truncated result discarded"
            );
            return None;
        }

        let info = unsafe { info.assume_init() };

        let ptr = info.pvi_cdir.vip_path.as_ptr() as *const libc::c_char;
        let cstr = unsafe { CStr::from_ptr(ptr) };
        match cstr.to_str() {
            Ok(s) if !s.is_empty() => Some(std::path::PathBuf::from(s)),
            _ => None,
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub fn cwd_now(&self) -> Option<std::path::PathBuf> {
        None
    }

    pub fn scan_output(&mut self) -> Vec<ServiceInfo> {
        let lines = self.ghostty.recent_output_lines();
        self.detect_services_in_lines(&lines)
    }

    fn detect_services_in_lines(&mut self, lines: &[String]) -> Vec<ServiceInfo> {
        let parsed: Vec<Option<ServiceInfo>> =
            lines.iter().map(|line| parse_service_line(line)).collect();
        let mut ports = parsed.iter().flatten().map(|info| info.port);
        let first_port = ports.next();
        let single_port = ports.all(|port| Some(port) == first_port);
        let (global_label, global_is_frontend) = if single_port {
            detect_framework(&lines.join(" "))
        } else {
            (None, false)
        };

        let mut results = Vec::new();
        let mut preceding: (Option<String>, bool) = (None, false);
        for (line, parsed) in lines.iter().zip(parsed) {
            let Some(mut info) = parsed else {
                let framework = detect_framework(line);
                if framework.0.is_some() {
                    preceding = framework;
                }
                continue;
            };
            let (borrowed, borrowed_frontend) = std::mem::take(&mut preceding);
            if self.reported_ports.contains(&info.port) {
                continue;
            }
            if info.label.is_none() {
                (info.label, info.is_frontend) = match borrowed {
                    Some(label) => (Some(label), borrowed_frontend),
                    None => (global_label.clone(), global_is_frontend),
                };
            }
            self.reported_ports.insert(info.port);
            results.push(info);
        }

        results
    }

    pub(crate) fn input_sent(&self) -> bool {
        self.keyboard_input_sent
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn queued_raw_input_for_test(&self) -> Vec<Vec<u8>> {
        self.pending_input
            .lock()
            .map(|queued| {
                queued
                    .iter()
                    .filter_map(|input| match input {
                        PendingTerminalInput::Raw(bytes) => Some(bytes.to_vec()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn write_to_pty(&self, input: impl Into<Cow<'static, [u8]>>) -> BackendInputResult {
        self.keyboard_input_sent
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.notify_or_buffer(input.into())
    }

    pub(super) fn set_terminal_focused(&mut self, focused: bool) {
        self.terminal_focused = focused;
        self.clipboard_gate.set_focused(focused);
    }

    fn set_osc52_mode(&mut self, mode: Osc52Mode) {
        self.osc52_mode = mode;
        self.clipboard_gate.set_policy(mode != Osc52Mode::Disabled);
    }

    fn dispatch_ghostty_input(
        &self,
        input: PendingTerminalInput,
        user_initiated: bool,
    ) -> BackendInputResult {
        if !self.host_link.accepts_input() {
            return BackendInputResult::Rejected;
        }
        let Ok(mut pending) = self.pending_input.lock() else {
            return BackendInputResult::Rejected;
        };
        let pending_bytes = pending.iter().fold(0usize, |total, item| {
            total.saturating_add(item.queued_bytes())
        });
        let total = pending_bytes.saturating_add(self.ghostty.queued_input_bytes());
        let queue_limit = input.queue_limit();
        if !input.fits_after(total) {
            log::warn!(
                target: "paneflow::terminal::ghostty",
                "Ghostty input rejected at the {} byte queue limit",
                queue_limit
            );
            return BackendInputResult::Rejected;
        }

        if self.ghostty.is_promoted() && pending.is_empty() {
            match input.try_send(&self.ghostty) {
                GhosttyInputSendResult::Sent => {
                    if user_initiated {
                        self.keyboard_input_sent
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    return BackendInputResult::Accepted;
                }
                GhosttyInputSendResult::Full => {}
                GhosttyInputSendResult::Closed => return BackendInputResult::Rejected,
            }
        }

        pending.push_back(input);
        if user_initiated {
            self.keyboard_input_sent
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        BackendInputResult::Accepted
    }

    pub(super) fn write_ghostty_key(
        &self,
        input: paneflow_terminal_ghostty::KeyInput,
    ) -> BackendInputResult {
        self.dispatch_ghostty_input(PendingTerminalInput::Key(input), true)
    }

    pub(super) fn write_ghostty_mouse(
        &self,
        input: paneflow_terminal_ghostty::MouseInput,
        repeat: usize,
    ) -> BackendInputResult {
        self.dispatch_ghostty_input(PendingTerminalInput::Mouse { input, repeat }, true)
    }

    pub(super) fn write_ghostty_focus(
        &self,
        event: paneflow_terminal_ghostty::FocusEvent,
    ) -> BackendInputResult {
        self.dispatch_ghostty_input(PendingTerminalInput::Focus(event), false)
    }

    pub(super) fn write_ghostty_paste(
        &self,
        text: String,
        allow_unsafe: bool,
        location: paneflow_terminal_ghostty::ClipboardLocation,
    ) -> BackendInputResult {
        self.dispatch_ghostty_input(
            PendingTerminalInput::Paste {
                text,
                allow_unsafe,
                location,
            },
            true,
        )
    }

    fn notify_or_buffer(&self, input: Cow<'static, [u8]>) -> BackendInputResult {
        if input.is_empty() {
            return BackendInputResult::Rejected;
        }
        self.dispatch_ghostty_input(PendingTerminalInput::Raw(input), false)
    }

    pub fn bind_runtime(&self, runtime_id: Option<&'static str>) {
        self.ghostty.bind_runtime(runtime_id);
    }

    pub fn retains_final_view(&self) -> bool {
        self.exited.is_some()
    }

    #[cfg(test)]
    pub fn extract_scrollback(&self) -> Option<String> {
        self.ghostty.extract_scrollback()
    }

    pub fn foreground_command(&self) -> Option<String> {
        self.cached_foreground_command.clone()
    }

    pub fn retain_reported_ports(&mut self, live: &[u16]) {
        self.reported_ports.retain(|p| live.contains(p));
    }

    pub fn note_announced_port(&mut self, port: u16) {
        const MAX_ANNOUNCED_PORTS: usize = 16;
        if !self.announced_ports.contains(&port) && self.announced_ports.len() < MAX_ANNOUNCED_PORTS
        {
            self.announced_ports.push(port);
        }
    }

    pub fn restore_scrollback(&self, text: &str) {
        self.ghostty.restore_scrollback(text);
    }
}

fn is_executable_path_title(title: &str) -> bool {
    let p = std::path::Path::new(title);
    p.is_absolute()
        && p.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
}

#[cfg(all(test, windows))]
mod title_filter_tests {
    use super::is_executable_path_title;

    #[test]
    fn drops_shell_self_path_title_but_keeps_human_labels() {
        assert!(is_executable_path_title(
            r"C:\Program Files\PowerShell\7\pwsh.exe"
        ));
        assert!(is_executable_path_title(r"C:\Windows\System32\cmd.exe"));
        assert!(!is_executable_path_title("Claude Code"));
        assert!(!is_executable_path_title(r"C:\dev\paneflow"));
        assert!(!is_executable_path_title(""));
        assert!(!is_executable_path_title("pwsh.exe"));
    }
}

impl Drop for TerminalState {
    fn drop(&mut self) {
        self.ghostty.shutdown();
        self.child_pid = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_pending_terminal_has_no_child_until_promoted() {
        let (state, _pending) = TerminalState::new_pending(80, 24);
        assert_eq!(state.child_pid, 0);
    }

    #[test]
    fn spawn_failure_is_reported_once_without_sensitive_error_text() {
        const CANARY: &str =
            r#"C:\Users\synthetic-user\private\launch.ps1 --token super-secret-canary"#;
        let error = anyhow::Error::new(io::Error::from_raw_os_error(5)).context(CANARY);
        let os_error = raw_os_error_from_anyhow(&error);
        assert_eq!(os_error, Some(5));

        let failure = TerminalBackendFailureDiagnostics::new(
            TerminalBackendFailurePhase::Initialization,
            TerminalBackendFailureDiagnostics::GHOSTTY_INITIALIZATION_FAILED,
            os_error,
        );
        let mut state = TerminalState::new_display_only(24, 80);
        state.report_spawn_failure(failure.clone(), "engine start failed");

        let diagnostics = state.backend_diagnostics();
        assert_eq!(diagnostics.failure, Some(failure));
        let formatted = diagnostics.to_string();
        assert_eq!(formatted.matches("reason_code=").count(), 1);
        assert!(formatted.contains("failure_phase=initialization"));
        assert!(formatted.contains("reason_code=ghostty_initialization_failed"));
        assert!(formatted.contains("os_error=5"));
        assert!(!formatted.contains(CANARY));
        assert!(!formatted.contains("private"));
        assert!(!formatted.contains("super-secret-canary"));
    }

    #[test]
    fn write_to_pty_buffers_input_while_display_only() {
        let (state, _events_tx) = TerminalState::new_pending(80, 24);
        state.write_to_pty(b"claude\r".to_vec());
        let queued = state.pending_input.lock().expect("pending_input lock");
        assert_eq!(queued.len(), 1);
        assert!(
            matches!(&queued[0], PendingTerminalInput::Raw(bytes) if bytes.as_ref() == b"claude\r")
        );
    }

    #[test]
    fn pending_input_is_bounded() {
        let (state, _events_tx) = TerminalState::new_pending(80, 24);
        let chunk = vec![b'x'; 8 * 1024];
        for _ in 0..(MAX_PENDING_INPUT_BYTES / chunk.len() + 2) {
            state.write_to_pty(chunk.clone());
        }
        let queued: usize = state
            .pending_input
            .lock()
            .expect("pending_input lock")
            .iter()
            .map(PendingTerminalInput::queued_bytes)
            .sum();
        assert!(
            queued <= MAX_PENDING_INPUT_BYTES,
            "buffered {queued} bytes exceeds the {MAX_PENDING_INPUT_BYTES} cap"
        );
    }

    fn test_key_input(
        action: paneflow_terminal_ghostty::KeyAction,
    ) -> paneflow_terminal_ghostty::KeyInput {
        paneflow_terminal_ghostty::KeyInput {
            key: paneflow_terminal_ghostty::Key::Function(5),
            action,
            modifiers: paneflow_terminal_ghostty::Modifiers::CONTROL,
            consumed_modifiers: paneflow_terminal_ghostty::Modifiers::empty(),
            text: String::new(),
            unshifted_codepoint: None,
            composing: false,
        }
    }

    fn test_mouse_input(
        action: paneflow_terminal_ghostty::MouseAction,
    ) -> paneflow_terminal_ghostty::MouseInput {
        paneflow_terminal_ghostty::MouseInput {
            action,
            button: Some(paneflow_terminal_ghostty::MouseButton::Left),
            modifiers: paneflow_terminal_ghostty::Modifiers::empty(),
            x: 8.0,
            y: 16.0,
            screen_width: 640,
            screen_height: 384,
            padding_top: 0,
            padding_bottom: 0,
            padding_left: 0,
            padding_right: 0,
            any_button_pressed: action != paneflow_terminal_ghostty::MouseAction::Release,
        }
    }

    #[test]
    fn control_releases_fit_after_general_input_saturates() {
        let general_limit = MAX_PENDING_INPUT_BYTES - INPUT_CONTROL_RESERVE_BYTES;
        let press =
            PendingTerminalInput::Key(test_key_input(paneflow_terminal_ghostty::KeyAction::Press));
        let key_release = PendingTerminalInput::Key(test_key_input(
            paneflow_terminal_ghostty::KeyAction::Release,
        ));
        let mouse_release = PendingTerminalInput::Mouse {
            input: test_mouse_input(paneflow_terminal_ghostty::MouseAction::Release),
            repeat: 1,
        };
        let focus = PendingTerminalInput::Focus(paneflow_terminal_ghostty::FocusEvent::Lost);

        assert!(!press.fits_after(general_limit));
        assert!(key_release.fits_after(general_limit));
        assert!(mouse_release.fits_after(general_limit));
        assert!(focus.fits_after(general_limit));
    }

    #[test]
    fn structured_input_is_queued_in_order_before_promotion() {
        let (state, _pending) = TerminalState::new_pending(80, 24);

        assert_eq!(
            state.write_ghostty_key(test_key_input(paneflow_terminal_ghostty::KeyAction::Press)),
            BackendInputResult::Accepted
        );
        assert_eq!(
            state.write_ghostty_mouse(
                test_mouse_input(paneflow_terminal_ghostty::MouseAction::Press),
                2,
            ),
            BackendInputResult::Accepted
        );
        assert_eq!(
            state.write_ghostty_focus(paneflow_terminal_ghostty::FocusEvent::Gained),
            BackendInputResult::Accepted
        );
        assert_eq!(
            state.write_ghostty_paste(
                "paste".to_string(),
                false,
                paneflow_terminal_ghostty::ClipboardLocation::Primary,
            ),
            BackendInputResult::Accepted
        );

        let queued = state.pending_input.lock().expect("pending_input lock");
        assert_eq!(queued.len(), 4);
        assert!(matches!(queued[0], PendingTerminalInput::Key(_)));
        assert!(matches!(queued[1], PendingTerminalInput::Mouse { .. }));
        assert!(matches!(queued[2], PendingTerminalInput::Focus(_)));
        assert!(matches!(
            queued[3],
            PendingTerminalInput::Paste {
                location: paneflow_terminal_ghostty::ClipboardLocation::Primary,
                ..
            }
        ));
    }

    #[test]
    fn child_exit_records_real_code_not_sentinel() {
        let mut state = TerminalState::new_display_only(24, 80);
        assert!(state.exited.is_none(), "fresh terminal has no exit code");

        state.process_ghostty_event(GhosttyUiEvent::ChildExited {
            code: 42,
            signal: None,
        });
        assert_eq!(
            state.exited,
            Some(42),
            "US-003: the real exit code must be recorded, not -1"
        );
    }

    #[test]
    fn exit_fallback_does_not_clobber_real_child_exit_code() {
        let mut state = TerminalState::new_display_only(24, 80);

        state.process_ghostty_event(GhosttyUiEvent::ChildExited {
            code: 1,
            signal: None,
        });
        state.process_ghostty_event(GhosttyUiEvent::RuntimeFailed(
            "engine mailbox closed".to_owned(),
        ));
        assert_eq!(
            state.exited,
            Some(1),
            "US-003: a later failure must not clobber the real exit code"
        );
    }

    #[test]
    fn natural_exit_retains_the_final_view_on_every_branch() {
        let live = TerminalState::new_display_only(24, 80);
        assert!(
            !live.retains_final_view(),
            "US-009: a live terminal is not a final view"
        );

        let mut clean = TerminalState::new_display_only(24, 80);
        clean.exited = Some(0);
        assert!(
            clean.retains_final_view(),
            "US-009: a clean exit keeps the passive final view"
        );

        let mut failed = TerminalState::new_display_only(24, 80);
        failed.exited = Some(127);
        assert!(
            failed.retains_final_view(),
            "US-009: a non-zero exit without input keeps the passive final view"
        );

        failed.write_to_pty(b"x".as_slice());
        assert!(
            failed.retains_final_view(),
            "US-009: prior keyboard input never turns an exit into a close"
        );
    }

    #[test]
    fn write_to_pty_marks_user_input_but_fresh_state_does_not() {
        let state = TerminalState::new_display_only(24, 80);
        assert!(
            !state
                .keyboard_input_sent
                .load(std::sync::atomic::Ordering::Relaxed),
            "fresh terminal must report no user input"
        );
        state.write_to_pty(b"a".as_slice());
        assert!(
            state
                .keyboard_input_sent
                .load(std::sync::atomic::Ordering::Relaxed),
            "write_to_pty must mark the session user-initiated"
        );
    }

    #[test]
    fn foreground_command_none_for_display_only() {
        let state = TerminalState::new_display_only(24, 80);
        assert!(
            state.foreground_command().is_none(),
            "display-only terminal has no foreground process to resolve"
        );
    }

    #[test]
    fn scan_output_uses_multiline_framework_context() {
        let mut state = TerminalState::new_display_only(24, 80);

        let services = state.detect_services_in_lines(&[
            "▲ Next.js 16.1.6".to_string(),
            "- Local: http://localhost:3000".to_string(),
        ]);

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].port, 3000);
        assert_eq!(services[0].label.as_deref(), Some("Next.js"));
        assert!(services[0].is_frontend);
    }

    #[test]
    fn a_service_chip_never_borrows_the_label_of_another_port() {
        let mut state = TerminalState::new_display_only(24, 80);

        let services = state.detect_services_in_lines(&[
            "▲ Next.js 16.1.6".to_string(),
            "- Local: http://localhost:3000".to_string(),
            "api listening on http://localhost:8080".to_string(),
        ]);

        assert_eq!(services.len(), 2);
        assert_eq!(services[0].label.as_deref(), Some("Next.js"));
        assert_eq!(services[1].port, 8080);
        assert_eq!(
            services[1].label, None,
            "8080 is not the Next.js server printed above 3000"
        );
        assert!(!services[1].is_frontend);
    }

    #[test]
    fn scan_output_dedups_until_port_leaves_live_set() {
        let mut state = TerminalState::new_display_only(24, 80);
        let lines = ["Vite ready at http://localhost:5173".to_string()];

        assert_eq!(state.detect_services_in_lines(&lines).len(), 1);
        assert!(state.detect_services_in_lines(&lines).is_empty());

        state.retain_reported_ports(&[]);
        let services = state.detect_services_in_lines(&lines);

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].port, 5173);
    }

    #[test]
    fn announced_ports_are_deduped_and_bounded() {
        let mut state = TerminalState::new_display_only(24, 80);
        state.note_announced_port(3000);
        state.note_announced_port(3000);
        for port in 3001..3025 {
            state.note_announced_port(port);
        }

        assert_eq!(state.announced_ports.len(), 16);
        assert_eq!(state.announced_ports[0], 3000);
        assert_eq!(
            state.announced_ports.iter().filter(|&&p| p == 3000).count(),
            1
        );
    }

    const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    #[test]
    fn search_rows_returns_unique_lines_and_reports_only_real_overflow() {
        let state = TerminalState::new_display_only(5, 80);
        state.write_output(
            b"first needle needle\r\nsecond needle\r\nthird needle\r\nwithout marker",
        );
        let backend = state.session_backend();

        let limited = backend
            .search_rows("needle", 2, QUERY_TIMEOUT)
            .expect("search answers");
        assert_eq!(limited.matches.len(), 2);
        assert!(limited.truncated);
        assert_eq!(limited.matches[0].1, "first needle needle");
        assert_eq!(limited.matches[1].1, "second needle");

        let exact = backend
            .search_rows("needle", 3, QUERY_TIMEOUT)
            .expect("search answers");
        assert_eq!(exact.matches.len(), 3);
        assert!(
            !exact.truncated,
            "exactly max_matches hits is not a truncated result"
        );
        assert_eq!(exact.matches[2].1, "third needle");
    }

    #[test]
    fn read_rows_returns_the_requested_window_only() {
        let state = TerminalState::new_display_only(5, 80);
        let output: String = (0..40).map(|index| format!("row {index}\r\n")).collect();
        state.write_output(output.as_bytes());

        let window = state
            .session_backend()
            .read_rows(4, 10, QUERY_TIMEOUT)
            .expect("read answers");

        assert_eq!(window.total_lines, 40);
        assert_eq!(window.lines, vec!["row 26", "row 27", "row 28", "row 29"]);
        assert!(!window.eof);
    }

    #[test]
    fn runtime_queries_report_a_stalled_runtime_as_a_timeout() {
        let state = TerminalState::new_display_only(5, 80);
        let backend = state.session_backend();
        backend.stall_runtime_for_test(std::time::Duration::from_millis(600));
        let timeout = std::time::Duration::from_millis(100);

        assert_eq!(
            backend.read_rows(10, 0, timeout),
            Err(crate::terminal::types::TerminalQueryError::TimedOut(
                timeout
            ))
        );
        assert_eq!(
            backend.search_rows("row", 10, timeout),
            Err(crate::terminal::types::TerminalQueryError::TimedOut(
                timeout
            ))
        );
    }

    #[test]
    fn cwd_now_none_for_display_only() {
        let state = TerminalState::new_display_only(24, 80);
        assert_eq!(state.child_pid, 0);
        assert!(
            state.cwd_now().is_none(),
            "display-only terminal has no shell CWD to resolve"
        );
    }

    #[test]
    fn pending_clipboard_ops_are_bounded() {
        let mut state = TerminalState::new_display_only(5, 20);

        for i in 0..(MAX_PENDING_CLIPBOARD_OPS + 2) {
            state.queue_clipboard_op(format!("op-{i}"));
        }

        assert_eq!(state.pending_clipboard_ops.len(), MAX_PENDING_CLIPBOARD_OPS);
        assert_eq!(state.pending_clipboard_ops[0], "op-2");
    }

    #[test]
    fn osc52_clipboard_off_refuses_every_program_clipboard_write() {
        let (mut state, _pending) = TerminalState::new_pending(80, 24);
        state.osc52_policy = paneflow_config::schema::Osc52ClipboardConfig::Off;
        state.promote_ghostty(SpawnedGhostty {
            child_pid: 0,
            cwd: std::env::current_dir().unwrap(),
        });
        state.set_terminal_focused(true);

        state.deliver_clipboard_text("secret".into());

        assert!(state.osc52_mode == Osc52Mode::Disabled);
        assert!(state.pending_clipboard_ops.is_empty());

        let (mut copying, _pending) = TerminalState::new_pending(80, 24);
        copying.promote_ghostty(SpawnedGhostty {
            child_pid: 0,
            cwd: std::env::current_dir().unwrap(),
        });
        copying.set_terminal_focused(true);
        copying.deliver_clipboard_text("copied".into());
        assert_eq!(copying.pending_clipboard_ops, vec!["copied".to_owned()]);
    }

    #[test]
    fn osc52_store_requires_focus_and_respects_the_shared_cap() {
        let mut state = TerminalState::new_display_only(5, 20);
        state.set_osc52_mode(Osc52Mode::CopyOnly);

        state.deliver_clipboard_text("unfocused".into());
        assert!(state.pending_clipboard_ops.is_empty());

        state.set_terminal_focused(true);
        state.deliver_clipboard_text("focused".into());
        assert_eq!(state.pending_clipboard_ops, vec!["focused".to_string()]);

        state.deliver_clipboard_text("x".repeat(MAX_OSC52_BYTES + 1));
        assert_eq!(state.pending_clipboard_ops.len(), 1);

        state.set_terminal_focused(false);
        state.deliver_clipboard_text("lost-focus".into());
        assert_eq!(state.pending_clipboard_ops.len(), 1);
    }

    #[test]
    fn restore_scrollback_strips_escape_and_osc_injection() {
        let hostile = "\x1b]8;;https://evil.example/\x07click\x1b]8;;\x07\
                       \x1b]0;PWNED\x07\x1b[31mred\x00\u{9b}38m";
        let state = TerminalState::new_display_only(6, 80);
        state.restore_scrollback(hostile);
        state.restore_scrollback("a\tb");

        assert_eq!(state.title, "Terminal", "OSC 0 must not retitle the pane");

        let backend = state.session_backend();
        let restored = (0..6)
            .filter_map(|row| backend.line_text_at(Point::new(row, 0)))
            .map(|line| line.text)
            .collect::<Vec<_>>()
            .join("\n");
        for marker in ["https://evil.example/", "click", "PWNED", "red", "38m"] {
            assert!(
                restored.contains(marker),
                "plain glyphs must survive; {marker:?} missing from {restored:?}"
            );
        }
        assert!(
            !restored.contains('\x1b') && !restored.contains('\x07'),
            "no VT introducer may reach the grid; got {restored:?}"
        );
    }

    #[test]
    fn extract_scrollback_drains_history_only() {
        let state = TerminalState::new_display_only(3, 80);
        state.restore_scrollback("history-alpha\nhistory-bravo\nvisible-charlie\nvisible-delta");

        let drained = state
            .extract_scrollback()
            .expect("seeded scrollback should not be empty");

        for marker in ["history-alpha", "history-bravo"] {
            assert!(
                drained.contains(marker),
                "drained scrollback must contain {marker:?}; got:\n{drained}"
            );
        }
        for marker in ["visible-charlie", "visible-delta"] {
            assert!(
                !drained.contains(marker),
                "active viewport must exclude {marker:?}; got:\n{drained}"
            );
        }
    }

    #[test]
    fn output_generation_advances_on_pty_output() {
        let mut state = TerminalState::new(None, 1, 1, Some((80, 24)), None)
            .expect("spawn a PTY-backed terminal");
        assert_eq!(
            state.output_generation, 0,
            "a fresh terminal has produced no output"
        );

        std::thread::sleep(std::time::Duration::from_millis(250));
        state.notify_or_buffer(Cow::Owned(b"echo PANEFLOW_GEN_OK\n".to_vec()));

        let mut advanced = false;
        for _ in 0..240 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            state.sync();
            if state.output_generation > 0 {
                advanced = true;
                break;
            }
        }
        assert!(
            advanced,
            "output_generation must advance once the PTY emits output"
        );
    }

    #[test]
    fn a_bell_flood_and_a_huge_grapheme_keep_a_live_pane_running_until_its_real_exit() {
        let dir = tempfile::tempdir().expect("flood fixture dir");
        let flood_path = dir.path().join("bell-flood.bin");
        let mut flood = b"\x07flood-txt".repeat(10_000);
        assert_eq!(flood.len(), 100_000);
        flood.extend_from_slice(b"\r\ne");
        flood.extend("\u{0301}".repeat(1_999).as_bytes());
        std::fs::write(&flood_path, &flood).expect("write flood fixture");
        let flood_arg = flood_path.to_string_lossy().into_owned();
        #[cfg(unix)]
        let (shell, shell_quoting, extra_args) = (
            "/bin/sh".to_owned(),
            ShellQuoting::Posix,
            vec![
                "-c".to_owned(),
                "cat \"$1\"; printf '\nPANEFLOW_FLOOD_%s\n' ok".to_owned(),
                "sh".to_owned(),
                flood_arg,
            ],
        );
        #[cfg(target_os = "windows")]
        let (shell, shell_quoting, extra_args) = (
            "cmd.exe".to_owned(),
            ShellQuoting::Cmd,
            vec![
                "/D".to_owned(),
                "/Q".to_owned(),
                "/C".to_owned(),
                format!("type \"{flood_arg}\" & echo. & echo PANEFLOW_FLOOD_^ok"),
            ],
        );
        let params = SpawnParams {
            shell,
            shell_quoting,
            extra_args,
            env: std::collections::HashMap::from([
                ("TERM".into(), "xterm-256color".into()),
                ("TERM_PROGRAM".into(), "paneflow".into()),
            ]),
            cwd: dir.path().to_path_buf(),
            cols: 80,
            rows: 24,
            profile: TerminalSurfaceProfile::Normal,
        };
        let (mut state, pending) =
            TerminalState::new_pending_with_shell_quoting(80, 24, shell_quoting);
        let spawned = state
            .ghostty_session()
            .start(pending.ghostty, params, 10_000)
            .expect("spawn the flood shell");
        state.promote_ghostty(spawned);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while state.exited.is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(25));
            state.sync();
        }
        assert_eq!(
            state.exited,
            Some(0),
            "the flood must not turn the pane into a failed runtime before its real exit"
        );
        let (content, _) = state.session_backend().render_content(
            TerminalWindowSize::new(80, 24, 8, 16),
            -100,
            100,
            false,
        );
        let screen: String = content.cells.iter().map(|cell| cell.c).collect();
        assert!(
            screen.contains("PANEFLOW_FLOOD_ok"),
            "the output after the flood reaches the screen: {screen:?}"
        );
    }
}
