use std::sync::{Arc, Mutex};

use futures::StreamExt;
use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyContext, MouseButton, Render, Styled, Window, div, prelude::*,
};
use paneflow_config::schema::{TerminalConfig, TerminalSurfaceProfile};

use super::TerminalState;
use super::element::TerminalElement;
use super::pty_session::{
    TerminalBackendFailureDiagnostics, TerminalBackendFailurePhase, raw_os_error_from_anyhow,
};
use super::service_detector::ServiceInfo;
use super::types::{
    CopyModeCursorState, CursorShape, HyperlinkZone, Line, Modes, Point, SearchHighlight,
    TerminalWindowSize,
};
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

use super::ghostty_session::GhosttyStartError;

struct GhosttyStartFailure {
    child_pid: Option<u32>,
    diagnostics: TerminalBackendFailureDiagnostics,
}

fn classify_ghostty_start_error(error: GhosttyStartError) -> GhosttyStartFailure {
    let (phase, reason_code, source) = match error {
        GhosttyStartError::Initialization(error) => (
            TerminalBackendFailurePhase::Initialization,
            TerminalBackendFailureDiagnostics::GHOSTTY_INITIALIZATION_FAILED,
            error,
        ),
        GhosttyStartError::OpenPty(error) => (
            TerminalBackendFailurePhase::OpenPty,
            TerminalBackendFailureDiagnostics::GHOSTTY_OPEN_PTY_FAILED,
            error,
        ),
        GhosttyStartError::Spawn(error) => (
            TerminalBackendFailurePhase::Spawn,
            TerminalBackendFailureDiagnostics::GHOSTTY_SPAWN_FAILED,
            error,
        ),
        GhosttyStartError::PostSpawn { child_pid, error } => {
            return GhosttyStartFailure {
                child_pid: Some(child_pid),
                diagnostics: TerminalBackendFailureDiagnostics::new(
                    TerminalBackendFailurePhase::PostSpawn,
                    TerminalBackendFailureDiagnostics::GHOSTTY_POST_SPAWN_FAILED,
                    raw_os_error_from_anyhow(&error),
                ),
            };
        }
    };
    GhosttyStartFailure {
        child_pid: None,
        diagnostics: TerminalBackendFailureDiagnostics::new(
            phase,
            reason_code,
            raw_os_error_from_anyhow(&source),
        ),
    }
}

const RENDER_WAKEUP_IMMEDIATELY: bool = true;

static BACKEND_START_FAILED_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn claim_backend_failure_report(reported: &std::sync::atomic::AtomicBool) -> bool {
    !reported.swap(true, std::sync::atomic::Ordering::Relaxed)
}

fn backend_failure_level(reported: &std::sync::atomic::AtomicBool) -> log::Level {
    if claim_backend_failure_report(reported) {
        log::Level::Error
    } else {
        log::Level::Debug
    }
}

fn log_backend_diagnostics(terminal: &TerminalState) {
    let diagnostics = terminal.backend_diagnostics();
    log::info!(
        target: "paneflow::terminal::backend",
        "Terminal backend selected: {diagnostics}"
    );
}

#[cfg(debug_assertions)]
pub(crate) fn probe_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("PANEFLOW_LATENCY_PROBE").as_deref() == Ok("1"))
}

fn spawn_error_message(failure: &TerminalBackendFailureDiagnostics) -> String {
    format!(
        "\x1b[1;31mError\x1b[0m: failed to start the terminal.\r\n\
         \r\n\
         Common causes:\r\n\
         \x20 \x20- PTY pool exhausted\r\n\
         \x20 \x20- Shell binary not found ($SHELL / default_shell)\r\n\
         \x20 \x20- Permission denied on /dev/ptmx\r\n\
         \r\n\
         \x1b[2mfailure_phase={} reason_code={} os_error={:?}\x1b[0m\r\n",
        failure.phase.as_str(),
        failure.reason_code,
        failure.os_error,
    )
}

fn engine_cursor_shape(shape: CursorShape) -> paneflow_terminal_ghostty::CursorShape {
    use paneflow_terminal_ghostty::CursorShape as Engine;
    match shape {
        CursorShape::Block | CursorShape::Vintage | CursorShape::Hidden => Engine::Block,
        CursorShape::Beam => Engine::Bar,
        CursorShape::Underline | CursorShape::DoubleUnderline => Engine::Underline,
        CursorShape::HollowBlock => Engine::HollowBlock,
    }
}

fn renderer_cursor_shape_from_config(
    shape: paneflow_config::schema::CursorShapeConfig,
) -> CursorShape {
    use paneflow_config::schema::CursorShapeConfig as C;
    match shape {
        C::Vintage => CursorShape::Vintage,
        C::Block => CursorShape::Block,
        C::Beam => CursorShape::Beam,
        C::Underline => CursorShape::Underline,
        C::DoubleUnderline => CursorShape::DoubleUnderline,
        C::Hollow => CursorShape::HollowBlock,
    }
}

pub(crate) fn hsla_from_hex_color(raw: &str) -> Option<Hsla> {
    let normalized = paneflow_config::schema::normalize_hex_color(raw)?;
    let rgb = u32::from_str_radix(&normalized[1..], 16).ok()?;
    Some(Hsla::from(gpui::rgb(rgb)))
}

fn cursor_color_override_from_config(terminal_config: &TerminalConfig) -> Option<Hsla> {
    terminal_config
        .cursor_color
        .as_deref()
        .and_then(hsla_from_hex_color)
}

pub(super) fn sanitize_osc52(text: &str) -> String {
    text.chars()
        .filter(|&c| c == '\t' || c == '\n' || !c.is_control())
        .collect()
}

#[derive(Clone, Copy)]
pub(super) struct ScrollbarDrag {
    pub(super) anchor_y: gpui::Pixels,
    pub(super) anchor_offset: usize,
    pub(super) metrics: super::element::ScrollbarMetrics,
    pub(super) last_target: usize,
}

#[derive(Clone)]
pub(super) struct HoverLinkCache {
    line: Line,
    cwd: Option<String>,
    line_text: String,
    zones: Vec<HyperlinkZone>,
}

pub struct TerminalView {
    pub terminal: TerminalState,
    focus_handle: FocusHandle,
    pub(super) cursor_visible: bool,
    pub(super) selecting: bool,
    pub(super) cell_width: gpui::Pixels,
    pub(super) line_height: gpui::Pixels,
    pub(super) element_origin: Arc<Mutex<gpui::Point<gpui::Pixels>>>,
    layout_cache: crate::terminal::element::SharedLayoutCache,
    pub(super) scrollbar_metrics: Arc<Mutex<Option<super::element::ScrollbarMetrics>>>,
    pub(super) scrollbar_drag: Option<ScrollbarDrag>,
    pub(super) scroll_remainder: f32,
    pub(super) search_active: bool,
    pub(super) search_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    pub(super) search_query: String,
    pub(super) search_generation: u64,
    pub(super) search_cancellation: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(super) search_matches: Vec<crate::search::SearchMatch>,
    pub(super) search_current: usize,
    pub(super) search_regex_mode: bool,
    pub(super) search_regex_error: Option<String>,
    pub(super) search_truncated: bool,
    appearance_theme_generation: u64,
    pub(super) option_as_meta: bool,
    pub(super) cursor_blink_mode: paneflow_config::schema::CursorBlinkConfig,
    pub(super) default_cursor_shape: CursorShape,
    pub(super) cursor_color_override: Option<Hsla>,
    pub(super) scroll_multiplier: f32,
    pub(super) integrated_glyphs_enabled: bool,
    pub(super) color_emoji_enabled: bool,
    pub(super) minimum_contrast: f32,
    pub(super) copy_mode_active: bool,
    pub(super) copy_cursor: Point,
    pub(super) copy_mode_frozen_offset: usize,
    was_focused: bool,
    focus_subscriptions: Option<(gpui::Subscription, gpui::Subscription)>,
    pub(super) ghostty_pressed_keys:
        std::collections::HashMap<String, paneflow_terminal_ghostty::KeyInput>,
    pub(super) ghostty_pending_text_key:
        Option<(gpui::Keystroke, paneflow_terminal_ghostty::KeyAction, bool)>,
    pub(super) hovered_cell: Option<Point>,
    pub(super) ctrl_hovered_link: Option<HyperlinkZone>,
    pub(super) link_modifier_held: bool,
    pub(super) hover_link_cache: Option<HoverLinkCache>,
    pub(super) mouse_down_link: Option<HyperlinkZone>,
    ime_marked_text: String,
    needs_initial_clear: Arc<std::sync::atomic::AtomicBool>,
    terminal_window_size: Arc<Mutex<Option<TerminalWindowSize>>>,
}

