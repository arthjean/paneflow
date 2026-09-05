use crate::ui_primitives::TooltipDelayExt;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, ClickEvent, Context, DragMoveEvent, Entity,
    EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, Pixels, Point, Render, SharedString, Size, StyleRefinement, Styled, Window,
    deferred, div, ease_out_quint, img, prelude::*, px, rgb, svg,
};

use crate::ui_primitives::squircle::{squircle_border, squircle_fill};
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

use crate::diff::DiffView;
use crate::markdown::MarkdownView;
use crate::pane_drag::{
    DragPreview, DropEdge, PaneDrag, ReviewSubjectDrag, SPLIT_EDGE_BAND, SessionDrag,
    compute_drop_edge, split_rect,
};
use crate::terminal::{TerminalEvent, TerminalView};

#[derive(Clone)]
pub enum PaneSurface {
    Terminal(Entity<TerminalView>),
    Markdown(Entity<MarkdownView>),
    Diff(Entity<DiffView>),
}

impl PaneSurface {
    pub fn as_terminal(&self) -> Option<&Entity<TerminalView>> {
        match self {
            PaneSurface::Terminal(t) => Some(t),
            PaneSurface::Markdown(_) | PaneSurface::Diff(_) => None,
        }
    }

    pub(crate) fn kind_icon(&self) -> &'static str {
        match self {
            PaneSurface::Terminal(_) => "icons/terminal.svg",
            PaneSurface::Markdown(_) => "icons/file-text.svg",
            PaneSurface::Diff(_) => "icons/git-branch.svg",
        }
    }

    pub(crate) fn kind_label(&self) -> &'static str {
        match self {
            PaneSurface::Terminal(_) => "Terminal",
            PaneSurface::Markdown(_) => "Markdown",
            PaneSurface::Diff(_) => "Diff",
        }
    }
}

fn pane_colors() -> crate::theme::UiColors {
    crate::theme::ui_colors()
}

fn pane_card_background(
    theme: &crate::theme::TerminalTheme,
    terminal_material_active: bool,
    terminal_selected: bool,
) -> Hsla {
    if !terminal_material_active || !terminal_selected {
        return theme.background;
    }

    #[cfg(target_os = "windows")]
    {
        gpui::transparent_black()
    }

    #[cfg(not(target_os = "windows"))]
    {
        theme.background
    }
}

const HEADER_CONTENT_HEIGHT: f32 = 28.0;
const PANE_HEADER_HEIGHT: f32 =
    HEADER_CONTENT_HEIGHT + crate::app::constants::PANE_CONTENT_INSET_Y * 2.0;
const HEADER_GAP: f32 = 7.0;
const SURFACE_TITLE_TOOLTIP_THRESHOLD: usize = 13;
const HEADER_TEXT_SIZE: f32 = 14.0;
const HEADER_TEXT_LINE_HEIGHT: f32 = 18.0;
const SECTION_PX: f32 = crate::app::constants::PANE_CONTENT_INSET_X;
const ACTION_BUTTON_SIZE: f32 = 22.0;
const CLOSE_BUTTON_SIZE: f32 = 15.0;
const CLOSE_GLYPH_SIZE: f32 = 9.0;
const CLOSE_BASE_ALPHA: f32 = 0.16;
const CLOSE_HOVER_ALPHA: f32 = 0.92;

const HEADER_GROUP: &str = "pane-header-group";
const HEADER_HOVER_MS: u64 = 120;
const PANE_DIM_FADE_MS: u64 = 130;
const PANE_DIM_EPSILON: f32 = 0.002;
const OVERLAY_MARGIN: f32 = 8.0;
const OVERLAY_RADIUS: f32 = 8.0;
const DROP_OVERLAY_BLUE: u32 = 0x007aff;
const DROP_OVERLAY_BACKGROUND_ALPHA: f32 = 0.10;

const SWAP_OVERLAY_FILL_ALPHA: f32 = 0.10;
const SWAP_OVERLAY_BORDER_ALPHA: f32 = 0.22;
const MAX_SURFACE_TITLE_LEN: usize = 24;

fn truncate_surface_title(raw: &str) -> String {
    if raw.chars().count() <= MAX_SURFACE_TITLE_LEN {
        return raw.to_string();
    }
    let head: String = raw.chars().take(MAX_SURFACE_TITLE_LEN - 1).collect();
    format!("{head}…")
}

pub enum PaneEvent {
    Remove,
    Split(crate::layout::SplitDirection),
    ToggleAgentSessions,
    ToggleDiffDock,
    OpenPaneMenu {
        position: Point<Pixels>,
    },
    DropSessionSplit {
        edge: Option<DropEdge>,
        agent: crate::agent_sessions::SessionAgent,
        session_id: String,
        cwd: String,
    },
    DropPaneMove {
        source_pane_id: u64,
        edge: Option<DropEdge>,
    },
    DropSubjectSplit {
        edge: Option<DropEdge>,
        subject: crate::diff::ReviewSubject,
    },
}

struct HeaderHoverMotion {
    live_progress: Rc<Cell<f32>>,
    from: f32,
    target: f32,
    epoch: u64,
}

impl HeaderHoverMotion {
    fn new(live_progress: Rc<Cell<f32>>) -> Self {
        Self {
            live_progress,
            from: 0.0,
            target: 0.0,
            epoch: 0,
        }
    }
}

pub struct Pane {
    pub surface: PaneSurface,
    attention: Option<String>,
    errored: bool,
    search_hits: Option<usize>,
    pub zoomed: bool,
    pub workspace_id: u64,
    header_hover_motion: std::collections::HashMap<SharedString, HeaderHoverMotion>,
    pub cached_config: paneflow_config::schema::PaneFlowConfig,
    drag_split_direction: Option<DropEdge>,
    overlay_prev_dir: Option<DropEdge>,
    overlay_from: (f32, f32, f32, f32),
    overlay_current: Rc<Cell<(f32, f32, f32, f32)>>,
    overlay_seq: usize,
    overlay_pane_size: Size<Pixels>,
    composer_slot: Option<crate::app::composer::ComposerSlot>,
    pending_prefill: bool,
    broadcast_stripe: Option<usize>,
    dimmed: bool,
    dim_from: f32,
    dim_alpha: Rc<Cell<f32>>,
    dim_seq: usize,
    diff_options_open: bool,
    diff_options_submenu: Option<crate::app::diff_dock::DiffOptionsSubmenu>,
}

