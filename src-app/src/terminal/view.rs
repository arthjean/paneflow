use std::sync::{Arc, Mutex};

use futures::StreamExt;
use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement,
    IntoElement, KeyContext, MouseButton, Render, Styled, Window, div, prelude::*,
};
use paneflow_config::schema::{TerminalConfig, TerminalSurfaceProfile};

use super::TerminalState;
use super::element::TerminalElement;
use super::host_link::{
    self, AttachRequest, FinalText, HostLinkEnd, HostLinkState, HostedAttachment, ResolveOutcome,
    SessionIntent,
};
use super::pty_session::{
    TerminalBackendEvents, TerminalBackendFailureDiagnostics, TerminalBackendFailurePhase,
    raw_os_error_from_anyhow,
};
use super::service_detector::ServiceInfo;
use super::types::{
    CopyModeCursorState, CursorShape, HyperlinkZone, Line, Modes, Point, SearchHighlight,
    TerminalWindowSize,
};

use super::ghostty_session::GhosttyStartError;
use super::path_picker::{PathPicker, PathPickerEvent};

pub(crate) mod conversation;
mod host_attach;

const RENDER_WAKEUP_IMMEDIATELY: bool = true;
const SEARCH_REGEX_TOGGLE_LABEL: &str = "Regular expression";
const COPY_MODE_BADGE_LABEL: &str = "Copy mode";

fn search_regex_toggle(
    active: bool,
    ui: crate::theme::UiColors,
    active_background: gpui::Hsla,
) -> gpui::Stateful<gpui::Div> {
    use gpui::{FontWeight, px};

    use crate::ui_primitives::AccessibleControlExt;

    div()
        .id("search-regex-toggle")
        .accessible_control(gpui::accesskit::Role::Button, SEARCH_REGEX_TOGGLE_LABEL)
        .aria_toggled(if active {
            gpui::accesskit::Toggled::True
        } else {
            gpui::accesskit::Toggled::False
        })
        .flex_none()
        .px(px(4.))
        .rounded(px(4.))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active {
            ui.text
        } else {
            ui.muted.opacity(0.6)
        })
        .when(active, |toggle| toggle.bg(active_background))
        .child(".*")
}

fn copy_mode_badge() -> gpui::Stateful<gpui::Div> {
    use crate::ui_primitives::TooltipDelayExt;

    div()
        .id("copy-mode-badge")
        .role(gpui::accesskit::Role::Status)
        .aria_label(COPY_MODE_BADGE_LABEL)
        .delayed_tooltip(crate::ui_primitives::text_tooltip(COPY_MODE_BADGE_LABEL))
}

#[cfg(debug_assertions)]
pub(crate) fn probe_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("PANEFLOW_LATENCY_PROBE").as_deref() == Ok("1"))
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

struct PathPickerSlot {
    picker: gpui::Entity<PathPicker>,
    _events: gpui::Subscription,
}

#[derive(Clone)]
pub(super) struct HoverLinkCache {
    line: Line,
    cwd: Option<String>,
    line_text: String,
    zones: Vec<HyperlinkZone>,
}

pub(super) type HoverLinkResolver =
    fn(&str, Line, &[usize], Option<&std::path::Path>) -> Vec<HyperlinkZone>;

pub(super) fn resolve_hover_links(
    line_text: &str,
    line: Line,
    char_to_column: &[usize],
    cwd: Option<&std::path::Path>,
) -> Vec<HyperlinkZone> {
    let mut zones =
        crate::terminal::element::detect_urls_on_line_mapped(line_text, line, char_to_column);
    zones.extend(crate::terminal::element::detect_file_paths_on_line_mapped(
        line_text,
        line,
        char_to_column,
        cwd,
    ));
    zones.extend(crate::terminal::element::detect_code_paths_on_line_mapped(
        line_text,
        line,
        char_to_column,
        cwd,
    ));
    zones
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
    pub(super) scrollbar_reveal: super::scrollbar_reveal::ScrollbarReveal,
    pub(super) scrollbar_hide_scheduled: bool,
    pub(super) scrollbar_seen_offset: usize,
    pub(super) scrollbar_visible: bool,
    pub(super) scrollbar_enabled: bool,
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
    pub(super) search_anchor_topmost: Line,
    pub(super) search_seen_output_generation: u64,
    pub(super) search_scan_in_flight: bool,
    pub(super) search_refresh_dirty: bool,
    pub(super) search_native_pending: Option<String>,
    pub(super) search_native_retry_scheduled: bool,
    pub(super) search_native_snapshot: Option<Arc<crate::search::NativeSearchState>>,
    pub(super) search_native_navigation_in_flight: Option<u64>,
    pub(super) search_native_navigation_generation: u64,
    pub(super) search_native_navigation_queue: std::collections::VecDeque<bool>,
    path_picker: Option<PathPickerSlot>,
    pub(super) clipboard_image_dir: Option<std::path::PathBuf>,
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
    focus_subscriptions: Option<(gpui::WindowId, gpui::Subscription, gpui::Subscription)>,
    pub(super) ghostty_pressed_keys:
        std::collections::HashMap<String, paneflow_terminal_ghostty::KeyInput>,
    pub(super) ghostty_pending_text_key:
        Option<(gpui::Keystroke, paneflow_terminal_ghostty::KeyAction, bool)>,
    pub(super) hovered_cell: Option<Point>,
    pub(super) ctrl_hovered_link: Option<HyperlinkZone>,
    pub(super) link_modifier_held: bool,
    pub(super) hover_link_cache: Option<HoverLinkCache>,
    pub(super) hover_link_resolver: HoverLinkResolver,
    pub(super) mouse_down_link: Option<HyperlinkZone>,
    pub(super) mouse_down_cell: Option<Point>,
    ime_marked_text: String,
    needs_initial_clear: Arc<std::sync::atomic::AtomicBool>,
    terminal_window_size: Arc<Mutex<Option<TerminalWindowSize>>>,
    launch: HostedLaunch,
    session_intent: SessionIntent,
    saved_scrollback: Option<String>,
    pump_epoch: u64,
    exit_announced: bool,
    relaunch_pending: bool,
    conversation: conversation::Conversation,
}

impl TerminalView {
    pub(crate) fn search_active(&self) -> bool {
        self.search_active
    }

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

    pub(crate) fn defer_restore_scrollback(&mut self, text: String) {
        self.saved_scrollback = Some(text);
    }