impl TerminalView {
    fn recorded_window_size(&self) -> Option<TerminalWindowSize> {
        *self
            .terminal_window_size
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn apply_backend_wakeup(&mut self, cx: &mut Context<Self>) {
        self.terminal.process_backend_wakeup();
        self.process_dirty_terminal(cx);
    }

    fn process_dirty_terminal(&mut self, cx: &mut Context<Self>) {
        if !self.terminal.dirty {
            return;
        }
        self.terminal.dirty = false;

        const BURST_THROTTLE: std::time::Duration = std::time::Duration::from_millis(300);
        let now = std::time::Instant::now();
        if self
            .terminal
            .last_activity_burst
            .is_none_or(|t| now.duration_since(t) >= BURST_THROTTLE)
        {
            self.terminal.last_activity_burst = Some(now);
            for service in self.terminal.scan_output() {
                cx.emit(TerminalEvent::ServiceDetected(service));
            }
            cx.emit(TerminalEvent::ActivityBurst);
        }

        if self.copy_mode_active {
            self.terminal
                .session_backend()
                .restore_display_offset(self.copy_mode_frozen_offset);
        }

        cx.notify();
    }

    pub(crate) fn restore_scrollback(&self, text: &str) {
        self.needs_initial_clear
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.terminal.restore_scrollback(text);
    }

    pub(crate) fn restore_replay(&self, replay: &[u8]) {
        self.needs_initial_clear
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.terminal.restore_replay(replay);
    }

    pub(crate) fn set_integrated_glyphs_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.integrated_glyphs_enabled != enabled {
            self.integrated_glyphs_enabled = enabled;
            cx.notify();
        }
    }

