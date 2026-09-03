use gpui::{Hsla, Rgba};

use crate::terminal::element::{MIN_APCA_CONTRAST, ensure_minimum_contrast};

#[derive(Clone, Copy)]
pub struct TerminalTheme {
    pub ui: Option<UiColors>,
    pub background: Hsla,
    pub foreground: Hsla,
    pub bright_foreground: Hsla,
    pub dim_foreground: Hsla,
    pub ansi_background: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
    pub selection_foreground: Hsla,
    pub scrollbar_thumb: Hsla,
    pub link_text: Hsla,
    pub title_bar_background: Hsla,
    pub title_bar_inactive_background: Hsla,
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
    pub syntax: SyntaxPalette,
}

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

    pub fn catppuccin_latte() -> Self {
        Self {
            comment: h(0x9ca0b0),
            comment_doc: h(0x8c8fa1),
            keyword: h(0x8839ef),
            function: h(0x1e66f5),
            r#type: h(0x179299),
            r#enum: h(0x179299),
            constructor: h(0x1e66f5),
            string: h(0x40a02b),
            string_escape: h(0x04a5e5),
            string_special: h(0xea76cb),
            number: h(0xfe640b),
            boolean: h(0xfe640b),
            constant: h(0xdf8e1d),
            constant_builtin: h(0x209fb5),
            property: h(0xd20f39),
            variable: h(0x4c4f69),
            variable_builtin: h(0xfe640b),
            operator: h(0x04a5e5),
            punctuation: h(0x5c5f77),
            punctuation_special: h(0xe64553),
            attribute: h(0x1e66f5),
            tag: h(0xd20f39),
            label: h(0xdc8a78),
            namespace: h(0x7287fd),
            title: h(0xd20f39),
            text_literal: h(0x40a02b),
            link_uri: h(0x04a5e5),
            link_text: h(0x1e66f5),
            emphasis: h(0xe64553),
            emphasis_strong: h(0xdd7878),
        }
    }

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
const TERMINAL_BACKGROUND_HEX: u32 = 0x181818;
const BORDER_HEX: u32 = 0x252525;

fn is_light_theme(theme: &TerminalTheme) -> bool {
    theme.background.l > 0.5
}

impl TerminalTheme {
    pub(crate) fn recompute_selection_foreground(&mut self) {
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

#[derive(Clone, Copy)]
pub struct UiColors {
    pub use_theme_diff_washes: bool,
    pub base: Hsla,
    pub surface: Hsla,
    pub overlay: Hsla,
    pub border: Hsla,
    pub subtle: Hsla,
    pub muted: Hsla,
    pub text: Hsla,
    pub accent: Hsla,
    pub tool_card_header_bg: Hsla,
    pub vc_added: Hsla,
    pub vc_modified: Hsla,
    pub vc_deleted: Hsla,
    pub vc_conflict: Hsla,
    pub vc_added_background: Hsla,
    pub vc_deleted_background: Hsla,
    pub vc_modified_background: Hsla,
    pub vc_word_added: Hsla,
    pub vc_word_deleted: Hsla,
    pub group_1: Hsla,
    pub group_2: Hsla,
    pub group_3: Hsla,
    pub group_4: Hsla,
    pub group_5: Hsla,
    pub group_6: Hsla,
    pub group_7: Hsla,
    pub group_8: Hsla,
    pub agent_error: Hsla,
    pub agent_stalled: Hsla,
    pub agent_claude: Hsla,
    pub agent_codex: Hsla,
}

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
            added: h(0x57d992),
            deleted: h(0xff6f6a),
            added_background: h(0x1d3a2b),
            deleted_background: h(0x402425),
            added_gutter_background: h(0x16281f),
            deleted_gutter_background: h(0x2c1718),
        }
    }

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

pub fn ui_colors() -> UiColors {
    let theme = super::watcher::active_theme();
    ui_colors_with(&theme)
}

