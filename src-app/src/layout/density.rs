use gpui::{Corners, Div, Edges, Hsla, Pixels, Styled, div, px};
use paneflow_config::schema::PaneFlowConfig;

use super::tree::{DIVIDER_PX, SplitDirection};

pub(crate) const COMPACT_DIVIDER_PX: f32 = 1.0;
pub(crate) const COMPACT_DIVIDER_HIT_PX: f32 = 7.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PaneDensity {
    comfort: f32,
}

impl PaneDensity {
    pub(crate) const COMFORTABLE: Self = Self { comfort: 1.0 };
    pub(crate) const COMPACT: Self = Self { comfort: 0.0 };

    pub(crate) fn from_config(config: &PaneFlowConfig) -> Self {
        if config.comfortable_density_enabled() {
            Self::COMFORTABLE
        } else {
            Self::COMPACT
        }
    }

    pub(crate) fn blended(comfort: f32) -> Self {
        Self {
            comfort: comfort.clamp(0.0, 1.0),
        }
    }

    pub(crate) fn comfort(self) -> f32 {
        self.comfort
    }

    pub(crate) fn is_compact(self) -> bool {
        self.comfort < 0.5
    }

    pub(crate) fn divider_px(self) -> f32 {
        COMPACT_DIVIDER_PX + (DIVIDER_PX - COMPACT_DIVIDER_PX) * self.comfort
    }

    pub(crate) fn divider_line_alpha(self) -> f32 {
        (1.0 - 2.0 * self.comfort).clamp(0.0, 1.0)
    }

    pub(crate) fn outer_gutter_px(self) -> f32 {
        crate::app::constants::PANE_OUTER_GUTTER * self.comfort
    }

