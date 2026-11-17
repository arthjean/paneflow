use gpui::{App, Hsla, Window, fill};

use super::super::LayoutState;
use super::super::geometry::CellGeometry;
use super::super::sprites::{Arm, Corner, DashGap, Lines, Powerline, Sprite};
use super::masks::{MaskKey, MaskShape, Outline, paint_mask};

pub fn paint_sprites(layout: &LayoutState, geom: &CellGeometry, window: &mut Window, cx: &App) {
    if layout.sprites().next().is_none() || layout.desired_cols == 0 || layout.desired_rows == 0 {
        return;
    }
    for glyph in layout.sprites() {
        let col_end = glyph.col + glyph.num_cols;
        if glyph.num_cols == 0
            || col_end > layout.desired_cols
            || glyph.line < 0
            || glyph.line as usize >= layout.desired_rows
        {
            continue;
        }
        let x0 = geom.x_device(glyph.col);
        let y0 = geom.y_device(glyph.line);
        let mut canvas = Canvas {
            geom,
            window,
            cx,
            x0,
            y0,
            width: geom.x_device(col_end) - x0,
            height: geom.y_device(glyph.line + 1) - y0,
            thickness: geom.metrics.box_thickness.max(1),
            color: glyph.color,
        };
        if canvas.width <= 0 || canvas.height <= 0 {
            continue;
        }
        match glyph.sprite {
            Sprite::Lines(lines) => canvas.lines(lines),
            Sprite::DashHorizontal { count, heavy, gap } => {
                canvas.dash_horizontal(count, heavy, gap)
            }
            Sprite::DashVertical { count, heavy, gap } => canvas.dash_vertical(count, heavy, gap),
            Sprite::Arc(corner) => canvas.mask(MaskShape::Arc(corner)),
            Sprite::DiagonalUpperRightToLowerLeft => canvas.mask(MaskShape::DiagonalRising),
            Sprite::DiagonalUpperLeftToLowerRight => canvas.mask(MaskShape::DiagonalFalling),
            Sprite::DiagonalCross => canvas.mask(MaskShape::DiagonalCross),
            Sprite::Shade(shade) => {
                let (w, h) = (canvas.width, canvas.height);
                let color = canvas.color.opacity(shade.alpha());
                canvas.rect_colored(0, 0, w, h, color);
            }
            Sprite::Braille(pattern) => canvas.braille(pattern),
            Sprite::Powerline(symbol) => canvas.mask(MaskShape::Powerline(symbol)),
            Sprite::CornerTriangleOutline(corner) => canvas.corner_triangle_outline(corner),
        }
    }
}

struct Canvas<'a> {
    geom: &'a CellGeometry,
    window: &'a mut Window,
    cx: &'a App,
    x0: i32,
    y0: i32,
    width: i32,
    height: i32,
    thickness: i32,
    color: Hsla,
}