pub fn ui_colors_with(theme: &TerminalTheme) -> UiColors {
    if let Some(ui) = theme.ui {
        return ui;
    }

    let is_light = is_light_theme(theme);
    if is_light {
        UiColors {
            use_theme_diff_washes: false,
            base: h(0xffffff),
            surface: h(0xf7f7f7),
            overlay: h(0xffffff),
            border: h(0xe6e6e6),
            subtle: h(0xeeeeee),
            muted: h(0x6a6a6a),
            text: h(0x262626),
            accent: h(0x4c6fff),
            tool_card_header_bg: h(0xf1f1f1),
            vc_added: h(0x40a02b),
            vc_modified: h(0xdf8e1d),
            vc_deleted: h(0xd20f39),
            vc_conflict: h(0xfe640b),
            vc_added_background: ha(0x40a02b, 0.16),
            vc_deleted_background: ha(0xd20f39, 0.16),
            vc_modified_background: ha(0xdf8e1d, 0.16),
            vc_word_added: ha(0x40a02b, 0.40),
            vc_word_deleted: ha(0xd20f39, 0.40),
            group_1: h(0x1e66f5),
            group_2: h(0x40a02b),
            group_3: h(0xdf8e1d),
            group_4: h(0xd20f39),
            group_5: h(0x8839ef),
            group_6: h(0x179299),
            group_7: h(0xfe640b),
            group_8: h(0x7287fd),
            agent_error: h(0xd20f39),
            agent_stalled: h(0x808080),
            agent_claude: h(0xe89271),
            agent_codex: h(0x5b6cff),
        }
    } else {
        UiColors {
            use_theme_diff_washes: false,
            base: h(TERMINAL_BACKGROUND_HEX),
            surface: h(0x212121),
            overlay: h(CHROME_BACKGROUND_HEX),
            border: h(BORDER_HEX),
            subtle: h(0x2a2a2a),
            muted: h(0xa0a0a0),
            text: h(0xdddddd),
            accent: h(0x57d5c4),
            tool_card_header_bg: h(0x2e2e2e),
            vc_added: h(0x57d992),
            vc_modified: h(0xffd166),
            vc_deleted: h(0xff6f6a),
            vc_conflict: h(0xffa657),
            vc_added_background: ha(0x57d992, 0.12),
            vc_deleted_background: ha(0xff6f6a, 0.12),
            vc_modified_background: ha(0xffd166, 0.12),
            vc_word_added: ha(0x57d992, 0.40),
            vc_word_deleted: ha(0xff6f6a, 0.40),
            group_1: h(0x7eb6ff),
            group_2: h(0x57d992),
            group_3: h(0xffd166),
            group_4: h(0xff6f6a),
            group_5: h(0xc79bff),
            group_6: h(0x57d5c4),
            group_7: h(0xffa657),
            group_8: h(0x9ea7ff),
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
        for (name, _) in crate::theme::builtin::THEMES {
            let theme = theme_by_name(name).expect("bundled theme not found");
            assert_selection_invariant(&theme, name);
        }
    }

    #[test]
    fn adversarial_selection_close_to_red_text_still_legible() {
        let mut theme = paneflow_dark();
        theme.foreground = h(0xff0000);
        theme.selection = ha(0xff0000, 0.4);
        theme.recompute_selection_foreground();
        assert_selection_invariant(&theme, "adversarial-red-on-red");
    }

    #[test]
    fn adversarial_selection_close_to_white_on_light_theme() {
        let mut theme = paneflow_light();
        theme.foreground = h(0xeeeeee);
        theme.selection = ha(0xf0f0f0, 0.5);
        theme.recompute_selection_foreground();
        assert_selection_invariant(&theme, "adversarial-white-on-light");
    }

    #[test]
    fn vc_diff_slots_distinct_with_subtle_zed_alpha_backgrounds() {
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
        let mut theme = paneflow_dark();
        theme.recompute_selection_foreground();
        let first = theme.selection_foreground;
        theme.recompute_selection_foreground();
        let second = theme.selection_foreground;
        assert_eq!(first, second);
    }

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
