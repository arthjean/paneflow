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
    App, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Pixels, SharedString, Window, px,
};

use super::{CellDimensions, TerminalFrameMetrics};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub(crate) const DEFAULT_FONT_SIZE: f32 = 13.0;
const POINTS_TO_PIXELS: f32 = 96.0 / 72.0;
pub(crate) const DEFAULT_LINE_HEIGHT: f32 = 1.2;
pub(crate) const DEFAULT_CELL_WIDTH: f32 = 0.6;
pub(crate) const DEFAULT_FONT_WEIGHT_KEY: &str = "normal";

/// Embedded monospace family - the bundled cross-platform default. Files:
/// `assets/fonts/GeistMono-{Regular,Medium,SemiBold,Bold}{,Italic}.ttf`,
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
pub(crate) const EMBEDDED_MONO_FAMILY: &str = "Geist Mono";
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
/// Users can write either an alias (`".PaneflowMono"` / `".PaneflowSans"`) or
/// a concrete embedded family (`"Geist Mono"` / `"Geist"`) in `paneflow.json`.
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
        PANEFLOW_MONO_ALIAS => EMBEDDED_MONO_FAMILY,
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
// Glyph fallback for codepoints Geist Mono doesn't cover (emoji, CJK,
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
    family: String,
    size: f32,
    line_height: f32,
    cell_width: f32,
    font_weight_key: &'static str,
    font_weight: FontWeight,
    /// US-008: render ligatures when `true`. Hot-reload comes for free
    /// via the surrounding 500 ms cache: editing `paneflow.json` is
    /// picked up on the next `cached_font_config()` call without any
    /// extra wiring.
    ligatures: bool,
    /// Sanitized user `font_fallbacks` (trimmed, empties dropped, `None`
    /// when absent/all-empty). Hot-reloaded via the same 500 ms cache as
    /// `family`/`size`/`ligatures`.
    fallbacks: Option<Vec<String>>,
    last_check: std::time::Instant,
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
/// Uses bundled Geist Mono so fresh installs are visually consistent across
/// Linux, macOS, and Windows while avoiding the Core Text empty-raster failure
/// documented by commit c3e2331.
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

/// Read font config, cached for 500ms (same pattern as theme cache).
pub(super) fn cached_font_config() -> (String, f32, f32, f32, FontWeight, bool, Option<Vec<String>>)
{
    use std::time::{Duration, Instant};
    const CHECK_INTERVAL: Duration = Duration::from_millis(500);

    let mut cache = FONT_CONFIG_CACHE.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(ref c) = *cache
        && c.last_check.elapsed() < CHECK_INTERVAL
    {
        return (
            c.family.clone(),
            c.size,
            c.line_height,
            c.cell_width,
            c.font_weight,
            c.ligatures,
            c.fallbacks.clone(),
        );
    }

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
    // sanitized to `None` when absent/all-empty so `base_font` keeps GPUI's
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
            || (prev.size - size).abs() > f32::EPSILON
            || (prev.line_height - line_height).abs() > f32::EPSILON
            || (prev.cell_width - cell_width).abs() > f32::EPSILON
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

    *cache = Some(CachedFontConfig {
        family: family.clone(),
        size,
        line_height,
        cell_width,
        font_weight_key,
        font_weight,
        ligatures,
        fallbacks: fallbacks.clone(),
        last_check: Instant::now(),
    });

    (
        family,
        size,
        line_height,
        cell_width,
        font_weight,
        ligatures,
        fallbacks,
    )
}

pub(super) fn base_font() -> Font {
    let (family, _, _, _, font_weight, ligatures, fallbacks) = cached_font_config();
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
        family: SharedString::from(family),
        features,
        // `None` matches Zed's terminal Font default
        // (zed/crates/terminal_view/src/terminal_element.rs:908-912) and is
        // kept unless the user opts in via the top-level `font_fallbacks`
        // array (already sanitized to non-empty-or-`None` by
        // `cached_font_config`). See the long-form rationale on the removed
        // `FONT_FALLBACKS` static above for why we never hardcode a chain.
        fallbacks: fallbacks.map(FontFallbacks::from_fonts),
        weight: font_weight,
        style: FontStyle::Normal,
    }
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