    pub(crate) fn set_color_emoji_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.color_emoji_enabled != enabled {
            self.color_emoji_enabled = enabled;
            cx.notify();
        }
    }

    pub(crate) fn set_minimum_contrast(&mut self, minimum_contrast: f32, cx: &mut Context<Self>) {
        if self.minimum_contrast != minimum_contrast {
            self.minimum_contrast = minimum_contrast;
            cx.notify();
        }
    }

    pub(crate) fn set_cursor_color_override(
        &mut self,
        color: Option<Hsla>,
        cx: &mut Context<Self>,
    ) {
        if self.cursor_color_override != color {
            self.cursor_color_override = color;
            cx.notify();
        }
    }

    pub fn new(workspace_id: u64, cx: &mut Context<Self>) -> Self {
        Self::with_cwd(workspace_id, None, None, cx)
    }

    pub fn with_cwd(
        workspace_id: u64,
        cwd: Option<std::path::PathBuf>,
        initial_size: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_cwd_and_env(workspace_id, cwd, initial_size, None, cx)
    }

    pub fn with_cwd_and_profile(
        workspace_id: u64,
        cwd: Option<std::path::PathBuf>,
        initial_size: Option<(usize, usize)>,
        profile: TerminalSurfaceProfile,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_cwd_env_and_profile(workspace_id, cwd, initial_size, None, profile, cx)
    }

    pub fn with_cwd_and_env(
        workspace_id: u64,
        cwd: Option<std::path::PathBuf>,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_cwd_env_and_profile(
            workspace_id,
            cwd,
            initial_size,
            user_env,
            TerminalSurfaceProfile::Normal,
            cx,
        )
    }

    pub fn with_cwd_env_and_profile(
        workspace_id: u64,
        cwd: Option<std::path::PathBuf>,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
        profile: TerminalSurfaceProfile,
        cx: &mut Context<Self>,
    ) -> Self {
        let surface_id = cx.entity_id().as_u64();

        let params = TerminalState::resolve_spawn_params_with_profile(
            cwd,
            workspace_id,
            surface_id,
            initial_size,
            user_env,
            profile,
        );
        let (terminal, pending) = TerminalState::new_pending_with_profile_and_shell_quoting(
            params.cols,
            params.rows,
            params.profile,
            params.shell_quoting,
        );
        let ghostty = terminal.ghostty_session();
        let ghostty_pending = pending.ghostty;
        let signal_mask = crate::terminal::pty_session::capture_foreground_signal_mask();

        let view = Self::from_terminal_state(workspace_id, terminal, cx);

        let executor = cx.background_executor().clone();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let outcome = executor
                    .spawn(async move {
                        let max_scrollback = paneflow_config::loader::load_config()
                            .terminal
                            .unwrap_or_default()
                            .resolved_scrollback_lines_for_profile(params.profile);
                        ghostty
                            .start(ghostty_pending, params, signal_mask, max_scrollback)
                            .map_err(classify_ghostty_start_error)
                    })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    match outcome {
                        Ok(spawned) => {
                            view.terminal.promote_ghostty(spawned);
                            if let Some(size) = view.recorded_window_size() {
                                view.terminal.notify_window_size(size);
                            }
                        }
                        Err(failure) => {
                            let after_child = match failure.child_pid {
                                Some(pid) => format!(" after child creation (pid={pid})"),
                                None => String::new(),
                            };
                            log::log!(
                                target: "paneflow::terminal::backend",
                                backend_failure_level(&BACKEND_START_FAILED_LOGGED),
                                "Ghostty startup failed{after_child}: failure_phase={} reason_code={} os_error={:?}",
                                failure.diagnostics.phase.as_str(),
                                failure.diagnostics.reason_code,
                                failure.diagnostics.os_error,
                            );
                            view.needs_initial_clear
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                            let message = spawn_error_message(&failure.diagnostics);
                            view.terminal
                                .report_spawn_failure(failure.diagnostics, &message);
                        }
                    }
                    log_backend_diagnostics(&view.terminal);
                    cx.notify();
                });
            },
        )
        .detach();

        view
    }

    fn from_terminal_state(
        _workspace_id: u64,
        mut terminal: TerminalState,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let search_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Search", cx));
        cx.observe(&search_input, |this, _input, cx| {
            this.on_search_input_changed(cx);
        })
        .detach();

        let events_rx = terminal.take_backend_events();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut events_rx = events_rx;
                let mut immediate_ghostty_wakeup_burst_active = false;
                while let Some(first_event) = events_rx.next().await {
                    let mut batch = Vec::with_capacity(32);
                    let mut dequeued = 1usize;
                    let render_wakeup_immediately = RENDER_WAKEUP_IMMEDIATELY;
                    let mut had_wakeup = first_event.is_wakeup();
                    let leading_immediate_wakeup = render_wakeup_immediately
                        && had_wakeup
                        && !immediate_ghostty_wakeup_burst_active;
                    if leading_immediate_wakeup {
                        immediate_ghostty_wakeup_burst_active = true;
                        let result = cx.update(|cx| {
                            this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                                view.apply_backend_wakeup(cx);
                            })
                        });
                        if result.is_err() {
                            break;
                        }
                        had_wakeup = false;
                    }
                    if !had_wakeup && !leading_immediate_wakeup {
                        batch.push(first_event);
                    }

                    let mut batch_window_elapsed = false;
                    {
                        let timer = futures::FutureExt::fuse(smol::Timer::after(
                            std::time::Duration::from_millis(4),
                        ));
                        futures::pin_mut!(timer);
                        loop {
                            futures::select_biased! {
                                event = events_rx.next() => {
                                    match event {
                                        Some(event) if event.is_wakeup() => {
                                            had_wakeup = true;
                                            dequeued += 1;
                                        }
                                        Some(event) => {
                                            batch.push(event);
                                            dequeued += 1;
                                        }
                                        None => break,
                                    }
                                    if dequeued >= 100 { break; }
                                }
                                _ = timer => {
                                    batch_window_elapsed = true;
                                    break;
                                },
                            }
                        }
                    }
                    if batch_window_elapsed {
                        immediate_ghostty_wakeup_burst_active = false;
                    }

                    let result = cx.update(|cx| {
                        this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                            let old_title = view.terminal.title.clone();
                            let old_cwd = view.terminal.current_cwd.clone();
                            let was_busy = view.terminal.progress.is_some();
                            view.terminal.sync_channels();
                            if had_wakeup {
                                view.terminal.process_backend_wakeup();
                            }
                            for event in batch {
                                view.terminal.process_backend_event(event);
                            }
                            if let Some((point, link)) = view.terminal.take_resolved_hover_link() {
                                view.apply_resolved_hover_link(point, link, cx);
                            }

                            let clipboard_ops =
                                std::mem::take(&mut view.terminal.pending_clipboard_ops);
                            for text in clipboard_ops {
                                cx.write_to_clipboard(ClipboardItem::new_string(sanitize_osc52(
                                    &text,
                                )));
                            }

                            for notification in
                                std::mem::take(&mut view.terminal.pending_notifications)
                            {
                                cx.emit(TerminalEvent::ProgramNotification {
                                    title: notification.title,
                                    body: notification.body,
                                });
                            }

                            let is_busy = view.terminal.progress.is_some();
                            if is_busy != was_busy && view.terminal.exited.is_none() {
                                cx.emit(TerminalEvent::AgentProgressChanged { busy: is_busy });
                            }

                            if view.terminal.exited.is_some()
                                && view.terminal.should_close_on_exit()
                            {
                                cx.emit(TerminalEvent::ChildExited);
                            }
                            if view.terminal.title != old_title {
                                cx.emit(TerminalEvent::TitleChanged);
                            }
                            if view.terminal.current_cwd != old_cwd
                                && let Some(ref cwd) = view.terminal.current_cwd
                            {
                                cx.emit(TerminalEvent::CwdChanged(cwd.clone()));
                            }
                            if view.terminal.take_shell_prompt_ready() {
                                cx.emit(TerminalEvent::ShellPromptReady);
                            }

                            view.process_dirty_terminal(cx);
                        })
                    });
                    if result.is_err() {
                        break;
                    }

                    smol::future::yield_now().await;
                }
            },
        )
        .detach();

        if let Some(global) = cx.try_global::<crate::terminal::blink::BlinkPhaseGlobal>() {
            let blink_phase = global.0.clone();
            cx.observe(
                &blink_phase,
                |view: &mut Self, phase, cx: &mut Context<Self>| {
                    if view.terminal.exited.is_some() {
                        return;
                    }
                    let new_visible = resolve_cursor_visible(
                        view.cursor_blink_mode,
                        view.terminal.cursor_blinking,
                        phase.read(cx).visible,
                    );
                    if new_visible != view.cursor_visible {
                        view.cursor_visible = new_visible;
                        if view.was_focused {
                            cx.notify();
                        }
                    }
                },
            )
            .detach();
        } else {
            log::warn!(
                "BlinkPhaseGlobal not installed - cursor will not blink for this TerminalView"
            );
        }

        if let Some(signal) = crate::theme::theme_signal(cx) {
            cx.observe(
                &signal,
                |_view: &mut Self, _signal, cx: &mut Context<Self>| {
                    cx.notify();
                },
            )
            .detach();
        } else {
            log::warn!(
                "ThemeSignalGlobal not installed - this TerminalView will not repaint on a theme change"
            );
        }

        let config = paneflow_config::loader::load_config();
        let terminal_config = config.terminal.clone().unwrap_or_default();
        let scroll_multiplier = terminal_config.resolved_scroll_multiplier();
        let cursor_blink_mode = terminal_config.cursor_blink.unwrap_or_default();
        let default_cursor_shape =
            renderer_cursor_shape_from_config(terminal_config.cursor_shape.unwrap_or_default());
        let cursor_color_override = cursor_color_override_from_config(&terminal_config);
        terminal.session_backend().set_default_cursor(
            engine_cursor_shape(default_cursor_shape),
            matches!(
                cursor_blink_mode,
                paneflow_config::schema::CursorBlinkConfig::On
            ),
        );
        let integrated_glyphs_enabled = terminal_config.resolved_integrated_glyphs();
        let color_emoji_enabled = terminal_config.resolved_color_emoji();
        let minimum_contrast = terminal_config.resolved_minimum_contrast();

        Self {
            terminal,
            focus_handle,
            cursor_visible: true,
            selecting: false,
            cell_width: gpui::px(8.0),
            line_height: gpui::px(16.0),
            element_origin: Arc::new(Mutex::new(gpui::Point::default())),
            layout_cache: Arc::new(Mutex::new(None)),
            scrollbar_metrics: Arc::new(Mutex::new(None)),
            scrollbar_drag: None,
            scroll_remainder: 0.0,
            search_active: false,
            search_input,
            search_query: String::new(),
            search_generation: 0,
            search_cancellation: None,
            search_matches: Vec::new(),
            search_current: 0,
            search_regex_mode: false,
            search_regex_error: None,
            search_truncated: false,
            appearance_theme_generation: crate::theme::theme_generation(),
            option_as_meta: config
                .option_as_meta
                .unwrap_or_else(crate::keys::default_option_as_meta),
            cursor_blink_mode,
            default_cursor_shape,
            cursor_color_override,
            scroll_multiplier,
            integrated_glyphs_enabled,
            color_emoji_enabled,
            minimum_contrast,
            copy_mode_active: false,
            copy_cursor: Point::new(0, 0),
            copy_mode_frozen_offset: 0,
            was_focused: false,
            focus_subscriptions: None,
            ghostty_pressed_keys: std::collections::HashMap::new(),
            ghostty_pending_text_key: None,
            hovered_cell: None,
            ctrl_hovered_link: None,
            link_modifier_held: false,
            hover_link_cache: None,
            mouse_down_link: None,
            ime_marked_text: String::new(),
            needs_initial_clear: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            terminal_window_size: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub(crate) fn display_only_for_test(workspace_id: u64, cx: &mut Context<Self>) -> Self {
        let mut terminal = TerminalState::new_display_only(24, 80);
        drop(terminal.take_backend_events());
        Self::from_terminal_state(workspace_id, terminal, cx)
    }
}

