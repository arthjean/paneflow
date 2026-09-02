//! Sidebar rendering for `PaneFlowApp`: workspace rows, action buttons,
//! notification dropdown, and the context-menu row helpers (in the
//! [`context_menu`] submodule).
//!
//! Extracted from `main.rs` per US-025 of the src-app refactor PRD - pure
//! code-motion, behaviour unchanged. Toast utilities and sidebar-adjacent
//! types (`WorkspaceContextMenu`, `WorkspaceDrag`, `WorkspaceDragPreview`)
//! remain in `main.rs` because they cross module boundaries.

pub(crate) mod context_menu;
pub(crate) mod customize_menu;

use crate::ui_primitives::TooltipDelayExt;
use gpui::{
    Animation, AnimationExt, AnyElement, AppContext, ClickEvent, Context, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render,
    SharedString, Styled, Window, div, prelude::*, px, rgb, svg,
};

use crate::{
    PaneFlowApp, SIDEBAR_WIDTH, TabContextMenu, TabDrag, WorkspaceContextMenu, WorkspaceDrag,
    WorkspaceDragPreview, ai_types,
    app::pane_palette::PalettePlacement,
    pane_drag::PaneDrag,
    ui_primitives::{ROW_RADIUS, squircle, squircle_skin},
    workspace::{Tab, Workspace},
};

/// Memoized sibling-worktree ordering. Group labels stay hidden, but sibling
/// worktrees remain contiguous as before the visual redesign.
#[derive(Default)]
pub(crate) struct SidebarOrderCache {
    signature: Option<u64>,
    order: Vec<usize>,
}

/// Debug-only render budget guard for the CLI sidebar. Mirrors the Agents
/// sidebar canary so projection or card regressions show up during profiling
/// without adding user-facing log noise.
struct SidebarRenderTimeCanary {
    start: std::time::Instant,
    workspace_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarAgentState {
    NeedsInput,
    Errored,
    Stalled,
    Finished,
    Thinking,
}

/// One row of the rail, in render order: a workspace folder row, or one of
/// its tab rows. The gaps between these rows are what a sidebar drop actually
/// aims at, so the same flattened plan drives both the rows and the dividers
/// between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarRow {
    Folder(usize),
    Tab(usize, usize),
}

/// An insertion point of the rail: the gap between two rows, or either end of
/// the list. Rendered as its own divider element sitting *between* the cards
/// rather than as a border painted on one of them - a card that lights up says
/// "into this one", which no sidebar drop ever means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SidebarDropSlot {
    /// `(workspace index, tab index)` a dropped tab or pane lands at. `None`
    /// for the gap above the very first row: it precedes every workspace, so
    /// it belongs to none of them.
    tab: Option<(usize, usize)>,
    /// Insertion index for a dropped folder, set only on the gaps that
    /// actually separate two folders. A gap inside a tab list is not a
    /// workspace boundary and shows no line for a folder drag.
    workspace: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SidebarAgentSummary {
    state: SidebarAgentState,
    count: usize,
}

pub(crate) const SIDEBAR_ROW_MARGIN_X: f32 = 8.0;
pub(crate) const SIDEBAR_ROW_PADDING_X: f32 = 8.0;
pub(crate) const SIDEBAR_ROW_PADDING_Y: f32 = 6.0;
/// Separates a row's title line from its meta line (a tab's checkout).
const SIDEBAR_ROW_GAP: f32 = 4.0;
/// Height of a row's title line, and with it the height of a single-line row:
/// `SIDEBAR_ROW_LINE_HEIGHT + 2 * SIDEBAR_ROW_PADDING_Y`.
///
/// Set explicitly because the default line height is a multiple of the font
/// size, so the rail's row height moved with the font metrics - it measured 23
/// px here and made every row 35 px tall. Pinning it keeps the row at 30 px,
/// the height the rail is designed against, whatever font resolves.
pub(crate) const SIDEBAR_ROW_LINE_HEIGHT: f32 = 18.0;
const SIDEBAR_TITLE_ROW_GAP: f32 = 8.0;
const SIDEBAR_AGENT_STATUS_SLOT_WIDTH: f32 = 48.0;
const SIDEBAR_AGENT_ICON_SLOT_WIDTH: f32 = 20.0;
/// Group the rail's file-manager drop placeholder reads its visibility from.
const SIDEBAR_DROP_GROUP: &str = "sidebar-drop-zone";
/// Gap between the drop placeholder and the rail's edges, so the rounded box
/// floats inside the sidebar instead of tracing its border.
const SIDEBAR_DROP_PLACEHOLDER_MARGIN: f32 = 6.0;
const SIDEBAR_DROP_PLACEHOLDER_RADIUS: f32 = 8.0;
/// Fill / hairline alpha of the drop placeholder, matching the pane-swap
/// placeholder (`pane.rs`) so both neutral drop targets read the same.
const SIDEBAR_DROP_PLACEHOLDER_FILL_ALPHA: f32 = 0.10;
const SIDEBAR_DROP_PLACEHOLDER_BORDER_ALPHA: f32 = 0.22;
/// Side of one square button of a row's hover action cluster.
const SIDEBAR_ACTION_BUTTON_SIZE: f32 = 20.0;
/// Gap between two buttons of the same cluster.
const SIDEBAR_ACTION_BUTTON_GAP: f32 = 4.0;
/// US-010: room the hover action cluster needs in the right corner of a
/// workspace row, plus the gap that keeps it off the trailing agent badge. The
/// folder cluster carries a single button - "new pane". Closing a workspace is
/// deliberately context-menu only (right click): the hover `x` sat one stray
/// click away from dropping every tab of the folder.
///
/// It is trailing padding on the title row, held at all times rather than
/// opened on hover: reserving it only under the pointer left whatever could
/// not shrink sitting under the button.
const SIDEBAR_ACTION_LANE_WIDTH: f32 = SIDEBAR_TITLE_ROW_GAP + SIDEBAR_ACTION_BUTTON_SIZE;
/// Vertical space between two rows of the rail. The divider element *is* that
/// space (the list itself sets no gap), so inserting the drop slots costs no
/// layout: nothing moves when a drag starts.
const SIDEBAR_ROW_SPACING: f32 = 4.0;
/// Thickness of the insertion line. Two pixels, not one: a hairline reads as
/// an artifact of the row above it.
const SIDEBAR_DROP_LINE_PX: f32 = 2.0;
/// How far above and below its gap a divider's invisible drop band reaches -
/// half a single-line row, so the bands of consecutive gaps tile the rail
/// without overlapping. Dropping anywhere over a row therefore aims at the
/// nearest gap, and the line the user sees is the one they get.
const SIDEBAR_DROP_BAND_REACH: f32 = SIDEBAR_ROW_LINE_HEIGHT / 2.0 + SIDEBAR_ROW_PADDING_Y;
const SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH: f32 =
    SIDEBAR_WIDTH - SIDEBAR_ROW_MARGIN_X * 2.0 - SIDEBAR_ROW_PADDING_X * 2.0;
/// US-008: leading affordances of a workspace folder row - the folder icon and
/// the gap before the title. The open/closed folder glyph is the whole
/// disclosure affordance; there is deliberately no chevron next to it.
///
/// A tab row reserves the icon width with an invisible placeholder and nothing
/// more, so every title in the rail - folder and tab alike - starts on the same
/// X. The folder icon is then the only thing that distinguishes a workspace
/// from its tabs.
const SIDEBAR_FOLDER_ICON_WIDTH: f32 = 14.0;
/// US-013: geometry of the per-pane icon cluster carried by a tab row.
///
/// The cap is the visual reference's: past four panes the cluster would eat
/// the title before it reaches the activity badge, so the tail folds into a
/// `+N`. Each slot is fixed and painted whether or not the pane's agent is
/// known yet, so the scan landing later swaps a glyph in place instead of
/// reflowing the row (edge case 15).
const SIDEBAR_TAB_ICON_SIZE: f32 = 16.0;
const SIDEBAR_TAB_ICON_GAP: f32 = 3.0;
const SIDEBAR_TAB_ICON_CAP: usize = 4;
/// One card per pane, not one capsule around the stack: each glyph carries its
/// own rounded fill plus hairline, and the cards slide over each other. The
/// height is the row's line height, so a stack never grows the row; the width
/// and the glyph keep the visual reference's proportions (card slightly taller
/// than wide, glyph about two thirds of the card width).
///
/// The card is a square chip: 24x24 around a 16px glyph, so the padding is
/// exactly 4px on all four sides. Both the difference (24 - 16 = 8) and each
/// side (4) are whole pixels on purpose - an odd gap splits into 4.5px per side
/// and GPUI's rounding then lands the glyph half a pixel off center, which on a
/// mark this small is visible as a lean.
///
/// Centering is left to `items_center` + `justify_center` and nothing else: no
/// per-icon nudge. The glyphs are optically centered *inside their own
/// viewBox*, which is the fix that survives a size change - see `codex.svg`,
/// whose viewBox is widened around the blossom's bounding box so the mark
/// carries the same relative margin as the Lucide-derived icons next to it.
///
/// The card is *taller* than the row's line height (18px). It is laid out
/// absolutely and overhangs into the row's 6px vertical padding rather than
/// pushing the row past 30px, so the height must stay under that 30px total -
/// the row shell clips its overflow, and a card past 30px would paint sliced.
/// `tab_card_fits_inside_a_row` guards both bounds.
///
/// The corner is the rail's own: `ROW_RADIUS`, traced by the same `squircle`
/// primitive the rows use, not GPUI's `rounded()` circular arc. A card sitting
/// inside a row must not answer its container's continuous corner with a
/// different curve. `trace` clamps the radius to half the shorter side, so a
/// 14px corner on a 24px card resolves to the full superellipse - the card is
/// all corner, which is exactly what makes it read as the row's own material.
const SIDEBAR_TAB_CARD_WIDTH: f32 = 24.0;
const SIDEBAR_TAB_CARD_HEIGHT: f32 = 24.0;
const SIDEBAR_TAB_CARD_ICON_SIZE: f32 = 16.0;
/// Stacked-card overlap: past the first slot every card slides back over its
/// predecessor, which is what reads as one cluster rather than a row of loose
/// cards. Children paint in declaration order, so the later pane sits on top -
/// the same direction as the visual reference.
const SIDEBAR_TAB_ICON_OVERLAP: f32 = 11.0;

/// Shared shell of every rail row, folder and tab alike, so a workspace row is
/// exactly as tall as a tab row: same padding, same corner, no minimum height.
/// A tab row only grows past that when it renders a meta line (its checkout),
/// which the `gap` then separates from the title.
fn sidebar_row_shell() -> gpui::Div {
    div()
        .px(px(SIDEBAR_ROW_PADDING_X))
        .py(px(SIDEBAR_ROW_PADDING_Y))
        .flex_none()
        // `relative()` belongs to the shell, not to its callers: the row's
        // fill and its hover action cluster are both absolutely positioned
        // against it.
        .relative()
        .overflow_x_hidden()
        .flex()
        .flex_col()
        .gap(px(SIDEBAR_ROW_GAP))
}

