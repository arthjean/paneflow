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

use crate::ui_primitives::action_menu::{ActionMenu, MenuHandlers, MenuItem};
use crate::ui_primitives::squircle::{
    layer_clearance, squircle_band_path, squircle_border, squircle_corners_border,
    squircle_corners_fill, squircle_corners_fill_beneath_border, squircle_fill,
    squircle_fill_beneath_border, squircle_path, squircle_stroke_path,
};
use crate::ui_primitives::{AccessibleControlExt, squircle_skin};

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
    Browser(Entity<crate::browser::BrowserView>),
}

impl PaneSurface {
    pub(crate) fn entity_id(&self) -> u64 {
        match self {
            Self::Terminal(view) => view.entity_id().as_u64(),
            Self::Markdown(view) => view.entity_id().as_u64(),
            Self::Browser(view) => view.entity_id().as_u64(),
        }
    }
    pub fn as_terminal(&self) -> Option<&Entity<TerminalView>> {
        match self {
            PaneSurface::Terminal(t) => Some(t),
            PaneSurface::Markdown(_) | PaneSurface::Browser(_) => None,
        }
    }

    pub(crate) fn kind_icon(&self) -> &'static str {
        match self {
            PaneSurface::Terminal(_) => "icons/terminal.svg",
            PaneSurface::Markdown(_) => "icons/file-text.svg",
            PaneSurface::Browser(_) => "icons/world.svg",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitFocus {
    Alone,
    Focused,
    Unfocused,
}

impl SplitFocus {
    fn dims(self) -> bool {
        self == Self::Unfocused
    }
}

fn hide_until_hover<E: Styled + InteractiveElement>(element: E, group: &SharedString) -> E {
    element
        .invisible()
        .group_hover(group.clone(), |style| style.visible())
}

fn pane_colors() -> crate::theme::UiColors {
    crate::theme::ui_colors()
}

pub(crate) fn pane_card_background(
    theme: &crate::theme::TerminalTheme,
    backdrop: crate::window_chrome::veil::Backdrop,
    terminal_selected: bool,
) -> Hsla {
    if terminal_selected {
        backdrop.terminal_card(theme.background)
    } else {
        theme.background
    }
}

#[derive(Clone, Copy)]
pub(crate) enum CardShape {
    Squircle(Pixels),
    Flush(gpui::Edges<bool>),
    Morph {
        radii: gpui::Corners<Pixels>,
        comfort: f32,
    },
}

impl From<crate::layout::CardFrame> for CardShape {
    fn from(frame: crate::layout::CardFrame) -> Self {
        match frame {
            crate::layout::CardFrame::Card => {
                Self::Squircle(crate::app::constants::PANE_CARD_RADIUS)
            }
            crate::layout::CardFrame::Flush(edges) => Self::Flush(edges),
            crate::layout::CardFrame::Morph { outer, comfort } => Self::Morph {
                radii: crate::layout::morph_corners(outer, comfort),
                comfort,
            },
        }
    }
}

impl CardShape {
    fn overlay(self, color: Hsla) -> AnyElement {
        match self {
            Self::Squircle(radius) => squircle_fill(radius, color),
            Self::Flush(edges) => crate::layout::flush_fill(edges, color).into_any_element(),
            Self::Morph { radii, .. } => squircle_corners_fill(radii, color),
        }
    }

    pub(crate) fn background(self, color: Hsla, border: Option<Hsla>) -> AnyElement {
        let (border_width, border_color) = card_border_edge(border);
        match self {
            Self::Squircle(radius) => pane_card_fill(radius, color, border),
            Self::Flush(edges) => crate::layout::flush_fill(edges, color).into_any_element(),
            Self::Morph { radii, .. } if card_paints_quads(color) => {
                squircle_corners_fill(radii, color)
            }
            Self::Morph { radii, .. } => {
                squircle_corners_fill_beneath_border(radii, color, border_width, border_color)
            }
        }
    }

    pub(crate) fn border(self, background: Hsla, color: Hsla, outline: bool) -> AnyElement {
        match self {
            Self::Squircle(radius) if card_paints_quads(background) => div()
                .absolute()
                .inset_0()
                .rounded(radius)
                .border_1()
                .border_color(color)
                .into_any_element(),
            Self::Squircle(radius) => {
                squircle_border(radius, CARD_BORDER_WIDTH, color).into_any_element()
            }
            Self::Flush(edges) if outline => {
                crate::layout::flush_outline(edges, color).into_any_element()
            }
            Self::Flush(edges) => crate::layout::flush_border(edges, color).into_any_element(),
            Self::Morph { radii, .. } if card_paints_quads(background) => div()
                .absolute()
                .inset_0()
                .rounded_tl(radii.top_left)
                .rounded_tr(radii.top_right)
                .rounded_br(radii.bottom_right)
                .rounded_bl(radii.bottom_left)
                .border_1()
                .border_color(color)
                .into_any_element(),
            Self::Morph { radii, .. } => {
                squircle_corners_border(radii, CARD_BORDER_WIDTH, color).into_any_element()
            }
        }
    }

    fn drop_veil_radius(self) -> Pixels {
        match self {
            Self::Squircle(radius) => crate::ui_primitives::interface_style::pick(
                (radius - px(OVERLAY_MARGIN)).max(px(OVERLAY_RADIUS)),
                px(OVERLAY_RADIUS),
            ),
            Self::Flush(_) | Self::Morph { .. } => px(OVERLAY_RADIUS),
        }
    }

    pub(crate) fn tab_chip_radius(self) -> Option<Pixels> {
        if crate::ui_primitives::interface_style::classic() {
            return Some(crate::ui_primitives::interface_style::CLASSIC_CONTROL);
        }
        let compact = crate::app::constants::PANEL_CORNER_RADIUS - px(UNIFIED_TAB_BAR_INSET);
        match self {
            Self::Squircle(_) => None,
            Self::Flush(_) => Some(compact),
            Self::Morph { comfort, .. } => {
                Some(compact + (px(UNIFIED_TAB_HEIGHT / 2.) - compact) * comfort)
            }
        }
    }

    fn drop_veil(self, background: Hsla, fill: Hsla, stroke: Hsla) -> AnyElement {
        let radius = self.drop_veil_radius();
        if !matches!(self, Self::Squircle(_)) || card_paints_quads(background) {
            return div()
                .absolute()
                .inset_0()
                .rounded(radius)
                .bg(fill)
                .border_2()
                .border_color(stroke)
                .into_any_element();
        }
        div()
            .absolute()
            .inset_0()
            .child(squircle_fill_beneath_border(radius, fill, px(2.), stroke))
            .child(squircle_border(radius, px(2.), stroke))
            .into_any_element()
    }
}

fn card_paints_quads(background: Hsla) -> bool {
    cfg!(target_os = "linux")
        && background.a < 1.0
        && !crate::ui_primitives::interface_style::circular()
}

fn card_border_edge(border: Option<Hsla>) -> (Pixels, Hsla) {
    border.map_or((px(0.), gpui::transparent_black()), |color| {
        (CARD_BORDER_WIDTH, color)
    })
}

fn pane_card_fill(radius: Pixels, color: Hsla, border: Option<Hsla>) -> AnyElement {
    if card_paints_quads(color) {
        div()
            .absolute()
            .inset_0()
            .rounded(radius)
            .bg(color)
            .into_any_element()
    } else {
        let (border_width, border_color) = card_border_edge(border);
        squircle_fill_beneath_border(radius, color, border_width, border_color)
    }
}

const CARD_BORDER_WIDTH: Pixels = px(1.);
const HEADER_GAP: f32 = 7.0;
const SECTION_PX: f32 = crate::app::constants::PANE_CONTENT_INSET_X;
const ACTION_BUTTON_SIZE: f32 = 22.0;

const HEADER_GROUP: &str = "pane-header-group";
const HEADER_HOVER_MS: u64 = 120;
const PANE_DIM_FADE_MS: u64 = 130;
const FOCUSED_PANE_BORDER_ALPHA: f32 = 0.24;
const UNFOCUSED_PANE_TITLE_ALPHA: f32 = 0.65;
const PANE_DIM_EPSILON: f32 = 0.002;
const OVERLAY_MARGIN: f32 = 8.0;
const UNIFIED_TAB_BAR_INSET: f32 = 6.0;
const UNIFIED_TAB_HEIGHT: f32 = 32.0;
const UNIFIED_TAB_WIDTH: f32 = 208.0;
const UNIFIED_TAB_MIN_WIDTH: f32 = 96.0;
const UNIFIED_TAB_ICON_ONLY_INSET: f32 = 10.0;
const UNIFIED_TAB_BORDER_WIDTH: f32 = 1.0;
const UNIFIED_TAB_ICON_ONLY_WIDTH: f32 =
    2. * UNIFIED_TAB_ICON_ONLY_INSET + TAB_BADGE_WIDTH + 2. * UNIFIED_TAB_BORDER_WIDTH;
const _: () = assert!(
    UNIFIED_TAB_ICON_ONLY_WIDTH + UNIFIED_TAB_BAR_INSET + 2. <= crate::layout::MIN_PANE_SIZE
);
const TAB_BAR_DIVIDER_WIDTH: f32 = 1.0;
const TAB_BAR_DIVIDER_MARGIN: f32 = 6.0;
const DETACHED_TAB_BUTTON_SIZE: f32 = 32.0;
const DETACHED_NEW_TAB_MARGIN: f32 = 7.0;
const DETACHED_REATTACH_MARGIN: f32 = 3.0;
pub(crate) const UNIFIED_TAB_CLOSE_GLYPH_SIZE: f32 = 12.0;
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
const NEW_TAB_DIGIT_MNEMONICS: &[char] = &['1', '2', '3', '4', '5', '6', '7', '8', '9'];
const TAB_CLOSE_SIZE: f32 = 16.0;
const TAB_CLOSE_GLYPH_SIZE: f32 = 11.0;
pub(crate) const TAB_FADE_WIDTH: f32 = 28.0;
pub(crate) const TAB_BADGE_WIDTH: f32 = 22.0;
pub(crate) const TAB_BADGE_HEIGHT: f32 = 18.0;
const TAB_BADGE_RADIUS: f32 = 5.5;
const TAB_BADGE_LOGO_SIZE: f32 = 10.0;
const TAB_BADGE_GRAPHITE_TOP: u32 = 0x3a3c3e;
const TAB_BADGE_GRAPHITE_BOTTOM: u32 = 0x1d2021;
const TAB_BADGE_GRAPHITE_BOTTOM_STOP: f32 = 0.6;
const TAB_BADGE_GRAPHITE_RING: u32 = 0x080b0c;
const TAB_BADGE_GRAPHITE_LOGO: u32 = 0xe8eaeb;
const TAB_BADGE_TINT_SHEEN: f32 = 0.1;
const TAB_BADGE_TINT_SHADE: f32 = 0.95;
const TAB_BADGE_RING_SHADE: f32 = 0.2;
const TAB_BADGE_RIM_ALPHA: f32 = 0.05;
const TAB_BADGE_HIGHLIGHT_ALPHA: f32 = 0.22;
const TAB_BADGE_DARK_GLYPH_LUMA: f32 = 0.6;

fn scale_rgb(color: gpui::Rgba, factor: f32) -> gpui::Rgba {
    gpui::Rgba {
        r: color.r * factor,
        g: color.g * factor,
        b: color.b * factor,
        a: color.a,
    }
}

fn lift_rgb(color: gpui::Rgba, amount: f32) -> gpui::Rgba {
    gpui::Rgba {
        r: color.r + (1. - color.r) * amount,
        g: color.g + (1. - color.g) * amount,
        b: color.b + (1. - color.b) * amount,
        a: color.a,
    }
}

pub(crate) fn tab_badge(agent: Option<crate::agent_launcher::TerminalAgent>) -> AnyElement {
    let logo_size = px(TAB_BADGE_LOGO_SIZE);
    let graphite = gpui::linear_gradient(
        180.,
        gpui::linear_color_stop(rgb(TAB_BADGE_GRAPHITE_TOP), 0.),
        gpui::linear_color_stop(
            rgb(TAB_BADGE_GRAPHITE_BOTTOM),
            TAB_BADGE_GRAPHITE_BOTTOM_STOP,
        ),
    );
    let graphite_ring: Hsla = rgb(TAB_BADGE_GRAPHITE_RING).into();
    let (background, ring, glyph) = match agent {
        None => (
            graphite,
            graphite_ring,
            img("icons/terminal-tab.svg")
                .w(px(TAB_BADGE_WIDTH))
                .h(px(TAB_BADGE_HEIGHT))
                .flex_none()
                .into_any_element(),
        ),
        Some(agent) => match agent.accent() {
            Some(tint) if !agent.icon_multicolor() => {
                let tint = rgb(tint);
                let luma = 0.2126 * tint.r + 0.7152 * tint.g + 0.0722 * tint.b;
                let logo_color = if luma > TAB_BADGE_DARK_GLYPH_LUMA {
                    gpui::black()
                } else {
                    gpui::white()
                };
                let (background, ring) = tinted_badge_colors(tint);
                (
                    background,
                    ring,
                    crate::settings::components::render_logo(
                        agent.icon_path(),
                        false,
                        logo_size,
                        logo_color,
                    ),
                )
            }
            _ => (
                graphite,
                graphite_ring,
                crate::settings::components::render_logo(
                    agent.icon_path(),
                    agent.icon_multicolor(),
                    logo_size,
                    rgb(TAB_BADGE_GRAPHITE_LOGO).into(),
                ),
            ),
        },
    };
    badge_tile(background, ring, glyph)
}

pub(crate) fn unified_tab_chip(
    id: SharedString,
    group: SharedString,
    active: bool,
    radius: Option<Pixels>,
) -> gpui::Stateful<gpui::Div> {
    let ui = pane_colors();
    let light = ui.base.l > 0.5;
    let active_border = if light {
        ui.accent.opacity(0.14)
    } else {
        ui.text.opacity(0.12)
    };
    let (active_bg, hover_bg) = if light {
        (ui.card, ui.text.opacity(0.05))
    } else {
        (
            crate::app::constants::sidebar_tab_active_background(),
            crate::app::constants::sidebar_tab_hover_background(),
        )
    };
    let active_shadow = if light {
        gpui::BoxShadow::new(px(0.), px(1.), gpui::black().opacity(0.10)).blur_radius(px(3.))
    } else {
        gpui::BoxShadow::new(px(0.), px(1.), gpui::black().opacity(0.24)).blur_radius(px(2.))
    };
    div()
        .id(id)
        .group(group)
        .flex_none()
        .h(px(UNIFIED_TAB_HEIGHT))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .pl(px(12.))
        .pr(px(8.))
        .map(|chip| match radius {
            Some(radius) => chip.rounded(radius),
            None => chip.rounded_full(),
        })
        .border_1()
        .border_color(gpui::transparent_black())
        .cursor(gpui::CursorStyle::Arrow)
        .when(active, |chip| {
            chip.bg(active_bg)
                .border_color(active_border)
                .shadow(vec![active_shadow])
        })
        .when(!active, |chip| chip.hover(|style| style.bg(hover_bg)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

const SANDBOXED_TAB_TOOLTIP: &str = "Sandboxed: Paneflow and other panes are out of reach";

const SANDBOX_TAB_MARKER_SIZE: f32 = 12.;

pub(crate) fn sandbox_tab_marker(color: Hsla) -> AnyElement {
    svg()
        .size(px(SANDBOX_TAB_MARKER_SIZE))
        .flex_none()
        .path("icons/shield-sandbox.svg")
        .text_color(color)
        .into_any_element()
}

pub(crate) fn unified_tab_icon(path: &'static str, color: Hsla) -> AnyElement {
    svg()
        .w(px(20.))
        .h(px(17.))
        .flex_none()
        .path(path)
        .text_color(color)
        .into_any_element()
}

pub(crate) fn unified_tab_label(color: Hsla) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .text_ellipsis()
        .whitespace_nowrap()
        .text_size(crate::ui_primitives::BODY)
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(color)
}

pub(crate) fn tab_close_button(id: SharedString, pill: bool) -> gpui::Stateful<gpui::Div> {
    let ui = pane_colors();
    div()
        .id(id)
        .flex_none()
        .size(px(TAB_CLOSE_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .map(|button| {
            if pill {
                button.rounded_full()
            } else {
                button.rounded(px(4.))
            }
        })
        .hover(|style| style.bg(ui.text.opacity(0.12)))
}

pub(crate) fn tab_strip_fades(scroll: gpui::ScrollHandle, fade_color: Hsla) -> AnyElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            if fade_color.a <= f32::EPSILON {
                return;
            }
            let hidden_right = scroll.max_offset().x + scroll.offset().x;
            let hidden_left = -scroll.offset().x;
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
    .inset_0()
    .into_any_element()
}

pub(crate) fn tinted_badge_colors(tint: gpui::Rgba) -> (gpui::Background, Hsla) {
    (
        gpui::linear_gradient(
            180.,
            gpui::linear_color_stop(lift_rgb(tint, TAB_BADGE_TINT_SHEEN), 0.),
            gpui::linear_color_stop(scale_rgb(tint, TAB_BADGE_TINT_SHADE), 1.),
        ),
        scale_rgb(tint, TAB_BADGE_RING_SHADE).into(),
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BadgeGeometry {
    ring: (gpui::Bounds<Pixels>, Pixels),
    body: (gpui::Bounds<Pixels>, Pixels),
    highlight: (gpui::Bounds<Pixels>, gpui::Bounds<Pixels>, Pixels),
    rim: (gpui::Bounds<Pixels>, Pixels),
}

fn badge_geometry(bounds: gpui::Bounds<Pixels>, scale_factor: f32) -> BadgeGeometry {
    let radius = px(TAB_BADGE_RADIUS);
    let clearance = layer_clearance(scale_factor);
    let rim = bounds.inset(clearance);
    let highlight_outer = bounds.inset(clearance * 2.);
    let highlight_inner = gpui::Bounds {
        origin: highlight_outer.origin + gpui::point(px(0.), px(1.)),
        size: gpui::size(
            highlight_outer.size.width,
            highlight_outer.size.height - px(1.),
        ),
    };
    BadgeGeometry {
        ring: (bounds.dilate(px(1.)), radius + px(1.)),
        body: (bounds, radius),
        highlight: (highlight_outer, highlight_inner, radius - clearance * 2.),
        rim: (rim, radius - clearance),
    }
}

pub(crate) fn paint_badge_tile(
    bounds: gpui::Bounds<Pixels>,
    background: gpui::Background,
    ring: Hsla,
    window: &mut Window,
) {
    let geometry = badge_geometry(bounds, window.scale_factor());
    let highlight = gpui::white().opacity(TAB_BADGE_HIGHLIGHT_ALPHA);
    let rim = gpui::white().opacity(TAB_BADGE_RIM_ALPHA);
    let (outer, inner, highlight_radius) = geometry.highlight;
    let layers = [
        (
            squircle_path(geometry.ring.0, geometry.ring.1),
            gpui::Background::from(ring),
        ),
        (squircle_path(geometry.body.0, geometry.body.1), background),
        (
            squircle_band_path(outer, inner, highlight_radius),
            highlight.into(),
        ),
        (
            squircle_stroke_path(geometry.rim.0, geometry.rim.1, px(1.)),
            rim.into(),
        ),
    ];
    for (path, fill) in layers {
        if let Some(path) = path {
            window.paint_path(path, fill);
        }
    }
}

pub(crate) fn badge_tile(
    background: gpui::Background,
    ring: Hsla,
    glyph: AnyElement,
) -> AnyElement {
    let tile = gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| paint_badge_tile(bounds, background, ring, window),
    )
    .size_full();
    div()
        .relative()
        .w(px(TAB_BADGE_WIDTH))
        .h(px(TAB_BADGE_HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(div().absolute().inset_0().child(tile))
        .child(glyph)
        .into_any_element()
}

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
    NewTab,
    OpenNewTabMenu,
    NewTabPreset(crate::app::pane_palette::Preset),
    SurfacesChanged,
    ActiveSurfaceChanged,
    CloseRequested,
    CloseSurfaceRequested(Entity<crate::terminal::TerminalView>),
    CloseViewRequested(gpui::EntityId),
    Split(crate::layout::SplitDirection),
    ToggleAgentSessions,
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
    pub(crate) restored_layout_id: Option<u64>,
    surfaces: Vec<PaneSurface>,
    active_surface: usize,
    attention: bool,
    errored: bool,
    search_hits: Option<usize>,
    pub zoomed: bool,
    pub workspace_id: u64,
    header_hover_motion: std::collections::HashMap<SharedString, HeaderHoverMotion>,
    tab_scroll: gpui::ScrollHandle,
    new_tab_menu: Option<Vec<crate::app::pane_palette::Preset>>,
    pub cached_config: paneflow_config::schema::PaneFlowConfig,
    backdrop: crate::window_chrome::veil::Backdrop,
    drag_split_direction: Option<DropEdge>,
    overlay_prev_dir: Option<DropEdge>,
    overlay_from: (f32, f32, f32, f32),
    overlay_current: Rc<Cell<(f32, f32, f32, f32)>>,
    overlay_seq: usize,
    overlay_pane_size: Size<Pixels>,
    split_focus: SplitFocus,
    dim_from: f32,
    dim_alpha: Rc<Cell<f32>>,
    dim_seq: usize,
    card_frame: crate::layout::CardFrame,
    grid_edges: gpui::Edges<bool>,
    shimmer_drawn: Cell<bool>,
    shimmer_ticker: Option<gpui::Task<()>>,
    tab_bar_width: Rc<Cell<f32>>,
    tab_strip_width: Rc<Cell<f32>>,
    tab_bar_identity_width: Rc<Cell<f32>>,
    tab_bar_actions_width: Rc<Cell<f32>>,
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
        let cached_config = (*crate::config_snapshot::current(cx)).clone();
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
            restored_layout_id: None,
            active_surface,
            attention: false,
            errored: false,
            search_hits: None,
            zoomed: false,
            workspace_id,
            header_hover_motion: std::collections::HashMap::new(),
            tab_scroll: gpui::ScrollHandle::new(),
            new_tab_menu: None,
            cached_config,
            backdrop: crate::window_chrome::veil::Backdrop::THEMED,
            drag_split_direction: None,
            overlay_prev_dir: None,
            overlay_from: (0.0, 0.0, 0.0, 0.0),
            overlay_current: Rc::new(Cell::new((0.0, 0.0, 0.0, 0.0))),
            overlay_seq: 0,
            overlay_pane_size: Size::default(),
            split_focus: SplitFocus::Alone,
            dim_from: 0.0,
            dim_alpha: Rc::new(Cell::new(0.0)),
            dim_seq: 0,
            card_frame: crate::layout::CardFrame::Card,
            grid_edges: gpui::Edges::default(),
            shimmer_drawn: Cell::new(false),
            shimmer_ticker: None,
            tab_bar_width: Rc::new(Cell::new(0.)),
            tab_strip_width: Rc::new(Cell::new(0.)),
            tab_bar_identity_width: Rc::new(Cell::new(0.)),
            tab_bar_actions_width: Rc::new(Cell::new(0.)),
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

    #[cfg(test)]
    pub fn push_surface(&mut self, surface: PaneSurface, cx: &mut Context<Self>) {
        if let PaneSurface::Terminal(t) = &surface {
            Self::subscribe_terminal(t, cx);
            Self::apply_terminal_render_config(t, &self.cached_config, cx);
        }
        self.surfaces.push(surface);
        self.active_surface = self.surfaces.len() - 1;
        self.tab_scroll.scroll_to_item(self.active_surface);
        cx.emit(PaneEvent::ActiveSurfaceChanged);
        cx.notify();
    }

    pub(crate) fn project_surfaces(
        &mut self,
        surfaces: Vec<PaneSurface>,
        active: usize,
        cx: &mut Context<Self>,
    ) {
        if surfaces.is_empty() {
            return;
        }
        let same = self.surfaces.len() == surfaces.len()
            && self
                .surfaces
                .iter()
                .zip(&surfaces)
                .all(|(old, new)| old.entity_id() == new.entity_id());
        let mut changed = false;
        if !same {
            for surface in &surfaces {
                let known = self
                    .surfaces
                    .iter()
                    .any(|old| old.entity_id() == surface.entity_id());
                if !known && let PaneSurface::Terminal(terminal) = surface {
                    Self::subscribe_terminal(terminal, cx);
                    Self::apply_terminal_render_config(terminal, &self.cached_config, cx);
                }
            }
            self.surfaces = surfaces;
            changed = true;
        }
        let active = active.min(self.surfaces.len() - 1);
        if active != self.active_surface {
            self.active_surface = active;
            self.tab_scroll.scroll_to_item(active);
            changed = true;
        }
        if changed {
            cx.emit(PaneEvent::SurfacesChanged);
            cx.notify();
        }
    }

    pub fn activate_surface(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx < self.surfaces.len() && idx != self.active_surface {
            self.active_surface = idx;
            self.tab_scroll.scroll_to_item(idx);
            cx.emit(PaneEvent::ActiveSurfaceChanged);
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
            None => cx.emit(PaneEvent::CloseViewRequested(
                crate::app::layout_bridge::surface_entity(&self.surfaces[idx]),
            )),
        }
    }

    #[cfg(test)]
    pub(crate) fn remove_surface_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.surfaces.len() || self.surfaces.len() == 1 {
            return;
        }
        self.surfaces.remove(idx);
        if self.active_surface > idx || self.active_surface >= self.surfaces.len() {
            self.active_surface = self.active_surface.saturating_sub(1);
        }
        cx.emit(PaneEvent::SurfacesChanged);
        cx.notify();
    }

    pub fn set_attention(&mut self, attention: bool, cx: &mut Context<Self>) {
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

    pub(crate) fn set_split_focus(&mut self, focus: SplitFocus, cx: &mut Context<Self>) {
        if self.split_focus == focus {
            return;
        }
        if self.split_focus.dims() != focus.dims() {
            self.dim_from = self.dim_alpha.get();
            self.dim_seq = self.dim_seq.wrapping_add(1);
        }
        self.split_focus = focus;
        cx.notify();
    }

    fn dimmed(&self) -> bool {
        self.split_focus.dims()
    }

    pub(crate) fn set_card_frame(
        &mut self,
        frame: crate::layout::CardFrame,
        grid_edges: gpui::Edges<bool>,
        cx: &mut Context<Self>,
    ) {
        if self.card_frame != frame || self.grid_edges != grid_edges {
            self.card_frame = frame;
            self.grid_edges = grid_edges;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn card_frame(&self) -> crate::layout::CardFrame {
        self.card_frame
    }

    pub(crate) fn title_band_slot(&self) -> Option<crate::window_chrome::title_band::HeaderSlot> {
        if self.is_detached() {
            return None;
        }
        let outer = match self.card_frame {
            crate::layout::CardFrame::Card => gpui::Edges::default(),
            crate::layout::CardFrame::Flush(_) | crate::layout::CardFrame::Morph { .. } => {
                self.grid_edges
            }
        };
        crate::window_chrome::title_band::active().map(|band| band.header_slot(outer))
    }

    fn card_shape(&self) -> CardShape {
        if self.is_detached() {
            return CardShape::Squircle(px(10.));
        }
        CardShape::from(self.card_frame)
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
            | TerminalEvent::SelectionCopied
            | TerminalEvent::Notice(_)
            | TerminalEvent::OpenMarkdownPath(_)
            | TerminalEvent::OpenCodePath { .. }
            | TerminalEvent::OpenInBrowser { .. }
            | TerminalEvent::LinkMenu { .. }
            | TerminalEvent::PickCodePath { .. }
            | TerminalEvent::TreeFileDropped { .. }
            | TerminalEvent::FontZoomChanged
            | TerminalEvent::FleetSearchRequested { .. }
            | TerminalEvent::ProgramNotification { .. }
            | TerminalEvent::AgentSessionChanged
            | TerminalEvent::AgentDeclared
            | TerminalEvent::ConversationReady
            | TerminalEvent::ShellPromptReady => {}
        })
        .detach();
    }

    fn surface_full_title(surface: &PaneSurface, cx: &App) -> String {
        match surface {
            PaneSurface::Markdown(md) => md.read(cx).title().to_string(),
            PaneSurface::Browser(view) => view.read(cx).title(),
            PaneSurface::Terminal(t) => Self::terminal_surface_full_title(t, cx),
        }
    }

    fn surface_title(surface: &PaneSurface, cx: &App) -> String {
        let raw = match surface {
            PaneSurface::Markdown(md) => md.read(cx).title().to_string(),
            PaneSurface::Browser(view) => view.read(cx).title(),
            PaneSurface::Terminal(t) => Self::terminal_surface_title(t, cx),
        };
        truncate_surface_title(&raw)
    }

    fn surface_icon(surface: &PaneSurface) -> &'static str {
        surface.kind_icon()
    }

    pub(crate) fn terminal_surface_title(terminal: &Entity<TerminalView>, cx: &App) -> String {
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
        label: &'static str,
        icon_path: &'static str,
        handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = pane_colors();
        self.action_button_shell(
            SharedString::from(id),
            label,
            crate::settings::components::render_logo(icon_path, false, px(14.), ui.muted),
            ui.muted,
            Some(ui.text),
            handler,
            cx,
        )
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
        label: &'static str,
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
            .accessible_control(gpui::accesskit::Role::Button, label)
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
        let visual =
            if !header_hover_animates(epoch, distance, crate::ui_primitives::reduce_motion()) {
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

    #[allow(clippy::too_many_arguments)]
    fn action_button_shell(
        &self,
        id: SharedString,
        label: &'static str,
        icon: AnyElement,
        base_tint: Hsla,
        hover_tint: Option<Hsla>,
        handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.header_button_shell(
            id,
            label,
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

    fn sync_shimmer_clock(&mut self, cx: &mut Context<Self>) {
        if !self.shimmer_drawn.replace(false) {
            self.shimmer_ticker = None;
            return;
        }
        if self.shimmer_ticker.is_some() {
            return;
        }
        self.shimmer_ticker = Some(cx.spawn(async move |pane, cx| {
            loop {
                let executor = cx.background_executor().clone();
                let elapsed = executor.now().saturating_duration_since(
                    crate::ui_primitives::animation_clock::animation_epoch(),
                );
                executor
                    .timer(crate::ui_primitives::shimmer::until_next_shimmer_frame(
                        elapsed,
                    ))
                    .await;
                if pane.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
    }

    pub fn active_terminal_opt(&self) -> Option<&Entity<TerminalView>> {
        self.surface().as_terminal()
    }

    fn render_header(&self, show_actions: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = pane_colors();

        let has_attention = self.attention;
        let has_errored = self.errored;
        let status_dot = pane_status_label(has_errored, has_attention).map(|label| {
            let marker = div()
                .id("pane-status-marker")
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .accessible_control(gpui::accesskit::Role::Image, label);
            if has_errored {
                marker
                    .size(px(12.0))
                    .child(
                        svg()
                            .size(px(12.0))
                            .flex_none()
                            .path("icons/triangle-alert.svg")
                            .text_color(ui.agent_error),
                    )
                    .into_any_element()
            } else {
                marker
                    .w(px(6.0))
                    .h(px(6.0))
                    .rounded_full()
                    .bg(ui.vc_conflict)
                    .into_any_element()
            }
        });

        let leading_slots: u8 = u8::from(has_errored || has_attention);
        let progress = self
            .surface()
            .as_terminal()
            .filter(|_| leading_slots < 2)
            .and_then(|terminal| header_chip(&terminal.read(cx).terminal));
        let progress_chip = progress.as_ref().map(|(tone, label)| {
            let chip = div()
                .flex_none()
                .px(px(4.))
                .rounded(px(3.))
                .bg(ui.subtle)
                .text_size(px(9.))
                .text_color(if *tone == ChipTone::Error {
                    ui.agent_error
                } else {
                    ui.muted
                });
            if *tone == ChipTone::Working
                && !self.dimmed()
                && !crate::ui_primitives::reduce_motion()
            {
                self.shimmer_drawn.set(true);
                chip.child(crate::ui_primitives::shimmer::shimmer_text(
                    label.clone(),
                    ui.muted,
                    ui.text,
                    label.clone(),
                ))
                .into_any_element()
            } else {
                chip.child(label.clone()).into_any_element()
            }
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
                        .child(search_hits_label(count))
                        .into_any_element()
                })
        };

        let has_identity = status_dot.is_some() || progress_chip.is_some() || match_badge.is_some();
        if !has_identity {
            self.tab_bar_identity_width.set(0.);
        }
        let identity = has_identity.then(|| {
            div()
                .id("pane-header-identity")
                .relative()
                .child(width_probe(self.tab_bar_identity_width.clone()))
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
            .when(
                self.title_band_slot().is_some_and(|slot| slot.in_band),
                |header| header.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            )
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
            .children(show_actions.then(|| {
                div()
                    .flex_none()
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap(px(HEADER_GAP))
                    .h_full()
                    .child(self.render_end_section(cx))
                    .child(width_probe(self.tab_bar_actions_width.clone()))
            }))
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
        let digits = NEW_TAB_DIGIT_MNEMONICS
            .iter()
            .copied()
            .map(Some)
            .chain(std::iter::repeat(None));
        let rows = presets
            .iter()
            .zip(digits)
            .enumerate()
            .map(|(index, (preset, digit))| {
                let item = new_tab_menu_item(pane_id, index, preset);
                match digit {
                    Some(digit) => item.mnemonic(digit),
                    None => item,
                }
            });
        let menu = ActionMenu::new().items(rows);
        let handlers = MenuHandlers {
            activate: Rc::new(cx.listener(
                |this, preset: &crate::app::pane_palette::Preset, _window, cx| {
                    this.close_new_tab_menu(cx);
                    cx.emit(PaneEvent::NewTabPreset(preset.clone()));
                },
            )),
            dismiss: Rc::new(cx.listener(|this, _: &(), _window, cx| {
                this.close_new_tab_menu(cx);
            })),
        };
        Some(
            deferred(crate::ui_primitives::menu_reveal(
                SharedString::from(format!("pane-{pane_id}-new-tab-menu-reveal")),
                div()
                    .absolute()
                    .top(px(button_size + 8.))
                    .left(px(button_inset + button_size))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .occlude()
                            .child(menu.element(
                                SharedString::from(format!("pane-{pane_id}-new-tab-menu")),
                                handlers,
                                ui,
                            )),
                    ),
            ))
            .with_priority(4)
            .into_any_element(),
        )
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
        let content_group = SharedString::from(format!("pane-content-{pane_id}"));
        let chip_radius = self.card_shape().tab_chip_radius();
        let band_slot = self.title_band_slot().filter(|_| unified);
        let detached = self.is_detached();
        let (pad_left, pad_right) = if !unified {
            (SECTION_PX, SECTION_PX)
        } else if let Some(slot) = band_slot {
            (UNIFIED_TAB_BAR_INSET + slot.leading, 2. + slot.trailing)
        } else if detached {
            (0., 2.)
        } else {
            (UNIFIED_TAB_BAR_INSET, 2.)
        };
        let fit = if unified && self.tab_bar_width.get() > 0. {
            let reattach_width = if detached {
                DETACHED_TAB_BUTTON_SIZE + DETACHED_REATTACH_MARGIN + TAB_BAR_GAP
            } else {
                0.
            };
            let new_tab_width = if !self.can_add_surface() {
                0.
            } else if detached {
                DETACHED_TAB_BUTTON_SIZE + 2. * DETACHED_NEW_TAB_MARGIN + TAB_BAR_GAP
            } else {
                ACTION_BUTTON_SIZE + TAB_BAR_GAP
            };
            let identity_width = self.tab_bar_identity_width.get();
            let header_base_width = if detached {
                0.
            } else if identity_width > 0. {
                TAB_BAR_GAP + identity_width + SECTION_PX
            } else {
                TAB_BAR_GAP + SECTION_PX
            };
            let actions_width = if detached {
                0.
            } else {
                TAB_BAR_DIVIDER_WIDTH
                    + 2. * TAB_BAR_DIVIDER_MARGIN
                    + TAB_BAR_GAP
                    + HEADER_GAP
                    + self.tab_bar_actions_width.get()
            };
            fit_tab_bar(
                self.tab_bar_width.get()
                    - pad_left
                    - pad_right
                    - reattach_width
                    - header_base_width,
                new_tab_width,
                actions_width,
            )
        } else {
            TabBarFit {
                actions: true,
                new_tab: true,
                label: true,
            }
        };

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
            let sandboxed = match surface {
                PaneSurface::Terminal(terminal) => terminal.read(cx).sandbox_request().is_some(),
                _ => false,
            };
            let full_title = if sandboxed {
                format!(
                    "{}\n{SANDBOXED_TAB_TOOLTIP}",
                    Self::surface_full_title(surface, cx)
                )
            } else {
                Self::surface_full_title(surface, cx)
            };
            let label = Self::surface_title(surface, cx);
            let (resting, hovered) = if active {
                (Some(rail_active), None)
            } else {
                (None, Some(rail_hover))
            };
            let text = if active { ui.text } else { ui.muted };
            let text = if self.dimmed() {
                text.opacity(UNFOCUSED_PANE_TITLE_ALPHA)
            } else {
                text
            };
            let group = SharedString::from(format!("pane-{pane_id}-tab-{index}-group"));
            let chip_id = SharedString::from(format!("pane-{pane_id}-tab-{index}"));
            let chip = if unified {
                let chip = unified_tab_chip(chip_id, group.clone(), active, chip_radius);
                if fit.label {
                    chip.w(px(UNIFIED_TAB_WIDTH))
                        .max_w_full()
                        .min_w(px(UNIFIED_TAB_MIN_WIDTH))
                } else {
                    chip.w(px(UNIFIED_TAB_ICON_ONLY_WIDTH))
                        .px(px(UNIFIED_TAB_ICON_ONLY_INSET))
                        .justify_center()
                }
            } else {
                squircle_skin(
                    div()
                        .id(chip_id)
                        .flex_none()
                        .h(px(TAB_BAR_HEIGHT))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.))
                        .pl(px(8.))
                        .pr(px(4.))
                        .cursor(gpui::CursorStyle::Arrow),
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
                .child(match (unified, surface) {
                    (true, PaneSurface::Terminal(terminal)) => {
                        tab_badge(terminal.read(cx).terminal.detected_agent)
                    }
                    (true, _) => unified_tab_icon(surface.kind_icon(), ui.muted),
                    (false, _) => svg()
                        .size(px(TAB_ICON_SIZE))
                        .flex_none()
                        .path(surface.kind_icon())
                        .text_color(ui.muted)
                        .into_any_element(),
                })
                .children(fit.label.then(|| {
                    if unified {
                        unified_tab_label(text).child(Self::surface_full_title(surface, cx))
                    } else {
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_size(crate::ui_primitives::BODY)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(text)
                            .child(label)
                    }
                }))
                .children((fit.label && sandboxed).then(|| sandbox_tab_marker(ui.muted)))
                .children(fit.label.then(|| {
                    tab_close_button(
                        SharedString::from(format!("pane-{pane_id}-tab-close-{index}")),
                        unified && chip_radius.is_none(),
                    )
                    .invisible()
                    .group_hover(group, |style| style.visible())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.close_surface(index, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        svg()
                            .size(px(if unified {
                                UNIFIED_TAB_CLOSE_GLYPH_SIZE
                            } else {
                                TAB_CLOSE_GLYPH_SIZE
                            }))
                            .flex_none()
                            .path("icons/close.svg")
                            .text_color(ui.muted),
                    )
                }));
            strip = strip.child(chip);
        }

        let fade_color = if let Some(background) = unified_background {
            background
        } else {
            pane_card_background(&crate::theme::active_theme(), self.backdrop, true)
        };
        let fades = tab_strip_fades(self.tab_scroll.clone(), fade_color);

        let mut bar = div()
            .id("pane-tab-bar")
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(if unified {
                band_slot.map_or(UNIFIED_TAB_HEIGHT + 2. * UNIFIED_TAB_BAR_INSET, |slot| {
                    slot.height
                })
            } else {
                TAB_BAR_HEIGHT + TAB_BAR_BOTTOM_INSET
            }))
            .pb(px(if unified { 0. } else { TAB_BAR_BOTTOM_INSET }))
            .pl(px(pad_left))
            .pr(px(pad_right))
            .gap(px(TAB_BAR_GAP))
            .relative()
            .overflow_hidden()
            .child(width_probe(self.tab_bar_width.clone()));

        if unified && self.is_detached() {
            bar = bar.child(
                squircle_skin(
                    div()
                        .id(SharedString::from(format!("pane-{pane_id}-tab-reattach")))
                        .flex_none()
                        .size(px(DETACHED_TAB_BUTTON_SIZE))
                        .mr(px(DETACHED_REATTACH_MARGIN))
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
                .child(fades)
                .child(keep_active_tab_in_view(
                    self.tab_scroll.clone(),
                    self.active_surface,
                    self.tab_strip_width.clone(),
                )),
        );

        if self.can_add_surface() && fit.new_tab {
            let menu_open = self.new_tab_menu.is_some();
            let new_tab_button = squircle_skin(
                div()
                    .id(SharedString::from(format!("pane-{pane_id}-tab-new")))
                    .flex_none()
                    .size(px(TAB_BAR_HEIGHT))
                    .when(unified && self.is_detached(), |button| {
                        button
                            .size(px(DETACHED_TAB_BUTTON_SIZE))
                            .mx(px(DETACHED_NEW_TAB_MARGIN))
                    })
                    .when(!self.is_detached(), |button| {
                        button
                            .size(px(ACTION_BUTTON_SIZE))
                            .when(self.dimmed(), |element| {
                                hide_until_hover(element, &content_group)
                            })
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
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                if menu_open {
                    this.new_tab_menu = None;
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
                .children(fit.actions.then(|| {
                    div()
                        .flex_none()
                        .w(px(TAB_BAR_DIVIDER_WIDTH))
                        .h(px(16.))
                        .mx(px(TAB_BAR_DIVIDER_MARGIN))
                        .when(self.dimmed(), |element| {
                            hide_until_hover(element, &content_group)
                        })
                        .bg(ui.border)
                }))
                .child(
                    div()
                        .h_full()
                        .flex_none()
                        .when(self.dimmed(), |element| {
                            hide_until_hover(element, &content_group)
                        })
                        .child(self.render_header(fit.actions, cx)),
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

        let detach = div().id("pane-detach-control").child(self.action_button(
            "pane-btn-detach",
            if self.is_detached() {
                "Return to workspace"
            } else {
                "Detach pane into a window"
            },
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
                    .id("pane-zoom-badge")
                    .accessible_control(gpui::accesskit::Role::Image, PANE_ZOOM_BADGE_LABEL)
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
                "Split right",
                "icons/split_vertical.svg",
                cx.listener(|_this, _, _window, cx| {
                    cx.emit(PaneEvent::Split(crate::layout::SplitDirection::Vertical));
                }),
                cx,
            ))
            .child(self.action_button(
                "pane-btn-split-h",
                "Split down",
                "icons/split_horizontal.svg",
                cx.listener(|_this, _, _window, cx| {
                    cx.emit(PaneEvent::Split(crate::layout::SplitDirection::Horizontal));
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
            PaneSurface::Browser(view) => view.read(cx).focus_handle(cx),
        }
    }
}

fn cached_surface_style() -> StyleRefinement {
    StyleRefinement::default().size_full()
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.backdrop = crate::window_chrome::window_backdrop(&self.cached_config, window);
        let terminal_selected = matches!(self.surface(), PaneSurface::Terminal(_));
        let body = match self.surface() {
            PaneSurface::Terminal(t) => t.clone().cached(cached_surface_style()).into_any_element(),
            PaneSurface::Markdown(m) => m.clone().into_any_element(),
            PaneSurface::Browser(view) => view
                .clone()
                .cached(cached_surface_style())
                .into_any_element(),
        };
        let theme = crate::theme::active_theme();
        let card_background = pane_card_background(&theme, self.backdrop, terminal_selected);

        let dim_target = if self.dimmed() {
            self.cached_config.resolved_unfocused_pane_dim_alpha()
        } else {
            0.0
        };
        let dim_fill = theme.background;
        let card_shape = self.card_shape();
        let dim_from = self.dim_from;
        let dim_live = self.dim_alpha.clone();
        let dim_layer = (dim_target > PANE_DIM_EPSILON || self.dim_alpha.get() > PANE_DIM_EPSILON)
            .then(|| {
                let distance = (dim_target - dim_from).abs();
                if self.dim_seq == 0 || distance <= f32::EPSILON {
                    dim_live.set(dim_target);
                    return card_shape.overlay(dim_fill.opacity(dim_target));
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
                            layer.child(card_shape.overlay(dim_fill.opacity(alpha)))
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
        let session_veil = div()
            .absolute()
            .inset_0()
            .invisible()
            .group_drag_over::<SessionDrag>(group_name.clone(), |s| s.visible())
            .child(card_shape.drop_veil(
                card_background,
                overlay_blue.opacity(DROP_OVERLAY_BACKGROUND_ALPHA),
                overlay_blue,
            ))
            .on_drop(cx.listener(move |this, drag: &SessionDrag, _window, cx| {
                let edge = this.drag_split_direction.take();
                cx.emit(PaneEvent::DropSessionSplit {
                    edge,
                    agent: drag.agent,
                    session_id: drag.session_id.clone(),
                    cwd: drag.cwd.clone(),
                });
                cx.notify();
            }));
        let move_veil = div()
            .absolute()
            .inset_0()
            .invisible()
            .group_drag_over::<PaneDrag>(group_name.clone(), |s| s.visible())
            .group_drag_over::<SurfaceDrag>(group_name.clone(), |s| s.visible())
            .child(card_shape.drop_veil(
                card_background,
                swap_tint.opacity(SWAP_OVERLAY_FILL_ALPHA),
                swap_tint.opacity(SWAP_OVERLAY_BORDER_ALPHA),
            ))
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
            }));
        let overlay = div()
            .absolute()
            .child(session_veil)
            .child(move_veil)
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

        let has_attention = self.attention;
        let attention_color = pane_colors().vc_conflict;
        self.sync_shimmer_clock(cx);
        let border_color = if has_attention {
            attention_color.opacity(0.7)
        } else if self.split_focus == SplitFocus::Focused && !self.zoomed {
            pane_colors().text.opacity(FOCUSED_PANE_BORDER_ALPHA)
        } else {
            pane_colors().border
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .overflow_hidden()
            .child(card_shape.background(card_background, Some(border_color)))
            .child(content)
            .child(card_shape.border(card_background, border_color, has_attention))
    }
}

const PANE_ZOOM_BADGE_LABEL: &str = "Zoomed pane";

fn header_hover_animates(epoch: u64, distance: f32, reduce_motion: bool) -> bool {
    epoch != 0 && distance > f32::EPSILON && !reduce_motion
}

fn pane_status_label(errored: bool, attention: bool) -> Option<&'static str> {
    if errored {
        Some("Agent error")
    } else if attention {
        Some("Agent needs attention")
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChipTone {
    Quiet,
    Working,
    Error,
}

fn header_chip(terminal: &crate::terminal::TerminalState) -> Option<(ChipTone, SharedString)> {
    use crate::app::declared_status::{DeclaredChip, DeclaredState, declared_chip};

    let declared = terminal.declared_status.as_ref();
    match declared_chip(declared) {
        DeclaredChip::Show { label, error } => {
            let tone = if error {
                ChipTone::Error
            } else if declared.and_then(DeclaredState::of) == Some(DeclaredState::Working) {
                ChipTone::Working
            } else {
                ChipTone::Quiet
            };
            Some((tone, label))
        }
        DeclaredChip::Hidden => None,
        DeclaredChip::Fallback => terminal.progress.and_then(|report| {
            progress_chip_label(report).map(|label| (progress_chip_tone(report.state), label))
        }),
    }
}

fn progress_chip_tone(state: paneflow_terminal_ghostty::ProgressState) -> ChipTone {
    use paneflow_terminal_ghostty::ProgressState;

    match state {
        ProgressState::Error => ChipTone::Error,
        ProgressState::Set | ProgressState::Indeterminate => ChipTone::Working,
        ProgressState::Pause | ProgressState::Remove => ChipTone::Quiet,
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

fn search_hits_label(count: usize) -> String {
    crate::app::plural(count, "hit", "hits")
}

fn new_tab_menu_item(
    pane_id: u64,
    index: usize,
    preset: &crate::app::pane_palette::Preset,
) -> MenuItem<crate::app::pane_palette::Preset> {
    let item = MenuItem::new(preset.label.clone(), preset.clone())
        .id(format!("pane-{pane_id}-new-tab-{index}"))
        .icon(preset.icon_path());
    if preset.ensure_launchable().is_ok() {
        item
    } else {
        item.detail("not installed")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TabBarFit {
    pub(crate) actions: bool,
    pub(crate) new_tab: bool,
    pub(crate) label: bool,
}

pub(crate) fn fit_tab_bar(room: f32, new_tab_width: f32, actions_width: f32) -> TabBarFit {
    let labelled = UNIFIED_TAB_MIN_WIDTH;
    TabBarFit {
        actions: room >= labelled + new_tab_width + actions_width,
        new_tab: room >= labelled + new_tab_width,
        label: room >= labelled,
    }
}

pub(crate) fn keep_active_tab_in_view(
    scroll: gpui::ScrollHandle,
    active: usize,
    width: Rc<Cell<f32>>,
) -> AnyElement {
    gpui::canvas(
        move |bounds, window, _| {
            let measured = f32::from(bounds.size.width);
            if width.replace(measured) != measured {
                scroll.scroll_to_item(active);
                window.request_animation_frame();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

fn width_probe(width: Rc<Cell<f32>>) -> impl IntoElement {
    gpui::canvas(
        move |bounds, window, _| {
            let measured = f32::from(bounds.size.width);
            if width.replace(measured) != measured {
                window.request_animation_frame();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

#[cfg(test)]
mod tests {
    use paneflow_terminal_ghostty::{ProgressReport, ProgressState};

    use gpui::{AppContext, Entity, TestAppContext};

    use super::{
        ChipTone, MAX_SURFACE_TITLE_LEN, PANE_ZOOM_BADGE_LABEL, Pane, PaneEvent, PaneSurface,
        badge_geometry, card_paints_quads, header_hover_animates, pane_card_background,
        pane_status_label, progress_chip_label, progress_chip_tone, search_hits_label, tab_badge,
        truncate_surface_title,
    };
    use crate::agent_launcher::TerminalAgent;
    use crate::terminal::TerminalView;
    use crate::ui_primitives::squircle::{squircle_builds, squircle_builds_since};

    struct BadgeStrip {
        tabs: usize,
        inset: gpui::Pixels,
    }

    impl gpui::Render for BadgeStrip {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::{ParentElement, Styled};
            gpui::div()
                .flex()
                .pl(self.inset)
                .children((0..self.tabs).map(|tab| {
                    tab_badge(match tab % 3 {
                        0 => None,
                        1 => Some(TerminalAgent::ClaudeCode),
                        _ => Some(TerminalAgent::Codex),
                    })
                }))
        }
    }

    const BADGE_CONTOURS: usize = 5;

    struct CardFill {
        border: Option<gpui::Hsla>,
    }

    impl gpui::Render for CardFill {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::{ParentElement, Styled};
            gpui::div().size_full().relative().child(
                super::CardShape::Squircle(gpui::px(12.)).background(gpui::white(), self.border),
            )
        }
    }

    #[gpui::test]
    fn a_card_fill_stops_under_an_opaque_border_and_stays_whole_otherwise(cx: &mut TestAppContext) {
        let window = gpui::size(gpui::px(200.), gpui::px(120.));
        let bounds = gpui::Bounds::new(gpui::point(gpui::px(0.), gpui::px(0.)), window);
        for (border, depth) in [
            (Some(gpui::black()), gpui::px(0.5)),
            (Some(gpui::black().opacity(0.3)), gpui::px(0.)),
            (None, gpui::px(0.)),
        ] {
            let (_view, cx) = cx.add_window_view(|_, _| CardFill { border });
            cx.simulate_resize(window);
            assert_eq!(
                draw_badge_frame(cx),
                vec![bounds.inset(depth)],
                "border {border:?}"
            );
        }
    }

    fn draw_badge_frame(cx: &mut gpui::VisualTestContext) -> Vec<gpui::Bounds<gpui::Pixels>> {
        let start = squircle_builds();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        squircle_builds_since(start)
    }

    #[gpui::test]
    fn twelve_tab_badges_build_one_path_per_painted_shape_on_every_frame(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| BadgeStrip {
            tabs: 12,
            inset: gpui::px(0.),
        });
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(80.)));
        let first = draw_badge_frame(cx);
        assert_eq!(
            first.len(),
            12 * BADGE_CONTOURS,
            "the ring, the body, the two contours of the highlight band and the rim, once per badge"
        );
        view.update(cx, |_, cx| cx.notify());
        let second = draw_badge_frame(cx);
        assert_eq!(second, first, "every frame builds the same shapes again");
    }

    #[gpui::test]
    fn a_badge_strip_that_moves_builds_its_shapes_at_the_new_origin(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| BadgeStrip {
            tabs: 12,
            inset: gpui::px(0.),
        });
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(80.)));
        let before = draw_badge_frame(cx);
        let shift = gpui::px(40.);
        view.update(cx, |strip, cx| {
            strip.inset = shift;
            cx.notify();
        });
        let after = draw_badge_frame(cx);
        assert_eq!(after.len(), before.len());
        for (moved, held) in after.iter().zip(&before) {
            assert_eq!(moved.size, held.size);
            assert_eq!(moved.origin, held.origin + gpui::point(shift, gpui::px(0.)));
        }
    }

    fn expected_badge_contours(
        body: gpui::Bounds<gpui::Pixels>,
        scale: f32,
    ) -> Vec<gpui::Bounds<gpui::Pixels>> {
        let layers = badge_geometry(body, scale);
        vec![
            layers.ring.0,
            layers.body.0,
            layers.highlight.0,
            layers.highlight.1,
            layers.rim.0,
        ]
    }

    #[gpui::test]
    fn a_scale_change_rebuilds_every_badge_shape_for_the_new_scale(cx: &mut TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, _| BadgeStrip {
            tabs: 1,
            inset: gpui::px(0.),
        });
        cx.simulate_resize(gpui::size(gpui::px(200.), gpui::px(80.)));
        for scale in [2., 1., 1.5] {
            cx.simulate_scale_factor_change(scale);
            let built = draw_badge_frame(cx);
            assert_eq!(built.len(), BADGE_CONTOURS);
            assert_eq!(
                built,
                expected_badge_contours(built[1], scale),
                "every contour follows the scale {scale}"
            );
        }
    }

    #[gpui::test]
    fn a_thousand_origins_build_each_badge_where_it_paints_and_retain_nothing(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, _| BadgeStrip {
            tabs: 1,
            inset: gpui::px(0.),
        });
        cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(80.)));
        let origin = draw_badge_frame(cx)[1].origin;
        for step in 1..=1000 {
            let inset = gpui::px(step as f32);
            view.update(cx, |strip, cx| {
                strip.inset = inset;
                cx.notify();
            });
            let built = draw_badge_frame(cx);
            assert_eq!(built.len(), BADGE_CONTOURS, "frame {step}");
            assert_eq!(built[1].origin, origin + gpui::point(inset, gpui::px(0.)));
        }
    }

    fn device_gap(
        outer: gpui::Bounds<gpui::Pixels>,
        inner: gpui::Bounds<gpui::Pixels>,
        scale: f32,
    ) -> f32 {
        [
            inner.left() - outer.left(),
            outer.right() - inner.right(),
            inner.top() - outer.top(),
            outer.bottom() - inner.bottom(),
        ]
        .into_iter()
        .map(|gap| f32::from(gap) * scale)
        .fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn every_badge_layer_clears_the_contours_beneath_it_by_half_a_device_pixel() {
        let bounds = gpui::Bounds::new(
            gpui::point(gpui::px(40.), gpui::px(7.)),
            gpui::size(gpui::px(22.), gpui::px(18.)),
        );
        for scale in [1., 1.25, 1.5, 2., 3.] {
            let layers = badge_geometry(bounds, scale);
            let (ring, body, rim) = (layers.ring.0, layers.body.0, layers.rim.0);
            let (outer, inner, _) = layers.highlight;
            let clear = |gap: f32| gap >= 0.5 - 1e-4;
            assert!(
                clear(device_gap(ring, body, scale)),
                "body inside the ring at {scale}"
            );
            assert!(
                clear(device_gap(body, rim, scale)),
                "rim inside the body at {scale}"
            );
            assert!(
                clear(device_gap(rim, outer, scale)),
                "highlight inside the rim edge at {scale}"
            );
            let rim_inner_top = f32::from(rim.top() + gpui::px(1.));
            let highlight_inner_top = f32::from(inner.top());
            assert!(
                clear((highlight_inner_top - rim_inner_top) * scale),
                "the highlight band ends below the rim's inner edge at {scale}"
            );
            let near =
                |a: gpui::Pixels, b: gpui::Pixels| (f32::from(a) - f32::from(b)).abs() < 1e-4;
            assert!(
                near(inner.top() - outer.top(), gpui::px(1.)),
                "the highlight band stays 1 px thick"
            );
            assert!(near(inner.bottom(), outer.bottom()));
            assert!(near(inner.left(), outer.left()) && near(inner.right(), outer.right()));
        }
    }

    #[test]
    fn pane_states_carry_a_text_label_and_not_only_a_color() {
        assert_eq!(pane_status_label(true, false), Some("Agent error"));
        assert_eq!(pane_status_label(true, true), Some("Agent error"));
        assert_eq!(
            pane_status_label(false, true),
            Some("Agent needs attention")
        );
        assert_eq!(pane_status_label(false, false), None);
        assert_eq!(PANE_ZOOM_BADGE_LABEL, "Zoomed pane");
    }

    #[test]
    fn the_header_hover_does_not_animate_under_reduce_motion() {
        assert!(header_hover_animates(1, 0.5, false));
        assert!(!header_hover_animates(1, 0.5, true));
        assert!(!header_hover_animates(0, 0.5, false));
        assert!(!header_hover_animates(1, 0.0, false));
    }

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
        cx.update(|_, cx| {
            cx.subscribe(&pane, move |_, event: &PaneEvent, _| {
                if matches!(event, PaneEvent::CloseRequested) {
                    requested_for_sub.set(true);
                }
            })
            .detach();
        });

        pane.update(cx, |pane, cx| pane.close_surface(0, cx));
        assert!(
            requested.get(),
            "closing the last tab asks the owner to close the pane"
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
    fn search_hits_label_names_a_single_hit_in_the_singular() {
        assert_eq!(search_hits_label(1), "1 hit");
        assert_eq!(search_hits_label(2), "2 hits");
        assert_eq!(search_hits_label(12), "12 hits");
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

    fn blended(
        blur: crate::window_chrome::veil::BlurSupport,
    ) -> crate::window_chrome::veil::Backdrop {
        crate::window_chrome::veil::Backdrop::new(
            paneflow_config::schema::BackgroundStyle::Blended,
            true,
            blur,
        )
    }

    #[test]
    fn the_backdrop_reaches_only_terminal_cards() {
        use crate::window_chrome::veil::{Backdrop, BlurSupport};
        let theme = crate::theme::paneflow_dark();

        for backdrop in [
            Backdrop::THEMED,
            blended(BlurSupport::Full),
            blended(BlurSupport::None),
        ] {
            assert_eq!(
                pane_card_background(&theme, backdrop, false),
                theme.background
            );
        }
        assert_eq!(
            pane_card_background(&theme, Backdrop::THEMED, true),
            theme.background
        );
    }

    #[test]
    fn a_blurred_terminal_card_shades_toward_the_theme() {
        use crate::window_chrome::veil::BlurSupport;
        let dark = crate::theme::paneflow_dark();
        let light = crate::theme::paneflow_light();

        assert_eq!(
            pane_card_background(&dark, blended(BlurSupport::Full), true),
            gpui::black().opacity(0.35)
        );
        assert_eq!(
            pane_card_background(&light, blended(BlurSupport::Full), true),
            gpui::white().opacity(0.35)
        );
    }

    #[test]
    fn an_unblurred_terminal_card_is_opaque() {
        use crate::window_chrome::veil::BlurSupport;
        for theme in [
            crate::theme::paneflow_dark(),
            crate::theme::paneflow_light(),
        ] {
            assert_eq!(
                pane_card_background(&theme, blended(BlurSupport::None), true),
                theme.background
            );
        }
    }

    #[test]
    fn the_drop_veil_nests_concentrically_inside_a_comfortable_card() {
        let card = super::CardShape::Squircle(crate::app::constants::PANE_CARD_RADIUS);
        assert_eq!(
            card.drop_veil_radius() + gpui::px(super::OVERLAY_MARGIN),
            crate::app::constants::PANE_CARD_RADIUS
        );
        assert_eq!(
            super::CardShape::Squircle(gpui::px(10.)).drop_veil_radius(),
            gpui::px(super::OVERLAY_RADIUS)
        );
        assert_eq!(
            super::CardShape::Flush(gpui::Edges::default()).drop_veil_radius(),
            gpui::px(super::OVERLAY_RADIUS)
        );
    }

    #[test]
    fn a_compact_tab_nests_concentrically_in_the_pane_corner() {
        let radius = super::CardShape::Flush(gpui::Edges::default())
            .tab_chip_radius()
            .unwrap();
        assert_eq!(
            radius + gpui::px(super::UNIFIED_TAB_BAR_INSET),
            crate::app::constants::PANEL_CORNER_RADIUS
        );
        assert_eq!(
            super::CardShape::Squircle(crate::app::constants::PANE_CARD_RADIUS).tab_chip_radius(),
            None
        );
    }

    #[test]
    fn a_classic_tab_takes_the_classic_control_radius_whatever_the_frame() {
        use crate::ui_primitives::interface_style::{CLASSIC_CONTROL, Shape, scoped};
        scoped(Shape::Classic, || {
            for shape in [
                super::CardShape::Flush(gpui::Edges::default()),
                super::CardShape::Squircle(crate::app::constants::PANE_CARD_RADIUS),
                super::CardShape::Morph {
                    radii: gpui::Corners::default(),
                    comfort: 0.5,
                },
            ] {
                assert_eq!(shape.tab_chip_radius(), Some(CLASSIC_CONTROL));
            }
        });
    }

    #[test]
    fn only_active_progress_shimmers() {
        assert_eq!(progress_chip_tone(ProgressState::Set), ChipTone::Working);
        assert_eq!(
            progress_chip_tone(ProgressState::Indeterminate),
            ChipTone::Working
        );
        assert_eq!(progress_chip_tone(ProgressState::Pause), ChipTone::Quiet);
        assert_eq!(progress_chip_tone(ProgressState::Error), ChipTone::Error);
    }

    #[test]
    fn a_translucent_linux_card_strokes_its_border_on_the_fill_geometry() {
        let theme = crate::theme::paneflow_dark();

        assert!(!card_paints_quads(theme.background));
        #[cfg(target_os = "linux")]
        assert!(card_paints_quads(pane_card_background(
            &theme,
            blended(crate::window_chrome::veil::BlurSupport::Full),
            true
        )));
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

    #[test]
    fn a_tab_bar_with_room_shows_every_control() {
        assert_eq!(
            super::fit_tab_bar(400., 25., 120.),
            super::TabBarFit {
                actions: true,
                new_tab: true,
                label: true,
            }
        );
    }

    #[test]
    fn a_narrow_tab_bar_drops_the_actions_then_the_new_tab_button_then_the_label() {
        let labelled = super::UNIFIED_TAB_MIN_WIDTH;
        assert_eq!(
            super::fit_tab_bar(labelled + 25. + 119., 25., 120.),
            super::TabBarFit {
                actions: false,
                new_tab: true,
                label: true,
            }
        );
        assert_eq!(
            super::fit_tab_bar(labelled + 24., 25., 120.),
            super::TabBarFit {
                actions: false,
                new_tab: false,
                label: true,
            }
        );
        assert_eq!(
            super::fit_tab_bar(labelled - 1., 25., 120.),
            super::TabBarFit {
                actions: false,
                new_tab: false,
                label: false,
            }
        );
    }
}
