#[cfg(target_os = "macos")]
use std::collections::HashSet;
use std::sync::LazyLock;

use gpui::{
    App, Font, FontFallbacks, FontFeatures, FontId, FontStyle, FontWeight, Pixels, SharedString,
    Window, px,
};

use super::face_tables;
use super::{CellDimensions, TerminalFrameMetrics};

pub(crate) const DEFAULT_FONT_SIZE: f32 = 13.0;
const POINTS_TO_PIXELS: f32 = 96.0 / 72.0;
pub(crate) const DEFAULT_LINE_HEIGHT: f32 = 1.0;
pub(crate) const DEFAULT_CELL_WIDTH: f32 = 1.0;
pub(crate) const DEFAULT_FONT_WEIGHT_KEY: &str = "normal";

pub(crate) const EMBEDDED_MONO_FAMILY: &str = "JetBrainsMono Nerd Font";
pub(crate) const JETBRAINS_MONO_NF_ALIAS: &str = "JetBrainsMono NF";
pub(crate) const LEGACY_JETBRAINS_MONO_NFM_FAMILY: &str = "JetBrainsMono Nerd Font Mono";
pub(crate) const JETBRAINS_MONO_NFM_ALIAS: &str = "JetBrainsMono NFM";
pub(crate) const LEGACY_GEIST_MONO_FAMILY: &str = "Geist Mono";
pub(crate) const LEGACY_EMBEDDED_MONO_FAMILY: &str = "Lilex";

pub(crate) const EMBEDDED_SANS_FAMILY: &str = "Geist";

pub(crate) const PANEFLOW_MONO_ALIAS: &str = ".PaneflowMono";
pub(crate) const PANEFLOW_SANS_ALIAS: &str = ".PaneflowSans";

fn expand_paneflow_alias(name: &str) -> &str {
    match name {
        PANEFLOW_MONO_ALIAS
        | JETBRAINS_MONO_NF_ALIAS
        | JETBRAINS_MONO_NFM_ALIAS
        | LEGACY_JETBRAINS_MONO_NFM_FAMILY => EMBEDDED_MONO_FAMILY,
        PANEFLOW_SANS_ALIAS => EMBEDDED_SANS_FAMILY,
        other => other,
    }
}

#[cfg(target_os = "macos")]
static INSTALLED_MONO_FONTS: LazyLock<HashSet<String>> =
    LazyLock::new(|| crate::fonts::load_mono_fonts().into_iter().collect());

struct CachedFontConfig {
    settings: FontSettings,
    family: String,
    font_weight_key: &'static str,
    ligatures: bool,
    mtime: Option<std::time::SystemTime>,
    last_check: std::time::Instant,
}

#[derive(Clone)]
pub(super) struct FontSettings {
    pub(super) font: Font,
    pub(super) size: f32,
    pub(super) line_height: f32,
    pub(super) cell_width: f32,
}

fn sanitize_font_fallbacks(configured: Option<&Vec<String>>) -> Option<Vec<String>> {
    let list: Vec<String> = configured?
        .iter()
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect();
    (!list.is_empty()).then_some(list)
}

fn canonical_font_weight_key(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|ch| match ch {
            '-' | ' ' => '_',
            _ => ch.to_ascii_lowercase(),
        })
        .collect()
}

fn font_weight_key_from_config(configured: Option<&str>) -> (&'static str, bool) {
    let Some(raw) = configured else {
        return (DEFAULT_FONT_WEIGHT_KEY, false);
    };
    let key = canonical_font_weight_key(raw);
    if key.is_empty() {
        return (DEFAULT_FONT_WEIGHT_KEY, false);
    }
    let resolved = match key.as_str() {
        "thin" => "thin",
        "extra_light" | "extralight" => "extra_light",
        "light" => "light",
        "semi_light" | "semilight" => "semi_light",
        "normal" => "normal",
        "medium" => "medium",
        "semi_bold" | "semibold" => "semi_bold",
        "bold" => "bold",
        "extra_bold" | "extrabold" => "extra_bold",
        "black" => "black",
        "extra_black" | "extrablack" => "extra_black",
        _ => return (DEFAULT_FONT_WEIGHT_KEY, true),
    };
    (resolved, false)
}

pub(crate) fn normalize_font_weight_key(configured: Option<&str>) -> &'static str {
    font_weight_key_from_config(configured).0
}

