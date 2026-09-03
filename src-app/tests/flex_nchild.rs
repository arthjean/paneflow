#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unwrap_in_result,
    clippy::panic
)]

use gpui::{
    AvailableSpace, InteractiveElement, ParentElement, Styled, TestAppContext, div, point, px,
    relative, size,
};

const TOLERANCE: f32 = 2.0;

fn assert_px_eq(actual: gpui::Pixels, expected: f32, label: &str) {
    let diff = (actual.as_f32() - expected).abs();
    assert!(
        diff < TOLERANCE,
        "{label}: expected ~{expected:.1}px, got {:.1}px (diff {diff:.1}px)",
        actual.as_f32()
    );
}

#[gpui::test]
fn test_three_children_flex_basis(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();

    let container_w = 900.0_f32;
    let container_h = 600.0_f32;

    cx.draw(
        point(px(0.), px(0.)),
        size(
            AvailableSpace::Definite(px(container_w)),
            AvailableSpace::Definite(px(container_h)),
        ),
        |_, _| {
            div()
                .flex()
                .flex_row()
                .w(px(container_w))
                .h(px(container_h))
                .child(
                    div()
                        .flex_basis(relative(0.33))
                        .flex_grow(1.0)
                        .flex_shrink(1.0)
                        .h_full()
                        .debug_selector(|| "c3-child-0".into()),
                )
                .child(
                    div()
                        .flex_basis(relative(0.33))
                        .flex_grow(1.0)
                        .flex_shrink(1.0)
                        .h_full()
                        .debug_selector(|| "c3-child-1".into()),
                )
                .child(
                    div()
                        .flex_basis(relative(0.34))
                        .flex_grow(1.0)
                        .flex_shrink(1.0)
                        .h_full()
                        .debug_selector(|| "c3-child-2".into()),
                )
        },
    );

    let b0 = cx.debug_bounds("c3-child-0").expect("child-0 not painted");
    let b1 = cx.debug_bounds("c3-child-1").expect("child-1 not painted");
    let b2 = cx.debug_bounds("c3-child-2").expect("child-2 not painted");

    assert!(b0.size.width > px(0.), "child-0 has zero width");
    assert!(b1.size.width > px(0.), "child-1 has zero width");
    assert!(b2.size.width > px(0.), "child-2 has zero width");

    let total = b0.size.width + b1.size.width + b2.size.width;
    assert_px_eq(total, container_w, "total width");

    assert_px_eq(b0.size.width, container_w * 0.33, "child-0 width");
    assert_px_eq(b1.size.width, container_w * 0.33, "child-1 width");
    assert_px_eq(b2.size.width, container_w * 0.34, "child-2 width");

    assert!(b1.origin.x >= b0.origin.x + b0.size.width - px(TOLERANCE));
    assert!(b2.origin.x >= b1.origin.x + b1.size.width - px(TOLERANCE));
}

#[gpui::test]
fn test_five_children_equal(cx: &mut TestAppContext) {
    const SELECTORS: [&str; 5] = [
        "c5-child-0",
        "c5-child-1",
        "c5-child-2",
        "c5-child-3",
        "c5-child-4",
    ];

    let cx = cx.add_empty_window();

    let container_w = 1000.0_f32;
    let container_h = 600.0_f32;

    cx.draw(
        point(px(0.), px(0.)),
        size(
            AvailableSpace::Definite(px(container_w)),
            AvailableSpace::Definite(px(container_h)),
        ),
        |_, _| {
            let mut container = div()
                .flex()
                .flex_row()
                .w(px(container_w))
                .h(px(container_h));

            for sel in SELECTORS {
                container = container.child(
                    div()
                        .flex_basis(relative(0.2))
                        .flex_grow(1.0)
                        .flex_shrink(1.0)
                        .h_full()
                        .debug_selector(|| sel.into()),
                );
            }
            container
        },
    );

    let expected_w = container_w / 5.0;

    for sel in SELECTORS {
        let bounds = cx
            .debug_bounds(sel)
            .unwrap_or_else(|| panic!("{sel} not painted"));
        assert_px_eq(bounds.size.width, expected_w, sel);
    }
}

