use gpui::Hsla;
use paneflow_config::schema::BackgroundStyle;
use std::sync::atomic::{AtomicU8, Ordering};

const DARK_SHELL_VEIL_OPACITY: f32 = 0.72;
const LIGHT_SHELL_VEIL_OPACITY: f32 = 0.80;
const TERMINAL_CARD_SHADE_OPACITY: f32 = 0.35;
const SEE_THROUGH_KEPT_WITHOUT_BLUR: f32 = 0.25;
const LIGHT_COLOR_LIGHTNESS: f32 = 0.5;

const FRAME_BLENDED: u8 = 0b01;
const FRAME_BLUR_FULL: u8 = 0b10;

static FRAME_BACKDROP: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlurSupport {
    Full,
    None,
}

impl BlurSupport {
    fn see_through_kept(self) -> f32 {
        match self {
            Self::Full => 1.0,
            Self::None => SEE_THROUGH_KEPT_WITHOUT_BLUR,
        }
    }
}

pub(crate) fn effective_style(style: BackgroundStyle, surface_has_alpha: bool) -> BackgroundStyle {
    if surface_has_alpha {
        style
    } else {
        BackgroundStyle::Themed
    }
}

pub(crate) fn thicken(base_alpha: f32, keep: f32) -> f32 {
    let keep = if keep.is_nan() {
        0.0
    } else {
        keep.clamp(0.0, 1.0)
    };
    let base_alpha = base_alpha.clamp(0.0, 1.0);
    1.0 - (1.0 - base_alpha) * keep
}

fn is_light(color: Hsla) -> bool {
    color.l > LIGHT_COLOR_LIGHTNESS
}

fn opaque(color: Hsla) -> Hsla {
    Hsla { a: 1.0, ..color }
}

