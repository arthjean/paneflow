//! Paint sub-passes for `TerminalElement`.
//!
//! Each sub-module owns a specific visual layer. `paint()` in
//! `terminal/element/mod.rs` orchestrates them in the fixed order:
//!
//! 1. `background`  - terminal background, per-cell bg rects, block quads
//! 2. `selection`   - selection highlight rects
//! 3. `overlay::search_highlights` - search match rects
//! 4. `text`        - batched `shape_line` glyph runs
//! 5. `overlay::hyperlink` - Ctrl+hover underline
//! 6. `cursor`      - primary cursor + copy-mode anchor cursor
//! 7. `scrollbar`   - right-edge thumb
//! 8. `overlay::ime` - IME handler registration + preedit overlay
//! 9. `overlay::exit` - process-exited centered message
//!
//! Every function here is a `pub fn` inside a `pub(super)` module - the
//! parent module boundary gates access to `element`, and every function
//! takes explicit args (no hidden state).
//!
//! Extracted from `terminal_element.rs` per US-015 of the src-app refactor PRD.

pub(super) mod background;
pub(super) mod cursor;
pub(super) mod overlay;
pub(super) mod scrollbar;
pub(super) mod selection;
pub(super) mod text;
