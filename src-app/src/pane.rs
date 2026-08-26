//! Pane - a mono-surface leaf holding exactly one view (a terminal, a markdown
//! viewer, or a diff view).
//!
//! Each leaf in the layout tree holds an `Entity<Pane>`, and each pane holds a
//! single [`PaneSurface`]. Multiplicity lives one level up: a workspace owns a
//! list of tabs and each tab owns a layout of panes (PRD
//! `prd-cli-tab-hierarchy-2026-Q3`, EP-001/EP-002). A pane therefore has no tab
//! strip, no selection cursor and no cross-pane tab gesture - a second surface
//! is reached by splitting the pane or by opening another workspace tab.
//!
//! Communication with the parent (layout tree owner) uses the Zed pattern:
//! Pane emits `PaneEvent` via `cx.emit()`, parent subscribes via `cx.subscribe()`.
//!
//! The pane header (surface name, agent pill, action cluster) replaces the
//! former tab strip and paints nothing of its own: the floating card behind it
//! owns the fill (commit `30e26c5`).

use crate::ui_primitives::TooltipDelayExt;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, ClickEvent, Context, DragMoveEvent, Entity,
    EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, Pixels, Point, Render, SharedString, Size, Styled, Window, deferred, div,
    ease_out_quint, img, prelude::*, px, rgb, svg,
};

use crate::ui_primitives::squircle::{squircle_border, squircle_fill};
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

use crate::diff::DiffView;
use crate::markdown::MarkdownView;
use crate::pane_drag::{
    DragPreview, DropEdge, MarkdownFileDrag, PaneDrag, SPLIT_EDGE_BAND, SessionDrag,
    compute_drop_edge, split_rect,
};
use crate::terminal::{TerminalEvent, TerminalView};

// ---------------------------------------------------------------------------
// PaneSurface - the single view a pane holds
// ---------------------------------------------------------------------------

/// The one surface a pane holds. Terminals, markdown viewers and diff views are
/// interchangeable here: the pane renders whichever it owns. A second surface
/// means a second pane (split) or a second workspace tab, never a second entry
/// in a strip.
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

    /// Icon of the surface *kind*, independent of what runs inside it. The
    /// pane header leads with it, and the sidebar's per-pane icon cluster
    /// (US-013) falls back to it for a pane with no detected agent.
    pub(crate) fn kind_icon(&self) -> &'static str {
        match self {
            PaneSurface::Terminal(_) => "icons/terminal.svg",
            PaneSurface::Markdown(_) => "icons/file-text.svg",
            PaneSurface::Diff(_) => "icons/git-branch.svg",
        }
    }

    /// Human label of the surface kind, for the tooltips that enumerate the
    /// panes of a tab (US-013). Deliberately not the surface *title*: the
    /// cluster names what a pane is, not what it currently shows.
    pub(crate) fn kind_label(&self) -> &'static str {
        match self {
            PaneSurface::Terminal(_) => "Terminal",
            PaneSurface::Markdown(_) => "Markdown",
            PaneSurface::Diff(_) => "Diff",
        }
    }
}

// ---------------------------------------------------------------------------
// Pane header color helpers - derived from active theme
// ---------------------------------------------------------------------------

fn pane_colors() -> crate::theme::UiColors {
    crate::theme::ui_colors()
}

/// Fill of the pane card.
///
/// The card is the only surface in the pane that paints a background: the
/// header, the content region and the terminal element are all transparent, so
/// this single rounded quad is what the corners clip cleanly (GPUI does not
/// clip a child's own background to its parent's radius).
///
/// Windows terminal material stays scoped to terminal surfaces: there the card
/// itself goes transparent so the Mica backdrop reads through the pane.
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

/// First line of an agent question, bounded for the collapsed peek badge
/// (US-020, orchestration-v2). Pure - unit-tested below.
fn peek_badge_line(message: &str) -> String {
    const BADGE_MAX_CHARS: usize = 80;
    let first = message.lines().next().unwrap_or("").trim();
    let mut line: String = first.chars().take(BADGE_MAX_CHARS).collect();
    if first.chars().count() > BADGE_MAX_CHARS {
        line.push('…');
    }
    line
}

/// Height of the header's content row.
const HEADER_CONTENT_HEIGHT: f32 = 28.0;
/// Header height derived from the shared content inset. Centering the content
/// row leaves 3px above and below it; the pane's reserved 1px border completes
/// the same 4px visual inset used on the left edge.
const PANE_HEADER_HEIGHT: f32 =
    HEADER_CONTENT_HEIGHT + crate::app::constants::PANE_CONTENT_INSET_Y * 2.0;
/// Gap between the header's children (icon, label, adornments, actions).
const HEADER_GAP: f32 = 7.0;
/// Approximate title capacity in the header before the CSS ellipsis is expected
/// to engage; above it the full name is offered as a tooltip.
const SURFACE_TITLE_TOOLTIP_THRESHOLD: usize = 13;
/// Header title size. Same value as the sidebar's row label (`text_sm()` =
/// 0.875rem over GPUI's 16px rem).
const HEADER_TEXT_SIZE: f32 = 14.0;
/// Header title line box, mirroring `SIDEBAR_ROW_LINE_HEIGHT`. Without it the
/// title falls back to GPUI's relative default and reads visually lighter than
/// the sidebar row at the same pixel size.
const HEADER_TEXT_LINE_HEIGHT: f32 = 18.0;
/// Section padding (start/end areas of the header). Same value as the grid's
/// horizontal inset, so the surface name and terminal column 0 share one rule.
const SECTION_PX: f32 = crate::app::constants::PANE_CONTENT_INSET_X;
/// Square size shared by header icon buttons.
const ACTION_BUTTON_SIZE: f32 = 22.0;
/// Diameter of the close affordance, and size of its glyph. Both carried over
/// verbatim from the tab-strip close button EP-002 US-007 removed: a round
/// chip washed with the theme text color, not a square icon button like its
/// neighbors in the trailing cluster.
const CLOSE_BUTTON_SIZE: f32 = 15.0;
const CLOSE_GLYPH_SIZE: f32 = 9.0;
/// Resting / hover fill alpha of the close chip, over `ui.text`.
const CLOSE_BASE_ALPHA: f32 = 0.16;
const CLOSE_HOVER_ALPHA: f32 = 0.92;

