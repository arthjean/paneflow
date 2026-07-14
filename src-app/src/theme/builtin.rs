//! Bundled terminal themes and the registry used to look them up by name.

use super::model::{SyntaxPalette, TerminalTheme, UiColors, h, ha};

pub type ThemeEntry = (&'static str, fn() -> TerminalTheme);

pub static THEMES: &[ThemeEntry] = &[
    ("One Dark", one_dark),
    ("PaneFlow Light", paneflow_light),
    ("Vercel", vercel),
    ("Claude", claude),
    ("Cursor", cursor),
];

pub fn paneflow_light() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: None,
        // Keep the work surface fully opaque and exactly white. Native
        // translucency is reserved for the navigation rail and title bar.
        background: h(0xffffff),
        foreground: h(0x25262b),
        bright_foreground: h(0x25262b),
        dim_foreground: h(0x777984),
        ansi_background: h(0xffffff),
        cursor: h(0x007aff),
        selection: ha(0x4c6fff, 0.20),
        // Filled below before returning the public theme value.
        selection_foreground: gpui::Hsla::default(),
        scrollbar_thumb: ha(0x25262b, 0.28),
        link_text: h(0x315ecf),
        // Opaque fallback for Linux compositors without blur support.
        title_bar_background: h(0xf3f4f9),
        title_bar_inactive_background: h(0xf5f5f8),
        black: h(0x383a42),
        red: h(0xe45649),
        green: h(0x50a14f),
        yellow: h(0xc18401),
        blue: h(0x4078f2),
        magenta: h(0xa626a4),
        cyan: h(0x0184bc),
        white: h(0xa0a1a7),
        bright_black: h(0x696c77),
        bright_red: h(0xd23d2d),
        bright_green: h(0x3e8a3e),
        bright_yellow: h(0xa67200),
        bright_blue: h(0x2e64d4),
        bright_magenta: h(0x8b1b8b),
        bright_cyan: h(0x016e9e),
        bright_white: h(0x383a42),
        dim_black: h(0xb0b1b5),
        dim_red: h(0xc9887f),
        dim_green: h(0x8ab88a),
        dim_yellow: h(0xb5a06a),
        dim_blue: h(0x7da0d6),
        dim_magenta: h(0xb881b8),
        dim_cyan: h(0x6aa9c0),
        dim_white: h(0x777984),
        syntax: SyntaxPalette::catppuccin_latte(),
    };
    theme.recompute_selection_foreground();
    theme
}

pub fn one_dark() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: None,
        background: h(0x282c34),
        // Keep One Dark surfaces while using the same softer terminal text
        // palette as Cursor and the Windows Terminal reference.
        foreground: h(0xc5d1cc),
        bright_foreground: h(0xf2eee8),
        dim_foreground: h(0x93938b),
        ansi_background: h(0x282c34),
        cursor: h(0x007aff),
        selection: ha(0x5aa6ff, 0.22),
        // Filled below before returning the public theme value.
        selection_foreground: gpui::Hsla::default(),
        scrollbar_thumb: ha(0x9aa8bd, 0.30),
        link_text: h(0x57d5c4),
        title_bar_background: h(0x21252b),
        title_bar_inactive_background: h(0x1b1f23),
        black: h(0x0f1a1d),
        red: h(0xd4555e),
        green: h(0x56b68a),
        yellow: h(0xd4a35c),
        blue: h(0x5b8fb4),
        magenta: h(0xb578b4),
        cyan: h(0x4dc4b0),
        white: h(0xb8c8c0),
        bright_black: h(0x3a4f4a),
        bright_red: h(0xe87878),
        bright_green: h(0x7ddba8),
        bright_yellow: h(0xe8c478),
        bright_blue: h(0x7cb3d4),
        bright_magenta: h(0xd09acf),
        bright_cyan: h(0x72dbc9),
        bright_white: h(0xe0ece6),
        dim_black: h(0x2c313a),
        dim_red: h(0xb85659),
        dim_green: h(0x3f9d74),
        dim_yellow: h(0xb4934b),
        dim_blue: h(0x5a82b8),
        dim_magenta: h(0x906cb8),
        dim_cyan: h(0x3d9b91),
        dim_white: h(0x93938b),
        syntax: SyntaxPalette::catppuccin_mocha(),
    };
    theme.recompute_selection_foreground();
    theme
}

