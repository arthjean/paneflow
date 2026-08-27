//! Terminal theme data model and UI palette.

use gpui::{Hsla, Rgba};

use crate::terminal::element::{MIN_APCA_CONTRAST, ensure_minimum_contrast};

/// Terminal color theme with an optional app-wide UI palette plus 36 terminal slots:
/// 5 base + cursor + selection + selection_foreground + scrollbar_thumb +
/// link_text + 2 title bar + 24 ANSI (8 hues x 3 intensities).
#[derive(Clone, Copy)]
pub struct TerminalTheme {
    /// Optional app-wide UI palette. Legacy themes derive their chrome colors
    /// from light/dark defaults; bundled custom themes can opt into exact UI
    /// tokens so the theme affects the whole app, not just ANSI colors.
    pub ui: Option<UiColors>,
    pub background: Hsla,
    pub foreground: Hsla,
    pub bright_foreground: Hsla,
    pub dim_foreground: Hsla,
    pub ansi_background: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
    /// US-007: foreground color for text inside the selection rect,
    /// guaranteed to satisfy APCA Lc ≥ `MIN_APCA_CONTRAST` against
    /// `selection`. Computed once at theme-load time by
    /// [`TerminalTheme::recompute_selection_foreground`] from the theme's
    /// regular `foreground`. Used by `build_layout` to override the
    /// per-cell `fg` for cells inside the selection.
    pub selection_foreground: Hsla,
    pub scrollbar_thumb: Hsla,
    /// Color for hyperlink underline and text on Ctrl+hover.
    pub link_text: Hsla,
    pub title_bar_background: Hsla,
    pub title_bar_inactive_background: Hsla,
    // 8 hues x 3 intensities = 24 ANSI colors
    pub black: Hsla,
    pub red: Hsla,
    pub green: Hsla,
    pub yellow: Hsla,
    pub blue: Hsla,
    pub magenta: Hsla,
    pub cyan: Hsla,
    pub white: Hsla,
    pub bright_black: Hsla,
    pub bright_red: Hsla,
    pub bright_green: Hsla,
    pub bright_yellow: Hsla,
    pub bright_blue: Hsla,
    pub bright_magenta: Hsla,
    pub bright_cyan: Hsla,
    pub bright_white: Hsla,
    /// Per-language syntax-highlighting colors for the diff view
    /// (prd-diff-syntax-palette-2026-Q3.md, EP-001). A dedicated semantic
    /// palette - NOT the 8-hue ANSI set above - so diff syntax can mirror the
    /// coverage of a modern editor's `SyntaxTheme` (≈18 distinct hues).
    /// `Copy`, snapshotted once per diff load via `DiffSyntax::from_theme`.
    pub syntax: SyntaxPalette,
}

/// Semantic syntax-highlighting palette for the diff view, mirroring the
/// *structure and coverage* of Zed's `SyntaxTheme` (a `name → color` map) in
/// Paneflow's Catppuccin brand. Each slot is one tree-sitter capture family;
/// `diff/syntax.rs::color_for_capture` resolves a capture name to a slot with
/// longest-prefix fallback. Color-only for v1 (no font-style); `Copy` and
/// allocation-free (~30 × `Hsla`).
#[derive(Clone, Copy)]
pub struct SyntaxPalette {
    pub comment: Hsla,
    pub comment_doc: Hsla,
    pub keyword: Hsla,
    pub function: Hsla,
    pub r#type: Hsla,
    pub r#enum: Hsla,
    pub constructor: Hsla,
    pub string: Hsla,
    pub string_escape: Hsla,
    pub string_special: Hsla,
    pub number: Hsla,
    pub boolean: Hsla,
    pub constant: Hsla,
    pub constant_builtin: Hsla,
    pub property: Hsla,
    pub variable: Hsla,
    pub variable_builtin: Hsla,
    pub operator: Hsla,
    pub punctuation: Hsla,
    pub punctuation_special: Hsla,
    pub attribute: Hsla,
    pub tag: Hsla,
    pub label: Hsla,
    pub namespace: Hsla,
    pub title: Hsla,
    pub text_literal: Hsla,
    pub link_uri: Hsla,
    pub link_text: Hsla,
    pub emphasis: Hsla,
    pub emphasis_strong: Hsla,
}

impl SyntaxPalette {
    /// Dark diff syntax palette (`paneflow_dark()`), tuned from the Codex App diff
    /// reference while keeping the existing semantic slot structure. ≥ 18
    /// distinct values.
    pub fn catppuccin_mocha() -> Self {
        Self {
            comment: h(0x989898),
            comment_doc: h(0xa0a0a0),
            keyword: h(0xb070ff),
            function: h(0xa868e8),
            r#type: h(0xf89850),
            r#enum: h(0xf0a060),
            constructor: h(0xf8a858),
            string: h(0x40c878),
            string_escape: h(0x70c8f0),
            string_special: h(0xf87878),
            number: h(0xf8c060),
            boolean: h(0xf0b858),
            constant: h(0xf8d878),
            constant_builtin: h(0x70c8f0),
            property: h(0xf0a060),
            variable: h(0xf89850),
            variable_builtin: h(0xf8a858),
            operator: h(0x70c8f0),
            punctuation: h(0xd8d0d0),
            punctuation_special: h(0xf87878),
            attribute: h(0x78d0f8),
            tag: h(0xf87070),
            label: h(0xf0c8b8),
            namespace: h(0xa868e8),
            title: h(0xff8080),
            text_literal: h(0x48d080),
            link_uri: h(0x70c8f0),
            link_text: h(0xb070ff),
            emphasis: h(0xf08090),
            emphasis_strong: h(0xf0c8b8),
        }
    }

