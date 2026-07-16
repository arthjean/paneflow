//! Pane - a tabbed container holding one or more views (terminals or markdown
//! viewers, freely mixed within the same tab strip).
//!
//! Each leaf in the split tree holds an `Entity<Pane>`. A Pane manages an
//! ordered list of [`TabContent`] tabs and a single `selected_idx` cursor.
//! Markdown tabs and terminal tabs share the strip - the user opens markdown
//! files by clicking the doc icon (or Cmd/Ctrl-clicking a `.md` path inside a
//! terminal), and a new tab is appended to the same pane rather than splitting.
//!
//! Communication with the parent (split tree owner) uses the Zed pattern:
//! Pane emits `PaneEvent` via `cx.emit()`, parent subscribes via `cx.subscribe()`.
//!
//! Tab bar UI is modeled after Zed's tab bar design.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, ClickEvent, Context, DragMoveEvent, Entity,
    EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, Pixels, Point, Render, SharedString, Size, Styled, Transformation, Window,
    deferred, div, ease_out_quint, img, percentage, prelude::*, px, rgb, svg,
};
use paneflow_config::schema::ButtonCommand;

use crate::settings::components::with_alpha;

use crate::diff::DiffView;
use crate::markdown::MarkdownView;
use crate::pane_drag::{
    DropEdge, InsertSide, MarkdownFileDrag, SPLIT_EDGE_BAND, SessionDrag, TabDrag, TabDragPreview,
    compute_drop_edge, insertion_side, reordered_index, split_rect,
};
use crate::terminal::{TerminalEvent, TerminalView};

// ---------------------------------------------------------------------------
// TabContent - a tab can hold either a terminal or a markdown viewer
// ---------------------------------------------------------------------------

/// A single tab inside a pane. Terminal and markdown tabs share the strip so
/// the user keeps tab navigation (Ctrl+Tab, click) regardless of content type
/// opening a markdown file from a terminal pane appends a tab next to the
/// existing terminals rather than splitting the layout.
#[derive(Clone)]
pub enum TabContent {
    Terminal(Entity<TerminalView>),
    Markdown(Entity<MarkdownView>),
    Diff(Entity<DiffView>),
}

impl TabContent {
    pub fn as_terminal(&self) -> Option<&Entity<TerminalView>> {
        match self {
            TabContent::Terminal(t) => Some(t),
            TabContent::Markdown(_) | TabContent::Diff(_) => None,
        }
    }