/// Hover group of the pane header. Scopes the close button's reveal; GPUI
/// resolves a group against the nearest ancestor that declared it, so every
/// pane can share the one name without leaking into its neighbors.
const HEADER_GROUP: &str = "pane-header-group";
/// Full-distance duration for every header hover transition.
const HEADER_HOVER_MS: u64 = 120;
/// Cross-fade duration for the unfocused-pane dim, at full travel. Short
/// enough that focus feels instantaneous, long enough not to strobe when
/// `Alt+Arrow` walks the tree.
const PANE_DIM_FADE_MS: u64 = 130;
/// Below this alpha the dim layer is indistinguishable from absent, so the
/// element is dropped entirely rather than painted at ~0.
const PANE_DIM_EPSILON: f32 = 0.002;
/// Uniform gap (px) between the drop-to-split preview overlay and its region's
/// edges, so the blue box floats inside the target half/pane (EP-003 US-008).
const OVERLAY_MARGIN: f32 = 8.0;
/// Corner radius (px) of the drop-to-split preview overlay.
const OVERLAY_RADIUS: f32 = 8.0;
/// Apple system blue (#007AFF), used for the CLI drop placement preview.
const DROP_OVERLAY_BLUE: u32 = 0x007aff;
/// Low-alpha fill so the placement card stays visible without washing the pane.
const DROP_OVERLAY_BACKGROUND_ALPHA: f32 = 0.10;

/// Fill / hairline alpha of the *pane-swap* placeholder. A pane dragged onto
/// another pane reorders the layout, it does not split it, so its placeholder
/// is neutral (a translucent wash of the theme's text color) instead of the
/// blue split preview: blue means "a new pane lands here", neutral means "these
/// two trade places".
const SWAP_OVERLAY_FILL_ALPHA: f32 = 0.10;
const SWAP_OVERLAY_BORDER_ALPHA: f32 = 0.22;
/// Hard upper bound on surface title length in characters. Mirrors Zed's
/// `MAX_SURFACE_TITLE_LEN` (`zed/crates/editor/src/items.rs:64`). Anything past
/// this is replaced with a trailing ellipsis before the CSS ellipsis layer.
const MAX_SURFACE_TITLE_LEN: usize = 24;

/// Char-boundary-safe `truncate_and_trailoff`. Counts chars (not bytes) so
/// filenames with multibyte UTF-8 (accents, CJK, emoji) don't trigger a
/// byte-index panic, and reserves one char for the trailing `…`.
fn truncate_surface_title(raw: &str) -> String {
    if raw.chars().count() <= MAX_SURFACE_TITLE_LEN {
        return raw.to_string();
    }
    let head: String = raw.chars().take(MAX_SURFACE_TITLE_LEN - 1).collect();
    format!("{head}…")
}

// ---------------------------------------------------------------------------
// Pane events - emitted to parent via cx.emit()
// ---------------------------------------------------------------------------

pub enum PaneEvent {
    /// This pane's surface ended (its process exited, or the user closed it) -
    /// the parent removes the pane from the layout tree. EP-002 US-004: a pane
    /// has exactly one surface, so this is what closing the last tab of a pane
    /// used to do, and no empty pane is ever left behind.
    Remove,
    /// Request a split in the given direction from this pane.
    Split(crate::layout::SplitDirection),
    /// Toggle the docked agent-sessions sidebar for the active terminal's cwd
    /// (PRD `prd-agent-sessions-sidebar-2026-Q3`). The parent resolves the cwd,
    /// binds this pane, and spawns the per-agent scans; no anchor is needed
    /// since the sidebar docks in the root layout rather than floating.
    ToggleAgentSessions,
    /// Toggle the right-docked git diff on this pane's workspace folder. The
    /// parent resolves the folder from the pane's workspace id.
    ToggleDiffDock,
    /// Right-click on the pane header: open the pane context menu at
    /// `position` (EP-002 US-007). This replaces the former tab context menu,
    /// which the removed tab strip used to anchor; it carries no index, and it
    /// no longer offers a cross-pane move.
    OpenPaneMenu { position: Point<Pixels> },
    /// An agent-session row was dropped out of the sessions sidebar onto this
    /// pane (bridges `prd-agent-sessions-sidebar` × `prd-pane-drag-drop`). The
    /// parent spawns a *fresh* terminal at `cwd` running the agent's resume
    /// command, then - for `edge = Some` - splits this (the emitting target)
    /// pane toward that edge. For `edge = None` (center band) it opens a NEW
    /// WORKSPACE TAB instead: EP-002 US-007 removed the pane-level tab strip,
    /// so a centered drop can no longer append next to the existing surface,
    /// and replacing a live terminal would destroy running work. Routed to
    /// `PaneFlowApp` because spawning a terminal needs the app-level CWD/port
    /// subscription wiring.
    DropSessionSplit {
        edge: Option<DropEdge>,
        agent: crate::agent_sessions::SessionAgent,
        session_id: String,
        cwd: String,
    },
    /// A markdown file was dropped out of the Files sidebar onto this (the
    /// emitting target) pane (PRD `prd-files-tree-sidebar-2026-Q3`, EP-003).
    /// For `edge = Some` the parent opens the file in a new pane split toward
    /// that edge; for `edge = None` (center band) it opens a new workspace tab
    /// holding the file, for the same reason as `DropSessionSplit` (EP-002
    /// US-007). Routed to `PaneFlowApp` (LayoutTree owner) to keep the tree
    /// mutation out of the drop callback (entity re-entrancy).
    DropMarkdownSplit {
        edge: Option<DropEdge>,
        path: std::path::PathBuf,
    },
    /// A pane was dragged by its header and dropped onto this (the emitting
    /// target) pane (PRD `prd-pane-drag-drop-2026-Q3.md`). Same gesture as the
    /// pre-EP-002 `DropSplit`, minus the tab strip: `edge = Some` detaches the
    /// source pane from wherever it sits and re-inserts it as a split of the
    /// target toward that edge (drop under a pane, to its right, ...);
    /// `edge = None` (center band) makes the two panes trade places, which is
    /// the mono-surface analog of the old "append as a tab" center drop.
    ///
    /// Carries the *source* pane's entity id; the parent re-resolves it in the
    /// tab that owns the target, so a drag whose source vanished mid-gesture is
    /// a no-op rather than a wrong move. Routed to `PaneFlowApp` because the
    /// layout tree lives there (and because mutating it from inside a drop
    /// callback would re-enter this entity).
    DropPaneMove {
        source_pane_id: u64,
        edge: Option<DropEdge>,
    },
}