impl EventEmitter<PaneEvent> for Pane {}

impl Pane {
    pub fn new(terminal: Entity<TerminalView>, workspace_id: u64, cx: &mut Context<Self>) -> Self {
        Self::new_with_surface(PaneSurface::Terminal(terminal), workspace_id, cx)
    }

    pub fn new_with_surface(
        surface: PaneSurface,
        workspace_id: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        let cached_config = paneflow_config::loader::load_config();
        if let PaneSurface::Terminal(t) = &surface {
            Self::subscribe_terminal(t, cx);
            Self::apply_terminal_render_config(t, &cached_config, cx);
        }
        Self {
            surface,
            attention: None,
            errored: false,
            search_hits: None,
            zoomed: false,
            workspace_id,
            header_hover_motion: std::collections::HashMap::new(),
            cached_config,
            drag_split_direction: None,
            overlay_prev_dir: None,
            overlay_from: (0.0, 0.0, 0.0, 0.0),
            overlay_current: Rc::new(Cell::new((0.0, 0.0, 0.0, 0.0))),
            overlay_seq: 0,
            overlay_pane_size: Size::default(),
            composer_slot: None,
            pending_prefill: false,
            broadcast_stripe: None,
            dimmed: false,
            dim_from: 0.0,
            dim_alpha: Rc::new(Cell::new(0.0)),
            dim_seq: 0,
            diff_options_open: false,
            diff_options_submenu: None,
        }
    }

    pub fn set_attention(&mut self, attention: Option<String>, cx: &mut Context<Self>) {
        if self.attention != attention {
            self.attention = attention;
            cx.notify();
        }
    }

    pub fn set_errored(&mut self, errored: bool, cx: &mut Context<Self>) {
        if self.errored != errored {
            self.errored = errored;
            cx.notify();
        }
    }

    pub fn set_search_hits(&mut self, hits: Option<usize>, cx: &mut Context<Self>) {
        if self.search_hits != hits {
            self.search_hits = hits;
            cx.notify();
        }
    }

    pub fn set_composer_slot(
        &mut self,
        slot: Option<crate::app::composer::ComposerSlot>,
        cx: &mut Context<Self>,
    ) {
        self.composer_slot = slot;
        cx.notify();
    }

    pub fn set_pending_prefill(&mut self, pending: bool, cx: &mut Context<Self>) {
        if self.pending_prefill != pending {
            self.pending_prefill = pending;
            cx.notify();
        }
    }

    pub fn set_dimmed(&mut self, dimmed: bool, cx: &mut Context<Self>) {
        if self.dimmed == dimmed {
            return;
        }
        self.dim_from = self.dim_alpha.get();
        self.dim_seq = self.dim_seq.wrapping_add(1);
        self.dimmed = dimmed;
        cx.notify();
    }

    pub fn set_broadcast_stripe(&mut self, color_idx: Option<usize>, cx: &mut Context<Self>) {
        if self.broadcast_stripe != color_idx {
            self.broadcast_stripe = color_idx;
            cx.notify();
        }
    }

    fn render_composer_overlay(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let slot = self.composer_slot.clone()?;
        let ui = pane_colors();

        let mut header = div().flex().flex_row().items_center().gap(px(6.)).child(
            div()
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ui.text)
                .child("Composer"),
        );

        let toggle = slot.toggle_broadcast.clone();
        let broadcast_label: SharedString = if slot.broadcast {
            match &slot.group_label {
                Some(label) => format!("Broadcast: {label}").into(),
                None => "Broadcast".into(),
            }
        } else {
            "Single pane".into()
        };
        let broadcast_bg = if slot.broadcast {
            ui.accent.opacity(0.15)
        } else {
            ui.subtle
        };
        let broadcast_text = if slot.broadcast { ui.accent } else { ui.muted };
        let broadcast_hover_text = if slot.broadcast { ui.accent } else { ui.text };
        header = header.child(
            div()
                .id("composer-broadcast-toggle")
                .px(px(6.))
                .py(px(2.))
                .rounded(px(4.))
                .text_size(px(10.))
                .bg(broadcast_bg)
                .text_color(broadcast_text)
                .animated_hover(move |style, delta| {
                    style.text_color(lerp_color(broadcast_text, broadcast_hover_text, delta));
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    toggle(cx);
                })
                .child(broadcast_label),
        );

        if slot.busy {
            header = header.child(
                div()
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .text_size(px(10.))
                    .bg(ui.vc_modified.opacity(0.15))
                    .text_color(ui.vc_modified)
                    .child("agent generating - Enter queues"),
            );
        }

        if slot.pending_count > 0 {
            let cancel = slot.cancel_pending.clone();
            header = header.child(
                div()
                    .id("composer-cancel-pending")
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .text_size(px(10.))
                    .bg(ui.subtle)
                    .text_color(ui.muted)
                    .animated_hover(move |style, delta| {
                        style.text_color(lerp_color(ui.muted, ui.vc_deleted, delta));
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cancel(cx);
                    })
                    .child(format!("{} queued · cancel", slot.pending_count)),
            );
        }

        let submit_chord = if cfg!(target_os = "macos") {
            "⌘+Enter"
        } else {
            "Ctrl+Enter"
        };
        let hint: SharedString = if slot.broadcast {
            "Enter pre-fills every ready member - broadcast never submits".into()
        } else {
            format!("Enter pre-fills without submitting · {submit_chord} pre-fills and submits")
                .into()
        };