/// The hairline that ties a workspace's tab rows to the folder above them.
///
/// Centered on the folder icon's slot rather than inset from the row's edge:
/// what the line stands under is that glyph, which is also where the tab rows
/// align their own leading slot. Painted before the row, so a selected row's
/// fill covers it instead of the line cutting across the card.
///
/// One segment per row rather than one line for the group: the rail is a flat
/// list of rows - that is what the drop dividers between them address - so
/// there is no element spanning a workspace's tabs to hang a single line on.
/// The rows are contiguous, so the segments read as one line.
fn render_sidebar_indent_guide(ui: crate::theme::UiColors, interrupted: bool) -> Vec<gpui::Div> {
    let color = ui.text.opacity(0.08);
    let left = px(SIDEBAR_ROW_PADDING_X + (SIDEBAR_FOLDER_ICON_WIDTH / 2.).floor());
    let segment = move || div().absolute().left(left).w(px(1.)).bg(color);
    if !interrupted {
        return vec![segment().top_0().bottom_0()];
    }
    // A row whose agent badge sits in that same column breaks the line rather
    // than letting it run under the glyph: the spinner is a moving mark, and a
    // hairline crossing it reads as part of it. The gap is the badge's own
    // slot, so the line resumes exactly where the glyph ends.
    let center = SIDEBAR_ROW_PADDING_Y + SIDEBAR_ROW_LINE_HEIGHT / 2.;
    let radius = SIDEBAR_AGENT_ICON_SLOT_WIDTH / 2.;
    vec![
        segment().top_0().h(px(center - radius)),
        segment().top(px(center + radius)).bottom_0(),
    ]
}

/// A rail row: the shared continuous-corner skin, with `body` on top.
fn squircle_row(
    shell: gpui::Stateful<gpui::Div>,
    group: SharedString,
    resting: Option<gpui::Hsla>,
    hovered: Option<gpui::Hsla>,
    body: impl IntoElement,
) -> gpui::Stateful<gpui::Div> {
    squircle_skin(shell, group, ROW_RADIUS, resting, hovered).child(body)
}

/// US-010: the action cluster a rail row opens under the pointer - absolute,
/// right-aligned, one 20x20 button per action, hidden until `group` is hovered.
///
/// Insets are resolved against this cluster's own parent - the element it is
/// added to - not against the row shell: taffy positions an absolute child
/// relative to its direct parent, it does not walk up to the nearest positioned
/// ancestor the way CSS does. The caller therefore adds it to a box that is
/// already inside the shell's padding, and must not re-apply that padding here.
///
/// The cluster is centered on the title line, not on the row: a row that grew a
/// meta line must not drift its actions down with it. The buttons are 20px on
/// an 18px line, hence the 1px overhang.
///
/// The hover toggle must NOT be `display`. `Div::prepaint` skips its children
/// when the computed style says `display: none` while `Div::paint` paints them,
/// and the two phases can disagree on hover within one frame (the group hitbox
/// is only known after prepaint). Flipping `display` on hover therefore paints
/// never-prepainted children and GPUI panics with "must call prepaint before
/// paint". `visibility` is only consulted in `Interactivity::paint`, so both
/// phases stay consistent - this is Zed's `visible_on_hover` idiom, and it also
/// keeps the hidden cluster from swallowing clicks (mouse listeners are
/// registered after the visibility check).
fn sidebar_hover_actions(group: SharedString) -> gpui::Div {
    div()
        .absolute()
        .top(px(
            (SIDEBAR_ROW_LINE_HEIGHT - SIDEBAR_ACTION_BUTTON_SIZE) / 2.
        ))
        .right(px(0.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(SIDEBAR_ACTION_BUTTON_GAP))
        .invisible()
        .group_hover(group, |style| style.visible())
}

/// One button of a [`sidebar_hover_actions`] cluster. The caller chains its own
/// tooltip and click handler.
///
/// `svg()` is a mask: it paints nothing without its own `text_color`, and the
/// parent's does NOT cascade - the same trap the sidebar header's `+`
/// documents.
fn sidebar_action_button(
    id: SharedString,
    icon: &'static str,
    icon_size: f32,
    ui: crate::theme::UiColors,
) -> gpui::Stateful<gpui::Div> {
    // The row underneath is already at the hover tint when this button is
    // reachable, so the button hovers into the active tint - one step further,
    // or it would be invisible against its own row.
    let active_bg = crate::app::constants::sidebar_tab_active_background();
    div()
        .id(id)
        .flex_none()
        .size(px(SIDEBAR_ACTION_BUTTON_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .text_color(ui.muted)
        .hover(move |style| style.bg(active_bg))
        .child(
            svg()
                .size(px(icon_size))
                .flex_none()
                .path(icon)
                .text_color(ui.muted),
        )
}

impl SidebarAgentSummary {
    fn slot_width(self) -> f32 {
        if self.state == SidebarAgentState::NeedsInput {
            SIDEBAR_AGENT_STATUS_SLOT_WIDTH
        } else if self.count > 1 {
            28.0
        } else {
            SIDEBAR_AGENT_ICON_SLOT_WIDTH
        }
    }

    fn tooltip_state(self) -> String {
        match self.state {
            SidebarAgentState::NeedsInput => {
                agent_status_sentence(self.count, "needs input", "need input")
            }
            SidebarAgentState::Errored => agent_status_sentence(self.count, "errored", "errored"),
            SidebarAgentState::Stalled => agent_status_sentence(self.count, "stalled", "stalled"),
            SidebarAgentState::Thinking => {
                agent_status_sentence(self.count, "thinking", "thinking")
            }
            SidebarAgentState::Finished => {
                "Agent finished · Click workspace or pane to dismiss".to_string()
            }
        }
    }
}

fn agent_status_sentence(count: usize, singular_state: &str, plural_state: &str) -> String {
    if count == 1 {
        format!("1 agent {singular_state}")
    } else {
        format!("{count} agents {plural_state}")
    }
}

impl SidebarRenderTimeCanary {
    fn new(workspace_count: usize) -> Self {
        Self {
            start: std::time::Instant::now(),
            workspace_count,
        }
    }
}

impl Drop for SidebarRenderTimeCanary {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        if elapsed > std::time::Duration::from_millis(16) {
            tracing::debug!(
                target: "paneflow_app::sidebar",
                "render_sidebar exceeded 16ms frame budget: {:.2}ms across {} workspaces",
                elapsed.as_secs_f64() * 1000.0,
                self.workspace_count
            );
        }
    }
}

/// Whether a tab row prints its insertion and deletion counts.
///
/// Two ways to get nothing: the switch is off, or the checkout is clean. The
/// counts answer "how much has changed here", and when the answer is none the
/// question was never asked - a permanent "0" would spend a slot on the rows
/// with the least to say.
fn tab_diffstat_visible(
    show: paneflow_config::schema::SidebarShow,
    stats: &crate::workspace::GitDiffStats,
) -> bool {
    show.diffstat_enabled() && (stats.insertions > 0 || stats.deletions > 0)
}

fn sidebar_agent_summary<'a, I>(sessions: I, completion_unread: bool) -> Option<SidebarAgentSummary>
where
    I: IntoIterator<Item = &'a ai_types::AgentSession>,
{
    let mut counts = [0usize; 4];
    for session in sessions {
        let index = match session.state {
            ai_types::AgentState::WaitingForInput => 0,
            ai_types::AgentState::Errored => 1,
            ai_types::AgentState::Stalled => 2,
            ai_types::AgentState::Thinking => 3,
            ai_types::AgentState::Finished => continue,
        };
        counts[index] += 1;
    }

    let priority = [
        SidebarAgentState::NeedsInput,
        SidebarAgentState::Errored,
        SidebarAgentState::Stalled,
    ];
    for (state, count) in priority.into_iter().zip(counts[..3].iter().copied()) {
        if count > 0 {
            return Some(SidebarAgentSummary { state, count });
        }
    }

    if completion_unread {
        return Some(SidebarAgentSummary {
            state: SidebarAgentState::Finished,
            count: 1,
        });
    }

    (counts[3] > 0).then_some(SidebarAgentSummary {
        state: SidebarAgentState::Thinking,
        count: counts[3],
    })
}

/// US-012: the sessions a workspace *folder* row speaks for.
///
/// Expanded, every session whose `surface_id` resolved is already spoken for
/// by its own tab row, so the folder keeps only the residue: sessions still at
/// `surface_id: None` - old shims, ancestor walks that never landed. That
/// residue is exactly FR-04, an unattributed session belongs to the project
/// and never to an arbitrary tab.
///
/// Collapsed, the tab rows are off screen, so the folder re-aggregates every
/// session again: the fold must hide no state (FR-05).
///
/// The completion notification is deliberately NOT filtered here - it is
/// workspace-scoped and carries no surface, so it stays on the folder row in
/// both states, for the same reason.
fn folder_row_sessions<'a, I>(
    sessions: I,
    expanded: bool,
) -> impl Iterator<Item = &'a ai_types::AgentSession>
where
    I: IntoIterator<Item = &'a ai_types::AgentSession>,
    I::IntoIter: 'a,
{
    sessions
        .into_iter()
        .filter(move |session| !expanded || session.surface_id.is_none())
}

/// US-012: the sessions one tab row speaks for - those whose `surface_id` is a
/// terminal of that tab's pane tree.
///
/// This filter and [`folder_row_sessions`] partition on `surface_id`, so a
/// session that resolves late simply migrates from the folder to its owning
/// tab on the next frame, counted once on either side and never on both.
fn tab_row_sessions<'a, I>(
    sessions: I,
    surfaces: &'a std::collections::HashSet<u64>,
) -> impl Iterator<Item = &'a ai_types::AgentSession>
where
    I: IntoIterator<Item = &'a ai_types::AgentSession>,
    I::IntoIter: 'a,
{
    sessions
        .into_iter()
        .filter(move |session| session.surface_id.is_some_and(|id| surfaces.contains(&id)))
}

/// US-013: how many pane icons a tab row paints, and how many fold into the
/// trailing `+N`.
fn tab_icon_cluster_split(pane_count: usize) -> (usize, usize) {
    let shown = pane_count.min(SIDEBAR_TAB_ICON_CAP);
    (shown, pane_count - shown)
}

/// US-013: what one pane contributes to its tab row's icon cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TabPaneIcon {
    path: &'static str,
    label: &'static str,
}

/// US-013: read a pane's icon straight from `terminal.detected_agent`, the
/// PID-authoritative scan result `apply_pane_scan` already deposited. No scan
/// is started here: a pane whose agent is not known yet keeps the generic
/// surface icon and swaps glyph in place once the 500 ms debounce lands.
fn tab_pane_icon(pane: &crate::pane::Pane, cx: &gpui::App) -> TabPaneIcon {
    let agent = pane
        .surface
        .as_terminal()
        .and_then(|terminal| terminal.read(cx).terminal.detected_agent);
    match agent {
        Some(agent) => TabPaneIcon {
            path: agent.icon_path(),
            label: agent.display_name(),
        },
        None => TabPaneIcon {
            path: pane.surface.kind_icon(),
            label: pane.surface.kind_label(),
        },
    }
}

/// US-009: label of a tab row. An unnamed tab (the default until it is renamed
/// or created from a named preset) falls back to its 1-based position, so the
/// list never shows a blank row.
fn tab_display_title(tab: &Tab, tab_idx: usize) -> String {
    if tab.title().trim().is_empty() {
        format!("Tab {}", tab_idx + 1)
    } else {
        tab.title().to_string()
    }
}

/// Index a remove-then-insert reorder must target so the moved item lands in
/// the gap at `slot`. Both [`crate::workspace::Workspace::reorder_tab`] and
/// `reorder_workspace` remove first, which slides everything after the source
/// up by one - so a move down aims one index lower than the gap it points at.
fn reorder_target(from: usize, slot: usize) -> usize {
    if from < slot { slot - 1 } else { slot }
}

