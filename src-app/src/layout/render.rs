//! `LayoutTree::render` - recursive GPUI flex rendering with drag-to-resize
//! divider handlers and per-frame container-size capture.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Entity, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled,
    Window, canvas, div, px, relative,
};

use super::tree::{
    DIVIDER_HIT_PX, DIVIDER_PX, DragState, LayoutTree, MIN_PANE_SIZE, SplitDirection,
    resize_adjacent_ratios,
};

pub(crate) type ResizeEndCallback = Rc<dyn Fn(&mut App)>;

/// EP-005: the half a pending split would create, drawn in place of the real
/// pane it will be carved out of. The preset picker stands there until a preset
/// is picked, so the user sees exactly where the new pane lands - inside the
/// tree, at the target's slot, not against the whole grid.
///
/// `element` is consumed by the matching leaf: an `AnyElement` is not `Clone`,
/// and exactly one leaf can match.
pub(crate) struct SplitPreview {
    pub(crate) target: Entity<crate::pane::Pane>,
    pub(crate) direction: SplitDirection,
    pub(crate) element: std::cell::RefCell<Option<AnyElement>>,
}

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
    div.debug_selector(move || selector.clone())
}

impl LayoutTree {
    /// Render the layout tree recursively as nested GPUI flex divs, with an
    /// optional pending-split preview injected at the leaf it targets (EP-005).
    #[allow(clippy::only_used_in_recursion)]
    pub(crate) fn render_with_preview(
        &self,
        window: &Window,
        cx: &App,
        on_resize_end: Option<ResizeEndCallback>,
        preview: Option<&SplitPreview>,
    ) -> AnyElement {
        match self {
            LayoutTree::Leaf(pane) => {
                // The preview splits this leaf the way the real split will:
                // same axis, same halves, same divider gap - so the picker
                // appears exactly where the new pane is about to be.
                if let Some(preview) = preview.filter(|preview| preview.target == *pane)
                    && let Some(element) = preview.element.borrow_mut().take()
                {
                    let dir = preview.direction;
                    let half = || {
                        div()
                            .flex()
                            .flex_basis(relative(0.5))
                            .flex_grow(1.0)
                            .flex_shrink(1.0)
                            .size_full()
                            .min_w(px(MIN_PANE_SIZE))
                            .min_h(px(MIN_PANE_SIZE))
                            .overflow_hidden()
                    };
                    // Inert gap: the real divider only exists once the split
                    // does, but the spacing must already match.
                    let gap = match dir {
                        SplitDirection::Horizontal => {
                            div().h(px(DIVIDER_PX)).w_full().flex_shrink_0()
                        }
                        SplitDirection::Vertical => {
                            div().w(px(DIVIDER_PX)).h_full().flex_shrink_0()
                        }
                    };
                    let container = div().flex().size_full().overflow_hidden();
                    let container = match dir {
                        SplitDirection::Horizontal => container.flex_col(),
                        SplitDirection::Vertical => container.flex_row(),
                    };
                    return container
                        .child(half().child(pane.clone()))
                        .child(gap)
                        .child(half().child(element))
                        .into_any_element();
                }
                div().size_full().child(pane.clone()).into_any_element()
            }

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

                        // The band paints nothing: panes are cards and the gap
                        // between them exposes the window shell. It still
                        // reserves DIVIDER_PX on the main axis and keeps the
                        // resize cursor plus the drag hitbox.
                        let divider_hit_margin = (DIVIDER_PX - DIVIDER_HIT_PX) / 2.0;
                        let divider = match dir {
                            SplitDirection::Horizontal => div()
                                .h(px(DIVIDER_HIT_PX))
                                .w_full()
                                .my(px(divider_hit_margin))
                                .flex_shrink_0()
                                .cursor_row_resize(),
                            SplitDirection::Vertical => div()
                                .w(px(DIVIDER_HIT_PX))
                                .h_full()
                                .mx(px(divider_hit_margin))
                                .flex_shrink_0()
                                .cursor_col_resize(),
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

                    let elem =
                        child
                            .node
                            .render_with_preview(window, cx, on_resize_end.clone(), preview);
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
    use std::time::Instant;

    use gpui::{
        AppContext, Entity, Render, TestAppContext,
        profiler::{self, FrameEvent, FrameTimingCollector},
        px, size,
    };

    use super::super::tree::LayoutChild;
    use crate::pane::Pane;
    use crate::terminal::bench_corpus::{
        CORPUS_SEED, cpu_model, deterministic_streams, percentile_us, process_cpu_time,
        resident_set_bytes,
    };
    use crate::terminal::{
        TerminalView, start_render_content_timing_probe, take_render_content_lock_durations,
    };

    use super::*;

    const TOLERANCE: f32 = 2.0;

    struct FrameTraceGuard {
        enabled_by_test: bool,
    }

    impl FrameTraceGuard {
        fn enable() -> Self {
            Self {
                enabled_by_test: profiler::set_trace_enabled(true),
            }
        }
    }

    impl Drop for FrameTraceGuard {
        fn drop(&mut self) {
            if self.enabled_by_test {
                profiler::set_trace_enabled(false);
            }
        }
    }

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

    fn equal_container(direction: SplitDirection, nodes: Vec<LayoutTree>) -> LayoutTree {
        let ratio = 1.0 / nodes.len() as f32;
        LayoutTree::Container {
            direction,
            children: nodes
                .into_iter()
                .map(|node| LayoutChild {
                    node,
                    ratio: Rc::new(Cell::new(ratio)),
                })
                .collect(),
            drag: Rc::new(Cell::new(None)),
            container_size: Rc::new(Cell::new(0.0)),
        }
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
            self.tree.render_with_preview(window, cx, None, None)
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

    #[gpui::test]
    #[ignore = "EP-004 performance gate: eight-pane GPUI input-to-paint P95"]
    #[allow(
        clippy::assertions_on_constants,
        reason = "the ignored benchmark must reject accidental debug-profile execution"
    )]
    fn eight_pane_gpui_input_to_paint_performance_gate(cx: &mut TestAppContext) {
        const INPUT_TO_FRAME_P95_LIMIT_US: u128 = 16_700;

        assert!(
            !cfg!(debug_assertions),
            "run this baseline with cargo test --release"
        );

        let terminals_for_test = Rc::new(std::cell::RefCell::new(Vec::with_capacity(8)));
        let terminals_for_window = terminals_for_test.clone();
        let (_view, cx) = cx.add_window_view(move |_, cx| {
            let rows = (0..2)
                .map(|_| {
                    let columns = (0..4)
                        .map(|_| {
                            let terminal = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
                            terminals_for_window.borrow_mut().push(terminal.clone());
                            LayoutTree::Leaf(cx.new(|cx| Pane::new(terminal, 1, cx)))
                        })
                        .collect();
                    equal_container(SplitDirection::Vertical, columns)
                })
                .collect();
            RenderHarness {
                tree: equal_container(SplitDirection::Horizontal, rows),
            }
        });
        cx.executor().allow_parking();
        cx.simulate_resize(size(px(1600.0), px(1000.0)));
        cx.run_until_parked();

        let target_window_id = cx.update(|window, _| window.window_handle().window_id());
        let terminals = terminals_for_test.borrow().clone();
        assert_eq!(terminals.len(), 8, "benchmark must keep eight panes live");
        let streams = deterministic_streams();
        let total_bytes = streams.iter().map(Vec::len).sum::<usize>() * terminals.len();
        let mut burst_to_park = Vec::with_capacity(streams.len());
        let rss_start = resident_set_bytes();
        let mut rss_peak = rss_start;
        start_render_content_timing_probe();
        let frame_trace = FrameTraceGuard::enable();
        let mut frame_collector = FrameTimingCollector::new();
        let cpu_start = process_cpu_time();
        let wall_start = Instant::now();

        for (index, stream) in streams.iter().enumerate() {
            let burst_start = Instant::now();
            let window_size = if index % 2 == 0 {
                size(px(1584.0), px(984.0))
            } else {
                size(px(1600.0), px(1000.0))
            };
            cx.simulate_resize(window_size);
            for terminal in &terminals {
                terminal.update(cx, |view, cx| {
                    view.terminal.write_output(stream);
                    cx.notify();
                });
            }
            cx.run_until_parked();
            burst_to_park.push(burst_start.elapsed());
            rss_peak = rss_peak.max(resident_set_bytes());
        }

        let wall = wall_start.elapsed();
        let cpu = process_cpu_time().saturating_sub(cpu_start);
        let rss_end = resident_set_bytes();
        // `collect_unseen` yields both draw and present events; the benchmark
        // measures draw pacing, so present submissions are dropped here.
        let frame_timings = frame_collector
            .collect_unseen()
            .into_iter()
            .filter_map(|event| match event {
                FrameEvent::Draw(timing) => Some(timing),
                FrameEvent::Present(_) => None,
            })
            .collect::<Vec<_>>();
        drop(frame_trace);
        let traced_frame_samples = frame_timings.len();
        // Every target-window draw paints all eight panes, but a draw with no
        // observed invalidation carries no dirty-to-draw duration. Count the
        // draws for the paint invariant and keep the timed subset for the
        // pacing percentiles.
        let target_window_draws = frame_timings
            .iter()
            .filter(|timing| timing.window_id == target_window_id)
            .count();
        let mut target_frames = frame_timings
            .into_iter()
            .filter(|timing| timing.window_id == target_window_id)
            .filter_map(|timing| timing.dirty_to_draw_duration())
            .collect::<Vec<_>>();
        let mut render_content_lock = take_render_content_lock_durations();
        assert!(
            !target_frames.is_empty(),
            "frame tracing must capture target-window dirty-to-draw timings"
        );
        assert_eq!(
            render_content_lock.len(),
            target_window_draws * terminals.len(),
            "each traced target-window draw must paint every active terminal exactly once"
        );

        burst_to_park.sort_unstable();
        target_frames.sort_unstable();
        render_content_lock.sort_unstable();
        let throughput = total_bytes as f64 / wall.as_secs_f64() / (1024.0 * 1024.0);
        let input_to_frame_p95_us = percentile_us(&target_frames, 95);
        println!(
            "{{\"seed\":\"0x{CORPUS_SEED:016x}\",\"panes\":8,\"streams_per_pane\":{},\"resize_events\":{},\"bytes\":{total_bytes},\"throughput_mib_s\":{throughput:.3},\"input_to_frame_samples\":{},\"input_to_frame_p50_us\":{},\"input_to_frame_p95_us\":{input_to_frame_p95_us},\"input_to_frame_p95_limit_us\":{INPUT_TO_FRAME_P95_LIMIT_US},\"burst_to_park_samples\":{},\"burst_to_park_p50_us\":{},\"burst_to_park_p95_us\":{},\"traced_frame_samples\":{traced_frame_samples},\"target_window_draws\":{target_window_draws},\"render_content_lock_samples\":{},\"render_content_lock_held_p50_us\":{},\"render_content_lock_held_p95_us\":{},\"wall_ms\":{},\"cpu_ms\":{},\"rss_start_bytes\":{rss_start},\"rss_peak_bytes\":{rss_peak},\"rss_end_bytes\":{rss_end},\"hardware\":{:?},\"platform\":{:?},\"profile\":\"release\",\"measurement_boundary\":\"per-target-window GPUI frame from first dirty invalidation through draw completion\",\"burst_measurement\":\"diagnostic wall time for resize plus eight terminal updates through GPUI dispatcher until parked\",\"backend_scope\":\"backend-neutral GPUI renderer; Ghostty parser and host are covered by separate qualification gates\",\"lock_measurement\":\"one render_content snapshot round-trip per pane in each traced target-window draw, covering the Ghostty state-lock hold\",\"presentation_scope\":\"GPUI test-platform scene generation; excludes Window::present, GPU submission, compositor, and display scanout\"}}",
            streams.len(),
            streams.len(),
            target_frames.len(),
            percentile_us(&target_frames, 50),
            burst_to_park.len(),
            percentile_us(&burst_to_park, 50),
            percentile_us(&burst_to_park, 95),
            render_content_lock.len(),
            percentile_us(&render_content_lock, 50),
            percentile_us(&render_content_lock, 95),
            wall.as_millis(),
            cpu.as_millis(),
            cpu_model(),
            format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        );
        assert!(
            input_to_frame_p95_us <= INPUT_TO_FRAME_P95_LIMIT_US,
            "eight-pane GPUI dirty-to-draw frame p95 {input_to_frame_p95_us} us exceeds {INPUT_TO_FRAME_P95_LIMIT_US} us"
        );
    }

    #[test]
    fn available_main_axis_excludes_fixed_dividers() {
        assert!(
            (available_main_axis_px(500.0, 3) - (500.0 - 2.0 * DIVIDER_PX)).abs() < f32::EPSILON
        );
    }

    #[test]
    fn available_main_axis_never_goes_negative() {
        assert_eq!(available_main_axis_px(4.0, 3), 0.0);
    }
}