/// Effective terminal font size. Config and per-pane overrides are stored in
/// points to match OS terminal settings; GPUI expects logical pixels.
/// `size_override` is EP-006 US-019's per-pane zoom: `Some(pt)` wins over the
/// global config; `None` falls back to the cached global (config + 500 ms
/// cache). The override is
/// already clamped to [8.0, 32.0] at every write site (action handler +
/// session ingress), so no re-validation here.
pub(super) fn font_size(size_override: Option<f32>) -> Pixels {
    if let Some(s) = size_override {
        return font_points_to_pixels(s);
    }
    let (_, size, _, _, _, _, _) = cached_font_config();
    font_points_to_pixels(size)
}

/// EP-006 US-019: the global (non-overridden) font size in points - the zoom
/// handlers' baseline for a pane that has no override yet.
pub fn global_font_size() -> f32 {
    let (_, size, _, _, _, _, _) = cached_font_config();
    size
}

pub fn resolve_frame_metrics(
    window: &mut Window,
    _cx: &mut App,
    size_override: Option<f32>,
) -> TerminalFrameMetrics {
    let font = base_font();
    let font_size = font_size(size_override);
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

    // Cell width and line height are config-derived terminal strides, not
    // measured glyph advances. They scale with the effective rendered size so
    // Windows Terminal-style multipliers stay comparable across font changes.
    let (_, _, line_multiplier, cell_multiplier, _, _, _) = cached_font_config();
    let cell_width_raw = px(font_size.as_f32() * cell_multiplier);
    let line_height_raw = px(font_size.as_f32() * line_multiplier);

    // US-002: snap raw font measurements to integer pixels via `.round()`
    // (WezTerm convention - minimizes layout-area drift on fractional
    // advances vs `floor`/`ceil`). Quantizing the cell stride at measure
    // time means every downstream coordinate `cell_width * col` is also
    // integer, eliminating the fractional residual that prevents adjacent
    // quads from sharing a pixel boundary. Trade-off: column count
    // `viewport / cell_width` may shift by ±1 on extreme aspect ratios.
    // Acceptable for pixel-perfect rendering (US-001 / US-003 / US-004).
    let cell_width = cell_width_raw.round();
    let line_height = line_height_raw.round();

    // PANEFLOW_PIXEL_PROBE: record both raw and snapped cell dimensions so a
    // future investigation can tell at a glance whether the snap was a
    // no-op (`raw == snapped`) or quantized a fractional residual. Origin
    // is logged separately from `paint()` via `record_origin`.
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
        // Rust's f32::round documents round-half-away-from-zero. Lock in
        // that behavior so a future `.round()` → `.round_ties_even()` swap
        // would surface here instead of as a silent layout shift.
        assert_eq!(px(8.5).round(), px(9.0));
        assert_eq!(px(8.6).round(), px(9.0));
        assert_eq!(px(8.49).round(), px(8.0));
    }

    #[test]
    fn round_snap_yields_integer_for_fractional_line_height() {
        // 13 pt x 96/72 x 1.2 multiplier = 20.8 logical px.
        let raw_lh = font_points_to_pixels(DEFAULT_FONT_SIZE) * DEFAULT_LINE_HEIGHT;
        let snapped = raw_lh.round();
        assert_eq!(snapped, px(21.0));
        assert!(snapped.as_f32().fract().abs() < 1e-6);
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
        assert_eq!(expand_paneflow_alias(".PaneflowMono"), "Geist Mono");
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
        assert_eq!(
            expand_paneflow_alias("JetBrainsMono NFM"),
            "JetBrainsMono NFM"
        );
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
        assert_eq!(resolve_font_family(Some(".PaneflowMono")), "Geist Mono");
        assert_eq!(resolve_font_family(Some(".PaneflowSans")), "Geist");
    }

    #[test]
    fn resolve_font_family_short_circuits_embedded_concrete_names() {
        // Users who write `"Geist Mono"`, `"Lilex"`, `"Geist"`, or
        // `"IBM Plex Sans"` in
        // paneflow.json get the embedded font even on platforms whose
        // INSTALLED_MONO_FONTS registry doesn't list them (Windows
        // pre-DirectWrite, container without fontconfig). The short
        // circuit before the registry lookup is what makes that work.
        assert_eq!(resolve_font_family(Some("Geist Mono")), "Geist Mono");
        assert_eq!(resolve_font_family(Some("Lilex")), "Lilex");
        assert_eq!(resolve_font_family(Some("Geist")), "Geist");
        assert_eq!(resolve_font_family(Some("IBM Plex Sans")), "IBM Plex Sans");
    }

    #[test]
    fn select_default_font_family_uses_bundled_geist_mono() {
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
    fn default_font_family_is_bundled_geist_mono() {
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