/// The insertion points of a rendered rail: one before every row, one after
/// the last, so `rows.len() + 1` in all.
///
/// A gap inherits its tab target from the row *above* it, which is the only
/// reading that matches what the eye sees: the line above a folder row cannot
/// mean "first tab of that folder" (its tabs render below it), it means "last
/// tab of the workspace the line is under".
fn sidebar_drop_slots(rows: &[SidebarRow], workspace_count: usize) -> Vec<SidebarDropSlot> {
    (0..=rows.len())
        .map(|k| SidebarDropSlot {
            tab: match k.checked_sub(1).map(|above| rows[above]) {
                Some(SidebarRow::Folder(ws)) => Some((ws, 0)),
                Some(SidebarRow::Tab(ws, tab)) => Some((ws, tab + 1)),
                None => None,
            },
            workspace: match rows.get(k) {
                Some(SidebarRow::Folder(ws)) => Some(*ws),
                // Past the last row: a folder dropped here lands at the end.
                None => Some(workspace_count),
                Some(SidebarRow::Tab(..)) => None,
            },
        })
        .collect()
}

impl PaneFlowApp {
    /// Issue #32: the inline rename box hosts the `rename_input` widget. The
    /// row above it owns the enter/escape handling, and the mouse guards keep
    /// a click inside the field from reaching the row's click, drag, and
    /// context-menu listeners.
    fn inline_rename_field(&self, ui: crate::theme::UiColors) -> gpui::Div {
        div()
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .text_color(ui.text)
            .text_sm()
            .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
            .bg(ui.overlay)
            .px_1()
            .rounded_sm()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(self.rename_input.clone())
    }

    fn sidebar_order_signature(workspaces: &[Workspace]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        workspaces.len().hash(&mut hasher);
        for workspace in workspaces {
            workspace.id.hash(&mut hasher);
            match &workspace.repo_root {
                Some(root) => root.hash(&mut hasher),
                None => 0u8.hash(&mut hasher),
            }
        }
        hasher.finish()
    }

    fn compute_display_order(workspaces: &[Workspace]) -> Vec<usize> {
        let mut repo_members: std::collections::HashMap<&std::path::Path, Vec<usize>> =
            std::collections::HashMap::new();
        for (index, workspace) in workspaces.iter().enumerate() {
            if let Some(root) = &workspace.repo_root {
                repo_members.entry(root.as_path()).or_default().push(index);
            }
        }

        let mut order = Vec::with_capacity(workspaces.len());
        let mut placed = vec![false; workspaces.len()];
        for (index, workspace) in workspaces.iter().enumerate() {
            if placed[index] {
                continue;
            }
            if let Some(root) = &workspace.repo_root
                && let Some(members) = repo_members.get(root.as_path())
                && members.len() >= 2
            {
                for &member in members {
                    order.push(member);
                    placed[member] = true;
                }
                continue;
            }
            order.push(index);
            placed[index] = true;
        }
        order
    }

