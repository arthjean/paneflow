//! N-ary tree layout for terminal panes.
//!
//! Leaf nodes hold terminal pane entities. Container nodes hold 2+ children
//! with a direction (horizontal/vertical) and per-child flex ratios.
//!
//! Module layout (US-029 of the src-app refactor PRD):
//! - [`tree`] - core types, constants, ratio helpers, `new_split`
//! - [`mutations`] - `split_at_*` and `swap_panes`
//! - [`close`] - `close_focused` and `remove_pane` (kept separate from
//!   `mutations` for the 280 LOC cap)
//! - [`queries`] - read-only traversal + `equalize_ratios`
//! - [`render`] - GPUI flex rendering with drag-to-resize
//! - [`presets`] - `from_panes_equal`, `main_vertical`, `tiled`
//! - [`navigation`] - `FocusDirection`, `FocusNav`, focus movement
//! - [`serde`] - `serialize` / `from_layout_node`

mod close;
mod mutations;
mod navigation;
mod presets;
mod queries;
mod render;
mod serde;
mod tree;

pub use navigation::{FocusDirection, FocusNav};
pub(crate) use render::SplitPreview;
pub(crate) use tree::MIN_PANE_SIZE;
pub use tree::{LayoutTree, SplitDirection};

/// Outer padding around the pane grid, matching the gap between two sibling
/// panes so every card sits at the same distance from its neighbors and from
/// the panel edge.
pub(crate) const PANE_GUTTER_PX: f32 = tree::DIVIDER_PX;

/// Hard cap on leaf panes in a single workspace. The persistence boundary and
/// live mutation paths share one canonical value.
pub(crate) use paneflow_config::schema::MAX_LAYOUT_LEAVES as MAX_PANES;
