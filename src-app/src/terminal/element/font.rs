//! Font resolution + cell measurement for the terminal renderer.
//!
//! Owns the embedded-font primary contract, the installed-monospace-font
//! registry (cross-platform), and the cached font config read from
//! `paneflow.json`. Exposes `resolve_frame_metrics` to turn the current font
//! config + size into a per-frame font snapshot and terminal cell strides.
//!
//! Extracted from `terminal_element.rs` per US-008 of the src-app refactor PRD.

#[cfg(target_os = "macos")]
use std::collections::HashSet;
use std::sync::LazyLock;

use gpui::{
    App, Font, FontFallbacks, FontFeatures, FontId, FontStyle, FontWeight, Pixels, SharedString,
    Window, px,
};

use super::face_tables;
use super::{CellDimensions, TerminalFrameMetrics};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub(crate) const DEFAULT_FONT_SIZE: f32 = 13.0;
const POINTS_TO_PIXELS: f32 = 96.0 / 72.0;
/// Multiplier of the face's own line height (`ascent + descent + line gap`),
/// Ghostty's `adjust-cell-height` expressed as a factor. `1.0` is the font's
/// design spacing.
pub(crate) const DEFAULT_LINE_HEIGHT: f32 = 1.0;
/// Multiplier of the face's widest ASCII advance. `1.0` is the font's design
/// spacing.
pub(crate) const DEFAULT_CELL_WIDTH: f32 = 1.0;
pub(crate) const DEFAULT_FONT_WEIGHT_KEY: &str = "normal";

/// Embedded monospace family - the bundled cross-platform default. Files:
/// `assets/fonts/JetBrainsMonoNerdFont-{Regular,Medium,SemiBold,Bold}{,Italic}.ttf`,
/// registered with GPUI at startup (`main.rs` → `Assets::load_fonts` →
/// `cx.text_system().add_fonts`).
///
/// Picking an *embedded* family as the primary instead of a system family
/// (Menlo / Cascadia Mono / DejaVu) sidesteps the failure mode behind commit
/// c3e2331: Core Text inside a signed .app bundle can return valid glyph_ids
/// for a system family while rasterizing them as empty bitmaps, and GPUI's
/// per-`Font` fallback chain only walks on missing-glyph not on empty-raster -
/// so the system primary "renders" zero glyphs and nothing falls through. With
/// an embedded family as primary, GPUI's text system owns the font tables
/// end-to-end and rasterization always works.
pub(crate) const EMBEDDED_MONO_FAMILY: &str = "JetBrainsMono Nerd Font";
/// Short name Nerd Fonts write in name ID 1 of the same files.
pub(crate) const JETBRAINS_MONO_NF_ALIAS: &str = "JetBrainsMono NF";
/// The `Mono` variant Paneflow bundled before the icons were left at their
/// designed size and constrained by the renderer instead (Ghostty's model).
/// Configs written against it keep resolving to the bundled family.
pub(crate) const LEGACY_JETBRAINS_MONO_NFM_FAMILY: &str = "JetBrainsMono Nerd Font Mono";
pub(crate) const JETBRAINS_MONO_NFM_ALIAS: &str = "JetBrainsMono NFM";
pub(crate) const LEGACY_GEIST_MONO_FAMILY: &str = "Geist Mono";
pub(crate) const LEGACY_EMBEDDED_MONO_FAMILY: &str = "Lilex";

/// Embedded UI/sans family. Files:
/// `assets/fonts/Geist-{Regular,Medium,SemiBold,Bold}{,Italic}.ttf`.
/// Used as Paneflow's primary UI family and as the `.PaneflowSans`
/// config alias target. The terminal stays mono by default; this
/// sans target exists for explicit user config and GPUI fallback.
pub(crate) const EMBEDDED_SANS_FAMILY: &str = "Geist";