/// Reversible hover transition state for one header interactive surface.
struct HeaderHoverMotion {
    /// Last progress painted by the animator, used to seed mid-flight reversals.
    live_progress: Rc<Cell<f32>>,
    from: f32,
    target: f32,
    /// Restarts GPUI's one-shot animation whenever the hover target changes.
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

// ---------------------------------------------------------------------------
// Pane - mono-surface container
// ---------------------------------------------------------------------------

pub struct Pane {
    /// The single view this pane owns (EP-002 US-004).
    pub surface: PaneSurface,
    /// US-018/US-020 (orchestration-v2): set when this pane's terminal has an
    /// agent session in `WaitingForInput`, holding the agent's question (≤512
    /// chars, UNTRUSTED display-only text; empty when none was captured).
    /// Pushed by `PaneFlowApp::sync_attention` recomputed from the session
    /// truth on every transition, never mutated locally. Drives the attention
    /// ring, the header dot and the peek overlay.
    attention: Option<String>,
    /// EP-004 US-010 (cli-cockpit): this pane's terminal has an agent session
    /// in `Errored` (the agent binary exited non-zero). Pushed by
    /// `PaneFlowApp::sync_attention` alongside `attention` - same idempotent
    /// recompute-from-session-truth contract. Drives the dedicated
    /// `agent_error` header dot, ranked above the waiting dot.
    errored: bool,
    /// EP-006 US-018 (cli-cockpit): transient fleet-grep match count for this
    /// pane's terminal. Pushed by `PaneFlowApp::push_fleet_badges` after a
    /// fan-out, cleared 4 s later or when the fleet overlay closes. FR-11: the
    /// LOWEST-priority header adornment - first to yield its slot.
    search_hits: Option<usize>,
    /// US-020: the peek badge is hovered - render the full question panel.
    peek_expanded: bool,
    /// Set to true when the workspace is zoomed on this pane.
    pub zoomed: bool,
    /// Workspace ID for spawning new terminals with correct env vars.
    pub workspace_id: u64,
    /// Per-surface hover progress for reversible header fades and tint shifts.
    header_hover_motion: std::collections::HashMap<SharedString, HeaderHoverMotion>,
    /// US-015: cached `paneflow.json` so `render_header` never calls the
    /// blocking `load_config()` per frame (the agent-button visibility gate and
    /// the launch command read it). Hydrated at creation, refreshed by
    /// `PaneFlowApp::process_config_changes` → `Workspace::propagate_config` on
    /// every `ConfigWatcher` reload, so a Settings flip (e.g. the Claude bypass
    /// toggle) takes effect on the next click without a per-frame disk read.
    pub cached_config: paneflow_config::schema::PaneFlowConfig,
    /// Live drop-to-split target (EP-003 US-007): the edge the blue overlay
    /// previews while a session or markdown file is dragged over this pane's
    /// content. `None` = center band (new workspace tab) or no drag. Updated by
    /// the content `on_drag_move` handler; reset on drop. While no drag is
    /// active the overlay is `invisible()` regardless of this value, so a stale
    /// value after a cancel is harmless (the next drag-move recomputes it).
    drag_split_direction: Option<DropEdge>,
    /// Previous drop region, kept only as a *fallback* start rect for the glide
    /// on the first crossing of a drag, before the live position cell
    /// ([`Self::overlay_current`]) holds anything meaningful. Set to the old
    /// value of `drag_split_direction` each time it changes.
    overlay_prev_dir: Option<DropEdge>,
    /// Start rect `(x, y, w, h)` of the current glide, captured at the instant
    /// the region changes. Captured from the overlay's *live* on-screen
    /// position ([`Self::overlay_current`]) rather than the previous region's
    /// resting rect, so a fast multi-band crossing redirects from wherever the
    /// box actually is mid-flight instead of jumping back to the prior target.
    overlay_from: (f32, f32, f32, f32),
    /// The overlay's live interpolated rect, written by the glide animator every
    /// frame and read back by `on_drag_move` to seed the next glide's start
    /// (see [`Self::overlay_from`]). `Rc<Cell>` because it is shared between the
    /// render-time animator closure and the event handler.
    overlay_current: Rc<Cell<(f32, f32, f32, f32)>>,
    /// Bumped every time `drag_split_direction` changes. Feeds the overlay's
    /// animation `ElementId`, so a new region restarts the glide from delta 0.
    overlay_seq: usize,
    /// Last observed content size (captured in the `on_drag_move` handler), used
    /// to convert a [`DropEdge`] into an absolute-pixel rectangle for the glide.
    overlay_pane_size: Size<Pixels>,
    /// EP-001 US-001/US-003 (cli-cockpit): the Composer overlay pushed by
    /// `PaneFlowApp::refresh_composer_slot` when this pane is the Composer
    /// target. `None` on every other pane. The pane renders it bottom-anchored
    /// and routes gestures back through the slot's closures - it never reads
    /// app state.
    composer_slot: Option<crate::app::composer::ComposerSlot>,
    /// EP-001 US-003: this pane's terminal holds a queued prompt
    /// (broadcast/Composer buffer awaiting the agent's next idle transition).
    /// Pushed by `PaneFlowApp::sync_pending_chips`; drives the "1 queued" chip.
    pending_prefill: bool,
    /// EP-001 US-002: broadcast-group stripe color index (`UiColors::group_*`)
    /// when this pane is a group member. Pushed by
    /// `PaneFlowApp::sync_broadcast_stripes`. The stripe is a DISTINCT element
    /// from the attention border below - the pane border slot stays the glow's.
    broadcast_stripe: Option<usize>,
    /// Ghostty-style unfocused dim: `true` when this pane is NOT the focused
    /// one in a multi-pane workspace. Pushed idempotently by
    /// [`crate::layout::LayoutTree::sync_unfocused_dim`] - never mutated
    /// locally, so focus stays the single source of truth.
    dimmed: bool,
    /// Dim alpha the cross-fade starts from, snapshotted from
    /// [`Self::dim_alpha`] at the instant [`Self::set_dimmed`] flips, so an
    /// interrupted fade resumes from the pixel that is actually on screen
    /// instead of snapping back to the previous endpoint.
    dim_from: f32,
    /// Live dim alpha, shared with the animation closure (which runs outside
    /// `&mut self`) so the next flip can read where the fade got to.
    dim_alpha: Rc<Cell<f32>>,
    /// Bumped on every [`Self::set_dimmed`] flip; keys the animation
    /// `ElementId` so GPUI restarts the ease instead of resuming the old one.
    dim_seq: usize,
}

impl EventEmitter<PaneEvent> for Pane {}

impl Pane {
    /// Create a new pane holding a single terminal.
    pub fn new(terminal: Entity<TerminalView>, workspace_id: u64, cx: &mut Context<Self>) -> Self {
        Self::new_with_surface(PaneSurface::Terminal(terminal), workspace_id, cx)
    }

    /// Create a new pane wrapping an existing surface moved in from elsewhere
    /// (drop-to-split, session restore). The pane-level subscription is wired
    /// for a terminal surface so `ChildExited`/`TitleChanged` route here, but -
    /// unlike [`crate::PaneFlowApp::create_pane`] - the app-level terminal
    /// subscription is NOT re-added, because a moved terminal already has one
    /// from its original creation (re-adding would double CWD/port events).
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
            peek_expanded: false,
            zoomed: false,
            workspace_id,
            header_hover_motion: std::collections::HashMap::new(),
            // US-015: hydrate the header config cache once at creation (not
            // per frame); refreshed on ConfigWatcher reload via propagation.
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