        let dismiss_backdrop = slot.dismiss.clone();
        let dismiss_out = slot.dismiss.clone();
        Some(
            deferred(
                div()
                    .id("composer-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        dismiss_backdrop(cx);
                    })
                    .child(squircle_fill(
                        crate::app::constants::PANE_CARD_RADIUS,
                        gpui::hsla(0., 0., 0., 0.25),
                    ))
                    .child(
                        div()
                            .id("composer-panel")
                            .occlude()
                            .m(px(8.))
                            .p(px(8.))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .bg(ui.overlay)
                            .border_1()
                            .border_color(ui.border)
                            .rounded(px(8.))
                            .shadow_lg()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_mouse_down_out(move |_, _, cx| {
                                dismiss_out(cx);
                            })
                            .child(header)
                            .child(div().max_h(px(180.)).child(slot.input.clone()))
                            .child(div().text_size(px(10.)).text_color(ui.muted).child(hint)),
                    ),
            )
            .with_priority(4)
            .into_any_element(),
        )
    }

    pub fn terminals(&self) -> impl Iterator<Item = &Entity<TerminalView>> {
        self.surface.as_terminal().into_iter()
    }

    pub fn apply_config(
        &mut self,
        config: &paneflow_config::schema::PaneFlowConfig,
        cx: &mut Context<Self>,
    ) {
        self.cached_config = config.clone();
        let terminals: Vec<Entity<TerminalView>> = self.terminals().cloned().collect();
        for terminal in terminals {
            Self::apply_terminal_render_config(&terminal, config, cx);
        }
        cx.notify();
    }

    fn apply_terminal_render_config(
        terminal: &Entity<TerminalView>,
        config: &paneflow_config::schema::PaneFlowConfig,
        cx: &mut Context<Self>,
    ) {
        let integrated_glyphs_enabled = config
            .terminal
            .as_ref()
            .is_none_or(|terminal| terminal.resolved_integrated_glyphs());
        let color_emoji_enabled = config
            .terminal
            .as_ref()
            .is_none_or(|terminal| terminal.resolved_color_emoji());
        let minimum_contrast = config
            .terminal
            .as_ref()
            .map_or(0.0, |terminal| terminal.resolved_minimum_contrast());
        let cursor_color_override = config
            .terminal
            .as_ref()
            .and_then(|terminal| terminal.cursor_color.as_deref())
            .and_then(crate::terminal::view::hsla_from_hex_color);
        terminal.update(cx, |terminal, cx| {
            terminal.set_integrated_glyphs_enabled(integrated_glyphs_enabled, cx);
            terminal.set_color_emoji_enabled(color_emoji_enabled, cx);
            terminal.set_minimum_contrast(minimum_contrast, cx);
            terminal.set_cursor_color_override(cursor_color_override, cx);
        });
    }

    pub fn contains_terminal(&self, terminal: &Entity<TerminalView>) -> bool {
        self.terminals().any(|t| t == terminal)
    }

    fn subscribe_terminal(terminal: &Entity<TerminalView>, cx: &mut Context<Self>) {
        cx.subscribe(
            terminal,
            |this, terminal, event: &TerminalEvent, cx| match event {
                TerminalEvent::ChildExited => {
                    if this.surface.as_terminal() == Some(&terminal) {
                        cx.emit(PaneEvent::Remove);
                    }
                }
                TerminalEvent::TitleChanged => {
                    cx.notify();
                }
                TerminalEvent::CwdChanged(_)
                | TerminalEvent::ActivityBurst
                | TerminalEvent::ServiceDetected(_)
                | TerminalEvent::CancelSwapMode
                | TerminalEvent::SelectionCopied
                | TerminalEvent::OpenMarkdownPath(_)
                | TerminalEvent::OpenCodePath { .. }
                | TerminalEvent::FontZoomChanged
                | TerminalEvent::FleetSearchRequested { .. }
                | TerminalEvent::AgentProgressChanged { .. }
                | TerminalEvent::ProgramNotification { .. }
                | TerminalEvent::ShellPromptReady => {}
            },
        )
        .detach();
    }

    fn surface_full_title(surface: &PaneSurface, cx: &App) -> String {
        match surface {
            PaneSurface::Markdown(md) => md.read(cx).title().to_string(),
            PaneSurface::Diff(d) => d.read(cx).title(),
            PaneSurface::Terminal(t) => Self::terminal_surface_full_title(t, cx),
        }
    }

    fn surface_title(surface: &PaneSurface, cx: &App) -> String {
        let raw = match surface {
            PaneSurface::Markdown(md) => md.read(cx).title().to_string(),
            PaneSurface::Diff(d) => d.read(cx).title(),
            PaneSurface::Terminal(t) => Self::terminal_surface_title(t, cx),
        };
        truncate_surface_title(&raw)
    }

    fn surface_icon(surface: &PaneSurface) -> &'static str {
        surface.kind_icon()
    }

    fn terminal_surface_title(terminal: &Entity<TerminalView>, cx: &App) -> String {
        let view = terminal.read(cx);
        if let Some(custom) = view.terminal.custom_name.as_ref().filter(|c| !c.is_empty()) {
            return custom.clone();
        }
        let raw = &view.terminal.title;
        if let Some(agent) = view.terminal.detected_agent {
            return agent.display_name().into();
        }
        if let Some(path_title) =
            Self::shell_path_title(raw).and_then(|path| Self::cwd_label(&path))
        {
            return path_title;
        }
        if let Some(agent_title) = Self::agent_title_from_terminal_title(raw) {
            return agent_title.into();
        }
        if Self::is_default_terminal_title(raw)
            && let Some(cwd) = view.terminal.current_cwd.as_deref()
            && let Some(label) = Self::cwd_label(cwd)
        {
            return label;
        }
        if raw.is_empty() {
            "Terminal".into()
        } else {
            raw.clone()
        }
    }

    fn terminal_surface_full_title(terminal: &Entity<TerminalView>, cx: &App) -> String {
        let view = terminal.read(cx);
        if let Some(custom) = view.terminal.custom_name.as_ref().filter(|c| !c.is_empty()) {
            return custom.clone();
        }
        let raw = &view.terminal.title;
        if let Some(agent) = view.terminal.detected_agent {
            return agent.display_name().into();
        }
        if let Some(path_title) = Self::shell_path_title(raw) {
            return path_title;
        }
        if let Some(agent_title) = Self::agent_title_from_terminal_title(raw) {
            return agent_title.into();
        }
        if Self::is_default_terminal_title(raw)
            && let Some(cwd) = view
                .terminal
                .current_cwd
                .as_ref()
                .filter(|cwd| !cwd.is_empty())
        {
            return cwd.clone();
        }
        if raw.is_empty() {
            "Terminal".into()
        } else {
            raw.clone()
        }
    }

    fn is_default_terminal_title(title: &str) -> bool {
        title.trim().is_empty() || title.trim().eq_ignore_ascii_case("terminal")
    }

    fn agent_title_from_terminal_title(title: &str) -> Option<&'static str> {
        let first = title.split_whitespace().next()?.trim();
        let first = first
            .strip_suffix(".exe")
            .or_else(|| first.strip_suffix(".EXE"))
            .unwrap_or(first);
        if let Some(agent) = crate::agent_launcher::TerminalAgent::from_binary(first) {
            return Some(agent.display_name());
        }
        match first.to_ascii_lowercase().as_str() {
            "nvim" | "neovim" => Some("Neovim"),
            "vim" => Some("Vim"),
            "top" | "htop" | "btop" => Some("System Monitor"),
            _ => None,
        }
    }

    fn shell_path_title(title: &str) -> Option<String> {
        let trimmed = title.rsplit(':').next()?.trim();
        if trimmed.starts_with('/') || trimmed.starts_with('~') {
            Some(trimmed.to_string())
        } else {
            None
        }
    }

    fn cwd_label(cwd: &str) -> Option<String> {
        let trimmed = cwd.trim();
        if trimmed.is_empty() {
            return None;
        }
        let path = std::path::Path::new(trimmed);
        if dirs::home_dir().as_deref() == Some(path) {
            return Some("~".into());
        }
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .or_else(|| Some(trimmed.to_string()))
    }

    fn action_button(
        &self,
        id: &'static str,
        icon_path: &'static str,
        handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = pane_colors();
        self.action_button_shell(
            SharedString::from(id),
            Self::command_icon(SharedString::from(icon_path), ui.muted, false),
            ui.muted,
            Some(ui.text),
            handler,
            cx,
        )
    }

    fn command_icon(icon_path: SharedString, tint: Hsla, multicolor: bool) -> AnyElement {
        if multicolor {
            img(icon_path).size(px(14.)).flex_none().into_any_element()
        } else {
            svg()
                .size(px(14.))
                .flex_none()
                .path(icon_path)
                .text_color(tint)
                .into_any_element()
        }
    }

    fn hover_motion_snapshot(&self, id: &SharedString) -> (Rc<Cell<f32>>, f32, f32, u64) {
        self.header_hover_motion
            .get(id)
            .map(|motion| {
                (
                    motion.live_progress.clone(),
                    motion.from,
                    motion.target,
                    motion.epoch,
                )
            })
            .unwrap_or_else(|| (Rc::new(Cell::new(0.0)), 0.0, 0.0, 0))
    }

    fn set_header_hover_target(
        &mut self,
        id: &SharedString,
        live_progress: &Rc<Cell<f32>>,
        target: f32,
    ) -> bool {
        let motion = self
            .header_hover_motion
            .entry(id.clone())
            .or_insert_with(|| HeaderHoverMotion::new(live_progress.clone()));
        if motion.target == target {
            return false;
        }

        motion.from = motion.live_progress.get();
        motion.target = target;
        motion.epoch = motion.epoch.saturating_add(1);
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn header_button_shell(
        &self,
        id: SharedString,
        icon: AnyElement,
        size: f32,
        radius: f32,
        base_tint: Hsla,
        hover_tint: Option<Hsla>,
        handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (live_progress, from, target, epoch) = self.hover_motion_snapshot(&id);

        let hover_id = id.clone();
        let hover_live_progress = live_progress.clone();
        let mouse_up_id = id.clone();
        let mouse_up_live_progress = live_progress.clone();
        let mouse_up_out_id = id.clone();
        let mouse_up_out_live_progress = live_progress.clone();
        let hover_background = crate::app::constants::sidebar_tab_hover_background();
        let active_background = crate::app::constants::sidebar_tab_active_background();
        let button = div()
            .id(id.clone())
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(size))
            .h(px(size))
            .rounded(px(radius))
            .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                let target = if *hovered { 1.0 } else { 0.0 };
                if this.set_header_hover_target(&hover_id, &hover_live_progress, target) {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    if this.set_header_hover_target(&mouse_up_id, &mouse_up_live_progress, 1.0) {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    if this.set_header_hover_target(
                        &mouse_up_out_id,
                        &mouse_up_out_live_progress,
                        0.0,
                    ) {
                        cx.notify();
                    }
                }),
            )
            .on_click(move |e, w, cx| handler(e, w, cx))
            .active(move |style| style.bg(active_background).opacity(0.82));

        let visual = div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius))
            .text_color(base_tint)
            .child(icon);

        let distance = (target - from).abs();
        let visual = if epoch == 0 || distance <= f32::EPSILON {
            live_progress.set(target);
            let tint = hover_tint
                .map(|hover_tint| base_tint.blend(hover_tint.opacity(target)))
                .unwrap_or(base_tint);
            visual
                .bg(hover_background.opacity(target))
                .text_color(tint)
                .into_any_element()
        } else {
            let animation_id = SharedString::from(format!("pane-action-hover-{id}-{epoch}"));
            let duration = Duration::from_secs_f32(
                Duration::from_millis(HEADER_HOVER_MS).as_secs_f32() * distance,
            );
            visual
                .with_animation(
                    animation_id,
                    Animation::new(duration).with_easing(ease_out_quint()),
                    move |visual, delta| {
                        let progress = (from + (target - from) * delta).clamp(0.0, 1.0);
                        live_progress.set(progress);
                        let tint = hover_tint
                            .map(|hover_tint| base_tint.blend(hover_tint.opacity(progress)))
                            .unwrap_or(base_tint);
                        visual
                            .bg(hover_background.opacity(progress))
                            .text_color(tint)
                    },
                )
                .into_any_element()
        };

        button.child(visual).into_any_element()
    }

    fn action_button_shell(
        &self,
        id: SharedString,
        icon: AnyElement,
        base_tint: Hsla,
        hover_tint: Option<Hsla>,
        handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.header_button_shell(
            id,
            icon,
            ACTION_BUTTON_SIZE,
            4.0,
            base_tint,
            hover_tint,
            handler,
            cx,
        )
    }

    fn close_chip_frame(visual: gpui::Div, progress: f32, ui: crate::theme::UiColors) -> gpui::Div {
        let alpha = CLOSE_BASE_ALPHA + (CLOSE_HOVER_ALPHA - CLOSE_BASE_ALPHA) * progress;
        visual
            .bg(crate::settings::components::with_alpha(ui.text, alpha))
            .child(
                svg()
                    .size(px(CLOSE_GLYPH_SIZE))
                    .flex_none()
                    .path("icons/close.svg")
                    .text_color(ui.text.blend(ui.base.opacity(progress))),
            )
    }

    fn render_close_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = pane_colors();
        let id = SharedString::from("pane-btn-close");
        let (live_progress, from, target, epoch) = self.hover_motion_snapshot(&id);

        let hover_id = id.clone();
        let hover_progress = live_progress.clone();
        let mouse_up_id = id.clone();
        let mouse_up_progress = live_progress.clone();
        let mouse_up_out_id = id.clone();
        let mouse_up_out_progress = live_progress.clone();

        let visual = div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .shadow_lg();
        let distance = (target - from).abs();
        let visual = if epoch == 0 || distance <= f32::EPSILON {
            live_progress.set(target);
            Self::close_chip_frame(visual, target, ui).into_any_element()
        } else {
            let duration = Duration::from_secs_f32(
                Duration::from_millis(HEADER_HOVER_MS).as_secs_f32() * distance,
            );
            let animated_progress = live_progress.clone();
            visual
                .with_animation(
                    SharedString::from(format!("pane-close-hover-{epoch}")),
                    Animation::new(duration).with_easing(ease_out_quint()),
                    move |visual, delta| {
                        let progress = (from + (target - from) * delta).clamp(0.0, 1.0);
                        animated_progress.set(progress);
                        Self::close_chip_frame(visual, progress, ui)
                    },
                )
                .into_any_element()
        };

        div()
            .id(id)
            .flex_none()
            .size(px(CLOSE_BUTTON_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                let target = if *hovered { 1.0 } else { 0.0 };
                if this.set_header_hover_target(&hover_id, &hover_progress, target) {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    if this.set_header_hover_target(&mouse_up_id, &mouse_up_progress, 1.0) {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    if this.set_header_hover_target(&mouse_up_out_id, &mouse_up_out_progress, 0.0) {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _e: &ClickEvent, _window, cx| {
                this.close(cx);
                cx.stop_propagation();
            }))
            .child(visual)
            .into_any_element()
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(PaneEvent::Remove);
    }

    fn apply_drag_edge(
        &mut self,
        bounds: gpui::Bounds<Pixels>,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let w = bounds.size.width.as_f32();
        let h = bounds.size.height.as_f32();
        let x = (pos.x - bounds.left()).as_f32();
        let y = (pos.y - bounds.top()).as_f32();
        let edge = compute_drop_edge(w, h, x, y, SPLIT_EDGE_BAND);
        self.apply_drag_region(bounds, edge, cx);
    }

    fn apply_drag_region(
        &mut self,
        bounds: gpui::Bounds<Pixels>,
        edge: Option<DropEdge>,
        cx: &mut Context<Self>,
    ) {
        let w = bounds.size.width.as_f32();
        let h = bounds.size.height.as_f32();
        self.overlay_pane_size = bounds.size;
        if self.drag_split_direction != edge {
            let live = self.overlay_current.get();
            self.overlay_from = if live.2 > 0.0 && live.3 > 0.0 {
                live
            } else {
                split_rect(self.overlay_prev_dir, w, h)
            };
            self.overlay_prev_dir = self.drag_split_direction;
            self.drag_split_direction = edge;
            self.overlay_seq = self.overlay_seq.wrapping_add(1);
            cx.notify();
        }
    }

    pub fn active_terminal_opt(&self) -> Option<&Entity<TerminalView>> {
        self.surface.as_terminal()
    }

    fn render_surface_title(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if let PaneSurface::Diff(diff) = &self.surface {
            return Self::render_diff_surface_title(diff, cx);
        }
        let full_title = Self::surface_full_title(&self.surface, cx);
        let display_title = Self::surface_title(&self.surface, cx);
        let show_tooltip = full_title != display_title
            || full_title.chars().count() > SURFACE_TITLE_TOOLTIP_THRESHOLD;
        let mut title = div()
            .id("pane-header-title")
            .min_w_0()
            .overflow_x_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(HEADER_TEXT_SIZE))
            .line_height(px(HEADER_TEXT_LINE_HEIGHT))
            .font_weight(gpui::FontWeight::MEDIUM)
            .child(display_title);
        if show_tooltip {
            title = title.delayed_tooltip(crate::ui_primitives::text_tooltip(full_title));
        }
        title.into_any_element()
    }

    fn render_diff_surface_title(
        diff: &Entity<crate::diff::DiffView>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let ui = pane_colors();
        let subject = diff.read(cx).subject();
        let repo = subject.repo_name();
        let branch = subject.branch_label();
        let tooltip = std::iter::once(subject.label().into())
            .chain(diff.read(cx).attribution_lines())
            .map(|line: SharedString| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        div()
            .id("pane-header-title")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(HEADER_GAP))
            .min_w_0()
            .overflow_x_hidden()
            .text_size(px(HEADER_TEXT_SIZE))
            .line_height(px(HEADER_TEXT_LINE_HEIGHT))
            .child(
                svg()
                    .size(px(13.))
                    .flex_none()
                    .path("icons/git-branch.svg")
                    .text_color(ui.muted),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(ui.text)
                    .child(repo),
            )
            .when(!branch.is_empty(), |title| {
                title.child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_color(ui.muted)
                        .child(format!("\u{b7} {branch}")),
                )
            })
            .delayed_tooltip(crate::ui_primitives::text_tooltip(tooltip))
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = pane_colors();

        let has_attention = self.attention.is_some();
        let has_errored = self.errored;
        let status_dot = (has_errored || has_attention).then(|| {
            div()
                .flex_none()
                .w(px(6.0))
                .h(px(6.0))
                .rounded_full()
                .bg(if has_errored {
                    ui.agent_error
                } else {
                    ui.vc_conflict
                })
                .into_any_element()
        });

        let has_pending = self.pending_prefill;
        let pending_chip = has_pending.then(|| {
            div()
                .flex_none()
                .px(px(4.))
                .rounded(px(3.))
                .bg(ui.subtle)
                .text_size(px(9.))
                .text_color(ui.muted)
                .child("1 queued")
                .into_any_element()
        });

        let leading_slots: u8 = u8::from(has_errored || has_attention) + u8::from(has_pending);
        let progress = self
            .surface
            .as_terminal()
            .and_then(|terminal| terminal.read(cx).terminal.progress)
            .filter(|_| leading_slots < 2)
            .and_then(|report| progress_chip_label(report).map(|label| (report.state, label)));
        let progress_chip = progress.as_ref().map(|(state, label)| {
            div()
                .flex_none()
                .px(px(4.))
                .rounded(px(3.))
                .bg(ui.subtle)
                .text_size(px(9.))
                .text_color(
                    if matches!(state, paneflow_terminal_ghostty::ProgressState::Error) {
                        ui.agent_error
                    } else {
                        ui.muted
                    },
                )
                .child(label.clone())
                .into_any_element()
        });

        let match_badge = {
            let slots_used: u8 = leading_slots + u8::from(progress.is_some());
            self.surface
                .as_terminal()
                .and(self.search_hits)
                .filter(|count| *count > 0 && slots_used < 2)
                .map(|count| {
                    div()
                        .flex_none()
                        .px(px(4.))
                        .rounded(px(3.))
                        .bg(ui.subtle)
                        .text_size(px(9.))
                        .text_color(ui.accent)
                        .child(format!("{count} hits"))
                        .into_any_element()
                })
        };

        let identity = div()
            .id("pane-header-identity")
            .flex()
            .flex_row()
            .items_center()
            .min_w_0()
            .max_w_full()
            .h_full()
            .gap(px(HEADER_GAP))
            .overflow_x_hidden()
            .text_color(ui.muted)
            .on_click(cx.listener(|this, _e: &ClickEvent, window, cx| {
                this.focus_handle(cx).focus(window, cx);
                cx.notify();
                cx.stop_propagation();
            }))
            .child(self.render_surface_title(cx))
            .children(status_dot)
            .children(pending_chip)
            .children(progress_chip)
            .children(match_badge);

        let close_tooltip = format!(
            "Close pane ({})",
            crate::keybindings::format_keystroke(
                self.cached_config
                    .shortcuts
                    .iter()
                    .find(|(_, action)| action.as_str() == "close_pane")
                    .map(|(key, _)| key.as_str())
                    .unwrap_or("secondary-shift-w"),
            )
        );
        let close_button = div()
            .id("pane-btn-close-slot")
            .flex_none()
            .invisible()
            .group_hover(HEADER_GROUP, |style| style.visible())
            .delayed_tooltip(crate::ui_primitives::text_tooltip(close_tooltip))
            .child(self.render_close_button(cx));

        div()
            .id("pane-header")
            .group(HEADER_GROUP)
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .h(px(PANE_HEADER_HEIGHT))
            .w_full()
            .px(px(SECTION_PX))
            .gap(px(HEADER_GAP))
            .overflow_hidden()
            .on_drag(
                PaneDrag {
                    pane_id: cx.entity().entity_id().as_u64(),
                    title: SharedString::from(Self::surface_title(&self.surface, cx)),
                    icon: SharedString::from(Self::surface_icon(&self.surface)),
                },
                |drag, _offset, _window, cx| {
                    cx.new(|_| DragPreview {
                        title: drag.title.clone(),
                        icon: drag.icon.clone(),
                    })
                },
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_this, e: &MouseDownEvent, _window, cx| {
                    cx.emit(PaneEvent::OpenPaneMenu {
                        position: e.position,
                    });
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h_full()
                    .child(close_button),
            )
            .child(identity)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .h_full()
                    .child(self.render_end_section(cx)),
            )
    }

    fn render_end_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = pane_colors();
        let end_section = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .h_full()
            .gap(px(0.));

        let is_diff = matches!(self.surface, PaneSurface::Diff(_));
        let show_sessions_button = !is_diff
            && !crate::agent_sessions::enabled_session_agents_from_config(&self.cached_config)
                .is_empty();

        let mut action_cluster = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .h_full()
            .gap(px(HEADER_GAP));

        if self.zoomed {
            action_cluster = action_cluster.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(4.))
                    .h(px(18.))
                    .rounded(px(3.))
                    .bg(ui.accent)
                    .text_size(px(10.))
                    .text_color(ui.base)
                    .child("Z"),
            );
        }

        action_cluster = match &self.surface {
            PaneSurface::Diff(diff) => {
                action_cluster.child(self.render_diff_options_button(diff.clone(), cx))
            }
            _ => action_cluster
                .child(self.action_button(
                    "pane-btn-split-v",
                    "icons/split_vertical.svg",
                    cx.listener(|_this, _, _window, cx| {
                        cx.emit(PaneEvent::Split(crate::layout::SplitDirection::Vertical));
                    }),
                    cx,
                ))
                .child(self.action_button(
                    "pane-btn-split-h",
                    "icons/split_horizontal.svg",
                    cx.listener(|_this, _, _window, cx| {
                        cx.emit(PaneEvent::Split(crate::layout::SplitDirection::Horizontal));
                    }),
                    cx,
                )),
        };
        action_cluster = action_cluster
            .when(show_sessions_button, |s| {
                s.child(self.action_button(
                    "pane-btn-claude-sessions",
                    "icons/sessions.svg",
                    cx.listener(|_this, _e: &ClickEvent, _window, cx| {
                        cx.emit(PaneEvent::ToggleAgentSessions);
                        cx.stop_propagation();
                    }),
                    cx,
                ))
            })
            .when(!is_diff, |s| {
                s.child(self.action_button(
                    "pane-btn-diff-dock",
                    "icons/layout-sidebar-right.svg",
                    cx.listener(|_this, _e: &ClickEvent, _window, cx| {
                        cx.emit(PaneEvent::ToggleDiffDock);
                        cx.stop_propagation();
                    }),
                    cx,
                ))
            });

        end_section.child(action_cluster)
    }

    fn render_diff_options_button(
        &self,
        diff: Entity<crate::diff::DiffView>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::app::diff_dock::{
            DiffOptionsMenuActions, DiffOptionsMenuState, OptionChoice, render_diff_options_menu,
        };

        let open = self.diff_options_open;
        let trigger = self.action_button(
            "pane-btn-diff-options",
            "icons/dots.svg",
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.diff_options_open = !open;
                if !this.diff_options_open {
                    this.diff_options_submenu = None;
                }
                cx.stop_propagation();
                cx.notify();
            }),
            cx,
        );
        let menu = open.then(|| {
            let ui = crate::theme::ui_colors();
            let view = diff.read(cx);
            let state = DiffOptionsMenuState {
                split: view.is_split(),
                options: view.options(),
                submenu: self.diff_options_submenu,
                all_collapsed: view.has_changes().then(|| view.all_collapsed()),
            };
            let pane = cx.weak_entity();
            let close: Rc<dyn Fn(&mut App)> = {
                let pane = pane.clone();
                Rc::new(move |cx| {
                    let _ = pane.update(cx, |this, cx| {
                        this.diff_options_open = false;
                        this.diff_options_submenu = None;
                        cx.notify();
                    });
                })
            };
            let actions = DiffOptionsMenuActions {
                toggle_submenu: Rc::new(move |submenu, cx| {
                    let _ = pane.update(cx, |this, cx| {
                        this.diff_options_submenu = if this.diff_options_submenu == Some(submenu) {
                            None
                        } else {
                            Some(submenu)
                        };
                        cx.notify();
                    });
                }),
                choose: {
                    let diff = diff.clone();
                    Rc::new(move |choice, cx| {
                        diff.update(cx, |view, cx| match choice {
                            OptionChoice::Layout(split) => view.set_split(split, cx),
                            OptionChoice::Diff(options) => view.set_options(options, cx),
                        });
                    })
                },
                set_all_collapsed: {
                    let diff = diff.clone();
                    Rc::new(move |collapse, cx| {
                        diff.update(cx, |view, cx| view.set_all_collapsed(collapse, cx));
                    })
                },
                refresh: {
                    let diff = diff.clone();
                    Rc::new(move |cx| {
                        diff.update(cx, |view, cx| view.refresh(cx));
                    })
                },
                dismiss: close,
            };
            render_diff_options_menu(ACTION_BUTTON_SIZE + 6., state, actions, ui)
        });

        div()
            .relative()
            .flex_none()
            .child(trigger)
            .children(menu)
            .into_any_element()
    }
}