/// Paneflow-side virtual font aliases. Mirror Zed's `.ZedMono` /
/// `.ZedSans` pattern from `crates/gpui/src/text_system.rs:1167-1173`,
/// but expanded at the Paneflow boundary (in `resolve_font_family`)
/// before the family name reaches GPUI - GPUI's pinned rev does not
/// know about Paneflow-specific aliases.
///
/// Users can write an alias (`".PaneflowMono"`, `"JetBrainsMono NF"`, the legacy `"JetBrainsMono NFM"`, or
/// `".PaneflowSans"`) or a concrete embedded family in `paneflow.json`.
/// Keeping the alias available lets a future swap
/// of the bundled fallback happen with a single edit to this constant
/// table instead of a config migration for every user.
pub(crate) const PANEFLOW_MONO_ALIAS: &str = ".PaneflowMono";
pub(crate) const PANEFLOW_SANS_ALIAS: &str = ".PaneflowSans";

/// Resolve a Paneflow-virtual alias to its concrete embedded family.
/// Returns the input unchanged when it isn't an alias. Pure function,
/// no I/O, used by `resolve_font_family` to expand aliases at the
/// Paneflow boundary before the family name reaches GPUI.
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

// Per-`Font` `fallbacks: Some(...)` was REMOVED on purpose. Paneflow
// previously attached a hardcoded chain (Noto Color Emoji, Symbols
// Nerd Font Mono, embedded sans, embedded mono) that, on macOS, was
// the trigger for the v0.2.12 "boxes drawn, no glyphs" bug:
// `apply_features_and_fallbacks` (gpui_macos/src/open_type.rs:30-73)
// rebuilds every CTFont with a Core Text cascade list assembled from
// `CTFontDescriptorCreateWithNameAndSize` for each fallback name.
// Two entries in the old chain - Noto Color Emoji and Symbols Nerd
// Font Mono - are NOT installed on a fresh macOS, and the resulting
// cascade list, while accepted by Core Text without erroring, ended
// up suppressing rasterization of the primary face. Icons rendered
// (different code path, walking GPUI's internal `fallback_font_stack`
// at gpui/src/text_system.rs:71-83) but text glyphs did not.
//
// Zed's terminal uses `fallbacks: None` by default
// (zed/crates/terminal_view/src/terminal_element.rs:908-912). It only
// wraps `Some(...)` when the user explicitly configures
// `terminal.font_fallbacks` in their settings. Paneflow mirrors that
// pattern: `base_font` emits `Some(FontFallbacks)` ONLY when the user sets
// the top-level `font_fallbacks` array in `paneflow.json` (e.g. a Nerd
// Font for Starship / oh-my-posh / Terminal-Icons glyphs that no Windows
// system font carries), and `None` otherwise - never a hardcoded chain.
//
// Glyph fallback for codepoints the primary font doesn't cover (emoji, CJK,
// symbols) still works: GPUI walks its built-in `fallback_font_stack`
// - which already ships `.ZedMono` (resolves to Lilex, which we still
// embed), `.ZedSans` (resolves to IBM Plex Sans, which we historically embed),
// then OS-canonical sans like Helvetica / Segoe UI / Arial. That
// chain is global, not per-`Font`, so it does NOT pollute the
// per-Font CTFont cascade list.

/// Registry of installed monospace families (Core Text), used ONLY on macOS to
/// validate a configured `font_family` against the documented c3e2331
/// empty-raster failure mode. Populated lazily on first access.
///
/// macOS-only by design: on Linux the equivalent `fc-list :spacing=mono`
/// validation wrongly rejected real monospace fonts that fontconfig didn't tag
/// (patched Nerd Fonts) and forked `fc-list` on the first terminal layout; on
/// Windows the registry was always empty (no enumeration). `resolve_font_family`
/// therefore trusts the configured family on those platforms.
#[cfg(target_os = "macos")]
static INSTALLED_MONO_FONTS: LazyLock<HashSet<String>> =
    LazyLock::new(|| crate::fonts::load_mono_fonts().into_iter().collect());

// ---------------------------------------------------------------------------
// Font config cache - avoids load_config() on every base_font()/font_size() call
// ---------------------------------------------------------------------------