    pub(crate) fn render_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let _render_canary = SidebarRenderTimeCanary::new(self.workspaces.len());
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();
        let mut sidebar = div()
            .relative()
            .w(px(SIDEBAR_WIDTH))
            .flex_shrink_0()
            .h_full()
            // Cockpit rail (#141414). The
            // border-right is gone: the rail and the #181818 content gutter
            // separate by a luminance step, not a drawn divider (the OpenAI
            // surface system - separation by luminance, not borders).
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                window.is_window_active(),
                self.cached_config.cockpit_chrome_material_enabled(),
            ))
            .flex()
            .flex_col();

        let new_workspace_tooltip = self.shortcut_for_action("new_workspace").map_or_else(
            || "New workspace".to_string(),
            |key| format!("New workspace  {key}"),
        );
        sidebar = sidebar.child(
            div()
                // Tight enough that the header reads as the list's first line
                // rather than a floating band: at 48 the header label sat 43 px
                // from the first row's label while consecutive rows sit 34
                // apart.
                .h(px(36.))
                .flex_none()
                .px(px(8.))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .pl(px(8.))
                        .text_size(px(13.))
                        .text_color(ui.muted)
                        .child("Workspaces"),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        // Tight enough that the pair reads as one trailing
                        // cluster rather than as two unrelated controls.
                        .gap(px(2.))
                        .child(customize_menu::render_customize_sidebar_button(
                            customize_menu::CustomizeMenuState {
                                open: self.sidebar_customize_menu_open,
                                submenu_open: self.sidebar_show_submenu_open,
                                show: self.cached_config.sidebar_show,
                                all_expanded: self.all_workspaces_expanded(),
                            },
                            SIDEBAR_FOLDER_ICON_WIDTH,
                            ui,
                            cx,
                        ))
                        .child({
                            // The header action is skinned as a rail row, not as its
                            // own control: same hover tint and same continuous corner,
                            // so it reads as part of the list it heads.
                            let hover_bg = crate::app::constants::sidebar_tab_hover_background();
                            squircle_skin(
                                div()
                                    .id("sidebar-new-workspace")
                                    .size(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center(),
                                "sidebar-new-workspace-group",
                                ROW_RADIUS,
                                None,
                                Some(hover_bg),
                            )
                            .delayed_tooltip(move |_w, cx| {
                                cx.new(|_| SidebarTooltip {
                                    label: new_workspace_tooltip.clone().into(),
                                })
                                .into()
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.create_workspace_with_picker(window, cx);
                            }))
                            .child(
                                svg()
                                    .size(px(SIDEBAR_FOLDER_ICON_WIDTH))
                                    .flex_none()
                                    .path("icons/folder-plus.svg")
                                    .text_color(ui.muted),
                            )
                        }),
                ),
        );

        // Workspace list - scrollable area. Wheel-scroll comes from
        // `overflow_y_scroll + track_scroll`; the visible scroll bar
        // is gone, so the list uses the full sidebar width without a
        // trailing gutter.
        let mut list = div()
            .id("workspace-list")
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .track_scroll(&self.sidebar_scroll)
            .flex()
            .flex_col()
            // No gap and no top padding: the drop dividers are the gaps, and
            // the leading one is the list's top padding.
            .pb(px(4.));

        if self.workspaces.is_empty() {
            list = list.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .px(px(16.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(ui.muted)
                            .child("Open a project folder"),
                    )
                    .child({
                        let hover_bg = crate::app::constants::sidebar_tab_active_background();
                        div()
                            .id("empty-new-ws")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .px(px(10.))
                            .py(px(5.))
                            .rounded(px(6.))
                            .bg(ui.subtle)
                            .text_color(ui.text)
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .hover(move |style| style.bg(hover_bg))
                            .on_click(cx.listener(|this, _: &ClickEvent, w, cx| {
                                this.create_workspace_with_picker(w, cx);
                            }))
                            .child(
                                svg()
                                    .size(px(12.))
                                    .flex_none()
                                    .path("icons/folder_open.svg")
                                    .text_color(ui.muted),
                            )
                            .child("Open folder")
                    }),
            );
        }

        list = self.render_workspace_rows(list, ui, cx);
        sidebar = sidebar.child(self.sidebar_list_wrapper(list, cx));
        sidebar = sidebar.child(self.render_sidebar_settings_footer(cx));
        sidebar
    }

    /// Flatten the rail into the rows it renders. Built once per frame because
    /// the drop dividers are defined by the gaps between these rows.
    fn sidebar_rows(&self) -> Vec<SidebarRow> {
        let signature = Self::sidebar_order_signature(&self.workspaces);
        if self.sidebar_order_cache.borrow().signature != Some(signature) {
            let order = Self::compute_display_order(&self.workspaces);
            let mut cache = self.sidebar_order_cache.borrow_mut();
            cache.order = order;
            cache.signature = Some(signature);
        }
        let order_cache = self.sidebar_order_cache.borrow();
        let mut rows = Vec::with_capacity(order_cache.order.len());
        for &i in &order_cache.order {
            rows.push(SidebarRow::Folder(i));
            // US-009: the tabs of an expanded workspace follow their folder row
            // as sibling children of the scrolling list, so a long tab list
            // scrolls with everything else instead of squeezing the rows above
            // it (`sidebar_workspace_rows_keep_height_when_list_overflows`).
            // An empty workspace shows no child row at all: its single tab is
            // the FR-01 placeholder, not something the user created, and
            // `open_tab` fills it in place. The folder simply reads as empty.
            if self.workspaces[i].sidebar_expanded && !self.workspaces[i].is_empty_shell() {
                for tab_idx in 0..self.workspaces[i].tab_count() {
                    rows.push(SidebarRow::Tab(i, tab_idx));
                }
            }
        }
        rows
    }

    /// Whether every workspace that has rows to show is unfolded.
    ///
    /// `None` when none of them has any: an empty shell shows no child row
    /// whether it is folded or not, so counting it would offer a "Collapse
    /// all" that visibly does nothing.
    fn all_workspaces_expanded(&self) -> Option<bool> {
        let mut rows_somewhere = false;
        let mut all = true;
        for ws in &self.workspaces {
            if ws.is_empty_shell() {
                continue;
            }
            rows_somewhere = true;
            all &= ws.sidebar_expanded;
        }
        rows_somewhere.then_some(all)
    }

    fn render_workspace_rows(
        &self,
        mut list: gpui::Stateful<gpui::Div>,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let rows = self.sidebar_rows();
        let slots = sidebar_drop_slots(&rows, self.workspaces.len());
        for (k, row) in rows.iter().enumerate() {
            list = list.child(self.render_drop_divider(k, slots[k], ui, cx));
            list = list.child(match *row {
                SidebarRow::Folder(i) => self.render_workspace_row(i, ui, cx).into_any_element(),
                SidebarRow::Tab(i, tab_idx) => {
                    self.render_tab_row(i, tab_idx, ui, cx).into_any_element()
                }
            });
        }
        if let Some(&trailing) = slots.last() {
            list = list.child(self.render_drop_divider(rows.len(), trailing, ui, cx));
        }
        list
    }

    /// The gap between two rows, rendered as a real element: an invisible drop
    /// band spanning half a row on either side, holding the insertion line it
    /// reveals while a matching drag hovers it.
    ///
    /// The band is absolutely positioned, so it reaches over its neighbors
    /// without displacing them, and it carries no click listener - a plain
    /// (`HitboxBehavior::Normal`) hitbox never occludes the rows underneath, so
    /// they stay clickable through it. The line itself is a child styled by
    /// `group_drag_over`, which is what keeps the visible mark 2 px thin while
    /// the target stays a comfortable 34 px tall.
    fn render_drop_divider(
        &self,
        key: usize,
        slot: SidebarDropSlot,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Neutral, like the pane placeholder in the grid: the rail already
        // spends its accent on the selected row, and a colored drop line
        // competed with it.
        let color = ui.text.opacity(0.5);
        let group = SharedString::from(format!("drop-slot-{key}"));
        let mut band = div()
            .id(SharedString::from(format!("drop-band-{key}")))
            .group(group.clone())
            .absolute()
            .top(px(-SIDEBAR_DROP_BAND_REACH))
            // Width, not a `left`/`right` inset pair: an absolutely positioned
            // element sized by its insets alone measures 0 px wide here, which
            // leaves both the line and its hitbox invisible.
            .w_full()
            .px(px(SIDEBAR_ROW_MARGIN_X))
            .h(px(SIDEBAR_ROW_SPACING + SIDEBAR_DROP_BAND_REACH * 2.0))
            .flex()
            .flex_col()
            .justify_center();
        // The line reads the band's hover state through `group_drag_over`, and
        // that lookup only runs for an element owning a hitbox: the whole
        // drag-over branch of GPUI's `compute_style_internal` sits behind
        // `if let Some(hitbox)`, while `should_insert_hitbox` counts
        // `drag_over_styles` but *not* `group_drag_over_styles`. Declaring the
        // line a group of its own is what earns it that hitbox - without it the
        // line is laid out and painted, yet never styled, so nothing shows.
        let mut line = div()
            .group(SharedString::from(format!("drop-line-{key}")))
            .h(px(SIDEBAR_DROP_LINE_PX))
            .w_full()
            .rounded_full();

        if let Some((ws_idx, tab_idx)) = slot.tab {
            let ws_id = self.workspaces.get(ws_idx).map(|ws| ws.id);
            band = band
                // US-011: a tab dropped in a gap of its own workspace reorders;
                // dropped in another workspace's gap it reattaches there,
                // keeping its pane tree and its live terminals.
                .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                    if ws_id == Some(drag.workspace_id) {
                        let Some(from) = this
                            .workspaces
                            .get(ws_idx)
                            .and_then(|ws| ws.tabs().iter().position(|tab| tab.id == drag.tab_id))
                        else {
                            return;
                        };
                        this.reorder_workspace_tab(drag, ws_idx, reorder_target(from, tab_idx), cx);
                    } else {
                        this.move_tab_to_workspace(drag, ws_idx, tab_idx, window, cx);
                    }
                }))
                // A pane dragged out of the grid leaves its current tab and
                // becomes a tab of this workspace, terminal still running.
                // Legal on the pane's *own* workspace too: the gesture is "give
                // this pane a tab of its own", not "reattach elsewhere".
                .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                    this.move_pane_to_new_tab(drag.pane_id, ws_idx, tab_idx, window, cx);
                }));
            line = line
                .group_drag_over::<TabDrag>(group.clone(), move |style| style.bg(color))
                .group_drag_over::<PaneDrag>(group.clone(), move |style| style.bg(color));
        }

        if let Some(ws_slot) = slot.workspace {
            band = band.on_drop(cx.listener(move |this, drag: &WorkspaceDrag, _window, cx| {
                // Re-resolve the source by id: the rail re-renders during the
                // drag, so the index captured when it started can be stale.
                let Some(from) = this.workspaces.iter().position(|ws| ws.id == drag.id) else {
                    return;
                };
                this.reorder_workspace(drag.id, reorder_target(from, ws_slot), cx);
            }));
            line =
                line.group_drag_over::<WorkspaceDrag>(group.clone(), move |style| style.bg(color));
        }

        div()
            .h(px(SIDEBAR_ROW_SPACING))
            .flex_none()
            .relative()
            .child(band.child(line))
    }

    fn render_workspace_row(
        &self,
        i: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ws = &self.workspaces[i];

        let title = ws.title.clone();

        let idx = i;
        let ws_id = ws.id;
        let ws_title: SharedString = ws.title.clone().into();
        // Two distinct tints, not one: the row lifts by the hover step, and
        // its own actions hover one step further into the active tint (see
        // `sidebar_action_button`). Sharing a single tint left those actions
        // invisible against the row they sit on.
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        // US-008 / US-010: one hover group per folder row - the trailing agent
        // badge fades out and the create-tab action fades in together.
        let group_name = SharedString::from(format!("ws-row-{ws_id}"));
        let is_expanded = ws.sidebar_expanded;

        let row_shell = sidebar_row_shell()
            .id(SharedString::from(format!("ws-{ws_id}")))
            .group(group_name.clone())
            .on_drag(
                WorkspaceDrag {
                    id: ws_id,
                    title: ws_title.clone(),
                },
                |drag, _offset, _window, cx| {
                    cx.new(|_| WorkspaceDragPreview {
                        title: drag.title.clone(),
                    })
                },
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                // No acknowledge here: `select_workspace` below answers the
                // marks of the tab this click actually reveals. Clearing the
                // whole workspace would drop a completion sitting in a tab the
                // user is not opening.
                this.dismiss_transient_surfaces();
                let was_renaming = this.renaming_tab.is_some();
                this.commit_rename(cx);
                this.select_workspace(idx, window, cx);
                // US-008: the whole card is the disclosure control, not just
                // the folder icon. Two clicks are exempt on either count: the
                // second half of a double-click would fold the row straight
                // back, and committing a rename ends an edit - folding the row
                // under the cursor would read as a side effect of typing.
                let is_double = matches!(e, ClickEvent::Mouse(m) if m.down.click_count >= 2);
                if !was_renaming && !is_double {
                    this.toggle_workspace_expanded(idx, cx);
                }
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, e: &ClickEvent, _window, cx| {
                if e.is_right_click()
                    && let Some(position) = e.mouse_position()
                {
                    this.commit_rename(cx);
                    this.dismiss_transient_surfaces();
                    this.workspace_menu_open = Some(WorkspaceContextMenu { idx, position });
                    cx.stop_propagation();
                    cx.notify();
                }
            }));

        // Row 1: title
        //
        // US-012: expanded, the folder only speaks for what no tab can claim;
        // collapsed, it speaks for everything again. The tooltip reads the very
        // same set, or an expanded folder would enumerate tools its badge is no
        // longer counting.
        let folder_sessions = || folder_row_sessions(ws.agent_sessions.values(), is_expanded);
        let agent_status = ai_types::workspace_agent_status(folder_sessions(), &ws.detected_agents);
        // Same partition as `folder_sessions`: expanded, the folder only keeps
        // the completions no tab row can claim; collapsed, its tab rows are off
        // screen so it speaks for all of them again.
        let completion_unread = if is_expanded {
            ws.agent_completion_notification.has_unattributed_unread()
        } else {
            ws.agent_completion_notification.is_unread()
        };
        let row_agent_status = sidebar_agent_summary(folder_sessions(), completion_unread);
        // A folder row carries the directory's own name and never an edit box:
        // the workspace title is derived from the folder, not typed.
        let title_el = div()
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(ui.text)
            .text_sm()
            .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
            .font_weight(FontWeight::MEDIUM)
            .child(title);

        // US-008: the open/closed folder glyph reports the disclosure state -
        // no chevron beside it, and no click target of its own. The whole card
        // is the disclosure affordance (see the row's click handler), so the
        // icon is a pure indicator; giving it a private handler would have made
        // its 14px square the one spot on the row that toggles without also
        // selecting the workspace.
        let folder_path = if is_expanded {
            "icons/folder-open.svg"
        } else {
            "icons/folder.svg"
        };
        let disclosure = div()
            .flex_none()
            .size(px(SIDEBAR_FOLDER_ICON_WIDTH))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .size(px(SIDEBAR_FOLDER_ICON_WIDTH))
                    .flex_none()
                    .path(folder_path)
                    .text_color(ui.muted),
            );

        let mut title_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SIDEBAR_TITLE_ROW_GAP))
            .w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
            .max_w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
            .min_w_0()
            .overflow_x_hidden()
            // The lane is reserved at all times, not opened on hover: a hover
            // padding only shifts what can still shrink, so a long name ended
            // up under the `+` anyway.
            .pr(px(SIDEBAR_ACTION_LANE_WIDTH))
            .child(disclosure)
            .child(title_el);
        if let Some(row_agent_status) = row_agent_status {
            let status_tooltip = sidebar_agent_status_tooltip(row_agent_status, &agent_status);
            title_row = title_row.child(render_workspace_agent_summary(
                row_agent_status,
                &format!("ws-{ws_id}"),
                status_tooltip,
                ui,
            ));
        }

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(SIDEBAR_ROW_GAP))
            .child(title_row);

        // US-010: hover action cluster on the folder row, on the Agents
        // `hover_actions_cluster` patron - absolute, right-aligned, 20x20
        // buttons, hidden until the row is hovered. The title row reserves the
        // matching lane (`SIDEBAR_ACTION_LANE_WIDTH`), so the cluster never
        // covers the agent badge.
        //
        // The `+` opens the « New pane » preset palette, which covers the
        // shell, the agents, and the workspace's custom commands.
        //
        // Closing the folder is not in this cluster: it drops every tab, pane
        // and terminal it holds without confirmation, which is too much for a
        // hover target one stray click away from the `+`. It lives in the row's
        // context menu (right click) and on `Ctrl+Shift+Q`.
        body = body.child(
            sidebar_hover_actions(group_name.clone()).child(
                sidebar_action_button(
                    SharedString::from(format!("ws-new-tab-{ws_id}")),
                    "icons/plus.svg",
                    12.,
                    ui,
                )
                .delayed_tooltip({
                    let label = SharedString::from(format!("New pane in {ws_title}"));
                    move |_w, cx| {
                        cx.new(|_| SidebarTooltip {
                            label: label.clone(),
                        })
                        .into()
                    }
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_pane_palette(idx, window, cx);
                    cx.stop_propagation();
                })),
            ),
        );

        // Pure hover affordance: selecting a workspace never leaves the row
        // filled. The folder is a container, not a leaf - the selected tab
        // underneath is the one that rests filled, and a second filled block
        // above it read as two selections at once.
        let row = squircle_row(row_shell, group_name.clone(), None, Some(hover_bg), body);

        div()
            .id(SharedString::from(format!("ws-drop-{ws_id}")))
            .mx(px(SIDEBAR_ROW_MARGIN_X))
            .flex_none()
            .flex()
            .flex_col()
            .rounded(ROW_RADIUS)
            .child(row)
    }

    /// US-009 / US-010 / US-011: one tab rendered as a child row of its
    /// workspace folder, on the Agents `thread_row` patron - a leading
    /// invisible placeholder carries the indent and there is deliberately no
    /// per-tab icon.
    fn render_tab_row(
        &self,
        ws_idx: usize,
        tab_idx: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ws = &self.workspaces[ws_idx];
        let ws_id = ws.id;
        let tab = &ws.tabs()[tab_idx];
        let tab_id = tab.id;
        let title = tab_display_title(tab, tab_idx);
        let is_active_tab = tab_idx == ws.active_tab_idx();
        let is_active_workspace = ws_idx == self.active_idx;
        let is_renaming = self.renaming_tab == Some((ws_idx, tab_idx));

        // Per-tab activity (US-012) and per-pane identity (US-013) are read in
        // one walk of the tab's leaves: `AgentSession::surface_id` holds a
        // terminal entity id, so a tab's sessions are exactly the workspace
        // sessions whose surface lives in one of that tab's panes, and the
        // cluster is that same leaf order.
        let panes = tab.collect_panes();
        let mut surfaces: std::collections::HashSet<u64> =
            std::collections::HashSet::with_capacity(panes.len());
        let mut pane_icons: Vec<TabPaneIcon> = Vec::with_capacity(panes.len());
        // US-012: the tab's own detected-agent set. The tooltip must name the
        // tools of THIS tab's panes: handed `Workspace::detected_agents` it
        // would append "Codex running" to a claude-only tab because a sibling
        // tab runs codex, which is exactly the misattribution this epic
        // removes. `apply_pane_scan` writes the workspace set and each
        // terminal's `detected_agent` from the same scan, so this is that set
        // restricted to the tab, not a second source of truth.
        let mut tab_agents: std::collections::HashSet<String> = std::collections::HashSet::new();
        for pane in &panes {
            let pane = pane.read(cx);
            for terminal in pane.terminals() {
                surfaces.insert(terminal.entity_id().as_u64());
                if let Some(agent) = terminal.read(cx).terminal.detected_agent {
                    tab_agents.insert(agent.binary().to_string());
                }
            }
            pane_icons.push(tab_pane_icon(pane, cx));
        }
        // The split picker holds a slot this tab is about to fill, but nothing
        // is split until a preset is picked - so the tab still counts one pane
        // and its icon would render bare. Card it early: the row then already
        // reads as the stack the choice is about to produce, and the pane the
        // new one lands next to is named rather than left ambiguous.
        let pending_split_pane = match self.pane_palette.as_ref().map(|p| &p.placement) {
            Some(PalettePlacement::Split { target, .. }) => {
                let target = target.entity_id();
                panes.iter().any(|pane| pane.entity_id() == target)
            }
            _ => false,
        };
        let tab_sessions = || tab_row_sessions(ws.agent_sessions.values(), &surfaces);
        // A finished turn belongs to the pane that ran it, so the completion
        // dot rests on this row and not on the folder above it. The session
        // itself is already gone by then (`AgentState::Finished` auto-clears),
        // which is why the mark is carried separately from `tab_sessions`.
        let row_agent_status = sidebar_agent_summary(
            tab_sessions(),
            ws.agent_completion_notification.is_unread_for(&surfaces),
        );
        let agent_status = ai_types::workspace_agent_status(tab_sessions(), &tab_agents);
        // One tint for both states, deliberately: the selected row rests at the
        // very fill a hovered row lifts to, so moving the pointer across the
        // rail never produces a block heavier than the selection it is passing
        // over. `sidebar_tab_active_background` is the stronger step, and it
        // stays reserved for what sits *on* a filled row - the hover action
        // buttons - which needs one step further to read at all.
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        // Leaf-only selection, on the Agents `thread_row` grammar: exactly one
        // row in the whole rail rests filled - the visible tab of the visible
        // workspace. The visible tab of another workspace stays flat and is
        // marked by its title color alone (US-009 AC3), so an expanded rail
        // reads as a tree instead of a stack of gray blocks.
        let (resting_bg, hovered_bg) = if is_active_tab && is_active_workspace {
            (Some(hover_bg), None)
        } else {
            (None, Some(hover_bg))
        };
        // Every title in the rail carries the same weight, selected or not: the
        // resting fill is what marks the visible tab now (US-009 AC3's dimmed
        // title is retired), and a muted title on top of it read as a disabled
        // row rather than an unselected one.
        let text_color = ui.text;

        let title_el = if is_renaming {
            self.inline_rename_field(ui)
        } else {
            div()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(text_color)
                .text_sm()
                .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                .child(title.clone())
        };

        // US-012: the activity badge leads the title, in the folder-icon slot.
        // Without a running agent the same slot stays empty, so the flex gap
        // lands the tab title on the same X as the workspace title above it -
        // no extra indent, no per-tab icon, the folder glyph alone marks the
        // level.
        let leading_slot = match row_agent_status {
            Some(status) => render_tab_agent_summary(
                status,
                &format!("tab-{tab_id}"),
                sidebar_agent_status_tooltip(status, &agent_status),
                ui,
            ),
            None => div()
                .flex_none()
                .w(px(SIDEBAR_FOLDER_ICON_WIDTH))
                .into_any_element(),
        };

        // No `overflow_x_hidden()` here, deliberately. GPUI's `overflow_mask`
        // builds a mask as soon as *either* axis is hidden, and on the
        // x-hidden/y-visible arm that mask still clamps Y to the element's own
        // bounds - 18px here, the title's line height. The pane cards are
        // taller than that by design (they overhang into the row's vertical
        // padding), so a mask on this row sliced their top and bottom off and
        // they painted squashed. The row cannot overflow horizontally anyway:
        // its width is pinned, every child but the title is `flex_none`, and
        // the title carries its own `overflow_x_hidden` + `text_ellipsis`.
        let mut title_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SIDEBAR_TITLE_ROW_GAP))
            .w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
            .max_w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
            .min_w_0()
            .child(leading_slot)
            .child(title_el);
        let tab_group = SharedString::from(format!("tab-row-group-{tab_id}"));
        // US-013: the cluster owns the trailing lane - the title is the only
        // thing that shrinks, so the icons are never pushed out of the row.
        //
        // The lane is shared with the row's close action, by swap rather than
        // by reservation: the pane cards state what the tab holds at rest, the
        // `x` takes their place under the pointer. A folder row can afford to
        // hold its lane open at all times (its trailing badge is narrow), but a
        // tab row's cluster is up to four cards wide - reserving room next to
        // it would eat the title on every row to serve a hover-only control.
        // An empty tab has no cards, so it reserves the button's own width
        // instead, or the `x` would land on the title's tail.
        match render_tab_pane_icons(
            &pane_icons,
            &format!("tab-{tab_id}"),
            pending_split_pane,
            ui,
        ) {
            Some(cluster) => {
                title_row = title_row.child(
                    div()
                        .flex_none()
                        // Hiding by `visibility`, not by `display`, for the
                        // reason `sidebar_hover_actions` documents: the two
                        // element phases must agree within the frame the hover
                        // flips. It also stops the hidden lane from holding its
                        // tooltip open under the close button.
                        .group_hover(tab_group.clone(), |style| style.invisible())
                        .child(cluster),
                );
            }
            None => {
                title_row = title_row.child(div().flex_none().w(px(SIDEBAR_ACTION_BUTTON_SIZE)));
            }
        }

        // US-010 patron, applied to a tab: closing drops the `Tab`, and with it
        // its panes and their terminals. Closing the last tab of a workspace
        // leaves an empty tab behind and never closes the workspace (FR-01) -
        // the folder row's own `x` is what closes that.
        title_row = title_row.child(
            sidebar_hover_actions(tab_group.clone()).child(
                sidebar_action_button(
                    SharedString::from(format!("tab-close-{tab_id}")),
                    "icons/close.svg",
                    12.,
                    ui,
                )
                .delayed_tooltip({
                    let label = SharedString::from("Close tab");
                    move |_w, cx| {
                        cx.new(|_| SidebarTooltip {
                            label: label.clone(),
                        })
                        .into()
                    }
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    // Re-resolve both indices by id: rows reorder under a drag,
                    // and a stale position would close the wrong tab.
                    if let Some((at_ws, at_tab)) = this
                        .workspaces
                        .iter()
                        .position(|ws| ws.id == ws_id)
                        .and_then(|at_ws| {
                            this.workspaces[at_ws]
                                .tabs()
                                .iter()
                                .position(|tab| tab.id == tab_id)
                                .map(|at_tab| (at_ws, at_tab))
                        })
                    {
                        this.commit_rename(cx);
                        this.close_workspace_tab(at_ws, at_tab, window, cx);
                    }
                    cx.stop_propagation();
                })),
            ),
        );

        let row_shell = sidebar_row_shell()
            .id(SharedString::from(format!("tab-row-{tab_id}")))
            .group(tab_group.clone())
            .on_drag(
                TabDrag {
                    workspace_id: ws_id,
                    tab_id,
                    title: SharedString::from(title),
                },
                |drag, _offset, _window, cx| {
                    cx.new(|_| WorkspaceDragPreview {
                        title: drag.title.clone(),
                    })
                },
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                let is_double = matches!(e, ClickEvent::Mouse(m) if m.down.click_count == 2);
                if is_double {
                    this.begin_tab_rename(ws_idx, tab_idx, cx);
                } else {
                    this.select_workspace_tab(ws_idx, tab_idx, window, cx);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, e: &ClickEvent, _window, cx| {
                if e.is_right_click()
                    && let Some(position) = e.mouse_position()
                {
                    this.commit_rename(cx);
                    this.dismiss_transient_surfaces();
                    this.tab_menu_open = Some(TabContextMenu {
                        ws_idx,
                        tab_idx,
                        position,
                    });
                    // Discussion #41: refresh the repository's worktree list
                    // for the menu's Worktree section. Off the render thread,
                    // and only on this gesture - the menu is the only reader.
                    this.spawn_worktree_listing(ws_idx, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            // Issue #32: enter/escape only - see the workspace row above.
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _window, cx| {
                if this.renaming_tab != Some((ws_idx, tab_idx)) {
                    return;
                }
                match e.keystroke.key.as_str() {
                    "enter" => {
                        this.commit_rename(cx);
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "escape" => {
                        this.renaming_tab = None;
                        cx.stop_propagation();
                        cx.notify();
                    }
                    _ => {}
                }
            }));

        // The rail's optional second line (branch, diffstat). Built as a column
        // only when that line exists, so with both switches off the row is the
        // single-line row it has always been - same height, same geometry,
        // nothing reserved.
        let body = match self.render_tab_checkout_meta(
            ws,
            tab,
            SIDEBAR_FOLDER_ICON_WIDTH + SIDEBAR_TITLE_ROW_GAP,
            ui,
        ) {
            Some(meta) => div()
                .flex()
                .flex_col()
                .gap(px(SIDEBAR_ROW_GAP))
                .child(title_row)
                .child(meta)
                .into_any_element(),
            None => title_row.into_any_element(),
        };

        let row = squircle_row(row_shell, tab_group, resting_bg, hovered_bg, body);

        div()
            .id(SharedString::from(format!("tab-drop-{tab_id}")))
            .mx(px(SIDEBAR_ROW_MARGIN_X))
            .flex_none()
            .flex()
            .flex_col()
            // The indent hairline is absolute against this box, so it spans a
            // row whether that row grew a second line or not.
            .relative()
            .rounded(ROW_RADIUS)
            .when(
                self.cached_config.sidebar_show.indent_guide_enabled(),
                |el| el.children(render_sidebar_indent_guide(ui, row_agent_status.is_some())),
            )
            .child(row)
    }

    /// The rail's optional git line: the branch on the left, the diffstat
    /// pinned right, each behind its own `sidebar_show` switch. Both read what
    /// the workspace already polls (`git_branch`, `git_stats`, refreshed off
    /// the render thread by the bootstrap git watcher), so this starts no
    /// subprocess of its own.
    ///
    /// `None` outside a repository, and `None` for a clean checkout with only
    /// the diffstat on: the counts answer "how much has changed here", and when
    /// the answer is nothing the question was never asked.
    /// The checkout a session tab row reports on: its own worktree when the
    /// tab is bound to one (discussion #41), otherwise its workspace's.
    ///
    /// The fallback is what makes the two switches useful before any tab is
    /// bound: an unbound tab still works in a checkout, it just shares it with
    /// its siblings. Binding is what makes the line differ between two tabs of
    /// one workspace; the line itself does not wait for it.
    ///
    /// Reads state the off-thread git probes already cache
    /// ([`crate::app::tab_worktree`]) or the workspace already holds; starts no
    /// subprocess of its own.
    /// The branch a tab row stands on, empty for a detached checkout. Keyed on
    /// by the pull-request marker, which only means something for a branch.
    fn tab_row_branch(&self, ws: &Workspace, tab: &Tab) -> String {
        match tab.worktree.as_ref() {
            Some(_) => self
                .tab_checkout_git(tab)
                .map(|git| git.branch.clone())
                .unwrap_or_default(),
            None => ws.git_branch.clone(),
        }
    }

    fn tab_row_checkout(
        &self,
        ws: &Workspace,
        tab: &Tab,
    ) -> Option<(String, crate::workspace::GitDiffStats)> {
        let label = |branch: &str, path: &std::path::Path| {
            crate::workspace::worktree::checkout_label(Some(branch), path, &ws.worktree_root)
        };
        match tab.worktree.as_ref() {
            Some(path) => {
                let git = self.tab_checkout_git(tab)?;
                git.is_repo
                    .then(|| (label(&git.branch, path), git.stats.clone()))
            }
            None => ws.is_git_repo.then(|| {
                (
                    label(&ws.git_branch, &ws.worktree_root),
                    ws.git_stats.clone(),
                )
            }),
        }
    }

    /// The second line of a session tab row: the branch on the left, the
    /// diffstat pinned right, each behind its own switch.
    ///
    /// `None` when both switches are off - the row is then the single-line row
    /// it has always been, same height, nothing reserved - and when the tab
    /// works outside a repository, where neither value exists.
    fn render_tab_checkout_meta(
        &self,
        ws: &Workspace,
        tab: &Tab,
        indent: f32,
        ui: crate::theme::UiColors,
    ) -> Option<AnyElement> {
        let show = self.cached_config.sidebar_show;
        if !show.any_enabled() {
            return None;
        }
        let (label, stats) = self.tab_row_checkout(ws, tab)?;
        let draw_branch = show.branch_enabled() && !label.is_empty();
        let draw_counts = tab_diffstat_visible(show, &stats);
        if !draw_branch && !draw_counts {
            return None;
        }

        // A branch already in review is marked by its own glyph rather than by
        // an extra one: the icon is the row's only spare millimeter, and "this
        // is a branch" is the least useful thing it can say once you know the
        // line is a branch. Absent PR, absent marker - the switch turns on the
        // lookup, not the icon.
        let pr = show
            .pr_enabled()
            .then(|| self.tab_row_branch(ws, tab))
            .and_then(|branch| {
                let repo_root = ws.repo_root.as_ref()?;
                (!branch.is_empty())
                    .then(|| self.pull_request_for(repo_root, &branch))
                    .flatten()
            });

        let branch = draw_branch.then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(3.))
                .flex_1()
                .min_w_0()
                .child(
                    svg()
                        .size(px(12.))
                        .flex_none()
                        .path(match pr {
                            Some(_) => "icons/git-pull-request.svg",
                            None => "icons/git-branch-sidebar.svg",
                        })
                        .text_color(match pr {
                            Some(pr) => pr.state.color(ui),
                            None => ui.muted,
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        // Same size as the tab's name above it, not the 10 px
                        // a meta line usually gets: the branch is what tells
                        // two tabs of one project apart, so it reads as a
                        // value and not as a caption.
                        .text_sm()
                        .text_color(ui.muted)
                        .child(label),
                )
        });

        let counts = draw_counts.then(|| {
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                // A hair under the branch beside it, deliberately: digits have
                // no descenders and almost no ascenders, so at one size they
                // fill the line where the text does not and read as the bigger
                // of the two. One step is the whole correction - at 11 px they
                // stopped reading as the same line.
                .text_size(px(12.))
                .child(
                    div()
                        .text_color(ui.vc_added)
                        .child(format!("+{}", stats.insertions)),
                )
                .child(
                    div()
                        .text_color(ui.vc_deleted)
                        .child(format!("\u{2212}{}", stats.deletions)),
                )
        });

        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(6.))
                // Room for the larger type: 14 px clipped the descenders once
                // the line stopped being a caption.
                .h(px(SIDEBAR_ROW_LINE_HEIGHT))
                // Starts under the title, not under the row's leading edge: a
                // branch beginning at the glyph column reads as a third level
                // of the tree rather than as a second line of this row.
                .pl(px(indent))
                .w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
                .max_w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
                .overflow_x_hidden()
                .when_some(branch, |row, branch| row.child(branch))
                .when_some(counts, |row, counts| row.child(counts))
                .into_any_element(),
        )
    }

    pub(crate) fn sidebar_list_wrapper(
        &self,
        list: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        // The visible scroll bar was removed; wheel-scroll on the
        // inner `list` (driven by `overflow_y_scroll + track_scroll`)
        // is the only scrolling surface now. The wrapper still
        // exists so callers keep a stable insertion point if a
        // trailing affordance lands here later.
        //
        // It is also the rail's drop target: a folder dragged out of the OS
        // file manager and released here is filed as a workspace. The whole
        // list area accepts it rather than a dedicated strip, because the
        // gesture aims at "the sidebar", not at a position in it - the rail's
        // order is the user's own, and a dropped folder joins at the end.
        div()
            .id("sidebar-list-wrapper")
            .relative()
            .group(SIDEBAR_DROP_GROUP)
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            // The wrapper is the drop target for the full area, margin band
            // included, so nothing lands in the gap around the placeholder.
            .on_drop(
                cx.listener(|this, paths: &gpui::ExternalPaths, _window, cx| {
                    this.open_workspace_folders(paths.paths(), cx);
                }),
            )
            .child(list)
            .child(Self::render_sidebar_drop_placeholder(cx))
    }

    /// Neutral drop placeholder for a folder dragged in from the OS file
    /// manager, on the pane-swap grammar (`pane.rs`): a translucent wash of the
    /// theme's text color with a hairline of the same, rounded and floating
    /// inside the rail. Neutral, not the blue split preview - blue means "a new
    /// pane lands here", and this files a folder instead.
    ///
    /// It is absolute, so it never reflows the list, and `invisible()` until a
    /// drag enters the wrapper's group. The `on_drop` is what earns it a
    /// hitbox: GPUI evaluates `group_drag_over` styles only inside
    /// `if let Some(hitbox)`, so a handler-less div would stay invisible
    /// forever (same trap documented on the pane overlay).
    fn render_sidebar_drop_placeholder(cx: &mut Context<Self>) -> impl IntoElement {
        let tint = crate::theme::ui_colors().text;
        div()
            .absolute()
            .top(px(SIDEBAR_DROP_PLACEHOLDER_MARGIN))
            .left(px(SIDEBAR_DROP_PLACEHOLDER_MARGIN))
            .right(px(SIDEBAR_DROP_PLACEHOLDER_MARGIN))
            .bottom(px(SIDEBAR_DROP_PLACEHOLDER_MARGIN))
            .rounded(px(SIDEBAR_DROP_PLACEHOLDER_RADIUS))
            .bg(tint.opacity(SIDEBAR_DROP_PLACEHOLDER_FILL_ALPHA))
            .border_2()
            .border_color(tint.opacity(SIDEBAR_DROP_PLACEHOLDER_BORDER_ALPHA))
            .invisible()
            .group_drag_over::<gpui::ExternalPaths>(SIDEBAR_DROP_GROUP, |style| style.visible())
            .on_drop(
                cx.listener(|this, paths: &gpui::ExternalPaths, _window, cx| {
                    this.open_workspace_folders(paths.paths(), cx);
                }),
            )
    }
}

