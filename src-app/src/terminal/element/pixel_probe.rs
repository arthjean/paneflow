use std::sync::OnceLock;

use gpui::{Pixels, Point};

const ROW_SAMPLE_LIMIT: usize = 16;

#[cfg(test)]
const ALIGNMENT_EPSILON: f32 = 1e-6;

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("PANEFLOW_PIXEL_PROBE").as_deref() == Ok("1"))
}

pub fn overlay_enabled() -> bool {
    static OVERLAY: OnceLock<bool> = OnceLock::new();
    *OVERLAY.get_or_init(|| std::env::var("PANEFLOW_PIXEL_PROBE_OVERLAY").as_deref() == Ok("1"))
}

fn fmt_pix(value: f32) -> String {
    format!("{value:.4}|frac={:+.6}", value.fract())
}

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
