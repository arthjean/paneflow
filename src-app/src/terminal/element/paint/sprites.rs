use gpui::{Hsla, PathBuilder, Pixels, Point, Window, fill, point, px};

use super::super::LayoutState;
use super::super::geometry::CellGeometry;
use super::super::sprites::{Arm, Corner, DashGap, Lines, Powerline, Sprite};

pub fn paint_sprites(layout: &LayoutState, geom: &CellGeometry, window: &mut Window) {
    if layout.sprites.is_empty() || layout.desired_cols == 0 || layout.desired_rows == 0 {
        return;
    }
    for glyph in &layout.sprites {
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
            Sprite::Arc(corner) => canvas.arc(corner),
            Sprite::DiagonalUpperRightToLowerLeft => canvas.diagonal_upper_right_to_lower_left(),
            Sprite::DiagonalUpperLeftToLowerRight => canvas.diagonal_upper_left_to_lower_right(),
            Sprite::DiagonalCross => {
                canvas.diagonal_upper_right_to_lower_left();
                canvas.diagonal_upper_left_to_lower_right();
            }
            Sprite::Shade(shade) => {
                let (w, h) = (canvas.width, canvas.height);
                let color = canvas.color.opacity(shade.alpha());
                canvas.rect_colored(0, 0, w, h, color);
            }
            Sprite::Braille(pattern) => canvas.braille(pattern),
            Sprite::Powerline(symbol) => canvas.powerline(symbol),
        }
    }
}