    fn restore_saved_scrollback(&mut self) {
        if let Some(text) = self.saved_scrollback.take() {
            self.restore_scrollback(&text);
        }
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

    pub(crate) fn set_option_as_meta(&mut self, option_as_meta: bool) {
        if self.option_as_meta != option_as_meta {
            self.option_as_meta = option_as_meta;
            self.terminal
                .session_backend()
                .set_option_as_alt(option_as_meta);
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
        Self::open(
            HostedLaunch {
                workspace_id,
                cwd,
                initial_size,
                user_env,
                profile,
                confine_to: None,
                fallback_to: None,
            },
            SessionIntent::Create,
            None,
            cx,
        )
    }

    pub(crate) fn spawned(
        workspace_id: u64,
        spawn: crate::workspace::SpawnCwd,
        user_env: Option<std::collections::HashMap<String, String>>,
        profile: TerminalSurfaceProfile,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::open(
            HostedLaunch {
                workspace_id,
                cwd: spawn.cwd,
                initial_size: None,
                user_env,
                profile,
                confine_to: spawn.confine_to,
                fallback_to: spawn.fallback,
            },
            SessionIntent::Create,
            None,
            cx,
        )
    }

    pub(crate) fn attach_restored(
        workspace_id: u64,
        cwd: Option<std::path::PathBuf>,
        user_env: Option<std::collections::HashMap<String, String>>,
        session: paneflow_config::schema::SessionId,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::open(
            HostedLaunch {
                workspace_id,
                cwd,
                initial_size: None,
                user_env,
                profile: TerminalSurfaceProfile::Normal,
                confine_to: None,
                fallback_to: None,
            },
            SessionIntent::Reattach,
            Some(session),
            cx,
        )
    }

    pub(crate) fn attach_existing(
        workspace_id: u64,
        cwd: Option<std::path::PathBuf>,
        session: paneflow_config::schema::SessionId,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::attach_restored(workspace_id, cwd, None, session, cx)
    }

    pub(crate) fn attach_restarting(
        workspace_id: u64,
        cwd: Option<std::path::PathBuf>,
        session: paneflow_config::schema::SessionId,
        expected: Option<paneflow_config::schema::SessionGeneration>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::open(
            HostedLaunch {
                workspace_id,
                cwd,
                initial_size: None,
                user_env: None,
                profile: TerminalSurfaceProfile::Normal,
                confine_to: None,
                fallback_to: None,
            },
            SessionIntent::Restart { expected },
            Some(session),
            cx,
        )
    }

    fn open(
        launch: HostedLaunch,
        intent: SessionIntent,
        session: Option<paneflow_config::schema::SessionId>,
        cx: &mut Context<Self>,
    ) -> Self {
        let surface_id = cx.entity_id().as_u64();
        let (params, shell_notice) =
            launch.spawn_params(surface_id, &crate::config_snapshot::current(cx));
        let (mut terminal, pending) = TerminalState::new_pending_with_shell_quoting(
            params.cols,
            params.rows,
            params.shell_quoting,
        );
        terminal.pending_host_notices.extend(shell_notice);
        if let Some(session) = session {
            terminal.session_id = session;
        }
        let view = Self::from_terminal_state(launch, terminal, cx);
        view.begin_hosted_attach(intent, params, pending, cx);
        view
    }

    fn from_terminal_state(
        launch: HostedLaunch,
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

        spawn_event_pump_task(terminal.take_backend_events(), 0, cx);

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

        let config = crate::config_snapshot::current(cx);
        let terminal_config = config.terminal.clone().unwrap_or_default();
        terminal.osc52_policy = terminal_config.osc52_clipboard.unwrap_or_default();
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
        let option_as_meta = config
            .option_as_meta
            .unwrap_or_else(crate::keys::default_option_as_meta);
        terminal.session_backend().set_option_as_alt(option_as_meta);
        let integrated_glyphs_enabled = terminal_config.resolved_integrated_glyphs();
        let color_emoji_enabled = terminal_config.resolved_color_emoji();
        let minimum_contrast = terminal_config.resolved_minimum_contrast();
        let scrollbar_enabled = terminal_config.resolved_scrollbar_visible();

        Self {
            terminal,
            focus_handle,
            cursor_visible: true,
            selecting: false,
            cell_width: gpui::px(8.0),
            line_height: gpui::px(16.0),
            element_origin: Arc::new(Mutex::new(gpui::Point::default())),
            layout_cache: Arc::default(),
            scrollbar_metrics: Arc::new(Mutex::new(None)),
            scrollbar_drag: None,
            scrollbar_reveal: super::scrollbar_reveal::ScrollbarReveal::default(),
            scrollbar_hide_scheduled: false,
            scrollbar_seen_offset: 0,
            scrollbar_visible: false,
            scrollbar_enabled,
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
            search_anchor_topmost: Line(0),
            search_seen_output_generation: 0,
            search_scan_in_flight: false,
            search_refresh_dirty: false,
            search_native_pending: None,
            search_native_retry_scheduled: false,
            search_native_snapshot: None,
            search_native_navigation_in_flight: None,
            search_native_navigation_generation: 0,
            search_native_navigation_queue: std::collections::VecDeque::new(),
            path_picker: None,
            clipboard_image_dir: super::clipboard_image::default_dir(),
            appearance_theme_generation: crate::theme::theme_generation(),
            option_as_meta,
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
            hover_link_resolver: resolve_hover_links,
            mouse_down_link: None,
            mouse_down_cell: None,
            ime_marked_text: String::new(),
            needs_initial_clear: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            terminal_window_size: Arc::new(Mutex::new(None)),
            launch,
            session_intent: SessionIntent::Create,
            saved_scrollback: None,
            pump_epoch: 0,
            exit_announced: false,
            relaunch_pending: false,
            conversation: conversation::Conversation::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn launch_workspace_id(&self) -> u64 {
        self.launch.workspace_id
    }

    pub(crate) fn move_to_workspace(&mut self, workspace_id: u64) {
        self.launch.workspace_id = workspace_id;
    }

    #[cfg(test)]
    pub(crate) fn display_only_for_test(workspace_id: u64, cx: &mut Context<Self>) -> Self {
        let mut terminal = TerminalState::new_display_only(24, 80);
        drop(terminal.take_backend_events());
        Self::from_terminal_state(
            HostedLaunch {
                workspace_id,
                cwd: None,
                initial_size: None,
                user_env: None,
                profile: TerminalSurfaceProfile::Normal,
                confine_to: None,
                fallback_to: None,
            },
            terminal,
            cx,
        )
    }
}

#[derive(Clone)]
struct HostedLaunch {
    workspace_id: u64,
    cwd: Option<std::path::PathBuf>,
    initial_size: Option<(usize, usize)>,
    user_env: Option<std::collections::HashMap<String, String>>,
    profile: TerminalSurfaceProfile,
    confine_to: Option<std::path::PathBuf>,
    fallback_to: Option<std::path::PathBuf>,
}

impl HostedLaunch {
    fn spawn_params(
        &self,
        surface_id: u64,
        config: &paneflow_config::schema::PaneFlowConfig,
    ) -> (crate::terminal::pty_session::SpawnParams, Option<String>) {
        TerminalState::resolve_spawn_launch(
            config,
            self.cwd.clone(),
            self.workspace_id,
            surface_id,
            self.initial_size,
            self.user_env.clone(),
            self.profile,
        )
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
        let _ = self.write_text(text);
    }

    pub(crate) fn write_text(&self, text: &str) -> Result<(), &'static str> {
        input_outcome(self.terminal.write_to_pty(text.as_bytes().to_vec()))
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.terminal
            .session_backend()
            .modes()
            .contains(Modes::BRACKETED_PASTE)
    }

    const DECLARED_AGENT_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

    pub fn declare_launched_agent(&mut self, agent: crate::agent_launcher::TerminalAgent) {
        self.terminal.bind_runtime(Some(agent.runtime().id));
        self.declare_agent(agent);
    }

    pub fn declare_agent(&mut self, agent: crate::agent_launcher::TerminalAgent) {
        self.terminal.detected_agent = Some(agent);
        self.terminal.agent_confirmed = false;
        self.terminal.agent_declared_until =
            std::time::Instant::now().checked_add(Self::DECLARED_AGENT_GRACE);
    }

    pub fn declare_agent_from_command(&mut self, command: &str) {
        if let Some(agent) = crate::agent_launcher::TerminalAgent::from_launch_command(command) {
            self.declare_launched_agent(agent);
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
            input_outcome(self.terminal.write_to_pty(seq.as_bytes().to_vec()))
                .map_err(str::to_owned)
        } else if let Some(ref key_char) = keystroke.key_char {
            if sequence_would_submit(key_char) {
                return Err(format!(
                    "keystroke '{keystroke_str}' would submit (CR/LF); use \
                     surface.send_text with submit=true (`paneflow send --submit`) instead"
                ));
            }
            input_outcome(self.terminal.write_to_pty(key_char.as_bytes().to_vec()))
                .map_err(str::to_owned)
        } else {
            Err(format!("keystroke '{keystroke_str}' produces no input"))
        }
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

fn link_under(zones: &[HyperlinkZone], point: Point) -> Option<HyperlinkZone> {
    zones
        .iter()
        .find(|zone| {
            point.line == zone.start.line
                && point.column >= zone.start.column
                && point.column <= zone.end.column
        })
        .cloned()
}

fn sequence_would_submit(seq: &str) -> bool {
    seq.contains('\r') || seq.contains('\n')
}

pub(crate) const INPUT_REJECTED: &str =
    "the pane did not accept the input: it has no input, exited, or its input queue is full";

pub(crate) fn input_outcome(
    result: super::pty_session::BackendInputResult,
) -> Result<(), &'static str> {
    match result {
        super::pty_session::BackendInputResult::Accepted => Ok(()),
        super::pty_session::BackendInputResult::Rejected => Err(INPUT_REJECTED),
    }
}

impl TerminalView {
    fn hovered_line_text(&self) -> Option<(Line, String, Vec<usize>)> {
        let point = self.hovered_cell?;
        let line = self.terminal.session_backend().line_text_at(point)?;
        Some((line.line, line.text, line.char_to_column))
    }

    pub(super) fn resolve_links_at_hover(&mut self, hover_point: Point, cx: &mut Context<Self>) {
        let Some((line, mut line_text, mut char_to_col)) = self.hovered_line_text() else {
            self.hover_link_cache = None;
            self.ctrl_hovered_link = None;
            return;
        };
        let trimmed_len = line_text.trim_end().len();
        line_text.truncate(trimmed_len);
        char_to_col.truncate(line_text.chars().count());
        let cwd = self.terminal.current_cwd.clone();
        if let Some(cache) = &self.hover_link_cache
            && cache.line == line
            && cache.cwd == cwd
            && cache.line_text == line_text
        {
            self.ctrl_hovered_link = link_under(&cache.zones, hover_point);
            return;
        }
        self.ctrl_hovered_link = None;
        let resolver = self.hover_link_resolver;
        let resolved = cx.background_executor().spawn({
            let line_text = line_text.clone();
            let cwd = cwd.clone();
            async move {
                resolver(
                    &line_text,
                    line,
                    &char_to_col,
                    cwd.as_deref().map(std::path::Path::new),
                )
            }
        });
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let zones = resolved.await;
                let _ = this.update(cx, |view, cx| {
                    view.hover_link_cache = Some(HoverLinkCache {
                        line,
                        cwd,
                        line_text,
                        zones,
                    });
                    if !view.link_modifier_held || view.hovered_cell != Some(hover_point) {
                        return;
                    }
                    let link = view
                        .hover_link_cache
                        .as_ref()
                        .and_then(|cache| link_under(&cache.zones, hover_point));
                    if link.is_some() {
                        view.ctrl_hovered_link = link;
                        cx.notify();
                    }
                });
            },
        )
        .detach();
    }
}

pub enum TerminalEvent {
    ChildExited,
    HostLinkResolved,
    TitleChanged,
    CwdChanged(String),
    ShellPromptReady,
    ActivityBurst,
    ServiceDetected(ServiceInfo),
    CancelSwapMode,
    SelectionCopied,
    Notice(String),
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
    ProgramNotification {
        title: String,
        body: String,
    },
    AgentSessionChanged,
    ConversationReady,
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
        if self.search_active {
            ctx.add("Search");
        }

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
        use gpui::{Hsla, px, svg};

        use crate::settings::components::with_alpha;
        use crate::ui_primitives::{AccessibleControlExt, ROW_RADIUS, squircle_skin};

        let ui = crate::theme::ui_colors();

        let regex_active = self.search_regex_mode;
        let has_regex_error = self.search_regex_error.is_some();
        let match_count = self.search_match_count();
        let has_matches = match_count > 0;
        let current_match = if has_matches {
            self.search_current + 1
        } else {
            0
        };

        let (status_text, status_color) = if has_regex_error {
            (
                if regex_active {
                    "Invalid regex"
                } else {
                    "Search failed"
                }
                .to_string(),
                ui.agent_error,
            )
        } else if self.search_query.is_empty() {
            (String::new(), ui.muted)
        } else if !has_matches && self.search_scan_in_flight {
            ("Searching...".to_string(), ui.muted)
        } else if !has_matches && self.search_truncated {
            ("Search incomplete".to_string(), ui.muted)
        } else if !has_matches {
            ("No results".to_string(), ui.muted)
        } else if self.search_truncated {
            (format!("{current_match}/{match_count}+"), ui.muted)
        } else {
            (format!("{current_match}/{match_count}"), ui.muted)
        };

        let button_hover = crate::app::constants::sidebar_tab_hover_background();

        let field = div()
            .id("search-field")
            .flex()
            .items_center()
            .flex_1()
            .min_w(px(0.))
            .text_size(px(14.))
            .text_color(ui.text)
            .child(self.search_input.clone());

        let icon_btn =
            move |id: &'static str, label: &'static str, icon: &'static str, color: Hsla| {
                squircle_skin(
                    div()
                        .id(id)
                        .accessible_control(gpui::accesskit::Role::Button, label)
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(28.)),
                    id,
                    ROW_RADIUS,
                    None,
                    Some(button_hover),
                )
                .child(svg().size(px(14.)).flex_none().path(icon).text_color(color))
            };
        let nav_color = if has_matches {
            ui.muted
        } else {
            ui.muted.opacity(0.35)
        };