fn sidebar_agent_status_tooltip(
    summary: SidebarAgentSummary,
    status: &ai_types::WorkspaceAgentStatus,
) -> SharedString {
    let state = summary.tooltip_state();
    if summary.state == SidebarAgentState::Finished {
        return state.into();
    }

    let mut details: Vec<String> = status
        .hooked
        .iter()
        .map(|aggregate| {
            format!(
                "{}{}",
                aggregate.tool.display_name(),
                aggregate.extra_suffix()
            )
        })
        .chain(
            status
                .unhooked
                .iter()
                .map(|tool| format!("{} running", tool.display_name())),
        )
        .collect();
    for label in &status.active_labels {
        if !details.iter().any(|detail| detail.starts_with(label)) {
            details.push(label.clone());
        }
    }

    if details.is_empty() {
        state.into()
    } else {
        format!("{state} · {}", details.join(", ")).into()
    }
}

/// US-013: one icon per pane of a tab, in the leaf order of its tree, so a
/// work thread is recognizable without opening it. `None` for an empty tab -
/// nothing is painted and the row keeps the height of every other row.
fn render_tab_pane_icons(
    icons: &[TabPaneIcon],
    row_key: &str,
    pending_pane: bool,
    ui: crate::theme::UiColors,
) -> Option<AnyElement> {
    if icons.is_empty() {
        return None;
    }
    let (shown, overflow) = tab_icon_cluster_split(icons.len());
    let tooltip: SharedString = icons
        .iter()
        .map(|icon| icon.label)
        .collect::<Vec<_>>()
        .join(", ")
        .into();

    // Every glyph is monochrome - one tint for the whole rail, no brand color.
    // That means `svg()` for all of them, and `svg()` is a mask: without its
    // own `text_color` it paints nothing, and the parent's does not cascade.
    let glyph = ui.text.opacity(0.75);
    // A lone pane is drawn bare: a card around a single icon is a box, not a
    // stack. From two panes up each one gets its own card and the cards slide
    // over each other, which is what reads as a stack. `pending_pane` is the
    // one exception - a split picker already stands in the second slot, so the
    // lone pane cards up now instead of popping into a card the moment the
    // preset is picked.
    if shown == 1 && !pending_pane {
        return Some(tab_pane_icon_lane(
            row_key,
            tooltip,
            overflow,
            ui,
            svg()
                .size(px(SIDEBAR_TAB_ICON_SIZE))
                .min_w(px(SIDEBAR_TAB_ICON_SIZE))
                .flex_none()
                .path(icons[0].path)
                .text_color(glyph),
        ));
    }

    let card_fill = crate::app::constants::sidebar_tab_icon_card_background();
    let card_border = ui.text.opacity(0.14);
    // Brighter than the bare glyph above: the card fill is a step darker than
    // the row, so the mark needs the contrast back to read as the same weight.
    let card_glyph = ui.text.opacity(0.92);

    // The stack is laid out absolutely, not by flex with negative margins.
    // The cluster sits at the end of a fixed-width title row, and a flex row
    // is free to shrink or clip a child whose size is derived from its
    // content: derived, the cards measured about half their width, then
    // vanished entirely once there were two of them. Stating the cluster's
    // exact box and placing each card at its own offset inside it takes the
    // row's remaining space out of the equation.
    let step = SIDEBAR_TAB_CARD_WIDTH - SIDEBAR_TAB_ICON_OVERLAP;
    let cluster_width = SIDEBAR_TAB_CARD_WIDTH + (shown.saturating_sub(1) as f32) * step;
    // The cluster reserves the row's line height, not the card's: the cards
    // are absolute, so their extra two pixels overhang into the row's vertical
    // padding instead of making a tab row taller than every other row.
    let card_overhang = (SIDEBAR_ROW_LINE_HEIGHT - SIDEBAR_TAB_CARD_HEIGHT) / 2.0;
    let mut cluster = div()
        .flex_none()
        .relative()
        .w(px(cluster_width))
        .min_w(px(cluster_width))
        .h(px(SIDEBAR_ROW_LINE_HEIGHT));
    for (slot, icon) in icons[..shown].iter().enumerate() {
        // Cards paint in declaration order, so the later pane sits on top -
        // the same direction as the visual reference.
        cluster = cluster.child(
            div()
                .absolute()
                .top(px(card_overhang))
                .left(px(slot as f32 * step))
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .w(px(SIDEBAR_TAB_CARD_WIDTH))
                .h(px(SIDEBAR_TAB_CARD_HEIGHT))
                // Fill and hairline are painted paths, not `bg()` + `border_1()`:
                // GPUI's box border is a circular arc and would betray the
                // squircle underneath it at the corners, where the two curves
                // diverge most. Both children are absolute, so the flex box
                // still centers the glyph alone.
                .child(squircle::squircle_fill(ROW_RADIUS, card_fill))
                .child(squircle::squircle_border(ROW_RADIUS, px(1.), card_border))
                .child(
                    svg()
                        .size(px(SIDEBAR_TAB_CARD_ICON_SIZE))
                        .flex_none()
                        .path(icon.path)
                        .text_color(card_glyph),
                ),
        );
    }
    Some(tab_pane_icon_lane(row_key, tooltip, overflow, ui, cluster))
}

