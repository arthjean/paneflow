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

pub(crate) const PANE_GUTTER_PX: f32 = tree::DIVIDER_PX;

pub(crate) use paneflow_config::schema::MAX_LAYOUT_LEAVES as MAX_PANES;
