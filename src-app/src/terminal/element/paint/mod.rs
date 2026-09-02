use gpui::{Font, FontWeight};

pub(super) mod background;
pub(super) mod box_drawing;
pub(super) mod cursor;
pub(super) mod kitty;
pub(super) mod overlay;
pub(super) mod scrollbar;
pub(super) mod selection;
pub(super) mod text;

fn display_font_for_intensity(font: &Font, base_weight: FontWeight) -> Font {
    let mut display_font = font.clone();
    if font.weight == FontWeight::BOLD {
        display_font.weight = if base_weight.0 >= FontWeight::BOLD.0 {
            base_weight
        } else {
            FontWeight((base_weight.0 + 200.0).clamp(FontWeight::SEMIBOLD.0, FontWeight::BOLD.0))
        };
    }
    display_font
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{FontFeatures, FontStyle};

    fn font(weight: FontWeight) -> Font {
        Font {
            family: "test-mono".into(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight,
            style: FontStyle::Normal,
        }
    }

    #[test]
    fn ansi_bold_uses_semibold_from_normal() {
        let display = display_font_for_intensity(&font(FontWeight::BOLD), FontWeight::NORMAL);
        assert_eq!(display.weight, FontWeight::SEMIBOLD);
    }

    #[test]
    fn ansi_bold_uses_bold_from_medium() {
        let display = display_font_for_intensity(&font(FontWeight::BOLD), FontWeight::MEDIUM);
        assert_eq!(display.weight, FontWeight::BOLD);
    }

    #[test]
    fn regular_runs_keep_their_configured_weight() {
        let display = display_font_for_intensity(&font(FontWeight::NORMAL), FontWeight::NORMAL);
        assert_eq!(display.weight, FontWeight::NORMAL);
    }

    #[test]
    fn heavy_base_weight_is_never_reduced() {
        let display = display_font_for_intensity(&font(FontWeight::BOLD), FontWeight::EXTRA_BOLD);
        assert_eq!(display.weight, FontWeight::EXTRA_BOLD);
    }
}