impl TerminalView {
    pub fn set_marked_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.ime_marked_text = text;
        {
            self.ghostty_pending_text_key = None;
        }
        cx.notify();
    }

    pub fn clear_marked_text(&mut self, cx: &mut Context<Self>) {
        self.ime_marked_text.clear();
        cx.notify();
    }

    pub fn commit_text(&mut self, text: &str, _cx: &mut Context<Self>) {
        let was_composing = !self.ime_marked_text.is_empty();
        self.ime_marked_text.clear();
        {
            let pending = if was_composing {
                self.ghostty_pending_text_key.take();
                None
            } else {
                self.ghostty_pending_text_key.take()
            };
            let release_id = pending
                .as_ref()
                .map(|(keystroke, _, _)| keystroke.key.clone());
            let input = pending
                .as_ref()
                .map(|(keystroke, action, prefer_character_input)| {
                    super::input::ghostty_text_key_input(
                        keystroke,
                        *action,
                        *prefer_character_input,
                        text,
                    )
                })
                .unwrap_or_else(|| paneflow_terminal_ghostty::KeyInput {
                    key: paneflow_terminal_ghostty::Key::Unidentified,
                    action: paneflow_terminal_ghostty::KeyAction::Press,
                    modifiers: paneflow_terminal_ghostty::Modifiers::empty(),
                    consumed_modifiers: paneflow_terminal_ghostty::Modifiers::empty(),
                    text: text.to_string(),
                    unshifted_codepoint: None,
                    composing: false,
                });
            let mut release = input.clone();
            release.action = paneflow_terminal_ghostty::KeyAction::Release;
            release.text.clear();
            let result = self.terminal.write_ghostty_key(input);
            if result == super::pty_session::BackendInputResult::Accepted
                && let Some(release_id) = release_id
            {
                self.ghostty_pressed_keys.insert(release_id, release);
            }
        }
    }

    pub fn send_text(&self, text: &str) {
        self.terminal.write_to_pty(text.as_bytes().to_vec());
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.terminal
            .session_backend()
            .modes()
            .contains(Modes::BRACKETED_PASTE)
    }

    const DECLARED_AGENT_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

    pub fn declare_agent(&mut self, agent: crate::agent_launcher::TerminalAgent) {
        self.terminal.detected_agent = Some(agent);
        self.terminal.agent_confirmed = false;
        self.terminal.agent_declared_until =
            std::time::Instant::now().checked_add(Self::DECLARED_AGENT_GRACE);
    }

    pub fn declare_agent_from_command(&mut self, command: &str) {
        if let Some(agent) = crate::agent_launcher::TerminalAgent::from_launch_command(command) {
            self.declare_agent(agent);
        }
    }

    pub fn send_command(&self, command: &str) {
        let mut bytes = command.as_bytes().to_vec();
        bytes.push(b'\r');
        self.terminal.write_to_pty(bytes);
    }

    pub fn send_keystroke(&self, keystroke_str: &str) -> Result<(), String> {
        let keystroke = gpui::Keystroke::parse(keystroke_str).map_err(|e| format!("{e}"))?;
        let mode = self.terminal.session_backend().modes();
        if let Some(seq) = crate::keys::to_esc_str(&keystroke, &mode, self.option_as_meta) {
            if sequence_would_submit(&seq) {
                return Err(format!(
                    "keystroke '{keystroke_str}' would submit (CR/LF); use \
                     surface.send_text with submit=true (`paneflow send --submit`) instead"
                ));
            }
            self.terminal.write_to_pty(seq.as_bytes().to_vec());
        } else if let Some(ref key_char) = keystroke.key_char {
            if sequence_would_submit(key_char) {
                return Err(format!(
                    "keystroke '{keystroke_str}' would submit (CR/LF); use \
                     surface.send_text with submit=true (`paneflow send --submit`) instead"
                ));
            }
            self.terminal.write_to_pty(key_char.as_bytes().to_vec());
        }
        Ok(())
    }

    pub fn marked_text_range(&self) -> Option<std::ops::Range<usize>> {
        if self.ime_marked_text.is_empty() {
            None
        } else {
            let utf16_len: usize = self.ime_marked_text.encode_utf16().count();
            Some(0..utf16_len)
        }
    }
}

fn sequence_would_submit(seq: &str) -> bool {
    seq.contains('\r') || seq.contains('\n')
}

impl TerminalView {
    fn hovered_line_text(&self) -> Option<(Line, String, Vec<usize>)> {
        let point = self.hovered_cell?;
        let line = self.terminal.session_backend().line_text_at(point)?;
        Some((line.line, line.text, line.char_to_column))
    }

    pub(super) fn detect_links_at_hover(&mut self) -> Vec<HyperlinkZone> {
        let Some((line, line_text, char_to_col)) = self.hovered_line_text() else {
            self.hover_link_cache = None;
            return Vec::new();
        };
        let trimmed = line_text.trim_end();
        let trimmed_chars = trimmed.chars().count();
        let map = &char_to_col[..trimmed_chars];
        let cwd_key = self.terminal.current_cwd.clone();
        if let Some(cache) = &self.hover_link_cache
            && cache.line == line
            && cache.cwd == cwd_key
            && cache.line_text == trimmed
        {
            return cache.zones.clone();
        }
        let cwd = cwd_key.as_deref().map(std::path::Path::new);

        let mut zones = crate::terminal::element::detect_urls_on_line_mapped(trimmed, line, map);
        zones.extend(crate::terminal::element::detect_file_paths_on_line_mapped(
            trimmed, line, map, cwd,
        ));
        zones.extend(crate::terminal::element::detect_code_paths_on_line_mapped(
            trimmed, line, map, cwd,
        ));
        self.hover_link_cache = Some(HoverLinkCache {
            line,
            cwd: cwd_key,
            line_text: trimmed.to_string(),
            zones: zones.clone(),
        });
        zones
    }

    #[allow(dead_code)]
    pub fn detect_url_at_hover(&self) -> Vec<HyperlinkZone> {
        let Some((line, line_text, char_to_col)) = self.hovered_line_text() else {
            return Vec::new();
        };
        let trimmed = line_text.trim_end();
        let trimmed_chars = trimmed.chars().count();
        crate::terminal::element::detect_urls_on_line_mapped(
            trimmed,
            line,
            &char_to_col[..trimmed_chars],
        )
    }

    #[allow(dead_code)]
    pub(super) fn detect_file_path_at_hover(&self) -> Vec<HyperlinkZone> {
        let Some((line, line_text, char_to_col)) = self.hovered_line_text() else {
            return Vec::new();
        };
        let trimmed = line_text.trim_end();
        let trimmed_chars = trimmed.chars().count();
        let map = &char_to_col[..trimmed_chars];
        let cwd = self
            .terminal
            .current_cwd
            .as_deref()
            .map(std::path::Path::new);
        crate::terminal::element::detect_file_paths_on_line_mapped(trimmed, line, map, cwd)
    }

    #[allow(dead_code)]
    pub(super) fn detect_code_path_at_hover(&self) -> Vec<HyperlinkZone> {
        let Some((line, line_text, char_to_col)) = self.hovered_line_text() else {
            return Vec::new();
        };
        let trimmed = line_text.trim_end();
        let trimmed_chars = trimmed.chars().count();
        let map = &char_to_col[..trimmed_chars];
        let cwd = self
            .terminal
            .current_cwd
            .as_deref()
            .map(std::path::Path::new);
        crate::terminal::element::detect_code_paths_on_line_mapped(trimmed, line, map, cwd)
    }
}

