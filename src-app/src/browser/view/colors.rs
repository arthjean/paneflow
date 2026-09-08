use crate::theme::UiColors;
use gpui::Hsla;

fn luminance(color: Hsla) -> f64 {
    let c: gpui::Rgba = color.into();
    let linear = |x: f32| {
        let x = f64::from(x);
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
}

pub(super) fn chrome_colors(mut ui: UiColors) -> UiColors {
    let backgrounds = [ui.base, ui.surface, ui.overlay];
    for color in [&mut ui.text, &mut ui.muted, &mut ui.vc_deleted] {
        let direction = if luminance(ui.base) > 0.5 {
            -0.01
        } else {
            0.01
        };
        for _ in 0..100 {
            if backgrounds.iter().all(|background| {
                let a = luminance(*color);
                let b = luminance(*background);
                (a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5
            }) {
                break;
            }
            color.l = (color.l + direction).clamp(0.0, 1.0);
        }
    }
    ui
}
