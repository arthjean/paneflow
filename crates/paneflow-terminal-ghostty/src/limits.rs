pub(crate) const MAX_SCROLLBACK_ROWS: usize = 100_000;
pub(crate) const MAX_GRID_ROWS: usize = MAX_SCROLLBACK_ROWS + u16::MAX as usize;
pub(crate) const MAX_GRID_CELLS: usize = 12_000_000;
pub(crate) const MAX_SNAPSHOT_CELLS: usize = 4_194_304;
