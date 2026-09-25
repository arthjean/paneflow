use crate::ui_primitives::TooltipDelayExt;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, ClickEvent, Context, DragMoveEvent, Entity,
    EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseUpEvent, Pixels, Point, Render, SharedString, Size, StyleRefinement,
    Styled, Window, deferred, div, ease_out_quint, img, prelude::*, px, rgb, svg,
};

use crate::ui_primitives::squircle::{squircle_border, squircle_fill};
use crate::ui_primitives::{AnimatedHoverExt, lerp_color, squircle_skin};

use crate::markdown::MarkdownView;
use crate::pane_drag::{
    DragPreview, DropEdge, PaneDrag, SPLIT_EDGE_BAND, SessionDrag, SurfaceDrag, compute_drop_edge,
    split_rect,
};
use crate::terminal::{TerminalEvent, TerminalView};

#[derive(Clone)]
pub enum PaneSurface {
    Terminal(Entity<TerminalView>),
    Markdown(Entity<MarkdownView>),
}

impl PaneSurface {
    pub(crate) fn entity_id(&self) -> u64 {
        match self {
            Self::Terminal(view) => view.entity_id().as_u64(),
            Self::Markdown(view) => view.entity_id().as_u64(),
        }
    }
    pub fn as_terminal(&self) -> Option<&Entity<TerminalView>> {
        match self {
            PaneSurface::Terminal(t) => Some(t),
            PaneSurface::Markdown(_) => None,
        }
    }

    pub(crate) fn kind_icon(&self) -> &'static str {
        match self {
            PaneSurface::Terminal(_) => "icons/terminal.svg",
            PaneSurface::Markdown(_) => "icons/file-text.svg",
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
        if theme.background.l <= 0.5 {
            theme.background.opacity(0.35)
        } else {
            gpui::transparent_black()
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        theme.background
    }
}

const HEADER_GAP: f32 = 7.0;
const SECTION_PX: f32 = crate::app::constants::PANE_CONTENT_INSET_X;
const ACTION_BUTTON_SIZE: f32 = 22.0;

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
pub const MAX_PANE_TABS: usize = 8;
const TAB_BAR_HEIGHT: f32 = 26.0;
const TAB_BAR_GAP: f32 = 3.0;
const TAB_BAR_BOTTOM_INSET: f32 = 6.0;
const TAB_ICON_SIZE: f32 = 13.0;
const NEW_TAB_MENU_WIDTH: f32 = 216.0;
const TAB_CLOSE_SIZE: f32 = 16.0;
const TAB_CLOSE_GLYPH_SIZE: f32 = 11.0;
const TAB_FADE_WIDTH: f32 = 28.0;

fn truncate_surface_title(raw: &str) -> String {
    if raw.chars().count() <= MAX_SURFACE_TITLE_LEN {
        return raw.to_string();
    }
    let head: String = raw.chars().take(MAX_SURFACE_TITLE_LEN - 1).collect();
    format!("{head}…")
}

pub enum PaneEvent {
    ToggleDetached {
        window: gpui::AnyWindowHandle,
    },
    Remove,
    NewTab,
    OpenNewTabMenu,
    NewTabPreset(crate::app::pane_palette::Preset),
    SurfacesChanged,
    CloseRequested,
    CloseSurfaceRequested(Entity<crate::terminal::TerminalView>),
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
    DropSurfaceMove {
        source_pane_id: u64,
        surface_id: u64,
        edge: Option<DropEdge>,
    },
    DropPaneMove {
        source_pane_id: u64,
        edge: Option<DropEdge>,
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
    pub(crate) detached: Option<crate::app::detached_panes::DetachedPanePlacement>,
    surfaces: Vec<PaneSurface>,
    active_surface: usize,
    attention: Option<String>,
    errored: bool,
    search_hits: Option<usize>,
    pub zoomed: bool,
    pub workspace_id: u64,
    header_hover_motion: std::collections::HashMap<SharedString, HeaderHoverMotion>,
    tab_scroll: gpui::ScrollHandle,
    new_tab_menu: Option<Vec<crate::app::pane_palette::Preset>>,
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
        Self::new_with_surfaces(vec![surface], 0, workspace_id, cx)
    }