fn font_weight_from_key(key: &str) -> FontWeight {
    match key {
        "thin" => FontWeight::THIN,
        "extra_light" => FontWeight::EXTRA_LIGHT,
        "light" => FontWeight::LIGHT,
        "semi_light" => FontWeight(350.0),
        "normal" => FontWeight::NORMAL,
        "medium" => FontWeight::MEDIUM,
        "semi_bold" => FontWeight::SEMIBOLD,
        "bold" => FontWeight::BOLD,
        "extra_bold" => FontWeight::EXTRA_BOLD,
        "black" | "extra_black" => FontWeight::BLACK,
        _ => FontWeight::NORMAL,
    }
}

static FONT_CONFIG_CACHE: std::sync::Mutex<Option<CachedFontConfig>> = std::sync::Mutex::new(None);
static DEFAULT_MONO_FAMILY: LazyLock<&'static str> =
    LazyLock::new(|| select_default_font_family(crate::fonts::load_mono_fonts()));

fn select_default_font_family<I, S>(_available_families: I) -> &'static str
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    EMBEDDED_MONO_FAMILY
}

pub(crate) fn default_font_family() -> &'static str {
    *DEFAULT_MONO_FAMILY
}

pub fn resolve_font_family(configured: Option<&str>) -> String {
    let candidate = configured
        .map(str::trim)
        .filter(|family| !family.is_empty())
        .map(expand_paneflow_alias)
        .unwrap_or(default_font_family());

    if candidate == EMBEDDED_MONO_FAMILY
        || candidate == LEGACY_GEIST_MONO_FAMILY
        || candidate == LEGACY_EMBEDDED_MONO_FAMILY
        || candidate == EMBEDDED_SANS_FAMILY
        || candidate == "IBM Plex Sans"
        || candidate == "IBM Plex Mono"
    {
        return candidate.to_string();
    }

    #[cfg(target_os = "macos")]
    if !INSTALLED_MONO_FONTS.is_empty() && !INSTALLED_MONO_FONTS.contains(candidate) {
        let fallback = default_font_family();
        log::warn!(
            "font_family '{candidate}' is not an installed monospace family; using default '{fallback}'"
        );
        return fallback.to_string();
    }

    candidate.to_string()
}

pub(super) fn cached_font_config() -> FontSettings {
    use std::time::{Duration, Instant};
    const CHECK_INTERVAL: Duration = Duration::from_millis(500);

    let mut cache = FONT_CONFIG_CACHE.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(c) = cache.as_mut() {
        if c.last_check.elapsed() < CHECK_INTERVAL {
            return c.settings.clone();
        }
        let mtime = crate::theme::config_mtime();
        if mtime.is_some() && mtime == c.mtime {
            c.last_check = Instant::now();
            return c.settings.clone();
        }
    }

    let mtime = crate::theme::config_mtime();
    let config = paneflow_config::loader::load_config();

    let family = resolve_font_family(config.font_family.as_deref());

    let size = config
        .font_size
        .map(|s| {
            if (8.0..=32.0).contains(&s) {
                s
            } else {
                log::warn!(
                    "font_size {s}pt out of range [8.0, 32.0]; using default {DEFAULT_FONT_SIZE}pt"
                );
                DEFAULT_FONT_SIZE
            }
        })
        .unwrap_or(DEFAULT_FONT_SIZE);

    let line_height = config
        .line_height
        .map(|lh| {
            if (0.8..=2.5).contains(&lh) {
                lh
            } else {
                log::warn!(
                    "line_height {lh} out of range [0.8, 2.5]; using default {DEFAULT_LINE_HEIGHT}"
                );
                DEFAULT_LINE_HEIGHT
            }
        })
        .unwrap_or(DEFAULT_LINE_HEIGHT);

    let cell_width = config
        .cell_width
        .map(|cw| {
            if (0.8..=2.0).contains(&cw) {
                cw
            } else {
                log::warn!(
                    "cell_width {cw} out of range [0.8, 2.0]; using default {DEFAULT_CELL_WIDTH}"
                );
                DEFAULT_CELL_WIDTH
            }
        })
        .unwrap_or(DEFAULT_CELL_WIDTH);

    let (font_weight_key, invalid_font_weight) =
        font_weight_key_from_config(config.font_weight.as_deref());
    if invalid_font_weight && let Some(raw) = config.font_weight.as_deref() {
        log::warn!("font_weight '{raw}' is not supported; using default {DEFAULT_FONT_WEIGHT_KEY}");
    }
    let font_weight = font_weight_from_key(font_weight_key);

    let ligatures = config
        .terminal
        .as_ref()
        .and_then(|t| t.ligatures)
        .unwrap_or(false);

    let fallbacks = sanitize_font_fallbacks(config.font_fallbacks.as_ref());

    let font_changed = cache.as_ref().is_none_or(|prev| {
        prev.family != family
            || (prev.settings.size - size).abs() > f32::EPSILON
            || (prev.settings.line_height - line_height).abs() > f32::EPSILON
            || (prev.settings.cell_width - cell_width).abs() > f32::EPSILON
            || prev.font_weight_key != font_weight_key
            || prev.ligatures != ligatures
    });
    if font_changed {
        let size_px = font_points_to_pixels(size);
        log::info!(
            "font: resolved family='{family}' size={size}pt ({:.2}px) line_height={line_height} cell_width={cell_width} font_weight={font_weight_key} ligatures={ligatures}",
            size_px.as_f32()
        );
    }

    let settings = FontSettings {
        font: build_font(&family, font_weight, ligatures, fallbacks),
        size,
        line_height,
        cell_width,
    };
    *cache = Some(CachedFontConfig {
        settings: settings.clone(),
        family,
        font_weight_key,
        ligatures,
        mtime,
        last_check: Instant::now(),
    });
    settings
}

