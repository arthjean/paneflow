use super::model::{SyntaxPalette, TerminalTheme, UiColors, h, ha};

pub type ThemeEntry = (&'static str, fn() -> TerminalTheme);

pub static THEMES: &[ThemeEntry] = &[
    ("Paneflow Dark", paneflow_dark),
    ("Paneflow Light", paneflow_light),
    ("Vercel Dark", vercel_dark),
    ("Vercel Light", vercel_light),
    ("Claude Dark", claude_dark),
    ("Claude Light", claude_light),
    ("Cursor Dark", cursor_dark),
    ("Cursor Light", cursor_light),
];

pub const DEFAULT_THEME: &str = "Paneflow Dark";

pub struct ThemePreset {
    pub name: &'static str,
    pub light: &'static str,
    pub dark: &'static str,
}

impl ThemePreset {
    pub fn variant(&self, is_light: bool) -> &'static str {
        if is_light { self.light } else { self.dark }
    }
}

pub static PRESETS: &[ThemePreset] = &[
    ThemePreset {
        name: "Paneflow",
        light: "Paneflow Light",
        dark: "Paneflow Dark",
    },
    ThemePreset {
        name: "Vercel",
        light: "Vercel Light",
        dark: "Vercel Dark",
    },
    ThemePreset {
        name: "Claude",
        light: "Claude Light",
        dark: "Claude Dark",
    },
    ThemePreset {
        name: "Cursor",
        light: "Cursor Light",
        dark: "Cursor Dark",
    },
];

static LEGACY_THEME_ALIASES: &[(&str, &str)] = &[
    ("One Dark", "Paneflow Dark"),
    ("PaneFlow Light", "Paneflow Light"),
    ("Vercel", "Vercel Dark"),
    ("Claude", "Claude Dark"),
    ("Cursor", "Cursor Dark"),
];

pub fn canonical_theme_name(name: &str) -> Option<&'static str> {
    THEMES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(n, _)| *n)
        .or_else(|| {
            LEGACY_THEME_ALIASES
                .iter()
                .find(|(legacy, _)| legacy.eq_ignore_ascii_case(name))
                .map(|(_, canonical)| *canonical)
        })
}

pub fn preset_for_theme(theme_name: &str) -> &'static ThemePreset {
    let name = canonical_theme_name(theme_name).unwrap_or(DEFAULT_THEME);
    PRESETS
        .iter()
        .find(|p| p.light == name || p.dark == name)
        .unwrap_or(&PRESETS[0])
}

pub fn preset_by_name(name: &str) -> Option<&'static ThemePreset> {
    PRESETS.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

pub fn theme_name_is_light(theme_name: &str) -> bool {
    canonical_theme_name(theme_name).is_some_and(|name| PRESETS.iter().any(|p| p.light == name))
}

pub fn paneflow_light() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: None,
        background: h(0xffffff),
        foreground: h(0x25262b),
        bright_foreground: h(0x25262b),
        dim_foreground: h(0x777984),
        ansi_background: h(0xffffff),
        cursor: h(0x007aff),
        selection: ha(0x4c6fff, 0.20),
        selection_foreground: gpui::Hsla::default(),
        scrollbar_thumb: ha(0x25262b, 0.28),
        link_text: h(0x315ecf),
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
        syntax: SyntaxPalette::catppuccin_latte(),
    };
    theme.recompute_selection_foreground();
    theme
}

pub fn paneflow_dark() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: None,
        background: h(0x282c34),
        foreground: h(0xc5d1cc),
        bright_foreground: h(0xf2eee8),
        dim_foreground: h(0x93938b),
        ansi_background: h(0x282c34),
        cursor: h(0x007aff),
        selection: ha(0x5aa6ff, 0.22),
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
        syntax: SyntaxPalette::catppuccin_mocha(),
    };
    theme.recompute_selection_foreground();
    theme
}

pub fn vercel_dark() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: Some(vercel_dark_ui()),
        background: h(0x000000),
        foreground: h(0xededed),
        bright_foreground: h(0xffffff),
        dim_foreground: h(0x8a8a8a),
        ansi_background: h(0x000000),
        cursor: h(0xffffff),
        selection: ha(0xffffff, 0.18),
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
        syntax: SyntaxPalette::vercel_dark(),
    };
    theme.recompute_selection_foreground();
    theme
}