    /// Catppuccin Latte - the light-theme syntax palette. Darker, saturated
    /// hues that read on the white editor surface; ≥ 18 distinct values.
    pub fn catppuccin_latte() -> Self {
        Self {
            comment: h(0x9ca0b0),             // Overlay0
            comment_doc: h(0x8c8fa1),         // Overlay1
            keyword: h(0x8839ef),             // Mauve
            function: h(0x1e66f5),            // Blue
            r#type: h(0x179299),              // Teal
            r#enum: h(0x179299),              // Teal
            constructor: h(0x1e66f5),         // Blue
            string: h(0x40a02b),              // Green
            string_escape: h(0x04a5e5),       // Sky
            string_special: h(0xea76cb),      // Pink
            number: h(0xfe640b),              // Peach
            boolean: h(0xfe640b),             // Peach
            constant: h(0xdf8e1d),            // Yellow
            constant_builtin: h(0x209fb5),    // Sapphire
            property: h(0xd20f39),            // Red
            variable: h(0x4c4f69),            // Text
            variable_builtin: h(0xfe640b),    // Peach
            operator: h(0x04a5e5),            // Sky
            punctuation: h(0x5c5f77),         // Subtext1
            punctuation_special: h(0xe64553), // Maroon
            attribute: h(0x1e66f5),           // Blue
            tag: h(0xd20f39),                 // Red
            label: h(0xdc8a78),               // Rosewater
            namespace: h(0x7287fd),           // Lavender
            title: h(0xd20f39),               // Red
            text_literal: h(0x40a02b),        // Green
            link_uri: h(0x04a5e5),            // Sky
            link_text: h(0x1e66f5),           // Blue
            emphasis: h(0xe64553),            // Maroon
            emphasis_strong: h(0xdd7878),     // Flamingo
        }
    }

    /// Vercel-inspired dark syntax palette: mostly monochrome neutrals with a
    /// few crisp product-style accents for code structure.
    pub fn vercel_dark() -> Self {
        Self {
            comment: h(0x737373),
            comment_doc: h(0x8a8a8a),
            keyword: h(0xffffff),
            function: h(0x7dd3fc),
            r#type: h(0x60a5fa),
            r#enum: h(0x93c5fd),
            constructor: h(0xa5b4fc),
            string: h(0x86efac),
            string_escape: h(0x67e8f9),
            string_special: h(0xf0abfc),
            number: h(0xfde68a),
            boolean: h(0xfbbf24),
            constant: h(0xf5d90a),
            constant_builtin: h(0x38bdf8),
            property: h(0xfca5a5),
            variable: h(0xe5e5e5),
            variable_builtin: h(0xfcd34d),
            operator: h(0x94a3b8),
            punctuation: h(0xa3a3a3),
            punctuation_special: h(0xf87171),
            attribute: h(0x7dd3fc),
            tag: h(0xfb7185),
            label: h(0xd4d4d4),
            namespace: h(0xc4b5fd),
            title: h(0xffffff),
            text_literal: h(0x86efac),
            link_uri: h(0x7dd3fc),
            link_text: h(0xffffff),
            emphasis: h(0xf9a8d4),
            emphasis_strong: h(0xf5f5f5),
        }
    }

    /// Claude Desktop-inspired dark syntax palette: graphite neutrals, ivory
    /// text, and restrained Claude orange accents.
    pub fn claude_dark() -> Self {
        Self {
            comment: h(0x75736d),
            comment_doc: h(0x93938b),
            keyword: h(0xd97757),
            function: h(0xddd5c8),
            r#type: h(0xb9b9ae),
            r#enum: h(0xb8a1c8),
            constructor: h(0xd3b49a),
            string: h(0x9ab38a),
            string_escape: h(0x95b8b2),
            string_special: h(0xd9905f),
            number: h(0xc3a45f),
            boolean: h(0xd97757),
            constant: h(0xc3c2b7),
            constant_builtin: h(0x8fa4b8),
            property: h(0xd3a082),
            variable: h(0xd7d0c6),
            variable_builtin: h(0xd5b976),
            operator: h(0x93938b),
            punctuation: h(0xa5a49c),
            punctuation_special: h(0xd97757),
            attribute: h(0xb9b9ae),
            tag: h(0xe68a6d),
            label: h(0xc3c2b7),
            namespace: h(0xb8a1c8),
            title: h(0xc3c2b7),
            text_literal: h(0x9ab38a),
            link_uri: h(0x8fa4b8),
            link_text: h(0xd97757),
            emphasis: h(0xd9905f),
            emphasis_strong: h(0xf2eee8),
        }
    }

    /// Cursor-inspired dark syntax palette: near-black editor neutrals,
    /// blue-white IDE accents, and Paneflow's stable status hues.
    pub fn cursor_dark() -> Self {
        Self {
            comment: h(0x6f6f6f),
            comment_doc: h(0x989898),
            keyword: h(0xa0d0f0),
            function: h(0xe8e8e8),
            r#type: h(0x7dd3fc),
            r#enum: h(0x8fb7ff),
            constructor: h(0xb8e0f0),
            string: h(0x57d992),
            string_escape: h(0x8bdcff),
            string_special: h(0xc79bff),
            number: h(0xffd166),
            boolean: h(0xffb86b),
            constant: h(0xd8d8d8),
            constant_builtin: h(0xa0d0f0),
            property: h(0xb8e0f0),
            variable: h(0xf5f5f5),
            variable_builtin: h(0xffd166),
            operator: h(0x989898),
            punctuation: h(0xb0b0b0),
            punctuation_special: h(0xff8580),
            attribute: h(0x7dd3fc),
            tag: h(0xff8580),
            label: h(0xd0d0d0),
            namespace: h(0xc79bff),
            title: h(0xffffff),
            text_literal: h(0x57d992),
            link_uri: h(0xa0d0f0),
            link_text: h(0x8fb7ff),
            emphasis: h(0xb8e0f0),
            emphasis_strong: h(0xffffff),
        }
    }