        let prev_btn = icon_btn(
            "search-prev",
            "Previous match",
            "icons/chevron_up.svg",
            nav_color,
        )
        .on_pointer_press(cx.listener(|this, _, _window, cx| this.search_prev(cx)));
        let next_btn = icon_btn(
            "search-next",
            "Next match",
            "icons/chevron_down.svg",
            nav_color,
        )
        .on_pointer_press(cx.listener(|this, _, _window, cx| this.search_next(cx)));
        let close_btn = icon_btn("search-close", "Close search", "icons/close.svg", ui.muted)
            .on_pointer_press(cx.listener(|this, _, window, cx| {
                this.dismiss_search(cx);
                this.focus_handle.clone().focus(window, cx);
            }));
        let regex_toggle = search_regex_toggle(regex_active, ui, button_hover)
            .on_pointer_press(cx.listener(|this, _, _window, cx| this.toggle_search_regex(cx)));

        squircle_skin(
            div()
                .id("search-overlay")
                .occlude()
                .flex()
                .flex_row()
                .items_center()
                .w(px(325.))
                .h(px(36.))
                .pl(px(14.))
                .pr(px(4.))
                .gap(px(8.)),
            "search-overlay",
            ROW_RADIUS,
            Some(ui.subtle),
            None,
        )
        .absolute()
        .top_2()
        .right_2()
        .child(crate::ui_primitives::squircle::squircle_border(
            ROW_RADIUS,
            px(1.),
            ui.border,
        ))
        .child(field)
        .child(regex_toggle)
        .when(!status_text.is_empty(), |el| {
            el.child(
                div()
                    .id("search-status")
                    .flex_none()
                    .text_size(px(13.))
                    .text_color(status_color)
                    .child(status_text.clone()),
            )
        })
        .child(
            div()
                .flex_none()
                .w(px(1.))
                .h(px(16.))
                .bg(with_alpha(ui.text, 0.12)),
        )
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
    fn open_path_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(slot) = &self.path_picker {
            let focus = slot.picker.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            return;
        }
        let cwd = self.terminal.current_cwd.clone();
        let quoting = self.terminal.shell_quoting;
        let anchor = self.cursor_cell_bounds();
        let picker = cx.new(|cx| PathPicker::new(cwd, quoting, anchor, window, cx));
        let events = cx.subscribe_in(&picker, window, Self::handle_path_picker_event);
        let focus = picker.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.path_picker = Some(PathPickerSlot {
            picker,
            _events: events,
        });
        cx.notify();
    }

    fn handle_path_picker_event(
        &mut self,
        picker: &gpui::Entity<PathPicker>,
        event: &PathPickerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .path_picker
            .as_ref()
            .is_none_or(|slot| slot.picker != *picker)
        {
            return;
        }
        self.path_picker = None;
        match event {
            PathPickerEvent::Picked(text) => {
                self.write_paste_text(text);
                self.focus_handle.focus(window, cx);
            }
            PathPickerEvent::Dismissed { refocus: true } => self.focus_handle.focus(window, cx),
            PathPickerEvent::Dismissed { refocus: false } => {}
        }
        cx.notify();
    }

    fn cursor_cell_bounds(&self) -> gpui::Bounds<gpui::Pixels> {
        let origin = *self
            .element_origin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let metrics = self.terminal.session_backend().grid_metrics();
        let last_row = metrics.screen_lines.saturating_sub(1) as i64;
        let row =
            (i64::from(metrics.cursor.line.0) + metrics.display_offset as i64).clamp(0, last_row);
        let column = metrics
            .cursor
            .column
            .0
            .min(metrics.columns.saturating_sub(1));
        gpui::Bounds::new(
            gpui::point(
                origin.x + self.cell_width * column as f32,
                origin.y + self.line_height * row as f32,
            ),
            gpui::size(self.cell_width, self.line_height),
        )
    }

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

