use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{App, Bounds, Hsla, Pixels, SharedString, TransformationMatrix, Window};

use super::super::geometry::CellGeometry;
use super::super::sprites::{Corner, Powerline};

const MAX_CACHED_MASKS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum MaskShape {
    Arc(Corner),
    DiagonalRising,
    DiagonalFalling,
    DiagonalCross,
    Powerline(Powerline),
    TriangleHypotenuse(Corner),
    Undercurl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct MaskKey {
    pub(super) shape: MaskShape,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) thickness: i32,
    pub(super) offset: i32,
}

impl MaskKey {
    fn asset_path(&self) -> SharedString {
        format!(
            "paneflow://terminal-mask/{:?}/{}x{}/{}/{}",
            self.shape, self.width, self.height, self.thickness, self.offset
        )
        .into()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Style {
    Fill,
    Stroke(f32),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Outline {
    style: Style,
    data: String,
}

impl Outline {
    pub(super) fn fill() -> Self {
        Self {
            style: Style::Fill,
            data: String::new(),
        }
    }

    pub(super) fn stroke(width: f32) -> Self {
        Self {
            style: Style::Stroke(width),
            data: String::new(),
        }
    }

    pub(super) fn move_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.data, "M{x} {y}");
    }

    pub(super) fn line_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.data, "L{x} {y}");
    }

    pub(super) fn cubic_to(&mut self, to: (f32, f32), ctrl_a: (f32, f32), ctrl_b: (f32, f32)) {
        let _ = write!(
            self.data,
            "C{} {} {} {} {} {}",
            ctrl_a.0, ctrl_a.1, ctrl_b.0, ctrl_b.1, to.0, to.1
        );
    }

    pub(super) fn close(&mut self) {
        self.data.push('Z');
    }

    fn svg(&self, width: i32, top: i32, height: i32) -> String {
        let paint = match self.style {
            Style::Fill => "fill=\"#000\"".to_string(),
            Style::Stroke(width) => {
                format!("fill=\"none\" stroke=\"#000\" stroke-width=\"{width}\"")
            }
        };
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 {top} {width} {height}\"><path d=\"{}\" {paint}/></svg>",
            self.data
        )
    }
}

pub(super) fn outline(key: MaskKey) -> Outline {
    match key.shape {
        MaskShape::Arc(corner) => super::sprites::arc_outline(corner, key),
        MaskShape::DiagonalRising => super::sprites::diagonal_outline(true, false, key),
        MaskShape::DiagonalFalling => super::sprites::diagonal_outline(false, true, key),
        MaskShape::DiagonalCross => super::sprites::diagonal_outline(true, true, key),
        MaskShape::Powerline(symbol) => super::sprites::powerline_outline(symbol, key),
        MaskShape::TriangleHypotenuse(corner) => {
            super::sprites::triangle_hypotenuse_outline(corner, key)
        }
        MaskShape::Undercurl => super::decorations::undercurl_outline(key),
    }
}

pub(super) fn band(key: MaskKey) -> (i32, i32) {
    match key.shape {
        MaskShape::Undercurl => super::decorations::undercurl_band(key),
        _ => (0, key.height),
    }
}

fn mask_svg(key: MaskKey) -> String {
    let (top, height) = band(key);
    outline(key).svg(key.width, top, height)
}

struct CachedMask {
    asset_path: SharedString,
    svg: Box<[u8]>,
}

thread_local! {
    static MASKS: RefCell<HashMap<MaskKey, CachedMask>> = RefCell::new(HashMap::new());
}

static WARNED: AtomicBool = AtomicBool::new(false);

pub(super) fn paint_mask(
    window: &mut Window,
    cx: &App,
    geom: &CellGeometry,
    origin: (i32, i32),
    key: MaskKey,
    color: Hsla,
) {
    let (top, height) = band(key);
    let bounds = geom.device_rect(origin.0, origin.1 + top, key.width, height);
    if let Err(error) = try_paint_mask(window, cx, bounds, key, color)
        && !WARNED.swap(true, Ordering::Relaxed)
    {
        log::warn!("terminal: a box-drawing or decoration mask failed to rasterize: {error:#}");
    }
}

