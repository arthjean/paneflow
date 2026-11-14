use std::cell::Cell;

use gpui::{App, Bounds, Corners, Pixels, px};
use paneflow_config::schema::InterfaceStyle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    Paneflow,
    Surface,
    Classic,
}

impl From<InterfaceStyle> for Shape {
    fn from(style: InterfaceStyle) -> Self {
        match style {
            InterfaceStyle::Paneflow => Self::Paneflow,
            InterfaceStyle::Classic => Self::Classic,
        }
    }
}

thread_local! {
    static ACTIVE: Cell<Shape> = const { Cell::new(Shape::Paneflow) };
}

pub(crate) fn set_interface_style(style: InterfaceStyle, cx: &mut App) {
    let shape = Shape::from(style);
    if ACTIVE.replace(shape) != shape {
        cx.refresh_windows();
    }
}

pub(crate) fn shape() -> Shape {
    ACTIVE.get()
}

struct Restore(Shape);

impl Drop for Restore {
    fn drop(&mut self) {
        ACTIVE.set(self.0);
    }
}

pub(crate) fn scoped<T>(shape: Shape, run: impl FnOnce() -> T) -> T {
    let _restore = Restore(ACTIVE.replace(shape));
    run()
}

pub(crate) fn surface_shape() -> Shape {
    match shape() {
        Shape::Paneflow => Shape::Surface,
        shape => shape,
    }
}

pub(crate) fn with_surface_shape<T>(run: impl FnOnce() -> T) -> T {
    scoped(surface_shape(), run)
}

pub(crate) fn with_paneflow_shape<T>(run: impl FnOnce() -> T) -> T {
    scoped(Shape::Paneflow, run)
}

pub(crate) fn circular() -> bool {
    shape() != Shape::Paneflow
}

pub(crate) fn classic() -> bool {
    shape() == Shape::Classic
}

pub(crate) fn pick<T>(paneflow: T, classic: T) -> T {
    match shape() {
        Shape::Paneflow | Shape::Surface => paneflow,
        Shape::Classic => classic,
    }
}

pub(crate) fn pick_surface<T>(paneflow: T, surface: T, classic: T) -> T {
    match shape() {
        Shape::Paneflow => paneflow,
        Shape::Surface => surface,
        Shape::Classic => classic,
    }
}

pub(crate) const SURFACE_CONTROL: Pixels = px(6.);
pub(crate) const SURFACE_ROW: Pixels = px(8.);
pub(crate) const SURFACE_CONTAINER: Pixels = px(12.);
pub(crate) const SURFACE_MENU: Pixels = px(14.);

pub(crate) const CLASSIC_COMPACT: Pixels = px(6.);
pub(crate) const CLASSIC_CONTROL: Pixels = px(8.);
pub(crate) const CLASSIC_CONTAINER: Pixels = px(12.);
pub(crate) const CLASSIC_MENU: Pixels = px(14.);

const CARD_DESIGN_RADIUS: f32 = 20.;
const CONTAINER_DESIGN_RADIUS: f32 = 16.;
const CONTROL_DESIGN_RADIUS: f32 = 8.;
const SURFACE_ROW_HEIGHT: Pixels = px(30.);
const CLASSIC_COMPACT_CONTROL_HEIGHT: Pixels = px(26.);

fn surface_corner(design: Pixels, short_side: Pixels) -> Pixels {
    let value = f32::from(design);
    let resolved = if value >= CARD_DESIGN_RADIUS {
        SURFACE_CONTAINER
    } else if value >= CONTAINER_DESIGN_RADIUS {
        SURFACE_MENU
    } else if value >= CONTROL_DESIGN_RADIUS {
        if short_side < SURFACE_ROW_HEIGHT {
            SURFACE_CONTROL
        } else {
            SURFACE_ROW
        }
    } else {
        design.min(SURFACE_CONTROL)
    };
    resolved.min(short_side / 2.).max(px(0.))
}

fn classic_corner(design: Pixels, short_side: Pixels) -> Pixels {
    let value = f32::from(design);
    let resolved = if value >= CARD_DESIGN_RADIUS {
        CLASSIC_CONTAINER
    } else if value >= CONTAINER_DESIGN_RADIUS {
        CLASSIC_MENU
    } else if value >= CONTROL_DESIGN_RADIUS {
        if short_side < CLASSIC_COMPACT_CONTROL_HEIGHT {
            CLASSIC_COMPACT
        } else {
            CLASSIC_CONTROL
        }
    } else {
        design.min(CLASSIC_COMPACT)
    };
    resolved.min(short_side / 2.).max(px(0.))
}

pub(crate) fn corner(design: Pixels, short_side: Pixels) -> Pixels {
    match shape() {
        Shape::Paneflow => design,
        Shape::Surface => surface_corner(design, short_side),
        Shape::Classic => classic_corner(design, short_side),
    }
}

