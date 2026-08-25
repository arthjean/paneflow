//! Workspace tab - the level that owns a pane layout tree.
//!
//! US-001 (prd-cli-tab-hierarchy): a workspace no longer owns a single
//! `LayoutTree`; it owns a list of [`Tab`], each carrying the tree the
//! workspace used to carry, plus the zoom `saved_layout` that used to live at
//! the workspace level. Split, zoom and focus mechanics are unchanged - they
//! now operate one level down.

use gpui::{App, Entity, Window};
use paneflow_config::schema::LayoutNode;

use crate::layout::LayoutTree;
use crate::pane::Pane;

/// Monotonic tab ID counter. Process-local and never persisted: the IPC
/// surface addresses surfaces by `surface_id`, never by tab index or tab id
/// (FR-07), so a stable in-memory identity is all the UI needs.
static NEXT_TAB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn next_tab_id() -> u64 {
    NEXT_TAB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// One working composition inside a workspace: a title plus the pane layout
/// tree, with the zoom bookkeeping that belongs to it.
pub struct Tab {
    /// Unique tab identifier, assigned at construction.
    pub id: u64,
    /// User-facing title. Empty means "unnamed" - the sidebar derives a
    /// fallback label (US-009).
    pub title: String,
    /// Pane layout tree. `None` for an empty tab (every pane closed).
    pub root: Option<LayoutTree>,
    /// Saved layout tree while zoomed. `Some(tree)` means this tab is zoomed
    /// and `root` holds only the zoomed pane as a single Leaf.
    pub saved_layout: Option<LayoutTree>,
}

impl Tab {
    /// Create a tab holding `root`, with a freshly allocated id.
    pub fn new(title: impl Into<String>, root: Option<LayoutTree>) -> Self {
        Self {
            id: next_tab_id(),
            title: title.into(),
            root,
            saved_layout: None,
        }
    }

    /// Create an untitled tab with no pane. Used to honour the "a workspace
    /// always has at least one tab" invariant (FR-01) when the last tab is
    /// closed.
    pub fn empty() -> Self {
        Self::new(String::new(), None)
    }

    pub fn is_zoomed(&self) -> bool {
        self.saved_layout.is_some()
    }

    /// Leave zoom, restoring the saved tree. Returns the pane that was zoomed.
    pub fn exit_zoom(&mut self, cx: &mut App) -> Option<Entity<Pane>> {
        let zoomed_pane = self.root.as_ref().and_then(|root| root.first_leaf());
        let saved = self.saved_layout.take()?;
        self.root = Some(saved);
        if let Some(pane) = &zoomed_pane {
            pane.update(cx, |pane, _| {
                pane.zoomed = false;
            });
        }
        zoomed_pane
    }

    pub fn pane_count(&self) -> usize {
        self.root.as_ref().map_or(0, |r| r.leaf_count())
    }

    /// Whether this tab can take one more pane.
    ///
    /// US-003 (prd-cli-tab-hierarchy): `MAX_PANES` bounds a *tab*, not a
    /// workspace. Every create site - keyboard split, drop-to-split, launch
    /// pad, IPC `surface.split` - gates on this single predicate so the cap
    /// cannot drift between them.
    pub fn can_add_pane(&self) -> bool {
        self.pane_count() < crate::layout::MAX_PANES
    }

    pub fn contains_pane(&self, pane: &Entity<Pane>) -> bool {
        self.root
            .as_ref()
            .is_some_and(|root| root.contains_leaf(pane))
            || self
                .saved_layout
                .as_ref()
                .is_some_and(|saved| saved.contains_leaf(pane))
    }

    pub fn any_pane(&self, f: &mut impl FnMut(&Entity<Pane>) -> bool) -> bool {
        if let Some(root) = &self.root
            && root.any_leaf(f)
        {
            return true;
        }
        if let Some(saved) = &self.saved_layout
            && saved.any_leaf(f)
        {
            return true;
        }
        false
    }

    /// Every pane of this tab, the zoom-saved tree included, in traversal
    /// order and without duplicates.
    pub fn collect_panes(&self) -> Vec<Entity<Pane>> {
        let mut panes = Vec::new();
        if let Some(root) = &self.root {
            panes.extend(root.collect_leaves());
        }
        if let Some(saved) = &self.saved_layout {
            for pane in saved.collect_leaves() {
                if !panes.contains(&pane) {
                    panes.push(pane);
                }
            }
        }
        panes
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        if let Some(root) = &self.root {
            root.focus_first(window, cx);
        }
    }

    /// Serialize this tab's layout to a `LayoutNode`.
    ///
    /// When zoomed, serializes the saved (un-zoomed) layout so the full pane
    /// arrangement is captured rather than just the single zoomed pane.
    pub fn serialize(&self, cx: &App) -> Option<LayoutNode> {
        let tree = self.saved_layout.as_ref().or(self.root.as_ref())?;
        Some(tree.serialize(cx))
    }

    /// Serialize this tab for session persistence without terminal output,
    /// which must remain local to the current process.
    pub fn serialize_without_scrollback(&self, cx: &App) -> Option<LayoutNode> {
        let tree = self.saved_layout.as_ref().or(self.root.as_ref())?;
        Some(tree.serialize_without_scrollback(cx))
    }
}