/// US-013: the trailing lane of a tab row - the pane cluster, and past the cap
/// the `+N`. The counter is text, not a glyph: it labels the stack from
/// outside it, so it neither joins the overlap nor makes a card's width depend
/// on how a digit measures. The lane carries the row's pane tooltip.
fn tab_pane_icon_lane(
    row_key: &str,
    tooltip: SharedString,
    overflow: usize,
    ui: crate::theme::UiColors,
    cluster: impl IntoElement,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("tab-panes-{row_key}")))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(SIDEBAR_TAB_ICON_GAP))
        .delayed_tooltip(move |_w, cx| {
            cx.new(|_| SidebarTooltip {
                label: tooltip.clone(),
            })
            .into()
        })
        .child(cluster)
        .when(overflow > 0, |d| {
            d.child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui.muted)
                    .child(format!("+{overflow}")),
            )
        })
        .into_any_element()
}

/// `row_key` scopes the element and animation ids to one sidebar row. It is a
/// string, not an id: workspace ids and tab ids come from independent counters,
/// so a folder row and a tab row could otherwise collide on the same numeric
/// key inside the same list.
fn render_workspace_agent_summary(
    summary: SidebarAgentSummary,
    row_key: &str,
    tooltip: SharedString,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let (color, glyph, label) = agent_summary_visual(summary, row_key, ui);

    div()
        .id(SharedString::from(format!("agent-status-{row_key}")))
        .w(px(summary.slot_width()))
        .h(px(20.))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap(px(3.))
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .delayed_tooltip(move |_w, cx| {
            cx.new(|_| SidebarTooltip {
                label: tooltip.clone(),
            })
            .into()
        })
        .child(glyph)
        .when_some(label, |d, label| d.child(label))
        .into_any_element()
}