    /// US-018/US-020 (orchestration-v2): set/clear this pane's attention state.
    /// `Some(question)` means the agent waits for input (an empty string = no
    /// question captured); `None` means it does not. Idempotent push from
    /// `PaneFlowApp::sync_attention` - repaints only on change.
    pub fn set_attention(&mut self, attention: Option<String>, cx: &mut Context<Self>) {
        if self.attention != attention {
            if attention.is_none() {
                self.peek_expanded = false;
            }
            self.attention = attention;
            cx.notify();
        }
    }

    /// EP-004 US-010 (cli-cockpit): set/clear the Errored flag (this pane's
    /// agent binary exited non-zero). Same idempotent push contract as
    /// [`Pane::set_attention`] - repaints only on change.
    pub fn set_errored(&mut self, errored: bool, cx: &mut Context<Self>) {
        if self.errored != errored {
            self.errored = errored;
            cx.notify();
        }
    }

    /// EP-006 US-018 (cli-cockpit): set/clear this pane's transient fleet-grep
    /// badge count. Same idempotent push contract as [`Pane::set_attention`].
    pub fn set_search_hits(&mut self, hits: Option<usize>, cx: &mut Context<Self>) {
        if self.search_hits != hits {
            self.search_hits = hits;
            cx.notify();
        }
    }

    /// EP-001 US-001 (cli-cockpit): install/clear the Composer overlay on
    /// this pane. Always notifies - the slot carries live `busy`/group data
    /// recomputed by the pusher, and the closure fields defeat `PartialEq`.
    pub fn set_composer_slot(
        &mut self,
        slot: Option<crate::app::composer::ComposerSlot>,
        cx: &mut Context<Self>,
    ) {
        self.composer_slot = slot;
        cx.notify();
    }

    /// EP-001 US-003: set/clear the queued-prompt indicator. Idempotent push
    /// from `PaneFlowApp::sync_pending_chips` - repaints only on change.
    pub fn set_pending_prefill(&mut self, pending: bool, cx: &mut Context<Self>) {
        if self.pending_prefill != pending {
            self.pending_prefill = pending;
            cx.notify();
        }
    }

    /// Set/clear this pane's unfocused dim. Idempotent push from
    /// [`crate::layout::LayoutTree::sync_unfocused_dim`]: it repaints only on
    /// a real flip, and snapshots the live alpha first so an interrupted
    /// cross-fade resumes from what is on screen.
    pub fn set_dimmed(&mut self, dimmed: bool, cx: &mut Context<Self>) {
        if self.dimmed == dimmed {
            return;
        }
        self.dim_from = self.dim_alpha.get();
        self.dim_seq = self.dim_seq.wrapping_add(1);
        self.dimmed = dimmed;
        cx.notify();
    }

    /// EP-001 US-002: set/clear the broadcast-group stripe color slot.
    /// Idempotent push from `PaneFlowApp::sync_broadcast_stripes`.
    pub fn set_broadcast_stripe(&mut self, color_idx: Option<usize>, cx: &mut Context<Self>) {
        if self.broadcast_stripe != color_idx {
            self.broadcast_stripe = color_idx;
            cx.notify();
        }
    }

    /// EP-001 US-001/US-003: the Composer overlay - a bottom-anchored prompt
    /// panel over a click-swallowing backdrop, so the terminal underneath
    /// receives neither keystrokes (the TextArea holds focus) nor clicks
    /// while the user is composing (theme-picker overlay model).
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

        // Broadcast toggle: shows the active group when armed, plain label
        // otherwise. With no group defined the click routes to a toast that
        // points at the picker (handled app-side).
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
            // US-001 AC5 chip (US-003 unified semantics): the target's agent
            // is generating - validation queues instead of delivering.
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

        // US-001 AC4: the explicit deliver-then-submit gesture is documented
        // right on the surface; US-003 AC7: it is unavailable in broadcast.
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
                    // The scrim takes the card's silhouette so it does not
                    // repaint the corners square over the pane behind it.
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