struct CachedFontConfig {
    settings: FontSettings,
    family: String,
    font_weight_key: &'static str,
    /// US-008: render ligatures when `true`. Hot-reload comes for free
    /// via the surrounding 500 ms cache: editing `paneflow.json` is
    /// picked up on the next `cached_font_config()` call without any
    /// extra wiring.
    ligatures: bool,
    /// Modification time of the config file the settings were read from,
    /// so the periodic check is a `stat` rather than a parse.
    mtime: Option<std::time::SystemTime>,
    last_check: std::time::Instant,
}

/// The resolved font settings the renderer reads for every pane on every
/// frame.
///
/// Cheap to clone: the `Font` holds shared strings and shared feature lists,
/// so a clone is a few reference-count bumps and no allocation.
#[derive(Clone)]
pub(super) struct FontSettings {
    pub(super) font: Font,
    /// Points, before any per-pane override.
    pub(super) size: f32,
    /// Multiplier of the rendered size that gives the row stride.
    pub(super) line_height: f32,
    /// Multiplier of the rendered size that gives the column stride.
    pub(super) cell_width: f32,
}

/// Normalize a configured `font_fallbacks` list before it reaches GPUI:
/// trim each entry, drop empties, and collapse an absent / all-empty list to
/// `None` so [`base_font`] emits `fallbacks: None` (GPUI's built-in stack
/// only) rather than an empty `FontFallbacks`. Pure - unit-tested.
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

/// The default monospace family PaneFlow uses out of the box.
///
/// Uses bundled JetBrainsMono Nerd Font so fresh installs are visually
/// consistent across Linux, macOS, and Windows while avoiding the Core Text
/// empty-raster failure documented by commit c3e2331.
///
/// Users can still override with any system font via
/// `paneflow.json#font_family` - `resolve_font_family` validates the
/// override against the installed-mono registry (when populated) and
/// degrades back to this default with a warning otherwise.
pub(crate) fn default_font_family() -> &'static str {
    *DEFAULT_MONO_FAMILY
}

pub fn resolve_font_family(configured: Option<&str>) -> String {
    let candidate = configured
        .map(str::trim)
        .filter(|family| !family.is_empty())
        .map(expand_paneflow_alias)
        .unwrap_or(default_font_family());

    // Embedded families are always resolvable: Assets::load_fonts
    // registers them directly with GPUI's text system at boot,
    // bypassing the OS font enumeration registry. Short-circuit before
    // the INSTALLED_MONO_FONTS lookup, which only sees system fonts.
    // Lilex and IBM Plex Mono are also embedded and remain valid explicit
    // choices. Installed families flow through normal system-font resolution.
    if candidate == EMBEDDED_MONO_FAMILY
        || candidate == LEGACY_GEIST_MONO_FAMILY
        || candidate == LEGACY_EMBEDDED_MONO_FAMILY
        || candidate == EMBEDDED_SANS_FAMILY
        || candidate == "IBM Plex Sans"
        || candidate == "IBM Plex Mono"
    {
        return candidate.to_string();
    }

    // The installed-monospace validation guards a macOS-specific Core Text
    // failure mode (a system family that resolves but rasterizes empty - commit
    // c3e2331), so it is gated to macOS. On Linux it wrongly rejected real
    // monospace fonts fontconfig didn't tag `:spacing=mono` (patched Nerd
    // Fonts) AND ran `fc-list` on the first terminal layout; on Windows the
    // registry was always empty. Elsewhere we trust the configured family -
    // GPUI's text system resolves it, and an unresolvable name already falls
    // through to the embedded fallback stack.
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

/// Read font config, cached and re-validated against the config file's
/// modification time every 500 ms (same pattern as the theme cache).
///
/// Runs on the render thread, so the periodic check is one `stat`; the
/// config is parsed again only when the file changed or is missing.
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

    // Read before the parse, so a write that lands between the two is seen
    // by the next check instead of being masked by a newer stamp.
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

    // US-008: ligatures are off by default to preserve the historical
    // monospaced cell-stride behavior. Opt-in via `terminal.ligatures: true`
    // in `paneflow.json`. Both `terminal == None` and
    // `terminal.ligatures == None` keep ligatures disabled.
    let ligatures = config
        .terminal
        .as_ref()
        .and_then(|t| t.ligatures)
        .unwrap_or(false);

    // User-configured fallback families (Nerd Font for icon glyphs, …),
    // sanitized to `None` when absent/all-empty so the font keeps GPUI's
    // built-in stack in that case.
    let fallbacks = sanitize_font_fallbacks(config.font_fallbacks.as_ref());

    // Diagnostic: log the effective resolved family the first time we
    // populate the cache, and on every subsequent change. This makes it
    // possible to confirm from `RUST_LOG=info` whether the embedded
    // fallback was selected (e.g. on a macOS install where Core Text
    // failed to surface `Menlo` from inside a signed .app bundle) without
    // adding a hot-path log on every render.
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
    // US-008: when the user opts into ligatures, hand GPUI the font's
    // native feature set untouched. Default behavior (and explicit
    // `ligatures: false`) keeps the historical `disable_ligatures()`
    // call so a Paneflow upgrade never silently flips ligatures on.
    let features = if ligatures {
        FontFeatures::default()
    } else {
        FontFeatures::disable_ligatures()
    };
    Font {
        family: SharedString::from(family.to_owned()),
        features,
        // `None` matches Zed's terminal Font default
        // (zed/crates/terminal_view/src/terminal_element.rs:908-912) and is
        // kept unless the user opts in via the top-level `font_fallbacks`
        // array (already sanitized to non-empty-or-`None` by
        // `cached_font_config`). See the long-form rationale on the removed
        // `FONT_FALLBACKS` static above for why we never hardcode a chain.
        fallbacks: fallbacks.map(FontFallbacks::from_fonts),
        weight,
        style: FontStyle::Normal,
    }
}