fn vercel_dark_ui() -> UiColors {
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

pub fn vercel_light() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: Some(vercel_light_ui()),
        background: h(0xffffff),
        foreground: h(0x171717),
        bright_foreground: h(0x000000),
        dim_foreground: h(0x666666),
        ansi_background: h(0xffffff),
        cursor: h(0x000000),
        selection: ha(0x0068d6, 0.16),
        selection_foreground: gpui::Hsla::default(),
        scrollbar_thumb: ha(0x171717, 0.24),
        link_text: h(0x0068d6),
        title_bar_background: h(0xfafafa),
        title_bar_inactive_background: h(0xf5f5f5),
        black: h(0x000000),
        red: h(0xcd2b31),
        green: h(0x0f7b0f),
        yellow: h(0xa35200),
        blue: h(0x0068d6),
        magenta: h(0x7820bc),
        cyan: h(0x067f6f),
        white: h(0x666666),
        bright_black: h(0x4d4d4d),
        bright_red: h(0xb3242a),
        bright_green: h(0x0b660b),
        bright_yellow: h(0x8a4500),
        bright_blue: h(0x0058b3),
        bright_magenta: h(0x651a9e),
        bright_cyan: h(0x056b5e),
        bright_white: h(0x171717),
        syntax: SyntaxPalette::vercel_light(),
    };
    theme.recompute_selection_foreground();
    theme
}

fn vercel_light_ui() -> UiColors {
    UiColors {
        use_theme_diff_washes: true,
        base: h(0xffffff),
        surface: h(0xfafafa),
        overlay: h(0xffffff),
        border: h(0xeaeaea),
        subtle: h(0xf2f2f2),
        muted: h(0x666666),
        text: h(0x171717),
        accent: h(0x000000),
        tool_card_header_bg: h(0xf0f0f0),
        vc_added: h(0x0f7b0f),
        vc_modified: h(0xa35200),
        vc_deleted: h(0xcd2b31),
        vc_conflict: h(0xbd4b00),
        vc_added_background: ha(0x0f7b0f, 0.16),
        vc_deleted_background: ha(0xcd2b31, 0.16),
        vc_modified_background: ha(0xa35200, 0.16),
        vc_word_added: ha(0x0f7b0f, 0.42),
        vc_word_deleted: ha(0xcd2b31, 0.42),
        group_1: h(0x000000),
        group_2: h(0x0068d6),
        group_3: h(0x0f7b0f),
        group_4: h(0xa35200),
        group_5: h(0x7820bc),
        group_6: h(0x067f6f),
        group_7: h(0xcd2b31),
        group_8: h(0x666666),
        agent_error: h(0xcd2b31),
        agent_stalled: h(0x666666),
        agent_claude: h(0xbd4b00),
        agent_codex: h(0x0068d6),
    }
}

pub fn claude_dark() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: Some(claude_dark_ui()),
        background: h(0x1f1f1e),
        foreground: h(0xf0f3f7),
        bright_foreground: h(0xffffff),
        dim_foreground: h(0x9ca7b5),
        ansi_background: h(0x1f1f1e),
        cursor: h(0xd97757),
        selection: ha(0x5a5a5a, 0.45),
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
        syntax: SyntaxPalette::claude_dark(),
    };
    theme.recompute_selection_foreground();
    theme
}

fn claude_dark_ui() -> UiColors {
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

pub fn claude_light() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: Some(claude_light_ui()),
        background: h(0xfaf9f5),
        foreground: h(0x3d3929),
        bright_foreground: h(0x1f1e1a),
        dim_foreground: h(0x83827d),
        ansi_background: h(0xfaf9f5),
        cursor: h(0xd97757),
        selection: ha(0xd97757, 0.20),
        selection_foreground: gpui::Hsla::default(),
        scrollbar_thumb: ha(0x3d3929, 0.24),
        link_text: h(0xc2552f),
        title_bar_background: h(0xf5f4ee),
        title_bar_inactive_background: h(0xf2f1ea),
        black: h(0x3d3929),
        red: h(0xc2552f),
        green: h(0x4f7a3f),
        yellow: h(0x9a6b1f),
        blue: h(0x4a6fa5),
        magenta: h(0x7a5ea8),
        cyan: h(0x2f7d72),
        white: h(0x83827d),
        bright_black: h(0x5c5949),
        bright_red: h(0xd9663c),
        bright_green: h(0x5f8f4c),
        bright_yellow: h(0xb07f28),
        bright_blue: h(0x5a82bb),
        bright_magenta: h(0x8f70bf),
        bright_cyan: h(0x389184),
        bright_white: h(0x1f1e1a),
        syntax: SyntaxPalette::claude_light(),
    };
    theme.recompute_selection_foreground();
    theme
}

