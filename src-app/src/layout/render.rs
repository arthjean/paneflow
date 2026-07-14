//! `LayoutTree::render` - recursive GPUI flex rendering with drag-to-resize
//! divider handlers and per-frame container-size capture.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled, Window,
    canvas, div, px,
};

use super::tree::{
    DIVIDER_HIT_PX, DIVIDER_PX, DragState, LayoutTree, MIN_PANE_SIZE, SplitDirection,
    resize_adjacent_ratios,
};

pub(crate) type ResizeEndCallback = Rc<dyn Fn(&mut App)>;

fn finish_drag(
    drag: &Cell<Option<DragState>>,
    on_resize_end: Option<&ResizeEndCallback>,
    cx: &mut App,
) {
    if drag.take().is_some()
        && let Some(on_resize_end) = on_resize_end
    {
        on_resize_end(cx);
    }
}

fn available_main_axis_px(container_px: f32, child_count: usize) -> f32 {
    let divider_px = DIVIDER_PX * child_count.saturating_sub(1) as f32;
    (container_px - divider_px).max(0.0)
}

#[cfg(test)]
fn with_debug_selector_for_test(div: gpui::Div, selector: String) -> gpui::Div {
    div.debug_selector(move || selector.clone().into())
}

impl LayoutTree {
    /// Render the layout tree recursively as nested GPUI flex divs.
    #[allow(clippy::only_used_in_recursion)]
    pub fn render(
        &self,
        window: &Window,
        cx: &App,
        on_resize_end: Option<ResizeEndCallback>,
    ) -> AnyElement {
        match self {
            LayoutTree::Leaf(pane) => div().size_full().child(pane.clone()).into_any_element(),

            LayoutTree::Container {
                direction,
                children,
                drag,
                container_size,
            } => {
                let dir = *direction;

                // Build container with drag tracking.
                // Pre-compute per-child constraints (max yieldable pixels) so the
                // drag closure can clamp based on nested subtree minimums.
                let drag_move = drag.clone();
                let size_for_drag = container_size.clone();
                let child_ratios: Vec<Rc<Cell<f32>>> =
                    children.iter().map(|c| c.ratio.clone()).collect();
                let child_count = children.len();
                let child_minimums: Vec<f32> = children
                    .iter()
                    .map(|child| child.node.min_main_axis_px(dir))
                    .collect();
                let resize_end_for_move = on_resize_end.clone();

                let mut container = div().flex().size_full().overflow_hidden().on_mouse_move(
                    move |e, window, cx| {
                        if let Some(ds) = drag_move.get() {
                            if e.pressed_button != Some(MouseButton::Left) {
                                finish_drag(&drag_move, resize_end_for_move.as_ref(), cx);
                                window.refresh();
                                return;
                            }
                            let csize = available_main_axis_px(size_for_drag.get(), child_count);
                            if csize <= 0.0 {
                                return;
                            }
                            let current_pos = match dir {
                                SplitDirection::Horizontal => e.position.y.as_f32(),
                                SplitDirection::Vertical => e.position.x.as_f32(),
                            };
                            let delta = current_pos - ds.start_pos;

                            let min_before = child_minimums
                                .get(ds.divider_idx)
                                .copied()
                                .unwrap_or(MIN_PANE_SIZE);
                            let min_after = child_minimums
                                .get(ds.divider_idx + 1)
                                .copied()
                                .unwrap_or(MIN_PANE_SIZE);
                            let Some((new_before, new_after)) = resize_adjacent_ratios(
                                ds.start_ratio_before,
                                ds.start_ratio_after,
                                delta,
                                csize,
                                min_before,
                                min_after,
                            ) else {
                                return;
                            };

                            if let Some(r) = child_ratios.get(ds.divider_idx) {
                                r.set(new_before);
                            }
                            if let Some(r) = child_ratios.get(ds.divider_idx + 1) {
                                r.set(new_after);
                            }

                            // Request a repaint so the new ratios take effect immediately.
                            // GPUI only auto-refreshes on mouse_move when cx.has_active_drag()
                            // (i.e., GPUI-managed drags). Our Cell-based drag needs an explicit
                            // refresh to avoid waiting for the next terminal poll cycle.
                            window.refresh();
                        }
                    },
                );

                let drag_up = drag.clone();
                let resize_end_for_up = on_resize_end.clone();
                container = container
                    .on_mouse_up(MouseButton::Left, {
                        let d = drag_up.clone();
                        let on_resize_end = resize_end_for_up.clone();
                        move |_e, _window, cx| {
                            finish_drag(&d, on_resize_end.as_ref(), cx);
                        }
                    })
                    .on_mouse_up_out(MouseButton::Left, move |_e, _window, cx| {
                        finish_drag(&drag_up, resize_end_for_up.as_ref(), cx);
                    });

                container = match dir {
                    SplitDirection::Horizontal => container.flex_col(),
                    SplitDirection::Vertical => container.flex_row(),
                };

                // Capture actual container bounds each frame via canvas prepaint.
                // The canvas fills the container (absolute + size_full) so it
                // receives the parent's bounds without affecting flex layout.
                let size_capture = container_size.clone();
                let drag_cancel = drag.clone();
                let resize_end_for_cancel = on_resize_end.clone();
                container = container.child(
                    canvas(
                        move |bounds, _window, cx| {
                            let main_axis: f32 = match dir {
                                SplitDirection::Horizontal => bounds.size.height.into(),
                                SplitDirection::Vertical => bounds.size.width.into(),
                            };
                            let prev = size_capture.get();
                            size_capture.set(main_axis);
                            // Cancel drag if container was resized (window resize)
                            if prev > 0.0 && (prev - main_axis).abs() > 1.0 {
                                finish_drag(&drag_cancel, resize_end_for_cancel.as_ref(), cx);
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                );

                // Render children with dividers between adjacent pairs
                for (i, child) in children.iter().enumerate() {
                    if i > 0 {
                        // Divider between children[i-1] and children[i]
                        let drag_for_div = drag.clone();
                        let divider_idx = i - 1;
                        let ratio_before = children[divider_idx].ratio.clone();
                        let ratio_after = child.ratio.clone();

                        let divider_hit_margin = (DIVIDER_PX - DIVIDER_HIT_PX) / 2.0;
                        let divider = match dir {
                            SplitDirection::Horizontal => div()
                                .h(px(DIVIDER_HIT_PX))
                                .w_full()
                                .my(px(divider_hit_margin))
                                .flex_shrink_0()
                                .cursor_row_resize()
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .h(px(DIVIDER_PX))
                                        .w_full()
                                        .bg(crate::theme::ui_colors().border),
                                ),
                            SplitDirection::Vertical => div()
                                .w(px(DIVIDER_HIT_PX))
                                .h_full()
                                .mx(px(divider_hit_margin))
                                .flex_shrink_0()
                                .cursor_col_resize()
                                .flex()
                                .justify_center()
                                .child(
                                    div()
                                        .w(px(DIVIDER_PX))
                                        .h_full()
                                        .bg(crate::theme::ui_colors().border),
                                ),
                        };

                        #[cfg(test)]
                        let divider = with_debug_selector_for_test(
                            divider,
                            format!("layout-divider-{divider_idx}"),
                        );

                        let divider =
                            divider.on_mouse_down(MouseButton::Left, move |e, _window, _cx| {
                                let pos = match dir {
                                    SplitDirection::Horizontal => e.position.y.as_f32(),
                                    SplitDirection::Vertical => e.position.x.as_f32(),
                                };
                                drag_for_div.set(Some(DragState {
                                    divider_idx,
                                    start_pos: pos,
                                    start_ratio_before: ratio_before.get(),
                                    start_ratio_after: ratio_after.get(),
                                }));
                            });

                        container = container.child(divider);
                    }

                    let elem = child.node.render(window, cx, on_resize_end.clone());
                    let child_wrapper = div()
                        .flex_basis(gpui::relative(child.ratio.get()))
                        .flex_grow(1.0)
                        .flex_shrink(1.0)
                        .size_full()
                        .min_w(px(80.))
                        .min_h(px(80.))
                        .overflow_hidden();
                    #[cfg(test)]
                    let child_wrapper =
                        with_debug_selector_for_test(child_wrapper, format!("layout-child-{i}"));
                    container = container.child(child_wrapper.child(elem));
                }

                container.into_any_element()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{AppContext, Entity, Render, TestAppContext, px, size};

    use super::super::tree::LayoutChild;
    use crate::pane::Pane;
    use crate::terminal::TerminalView;

    use super::*;

    const TOLERANCE: f32 = 2.0;

    fn test_pane(cx: &mut impl AppContext, workspace_id: u64) -> Entity<Pane> {
        let terminal = cx.new(|cx| TerminalView::display_only_for_test(workspace_id, cx));
        cx.new(|cx| Pane::new(terminal, workspace_id, cx))
    }

    fn assert_px_eq(actual: gpui::Pixels, expected: f32, label: &str) {
        let diff = (actual.as_f32() - expected).abs();
        assert!(
            diff < TOLERANCE,
            "{label}: expected ~{expected:.1}px, got {:.1}px (diff {diff:.1}px)",
            actual.as_f32()
        );
    }

    struct RenderHarness {
        tree: LayoutTree,
    }

    impl Render for RenderHarness {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            self.tree.render(window, cx, None)
        }
    }

    #[gpui::test]
    fn render_three_child_tree_uses_production_dividers(cx: &mut TestAppContext) {
        let container_w = 900.0_f32;
        let container_h = 600.0_f32;
        let captured_container_size = Rc::new(Cell::new(0.0));
        let captured_for_tree = captured_container_size.clone();
        let (_view, cx) = cx.add_window_view(move |_, cx| {
            let panes = vec![test_pane(cx, 1), test_pane(cx, 1), test_pane(cx, 1)];
            let ratio = 1.0 / panes.len() as f32;
            let children = panes
                .into_iter()
                .map(|pane| LayoutChild {
                    node: LayoutTree::Leaf(pane),
                    ratio: Rc::new(Cell::new(ratio)),
                })
                .collect();

            RenderHarness {
                tree: LayoutTree::Container {
                    direction: SplitDirection::Vertical,
                    children,
                    drag: Rc::new(Cell::new(None)),
                    container_size: captured_for_tree,
                },
            }
        });
        cx.simulate_resize(size(px(container_w), px(container_h)));
        cx.run_until_parked();

        let b0 = cx
            .debug_bounds("layout-child-0")
            .expect("child-0 not painted");
        let b1 = cx
            .debug_bounds("layout-child-1")
            .expect("child-1 not painted");
        let b2 = cx
            .debug_bounds("layout-child-2")
            .expect("child-2 not painted");
        let divider0 = cx
            .debug_bounds("layout-divider-0")
            .expect("divider-0 not painted");
        let divider1 = cx
            .debug_bounds("layout-divider-1")
            .expect("divider-1 not painted");

        assert!(b0.size.width > px(80.), "child-0 below visible minimum");
        assert!(b1.size.width > px(80.), "child-1 below visible minimum");
        assert!(b2.size.width > px(80.), "child-2 below visible minimum");
        assert_px_eq(divider0.size.width, DIVIDER_HIT_PX, "divider-0 hit width");
        assert_px_eq(divider1.size.width, DIVIDER_HIT_PX, "divider-1 hit width");
        assert_px_eq(
            b0.size.width + b1.size.width + b2.size.width,
            container_w - 2.0 * DIVIDER_PX,
            "total pane width",
        );
        assert_px_eq(
            px(captured_container_size.get()),
            container_w,
            "captured main-axis container size",
        );
    }

    #[test]
    fn available_main_axis_excludes_fixed_dividers() {
        assert!((available_main_axis_px(500.0, 3) - 492.0).abs() < f32::EPSILON);
    }

    #[test]
    fn available_main_axis_never_goes_negative() {
        assert_eq!(available_main_axis_px(4.0, 3), 0.0);
    }
}
