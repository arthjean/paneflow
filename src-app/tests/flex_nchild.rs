// Integration test - compiled as its own crate and does not inherit
// `src-app/src/main.rs`'s `#![cfg_attr(test, allow(...))]`. Per CLAUDE.md
// (clippy issue #13981 workaround), declare the same test-allow set
// locally so `cargo clippy --all-targets -- -D warnings` passes under
// US-007's strict workspace lints.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unwrap_in_result,
    clippy::panic
)]

//! Layout engine flexbox validation tests.
//!
//! US-001: Proves GPUI's Taffy-backed flexbox distributes space correctly
//! across N > 2 children using `flex_basis(relative(ratio))`.
//!
//! US-003: Validates the GPUI flexbox proxy used by `LayoutTree::render()`,
//! including divider sizing and deeply nested trees. The production render
//! method has module-level tests in `src-app/src/layout/render.rs`.

use gpui::{
    AvailableSpace, InteractiveElement, ParentElement, Styled, TestAppContext, div, point, px,
    relative, size,
};

const TOLERANCE: f32 = 2.0;

/// Helper: assert a pixel value is within tolerance of expected.
fn assert_px_eq(actual: gpui::Pixels, expected: f32, label: &str) {
    let diff = (actual.as_f32() - expected).abs();
    assert!(
        diff < TOLERANCE,
        "{label}: expected ~{expected:.1}px, got {:.1}px (diff {diff:.1}px)",
        actual.as_f32()
    );
}

/// AC-1: A flex_row container with 3 children using ratios [0.33, 0.33, 0.34]
/// renders all three children with proportional widths.
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
                        .flex_grow()
                        .flex_shrink()
                        .h_full()
                        .debug_selector(|| "c3-child-0".into()),
                )
                .child(
                    div()
                        .flex_basis(relative(0.33))
                        .flex_grow()
                        .flex_shrink()
                        .h_full()
                        .debug_selector(|| "c3-child-1".into()),
                )
                .child(
                    div()
                        .flex_basis(relative(0.34))
                        .flex_grow()
                        .flex_shrink()
                        .h_full()
                        .debug_selector(|| "c3-child-2".into()),
                )
        },
    );

    let b0 = cx.debug_bounds("c3-child-0").expect("child-0 not painted");
    let b1 = cx.debug_bounds("c3-child-1").expect("child-1 not painted");
    let b2 = cx.debug_bounds("c3-child-2").expect("child-2 not painted");

    // All three children should be visible
    assert!(b0.size.width > px(0.), "child-0 has zero width");
    assert!(b1.size.width > px(0.), "child-1 has zero width");
    assert!(b2.size.width > px(0.), "child-2 has zero width");

    // Children should fill the container
    let total = b0.size.width + b1.size.width + b2.size.width;
    assert_px_eq(total, container_w, "total width");

    // Each child should be proportional to its ratio
    // With ratios summing to 1.0 and flex_grow=1, Taffy distributes exactly by basis
    assert_px_eq(b0.size.width, container_w * 0.33, "child-0 width");
    assert_px_eq(b1.size.width, container_w * 0.33, "child-1 width");
    assert_px_eq(b2.size.width, container_w * 0.34, "child-2 width");

    // Children should be laid out left-to-right without overlap
    assert!(b1.origin.x >= b0.origin.x + b0.size.width - px(TOLERANCE));
    assert!(b2.origin.x >= b1.origin.x + b1.size.width - px(TOLERANCE));
}

/// AC-2: A flex_row container with 5 children, each ratio = 0.2, renders
/// all five children with equal widths.
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
                        .flex_grow()
                        .flex_shrink()
                        .h_full()
                        .debug_selector(|| sel.into()),
                );
            }
            container
        },
    );

    let expected_w = container_w / 5.0; // 200px each

    for sel in SELECTORS {
        let bounds = cx
            .debug_bounds(sel)
            .unwrap_or_else(|| panic!("{sel} not painted"));
        assert_px_eq(bounds.size.width, expected_w, sel);
    }
}