/// The base terminal font, resolved once per config change and shared. The
/// renderer reads it through `resolve_frame_metrics`; the benchmark reads it
/// alone.
#[cfg(test)]
pub(crate) fn base_font() -> Font {
    cached_font_config().font
}

/// EP-006 US-019: bounds shared by the global config validation, the
/// per-pane zoom steps, and the session-restore ingress.
pub const MIN_FONT_SIZE: f32 = 8.0;
pub const MAX_FONT_SIZE: f32 = 32.0;

fn font_points_to_pixels(size_points: f32) -> Pixels {
    px(size_points * POINTS_TO_PIXELS)
}

/// EP-006 US-019: validate a `font_size` read back from session.json
/// (UNTRUSTED-adjacent: local-only but validated anyway, US-057/EP-010
/// invariant). NaN/±inf are DROPPED (`None` - they would poison the cell
/// geometry); finite out-of-range values are clamped. Pure - unit-tested.
pub fn sanitize_font_override(raw: f32) -> Option<f32> {
    if !raw.is_finite() {
        return None;
    }
    Some(raw.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE))
}

/// EP-006 US-019: the global (non-overridden) font size in points - the zoom
/// handlers' baseline for a pane that has no override yet.
pub fn global_font_size() -> f32 {
    cached_font_config().size
}

