use gpui::Hsla;

use crate::theme::{TerminalTheme, active_theme};

#[derive(Clone, Copy)]
pub(crate) struct MarkdownPalette {
    pub background: Hsla,
    pub body: Hsla,
    pub heading: Hsla,
    pub code_bg: Hsla,
    pub code_fg: Hsla,
    pub blockquote_border: Hsla,
    pub blockquote_text: Hsla,
    pub link: Hsla,
    pub rule: Hsla,
}

impl MarkdownPalette {
    pub(crate) fn from_active() -> Self {
        Self::from_terminal(&active_theme())
    }

    pub(crate) fn from_terminal(t: &TerminalTheme) -> Self {
        Self {
            background: t.background,
            body: t.foreground,
            heading: t.bright_foreground,
            code_bg: t.ansi_background,
            code_fg: t.foreground,
            blockquote_border: t.link_text,
            blockquote_text: t.dim_foreground,
            link: t.link_text,
            rule: t.dim_foreground,
        }
    }
}