fn build_font(
    family: &str,
    weight: FontWeight,
    ligatures: bool,
    fallbacks: Option<Vec<String>>,
) -> Font {
    let features = if ligatures {
        FontFeatures::default()
    } else {
        FontFeatures::disable_ligatures()
    };
    Font {
        family: SharedString::from(family.to_owned()),
        features,
        fallbacks: fallbacks.map(FontFallbacks::from_fonts),
        weight,
        style: FontStyle::Normal,
    }
}

#[cfg(test)]
pub(crate) fn base_font() -> Font {
    cached_font_config().font
}

pub const MIN_FONT_SIZE: f32 = 8.0;
pub const MAX_FONT_SIZE: f32 = 32.0;

fn font_points_to_pixels(size_points: f32) -> Pixels {
    px(size_points * POINTS_TO_PIXELS)
}

pub fn sanitize_font_override(raw: f32) -> Option<f32> {
    if !raw.is_finite() {
        return None;
    }
    Some(raw.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE))
}

pub fn global_font_size() -> f32 {
    cached_font_config().size
}

pub fn resolve_frame_metrics(
    window: &mut Window,
    _cx: &mut App,
    size_override: Option<f32>,
) -> TerminalFrameMetrics {
    let settings = cached_font_config();
    let font = settings.font;
    let font_size = font_points_to_pixels(size_override.unwrap_or(settings.size));
    let font_id = window.text_system().resolve_font(&font);

    {
        use std::sync::Once;
        static LOG_ONCE: Once = Once::new();
        LOG_ONCE.call_once(|| {
            let resolved = window.text_system().get_font_for_id(font_id);
            match resolved {
                Some(actual) if actual.family == font.family => {
                    log::info!(
                        "font diagnostic: PRIMARY MATCH requested='{}' resolved='{}'",
                        font.family,
                        actual.family,
                    );
                }
                Some(actual) => {
                    log::warn!(
                        "font diagnostic: SILENT FALLBACK requested='{}' resolved='{}' \
                         (GPUI walked fallback_font_stack - primary `font_id` failed)",
                        font.family,
                        actual.family,
                    );
                }
                None => {
                    log::warn!(
                        "font diagnostic: get_font_for_id returned None for resolved \
                         id of requested='{}' (cache mapping anomaly)",
                        font.family,
                    );
                }
            }
        });
    }

    let scale_factor = sanitize_scale_factor(window.scale_factor());
    let metrics = cell_metrics_for(
        window,
        font_id,
        font_size,
        scale_factor,
        settings.line_height,
        settings.cell_width,
    );
    let cell_width = metrics.cell_width_px();
    let line_height = metrics.cell_height_px();

    #[cfg(debug_assertions)]
    super::pixel_probe::record_cell_dimensions(
        px(metrics.face_width / scale_factor),
        cell_width,
        px(metrics.face_height / scale_factor),
        line_height,
        scale_factor,
    );

    TerminalFrameMetrics {
        dimensions: CellDimensions {
            cell_width,
            line_height,
        },
        base_font: font,
        font_size,
        metrics,
    }
}