impl Focusable for Pane {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.surface {
            PaneSurface::Terminal(t) => t.read(cx).focus_handle(cx),
            PaneSurface::Markdown(m) => m.read(cx).focus_handle(cx),
            PaneSurface::Diff(d) => d.read(cx).focus_handle(cx),
        }
    }
}

fn cached_surface_style() -> StyleRefinement {
    StyleRefinement::default().size_full()
}

impl Render for Pane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal_selected = matches!(self.surface, PaneSurface::Terminal(_));
        let body = match &self.surface {
            PaneSurface::Terminal(t) => t.clone().cached(cached_surface_style()).into_any_element(),
            PaneSurface::Markdown(m) => m.clone().into_any_element(),
            PaneSurface::Diff(d) => d.clone().into_any_element(),
        };
        let theme = crate::theme::active_theme();
        let card_background = pane_card_background(
            &theme,
            self.cached_config.windows_terminal_material_enabled(),
            terminal_selected,
        );

        let dim_target = if self.dimmed && self.composer_slot.is_none() {
            self.cached_config.resolved_unfocused_pane_dim_alpha()
        } else {
            0.0
        };
        let dim_fill = theme.background;
        let dim_from = self.dim_from;
        let dim_live = self.dim_alpha.clone();
        let dim_layer = (dim_target > PANE_DIM_EPSILON || self.dim_alpha.get() > PANE_DIM_EPSILON)
            .then(|| {
                let distance = (dim_target - dim_from).abs();
                if self.dim_seq == 0 || distance <= f32::EPSILON {
                    dim_live.set(dim_target);
                    return squircle_fill(
                        crate::app::constants::PANE_CARD_RADIUS,
                        dim_fill.opacity(dim_target),
                    )
                    .into_any_element();
                }
                let anim_id = SharedString::from(format!(
                    "pane-dim-{}-{}",
                    cx.entity().entity_id().as_u64(),
                    self.dim_seq
                ));
                let duration = Duration::from_secs_f32(
                    Duration::from_millis(PANE_DIM_FADE_MS).as_secs_f32() * distance,
                );
                div()
                    .absolute()
                    .inset_0()
                    .with_animation(
                        anim_id,
                        Animation::new(duration).with_easing(ease_out_quint()),
                        move |layer, delta| {
                            let alpha =
                                (dim_from + (dim_target - dim_from) * delta).clamp(0.0, 1.0);
                            dim_live.set(alpha);
                            layer.child(squircle_fill(
                                crate::app::constants::PANE_CARD_RADIUS,
                                dim_fill.opacity(alpha),
                            ))
                        },
                    )
                    .into_any_element()
            });

        let group_name =
            SharedString::from(format!("pane-content-{}", cx.entity().entity_id().as_u64()));

        let (cw, ch) = (
            self.overlay_pane_size.width.as_f32(),
            self.overlay_pane_size.height.as_f32(),
        );
        let from_rect = self.overlay_from;
        let to_rect = split_rect(self.drag_split_direction, cw, ch);
        let live_rect = self.overlay_current.clone();
        let overlay_anim_id = SharedString::from(format!(
            "pane-overlay-{}-{}",
            cx.entity().entity_id().as_u64(),
            self.overlay_seq
        ));

        let overlay_blue = Hsla::from(rgb(DROP_OVERLAY_BLUE));
        let swap_tint = pane_colors().text;
        let overlay = div()
            .absolute()
            .bg(overlay_blue.opacity(DROP_OVERLAY_BACKGROUND_ALPHA))
            .rounded(px(OVERLAY_RADIUS))
            .border_2()
            .border_color(overlay_blue)
            .invisible()
            .group_drag_over::<SessionDrag>(group_name.clone(), |s| s.visible())
            .group_drag_over::<ReviewSubjectDrag>(group_name.clone(), |s| s.visible())
            .group_drag_over::<PaneDrag>(group_name.clone(), move |s| {
                s.visible()
                    .bg(swap_tint.opacity(SWAP_OVERLAY_FILL_ALPHA))
                    .border_color(swap_tint.opacity(SWAP_OVERLAY_BORDER_ALPHA))
            })
            .on_drop(cx.listener(move |this, drag: &PaneDrag, _window, cx| {
                let edge = this.drag_split_direction.take();
                cx.emit(PaneEvent::DropPaneMove {
                    source_pane_id: drag.pane_id,
                    edge,
                });
                cx.notify();
            }))
            .on_drop(cx.listener(move |this, drag: &SessionDrag, _window, cx| {
                let edge = this.drag_split_direction.take();
                cx.emit(PaneEvent::DropSessionSplit {
                    edge,
                    agent: drag.agent,
                    session_id: drag.session_id.clone(),
                    cwd: drag.cwd.clone(),
                });
                cx.notify();
            }))
            .on_drop(
                cx.listener(move |this, drag: &ReviewSubjectDrag, _window, cx| {
                    let edge = this.drag_split_direction.take();
                    cx.emit(PaneEvent::DropSubjectSplit {
                        edge,
                        subject: drag.subject.clone(),
                    });
                    cx.notify();
                }),
            )
            .with_animation(
                overlay_anim_id,
                Animation::new(Duration::from_millis(130)).with_easing(ease_out_quint()),
                move |overlay, delta| {
                    let lerp = |a: f32, b: f32| a + (b - a) * delta;
                    let raw = (
                        lerp(from_rect.0, to_rect.0),
                        lerp(from_rect.1, to_rect.1),
                        lerp(from_rect.2, to_rect.2),
                        lerp(from_rect.3, to_rect.3),
                    );
                    let m = OVERLAY_MARGIN;
                    let cur = (
                        raw.0 + m,
                        raw.1 + m,
                        (raw.2 - 2.0 * m).max(0.0),
                        (raw.3 - 2.0 * m).max(0.0),
                    );
                    live_rect.set(raw);
                    overlay
                        .left(px(cur.0))
                        .top(px(cur.1))
                        .w(px(cur.2))
                        .h(px(cur.3))
                },
            );

        let content = div()
            .id("pane-content")
            .group(group_name)
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .overflow_hidden()
            .on_drag_move::<SessionDrag>(cx.listener(
                |this, e: &DragMoveEvent<SessionDrag>, _window, cx| {
                    this.apply_drag_edge(e.bounds, e.event.position, cx);
                },
            ))
            .on_drag_move::<PaneDrag>(cx.listener(
                |this, e: &DragMoveEvent<PaneDrag>, _window, cx| {
                    this.apply_drag_edge(e.bounds, e.event.position, cx);
                },
            ))
            .on_drag_move::<ReviewSubjectDrag>(cx.listener(
                |this, e: &DragMoveEvent<ReviewSubjectDrag>, _window, cx| {
                    this.apply_drag_edge(e.bounds, e.event.position, cx);
                },
            ))
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().w_full().child(body))
            .children(dim_layer)
            .child(overlay);

        let has_attention = self.attention.is_some();
        let attention_color = pane_colors().vc_conflict;
        let composer = self.render_composer_overlay(cx);
        let card_radius = crate::app::constants::PANE_CARD_RADIUS;
        div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .overflow_hidden()
            .child(squircle_fill(card_radius, card_background))
            .child(content)
            .when_some(self.broadcast_stripe, |d, idx| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(card_radius)
                        .bottom(card_radius)
                        .w(px(3.))
                        .bg(pane_colors().group_color(idx)),
                )
            })
            .child(squircle_border(
                card_radius,
                px(1.),
                if has_attention {
                    attention_color.opacity(0.7)
                } else {
                    pane_colors().border
                },
            ))
            .children(composer)
    }
}