pub(crate) fn corners(bounds: Bounds<Pixels>, design: Corners<Pixels>) -> Corners<Pixels> {
    let short_side = bounds.size.width.min(bounds.size.height);
    design.map(|radius| corner(*radius, short_side))
}

pub(crate) fn frame_corner(design: Pixels) -> Pixels {
    corner(design, Pixels::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paneflow_keeps_every_design_radius() {
        for design in [3., 6., 9., 14., 18., 20., 24.] {
            assert_eq!(corner(px(design), px(32.)), px(design));
        }
    }

    #[test]
    fn menus_and_cards_land_on_the_surface_scale() {
        for (design, short_side, expected) in [
            (24., 600., 12.),
            (20., 300., 12.),
            (18., 200., 14.),
            (16., 120., 14.),
            (14., 36., 8.),
            (14., 30., 8.),
            (14., 29., 6.),
            (14., 26., 6.),
            (9., 32., 8.),
            (8., 28., 6.),
            (8., 22., 6.),
            (7., 22., 6.),
            (6., 20., 6.),
            (4., 20., 4.),
            (3., 6., 3.),
        ] {
            assert_eq!(
                surface_corner(px(design), px(short_side)),
                px(expected),
                "design {design} on a {short_side} px side"
            );
        }
    }

    #[test]
    fn classic_surfaces_land_on_the_classic_scale() {
        for (design, short_side, expected) in [
            (24., 600., 12.),
            (20., 300., 12.),
            (18., 200., 14.),
            (14., 36., 8.),
            (14., 28., 8.),
            (14., 26., 8.),
            (14., 24., 6.),
            (12., 36., 8.),
            (9., 32., 8.),
            (8., 28., 8.),
            (8., 22., 6.),
            (7., 22., 6.),
            (6., 22., 6.),
            (4., 20., 4.),
            (3., 6., 3.),
        ] {
            assert_eq!(
                classic_corner(px(design), px(short_side)),
                px(expected),
                "design {design} on a {short_side} px side"
            );
        }
    }

    #[test]
    fn a_circular_corner_never_exceeds_half_the_short_side() {
        assert_eq!(surface_corner(px(24.), px(10.)), px(5.));
        assert_eq!(surface_corner(px(18.), px(20.)), px(10.));
        assert_eq!(surface_corner(px(9.), px(0.)), px(0.));
        assert_eq!(classic_corner(px(24.), px(10.)), px(5.));
        assert_eq!(classic_corner(px(18.), px(20.)), px(10.));
        assert_eq!(classic_corner(px(9.), px(0.)), px(0.));
    }

    #[test]
    fn every_interface_style_selects_its_own_shape() {
        assert_eq!(Shape::from(InterfaceStyle::Paneflow), Shape::Paneflow);
        assert_eq!(Shape::from(InterfaceStyle::Classic), Shape::Classic);
    }

    #[test]
    fn paneflow_menus_and_cards_take_the_surface_shape_and_classic_keeps_its_own() {
        assert_eq!(surface_shape(), Shape::Surface);
        with_surface_shape(|| {
            assert!(circular());
            assert!(!classic());
            assert_eq!(frame_corner(px(18.)), SURFACE_MENU);
            assert_eq!(frame_corner(px(20.)), SURFACE_CONTAINER);
            assert_eq!(pick(1, 2), 1);
            assert_eq!(pick_surface(1, 2, 3), 2);
        });
        assert_eq!(shape(), Shape::Paneflow);
        scoped(Shape::Classic, || {
            assert_eq!(surface_shape(), Shape::Classic);
            with_surface_shape(|| assert_eq!(shape(), Shape::Classic));
        });
    }

    #[test]
    fn a_paneflow_scope_keeps_the_continuous_corners_in_the_classic_style() {
        scoped(Shape::Classic, || {
            with_paneflow_shape(|| {
                assert!(!circular());
                assert_eq!(corner(px(13.), px(400.)), px(13.));
                assert_eq!(pick(1, 2), 1);
            });
            assert!(classic());
        });
    }

    #[test]
    fn the_shape_override_is_scoped_to_its_closure() {
        assert!(!circular());
        scoped(Shape::Classic, || {
            assert!(circular());
            assert!(classic());
            assert_eq!(corner(px(24.), px(400.)), CLASSIC_CONTAINER);
            assert_eq!(frame_corner(px(18.)), CLASSIC_MENU);
            assert_eq!(pick(1, 2), 2);
            assert_eq!(pick_surface(1, 2, 3), 3);
        });
        assert!(!circular());
        assert_eq!(pick(1, 2), 1);
        assert_eq!(pick_surface(1, 2, 3), 1);
    }
}