fn claude_light_ui() -> UiColors {
    UiColors {
        use_theme_diff_washes: false,
        base: h(0xfaf9f5),
        surface: h(0xffffff),
        overlay: h(0xffffff),
        border: h(0xe5e2d9),
        subtle: h(0xf0eee6),
        muted: h(0x83827d),
        text: h(0x3d3929),
        accent: h(0xd97757),
        tool_card_header_bg: h(0xf3f0e7),
        vc_added: h(0x40a02b),
        vc_modified: h(0xdf8e1d),
        vc_deleted: h(0xd20f39),
        vc_conflict: h(0xfe640b),
        vc_added_background: ha(0x40a02b, 0.16),
        vc_deleted_background: ha(0xd20f39, 0.16),
        vc_modified_background: ha(0xdf8e1d, 0.16),
        vc_word_added: ha(0x40a02b, 0.40),
        vc_word_deleted: ha(0xd20f39, 0.40),
        group_1: h(0xd97757),
        group_2: h(0x4a6fa5),
        group_3: h(0x4f7a3f),
        group_4: h(0x9a6b1f),
        group_5: h(0x7a5ea8),
        group_6: h(0x2f7d72),
        group_7: h(0xc2552f),
        group_8: h(0x83827d),
        agent_error: h(0xd20f39),
        agent_stalled: h(0x83827d),
        agent_claude: h(0xd97757),
        agent_codex: h(0x4a6fa5),
    }
}

pub fn cursor_dark() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: Some(cursor_dark_ui()),
        background: h(0x141414),
        foreground: h(0xc5d1cc),
        bright_foreground: h(0xffffff),
        dim_foreground: h(0x989898),
        ansi_background: h(0x141414),
        cursor: h(0xa0d0f0),
        selection: ha(0xa0d0f0, 0.24),
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
        syntax: SyntaxPalette::cursor_dark(),
    };
    theme.recompute_selection_foreground();
    theme
}

fn cursor_dark_ui() -> UiColors {
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

pub fn cursor_light() -> TerminalTheme {
    let mut theme = TerminalTheme {
        ui: Some(cursor_light_ui()),
        background: h(0xffffff),
        foreground: h(0x1e1e1e),
        bright_foreground: h(0x000000),
        dim_foreground: h(0x767676),
        ansi_background: h(0xffffff),
        cursor: h(0x0f6fc5),
        selection: ha(0x0f6fc5, 0.18),
        selection_foreground: gpui::Hsla::default(),
        scrollbar_thumb: ha(0x1e1e1e, 0.24),
        link_text: h(0x0f6fc5),
        title_bar_background: h(0xf5f5f5),
        title_bar_inactive_background: h(0xf0f0f0),
        black: h(0x000000),
        red: h(0xcd3131),
        green: h(0x107c10),
        yellow: h(0x949800),
        blue: h(0x0451a5),
        magenta: h(0xbc05bc),
        cyan: h(0x0598bc),
        white: h(0x555555),
        bright_black: h(0x666666),
        bright_red: h(0xa81b1b),
        bright_green: h(0x0d640d),
        bright_yellow: h(0x7a7d00),
        bright_blue: h(0x033b78),
        bright_magenta: h(0x8f048f),
        bright_cyan: h(0x047394),
        bright_white: h(0x1e1e1e),
        syntax: SyntaxPalette::cursor_light(),
    };
    theme.recompute_selection_foreground();
    theme
}

fn cursor_light_ui() -> UiColors {
    UiColors {
        use_theme_diff_washes: false,
        base: h(0xffffff),
        surface: h(0xf5f5f5),
        overlay: h(0xffffff),
        border: h(0xe0e0e0),
        subtle: h(0xececec),
        muted: h(0x767676),
        text: h(0x1e1e1e),
        accent: h(0x0f6fc5),
        tool_card_header_bg: h(0xeaeaea),
        vc_added: h(0x40a02b),
        vc_modified: h(0xdf8e1d),
        vc_deleted: h(0xd20f39),
        vc_conflict: h(0xfe640b),
        vc_added_background: ha(0x40a02b, 0.16),
        vc_deleted_background: ha(0xd20f39, 0.16),
        vc_modified_background: ha(0xdf8e1d, 0.16),
        vc_word_added: ha(0x40a02b, 0.40),
        vc_word_deleted: ha(0xd20f39, 0.40),
        group_1: h(0x0f6fc5),
        group_2: h(0x0451a5),
        group_3: h(0x40a02b),
        group_4: h(0xdf8e1d),
        group_5: h(0x8839ef),
        group_6: h(0x0598bc),
        group_7: h(0xd20f39),
        group_8: h(0x555555),
        agent_error: h(0xd20f39),
        agent_stalled: h(0x767676),
        agent_claude: h(0xfe640b),
        agent_codex: h(0x0f6fc5),
    }
}

pub fn theme_by_name(name: &str) -> Option<TerminalTheme> {
    let canonical = canonical_theme_name(name)?;
    THEMES.iter().find(|(n, _)| *n == canonical).map(|(_, f)| {
        let mut theme = f();
        theme.recompute_selection_foreground();
        theme
    })
}