impl Canvas<'_> {
    fn light(&self) -> i32 {
        self.thickness
    }

    fn heavy(&self) -> i32 {
        self.thickness * 2
    }

    fn rect(&mut self, x: i32, y: i32, width: i32, height: i32) {
        let color = self.color;
        self.rect_colored(x, y, width, height, color);
    }

    fn rect_colored(&mut self, x: i32, y: i32, width: i32, height: i32, color: Hsla) {
        if width <= 0 || height <= 0 {
            return;
        }
        let bounds = self
            .geom
            .device_rect(self.x0 + x, self.y0 + y, width, height);
        self.window.paint_quad(fill(bounds, color));
    }

    fn boxed(&mut self, x1: i32, y1: i32, x2: i32, y2: i32) {
        self.rect(x1, y1, x2 - x1, y2 - y1);
    }

    fn mask(&mut self, shape: MaskShape) {
        let key = MaskKey {
            shape,
            width: self.width,
            height: self.height,
            thickness: self.thickness,
            offset: 0,
        };
        paint_mask(
            self.window,
            self.cx,
            self.geom,
            (self.x0, self.y0),
            key,
            self.color,
        );
    }

    fn corner_triangle_outline(&mut self, corner: Corner) {
        let (w, h) = (self.width, self.height);
        let t = self.light().min(w).min(h);
        let top = matches!(corner, Corner::TopLeft | Corner::TopRight);
        let left = matches!(corner, Corner::TopLeft | Corner::BottomLeft);
        let leg_y = if top { 0 } else { h - t };
        let leg_x = if left { 0 } else { w - t };
        self.rect(0, leg_y, w, t);
        self.rect(leg_x, if top { t } else { 0 }, t, h - t);
        self.mask(MaskShape::TriangleHypotenuse(corner));
    }

    fn lines(&mut self, lines: Lines) {
        let (w, h) = (self.width, self.height);
        let light = self.light();
        let heavy = self.heavy();

        let h_light_top = (h - light).max(0) / 2;
        let h_light_bottom = h_light_top + light;
        let h_heavy_top = (h - heavy).max(0) / 2;
        let h_heavy_bottom = h_heavy_top + heavy;
        let h_double_top = (h_light_top - light).max(0);
        let h_double_bottom = h_light_bottom + light;

        let v_light_left = (w - light).max(0) / 2;
        let v_light_right = v_light_left + light;
        let v_heavy_left = (w - heavy).max(0) / 2;
        let v_heavy_right = v_heavy_left + heavy;
        let v_double_left = (v_light_left - light).max(0);
        let v_double_right = v_light_right + light;

        let Lines {
            up,
            right,
            down,
            left,
        } = lines;

        let up_bottom = if left == Arm::Heavy || right == Arm::Heavy {
            h_heavy_bottom
        } else if left != right || down == up {
            if left == Arm::Double || right == Arm::Double {
                h_double_bottom
            } else {
                h_light_bottom
            }
        } else if left == Arm::None && right == Arm::None {
            h_light_bottom
        } else {
            h_light_top
        };

        let down_top = if left == Arm::Heavy || right == Arm::Heavy {
            h_heavy_top
        } else if left != right || up == down {
            if left == Arm::Double || right == Arm::Double {
                h_double_top
            } else {
                h_light_top
            }
        } else if left == Arm::None && right == Arm::None {
            h_light_top
        } else {
            h_light_bottom
        };

        let left_right = if up == Arm::Heavy || down == Arm::Heavy {
            v_heavy_right
        } else if up != down || left == right {
            if up == Arm::Double || down == Arm::Double {
                v_double_right
            } else {
                v_light_right
            }
        } else if up == Arm::None && down == Arm::None {
            v_light_right
        } else {
            v_light_left
        };

        let right_left = if up == Arm::Heavy || down == Arm::Heavy {
            v_heavy_left
        } else if up != down || right == left {
            if up == Arm::Double || down == Arm::Double {
                v_double_left
            } else {
                v_light_left
            }
        } else if up == Arm::None && down == Arm::None {
            v_light_left
        } else {
            v_light_right
        };

        match up {
            Arm::None => {}
            Arm::Light => self.boxed(v_light_left, 0, v_light_right, up_bottom),
            Arm::Heavy => self.boxed(v_heavy_left, 0, v_heavy_right, up_bottom),
            Arm::Double => {
                let left_bottom = if left == Arm::Double {
                    h_light_top
                } else {
                    up_bottom
                };
                let right_bottom = if right == Arm::Double {
                    h_light_top
                } else {
                    up_bottom
                };
                self.boxed(v_double_left, 0, v_light_left, left_bottom);
                self.boxed(v_light_right, 0, v_double_right, right_bottom);
            }
        }

        match right {
            Arm::None => {}
            Arm::Light => self.boxed(right_left, h_light_top, w, h_light_bottom),
            Arm::Heavy => self.boxed(right_left, h_heavy_top, w, h_heavy_bottom),
            Arm::Double => {
                let top_left = if up == Arm::Double {
                    v_light_right
                } else {
                    right_left
                };
                let bottom_left = if down == Arm::Double {
                    v_light_right
                } else {
                    right_left
                };
                self.boxed(top_left, h_double_top, w, h_light_top);
                self.boxed(bottom_left, h_light_bottom, w, h_double_bottom);
            }
        }

        match down {
            Arm::None => {}
            Arm::Light => self.boxed(v_light_left, down_top, v_light_right, h),
            Arm::Heavy => self.boxed(v_heavy_left, down_top, v_heavy_right, h),
            Arm::Double => {
                let left_top = if left == Arm::Double {
                    h_light_bottom
                } else {
                    down_top
                };
                let right_top = if right == Arm::Double {
                    h_light_bottom
                } else {
                    down_top
                };
                self.boxed(v_double_left, left_top, v_light_left, h);
                self.boxed(v_light_right, right_top, v_double_right, h);
            }
        }

        match left {
            Arm::None => {}
            Arm::Light => self.boxed(0, h_light_top, left_right, h_light_bottom),
            Arm::Heavy => self.boxed(0, h_heavy_top, left_right, h_heavy_bottom),
            Arm::Double => {
                let top_right = if up == Arm::Double {
                    v_light_left
                } else {
                    left_right
                };
                let bottom_right = if down == Arm::Double {
                    v_light_left
                } else {
                    left_right
                };
                self.boxed(0, h_double_top, top_right, h_light_top);
                self.boxed(0, h_light_bottom, bottom_right, h_double_bottom);
            }
        }
    }

    fn desired_gap(&self, gap: DashGap) -> i32 {
        match gap {
            DashGap::AtLeastFour => self.light().max(4),
            DashGap::Light => self.light(),
            DashGap::Heavy => self.heavy(),
        }
    }

    fn dash_horizontal(&mut self, count: u8, heavy: bool, gap: DashGap) {
        let count = i32::from(count.clamp(2, 4));
        let thick = if heavy { self.heavy() } else { self.light() };
        let w = self.width;
        if w < count * 2 {
            self.lines(Lines {
                up: Arm::None,
                right: Arm::Light,
                down: Arm::None,
                left: Arm::Light,
            });
            return;
        }
        let gap_width = self.desired_gap(gap).min(w / (2 * count));
        let total_dash = w - gap_width * count;
        let dash_width = total_dash / count;
        let mut extra = total_dash % count;
        let y = (self.height - thick).max(0) / 2;
        let mut x = gap_width / 2;
        for _ in 0..count {
            let mut x1 = x + dash_width;
            if extra > 0 {
                extra -= 1;
                x1 += 1;
            }
            self.boxed(x, y, x1, y + thick);
            x = x1 + gap_width;
        }
    }

    fn dash_vertical(&mut self, count: u8, heavy: bool, gap: DashGap) {
        let count = i32::from(count.clamp(2, 4));
        let thick = if heavy { self.heavy() } else { self.light() };
        let h = self.height;
        if h < count * 2 {
            self.lines(Lines {
                up: Arm::Light,
                right: Arm::None,
                down: Arm::Light,
                left: Arm::None,
            });
            return;
        }
        let gap_height = self.desired_gap(gap).min(h / (2 * count));
        let total_dash = h - gap_height * count;
        let dash_height = total_dash / count;
        let mut extra = total_dash % count;
        let x = (self.width - thick).max(0) / 2;
        let mut y = 0;
        for _ in 0..count {
            let mut y1 = y + dash_height;
            if extra > 0 {
                extra -= 1;
                y1 += 1;
            }
            self.boxed(x, y, x + thick, y1);
            y = y1 + gap_height;
        }
    }

    fn braille(&mut self, pattern: u8) {
        let (width, height) = (self.width, self.height);
        let mut w = (width / 4).min(height / 8);
        let mut x_spacing = width / 4;
        let mut y_spacing = height / 8;
        let mut x_margin = x_spacing / 2;
        let mut y_margin = y_spacing / 2;

        let mut x_left = width - 2 * x_margin - x_spacing - 2 * w;
        let mut y_left = height - 2 * y_margin - 3 * y_spacing - 4 * w;

        if x_left >= 2 && y_left >= 4 && w == 0 {
            w += 1;
            x_left -= 2;
            y_left -= 4;
        }
        if x_left >= 2 && x_margin == 0 {
            x_margin = 1;
            x_left -= 2;
        }
        if y_left >= 2 && y_margin == 0 {
            y_margin = 1;
            y_left -= 2;
        }
        if x_left >= 1 {
            x_spacing += 1;
            x_left -= 1;
        }
        if y_left >= 3 {
            y_spacing += 1;
            y_left -= 3;
        }
        if x_left >= 2 {
            x_margin += 1;
            x_left -= 2;
        }
        if y_left >= 2 {
            y_margin += 1;
            y_left -= 2;
        }
        if x_left >= 2 && y_left >= 4 {
            w += 1;
        }
        if w <= 0 {
            return;
        }

        let xs = [x_margin, x_margin + w + x_spacing];
        let mut ys = [y_margin; 4];
        for i in 1..4 {
            ys[i] = ys[i - 1] + w + y_spacing;
        }

        let dots = [
            (0x01, 0, 0),
            (0x02, 0, 1),
            (0x04, 0, 2),
            (0x08, 1, 0),
            (0x10, 1, 1),
            (0x20, 1, 2),
            (0x40, 0, 3),
            (0x80, 1, 3),
        ];
        for (bit, col, row) in dots {
            if pattern & bit != 0 {
                self.rect(xs[col], ys[row], w, w);
            }
        }
    }
}