    pub fn new_with_surfaces(
        surfaces: Vec<PaneSurface>,
        active_surface: usize,
        workspace_id: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        debug_assert!(!surfaces.is_empty(), "a pane needs at least one surface");
        let cached_config = paneflow_config::loader::load_config();
        for surface in &surfaces {
            if let PaneSurface::Terminal(t) = surface {
                Self::subscribe_terminal(t, cx);
                Self::apply_terminal_render_config(t, &cached_config, cx);
            }
        }
        let active_surface = active_surface.min(surfaces.len().saturating_sub(1));
        Self {
            surfaces,
            detached: None,
            active_surface,
            attention: None,
            errored: false,
            search_hits: None,
            zoomed: false,
            workspace_id,
            header_hover_motion: std::collections::HashMap::new(),
            tab_scroll: gpui::ScrollHandle::new(),
            new_tab_menu: None,
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
        }
    }

    pub fn surface(&self) -> &PaneSurface {
        &self.surfaces[self.active_surface]
    }

    pub(crate) fn is_detached(&self) -> bool {
        self.detached.is_some()
    }

    pub(crate) fn window_title(&self, cx: &App) -> String {
        Self::surface_full_title(self.surface(), cx)
    }

    pub(crate) fn toggle_detached(&self, window: &Window, cx: &mut Context<Self>) {
        cx.emit(PaneEvent::ToggleDetached {
            window: window.window_handle(),
        });
    }

    pub fn surfaces(&self) -> &[PaneSurface] {
        &self.surfaces
    }

    pub fn active_surface_idx(&self) -> usize {
        self.active_surface
    }

    pub(crate) fn reveal_active_surface(&self) {
        self.tab_scroll.scroll_to_item(self.active_surface);
    }

    pub fn can_add_surface(&self) -> bool {
        self.surfaces.len() < MAX_PANE_TABS
    }

    pub fn push_surface(&mut self, surface: PaneSurface, cx: &mut Context<Self>) {
        if let PaneSurface::Terminal(t) = &surface {
            Self::subscribe_terminal(t, cx);
            Self::apply_terminal_render_config(t, &self.cached_config, cx);
        }
        self.surfaces.push(surface);
        self.active_surface = self.surfaces.len() - 1;
        self.tab_scroll.scroll_to_item(self.active_surface);
        cx.notify();
    }

    pub fn activate_surface(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx < self.surfaces.len() && idx != self.active_surface {
            self.active_surface = idx;
            self.tab_scroll.scroll_to_item(idx);
            cx.notify();
        }
    }

    pub(crate) fn close_surface(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.surfaces.len() {
            return;
        }
        if self.surfaces.len() == 1 {
            cx.emit(PaneEvent::CloseRequested);
            return;
        }
        match crate::app::hosted_sessions::surface_terminal(&self.surfaces[idx]) {
            Some(terminal) => cx.emit(PaneEvent::CloseSurfaceRequested(terminal)),
            None => self.remove_surface_at(idx, cx),
        }
    }

    pub(crate) fn remove_surface(
        &mut self,
        terminal: &Entity<crate::terminal::TerminalView>,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = self
            .surfaces
            .iter()
            .position(|surface| surface.as_terminal() == Some(terminal))
        {
            self.remove_surface_at(idx, cx);
        }
    }

    pub(crate) fn remove_surface_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.surfaces.len() {
            return;
        }
        if self.surfaces.len() == 1 {
            cx.emit(PaneEvent::Remove);
            return;
        }
        self.surfaces.remove(idx);
        if self.active_surface > idx || self.active_surface >= self.surfaces.len() {
            self.active_surface = self.active_surface.saturating_sub(1);
        }
        cx.emit(PaneEvent::SurfacesChanged);
        cx.notify();
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
        self.surfaces.iter().filter_map(PaneSurface::as_terminal)
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
        let minimum_contrast = config.terminal.as_ref().map_or_else(
            || paneflow_config::schema::TerminalConfig::default().resolved_minimum_contrast(),
            paneflow_config::schema::TerminalConfig::resolved_minimum_contrast,
        );
        let cursor_color_override = config
            .terminal
            .as_ref()
            .and_then(|terminal| terminal.cursor_color.as_deref())
            .and_then(crate::terminal::view::hsla_from_hex_color);
        let option_as_meta = config
            .option_as_meta
            .unwrap_or_else(crate::keys::default_option_as_meta);
        terminal.update(cx, |terminal, cx| {
            terminal.set_option_as_meta(option_as_meta);
            terminal.set_integrated_glyphs_enabled(integrated_glyphs_enabled, cx);
            terminal.set_color_emoji_enabled(color_emoji_enabled, cx);
            terminal.set_minimum_contrast(minimum_contrast, cx);
            terminal.set_cursor_color_override(cursor_color_override, cx);
            cx.notify();
        });
    }