    /// Stable identity of the tab's backing entity, regardless of variant.
    /// US-020: lets per-tab click closures re-resolve their live index by
    /// identity when the `Vec` mutates between render and click.
    pub fn entity_id(&self) -> gpui::EntityId {
        match self {
            TabContent::Terminal(t) => t.entity_id(),
            TabContent::Markdown(m) => m.entity_id(),
            TabContent::Diff(d) => d.entity_id(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tab bar color helpers - derived from active theme
// ---------------------------------------------------------------------------

fn tab_colors() -> crate::theme::UiColors {
    crate::theme::ui_colors()
}

fn tab_bar_background(theme: &crate::theme::TerminalTheme, terminal_material_active: bool) -> Hsla {
    if terminal_material_active && cfg!(target_os = "windows") {
        gpui::transparent_black()
    } else {
        theme.background
    }
}

fn pane_content_background(
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

/// Tab bar total height. Matches the Agents bottom-panel strip so the CLI and
/// Agents views speak one tab language: rounded chips floating with even
/// breathing room above the terminal body.
const TAB_BAR_HEIGHT: f32 = 40.0;
/// Chip height inside the bar. The slack on each side (bar 40 vs chip 28) is the
/// vertical float gap that makes a tab read as a chip, not a full-height slab.
const TAB_HEIGHT: f32 = 28.0;
/// Chip corner radius (rounded chip, not a square editor tab).
const TAB_RADIUS: f32 = 8.0;
/// Leading inner padding of a chip, before the icon.
const TAB_PL: f32 = 11.0;
/// Trailing inner padding of a chip, after the close button.
const TAB_PR: f32 = 5.0;
/// Gap between a chip's children (icon, label, adornments, close).
const TAB_GAP: f32 = 7.0;
/// Gap between adjacent chips in the strip.
const STRIP_GAP: f32 = 4.0;
/// Leading inset of the strip so the first chip isn't flush to the edge.
const STRIP_PL: f32 = 8.0;
/// Fixed chip width. Longer labels get truncated with ellipsis inside this box.
const TAB_WIDTH: f32 = 140.0;
/// Approximate title capacity inside `TAB_WIDTH` after the leading slot, gaps,
/// and horizontal padding. Above this, the CSS ellipsis is expected to engage.
const TAB_TITLE_TOOLTIP_THRESHOLD: usize = 13;
/// Leading icon / close-button slot size inside a chip.
const LEADING_SLOT_SIZE: f32 = 15.0;
/// Section padding (start/end areas of the bar).
const SECTION_PX: f32 = 6.0;
/// Square size shared by tab-bar icon buttons.
const ACTION_BUTTON_SIZE: f32 = 22.0;
/// Square size of the new-tab affordance that trails the last tab chip.
const ADD_TAB_BUTTON_SIZE: f32 = TAB_HEIGHT;
/// Plus glyph size inside the new-tab affordance.
const ADD_TAB_ICON_SIZE: f32 = 16.0;
/// Approximate width of the compact zoom badge in the action cluster.
const ZOOM_BADGE_WIDTH: f32 = 18.0;
/// Duration for folding/unfolding the tab-bar action cluster.
const ACTION_CLUSTER_ANIMATION_MS: u64 = 280;
/// Uniform gap (px) between the drop-to-split preview overlay and its region's
/// edges, so the blue box floats inside the target half/pane (EP-003 US-008).
const OVERLAY_MARGIN: f32 = 8.0;
/// Corner radius (px) of the drop-to-split preview overlay.
const OVERLAY_RADIUS: f32 = 8.0;
/// Apple system blue (#007AFF), used for the CLI drop placement preview.
const DROP_OVERLAY_BLUE: u32 = 0x007aff;
/// Low-alpha fill so the placement card stays visible without washing the pane.
const DROP_OVERLAY_BACKGROUND_ALPHA: f32 = 0.10;
/// Hard upper bound on tab title length in characters. Mirrors Zed's
/// `MAX_TAB_TITLE_LEN` (`zed/crates/editor/src/items.rs:64`). Anything past
/// this is replaced with a trailing ellipsis before the CSS ellipsis layer.
const MAX_TAB_TITLE_LEN: usize = 24;

/// Char-boundary-safe `truncate_and_trailoff`. Counts chars (not bytes) so
/// filenames with multibyte UTF-8 (accents, CJK, emoji) don't trigger a
/// byte-index panic, and reserves one char for the trailing `…`.
fn truncate_tab_title(raw: &str) -> String {
    if raw.chars().count() <= MAX_TAB_TITLE_LEN {
        return raw.to_string();
    }
    let head: String = raw.chars().take(MAX_TAB_TITLE_LEN - 1).collect();
    format!("{head}…")
}

// ---------------------------------------------------------------------------
// Pane events - emitted to parent via cx.emit()
// ---------------------------------------------------------------------------

pub enum PaneEvent {
    /// The last tab was closed - parent should remove this pane from the split tree.
    Remove,
    /// Request a split in the given direction from this pane.
    Split(crate::layout::SplitDirection),
    /// Request a fresh terminal tab in this pane. Routed to `PaneFlowApp` (not
    /// handled in the `Pane`) so the new terminal spawns at the owning
    /// workspace's cwd - the `Pane` knows only its `workspace_id`, not the
    /// directory - and gets the app-level CWD/port/service subscription wired,
    /// exactly like `DropSplit` / `DuplicateTabInto`. Without this, a new tab on
    /// Windows opened in the process `current_dir()` (`C:\Program Files\PaneFlow`).
    NewTerminalTab,
    /// Toggle the docked agent-sessions sidebar for the active terminal's cwd
    /// (PRD `prd-agent-sessions-sidebar-2026-Q3`). The parent resolves the cwd,
    /// binds this pane, and spawns the per-agent scans; no anchor is needed
    /// since the sidebar docks in the root layout rather than floating.
    ToggleAgentSessions,
    /// Toggle the docked Files sidebar for the active workspace's folder
    /// (PRD `prd-files-tree-sidebar-2026-Q3`, EP-001). Payload-free: the parent
    /// resolves the active workspace's `cwd` to the tree root and enforces
    /// mutual exclusion with the sessions sidebar.
    ToggleFilesSidebar,
    /// Copy the active terminal's human-readable surface reference (its
    /// disambiguated name, e.g. `cargo-run`) to the clipboard (US-010).
    /// Carries the surface_id; the parent resolves the globally-disambiguated
    /// name via `collect_surface_meta` so the copied value matches what the
    /// MCP `list_panes` tool advertises.
    CopySurfaceRef(u64),
    /// A surface's custom name changed via inline rename (US-013) - the parent
    /// should persist the session so the name survives restart.
    SurfaceRenamed,
    /// The tab order, active tab, or membership changed without changing the
    /// layout tree. The app persists this because pane-local mutations can
    /// otherwise be lost on crash.
    TabsChanged,
    /// Right-click on a tab requested the "Move to pane…" context menu
    /// (EP-002 US-006, WCAG 2.5.7 non-drag alternative).
    OpenTabMenu {
        tab_id: gpui::EntityId,
        position: Point<Pixels>,
    },
    /// A tab was dropped on this pane's content edge to create a split
    /// (EP-003 US-009). The parent owns the `LayoutTree`, so it performs the
    /// `split_at_pane` and moves (or, with `duplicate`, copies) the dragged
    /// terminal into the new pane. The emitting pane is the split *target*.
    DropSplit {
        edge: DropEdge,
        source_pane: Entity<Pane>,
        source_tab_id: gpui::EntityId,
        /// `true` when the duplicate modifier was held (Ctrl on Linux/Windows,
        /// Alt on macOS) - spawn a fresh terminal at the dragged tab's CWD
        /// instead of moving the original (US-010).
        duplicate: bool,
    },
    /// A tab was dropped on this pane's tab strip (or content center) with the
    /// duplicate modifier held (EP-003 US-010). The parent spawns a fresh
    /// terminal at the dragged tab's CWD and inserts it into this - the
    /// emitting - pane at `dest_idx`, leaving the original in place. Routed to
    /// `PaneFlowApp` because spawning a terminal needs the app-level CWD/port
    /// subscription wiring (mirrors `DropSplit`'s duplicate path).
    DuplicateTabInto {
        source_pane: Entity<Pane>,
        source_tab_id: gpui::EntityId,
        dest_idx: usize,
    },
    /// An agent-session row was dropped out of the sessions sidebar onto this
    /// pane (bridges `prd-agent-sessions-sidebar` × `prd-pane-drag-drop`). The
    /// parent spawns a *fresh* terminal at `cwd` running the agent's resume
    /// command, then - for `edge = Some` - splits this (the emitting target)
    /// pane toward that edge, or - for `edge = None` (center) - appends it as a
    /// new tab here. Routed to `PaneFlowApp` because spawning a terminal needs
    /// the app-level CWD/port subscription wiring (mirrors `DropSplit`).
    DropSessionSplit {
        edge: Option<DropEdge>,
        agent: crate::agent_sessions::SessionAgent,
        session_id: String,
        cwd: String,
    },
    /// A markdown file was dropped out of the Files sidebar onto this (the
    /// emitting target) pane (PRD `prd-files-tree-sidebar-2026-Q3`, EP-003).
    /// For `edge = Some` the parent opens the file in a new pane split toward
    /// that edge; for `edge = None` (center) it appends the markdown as a new
    /// tab here. Routed to `PaneFlowApp` (LayoutTree owner) to keep the tree
    /// mutation out of the drop callback (entity re-entrancy, mirrors
    /// `DropSessionSplit`).
    DropMarkdownSplit {
        edge: Option<DropEdge>,
        path: std::path::PathBuf,
    },
}

/// Inline tab-rename state (US-013).
struct TabRename {
    /// Index of the tab being renamed.
    idx: usize,
    /// In-progress name buffer.
    buffer: String,
}

// ---------------------------------------------------------------------------
// Pane - tabbed terminal container
// ---------------------------------------------------------------------------

pub struct Pane {
    pub tabs: Vec<TabContent>,
    pub selected_idx: usize,
    /// US-018/US-020 (orchestration-v2): terminals of this pane whose agent
    /// session is `WaitingForInput`, with the agent's question (≤512 chars,
    /// UNTRUSTED display-only text). Pushed by `PaneFlowApp::sync_attention`
    /// recomputed from the session truth on every transition, never
    /// mutated locally. Drives the attention ring, the tab dot and the peek
    /// overlay.
    attention: std::collections::HashMap<gpui::EntityId, Option<String>>,
    /// EP-004 US-010 (cli-cockpit): terminals of this pane whose agent
    /// session is `Errored` (the agent binary exited non-zero). Pushed by
    /// `PaneFlowApp::sync_attention` alongside `attention` - same idempotent
    /// recompute-from-session-truth contract. Drives the dedicated
    /// `agent_error` tab dot (tab-anatomy state slot, ranked above the
    /// waiting dot).
    errored: std::collections::HashSet<gpui::EntityId>,
    /// EP-006 US-018 (cli-cockpit): transient fleet-grep match counts per
    /// terminal. Pushed by `PaneFlowApp::push_fleet_badges` after a fan-out,
    /// cleared 4 s later or when the fleet overlay closes. FR-11: the
    /// LOWEST-priority tab adornment - first to yield its slot.
    search_hits: std::collections::HashMap<gpui::EntityId, usize>,
    /// US-020: the peek badge is hovered - render the full question panel.
    peek_expanded: bool,
    /// Set to true when the workspace is zoomed on this pane.
    pub zoomed: bool,
    /// Workspace ID for spawning new terminals with correct env vars.
    pub workspace_id: u64,
    /// Workspace-specific command buttons rendered in the tab bar after the
    /// built-in defaults. Populated/updated by `Workspace::propagate_custom_buttons`.
    pub custom_buttons: Vec<ButtonCommand>,
    /// Local UI state for folding the dense right-side tab-bar action cluster.
    /// Kept per pane and intentionally not persisted: it is a transient layout
    /// preference for the current window, not workspace state.
    tab_bar_actions_collapsed: bool,
    /// Incremented on every action-cluster toggle so the GPUI one-shot
    /// animation gets a fresh element id and replays both ways.
    tab_bar_actions_animation_epoch: u64,
    /// US-015: cached `paneflow.json` so `render_tab_bar` never calls the
    /// blocking `load_config()` per frame (the agent-button visibility gate and
    /// the launch command read it). Hydrated at creation, refreshed by
    /// `PaneFlowApp::process_config_changes` → `Workspace::propagate_config` on
    /// every `ConfigWatcher` reload, so a Settings flip (e.g. the Claude bypass
    /// toggle) takes effect on the next click without a per-frame disk read.
    pub cached_config: paneflow_config::schema::PaneFlowConfig,
    /// Inline tab-rename state (US-013). `None` when not renaming.
    rename: Option<TabRename>,
    /// Focus target for the inline rename input, so keystrokes route to the
    /// rename handler (not the terminal) while a tab name is being edited.
    rename_focus: FocusHandle,
    /// Live drop-to-split target (EP-003 US-007): the edge the blue overlay
    /// previews while a tab is dragged over this pane's content. `None` =
    /// center band (move-into-pane) or no drag. Updated by the content
    /// `on_drag_move` handler; reset on drop. While no drag is active the
    /// overlay is `invisible()` regardless of this value, so a stale value
    /// after a cancel is harmless (the next drag-move recomputes it).
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
    /// EP-001 US-003: terminals of this pane holding a queued prompt
    /// (broadcast/Composer buffer awaiting the agent's next idle transition).
    /// Pushed by `PaneFlowApp::sync_pending_chips`; drives the "1 queued" tab
    /// chip.
    pending_prefill: std::collections::HashSet<gpui::EntityId>,
    /// EP-001 US-002: broadcast-group stripe color index (`UiColors::group_*`)
    /// when this pane is a group member. Pushed by
    /// `PaneFlowApp::sync_broadcast_stripes`. The stripe is a DISTINCT element
    /// from the attention border below - the pane border slot stays the glow's.
    broadcast_stripe: Option<usize>,
}

impl EventEmitter<PaneEvent> for Pane {}

impl Pane {
    /// Create a new pane with a single terminal tab.
    pub fn new(terminal: Entity<TerminalView>, workspace_id: u64, cx: &mut Context<Self>) -> Self {
        Self::subscribe_terminal(&terminal, cx);
        let cached_config = paneflow_config::loader::load_config();
        Self::apply_terminal_render_config(&terminal, &cached_config, cx);
        Self {
            tabs: vec![TabContent::Terminal(terminal)],
            selected_idx: 0,
            attention: std::collections::HashMap::new(),
            errored: std::collections::HashSet::new(),
            search_hits: std::collections::HashMap::new(),
            peek_expanded: false,
            zoomed: false,
            workspace_id,
            custom_buttons: Vec::new(),
            tab_bar_actions_collapsed: false,
            tab_bar_actions_animation_epoch: 0,
            // US-015: hydrate the tab-bar config cache once at creation (not
            // per frame); refreshed on ConfigWatcher reload via propagation.
            cached_config,
            rename: None,
            rename_focus: cx.focus_handle(),
            drag_split_direction: None,
            overlay_prev_dir: None,
            overlay_from: (0.0, 0.0, 0.0, 0.0),
            overlay_current: Rc::new(Cell::new((0.0, 0.0, 0.0, 0.0))),
            overlay_seq: 0,
            overlay_pane_size: Size::default(),
            composer_slot: None,
            pending_prefill: std::collections::HashSet::new(),
            broadcast_stripe: None,
        }
    }

    /// Create a new pane wrapping an existing tab moved in from elsewhere
    /// (EP-003 drop-to-split). The pane-level subscription is wired for a
    /// terminal tab so `ChildExited`/`TitleChanged` route here, but - unlike
    /// [`crate::PaneFlowApp::create_pane`] - the app-level terminal
    /// subscription is NOT re-added, because the moved terminal already has
    /// one from its original creation (re-adding would double CWD/port events).
    pub fn new_with_tab(tab: TabContent, workspace_id: u64, cx: &mut Context<Self>) -> Self {
        Self::new_with_tabs(vec![tab], 0, workspace_id, cx)
    }

    pub fn new_with_tabs(
        tabs: Vec<TabContent>,
        selected_idx: usize,
        workspace_id: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        let cached_config = paneflow_config::loader::load_config();
        for tab in &tabs {
            if let TabContent::Terminal(t) = tab {
                Self::subscribe_terminal(t, cx);
                Self::apply_terminal_render_config(t, &cached_config, cx);
            }
        }
        let selected_idx = selected_idx.min(tabs.len().saturating_sub(1));
        Self {
            tabs,
            selected_idx,
            attention: std::collections::HashMap::new(),
            errored: std::collections::HashSet::new(),
            search_hits: std::collections::HashMap::new(),
            peek_expanded: false,
            zoomed: false,
            workspace_id,
            custom_buttons: Vec::new(),
            tab_bar_actions_collapsed: false,
            tab_bar_actions_animation_epoch: 0,
            // US-015: see `Pane::new`.
            cached_config,
            rename: None,
            rename_focus: cx.focus_handle(),
            drag_split_direction: None,
            overlay_prev_dir: None,
            overlay_from: (0.0, 0.0, 0.0, 0.0),
            overlay_current: Rc::new(Cell::new((0.0, 0.0, 0.0, 0.0))),
            overlay_seq: 0,
            overlay_pane_size: Size::default(),
            composer_slot: None,
            pending_prefill: std::collections::HashSet::new(),
            broadcast_stripe: None,
        }
    }

    /// US-018/US-020 (orchestration-v2): replace this pane's attention map
    /// (terminals whose agent waits for input + their question). Idempotent
    /// push from `PaneFlowApp::sync_attention` - repaints only on change.
    pub fn set_attention(
        &mut self,
        attention: std::collections::HashMap<gpui::EntityId, Option<String>>,
        cx: &mut Context<Self>,
    ) {
        if self.attention != attention {
            if attention.is_empty() {
                self.peek_expanded = false;
            }
            self.attention = attention;
            cx.notify();
        }
    }

    /// EP-004 US-010 (cli-cockpit): replace this pane's Errored set
    /// (terminals whose agent binary exited non-zero). Same idempotent
    /// push contract as [`Pane::set_attention`] - repaints only on change.
    pub fn set_errored(
        &mut self,
        errored: std::collections::HashSet<gpui::EntityId>,
        cx: &mut Context<Self>,
    ) {
        if self.errored != errored {
            self.errored = errored;
            cx.notify();
        }
    }

    /// EP-006 US-018 (cli-cockpit): replace this pane's transient fleet-grep
    /// badge counts. Same idempotent push contract as [`Pane::set_attention`].
    pub fn set_search_hits(
        &mut self,
        hits: std::collections::HashMap<gpui::EntityId, usize>,
        cx: &mut Context<Self>,
    ) {
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

    /// EP-001 US-003: replace the queued-prompt indicator set. Idempotent
    /// push from `PaneFlowApp::sync_pending_chips` - repaints only on change.
    pub fn set_pending_prefill(
        &mut self,
        pending: std::collections::HashSet<gpui::EntityId>,
        cx: &mut Context<Self>,
    ) {
        if self.pending_prefill != pending {
            self.pending_prefill = pending;
            cx.notify();
        }
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
        let ui = tab_colors();

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
        header = header.child(
            div()
                .id("composer-broadcast-toggle")
                .px(px(6.))
                .py(px(2.))
                .rounded(px(4.))
                .text_size(px(10.))
                .cursor_pointer()
                .when(slot.broadcast, |d| {
                    d.bg(ui.accent.opacity(0.15)).text_color(ui.accent)
                })
                .when(!slot.broadcast, |d| {
                    d.bg(ui.subtle)
                        .text_color(ui.muted)
                        .hover(|s| s.text_color(ui.text))
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
                    .cursor_pointer()
                    .hover(|s| s.text_color(ui.vc_deleted))
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
                    .bg(gpui::hsla(0., 0., 0., 0.25))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        dismiss_backdrop(cx);
                    })
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
    /// Top-right under the tab bar so the agent's prompt line stays visible.
    fn render_peek_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.attention.is_empty() {
            return None;
        }
        // Prefer the visible tab's question; else the first waiting tab's.
        let message = self
            .tabs
            .get(self.selected_idx)
            .and_then(|t| t.as_terminal())
            .and_then(|t| self.attention.get(&t.entity_id()))
            .or_else(|| {
                self.tabs
                    .iter()
                    .filter_map(TabContent::as_terminal)
                    .find_map(|t| self.attention.get(&t.entity_id()))
            })
            .cloned()
            .flatten();
        let ui = tab_colors();
        let full = message.unwrap_or_else(|| "waiting for input".to_string());
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
                .top(px(TAB_BAR_HEIGHT + 6.0))
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

    /// Iterate over the terminal entities in this pane. Markdown tabs are
    /// skipped. Used by event handlers that need to scan terminals - sidebar
    /// counters, AI-tool PID owner lookups, layout serialization.
    pub fn terminals(&self) -> impl Iterator<Item = &Entity<TerminalView>> {
        self.tabs.iter().filter_map(TabContent::as_terminal)
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
        let terminal_material_active = config.windows_terminal_material_enabled();
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
            terminal.set_terminal_material_active(terminal_material_active, cx);
            terminal.set_integrated_glyphs_enabled(integrated_glyphs_enabled, cx);
            terminal.set_color_emoji_enabled(color_emoji_enabled, cx);
            terminal.set_cursor_color_override(cursor_color_override, cx);
        });
    }

    /// True when `terminal` is one of this pane's tabs.
    pub fn contains_terminal(&self, terminal: &Entity<TerminalView>) -> bool {
        self.terminals().any(|t| t == terminal)
    }

    /// Append a new terminal tab and focus it.
    pub fn add_tab(&mut self, terminal: Entity<TerminalView>, cx: &mut Context<Self>) {
        Self::subscribe_terminal(&terminal, cx);
        Self::apply_terminal_render_config(&terminal, &self.cached_config, cx);
        self.tabs.push(TabContent::Terminal(terminal));
        self.selected_idx = self.tabs.len().saturating_sub(1);
    }

    /// Append a markdown viewer tab and focus it. Used by the doc-button
    /// handler in this pane's tab strip and by the Cmd/Ctrl-click flow on
    /// `.md` paths inside a terminal - both routes converge on this method
    /// via `PaneFlowApp::open_markdown_in_pane`.
    ///
    /// Markdown tabs don't need an event subscription: `MarkdownView` does
    /// not emit pane-level events. Closing the tab through the tab strip's
    /// close button drops the entity, which in turn drops its file watcher.
    pub fn add_markdown_tab(&mut self, markdown: Entity<MarkdownView>, _cx: &mut Context<Self>) {
        self.tabs.push(TabContent::Markdown(markdown));
        self.selected_idx = self.tabs.len().saturating_sub(1);
    }

    /// Append a multi-worktree diff tab and select it. Like markdown tabs,
    /// `DiffView` emits no pane-level events, so no subscription is needed;
    /// closing the tab drops the entity (and any future watchers it owns).
    pub fn add_diff_tab(&mut self, diff: Entity<DiffView>, _cx: &mut Context<Self>) {
        self.tabs.push(TabContent::Diff(diff));
        self.selected_idx = self.tabs.len().saturating_sub(1);
    }

    /// Subscribe to a terminal's events - close tab on exit, repaint on title change.
    fn subscribe_terminal(terminal: &Entity<TerminalView>, cx: &mut Context<Self>) {
        cx.subscribe(terminal, |this, terminal, event: &TerminalEvent, cx| {
            match event {
                TerminalEvent::ChildExited => {
                    if let Some(idx) = this
                        .tabs
                        .iter()
                        .position(|t| t.as_terminal() == Some(&terminal))
                    {
                        this.close_tab_at(idx, cx);
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

    /// Get a display title for a tab. Markdown tabs use the file basename;
    /// terminal tabs detect well-known programs from the OSC title.
    ///
    /// Both variants are capped at 24 chars (Zed `MAX_TAB_TITLE_LEN`,
    /// `crates/editor/src/items.rs:64`). The CSS truncation chain
    /// (`min_w_0 + overflow_x_hidden + text_ellipsis`) on the title div
    /// is a second layer that catches edge cases - but Zed's experience is
    /// that flex layouts with `max_w` (no explicit `w`) sometimes fail to
    /// propagate the constraint, so capping the string up front is
    /// load-bearing for visual consistency. Without this, a long markdown
    /// filename like `prd-opencode-sessions.md` overflows the tab chip.
    fn tab_full_title(tab: &TabContent, cx: &App) -> String {
        match tab {
            TabContent::Markdown(md) => md.read(cx).title().to_string(),
            TabContent::Diff(d) => d.read(cx).title(),
            TabContent::Terminal(t) => Self::terminal_tab_full_title(t, cx),
        }
    }

    fn tab_title(tab: &TabContent, cx: &App) -> String {
        let raw = match tab {
            TabContent::Markdown(md) => md.read(cx).title().to_string(),
            TabContent::Diff(d) => d.read(cx).title(),
            TabContent::Terminal(t) => Self::terminal_tab_title(t, cx),
        };
        truncate_tab_title(&raw)
    }

    /// Icon path for a tab (rendered as a small leading SVG inside the tab
    /// chip). Differentiates terminal and markdown tabs at a glance.
    fn tab_icon(tab: &TabContent) -> &'static str {
        match tab {
            TabContent::Terminal(_) => "icons/terminal.svg",
            TabContent::Markdown(_) => "icons/file-text.svg",
            TabContent::Diff(_) => "icons/git-branch.svg",
        }
    }

    fn terminal_tab_title(terminal: &Entity<TerminalView>, cx: &App) -> String {
        let view = terminal.read(cx);
        // US-013: a user-assigned custom name wins over the OSC-derived title
        // so a renamed tab visibly shows its new name.
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
        // uniformly in `tab_title` via `truncate_tab_title`, which counts
        // chars (not bytes) so multibyte UTF-8 stays sound.
        if raw.is_empty() {
            "Terminal".into()
        } else {
            raw.clone()
        }
    }

    fn terminal_tab_full_title(terminal: &Entity<TerminalView>, cx: &App) -> String {
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

    fn add_action_cluster_width(width: &mut f32, count: &mut usize, item_width: f32) {
        if *count > 0 {
            *width += TAB_GAP;
        }
        *width += item_width;
        *count += 1;
    }

    fn tab_bar_action_cluster_width(
        zoomed: bool,
        fixed_buttons: usize,
        agent_buttons: usize,
        custom_buttons: usize,
    ) -> f32 {
        let mut width = 0.0;
        let mut count = 0;
        if zoomed {
            Self::add_action_cluster_width(&mut width, &mut count, ZOOM_BADGE_WIDTH);
        }
        for _ in 0..(fixed_buttons + agent_buttons + custom_buttons) {
            Self::add_action_cluster_width(&mut width, &mut count, ACTION_BUTTON_SIZE);
        }
        width
    }

    /// Render a small icon button for the tab bar end section.
    fn action_button(
        id: &'static str,
        icon_path: &'static str,
        handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        Self::command_button(
            SharedString::from(id),
            SharedString::from(icon_path),
            tab_colors().muted,
            false,
            handler,
        )
    }

    /// Render the plus button that sits directly after the last tab chip.
    fn new_tab_button(cx: &mut Context<Self>) -> impl IntoElement {
        let ui = tab_colors();
        div()
            .id("pane-btn-new-tab")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(ADD_TAB_BUTTON_SIZE))
            .h(px(ADD_TAB_BUTTON_SIZE))
            .rounded(px(TAB_RADIUS))
            .cursor_pointer()
            .hover(|s| s.bg(crate::app::constants::sidebar_tab_hover_background()))
            .on_click(cx.listener(|_this, _e: &ClickEvent, _window, cx| {
                // See `PaneEvent::NewTerminalTab`: spawning in the app gives
                // the new terminal the workspace cwd + app-level subscriptions.
                cx.emit(PaneEvent::NewTerminalTab);
                cx.stop_propagation();
            }))
            .child(
                svg()
                    .size(px(ADD_TAB_ICON_SIZE))
                    .flex_none()
                    .path("icons/plus.svg")
                    .text_color(ui.muted),
            )
    }

    /// A 14px tab-bar icon. Monochrome logos render as a `text_color`-tinted
    /// `svg()` mask; multi-color logos render via `img()`, which rasterizes
    /// the SVG (resvg) and keeps every native fill/gradient - a `text_color`
    /// tint has no effect there, so `tint` is ignored when `multicolor`.
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

    /// Render a small icon button with a caller-supplied tint colour. Used
    /// for most built-in agent buttons and for user-defined `custom_buttons`
    /// (muted, matching the other controls). `multicolor` switches the icon
    /// to native-color `img()` rendering (see [`Self::command_icon`]).
    fn command_button(
        id: SharedString,
        icon_path: SharedString,
        tint: Hsla,
        multicolor: bool,
        handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(ACTION_BUTTON_SIZE))
            .h(px(ACTION_BUTTON_SIZE))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|s| s.bg(crate::app::constants::sidebar_tab_hover_background()))
            .on_click(move |e, w, cx| handler(e, w, cx))
            .child(Self::command_icon(icon_path, tint, multicolor))
    }

    /// Close a tab at the given index. Emits `PaneEvent::Remove` if the pane becomes empty.
    pub fn close_tab_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.tabs.len() {
            return;
        }
        let selected_id = self.tabs.get(self.selected_idx).map(TabContent::entity_id);
        let removed_id = self.tabs[idx].entity_id();
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            cx.emit(PaneEvent::Remove);
            return;
        }
        self.restore_selection_after_removal(idx, removed_id, selected_id);
        cx.emit(PaneEvent::TabsChanged);
        cx.notify();
    }

    pub fn index_for_tab_id(&self, tab_id: gpui::EntityId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.entity_id() == tab_id)
    }

    fn restore_selection_after_removal(
        &mut self,
        removed_idx: usize,
        removed_id: gpui::EntityId,
        selected_id: Option<gpui::EntityId>,
    ) {
        if self.tabs.is_empty() {
            self.selected_idx = 0;
            return;
        }
        match selected_id {
            Some(id) if id != removed_id => {
                if let Some(idx) = self.index_for_tab_id(id) {
                    self.selected_idx = idx;
                } else {
                    self.selected_idx = self.selected_idx.min(self.tabs.len() - 1);
                }
            }
            _ => {
                self.selected_idx = removed_idx.min(self.tabs.len() - 1);
            }
        }
    }

    /// Move a tab from one slot to another within this pane (EP-001 US-002).
    ///
    /// Single mutation entry point for same-pane reordering - drag-drop today,
    /// any future keyboard/menu reorder routes through here too. The moved tab
    /// becomes the selected tab so `selected_idx` follows it (per the AC). A
    /// no-op move (origin slot, out of range, or a trailing drop that resolves
    /// to the current last slot) skips `cx.notify()` so there's no flicker.
    ///
    /// `to` is treated as the desired final index; callers pass `tabs.len() - 1`
    /// for "drop on the trailing area". Insert is into the post-removal vec, so
    /// inserting at the (clamped) target index yields the dragged tab's final
    /// position in both forward and backward moves.
    pub fn reorder_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let Some(dest) = reordered_index(from, to, self.tabs.len()) else {
            return;
        };
        let tab = self.tabs.remove(from);
        self.tabs.insert(dest, tab);
        self.selected_idx = dest;
        cx.emit(PaneEvent::TabsChanged);
        cx.notify();
    }

    pub fn reorder_tab_by_id(&mut self, tab_id: gpui::EntityId, to: usize, cx: &mut Context<Self>) {
        if let Some(from) = self.index_for_tab_id(tab_id) {
            self.reorder_tab(from, to, cx);
        }
    }

    /// Remove a tab for a cross-pane move (EP-002 US-004). Unlike
    /// [`Self::close_tab_at`], this does NOT emit `PaneEvent::Remove` when the
    /// pane empties - the move orchestration ([`crate::pane_drag::move_tab_into`])
    /// decides source cleanup so the tree owner reflows exactly once. Clamps
    /// `selected_idx` if it pointed past the removed slot. Returns the tab, or
    /// `None` if the index is out of range.
    pub fn take_tab_for_move(&mut self, idx: usize) -> Option<TabContent> {
        if idx >= self.tabs.len() {
            return None;
        }
        let selected_id = self.tabs.get(self.selected_idx).map(TabContent::entity_id);
        let removed_id = self.tabs[idx].entity_id();
        let tab = self.tabs.remove(idx);
        if !self.tabs.is_empty() {
            self.restore_selection_after_removal(idx, removed_id, selected_id);
        }
        Some(tab)
    }

    pub fn take_tab_for_move_by_id(&mut self, tab_id: gpui::EntityId) -> Option<TabContent> {
        let idx = self.index_for_tab_id(tab_id)?;
        self.take_tab_for_move(idx)
    }

    /// Insert a tab moved in from another pane (EP-002 US-004), making it the
    /// selected, focused tab. Terminal tabs are re-subscribed so
    /// `ChildExited`/`TitleChanged` route to this pane; the source's now-stale
    /// subscription degrades to a no-op (it can't find the moved terminal in
    /// its own `tabs`). `dest_idx` is clamped to `[0, len]` - pass `tabs.len()`
    /// to append.
    pub fn insert_moved_tab(
        &mut self,
        tab: TabContent,
        dest_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let TabContent::Terminal(t) = &tab {
            Self::subscribe_terminal(t, cx);
            Self::apply_terminal_render_config(t, &self.cached_config, cx);
        }
        let at = dest_idx.min(self.tabs.len());
        self.tabs.insert(at, tab);
        self.selected_idx = at;
        self.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Insert a freshly-spawned duplicate terminal (EP-003 US-010) at
    /// `dest_idx`, making it the selected tab. Like [`Self::insert_moved_tab`]
    /// but without a `Window`: the duplicate is created in `PaneFlowApp`'s
    /// `DuplicateTabInto` subscription handler, which has no `Window`, so focus
    /// is applied by the app via `pending_pane_focus` on the next render.
    /// `dest_idx` is clamped to `[0, len]`. Pane-level subscription is wired so
    /// `ChildExited`/`TitleChanged` route here; the app-level CWD/port
    /// subscription is wired by the caller (the handler), mirroring
    /// `create_pane`.
    pub fn insert_duplicated_tab(
        &mut self,
        tab: TabContent,
        dest_idx: usize,
        cx: &mut Context<Self>,
    ) {
        if let TabContent::Terminal(t) = &tab {
            Self::subscribe_terminal(t, cx);
            Self::apply_terminal_render_config(t, &self.cached_config, cx);
        }
        let at = dest_idx.min(self.tabs.len());
        self.tabs.insert(at, tab);
        self.selected_idx = at;
        cx.notify();
    }

    /// Shared `on_drag_move` body for both [`TabDrag`] and [`SessionDrag`]:
    /// resolve the cursor (relative to the content `bounds`) to a split edge
    /// and, when it changes, seed the overlay glide and request a repaint. Both
    /// drag types drive the same blue preview, so the geometry lives here once.
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
        self.overlay_pane_size = bounds.size;
        let edge = compute_drop_edge(w, h, x, y, SPLIT_EDGE_BAND);
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

    /// Human-readable label for this pane's active tab, used by the
    /// "Move to pane…" menu (EP-002 US-006) to identify each destination.
    pub fn active_tab_label(&self, cx: &App) -> String {
        self.tabs
            .get(self.selected_idx)
            .map(|t| Self::tab_title(t, cx))
            .unwrap_or_else(|| "Empty".into())
    }

    /// Close the currently selected tab. Returns `true` if the pane is now empty.
    pub fn close_selected_tab(&mut self, cx: &mut Context<Self>) -> bool {
        self.close_tab_at(self.selected_idx, cx);
        self.tabs.is_empty()
    }

    /// Get the currently selected terminal entity, if any. Returns `None`
    /// when the active tab is a markdown viewer or the pane is empty - all
    /// callers must handle the absence (event handlers, workspace ops, IPC,
    /// in-pane action buttons) so a markdown tab never triggers a panic.
    pub fn active_terminal_opt(&self) -> Option<&Entity<TerminalView>> {
        self.tabs
            .get(self.selected_idx)
            .and_then(TabContent::as_terminal)
    }

    // -----------------------------------------------------------------------
    // Tab bar rendering - Zed-style design
    // -----------------------------------------------------------------------

    /// Render a tab's title slot. While that tab is being renamed (US-013) the
    /// slot becomes a focusable inline input capturing keystrokes; otherwise
    /// it's the normal ellipsized title.
    fn render_tab_title(&self, i: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = tab_colors();
        if self.rename.as_ref().map(|r| r.idx) == Some(i) {
            let buffer = self
                .rename
                .as_ref()
                .map(|r| r.buffer.clone())
                .unwrap_or_default();
            div()
                .flex_1()
                .min_w_0()
                .track_focus(&self.rename_focus)
                .bg(ui.overlay)
                .px_1()
                .rounded_sm()
                .text_color(ui.text)
                .text_size(px(12.5))
                .on_key_down(cx.listener(
                    |this, e: &gpui::KeyDownEvent, window: &mut Window, cx| {
                        if this.rename.is_none() {
                            return;
                        }
                        match e.keystroke.key.as_str() {
                            "enter" => this.commit_rename(window, cx),
                            "escape" => {
                                this.rename = None;
                                this.focus_handle(cx).focus(window, cx);
                                cx.notify();
                            }
                            "backspace" => {
                                if let Some(r) = this.rename.as_mut() {
                                    r.buffer.pop();
                                }
                                cx.notify();
                            }
                            _ => {
                                if let Some(ch) = &e.keystroke.key_char
                                    && !ch.is_empty()
                                    && !e.keystroke.modifiers.control
                                    && !e.keystroke.modifiers.platform
                                    && let Some(r) = this.rename.as_mut()
                                {
                                    r.buffer.push_str(ch);
                                    cx.notify();
                                }
                            }
                        }
                    },
                ))
                .child(format!("{buffer}|"))
                .into_any_element()
        } else {
            let full_title = Self::tab_full_title(&self.tabs[i], cx);
            let display_title = Self::tab_title(&self.tabs[i], cx);
            let show_tooltip = full_title != display_title
                || full_title.chars().count() > TAB_TITLE_TOOLTIP_THRESHOLD;
            let mut title = div()
                .id(SharedString::from(format!("pane-tab-title-{i}")))
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(12.5))
                .child(display_title);
            if show_tooltip {
                title = title.tooltip(crate::ui_primitives::text_tooltip(full_title));
            }
            title.into_any_element()
        }
    }

    /// Commit the in-progress inline rename (US-013): a non-empty buffer sets
    /// the tab's terminal custom name; an empty one clears it (reverting to the
    /// auto-derived name). Emits `SurfaceRenamed` so the app persists the
    /// session, then returns focus to the terminal.
    fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.rename.take() else {
            return;
        };
        if let Some(TabContent::Terminal(t)) = self.tabs.get(state.idx).cloned() {
            let trimmed = state.buffer.trim();
            let new_name = (!trimmed.is_empty()).then(|| trimmed.to_string());
            t.update(cx, |view, _cx| {
                view.terminal.custom_name = new_name;
            });
            cx.emit(PaneEvent::SurfaceRenamed);
        }
        self.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn render_tab_bar(
        &self,
        _is_active: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tab_count = self.tabs.len();
        let ui = tab_colors();
        let theme = crate::theme::active_theme();
        // Handle to this pane, captured once for the per-tab drag closures
        // (EP-001): same-pane vs cross-pane is decided by comparing the
        // drag's `source_pane` to this entity. `accent` tints the insertion
        // indicator drawn during a same-pane reorder hover.
        let self_entity = cx.entity();
        let accent = ui.accent;
        // Tab strip uses the terminal background so it melts into the terminal
        // body below it - one clean surface (Arthur). When Windows terminal
        // material is enabled, the strip goes transparent too so the tab bar
        // and terminal body share the same native backdrop.
        let bar_bg = tab_bar_background(
            &theme,
            self.cached_config.windows_terminal_material_enabled(),
        );

        // Outer container: full-width, fixed height, tab_bar background. The
        // chips are shorter than the bar, so center them vertically to float.
        let bar = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(TAB_BAR_HEIGHT))
            .bg(bar_bg);

        // Scrollable tab area (Zed pattern: overflow_x_scroll on inner row)
        let highlight_entity = self_entity.clone();
        let tabs_area = div()
            .id("pane-tabs-area")
            .relative()
            .flex_1()
            .h_full()
            .overflow_x_hidden()
            // EP-002 US-005: while a tab from *another* pane hovers this strip,
            // tint it so the cross-pane drop target is obvious. The per-slot
            // insertion border (US-002) only shows for same-pane drags, so the
            // two indicators never collide; the source pane's own strip shows
            // no pane-level highlight (the guard below).
            .drag_over::<TabDrag>(move |style, drag, _window, _cx| {
                if drag.source_pane != highlight_entity {
                    style.bg(accent.opacity(0.12))
                } else {
                    style
                }
            })
            .on_click(cx.listener(|_this, e: &ClickEvent, _window, cx| {
                if matches!(e, ClickEvent::Mouse(m) if m.down.click_count == 2) {
                    // Routed to PaneFlowApp so the new terminal spawns at the
                    // workspace cwd (the Pane doesn't know it) with app-level
                    // subscriptions wired - see `PaneEvent::NewTerminalTab`.
                    cx.emit(PaneEvent::NewTerminalTab);
                }
            }));

        let mut tabs_row = div()
            .id("pane-tabs-scroll")
            .flex()
            .flex_row()
            .items_center()
            .h_full()
            .gap(px(STRIP_GAP))
            .pl(px(STRIP_PL))
            .overflow_x_scroll();

        for i in 0..tab_count {
            tabs_row = tabs_row.child(self.render_tab(i, ui, cx));
        }
        tabs_row = tabs_row.child(Self::new_tab_button(cx));

        // Trailing drop zone (EP-001 US-002): the leftover strip space after
        // the last tab control. `flex_1` claims whatever width the tabs don't, so a
        // drop here moves the dragged tab to the last slot. When the strip
        // overflows (overflow_x_scroll), this collapses to zero width and is
        // simply not a drop target - which is correct, there is no trailing
        // area to aim at. Lives inside `tabs_row` so it never overlaps a tab,
        // keeping its `on_drop` distinct from the per-tab handlers.
        tabs_row = tabs_row.child(div().id("pane-tabs-trailing").flex_1().h_full().on_drop(
            cx.listener(move |this, drag: &TabDrag, window, cx| {
                if crate::pane_drag::duplicate_modifier_held(window) {
                    // US-010: modifier held, duplicate at the dragged tab's CWD
                    // into this pane's last slot; the original stays put.
                    cx.emit(PaneEvent::DuplicateTabInto {
                        source_pane: drag.source_pane.clone(),
                        source_tab_id: drag.source_tab_id,
                        dest_idx: this.tabs.len(),
                    });
                } else if drag.source_pane == cx.entity() {
                    // Same pane: reorder to the last slot (EP-001 US-002). Use
                    // the live count so a tab opened/closed since render is
                    // accounted for.
                    this.reorder_tab_by_id(
                        drag.source_tab_id,
                        this.tabs.len().saturating_sub(1),
                        cx,
                    );
                } else {
                    // Cross-pane: append the migrated terminal after the last
                    // tab of this pane (EP-002 US-004).
                    let dest_idx = this.tabs.len();
                    crate::pane_drag::move_tab_into(
                        this,
                        cx,
                        &drag.source_pane,
                        drag.source_tab_id,
                        dest_idx,
                        window,
                    );
                }
            }),
        ));

        let tabs_area = tabs_area.child(tabs_row);

        bar.child(tabs_area).child(self.render_end_section(cx))
    }

    /// Render a single tab chip (US-051: code-motion out of `render_tab_bar`).
    /// The chip skin matches the Agents bottom-panel tabs: a rounded, translucent
    /// pill that lifts when active and washes in on hover. The palette-derived
    /// `accent` and the pane handle are recomputed here so the loop call site
    /// stays a one-liner.
    fn render_tab(
        &self,
        i: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let accent = ui.accent;
        let self_entity = cx.entity();
        let is_selected = i == self.selected_idx;
        let tab_idx = i;
        // US-020: stable identity for the close button's click closure, so
        // it survives a vec mutation between render and click.
        let tab_id = self.tabs[i].entity_id();
        // Chip skin shared with the Agents bottom-panel tabs: the active chip
        // lifts on a whisper of the text color; inactive chips are bare and only
        // wash in on hover. Foreground tracks the same active/idle split.
        let (chip_bg, chip_fg) = if is_selected {
            (with_alpha(ui.text, 0.09), ui.text)
        } else {
            (with_alpha(ui.text, 0.0), ui.muted)
        };
        let chip_hover = with_alpha(ui.text, if is_selected { 0.09 } else { 0.05 });
        let close_base_bg = with_alpha(ui.text, 0.16);
        let close_hover_bg = with_alpha(ui.text, 0.92);
        let close_hover_fg = ui.base;

        // US-018 (orchestration-v2): an agent waiting for input in this tab
        // shows the attention-colored dot, so a hidden waiting tab stays
        // discoverable.
        // EP-004 US-010: an Errored agent (binary exited non-zero) wins over
        // waiting - a crash is the most salient state and must never hide
        // behind a waiting dot. Dedicated `agent_error` slot, distinct from
        // the attention orange (a session is either waiting OR errored, but
        // two terminals of the same tab strip can show one of each).
        let has_attention = self
            .tabs
            .get(i)
            .and_then(|t| t.as_terminal())
            .is_some_and(|t| self.attention.contains_key(&t.entity_id()));
        let has_errored = self
            .tabs
            .get(i)
            .and_then(|t| t.as_terminal())
            .is_some_and(|t| self.errored.contains(&t.entity_id()));
        let status_dot = (has_errored || has_attention).then(|| {
            div()
                .flex_none()
                .w(px(6.0))
                .h(px(6.0))
                .ml_1()
                .rounded_full()
                .bg(if has_errored {
                    ui.agent_error
                } else {
                    ui.vc_conflict
                })
                .into_any_element()
        });

        // EP-001 US-003 (cli-cockpit): queued-prompt chip - tab-anatomy slot
        // ranked just below the state dot. Zero-size placeholder otherwise so
        // tab layout/truncation is unaffected (same convention as status_dot).
        let has_pending = self
            .tabs
            .get(i)
            .and_then(|t| t.as_terminal())
            .is_some_and(|t| self.pending_prefill.contains(&t.entity_id()));
        let pending_chip = has_pending.then(|| {
            div()
                .flex_none()
                .ml_1()
                .px(px(4.))
                .rounded(px(3.))
                .bg(ui.subtle)
                .text_size(px(9.))
                .text_color(ui.muted)
                .child("1 queued")
                .into_any_element()
        });

        // EP-005 US-013 + EP-006 US-018 - identity pill and the transient
        // fleet-match badge, governed by the FR-11 tab
        // anatomy: at most 2 adornments per tab, in priority order state
        // dot > queued chip > identity pill > match badge.
        // The dot and chip claim their slots above; the pill degrades to
        // its icon alone ("point coloré") when it shares the tab with
        // another adornment; and the match badge - lowest priority,
        // "s'efface en premier" - takes the last slot if any.
        let (agent_pill, match_badge) = {
            let term_meta = self.tabs.get(i).and_then(|t| t.as_terminal()).map(|t| {
                let r = t.read(cx);
                (
                    r.terminal.detected_agent,
                    r.terminal.agent_confirmed,
                    self.search_hits.get(&t.entity_id()).copied(),
                )
            });
            let mut slots_used: u8 = u8::from(has_errored || has_attention) + u8::from(has_pending);
            let mut pill = None;
            let mut hits_badge = None;
            if let Some((agent, confirmed, hits)) = term_meta {
                if let Some(agent) = agent
                    && slots_used < 2
                {
                    let compact = slots_used == 1;
                    pill = Some(Self::render_agent_pill(i, agent, confirmed, compact, ui));
                    slots_used += 1;
                }
                if slots_used < 2
                    && let Some(count) = hits.filter(|c| *c > 0)
                {
                    hits_badge = Some(
                        div()
                            .flex_none()
                            .ml_1()
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

        let tab_hover_group = SharedString::from(format!("pane-tab-hover-{i}"));
        let mut tab = div()
            .id(SharedString::from(format!("pane-tab-{i}")))
            .group(tab_hover_group.clone())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .h(px(TAB_HEIGHT))
            .w(px(TAB_WIDTH))
            .flex_shrink_0()
            // Clip at the fixed chip width; the title slot inside owns
            // ellipsis and keeps a stable right padding.
            .overflow_x_hidden()
            .rounded(px(TAB_RADIUS))
            .cursor_pointer()
            .text_size(px(12.5))
            .text_color(chip_fg)
            .bg(chip_bg)
            .hover(move |d| d.bg(chip_hover));

        // EP-001 drag wiring. GPUI's managed drag applies its own movement
        // threshold before firing `on_drag`, so a plain click (select) and
        // a double-click (rename) on the inner `content` div are unaffected.
        // Title/icon are snapshotted into the payload so the floating ghost
        // renders without re-reading the entity.
        {
            let drag_title: SharedString = Self::tab_title(&self.tabs[i], cx).into();
            let drag_icon: SharedString = Self::tab_icon(&self.tabs[i]).into();
            let pane_entity = self_entity.clone();
            tab = tab
                .on_drag(
                    TabDrag {
                        source_pane: pane_entity.clone(),
                        source_idx: tab_idx,
                        source_tab_id: tab_id,
                        title: drag_title.clone(),
                        icon: drag_icon.clone(),
                    },
                    |drag, _offset, _window, cx| {
                        cx.new(|_| TabDragPreview {
                            title: drag.title.clone(),
                            icon: drag.icon.clone(),
                        })
                    },
                )
                // Insertion indicator: 2px border on the side the tab will
                // land. Same-pane only - a cross-pane hover shows nothing
                // in the strip (EP-002 adds the pane-level highlight); the
                // drag's own origin slot shows nothing either.
                .drag_over::<TabDrag>(move |style, drag, _window, cx| {
                    if drag.source_pane != pane_entity {
                        return style;
                    }
                    let pane = pane_entity.read(cx);
                    let source_idx = pane
                        .index_for_tab_id(drag.source_tab_id)
                        .unwrap_or(drag.source_idx);
                    let target_idx = pane.index_for_tab_id(tab_id).unwrap_or(tab_idx);
                    match insertion_side(source_idx, target_idx) {
                        Some(InsertSide::Left) => style.border_l_2().border_color(accent),
                        Some(InsertSide::Right) => style.border_r_2().border_color(accent),
                        None => style,
                    }
                })
                .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                    if crate::pane_drag::duplicate_modifier_held(window) {
                        // US-010: modifier held → spawn a fresh terminal at
                        // the dragged tab's CWD into this pane at the dropped
                        // slot; the original stays put. Routed to PaneFlowApp
                        // (it wires the app-level CWD/port subscription).
                        cx.emit(PaneEvent::DuplicateTabInto {
                            source_pane: drag.source_pane.clone(),
                            source_tab_id: drag.source_tab_id,
                            dest_idx: tab_idx,
                        });
                    } else if drag.source_pane == cx.entity() {
                        // Same pane: reorder in place (EP-001 US-002).
                        let dest_idx = this.index_for_tab_id(tab_id).unwrap_or(tab_idx);
                        this.reorder_tab_by_id(drag.source_tab_id, dest_idx, cx);
                    } else {
                        // Cross-pane: migrate the terminal into this pane at
                        // the dropped slot, preserving its PTY (EP-002 US-004).
                        crate::pane_drag::move_tab_into(
                            this,
                            cx,
                            &drag.source_pane,
                            drag.source_tab_id,
                            tab_idx,
                            window,
                        );
                    }
                }))
                // Right-click opens the "Move to pane…" menu (EP-002 US-006,
                // the WCAG 2.5.7 non-drag alternative). The pane emits its
                // index + anchor; `PaneFlowApp` resolves the sibling panes
                // and paints the menu at the app layer.
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |_this, e: &MouseDownEvent, _window, cx| {
                        cx.emit(PaneEvent::OpenTabMenu {
                            tab_id,
                            position: e.position,
                        });
                        cx.stop_propagation();
                    }),
                );
        }

        // Leading slot swaps the normal tab icon for the close affordance on
        // tab hover. Keeping both in the same fixed box avoids fragile overlay
        // anchoring and keeps the label start stable.
        let close_hover_group = SharedString::from(format!("pane-tab-close-hover-{i}"));
        let close_btn = div()
            .id(SharedString::from(format!("pane-tab-close-{i}")))
            .group(close_hover_group.clone())
            .absolute()
            .left_0()
            .top_0()
            .size(px(LEADING_SLOT_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(close_base_bg)
            .opacity(0.)
            .group_hover(tab_hover_group.clone(), |s| s.opacity(1.))
            .cursor_pointer()
            .shadow_lg()
            .hover(move |d| d.bg(close_hover_bg))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _window, cx| {
                // US-020: resolve the live index by identity, not by the
                // stale render-time `tab_idx`. A `ChildExited` on another
                // terminal (pane.rs:348) can shift the vec between render
                // and this click; closing by position would silently close
                // the neighbour that slid into this slot (data loss).
                if let Some(idx) = this.tabs.iter().position(|t| t.entity_id() == tab_id) {
                    this.close_tab_at(idx, cx);
                }
                cx.stop_propagation();
            }))
            .child(
                svg()
                    .size(px(9.))
                    .flex_none()
                    .path("icons/close.svg")
                    .text_color(ui.text)
                    .group_hover(close_hover_group, move |s| s.text_color(close_hover_fg)),
            );

        // Inner content row: [icon] [label] [adornments] [close]. The icon
        // (terminal vs markdown vs diff) is a bare 13px glyph tinted to the
        // chip foreground, matching the Agents bottom-panel tab.
        let icon_path = Self::tab_icon(&self.tabs[i]);
        let leading_slot = div()
            .relative()
            .flex_none()
            .size(px(LEADING_SLOT_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .size(px(13.))
                    .flex_none()
                    .path(icon_path)
                    .text_color(chip_fg)
                    .group_hover(tab_hover_group.clone(), |s| s.opacity(0.)),
            )
            .child(close_btn);
        let content = div()
            .id(SharedString::from(format!("pane-tab-content-{i}")))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(TAB_GAP))
            .h_full()
            .pl(px(TAB_PL))
            .pr(px(TAB_PR))
            // Keep the chip content start-aligned: no full-width stretch, while
            // still allowing the title slot to shrink and ellipsize under the
            // tab's max width.
            .min_w_0()
            .w_full()
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                if tab_idx >= this.tabs.len() {
                    cx.stop_propagation();
                    return;
                }
                let is_double = matches!(e, ClickEvent::Mouse(m) if m.down.click_count == 2);
                if is_double {
                    // US-013: double-click a terminal tab to rename it.
                    if let Some(TabContent::Terminal(t)) = this.tabs.get(tab_idx) {
                        let buffer = t.read(cx).terminal.custom_name.clone().unwrap_or_default();
                        this.rename = Some(TabRename {
                            idx: tab_idx,
                            buffer,
                        });
                        this.rename_focus.focus(window, cx);
                    }
                } else {
                    this.selected_idx = tab_idx;
                    this.focus_handle(cx).focus(window, cx);
                }
                cx.notify();
                cx.stop_propagation();
            }))
            .children(status_dot)
            .children(pending_chip)
            .child(leading_slot)
            .child(self.render_tab_title(i, cx))
            .children(agent_pill)
            .children(match_badge);

        tab.child(content).into_any_element()
    }