const ARC_C: f32 = (std::f32::consts::SQRT_2 - 1.0) * 4.0 / 3.0;

fn stroke_width(thickness: i32) -> f32 {
    thickness.max(1) as f32
}

pub(super) fn arc_outline(corner: Corner, key: MaskKey) -> Outline {
    let (w, h) = (key.width as f32, key.height as f32);
    let t = key.thickness as f32;
    let cx = ((key.width - key.thickness).max(0) / 2) as f32 + t / 2.0;
    let cy = ((key.height - key.thickness).max(0) / 2) as f32 + t / 2.0;
    let r = w.min(h) / 2.0;
    let s = 0.25;
    let (edge_y, dy) = match corner {
        Corner::TopLeft | Corner::TopRight => (0.0, -1.0),
        Corner::BottomLeft | Corner::BottomRight => (h, 1.0),
    };
    let (edge_x, dx) = match corner {
        Corner::TopLeft | Corner::BottomLeft => (0.0, -1.0),
        Corner::TopRight | Corner::BottomRight => (w, 1.0),
    };
    let mut outline = Outline::stroke(stroke_width(key.thickness));
    outline.move_to(cx, edge_y);
    outline.line_to(cx, cy + dy * r);
    outline.cubic_to(
        (cx + dx * r, cy),
        (cx, cy + dy * s * r),
        (cx + dx * s * r, cy),
    );
    outline.line_to(edge_x, cy);
    outline
}

