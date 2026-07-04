//! Debug-only pixel-coordinate probe (US-001, terminal-rendering-parity PRD).
//!
//! Records the actual coordinates submitted to GPUI so a future rendering
//! investigation starts from data, not hypothesis. The 6 reverted block-char
//! fix attempts in `debug_block_char_rendering.md` are the cautionary tale this
//! module exists to prevent.
//!
//! ## Activation
//!
//! Two independent env vars, read once at first call into `OnceLock<bool>`s
//! (matches the `PANEFLOW_LATENCY_PROBE` idiom at `terminal/view.rs`):
//!
//! - `PANEFLOW_PIXEL_PROBE=1` enables structured `log::debug!` records on the
//!   `paneflow::pixel_probe` target. Display them with
//!   `RUST_LOG=paneflow::pixel_probe=debug`.
//! - `PANEFLOW_PIXEL_PROBE_OVERLAY=1` (independent - NOT activated by the
//!   first var alone) enables a translucent red bounds overlay above every
//!   cell after the text pass.
//!
//! The whole module is gated `#[cfg(debug_assertions)]` at its parent
//! declaration, so release builds compile to a no-op (zero runtime cost,
//! the symbols never link).
//!
//! ## Output volume
//!
//! Per repaint of a single terminal element:
//! - One `cell_dims` line carrying both the raw measured values and the
//!   integer-snapped values (US-002), plus `scale_factor`.
//! - One `origin` line carrying the gutter-adjusted grid origin.
//! - One `glyph` line per text run whose first column is below
//!   `ROW_SAMPLE_LIMIT` (16). Bounds log volume on wide terminals.
//! - One `bg` line per background rect whose first column is below the same
//!   limit. Same rationale.
//! - One `block_quad` line per block quad (already low-cardinality).

use std::sync::OnceLock;

use gpui::{Pixels, Point};

/// Maximum column index (exclusive) sampled per row. Guards against log
/// blow-up on wide terminals while still covering the visually-interesting
/// left edge where alignment artifacts surface first.
const ROW_SAMPLE_LIMIT: usize = 16;

/// Tolerance for "value is on a pixel boundary". 1e-6 is well below any
/// quantum we'd ever care about (pixels are integer ground truth) while
/// being permissive enough to absorb floating-point round-trip noise from
/// `Pixels` arithmetic. Only consumed by `assert_pixel_aligned`, which
/// itself is test-only.
#[cfg(test)]
const ALIGNMENT_EPSILON: f32 = 1e-6;

/// Cached `PANEFLOW_PIXEL_PROBE=1` flag. Read once from the environment
/// on first call, then served from memory.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("PANEFLOW_PIXEL_PROBE").as_deref() == Ok("1"))
}

/// Cached `PANEFLOW_PIXEL_PROBE_OVERLAY=1` flag. Independent of `enabled()`
/// the overlay can be drawn without log records and vice versa.
pub fn overlay_enabled() -> bool {
    static OVERLAY: OnceLock<bool> = OnceLock::new();
    *OVERLAY.get_or_init(|| std::env::var("PANEFLOW_PIXEL_PROBE_OVERLAY").as_deref() == Ok("1"))
}

/// Format an `f32` together with its fractional residual so a reader can
/// see at a glance whether a value is pixel-snapped.
fn fmt_pix(value: f32) -> String {
    format!("{value:.4}|frac={:+.6}", value.fract())
}

/// Log raw + snapped cell dimensions. Called by `resolve_frame_metrics()`
/// for each rendered terminal frame. Logging both the raw
/// (font-system measurement) and snapped (US-002 integer-rounded) values
/// in the same record lets a reader spot a fractional residual at a glance
/// the snap is a no-op when the raw value is already integer.
///
/// The cell origin is not known at measure time - it is computed later in
/// `paint()`. Use [`record_origin`] from the paint pass to log it for the
/// same frame.
pub fn record_cell_dimensions(
    cell_width_raw: Pixels,
    cell_width_snapped: Pixels,
    line_height_raw: Pixels,
    line_height_snapped: Pixels,
    scale_factor: f32,
) {
    if !enabled() {
        return;
    }
    log::debug!(
        target: "paneflow::pixel_probe",
        "cell_dims cell_width_raw={} cell_width_snapped={} line_height_raw={} line_height_snapped={} scale_factor={scale_factor}",
        fmt_pix(cell_width_raw.as_f32()),
        fmt_pix(cell_width_snapped.as_f32()),
        fmt_pix(line_height_raw.as_f32()),
        fmt_pix(line_height_snapped.as_f32()),
    );
}