#[gpui::test]
fn test_imprecise_sum(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();

    let container_w = 900.0_f32;
    let container_h = 600.0_f32;

    cx.draw(
        point(px(0.), px(0.)),
        size(
            AvailableSpace::Definite(px(container_w)),
            AvailableSpace::Definite(px(container_h)),
        ),
        |_, _| {
            div()
                .flex()
                .flex_row()
                .w(px(container_w))
                .h(px(container_h))
                .child(
                    div()
                        .flex_basis(relative(0.33))
                        .flex_grow(1.0)
                        .flex_shrink(1.0)
                        .h_full()
                        .debug_selector(|| "imp-child-0".into()),
                )
                .child(
                    div()
                        .flex_basis(relative(0.33))
                        .flex_grow(1.0)
                        .flex_shrink(1.0)
                        .h_full()
                        .debug_selector(|| "imp-child-1".into()),
                )
                .child(
                    div()
                        .flex_basis(relative(0.33))
                        .flex_grow(1.0)
                        .flex_shrink(1.0)
                        .h_full()
                        .debug_selector(|| "imp-child-2".into()),
                )
        },
    );

    let b0 = cx.debug_bounds("imp-child-0").expect("child-0 not painted");
    let b1 = cx.debug_bounds("imp-child-1").expect("child-1 not painted");
    let b2 = cx.debug_bounds("imp-child-2").expect("child-2 not painted");

    let total = b0.size.width + b1.size.width + b2.size.width;
    assert_px_eq(total, container_w, "total width (imprecise sum)");

    let rightmost = b2.origin.x + b2.size.width;
    assert!(
        rightmost <= px(container_w) + px(TOLERANCE),
        "rightmost edge {rightmost:?} exceeds container {container_w}px"
    );

    let expected_each = container_w / 3.0;
    let grow_tolerance = 4.0_f32;
    for (bounds, label) in [
        (b0, "child-0 (imprecise)"),
        (b1, "child-1 (imprecise)"),
        (b2, "child-2 (imprecise)"),
    ] {
        let diff = (bounds.size.width.as_f32() - expected_each).abs();
        assert!(
            diff < grow_tolerance,
            "{label}: expected ~{expected_each:.1}px, got {:.1}px (diff {diff:.1}px)",
            bounds.size.width.as_f32()
        );
    }
}

const DIVIDER_PX: f32 = 4.0;

fn pane_div(ratio: f32, selector: &'static str) -> gpui::Div {
    div()
        .flex_basis(relative(ratio))
        .flex_grow(1.0)
        .flex_shrink(1.0)
        .size_full()
        .min_w(px(80.))
        .min_h(px(80.))
        .overflow_hidden()
        .debug_selector(|| selector.into())
}

fn v_divider() -> gpui::Div {
    div().w(px(DIVIDER_PX)).h_full().flex_shrink_0()
}

fn h_divider() -> gpui::Div {
    div().h(px(DIVIDER_PX)).w_full().flex_shrink_0()
}

