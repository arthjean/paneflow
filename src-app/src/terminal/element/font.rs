#[cfg(target_os = "macos")]
use std::collections::HashSet;
use std::sync::LazyLock;

use gpui::{
    App, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Pixels, SharedString, Window, px,
};

use super::{CellDimensions, TerminalFrameMetrics};

pub(crate) const DEFAULT_FONT_SIZE: f32 = 13.0;
const POINTS_TO_PIXELS: f32 = 96.0 / 72.0;
pub(crate) const DEFAULT_LINE_HEIGHT: f32 = 1.2;
pub(crate) const DEFAULT_CELL_WIDTH: f32 = 0.6;
pub(crate) const DEFAULT_FONT_WEIGHT_KEY: &str = "normal";

pub(crate) const EMBEDDED_MONO_FAMILY: &str = "JetBrainsMono Nerd Font Mono";
pub(crate) const JETBRAINS_MONO_NFM_ALIAS: &str = "JetBrainsMono NFM";
pub(crate) const LEGACY_GEIST_MONO_FAMILY: &str = "Geist Mono";
pub(crate) const LEGACY_EMBEDDED_MONO_FAMILY: &str = "Lilex";

pub(crate) const EMBEDDED_SANS_FAMILY: &str = "Geist";

pub(crate) const PANEFLOW_MONO_ALIAS: &str = ".PaneflowMono";
pub(crate) const PANEFLOW_SANS_ALIAS: &str = ".PaneflowSans";

fn expand_paneflow_alias(name: &str) -> &str {
    match name {
        PANEFLOW_MONO_ALIAS | JETBRAINS_MONO_NFM_ALIAS => EMBEDDED_MONO_FAMILY,
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
            if (1.0..=2.5).contains(&lh) {
                lh
            } else {
                log::warn!(
                    "line_height {lh} out of range [1.0, 2.5]; using default {DEFAULT_LINE_HEIGHT}"
                );
                DEFAULT_LINE_HEIGHT
            }
        })
        .unwrap_or(DEFAULT_LINE_HEIGHT);

    let cell_width = config
        .cell_width
        .map(|cw| {
            if (0.3..=2.0).contains(&cw) {
                cw
            } else {
                log::warn!(
                    "cell_width {cw} out of range [0.3, 2.0]; using default {DEFAULT_CELL_WIDTH}"
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

    let cell_width_raw = px(font_size.as_f32() * settings.cell_width);
    let line_height_raw = px(font_size.as_f32() * settings.line_height);

    let cell_width = cell_width_raw.round();
    let line_height = line_height_raw.round();

    #[cfg(debug_assertions)]
    super::pixel_probe::record_cell_dimensions(
        cell_width_raw,
        cell_width,
        line_height_raw,
        line_height,
        window.scale_factor(),
    );

    TerminalFrameMetrics {
        dimensions: CellDimensions {
            cell_width,
            line_height,
        },
        base_font: font,
        font_size,
    }
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
    fn default_cell_width_matches_windows_terminal_multiplier() {
        let raw = font_points_to_pixels(DEFAULT_FONT_SIZE) * DEFAULT_CELL_WIDTH;
        assert!(
            (raw.as_f32() - 10.4).abs() < 0.00001,
            "13pt x 0.6 should be 10.4px, got {}",
            raw.as_f32()
        );
        assert_eq!(raw.round(), px(10.0));
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
    fn round_snap_yields_integer_for_fractional_line_height() {
        let raw_lh = font_points_to_pixels(DEFAULT_FONT_SIZE) * DEFAULT_LINE_HEIGHT;
        let snapped = raw_lh.round();
        assert_eq!(snapped, px(21.0));
        assert!(snapped.as_f32().fract().abs() < 1e-6);
    }

    #[test]
    fn expand_paneflow_alias_resolves_mono_alias() {
        assert_eq!(expand_paneflow_alias(".PaneflowMono"), EMBEDDED_MONO_FAMILY);
        assert_eq!(
            expand_paneflow_alias(".PaneflowMono"),
            "JetBrainsMono Nerd Font Mono"
        );
        assert_eq!(
            expand_paneflow_alias("JetBrainsMono NFM"),
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
            "JetBrainsMono Nerd Font Mono"
        );
        assert_eq!(
            resolve_font_family(Some("JetBrainsMono NFM")),
            "JetBrainsMono Nerd Font Mono"
        );
        assert_eq!(resolve_font_family(Some(".PaneflowSans")), "Geist");
    }

    #[test]
    fn resolve_font_family_short_circuits_embedded_concrete_names() {
        assert_eq!(
            resolve_font_family(Some("JetBrainsMono Nerd Font Mono")),
            "JetBrainsMono Nerd Font Mono"
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