pub(super) fn diagonal_outline(rising: bool, falling: bool, key: MaskKey) -> Outline {
    let (w, h) = (key.width as f32, key.height as f32);
    let (sx, sy) = ((w / h).min(1.0), (h / w).min(1.0));
    let mut outline = Outline::stroke(stroke_width(key.thickness));
    if rising {
        outline.move_to(w + 0.5 * sx, -0.5 * sy);
        outline.line_to(-0.5 * sx, h + 0.5 * sy);
    }
    if falling {
        outline.move_to(-0.5 * sx, -0.5 * sy);
        outline.line_to(w + 0.5 * sx, h + 0.5 * sy);
    }
    outline
}

fn polygon(outline: &mut Outline, points: &[(f32, f32)]) {
    let mut points = points.iter();
    if let Some(&(x, y)) = points.next() {
        outline.move_to(x, y);
    }
    for &(x, y) in points {
        outline.line_to(x, y);
    }
    outline.close();
}

pub(super) fn triangle_hypotenuse_outline(corner: Corner, key: MaskKey) -> Outline {
    let (w, h) = (key.width as f32, key.height as f32);
    let t = (key.thickness.max(1) as f32).min(w).min(h);
    let inset = 1.0 - t * (1.0 / (w * w) + 1.0 / (h * h)).sqrt();
    let outer_at_leg_x = (t, h * (1.0 - t / w));
    let outer_at_leg_y = (w * (1.0 - t / h), t);
    let inner_at_leg_y = (w * (inset - t / h), t);
    let inner_at_leg_x = (t, h * (inset - t / w));
    let corner_point = |(x, y): (f32, f32)| {
        let x = match corner {
            Corner::TopLeft | Corner::BottomLeft => x,
            Corner::TopRight | Corner::BottomRight => w - x,
        };
        let y = match corner {
            Corner::TopLeft | Corner::TopRight => y,
            Corner::BottomLeft | Corner::BottomRight => h - y,
        };
        (x, y)
    };
    let points: Vec<(f32, f32)> = if inner_at_leg_y.0 > t {
        vec![
            outer_at_leg_x,
            outer_at_leg_y,
            inner_at_leg_y,
            inner_at_leg_x,
        ]
    } else {
        vec![outer_at_leg_x, outer_at_leg_y, (t, t)]
    };
    let points: Vec<(f32, f32)> = points.into_iter().map(corner_point).collect();
    let mut outline = Outline::fill();
    polygon(&mut outline, &points);
    outline
}