pub fn vercel() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: Some(vercel_ui()),
        background: h(0x000000),
        foreground: h(0xededed),
        bright_foreground: h(0xffffff),
        dim_foreground: h(0x8a8a8a),
        ansi_background: h(0x000000),
        cursor: h(0xffffff),
        selection: ha(0xffffff, 0.18),
        // Filled below before returning the public theme value.
        selection_foreground: gpui::Hsla::default(),
        scrollbar_thumb: ha(0xffffff, 0.24),
        link_text: h(0x3291ff),
        title_bar_background: h(0x000000),
        title_bar_inactive_background: h(0x050505),
        black: h(0x000000),
        red: h(0xff5a5f),
        green: h(0x00d084),
        yellow: h(0xf5d90a),
        blue: h(0x3291ff),
        magenta: h(0xbf5af2),
        cyan: h(0x50e3c2),
        white: h(0xe5e5e5),
        bright_black: h(0x666666),
        bright_red: h(0xff7a7f),
        bright_green: h(0x33e29f),
        bright_yellow: h(0xffe45c),
        bright_blue: h(0x5eb0ff),
        bright_magenta: h(0xd78cff),
        bright_cyan: h(0x7ff0d8),
        bright_white: h(0xffffff),
        dim_black: h(0x111111),
        dim_red: h(0xa84145),
        dim_green: h(0x16855f),
        dim_yellow: h(0xa89619),
        dim_blue: h(0x2166ad),
        dim_magenta: h(0x7f3aa6),
        dim_cyan: h(0x358f7c),
        dim_white: h(0x8a8a8a),
        syntax: SyntaxPalette::vercel_dark(),
    };
    theme.recompute_selection_foreground();
    theme
}

fn vercel_ui() -> UiColors {
    UiColors {
        use_theme_diff_washes: true,
        base: h(0x000000),
        surface: h(0x0a0a0a),
        overlay: h(0x111111),
        border: h(0x2a2a2a),
        subtle: h(0x1a1a1a),
        muted: h(0x8a8a8a),
        text: h(0xededed),
        accent: h(0xffffff),
        tool_card_header_bg: h(0x161616),
        vc_added: h(0x00d084),
        vc_modified: h(0xf5a623),
        vc_deleted: h(0xff5a5f),
        vc_conflict: h(0xff7a18),
        vc_added_background: ha(0x00d084, 0.16),
        vc_deleted_background: ha(0xff5a5f, 0.16),
        vc_modified_background: ha(0xf5a623, 0.16),
        vc_word_added: ha(0x00d084, 0.42),
        vc_word_deleted: ha(0xff5a5f, 0.42),
        group_1: h(0xffffff),
        group_2: h(0x3291ff),
        group_3: h(0x00d084),
        group_4: h(0xf5a623),
        group_5: h(0xbf5af2),
        group_6: h(0x50e3c2),
        group_7: h(0xff5a5f),
        group_8: h(0x999999),
        agent_error: h(0xff5a5f),
        agent_stalled: h(0x8a8a8a),
        agent_claude: h(0xff7a18),
        agent_codex: h(0x3291ff),
    }
}

pub fn claude() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: Some(claude_ui()),
        background: h(0x1f1f1e),
        foreground: h(0xf0f3f7),
        bright_foreground: h(0xffffff),
        dim_foreground: h(0x9ca7b5),
        ansi_background: h(0x1f1f1e),
        cursor: h(0xd97757),
        selection: ha(0x5a5a5a, 0.45),
        // Filled below before returning the public theme value.
        selection_foreground: gpui::Hsla::default(),
        scrollbar_thumb: ha(0xc3c2b7, 0.24),
        link_text: h(0xc3c2b7),
        title_bar_background: h(0x1f1f1e),
        title_bar_inactive_background: h(0x1e1e1d),
        black: h(0x262626),
        red: h(0xd97757),
        green: h(0x9ab38a),
        yellow: h(0xc3a45f),
        blue: h(0x8fa4b8),
        magenta: h(0xb8a1c8),
        cyan: h(0x95b8b2),
        white: h(0xf0f3f7),
        bright_black: h(0x5a5a5a),
        bright_red: h(0xe68a6d),
        bright_green: h(0xb1c79e),
        bright_yellow: h(0xd5b976),
        bright_blue: h(0xa9b8c8),
        bright_magenta: h(0xcab7d7),
        bright_cyan: h(0xabcac4),
        bright_white: h(0xffffff),
        dim_black: h(0x1e1e1d),
        dim_red: h(0x985d49),
        dim_green: h(0x6f8066),
        dim_yellow: h(0x8f794d),
        dim_blue: h(0x687887),
        dim_magenta: h(0x7f708c),
        dim_cyan: h(0x6d8581),
        dim_white: h(0x9ca7b5),
        syntax: SyntaxPalette::claude_dark(),
    };
    theme.recompute_selection_foreground();
    theme
}

