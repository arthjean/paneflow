use crate::theme::{UiColors, WCAG_AA_TEXT_RATIO, readable_on};

pub(super) fn chrome_colors(mut ui: UiColors) -> UiColors {
    let backgrounds = [ui.base, ui.surface, ui.overlay];
    for color in [&mut ui.text, &mut ui.muted, &mut ui.vc_deleted] {
        *color = readable_on(*color, backgrounds, WCAG_AA_TEXT_RATIO);
    }
    ui
}
