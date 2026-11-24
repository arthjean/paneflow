mod density;
mod navigation;
mod presets;
mod queries;
mod render;
mod serde;
mod tree;

pub(crate) use density::{
    CardFrame, PaneDensity, flush_border, flush_fill, flush_outline, grid_edges, morph_corners,
};
pub use navigation::{FocusDirection, FocusNav};
pub(crate) use render::SplitPreview;
#[cfg(test)]
pub(crate) use tree::DragState;
pub(crate) use tree::MIN_PANE_SIZE;
pub(crate) use tree::{LayoutChild, SplitCells};
pub use tree::{LayoutTree, SplitDirection};

pub(crate) const PANE_GUTTER_PX: f32 = tree::DIVIDER_PX;

pub(crate) use paneflow_config::schema::MAX_LAYOUT_LEAVES as MAX_PANES;