pub fn resolve_frame_metrics(
    window: &mut Window,
    _cx: &mut App,
    size_override: Option<f32>,
) -> TerminalFrameMetrics {
    // One cache read per frame. Config and per-pane overrides are stored in
    // points to match OS terminal settings; GPUI expects logical pixels. The
    // override (EP-006 US-019's per-pane zoom) is already clamped to
    // [8.0, 32.0] at every write site, so no re-validation here.
    let settings = cached_font_config();
    let font = settings.font;
    let font_size = font_points_to_pixels(size_override.unwrap_or(settings.size));
    let font_id = window.text_system().resolve_font(&font);

    // DIAGNOSTIC A - fires once per process. Surfaces whether GPUI's
    // `resolve_font` actually loaded the requested family or
    // silently fell back to the `fallback_font_stack`
    // (gpui/src/text_system.rs:148-160). The Paneflow log line
    // `font: resolved family='...'` reflects only what Paneflow
    // requested as input - it is NOT proof that GPUI returned a
    // FontId pointing at that family. If `get_font_for_id` returns a
    // different family, GPUI silently fell through to a system font
    // that may not rasterize correctly inside a signed .app on
    // macOS. Tied to the v0.2.12 "boxes drawn, no glyphs" bug.
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

    // The grid is measured on the face, in device pixels, the way Ghostty's
    // `Metrics.calc` does it: the widest ASCII advance and the face's own
    // line height, each rounded to whole device pixels, with the baseline
    // rounded to a pixel row. The config multipliers scale those measured
    // strides (Ghostty's `adjust-cell-width` / `adjust-cell-height`).
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

    // PANEFLOW_PIXEL_PROBE: record the unrounded face strides next to the
    // integer cell so a future investigation can tell at a glance how much
    // the rounding moved the grid. Origin is logged separately from `paint()`
    // via `record_origin`.
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

// ---------------------------------------------------------------------------
// Cell metrics (Ghostty `src/font/Metrics.zig`)
// ---------------------------------------------------------------------------

/// Unrounded measurements of the primary face at the rendered size, in device
/// pixels. Zero means "the font did not say", and the accessor falls back to
/// the same estimators Ghostty's `FaceMetrics` uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FaceMetrics {
    pub ascent: f32,
    /// Positive magnitude below the baseline.
    pub descent: f32,
    pub line_gap: f32,
    /// Widest advance among the printable ASCII glyphs.
    pub advance: f32,
    /// Top of the underline relative to the baseline, negative below it.
    pub underline_position: f32,
    pub underline_thickness: f32,
    /// Top of the strikeout stroke above the baseline.
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

/// The integer device-pixel grid every paint pass draws on, plus the
/// unrounded face measurements the icon constraint needs. Every `i32` is a
/// count of device pixels; `*_px()` converts back to GPUI logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub scale_factor: f32,
    pub cell_width: i32,
    pub cell_height: i32,
    /// Distance from the cell bottom to the baseline.
    pub cell_baseline: i32,
    /// Distance from the cell top to the top of the underline.
    pub underline_position: i32,
    pub underline_thickness: i32,
    /// Distance from the cell top to the top of the strikethrough.
    pub strikethrough_position: i32,
    pub strikethrough_thickness: i32,
    pub box_thickness: i32,
    pub cursor_thickness: i32,
    /// Unrounded advance, for centering the face in a wider cell.
    pub face_width: f32,
    /// Unrounded line height, for the icon constraint.
    pub face_height: f32,
    /// Offset from the cell bottom to the bottom of the face box.
    pub face_y: f32,
    /// Icon constraint height across two cells.
    pub icon_height: f32,
    /// Icon constraint height inside a single cell.
    pub icon_height_single: f32,
}

impl CellMetrics {
    /// Logical pixels for a device-pixel count.
    pub fn logical(&self, device: i32) -> Pixels {
        px(device as f32 / self.scale_factor)
    }

    pub fn cell_width_px(&self) -> Pixels {
        self.logical(self.cell_width)
    }

    pub fn cell_height_px(&self) -> Pixels {
        self.logical(self.cell_height)
    }

    /// Distance from the cell top to the baseline, in logical pixels.
    pub fn baseline_px(&self) -> Pixels {
        self.logical(self.cell_height - self.cell_baseline)
    }

    /// Horizontal offset that centers the face in a cell wider than its
    /// advance, rounded to a whole device pixel so hinting survives
    /// (Ghostty `freetype.zig:503-512`). Zero when the cell is the advance.
    pub fn face_center_dx(&self) -> i32 {
        ((self.cell_width as f32 - self.face_width) / 2.0)
            .round()
            .max(0.0) as i32
    }
}