    /// Vercel-inspired light syntax palette: near-black structure on white,
    /// with the Geist accent hues darkened enough to read on a light surface.
    pub fn vercel_light() -> Self {
        Self {
            comment: h(0x6b7280),
            comment_doc: h(0x52525b),
            keyword: h(0x000000),
            function: h(0x0068d6),
            r#type: h(0x067f6f),
            r#enum: h(0x0a6e64),
            constructor: h(0x4338ca),
            string: h(0x0f7b0f),
            string_escape: h(0x0e7490),
            string_special: h(0xa21caf),
            number: h(0xa35200),
            boolean: h(0xb45309),
            constant: h(0x854d0e),
            constant_builtin: h(0x0369a1),
            property: h(0xbe123c),
            variable: h(0x27272a),
            variable_builtin: h(0x92400e),
            operator: h(0x475569),
            punctuation: h(0x64748b),
            punctuation_special: h(0xcd2b31),
            attribute: h(0x0068d6),
            tag: h(0xcd2b31),
            label: h(0x3f3f46),
            namespace: h(0x7820bc),
            title: h(0x000000),
            text_literal: h(0x0f7b0f),
            link_uri: h(0x0068d6),
            link_text: h(0x1d4ed8),
            emphasis: h(0xbe185d),
            emphasis_strong: h(0x18181b),
        }
    }

    /// Claude Desktop-inspired light syntax palette: warm ink on the cream
    /// paper surface, with Claude orange carrying keywords and emphasis.
    pub fn claude_light() -> Self {
        Self {
            comment: h(0x8f8b7d),
            comment_doc: h(0x7a7667),
            keyword: h(0xc2552f),
            function: h(0x4a6fa5),
            r#type: h(0x2f7d72),
            r#enum: h(0x7a5ea8),
            constructor: h(0xa2643a),
            string: h(0x4f7a3f),
            string_escape: h(0x2b7f8f),
            string_special: h(0xb0562e),
            number: h(0x9a6b1f),
            boolean: h(0xc2552f),
            constant: h(0x6b5f3d),
            constant_builtin: h(0x3f6b8a),
            property: h(0xa14d5a),
            variable: h(0x4a4636),
            variable_builtin: h(0x8a6b2a),
            operator: h(0x6f6a58),
            punctuation: h(0x807b68),
            punctuation_special: h(0xc2552f),
            attribute: h(0x2f7d72),
            tag: h(0xb4442a),
            label: h(0x6b6552),
            namespace: h(0x7a5ea8),
            title: h(0x8a4a2a),
            text_literal: h(0x4f7a3f),
            link_uri: h(0x2b6f9a),
            link_text: h(0xc2552f),
            emphasis: h(0xb0562e),
            emphasis_strong: h(0x2e2b1e),
        }
    }

    /// Cursor-inspired light syntax palette: the VS Code Light+ identity the
    /// editor ships by default, kept recognizable token family by token family.
    pub fn cursor_light() -> Self {
        Self {
            comment: h(0x008000),
            comment_doc: h(0x3f8f3f),
            keyword: h(0x0000ff),
            function: h(0x795e26),
            r#type: h(0x267f99),
            r#enum: h(0x2b91af),
            constructor: h(0x3b8ea5),
            string: h(0xa31515),
            string_escape: h(0xd16969),
            string_special: h(0x811f3f),
            number: h(0x098658),
            boolean: h(0x0000ff),
            constant: h(0x0070c1),
            constant_builtin: h(0x0451a5),
            property: h(0x0b5394),
            variable: h(0x001080),
            variable_builtin: h(0x0070c1),
            operator: h(0x393a34),
            punctuation: h(0x5a5a5a),
            punctuation_special: h(0xaf00db),
            attribute: h(0xe50000),
            tag: h(0x800000),
            label: h(0x3d3d3d),
            namespace: h(0x4b69c6),
            title: h(0x800000),
            text_literal: h(0xa31515),
            link_uri: h(0x0f6fc5),
            link_text: h(0x0000ff),
            emphasis: h(0xaf00db),
            emphasis_strong: h(0x000080),
        }
    }

    /// All slots as a flat array - for tests counting distinct hues and for any
    /// future iteration over the palette.
    #[cfg(test)]
    pub(crate) fn all_slots(&self) -> [Hsla; 30] {
        [
            self.comment,
            self.comment_doc,
            self.keyword,
            self.function,
            self.r#type,
            self.r#enum,
            self.constructor,
            self.string,
            self.string_escape,
            self.string_special,
            self.number,
            self.boolean,
            self.constant,
            self.constant_builtin,
            self.property,
            self.variable,
            self.variable_builtin,
            self.operator,
            self.punctuation,
            self.punctuation_special,
            self.attribute,
            self.tag,
            self.label,
            self.namespace,
            self.title,
            self.text_literal,
            self.link_uri,
            self.link_text,
            self.emphasis,
            self.emphasis_strong,
        ]
    }
}

pub(super) fn h(hex: u32) -> Hsla {
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    Hsla::from(Rgba { r, g, b, a: 1.0 })
}

pub(super) fn ha(hex: u32, alpha: f32) -> Hsla {
    let mut color = h(hex);
    color.a = alpha;
    color
}