fn progress_chip_label(report: paneflow_terminal_ghostty::ProgressReport) -> Option<SharedString> {
    use paneflow_terminal_ghostty::ProgressState;

    match report.state {
        ProgressState::Set | ProgressState::Error => Some(match report.percent {
            Some(percent) => SharedString::from(format!("{percent}%")),
            None if matches!(report.state, ProgressState::Error) => {
                SharedString::new_static("error")
            }
            None => SharedString::new_static("working"),
        }),
        ProgressState::Indeterminate => Some(SharedString::new_static("working")),
        ProgressState::Pause => Some(SharedString::new_static("paused")),
        ProgressState::Remove => None,
    }
}

#[cfg(test)]
mod tests {
    use paneflow_terminal_ghostty::{ProgressReport, ProgressState};

    use super::{
        MAX_SURFACE_TITLE_LEN, pane_card_background, progress_chip_label, truncate_surface_title,
    };

    #[test]
    fn progress_chip_label_prefers_the_percentage_and_names_every_other_state() {
        let label = |state, percent| progress_chip_label(ProgressReport { state, percent });

        assert_eq!(label(ProgressState::Set, Some(42)).as_deref(), Some("42%"));
        assert_eq!(
            label(ProgressState::Error, Some(80)).as_deref(),
            Some("80%")
        );
        assert_eq!(label(ProgressState::Set, None).as_deref(), Some("working"));
        assert_eq!(label(ProgressState::Error, None).as_deref(), Some("error"));
        assert_eq!(
            label(ProgressState::Indeterminate, Some(10)).as_deref(),
            Some("working")
        );
        assert_eq!(
            label(ProgressState::Pause, Some(10)).as_deref(),
            Some("paused")
        );
        assert_eq!(label(ProgressState::Remove, None), None);
    }

