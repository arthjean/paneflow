//! Sidebar rendering for `PaneFlowApp`: workspace rows, action buttons,
//! notification dropdown, and the context-menu row helpers (in the
//! [`context_menu`] submodule).
//!
//! Extracted from `main.rs` per US-025 of the src-app refactor PRD - pure
//! code-motion, behaviour unchanged. Toast utilities and sidebar-adjacent
//! types (`WorkspaceContextMenu`, `WorkspaceDrag`, `WorkspaceDragPreview`)
//! remain in `main.rs` because they cross module boundaries.

pub(crate) mod context_menu;

use crate::ui_primitives::TooltipDelayExt;
use gpui::{
    Animation, AnimationExt, AnyElement, AppContext, ClickEvent, Context, CursorStyle, FontWeight,
    Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render,
    SharedString, Styled, Window, div, prelude::*, px, rgb, svg,
};

use crate::{
    PaneFlowApp, SIDEBAR_WIDTH, TabContextMenu, TabDrag, WorkspaceContextMenu, WorkspaceDrag,
    WorkspaceDragPreview, ai_types,
    pane_drag::PaneDrag,
    ui_primitives::{ROW_RADIUS, squircle_skin},
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SidebarServiceSummary {
    primary: u16,
    overflow: usize,
}

pub(crate) const SIDEBAR_ROW_MARGIN_X: f32 = 8.0;
pub(crate) const SIDEBAR_ROW_PADDING_X: f32 = 8.0;
pub(crate) const SIDEBAR_ROW_PADDING_Y: f32 = 6.0;
/// Separates a row's title line from its meta line (a detected service).
const SIDEBAR_ROW_GAP: f32 = 4.0;
/// Height of a row's title line, and with it the height of a single-line row:
/// `SIDEBAR_ROW_LINE_HEIGHT + 2 * SIDEBAR_ROW_PADDING_Y`.
///
/// Set explicitly because the default line height is a multiple of the font
/// size, so the rail's row height moved with the font metrics - it measured 23
/// px here and made every row 35 px tall. Pinning it keeps the row at 30 px,
/// the height the rail is designed against, whatever font resolves.
pub(crate) const SIDEBAR_ROW_LINE_HEIGHT: f32 = 18.0;
pub(crate) const SIDEBAR_TITLE_ROW_GAP: f32 = 8.0;
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
/// Vertical space between two rows of the rail. The divider element *is* that
/// space (the list itself sets no gap), so inserting the drop slots costs no
/// layout: nothing moves when a drag starts.
pub(crate) const SIDEBAR_ROW_SPACING: f32 = 4.0;
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
pub(crate) const SIDEBAR_FOLDER_ICON_WIDTH: f32 = 14.0;
/// Extra left inset a tab row takes over its workspace row, on top of the
/// shared [`SIDEBAR_ROW_MARGIN_X`].
///
/// Chosen so the tab title lands on exactly the workspace title's X: the
/// workspace title starts at `margin + padding + folder glyph + gap` = 38, and
/// a tab row pays its own `padding` again, so the inset is `38 - margin -
/// padding` = 22. Anything less and the two titles disagree by a few pixels,
/// which reads as a misalignment rather than as a level.
pub(crate) const SIDEBAR_TAB_INDENT: f32 = SIDEBAR_FOLDER_ICON_WIDTH + SIDEBAR_TITLE_ROW_GAP;
/// Lane a workspace row's SECOND line steps over: the leading glyph and the gap
/// after it, so the line starts under the title rather than under the glyph.
/// A branch beginning under the folder mark reads as a third column instead of
/// as a continuation of the name above it.
pub(crate) const SIDEBAR_META_INDENT: f32 = SIDEBAR_FOLDER_ICON_WIDTH + SIDEBAR_TITLE_ROW_GAP;
/// X of the guide that ties a workspace row to its tabs, from the rail's left
/// edge: the centre line of the folder glyph above it. The tab rows start 7 px
/// to its right, so a filled row never touches the line.
pub(crate) const SIDEBAR_GUIDE_X: f32 =
    SIDEBAR_ROW_MARGIN_X + SIDEBAR_ROW_PADDING_X + SIDEBAR_FOLDER_ICON_WIDTH / 2.0;
/// Extra space above a workspace row that follows another workspace, and the
/// hairline drawn in the middle of it. Both are what separates two groups now
/// that tab rows are indented instead of flush.
const SIDEBAR_GROUP_GAP: f32 = 10.0;
const SIDEBAR_GROUP_SEPARATOR_ALPHA: f32 = 0.13;
pub(crate) const SIDEBAR_GUIDE_ALPHA: f32 = 0.22;
/// Marks a folded workspace row paints before the tail folds into a `+N`, and
/// the side of the square that stands for a tab with nothing running.
const SIDEBAR_FOLDED_MARK_CAP: usize = 5;
/// Space between two marks of a folded row's cluster. Wider than the 3px the
/// badge uses between its glyph and its word: those two are one thing read
/// together, while these are N separate answers that have to be countable at a
/// glance - too tight and four marks read as one smear.
const SIDEBAR_FOLDED_MARK_GAP: f32 = 5.0;
const SIDEBAR_IDLE_MARK_SIZE: f32 = 7.0;
const SIDEBAR_IDLE_MARK_ALPHA: f32 = 0.32;
/// Content width of a tab row: the rail minus its indent, its own margin and
/// its own padding on both sides.
const SIDEBAR_TAB_ROW_CONTENT_WIDTH: f32 =
    SIDEBAR_WIDTH - SIDEBAR_ROW_MARGIN_X * 2.0 - SIDEBAR_TAB_INDENT - SIDEBAR_ROW_PADDING_X * 2.0;
/// Shared shell of every rail row, folder and tab alike, so a workspace row is
/// exactly as tall as a tab row: same padding, same corner, no minimum height.
/// A workspace only grows past that when it renders a meta line (a detected
/// service), which the `gap` then separates from the title.
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
        .cursor(CursorStyle::PointingHand)
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

fn visible_service_ports(
    active_ports: &[u16],
    service_labels: &std::collections::HashMap<u16, crate::terminal::ServiceInfo>,
) -> Vec<u16> {
    active_ports
        .iter()
        .copied()
        .filter(|port| service_labels.contains_key(port))
        .collect()
}

fn sidebar_service_summary(
    active_ports: &[u16],
    service_labels: &std::collections::HashMap<u16, crate::terminal::ServiceInfo>,
) -> Option<SidebarServiceSummary> {
    let visible = visible_service_ports(active_ports, service_labels);
    let primary = visible
        .iter()
        .copied()
        .find(|port| {
            service_labels
                .get(port)
                .is_some_and(|info| info.is_frontend)
        })
        .or_else(|| visible.first().copied())?;
    Some(SidebarServiceSummary {
        primary,
        overflow: visible.len().saturating_sub(1),
    })
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
/// `speaks_for_all` is that second case, and it is deliberately NOT spelled
/// `!expanded`. A workspace whose single unnamed tab is folded into its own row
/// ([`Workspace::solo_tab`]) is expanded and still has no tab row on screen -
/// a third case the old wording could not express, and one where an agent
/// waiting for input would have had no row left to report it.
///
/// The completion notification is deliberately NOT filtered here - it is
/// workspace-scoped and carries no surface, so it stays on the folder row in
/// every state, for the same reason.
fn folder_row_sessions<'a, I>(
    sessions: I,
    speaks_for_all: bool,
) -> impl Iterator<Item = &'a ai_types::AgentSession>
where
    I: IntoIterator<Item = &'a ai_types::AgentSession>,
    I::IntoIter: 'a,
{
    sessions
        .into_iter()
        .filter(move |session| speaks_for_all || session.surface_id.is_none())
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
            // The row underneath now asks for a pointing hand; a field being
            // typed into must not inherit it.
            .cursor(CursorStyle::IBeam)
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
            //
            // A workspace holding one unnamed tab shows none either, for a
            // different reason: the tab is real, but a tree with one leaf is
            // two rows saying the same thing. Its row absorbs the tab (see
            // `Workspace::solo_tab`), and both cases answer `hides_its_tabs`.
            if self.workspaces[i].sidebar_expanded && !self.workspaces[i].hides_its_tabs() {
                for tab_idx in 0..self.workspaces[i].tab_count() {
                    rows.push(SidebarRow::Tab(i, tab_idx));
                }
            }
        }
        rows
    }

    fn render_workspace_rows(
        &self,
        mut list: gpui::Stateful<gpui::Div>,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let rows = self.sidebar_rows();
        let slots = sidebar_drop_slots(&rows, self.workspaces.len());
        let mut seen_workspace = false;
        for (k, row) in rows.iter().enumerate() {
            list = list.child(self.render_drop_divider(k, slots[k], ui, cx));
            list = list.child(match *row {
                SidebarRow::Folder(i) => {
                    let follows_workspace = std::mem::replace(&mut seen_workspace, true);
                    self.render_workspace_row(i, follows_workspace, ui, cx)
                        .into_any_element()
                }
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

    /// `follows_workspace` is positional, not `i > 0`: the rail renders in
    /// `compute_display_order`, so the workspace that happens to be first in
    /// `self.workspaces` is not the one that heads the list. Separating on the
    /// index would have drawn the group rule above whichever workspace was
    /// stored first and skipped it on the one actually at the top.
    fn render_workspace_row(
        &self,
        i: usize,
        follows_workspace: bool,
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
            // Every row in the rail is a click target - selecting a tab,
            // opening or folding a workspace - so the pointer says so, the way
            // the app's other rows already do (`pane_palette`,
            // `surface_picker`). Set here and not on `sidebar_row_shell`:
            // `.cursor()` asks GPUI for the view being rendered, and the layout
            // tests measure a bare shell outside one.
            .cursor(CursorStyle::PointingHand)
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
                let has_children = this
                    .workspaces
                    .get(idx)
                    .is_some_and(|ws| !ws.hides_its_tabs());
                if !was_renaming && !is_double && has_children {
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
        // The row answers for everything its workspace holds whenever no tab
        // row of its own is on screen - folded shut, or folded *into* it
        // because the workspace has a single unnamed tab.
        let solo_tab = ws.solo_tab();
        let speaks_for_all = !is_expanded || ws.hides_its_tabs();
        let folder_sessions = || folder_row_sessions(ws.agent_sessions.values(), speaks_for_all);
        let agent_status = ai_types::workspace_agent_status(folder_sessions(), &ws.detected_agents);
        // Same partition as `folder_sessions`, for the same reason.
        let completion_unread = if speaks_for_all {
            ws.agent_completion_notification.is_unread()
        } else {
            ws.agent_completion_notification.has_unattributed_unread()
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
        // A solo workspace row is not a container: it IS the tab, so an
        // open/closed folder glyph there would report a disclosure state for
        // children that do not exist. It leads with what the tab is running
        // instead - the very glyph `tab_pane_icon` picks for a pane, the
        // detected agent's or the surface kind's - so the row says something
        // about the work rather than restating that a project is a directory.
        let folder_path = match solo_tab {
            Some(tab) => tab
                .collect_panes()
                .first()
                .map_or("icons/terminal.svg", |pane| {
                    tab_pane_icon(pane.read(cx), cx).path
                }),
            None if is_expanded => "icons/folder-open.svg",
            None => "icons/folder.svg",
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
            .child(disclosure)
            .child(title_el);
        // Folded with tab rows of its own to answer for, the row says what is in
        // each of them instead of aggregating them into one word. A folded solo
        // workspace is excluded: its row already IS the tab, so a single mark
        // would only repeat the badge beside it.
        let folded_marks = (!is_expanded && !ws.hides_its_tabs())
            .then(|| render_folded_tab_marks(ws, ws_id, ui, group_name.clone(), cx))
            .flatten();
        match folded_marks {
            Some(marks) => title_row = title_row.child(marks),
            None => {
                if let Some(row_agent_status) = row_agent_status {
                    let status_tooltip =
                        sidebar_agent_status_tooltip(row_agent_status, &agent_status);
                    title_row = title_row.child(render_agent_badge(
                        row_agent_status,
                        &format!("ws-{ws_id}"),
                        status_tooltip,
                        ui,
                        group_name.clone(),
                    ));
                }
            }
        }

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(SIDEBAR_ROW_GAP))
            .child(title_row);

        if let Some(meta_row) = self.render_workspace_meta_row(ws, ui, cx) {
            body = body.child(meta_row);
        }

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

        // Two workspaces used to be told apart by nothing but their glyph,
        // because every row in the rail was flush and identically sized. The
        // tabs are indented now, so the group needs a top edge of its own: a
        // wider gap above a workspace that follows another, with a hairline in
        // the middle of it. The line is absolutely positioned inside that
        // margin, so it costs no layout and the drop dividers still tile the
        // rail unchanged.
        div()
            .id(SharedString::from(format!("ws-drop-{ws_id}")))
            .relative()
            .mx(px(SIDEBAR_ROW_MARGIN_X))
            .when(follows_workspace, |d| d.mt(px(SIDEBAR_GROUP_GAP)))
            .flex_none()
            .flex()
            .flex_col()
            .rounded(ROW_RADIUS)
            .when(follows_workspace, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(SIDEBAR_ROW_PADDING_X))
                        .right(px(SIDEBAR_ROW_PADDING_X))
                        .top(px(-SIDEBAR_GROUP_GAP / 2.0))
                        .h(px(1.))
                        .bg(ui.text.opacity(SIDEBAR_GROUP_SEPARATOR_ALPHA)),
                )
            })
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
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        // The row's own colour, when it has one: the status wash, in the hue of
        // the badge it carries. It is what lets the eye sort a rail of rows
        // before reading a single word.
        let wash = row_agent_status.and_then(|status| agent_summary_wash(status, ui));
        // Leaf-only selection, on the Agents `thread_row` grammar: exactly one
        // row in the whole rail rests filled - the visible tab of the visible
        // workspace. The visible tab of another workspace stays flat and is
        // marked by its title color alone (US-009 AC3), so an expanded rail
        // reads as a tree instead of a stack of gray blocks.
        //
        // The selected row used to rest at the very fill a hover produced, so
        // hovering it changed nothing at all - the one row the pointer is most
        // often on was the only one in the rail with no feedback. It gets a
        // step of its own now: `sidebar_tab_active_background`, the tint the
        // app already reserves for what sits ON a filled row. The resting
        // selection stays at the lighter value, so crossing the rail still
        // never produces a block heavier than the selection it passes over.
        let is_selected = is_active_tab && is_active_workspace;
        let (resting_bg, hovered_bg) = if is_selected {
            (
                Some(over(hover_bg, wash)),
                Some(crate::app::constants::sidebar_tab_selected_hover_increment()),
            )
        } else {
            (wash, Some(hover_bg))
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

        // No `overflow_x_hidden()` here, deliberately. GPUI's `overflow_mask`
        // builds a mask as soon as *either* axis is hidden, and on the
        // x-hidden/y-visible arm that mask still clamps Y to the element's own
        // bounds - 18px here, the title's line height. The pane cards are
        // taller than that by design (they overhang into the row's vertical
        // padding), so a mask on this row sliced their top and bottom off and
        // they painted squashed. The row cannot overflow horizontally anyway:
        // its width is pinned, every child but the title is `flex_none`, and
        // the title carries its own `overflow_x_hidden` + `text_ellipsis`.
        let tab_group = SharedString::from(format!("tab-row-group-{tab_id}"));
        let mut title_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SIDEBAR_TITLE_ROW_GAP))
            .w(px(SIDEBAR_TAB_ROW_CONTENT_WIDTH))
            .max_w(px(SIDEBAR_TAB_ROW_CONTENT_WIDTH))
            .min_w_0()
            .child(title_el);
        // The status trails now, on the workspace row's own X, and the
        // per-pane icon cluster is gone. It had taken the trailing lane, which
        // is what pushed the badge into the folder-glyph column on the left -
        // a coloured mark aligned with the glyphs that mark the level above it,
        // which is precisely what made a tab hard to tell from a workspace. The
        // cluster restated what the tab holds; the row now states what it is
        // doing, which is the thing a rail is scanned for.
        if let Some(status) = row_agent_status {
            title_row = title_row.child(render_agent_badge(
                status,
                &format!("tab-{tab_id}"),
                sidebar_agent_status_tooltip(status, &agent_status),
                ui,
                tab_group.clone(),
            ));
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
            // Every row in the rail is a click target - selecting a tab,
            // opening or folding a workspace - so the pointer says so, the way
            // the app's other rows already do (`pane_palette`,
            // `surface_picker`). Set here and not on `sidebar_row_shell`:
            // `.cursor()` asks GPUI for the view being rendered, and the layout
            // tests measure a bare shell outside one.
            .cursor(CursorStyle::PointingHand)
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

        // The detailed density's second line, on the row that stands for the
        // work. Until #41 binds a tab to a worktree of its own, the only branch
        // a tab can report is the one its workspace is checked out on, so every
        // tab of a workspace shows the same pair. That is the honest state of
        // the feature, and the shape does not change when #41 lands - only
        // where the values come from.
        let body: AnyElement = match self
            .sidebar_is_detailed()
            .then(|| self.render_git_meta(&ws.git_branch, &ws.git_stats, ws.is_git_repo, ui))
            .flatten()
        {
            Some(meta) => div()
                .flex()
                .flex_col()
                .gap(px(SIDEBAR_ROW_GAP))
                .child(title_row)
                .child(
                    div()
                        .w(px(SIDEBAR_TAB_ROW_CONTENT_WIDTH))
                        .max_w(px(SIDEBAR_TAB_ROW_CONTENT_WIDTH))
                        .h(px(14.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .child(meta),
                )
                .into_any_element(),
            None => title_row.into_any_element(),
        };

        let row = squircle_row(row_shell, tab_group, resting_bg, hovered_bg, body);

        // The indent, and the guide that makes it read as a level rather than
        // as a misalignment. The rail is a flat list of sibling rows - the tabs
        // of a workspace are not nested inside its row - so the guide cannot be
        // one element spanning the group: each tab row paints its own segment
        // at `SIDEBAR_GUIDE_X`, reaching `SIDEBAR_ROW_SPACING` above itself to
        // bridge the divider element that separates it from the row above. The
        // segments therefore join into one continuous line running from the
        // bottom of the workspace row down to the last tab, and a tab that
        // moves or is dropped carries its own piece of it.
        //
        // Absolutely positioned, so it costs no layout and nothing shifts when
        // a group gains or loses a tab.
        div()
            .id(SharedString::from(format!("tab-drop-{tab_id}")))
            .relative()
            .ml(px(SIDEBAR_ROW_MARGIN_X + SIDEBAR_TAB_INDENT))
            .mr(px(SIDEBAR_ROW_MARGIN_X))
            .flex_none()
            .flex()
            .flex_col()
            .rounded(ROW_RADIUS)
            .child(
                div()
                    .absolute()
                    // Both insets are measured from this wrapper, which starts
                    // at the indent - hence the subtraction back to the rail's
                    // own left edge.
                    .left(px(SIDEBAR_GUIDE_X
                        - SIDEBAR_ROW_MARGIN_X
                        - SIDEBAR_TAB_INDENT))
                    .top(px(-SIDEBAR_ROW_SPACING))
                    .bottom(px(0.))
                    .w(px(1.))
                    .bg(ui.text.opacity(SIDEBAR_GUIDE_ALPHA)),
            )
            .child(row)
    }

    fn render_workspace_meta_row(
        &self,
        ws: &Workspace,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // `4f8c982` took the branch and the diffstat out of the rail: "both
        // crowded the row for little signal, and the Diff view is where change
        // counts belong". That holds for a rail of projects nobody is comparing
        // and stops holding the moment several checkouts are being read at once,
        // so the two come back only under `SidebarDensity::Detailed`, and only
        // on a workspace row - the row that actually has a branch. A tab has
        // none of its own until #41 binds one to a worktree, and printing the
        // workspace's branch on every tab it holds would be exactly the
        // near-constant line the original note called noise.
        // ... and only on a row that stands for WORK, never on a group head.
        // A workspace with tabs has its checkout described by each of them; the
        // head would repeat underneath itself. That leaves the solo row, which
        // is a tab, and the tab rows themselves.
        let git = (self.sidebar_is_detailed() && ws.solo_tab().is_some())
            .then(|| self.render_git_meta(&ws.git_branch, &ws.git_stats, ws.is_git_repo, ui))
            .flatten();
        let service = sidebar_service_summary(&ws.active_ports, &ws.service_labels);
        if git.is_none() && service.is_none() {
            return None;
        }
        // The second line starts where the TITLE starts (`SIDEBAR_META_INDENT`),
        // and the width gives that lane back so the line still ends flush with
        // the title's own right edge.
        let meta_indent = SIDEBAR_META_INDENT;
        let mut meta_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .ml(px(meta_indent))
            .w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH - meta_indent))
            .max_w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH - meta_indent))
            .h(px(14.))
            .overflow_x_hidden()
            .whitespace_nowrap()
            .text_xs()
            .text_color(ui.muted);

        if let Some(git) = git {
            meta_row = meta_row.child(git);
        }
        let Some(service) = service else {
            return Some(meta_row.into_any_element());
        };
        let port = service.primary;
        let workspace_id = ws.id;
        let info = ws.service_labels.get(&port);
        let is_frontend = info.is_some_and(|service| service.is_frontend);
        let service_name = info
            .and_then(|service| service.label.clone())
            .unwrap_or_else(|| "Local service".to_string());
        let service_tooltip: SharedString = format!("{service_name}  :{port}").into();

        if is_frontend {
            let url = info
                .and_then(|service| service.url.clone())
                .unwrap_or_else(|| format!("http://localhost:{port}"));
            meta_row = meta_row.child(
                div()
                    .id(SharedString::from(format!("port-{workspace_id}-{port}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.))
                    .text_size(px(10.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui.muted)
                    .hover(move |style| style.text_color(ui.text))
                    .delayed_tooltip({
                        let label = service_tooltip.clone();
                        move |_w, cx| {
                            cx.new(|_| SidebarTooltip {
                                label: label.clone(),
                            })
                            .into()
                        }
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.open_workspace_service_url(&url, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        svg()
                            .size(px(10.))
                            .flex_none()
                            .path("icons/world.svg")
                            .text_color(ui.muted),
                    )
                    .child(format!(":{port}")),
            );
        } else {
            meta_row = meta_row.child(
                div()
                    .id(SharedString::from(format!(
                        "port-{workspace_id}-{port}-info"
                    )))
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .delayed_tooltip({
                        let label = service_tooltip.clone();
                        move |_w, cx| {
                            cx.new(|_| SidebarTooltip {
                                label: label.clone(),
                            })
                            .into()
                        }
                    })
                    .child(format!(":{port}")),
            );
        }

        if service.overflow > 0 {
            let overflow = service.overflow;
            meta_row = meta_row.child(
                div()
                    .id(SharedString::from(format!("ports-{workspace_id}-overflow")))
                    .flex_none()
                    .text_size(px(10.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui.muted)
                    .delayed_tooltip(move |_w, cx| {
                        cx.new(|_| SidebarTooltip {
                            label: format!(
                                "{overflow} more services · Right-click workspace to view"
                            )
                            .into(),
                        })
                        .into()
                    })
                    .child(format!("+{overflow}")),
            );
        }

        Some(meta_row.into_any_element())
    }

    /// The detailed line's git half: the branch on the left, the diffstat on
    /// the right, both from what the workspace already polls (`git_branch`,
    /// `git_stats`) - this starts no subprocess of its own.
    ///
    /// `None` for a workspace that is not in a repository, which is the whole
    /// reason the row can afford the line at all: it appears only where it says
    /// something.
    pub(crate) fn sidebar_is_detailed(&self) -> bool {
        self.cached_config.resolved_sidebar_density()
            == paneflow_config::schema::SidebarDensity::Detailed
    }

    fn render_git_meta(
        &self,
        branch: &str,
        stats: &crate::workspace::GitDiffStats,
        is_git_repo: bool,
        ui: crate::theme::UiColors,
    ) -> Option<AnyElement> {
        if !is_git_repo || branch.is_empty() {
            return None;
        }
        // A clean checkout prints nothing at all. The counts answer "how much
        // has changed here"; when the answer is nothing, the question was never
        // asked, and a "clean" chip would spend a permanent slot on the rows
        // that have the least to say. The branch alone is the whole line.
        let counts = (stats.insertions > 0 || stats.deletions > 0).then(|| {
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .text_size(px(10.))
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
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .child(
                    svg()
                        .size(px(10.))
                        .flex_none()
                        .path("icons/git-branch-sidebar.svg")
                        .text_color(ui.muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(10.))
                        .text_color(ui.muted)
                        .child(branch.to_string()),
                )
                .when_some(counts, |d, counts| d.child(counts))
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

/// What a FOLDED workspace row says about what is inside it: one mark per tab,
/// in tab order.
///
/// Folded, its tab rows are off screen and the row falls back to a single
/// aggregate badge - one word for the whole workspace, which answers "something
/// needs input" without saying how much of the workspace is fine. The marks
/// answer the question the fold actually raises: four tabs, one waiting, one
/// working, one errored, one idle.
///
/// Only when folded. Expanded, every tab already speaks for itself and the
/// marks would be the same information twice.
///
/// The idle mark is a SQUARE, not a dot: a grey dot reads as a dimmed version
/// of the blue `Finished` dot rather than as a state of its own.
fn render_folded_tab_marks(
    ws: &Workspace,
    ws_id: u64,
    ui: crate::theme::UiColors,
    group: SharedString,
    cx: &gpui::App,
) -> Option<AnyElement> {
    let tabs = ws.tabs();
    if tabs.is_empty() {
        return None;
    }
    let mut marks = div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .h(px(20.))
        .gap(px(SIDEBAR_FOLDED_MARK_GAP));

    for (idx, tab) in tabs.iter().take(SIDEBAR_FOLDED_MARK_CAP).enumerate() {
        let surfaces = tab.surface_ids(cx);
        let summary = sidebar_agent_summary(
            tab_row_sessions(ws.agent_sessions.values(), &surfaces),
            ws.agent_completion_notification.is_unread_for(&surfaces),
        );
        let title = tab_display_title(tab, idx);
        let tooltip: SharedString = match summary {
            Some(summary) => format!("{title} \u{b7} {}", agent_summary_word(summary)).into(),
            None => title.into(),
        };
        let glyph = match summary {
            Some(summary) => {
                agent_summary_visual(summary, &format!("ws-{ws_id}-mark-{}", tab.id), ui).1
            }
            None => div()
                .size(px(SIDEBAR_IDLE_MARK_SIZE))
                .flex_none()
                .rounded(px(2.))
                .bg(ui.muted.opacity(SIDEBAR_IDLE_MARK_ALPHA))
                .into_any_element(),
        };
        marks = marks.child(
            div()
                .id(SharedString::from(format!("ws-{ws_id}-mark-{}", tab.id)))
                .size(px(11.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .delayed_tooltip(move |_w, cx| {
                    cx.new(|_| SidebarTooltip {
                        label: tooltip.clone(),
                    })
                    .into()
                })
                .child(glyph),
        );
    }

    if let Some(overflow) = tabs.len().checked_sub(SIDEBAR_FOLDED_MARK_CAP)
        && overflow > 0
    {
        marks = marks.child(
            div()
                .flex_none()
                .text_size(px(10.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui.muted.opacity(0.7))
                .child(format!("+{overflow}")),
        );
    }

    Some(
        div()
            .flex_none()
            // Same swap the badge's word makes: the action button takes this
            // space under the pointer.
            .group_hover(group, |style| style.invisible())
            .child(marks)
            .into_any_element(),
    )
}

/// The activity badge, trailing, on a workspace row and on a tab row alike -
/// which is the point: they line up on one X, and a coloured mark on a tab no
/// longer sits in the folder-glyph column pretending to be a level.
///
/// `row_key` scopes the element and animation ids to one sidebar row. It is a
/// string, not an id: workspace ids and tab ids come from independent counters,
/// so a folder row and a tab row could otherwise collide on the same numeric
/// key inside the same list.
///
/// The badge takes its natural width instead of a fixed slot, and the row's
/// hover action is NOT given a reserved lane beside it. The lane is shared by
/// swap, the grammar `render_tab_row`'s pane cards already used: under the
/// pointer the word goes `invisible` - keeping its width, so nothing reflows -
/// and the absolutely positioned action cluster paints over exactly that space.
/// The glyph survives the swap on purpose: losing the state at the moment the
/// user reaches for the row is the regression this whole arrangement exists to
/// avoid.
fn render_agent_badge(
    summary: SidebarAgentSummary,
    row_key: &str,
    tooltip: SharedString,
    ui: crate::theme::UiColors,
    group: SharedString,
) -> AnyElement {
    let (color, glyph, label) = agent_summary_visual(summary, row_key, ui);

    div()
        .id(SharedString::from(format!("agent-status-{row_key}")))
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
        .when_some(label, |d, label| {
            d.child(
                div()
                    .flex_none()
                    .group_hover(group.clone(), |style| style.invisible())
                    .child(label),
            )
        })
        .into_any_element()
}

/// Alpha-composite `top` over `bottom`, source-over, in straight RGB.
///
/// The rail now stacks two independent things in one fill: a status wash that
/// belongs to the row's state, and a selection tint that belongs to where the
/// user is. `squircle_skin` takes one resting colour, so the two have to be
/// resolved into it rather than layered as two elements - and a translucent
/// tint over a translucent wash is not either of them.
///
/// `top` at full alpha short-circuits, which is what keeps the Linux arm of
/// `sidebar_tab_background` (an opaque blend onto the title bar) behaving
/// exactly as it did.
fn over(top: Hsla, bottom: Option<Hsla>) -> Hsla {
    let Some(bottom) = bottom else { return top };
    if top.a >= 1.0 {
        return top;
    }
    let (t, b) = (gpui::Rgba::from(top), gpui::Rgba::from(bottom));
    let a = t.a + b.a * (1.0 - t.a);
    if a <= f32::EPSILON {
        return top;
    }
    let mix = |ct: f32, cb: f32| (ct * t.a + cb * b.a * (1.0 - t.a)) / a;
    Hsla::from(gpui::Rgba {
        r: mix(t.r, b.r),
        g: mix(t.g, b.g),
        b: mix(t.b, b.b),
        a,
    })
}

/// The word a state answers to. A coloured mark alone never says what it
/// means: the bell already carried "Input" and nothing else carried anything,
/// so a blue dot, a red octagon and a grey triangle were three glyphs the user
/// had to have been taught. The count rides the same label rather than a
/// second slot, which is what keeps every badge in the rail one X wide.
fn agent_summary_word(summary: SidebarAgentSummary) -> String {
    let word = match summary.state {
        SidebarAgentState::NeedsInput => "Input",
        SidebarAgentState::Errored => "Error",
        SidebarAgentState::Stalled => "Stalled",
        SidebarAgentState::Thinking => "Working",
        SidebarAgentState::Finished => "Done",
    };
    if summary.count > 1 {
        format!("{word} {}", summary.count)
    } else {
        word.to_string()
    }
}

/// The wash a row in this state carries, or `None` for the states that must
/// not tint one.
///
/// A wash is not a glyph. `Finished`'s pale blue is the right foreground and
/// the wrong background: laid over the rail at any readable alpha it mostly
/// adds white and lands on the very grey the selection fill produces, so the
/// wash uses a deeper blue of the same hue while the dot keeps the pale one.
/// `Stalled` and `Thinking` are grey by definition and get no wash at all - a
/// grey film on a grey rail reads as a hover, which is exactly the signal it
/// would be stealing.
fn agent_summary_wash(summary: SidebarAgentSummary, ui: crate::theme::UiColors) -> Option<Hsla> {
    match summary.state {
        SidebarAgentState::NeedsInput => Some(Hsla::from(rgb(0xFBBF24)).opacity(0.10)),
        SidebarAgentState::Errored => Some(ui.agent_error.opacity(0.10)),
        SidebarAgentState::Finished => Some(Hsla::from(rgb(0x2E8FFF)).opacity(0.16)),
        SidebarAgentState::Stalled | SidebarAgentState::Thinking => None,
    }
}

fn agent_summary_visual(
    summary: SidebarAgentSummary,
    row_key: &str,
    ui: crate::theme::UiColors,
) -> (gpui::Hsla, AnyElement, Option<String>) {
    let label = Some(agent_summary_word(summary));
    match summary.state {
        SidebarAgentState::NeedsInput => (
            rgb(0xFBBF24).into(),
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/bell.svg")
                .text_color(rgb(0xFBBF24))
                .into_any_element(),
            label,
        ),
        SidebarAgentState::Errored => (
            ui.agent_error,
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/x_circle.svg")
                .text_color(ui.agent_error)
                .into_any_element(),
            label,
        ),
        SidebarAgentState::Stalled => (
            ui.agent_stalled,
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/triangle-alert.svg")
                .text_color(ui.agent_stalled)
                .into_any_element(),
            label,
        ),
        SidebarAgentState::Thinking => {
            let color = ui.muted;
            (color, render_comet_trail_loader(row_key, color), label)
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
                label,
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
        ROW_RADIUS, SIDEBAR_DROP_BAND_REACH, SIDEBAR_DROP_LINE_PX, SIDEBAR_FOLDED_MARK_CAP,
        SIDEBAR_FOLDED_MARK_GAP, SIDEBAR_FOLDER_ICON_WIDTH, SIDEBAR_GUIDE_X, SIDEBAR_META_INDENT,
        SIDEBAR_ROW_LINE_HEIGHT, SIDEBAR_ROW_MARGIN_X, SIDEBAR_ROW_PADDING_X,
        SIDEBAR_ROW_PADDING_Y, SIDEBAR_ROW_SPACING, SIDEBAR_TAB_INDENT,
        SIDEBAR_TAB_ROW_CONTENT_WIDTH, SIDEBAR_TITLE_ROW_GAP, SIDEBAR_WIDTH,
        SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH, SidebarAgentState, SidebarAgentSummary,
        SidebarDropSlot, SidebarRow, SidebarServiceSummary, agent_summary_wash, agent_summary_word,
        folder_row_sessions, over, reorder_target, sidebar_agent_summary, sidebar_drop_slots,
        sidebar_row_shell, sidebar_service_summary, tab_display_title, tab_row_sessions,
        visible_service_ports,
    };
    use crate::agent_launcher::TerminalAgent;
    use crate::ai_types::{AgentSession, AgentState};
    use crate::terminal::ServiceInfo;
    use crate::workspace::Tab;
    use gpui::{
        AvailableSpace, Hsla, InteractiveElement, ParentElement, Styled, TestAppContext, div,
        point, px, size,
    };
    use std::collections::{HashMap, HashSet};

    fn session(state: AgentState) -> AgentSession {
        AgentSession::new(TerminalAgent::ClaudeCode, state)
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
    fn visible_service_ports_hide_unlabeled_ephemeral_ports() {
        let labels = HashMap::from([
            (
                3000,
                ServiceInfo {
                    port: 3000,
                    url: Some("http://localhost:3000".to_string()),
                    label: Some("Next.js".to_string()),
                    is_frontend: true,
                },
            ),
            (
                8000,
                ServiceInfo {
                    port: 8000,
                    url: Some("http://localhost:8000".to_string()),
                    label: Some("Fastify".to_string()),
                    is_frontend: false,
                },
            ),
        ]);

        assert_eq!(
            visible_service_ports(&[3000, 53154, 8000, 53155], &labels),
            vec![3000, 8000]
        );
    }

    #[test]
    fn sidebar_service_summary_prefers_frontend_and_counts_overflow() {
        let labels = HashMap::from([
            (
                3000,
                ServiceInfo {
                    port: 3000,
                    url: Some("http://localhost:3000".to_string()),
                    label: Some("API".to_string()),
                    is_frontend: false,
                },
            ),
            (
                5173,
                ServiceInfo {
                    port: 5173,
                    url: Some("http://localhost:5173".to_string()),
                    label: Some("Vite".to_string()),
                    is_frontend: true,
                },
            ),
            (
                8000,
                ServiceInfo {
                    port: 8000,
                    url: Some("http://localhost:8000".to_string()),
                    label: Some("Fastify".to_string()),
                    is_frontend: false,
                },
            ),
        ]);

        assert_eq!(
            sidebar_service_summary(&[3000, 53154, 5173, 8000], &labels),
            Some(SidebarServiceSummary {
                primary: 5173,
                overflow: 2,
            })
        );
    }

    #[test]
    fn sidebar_service_summary_falls_back_to_first_visible_service() {
        let labels = HashMap::from([
            (
                3000,
                ServiceInfo {
                    port: 3000,
                    url: None,
                    label: Some("API".to_string()),
                    is_frontend: false,
                },
            ),
            (
                8000,
                ServiceInfo {
                    port: 8000,
                    url: None,
                    label: Some("Worker".to_string()),
                    is_frontend: false,
                },
            ),
        ]);

        assert_eq!(
            sidebar_service_summary(&[3000, 8000], &labels),
            Some(SidebarServiceSummary {
                primary: 3000,
                overflow: 1,
            })
        );
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
    fn a_folder_with_tab_rows_keeps_only_the_unattributed_sessions() {
        let sessions = attributed_sessions();
        // FR-04: the residue is the `surface_id: None` session, and nothing
        // else - the two resolved ones are spoken for by their tab rows.
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(sessions.iter(), false), false),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Thinking,
                count: 1
            })
        );

        // ... and such a folder with no residue paints nothing at all.
        let resolved = [attributed_sessions()[0].clone()];
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(resolved.iter(), false), false),
            None
        );
    }

    #[test]
    fn a_folder_with_no_tab_rows_re_aggregates_every_tab() {
        let sessions = attributed_sessions();
        // FR-05: hiding tab rows hides no state, so the row falls back to the
        // full precedence over every session, resolved or not.
        //
        // Two situations reach this, and the second is why the flag is not
        // spelled `!expanded`: a folded workspace, and an expanded one whose
        // single unnamed tab was folded INTO its row. Without it, an agent
        // waiting for input in a solo workspace would have no row to report
        // from - the tab row that used to carry it is not rendered any more.
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(sessions.iter(), true), false),
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
        let folder_before = folder_row_sessions(sessions.iter(), false).count();
        let tab_before = tab_row_sessions(sessions.iter(), &surfaces).count();
        assert_eq!((folder_before, tab_before), (1, 1));

        sessions[2].surface_id = Some(33);
        let folder_after = folder_row_sessions(sessions.iter(), false).count();
        let tab_after = tab_row_sessions(sessions.iter(), &surfaces).count();
        assert_eq!((folder_after, tab_after), (0, 2));
        assert_eq!(folder_before + tab_before, folder_after + tab_after);
    }

    #[test]
    fn a_workspace_second_line_starts_under_its_title() {
        // The lane the meta line steps over must be the one the title row
        // actually spends before its text: the glyph and the gap after it.
        // Anything else and the branch begins under the folder mark, reading as
        // a third column rather than as a continuation of the name above it.
        assert_eq!(
            SIDEBAR_META_INDENT,
            SIDEBAR_FOLDER_ICON_WIDTH + SIDEBAR_TITLE_ROW_GAP,
            "the second line no longer clears the row's leading glyph"
        );
        // A tab row has no leading glyph, so its own second line steps over
        // nothing - the two indents are deliberately equal but unrelated, and
        // this pins that they stay computed apart.
        assert_eq!(SIDEBAR_META_INDENT, SIDEBAR_TAB_INDENT);
    }

    #[test]
    fn every_row_puts_its_badge_on_one_x() {
        // The whole point of the trailing badge: a workspace row and a tab row
        // must end their content on the same X, or the status marks zigzag down
        // the rail. Neither reserves a lane for its hover button any more - the
        // lane is shared by swap - so the only thing that could still separate
        // them is a difference in trailing padding, and there is none.
        let row_right = SIDEBAR_WIDTH - SIDEBAR_ROW_MARGIN_X - SIDEBAR_ROW_PADDING_X;
        let tab_right = SIDEBAR_ROW_MARGIN_X
            + SIDEBAR_TAB_INDENT
            + SIDEBAR_ROW_PADDING_X
            + SIDEBAR_TAB_ROW_CONTENT_WIDTH;
        assert_eq!(
            row_right, tab_right,
            "a tab row's content no longer ends where a workspace row's does"
        );
    }

    #[test]
    fn an_indented_tab_title_lands_under_its_workspace_title() {
        // The indent is not a loose "looks nested" value: the tab title has to
        // land on exactly the workspace title's X. A few pixels off and the two
        // read as a misalignment rather than as a level, which is the whole
        // failure this indent exists to fix.
        let workspace_title_x = SIDEBAR_ROW_MARGIN_X
            + SIDEBAR_ROW_PADDING_X
            + SIDEBAR_FOLDER_ICON_WIDTH
            + SIDEBAR_TITLE_ROW_GAP;
        let tab_title_x = SIDEBAR_ROW_MARGIN_X + SIDEBAR_TAB_INDENT + SIDEBAR_ROW_PADDING_X;
        assert_eq!(workspace_title_x, tab_title_x);
    }

    #[test]
    fn the_guide_clears_the_rows_it_ties_together() {
        // The guide rides the folder glyph's centre line and the tab rows start
        // to its right. A filled row touching the line looks like a rendering
        // fault, so the clearance is the property, not the coordinate.
        let tab_left = SIDEBAR_ROW_MARGIN_X + SIDEBAR_TAB_INDENT;
        assert!(
            SIDEBAR_GUIDE_X < tab_left,
            "the guide now runs under the tab rows it is meant to sit beside"
        );
        assert!(
            tab_left - SIDEBAR_GUIDE_X >= 4.,
            "the guide is close enough to the rows that a filled one touches it"
        );
    }

    #[test]
    fn the_folded_cluster_caps_before_it_eats_the_title() {
        // Five 11px marks plus their gaps is what a 300px rail can spare next
        // to a name; past that the tail folds into a `+N`.
        let marks = SIDEBAR_FOLDED_MARK_CAP as f32;
        let cluster = marks * 11. + (marks - 1.) * SIDEBAR_FOLDED_MARK_GAP;
        assert!(
            cluster < SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH / 2.,
            "the folded cluster now takes more than half the row from its title"
        );
    }

    #[test]
    fn every_state_says_what_it_is() {
        // A coloured mark alone teaches nobody: the bell used to be the only
        // state carrying a word, so a blue dot and a grey triangle were glyphs
        // the user had to have been taught. Every state answers with one now,
        // and the count rides that same label rather than a second slot.
        for state in [
            SidebarAgentState::NeedsInput,
            SidebarAgentState::Errored,
            SidebarAgentState::Stalled,
            SidebarAgentState::Thinking,
            SidebarAgentState::Finished,
        ] {
            let one = agent_summary_word(SidebarAgentSummary { state, count: 1 });
            assert!(!one.is_empty(), "{state:?} has no word");
            assert!(
                !one.chars().next().is_some_and(|c| c.is_ascii_digit()),
                "{state:?} answers with a bare count"
            );
            let many = agent_summary_word(SidebarAgentSummary { state, count: 3 });
            assert_eq!(many, format!("{one} 3"), "{state:?} drops its count");
        }
    }

    #[test]
    fn only_a_coloured_state_washes_its_row() {
        // A grey film on a grey rail reads as a hover, which is the one signal
        // the wash must not steal. So the two grey states carry none.
        let ui = crate::theme::ui_colors();
        for state in [SidebarAgentState::Stalled, SidebarAgentState::Thinking] {
            assert!(
                agent_summary_wash(SidebarAgentSummary { state, count: 1 }, ui).is_none(),
                "{state:?} washes its row in the colour of a hover"
            );
        }
        for state in [
            SidebarAgentState::NeedsInput,
            SidebarAgentState::Errored,
            SidebarAgentState::Finished,
        ] {
            let wash = agent_summary_wash(SidebarAgentSummary { state, count: 1 }, ui)
                .unwrap_or_else(|| panic!("{state:?} lost its wash"));
            assert!(
                wash.a > 0. && wash.a < 0.25,
                "{state:?} washes at {}, which paints over the row instead of tinting it",
                wash.a
            );
        }
    }

    #[test]
    fn a_wash_survives_a_translucent_tint_over_it() {
        // The two are composited into one resting colour, so the selection must
        // not swallow the state: an `over` that returned the top colour would
        // leave every selected row the same grey whatever it is doing.
        //
        // The tint is spelled out here rather than read from
        // `sidebar_tab_hover_background`, which is PLATFORM-DEPENDENT: its Linux
        // arm blends onto the title bar and hands back an opaque colour, so an
        // assertion phrased against it held on macOS and could not hold on
        // Linux. What is being tested is the compositing rule, and that rule is
        // the same everywhere.
        let ui = crate::theme::ui_colors();
        let wash = agent_summary_wash(
            SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                count: 1,
            },
            ui,
        );
        assert!(wash.is_some(), "the state under test lost its wash");

        let tint = Hsla::from(gpui::rgb(0xffffff)).opacity(0.07);
        let composed = over(tint, wash);
        assert!(
            composed.a > tint.a,
            "compositing dropped the wash underneath the tint"
        );
        assert_eq!(
            over(tint, None),
            tint,
            "a row with no wash must rest at exactly the tint"
        );

        // ... and the short-circuit that keeps the Linux arm behaving as it did:
        // an opaque tint has nothing to composite with, so it is the answer.
        let opaque = Hsla { a: 1.0, ..tint };
        assert_eq!(
            over(opaque, wash),
            opaque,
            "an opaque tint must not be reopened by the wash under it"
        );
    }
}
