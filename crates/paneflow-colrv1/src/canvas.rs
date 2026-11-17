use skrifa::color::{
    Brush, ColorPainter, ColorStop, CompositeMode, Extend, Transform as FontTransform,
};
use skrifa::raw::types::BoundingBox;
use skrifa::GlyphId;
use tiny_skia::{
    BlendMode, Color, FillRule, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Point, Rect, Shader,
    Transform,
};

use crate::color_line::ColorLine;
use crate::geometry::{apply_extend, linear_t, radial_t, sweep_t};
use crate::outline::OutlineSource;

pub const FOREGROUND_PALETTE_INDEX: u16 = 0xFFFF;

pub struct Canvas<'a, O> {
    width: u32,
    height: u32,
    palette: &'a [Color],
    foreground: Color,
    outlines: &'a O,
    transforms: Vec<Transform>,
    clips: Vec<Mask>,
    layers: Vec<Pixmap>,
    blank_layer: Pixmap,
    blank_mask: Mask,
}

impl<'a, O: OutlineSource> Canvas<'a, O> {
    pub fn new(
        width: u32,
        height: u32,
        root: Transform,
        palette: &'a [Color],
        foreground: Color,
        outlines: &'a O,
    ) -> Option<Self> {
        let blank_layer = Pixmap::new(width, height)?;
        let blank_mask = Mask::new(width, height)?;
        Some(Self {
            width,
            height,
            palette,
            foreground,
            outlines,
            transforms: vec![root],
            clips: Vec::new(),
            layers: vec![blank_layer.clone()],
            blank_layer,
            blank_mask,
        })
    }

    pub fn into_pixmap(mut self) -> Pixmap {
        while self.layers.len() > 1 {
            self.pop_layer_with_mode(CompositeMode::SrcOver);
        }
        self.layers.pop().unwrap_or(self.blank_layer)
    }

    fn transform(&self) -> Transform {
        self.transforms
            .last()
            .copied()
            .unwrap_or_else(Transform::identity)
    }

    fn push_clip_path(&mut self, path: Option<tiny_skia::Path>) {
        let transform = self.transform();
        let mask = match (path, self.clips.last()) {
            (Some(path), Some(parent)) => {
                let mut mask = parent.clone();
                mask.intersect_path(&path, FillRule::Winding, true, transform);
                mask
            }
            (Some(path), None) => {
                let mut mask = self.blank_mask.clone();
                mask.fill_path(&path, FillRule::Winding, true, transform);
                mask
            }
            (None, _) => self.blank_mask.clone(),
        };
        self.clips.push(mask);
    }

    fn palette_color(&self, palette_index: u16, alpha: f32) -> Color {
        let base = if palette_index == FOREGROUND_PALETTE_INDEX {
            self.foreground
        } else {
            self.palette
                .get(usize::from(palette_index))
                .copied()
                .unwrap_or(Color::TRANSPARENT)
        };
        let mut color = base;
        color.apply_opacity(alpha.clamp(0.0, 1.0));
        color
    }

    fn color_line(&self, stops: &[ColorStop]) -> Option<ColorLine> {
        ColorLine::new(stops.iter().map(|stop| {
            (
                stop.offset,
                self.palette_color(stop.palette_index, stop.alpha),
            )
        }))
    }

    fn full_rect(&self) -> Option<Rect> {
        Rect::from_xywh(0.0, 0.0, self.width as f32, self.height as f32)
    }

    fn fill_solid(&mut self, color: Color) {
        let Some(rect) = self.full_rect() else {
            return;
        };
        let paint = Paint {
            shader: Shader::SolidColor(color),
            anti_alias: false,
            ..Paint::default()
        };
        let clip = self.clips.last();
        if let Some(layer) = self.layers.last_mut() {
            layer.fill_rect(rect, &paint, Transform::identity(), clip);
        }
    }