    pub fn contains_terminal(&self, terminal: &Entity<TerminalView>) -> bool {
        self.terminals().any(|t| t == terminal)
    }

    fn subscribe_terminal(terminal: &Entity<TerminalView>, cx: &mut Context<Self>) {
        cx.subscribe(terminal, |_, _, event: &TerminalEvent, cx| match event {
            TerminalEvent::ChildExited
            | TerminalEvent::TitleChanged
            | TerminalEvent::HostLinkResolved => {
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
            | TerminalEvent::ProgramNotification { .. }
            | TerminalEvent::ShellPromptReady => {}
        })
        .detach();
    }

    fn surface_full_title(surface: &PaneSurface, cx: &App) -> String {
        match surface {
            PaneSurface::Markdown(md) => md.read(cx).title().to_string(),
            PaneSurface::Terminal(t) => Self::terminal_surface_full_title(t, cx),
        }
    }

    fn surface_title(surface: &PaneSurface, cx: &App) -> String {
        let raw = match surface {
            PaneSurface::Markdown(md) => md.read(cx).title().to_string(),
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
        let button = div()
            .id(id.clone())
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(size))
            .h(px(size))
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
            .active(|style| style.opacity(0.82));

        let visual = div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(base_tint)
            .child(icon);

        let distance = (target - from).abs();
        let visual = if epoch == 0 || distance <= f32::EPSILON {
            live_progress.set(target);
            let tint = hover_tint
                .map(|hover_tint| base_tint.blend(hover_tint.opacity(target)))
                .unwrap_or(base_tint);
            visual.text_color(tint).into_any_element()
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
                        visual.text_color(tint)
                    },
                )
                .into_any_element()
        };