#[gpui::test]
fn test_three_children_with_dividers(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();

    let container_w = 900.0_f32;
    let container_h = 600.0_f32;

    cx.draw(
        point(px(0.), px(0.)),
        size(
            AvailableSpace::Definite(px(container_w)),
            AvailableSpace::Definite(px(container_h)),
        ),
        |_, _| {
            div()
                .flex()
                .flex_row()
                .size_full()
                .overflow_hidden()
                .child(pane_div(0.33, "render-c0"))
                .child(v_divider())
                .child(pane_div(0.33, "render-c1"))
                .child(v_divider())
                .child(pane_div(0.34, "render-c2"))
        },
    );

    let b0 = cx.debug_bounds("render-c0").expect("child-0 not painted");
    let b1 = cx.debug_bounds("render-c1").expect("child-1 not painted");
    let b2 = cx.debug_bounds("render-c2").expect("child-2 not painted");

    assert!(b0.size.width > px(0.), "child-0 has zero width");
    assert!(b1.size.width > px(0.), "child-1 has zero width");
    assert!(b2.size.width > px(0.), "child-2 has zero width");

    let available = container_w - 2.0 * DIVIDER_PX;
    let total_pane_width = b0.size.width + b1.size.width + b2.size.width;
    assert_px_eq(
        total_pane_width,
        available,
        "total pane width (with dividers)",
    );

    assert_px_eq(b0.size.width, available * 0.33, "pane-0 width");
    assert_px_eq(b1.size.width, available * 0.33, "pane-1 width");
    assert_px_eq(b2.size.width, available * 0.34, "pane-2 width");

    assert!(b0.size.width >= px(80.), "pane-0 below 80px minimum");
    assert!(b1.size.width >= px(80.), "pane-1 below 80px minimum");
    assert!(b2.size.width >= px(80.), "pane-2 below 80px minimum");
}

#[gpui::test]
fn test_deeply_nested_four_levels(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();

    let container_w = 1200.0_f32;
    let container_h = 800.0_f32;

    cx.draw(
        point(px(0.), px(0.)),
        size(
            AvailableSpace::Definite(px(container_w)),
            AvailableSpace::Definite(px(container_h)),
        ),
        |_, _| {
            div()
                .flex()
                .flex_row()
                .size_full()
                .overflow_hidden()
                .child(
                    div()
                        .flex_basis(relative(0.6))
                        .flex_grow(1.0)
                        .flex_shrink(1.0)
                        .size_full()
                        .min_w(px(80.))
                        .min_h(px(80.))
                        .overflow_hidden()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .size_full()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .flex_basis(relative(0.5))
                                        .flex_grow(1.0)
                                        .flex_shrink(1.0)
                                        .size_full()
                                        .min_w(px(80.))
                                        .min_h(px(80.))
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .size_full()
                                                .overflow_hidden()
                                                .child(pane_div(0.5, "deep-A"))
                                                .child(v_divider())
                                                .child(pane_div(0.5, "deep-B")),
                                        ),
                                )
                                .child(h_divider())
                                .child(pane_div(0.5, "deep-C")),
                        ),
                )
                .child(v_divider())
                .child(pane_div(0.4, "deep-D"))
        },
    );

    let a = cx.debug_bounds("deep-A").expect("leaf A not painted");
    let b = cx.debug_bounds("deep-B").expect("leaf B not painted");
    let c = cx.debug_bounds("deep-C").expect("leaf C not painted");
    let d = cx.debug_bounds("deep-D").expect("leaf D not painted");

    assert!(
        a.size.width > px(0.) && a.size.height > px(0.),
        "A invisible"
    );
    assert!(
        b.size.width > px(0.) && b.size.height > px(0.),
        "B invisible"
    );
    assert!(
        c.size.width > px(0.) && c.size.height > px(0.),
        "C invisible"
    );
    assert!(
        d.size.width > px(0.) && d.size.height > px(0.),
        "D invisible"
    );

    for (bounds, label) in [(&a, "A"), (&b, "B"), (&c, "C"), (&d, "D")] {
        assert!(
            bounds.size.width >= px(80.),
            "leaf {label} width {:.0}px < 80px",
            bounds.size.width.as_f32()
        );
        assert!(
            bounds.size.height >= px(80.),
            "leaf {label} height {:.0}px < 80px",
            bounds.size.height.as_f32()
        );
    }

    assert!(d.origin.x > a.origin.x, "D should be to the right of A");
    let d_ratio = d.size.width.as_f32() / container_w;
    assert!(
        (d_ratio - 0.4).abs() < 0.05,
        "D width ratio {d_ratio:.2} should be ~0.4"
    );
    assert_px_eq(a.origin.y, b.origin.y.as_f32(), "A and B same y-origin");
    assert!(c.origin.y > a.origin.y, "C should be below A");
}