/// Port of Ghostty's `Metrics.calc`: round the face strides to whole device
/// pixels, center the face vertically in the rounded cell, and derive every
/// decoration from the font tables with a one-pixel floor.
pub(crate) fn cell_metrics_from_face(
    face: FaceMetrics,
    scale_factor: f32,
    line_height_multiplier: f32,
    cell_width_multiplier: f32,
) -> CellMetrics {
    let face_width = face.advance.max(1.0);
    let face_height = face.line_height().max(1.0);

    // `round` rather than `ceil`: at most half a pixel from the design width,
    // so the apparent spacing matches between low and high DPI. A glyph with
    // no side bearing may touch the next cell by a pixel, which is what such
    // glyphs are drawn to do anyway.
    let cell_width = (face_width * cell_width_multiplier).round().max(1.0);
    let cell_height = (face_height * line_height_multiplier).round().max(1.0);

    // Half the line gap above the face and half below, so text never bumps
    // against either edge of the cell.
    let face_baseline = face.line_gap / 2.0 + face.descent;
    // Center the face in the rounded (and possibly adjusted) cell: the extra
    // or missing height is split evenly above and below.
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
        // Same heuristic as the Nerd Fonts patcher.
        icon_height_single: (2.0 * cap_height + face_height) / 3.0,
    }
}

/// Measure the resolved face at `font_size × scale_factor`. Embedded faces
/// are read from their tables; a system font goes through GPUI's accessors,
/// which carry no line gap or underline metrics, so those use the estimators.
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
        // GPUI stores the descent with a platform-dependent sign.
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

/// Exact-bits key of everything `cell_metrics_for` reads.
#[derive(Clone, Copy, PartialEq, Eq)]
struct CellMetricsKey {
    font_id: FontId,
    font_size: u32,
    scale_factor: u32,
    line_height_multiplier: u32,
    cell_width_multiplier: u32,
}

