use gpui::{Hsla, Rgba};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Oklab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Oklch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
}

const ACHROMATIC_CHROMA: f32 = 1.0e-4;
const GAMUT_TOLERANCE: f32 = 1.0e-4;
const GAMUT_SEARCH_STEPS: usize = 18;

impl Oklab {
    pub(super) fn to_oklch(self) -> Oklch {
        let chroma = self.a.hypot(self.b);
        let hue = if chroma <= ACHROMATIC_CHROMA {
            0.0
        } else {
            let degrees = self.b.atan2(self.a).to_degrees();
            if degrees < 0.0 {
                degrees + 360.0
            } else {
                degrees
            }
        };
        Oklch {
            l: self.l,
            c: chroma,
            h: hue,
        }
    }

    pub(super) fn distance(self, other: Self) -> f32 {
        let dl = self.l - other.l;
        let da = self.a - other.a;
        let db = self.b - other.b;
        (dl * dl + da * da + db * db).sqrt()
    }
}

impl Oklch {
    pub(super) fn to_oklab(self) -> Oklab {
        let radians = self.h.to_radians();
        Oklab {
            l: self.l,
            a: self.c * radians.cos(),
            b: self.c * radians.sin(),
        }
    }
}

fn srgb_channel_to_linear(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.040_449_936 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_channel_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

fn linear_srgb_to_oklab(r: f32, g: f32, b: f32) -> Oklab {
    let long = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let medium = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let short = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;

    let long = long.cbrt();
    let medium = medium.cbrt();
    let short = short.cbrt();

    Oklab {
        l: 0.210_454_26 * long + 0.793_617_8 * medium - 0.004_072_047 * short,
        a: 1.977_998_5 * long - 2.428_592_2 * medium + 0.450_593_7 * short,
        b: 0.025_904_037 * long + 0.782_771_77 * medium - 0.808_675_77 * short,
    }
}

fn oklab_to_linear_srgb(lab: Oklab) -> (f32, f32, f32) {
    let long = lab.l + 0.396_337_78 * lab.a + 0.215_803_76 * lab.b;
    let medium = lab.l - 0.105_561_346 * lab.a - 0.063_854_17 * lab.b;
    let short = lab.l - 0.089_484_18 * lab.a - 1.291_485_5 * lab.b;

    let long = long * long * long;
    let medium = medium * medium * medium;
    let short = short * short * short;

    (
        4.076_741_7 * long - 3.307_711_6 * medium + 0.230_969_94 * short,
        -1.268_438 * long + 2.609_757_4 * medium - 0.341_319_38 * short,
        -0.004_196_086_3 * long - 0.703_418_6 * medium + 1.707_614_7 * short,
    )
}

pub(super) fn srgb_to_oklab(color: Rgba) -> Oklab {
    linear_srgb_to_oklab(
        srgb_channel_to_linear(color.r),
        srgb_channel_to_linear(color.g),
        srgb_channel_to_linear(color.b),
    )
}

pub(super) fn oklab_to_srgb(lab: Oklab, alpha: f32) -> Rgba {
    let (r, g, b) = oklab_to_linear_srgb(lab);
    Rgba {
        r: linear_channel_to_srgb(r).clamp(0.0, 1.0),
        g: linear_channel_to_srgb(g).clamp(0.0, 1.0),
        b: linear_channel_to_srgb(b).clamp(0.0, 1.0),
        a: alpha,
    }
}

fn is_in_gamut(lab: Oklab) -> bool {
    let (r, g, b) = oklab_to_linear_srgb(lab);
    let within = |value: f32| (-GAMUT_TOLERANCE..=1.0 + GAMUT_TOLERANCE).contains(&value);
    within(r) && within(g) && within(b)
}

pub(super) fn clip_chroma_to_gamut(color: Oklch) -> Oklch {
    let clamped = Oklch {
        l: color.l.clamp(0.0, 1.0),
        c: color.c.max(0.0),
        h: color.h,
    };
    if is_in_gamut(clamped.to_oklab()) {
        return clamped;
    }
    let mut low = 0.0;
    let mut high = clamped.c;
    for _ in 0..GAMUT_SEARCH_STEPS {
        let mid = (low + high) * 0.5;
        if is_in_gamut(Oklch { c: mid, ..clamped }.to_oklab()) {
            low = mid;
        } else {
            high = mid;
        }
    }
    Oklch { c: low, ..clamped }
}

pub(super) fn hsla_to_oklab(color: Hsla) -> Oklab {
    srgb_to_oklab(Rgba::from(color))
}

pub(super) fn hsla_to_oklch(color: Hsla) -> Oklch {
    hsla_to_oklab(color).to_oklch()
}

pub(super) fn oklch_to_hsla_in_gamut(color: Oklch, alpha: f32) -> Hsla {
    Hsla::from(oklab_to_srgb(clip_chroma_to_gamut(color).to_oklab(), alpha))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: 1.0,
        }
    }

    const PRIMARIES: [(u8, u8, u8); 8] = [
        (255, 255, 255),
        (0, 0, 0),
        (255, 0, 0),
        (0, 255, 0),
        (0, 0, 255),
        (0, 255, 255),
        (255, 0, 255),
        (255, 255, 0),
    ];

    #[test]
    fn the_primaries_round_trip_within_one_eight_bit_step() {
        for (r, g, b) in PRIMARIES {
            let source = rgb(r, g, b);
            let back = oklab_to_srgb(srgb_to_oklab(source), 1.0);
            for (channel, (from, to)) in
                [(source.r, back.r), (source.g, back.g), (source.b, back.b)]
                    .into_iter()
                    .enumerate()
            {
                assert!(
                    (from - to).abs() <= 1.0 / 255.0,
                    "rgb({r},{g},{b}) channel {channel} drifted from {from} to {to}"
                );
            }
        }
    }

    #[test]
    fn the_reference_values_match_ottossons_article() {
        let expected = [
            ((255u8, 255u8, 255u8), (1.0f32, 0.0f32, 0.0f32)),
            ((0, 0, 0), (0.0, 0.0, 0.0)),
            ((255, 0, 0), (0.627_955, 0.224_863, 0.125_846)),
            ((0, 255, 0), (0.866_440, -0.233_888, 0.179_498)),
            ((0, 0, 255), (0.452_014, -0.032_457, -0.311_528)),
        ];
        for ((r, g, b), (l, a, bb)) in expected {
            let lab = srgb_to_oklab(rgb(r, g, b));
            assert!(
                (lab.l - l).abs() < 0.001
                    && (lab.a - a).abs() < 0.001
                    && (lab.b - bb).abs() < 0.001,
                "rgb({r},{g},{b}) gave {lab:?}, expected L {l} a {a} b {bb}"
            );
        }
    }

    #[test]
    fn an_out_of_gamut_chroma_is_clipped_with_its_hue_intact() {
        for hue in (0..360).step_by(15) {
            let requested = Oklch {
                l: 0.55,
                c: 0.45,
                h: hue as f32,
            };
            let clipped = clip_chroma_to_gamut(requested);
            assert!(
                is_in_gamut(clipped.to_oklab()),
                "hue {hue} stayed out of gamut at chroma {}",
                clipped.c
            );
            assert!(
                clipped.c < requested.c,
                "hue {hue} needed no clipping at chroma {}",
                requested.c
            );
            let recovered = srgb_to_oklab(oklab_to_srgb(clipped.to_oklab(), 1.0)).to_oklch();
            let drift = (recovered.h - requested.h)
                .abs()
                .min(360.0 - (recovered.h - requested.h).abs());
            assert!(drift <= 1.0, "hue {hue} drifted by {drift} degrees");
            let wider = Oklch {
                c: clipped.c * 1.05 + 0.005,
                ..requested
            };
            assert!(
                !is_in_gamut(wider.to_oklab()),
                "hue {hue} left representable chroma on the table at {}",
                clipped.c
            );
        }
    }

    #[test]
    fn a_grey_reports_a_stable_hue_and_no_nan() {
        for level in [0u8, 64, 128, 200, 255] {
            let lch = srgb_to_oklab(rgb(level, level, level)).to_oklch();
            assert_eq!(lch.h, 0.0, "grey {level} must report hue 0");
            assert!(lch.c.is_finite() && lch.l.is_finite());
            let back = oklch_to_hsla_in_gamut(lch, 1.0);
            assert!(back.h.is_finite() && back.s.is_finite() && back.l.is_finite());
        }
    }

    #[test]
    fn a_zero_chroma_round_trips_through_oklch() {
        let grey = Oklch {
            l: 0.5,
            c: 0.0,
            h: 0.0,
        };
        let rgba = Rgba::from(oklch_to_hsla_in_gamut(grey, 1.0));
        assert!((rgba.r - rgba.g).abs() < 1.0 / 255.0 && (rgba.g - rgba.b).abs() < 1.0 / 255.0);
    }
}