/// US-012: on a tab row the activity badge leads the title instead of trailing
/// it - the trailing lane belongs to the pane-icon cluster. It occupies exactly
/// the folder-icon slot the invisible placeholder would have taken, so the tab
/// title keeps the same X whether or not an agent is running, and the count
/// stays in the tooltip rather than widening the slot and shifting the title.
fn render_tab_agent_summary(
    summary: SidebarAgentSummary,
    row_key: &str,
    tooltip: SharedString,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let (_, glyph, _) = agent_summary_visual(summary, row_key, ui);

    div()
        .id(SharedString::from(format!("agent-status-{row_key}")))
        .w(px(SIDEBAR_FOLDER_ICON_WIDTH))
        .h(px(20.))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .delayed_tooltip(move |_w, cx| {
            cx.new(|_| SidebarTooltip {
                label: tooltip.clone(),
            })
            .into()
        })
        .child(glyph)
        .into_any_element()
}

fn agent_summary_visual(
    summary: SidebarAgentSummary,
    row_key: &str,
    ui: crate::theme::UiColors,
) -> (gpui::Hsla, AnyElement, Option<String>) {
    match summary.state {
        SidebarAgentState::NeedsInput => (
            rgb(0xFBBF24).into(),
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/bell.svg")
                .text_color(rgb(0xFBBF24))
                .into_any_element(),
            Some(if summary.count > 1 {
                format!("Input {}", summary.count)
            } else {
                "Input".to_string()
            }),
        ),
        SidebarAgentState::Errored => (
            ui.agent_error,
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/x_circle.svg")
                .text_color(ui.agent_error)
                .into_any_element(),
            (summary.count > 1).then(|| summary.count.to_string()),
        ),
        SidebarAgentState::Stalled => (
            ui.agent_stalled,
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/triangle-alert.svg")
                .text_color(ui.agent_stalled)
                .into_any_element(),
            (summary.count > 1).then(|| summary.count.to_string()),
        ),
        SidebarAgentState::Thinking => {
            let color = ui.muted;
            (
                color,
                render_comet_trail_loader(row_key, color),
                (summary.count > 1).then(|| summary.count.to_string()),
            )
        }
        SidebarAgentState::Finished => {
            let color: gpui::Hsla = rgb(0x83C3FF).into();
            (
                color,
                div()
                    .size(px(11.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(px(7.)).rounded_full().bg(color))
                    .into_any_element(),
                None,
            )
        }
    }
}

/// Compact GPUI adaptation of Dot Matrix's `Comet Trail` loader. The 3x3
/// perimeter leaves room for larger dots while keeping the native sidebar free
/// of a web runtime, glow, or accent color.
fn render_comet_trail_loader(row_key: &str, color: gpui::Hsla) -> AnyElement {
    static SYNC_EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

    const MATRIX_SIZE: usize = 3;
    const DOT_SIZE: f32 = 3.0;
    const DOT_GAP: f32 = 1.0;
    const CYCLE_MS: u64 = 720;
    const PERIMETER: usize = 8;
    const BASE_OPACITY: f32 = 0.06;
    const TAIL_OPACITIES: [f32; 3] = [0.8144, 0.4864, 0.2568];

    div()
        .size(px(11.))
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(DOT_GAP))
        .with_animation(
            SharedString::from(format!("comet-trail-{row_key}")),
            Animation::new(std::time::Duration::from_millis(CYCLE_MS)).repeat(),
            move |loader, _delta| {
                let cycle_elapsed = SYNC_EPOCH
                    .get_or_init(std::time::Instant::now)
                    .elapsed()
                    .as_millis()
                    % u128::from(CYCLE_MS);
                let head = (cycle_elapsed * PERIMETER as u128 / u128::from(CYCLE_MS)) as usize;

                loader.children((0..MATRIX_SIZE).map(|row| {
                    div()
                        .h(px(DOT_SIZE))
                        .flex_none()
                        .flex()
                        .flex_row()
                        .gap(px(DOT_GAP))
                        .children((0..MATRIX_SIZE).map(move |col| {
                            let order = match (row, col) {
                                (0, 0) => Some(0),
                                (0, 1) => Some(1),
                                (0, 2) => Some(2),
                                (1, 2) => Some(3),
                                (2, 2) => Some(4),
                                (2, 1) => Some(5),
                                (2, 0) => Some(6),
                                (1, 0) => Some(7),
                                _ => None,
                            };
                            let opacity = order.map_or_else(
                                || if head.is_multiple_of(2) { 0.1 } else { 0.18 },
                                |order| {
                                    let trail = (head + PERIMETER - order) % PERIMETER;
                                    TAIL_OPACITIES.get(trail).copied().unwrap_or(BASE_OPACITY)
                                },
                            );

                            div()
                                .size(px(DOT_SIZE))
                                .flex_none()
                                .rounded_full()
                                .bg(color.opacity(opacity))
                        }))
                }))
            },
        )
        .into_any_element()
}

/// Lightweight tooltip body reused by sidebar affordances that just
/// need to show one short label.
/// `pub(crate)`: the tab identity pill (EP-005, pane.rs) reuses it rather
/// than duplicating a fourth one-label tooltip body.
pub(crate) struct SidebarTooltip {
    pub(crate) label: SharedString,
}

impl Render for SidebarTooltip {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::ui_primitives::tooltip_shell().child(self.label.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ROW_RADIUS, SIDEBAR_ACTION_BUTTON_SIZE, SIDEBAR_ACTION_LANE_WIDTH, SIDEBAR_DROP_BAND_REACH,
        SIDEBAR_DROP_LINE_PX, SIDEBAR_FOLDER_ICON_WIDTH, SIDEBAR_ROW_LINE_HEIGHT,
        SIDEBAR_ROW_MARGIN_X, SIDEBAR_ROW_PADDING_Y, SIDEBAR_ROW_SPACING, SIDEBAR_TAB_CARD_HEIGHT,
        SIDEBAR_TAB_CARD_ICON_SIZE, SIDEBAR_TAB_CARD_WIDTH, SIDEBAR_TAB_ICON_CAP,
        SIDEBAR_TAB_ICON_SIZE, SIDEBAR_TITLE_ROW_GAP, SIDEBAR_WIDTH, SidebarAgentState,
        SidebarAgentSummary, SidebarDropSlot, SidebarRow, folder_row_sessions, reorder_target,
        sidebar_agent_summary, sidebar_drop_slots, sidebar_row_shell, tab_diffstat_visible,
        tab_display_title, tab_icon_cluster_split, tab_row_sessions,
    };
    use crate::agent_launcher::TerminalAgent;
    use crate::ai_types::{AgentSession, AgentState};
    use crate::workspace::Tab;
    use gpui::{
        AvailableSpace, InteractiveElement, ParentElement, Styled, TestAppContext, div, point, px,
        size,
    };
    use std::collections::HashSet;

    fn session(state: AgentState) -> AgentSession {
        AgentSession::new(TerminalAgent::ClaudeCode, state)
    }

    fn show(diffstat: bool) -> paneflow_config::schema::SidebarShow {
        paneflow_config::schema::SidebarShow {
            branch: Some(false),
            diffstat: Some(diffstat),
            pr: Some(false),
            indent_guide: Some(false),
        }
    }

    fn dirty() -> crate::workspace::GitDiffStats {
        crate::workspace::GitDiffStats {
            files_changed: 3,
            insertions: 142,
            deletions: 38,
        }
    }

    #[test]
    fn the_diffstat_needs_both_a_switch_and_something_to_report() {
        let clean = crate::workspace::GitDiffStats::default();
        assert!(
            !tab_diffstat_visible(show(false), &dirty()),
            "the counts are opt-in even on a dirty checkout"
        );
        assert!(
            !tab_diffstat_visible(show(true), &clean),
            "a clean checkout prints its branch and stops"
        );
        assert!(tab_diffstat_visible(show(true), &dirty()));
    }

    #[test]
    fn reorder_target_accounts_for_the_removed_source() {
        // Moving down: the source leaves first, so everything after it slides
        // up and the gap the line pointed at is one index lower.
        assert_eq!(reorder_target(0, 3), 2);
        // Moving up: nothing before the gap moves.
        assert_eq!(reorder_target(4, 1), 1);
        // The two gaps around the source are both no-ops.
        assert_eq!(reorder_target(2, 2), 2);
        assert_eq!(reorder_target(2, 3), 2);
    }

    #[test]
    fn drop_slots_sit_between_the_rendered_rows() {
        // One expanded workspace with two tabs, then a collapsed one.
        let rows = [
            SidebarRow::Folder(0),
            SidebarRow::Tab(0, 0),
            SidebarRow::Tab(0, 1),
            SidebarRow::Folder(1),
        ];
        let slots = sidebar_drop_slots(&rows, 2);

        assert_eq!(slots.len(), rows.len() + 1);
        // Above everything: a folder lands first, a tab has nowhere to go.
        assert_eq!(
            slots[0],
            SidebarDropSlot {
                tab: None,
                workspace: Some(0)
            }
        );
        // Under a folder row: that workspace's first tab. Not a folder gap.
        assert_eq!(
            slots[1],
            SidebarDropSlot {
                tab: Some((0, 0)),
                workspace: None
            }
        );
        assert_eq!(
            slots[2],
            SidebarDropSlot {
                tab: Some((0, 1)),
                workspace: None
            }
        );
        // Between two folders: appends to the one above, and reorders folders.
        assert_eq!(
            slots[3],
            SidebarDropSlot {
                tab: Some((0, 2)),
                workspace: Some(1)
            }
        );
        // Past the last row: appends to the collapsed workspace, folder last.
        assert_eq!(
            slots[4],
            SidebarDropSlot {
                tab: Some((1, 0)),
                workspace: Some(2)
            }
        );
    }