fn claude_ui() -> UiColors {
    UiColors {
        use_theme_diff_washes: false,
        base: h(0x1f1f1e),
        surface: h(0x262626),
        overlay: h(0x2c2c2a),
        border: h(0x333333),
        subtle: h(0x363635),
        muted: h(0x93938b),
        text: h(0xe3dacc),
        accent: h(0xd97757),
        tool_card_header_bg: h(0x313131),
        // Keep Paneflow's canonical dark diff/status hues. Claude only changes
        // the surrounding surfaces; Review/sidebar red, green and yellow must
        // remain stable across dark themes.
        vc_added: h(0x57d992),
        vc_modified: h(0xffd166),
        vc_deleted: h(0xff6f6a),
        vc_conflict: h(0xffa657),
        vc_added_background: ha(0x57d992, 0.12),
        vc_deleted_background: ha(0xff6f6a, 0.12),
        vc_modified_background: ha(0xffd166, 0.12),
        vc_word_added: ha(0x57d992, 0.40),
        vc_word_deleted: ha(0xff6f6a, 0.40),
        group_1: h(0xd97757),
        group_2: h(0x8fa4b8),
        group_3: h(0x9ab38a),
        group_4: h(0xc3a45f),
        group_5: h(0xb8a1c8),
        group_6: h(0x95b8b2),
        group_7: h(0xe68a6d),
        group_8: h(0xc3c2b7),
        agent_error: h(0xd97757),
        agent_stalled: h(0x93938b),
        agent_claude: h(0xd97757),
        agent_codex: h(0x8fa4b8),
    }
}

pub fn cursor() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: Some(cursor_ui()),
        background: h(0x141414),
        // Match the softer "Northern Forest" Windows Terminal text palette.
        // Cursor surfaces and backgrounds stay unchanged; only terminal
        // foreground and ANSI slots are aligned.
        foreground: h(0xc5d1cc),
        bright_foreground: h(0xffffff),
        dim_foreground: h(0x989898),
        ansi_background: h(0x141414),
        cursor: h(0xa0d0f0),
        selection: ha(0xa0d0f0, 0.24),
        // Filled below before returning the public theme value.
        selection_foreground: gpui::Hsla::default(),
        scrollbar_thumb: ha(0xf0f0f0, 0.22),
        link_text: h(0xa0d0f0),
        title_bar_background: h(0x181818),
        title_bar_inactive_background: h(0x181818),
        black: h(0x0f1a1d),
        red: h(0xd4555e),
        green: h(0x56b68a),
        yellow: h(0xd4a35c),
        blue: h(0x5b8fb4),
        magenta: h(0xb578b4),
        cyan: h(0x4dc4b0),
        white: h(0xb8c8c0),
        bright_black: h(0x3a4f4a),
        bright_red: h(0xe87878),
        bright_green: h(0x7ddba8),
        bright_yellow: h(0xe8c478),
        bright_blue: h(0x7cb3d4),
        bright_magenta: h(0xd09acf),
        bright_cyan: h(0x72dbc9),
        bright_white: h(0xe0ece6),
        dim_black: h(0x101010),
        dim_red: h(0xb85659),
        dim_green: h(0x3f9d74),
        dim_yellow: h(0xb4934b),
        dim_blue: h(0x648098),
        dim_magenta: h(0x906cb8),
        dim_cyan: h(0x507490),
        dim_white: h(0x989898),
        syntax: SyntaxPalette::cursor_dark(),
    };
    theme.recompute_selection_foreground();
    theme
}

fn cursor_ui() -> UiColors {
    UiColors {
        use_theme_diff_washes: false,
        base: h(0x141414),
        surface: h(0x181818),
        overlay: h(0x202020),
        border: h(0x292929),
        subtle: h(0x242424),
        muted: h(0x989898),
        text: h(0xf0f0f0),
        accent: h(0xa0d0f0),
        tool_card_header_bg: h(0x242424),
        // Keep Paneflow's canonical dark diff/status hues. Cursor only changes
        // the surrounding IDE surfaces; Review/sidebar status colors stay
        // stable across dark themes.
        vc_added: h(0x57d992),
        vc_modified: h(0xffd166),
        vc_deleted: h(0xff6f6a),
        vc_conflict: h(0xffa657),
        vc_added_background: ha(0x57d992, 0.12),
        vc_deleted_background: ha(0xff6f6a, 0.12),
        vc_modified_background: ha(0xffd166, 0.12),
        vc_word_added: ha(0x57d992, 0.40),
        vc_word_deleted: ha(0xff6f6a, 0.40),
        group_1: h(0xa0d0f0),
        group_2: h(0x4074e0),
        group_3: h(0x57d992),
        group_4: h(0xffd166),
        group_5: h(0xc79bff),
        group_6: h(0x7dd3fc),
        group_7: h(0xff6f6a),
        group_8: h(0xf0f0f0),
        agent_error: h(0xff6f6a),
        agent_stalled: h(0x989898),
        agent_claude: h(0xffa657),
        agent_codex: h(0xa0d0f0),
    }
}

/// Look up a bundled theme by name (case-insensitive).
///
/// Returns a finalized theme: `selection_foreground` is computed via APCA
/// before return. Callers may further modify the theme (e.g. via
/// `apply_surface_overrides`) - that path also re-runs the recomputation,
/// so the invariant `apca_contrast(selection_foreground, selection) ≥ 45.0`
/// holds at every observation point.
pub fn theme_by_name(name: &str) -> Option<TerminalTheme> {
    let name_lower = name.to_lowercase();
    THEMES
        .iter()
        .find(|(n, _)| n.to_lowercase() == name_lower)
        .map(|(_, f)| {
            let mut theme = f();
            theme.recompute_selection_foreground();
            theme
        })
}