        squircle_skin(
            button,
            SharedString::from(format!("pane-action-skin-{id}")),
            crate::ui_primitives::ROW_RADIUS,
            None,
            Some(hover_background),
        )
        .child(visual)
        .into_any_element()
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
            base_tint,
            hover_tint,
            handler,
            cx,
        )
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(PaneEvent::CloseRequested);
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
        self.surface().as_terminal()
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
            .surface()
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
            self.surface()
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

        let has_identity = status_dot.is_some()
            || pending_chip.is_some()
            || progress_chip.is_some()
            || match_badge.is_some();
        let identity = has_identity.then(|| {
            div()
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
                .children(status_dot)
                .children(pending_chip)
                .children(progress_chip)
                .children(match_badge)
        });

        div()
            .id("pane-header")
            .group(HEADER_GROUP)
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .h_full()
            .pr(px(SECTION_PX))
            .gap(px(HEADER_GAP))
            .overflow_hidden()
            .on_drag(
                PaneDrag {
                    pane_id: cx.entity().entity_id().as_u64(),
                    title: SharedString::from(Self::surface_title(self.surface(), cx)),
                    icon: SharedString::from(Self::surface_icon(self.surface())),
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
            .children(identity)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap(px(HEADER_GAP))
                    .h_full()
                    .child(self.render_end_section(cx)),
            )
    }

    pub(crate) fn open_new_tab_menu(
        &mut self,
        presets: Vec<crate::app::pane_palette::Preset>,
        cx: &mut Context<Self>,
    ) {
        self.new_tab_menu = Some(presets);
        cx.notify();
    }

    pub(crate) fn close_new_tab_menu(&mut self, cx: &mut Context<Self>) {
        if self.new_tab_menu.take().is_some() {
            cx.notify();
        }
    }

    fn render_new_tab_menu(
        &self,
        pane_id: u64,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let presets = self.new_tab_menu.as_ref()?;
        let (button_size, button_inset) = if self.is_detached() {
            (32., 7.)
        } else {
            (ACTION_BUTTON_SIZE, 0.)
        };
        let mut menu = crate::settings::components::menu_panel(
            div().id(SharedString::from(format!("pane-{pane_id}-new-tab-menu"))),
            ui,
        )
        .w(px(NEW_TAB_MENU_WIDTH))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                this.close_new_tab_menu(cx);
            }),
        );
        for (index, preset) in presets.iter().enumerate() {
            menu = menu.child(self.render_new_tab_menu_row(pane_id, index, preset, ui, cx));
        }
        Some(
            deferred(crate::ui_primitives::menu_reveal(
                SharedString::from(format!("pane-{pane_id}-new-tab-menu-reveal")),
                div()
                    .absolute()
                    .top(px(button_size + 8.))
                    .left(px(button_inset + button_size - NEW_TAB_MENU_WIDTH))
                    .occlude()
                    .child(menu),
            ))
            .with_priority(4)
            .into_any_element(),
        )
    }

    fn render_new_tab_menu_row(
        &self,
        pane_id: u64,
        index: usize,
        preset: &crate::app::pane_palette::Preset,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let launchable = preset.ensure_launchable().is_ok();
        let icon_path = preset.icon_path();
        let icon = if preset.icon_multicolor() {
            img(icon_path).size(px(15.)).flex_none().into_any_element()
        } else {
            svg()
                .size(px(15.))
                .flex_none()
                .path(icon_path)
                .text_color(
                    preset
                        .accent()
                        .map_or(ui.muted, |accent| rgb(accent).into()),
                )
                .into_any_element()
        };
        let chosen = preset.clone();
        crate::settings::components::menu_row(
            SharedString::from(format!("pane-{pane_id}-new-tab-{index}")),
            false,
            ui,
        )
        .gap(px(9.))
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.close_new_tab_menu(cx);
            cx.emit(PaneEvent::NewTabPreset(chosen.clone()));
            cx.stop_propagation();
        }))
        .child(icon)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(13.))
                .text_color(if launchable { ui.text } else { ui.muted })
                .child(preset.label.clone()),
        )
        .when(!launchable, |row| {
            row.child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .child("not installed"),
            )
        })
        .into_any_element()
    }

    pub(crate) fn render_tab_bar(
        &self,
        unified_background: Option<gpui::Hsla>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let unified = unified_background.is_some();
        let ui = pane_colors();
        let pane_id = cx.entity().entity_id().as_u64();
        let rail_hover = crate::app::constants::sidebar_tab_hover_background();
        let rail_active = crate::app::constants::sidebar_tab_active_background();

        let mut strip = div()
            .id("pane-tab-strip")
            .flex()
            .flex_row()
            .items_center()
            .h_full()
            .gap(px(TAB_BAR_GAP))
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll);

        for (index, surface) in self.surfaces.iter().enumerate() {
            let active = index == self.active_surface;
            let full_title = Self::surface_full_title(surface, cx);
            let label = Self::surface_title(surface, cx);
            let (resting, hovered) = if active {
                (Some(rail_active), None)
            } else {
                (None, Some(rail_hover))
            };
            let text = if active { ui.text } else { ui.muted };
            let unified_active_border = if ui.base.l > 0.5 {
                ui.accent.opacity(0.14)
            } else {
                ui.text.opacity(0.12)
            };
            let unified_active_shadow = if ui.base.l > 0.5 {
                gpui::BoxShadow::new(px(0.), px(1.), gpui::black().opacity(0.10))
                    .blur_radius(px(3.))
            } else {
                gpui::BoxShadow::new(px(0.), px(1.), gpui::black().opacity(0.24))
                    .blur_radius(px(2.))
            };
            let group = SharedString::from(format!("pane-{pane_id}-tab-{index}-group"));
            let chip = div()
                .id(SharedString::from(format!("pane-{pane_id}-tab-{index}")))
                .flex_none()
                .h(px(if unified { 32. } else { TAB_BAR_HEIGHT }))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .pl(px(8.))
                .pr(px(4.))
                .cursor(gpui::CursorStyle::Arrow);
            let chip = if unified {
                chip.group(group.clone())
                    .rounded_full()
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .w(px(208.))
                    .max_w_full()
                    .min_w(px(96.))
                    .pl(px(12.))
                    .pr(px(8.))
                    .gap(px(10.))
                    .when(active, |chip| {
                        chip.bg(if ui.base.l > 0.5 { ui.base } else { ui.overlay })
                            .border_color(unified_active_border)
                            .shadow(vec![unified_active_shadow])
                    })
                    .when(!active, |chip| chip.hover(|style| style.bg(rail_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            } else {
                squircle_skin(
                    chip,
                    group.clone(),
                    crate::ui_primitives::ROW_RADIUS,
                    resting,
                    hovered,
                )
            };
            let chip = chip
                .on_drag(
                    SurfaceDrag {
                        pane_id,
                        surface_id: surface.entity_id(),
                        title: Self::surface_title(surface, cx).into(),
                        icon: surface.kind_icon().into(),
                    },
                    |drag, _, _, cx| {
                        cx.new(|_| DragPreview {
                            title: drag.title.clone(),
                            icon: drag.icon.clone(),
                        })
                    },
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.activate_surface(index, cx);
                    this.focus_handle(cx).focus(window, cx);
                    cx.stop_propagation();
                }))
                .delayed_tooltip(crate::ui_primitives::text_tooltip(full_title))
                .child(if unified && matches!(surface, PaneSurface::Terminal(_)) {
                    img("icons/terminal-tab.svg")
                        .w(px(20.))
                        .h(px(17.))
                        .flex_none()
                        .into_any_element()
                } else {
                    svg()
                        .size(px(TAB_ICON_SIZE))
                        .when(unified, |icon| icon.w(px(20.)).h(px(17.)))
                        .flex_none()
                        .path(surface.kind_icon())
                        .text_color(ui.muted)
                        .into_any_element()
                })
                .child(
                    div()
                        .when(unified, |label| label.flex_1().min_w_0().text_ellipsis())
                        .when(!unified, |label| label.flex_none())
                        .whitespace_nowrap()
                        .text_size(crate::ui_primitives::BODY)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(text)
                        .child(if unified {
                            Self::surface_full_title(surface, cx)
                        } else {
                            label
                        }),
                )
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "pane-{pane_id}-tab-close-{index}"
                        )))
                        .flex_none()
                        .size(px(TAB_CLOSE_SIZE))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .when(unified, |button| button.rounded_full())
                        .invisible()
                        .group_hover(group, |style| style.visible())
                        .hover(|style| style.bg(ui.text.opacity(0.12)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.close_surface(index, cx);
                            cx.stop_propagation();
                        }))
                        .child(
                            svg()
                                .size(px(TAB_CLOSE_GLYPH_SIZE))
                                .when(unified, |icon| icon.size(px(12.)))
                                .flex_none()
                                .path("icons/close.svg")
                                .text_color(ui.muted),
                        ),
                );
            strip = strip.child(chip);
        }

        let fade_color = if let Some(background) = unified_background {
            background
        } else {
            pane_card_background(
                &crate::theme::active_theme(),
                self.cached_config.windows_terminal_material_enabled(),
                true,
            )
        };
        let fade_scroll = self.tab_scroll.clone();
        let fades = gpui::canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                if fade_color.a <= f32::EPSILON {
                    return;
                }
                let hidden_right = fade_scroll.max_offset().x + fade_scroll.offset().x;
                let hidden_left = -fade_scroll.offset().x;
                let fade_width = px(TAB_FADE_WIDTH).min(bounds.size.width);
                if hidden_right > px(1.) {
                    let fade = gpui::Bounds {
                        origin: gpui::point(bounds.right() - fade_width, bounds.top()),
                        size: gpui::size(fade_width, bounds.size.height),
                    };
                    window.paint_quad(gpui::fill(
                        fade,
                        gpui::linear_gradient(
                            90.,
                            gpui::linear_color_stop(fade_color.opacity(0.), 0.),
                            gpui::linear_color_stop(fade_color, 1.),
                        ),
                    ));
                }
                if hidden_left > px(1.) {
                    let fade = gpui::Bounds {
                        origin: bounds.origin,
                        size: gpui::size(fade_width, bounds.size.height),
                    };
                    window.paint_quad(gpui::fill(
                        fade,
                        gpui::linear_gradient(
                            90.,
                            gpui::linear_color_stop(fade_color, 0.),
                            gpui::linear_color_stop(fade_color.opacity(0.), 1.),
                        ),
                    ));
                }
            },
        )
        .absolute()
        .inset_0();

        let mut bar = div()
            .id("pane-tab-bar")
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(if unified {
                44.
            } else {
                TAB_BAR_HEIGHT + TAB_BAR_BOTTOM_INSET
            }))
            .pb(px(if unified { 0. } else { TAB_BAR_BOTTOM_INSET }))
            .px(px(if unified { 2. } else { SECTION_PX }))
            .when(unified && !self.is_detached(), |bar| bar.pl(px(6.)))
            .when(unified && self.is_detached(), |bar| bar.pl(px(0.)))
            .gap(px(TAB_BAR_GAP))
            .overflow_hidden();

        if unified && self.is_detached() {
            bar = bar.child(
                squircle_skin(
                    div()
                        .id(SharedString::from(format!("pane-{pane_id}-tab-reattach")))
                        .flex_none()
                        .size(px(32.))
                        .mr(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor(gpui::CursorStyle::Arrow),
                    SharedString::from(format!("pane-{pane_id}-tab-reattach-group")),
                    crate::ui_primitives::ROW_RADIUS,
                    None,
                    Some(rail_hover),
                )
                .delayed_tooltip(crate::ui_primitives::text_tooltip("Return to workspace"))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.toggle_detached(window, cx);
                    cx.stop_propagation();
                }))
                .child(
                    svg()
                        .size(px(20.))
                        .flex_none()
                        .path("icons/dock-pane.svg")
                        .text_color(ui.muted),
                ),
            );
        }

        bar = bar.child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(strip)
                .child(fades),
        );

        if self.can_add_surface() {
            let new_tab_button = squircle_skin(
                div()
                    .id(SharedString::from(format!("pane-{pane_id}-tab-new")))
                    .flex_none()
                    .size(px(TAB_BAR_HEIGHT))
                    .when(unified && self.is_detached(), |button| {
                        button.size(px(32.)).mx(px(7.))
                    })
                    .when(!self.is_detached(), |button| {
                        button
                            .size(px(ACTION_BUTTON_SIZE))
                            .when(self.dimmed, |button| button.invisible())
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(gpui::CursorStyle::Arrow),
                SharedString::from(format!("pane-{pane_id}-tab-new-group")),
                crate::ui_primitives::ROW_RADIUS,
                None,
                Some(rail_hover),
            )
            .when(self.new_tab_menu.is_none(), |button| {
                button.delayed_tooltip(crate::ui_primitives::text_tooltip("New tab"))
            })
            .when(unified, |button| {
                button.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                if this.new_tab_menu.take().is_some() {
                    cx.notify();
                } else {
                    cx.emit(PaneEvent::OpenNewTabMenu);
                }
                cx.stop_propagation();
            }))
            .child(
                svg()
                    .size(px(TAB_ICON_SIZE))
                    .when(unified, |icon| {
                        icon.size(px(if self.is_detached() { 20. } else { 14. }))
                    })
                    .flex_none()
                    .path("icons/plus.svg")
                    .text_color(ui.muted),
            );
            bar = bar.child(
                div()
                    .relative()
                    .flex_none()
                    .child(new_tab_button)
                    .children(self.render_new_tab_menu(pane_id, ui, cx)),
            );
        }

        if !self.is_detached() {
            bar = bar
                .child(
                    div()
                        .flex_none()
                        .w(px(1.))
                        .h(px(16.))
                        .mx(px(6.))
                        .when(self.dimmed, |divider| divider.invisible())
                        .bg(ui.border),
                )
                .child(
                    div()
                        .h_full()
                        .flex_none()
                        .when(self.dimmed, |actions| actions.invisible())
                        .child(self.render_header(cx)),
                );
        }
        Some(bar.into_any_element())
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

        let detach = div()
            .id("pane-detach-control")
            .delayed_tooltip(crate::ui_primitives::text_tooltip(if self.is_detached() {
                "Return to workspace"
            } else {
                "Detach pane into a window"
            }))
            .child(self.action_button(
                "pane-btn-detach",
                if self.is_detached() {
                    "icons/dock-pane.svg"
                } else {
                    "icons/detach-pane.svg"
                },
                cx.listener(|this, _, window, cx| {
                    this.toggle_detached(window, cx);
                    cx.stop_propagation();
                }),
                cx,
            ));
        if self.is_detached() {
            return end_section.child(detach);
        }

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

        action_cluster = action_cluster
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
            ))
            .child(self.action_button(
                "pane-btn-diff-dock",
                "icons/layout-sidebar-right.svg",
                cx.listener(|_this, _e: &ClickEvent, _window, cx| {
                    cx.emit(PaneEvent::ToggleDiffDock);
                    cx.stop_propagation();
                }),
                cx,
            ));

        end_section.child(action_cluster.child(detach))
    }
}

