//! Tab drag-and-drop primitives (PRD `prd-pane-drag-drop-2026-Q3.md`,
//! EP-001 reorder + EP-002 cross-pane move).
//!
//! Holds the [`TabDrag`] payload carried by GPUI's managed drag API and the
//! [`TabDragPreview`] ghost entity rendered under the cursor. Wiring
//! (`on_drag` / `drag_over` / `on_drop`) lives in `Pane::render_tab_bar`.
//! Same-pane reorder goes through `Pane::reorder_tab`; cross-pane move goes
//! through [`move_tab_into`] (shared by the drag and the "Move to pane…" menu).
//! Extracted into its own module to keep `pane.rs` near the 250-line budget.
//!
//! Mirrors the in-repo `WorkspaceDrag` / `WorkspaceDragPreview` precedent
//! (`app/drag.rs`, `sidebar/mod.rs`) - same GPUI commit, identical API shape.

use gpui::{
    Context, Entity, EntityId, FontWeight, IntoElement, ParentElement, Render, SharedString,
    Styled, Window, div, px, svg,
};

use crate::agent_sessions::SessionAgent;
use crate::pane::{Pane, PaneEvent};

/// Drag payload for a terminal/markdown tab. Cloned cheaply (the `content`
/// field is an `Entity` handle, not the entity itself) so GPUI can stash it for
/// the duration of the drag. `title` and `icon` are snapshotted at drag start
/// so the floating [`TabDragPreview`] can render without re-reading the entity.
///
/// `source_pane` + `source_tab_id` identify where the tab came from. The
/// source index is kept only as a drag-start hint for insertion previews.
#[derive(Clone)]
pub struct TabDrag {
    pub source_pane: Entity<Pane>,
    pub source_idx: usize,
    pub source_tab_id: EntityId,
    pub title: SharedString,
    pub icon: SharedString,
}

/// Drag payload for an agent-session row dragged out of the docked sessions
/// sidebar (bridges `prd-agent-sessions-sidebar` and this PRD). Unlike
/// [`TabDrag`], which migrates an *existing* tab, dropping a `SessionDrag` on a
/// pane spawns a *fresh* terminal at the session's `cwd` running the agent's
/// `--resume` command. Cloned cheaply (owned ids); `title`/`icon` snapshot the
/// row so the shared [`TabDragPreview`] ghost renders without the sidebar.
#[derive(Clone)]
pub struct SessionDrag {
    pub agent: SessionAgent,
    pub session_id: String,
    pub cwd: String,
    pub title: SharedString,
    pub icon: SharedString,
}

/// Drag payload for a markdown file dragged out of the docked Files sidebar
/// (PRD `prd-files-tree-sidebar-2026-Q3`, EP-003). Dropping it on a pane opens
/// the file via `MarkdownView::open` - into a new split (edge) or appended as a
/// tab (center) - without a process. Only markdown rows are draggable; every
/// other file is inert. Cloned cheaply (an owned `PathBuf` + snapshotted
/// `title`/`icon`) so the shared [`TabDragPreview`] ghost renders without the
/// sidebar.
#[derive(Clone)]
pub struct MarkdownFileDrag {
    pub path: std::path::PathBuf,
    pub title: SharedString,
    pub icon: SharedString,
}

/// Floating ghost rendered under the cursor during a tab drag - a compact
/// version of the tab chip (leading icon + label).
pub struct TabDragPreview {
    pub title: SharedString,
    pub icon: SharedString,
}

impl Render for TabDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(ui.overlay)
            .border_1()
            .border_color(ui.border)
            .shadow_lg()
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(ui.text)
            .child(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path(self.icon.clone())
                    .text_color(ui.muted),
            )
            .child(self.title.clone())
    }
}

/// True when the per-OS duplicate-on-drop modifier is held (EP-003 US-010):
/// Alt on macOS, Ctrl on Linux/Windows. Shift is deliberately never used - it
/// collides with terminal text selection (FR-10). Single home for the `#[cfg]`
/// so every drop site (strip, trailing, content edge, content center) reads the
/// modifier identically.
pub fn duplicate_modifier_held(window: &Window) -> bool {
    let modifiers = window.modifiers();
    #[cfg(target_os = "macos")]
    {
        modifiers.alt
    }
    #[cfg(not(target_os = "macos"))]
    {
        modifiers.control
    }
}

/// Side of a hovered target tab on which to paint the 2px insertion indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertSide {
    Left,
    Right,
}

/// The four pane edges a drop-to-split can target (EP-003). This is a 4-way
/// edge, distinct from the codebase's 2-way [`crate::layout::SplitDirection`]
/// (`Horizontal`=stacked / `Vertical`=side-by-side): an edge encodes both the
/// split axis *and* which side the new pane lands on, which the 2-way enum
/// can't express. The commit maps each edge to a `SplitDirection` plus an
/// optional `swap_panes` for the "before" edges (Up/Left).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropEdge {
    Up,
    Down,
    Left,
    Right,
}