struct Canvas<'a> {
    geom: &'a CellGeometry,
    window: &'a mut Window,
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

    fn pt(&self, x: f32, y: f32) -> Point<Pixels> {
        point(
            px((self.x0 as f32 + x) / self.geom.scale_factor),
            px((self.y0 as f32 + y) / self.geom.scale_factor),
        )
    }

    fn stroke_width(&self, device: i32) -> Pixels {
        px(device.max(1) as f32 / self.geom.scale_factor)
    }

    fn paint_path(&mut self, builder: PathBuilder) {
        if let Ok(path) = builder.build() {
            self.window.paint_path(path, self.color);
        }
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

    fn arc(&mut self, corner: Corner) {
        let thick = self.light();
        let (w, h) = (self.width as f32, self.height as f32);
        let t = thick as f32;
        let cx = ((self.width - thick).max(0) / 2) as f32 + t / 2.0;
        let cy = ((self.height - thick).max(0) / 2) as f32 + t / 2.0;
        let r = w.min(h) / 2.0;
        let s = 0.25;

        let mut path = PathBuilder::stroke(self.stroke_width(thick));
        match corner {
            Corner::TopLeft => {
                path.move_to(self.pt(cx, 0.0));
                path.line_to(self.pt(cx, cy - r));
                path.cubic_bezier_to(
                    self.pt(cx - r, cy),
                    self.pt(cx, cy - s * r),
                    self.pt(cx - s * r, cy),
                );
                path.line_to(self.pt(0.0, cy));
            }
            Corner::TopRight => {
                path.move_to(self.pt(cx, 0.0));
                path.line_to(self.pt(cx, cy - r));
                path.cubic_bezier_to(
                    self.pt(cx + r, cy),
                    self.pt(cx, cy - s * r),
                    self.pt(cx + s * r, cy),
                );
                path.line_to(self.pt(w, cy));
            }
            Corner::BottomLeft => {
                path.move_to(self.pt(cx, h));
                path.line_to(self.pt(cx, cy + r));
                path.cubic_bezier_to(
                    self.pt(cx - r, cy),
                    self.pt(cx, cy + s * r),
                    self.pt(cx - s * r, cy),
                );
                path.line_to(self.pt(0.0, cy));
            }
            Corner::BottomRight => {
                path.move_to(self.pt(cx, h));
                path.line_to(self.pt(cx, cy + r));
                path.cubic_bezier_to(
                    self.pt(cx + r, cy),
                    self.pt(cx, cy + s * r),
                    self.pt(cx + s * r, cy),
                );
                path.line_to(self.pt(w, cy));
            }
        }
        self.paint_path(path);
    }

    fn diagonal_slopes(&self) -> (f32, f32) {
        let (w, h) = (self.width as f32, self.height as f32);
        ((w / h).min(1.0), (h / w).min(1.0))
    }

    fn diagonal_upper_right_to_lower_left(&mut self) {
        let (w, h) = (self.width as f32, self.height as f32);
        let (sx, sy) = self.diagonal_slopes();
        let mut path = PathBuilder::stroke(self.stroke_width(self.light()));
        path.move_to(self.pt(w + 0.5 * sx, -0.5 * sy));
        path.line_to(self.pt(-0.5 * sx, h + 0.5 * sy));
        self.paint_path(path);
    }

    fn diagonal_upper_left_to_lower_right(&mut self) {
        let (w, h) = (self.width as f32, self.height as f32);
        let (sx, sy) = self.diagonal_slopes();
        let mut path = PathBuilder::stroke(self.stroke_width(self.light()));
        path.move_to(self.pt(-0.5 * sx, -0.5 * sy));
        path.line_to(self.pt(w + 0.5 * sx, h + 0.5 * sy));
        self.paint_path(path);
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

    fn fill_polygon(&mut self, points: &[(f32, f32)]) {
        let mut path = PathBuilder::fill();
        let mut iter = points.iter();
        if let Some(&(x, y)) = iter.next() {
            path.move_to(self.pt(x, y));
        }
        for &(x, y) in iter {
            path.line_to(self.pt(x, y));
        }
        path.close();
        self.paint_path(path);
    }

    fn powerline(&mut self, symbol: Powerline) {
        let (w, h) = (self.width as f32, self.height as f32);
        match symbol {
            Powerline::RightTriangle => self.fill_polygon(&[(0.0, 0.0), (w, h / 2.0), (0.0, h)]),
            Powerline::LeftTriangle => self.fill_polygon(&[(w, 0.0), (0.0, h / 2.0), (w, h)]),
            Powerline::LowerLeftTriangle => self.fill_polygon(&[(0.0, 0.0), (w, h), (0.0, h)]),
            Powerline::LowerRightTriangle => self.fill_polygon(&[(w, 0.0), (w, h), (0.0, h)]),
            Powerline::UpperLeftTriangle => self.fill_polygon(&[(0.0, 0.0), (w, 0.0), (0.0, h)]),
            Powerline::UpperRightTriangle => self.fill_polygon(&[(0.0, 0.0), (w, 0.0), (w, h)]),
            Powerline::RightChevron => self.chevron(false),
            Powerline::LeftChevron => self.chevron(true),
            Powerline::RightHalfCircle => self.half_circle(false),
            Powerline::LeftHalfCircle => self.half_circle(true),
            Powerline::RightHalfCircleOutline => self.half_circle_outline(false),
            Powerline::LeftHalfCircleOutline => self.half_circle_outline(true),
            Powerline::LeftTrapezoid => self.trapezoids(false),
            Powerline::RightTrapezoid => self.trapezoids(true),
        }
    }

    fn mirror_x(&self, mirrored: bool, x: f32) -> f32 {
        if mirrored { self.width as f32 - x } else { x }
    }

    fn chevron(&mut self, mirrored: bool) {
        let (w, h) = (self.width as f32, self.height as f32);
        let mut path = PathBuilder::stroke(self.stroke_width(self.light()));
        path.move_to(self.pt(self.mirror_x(mirrored, 0.0), 0.0));
        path.line_to(self.pt(self.mirror_x(mirrored, w), h / 2.0));
        path.line_to(self.pt(self.mirror_x(mirrored, 0.0), h));
        self.paint_path(path);
    }

    const ARC_C: f32 = (std::f32::consts::SQRT_2 - 1.0) * 4.0 / 3.0;

    fn half_circle(&mut self, mirrored: bool) {
        let (w, h) = (self.width as f32, self.height as f32);
        let r = w.min(h / 2.0);
        let c = Self::ARC_C;
        let m = |this: &Self, x: f32| this.mirror_x(mirrored, x);
        let mut path = PathBuilder::fill();
        path.move_to(self.pt(m(self, 0.0), 0.0));
        path.cubic_bezier_to(
            self.pt(m(self, r), r),
            self.pt(m(self, r * c), 0.0),
            self.pt(m(self, r), r - r * c),
        );
        path.line_to(self.pt(m(self, r), h - r));
        path.cubic_bezier_to(
            self.pt(m(self, 0.0), h),
            self.pt(m(self, r), h - r + r * c),
            self.pt(m(self, r * c), h),
        );
        path.close();
        self.paint_path(path);
    }

    fn half_circle_outline(&mut self, mirrored: bool) {
        let (w, h) = (self.width as f32, self.height as f32);
        let t = self.light() as f32;
        let r = (w.min(h / 2.0) - t / 2.0).max(t);
        let c = Self::ARC_C;
        let oy = t / 2.0;
        let m = |this: &Self, x: f32| this.mirror_x(mirrored, x);
        let mut path = PathBuilder::stroke(self.stroke_width(self.light()));
        path.move_to(self.pt(m(self, 0.0), oy));
        path.cubic_bezier_to(
            self.pt(m(self, r), oy + r),
            self.pt(m(self, r * c), oy),
            self.pt(m(self, r), oy + r - r * c),
        );
        path.line_to(self.pt(m(self, r), h - oy - r));
        path.cubic_bezier_to(
            self.pt(m(self, 0.0), h - oy),
            self.pt(m(self, r), h - oy - r + r * c),
            self.pt(m(self, r * c), h - oy),
        );
        self.paint_path(path);
    }

    fn trapezoids(&mut self, mirrored: bool) {
        let (w, h) = (self.width as f32, self.height as f32);
        let t = self.light() as f32;
        let m = |this: &Self, x: f32| this.mirror_x(mirrored, x);
        let top = [
            (m(self, 0.0), 0.0),
            (m(self, w), 0.0),
            (m(self, w / 2.0), h / 2.0 - t / 2.0),
            (m(self, 0.0), h / 2.0 - t / 2.0),
        ];
        let bottom = [
            (m(self, 0.0), h),
            (m(self, w), h),
            (m(self, w / 2.0), h / 2.0 + t / 2.0),
            (m(self, 0.0), h / 2.0 + t / 2.0),
        ];
        self.fill_polygon(&top);
        self.fill_polygon(&bottom);
    }
}