impl Focusable for Pane {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.surface() {
            PaneSurface::Terminal(t) => t.read(cx).focus_handle(cx),
            PaneSurface::Markdown(m) => m.read(cx).focus_handle(cx),
        }
    }
}

fn cached_surface_style() -> StyleRefinement {
    StyleRefinement::default().size_full()
}

impl Render for Pane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal_selected = matches!(self.surface(), PaneSurface::Terminal(_));
        let body = match self.surface() {
            PaneSurface::Terminal(t) => t.clone().cached(cached_surface_style()).into_any_element(),
            PaneSurface::Markdown(m) => m.clone().into_any_element(),
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
            .group_drag_over::<PaneDrag>(group_name.clone(), move |s| {
                s.visible()
                    .bg(swap_tint.opacity(SWAP_OVERLAY_FILL_ALPHA))
                    .border_color(swap_tint.opacity(SWAP_OVERLAY_BORDER_ALPHA))
            })
            .group_drag_over::<SurfaceDrag>(group_name.clone(), move |s| {
                s.visible()
                    .bg(swap_tint.opacity(SWAP_OVERLAY_FILL_ALPHA))
                    .border_color(swap_tint.opacity(SWAP_OVERLAY_BORDER_ALPHA))
            })
            .on_drop(cx.listener(|this, drag: &SurfaceDrag, _, cx| {
                cx.emit(PaneEvent::DropSurfaceMove {
                    source_pane_id: drag.pane_id,
                    surface_id: drag.surface_id,
                    edge: this.drag_split_direction.take(),
                });
                cx.notify();
            }))
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
            .on_drag_move::<SurfaceDrag>(cx.listener(
                |this, e: &DragMoveEvent<SurfaceDrag>, _, cx| {
                    this.apply_drag_edge(e.bounds, e.event.position, cx);
                },
            ))
            .when(!self.is_detached(), |root| {
                root.children(self.render_tab_bar(Some(card_background), cx))
            })
            .child(div().flex_1().min_h_0().w_full().child(body))
            .children(dim_layer)
            .child(overlay);

        let has_attention = self.attention.is_some();
        let attention_color = pane_colors().vc_conflict;
        let composer = self.render_composer_overlay(cx);
        let card_radius = if self.is_detached() {
            px(10.)
        } else {
            crate::app::constants::PANE_CARD_RADIUS
        };
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

    use gpui::{AppContext, Entity, TestAppContext};

    use super::{
        MAX_SURFACE_TITLE_LEN, Pane, PaneEvent, PaneSurface, pane_card_background,
        progress_chip_label, truncate_surface_title,
    };
    use crate::terminal::TerminalView;

    fn terminal_surface(cx: &mut impl AppContext) -> PaneSurface {
        PaneSurface::Terminal(cx.new(|cx| TerminalView::display_only_for_test(1, cx)))
    }

    fn tabbed_pane(count: usize, cx: &mut impl AppContext) -> Entity<Pane> {
        let surfaces: Vec<PaneSurface> = (0..count).map(|_| terminal_surface(cx)).collect();
        cx.new(|cx| Pane::new_with_surfaces(surfaces, 0, 1, cx))
    }

    fn tab_state(pane: &Entity<Pane>, cx: &mut gpui::VisualTestContext) -> (usize, usize) {
        cx.update(|_, cx| {
            let pane = pane.read(cx);
            (pane.surfaces().len(), pane.active_surface_idx())
        })
    }

    #[gpui::test]
    fn push_surface_activates_the_new_tab_and_close_keeps_the_neighbor(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let pane = tabbed_pane(1, cx);
        let extra = cx.update(|_, cx| terminal_surface(cx));
        pane.update(cx, |pane, cx| pane.push_surface(extra, cx));
        assert_eq!(tab_state(&pane, cx), (2, 1));

        pane.update(cx, |pane, cx| pane.activate_surface(0, cx));
        assert_eq!(tab_state(&pane, cx), (2, 0));

        pane.update(cx, |pane, cx| pane.remove_surface_at(0, cx));
        assert_eq!(tab_state(&pane, cx), (1, 0));
    }

    #[gpui::test]
    fn closing_a_tab_asks_the_owner_before_anything_is_removed(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let pane = tabbed_pane(2, cx);
        let asked = std::rc::Rc::new(std::cell::Cell::new(false));
        let asked_for_sub = asked.clone();
        cx.update(|_, cx| {
            cx.subscribe(&pane, move |_, event: &PaneEvent, _| {
                if matches!(event, PaneEvent::CloseSurfaceRequested(_)) {
                    asked_for_sub.set(true);
                }
            })
            .detach();
        });

        pane.update(cx, |pane, cx| pane.close_surface(0, cx));
        assert!(asked.get(), "closing a tab is a request, not a removal");
        assert_eq!(tab_state(&pane, cx), (2, 0), "nothing was removed yet");
    }

    #[gpui::test]
    fn closing_a_tab_before_the_active_one_shifts_the_active_index(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let pane = tabbed_pane(3, cx);
        pane.update(cx, |pane, cx| pane.activate_surface(2, cx));
        let active_before =
            cx.update(|_, cx| pane.read(cx).surface().as_terminal().map(Entity::entity_id));

        pane.update(cx, |pane, cx| pane.remove_surface_at(0, cx));
        assert_eq!(tab_state(&pane, cx), (2, 1));
        let active_after =
            cx.update(|_, cx| pane.read(cx).surface().as_terminal().map(Entity::entity_id));
        assert_eq!(active_after, active_before);
    }

    #[gpui::test]
    fn closing_the_last_tab_asks_to_remove_the_pane(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let pane = tabbed_pane(1, cx);
        let requested = std::rc::Rc::new(std::cell::Cell::new(false));
        let requested_for_sub = requested.clone();
        let removed = std::rc::Rc::new(std::cell::Cell::new(false));
        let removed_for_sub = removed.clone();
        cx.update(|_, cx| {
            cx.subscribe(&pane, move |_, event: &PaneEvent, _| match event {
                PaneEvent::CloseRequested => requested_for_sub.set(true),
                PaneEvent::Remove => removed_for_sub.set(true),
                _ => {}
            })
            .detach();
        });

        pane.update(cx, |pane, cx| pane.close_surface(0, cx));
        assert!(
            requested.get(),
            "closing the last tab asks the owner to close the pane"
        );
        assert!(
            !removed.get(),
            "no removal happens before the owner decides"
        );
        assert_eq!(tab_state(&pane, cx), (1, 0));
    }

    #[gpui::test]
    fn a_natural_exit_keeps_the_surface_as_a_passive_final_view(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        for count in [1usize, 2] {
            let pane = tabbed_pane(count, cx);
            let terminal = cx.update(|_, cx| {
                pane.read(cx).surfaces()[0]
                    .as_terminal()
                    .expect("a terminal surface")
                    .clone()
            });
            let events = std::rc::Rc::new(std::cell::Cell::new(0usize));
            let events_for_sub = events.clone();
            cx.update(|_, cx| {
                cx.subscribe(&pane, move |_, _: &PaneEvent, _| {
                    events_for_sub.set(events_for_sub.get() + 1);
                })
                .detach();
            });

            terminal.update(cx, |_, cx| {
                cx.emit(crate::terminal::TerminalEvent::ChildExited)
            });

            assert_eq!(
                events.get(),
                0,
                "US-009: a natural exit never asks the owner to stop or remove anything"
            );
            assert_eq!(tab_state(&pane, cx), (count, 0));
            cx.update(|_, cx| {
                assert!(
                    pane.read(cx).contains_terminal(&terminal),
                    "US-009: the exited terminal stays in place as a final view"
                );
            });
        }
    }

    #[gpui::test]
    fn moving_a_surface_keeps_the_terminal_and_its_final_view_in_the_destination(
        cx: &mut TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let source = tabbed_pane(2, cx);
        let target = tabbed_pane(1, cx);
        let surface = cx.update(|_, cx| source.read(cx).surfaces()[0].clone());
        let terminal = surface.as_terminal().unwrap().clone();
        target.update(cx, |pane, cx| pane.push_surface(surface, cx));
        source.update(cx, |pane, cx| pane.remove_surface_at(0, cx));
        cx.update(|_, cx| {
            assert!(!source.read(cx).contains_terminal(&terminal));
            assert_eq!(target.read(cx).active_terminal_opt(), Some(&terminal));
        });
        terminal.update(cx, |_, cx| {
            cx.emit(crate::terminal::TerminalEvent::ChildExited)
        });
        assert_eq!(tab_state(&source, cx), (1, 0));
        assert_eq!(tab_state(&target, cx), (2, 1));
        cx.update(|_, cx| {
            assert_eq!(target.read(cx).active_terminal_opt(), Some(&terminal));
        });
    }

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
        assert_eq!(material, theme.background.opacity(0.35));
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
