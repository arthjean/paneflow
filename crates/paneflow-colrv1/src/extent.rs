use skrifa::color::{Brush, ColorPainter, CompositeMode, Transform as FontTransform};
use skrifa::raw::types::BoundingBox;
use skrifa::GlyphId;
use tiny_skia::{Path, Rect, Transform};

use crate::canvas::{clip_box_path, to_skia_transform};
use crate::outline::OutlineSource;

pub struct ExtentProbe<'a, O> {
    outlines: &'a O,
    transforms: Vec<Transform>,
    clips: Vec<Option<Rect>>,
    painted: Option<Rect>,
}

impl<'a, O: OutlineSource> ExtentProbe<'a, O> {
    pub fn new(root: Transform, outlines: &'a O) -> Self {
        Self {
            outlines,
            transforms: vec![root],
            clips: Vec::new(),
            painted: None,
        }
    }

    pub fn painted(&self) -> Option<Rect> {
        self.painted
    }

    fn transform(&self) -> Transform {
        self.transforms
            .last()
            .copied()
            .unwrap_or_else(Transform::identity)
    }

    fn push_clip_path(&mut self, path: Option<Path>) {
        let bounds = path
            .and_then(|path| path.transform(self.transform()))
            .map(|path| path.bounds());
        let clipped = match self.clips.last() {
            None => bounds,
            Some(Some(parent)) => bounds.and_then(|bounds| bounds.intersect(parent)),
            Some(None) => None,
        };
        self.clips.push(clipped);
    }
}

fn union(left: Option<Rect>, right: Rect) -> Option<Rect> {
    let Some(left) = left else {
        return Some(right);
    };
    Rect::from_ltrb(
        left.left().min(right.left()),
        left.top().min(right.top()),
        left.right().max(right.right()),
        left.bottom().max(right.bottom()),
    )
}

impl<O: OutlineSource> ColorPainter for ExtentProbe<'_, O> {
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

    fn fill(&mut self, _brush: Brush<'_>) {
        if let Some(Some(clip)) = self.clips.last() {
            self.painted = union(self.painted, *clip);
        }
    }

    fn push_layer(&mut self, _composite_mode: CompositeMode) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::PathBuilder;

    struct Squares;

    impl OutlineSource for Squares {
        fn glyph_path(&self, glyph_id: GlyphId) -> Option<Path> {
            let half = glyph_id.to_u32() as f32;
            Some(PathBuilder::from_rect(Rect::from_ltrb(
                -half, -half, half, half,
            )?))
        }
    }

    fn solid() -> Brush<'static> {
        Brush::Solid {
            palette_index: 0,
            alpha: 1.0,
        }
    }

    #[test]
    fn the_painted_extent_is_the_union_of_every_clipped_fill() {
        let mut probe = ExtentProbe::new(Transform::from_scale(2.0, 2.0), &Squares);
        probe.push_clip_glyph(GlyphId::new(5));
        probe.fill(solid());
        probe.pop_clip();
        probe.push_transform(FontTransform {
            dx: 30.0,
            ..FontTransform::default()
        });
        probe.push_clip_glyph(GlyphId::new(2));
        probe.push_clip_glyph(GlyphId::new(10));
        probe.fill(solid());
        probe.pop_clip();
        probe.pop_clip();
        probe.pop_transform();
        probe.fill(solid());
        assert_eq!(probe.painted(), Rect::from_ltrb(-10.0, -10.0, 64.0, 10.0));
    }

    #[test]
    fn a_fill_outside_every_clip_adds_nothing() {
        let mut probe = ExtentProbe::new(Transform::identity(), &Squares);
        probe.fill(solid());
        assert_eq!(probe.painted(), None);
    }
}
