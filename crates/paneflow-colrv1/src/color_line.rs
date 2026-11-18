use tiny_skia::{Color, PremultipliedColorU8};

const TABLE_SIZE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearPremultiplied([f32; 4]);

impl LinearPremultiplied {
    pub fn from_srgb(color: Color) -> Self {
        let alpha = color.alpha();
        Self([
            srgb_to_linear(color.red()) * alpha,
            srgb_to_linear(color.green()) * alpha,
            srgb_to_linear(color.blue()) * alpha,
            alpha,
        ])
    }

    fn lerp(self, other: Self, fraction: f32) -> Self {
        let mut channels = [0.0; 4];
        for (index, channel) in channels.iter_mut().enumerate() {
            *channel = self.0[index] + (other.0[index] - self.0[index]) * fraction;
        }
        Self(channels)
    }

    pub fn to_srgb_premultiplied(self) -> PremultipliedColorU8 {
        let alpha = self.0[3].clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return PremultipliedColorU8::TRANSPARENT;
        }
        let alpha_byte = to_byte(alpha);
        let encode = |channel: f32| {
            let straight = (channel / alpha).clamp(0.0, 1.0);
            to_byte(linear_to_srgb(straight) * alpha).min(alpha_byte)
        };
        PremultipliedColorU8::from_rgba(
            encode(self.0[0]),
            encode(self.0[1]),
            encode(self.0[2]),
            alpha_byte,
        )
        .unwrap_or(PremultipliedColorU8::TRANSPARENT)
    }
}

pub struct ColorLine {
    stops: Vec<(f32, LinearPremultiplied)>,
}

impl ColorLine {
    pub fn new(stops: impl IntoIterator<Item = (f32, Color)>) -> Option<Self> {
        let mut stops: Vec<(f32, LinearPremultiplied)> = stops
            .into_iter()
            .map(|(offset, color)| (offset, LinearPremultiplied::from_srgb(color)))
            .collect();
        if stops.is_empty() {
            return None;
        }
        stops.sort_by(|left, right| left.0.total_cmp(&right.0));
        Some(Self { stops })
    }

    pub fn at(&self, t: f32) -> LinearPremultiplied {
        let Some(&(first_offset, first_color)) = self.stops.first() else {
            return LinearPremultiplied([0.0; 4]);
        };
        if t <= first_offset {
            return first_color;
        }
        for pair in self.stops.windows(2) {
            let (start_offset, start_color) = pair[0];
            let (end_offset, end_color) = pair[1];
            if t < end_offset {
                let span = end_offset - start_offset;
                if span <= f32::EPSILON {
                    return end_color;
                }
                return start_color.lerp(end_color, (t - start_offset) / span);
            }
        }
        self.stops
            .last()
            .map_or(first_color, |&(_, last_color)| last_color)
    }

    pub fn table(&self) -> ColorTable {
        let last = (TABLE_SIZE - 1) as f32;
        ColorTable(
            (0..TABLE_SIZE)
                .map(|index| self.at(index as f32 / last).to_srgb_premultiplied())
                .collect(),
        )
    }
}

pub struct ColorTable(Vec<PremultipliedColorU8>);

impl ColorTable {
    pub fn sample(&self, t: f32) -> PremultipliedColorU8 {
        let last = self.0.len().saturating_sub(1);
        let index = (t.clamp(0.0, 1.0) * last as f32).round() as usize;
        self.0
            .get(index.min(last))
            .copied()
            .unwrap_or(PremultipliedColorU8::TRANSPARENT)
    }
}

fn to_byte(unit: f32) -> u8 {
    (unit.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub fn srgb_to_linear(encoded: f32) -> f32 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(color: PremultipliedColorU8) -> [u8; 4] {
        [color.red(), color.green(), color.blue(), color.alpha()]
    }

    #[test]
    fn the_srgb_transfer_function_round_trips() {
        for byte in [0u8, 1, 10, 64, 128, 200, 255] {
            let encoded = f32::from(byte) / 255.0;
            let back = linear_to_srgb(srgb_to_linear(encoded));
            assert!((back - encoded).abs() < 1e-5, "{byte}");
        }
    }

    #[test]
    fn stops_are_returned_exactly_and_held_past_the_ends() {
        let line = ColorLine::new([
            (0.25, Color::from_rgba8(255, 0, 0, 255)),
            (0.75, Color::from_rgba8(0, 0, 255, 255)),
        ])
        .expect("two stops make a color line");
        assert_eq!(rgba(line.at(0.0).to_srgb_premultiplied()), [255, 0, 0, 255]);
        assert_eq!(
            rgba(line.at(0.25).to_srgb_premultiplied()),
            [255, 0, 0, 255]
        );
        assert_eq!(rgba(line.at(1.0).to_srgb_premultiplied()), [0, 0, 255, 255]);
    }

    #[test]
    fn interpolation_happens_in_linear_light() {
        let line = ColorLine::new([
            (0.0, Color::from_rgba8(0, 0, 0, 255)),
            (1.0, Color::from_rgba8(255, 255, 255, 255)),
        ])
        .expect("two stops make a color line");
        let middle = rgba(line.at(0.5).to_srgb_premultiplied());
        assert_eq!(middle, [188, 188, 188, 255]);
    }

    #[test]
    fn interpolation_premultiplies_alpha() {
        let line = ColorLine::new([
            (0.0, Color::from_rgba8(255, 0, 0, 255)),
            (1.0, Color::from_rgba8(0, 0, 255, 0)),
        ])
        .expect("two stops make a color line");
        let [red, green, blue, alpha] = rgba(line.at(0.5).to_srgb_premultiplied());
        assert_eq!([green, blue, alpha], [0, 0, 128]);
        assert!((127..=128).contains(&red), "{red}");
    }

    #[test]
    fn a_hard_stop_switches_color_at_its_offset() {
        let line = ColorLine::new([
            (0.0, Color::from_rgba8(255, 0, 0, 255)),
            (0.5, Color::from_rgba8(255, 0, 0, 255)),
            (0.5, Color::from_rgba8(0, 255, 0, 255)),
            (1.0, Color::from_rgba8(0, 255, 0, 255)),
        ])
        .expect("four stops make a color line");
        assert_eq!(
            rgba(line.at(0.49).to_srgb_premultiplied()),
            [255, 0, 0, 255]
        );
        assert_eq!(rgba(line.at(0.5).to_srgb_premultiplied()), [0, 255, 0, 255]);
    }
}