    /// US-020 (orchestration-v2): a compact badge on the pane showing the
    /// waiting agent's question without stealing focus; hover expands to the
    /// full message (≤512 chars, plain inert text - no links, no ANSI).
    /// Top-right under the pane header so the agent's prompt line stays visible.
    fn render_peek_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let question = self.attention.clone()?;
        let ui = pane_colors();
        let full = if question.is_empty() {
            "waiting for input".to_string()
        } else {
            question
        };
        let shown = if self.peek_expanded {
            full
        } else {
            peek_badge_line(&full)
        };
        Some(
            div()
                .id(SharedString::from(format!(
                    "peek-{}",
                    cx.entity().entity_id().as_u64()
                )))
                .absolute()
                .top(px(PANE_HEADER_HEIGHT + 6.0))
                .right_2()
                .max_w(px(420.0))
                .overflow_hidden()
                .bg(ui.overlay)
                .border_1()
                .border_color(ui.vc_conflict.opacity(0.6))
                .rounded_md()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(ui.text)
                .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                    if this.peek_expanded != *hovered {
                        this.peek_expanded = *hovered;
                        cx.notify();
                    }
                }))
                .child(shown)
                .into_any_element(),
        )
    }

    /// Iterate over this pane's terminal - zero items when its surface is a
    /// markdown or diff view. Kept as an iterator so the many callers that scan
    /// panes uniformly (sidebar counters, AI-tool PID owner lookups, layout
    /// serialization) keep working unchanged after EP-002 US-004.
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
        let cursor_color_override = config
            .terminal
            .as_ref()
            .and_then(|terminal| terminal.cursor_color.as_deref())
            .and_then(crate::terminal::view::hsla_from_hex_color);
        terminal.update(cx, |terminal, cx| {
            terminal.set_integrated_glyphs_enabled(integrated_glyphs_enabled, cx);
            terminal.set_color_emoji_enabled(color_emoji_enabled, cx);
            terminal.set_cursor_color_override(cursor_color_override, cx);
        });
    }

    /// True when `terminal` is this pane's surface.
    pub fn contains_terminal(&self, terminal: &Entity<TerminalView>) -> bool {
        self.terminals().any(|t| t == terminal)
    }

    /// Subscribe to the terminal's events - remove the pane on exit, repaint on
    /// title change. EP-002 US-004: a pane holds exactly one surface, so a
    /// terminal whose process exits takes the pane down with it (what closing
    /// the last tab of a pane used to do) instead of leaving an empty pane in
    /// the tree.
    fn subscribe_terminal(terminal: &Entity<TerminalView>, cx: &mut Context<Self>) {
        cx.subscribe(terminal, |this, terminal, event: &TerminalEvent, cx| {
            match event {
                TerminalEvent::ChildExited => {
                    if this.surface.as_terminal() == Some(&terminal) {
                        cx.emit(PaneEvent::Remove);
                    }
                }
                TerminalEvent::TitleChanged => {
                    cx.notify();
                }
                // CwdChanged, ActivityBurst, ServiceDetected, SelectionCopied are
                // handled by PaneFlowApp's direct subscription to each TerminalView.
                TerminalEvent::CwdChanged(_)
                | TerminalEvent::ActivityBurst
                | TerminalEvent::ServiceDetected(_)
                | TerminalEvent::CancelSwapMode
                | TerminalEvent::SelectionCopied
                | TerminalEvent::OpenMarkdownPath(_)
                | TerminalEvent::OpenCodePath { .. }
                | TerminalEvent::FontZoomChanged
                | TerminalEvent::FleetSearchRequested { .. } => {}
            }
        })
        .detach();
    }

    /// Get a display title for a surface. Markdown surfaces use the file
    /// basename; terminal surfaces detect well-known programs from the OSC
    /// title.
    ///
    /// Both variants are capped at 24 chars (Zed `MAX_SURFACE_TITLE_LEN`,
    /// `crates/editor/src/items.rs:64`). The CSS truncation chain
    /// (`min_w_0 + overflow_x_hidden + text_ellipsis`) on the title div
    /// is a second layer that catches edge cases - but Zed's experience is
    /// that flex layouts with `max_w` (no explicit `w`) sometimes fail to
    /// propagate the constraint, so capping the string up front is
    /// load-bearing for visual consistency. Without this, a long markdown
    /// filename like `prd-opencode-sessions.md` overflows the header.
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

    /// Icon path for a surface (rendered as a small leading SVG in the header).
    /// Differentiates terminal, markdown and diff surfaces at a glance.
    fn surface_icon(surface: &PaneSurface) -> &'static str {
        surface.kind_icon()
    }

    fn terminal_surface_title(terminal: &Entity<TerminalView>, cx: &App) -> String {
        let view = terminal.read(cx);
        // US-013: a user-assigned custom name wins over the OSC-derived title
        // so a renamed surface visibly shows its new name.
        if let Some(custom) = view.terminal.custom_name.as_ref().filter(|c| !c.is_empty()) {
            return custom.clone();
        }
        let raw = &view.terminal.title;
        if let Some(agent) = view.terminal.detected_agent {
            return agent.display_name().into();
        }
        // For shell titles like "user@host: /path/to/dir", extract the last path component
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
        // Fallback: pass the raw title through. Length capping happens
        // uniformly in `surface_title` via `truncate_surface_title`, which counts
        // chars (not bytes) so multibyte UTF-8 stays sound.
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

    /// Render a small icon button for the pane header end section.
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

    /// A 14px header icon. Monochrome logos receive their tint directly:
    /// GPUI does not preserve inherited text color through every `AnyElement`
    /// and animation boundary. Multi-color logos render via `img()`, which
    /// keeps every native fill and gradient.
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

    /// Shared shell for header icon buttons. The live progress cell lets a
    /// rapid enter/exit reverse from the currently painted value instead of
    /// snapping to an endpoint.
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

    /// Fixed-size wrapper for the far-right header action cluster.
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

    /// Paint one frame of the close chip at `progress` (0 = resting, 1 =
    /// hovered). Fill and glyph move together: the circle washes from a faint
    /// tint of the theme text color up to a near-solid one while the cross
    /// inverts to the base color, so the chip reads as filled rather than as a
    /// glyph that merely brightened. The glyph is rebuilt here rather than
    /// passed in because `svg()` is a mask with no inherited color: it has to
    /// carry its own tint on every frame.
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

    /// The close affordance, in the skin of the tab-strip close button that
    /// EP-002 US-007 retired: a `shadow_lg` round chip, 15px, whose fill and
    /// glyph cross-fade on hover over `HEADER_HOVER_MS`. It rides the same
    /// hover-motion map as the trailing action buttons, so an interrupted
    /// hover resumes from where it was instead of snapping.
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
            // The header is a drag handle; without this the press that closes a
            // pane would also arm a pane drag.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _e: &ClickEvent, _window, cx| {
                this.close(cx);
                cx.stop_propagation();
            }))
            .child(visual)
            .into_any_element()
    }

    /// Close this pane (EP-002 US-004). A pane holds exactly one surface, so
    /// closing it always removes the pane from the layout tree - there is no
    /// intermediate "empty pane" state, and the parent owns the reflow.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(PaneEvent::Remove);
    }

    /// Shared `on_drag_move` body for every drag type accepted by a pane
    /// ([`SessionDrag`], [`MarkdownFileDrag`], [`PaneDrag`]): resolve the cursor (relative to
    /// the content `bounds`) to a split edge and, when it changes, seed the
    /// overlay glide and request a repaint. Both drag types drive the same blue
    /// preview, so the geometry lives here once.
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

    /// Seed the overlay glide for a new drop region and request a repaint when
    /// (and only when) the region actually changed.
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

    /// This pane's terminal, when its surface is one. `None` for a markdown or
    /// diff surface - every caller (event handlers, workspace ops, IPC, header
    /// action buttons) must handle that absence. Unlike the pre-EP-002 API this
    /// can no longer miss because of a stale index: the `Option` now encodes
    /// the surface's kind and nothing else.
    pub fn active_terminal_opt(&self) -> Option<&Entity<TerminalView>> {
        self.surface.as_terminal()
    }

    // -----------------------------------------------------------------------
    // Pane header rendering - identity + action cluster, no tab strip
    // -----------------------------------------------------------------------

    /// Render the surface's title slot: an ellipsized label, and nothing more.
    /// The inline rename editor that used to live here went out with its only
    /// gesture (double-click); `surface.rename` over IPC remains the way to set
    /// a custom name.
    fn render_surface_title(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
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

    /// The pane header (EP-002 US-006). It carries the surface name, the FR-11
    /// status adornments, the agent pill and the existing action cluster - and
    /// no list of tabs, which EP-002 removed. Like the strip it replaces, it
    /// paints nothing of its own: the floating card behind it owns the fill,
    /// which also keeps the card's top arcs intact (commit `30e26c5`). The
    /// unfocused dim from `f2bdd9d` covers the header exactly like the surface
    /// below it: the header is rendered *inside* the dim host (`pane-content`),
    /// under the dim layer and under the drop overlay.
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = pane_colors();

        // US-018 (orchestration-v2): an agent waiting for input shows the
        // attention-colored dot.
        // EP-004 US-010: an Errored agent (binary exited non-zero) wins over
        // waiting - a crash is the most salient state and must never hide
        // behind a waiting dot. Dedicated `agent_error` slot, distinct from
        // the attention orange.
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

        // EP-001 US-003 (cli-cockpit): queued-prompt chip - ranked just below
        // the state dot in the FR-11 anatomy.
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

        // EP-005 US-013 + EP-006 US-018 - identity pill and the transient
        // fleet-match badge, governed by the FR-11 anatomy: at most 2
        // adornments, in priority order state dot > queued chip > identity
        // pill > match badge. The pill degrades to its icon alone ("point
        // coloré") when it shares the header with another adornment; the match
        // badge - lowest priority, "s'efface en premier" - takes the last slot
        // if any.
        let (agent_pill, match_badge) = {
            let term_meta = self.surface.as_terminal().map(|t| {
                let r = t.read(cx);
                (r.terminal.detected_agent, r.terminal.agent_confirmed)
            });
            let mut slots_used: u8 = u8::from(has_errored || has_attention) + u8::from(has_pending);
            let mut pill = None;
            let mut hits_badge = None;
            if let Some((agent, confirmed)) = term_meta {
                if let Some(agent) = agent
                    && slots_used < 2
                {
                    let compact = slots_used == 1;
                    pill = Some(Self::render_agent_pill(agent, confirmed, compact, ui));
                    slots_used += 1;
                }
                if slots_used < 2
                    && let Some(count) = self.search_hits.filter(|c| *c > 0)
                {
                    hits_badge = Some(
                        div()
                            .flex_none()
                            .px(px(4.))
                            .rounded(px(3.))
                            .bg(ui.subtle)
                            .text_size(px(9.))
                            .text_color(ui.accent)
                            .child(format!("{count} hits"))
                            .into_any_element(),
                    );
                }
            }
            (pill, hits_badge)
        };

        // Identity area. `flex_1` + `min_w_0` + `overflow_x_hidden` is what
        // keeps a long surface name inside the card: the title slot ellipsizes
        // and the action cluster (`flex_none`, rendered as this row's sibling)
        // is never pushed out of the header.
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
            // A click on the name focuses the surface, and that is all it does:
            // a double-click no longer opens the inline rename (Arthur). The
            // name is a label here, not a control - double-clicking a word in
            // the header should not put the pane into an editing mode the user
            // did not ask for.
            .on_click(cx.listener(|this, _e: &ClickEvent, window, cx| {
                this.focus_handle(cx).focus(window, cx);
                cx.notify();
                cx.stop_propagation();
            }))
            .child(self.render_surface_title(cx))
            .children(status_dot)
            .children(pending_chip)
            .children(agent_pill)
            .children(match_badge);

        // Close the pane. It lives in the header's leading corner, alone and
        // opposite the constructive actions: the one destructive control here
        // must not sit a pixel away from "split" in the same cluster. It is
        // invisible at rest and revealed by `group_hover` on the header, the
        // same reveal the sidebar rows use - a cockpit shows one cross per
        // visible pane otherwise, which is noise on a surface meant to melt
        // into the terminal.
        //
        // Revealing it shifts nothing: the leading zone is `flex_1` with a zero
        // basis, so its width comes from the free space it is granted, never
        // from its content. The centered name therefore holds its position
        // whether the cross is painted or not.
        //
        // The tooltip quotes the *live* binding: a user who remapped
        // `close_pane` in Settings reads their own key here, not the default.
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

        // Header melts into the terminal body below it - one clean surface
        // (Arthur). It paints nothing of its own: the pane card behind it owns
        // the fill, which also keeps the card's top arcs intact.
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
            // Pane reordering (PRD `prd-pane-drag-drop-2026-Q3.md`): the header
            // is the grab handle, exactly as the tab chip was before EP-002
            // removed the strip. GPUI applies its own movement threshold before
            // firing `on_drag`, so the identity's click (focus) still works and
            // the action buttons keep their own hitboxes.
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
            // EP-002 US-007: the pane context menu is anchored here now that
            // the tab strip is gone, so the surface actions it carries (copy
            // path, cancel a queued prompt, close) stay reachable.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_this, e: &MouseDownEvent, _window, cx| {
                    cx.emit(PaneEvent::OpenPaneMenu {
                        position: e.position,
                    });
                    cx.stop_propagation();
                }),
            )
            // True optical centering (Arthur), done with three flex zones
            // rather than an absolute layer or a reserved gutter: both side
            // zones are `flex_1` with a zero basis, so they split the slack
            // evenly and the name lands on the pane's own center line. The
            // leading zone carries `min_w_0` and can collapse to nothing; the
            // trailing one keeps its automatic (content) minimum, so the action
            // cluster is never squeezed. On a narrow pane the name therefore
            // takes every pixel up to the buttons and only *then* ellipsizes -
            // a symmetric reserve truncated it at half the width it had.
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

    /// EP-005 US-013: compact agent identity pill for this pane's surface.
    /// PID-sourced (never the OSC title); `compact` renders the icon alone (the
    /// FR-11 "point coloré" degradation); an unconfirmed (session-restored,
    /// pre-first-scan) pill renders at 0.6 opacity with a "last known"
    /// tooltip.
    fn render_agent_pill(
        agent: crate::agent_launcher::TerminalAgent,
        confirmed: bool,
        compact: bool,
        ui: crate::theme::UiColors,
    ) -> gpui::AnyElement {
        // Multi-color brand logos need `img()` (resvg keeps every fill);
        // monochrome logos are `svg()` masks tinted with the brand accent
        // or the theme text color - same split as the header launchers.
        let icon: gpui::AnyElement = if agent.icon_multicolor() {
            img(agent.icon_path())
                .w(px(11.))
                .h(px(11.))
                .flex_none()
                .into_any_element()
        } else {
            let tint: Hsla = agent.accent().map(|c| rgb(c).into()).unwrap_or(ui.text);
            svg()
                .size(px(11.))
                .flex_none()
                .path(agent.icon_path())
                .text_color(tint)
                .into_any_element()
        };
        let tooltip_label: SharedString = if confirmed {
            agent.display_name().into()
        } else {
            format!("{} (last known - awaiting scan)", agent.display_name()).into()
        };
        let mut pill = div()
            .id("pane-header-agent-pill")
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(px(3.))
            .px(px(4.))
            .h(px(14.))
            .rounded(px(7.))
            .bg(ui.subtle)
            .child(icon);
        if !compact {
            // Short name: first word of the display name ("Claude Code" →
            // "Claude") keeps the pill compact under header truncation.
            let short = agent
                .display_name()
                .split_whitespace()
                .next()
                .unwrap_or(agent.display_name())
                .to_string();
            pill = pill.child(div().text_size(px(9.)).text_color(ui.text).child(short));
        }
        if !confirmed {
            pill = pill.opacity(0.6);
        }
        pill.delayed_tooltip(move |_w, cx| {
            let label = tooltip_label.clone();
            cx.new(|_| crate::app::sidebar::SidebarTooltip { label })
                .into()
        })
        .into_any_element()
    }

    /// Trailing action-button cluster of the pane header (US-051: code-motion
    /// out of the former tab bar). Zoom badge + the five header actions: the
    /// two splits, the files tree, the agent-sessions sidebar and the diff
    /// dock. Deliberately fixed - the agent launchers and the per-workspace
    /// custom buttons moved out of the header (they stay reachable from the
    /// pane palette), so the cluster needs neither a fold toggle nor a
    /// computed width.
    /// Self-contained - recomputes the palette it needs.
    fn render_end_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = pane_colors();
        // End section: action buttons. No separator rules: the header melts
        // into the terminal body, so the action cluster floats on the same
        // surface as the surface name.
        // No padding of its own: the header root already carries `SECTION_PX`
        // on both edges, so the button shells stop on the same vertical rule as
        // the leading surface icon on the left. Adding it here again doubled
        // the right inset (and the icons, centered in a 22px shell, read a
        // further 4px in).
        let end_section = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .h_full()
            .gap(px(0.));

        let show_sessions_button =
            !crate::agent_sessions::enabled_session_agents_from_config(&self.cached_config)
                .is_empty();

        let mut action_cluster = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .h_full()
            .gap(px(HEADER_GAP));

        // Zoom indicator badge
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
            // Split vertical (panes side by side)
            .child(self.action_button(
                "pane-btn-split-v",
                "icons/split_vertical.svg",
                cx.listener(|_this, _, _window, cx| {
                    cx.emit(PaneEvent::Split(crate::layout::SplitDirection::Vertical));
                }),
                cx,
            ))
            // Split horizontal (panes top/bottom)
            .child(self.action_button(
                "pane-btn-split-h",
                "icons/split_horizontal.svg",
                cx.listener(|_this, _, _window, cx| {
                    cx.emit(PaneEvent::Split(crate::layout::SplitDirection::Horizontal));
                }),
                cx,
            ))
            // Agent session history for the active terminal's cwd. The cwd
            // lookup + filesystem scan happens in
            // `PaneFlowApp::handle_pane_event`; this button just toggles the
            // docked sidebar.
            //
            // Hidden when the user has toggled off every AI agent in
            // Settings → AI Agent: with no agent enabled the sidebar would open
            // empty.
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
            // Right-docked git diff for this pane's workspace folder. Trails
            // the cluster so the two splits keep their leading slots.
            .child(self.action_button(
                "pane-btn-diff-dock",
                "icons/git-pull-request.svg",
                cx.listener(|_this, _e: &ClickEvent, _window, cx| {
                    cx.emit(PaneEvent::ToggleDiffDock);
                    cx.stop_propagation();
                }),
                cx,
            ));

        end_section.child(action_cluster)
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