fn spawn_event_pump_task(
    events_rx: TerminalBackendEvents,
    epoch: u64,
    cx: &mut Context<TerminalView>,
) {
    cx.spawn(
        async move |this: gpui::WeakEntity<TerminalView>, cx: &mut gpui::AsyncApp| {
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
                        this.update(
                            cx,
                            |view: &mut TerminalView, cx: &mut Context<TerminalView>| {
                                if view.pump_epoch != epoch {
                                    return false;
                                }
                                view.apply_backend_wakeup(cx);
                                true
                            },
                        )
                    });
                    if !matches!(result, Ok(true)) {
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
                    this.update(
                        cx,
                        |view: &mut TerminalView, cx: &mut Context<TerminalView>| {
                            if view.pump_epoch != epoch {
                                return false;
                            }
                            let old_title = view.terminal.title.clone();
                            let old_cwd = view.terminal.current_cwd.clone();
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

                            for notice in std::mem::take(&mut view.terminal.pending_host_notices) {
                                cx.emit(TerminalEvent::Notice(notice));
                            }
                            for notification in
                                std::mem::take(&mut view.terminal.pending_notifications)
                            {
                                cx.emit(TerminalEvent::ProgramNotification {
                                    title: notification.title,
                                    body: notification.body,
                                });
                            }

                            if view.terminal.retains_final_view() && !view.exit_announced {
                                view.exit_announced = true;
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
                            view.relaunch_when_ended(cx);
                            true
                        },
                    )
                });
                if !matches!(result, Ok(true)) {
                    break;
                }

                smol::future::yield_now().await;
            }
        },
    )
    .detach();
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let window_id = window.window_handle().window_id();
        if self.focus_subscriptions.as_ref().map(|binding| binding.0) != Some(window_id) {
            if self.focus_subscriptions.take().is_some() {
                self.apply_terminal_focus(false);
                self.release_ghostty_pressed_keys();
                self.selecting = false;
                self.scrollbar_drag = None;
                self.scroll_remainder = 0.0;
                self.hovered_cell = None;
                self.ctrl_hovered_link = None;
                self.link_modifier_held = false;
                self.hover_link_cache = None;
                self.mouse_down_link = None;
                self.mouse_down_cell = None;
                self.ime_marked_text.clear();
                self.path_picker = None;
                *self
                    .element_origin
                    .lock()
                    .unwrap_or_else(|err| err.into_inner()) = gpui::Point::default();
                *self
                    .scrollbar_metrics
                    .lock()
                    .unwrap_or_else(|err| err.into_inner()) = None;
                *self
                    .layout_cache
                    .lock()
                    .unwrap_or_else(|err| err.into_inner()) =
                    crate::terminal::element::TerminalRenderCache::default();
            }
            let focus_handle = self.focus_handle.clone();
            let focus_in = cx.on_focus_in(&focus_handle, window, |view, _window, cx| {
                view.apply_terminal_focus(true);
                cx.notify();
            });
            let focus_out = cx.on_focus_out(&focus_handle, window, |view, _event, _window, cx| {
                view.apply_terminal_focus(false);
                cx.notify();
            });
            self.focus_subscriptions = Some((window_id, focus_in, focus_out));
        }

        let focused = self.focus_handle.is_focused(window) && window.is_window_active();
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

        self.sync_search_with_terminal(cx);
        if self.search_native_navigation_in_flight.is_none()
            && !self.search_native_navigation_queue.is_empty()
        {
            cx.on_next_frame(window, |view, _window, cx| {
                view.dispatch_native_search_navigation(cx);
            });
        }

        let search_match_rects = if self.search_active && !self.search_regex_mode {
            self.search_native_snapshot
                .as_ref()
                .map(|state| {
                    state
                        .viewport_matches
                        .iter()
                        .map(|found| SearchHighlight {
                            start: found.start,
                            end: found.end,
                            is_active: state.selected_match.as_ref().is_some_and(|selected| {
                                selected.start == found.start && selected.end == found.end
                            }),
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else if self.search_active && !self.search_matches.is_empty() {
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

        let search_rail_lines: Arc<[usize]> = if self.search_active && !self.search_regex_mode {
            self.search_native_snapshot
                .as_ref()
                .map(|state| state.rail_offsets.clone())
                .unwrap_or_default()
        } else if self.search_active && !self.search_matches.is_empty() {
            let bottom = backend.bottommost_line();
            self.search_matches
                .iter()
                .map(|m| bottom.0.saturating_sub(m.start.line.0).max(0) as usize)
                .collect()
        } else {
            Arc::default()
        };

        let scrollbar_presence = self.scrollbar_presence(window, cx);

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
            scrollbar_presence,
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
            .cursor(
                if self.scrollbar_drag.is_some()
                    || self.scrollbar_gutter_contains(window.mouse_position())
                {
                    gpui::CursorStyle::Arrow
                } else if self.ctrl_hovered_link.is_some() {
                    gpui::CursorStyle::PointingHand
                } else {
                    gpui::CursorStyle::IBeam
                },
            )
            .on_modifiers_changed(cx.listener(Self::handle_modifiers_changed))
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_key_up(cx.listener(Self::handle_key_up))
            .on_any_mouse_down(cx.listener(Self::handle_mouse_down))
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered && this.scrollbar_set_hovered(false) {
                    cx.notify();
                }
            }))
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
            .on_action(
                cx.listener(|this, _: &crate::TerminalSelectAll, window, cx| {
                    this.handle_select_all(window, cx);
                }),
            )
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
            .on_action(
                cx.listener(|this, _: &crate::AcceptConversationNotice, _window, cx| {
                    this.accept_conversation_banner(cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::DismissConversationNotice, _window, cx| {
                    this.dismiss_conversation_banner(cx);
                }),
            )
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
            .on_action(cx.listener(|this, _: &crate::InsertPath, window, cx| {
                this.open_path_picker(window, cx);
            }))
            .size_full()
            .child(terminal_body);

        if search_active {
            el = el.child(self.render_search_overlay(cx));
        }

        if let Some(bar) = self.render_host_link_bar(crate::theme::ui_colors(), cx) {
            el = el.child(bar);
        }

        if let Some(banner) = self.render_conversation_banner(crate::theme::ui_colors(), cx) {
            el = el.child(banner);
        }

        if self.copy_mode_active {
            let copy_badge = copy_mode_badge()
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

        div()
            .size_full()
            .child(el)
            .children(self.path_picker.as_ref().map(|slot| slot.picker.clone()))
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

    fn a11y_node(element: &gpui::Stateful<gpui::Div>) -> gpui::accesskit::Node {
        use gpui::Element as _;
        let mut node =
            gpui::accesskit::Node::new(element.a11y_role().expect("the control exposes a role"));
        element.write_a11y_info(&mut node);
        node
    }

    #[test]
    fn the_regex_toggle_is_always_rendered_with_its_pressed_state() {
        use gpui::Element as _;
        let ui = crate::theme::ui_colors();
        for (active, expected) in [
            (true, gpui::accesskit::Toggled::True),
            (false, gpui::accesskit::Toggled::False),
        ] {
            let toggle = search_regex_toggle(active, ui, ui.subtle);
            assert_eq!(toggle.a11y_role(), Some(gpui::accesskit::Role::Button));
            let node = a11y_node(&toggle);
            assert_eq!(node.label(), Some(SEARCH_REGEX_TOGGLE_LABEL));
            assert_eq!(node.toggled(), Some(expected));
        }
    }

    #[test]
    fn the_copy_mode_badge_has_a_role_and_a_name() {
        use gpui::Element as _;
        let badge = copy_mode_badge();
        assert_eq!(badge.a11y_role(), Some(gpui::accesskit::Role::Status));
        assert_eq!(a11y_node(&badge).label(), Some("Copy mode"));
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
        assert_eq!(state.extract_scrollback(), None);
    }

    const HOST_WINDOW_W: f32 = 800.0;
    const HOST_WINDOW_H: f32 = 600.0;

    struct TerminalHost {
        terminal: Option<Entity<TerminalView>>,
        cached: bool,
    }

    impl Render for TerminalHost {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let mut root = div().size_full();
            if let Some(terminal) = self.terminal.clone() {
                root = if self.cached {
                    root.child(terminal.cached(gpui::StyleRefinement::default().size_full()))
                } else {
                    root.child(terminal)
                };
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
                cached: false,
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
    fn moving_terminal_between_windows_rebinds_focus_and_clears_pointer_state(
        cx: &mut gpui::TestAppContext,
    ) {
        let (terminal, first_window) = {
            let (terminal, host, first) = hosted_terminal(cx);
            focus_terminal(&terminal, first);
            let first_window = terminal.read_with(first, |view, _| {
                assert!(view.was_focused);
                view.focus_subscriptions.as_ref().unwrap().0
            });
            terminal.update(first, |view, _| {
                view.selecting = true;
                view.ime_marked_text = "pending composition".into();
                view.scroll_remainder = 0.5;
                view.terminal
                    .restore_scrollback("preserved session content");
            });
            host.update(first, |host, cx| {
                host.terminal = None;
                cx.notify();
            });
            first.run_until_parked();
            (terminal, first_window)
        };
        let moved = terminal.clone();
        let (_host, second) = cx.add_window_view(move |_window, _cx| TerminalHost {
            terminal: Some(moved),
            cached: false,
        });
        second.simulate_resize(gpui::size(gpui::px(HOST_WINDOW_W), gpui::px(HOST_WINDOW_H)));
        second.run_until_parked();
        terminal.read_with(second, |view, _| {
            assert_ne!(view.focus_subscriptions.as_ref().unwrap().0, first_window);
            assert!(!view.selecting);
            assert!(view.ime_marked_text.is_empty());
            assert_eq!(view.scroll_remainder, 0.0);
        });
        second.update(|window, _cx| window.activate_window());
        focus_terminal(&terminal, second);
        terminal.read_with(second, |view, _| assert!(view.was_focused));
        second.update(|window, _cx| window.blur());
        second.run_until_parked();
        terminal.read_with(second, |view, _| assert!(!view.was_focused));
    }

    #[gpui::test]
    fn scrollbar_hover_requests_a_frame_while_the_hide_timer_is_pending(
        cx: &mut gpui::TestAppContext,
    ) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        terminal.update(cx, |view, cx| {
            view.terminal.restore_scrollback(&"history\n".repeat(200));
            view.scrollbar_reveal.touch(std::time::Instant::now());
            cx.notify();
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_next_frame(cx);
        });
        let position = terminal.read_with(cx, |view, _| {
            assert!(view.scrollbar_hide_scheduled);
            let metrics = view
                .scrollbar_metrics
                .lock()
                .unwrap()
                .expect("scrollback has a scrollbar");
            gpui::point(
                metrics.strip_left + gpui::px(5.0),
                metrics.track_top + gpui::px(5.0),
            )
        });
        cx.update(|window, cx| {
            window.simulate_mouse_move(position, cx);
            window.draw(cx).clear(cx);
            assert!(
                window.simulate_next_frame(cx) > 0,
                "hover must request a frame without waiting for the hide timer"
            );
        });
        terminal.read_with(cx, |view, _| {
            assert!(view.scrollbar_reveal.is_pinned());
            assert!(view.hovered_cell.is_none());
        });
    }

    #[gpui::test]
    fn font_only_pane_config_notifies_a_cached_terminal_view(cx: &mut gpui::TestAppContext) {
        let (terminal, host, cx) = hosted_terminal(cx);
        host.update(cx, |host, cx| {
            host.cached = true;
            cx.notify();
        });
        let pane = cx.new(|cx| crate::pane::Pane::new(terminal.clone(), 1, cx));
        let config = paneflow_config::schema::PaneFlowConfig::default();
        pane.update(cx, |pane, cx| pane.apply_config(&config, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let probe = watch_notifications(&terminal, cx);
        pane.update(cx, |_, cx| cx.notify());
        assert_eq!(probe.hits(), 0);

        let mut changed = config.clone();
        changed.font_size = Some(config.font_size.unwrap_or(13.0) + 2.0);
        pane.update(cx, |pane, cx| pane.apply_config(&changed, cx));

        assert!(
            probe.hits() > 0,
            "font-only config changes must invalidate the cached terminal entity"
        );
    }

    #[gpui::test]
    fn a_config_reload_pushes_option_as_meta_to_the_terminal(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        let pane = cx.new(|cx| crate::pane::Pane::new(terminal.clone(), 1, cx));
        for option_as_meta in [true, false, true] {
            let config = paneflow_config::schema::PaneFlowConfig {
                option_as_meta: Some(option_as_meta),
                ..Default::default()
            };
            pane.update(cx, |pane, cx| pane.apply_config(&config, cx));
            terminal.read_with(cx, |view, _| {
                assert_eq!(view.option_as_meta, option_as_meta);
            });
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

    fn pastes_after(
        text: &str,
        answer: Option<&str>,
        cx: &mut gpui::TestAppContext,
    ) -> (Vec<(String, bool)>, bool) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        focus_terminal(&terminal, cx);
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        cx.dispatch_action(crate::TerminalPaste);
        let prompted = cx.has_pending_prompt();
        if let Some(answer) = answer {
            cx.simulate_prompt_answer(answer);
        }
        cx.run_until_parked();
        (
            terminal.read_with(cx, |view, _| view.terminal.queued_pastes()),
            prompted,
        )
    }

    #[gpui::test]
    fn a_multiline_paste_without_bracketed_paste_asks_and_cancel_sends_nothing(
        cx: &mut gpui::TestAppContext,
    ) {
        let (queued, prompted) = pastes_after("echo one\necho two\n", Some("Cancel"), cx);
        assert!(prompted, "a multi-line paste asks for confirmation");
        assert!(queued.is_empty(), "{queued:?}");
    }

    #[gpui::test]
    fn a_confirmed_multiline_paste_is_sent_once(cx: &mut gpui::TestAppContext) {
        let (queued, prompted) = pastes_after("echo one\r\necho two", Some("Paste"), cx);
        assert!(prompted);
        assert_eq!(queued, vec![("echo one\necho two".to_owned(), true)]);
    }

    #[gpui::test]
    fn a_single_line_paste_never_asks(cx: &mut gpui::TestAppContext) {
        let (queued, prompted) = pastes_after("echo hello", None, cx);
        assert!(
            !prompted,
            "a single-line paste is sent without confirmation"
        );
        assert_eq!(queued, vec![("echo hello".to_owned(), false)]);
    }

    #[gpui::test]
    fn an_image_only_clipboard_pastes_the_path_of_a_png_copy(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        focus_terminal(&terminal, cx);
        let dir = tempfile::tempdir().expect("tempdir");
        let stored_in = dir.path().to_path_buf();
        terminal.update(cx, |view, _| view.clipboard_image_dir = Some(stored_in));
        let mut bmp = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut bmp, image::ImageFormat::Bmp)
            .expect("encode the fixture");
        let copied = gpui::Image::from_bytes(gpui::ImageFormat::Bmp, bmp.into_inner());
        cx.write_to_clipboard(ClipboardItem::new_image(&copied));

        cx.dispatch_action(crate::TerminalPaste);
        cx.run_until_parked();

        let file_name = format!("{:016x}.png", copied.id);
        let queued = terminal.read_with(cx, |view, _| view.terminal.queued_pastes());
        assert!(
            matches!(queued.as_slice(), [(text, false)] if text.contains(&file_name)),
            "one paste of the stored PNG path, got {queued:?}"
        );
        let stored = std::fs::read(dir.path().join(&file_name)).expect("the PNG copy exists");
        assert!(stored.starts_with(b"\x89PNG"));
    }

    #[gpui::test]
    fn terminal_shortcuts_stay_active_while_the_search_field_has_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        cx.update(|_window, cx| {
            cx.bind_keys([gpui::KeyBinding::new(
                "ctrl-shift-f",
                crate::ToggleSearch,
                Some("Terminal"),
            )]);
        });
        focus_terminal(&terminal, cx);

        cx.simulate_keystrokes("ctrl-shift-f");
        let search_focused = cx.update(|window, cx| {
            let view = terminal.read(cx);
            view.search_active && view.search_input.read(cx).focus_handle.is_focused(window)
        });
        assert!(
            search_focused,
            "the shortcut opens and focuses the search field"
        );

        cx.simulate_keystrokes("ctrl-shift-f");
        assert!(
            !terminal.read_with(cx, |view, _| view.search_active),
            "a Terminal shortcut still fires from inside the search field"
        );
    }

    #[gpui::test]
    fn edit_paste_and_copy_with_the_search_field_focused_act_on_the_field(
        cx: &mut gpui::TestAppContext,
    ) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        focus_terminal(&terminal, cx);
        cx.dispatch_action(crate::ToggleSearch);
        cx.write_to_clipboard(ClipboardItem::new_string("needle".to_owned()));

        cx.dispatch_action(crate::TerminalPaste);
        assert_eq!(
            terminal.read_with(cx, |view, cx| view.search_input.read(cx).value()),
            "needle"
        );

        cx.write_to_clipboard(ClipboardItem::new_string("untouched".to_owned()));
        cx.dispatch_action(crate::TerminalCopy);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("untouched".to_owned()),
            "copy without a field selection leaves the clipboard alone"
        );
    }

    #[gpui::test]
    fn a_key_that_reveals_the_cursor_notifies_the_terminal_view(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        focus_terminal(&terminal, cx);
        terminal.update(cx, |view, _cx| view.cursor_visible = false);
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        cx.simulate_keystrokes("shift");

        assert!(terminal.read_with(cx, |view, _| view.cursor_visible));
        assert!(
            probe.hits() > 0,
            "revealing the cursor must repaint the terminal view"
        );
    }

    fn record_events(
        view: &Entity<TerminalView>,
        cx: &mut gpui::VisualTestContext,
    ) -> (
        std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        gpui::Subscription,
    ) {
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = events.clone();
        let subscription = cx.update(|_window, cx| {
            cx.subscribe(view, move |_view, event: &TerminalEvent, _cx| {
                let label = match event {
                    TerminalEvent::Notice(message) => format!("notice:{message}"),
                    TerminalEvent::SelectionCopied => "copied".to_owned(),
                    TerminalEvent::OpenCodePath { path, .. } => {
                        format!("open:{}", path.display())
                    }
                    _ => return,
                };
                sink.borrow_mut().push(label);
            })
        });
        (events, subscription)
    }

    fn settle_until(
        cx: &mut gpui::VisualTestContext,
        what: &str,
        mut done: impl FnMut(&mut gpui::VisualTestContext) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                window.simulate_next_frame(cx);
            });
            if done(cx) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn resolve_after_five_seconds(
        _line_text: &str,
        line: Line,
        _char_to_column: &[usize],
        _cwd: Option<&std::path::Path>,
    ) -> Vec<HyperlinkZone> {
        std::thread::sleep(std::time::Duration::from_secs(5));
        vec![code_path_zone(line)]
    }

    fn resolve_immediately(
        _line_text: &str,
        line: Line,
        _char_to_column: &[usize],
        _cwd: Option<&std::path::Path>,
    ) -> Vec<HyperlinkZone> {
        vec![code_path_zone(line)]
    }

    fn code_path_zone(line: Line) -> HyperlinkZone {
        HyperlinkZone {
            uri: "src/main.rs".to_owned(),
            start: Point::new(line.0, 0),
            end: Point::new(line.0, 40),
            is_openable: true,
            source: crate::terminal::types::HyperlinkSource::CodePath,
            line: None,
            col: None,
        }
    }

    #[gpui::test]
    fn a_link_hovered_after_scrolling_back_fifty_lines_is_read_from_the_displayed_line(
        cx: &mut gpui::TestAppContext,
    ) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        focus_terminal(&terminal, cx);
        terminal.update(cx, |view, _cx| {
            let lines: String = (0..200)
                .map(|index| format!("n{index:03} https://example.com/n{index:03}\r\n"))
                .collect();
            view.terminal.write_output(lines.as_bytes());
        });
        settle_until(cx, "the output", |cx| {
            terminal.read_with(cx, |view, _| {
                view.terminal
                    .session_backend()
                    .grid_metrics()
                    .topmost_line
                    .0
                    < -100
            })
        });
        terminal.update(cx, |view, _cx| {
            view.terminal.session_backend().scroll_delta(50);
        });
        settle_until(cx, "the scroll", |cx| {
            terminal.read_with(cx, |view, _| {
                view.terminal
                    .session_backend()
                    .grid_metrics()
                    .display_offset
                    == 50
            })
        });

        cx.simulate_mouse_move(
            gpui::point(gpui::px(120.0), gpui::px(120.0)),
            None,
            link_modifiers(),
        );
        settle_until(cx, "the hovered link", |cx| {
            terminal.read_with(cx, |view, _| view.ctrl_hovered_link.is_some())
        });

        let (hovered, link, metrics) = terminal.read_with(cx, |view, _| {
            (
                view.hovered_cell.expect("a hovered cell"),
                view.ctrl_hovered_link.clone().expect("a hovered link"),
                view.terminal.session_backend().grid_metrics(),
            )
        });
        assert_eq!(metrics.display_offset, 50);
        let viewport_row = hovered.line.0 + 50;
        let bottom_line = 199 - (i32::try_from(metrics.screen_lines).unwrap() - 2);
        let displayed = bottom_line - 50 + viewport_row;
        assert_eq!(link.uri, format!("https://example.com/n{displayed:03}"));
        assert_eq!(link.start.line, hovered.line);
    }

    #[gpui::test]
    fn a_hover_path_resolver_blocked_for_five_seconds_never_stalls_a_frame(
        cx: &mut gpui::TestAppContext,
    ) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        terminal.update(cx, |view, _cx| {
            view.terminal
                .write_output(b"see src/main.rs:12 for details\r\n");
        });
        settle_until(cx, "the output", |cx| {
            terminal.read_with(cx, |view, _| {
                view.terminal
                    .session_backend()
                    .line_text_at(Point::new(0, 0))
                    .is_some_and(|line| line.text.contains("main.rs"))
            })
        });
        let hovered = Point::new(0, 6);
        let moved = Point::new(1, 6);

        let started = std::time::Instant::now();
        terminal.update(cx, |view, cx| {
            view.hover_link_resolver = resolve_after_five_seconds;
            view.link_modifier_held = true;
            view.hovered_cell = Some(hovered);
            view.resolve_links_at_hover(hovered, cx);
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_next_frame(cx);
        });
        let frame = started.elapsed();
        assert!(
            frame < std::time::Duration::from_millis(50),
            "hover and frame took {frame:?} while the resolver was blocked"
        );

        terminal.update(cx, |view, _cx| view.hovered_cell = Some(moved));
        cx.run_until_parked();
        assert!(
            terminal.read_with(cx, |view, _| view.ctrl_hovered_link.is_none()),
            "a result for a cell the pointer left is never applied"
        );

        terminal.update(cx, |view, cx| {
            view.hover_link_cache = None;
            view.hover_link_resolver = resolve_immediately;
            view.hovered_cell = Some(hovered);
            view.resolve_links_at_hover(hovered, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            terminal.read_with(cx, |view, _| view
                .ctrl_hovered_link
                .clone()
                .map(|link| link.uri)),
            Some("src/main.rs".to_owned())
        );
    }

    #[gpui::test]
    fn a_drag_started_on_a_link_selects_text_and_never_opens_the_link(
        cx: &mut gpui::TestAppContext,
    ) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        focus_terminal(&terminal, cx);
        terminal.update(cx, |view, _cx| {
            view.hover_link_resolver = resolve_immediately;
            view.terminal
                .write_output(b"see src/main.rs:12 for the details of this failure\r\n");
        });
        settle_until(cx, "the output", |cx| {
            terminal.read_with(cx, |view, _| {
                view.terminal
                    .session_backend()
                    .line_text_at(Point::new(0, 0))
                    .is_some_and(|line| line.text.contains("main.rs"))
            })
        });
        let (events, _subscription) = record_events(&terminal, cx);
        let (cell_width, line_height) = terminal.read_with(cx, |view, _| {
            (view.cell_width.as_f32(), view.line_height.as_f32())
        });
        let origin = terminal.read_with(cx, |view, _| {
            *view
                .element_origin
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        });
        let at_column = |column: f32| {
            gpui::point(
                origin.x + gpui::px((column + 0.5) * cell_width),
                origin.y + gpui::px(0.5 * line_height),
            )
        };

        cx.simulate_mouse_move(at_column(2.0), None, link_modifiers());
        settle_until(cx, "the hovered link", |cx| {
            terminal.read_with(cx, |view, _| view.ctrl_hovered_link.is_some())
        });
        cx.simulate_mouse_down(at_column(2.0), MouseButton::Left, link_modifiers());
        cx.simulate_mouse_move(at_column(20.0), Some(MouseButton::Left), link_modifiers());
        cx.simulate_mouse_up(at_column(20.0), MouseButton::Left, link_modifiers());
        settle_until(cx, "the copied selection", |_cx| {
            events.borrow().iter().any(|event| event == "copied")
        });
        assert!(
            !events
                .borrow()
                .iter()
                .any(|event| event.starts_with("open:")),
            "{:?}",
            events.borrow()
        );

        events.borrow_mut().clear();
        cx.simulate_mouse_move(at_column(3.0), None, link_modifiers());
        settle_until(cx, "the hovered link", |cx| {
            terminal.read_with(cx, |view, _| view.ctrl_hovered_link.is_some())
        });
        cx.simulate_mouse_down(at_column(3.0), MouseButton::Left, link_modifiers());
        cx.simulate_mouse_up(at_column(3.0), MouseButton::Left, link_modifiers());
        cx.run_until_parked();
        assert_eq!(*events.borrow(), vec!["open:src/main.rs".to_owned()]);
    }

    #[gpui::test]
    fn a_copy_over_the_selection_limit_shows_a_notice_and_keeps_the_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        focus_terminal(&terminal, cx);
        let row = "\u{20ac}".repeat(79);
        terminal.update(cx, |view, _cx| {
            let text: String = (0..3_000).map(|_| format!("{row}\r\n")).collect();
            view.terminal.write_output(text.as_bytes());
        });
        settle_until(cx, "the output", |cx| {
            terminal.read_with(cx, |view, _| {
                view.terminal
                    .session_backend()
                    .grid_metrics()
                    .topmost_line
                    .0
                    < -2_500
            })
        });
        let (events, _subscription) = record_events(&terminal, cx);
        terminal.update(cx, |view, _cx| {
            let backend = view.terminal.session_backend();
            let metrics = backend.grid_metrics();
            backend.press_selection(
                crate::terminal::types::SelectionKind::Simple,
                Point::new(metrics.topmost_line.0, 0),
                (0.0, 0.0),
            );
            backend.drag_selection(
                Point::new(metrics.bottommost_line.0, 78),
                (1.0, 1.0),
                backend.selection_geometry(view.cell_width.as_f32(), view.line_height.as_f32()),
                false,
            );
        });
        settle_until(cx, "the selection", |cx| {
            terminal.read_with(cx, |view, _| {
                view.terminal.session_backend().selection_range().is_some()
            })
        });

        let release = gpui::MouseUpEvent {
            button: MouseButton::Left,
            position: gpui::point(gpui::px(-10.0), gpui::px(-10.0)),
            modifiers: gpui::Modifiers::default(),
            click_count: 1,
        };
        let started = std::time::Instant::now();
        cx.update(|window, cx| {
            terminal.update(cx, |view, cx| view.handle_mouse_up(&release, window, cx));
        });
        let dispatch = started.elapsed();
        assert!(
            events.borrow().is_empty(),
            "the copy runs off the UI thread"
        );
        assert!(
            dispatch < std::time::Duration::from_millis(50),
            "mouse-up took {dispatch:?}"
        );

        cx.run_until_parked();
        assert_eq!(
            *events.borrow(),
            vec![format!(
                "notice:{}",
                crate::terminal::input::selection_too_large_notice(400_000)
            )]
        );
        assert!(
            terminal.read_with(cx, |view, _| {
                view.terminal.session_backend().selection_range().is_some()
            }),
            "a refused copy keeps the selection"
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
    fn native_search_counts_and_reaches_matches_above_and_below_the_viewport(
        cx: &mut gpui::TestAppContext,
    ) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        terminal.update(cx, |view, cx| {
            let text = format!(
                "offscreen-marker top\r\n{}offscreen-marker middle\r\n{}offscreen-marker bottom\r\n",
                "filler\r\n".repeat(1_000),
                "filler\r\n".repeat(1_000),
            );
            view.terminal.write_output(text.as_bytes());
            view.arm_search("offscreen-marker", false, cx);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                window.simulate_next_frame(cx);
            });
            if terminal.read_with(cx, |view, _| {
                view.search_match_count() == 3 && !view.search_scan_in_flight
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "full-history search did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        for expected in [1, 0, 2] {
            terminal.update(cx, |view, cx| view.search_prev(cx));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                cx.run_until_parked();
                cx.update(|window, cx| {
                    window.draw(cx).clear(cx);
                    window.simulate_next_frame(cx);
                });
                if terminal.read_with(cx, |view, _| {
                    view.search_native_navigation_in_flight.is_none()
                }) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "offscreen navigation did not finish"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            terminal.read_with(cx, |view, _| {
                assert_eq!(view.search_match_count(), 3);
                assert_eq!(view.search_current, expected);
                let search = view.search_native_snapshot.as_ref().unwrap();
                let selected = search.selected_match.as_ref().unwrap();
                assert!(
                    search.viewport_matches.iter().any(|found| {
                        found.start == selected.start && found.end == selected.end
                    })
                );
                let metrics = view.terminal.session_backend().grid_metrics();
                let row = selected.start.line.0 + metrics.display_offset as i32;
                assert!(row >= 0);
            });
        }
    }

    #[gpui::test]
    fn native_search_queues_repeated_navigation_until_each_frame_acknowledges_it(
        cx: &mut gpui::TestAppContext,
    ) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        terminal.update(cx, |view, cx| {
            view.terminal
                .write_output("marker\r\n".repeat(30).as_bytes());
            view.search_active = true;
            view.search_query = "marker".into();
            assert!(
                view.terminal
                    .session_backend()
                    .set_native_search("marker".into())
            );
            cx.notify();
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            terminal.update(cx, |view, cx| view.sync_search_with_terminal(cx));
            if terminal.read_with(cx, |view, _| view.search_match_count() == 30) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "search did not finish"
            );
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        terminal.update(cx, |view, cx| {
            for previous in [true, true, true, false, true, false] {
                if previous {
                    view.search_prev(cx);
                } else {
                    view.search_next(cx);
                }
            }
            assert!(view.search_native_navigation_in_flight.is_some());
            assert_eq!(view.search_native_navigation_queue.len(), 5);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                window.simulate_next_frame(cx);
            });
            let done = terminal.read_with(cx, |view, _| {
                view.search_native_navigation_in_flight.is_none()
                    && view.search_native_navigation_queue.is_empty()
            });
            if done {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "navigation did not drain"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        terminal.read_with(cx, |view, _| {
            assert_eq!(view.search_current, 27);
            assert_eq!(view.search_match_count(), 30);
        });
    }

    #[gpui::test]
    fn search_highlighting_notifies_the_terminal_view(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        terminal.update(cx, |view, _cx| {
            view.search_active = true;
            view.search_regex_mode = true;
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
    fn a_terminal_in_an_inactive_window_does_not_redraw_for_the_blink(
        cx: &mut gpui::TestAppContext,
    ) {
        let phase = install_blink_phase(cx);
        let (terminal, _host, cx) = hosted_terminal(cx);
        focus_terminal(&terminal, cx);
        terminal.update(cx, |view, _cx| {
            view.cursor_blink_mode = paneflow_config::schema::CursorBlinkConfig::On;
        });
        cx.deactivate_window();
        cx.update(|window, _cx| window.refresh());
        cx.run_until_parked();
        let probe = watch_notifications(&terminal, cx);
        probe.reset();

        for frame in 0..8 {
            cx.update(|_window, cx| {
                phase.update(cx, |phase, cx| {
                    phase.visible = frame % 2 == 1;
                    cx.notify();
                })
            });
            cx.run_until_parked();
        }

        assert!(
            !terminal.read_with(cx, |view, _| view.was_focused),
            "inactive window: the terminal must not count as focused"
        );
        assert_eq!(
            probe.hits(),
            0,
            "inactive window: the blink must not redraw the terminal"
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

    fn open_path_picker(terminal: &Entity<TerminalView>, cx: &mut gpui::VisualTestContext) {
        focus_terminal(terminal, cx);
        cx.dispatch_action(crate::InsertPath);
        cx.run_until_parked();
        let picker = terminal
            .read_with(cx, |view, _| {
                view.path_picker.as_ref().map(|slot| slot.picker.clone())
            })
            .expect("insert_path must open the picker");
        cx.update(|window, cx| {
            assert!(
                picker.read(cx).focus_handle(cx).is_focused(window),
                "the picker field takes the focus"
            );
        });
    }

    #[gpui::test]
    fn escape_closes_the_path_picker_and_refocuses_the_terminal(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        open_path_picker(&terminal, cx);

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();

        terminal.read_with(cx, |view, _| assert!(view.path_picker.is_none()));
        cx.update(|window, cx| {
            assert!(terminal.read(cx).focus_handle.is_focused(window));
        });
    }

    #[gpui::test]
    fn moving_the_focus_away_closes_the_path_picker(cx: &mut gpui::TestAppContext) {
        let (terminal, _host, cx) = hosted_terminal(cx);
        open_path_picker(&terminal, cx);

        cx.update(|window, cx| {
            let elsewhere = cx.focus_handle();
            window.focus(&elsewhere, cx);
        });
        cx.run_until_parked();

        terminal.read_with(cx, |view, _| assert!(view.path_picker.is_none()));
        cx.update(|window, cx| {
            assert!(
                !terminal.read(cx).focus_handle.is_focused(window),
                "a blur must not pull the focus back to the terminal"
            );
        });
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