pub enum TerminalEvent {
    ChildExited,
    TitleChanged,
    CwdChanged(String),
    ShellPromptReady,
    ActivityBurst,
    ServiceDetected(ServiceInfo),
    CancelSwapMode,
    SelectionCopied,
    OpenMarkdownPath(std::path::PathBuf),
    OpenCodePath {
        path: std::path::PathBuf,
        line: Option<u32>,
        col: Option<u32>,
    },
    FontZoomChanged,
    FleetSearchRequested {
        query: String,
        regex: bool,
    },
    AgentProgressChanged {
        busy: bool,
    },
    ProgramNotification {
        title: String,
        body: String,
    },
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl gpui::Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    fn dispatch_context(&self) -> KeyContext {
        let mode = self.terminal.session_backend().modes();
        let mut ctx = KeyContext::default();
        ctx.add("Terminal");

        if mode.contains(Modes::ALT_SCREEN) {
            ctx.set("screen", "alt");
        } else {
            ctx.set("screen", "normal");
        }

        if mode.contains(Modes::APP_CURSOR) {
            ctx.add("DECCKM");
        }
        if mode.contains(Modes::APP_KEYPAD) {
            ctx.add("DECPAM");
        }
        if mode.contains(Modes::BRACKETED_PASTE) {
            ctx.add("bracketed_paste");
        }
        if mode.contains(Modes::FOCUS_IN_OUT) {
            ctx.add("report_focus");
        }
        if mode.contains(Modes::ALTERNATE_SCROLL) {
            ctx.add("alternate_scroll");
        }

        if mode.intersects(Modes::MOUSE_MODE) {
            ctx.add("any_mouse_reporting");
            if mode.contains(Modes::MOUSE_MOTION) {
                ctx.set("mouse_reporting", "motion");
            } else if mode.contains(Modes::MOUSE_DRAG) {
                ctx.set("mouse_reporting", "drag");
            } else {
                ctx.set("mouse_reporting", "click");
            }
        } else {
            ctx.set("mouse_reporting", "off");
        }

        if mode.contains(Modes::SGR_MOUSE) {
            ctx.set("mouse_format", "sgr");
        } else if mode.contains(Modes::UTF8_MOUSE) {
            ctx.set("mouse_format", "utf8");
        } else {
            ctx.set("mouse_format", "normal");
        }

        ctx
    }

    fn render_search_overlay(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        use gpui::{FontWeight, Hsla, MouseButton, hsla, px, svg};

        let ui = crate::theme::ui_colors();

        let regex_active = self.search_regex_mode;
        let has_regex_error = self.search_regex_error.is_some();
        let match_count = self.search_matches.len();
        let has_matches = match_count > 0;
        let current_match = if has_matches {
            self.search_current + 1
        } else {
            0
        };

        let (status_text, status_color) = if has_regex_error {
            ("Invalid regex".to_string(), ui.agent_error)
        } else if self.search_query.is_empty() {
            (String::new(), ui.muted)
        } else if !has_matches {
            ("No results".to_string(), ui.muted)
        } else if self.search_truncated {
            (format!("{current_match}/{match_count}+"), ui.muted)
        } else {
            (format!("{current_match}/{match_count}"), ui.muted)
        };

        let field = div()
            .id("search-field")
            .flex()
            .items_center()
            .min_w(px(160.))
            .max_w(px(320.))
            .text_size(px(13.))
            .text_color(ui.text)
            .child(self.search_input.clone());

        let regex_background = if regex_active {
            ui.subtle
        } else {
            ui.subtle.opacity(0.0)
        };
        let regex_toggle = div()
            .id("search-regex-toggle")
            .flex()
            .items_center()
            .justify_center()
            .size(px(22.))
            .rounded(px(5.))
            .border_1()
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .bg(regex_background)
            .border_color(if regex_active {
                ui.accent
            } else {
                hsla(0., 0., 0., 0.)
            })
            .text_color(if regex_active { ui.text } else { ui.muted })
            .animated_hover(move |style, delta| {
                style.bg(lerp_color(regex_background, ui.subtle, delta));
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.toggle_search_regex(cx)),
            )
            .child(".*");

        let fleet_toggle = div()
            .id("search-fleet-toggle")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .h(px(22.))
            .px(px(7.))
            .rounded(px(5.))
            .text_size(px(12.))
            .text_color(ui.muted)
            .animated_hover(move |style, delta| {
                style.bg(lerp_color(ui.subtle.opacity(0.0), ui.subtle, delta));
            })
            .child(
                svg()
                    .size(px(13.))
                    .flex_none()
                    .path("icons/world.svg")
                    .text_color(ui.muted),
            )
            .child("Fleet")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.request_fleet_search(cx)),
            );

        let icon_btn = |id: &'static str, icon: &'static str, color: Hsla| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded(px(5.))
                .animated_hover(move |style, delta| {
                    style.bg(lerp_color(ui.subtle.opacity(0.0), ui.subtle, delta));
                })
                .child(svg().size(px(14.)).flex_none().path(icon).text_color(color))
        };
        let nav_color = if has_matches {
            ui.muted
        } else {
            ui.muted.opacity(0.35)
        };

        let prev_btn = icon_btn("search-prev", "icons/chevron_up.svg", nav_color).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _window, cx| this.search_prev(cx)),
        );
        let next_btn = icon_btn("search-next", "icons/chevron_down.svg", nav_color).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _window, cx| this.search_next(cx)),
        );
        let close_btn = icon_btn("search-close", "icons/close.svg", ui.muted).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                this.dismiss_search(cx);
                this.focus_handle.clone().focus(window, cx);
            }),
        );

        div()
            .id("search-overlay")
            .occlude()
            .absolute()
            .top_2()
            .right_2()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(8.))
            .bg(ui.overlay)
            .border_1()
            .border_color(ui.border)
            .shadow_lg()
            .child(
                svg()
                    .size(px(15.))
                    .flex_none()
                    .path("icons/tool_search.svg")
                    .text_color(ui.muted),
            )
            .child(field)
            .child(regex_toggle)
            .child(fleet_toggle)
            .when(!status_text.is_empty(), |el| {
                el.child(
                    div()
                        .id("search-status")
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(status_color)
                        .child(status_text.clone()),
                )
            })
            .child(div().flex_none().w(px(1.)).h(px(16.)).bg(ui.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.))
                    .child(prev_btn)
                    .child(next_btn)
                    .child(close_btn),
            )
            .into_any_element()
    }
}

