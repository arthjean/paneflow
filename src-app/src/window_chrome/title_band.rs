use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TitleBand {
    pub(crate) height: f32,
    pub(crate) leading_inset: f32,
    pub(crate) trailing_inset: f32,
    pub(crate) window_corners: gpui::Corners<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OpenWindowEdges {
    pub(crate) top: bool,
    pub(crate) left: bool,
    pub(crate) right: bool,
}

pub(crate) fn window_corners_under_the_panel(
    rounded: gpui::Corners<bool>,
    open: OpenWindowEdges,
) -> gpui::Corners<bool> {
    gpui::Corners {
        top_left: rounded.top_left && open.top && open.left,
        top_right: rounded.top_right && open.top && open.right,
        bottom_right: rounded.bottom_right && open.right,
        bottom_left: rounded.bottom_left && open.left,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeaderSlot {
    pub(crate) height: f32,
    pub(crate) leading: f32,
    pub(crate) trailing: f32,
    pub(crate) in_band: bool,
}

impl TitleBand {
    pub(crate) fn header_slot(self, outer: gpui::Edges<bool>) -> HeaderSlot {
        HeaderSlot {
            height: self.height,
            leading: if outer.top && outer.left {
                self.leading_inset
            } else {
                0.
            },
            trailing: if outer.top && outer.right {
                self.trailing_inset
            } else {
                0.
            },
            in_band: outer.top,
        }
    }
}

thread_local! {
    static ACTIVE: Cell<Option<TitleBand>> = const { Cell::new(None) };
}

pub(crate) fn set(band: Option<TitleBand>) -> bool {
    ACTIVE.replace(band) != band
}

pub(crate) fn active() -> Option<TitleBand> {
    ACTIVE.get()
}

pub(crate) fn leading_inset(reserved_until: f32, content_left: f32) -> f32 {
    if reserved_until <= 0. {
        return 0.;
    }
    (reserved_until + CONTROL_GAP - content_left).max(0.)
}

pub(crate) fn trailing_inset(reserved_from: f32, content_right: f32) -> f32 {
    if reserved_from <= 0. {
        return 0.;
    }
    (content_right - reserved_from + CONTROL_GAP).max(0.)
}

const CONTROL_GAP: f32 = 8.;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_clear_of_the_window_controls_reserves_nothing() {
        assert_eq!(leading_inset(120., 280.), 0.);
        assert_eq!(trailing_inset(1700., 1300.), 0.);
    }

    #[test]
    fn content_under_the_window_controls_reserves_their_width_and_a_gap() {
        assert_eq!(leading_inset(120., 0.), 128.);
        assert_eq!(trailing_inset(1700., 1849.), 157.);
    }

    #[test]
    fn only_a_header_in_the_top_row_makes_room_for_the_window_controls() {
        let band = TitleBand {
            height: 40.,
            leading_inset: 128.,
            trailing_inset: 157.,
            window_corners: gpui::Corners::default(),
        };
        let top_left = band.header_slot(gpui::Edges {
            top: true,
            right: false,
            bottom: false,
            left: true,
        });
        assert_eq!(
            top_left,
            HeaderSlot {
                height: 40.,
                leading: 128.,
                trailing: 0.,
                in_band: true
            }
        );
        let bottom_right = band.header_slot(gpui::Edges {
            top: false,
            right: true,
            bottom: true,
            left: false,
        });
        assert_eq!(
            bottom_right,
            HeaderSlot {
                height: 40.,
                leading: 0.,
                trailing: 0.,
                in_band: false
            }
        );
    }

    #[test]
    fn unmeasured_controls_reserve_nothing() {
        assert_eq!(leading_inset(0., 0.), 0.);
        assert_eq!(trailing_inset(0., 1849.), 0.);
    }

    const ROUNDED: gpui::Corners<bool> = gpui::Corners {
        top_left: true,
        top_right: true,
        bottom_right: true,
        bottom_left: true,
    };

    #[test]
    fn the_panel_reaches_the_right_window_corners_beside_a_primary_sidebar() {
        let corners = window_corners_under_the_panel(
            ROUNDED,
            OpenWindowEdges {
                top: true,
                left: false,
                right: true,
            },
        );
        assert!(corners.top_right && corners.bottom_right);
        assert!(!corners.top_left && !corners.bottom_left);
    }

    #[test]
    fn a_side_rail_or_a_title_bar_row_keeps_the_panel_off_those_window_corners() {
        let corners = window_corners_under_the_panel(
            ROUNDED,
            OpenWindowEdges {
                top: false,
                left: true,
                right: false,
            },
        );
        assert!(!corners.top_left && !corners.top_right && !corners.bottom_right);
        assert!(corners.bottom_left);
    }

    #[test]
    fn a_square_window_corner_is_never_reached() {
        let tiled_right = gpui::Corners {
            top_right: false,
            bottom_right: false,
            ..ROUNDED
        };
        let corners = window_corners_under_the_panel(
            tiled_right,
            OpenWindowEdges {
                top: true,
                left: true,
                right: true,
            },
        );
        assert!(corners.top_left && corners.bottom_left);
        assert!(!corners.top_right && !corners.bottom_right);
    }
}