const CHROME_BACKGROUND_HEX: u32 = 0x141414;
// Shared right-panel background for terminals, tab bars, Diff, and Agents.
const TERMINAL_BACKGROUND_HEX: u32 = 0x181818;
const BORDER_HEX: u32 = 0x252525;

fn is_light_theme(theme: &TerminalTheme) -> bool {
    theme.background.l > 0.5
}

impl TerminalTheme {
    /// US-007: recompute [`Self::selection_foreground`] from the current
    /// `foreground` and `selection` colors so APCA Lc(selection_fg, selection)
    /// ≥ [`MIN_APCA_CONTRAST`]. Called at theme-load time (and on every
    /// hot-reload). Reusing the same `ensure_minimum_contrast` algorithm as
    /// per-cell text guarantees consistent visual semantics - selected text
    /// is no harder to read than non-selected text on near-luminance themes.
    pub(crate) fn recompute_selection_foreground(&mut self) {
        // The `selection` slot's alpha represents how the selection blends
        // with the cell background underneath. For contrast purposes we
        // approximate the perceived selection background as the opaque
        // version of `selection` (alpha = 1.0). A future refinement could
        // alpha-composite against the actual cell `background` for a
        // tighter contrast estimate, but the simple opaque-bg model is
        // what `MIN_APCA_CONTRAST` was tuned for in the per-cell path.
        let selection_bg_opaque = Hsla {
            a: 1.0,
            ..self.selection
        };
        self.selection_foreground =
            ensure_minimum_contrast(self.foreground, selection_bg_opaque, MIN_APCA_CONTRAST);
    }
}

pub(super) fn apply_surface_overrides(mut theme: TerminalTheme) -> TerminalTheme {
    if theme.ui.is_some() || is_light_theme(&theme) {
        // US-007: light themes skip surface overrides but still need their
        // selection_foreground populated; do it here so every theme exiting
        // this function has a valid value regardless of branch taken.
        theme.recompute_selection_foreground();
        return theme;
    }

    let chrome_bg = h(CHROME_BACKGROUND_HEX);
    let terminal_bg = h(TERMINAL_BACKGROUND_HEX);
    theme.title_bar_background = chrome_bg;
    theme.title_bar_inactive_background = chrome_bg;
    theme.background = terminal_bg;
    theme.ansi_background = terminal_bg;
    theme.foreground = h(0xf0f3f7);
    theme.bright_foreground = h(0xffffff);
    theme.dim_foreground = h(0x9ca7b5);
    theme.selection = ha(0x5aa6ff, 0.22);
    theme.scrollbar_thumb = ha(0x9aa8bd, 0.30);
    theme.link_text = h(0x57d5c4);
    theme.recompute_selection_foreground();
    theme
}

// ---------------------------------------------------------------------------
// UI color palette - derived from the active terminal theme
// ---------------------------------------------------------------------------

/// Colors for the app chrome (sidebar, settings, badges, etc.).
#[derive(Clone, Copy)]
pub struct UiColors {
    /// Use the theme's `vc_*` washes directly for the Diff surface. Default
    /// dark themes keep Paneflow's historical opaque Codex-style diff washes;
    /// custom bundled themes opt into their own line backgrounds.
    pub use_theme_diff_washes: bool,
    pub base: Hsla,    // deepest background (settings sidebar, app bg)
    pub surface: Hsla, // card/panel background
    pub overlay: Hsla, // dropdown/popover bg
    pub border: Hsla,  // borders, dividers
    pub subtle: Hsla,  // hover bg, badge bg
    pub muted: Hsla,   // secondary text, labels
    pub text: Hsla,    // primary text
    pub accent: Hsla,  // active indicator, highlighted items
    /// Distinct background for the `WaitingForConfirmation` tool-card
    /// header (US-110 AC #2 of `tasks/prd-agent-ui-refactor-2026-Q3.md`).
    /// Mirrors Zed's `tool_card_header_bg` -- an accent-tinted variant
    /// of the card surface that signals "this row is actionable" at a
    /// glance without redrawing the whole card.
    pub tool_card_header_bg: Hsla,
    // US-007 (prd-git-diff-mode-2026-Q3.md): curated version-control
    // colors for the Git Diff surface, mirroring Zed's `StatusColors`
    // model (`crates/theme/src/styles/status.rs`) - first-class slots,
    // NOT terminal-ANSI-derived. Light/dark variants are resolved in
    // `ui_colors_with`. `vc_*` are the foreground (status icons, file
    // labels, hunk gutter); `*_background` default to the foreground at
    // 0.25 alpha (Zed's `*_background` convention) for line washes;
    // `vc_word_*` are the stronger intra-line word-diff emphasis.
    /// Added / created (green).
    pub vc_added: Hsla,
    /// Modified / changed (yellow).
    pub vc_modified: Hsla,
    /// Deleted / removed (red).
    pub vc_deleted: Hsla,
    /// Merge conflict (orange - distinct from delete-red).
    pub vc_conflict: Hsla,
    /// Added-line background wash.
    pub vc_added_background: Hsla,
    /// Deleted-line background wash.
    pub vc_deleted_background: Hsla,
    /// Modified-line background wash.
    pub vc_modified_background: Hsla,
    /// Intra-line word-diff emphasis (added side).
    pub vc_word_added: Hsla,
    /// Intra-line word-diff emphasis (deleted side).
    pub vc_word_deleted: Hsla,
    // EP-001 (CLI Cockpit, US-002): broadcast-group
    // stripe palette - eight first-class slots so render code never inlines a
    // hex (FR-08). Positional identity colors (not semantic status colors):
    // they only need to stay mutually distinguishable and readable as a 3px
    // pane-edge stripe on both bundled themes.
    pub group_1: Hsla,
    pub group_2: Hsla,
    pub group_3: Hsla,
    pub group_4: Hsla,
    pub group_5: Hsla,
    pub group_6: Hsla,
    pub group_7: Hsla,
    pub group_8: Hsla,
    // EP-004 (CLI Cockpit): agent terminal-state
    // slots (FR-08 - no inline hex in render code). Both are deliberately
    // distinct from `vc_conflict` (the attention/waiting dot) so a crashed
    // agent never reads as "needs input".
    /// US-010: `AgentState::Errored` - tab dot + sidebar badge (red).
    pub agent_error: Hsla,
    /// US-011: `AgentState::Stalled` - sidebar badge (muted grey-blue:
    /// "silent", not "failing").
    pub agent_stalled: Hsla,
    // EP-005 US-013: per-tool identity colors, promoted from the inline
    // hexes the sidebar spinner rows used (FR-08). Brand hues, identical
    // on both themes by design (they tint text on the theme surface).
    /// Claude rows/spinner (Anthropic salmon).
    pub agent_claude: Hsla,
    /// Codex rows/spinner (Codex indigo).
    pub agent_codex: Hsla,
}