fn try_paint_mask(
    window: &mut Window,
    cx: &App,
    bounds: Bounds<Pixels>,
    key: MaskKey,
    color: Hsla,
) -> anyhow::Result<()> {
    if key.width <= 0 || band(key).1 <= 0 {
        return Ok(());
    }
    MASKS.with(|masks| {
        let mut masks = masks.borrow_mut();
        if masks.len() >= MAX_CACHED_MASKS && !masks.contains_key(&key) {
            masks.clear();
        }
        let mask = masks.entry(key).or_insert_with(|| CachedMask {
            asset_path: key.asset_path(),
            svg: mask_svg(key).into_bytes().into(),
        });
        window.paint_svg(
            bounds,
            mask.asset_path.clone(),
            Some(&mask.svg),
            TransformationMatrix::unit(),
            color,
            cx,
        )
    })
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use gpui::{Context, IntoElement, Render, Styled, canvas, hsla, point, px, size};

    use super::super::super::sprites::{Corner, Powerline};
    use super::*;

    const CORNERS: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];

    const POWERLINE: [Powerline; 14] = [
        Powerline::RightTriangle,
        Powerline::RightChevron,
        Powerline::LeftTriangle,
        Powerline::LeftChevron,
        Powerline::RightHalfCircle,
        Powerline::RightHalfCircleOutline,
        Powerline::LeftHalfCircle,
        Powerline::LeftHalfCircleOutline,
        Powerline::LowerLeftTriangle,
        Powerline::LowerRightTriangle,
        Powerline::UpperLeftTriangle,
        Powerline::UpperRightTriangle,
        Powerline::LeftTrapezoid,
        Powerline::RightTrapezoid,
    ];

    fn every_shape() -> Vec<MaskShape> {
        let mut shapes: Vec<MaskShape> = CORNERS.into_iter().map(MaskShape::Arc).collect();
        shapes.extend([
            MaskShape::DiagonalRising,
            MaskShape::DiagonalFalling,
            MaskShape::DiagonalCross,
            MaskShape::Undercurl,
        ]);
        shapes.extend(POWERLINE.into_iter().map(MaskShape::Powerline));
        shapes.extend(CORNERS.into_iter().map(MaskShape::TriangleHypotenuse));
        shapes
    }

    fn key(shape: MaskShape, width: i32, height: i32, thickness: i32) -> MaskKey {
        let offset = if shape == MaskShape::Undercurl {
            height * 2 / 3
        } else {
            0
        };
        MaskKey {
            shape,
            width,
            height,
            thickness,
            offset,
        }
    }

    struct Coverage {
        width: u32,
        height: i32,
        alpha: Vec<u8>,
    }

    impl Coverage {
        fn of(key: MaskKey) -> Self {
            let svg = mask_svg(key);
            let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &Default::default())
                .expect("every mask is valid SVG");
            let band = band(key).1;
            let (width, height) = (key.width as u32 * 2, band as u32 * 2);
            let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).unwrap();
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(2.0, 2.0),
                &mut pixmap.as_mut(),
            );
            Self {
                width,
                height: band,
                alpha: pixmap.pixels().iter().map(|pixel| pixel.alpha()).collect(),
            }
        }

        fn at(&self, x: i32, y: i32) -> f32 {
            let mut sum = 0.0;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let index = (y as u32 * 2 + dy) * self.width + x as u32 * 2 + dx;
                sum += f32::from(self.alpha[index as usize]) / 255.0;
            }
            sum / 4.0
        }

        fn total(&self) -> f32 {
            self.alpha.iter().map(|&a| f32::from(a) / 255.0).sum()
        }
    }

    #[test]
    fn every_terminal_mask_is_valid_svg_with_coverage() {
        for shape in every_shape() {
            for (width, height, thickness) in [(9, 18, 1), (18, 36, 2), (4, 7, 1)] {
                let coverage = Coverage::of(key(shape, width, height, thickness));
                assert!(
                    coverage.total() > 0.5,
                    "{shape:?} at {width}x{height} covers nothing"
                );
            }
        }
    }

    #[test]
    fn a_rounded_corner_joins_the_two_cell_edges_it_names() {
        let (w, h, t) = (18, 36, 2);
        let mid_x = (w - t) / 2;
        let mid_y = (h - t) / 2;
        for corner in CORNERS {
            let coverage = Coverage::of(key(MaskShape::Arc(corner), w, h, t));
            let (edge_y, far_y) = match corner {
                Corner::TopLeft | Corner::TopRight => (0, h - 1),
                Corner::BottomLeft | Corner::BottomRight => (h - 1, 0),
            };
            let (edge_x, far_x) = match corner {
                Corner::TopLeft | Corner::BottomLeft => (0, w - 1),
                Corner::TopRight | Corner::BottomRight => (w - 1, 0),
            };
            assert!(
                coverage.at(mid_x, edge_y) > 0.99,
                "{corner:?} leaves its vertical edge"
            );
            assert!(
                coverage.at(edge_x, mid_y) > 0.99,
                "{corner:?} leaves its horizontal edge"
            );
            assert_eq!(
                coverage.at(far_x, far_y),
                0.0,
                "{corner:?} reaches the far corner"
            );
            assert_eq!(
                coverage.at(edge_x, edge_y),
                0.0,
                "{corner:?} fills its own corner"
            );
        }
    }

    #[test]
    fn a_triangle_hypotenuse_stays_off_its_legs_and_leaves_the_inside_empty() {
        for (w, h, t) in [(18, 36, 2), (9, 18, 1), (4, 7, 1)] {
            for corner in CORNERS {
                let coverage = Coverage::of(key(MaskShape::TriangleHypotenuse(corner), w, h, t));
                let left = matches!(corner, Corner::TopLeft | Corner::BottomLeft);
                let top = matches!(corner, Corner::TopLeft | Corner::TopRight);
                let leg_x = if left { 0 } else { w - 1 };
                let leg_y = if top { 0 } else { h - 1 };
                let far_x = w - 1 - leg_x;
                let far_y = h - 1 - leg_y;
                for y in 0..h {
                    for x in 0..t {
                        let x = if left { x } else { w - 1 - x };
                        assert_eq!(
                            coverage.at(x, y),
                            0.0,
                            "{corner:?} {w}x{h} paints its vertical leg at {x},{y}"
                        );
                    }
                }
                for x in 0..w {
                    for y in 0..t {
                        let y = if top { y } else { h - 1 - y };
                        assert_eq!(
                            coverage.at(x, y),
                            0.0,
                            "{corner:?} {w}x{h} paints its horizontal leg at {x},{y}"
                        );
                    }
                }
                assert_eq!(
                    coverage.at(far_x, far_y),
                    0.0,
                    "{corner:?} {w}x{h} reaches the far corner"
                );
                let towards = |from: i32, to: i32| from + (to - from) / 4;
                assert!(
                    coverage.at(
                        towards(leg_x, far_x).max(t).min(w - 1 - t),
                        (h / 2).min(h - 1)
                    ) < 0.5
                        || w < 8,
                    "{corner:?} {w}x{h} fills the inside"
                );
                let ink: f32 = (0..h)
                    .flat_map(|y| (0..w).map(move |x| (x, y)))
                    .map(|(x, y)| coverage.at(x, y))
                    .sum();
                let hypotenuse = ((w * w + h * h) as f32).sqrt()
                    * (1.0 - t as f32 / w as f32 - t as f32 / h as f32);
                assert!(
                    ink > 0.5 * t as f32 * hypotenuse && ink < 1.5 * t as f32 * hypotenuse,
                    "{corner:?} {w}x{h} inks {ink} pixels for a stroke {hypotenuse} long between the legs and {t} wide"
                );
            }
        }
    }

    #[test]
    fn a_diagonal_runs_corner_to_corner_and_a_cross_draws_both() {
        let (w, h, t) = (18, 36, 2);
        let rising = Coverage::of(key(MaskShape::DiagonalRising, w, h, t));
        let falling = Coverage::of(key(MaskShape::DiagonalFalling, w, h, t));
        let cross = Coverage::of(key(MaskShape::DiagonalCross, w, h, t));
        assert!(rising.at(w - 1, 0) > 0.4 && rising.at(0, h - 1) > 0.4);
        assert_eq!(rising.at(0, 0), 0.0);
        assert!(falling.at(0, 0) > 0.4 && falling.at(w - 1, h - 1) > 0.4);
        assert_eq!(falling.at(w - 1, 0), 0.0);
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert!(cross.at(x, y) > 0.4, "the cross misses the corner {x},{y}");
        }
    }

    #[test]
    fn an_undercurl_leaves_a_cell_at_the_height_it_enters_the_next() {
        let (w, h, t) = (18, 45, 2);
        let undercurl = key(MaskShape::Undercurl, w, h, t);
        let coverage = Coverage::of(undercurl);
        let (top, height) = band(undercurl);
        let amplitude = w as f32 / std::f32::consts::PI;
        assert_eq!(coverage.height, height);
        assert!(top > 0 && top + height <= h);
        assert!(
            height as f32 <= amplitude + t as f32 + 3.0,
            "the mask spans {height} rows for a wave {amplitude} high"
        );
        let column = |x: i32| (0..height).map(|y| coverage.at(x, y)).collect::<Vec<_>>();
        let (left, right) = (column(0), column(w - 1));
        for y in 0..height as usize {
            assert!(
                (left[y] - right[y]).abs() < 0.2,
                "row {y}: the wave enters at {} and leaves at {}",
                left[y],
                right[y]
            );
        }
        let crest = (0..height).find(|&y| coverage.at(w / 2, y) > 0.5).unwrap();
        let trough = (0..height).find(|&y| coverage.at(0, y) > 0.5).unwrap();
        assert!(
            crest < trough,
            "the crest at {crest} sits below the trough at {trough}"
        );
        assert_eq!(coverage.at(w / 2, height - 1), 0.0);
        assert_eq!(coverage.at(0, 0), 0.0);
    }

    struct MaskView {
        keys: Vec<MaskKey>,
        results: Rc<RefCell<Vec<Result<(), String>>>>,
    }

    impl Render for MaskView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let keys = self.keys.clone();
            let results = self.results.clone();
            canvas(
                |_, _, _| {},
                move |_, _, window, cx| {
                    for key in &keys {
                        let bounds = Bounds::new(
                            point(px(10.0), px(10.0)),
                            size(px(key.width as f32 / 2.0), px(band(*key).1 as f32 / 2.0)),
                        );
                        let result =
                            try_paint_mask(window, cx, bounds, *key, hsla(0.0, 0.0, 1.0, 1.0))
                                .map_err(|error| format!("{key:?}: {error:#}"));
                        results.borrow_mut().push(result);
                    }
                },
            )
            .size_full()
        }
    }

    fn paint(cx: &mut gpui::TestAppContext, keys: Vec<MaskKey>) -> Vec<Result<(), String>> {
        let results = Rc::new(RefCell::new(Vec::new()));
        let shared = results.clone();
        let (_view, cx) = cx.add_window_view(move |_, _| MaskView {
            keys,
            results: shared,
        });
        cx.simulate_scale_factor_change(2.0);
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        results.take()
    }

    fn cached_masks() -> usize {
        MASKS.with(|masks| masks.borrow().len())
    }

    #[gpui::test]
    fn every_terminal_mask_rasterizes_through_the_gpui_svg_renderer(cx: &mut gpui::TestAppContext) {
        let keys: Vec<MaskKey> = every_shape()
            .into_iter()
            .map(|shape| key(shape, 18, 36, 2))
            .collect();
        let count = keys.len();
        let results = paint(cx, keys);
        assert!(results.len() >= count);
        for result in results {
            assert_eq!(result, Ok(()));
        }
    }

    #[gpui::test]
    fn a_mask_is_built_once_per_shape_and_cell_size(cx: &mut gpui::TestAppContext) {
        let before = cached_masks();
        let arc = key(MaskShape::Arc(Corner::TopLeft), 18, 36, 2);
        let larger = key(MaskShape::Arc(Corner::TopLeft), 20, 40, 2);
        let results = paint(cx, vec![arc; 50]);
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(cached_masks(), before + 1);
        let results = paint(cx, vec![arc, larger]);
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(cached_masks(), before + 2);
    }
}
