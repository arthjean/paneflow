//! Terminal theming with 36 color slots (see [`model::TerminalTheme`]),
//! compatible with Zed's terminal theme format.

mod builtin;
mod model;
mod watcher;

pub use builtin::{
    DEFAULT_THEME, PRESETS, THEMES, ThemeEntry, ThemePreset, canonical_theme_name, claude_dark,
    claude_light, cursor_dark, cursor_light, paneflow_dark, paneflow_light, preset_by_name,
    preset_for_theme, theme_by_name, theme_name_is_light, vercel_dark, vercel_light,
};
pub use model::{DiffColors, SyntaxPalette, TerminalTheme, UiColors, ui_colors, ui_colors_with};
pub use watcher::{
    ThemeWatcher, active_theme, config_mtime, invalidate_theme_cache, theme_generation,
};

/// Keep the small subset of Zed's global theme used by the Markdown renderer
/// aligned with PaneFlow's active palette. The renderer reads these slots for
/// table rows and borders instead of PaneFlow's own [`UiColors`].
pub fn sync_markdown_global_theme(cx: &mut gpui::App) {
    use theme::ActiveTheme as _;

    let ui = ui_colors();
    let already_synced = {
        let colors = cx.theme().colors();
        colors.title_bar_background == ui.surface
            && colors.panel_background == ui.base
            && colors.border == ui.border
            && colors.border_variant == ui.border
    };
    if already_synced {
        return;
    }

    let mut new_theme = (**cx.theme()).clone();
    new_theme.styles.colors.title_bar_background = ui.surface;
    new_theme.styles.colors.panel_background = ui.base;
    new_theme.styles.colors.border = ui.border;
    new_theme.styles.colors.border_variant = ui.border;
    ::theme::GlobalTheme::update_theme(cx, std::sync::Arc::new(new_theme));
}