pub(super) fn powerline_outline(symbol: Powerline, key: MaskKey) -> Outline {
    let (w, h) = (key.width as f32, key.height as f32);
    let t = key.thickness as f32;
    let filled = |points: &[(f32, f32)]| {
        let mut outline = Outline::fill();
        polygon(&mut outline, points);
        outline
    };
    match symbol {
        Powerline::RightTriangle => filled(&[(0.0, 0.0), (w, h / 2.0), (0.0, h)]),
        Powerline::LeftTriangle => filled(&[(w, 0.0), (0.0, h / 2.0), (w, h)]),
        Powerline::LowerLeftTriangle => filled(&[(0.0, 0.0), (w, h), (0.0, h)]),
        Powerline::LowerRightTriangle => filled(&[(w, 0.0), (w, h), (0.0, h)]),
        Powerline::UpperLeftTriangle => filled(&[(0.0, 0.0), (w, 0.0), (0.0, h)]),
        Powerline::UpperRightTriangle => filled(&[(0.0, 0.0), (w, 0.0), (w, h)]),
        Powerline::RightChevron => chevron(false, key),
        Powerline::LeftChevron => chevron(true, key),
        Powerline::RightHalfCircle => half_circle(false, key),
        Powerline::LeftHalfCircle => half_circle(true, key),
        Powerline::RightHalfCircleOutline => half_circle_outline(false, key),
        Powerline::LeftHalfCircleOutline => half_circle_outline(true, key),
        Powerline::LeftTrapezoid => trapezoids(false, w, h, t),
        Powerline::RightTrapezoid => trapezoids(true, w, h, t),
    }
}

fn mirror(mirrored: bool, width: f32, x: f32) -> f32 {
    if mirrored { width - x } else { x }
}

fn chevron(mirrored: bool, key: MaskKey) -> Outline {
    let (w, h) = (key.width as f32, key.height as f32);
    let m = |x: f32| mirror(mirrored, w, x);
    let mut outline = Outline::stroke(stroke_width(key.thickness));
    outline.move_to(m(0.0), 0.0);
    outline.line_to(m(w), h / 2.0);
    outline.line_to(m(0.0), h);
    outline
}

fn half_circle(mirrored: bool, key: MaskKey) -> Outline {
    let (w, h) = (key.width as f32, key.height as f32);
    let r = w.min(h / 2.0);
    let c = ARC_C;
    let m = |x: f32| mirror(mirrored, w, x);
    let mut outline = Outline::fill();
    outline.move_to(m(0.0), 0.0);
    outline.cubic_to((m(r), r), (m(r * c), 0.0), (m(r), r - r * c));
    outline.line_to(m(r), h - r);
    outline.cubic_to((m(0.0), h), (m(r), h - r + r * c), (m(r * c), h));
    outline.close();
    outline
}

fn half_circle_outline(mirrored: bool, key: MaskKey) -> Outline {
    let (w, h) = (key.width as f32, key.height as f32);
    let t = key.thickness as f32;
    let r = (w.min(h / 2.0) - t / 2.0).max(t);
    let c = ARC_C;
    let oy = t / 2.0;
    let m = |x: f32| mirror(mirrored, w, x);
    let mut outline = Outline::stroke(stroke_width(key.thickness));
    outline.move_to(m(0.0), oy);
    outline.cubic_to((m(r), oy + r), (m(r * c), oy), (m(r), oy + r - r * c));
    outline.line_to(m(r), h - oy - r);
    outline.cubic_to(
        (m(0.0), h - oy),
        (m(r), h - oy - r + r * c),
        (m(r * c), h - oy),
    );
    outline
}

fn trapezoids(mirrored: bool, w: f32, h: f32, t: f32) -> Outline {
    let m = |x: f32| mirror(mirrored, w, x);
    let mut outline = Outline::fill();
    polygon(
        &mut outline,
        &[
            (m(0.0), 0.0),
            (m(w), 0.0),
            (m(w / 2.0), h / 2.0 - t / 2.0),
            (m(0.0), h / 2.0 - t / 2.0),
        ],
    );
    polygon(
        &mut outline,
        &[
            (m(0.0), h),
            (m(w), h),
            (m(w / 2.0), h / 2.0 + t / 2.0),
            (m(0.0), h / 2.0 + t / 2.0),
        ],
    );
    outline
}