fn sanitize_scale_factor(raw: f32) -> f32 {
    if raw.is_finite() && raw > 0.0 {
        raw
    } else {
        1.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FaceMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub advance: f32,
    pub underline_position: f32,
    pub underline_thickness: f32,
    pub strikethrough_position: f32,
    pub strikethrough_thickness: f32,
    pub x_height: f32,
    pub cap_height: f32,
}

impl FaceMetrics {
    fn line_height(&self) -> f32 {
        self.ascent + self.descent + self.line_gap
    }

    fn cap_height(&self) -> f32 {
        if self.cap_height > 0.0 {
            self.cap_height
        } else {
            0.75 * self.ascent
        }
    }

    fn x_height(&self) -> f32 {
        if self.x_height > 0.0 {
            self.x_height
        } else {
            0.75 * self.cap_height()
        }
    }

    fn underline_thickness(&self) -> f32 {
        if self.underline_thickness > 0.0 {
            self.underline_thickness
        } else {
            0.15 * self.x_height()
        }
    }

    fn underline_position(&self) -> f32 {
        if self.underline_position != 0.0 {
            self.underline_position
        } else {
            -self.underline_thickness()
        }
    }

    fn strikethrough_thickness(&self) -> f32 {
        if self.strikethrough_thickness > 0.0 {
            self.strikethrough_thickness
        } else {
            self.underline_thickness()
        }
    }

    fn strikethrough_position(&self) -> f32 {
        if self.strikethrough_position != 0.0 {
            self.strikethrough_position
        } else {
            (self.x_height() + self.strikethrough_thickness()) * 0.5
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub scale_factor: f32,
    pub cell_width: i32,
    pub cell_height: i32,
    pub cell_baseline: i32,
    pub underline_position: i32,
    pub underline_thickness: i32,
    pub strikethrough_position: i32,
    pub strikethrough_thickness: i32,
    pub box_thickness: i32,
    pub cursor_thickness: i32,
    pub face_width: f32,
    pub face_height: f32,
    pub face_y: f32,
    pub icon_height: f32,
    pub icon_height_single: f32,
}

impl CellMetrics {
    pub fn logical(&self, device: i32) -> Pixels {
        px(device as f32 / self.scale_factor)
    }

    pub fn cell_width_px(&self) -> Pixels {
        self.logical(self.cell_width)
    }

    pub fn cell_height_px(&self) -> Pixels {
        self.logical(self.cell_height)
    }

    pub fn baseline_px(&self) -> Pixels {
        self.logical(self.cell_height - self.cell_baseline)
    }

    pub fn face_center_dx(&self) -> i32 {
        ((self.cell_width as f32 - self.face_width) / 2.0)
            .round()
            .max(0.0) as i32
    }
}

pub(crate) fn cell_metrics_from_face(
    face: FaceMetrics,
    scale_factor: f32,
    line_height_multiplier: f32,
    cell_width_multiplier: f32,
) -> CellMetrics {
    let face_width = face.advance.max(1.0);
    let face_height = face.line_height().max(1.0);

    let cell_width = (face_width * cell_width_multiplier).round().max(1.0);
    let cell_height = (face_height * line_height_multiplier).round().max(1.0);

    let face_baseline = face.line_gap / 2.0 + face.descent;
    let cell_baseline = (face_baseline + (cell_height - face_height) / 2.0)
        .round()
        .clamp(0.0, cell_height);
    let face_y = cell_baseline - face_baseline;
    let top_to_baseline = cell_height - cell_baseline;

    let underline_thickness = face.underline_thickness().ceil().max(1.0);
    let strikethrough_thickness = face.strikethrough_thickness().ceil().max(1.0);
    let underline_position = (top_to_baseline - face.underline_position()).round();
    let strikethrough_position = (top_to_baseline - face.strikethrough_position()).round();

    let cap_height = face.cap_height();

    CellMetrics {
        scale_factor,
        cell_width: cell_width as i32,
        cell_height: cell_height as i32,
        cell_baseline: cell_baseline as i32,
        underline_position: underline_position as i32,
        underline_thickness: underline_thickness as i32,
        strikethrough_position: strikethrough_position as i32,
        strikethrough_thickness: strikethrough_thickness as i32,
        box_thickness: underline_thickness as i32,
        cursor_thickness: underline_thickness as i32,
        face_width,
        face_height,
        face_y,
        icon_height: face_height,
        icon_height_single: (2.0 * cap_height + face_height) / 3.0,
    }
}

fn measure_face(window: &Window, font_id: FontId, size_device: f32) -> FaceMetrics {
    let text_system = window.text_system();
    let family = text_system.get_font_for_id(font_id).map(|font| font.family);
    if let Some(tables) = family
        .as_deref()
        .and_then(face_tables::embedded_face_tables)
    {
        let scale = size_device / tables.units_per_em.max(1.0);
        return FaceMetrics {
            ascent: tables.ascent * scale,
            descent: tables.descent * scale,
            line_gap: tables.line_gap * scale,
            advance: tables.advance * scale,
            underline_position: tables.underline_position * scale,
            underline_thickness: tables.underline_thickness * scale,
            strikethrough_position: tables.strikethrough_position * scale,
            strikethrough_thickness: tables.strikethrough_thickness * scale,
            x_height: tables.x_height * scale,
            cap_height: tables.cap_height * scale,
        };
    }

    let size = px(size_device);
    let advance = (0x20u32..0x7f)
        .filter_map(char::from_u32)
        .filter_map(|ch| text_system.advance(font_id, size, ch).ok())
        .map(|advance| advance.width.as_f32())
        .fold(0.0, f32::max);
    FaceMetrics {
        ascent: text_system.ascent(font_id, size).as_f32(),
        descent: text_system.descent(font_id, size).as_f32().abs(),
        line_gap: 0.0,
        advance,
        underline_position: 0.0,
        underline_thickness: 0.0,
        strikethrough_position: 0.0,
        strikethrough_thickness: 0.0,
        x_height: text_system.x_height(font_id, size).as_f32(),
        cap_height: text_system.cap_height(font_id, size).as_f32(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CellMetricsKey {
    font_id: FontId,
    font_size: u32,
    scale_factor: u32,
    line_height_multiplier: u32,
    cell_width_multiplier: u32,
}

thread_local! {
    static CELL_METRICS_MEMO: std::cell::Cell<Option<(CellMetricsKey, CellMetrics)>> =
        const { std::cell::Cell::new(None) };
}

fn cell_metrics_for(
    window: &Window,
    font_id: FontId,
    font_size: Pixels,
    scale_factor: f32,
    line_height_multiplier: f32,
    cell_width_multiplier: f32,
) -> CellMetrics {
    let key = CellMetricsKey {
        font_id,
        font_size: font_size.as_f32().to_bits(),
        scale_factor: scale_factor.to_bits(),
        line_height_multiplier: line_height_multiplier.to_bits(),
        cell_width_multiplier: cell_width_multiplier.to_bits(),
    };
    if let Some((cached_key, metrics)) = CELL_METRICS_MEMO.with(std::cell::Cell::get)
        && cached_key == key
    {
        return metrics;
    }
    let face = measure_face(window, font_id, font_size.as_f32() * scale_factor);
    let metrics = cell_metrics_from_face(
        face,
        scale_factor,
        line_height_multiplier,
        cell_width_multiplier,
    );
    log::info!(
        "font: cell {}x{} device px at scale {scale_factor} (face {:.2}x{:.2}, baseline {} from bottom, underline y={} t={})",
        metrics.cell_width,
        metrics.cell_height,
        metrics.face_width,
        metrics.face_height,
        metrics.cell_baseline,
        metrics.underline_position,
        metrics.underline_thickness,
    );
    CELL_METRICS_MEMO.with(|memo| memo.set(Some((key, metrics))));
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_snap_no_op_for_integer_advance() {
        let raw = px(9.0);
        assert_eq!(raw.round(), raw);

        let raw_lh = px(20.0);
        assert_eq!(raw_lh.round(), raw_lh);
    }

    #[test]
    fn round_snap_yields_integer_for_fractional_advance() {
        let raw = px(8.4);
        let snapped = raw.round();
        assert_eq!(snapped, px(8.0));
        assert!(
            snapped.as_f32().fract().abs() < 1e-6,
            "snapped 8.4 should be integer, got {}",
            snapped.as_f32()
        );
    }

    #[test]
    fn sanitize_font_override_drops_non_finite_and_clamps() {
        assert_eq!(sanitize_font_override(f32::NAN), None);
        assert_eq!(sanitize_font_override(f32::INFINITY), None);
        assert_eq!(sanitize_font_override(f32::NEG_INFINITY), None);
        assert_eq!(sanitize_font_override(0.0), Some(MIN_FONT_SIZE));
        assert_eq!(sanitize_font_override(-5.0), Some(MIN_FONT_SIZE));
        assert_eq!(sanitize_font_override(1000.0), Some(MAX_FONT_SIZE));
        assert_eq!(sanitize_font_override(14.0), Some(14.0));
        assert_eq!(sanitize_font_override(8.0), Some(8.0));
        assert_eq!(sanitize_font_override(32.0), Some(32.0));
    }

    #[test]
    fn terminal_font_points_convert_to_logical_pixels() {
        let px_size = font_points_to_pixels(13.0).as_f32();
        assert!(
            (px_size - 17.333334).abs() < 0.00001,
            "13pt should render as 17.333px, got {px_size}"
        );
    }

    #[test]
    fn default_multipliers_keep_the_design_spacing() {
        assert_eq!(DEFAULT_CELL_WIDTH, 1.0);
        assert_eq!(DEFAULT_LINE_HEIGHT, 1.0);
    }

    fn jetbrains_mono(size: f32) -> FaceMetrics {
        let s = size / 1000.0;
        FaceMetrics {
            ascent: 1020.0 * s,
            descent: 300.0 * s,
            line_gap: 0.0,
            advance: 600.0 * s,
            underline_position: -155.0 * s,
            underline_thickness: 50.0 * s,
            strikethrough_position: 320.0 * s,
            strikethrough_thickness: 50.0 * s,
            x_height: 550.0 * s,
            cap_height: 730.0 * s,
        }
    }

    #[test]
    fn cell_metrics_round_the_face_to_whole_device_pixels() {
        let m = cell_metrics_from_face(jetbrains_mono(17.333334), 1.0, 1.0, 1.0);
        assert_eq!((m.cell_width, m.cell_height, m.cell_baseline), (10, 23, 5));
        assert_eq!(m.cell_width_px(), px(10.0));
        assert_eq!(m.cell_height_px(), px(23.0));
        assert_eq!(m.baseline_px(), px(18.0));
        assert_eq!(m.underline_position, 21);
        assert_eq!(m.underline_thickness, 1);
        assert_eq!(m.strikethrough_position, 12);
        assert_eq!(m.box_thickness, 1);

        let m = cell_metrics_from_face(jetbrains_mono(16.0), 1.0, 1.0, 1.0);
        assert_eq!((m.cell_width, m.cell_height, m.cell_baseline), (10, 21, 5));
    }

    #[test]
    fn cell_metrics_convert_device_pixels_back_through_the_scale() {
        let m = cell_metrics_from_face(jetbrains_mono(34.666668), 2.0, 1.0, 1.0);
        assert_eq!((m.cell_width, m.cell_height), (21, 46));
        assert_eq!(m.cell_width_px(), px(10.5));
        assert_eq!(m.cell_height_px(), px(23.0));
        assert_eq!(m.underline_thickness, 2);
        assert_eq!(m.logical(m.underline_thickness), px(1.0));
    }

    #[test]
    fn cell_multipliers_split_the_extra_height_around_the_face() {
        let base = cell_metrics_from_face(jetbrains_mono(16.0), 1.0, 1.0, 1.0);
        let tall = cell_metrics_from_face(jetbrains_mono(16.0), 1.0, 1.5, 1.0);
        assert_eq!(tall.cell_height, 32);
        assert_eq!(tall.cell_baseline - base.cell_baseline, 5);
        let wide = cell_metrics_from_face(jetbrains_mono(16.0), 1.0, 1.0, 1.4);
        assert_eq!(wide.cell_width, 13);
        assert_eq!(wide.face_center_dx(), 2);
        assert_eq!(base.face_center_dx(), 0);
    }

    #[test]
    fn cell_metrics_estimate_missing_tables_and_floor_thicknesses() {
        let face = FaceMetrics {
            ascent: 12.0,
            descent: 3.0,
            line_gap: 0.0,
            advance: 7.0,
            underline_position: 0.0,
            underline_thickness: 0.0,
            strikethrough_position: 0.0,
            strikethrough_thickness: 0.0,
            x_height: 0.0,
            cap_height: 0.0,
        };
        let m = cell_metrics_from_face(face, 1.0, 1.0, 1.0);
        assert_eq!((m.cell_width, m.cell_height, m.cell_baseline), (7, 15, 3));
        assert_eq!(m.underline_thickness, 2);
        assert_eq!(m.underline_position, 13);
        assert_eq!(m.strikethrough_thickness, 2);
        assert!(m.strikethrough_position < 12 && m.strikethrough_position > 4);
        assert_eq!(m.cursor_thickness, 2);
    }

    #[test]
    fn font_weight_key_defaults_to_normal_and_accepts_aliases() {
        assert_eq!(normalize_font_weight_key(None), DEFAULT_FONT_WEIGHT_KEY);
        assert_eq!(normalize_font_weight_key(Some("")), DEFAULT_FONT_WEIGHT_KEY);
        assert_eq!(
            normalize_font_weight_key(Some("unknown")),
            DEFAULT_FONT_WEIGHT_KEY
        );
        assert_eq!(
            normalize_font_weight_key(Some("Extra-Light")),
            "extra_light"
        );
        assert_eq!(normalize_font_weight_key(Some("Semi Light")), "semi_light");
        assert_eq!(normalize_font_weight_key(Some("semibold")), "semi_bold");
        assert_eq!(
            normalize_font_weight_key(Some("Extra Black")),
            "extra_black"
        );
    }

    #[test]
    fn font_weight_mapping_matches_gpui_supported_weights() {
        assert_eq!(font_weight_from_key("thin"), FontWeight::THIN);
        assert_eq!(font_weight_from_key("extra_light"), FontWeight::EXTRA_LIGHT);
        assert_eq!(font_weight_from_key("semi_light"), FontWeight(350.0));
        assert_eq!(font_weight_from_key("normal"), FontWeight::NORMAL);
        assert_eq!(font_weight_from_key("semi_bold"), FontWeight::SEMIBOLD);
        assert_eq!(font_weight_from_key("extra_bold"), FontWeight::EXTRA_BOLD);
        assert_eq!(font_weight_from_key("extra_black"), FontWeight::BLACK);
        assert_eq!(font_weight_from_key("unsupported"), FontWeight::NORMAL);
    }

    #[test]
    fn round_snap_handles_half_away_from_zero() {
        assert_eq!(px(8.5).round(), px(9.0));
        assert_eq!(px(8.6).round(), px(9.0));
        assert_eq!(px(8.49).round(), px(8.0));
    }

    #[test]
    fn scale_factor_falls_back_to_one_when_unusable() {
        assert_eq!(sanitize_scale_factor(0.0), 1.0);
        assert_eq!(sanitize_scale_factor(f32::NAN), 1.0);
        assert_eq!(sanitize_scale_factor(1.5), 1.5);
    }

    #[test]
    fn expand_paneflow_alias_resolves_mono_alias() {
        assert_eq!(expand_paneflow_alias(".PaneflowMono"), EMBEDDED_MONO_FAMILY);
        assert_eq!(
            expand_paneflow_alias(".PaneflowMono"),
            "JetBrainsMono Nerd Font"
        );
        assert_eq!(
            expand_paneflow_alias("JetBrainsMono NF"),
            EMBEDDED_MONO_FAMILY
        );
        assert_eq!(
            expand_paneflow_alias("JetBrainsMono NFM"),
            EMBEDDED_MONO_FAMILY
        );
        assert_eq!(
            expand_paneflow_alias("JetBrainsMono Nerd Font Mono"),
            EMBEDDED_MONO_FAMILY
        );
    }

    #[test]
    fn expand_paneflow_alias_resolves_sans_alias() {
        assert_eq!(expand_paneflow_alias(".PaneflowSans"), EMBEDDED_SANS_FAMILY);
        assert_eq!(expand_paneflow_alias(".PaneflowSans"), "Geist");
    }

    #[test]
    fn expand_paneflow_alias_passes_concrete_names_through() {
        assert_eq!(expand_paneflow_alias("Menlo"), "Menlo");
        assert_eq!(expand_paneflow_alias("Cascadia Mono"), "Cascadia Mono");
        assert_eq!(expand_paneflow_alias("Lilex"), "Lilex");
        assert_eq!(expand_paneflow_alias(""), "");
        assert_eq!(expand_paneflow_alias(".paneflowmono"), ".paneflowmono");
    }

    #[test]
    fn resolve_font_family_default_returns_platform_default() {
        assert_eq!(resolve_font_family(None), default_font_family());
        assert_eq!(resolve_font_family(Some("")), default_font_family());
        assert_eq!(resolve_font_family(Some("   ")), default_font_family());
    }

    #[test]
    fn resolve_font_family_expands_paneflow_aliases() {
        assert_eq!(
            resolve_font_family(Some(".PaneflowMono")),
            EMBEDDED_MONO_FAMILY
        );
        assert_eq!(
            resolve_font_family(Some("JetBrainsMono NF")),
            EMBEDDED_MONO_FAMILY
        );
        assert_eq!(
            resolve_font_family(Some("JetBrainsMono NFM")),
            EMBEDDED_MONO_FAMILY
        );
        assert_eq!(
            resolve_font_family(Some("JetBrainsMono Nerd Font Mono")),
            EMBEDDED_MONO_FAMILY
        );
        assert_eq!(resolve_font_family(Some(".PaneflowSans")), "Geist");
    }

    #[test]
    fn resolve_font_family_short_circuits_embedded_concrete_names() {
        assert_eq!(
            resolve_font_family(Some("JetBrainsMono Nerd Font")),
            "JetBrainsMono Nerd Font"
        );
        assert_eq!(resolve_font_family(Some("Geist Mono")), "Geist Mono");
        assert_eq!(resolve_font_family(Some("Lilex")), "Lilex");
        assert_eq!(resolve_font_family(Some("Geist")), "Geist");
        assert_eq!(resolve_font_family(Some("IBM Plex Sans")), "IBM Plex Sans");
    }

    #[test]
    fn select_default_font_family_uses_bundled_jetbrains_mono_nfm() {
        assert_eq!(
            select_default_font_family(["Menlo", "JetBrainsMono NFM", EMBEDDED_MONO_FAMILY]),
            EMBEDDED_MONO_FAMILY
        );
    }

    #[test]
    fn select_default_font_family_does_not_depend_on_installed_fonts() {
        assert_eq!(
            select_default_font_family(["Menlo", "Cascadia Mono", LEGACY_EMBEDDED_MONO_FAMILY]),
            EMBEDDED_MONO_FAMILY
        );
    }

    #[test]
    fn default_font_family_is_bundled_jetbrains_mono_nfm() {
        assert_eq!(default_font_family(), EMBEDDED_MONO_FAMILY);
    }

    #[test]
    fn sanitize_font_fallbacks_absent_is_none() {
        assert_eq!(sanitize_font_fallbacks(None), None);
    }

    #[test]
    fn sanitize_font_fallbacks_empty_list_is_none() {
        assert_eq!(sanitize_font_fallbacks(Some(&vec![])), None);
    }

    #[test]
    fn sanitize_font_fallbacks_all_blank_is_none() {
        let cfg = vec!["".to_string(), "   ".to_string(), "\t".to_string()];
        assert_eq!(sanitize_font_fallbacks(Some(&cfg)), None);
    }

    #[test]
    fn sanitize_font_fallbacks_trims_and_drops_blanks() {
        let cfg = vec![
            "  FiraCode Nerd Font Mono  ".to_string(),
            "".to_string(),
            "Segoe UI Emoji".to_string(),
        ];
        assert_eq!(
            sanitize_font_fallbacks(Some(&cfg)),
            Some(vec![
                "FiraCode Nerd Font Mono".to_string(),
                "Segoe UI Emoji".to_string(),
            ])
        );
    }

    #[test]
    fn sanitize_font_fallbacks_preserves_order() {
        let cfg = vec!["B".to_string(), "A".to_string(), "B".to_string()];
        assert_eq!(
            sanitize_font_fallbacks(Some(&cfg)),
            Some(vec!["B".to_string(), "A".to_string(), "B".to_string()])
        );
    }
}
