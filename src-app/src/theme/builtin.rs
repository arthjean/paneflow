//! Bundled terminal themes and the registry used to look them up by name.

use super::model::{SyntaxPalette, TerminalTheme, h, ha};

pub type ThemeEntry = (&'static str, fn() -> TerminalTheme);

pub static THEMES: &[ThemeEntry] = &[("One Dark", one_dark), ("PaneFlow Light", paneflow_light)];

pub fn paneflow_light() -> TerminalTheme {
    let mut theme = TerminalTheme {
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
        background: h(0x282c34),
        foreground: h(0xf0f3f7),
        bright_foreground: h(0xffffff),
        dim_foreground: h(0x9ca7b5),
        ansi_background: h(0x282c34),
        cursor: h(0x007aff),
        selection: ha(0x5aa6ff, 0.22),
        // Filled below before returning the public theme value.
        selection_foreground: gpui::Hsla::default(),
        scrollbar_thumb: ha(0x9aa8bd, 0.30),
        link_text: h(0x57d5c4),
        title_bar_background: h(0x21252b),
        title_bar_inactive_background: h(0x1b1f23),
        black: h(0x3f4451),
        red: h(0xff6f6a),
        green: h(0x57d992),
        yellow: h(0xffd166),
        blue: h(0x7eb6ff),
        magenta: h(0xc79bff),
        cyan: h(0x57d5c4),
        white: h(0xf0f3f7),
        bright_black: h(0x4f5666),
        bright_red: h(0xff8580),
        bright_green: h(0x72e3a5),
        bright_yellow: h(0xffdc80),
        bright_blue: h(0x9bc6ff),
        bright_magenta: h(0xd4b0ff),
        bright_cyan: h(0x72e0d2),
        bright_white: h(0xffffff),
        dim_black: h(0x2c313a),
        dim_red: h(0xb85659),
        dim_green: h(0x3f9d74),
        dim_yellow: h(0xb4934b),
        dim_blue: h(0x5a82b8),
        dim_magenta: h(0x906cb8),
        dim_cyan: h(0x3d9b91),
        dim_white: h(0x9ca7b5),
        syntax: SyntaxPalette::catppuccin_mocha(),
    };
    theme.recompute_selection_foreground();
    theme
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
