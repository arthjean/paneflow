# Debugging terminal rendering

PaneFlow ships two complementary debug-only probes for the terminal renderer.
Both are gated `#[cfg(debug_assertions)]` and compile to no-ops in release
builds, so they cost nothing to leave in the source tree.

## `PANEFLOW_LATENCY_PROBE=1` - keystroke-to-pixel timing

Captures per-phase latency from `KeyDownEvent` through `paint()` to the next
display refresh. Useful when investigating perceived input lag. Wired in
`src-app/src/terminal/view.rs`.

## `PANEFLOW_PIXEL_PROBE=1` - cell / glyph coordinate logging

Captures the raw coordinates the terminal renderer hands to GPUI. Use it when
investigating visual artifacts (gaps between cells, glyph misalignment, block
character rendering). Defined in
`src-app/src/terminal/element/pixel_probe.rs`.

### Activate

```sh
PANEFLOW_PIXEL_PROBE=1 RUST_LOG=paneflow::pixel_probe=debug cargo run
```

`PANEFLOW_PIXEL_PROBE` is read once at first probe call and cached in a
`OnceLock`, so flipping it mid-session has no effect. Restart the binary.

### Output format

Each record is one `log::debug!` line on the `paneflow::pixel_probe` target.
Numeric fields are formatted as `<value>|frac=<+0.000000>` so a fractional
residual is visible at a glance.

| Record | Fields |
|---|---|
| `cell_dims` | `cell_width_raw`, `cell_width_snapped`, `line_height_raw`, `line_height_snapped`, `scale_factor` |
| `origin` | `x`, `y` |
| `glyph` | `line`, `col`, `x`, `y` (one per text run, sampled to first 16 columns of each row) |
| `bg` | `col`, `line`, `x`, `y`, `w`, `h` (one per cell background, same row sampling) |
| `block_quad` | `col`, `line`, `x`, `y`, `w`, `h` (block elements `▀ ▄ █` etc.) |

Row sampling caps log volume on wide terminals while keeping the visually
interesting left edge fully covered - gaps and alignment artifacts almost
always surface there first.

### Example session

Reproduce the Claude Code banner gap regression from
`debug_block_char_rendering.md`:

```sh
PANEFLOW_PIXEL_PROBE=1 RUST_LOG=paneflow::pixel_probe=debug cargo run 2> probe.log
# In the spawned terminal, run a TUI that renders block characters.
claude
# Quit Paneflow, inspect probe.log:
grep cell_dims probe.log | head -3
grep block_quad probe.log | head -10
```

Look for fractional residuals: `frac=+0.400000` means an 8.4-px value reached
this site, which is the proximate cause US-002 / US-003 / US-004 are designed
to eliminate.

## `PANEFLOW_PIXEL_PROBE_OVERLAY=1` - visual cell-bounds overlay

Independent of `PANEFLOW_PIXEL_PROBE`. Draws a 1-px red border (alpha 0.3) on
every cell in every visible terminal pane, painted after the text pass so the
borders sit above glyphs.

```sh
PANEFLOW_PIXEL_PROBE_OVERLAY=1 cargo run
```

Activate alone for a purely-visual sanity check, or combine with
`PANEFLOW_PIXEL_PROBE=1` to correlate the on-screen grid with the logged
coordinates.

## Asserting alignment in tests

`pixel_probe::assert_pixel_aligned(value, label)` panics when `value.fract()`
exceeds `1e-6`. US-003 / US-004 use it to fail loudly on fractional regressions
in the snapped paint paths.

```rust
#[cfg(test)]
use crate::terminal::element::pixel_probe::assert_pixel_aligned;

assert_pixel_aligned(glyph_x.as_f32(), "glyph_x after US-003 snap");
```

## Release builds

The whole `pixel_probe` module is gated `#[cfg(debug_assertions)]` at its
declaration in `src-app/src/terminal/element/mod.rs`. The call sites
(`resolve_frame_metrics`, `paint`, `paint_text_runs`, `paint_cell_backgrounds`,
`paint_block_quads`) are likewise gated, and the overlay paint pass is
both `cfg`-gated and behind a runtime check on `overlay_enabled()`. A
release build links nothing from this module.