    /// EP-005 US-013: compact agent identity pill for one tab. PID-sourced
    /// (never the OSC title); `compact` renders the icon alone (the FR-11
    /// "point coloré" degradation); an unconfirmed (session-restored,
    /// pre-first-scan) pill renders at 0.6 opacity with a "last known"
    /// tooltip.
    fn render_agent_pill(
        tab_idx: usize,
        agent: crate::agent_launcher::TerminalAgent,
        confirmed: bool,
        compact: bool,
        ui: crate::theme::UiColors,
    ) -> gpui::AnyElement {
        // Multi-color brand logos need `img()` (resvg keeps every fill);
        // monochrome logos are `svg()` masks tinted with the brand accent
        // or the theme text color - same split as the tab-bar launchers.
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
            .id(SharedString::from(format!("tab-agent-pill-{tab_idx}")))
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(px(3.))
            .ml_1()
            .px(px(4.))
            .h(px(14.))
            .rounded(px(7.))
            .bg(ui.subtle)
            .child(icon);
        if !compact {
            // Short name: first word of the display name ("Claude Code" →
            // "Claude") keeps the pill compact under tab truncation.
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
        pill.tooltip(move |_w, cx| {
            let label = tooltip_label.clone();
            cx.new(|_| crate::app::sidebar::SidebarTooltip { label })
                .into()
        })
        .into_any_element()
    }

    /// Trailing action-button cluster of the tab bar (US-051: code-motion out
    /// of `render_tab_bar`). Zoom badge + surface-ref / split / files
    /// / sessions buttons, the built-in agent launchers, and the per-workspace
    /// custom buttons. Self-contained - recomputes the palette it needs.
    fn render_end_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = tab_colors();
        // End section: action buttons. No separator rules: the chip strip melts
        // into the terminal body, so the action cluster floats on the same
        // surface as the tabs.
        let mut end_section = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .h_full()
            .px(px(SECTION_PX))
            .gap(px(0.));

        let actions_collapsed = self.tab_bar_actions_collapsed;
        let animation_epoch = self.tab_bar_actions_animation_epoch;
        let toggle_icon_base = svg()
            .size(px(14.))
            .flex_none()
            .path("icons/chevron-right.svg")
            .text_color(ui.muted);
        let toggle_icon = if animation_epoch == 0 {
            if actions_collapsed {
                toggle_icon_base
                    .with_transformation(Transformation::rotate(percentage(0.5)))
                    .into_any_element()
            } else {
                toggle_icon_base.into_any_element()
            }
        } else {
            toggle_icon_base
                .with_animation(
                    SharedString::from(format!("pane-actions-chevron-{animation_epoch}")),
                    Animation::new(Duration::from_millis(ACTION_CLUSTER_ANIMATION_MS))
                        .with_easing(ease_out_quint()),
                    move |icon, delta| {
                        let rotation = if actions_collapsed {
                            0.5 * delta
                        } else {
                            0.5 * (1.0 - delta)
                        };
                        icon.with_transformation(Transformation::rotate(percentage(rotation)))
                    },
                )
                .into_any_element()
        };
        end_section = end_section.child(
            div()
                .id("pane-btn-toggle-actions")
                .flex()
                .items_center()
                .justify_center()
                .w(px(ACTION_BUTTON_SIZE))
                .h(px(ACTION_BUTTON_SIZE))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(crate::app::constants::sidebar_tab_hover_background()))
                .on_click(cx.listener(|this, _e: &ClickEvent, _window, cx| {
                    this.tab_bar_actions_collapsed = !this.tab_bar_actions_collapsed;
                    this.tab_bar_actions_animation_epoch =
                        this.tab_bar_actions_animation_epoch.saturating_add(1);
                    cx.notify();
                    cx.stop_propagation();
                }))
                .child(toggle_icon),
        );