impl Render for Pane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal_selected = matches!(self.surface, PaneSurface::Terminal(_));
        let body = match &self.surface {
            PaneSurface::Terminal(t) => t.clone().into_any_element(),
            PaneSurface::Markdown(m) => m.clone().into_any_element(),
            PaneSurface::Diff(d) => d.clone().into_any_element(),
        };
        let theme = crate::theme::active_theme();
        let card_background = pane_card_background(
            &theme,
            self.cached_config.windows_terminal_material_enabled(),
            terminal_selected,
        );

        // Unfocused-pane dim (Ghostty `unfocused-split-opacity`, rebuilt as a
        // single layer instead of one per apprt): a plain compositing quad over
        // this pane's content, never a renderer or GPU effect. It sits inside
        // `content`, so the pane header, the US-018 attention glow, the peek badge,
        // the broadcast stripe and the Composer all stay at full contrast - the
        // dim only ever touches terminal/markdown/diff output. It carries no
        // `id` and no handlers, so GPUI inserts no hitbox for it and it cannot
        // swallow a click (Ghostty needs three explicit opt-outs for this).
        //
        // The Composer steals focus from the pane it is attached to, so a pane
        // hosting one is never dimmed even though it reads as unfocused.
        // `resolved_unfocused_pane_dim_alpha` already inverts opacity -> alpha
        // once, in the config crate; nothing here re-derives `1 - x`.
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

        // EP-003 drop-to-split: the content region hosts the drag-move
        // direction probe, the drop commit, and the blue preview overlay.
        // A unique group name (per pane entity) scopes `group_drag_over` so
        // only this pane's overlay reacts while a drag hovers its content.
        let group_name =
            SharedString::from(format!("pane-content-{}", cx.entity().entity_id().as_u64()));

        // Glide geometry: lerp the overlay from its previous region's rect to
        // the current one over a short ease, so the preview slides between
        // halves/center instead of hard-snapping (the cmux feel; Zed itself
        // snaps). The seq keys the animation ElementId, restarting the ease
        // each time the region changes; `split_rect` maps a `DropEdge` to an
        // absolute-pixel rect within the cached content size.
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

        // Translucent preview: full pane for center (move-into), or the half
        // the new split would occupy for an edge. `invisible()` by default and
        // only shown via `group_drag_over`, so it never paints - and so never
        // hit-tests / blocks terminal mouse input - unless something is being
        // dragged over this pane (US-008). Geometry is set per-frame by the
        // glide animator below (absolute px), not statically.
        // The overlay is also the drop target. Carrying `on_drop` here (rather
        // than on the parent `content`) is what gives the overlay its own
        // hitbox: GPUI's `should_insert_hitbox` keys off `drop_listeners`
        // (among others) but NOT off `group_drag_over`, so a handler-less div
        // never allocates a hitbox and its `group_drag_over` style is never
        // evaluated - i.e. the overlay would stay `invisible()` forever. This
        // mirrors Zed's `crates/workspace/src/pane.rs` drag-target div, which
        // is likewise `.invisible()` + `group_drag_over` + `on_drop`. The
        // hitbox is `HitboxBehavior::Normal`, so it never blocks the terminal's
        // mouse input behind it (Risk #3).
        let overlay_blue = Hsla::from(rgb(DROP_OVERLAY_BLUE));
        let swap_tint = pane_colors().text;
        let overlay = div()
            .absolute()
            .bg(overlay_blue.opacity(DROP_OVERLAY_BACKGROUND_ALPHA))
            .rounded(px(OVERLAY_RADIUS))
            .border_2()
            .border_color(overlay_blue)
            .invisible()
            // A session dragged from the sidebar lights up the drop overlay.
            .group_drag_over::<SessionDrag>(group_name.clone(), |s| s.visible())
            // A markdown file dragged from the Files sidebar - same overlay.
            .group_drag_over::<MarkdownFileDrag>(group_name.clone(), |s| s.visible())
            // A pane dragged by its header: same overlay, neutral palette -
            // the drop swaps the two panes instead of splitting this one.
            .group_drag_over::<PaneDrag>(group_name.clone(), move |s| {
                s.visible()
                    .bg(swap_tint.opacity(SWAP_OVERLAY_FILL_ALPHA))
                    .border_color(swap_tint.opacity(SWAP_OVERLAY_BORDER_ALPHA))
            })
            // Pane swap: hand the source id to `PaneFlowApp`, which owns the
            // layout tree (mutating it here would re-enter this entity).
            .on_drop(cx.listener(move |this, drag: &PaneDrag, _window, cx| {
                let edge = this.drag_split_direction.take();
                cx.emit(PaneEvent::DropPaneMove {
                    source_pane_id: drag.pane_id,
                    edge,
                });
                cx.notify();
            }))
            // Markdown drop: open the file via `MarkdownView`, split toward the
            // previewed edge (or, for the center band, in a new workspace tab -
            // EP-002 US-007). Tree mutation + open live in `PaneFlowApp`, so
            // emit + defer out of this callback (entity re-entrancy, mirrors
            // the session drop).
            .on_drop(
                cx.listener(move |this, drag: &MarkdownFileDrag, _window, cx| {
                    let edge = this.drag_split_direction.take();
                    cx.emit(PaneEvent::DropMarkdownSplit {
                        edge,
                        path: drag.path.clone(),
                    });
                    cx.notify();
                }),
            )
            // Session drop: spawn a fresh terminal running the resume command,
            // split toward the previewed edge (or, for the center band, in a
            // new workspace tab - EP-002 US-007). Tree mutation + spawn live in
            // `PaneFlowApp`, so emit and defer out of this callback (entity
            // re-entrancy, Risk #1).
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
            // Glide between regions: lerp the absolute-px rect from the previous
            // region to the current one over a short ease-out. The animation
            // self-drives frames until it settles (no terminal-poll dependency),
            // and restarts whenever `overlay_anim_id` changes (region change).
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
                    // Inset the visible box by a uniform margin so it floats
                    // inside the region (gap on every side, including the center
                    // line). The margin is applied *after* the lerp and is NOT
                    // stored in `live_rect` - seeding the next glide stays in the
                    // un-inset region space so `from`/`to` remain consistent.
                    let m = OVERLAY_MARGIN;
                    let cur = (
                        raw.0 + m,
                        raw.1 + m,
                        (raw.2 - 2.0 * m).max(0.0),
                        (raw.3 - 2.0 * m).max(0.0),
                    );
                    // Publish the *un-inset* live rect so the next region change
                    // can lerp from the box's actual mid-flight position, not
                    // the old target (kills the fast-crossing jump).
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
            // US-007: map the cursor within the content bounds to a split edge.
            // Stays on `content` (full pane) - the overlay shrinks to a half
            // when `dir = Some(edge)`, so probing there would miss the cursor
            // moving back toward the center band. `content` keeps its hitbox via
            // `.group(group_name)`.
            // Same edge-band probe for a session dragged out of the sidebar, so
            // it gets the identical blue preview (bridges the sessions PRD).
            .on_drag_move::<SessionDrag>(cx.listener(
                |this, e: &DragMoveEvent<SessionDrag>, _window, cx| {
                    this.apply_drag_edge(e.bounds, e.event.position, cx);
                },
            ))
            // Identical edge-band probe for a markdown file dragged in.
            .on_drag_move::<MarkdownFileDrag>(cx.listener(
                |this, e: &DragMoveEvent<MarkdownFileDrag>, _window, cx| {
                    this.apply_drag_edge(e.bounds, e.event.position, cx);
                },
            ))
            // Same edge-band probe for a pane dragged by its header: the
            // placeholder previews the half the pane will occupy (or the whole
            // pane, in the center band, where the two swap).
            .on_drag_move::<PaneDrag>(cx.listener(
                |this, e: &DragMoveEvent<PaneDrag>, _window, cx| {
                    this.apply_drag_edge(e.bounds, e.event.position, cx);
                },
            ))
            // EP-002 US-006: the header is a child of this host, not a
            // sibling of it, so the unfocused dim below covers the header
            // exactly like the surface under it. Ordering matters: dim over
            // header + body, drop overlay over the dim (a preview must stay
            // crisp on an unfocused pane), and the overlay stays inside
            // `group_name` so its `group_drag_over` still resolves.
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().w_full().child(body))
            .children(dim_layer)
            .child(overlay);

        // The blue active-pane focus ring is removed (Arthur): no border tint on
        // focus. The 1px border is the card's resting hairline, and doubles as
        // the US-018 attention glow slot - the width never changes, so the glow
        // paints without reflow.
        //
        // US-018 (orchestration-v2): a pane whose agent waits for input glows
        // with the attention color - amplify the waiting pane, never degrade
        // the others. This stays: it signals "agent needs you", not focus.
        let has_attention = self.attention.is_some();
        let attention_color = pane_colors().vc_conflict;
        let peek = self.render_peek_overlay(cx);
        let composer = self.render_composer_overlay(cx);
        let card_radius = crate::app::constants::PANE_CARD_RADIUS;
        div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .overflow_hidden()
            // The pane is a floating card over the window shell. Its silhouette
            // is a superellipse, which GPUI's `rounded()` cannot express, so the
            // fill and the hairline are painted as paths under and over the
            // subtree instead of as quad properties. Everything between them is
            // transparent: this pair is the card.
            //
            // The hairline keeps the card separable from its neighbors even on
            // themes whose shell and terminal colors coincide.
            .child(squircle_fill(card_radius, card_background))
            .child(content)
            .children(peek)
            // EP-001 US-002: broadcast-group stripe - a DISTINCT left-edge
            // element; the pane border slot above stays the attention glow's
            // (Files NOT to Modify). Absolutely positioned so it never
            // perturbs the header/content flex chain.
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