    pub(crate) fn frame(self, outer: Edges<bool>) -> CardFrame {
        if self.comfort >= 1.0 {
            CardFrame::Card
        } else if self.comfort <= 0.0 {
            CardFrame::Flush(outer)
        } else {
            CardFrame::Morph {
                outer,
                comfort: self.comfort,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CardFrame {
    Card,
    Flush(Edges<bool>),
    Morph { outer: Edges<bool>, comfort: f32 },
}

pub(crate) fn morph_corners(outer: Edges<bool>, comfort: f32) -> Corners<Pixels> {
    let card = crate::app::constants::PANE_CARD_RADIUS;
    let flush = crate::app::constants::PANEL_CORNER_RADIUS;
    let corner = |a: bool, b: bool| {
        let from = if a && b { flush } else { px(0.) };
        from + (card - from) * comfort
    };
    Corners {
        top_left: corner(outer.top, outer.left),
        top_right: corner(outer.top, outer.right),
        bottom_right: corner(outer.bottom, outer.right),
        bottom_left: corner(outer.bottom, outer.left),
    }
}

pub(crate) fn grid_edges(right_is_outer: bool) -> Edges<bool> {
    Edges {
        top: true,
        right: right_is_outer,
        bottom: true,
        left: true,
    }
}

pub(crate) fn child_edges(
    parent: Edges<bool>,
    direction: SplitDirection,
    first: bool,
    last: bool,
) -> Edges<bool> {
    match direction {
        SplitDirection::Vertical => Edges {
            left: parent.left && first,
            right: parent.right && last,
            ..parent
        },
        SplitDirection::Horizontal => Edges {
            top: parent.top && first,
            bottom: parent.bottom && last,
            ..parent
        },
    }
}

pub(crate) fn flush_corners(div: Div, outer: Edges<bool>) -> Div {
    let radius = crate::app::constants::PANEL_CORNER_RADIUS;
    let corner = |a: bool, b: bool| if a && b { radius } else { px(0.) };
    div.rounded_tl(corner(outer.top, outer.left))
        .rounded_tr(corner(outer.top, outer.right))
        .rounded_br(corner(outer.bottom, outer.right))
        .rounded_bl(corner(outer.bottom, outer.left))
}

pub(crate) fn flush_fill(outer: Edges<bool>, color: Hsla) -> Div {
    flush_corners(div().absolute().inset_0(), outer).bg(color)
}

pub(crate) fn flush_border(outer: Edges<bool>, color: Hsla) -> Div {
    let width = |outer: bool| if outer { px(1.) } else { px(0.) };
    flush_corners(div().absolute().inset_0(), outer)
        .border_t(width(outer.top))
        .border_r(width(outer.right))
        .border_b(width(outer.bottom))
        .border_l(width(outer.left))
        .border_color(color)
}

pub(crate) fn flush_outline(outer: Edges<bool>, color: Hsla) -> Div {
    flush_corners(div().absolute().inset_0(), outer)
        .border_1()
        .border_color(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Edges<bool> {
        grid_edges(true)
    }

    #[test]
    fn a_density_halfway_through_its_transition_blends_every_metric() {
        let density = PaneDensity::blended(0.5);
        assert_eq!(
            density.divider_px(),
            (DIVIDER_PX + COMPACT_DIVIDER_PX) / 2.0
        );
        assert_eq!(
            density.outer_gutter_px(),
            crate::app::constants::PANE_OUTER_GUTTER / 2.0
        );
        assert_eq!(density.divider_line_alpha(), 0.0);
        assert_eq!(
            density.frame(all()),
            CardFrame::Morph {
                outer: all(),
                comfort: 0.5
            }
        );
    }

    #[test]
    fn a_morphing_card_starts_at_the_flush_corners_and_ends_at_the_card_radius() {
        let inner_right = Edges {
            right: false,
            ..all()
        };
        let flush = morph_corners(inner_right, 0.0);
        assert_eq!(flush.top_left, crate::app::constants::PANEL_CORNER_RADIUS);
        assert_eq!(flush.top_right, px(0.));
        let card = morph_corners(inner_right, 1.0);
        assert_eq!(card.top_right, crate::app::constants::PANE_CARD_RADIUS);
        assert_eq!(card.top_left, crate::app::constants::PANE_CARD_RADIUS);
    }

    #[test]
    fn comfortable_density_keeps_the_card_gutters() {
        let density = PaneDensity::COMFORTABLE;
        assert_eq!(density.divider_px(), DIVIDER_PX);
        assert_eq!(
            density.outer_gutter_px(),
            crate::app::constants::PANE_OUTER_GUTTER
        );
        assert_eq!(density.frame(all()), CardFrame::Card);
    }

    #[test]
    fn compact_density_joins_panes_with_a_hairline() {
        let density = PaneDensity::COMPACT;
        assert_eq!(density.divider_px(), 1.0);
        assert_eq!(density.outer_gutter_px(), 0.0);
        assert_eq!(density.frame(all()), CardFrame::Flush(all()));
    }

    #[test]
    fn density_follows_the_config_and_defaults_to_comfortable() {
        let mut config = PaneFlowConfig::default();
        assert_eq!(PaneDensity::from_config(&config), PaneDensity::COMFORTABLE);
        config.comfortable_density = Some(false);
        assert_eq!(PaneDensity::from_config(&config), PaneDensity::COMPACT);
    }

    #[test]
    fn a_row_split_hands_the_left_edge_to_the_first_child_and_the_right_to_the_last() {
        let first = child_edges(all(), SplitDirection::Vertical, true, false);
        let middle = child_edges(all(), SplitDirection::Vertical, false, false);
        let last = child_edges(all(), SplitDirection::Vertical, false, true);
        assert!(first.left && !first.right && first.top && first.bottom);
        assert!(!middle.left && !middle.right && middle.top && middle.bottom);
        assert!(!last.left && last.right && last.top && last.bottom);
    }

    #[test]
    fn a_column_split_hands_the_top_edge_to_the_first_child_and_the_bottom_to_the_last() {
        let first = child_edges(all(), SplitDirection::Horizontal, true, false);
        let last = child_edges(all(), SplitDirection::Horizontal, false, true);
        assert!(first.top && !first.bottom && first.left && first.right);
        assert!(!last.top && last.bottom && last.left && last.right);
    }

    #[test]
    fn an_inner_edge_never_becomes_outer_again_deeper_in_the_tree() {
        let right_column = child_edges(all(), SplitDirection::Vertical, false, true);
        let bottom_right = child_edges(right_column, SplitDirection::Horizontal, false, true);
        assert_eq!(
            bottom_right,
            Edges {
                top: false,
                right: true,
                bottom: true,
                left: false,
            }
        );
    }

    #[test]
    fn a_grid_beside_the_dock_has_no_outer_right_edge() {
        let edges = grid_edges(false);
        assert!(edges.top && edges.bottom && edges.left && !edges.right);
    }
}