        let config = &self.cached_config;
        let visible_agents = crate::agent_launcher::TerminalAgent::visible(config);
        let show_sessions_button =
            !crate::agent_sessions::enabled_session_agents_from_config(config).is_empty();
        let fixed_button_count = 4 + usize::from(show_sessions_button);
        let action_cluster_width = Self::tab_bar_action_cluster_width(
            self.zoomed,
            fixed_button_count,
            visible_agents.len(),
            self.custom_buttons.len(),
        );

        let mut action_cluster = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .h_full()
            .w(px(action_cluster_width))
            .gap(px(TAB_GAP));

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
            // Copy this surface's reference (its human-readable name) so it
            // can be pasted into an AI agent ("read the logs in cargo-run").
            // US-010: fallback affordance for when semantic disambiguation by
            // the agent isn't enough. Emits the surface_id; the app resolves
            // the disambiguated name so the copied value matches `list_panes`.
            .child(Self::action_button(
                "pane-btn-copy-ref",
                "icons/link.svg",
                cx.listener(|this, _, _window, cx| {
                    let Some(terminal) = this.active_terminal_opt() else {
                        return;
                    };
                    let surface_id = terminal.entity_id().as_u64();
                    cx.emit(PaneEvent::CopySurfaceRef(surface_id));
                }),
            ))
            // Split vertical (panes side by side)
            .child(Self::action_button(
                "pane-btn-split-v",
                "icons/split_vertical.svg",
                cx.listener(|_this, _, _window, cx| {
                    cx.emit(PaneEvent::Split(crate::layout::SplitDirection::Vertical));
                }),
            ))
            // Split horizontal (panes top/bottom)
            .child(Self::action_button(
                "pane-btn-split-h",
                "icons/split_horizontal.svg",
                cx.listener(|_this, _, _window, cx| {
                    cx.emit(PaneEvent::Split(crate::layout::SplitDirection::Horizontal));
                }),
            ))
            // Toggle the docked Files sidebar (PRD files-tree EP-001): a tree
            // of the active workspace's folder, replacing the former native
            // markdown picker. Markdown rows there are click-to-open into the
            // active pane (and drag-to-pane in EP-003). The Cmd/Ctrl-click `.md`
            // hyperlink path (`TerminalEvent::OpenMarkdownPath`) is untouched.
            .child(Self::action_button(
                "pane-btn-files",
                "icons/folder.svg",
                cx.listener(|_this, _e: &ClickEvent, _window, cx| {
                    cx.emit(PaneEvent::ToggleFilesSidebar);
                    cx.stop_propagation();
                }),
            ))
            // Agent session history for the active terminal's cwd. The cwd
            // lookup + filesystem scan happens in
            // `PaneFlowApp::handle_pane_event`; this button just toggles the
            // docked sidebar.
            //
            // Hidden when the user has toggled off every AI-agent button in
            // Settings → AI Agent: with no agent visible the sidebar would open
            // empty, so the icon itself is suppressed for symmetry with the
            // launcher buttons below.
            .when(show_sessions_button, |s| {
                s.child(Self::action_button(
                    "pane-btn-claude-sessions",
                    "icons/sessions.svg",
                    cx.listener(|_this, _e: &ClickEvent, _window, cx| {
                        cx.emit(PaneEvent::ToggleAgentSessions);
                        cx.stop_propagation();
                    }),
                ))
            });