thread_local! {
    /// One entry: every pane of a window shares the font, and the key only
    /// changes on a config edit, a zoom step, or a monitor move.
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
        // When the font system already returns an integer advance (the case
        // observed in `debug_block_char_rendering.md`: cell_width=9.0), the
        // snap must be a no-op so US-002 introduces no regression on
        // already-aligned environments.
        let raw = px(9.0);
        assert_eq!(raw.round(), raw);

        let raw_lh = px(20.0);
        assert_eq!(raw_lh.round(), raw_lh);
    }

    #[test]
    fn round_snap_yields_integer_for_fractional_advance() {
        // 8.4 px advance is the canonical fractional case from the PRD
        // (DejaVu Sans Mono at 14 pt @ 1.0 DPI on Linux).
        let raw = px(8.4);
        let snapped = raw.round();
        assert_eq!(snapped, px(8.0));
        assert!(
            snapped.as_f32().fract().abs() < 1e-6,
            "snapped 8.4 should be integer, got {}",
            snapped.as_f32()
        );
    }

    // EP-006 US-019: session.json ingress for the per-pane zoom - NaN/inf
    // dropped, finite values clamped to [8.0, 32.0] (PRD AC + test).
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

    /// JetBrains Mono at 13 pt (17.33 px): 0.6 em advance and 1.32 em face.
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
        // 17.33 px: advance 10.4 -> 10, face 22.88 -> 23, baseline 5 from
        // the bottom (descent 5.2 + half of the 0.12 px rounding slack).
        let m = cell_metrics_from_face(jetbrains_mono(17.333334), 1.0, 1.0, 1.0);
        assert_eq!((m.cell_width, m.cell_height, m.cell_baseline), (10, 23, 5));
        assert_eq!(m.cell_width_px(), px(10.0));
        assert_eq!(m.cell_height_px(), px(23.0));
        assert_eq!(m.baseline_px(), px(18.0));
        // post table: top of the underline 2.7 px below the baseline.
        assert_eq!(m.underline_position, 21);
        assert_eq!(m.underline_thickness, 1);
        // OS/2: top of the strikeout 5.5 px above the baseline.
        assert_eq!(m.strikethrough_position, 12);
        assert_eq!(m.box_thickness, 1);

        // 16 px (Ghostty's Linux default): the same 10x21 grid.
        let m = cell_metrics_from_face(jetbrains_mono(16.0), 1.0, 1.0, 1.0);
        assert_eq!((m.cell_width, m.cell_height, m.cell_baseline), (10, 21, 5));
    }

    #[test]
    fn cell_metrics_convert_device_pixels_back_through_the_scale() {
        let m = cell_metrics_from_face(jetbrains_mono(34.666668), 2.0, 1.0, 1.0);
        assert_eq!((m.cell_width, m.cell_height), (21, 46));
        assert_eq!(m.cell_width_px(), px(10.5));
        assert_eq!(m.cell_height_px(), px(23.0));
        // 0.05 em at 34.67 px = 1.73 -> 2 device px = 1 logical px.
        assert_eq!(m.underline_thickness, 2);
        assert_eq!(m.logical(m.underline_thickness), px(1.0));
    }

    #[test]
    fn cell_multipliers_split_the_extra_height_around_the_face() {
        let base = cell_metrics_from_face(jetbrains_mono(16.0), 1.0, 1.0, 1.0);
        let tall = cell_metrics_from_face(jetbrains_mono(16.0), 1.0, 1.5, 1.0);
        assert_eq!(tall.cell_height, 32);
        // 11 extra rows: 5 below the baseline, 6 above.
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
        // cap 9, ex 6.75, underline 1.01 -> 2 px, placed one thickness below.
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
        // Rust's f32::round documents round-half-away-from-zero. Lock in
        // that behavior so a future `.round()` → `.round_ties_even()` swap
        // would surface here instead of as a silent layout shift.
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

    // ─── Paneflow virtual-alias resolution ────────────────────────────
    // Lock in the contract that `.PaneflowMono` and `.PaneflowSans`
    // resolve to the embedded family names BEFORE leaving Paneflow.
    // GPUI's pinned rev does not know these aliases - a regression
    // here would surface as "embedded font registered but never
    // selected because GPUI sees the literal alias string".

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
        // Names from configs written against the previously bundled `Mono`
        // variant keep resolving to the bundled family.
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
        // System fonts and any non-alias string round-trip unchanged.
        // Critical for `resolve_font_family` correctness: the alias
        // expansion must not eat user-configured system fonts.
        assert_eq!(expand_paneflow_alias("Menlo"), "Menlo");
        assert_eq!(expand_paneflow_alias("Cascadia Mono"), "Cascadia Mono");
        assert_eq!(expand_paneflow_alias("Lilex"), "Lilex");
        assert_eq!(expand_paneflow_alias(""), "");
        // Case-sensitive: `.paneflowmono` is not `.PaneflowMono`.
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
        // Both aliases must resolve through to their embedded targets
        // - the value GPUI's `text_system().resolve_font` will look
        // up against the registered TTFs.
        assert_eq!(
            resolve_font_family(Some(".PaneflowMono")),
            EMBEDDED_MONO_FAMILY
        );
        assert_eq!(
            resolve_font_family(Some("JetBrainsMono NF")),
            EMBEDDED_MONO_FAMILY
        );
        // Legacy names from before the swap to the non-Mono Nerd Font.
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
        // Users who write the canonical JetBrainsMono name, `"Geist Mono"`,
        // `"Lilex"`, `"Geist"`, or `"IBM Plex Sans"` in
        // paneflow.json get the embedded font even on platforms whose
        // INSTALLED_MONO_FONTS registry doesn't list them (Windows
        // pre-DirectWrite, container without fontconfig). The short
        // circuit before the registry lookup is what makes that work.
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

    // ─── font_fallbacks sanitization ─────────────────────────────────
    // The wiring that lets a user keep a bundled primary while adding
    // a Nerd Font fallback for Starship / oh-my-posh icons. The sanitizer
    // must collapse absent/all-empty lists to `None` so `base_font` emits
    // `fallbacks: None` (GPUI's built-in stack) rather than an empty
    // `FontFallbacks`, and must trim + drop blank entries.

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
        // Fallback order is significant - GPUI consults entries in order,
        // so the sanitizer must never reorder or dedupe.
        let cfg = vec!["B".to_string(), "A".to_string(), "B".to_string()];
        assert_eq!(
            sanitize_font_fallbacks(Some(&cfg)),
            Some(vec!["B".to_string(), "A".to_string(), "B".to_string()])
        );
    }
}