/// AC-3: Ratios that don't sum to exactly 1.0 (0.33+0.33+0.33=0.99) must not
/// leave a visible gap or overflow. flex_grow absorbs the remainder.
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
                        .flex_grow()
                        .flex_shrink()
                        .h_full()
                        .debug_selector(|| "imp-child-0".into()),
                )
                .child(
                    div()
                        .flex_basis(relative(0.33))
                        .flex_grow()
                        .flex_shrink()
                        .h_full()
                        .debug_selector(|| "imp-child-1".into()),
                )
                .child(
                    div()
                        .flex_basis(relative(0.33))
                        .flex_grow()
                        .flex_shrink()
                        .h_full()
                        .debug_selector(|| "imp-child-2".into()),
                )
        },
    );

    let b0 = cx.debug_bounds("imp-child-0").expect("child-0 not painted");
    let b1 = cx.debug_bounds("imp-child-1").expect("child-1 not painted");
    let b2 = cx.debug_bounds("imp-child-2").expect("child-2 not painted");

    // No gap: children should fill the entire container width
    let total = b0.size.width + b1.size.width + b2.size.width;
    assert_px_eq(total, container_w, "total width (imprecise sum)");

    // No overflow: no child extends beyond the container
    let rightmost = b2.origin.x + b2.size.width;
    assert!(
        rightmost <= px(container_w) + px(TOLERANCE),
        "rightmost edge {rightmost:?} exceeds container {container_w}px"
    );

    // All children should be approximately equal.
    // flex_grow distributes the 1% remainder (~3px each), so allow slightly wider tolerance.
    let expected_each = container_w / 3.0; // ~300px
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

// ===========================================================================
// US-003: Render N-ary tree via GPUI flexbox
//
// These tests replicate the GPUI layout shape used by LayoutTree::render(),
// including dividers between children and deeply nested containers.
// ===========================================================================

const DIVIDER_PX: f32 = 4.0;

/// Helper: build a child pane div matching the render wrapper's layout inputs.
fn pane_div(ratio: f32, selector: &'static str) -> gpui::Div {
    div()
        .flex_basis(relative(ratio))
        .flex_grow()
        .flex_shrink()
        .size_full()
        .min_w(px(80.))
        .min_h(px(80.))
        .overflow_hidden()
        .debug_selector(|| selector.into())
}

/// Helper: build a vertical divider with the same visible width as render.rs.
fn v_divider() -> gpui::Div {
    div().w(px(DIVIDER_PX)).h_full().flex_shrink_0()
}

/// Helper: build a horizontal divider with the same visible width as render.rs.
fn h_divider() -> gpui::Div {
    div().h(px(DIVIDER_PX)).w_full().flex_shrink_0()
}

/// AC-4: A 3-child container with ratios [0.33, 0.33, 0.34] and 4px dividers
/// between each pair - all three panes visible with roughly equal size.
/// This replicates the render.rs flex shape with test-only debug selectors.
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
            // flex_row = SplitDirection::Vertical (panes side by side)
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

    // All three panes must be visible (non-zero width)
    assert!(b0.size.width > px(0.), "child-0 has zero width");
    assert!(b1.size.width > px(0.), "child-1 has zero width");
    assert!(b2.size.width > px(0.), "child-2 has zero width");

    // Available space = container - 2 dividers = 900 - 8 = 892px
    let available = container_w - 2.0 * DIVIDER_PX;
    let total_pane_width = b0.size.width + b1.size.width + b2.size.width;
    assert_px_eq(
        total_pane_width,
        available,
        "total pane width (with dividers)",
    );

    // Each pane should be roughly proportional
    assert_px_eq(b0.size.width, available * 0.33, "pane-0 width");
    assert_px_eq(b1.size.width, available * 0.33, "pane-1 width");
    assert_px_eq(b2.size.width, available * 0.34, "pane-2 width");

    // No pane below 80px minimum
    assert!(b0.size.width >= px(80.), "pane-0 below 80px minimum");
    assert!(b1.size.width >= px(80.), "pane-1 below 80px minimum");
    assert!(b2.size.width >= px(80.), "pane-2 below 80px minimum");
}

/// AC-5: A 4-level deep nested tree renders without stack overflow or visual glitch.
/// Structure: Vertical[Horizontal[Vertical[A, B], C], D]
/// This matches the layout shape used for a deeply nested split tree.
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
            // Level 1: Vertical split (flex_row) - left subtree + D
            div()
                .flex()
                .flex_row()
                .size_full()
                .overflow_hidden()
                .child(
                    // Level 2: Horizontal split (flex_col) - top subtree + C
                    div()
                        .flex_basis(relative(0.6))
                        .flex_grow()
                        .flex_shrink()
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
                                    // Level 3: Vertical split (flex_row) - A + B
                                    div()
                                        .flex_basis(relative(0.5))
                                        .flex_grow()
                                        .flex_shrink()
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
                                                // Level 4: Leaves A and B
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

    // All 4 leaves must be visible
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

    // No pane below 80px in either dimension
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

    // D should be on the right side of the container
    assert!(d.origin.x > a.origin.x, "D should be to the right of A");
    // D should occupy roughly 40% of the total width
    let d_ratio = d.size.width.as_f32() / container_w;
    assert!(
        (d_ratio - 0.4).abs() < 0.05,
        "D width ratio {d_ratio:.2} should be ~0.4"
    );
    // A and B should be side by side (same row)
    assert_px_eq(a.origin.y, b.origin.y.as_f32(), "A and B same y-origin");
    // C should be below A/B
    assert!(c.origin.y > a.origin.y, "C should be below A");
}