        // Built-in agent launcher buttons (the 15 CLI coding agents).
        // `TerminalAgent::visible` applies the per-agent `*_button_visible`
        // gate and is the same source of truth the Agents-view picker iterates.
        // US-015: read the cached config (no per-frame `load_config()`); the
        // click handler reads `this.cached_config` live so the Claude bypass
        // toggle still takes effect on the next click (the cache is refreshed
        // by the ConfigWatcher propagation).
        for agent in visible_agents {
            let tint: Hsla = match agent.accent() {
                Some(c) => rgb(c).into(),
                None => tab_colors().text,
            };
            action_cluster = action_cluster.child(Self::command_button(
                SharedString::from(format!("pane-btn-{}", agent.tag())),
                SharedString::from(agent.icon_path()),
                tint,
                agent.icon_multicolor(),
                cx.listener(move |this, _, _window, cx| {
                    let Some(terminal) = this.active_terminal_opt() else {
                        return;
                    };
                    // US-015: read the bypass field from the pane's cache (kept
                    // fresh by ConfigWatcher propagation) instead of a disk read.
                    let cmd = agent.launch_command(&this.cached_config);
                    terminal.read(cx).send_command(&cmd);
                }),
            ));
        }

        // User-defined command buttons (persisted per workspace).
        for btn in &self.custom_buttons {
            let command = btn.command.clone();
            let id = SharedString::from(format!("pane-btn-custom-{}", btn.id));
            let icon = SharedString::from(btn.icon.clone());
            action_cluster = action_cluster.child(Self::command_button(
                id,
                icon,
                ui.muted,
                false,
                cx.listener(move |this, _, _window, cx| {
                    let Some(terminal) = this.active_terminal_opt() else {
                        return;
                    };
                    terminal.read(cx).send_command(&command);
                }),
            ));
        }

