mod builtin;
mod model;
mod signal;
mod watcher;

pub use builtin::{
    DEFAULT_THEME, PRESETS, THEMES, ThemeEntry, ThemePreset, canonical_theme_name, claude_dark,
    claude_light, cursor_dark, cursor_light, paneflow_dark, paneflow_light, preset_by_name,
    preset_for_theme, theme_by_name, theme_name_is_light, vercel_dark, vercel_light,
};
pub use model::{DiffColors, SyntaxPalette, TerminalTheme, UiColors, ui_colors, ui_colors_with};
pub use signal::{
    ThemeSignal, ThemeSignalGlobal, install_theme_signal, publish_theme_generation, theme_signal,
};
pub use watcher::{
    ThemeWatcher, active_theme, config_mtime, invalidate_theme_cache, theme_generation,
};
