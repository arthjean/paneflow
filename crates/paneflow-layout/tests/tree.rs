#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::Fixture;
use paneflow_layout::{Direction, Op, OpError, PaneId, Preset};

fn three_equal() -> Fixture {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    fx.split("b", Direction::Vertical, "c");
    let tab = fx.tab_id(0, 0);
    fx.apply(Op::ApplyPreset {
        tab,
        preset: Preset::EvenHorizontal,
    });
    assert_eq!(fx.shape(0, 0), "V(0.333 a, 0.333 b, 0.333 c)");
    fx
}

#[test]
fn removing_a_pane_hands_its_share_to_the_remaining_siblings() {
    let mut fx = three_equal();
    fx.apply(Op::ClosePane {
        pane: fx.pane_id("b"),
        keep_undo: false,
    });
    assert_eq!(fx.shape(0, 0), "V(0.500 a, 0.500 c)");
}

#[test]
fn removing_one_of_two_panes_collapses_the_split_into_a_leaf() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Horizontal, "b");
    fx.apply(Op::ClosePane {
        pane: fx.pane_id("b"),
        keep_undo: false,
    });
    assert_eq!(fx.shape(0, 0), "a");
}

#[test]
fn removing_an_absent_pane_is_refused_and_keeps_the_layout() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Horizontal, "b");
    let revision = fx.model.revision();
    assert_eq!(
        fx.try_apply(Op::ClosePane {
            pane: PaneId(777_777),
            keep_undo: false,
        })
        .err(),
        Some(OpError::NotFound)
    );
    assert_eq!(fx.shape(0, 0), "H(0.500 a, 0.500 b)");
    assert_eq!(fx.model.revision(), revision);
}

#[test]
fn swapping_with_an_absent_source_is_refused_and_keeps_the_order() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    assert_eq!(
        fx.try_apply(Op::SwapPanes {
            a: PaneId(777_777),
            b: fx.pane_id("b"),
        })
        .err(),
        Some(OpError::NotFound)
    );
    assert_eq!(fx.shape(0, 0), "V(0.500 a, 0.500 b)");
}

#[test]
fn splitting_across_the_parent_direction_wraps_the_target_in_a_new_split() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    fx.split("a", Direction::Horizontal, "c");
    assert_eq!(fx.shape(0, 0), "V(0.500 H(0.500 a, 0.500 c), 0.500 b)");
}