#[cfg(test)]
mod tests {
    use super::{
        MAX_SURFACE_TITLE_LEN, pane_card_background, peek_badge_line, truncate_surface_title,
    };

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
    fn peek_badge_takes_first_line_bounded() {
        assert_eq!(
            peek_badge_line("Allow `cargo test`?\ndetails…"),
            "Allow `cargo test`?"
        );
        let long = "x".repeat(120);
        let badge = peek_badge_line(&long);
        assert_eq!(badge.chars().count(), 81, "80 chars + ellipsis");
        assert!(badge.ends_with('…'));
        assert_eq!(peek_badge_line(""), "");
        // Multibyte safety: counts chars, not bytes.
        let accents = "é".repeat(100);
        assert!(peek_badge_line(&accents).ends_with('…'));
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
        // 25 chars in -> 24 chars out (23 head + ellipsis).
        let input = "prd-opencode-sessions.mdX";
        let out = truncate_surface_title(input);
        assert_eq!(out.chars().count(), MAX_SURFACE_TITLE_LEN);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn multibyte_utf8_does_not_panic() {
        // Earlier byte-slice path (`&raw[..23]`) panicked when index 23
        // landed in the middle of an accented or CJK char. The char-based
        // implementation must stay sound.
        let input = "événement-très-très-long-fichier.md"; // many multibyte chars
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