pub(crate) fn chrome_over_shell() -> Hsla {
    gpui::transparent_black()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Backdrop {
    style: BackgroundStyle,
    blur: BlurSupport,
}

impl Backdrop {
    pub(crate) const THEMED: Self = Self {
        style: BackgroundStyle::Themed,
        blur: BlurSupport::None,
    };

    pub(crate) fn new(style: BackgroundStyle, surface_has_alpha: bool, blur: BlurSupport) -> Self {
        Self {
            style: effective_style(style, surface_has_alpha),
            blur,
        }
    }

    pub(crate) fn is_blended(self) -> bool {
        self.style == BackgroundStyle::Blended
    }

    pub(crate) fn shell(self, color: Hsla) -> Hsla {
        if !self.is_blended() {
            return opaque(color);
        }
        let base = if is_light(color) {
            LIGHT_SHELL_VEIL_OPACITY
        } else {
            DARK_SHELL_VEIL_OPACITY
        };
        Hsla {
            a: thicken(base, self.blur.see_through_kept()),
            ..color
        }
    }

    pub(crate) fn over_shell(self, color: Hsla) -> Hsla {
        if self.is_blended() {
            gpui::transparent_black()
        } else {
            opaque(color)
        }
    }

    pub(crate) fn terminal_card(self, theme_background: Hsla) -> Hsla {
        if !self.is_blended() || self.blur != BlurSupport::Full {
            return opaque(theme_background);
        }
        let shade = if is_light(theme_background) {
            gpui::white()
        } else {
            gpui::black()
        };
        shade.opacity(TERMINAL_CARD_SHADE_OPACITY)
    }

    pub(crate) fn tint_over_shell(self, tint: Hsla, shell: Hsla) -> Hsla {
        if self.is_blended() {
            tint
        } else {
            opaque(shell).blend(tint)
        }
    }

    fn encode(self) -> u8 {
        let blended = if self.is_blended() { FRAME_BLENDED } else { 0 };
        let blur = if self.blur == BlurSupport::Full {
            FRAME_BLUR_FULL
        } else {
            0
        };
        blended | blur
    }

    fn decode(bits: u8) -> Self {
        Self {
            style: if bits & FRAME_BLENDED != 0 {
                BackgroundStyle::Blended
            } else {
                BackgroundStyle::Themed
            },
            blur: if bits & FRAME_BLUR_FULL != 0 {
                BlurSupport::Full
            } else {
                BlurSupport::None
            },
        }
    }
}

pub(crate) fn note_frame_backdrop(backdrop: Backdrop) {
    FRAME_BACKDROP.store(backdrop.encode(), Ordering::Relaxed);
}

pub(crate) fn frame_backdrop() -> Backdrop {
    Backdrop::decode(FRAME_BACKDROP.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-6;

    fn dark() -> Hsla {
        Hsla::from(gpui::rgb(0x141414))
    }

    fn light() -> Hsla {
        Hsla::from(gpui::rgb(0xf5f7fd))
    }

    fn blended(blur: BlurSupport) -> Backdrop {
        Backdrop::new(BackgroundStyle::Blended, true, blur)
    }

    fn assert_alpha(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < EPSILON,
            "alpha {actual} differs from {expected}"
        );
    }

    #[test]
    fn a_blurred_backdrop_veils_the_shell_at_the_base_opacity() {
        assert_alpha(blended(BlurSupport::Full).shell(dark()).a, 0.72);
        assert_alpha(blended(BlurSupport::Full).shell(light()).a, 0.80);
    }

    #[test]
    fn an_unblurred_backdrop_keeps_a_quarter_of_the_see_through() {
        assert_alpha(blended(BlurSupport::None).shell(dark()).a, 0.93);
        assert_alpha(blended(BlurSupport::None).shell(light()).a, 0.95);
    }

    #[test]
    fn the_shell_veil_keeps_the_theme_hue() {
        let veil = blended(BlurSupport::None).shell(dark());
        assert_eq!((veil.h, veil.s, veil.l), (dark().h, dark().s, dark().l));
    }

    #[test]
    fn a_blurred_terminal_card_is_a_shade_over_the_shell_veil() {
        let full = blended(BlurSupport::Full);
        assert_eq!(full.terminal_card(dark()), gpui::black().opacity(0.35));
        assert_eq!(full.terminal_card(light()), gpui::white().opacity(0.35));
    }

    #[test]
    fn an_unblurred_terminal_card_paints_the_opaque_theme_background() {
        let none = blended(BlurSupport::None);
        for background in [dark(), light(), dark().opacity(0.4)] {
            assert_eq!(none.terminal_card(background), opaque(background));
        }
    }

    #[test]
    fn themed_fills_are_opaque_whatever_the_blur_support() {
        for blur in [BlurSupport::Full, BlurSupport::None] {
            for surface_has_alpha in [false, true] {
                let themed = Backdrop::new(BackgroundStyle::Themed, surface_has_alpha, blur);
                for color in [dark(), light(), light().opacity(0.3)] {
                    assert_eq!(themed.shell(color).a, 1.0);
                    assert_eq!(themed.terminal_card(color).a, 1.0);
                    assert_eq!(themed.over_shell(color).a, 1.0);
                }
            }
        }
    }

    #[test]
    fn blended_needs_a_surface_with_alpha() {
        assert_eq!(
            effective_style(BackgroundStyle::Blended, false),
            BackgroundStyle::Themed
        );
        assert_eq!(
            effective_style(BackgroundStyle::Blended, true),
            BackgroundStyle::Blended
        );
        assert_eq!(
            effective_style(BackgroundStyle::Themed, true),
            BackgroundStyle::Themed
        );
        let without_alpha = Backdrop::new(BackgroundStyle::Blended, false, BlurSupport::Full);
        assert!(!without_alpha.is_blended());
        assert_eq!(without_alpha.shell(dark()).a, 1.0);
        assert_eq!(without_alpha.terminal_card(dark()).a, 1.0);
    }

    #[test]
    fn thickening_clamps_the_keep_factor() {
        for base in [0.0, 0.35, 0.72, 0.80, 1.0] {
            for keep in [
                -3.0,
                -0.1,
                0.0,
                0.5,
                1.0,
                1.1,
                42.0,
                f32::NAN,
                f32::INFINITY,
            ] {
                let alpha = thicken(base, keep);
                assert!(
                    (base - EPSILON..=1.0 + EPSILON).contains(&alpha),
                    "thicken({base}, {keep}) = {alpha}"
                );
            }
        }
        assert_alpha(thicken(0.72, 7.0), 0.72);
        assert_alpha(thicken(0.72, -7.0), 1.0);
        assert_alpha(thicken(0.72, f32::NAN), 1.0);
    }

    #[test]
    fn blended_layers_clear_over_the_veil_and_themed_layers_paint_their_color() {
        let blended = blended(BlurSupport::Full);
        assert_eq!(blended.over_shell(dark()), gpui::transparent_black());
        assert_eq!(Backdrop::THEMED.over_shell(dark()), opaque(dark()));
    }

    #[test]
    fn chrome_lets_the_shell_show_through_in_every_style() {
        assert_eq!(chrome_over_shell(), gpui::transparent_black());
        for backdrop in [
            Backdrop::THEMED,
            blended(BlurSupport::Full),
            blended(BlurSupport::None),
        ] {
            assert!(backdrop.shell(dark()).a > 0.0);
        }
    }

    #[test]
    fn a_tint_is_preblended_only_over_an_opaque_shell() {
        let tint = gpui::white().opacity(0.16);
        assert_eq!(
            blended(BlurSupport::None).tint_over_shell(tint, dark()),
            tint
        );
        let themed = Backdrop::THEMED.tint_over_shell(tint, dark());
        assert_eq!(themed, opaque(dark()).blend(tint));
        assert_eq!(themed.a, 1.0);
    }

    #[test]
    fn the_frame_backdrop_round_trips_every_backdrop() {
        for style in [BackgroundStyle::Themed, BackgroundStyle::Blended] {
            for blur in [BlurSupport::Full, BlurSupport::None] {
                let backdrop = Backdrop::new(style, true, blur);
                assert_eq!(Backdrop::decode(backdrop.encode()), backdrop);
            }
        }
    }
}