    fn fill_gradient(
        &mut self,
        stops: &[ColorStop],
        extend: Extend,
        parameter: impl Fn(Point) -> Option<f32>,
    ) {
        let Some(line) = self.color_line(stops) else {
            return;
        };
        let Some(inverse) = self.transform().invert() else {
            return;
        };
        let table = line.table();
        let mut source = self.blank_layer.clone();
        let coverage = self.clips.last().map(Mask::data);
        let width = self.width as usize;
        for (index, pixel) in source.pixels_mut().iter_mut().enumerate() {
            if coverage.is_some_and(|coverage| coverage.get(index).is_none_or(|&c| c == 0)) {
                continue;
            }
            let mut point =
                Point::from_xy((index % width) as f32 + 0.5, (index / width) as f32 + 0.5);
            inverse.map_point(&mut point);
            if let Some(t) = parameter(point) {
                *pixel = table.sample(apply_extend(t, extend));
            }
        }
        let clip = self.clips.last();
        if let Some(layer) = self.layers.last_mut() {
            layer.draw_pixmap(
                0,
                0,
                source.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                clip,
            );
        }
    }
}

pub fn to_skia_transform(transform: FontTransform) -> Transform {
    Transform::from_row(
        transform.xx,
        transform.yx,
        transform.xy,
        transform.yy,
        transform.dx,
        transform.dy,
    )
}

pub fn blend_mode(mode: CompositeMode) -> BlendMode {
    match mode {
        CompositeMode::Clear | CompositeMode::Unknown => BlendMode::Clear,
        CompositeMode::Src => BlendMode::Source,
        CompositeMode::Dest => BlendMode::Destination,
        CompositeMode::SrcOver => BlendMode::SourceOver,
        CompositeMode::DestOver => BlendMode::DestinationOver,
        CompositeMode::SrcIn => BlendMode::SourceIn,
        CompositeMode::DestIn => BlendMode::DestinationIn,
        CompositeMode::SrcOut => BlendMode::SourceOut,
        CompositeMode::DestOut => BlendMode::DestinationOut,
        CompositeMode::SrcAtop => BlendMode::SourceAtop,
        CompositeMode::DestAtop => BlendMode::DestinationAtop,
        CompositeMode::Xor => BlendMode::Xor,
        CompositeMode::Plus => BlendMode::Plus,
        CompositeMode::Screen => BlendMode::Screen,
        CompositeMode::Overlay => BlendMode::Overlay,
        CompositeMode::Darken => BlendMode::Darken,
        CompositeMode::Lighten => BlendMode::Lighten,
        CompositeMode::ColorDodge => BlendMode::ColorDodge,
        CompositeMode::ColorBurn => BlendMode::ColorBurn,
        CompositeMode::HardLight => BlendMode::HardLight,
        CompositeMode::SoftLight => BlendMode::SoftLight,
        CompositeMode::Difference => BlendMode::Difference,
        CompositeMode::Exclusion => BlendMode::Exclusion,
        CompositeMode::Multiply => BlendMode::Multiply,
        CompositeMode::HslHue => BlendMode::Hue,
        CompositeMode::HslSaturation => BlendMode::Saturation,
        CompositeMode::HslColor => BlendMode::Color,
        CompositeMode::HslLuminosity => BlendMode::Luminosity,
    }
}

pub fn clip_box_path(clip_box: BoundingBox<f32>) -> Option<tiny_skia::Path> {
    let rect = Rect::from_ltrb(
        clip_box.x_min,
        clip_box.y_min,
        clip_box.x_max,
        clip_box.y_max,
    )?;
    Some(PathBuilder::from_rect(rect))
}