        let target_width = if actions_collapsed {
            0.0
        } else {
            TAB_GAP + action_cluster_width
        };
        let target_opacity = if actions_collapsed { 0.0 } else { 1.0 };
        let action_cluster_shell = div()
            .flex_none()
            .h_full()
            .overflow_x_hidden()
            .child(div().ml(px(TAB_GAP)).h_full().child(action_cluster));
        let action_cluster_element = if animation_epoch == 0 {
            action_cluster_shell
                .w(px(target_width))
                .opacity(target_opacity)
                .into_any_element()
        } else {
            action_cluster_shell
                .with_animation(
                    SharedString::from(format!("pane-actions-cluster-{animation_epoch}")),
                    Animation::new(Duration::from_millis(ACTION_CLUSTER_ANIMATION_MS))
                        .with_easing(ease_out_quint()),
                    move |cluster, delta| {
                        let progress = if actions_collapsed {
                            1.0 - delta
                        } else {
                            delta
                        };
                        cluster
                            .w(px((TAB_GAP + action_cluster_width) * progress))
                            .opacity(progress)
                    },
                )
                .into_any_element()
        };

        end_section = end_section.child(action_cluster_element);

        end_section
    }
}

impl gpui::Focusable for Pane {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.tabs.get(self.selected_idx) {
            Some(TabContent::Terminal(t)) => t.read(cx).focus_handle(cx),
            Some(TabContent::Markdown(m)) => m.read(cx).focus_handle(cx),
            Some(TabContent::Diff(d)) => d.read(cx).focus_handle(cx),
            None => cx.focus_handle(),
        }
    }
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_tab = self.tabs.get(self.selected_idx);
        let terminal_selected = matches!(selected_tab, Some(TabContent::Terminal(_)));
        let body = match selected_tab {
            Some(TabContent::Terminal(t)) => t.clone().into_any_element(),
            Some(TabContent::Markdown(m)) => m.clone().into_any_element(),
            Some(TabContent::Diff(d)) => d.clone().into_any_element(),
            None => div().size_full().into_any_element(),
        };
        let content_background = pane_content_background(
            &crate::theme::active_theme(),
            self.cached_config.windows_terminal_material_enabled(),
            terminal_selected,
        );

        // EP-003 drop-to-split: the content region hosts the drag-move
        // direction probe, the drop commit, and the blue preview overlay.
        // A unique group name (per pane entity) scopes `group_drag_over` so
        // only this pane's overlay reacts while a tab hovers its content.
        let group_name =
            SharedString::from(format!("pane-content-{}", cx.entity().entity_id().as_u64()));
        let accent = tab_colors().accent;

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
        // hit-tests / blocks terminal mouse input - unless a tab is being
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
        let overlay = div()
            .absolute()
            .bg(overlay_blue.opacity(DROP_OVERLAY_BACKGROUND_ALPHA))
            .rounded(px(OVERLAY_RADIUS))
            .border_2()
            .border_color(overlay_blue)
            .invisible()
            .group_drag_over::<TabDrag>(group_name.clone(), |s| s.visible())
            // A session dragged from the sidebar lights up the same overlay.
            .group_drag_over::<SessionDrag>(group_name.clone(), |s| s.visible())
            // A markdown file dragged from the Files sidebar - same overlay.
            .group_drag_over::<MarkdownFileDrag>(group_name.clone(), |s| s.visible())
            // Markdown drop: open the file via `MarkdownView`, split toward the
            // previewed edge (or append as a tab for center). Tree mutation +
            // open live in `PaneFlowApp`, so emit + defer out of this callback
            // (entity re-entrancy, mirrors the session drop).
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
            // split toward the previewed edge (or append as a tab for center).
            // Tree mutation + spawn live in `PaneFlowApp`, so emit and defer out
            // of this callback (entity re-entrancy, Risk #1).
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
            // US-009 / US-010: commit. `take()` also resets the preview state.
            .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                let edge = this.drag_split_direction.take();
                // Duplicate when the per-OS modifier is held (US-010); Shift is
                // deliberately never used (terminal selection).
                let duplicate = crate::pane_drag::duplicate_modifier_held(window);
                match edge {
                    Some(edge) => {
                        // Tree mutation lives in `PaneFlowApp` (owner of the
                        // LayoutTree); emitting defers it out of this drop
                        // callback, avoiding entity re-entrancy.
                        cx.emit(PaneEvent::DropSplit {
                            edge,
                            source_pane: drag.source_pane.clone(),
                            source_tab_id: drag.source_tab_id,
                            duplicate,
                        });
                    }
                    None if duplicate => {
                        // Center band + modifier: duplicate the dragged tab's
                        // CWD into this pane as a new tab (US-010). Works even
                        // for a same-pane drop (spawns a sibling shell).
                        cx.emit(PaneEvent::DuplicateTabInto {
                            source_pane: drag.source_pane.clone(),
                            source_tab_id: drag.source_tab_id,
                            dest_idx: this.tabs.len(),
                        });
                    }
                    None => {
                        // Center band: move the tab into this pane (US-004
                        // path). A same-pane center drop is a no-op.
                        if drag.source_pane != cx.entity() {
                            let dest_idx = this.tabs.len();
                            crate::pane_drag::move_tab_into(
                                this,
                                cx,
                                &drag.source_pane,
                                drag.source_tab_id,
                                dest_idx,
                                window,
                            );
                        }
                    }
                }
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
            .flex_1()
            .size_full()
            .overflow_hidden()
            .bg(content_background)
            // US-007: map the cursor within the content bounds to a split edge.
            // Stays on `content` (full pane) - the overlay shrinks to a half
            // when `dir = Some(edge)`, so probing there would miss the cursor
            // moving back toward the center band. `content` keeps its hitbox via
            // `.group(group_name)`.
            .on_drag_move::<TabDrag>(cx.listener(
                |this, e: &DragMoveEvent<TabDrag>, _window, cx| {
                    this.apply_drag_edge(e.bounds, e.event.position, cx);
                },
            ))
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
            .child(body)
            .child(overlay);

        // The blue active-pane focus ring is removed (Arthur): no border tint on
        // focus. The 1px border is still reserved so the US-018 attention glow
        // can paint without reflow.
        //
        // US-018 (orchestration-v2): a pane whose agent waits for input glows
        // with the attention color - amplify the waiting pane, never degrade
        // the others. This stays: it signals "agent needs you", not focus.
        let is_active = self.focus_handle(cx).is_focused(window);
        let has_attention = !self.attention.is_empty();
        let attention_color = tab_colors().vc_conflict;
        let peek = self.render_peek_overlay(cx);
        let composer = self.render_composer_overlay(cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .border_1()
            .border_color(if has_attention {
                attention_color.opacity(0.7)
            } else {
                accent.opacity(0.)
            })
            .child(self.render_tab_bar(is_active, window, cx))
            .child(content)
            .children(peek)
            // EP-001 US-002: broadcast-group stripe - a DISTINCT left-edge
            // element; the pane border slot above stays the attention glow's
            // (Files NOT to Modify). Absolutely positioned so it never
            // perturbs the tab/content flex chain.
            .when_some(self.broadcast_stripe, |d, idx| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(3.))
                        .bg(tab_colors().group_color(idx)),
                )
            })
            .children(composer)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_TAB_TITLE_LEN, pane_content_background, peek_badge_line, tab_bar_background,
        truncate_tab_title,
    };

    #[test]
    fn terminal_material_scopes_tab_bar_to_windows() {
        let theme = crate::theme::one_dark();

        assert_eq!(tab_bar_background(&theme, false), theme.background);
        if cfg!(target_os = "windows") {
            assert_eq!(tab_bar_background(&theme, true).a, 0.0);
        } else {
            assert_eq!(tab_bar_background(&theme, true), theme.background);
        }
    }

    #[test]
    fn terminal_material_scopes_content_to_windows_terminal_tabs() {
        let theme = crate::theme::one_dark();

        assert_eq!(
            pane_content_background(&theme, true, false),
            theme.background
        );
        assert_eq!(
            pane_content_background(&theme, false, true),
            theme.background
        );

        let material = pane_content_background(&theme, true, true);
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
        assert_eq!(truncate_tab_title("README.md"), "README.md");
        assert_eq!(truncate_tab_title("Terminal"), "Terminal");
    }

    #[test]
    fn exactly_max_chars_is_not_truncated() {
        let s: String = "x".repeat(MAX_TAB_TITLE_LEN);
        assert_eq!(truncate_tab_title(&s), s);
    }

    #[test]
    fn over_max_gets_ellipsis() {
        // 25 chars in -> 24 chars out (23 head + ellipsis).
        let input = "prd-opencode-sessions.mdX";
        let out = truncate_tab_title(input);
        assert_eq!(out.chars().count(), MAX_TAB_TITLE_LEN);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn multibyte_utf8_does_not_panic() {
        // Earlier byte-slice path (`&raw[..23]`) panicked when index 23
        // landed in the middle of an accented or CJK char. The char-based
        // implementation must stay sound.
        let input = "événement-très-très-long-fichier.md"; // many multibyte chars
        let out = truncate_tab_title(input);
        assert_eq!(out.chars().count(), MAX_TAB_TITLE_LEN);
        assert!(out.ends_with('…'));
        let cjk = "プロジェクト・パネフロー・テスト・ドキュメント.md";
        let out = truncate_tab_title(cjk);
        assert_eq!(out.chars().count(), MAX_TAB_TITLE_LEN);
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
    fn tab_bar_action_cluster_width_counts_items_and_gaps() {
        assert_eq!(
            super::Pane::tab_bar_action_cluster_width(false, 2, 1, 0),
            super::ACTION_BUTTON_SIZE * 3.0 + super::TAB_GAP * 2.0
        );
        assert_eq!(
            super::Pane::tab_bar_action_cluster_width(true, 1, 0, 0),
            super::ZOOM_BADGE_WIDTH + super::TAB_GAP + super::ACTION_BUTTON_SIZE
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