    #[gpui::test]
    /// The divider carries the insertion line as an absolutely positioned band
    /// so it can reach over its neighbors without displacing them - a geometry
    /// worth pinning, since getting it wrong paints nothing at all.
    fn a_drop_divider_spans_the_rail_and_reaches_over_its_neighbors(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(SIDEBAR_WIDTH)),
                AvailableSpace::Definite(px(100.)),
            ),
            |_, _| {
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(div().h(px(30.)))
                    .child(
                        div()
                            .h(px(SIDEBAR_ROW_SPACING))
                            .flex_none()
                            .relative()
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-SIDEBAR_DROP_BAND_REACH))
                                    .w_full()
                                    .px(px(SIDEBAR_ROW_MARGIN_X))
                                    .h(px(SIDEBAR_ROW_SPACING + SIDEBAR_DROP_BAND_REACH * 2.0))
                                    .flex()
                                    .flex_col()
                                    .justify_center()
                                    .debug_selector(|| "band".into())
                                    .child(
                                        div()
                                            .h(px(SIDEBAR_DROP_LINE_PX))
                                            .w_full()
                                            .bg(gpui::rgb(0xff0000))
                                            .debug_selector(|| "line".into()),
                                    ),
                            ),
                    )
            },
        );
        let band = cx.debug_bounds("band").expect("drop band not painted");
        let line = cx.debug_bounds("line").expect("drop line not painted");

        // Sized by a width, never by a `left`/`right` inset pair: insets alone
        // leave an absolutely positioned element 0 px wide, and a zero-width
        // band is both invisible and impossible to hover.
        assert_eq!(band.size.width, px(SIDEBAR_WIDTH));
        assert_eq!(
            line.size.width,
            px(SIDEBAR_WIDTH - 2. * SIDEBAR_ROW_MARGIN_X)
        );
        // Reaches half a row above the gap it owns, so consecutive bands tile
        // the rail: every drop aims at the nearest gap.
        assert_eq!(band.origin.y, px(30.) - px(SIDEBAR_DROP_BAND_REACH));
        assert_eq!(
            band.size.height,
            px(SIDEBAR_ROW_SPACING + SIDEBAR_DROP_BAND_REACH * 2.0)
        );
        // The line rests in the gap itself, centered in the band.
        assert_eq!(line.origin.y, px(30.) + px(SIDEBAR_ROW_SPACING / 2.0 - 1.0));
    }

    #[gpui::test]
    fn a_single_line_row_is_thirty_pixels_tall(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(SIDEBAR_WIDTH)),
                AvailableSpace::Definite(px(100.)),
            ),
            |_, _| {
                sidebar_row_shell()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .child(div().flex_none().size(px(SIDEBAR_FOLDER_ICON_WIDTH)))
                            .child(
                                div()
                                    .text_sm()
                                    .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                                    .child("paneflow"),
                            ),
                    )
                    .debug_selector(|| "row".into())
            },
        );

        let bounds = cx.debug_bounds("row").expect("row not painted");
        assert_eq!(
            bounds.size.height,
            px(SIDEBAR_ROW_LINE_HEIGHT + 2. * SIDEBAR_ROW_PADDING_Y),
            "a title line must not let the font's own line height set the row height"
        );
        assert_eq!(bounds.size.height, px(30.));
        // `squircle::trace` silently clamps the radius to half the shorter
        // side, so a corner larger than this would stop being the corner the
        // constant claims.
        assert!(
            ROW_RADIUS <= bounds.size.height / 2.,
            "row corner {ROW_RADIUS:?} exceeds half of a {:?} row",
            bounds.size.height
        );
    }

    #[gpui::test]
    fn sidebar_workspace_rows_keep_height_when_list_overflows(cx: &mut TestAppContext) {
        const ROWS: [&str; 8] = [
            "sidebar-row-0",
            "sidebar-row-1",
            "sidebar-row-2",
            "sidebar-row-3",
            "sidebar-row-4",
            "sidebar-row-5",
            "sidebar-row-6",
            "sidebar-row-7",
        ];

        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(240.)),
                AvailableSpace::Definite(px(200.)),
            ),
            |_, _| {
                let mut list = div()
                    .w(px(240.))
                    .h(px(200.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(4.));

                for selector in ROWS {
                    list = list.child(
                        sidebar_row_shell()
                            .child(div().h(px(20.)).flex_none())
                            .child(div().h(px(14.)).flex_none())
                            .debug_selector(move || selector.into()),
                    );
                }
                list
            },
        );

        for selector in ROWS {
            let bounds = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} not painted"));
            assert_eq!(bounds.size.height, px(50.), "{selector}");
        }
    }

    #[test]
    fn sidebar_agent_summary_hides_idle_without_signal() {
        assert_eq!(sidebar_agent_summary(std::iter::empty(), false), None);
    }

    #[test]
    fn sidebar_agent_summary_counts_winning_needs_input_sessions() {
        let sessions = [
            session(AgentState::WaitingForInput),
            session(AgentState::Errored),
            session(AgentState::WaitingForInput),
        ];
        assert_eq!(
            sidebar_agent_summary(sessions.iter(), false),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                count: 2
            })
        );
    }

    #[test]
    fn sidebar_agent_summary_applies_sidebar_priority() {
        let cases = [
            (
                vec![AgentState::Finished, AgentState::Thinking],
                SidebarAgentState::Thinking,
            ),
            (
                vec![AgentState::Thinking, AgentState::Stalled],
                SidebarAgentState::Stalled,
            ),
            (
                vec![AgentState::Stalled, AgentState::Errored],
                SidebarAgentState::Errored,
            ),
            (
                vec![AgentState::Errored, AgentState::WaitingForInput],
                SidebarAgentState::NeedsInput,
            ),
        ];
        for (states, expected) in cases {
            let sessions: Vec<_> = states.into_iter().map(session).collect();
            assert_eq!(
                sidebar_agent_summary(sessions.iter(), false).map(|summary| summary.state),
                Some(expected)
            );
        }
    }

    #[test]
    fn sidebar_agent_summary_surfaces_unread_completion_without_live_session() {
        assert_eq!(
            sidebar_agent_summary(std::iter::empty(), true),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Finished,
                count: 1
            })
        );
    }

    #[test]
    fn sidebar_agent_summary_hides_acknowledged_finished_session() {
        let sessions = [session(AgentState::Finished)];
        assert_eq!(sidebar_agent_summary(sessions.iter(), false), None);
    }

    #[test]
    fn tab_display_title_falls_back_to_position() {
        // US-009: a tab is unnamed until it is renamed or created from a named
        // preset, and a blank sidebar row would be unclickable in practice.
        let unnamed = Tab::new(String::new(), None);
        assert_eq!(tab_display_title(&unnamed, 0), "Tab 1");
        assert_eq!(tab_display_title(&unnamed, 4), "Tab 5");

        let blank = Tab::new("   ".to_string(), None);
        assert_eq!(tab_display_title(&blank, 1), "Tab 2");

        let named = Tab::new("build".to_string(), None);
        assert_eq!(tab_display_title(&named, 3), "build");
    }

    /// US-012: a session bound to a terminal of the tab, one bound elsewhere,
    /// one never resolved. Only the first may reach the tab row.
    fn attributed_sessions() -> [AgentSession; 3] {
        let mut mine = session(AgentState::WaitingForInput);
        mine.surface_id = Some(11);
        let mut other_tab = session(AgentState::Errored);
        other_tab.surface_id = Some(22);
        let unattributed = session(AgentState::Thinking);
        [mine, other_tab, unattributed]
    }

    #[test]
    fn tab_row_speaks_only_for_the_sessions_of_its_own_surfaces() {
        let sessions = attributed_sessions();
        let surfaces = HashSet::from([11u64]);
        assert_eq!(
            sidebar_agent_summary(tab_row_sessions(sessions.iter(), &surfaces), false),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                count: 1
            }),
            "a tab must not inherit a sibling tab's session, nor an unattributed one"
        );

        // FR-03: a tab with no terminal of its own stays silent even while the
        // workspace is busy.
        assert_eq!(
            sidebar_agent_summary(tab_row_sessions(sessions.iter(), &HashSet::new()), false),
            None
        );
    }

    #[test]
    fn expanded_folder_keeps_only_the_unattributed_sessions() {
        let sessions = attributed_sessions();
        // FR-04: the residue is the `surface_id: None` session, and nothing
        // else - the two resolved ones are spoken for by their tab rows.
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(sessions.iter(), true), false),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Thinking,
                count: 1
            })
        );

        // ... and an expanded folder with no residue paints nothing at all.
        let resolved = [attributed_sessions()[0].clone()];
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(resolved.iter(), true), false),
            None
        );
    }

    #[test]
    fn collapsed_folder_re_aggregates_every_tab() {
        let sessions = attributed_sessions();
        // FR-05: folding hides no state, so the collapsed row falls back to the
        // full precedence over every session, resolved or not.
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(sessions.iter(), false), false),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                count: 1
            })
        );
    }

    #[test]
    fn a_late_resolution_never_double_counts() {
        // Edge case 6: the two filters partition on `surface_id`, so a session
        // is counted by the folder or by its tab, never by both.
        let mut sessions = attributed_sessions().to_vec();
        let surfaces = HashSet::from([11u64, 33u64]);
        let folder_before = folder_row_sessions(sessions.iter(), true).count();
        let tab_before = tab_row_sessions(sessions.iter(), &surfaces).count();
        assert_eq!((folder_before, tab_before), (1, 1));

        sessions[2].surface_id = Some(33);
        let folder_after = folder_row_sessions(sessions.iter(), true).count();
        let tab_after = tab_row_sessions(sessions.iter(), &surfaces).count();
        assert_eq!((folder_after, tab_after), (0, 2));
        assert_eq!(folder_before + tab_before, folder_after + tab_after);
    }

    #[test]
    fn the_folder_action_lane_holds_its_button() {
        // The folder row reserves this lane as trailing padding at all times,
        // so it is the one geometry that must track the cluster's real width:
        // a lane narrower than the cluster leaves a button sitting on the agent
        // badge, which is what the reservation exists to prevent.
        let cluster = SIDEBAR_ACTION_BUTTON_SIZE + SIDEBAR_TITLE_ROW_GAP;
        assert_eq!(
            SIDEBAR_ACTION_LANE_WIDTH, cluster,
            "the reserved lane no longer matches the cluster it holds"
        );
        // A tab row reserves nothing: it hides its pane cluster and drops the
        // button in its place. That only holds while the narrowest cluster - a
        // lone bare glyph - plus the row's gap still clears the button, or the
        // `x` would overhang a title it is not covering for.
        let narrowest_cluster = SIDEBAR_TAB_ICON_SIZE + SIDEBAR_TITLE_ROW_GAP;
        assert!(
            narrowest_cluster >= SIDEBAR_ACTION_BUTTON_SIZE,
            "a tab row's close button now overhangs past its pane cluster onto the title"
        );
    }

    #[test]
    fn tab_card_fits_inside_a_row() {
        // The pane cards are absolute and overhang the title's line height into
        // the row's vertical padding. The row shell clips its own overflow, so
        // a card taller than the whole row paints sliced - which is exactly how
        // an oversized card failed once. This is the bound that keeps it whole.
        let row_height = SIDEBAR_ROW_LINE_HEIGHT + 2. * SIDEBAR_ROW_PADDING_Y;
        assert!(
            SIDEBAR_TAB_CARD_HEIGHT <= row_height,
            "a {SIDEBAR_TAB_CARD_HEIGHT}px card overflows a {row_height}px row and would be clipped"
        );
        // The glyph must leave breathing room on every side, or the mark reads
        // as a framed icon instead of a chip.
        assert!(
            SIDEBAR_TAB_CARD_ICON_SIZE + 8. <= SIDEBAR_TAB_CARD_WIDTH.min(SIDEBAR_TAB_CARD_HEIGHT),
            "a {SIDEBAR_TAB_CARD_ICON_SIZE}px glyph leaves under 4px of padding in the card"
        );
        // ... and the leftover must split into whole pixels on both axes, or
        // rounding lands the glyph off center by half a pixel.
        for side in [SIDEBAR_TAB_CARD_WIDTH, SIDEBAR_TAB_CARD_HEIGHT] {
            let gap = side - SIDEBAR_TAB_CARD_ICON_SIZE;
            assert_eq!(
                gap % 2.,
                0.,
                "a {gap}px gap around the glyph centers it on a half pixel"
            );
        }
    }

    #[test]
    fn tab_icon_cluster_caps_at_four_panes() {
        // US-013: up to the cap every pane gets its own slot; past it the tail
        // folds into a single `+N`.
        assert_eq!(tab_icon_cluster_split(0), (0, 0));
        assert_eq!(tab_icon_cluster_split(1), (1, 0));
        assert_eq!(
            tab_icon_cluster_split(SIDEBAR_TAB_ICON_CAP),
            (SIDEBAR_TAB_ICON_CAP, 0)
        );
        assert_eq!(
            tab_icon_cluster_split(SIDEBAR_TAB_ICON_CAP + 3),
            (SIDEBAR_TAB_ICON_CAP, 3)
        );
    }
}