    #[test]
    fn terminal_material_scopes_the_card_to_windows_terminal_surfaces() {
        let theme = crate::theme::paneflow_dark();

        assert_eq!(pane_card_background(&theme, true, false), theme.background);
        assert_eq!(pane_card_background(&theme, false, true), theme.background);

        let material = pane_card_background(&theme, true, true);
        #[cfg(target_os = "windows")]
        assert_eq!(material.a, 0.0);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(material, theme.background);
    }

    #[test]
    fn short_titles_pass_through_unchanged() {
        assert_eq!(truncate_surface_title("README.md"), "README.md");
        assert_eq!(truncate_surface_title("Terminal"), "Terminal");
    }

    #[test]
    fn exactly_max_chars_is_not_truncated() {
        let s: String = "x".repeat(MAX_SURFACE_TITLE_LEN);
        assert_eq!(truncate_surface_title(&s), s);
    }

    #[test]
    fn over_max_gets_ellipsis() {
        let input = "prd-opencode-sessions.mdX";
        let out = truncate_surface_title(input);
        assert_eq!(out.chars().count(), MAX_SURFACE_TITLE_LEN);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn multibyte_utf8_does_not_panic() {
        let input = "événement-très-très-long-fichier.md";
        let out = truncate_surface_title(input);
        assert_eq!(out.chars().count(), MAX_SURFACE_TITLE_LEN);
        assert!(out.ends_with('…'));
        let cjk = "プロジェクト・パネフロー・テスト・ドキュメント.md";
        let out = truncate_surface_title(cjk);
        assert_eq!(out.chars().count(), MAX_SURFACE_TITLE_LEN);
    }

    #[test]
    fn cwd_label_uses_last_path_component() {
        let cwd = std::env::temp_dir().join("paneflow-tab-title");

        assert_eq!(
            super::Pane::cwd_label(&cwd.to_string_lossy()),
            Some("paneflow-tab-title".into())
        );
    }

    #[test]
    fn agent_title_detection_uses_exact_command_token() {
        assert_eq!(
            super::Pane::agent_title_from_terminal_title("codex"),
            Some("Codex")
        );
        assert_eq!(
            super::Pane::agent_title_from_terminal_title("codex.exe"),
            Some("Codex")
        );
        assert_eq!(
            super::Pane::agent_title_from_terminal_title("user@host: /repo/codex-adapter"),
            None,
            "repo names must not be mistaken for agent processes"
        );
    }
}
