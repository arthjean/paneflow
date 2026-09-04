use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    Context, InteractiveElement, IntoElement, Modifiers, ParentElement, Render, ScrollStrategy,
    SharedString, StatefulInteractiveElement, TestAppContext, div, point, size,
};

use super::*;
use crate::app::files_sidebar::ROW_HEIGHT;
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};

const ROW_COUNT: usize = 2_000;
const VIEWPORT_ROWS: usize = 10;

struct FilesListHarness {
    row_count: usize,
    scroll: UniformListScrollHandle,
    ranges: Rc<RefCell<Vec<Range<usize>>>>,
    clicked: Rc<Cell<Option<usize>>>,
}

impl Render for FilesListHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let ranges = self.ranges.clone();
        let clicked = self.clicked.clone();
        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .child(files_list(
                self.row_count,
                &self.scroll,
                move |range, _, _| {
                    ranges.borrow_mut().push(range.clone());
                    range
                        .map(|ix| {
                            let clicked = clicked.clone();
                            squircle_skin(
                                div().id(("files-list-row", ix)),
                                SharedString::from(format!("files-list-group-{ix}")),
                                ROW_RADIUS,
                                None,
                                Some(gpui::white()),
                            )
                            .w_full()
                            .h(ROW_HEIGHT)
                            .flex_none()
                            .on_click(move |_, _, _| clicked.set(Some(ix)))
                            .into_any_element()
                        })
                        .collect()
                },
            ))
    }
}

fn harness(
    cx: &mut TestAppContext,
) -> (gpui::Entity<FilesListHarness>, &mut gpui::VisualTestContext) {
    let (view, cx) = cx.add_window_view(|_, _| FilesListHarness {
        row_count: ROW_COUNT,
        scroll: UniformListScrollHandle::new(),
        ranges: Rc::default(),
        clicked: Rc::new(Cell::new(None)),
    });
    cx.simulate_resize(size(px(300.), ROW_HEIGHT * VIEWPORT_ROWS + px(8.)));
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.simulate_mouse_move(point(px(350.), px(0.)), cx);
    });
    (view, cx)
}

#[gpui::test]
fn files_list_hover_builds_only_viewport_rows(cx: &mut TestAppContext) {
    let (view, cx) = harness(cx);
    let ranges = view.read_with(cx, |view, _| view.ranges.clone());
    for row in [1, 3, 8, 2, 7] {
        ranges.borrow_mut().clear();
        let position = point(px(150.), px(4.) + ROW_HEIGHT * row + ROW_HEIGHT / 2.);
        cx.update(|window, cx| window.simulate_mouse_move(position, cx));
        let requests = ranges.borrow();
        assert!(
            !requests.is_empty(),
            "moving to another row must repaint its hover"
        );
        let built: usize = requests.iter().map(Range::len).sum();
        assert!(
            built <= (VIEWPORT_ROWS + 2) * 2,
            "hover built {built} rows for a {VIEWPORT_ROWS}-row viewport: {requests:?}"
        );
        assert!(
            requests
                .iter()
                .all(|range| range.len() <= VIEWPORT_ROWS + 1)
        );
    }
}

#[gpui::test]
fn files_list_scroll_preserves_row_targets_and_clamps_after_shrink(cx: &mut TestAppContext) {
    let (view, cx) = harness(cx);
    let (scroll, ranges, clicked) = view.read_with(cx, |view, _| {
        (
            view.scroll.clone(),
            view.ranges.clone(),
            view.clicked.clone(),
        )
    });
    ranges.borrow_mut().clear();
    view.update(cx, |_, cx| {
        scroll.scroll_to_item(ROW_COUNT - 1, ScrollStrategy::Bottom);
        cx.notify();
    });
    assert!(ranges.borrow().iter().any(|range| range.end == ROW_COUNT));
    cx.simulate_click(
        point(
            px(150.),
            px(4.) + ROW_HEIGHT * VIEWPORT_ROWS - ROW_HEIGHT / 2.,
        ),
        Modifiers::default(),
    );
    assert_eq!(clicked.get(), Some(ROW_COUNT - 1));

    ranges.borrow_mut().clear();
    view.update(cx, |view, cx| {
        view.row_count = 3;
        cx.notify();
    });
    assert!(ranges.borrow().contains(&(0..3)));
    assert!(ranges.borrow().iter().all(|range| range.end <= 3));
    cx.simulate_click(
        point(px(150.), px(4.) + ROW_HEIGHT / 2.),
        Modifiers::default(),
    );
    assert_eq!(clicked.get(), Some(0));
}