/// Log the gutter-adjusted grid origin. Called from `paint()` once per
/// frame after the [`CellGeometry`](super::geometry::CellGeometry) is
/// assembled - keeps origin coordinates separate from the dimension
/// record so a reader can tell which value came from where.
pub fn record_origin(origin: Point<Pixels>) {
    if !enabled() {
        return;
    }
    log::debug!(
        target: "paneflow::pixel_probe",
        "origin x={} y={}",
        fmt_pix(origin.x.as_f32()),
        fmt_pix(origin.y.as_f32()),
    );
}

/// Log a glyph-run origin. Filtered to the first `ROW_SAMPLE_LIMIT` columns
/// of each row to bound output on wide terminals.
pub fn record_glyph(line: i32, col_start: usize, x: Pixels, y: Pixels) {
    if !enabled() || col_start >= ROW_SAMPLE_LIMIT {
        return;
    }
    log::debug!(
        target: "paneflow::pixel_probe",
        "glyph line={line} col={col_start} x={} y={}",
        fmt_pix(x.as_f32()),
        fmt_pix(y.as_f32()),
    );
}

/// Log a per-cell background rect. Same row-sampling filter as `record_glyph`.
pub fn record_background(
    col: usize,
    line: i32,
    x: Pixels,
    y: Pixels,
    width: Pixels,
    height: Pixels,
) {
    if !enabled() || col >= ROW_SAMPLE_LIMIT {
        return;
    }
    log::debug!(
        target: "paneflow::pixel_probe",
        "bg col={col} line={line} x={} y={} w={} h={}",
        fmt_pix(x.as_f32()),
        fmt_pix(y.as_f32()),
        fmt_pix(width.as_f32()),
        fmt_pix(height.as_f32()),
    );
}

/// Log a block-element quad. Block quads are already low-cardinality so no
/// row sampling is applied - useful for spotting fractional residuals on the
/// exact codepoints US-005 will deep-dive.
pub fn record_block_quad(
    col: usize,
    line: i32,
    x: Pixels,
    y: Pixels,
    width: Pixels,
    height: Pixels,
) {
    if !enabled() {
        return;
    }
    log::debug!(
        target: "paneflow::pixel_probe",
        "block_quad col={col} line={line} x={} y={} w={} h={}",
        fmt_pix(x.as_f32()),
        fmt_pix(y.as_f32()),
        fmt_pix(width.as_f32()),
        fmt_pix(height.as_f32()),
    );
}

/// Test helper used by US-003 / US-004 alignment regressions: assert that
/// `value` sits on a pixel boundary (within `ALIGNMENT_EPSILON`).
///
/// `assert!` is intentional - fractional values in a snapped path indicate
/// a real regression and the test must fail loudly. `clippy::panic` targets
/// the `panic!` macro specifically; `assert!` is permitted.
///
/// Gated `#[cfg(test)]` because no production caller exists; future
/// regression tests in this crate's `#[cfg(test)] mod tests` blocks will
/// link against it.
#[cfg(test)]
pub fn assert_pixel_aligned(value: f32, label: &str) {
    let frac = value.fract().abs();
    assert!(
        frac < ALIGNMENT_EPSILON,
        "{label} not pixel-aligned: value={value} fract={frac} (threshold={ALIGNMENT_EPSILON})",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_pixel_aligned_accepts_integers() {
        assert_pixel_aligned(0.0, "zero");
        assert_pixel_aligned(8.0, "eight");
        assert_pixel_aligned(-12.0, "negative");
    }

    #[test]
    fn assert_pixel_aligned_accepts_subepsilon_drift() {
        assert_pixel_aligned(8.0 + ALIGNMENT_EPSILON / 2.0, "drift");
    }

    #[test]
    #[should_panic(expected = "not pixel-aligned")]
    fn assert_pixel_aligned_rejects_fractional() {
        assert_pixel_aligned(8.4, "fractional");
    }

    #[test]
    fn fmt_pix_renders_value_and_fraction() {
        let s = fmt_pix(8.4);
        assert!(s.contains("8.4"), "expected raw value in '{s}'");
        assert!(s.contains("frac="), "expected fractional residual in '{s}'");
    }

    #[test]
    fn fmt_pix_zero_fraction_for_integers() {
        let s = fmt_pix(9.0);
        assert!(s.contains("frac=+0.000000"), "got '{s}'");
    }
}