/// Effective version-control diff colors for the Git Diff / Review surfaces.
///
/// On dark themes the foreground plus the line and gutter washes are the
/// Codex-app-sampled green/red, a deliberate override of the muted `vc_*` theme
/// slots (which read too desaturated on the dense diff body). On light themes
/// they fall through to the theme `vc_*` slots. Single source for the Agents
/// diff dock, the Diff/Review view, and the diff sidebar so the three never
/// drift.
#[derive(Clone, Copy)]
pub struct DiffColors {
    pub added: Hsla,
    pub deleted: Hsla,
    pub added_background: Hsla,
    pub deleted_background: Hsla,
    pub added_gutter_background: Hsla,
    pub deleted_gutter_background: Hsla,
}

impl UiColors {
    /// Resolve the canonical diff color set (see [`DiffColors`]). Dark/light is
    /// keyed off `base.l` so any render path holding a `UiColors` can call it
    /// without re-locking the theme cache.
    pub fn diff_colors(&self) -> DiffColors {
        if self.base.l > 0.5 || self.use_theme_diff_washes {
            return DiffColors {
                added: self.vc_added,
                deleted: self.vc_deleted,
                added_background: self.vc_added_background,
                deleted_background: self.vc_deleted_background,
                added_gutter_background: self.vc_added_background,
                deleted_gutter_background: self.vc_deleted_background,
            };
        }
        DiffColors {
            // Codex-inspired dark diff panel, softened for the neutral shell.
            added: h(0x57d992),
            deleted: h(0xff6f6a),
            added_background: h(0x1d3a2b),
            deleted_background: h(0x402425),
            added_gutter_background: h(0x16281f),
            deleted_gutter_background: h(0x2c1718),
        }
    }

    /// Stripe color for broadcast-group slot `idx` (0-based). Wraps modulo 8
    /// so an out-of-range index (impossible via the picker, which caps group
    /// creation at 8) can never panic the render path.
    pub fn group_color(&self, idx: usize) -> Hsla {
        match idx % 8 {
            0 => self.group_1,
            1 => self.group_2,
            2 => self.group_3,
            3 => self.group_4,
            4 => self.group_5,
            5 => self.group_6,
            6 => self.group_7,
            _ => self.group_8,
        }
    }
}

/// Derive UI colors from the active terminal theme.
///
/// Light themes get light UI; dark themes get dark UI. Calls
/// [`ui_colors_with`] under the hood after a single theme lookup --
/// render paths that already have a `TerminalTheme` in hand should
/// call [`ui_colors_with`] directly to avoid re-locking the theme
/// cache (Composer / ThreadView render do this).
pub fn ui_colors() -> UiColors {
    let theme = super::watcher::active_theme();
    ui_colors_with(&theme)
}