impl<O: OutlineSource> ColorPainter for Canvas<'_, O> {
    fn push_transform(&mut self, transform: FontTransform) {
        let combined = self.transform().pre_concat(to_skia_transform(transform));
        self.transforms.push(combined);
    }

    fn pop_transform(&mut self) {
        if self.transforms.len() > 1 {
            self.transforms.pop();
        }
    }

    fn push_clip_glyph(&mut self, glyph_id: GlyphId) {
        let path = self.outlines.glyph_path(glyph_id);
        self.push_clip_path(path);
    }

    fn push_clip_box(&mut self, clip_box: BoundingBox<f32>) {
        self.push_clip_path(clip_box_path(clip_box));
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn fill(&mut self, brush: Brush<'_>) {
        match brush {
            Brush::Solid {
                palette_index,
                alpha,
            } => {
                let color = self.palette_color(palette_index, alpha);
                self.fill_solid(color);
            }
            Brush::LinearGradient {
                p0,
                p1,
                color_stops,
                extend,
            } => {
                let p0 = Point::from_xy(p0.x, p0.y);
                let p1 = Point::from_xy(p1.x, p1.y);
                if linear_t(p0, p0, p1).is_none() {
                    return;
                }
                self.fill_gradient(color_stops, extend, |point| linear_t(point, p0, p1));
            }
            Brush::RadialGradient {
                c0,
                r0,
                c1,
                r1,
                color_stops,
                extend,
            } => {
                let c0 = Point::from_xy(c0.x, c0.y);
                let c1 = Point::from_xy(c1.x, c1.y);
                if c0 == c1 && r0 == r1 {
                    return;
                }
                self.fill_gradient(color_stops, extend, |point| radial_t(point, c0, r0, c1, r1));
            }
            Brush::SweepGradient {
                c0,
                start_angle,
                end_angle,
                color_stops,
                extend,
            } => {
                if start_angle == end_angle && extend != Extend::Pad {
                    return;
                }
                let center = Point::from_xy(c0.x, c0.y);
                self.fill_gradient(color_stops, extend, |point| {
                    Some(sweep_t(point, center, start_angle, end_angle))
                });
            }
        }
    }

    fn push_layer(&mut self, _composite_mode: CompositeMode) {
        self.layers.push(self.blank_layer.clone());
    }

    fn pop_layer(&mut self) {
        self.pop_layer_with_mode(CompositeMode::SrcOver);
    }

    fn pop_layer_with_mode(&mut self, composite_mode: CompositeMode) {
        if self.layers.len() < 2 {
            return;
        }
        let Some(layer) = self.layers.pop() else {
            return;
        };
        let paint = PixmapPaint {
            blend_mode: blend_mode(composite_mode),
            ..PixmapPaint::default()
        };
        let clip = self.clips.last();
        if let Some(target) = self.layers.last_mut() {
            target.draw_pixmap(0, 0, layer.as_ref(), &paint, Transform::identity(), clip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skrifa::raw::types::Point as FontPoint;
    use tiny_skia::Path;

    const SIZE: u32 = 64;

    struct Squares;

    impl OutlineSource for Squares {
        fn glyph_path(&self, glyph_id: GlyphId) -> Option<Path> {
            let half = glyph_id.to_u32() as f32;
            let rect = Rect::from_ltrb(-half, -half, half, half)?;
            Some(PathBuilder::from_rect(rect))
        }
    }

    fn palette() -> Vec<Color> {
        vec![
            Color::from_rgba8(255, 0, 0, 255),
            Color::from_rgba8(0, 0, 255, 255),
            Color::from_rgba8(0, 255, 0, 255),
        ]
    }

    fn y_up_root() -> Transform {
        let center = SIZE as f32 / 2.0;
        Transform::from_row(1.0, 0.0, 0.0, -1.0, center, center)
    }

    fn pixel(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let color = pixmap
            .pixel(x, y)
            .expect("the pixel lies inside the canvas")
            .demultiply();
        [color.red(), color.green(), color.blue(), color.alpha()]
    }

    fn red_share(color: [u8; 4]) -> f32 {
        f32::from(color[0]) / (f32::from(color[0]) + f32::from(color[2])).max(1.0)
    }

    fn stops() -> [ColorStop; 2] {
        [
            ColorStop {
                offset: 0.0,
                palette_index: 0,
                alpha: 1.0,
            },
            ColorStop {
                offset: 1.0,
                palette_index: 1,
                alpha: 1.0,
            },
        ]
    }

    fn paint_sweep(start_angle: f32, end_angle: f32, extend: Extend) -> Pixmap {
        let palette = palette();
        let mut canvas = Canvas::new(SIZE, SIZE, y_up_root(), &palette, Color::BLACK, &Squares)
            .expect("a 64 pixel canvas allocates");
        let stops = stops();
        canvas.fill(Brush::SweepGradient {
            c0: FontPoint::new(0.0, 0.0),
            start_angle,
            end_angle,
            color_stops: &stops,
            extend,
        });
        canvas.into_pixmap()
    }

    fn at_angle(pixmap: &Pixmap, clockwise_degrees: f32) -> [u8; 4] {
        let radians = clockwise_degrees.to_radians();
        let center = SIZE as f32 / 2.0;
        let x = center + 20.0 * radians.cos();
        let y = center + 20.0 * radians.sin();
        pixel(pixmap, x as u32, y as u32)
    }

    #[test]
    fn a_sweep_turns_clockwise_on_screen() {
        let pixmap = paint_sweep(0.0, 360.0, Extend::Pad);
        let east = at_angle(&pixmap, 2.0);
        let south = at_angle(&pixmap, 90.0);
        let west = at_angle(&pixmap, 180.0);
        let north = at_angle(&pixmap, 270.0);
        assert!(red_share(east) > 0.9, "{east:?}");
        assert!(red_share(south) > red_share(west), "{south:?} {west:?}");
        assert!(red_share(west) > red_share(north), "{west:?} {north:?}");
        assert!(red_share(north) < 0.45, "{north:?}");
        assert_eq!(east[3], 255);
    }

    #[test]
    fn a_sweep_honors_every_extend_mode_outside_its_span() {
        let pad = paint_sweep(90.0, 180.0, Extend::Pad);
        let repeat = paint_sweep(90.0, 180.0, Extend::Repeat);
        let reflect = paint_sweep(90.0, 180.0, Extend::Reflect);

        let before = 67.5;
        let after = 202.5;
        assert!(red_share(at_angle(&pad, before)) > 0.95);
        assert!(red_share(at_angle(&pad, after)) < 0.05);

        let repeat_before = red_share(at_angle(&repeat, before));
        let repeat_after = red_share(at_angle(&repeat, after));
        let reflect_before = red_share(at_angle(&reflect, before));
        let reflect_after = red_share(at_angle(&reflect, after));
        assert!(repeat_before < 0.5 && repeat_after > 0.5);
        assert!(reflect_before > 0.5 && reflect_after < 0.5);
        assert!((repeat_before - reflect_after).abs() < 0.05);
        assert!((repeat_after - reflect_before).abs() < 0.05);
    }

    #[test]
    fn a_glyph_clip_confines_a_fill_and_restores_on_pop() {
        let palette = palette();
        let mut canvas = Canvas::new(SIZE, SIZE, y_up_root(), &palette, Color::BLACK, &Squares)
            .expect("a 64 pixel canvas allocates");
        canvas.push_clip_glyph(GlyphId::new(10));
        canvas.fill(Brush::Solid {
            palette_index: 0,
            alpha: 1.0,
        });
        canvas.pop_clip();
        canvas.push_transform(FontTransform {
            dx: 20.0,
            ..FontTransform::default()
        });
        canvas.push_clip_glyph(GlyphId::new(4));
        canvas.fill(Brush::Solid {
            palette_index: 1,
            alpha: 0.5,
        });
        canvas.pop_clip();
        canvas.pop_transform();
        let pixmap = canvas.into_pixmap();
        assert_eq!(pixel(&pixmap, 32, 32), [255, 0, 0, 255]);
        assert_eq!(pixel(&pixmap, 32, 20)[3], 0);
        let translucent = pixel(&pixmap, 52, 32);
        assert!(translucent[0] == 0 && translucent[1] == 0 && translucent[2] > 240);
        assert!((126..=129).contains(&translucent[3]), "{translucent:?}");
        assert_eq!(pixel(&pixmap, 20, 32)[3], 0);
    }

    #[test]
    fn composite_layers_blend_source_onto_backdrop() {
        let palette = palette();
        let mut canvas = Canvas::new(SIZE, SIZE, y_up_root(), &palette, Color::BLACK, &Squares)
            .expect("a 64 pixel canvas allocates");
        canvas.push_layer(CompositeMode::SrcOver);
        canvas.push_clip_glyph(GlyphId::new(20));
        canvas.fill(Brush::Solid {
            palette_index: 0,
            alpha: 1.0,
        });
        canvas.pop_clip();
        canvas.push_layer(CompositeMode::SrcIn);
        canvas.push_clip_glyph(GlyphId::new(8));
        canvas.fill(Brush::Solid {
            palette_index: 2,
            alpha: 1.0,
        });
        canvas.pop_clip();
        canvas.pop_layer_with_mode(CompositeMode::SrcIn);
        canvas.pop_layer_with_mode(CompositeMode::SrcOver);
        let pixmap = canvas.into_pixmap();
        assert_eq!(pixel(&pixmap, 32, 32), [0, 255, 0, 255]);
        assert_eq!(pixel(&pixmap, 32, 16)[3], 0);
    }

    #[test]
    fn every_composite_mode_maps_to_its_blend_mode_and_unknown_clears() {
        let cases = [
            (CompositeMode::Clear, BlendMode::Clear),
            (CompositeMode::Src, BlendMode::Source),
            (CompositeMode::Dest, BlendMode::Destination),
            (CompositeMode::SrcOver, BlendMode::SourceOver),
            (CompositeMode::DestOver, BlendMode::DestinationOver),
            (CompositeMode::SrcIn, BlendMode::SourceIn),
            (CompositeMode::DestIn, BlendMode::DestinationIn),
            (CompositeMode::SrcOut, BlendMode::SourceOut),
            (CompositeMode::DestOut, BlendMode::DestinationOut),
            (CompositeMode::SrcAtop, BlendMode::SourceAtop),
            (CompositeMode::DestAtop, BlendMode::DestinationAtop),
            (CompositeMode::Xor, BlendMode::Xor),
            (CompositeMode::Plus, BlendMode::Plus),
            (CompositeMode::Screen, BlendMode::Screen),
            (CompositeMode::Overlay, BlendMode::Overlay),
            (CompositeMode::Darken, BlendMode::Darken),
            (CompositeMode::Lighten, BlendMode::Lighten),
            (CompositeMode::ColorDodge, BlendMode::ColorDodge),
            (CompositeMode::ColorBurn, BlendMode::ColorBurn),
            (CompositeMode::HardLight, BlendMode::HardLight),
            (CompositeMode::SoftLight, BlendMode::SoftLight),
            (CompositeMode::Difference, BlendMode::Difference),
            (CompositeMode::Exclusion, BlendMode::Exclusion),
            (CompositeMode::Multiply, BlendMode::Multiply),
            (CompositeMode::HslHue, BlendMode::Hue),
            (CompositeMode::HslSaturation, BlendMode::Saturation),
            (CompositeMode::HslColor, BlendMode::Color),
            (CompositeMode::HslLuminosity, BlendMode::Luminosity),
            (CompositeMode::new(200), BlendMode::Clear),
        ];
        for (mode, expected) in cases {
            assert_eq!(blend_mode(mode), expected, "{mode:?}");
        }
    }

    #[test]
    fn linear_and_radial_brushes_follow_the_transform() {
        let palette = palette();
        let stops = stops();
        let mut canvas = Canvas::new(SIZE, SIZE, y_up_root(), &palette, Color::BLACK, &Squares)
            .expect("a 64 pixel canvas allocates");
        canvas.push_transform(FontTransform {
            xx: 0.0,
            yx: 1.0,
            xy: -1.0,
            yy: 0.0,
            ..FontTransform::default()
        });
        canvas.fill(Brush::LinearGradient {
            p0: FontPoint::new(-20.0, 0.0),
            p1: FontPoint::new(20.0, 0.0),
            color_stops: &stops,
            extend: Extend::Pad,
        });
        canvas.pop_transform();
        let linear = canvas.into_pixmap();
        assert!(red_share(pixel(&linear, 32, 60)) > 0.95);
        assert!(red_share(pixel(&linear, 32, 4)) < 0.05);

        let mut canvas = Canvas::new(SIZE, SIZE, y_up_root(), &palette, Color::BLACK, &Squares)
            .expect("a 64 pixel canvas allocates");
        canvas.fill(Brush::RadialGradient {
            c0: FontPoint::new(0.0, 0.0),
            r0: 0.0,
            c1: FontPoint::new(0.0, 0.0),
            r1: 10.0,
            color_stops: &stops,
            extend: Extend::Repeat,
        });
        let radial = canvas.into_pixmap();
        let center = red_share(pixel(&radial, 32, 32));
        let rim = red_share(pixel(&radial, 41, 32));
        let repeated = red_share(pixel(&radial, 43, 32));
        assert!(center > 0.7, "{center}");
        assert!(rim < 0.3, "{rim}");
        assert!(repeated > 0.6, "{repeated}");
    }
}