impl TerminalView {
    fn apply_terminal_focus(&mut self, focused: bool) {
        if focused == self.was_focused {
            return;
        }

        self.terminal.set_terminal_focused(focused);
        if !focused {
            self.release_ghostty_pressed_keys();
        }
        let reports_focus = self
            .terminal
            .session_backend()
            .modes()
            .contains(Modes::FOCUS_IN_OUT);
        if reports_focus {
            self.terminal.write_ghostty_focus(if focused {
                paneflow_terminal_ghostty::FocusEvent::Gained
            } else {
                paneflow_terminal_ghostty::FocusEvent::Lost
            });
        }
        self.was_focused = focused;
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_subscriptions.is_none() {
            let focus_handle = self.focus_handle.clone();
            let focus_in = cx.on_focus_in(&focus_handle, window, |view, _window, cx| {
                view.apply_terminal_focus(true);
                cx.notify();
            });
            let focus_out = cx.on_focus_out(&focus_handle, window, |view, _event, _window, cx| {
                view.apply_terminal_focus(false);
                cx.notify();
            });
            self.focus_subscriptions = Some((focus_in, focus_out));
        }

        let focused = self.focus_handle.is_focused(window);
        self.apply_terminal_focus(focused);
        let backend = self.terminal.session_backend();
        let theme_generation = crate::theme::theme_generation();
        if self.appearance_theme_generation != theme_generation && backend.refresh_appearance() {
            self.appearance_theme_generation = theme_generation;
        }
        let terminal_mode = backend.modes();

        let frame_metrics = crate::terminal::element::resolve_frame_metrics(
            window,
            cx,
            self.terminal.font_size_override,
        );
        self.cell_width = frame_metrics.dimensions.cell_width;
        self.line_height = frame_metrics.dimensions.line_height;

        #[cfg(debug_assertions)]
        let keystroke_at = self.terminal.last_keystroke_at.take();

        let search_match_rects = if self.search_active && !self.search_matches.is_empty() {
            self.search_matches
                .iter()
                .enumerate()
                .map(|(i, m)| SearchHighlight {
                    start: m.start,
                    end: m.end,
                    is_active: i == self.search_current,
                })
                .collect()
        } else {
            Vec::new()
        };

        let copy_cursor_state = if self.copy_mode_active {
            let (anchor_grid_line, anchor_col) = backend
                .selection_range()
                .map(|range| (Some(range.start.line.0), range.start.column.0))
                .unwrap_or((None, 0));
            Some(CopyModeCursorState {
                grid_line: self.copy_cursor.line.0,
                col: self.copy_cursor.column.0,
                anchor_grid_line,
                anchor_col,
            })
        } else {
            None
        };

        let alt_screen = terminal_mode.contains(Modes::ALT_SCREEN);
        let cursor_visible = self.cursor_visible || alt_screen;

        let search_rail_lines: Vec<usize> = if self.search_active && !self.search_matches.is_empty()
        {
            let bottom = backend.bottommost_line();
            self.search_matches
                .iter()
                .map(|m| bottom.0.saturating_sub(m.start.line.0).max(0) as usize)
                .collect()
        } else {
            Vec::new()
        };

        let terminal_element = TerminalElement::new(
            self.terminal.session_backend(),
            cursor_visible,
            focused,
            self.terminal.exited,
            self.terminal.exit_signal.clone(),
            self.element_origin.clone(),
            search_match_rects,
            copy_cursor_state,
            self.ctrl_hovered_link
                .as_ref()
                .map(|link| (link.start.line.0, link.start.column.0, link.end.column.0)),
            self.ime_marked_text.clone(),
            self.focus_handle.clone(),
            cx.entity().clone(),
            self.needs_initial_clear.clone(),
            self.terminal_window_size.clone(),
            self.scrollbar_metrics.clone(),
            search_rail_lines,
            self.default_cursor_shape,
            self.cursor_color_override,
            self.integrated_glyphs_enabled,
            self.color_emoji_enabled,
            self.minimum_contrast,
            frame_metrics,
            alt_screen,
            self.layout_cache.clone(),
            #[cfg(debug_assertions)]
            keystroke_at,
        );

        let terminal_body = terminal_element;

        let search_active = self.search_active;

        let mut el = div()
            .id("terminal-view")
            .key_context(self.dispatch_context())
            .track_focus(&self.focus_handle)
            .cursor(if self.ctrl_hovered_link.is_some() {
                gpui::CursorStyle::PointingHand
            } else {
                gpui::CursorStyle::IBeam
            })
            .on_modifiers_changed(cx.listener(Self::handle_modifiers_changed))
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_key_up(cx.listener(Self::handle_key_up))
            .on_any_mouse_down(cx.listener(Self::handle_mouse_down))
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::handle_mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::handle_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::handle_mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::handle_mouse_up))
            .on_action(cx.listener(|this, _: &crate::TerminalCopy, window, cx| {
                this.handle_copy(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::TerminalPaste, window, cx| {
                this.handle_paste(window, cx);
            }))
            .on_scroll_wheel(cx.listener(Self::handle_scroll_wheel))
            .on_action(cx.listener(|this, _: &crate::ScrollPageUp, window, cx| {
                this.handle_scroll_page_up(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::ScrollPageDown, window, cx| {
                this.handle_scroll_page_down(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::JumpPrevPrompt, _window, cx| {
                this.jump_to_prompt(true, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::JumpNextPrompt, _window, cx| {
                this.jump_to_prompt(false, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::ToggleSearch, window, cx| {
                this.toggle_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::DismissSearch, window, cx| {
                this.dismiss_search(cx);
                this.focus_handle.clone().focus(window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &crate::ToggleSearchRegex, _window, cx| {
                    this.toggle_search_regex(cx);
                }),
            )
            .on_action(cx.listener(|this, _: &crate::SearchNext, _window, cx| {
                this.search_next(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::SearchPrev, _window, cx| {
                this.search_prev(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::ToggleCopyMode, _window, cx| {
                this.toggle_copy_mode(cx);
            }))
            .on_action(
                cx.listener(|this, _: &crate::FontSizeIncrease, _window, cx| {
                    this.font_zoom_step(1.0, cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::FontSizeDecrease, _window, cx| {
                    this.font_zoom_step(-1.0, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &crate::FontSizeReset, _window, cx| {
                this.font_zoom_reset(cx);
            }))
            .on_action(
                cx.listener(|this, _: &crate::ToggleFleetSearch, _window, cx| {
                    this.request_fleet_search(cx);
                }),
            )
            .on_drop(cx.listener(Self::handle_file_drop))
            .on_action(
                cx.listener(|this, _: &crate::ClearScrollHistory, _window, cx| {
                    this.clear_scroll_history(cx);
                }),
            )
            .on_action(cx.listener(|this, _: &crate::ResetTerminal, _window, cx| {
                this.reset_terminal(cx);
            }))
            .size_full()
            .child(terminal_body);

        if search_active {
            el = el.key_context("Search");
            el = el.child(self.render_search_overlay(cx));
        }

        if self.copy_mode_active {
            let copy_badge = div()
                .id("copy-mode-badge")
                .absolute()
                .top_1()
                .right_1()
                .px_2()
                .py(gpui::px(2.0))
                .rounded_md()
                .bg(gpui::rgba(0x89b4facc))
                .text_color(gpui::rgb(0x1e1e2e))
                .text_size(gpui::px(11.0))
                .font_weight(gpui::FontWeight::BOLD)
                .child("COPY");
            el = el.child(copy_badge);
        }

        el
    }
}

fn resolve_cursor_visible(
    mode: paneflow_config::schema::CursorBlinkConfig,
    decscusr_blinking: bool,
    phase_visible: bool,
) -> bool {
    use paneflow_config::schema::CursorBlinkConfig as M;
    match mode {
        M::On => phase_visible,
        M::Off => true,
        M::TerminalControlled => {
            if decscusr_blinking {
                phase_visible
            } else {
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::Entity;

    use super::*;

    #[test]
    fn ghostty_start_errors_carry_their_phase_and_reason_code() {
        for (error, phase, reason_code) in [
            (
                GhosttyStartError::Initialization(anyhow::anyhow!("engine")),
                TerminalBackendFailurePhase::Initialization,
                TerminalBackendFailureDiagnostics::GHOSTTY_INITIALIZATION_FAILED,
            ),
            (
                GhosttyStartError::OpenPty(anyhow::anyhow!("pty")),
                TerminalBackendFailurePhase::OpenPty,
                TerminalBackendFailureDiagnostics::GHOSTTY_OPEN_PTY_FAILED,
            ),
            (
                GhosttyStartError::Spawn(anyhow::anyhow!("spawn")),
                TerminalBackendFailurePhase::Spawn,
                TerminalBackendFailureDiagnostics::GHOSTTY_SPAWN_FAILED,
            ),
        ] {
            let failure = classify_ghostty_start_error(error);
            assert_eq!(failure.child_pid, None);
            assert_eq!(failure.diagnostics.phase, phase);
            assert_eq!(failure.diagnostics.reason_code, reason_code);
        }

        let os_error = anyhow::Error::new(std::io::Error::from_raw_os_error(5));
        let failure = classify_ghostty_start_error(GhosttyStartError::PostSpawn {
            child_pid: 4321,
            error: os_error,
        });
        assert_eq!(failure.child_pid, Some(4321));
        assert_eq!(
            failure.diagnostics.phase,
            TerminalBackendFailurePhase::PostSpawn
        );
        assert_eq!(
            failure.diagnostics.reason_code,
            TerminalBackendFailureDiagnostics::GHOSTTY_POST_SPAWN_FAILED
        );
        assert_eq!(failure.diagnostics.os_error, Some(5));
    }

    #[test]
    fn backend_start_failure_reports_once_per_process() {
        let reported = std::sync::atomic::AtomicBool::new(false);
        assert!(claim_backend_failure_report(&reported));
        assert!(!claim_backend_failure_report(&reported));

        let fresh = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(backend_failure_level(&fresh), log::Level::Error);
        assert_eq!(backend_failure_level(&fresh), log::Level::Debug);
        assert_eq!(backend_failure_level(&fresh), log::Level::Debug);
    }

    #[test]
    fn sequence_would_submit_flags_cr_and_lf_only() {
        assert!(sequence_would_submit("\r"));
        assert!(sequence_would_submit("\n"));
        assert!(sequence_would_submit("text\rmore"));
        assert!(!sequence_would_submit("\x1b[A"));
        assert!(!sequence_would_submit("\x03"));
        assert!(!sequence_would_submit("a"));
    }

    #[test]
    fn enter_like_keystrokes_resolve_to_submitting_sequences() {
        for name in ["enter", "ctrl-m", "ctrl-j"] {
            let ks = gpui::Keystroke::parse(name).expect("parse");
            let seq = crate::keys::to_esc_str(&ks, &Modes::empty(), false)
                .unwrap_or_else(|| panic!("{name} must resolve to a sequence"));
            assert!(
                sequence_would_submit(&seq),
                "{name} resolved to {seq:?}, expected a CR/LF sequence"
            );
        }
    }

    #[test]
    fn sanitize_osc52_strips_injection_controls_keeps_tab_and_newline() {
        let dirty = "echo hi\r\x1b[31mX\x1b[0m\u{7f}\u{0085}\tcol\nnext - café 🦀";
        let clean = sanitize_osc52(dirty);
        assert_eq!(clean, "echo hi[31mX[0m\tcol\nnext - café 🦀");
        assert!(
            !clean.contains('\r'),
            "CR (commits a line on paste) removed"
        );
        assert!(!clean.contains('\u{1b}'), "ESC removed");
        assert!(!clean.contains('\u{7f}'), "DEL removed");
        assert!(!clean.contains('\u{85}'), "C1 (NEL) removed");
        assert!(clean.contains('\t') && clean.contains('\n'), "TAB/LF kept");
    }

    #[test]
    fn scrollback_round_trip() {
        let state = TerminalState::new_display_only(3, 80);

        state.restore_scrollback("history one\nhistory two\nvisible three\nvisible four");

        let scrollback = state.extract_scrollback();
        assert!(scrollback.is_some(), "Expected scrollback content");
        let text = scrollback.unwrap();
        assert!(
            text.contains("history one"),
            "Missing 'history one' in: {text}"
        );
        assert!(
            text.contains("history two"),
            "Missing 'history two' in: {text}"
        );
        assert!(!text.contains("visible three"), "Leaked viewport: {text}");
        assert!(!text.contains("visible four"), "Leaked viewport: {text}");
    }

    #[test]
    fn extract_scrollback_empty_terminal_returns_none() {
        let state = TerminalState::new_display_only(24, 80);
        let scrollback = state.extract_scrollback();
        if let Some(ref text) = scrollback {
            assert!(
                text.trim().is_empty(),
                "Expected empty or whitespace-only scrollback, got: {text}"
            );
        }
    }

    const HOST_WINDOW_W: f32 = 800.0;
    const HOST_WINDOW_H: f32 = 600.0;

    struct TerminalHost {
        terminal: Option<Entity<TerminalView>>,
    }

    impl Render for TerminalHost {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let mut root = div().size_full();
            if let Some(terminal) = self.terminal.clone() {
                root = root.child(terminal);
            }
            root
        }
    }

    struct NotifyProbe {
        hits: std::rc::Rc<std::cell::Cell<usize>>,
        _subscription: gpui::Subscription,
    }

    impl NotifyProbe {
        fn hits(&self) -> usize {
            self.hits.get()
        }

        fn reset(&self) {
            self.hits.set(0);
        }
    }

    fn install_blink_phase(
        cx: &mut gpui::TestAppContext,
    ) -> Entity<crate::terminal::blink::BlinkPhase> {
        cx.update(|cx| {
            let phase = cx.new(|_| crate::terminal::blink::BlinkPhase::default());
            cx.set_global(crate::terminal::blink::BlinkPhaseGlobal(phase.clone()));
            phase
        })
    }

    fn hosted_terminal(
        cx: &mut gpui::TestAppContext,
    ) -> (
        Entity<TerminalView>,
        Entity<TerminalHost>,
        &mut gpui::VisualTestContext,
    ) {
        let captured = std::rc::Rc::new(std::cell::RefCell::new(None));
        let sink = captured.clone();
        let (host, cx) = cx.add_window_view(move |_window, cx| {
            let terminal = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
            *sink.borrow_mut() = Some(terminal.clone());
            TerminalHost {
                terminal: Some(terminal),
            }
        });
        cx.update(|window, _cx| window.activate_window());
        cx.simulate_resize(gpui::size(gpui::px(HOST_WINDOW_W), gpui::px(HOST_WINDOW_H)));
        cx.run_until_parked();
        let terminal = captured
            .borrow()
            .clone()
            .expect("the host must build its terminal view");
        (terminal, host, cx)
    }

    fn watch_notifications(
        view: &Entity<TerminalView>,
        cx: &mut gpui::VisualTestContext,
    ) -> NotifyProbe {
        let hits = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let sink = hits.clone();
        let subscription = cx.update(|_window, cx| {
            cx.observe(view, move |_view, _cx| {
                sink.set(sink.get() + 1);
            })
        });
        NotifyProbe {
            hits,
            _subscription: subscription,
        }
    }

    fn focus_terminal(view: &Entity<TerminalView>, cx: &mut gpui::VisualTestContext) {
        let handle = view.read_with(cx, |view, _| view.focus_handle.clone());
        cx.update(|window, cx| handle.focus(window, cx));
        cx.run_until_parked();
    }

    fn link_modifiers() -> gpui::Modifiers {
        #[cfg(target_os = "macos")]
        {
            gpui::Modifiers {
                platform: true,
                ..Default::default()
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            gpui::Modifiers {
                control: true,
                ..Default::default()
            }
        }
    }

    #[gpui::test]
    fn pty_output_notifies_the_terminal_view(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        terminal.update(cx, |view, cx| {
            view.terminal.write_output(b"paneflow output\n");
            view.apply_backend_wakeup(cx);
        });
        cx.run_until_parked();

        assert!(
            probe.hits() > 0,
            "pty output: the terminal view must notify itself"
        );
    }

    #[gpui::test]
    fn a_kitty_image_notifies_the_terminal_view(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        terminal.update(cx, |view, cx| {
            view.terminal
                .write_output(b"\x1b_Gf=24,s=1,v=1,a=T;AAAA\x1b\\");
            view.apply_backend_wakeup(cx);
        });
        cx.run_until_parked();

        assert!(
            probe.hits() > 0,
            "kitty image: the terminal view must notify itself"
        );
    }

    #[gpui::test]
    fn a_resize_notifies_the_terminal_view(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        terminal.update(cx, |view, cx| {
            view.terminal
                .notify_window_size(TerminalWindowSize::new(120, 40, 8, 16));
            view.apply_backend_wakeup(cx);
        });
        cx.run_until_parked();

        assert!(
            probe.hits() > 0,
            "resize: the terminal view must notify itself"
        );
    }

    #[gpui::test]
    fn the_process_exit_banner_notifies_the_terminal_view(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        terminal.update(cx, |view, cx| {
            view.terminal.exited = Some(0);
            view.apply_backend_wakeup(cx);
        });
        cx.run_until_parked();

        assert!(
            probe.hits() > 0,
            "process exit banner: the terminal view must notify itself"
        );
    }

    #[gpui::test]
    fn a_mouse_selection_notifies_the_terminal_view(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        cx.simulate_mouse_down(
            gpui::point(gpui::px(120.0), gpui::px(120.0)),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();

        assert!(
            terminal.read_with(cx, |view, _| view.selecting),
            "mouse selection: the press must arm a selection"
        );
        assert!(
            probe.hits() > 0,
            "mouse selection: the terminal view must notify itself"
        );
    }

    #[gpui::test]
    fn a_hovered_link_notifies_the_terminal_view(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        focus_terminal(&terminal, cx);
        cx.simulate_mouse_move(
            gpui::point(gpui::px(120.0), gpui::px(120.0)),
            None,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        cx.simulate_modifiers_change(link_modifiers());
        cx.run_until_parked();

        assert!(
            terminal.read_with(cx, |view, _| view.link_modifier_held),
            "hovered link: the open-link modifier must be recorded"
        );
        assert!(
            probe.hits() > 0,
            "hovered link: the terminal view must notify itself"
        );
    }

    #[gpui::test]
    fn search_highlighting_notifies_the_terminal_view(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        terminal.update(cx, |view, _cx| {
            view.search_active = true;
            view.search_query = "paneflow".into();
            view.search_matches = vec![
                crate::search::SearchMatch {
                    start: Point::new(0, 0),
                    end: Point::new(0, 7),
                },
                crate::search::SearchMatch {
                    start: Point::new(1, 0),
                    end: Point::new(1, 7),
                },
            ];
            view.search_current = 0;
        });
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        terminal.update(cx, |view, cx| view.search_next(cx));
        cx.run_until_parked();

        assert_eq!(
            terminal.read_with(cx, |view, _| view.search_current),
            1,
            "search highlighting: the active match must advance"
        );
        assert!(
            probe.hits() > 0,
            "search highlighting: the terminal view must notify itself"
        );
    }

    #[gpui::test]
    fn a_caret_phase_change_notifies_the_focused_terminal_view(cx: &mut gpui::TestAppContext) {
        let phase = install_blink_phase(cx);
        let (terminal, _host, cx) = hosted_terminal(cx);
        focus_terminal(&terminal, cx);
        terminal.update(cx, |view, _cx| {
            view.cursor_blink_mode = paneflow_config::schema::CursorBlinkConfig::On;
            view.cursor_visible = true;
        });
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        cx.update(|_window, cx| {
            phase.update(cx, |phase, cx| {
                phase.visible = false;
                cx.notify();
            })
        });
        cx.run_until_parked();

        assert!(
            !terminal.read_with(cx, |view, _| view.cursor_visible),
            "caret phase: the focused view must follow the blink phase"
        );
        assert!(
            probe.hits() > 0,
            "caret phase: the terminal view must notify itself"
        );
    }

    #[gpui::test]
    fn a_theme_change_notifies_the_terminal_view(cx: &mut gpui::TestAppContext) {
        cx.update(crate::theme::install_theme_signal);
        let (terminal, _host, cx) = hosted_terminal(cx);
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        cx.update(|_window, cx| {
            crate::theme::invalidate_theme_cache();
            crate::theme::publish_theme_generation(cx);
        });
        cx.run_until_parked();

        assert!(
            probe.hits() > 0,
            "theme change: the terminal view must notify itself"
        );
    }

    #[gpui::test]
    fn an_idle_unfocused_terminal_stays_silent_across_sixty_frames(cx: &mut gpui::TestAppContext) {
        let phase = install_blink_phase(cx);
        let (terminal, _host, cx) = hosted_terminal(cx);
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        for frame in 0..60 {
            cx.update(|_window, cx| {
                phase.update(cx, |phase, cx| {
                    phase.visible = frame % 2 == 0;
                    cx.notify();
                })
            });
            cx.run_until_parked();
        }

        assert!(
            !terminal.read_with(cx, |view, _| view.was_focused),
            "idle terminal: the view must stay unfocused"
        );
        assert_eq!(
            probe.hits(),
            0,
            "idle terminal: an unfocused terminal without output or hover must not notify"
        );
    }

    #[gpui::test]
    fn a_closed_pane_stops_notifying_after_its_exit_banner(cx: &mut gpui::TestAppContext) {
        let (terminal, host, cx) = hosted_terminal(cx);
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        terminal.update(cx, |view, cx| {
            view.terminal.exited = Some(0);
            view.apply_backend_wakeup(cx);
        });
        cx.run_until_parked();
        assert!(
            probe.hits() > 0,
            "closed pane: the exit banner must notify before the pane closes"
        );

        let weak = terminal.downgrade();
        drop(terminal);
        host.update(cx, |host, cx| {
            host.terminal = None;
            cx.notify();
        });
        cx.run_until_parked();
        probe.reset();

        let outcome = cx.update(|_window, cx| {
            weak.update(cx, |view, cx| {
                view.apply_backend_wakeup(cx);
            })
        });

        assert!(
            outcome.is_err(),
            "closed pane: the released view must not be updatable"
        );
        assert_eq!(
            probe.hits(),
            0,
            "closed pane: a released terminal view must not notify"
        );
    }

    #[test]
    fn cursor_blink_override_resolves_correctly() {
        use paneflow_config::schema::CursorBlinkConfig as M;
        assert!(resolve_cursor_visible(M::On, false, true));
        assert!(!resolve_cursor_visible(M::On, false, false));
        assert!(resolve_cursor_visible(M::Off, true, false));
        assert!(!resolve_cursor_visible(M::TerminalControlled, true, false));
        assert!(resolve_cursor_visible(M::TerminalControlled, true, true));
        assert!(resolve_cursor_visible(M::TerminalControlled, false, false));
    }
}