/// Derive UI colors from an already-resolved terminal theme. Identical
/// output to [`ui_colors`] but skips the global theme-cache lock --
/// the agents UI calls `ui_colors` once per render entry and again
/// for every visible item, so passing the cached theme through saves
/// O(visible_items) mutex acquisitions per frame.
pub fn ui_colors_with(theme: &TerminalTheme) -> UiColors {
    if let Some(ui) = theme.ui {
        return ui;
    }

    let is_light = is_light_theme(theme);
    if is_light {
        UiColors {
            use_theme_diff_washes: false,
            // Codex-style light shell: the right-hand work area is pure white,
            // while controls use neutral, near-white layers for hierarchy. Every
            // grey here is hue-free by design (see the dark arm below).
            base: h(0xffffff),
            surface: h(0xf7f7f7),
            overlay: h(0xffffff),
            border: h(0xe6e6e6),
            subtle: h(0xeeeeee),
            muted: h(0x6a6a6a),
            text: h(0x262626),
            accent: h(0x4c6fff),
            // Light theme: a surface one step darker than the card so the
            // awaiting-confirmation row stands out without overwhelming the
            // chat stream.
            tool_card_header_bg: h(0xf1f1f1),
            // Curated diff palette (Catppuccin Latte family) - darker,
            // saturated hues that read on a light surface.
            vc_added: h(0x40a02b),
            vc_modified: h(0xdf8e1d),
            vc_deleted: h(0xd20f39),
            vc_conflict: h(0xfe640b),
            // Subtle line wash (Zed editor_diff_hunk_*_background, light a=0x29=0.16);
            // the opaque gutter hunk bar carries the strong status signal.
            vc_added_background: ha(0x40a02b, 0.16),
            vc_deleted_background: ha(0xd20f39, 0.16),
            vc_modified_background: ha(0xdf8e1d, 0.16),
            vc_word_added: ha(0x40a02b, 0.40),
            vc_word_deleted: ha(0xd20f39, 0.40),
            // Broadcast stripes (Catppuccin Latte family) - saturated hues
            // that hold up as a thin stripe on a light pane edge.
            group_1: h(0x1e66f5),
            group_2: h(0x40a02b),
            group_3: h(0xdf8e1d),
            group_4: h(0xd20f39),
            group_5: h(0x8839ef),
            group_6: h(0x179299),
            group_7: h(0xfe640b),
            group_8: h(0x7287fd),
            // Agent state (Latte family): saturated red for a crash, the
            // neutral overlay grey for a silent session.
            agent_error: h(0xd20f39),
            agent_stalled: h(0x808080),
            agent_claude: h(0xe89271),
            agent_codex: h(0x5b6cff),
        }
    } else {
        UiColors {
            use_theme_diff_washes: false,
            // Every neutral in the shell is hue-free: greys carry no blue
            // cast, so color in the UI only ever means status (`vc_*`),
            // identity (`group_*`) or the accent. The greys below hold the
            // luminance the blue-tinted ones had, so contrast is unchanged -
            // only the hue is gone.
            base: h(TERMINAL_BACKGROUND_HEX),
            surface: h(0x212121),
            overlay: h(CHROME_BACKGROUND_HEX),
            border: h(BORDER_HEX),
            subtle: h(0x2a2a2a),
            muted: h(0xa0a0a0),
            text: h(0xdddddd),
            accent: h(0x57d5c4),
            // Dark theme: a touch lighter than the card surface so the
            // awaiting row reads even at a glance.
            tool_card_header_bg: h(0x2e2e2e),
            // Premium dark diff palette: Codex-like red/green intent,
            // softened to sit inside the neutral terminal surface.
            vc_added: h(0x57d992),
            vc_modified: h(0xffd166),
            vc_deleted: h(0xff6f6a),
            vc_conflict: h(0xffa657),
            // Subtle line wash (Zed editor_diff_hunk_*_background, dark a=0x1f=0.12);
            // the opaque gutter hunk bar carries the strong status signal.
            vc_added_background: ha(0x57d992, 0.12),
            vc_deleted_background: ha(0xff6f6a, 0.12),
            vc_modified_background: ha(0xffd166, 0.12),
            vc_word_added: ha(0x57d992, 0.40),
            vc_word_deleted: ha(0xff6f6a, 0.40),
            // Broadcast stripes: high-luminance accents that keep their
            // identity against the neutral pane edge.
            group_1: h(0x7eb6ff),
            group_2: h(0x57d992),
            group_3: h(0xffd166),
            group_4: h(0xff6f6a),
            group_5: h(0xc79bff),
            group_6: h(0x57d5c4),
            group_7: h(0xffa657),
            group_8: h(0x9ea7ff),
            // Agent state: clear, bright marks on the muted cockpit shell.
            agent_error: h(0xff6f6a),
            agent_stalled: h(0xa0a0a0),
            agent_claude: h(0xffa657),
            agent_codex: h(0x7eb6ff),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::element::apca_contrast;
    use crate::theme::builtin::{
        claude_dark, claude_light, cursor_dark, cursor_light, paneflow_dark, paneflow_light,
        theme_by_name, vercel_dark, vercel_light,
    };

    #[test]
    fn light_theme_keeps_light_surfaces_after_overrides() {
        let theme = apply_surface_overrides(paneflow_light());

        assert!(theme.background.l > 0.5);
        assert_eq!(theme.background, h(0xffffff));
        assert!(theme.ansi_background.l > 0.5);
        assert!(theme.title_bar_background.l > 0.5);
    }

    #[test]
    fn light_ui_keeps_the_work_area_pure_white() {
        let ui = ui_colors_with(&paneflow_light());

        assert_eq!(ui.base, h(0xffffff));
        assert_eq!(ui.overlay, h(0xffffff));
        assert_eq!(ui.border, h(0xe6e6e6));
        assert_ne!(ui.surface, ui.base);
        assert_ne!(ui.border, ui.base);
        assert_ne!(ui.text, ui.base);
    }

    #[test]
    fn dark_theme_still_uses_dark_surface_overrides() {
        let theme = apply_surface_overrides(paneflow_dark());

        assert_eq!(theme.background.l, h(TERMINAL_BACKGROUND_HEX).l);
        assert_eq!(theme.ansi_background.l, h(TERMINAL_BACKGROUND_HEX).l);
        assert_eq!(theme.title_bar_background.l, h(CHROME_BACKGROUND_HEX).l);
    }

    #[test]
    fn dark_ui_uses_cockpit_surface_palette() {
        let ui = ui_colors_with(&paneflow_dark());

        assert_eq!(ui.base, h(TERMINAL_BACKGROUND_HEX));
        assert_eq!(ui.overlay, h(CHROME_BACKGROUND_HEX));
        assert_eq!(ui.border, h(BORDER_HEX));
    }

    #[test]
    fn custom_dark_themes_keep_their_app_wide_palette() {
        for (
            label,
            theme,
            expected_base,
            expected_title,
            expected_surface,
            expected_accent,
            expected_theme_diff_washes,
        ) in [
            (
                "Vercel Dark",
                vercel_dark(),
                h(0x000000),
                h(0x000000),
                h(0x0a0a0a),
                h(0xffffff),
                true,
            ),
            (
                "Claude Dark",
                claude_dark(),
                h(0x1f1f1e),
                h(0x1f1f1e),
                h(0x262626),
                h(0xd97757),
                false,
            ),
            (
                "Cursor Dark",
                cursor_dark(),
                h(0x141414),
                h(0x181818),
                h(0x181818),
                h(0xa0d0f0),
                false,
            ),
        ] {
            let theme = apply_surface_overrides(theme);
            let ui = ui_colors_with(&theme);

            assert_eq!(theme.background, expected_base, "{label} background");
            assert_eq!(
                theme.title_bar_background, expected_title,
                "{label} title bar"
            );
            assert_eq!(ui.base, expected_base, "{label} ui base");
            assert_eq!(ui.surface, expected_surface, "{label} ui surface");
            assert_eq!(ui.accent, expected_accent, "{label} ui accent");
            assert_eq!(
                ui.use_theme_diff_washes, expected_theme_diff_washes,
                "{label} diff wash mode"
            );
        }
    }

    /// US-007 invariant: for any theme exiting `apply_surface_overrides` or
    /// `theme_by_name`, the selection foreground must satisfy the same
    /// APCA Lc threshold the per-cell contrast pass uses. A near-luminance
    /// theme without this invariant would render selected text illegibly.
    fn assert_selection_invariant(theme: &TerminalTheme, label: &str) {
        let bg_opaque = Hsla {
            a: 1.0,
            ..theme.selection
        };
        let lc = apca_contrast(theme.selection_foreground, bg_opaque).abs();
        assert!(
            lc >= MIN_APCA_CONTRAST,
            "{label}: APCA Lc({lc}) < {MIN_APCA_CONTRAST} for selection_foreground vs selection"
        );
    }

    #[test]
    fn bundled_themes_satisfy_selection_contrast_invariant() {
        // All bundled themes must produce a readable selection foreground.
        for (label, theme) in [
            ("Paneflow Dark", apply_surface_overrides(paneflow_dark())),
            ("Paneflow Light", apply_surface_overrides(paneflow_light())),
            ("Vercel Dark", apply_surface_overrides(vercel_dark())),
            ("Vercel Light", apply_surface_overrides(vercel_light())),
            ("Claude Dark", apply_surface_overrides(claude_dark())),
            ("Claude Light", apply_surface_overrides(claude_light())),
            ("Cursor Dark", apply_surface_overrides(cursor_dark())),
            ("Cursor Light", apply_surface_overrides(cursor_light())),
        ] {
            assert_selection_invariant(&theme, label);
        }
    }

    #[test]
    fn theme_by_name_returns_invariant_satisfying_themes() {
        // theme_by_name is the public entry point; users may call it without
        // going through apply_surface_overrides, so it must finalize on the
        // way out. Iterate the live table so the test tracks the bundled set.
        for (name, _) in crate::theme::builtin::THEMES {
            let theme = theme_by_name(name).expect("bundled theme not found");
            assert_selection_invariant(&theme, name);
        }
    }

    #[test]
    fn adversarial_selection_close_to_red_text_still_legible() {
        // Construct a synthetic theme whose `selection` background is a
        // strong red, the same hue as theme.foreground, then assert the
        // recomputed selection_foreground still satisfies the invariant.
        // This is the canonical "user picked a clashing selection color"
        // failure mode US-007 must guard against.
        let mut theme = paneflow_dark();
        theme.foreground = h(0xff0000); // bright red text
        theme.selection = ha(0xff0000, 0.4); // selection of the same hue
        theme.recompute_selection_foreground();
        assert_selection_invariant(&theme, "adversarial-red-on-red");
    }

    #[test]
    fn adversarial_selection_close_to_white_on_light_theme() {
        // Light theme + near-white selection background. The algorithm must
        // pick a dark foreground for legibility.
        let mut theme = paneflow_light();
        theme.foreground = h(0xeeeeee); // near-white text
        theme.selection = ha(0xf0f0f0, 0.5); // very pale selection
        theme.recompute_selection_foreground();
        assert_selection_invariant(&theme, "adversarial-white-on-light");
    }

    #[test]
    fn vc_diff_slots_distinct_with_subtle_zed_alpha_backgrounds() {
        // US-007 (prd-git-diff-mode-2026-Q3.md): the curated diff slots are
        // distinct hues. The line-wash backgrounds are subtle (Zed's
        // editor_diff_hunk_*_background: 0.12 dark / 0.16 light) - the opaque
        // gutter hunk bar carries the strong status signal, not the wash.
        let dark = ui_colors_with(&paneflow_dark());
        assert_ne!(dark.vc_added, dark.vc_deleted);
        assert_ne!(dark.vc_added, dark.vc_modified);
        assert_ne!(dark.vc_deleted, dark.vc_modified);
        for bg in [
            dark.vc_added_background,
            dark.vc_deleted_background,
            dark.vc_modified_background,
        ] {
            assert!(
                (bg.a - 0.12).abs() < 1e-6,
                "dark diff background alpha must be 0.12, got {}",
                bg.a
            );
        }
        // The light theme resolves a distinct (darker, saturated) palette with
        // a slightly stronger wash to read on the light editor surface.
        let light = ui_colors_with(&paneflow_light());
        assert_ne!(light.vc_added, dark.vc_added);
        for bg in [
            light.vc_added_background,
            light.vc_deleted_background,
            light.vc_modified_background,
        ] {
            assert!(
                (bg.a - 0.16).abs() < 1e-6,
                "light diff background alpha must be 0.16, got {}",
                bg.a
            );
        }

        let vercel = ui_colors_with(&vercel_dark());
        let diff = vercel.diff_colors();
        assert_eq!(diff.added, vercel.vc_added);
        assert_eq!(diff.deleted, vercel.vc_deleted);
        assert_eq!(diff.added_background, vercel.vc_added_background);

        let claude = ui_colors_with(&claude_dark());
        let diff = claude.diff_colors();
        let canonical_dark_diff = dark.diff_colors();
        assert_eq!(diff.added, canonical_dark_diff.added);
        assert_eq!(diff.deleted, canonical_dark_diff.deleted);
        assert_eq!(diff.added_background, canonical_dark_diff.added_background);
        assert_eq!(
            diff.deleted_background,
            canonical_dark_diff.deleted_background
        );
        assert_eq!(claude.vc_modified, dark.vc_modified);

        let cursor = ui_colors_with(&cursor_dark());
        let diff = cursor.diff_colors();
        assert_eq!(diff.added, canonical_dark_diff.added);
        assert_eq!(diff.deleted, canonical_dark_diff.deleted);
        assert_eq!(diff.added_background, canonical_dark_diff.added_background);
        assert_eq!(
            diff.deleted_background,
            canonical_dark_diff.deleted_background
        );
        assert_eq!(cursor.vc_modified, dark.vc_modified);
    }

    #[test]
    fn recompute_is_idempotent() {
        // Running the recompute twice must yield the same value - guards
        // against accidental mutation of `foreground` or `selection` during
        // the algorithm.
        let mut theme = paneflow_dark();
        theme.recompute_selection_foreground();
        let first = theme.selection_foreground;
        theme.recompute_selection_foreground();
        let second = theme.selection_foreground;
        assert_eq!(first, second);
    }

    // ------------------------------------------------------------------
    // EP-001 / US-001 + US-007 (prd-diff-syntax-palette-2026-Q3.md):
    // the per-language syntax palette must be richly populated on bundled
    // themes and stay readable on the light theme.
    // ------------------------------------------------------------------

    /// Count pairwise-distinct colors. `Hsla` is `PartialEq` but neither `Eq`
    /// nor `Hash`, so a `HashSet` is out; O(n²) over 30 slots is trivial.
    fn distinct_count(colors: &[Hsla]) -> usize {
        let mut seen: Vec<Hsla> = Vec::new();
        for &c in colors {
            if !seen.contains(&c) {
                seen.push(c);
            }
        }
        seen.len()
    }

    #[test]
    fn bundled_themes_populate_at_least_18_distinct_syntax_hues() {
        // US-001 AC #1/#5: ≥ 18 distinct color values per theme (up from 8).
        for (label, theme) in [
            ("Paneflow Dark", paneflow_dark()),
            ("Paneflow Light", paneflow_light()),
            ("Vercel Dark", vercel_dark()),
            ("Vercel Light", vercel_light()),
            ("Claude Dark", claude_dark()),
            ("Claude Light", claude_light()),
            ("Cursor Dark", cursor_dark()),
            ("Cursor Light", cursor_light()),
        ] {
            let distinct = distinct_count(&theme.syntax.all_slots());
            assert!(
                distinct >= 18,
                "{label}: syntax palette has only {distinct} distinct hues (< 18)"
            );
        }
    }

    #[test]
    fn no_syntax_slot_equals_default_or_foreground() {
        // US-001 AC #2/#5 (unhappy path): a slot left at the default `Hsla`
        // (transparent black) or equal to the theme foreground would render
        // that token family invisible / indistinguishable from plain text.
        let default = Hsla::default();
        for (label, theme) in [
            ("Paneflow Dark", paneflow_dark()),
            ("Paneflow Light", paneflow_light()),
            ("Vercel Dark", vercel_dark()),
            ("Vercel Light", vercel_light()),
            ("Claude Dark", claude_dark()),
            ("Claude Light", claude_light()),
            ("Cursor Dark", cursor_dark()),
            ("Cursor Light", cursor_light()),
        ] {
            for (i, slot) in theme.syntax.all_slots().iter().enumerate() {
                assert_ne!(*slot, default, "{label}: syntax slot #{i} left at default");
                assert_ne!(
                    *slot, theme.foreground,
                    "{label}: syntax slot #{i} equals foreground"
                );
            }
        }
    }

    #[test]
    fn light_theme_comment_and_punctuation_perceptibly_off_foreground() {
        // US-001 AC #4: on the light theme, comment and punctuation must clear
        // a perceptible APCA margin from the row foreground (not just `!=`).
        let theme = paneflow_light();
        for (slot_label, slot) in [
            ("comment", theme.syntax.comment),
            ("punctuation", theme.syntax.punctuation),
        ] {
            let lc = apca_contrast(slot, theme.foreground).abs();
            assert!(
                lc > 5.0,
                "Latte: {slot_label} too close to foreground (APCA Lc {lc:.1})"
            );
        }
    }

    #[test]
    fn latte_core_slots_distinct_and_clear_of_background() {
        // US-007 AC #1/#2: the highest-traffic families (comment / string /
        // keyword / operator) are mutually distinct and distinct from
        // foreground on the light theme; and NO family near-equals the light
        // editor background (a slot collapsing onto bg = invisible category).
        let theme = paneflow_light();
        let p = theme.syntax;
        let core = [p.comment, p.string, p.keyword, p.operator];
        assert_eq!(
            distinct_count(&core),
            4,
            "Latte: comment/string/keyword/operator not mutually distinct"
        );
        for c in core {
            assert_ne!(c, theme.foreground, "Latte: core slot equals foreground");
        }
        for (i, slot) in p.all_slots().iter().enumerate() {
            let lc = apca_contrast(*slot, theme.background).abs();
            assert!(
                lc > 5.0,
                "Latte: syntax slot #{i} too close to background (APCA Lc {lc:.1})"
            );
        }
    }
}