impl DropEdge {
    /// Map a drop edge to the 2-way [`crate::layout::SplitDirection`] plus
    /// whether the new pane swaps to the "before" position. `split_at_pane`
    /// always inserts *after* the target, so the leading edges (Up/Left) swap
    /// the moved/duplicated pane onto the correct side. Single source for the
    /// three drop-to-split handlers (DropSplit / dropped tab / dropped session).
    pub fn to_split(self) -> (crate::layout::SplitDirection, bool) {
        match self {
            DropEdge::Up => (crate::layout::SplitDirection::Horizontal, true),
            DropEdge::Down => (crate::layout::SplitDirection::Horizontal, false),
            DropEdge::Left => (crate::layout::SplitDirection::Vertical, true),
            DropEdge::Right => (crate::layout::SplitDirection::Vertical, false),
        }
    }
}

/// Fraction of a pane's *smaller* dimension that counts as an edge band for
/// drop-to-split (Zed's `drop_target_size` default). Cursor inside any edge
/// band → split toward the nearest edge; the center 60% → move-into-pane.
pub const SPLIT_EDGE_BAND: f32 = 0.20;

/// Resolve a cursor position (relative to a pane's content bounds) to a split
/// edge, using `band` as the fraction of the smaller dimension for each edge
/// strip. The nearest edge wins by min-distance; a cursor in the center
/// (outside every band) returns `None` = move-into-pane. Ported from Zed's
/// `handle_drag_move` (`crates/workspace/src/pane.rs`), adapted to compute in
/// `f32` (GPUI `Pixels` isn't `Ord`) and to Paneflow's [`DropEdge`].
///
/// `width`/`height` are the content size; `x`/`y` the cursor offset from the
/// content's top-left. Correct for non-square panes (uses both dimensions).
pub fn compute_drop_edge(width: f32, height: f32, x: f32, y: f32, band: f32) -> Option<DropEdge> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let size = width.min(height) * band;
    let in_band = x < size || x > width - size || y < size || y > height - size;
    if !in_band {
        return None;
    }
    // Distance from the cursor to each edge; the closest edge wins.
    let candidates = [
        (DropEdge::Up, y),
        (DropEdge::Right, width - x),
        (DropEdge::Down, height - y),
        (DropEdge::Left, x),
    ];
    candidates
        .into_iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(edge, _)| edge)
}

/// The blue preview overlay's target rectangle `(x, y, w, h)` (content-local
/// pixels) for a given drop direction over a pane of size `width`×`height`.
/// `None` (center / move-into-pane) fills the whole pane; each edge fills the
/// corresponding half. Used to drive the overlay's glide animation (US-008):
/// lerping between two of these rects as the cursor crosses band boundaries is
/// what makes the preview slide instead of snapping.
pub fn split_rect(dir: Option<DropEdge>, width: f32, height: f32) -> (f32, f32, f32, f32) {
    let (hw, hh) = (width * 0.5, height * 0.5);
    match dir {
        None => (0.0, 0.0, width, height),
        Some(DropEdge::Up) => (0.0, 0.0, width, hh),
        Some(DropEdge::Down) => (0.0, hh, width, hh),
        Some(DropEdge::Left) => (0.0, 0.0, hw, height),
        Some(DropEdge::Right) => (hw, 0.0, hw, height),
    }
}

/// Which side of a same-pane target tab the insertion indicator belongs on,
/// given the drag's origin slot. `None` = the target *is* the origin slot, so
/// no indicator shows and the eventual drop is a no-op (mirrors Zed's
/// `drag_over::<DraggedTab>` border logic: before-origin → left, after → right).
pub fn insertion_side(source_idx: usize, target_idx: usize) -> Option<InsertSide> {
    use std::cmp::Ordering;
    match target_idx.cmp(&source_idx) {
        Ordering::Less => Some(InsertSide::Left),
        Ordering::Greater => Some(InsertSide::Right),
        Ordering::Equal => None,
    }
}

/// Resulting position of a tab moved from `from` to `to` within a strip of
/// `len` tabs. Returns `None` when the move is a no-op (out of range or
/// landing on its own slot) so the caller can skip the mutation + repaint.
/// `to` is clamped to the last valid index, which makes "drop on the trailing
/// area" (callers pass `len - 1`) land at the end.
pub fn reordered_index(from: usize, to: usize, len: usize) -> Option<usize> {
    if from >= len || len == 0 {
        return None;
    }
    let to = to.min(len - 1);
    (from != to).then_some(to)
}

/// Move a tab from `source` into `dest` at `dest_idx` (EP-002 US-004/US-006),
/// preserving the running PTY - the entity *handle* migrates, the entity is
/// untouched. Shared by the drag-drop path (`dest` = the pane whose `on_drop`
/// fired) and the context-menu path (`dest` chosen from the menu). The moved
/// tab becomes selected + focused in `dest`; terminal tabs are re-subscribed
/// there. If `source` empties, it emits [`PaneEvent::Remove`] so the tree owner
/// (`PaneFlowApp`) reflows it away through the existing cleanup path.
///
/// `dest_cx` is the destination pane's context (the live `&mut Context<Pane>`
/// in the drop listener, or the one handed in by `dest.update(...)` for the
/// menu path). **The caller MUST guarantee `source != dest`** - `source.update`
/// runs while `dest` is mutably borrowed, so a same-entity call would be GPUI
/// re-entrancy. Same-pane moves are routed to `Pane::reorder_tab` upstream, so
/// this precondition always holds.
pub fn move_tab_into(
    dest: &mut Pane,
    dest_cx: &mut Context<Pane>,
    source: &Entity<Pane>,
    source_tab_id: EntityId,
    dest_idx: usize,
    window: &mut Window,
) {
    let (taken, source_emptied) = source.update(dest_cx, |src, src_cx| {
        let tab = src.take_tab_for_move_by_id(source_tab_id);
        let emptied = tab.is_some() && src.tabs.is_empty();
        // A surviving source still needs a repaint (its strip lost a tab and
        // `selected_idx` may have shifted); an emptied one is handled below.
        if tab.is_some() && !emptied {
            src_cx.notify();
        }
        (tab, emptied)
    });
    let Some(tab) = taken else {
        return;
    };
    dest.insert_moved_tab(tab, dest_idx, window, dest_cx);
    dest_cx.emit(PaneEvent::TabsChanged);
    if source_emptied {
        // Reuse the existing Remove path: the tree owner finds the now-empty
        // source pane and drops it from the layout, reflowing siblings.
        source.update(dest_cx, |_src, src_cx| src_cx.emit(PaneEvent::Remove));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_side_picks_left_before_origin() {
        assert_eq!(insertion_side(3, 1), Some(InsertSide::Left));
    }

    #[test]
    fn insertion_side_picks_right_after_origin() {
        assert_eq!(insertion_side(1, 3), Some(InsertSide::Right));
    }

    #[test]
    fn insertion_side_none_on_origin() {
        assert_eq!(insertion_side(2, 2), None);
    }

    #[test]
    fn reorder_noop_on_same_slot() {
        assert_eq!(reordered_index(2, 2, 5), None);
    }

    #[test]
    fn reorder_clamps_to_last() {
        // Trailing-area drop: callers pass len - 1; an over-large `to` clamps.
        assert_eq!(reordered_index(0, 99, 4), Some(3));
    }

    #[test]
    fn reorder_out_of_range_is_noop() {
        assert_eq!(reordered_index(7, 1, 4), None);
        assert_eq!(reordered_index(0, 0, 0), None);
    }

    #[test]
    fn reorder_moves_forward_and_back() {
        assert_eq!(reordered_index(1, 3, 5), Some(3));
        assert_eq!(reordered_index(3, 1, 5), Some(1));
    }

    #[test]
    fn drop_edge_center_is_none() {
        // 1000x800, 20% band → 160px strips; center is move-into-pane.
        assert_eq!(compute_drop_edge(1000., 800., 500., 400., 0.20), None);
    }

    #[test]
    fn drop_edge_picks_nearest_edge() {
        assert_eq!(
            compute_drop_edge(1000., 800., 40., 400., 0.20),
            Some(DropEdge::Left)
        );
        assert_eq!(
            compute_drop_edge(1000., 800., 960., 400., 0.20),
            Some(DropEdge::Right)
        );
        assert_eq!(
            compute_drop_edge(1000., 800., 500., 30., 0.20),
            Some(DropEdge::Up)
        );
        assert_eq!(
            compute_drop_edge(1000., 800., 500., 770., 0.20),
            Some(DropEdge::Down)
        );
    }

    #[test]
    fn drop_edge_non_square_uses_smaller_dimension() {
        // Tall pane 200x1000 → band = 200*0.2 = 40px. A cursor near the right
        // edge resolves to Right even though the pane is far taller than wide.
        assert_eq!(
            compute_drop_edge(200., 1000., 180., 500., 0.20),
            Some(DropEdge::Right)
        );
    }

    #[test]
    fn drop_edge_degenerate_bounds_is_none() {
        assert_eq!(compute_drop_edge(0., 0., 0., 0., 0.20), None);
    }

    #[test]
    fn split_rect_center_fills_pane() {
        assert_eq!(split_rect(None, 800., 600.), (0., 0., 800., 600.));
    }

    #[test]
    fn split_rect_edges_cover_correct_half() {
        assert_eq!(
            split_rect(Some(DropEdge::Up), 800., 600.),
            (0., 0., 800., 300.)
        );
        assert_eq!(
            split_rect(Some(DropEdge::Down), 800., 600.),
            (0., 300., 800., 300.)
        );
        assert_eq!(
            split_rect(Some(DropEdge::Left), 800., 600.),
            (0., 0., 400., 600.)
        );
        assert_eq!(
            split_rect(Some(DropEdge::Right), 800., 600.),
            (400., 0., 400., 600.)
        );
    }
}
