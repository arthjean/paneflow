use gpui::{Entity, TestAppContext, VisualTestContext};

use super::super::controls::{EditorDisplay, editor_display, set_editor_display};
use super::super::rows::{self, RowSlot};
use super::tests::*;
use super::*;

fn long_line(words: usize) -> String {
    (0..words)
        .map(|word| format!("word{word:03}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn wrap_file(lines: usize) -> String {
    let mut text = String::new();
    for row in 0..lines {
        if row % 5 == 2 {
            text.push_str(&long_line(60));
        } else {
            text.push_str(&format!("let short_{row} = {row};"));
        }
        text.push('\n');
    }
    text
}

fn toggle_wrap(view: &Entity<CodeView>, cx: &mut VisualTestContext) {
    let controls = view.read_with(cx, |view, _| view.controls.clone());
    cx.update(|window, cx| {
        controls.update(cx, |controls, cx| {
            controls.toggle_word_wrap_for_test(window, cx)
        });
    });
    frame(view, cx);
}

#[gpui::test]
fn word_wrap_from_the_controls_menu_folds_long_lines_and_drops_the_horizontal_scroll(
    cx: &mut TestAppContext,
) {
    let (view, cx) = scrolled(cx, "/nonexistent/wrap.md", &wrap_file(40));
    let (wide, rows_before) = view.read_with(cx, |view, _| {
        (
            view.geometry.get().max_h_offset,
            view.scroll.map().total_rows(),
        )
    });
    assert!(wide > 0.0, "the long lines overflow before wrapping");
    assert_eq!(rows_before, 41);

    toggle_wrap(&view, cx);

    view.read_with(cx, |view, cx| {
        assert!(view.word_wrap());
        assert!(
            view.controls.read(cx).word_wrap(),
            "the menu shows the toggle on"
        );
        assert_eq!(view.geometry.get().max_h_offset, 0.0);
        assert_eq!(view.h_offset, 0.0);
        assert!(view.navigation.layout.get().horizontal.is_none());
        let map = view.scroll.map();
        assert!(
            map.total_rows() > rows_before,
            "long lines take several rows"
        );
        assert_eq!(map.text_rows(0), 1);
        assert!(map.text_rows(2) > 1, "line 3 wraps");
    });

    toggle_wrap(&view, cx);
    view.read_with(cx, |view, _| {
        assert!(!view.word_wrap());
        assert_eq!(view.scroll.map().total_rows(), rows_before);
        assert!(view.geometry.get().max_h_offset > 0.0);
    });
}

#[gpui::test]
fn the_setting_sets_the_initial_state_and_the_toggle_stays_with_its_tab(cx: &mut TestAppContext) {
    let saved = editor_display();
    set_editor_display(EditorDisplay {
        word_wrap: true,
        ..saved
    });
    let (first, cx) = scrolled(cx, "/nonexistent/one.md", &wrap_file(10));
    let second = cx.update(|window, cx| {
        let seeded = seeded_view(PathBuf::from("/nonexistent/two.md"), &wrap_file(10));
        let _ = window;
        cx.new(seeded)
    });
    set_editor_display(saved);

    assert!(first.read_with(cx, |view, _| view.word_wrap()));
    assert!(second.read_with(cx, |view, _| view.word_wrap()));
    toggle_wrap(&first, cx);
    assert!(!first.read_with(cx, |view, _| view.word_wrap()));
    assert!(
        second.read_with(cx, |view, _| view.word_wrap()),
        "the menu toggle only applies to its own tab"
    );
}

#[gpui::test]
fn up_and_down_walk_the_visual_rows_of_a_wrapped_line(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/walk.md", &wrap_file(10));
    toggle_wrap(&view, cx);

    view.update_in(cx, |view, window, cx| {
        let wrapped_rows = view.scroll.map().text_rows(2);
        assert!(wrapped_rows >= 3, "the probe needs three visual rows");
        view.set_cursor_row(2, cx);
        view.right(&CeRight, window, cx);
        view.right(&CeRight, window, cx);
        let doc = view.document().expect("document");
        let start = doc.line_to_byte(2);
        assert_eq!(view.cursor(), start + 2);

        view.down(&CeDown, window, cx);
        assert_eq!(view.cursor_row(), 2, "Down stays on the wrapped line");
        let second = view.cursor();
        assert!(second > start + 2);
        let doc = view.document().expect("document");
        assert_eq!(
            view.scroll.text_row(doc, second),
            view.scroll.map().line_top(2) + 1
        );
        assert_eq!(
            view.scroll.segment_column(doc, second),
            2,
            "the goal column holds"
        );

        view.up(&CeUp, window, cx);
        assert_eq!(view.cursor(), start + 2, "Up comes back to the first row");
        view.up(&CeUp, window, cx);
        assert_eq!(view.cursor_row(), 1);
    });
}

#[gpui::test]
fn a_click_on_a_continuation_row_lands_inside_the_wrapped_line(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/click.md", &wrap_file(10));
    toggle_wrap(&view, cx);

    let (row_y, text_x, line_start, line_end) = view.read_with(cx, |view, _| {
        let map = view.scroll.map();
        let hits = view.hits.borrow();
        let doc = view.document().expect("document");
        let range = doc.line_byte_range(2).expect("line 3");
        (
            hits.row_top(map.line_top(2) + 1) + CODE_ROW_HEIGHT / 2.0,
            hits.text_x,
            range.start,
            range.end,
        )
    });
    let offset = view.read_with(cx, |view, _| {
        view.offset_at(point(px(text_x + 1.0), px(row_y)))
            .expect("an offset")
    });
    assert!(offset > line_start && offset < line_end);
    let first = view.read_with(cx, |view, _| {
        let doc = view.document().expect("document");
        view.scroll.segments(doc, 2)[1].start
    });
    assert_eq!(
        offset,
        line_start + first,
        "the click lands on the second row's first column"
    );
}

#[gpui::test]
fn the_line_at_the_top_stays_at_the_top_when_the_width_changes(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/anchor.md", &wrap_file(400));
    toggle_wrap(&view, cx);
    view.update(cx, |view, cx| {
        let top = view.scroll.map().line_top(150) as f64;
        view.scroll.set_rows(top);
        cx.notify();
    });
    cx.run_until_parked();
    let before = view.read_with(cx, |view, _| view.scroll.line_position());
    assert_eq!(before.floor(), 150.0);

    cx.simulate_resize(size(px(520.), VIEWPORT.y));
    cx.run_until_parked();
    frame(&view, cx);

    let (after, cols) = view.read_with(cx, |view, _| {
        (
            view.scroll.line_position(),
            view.scroll.wrap_key().map(|key| key.cols),
        )
    });
    assert!(cols.is_some());
    assert_eq!(
        after.floor(),
        150.0,
        "line 151 is still the first visible line"
    );
    assert!((after - before).abs() < 1e-6);
}

#[gpui::test]
fn numbers_and_markers_sit_on_the_first_row_of_each_line(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/numbers.md", &wrap_file(12));
    toggle_wrap(&view, cx);
    view.read_with(cx, |view, _| {
        let map = view.scroll.map();
        let hits = view.hits.borrow();
        let continuation = map.line_top(2) + 1;
        assert!(matches!(
            map.slot_at(continuation),
            RowSlot::Text {
                line: 2,
                segment: 1
            }
        ));
        assert_eq!(view.row_top(3), hits.row_top(map.line_top(3)));
        let blocks = [paneflow_textdiff::Block {
            lines: 2..4,
            base_lines: 2..3,
            dirty: false,
            too_big: false,
        }];
        let rects = super::super::markers::marker_rects(&blocks, &map, 0, 2_000.0);
        assert_eq!(
            rects[0].y,
            rows::rows_to_px(map.line_top(2) as f64) as f32 + 1.0
        );
        assert_eq!(
            rects[0].y + rects[0].h,
            rows::rows_to_px(map.line_top(4) as f64) as f32 - 1.0,
            "a bar spans every visual row of its lines"
        );
    });
}

#[gpui::test]
fn a_read_only_giant_line_wraps(cx: &mut TestAppContext) {
    let giant = "x".repeat(super::super::load::MAX_LINE_CHARS + 500);
    let text = format!("first\n{giant}\nlast\n");
    let (view, cx) = scrolled(cx, "/nonexistent/giant.json", &text);
    assert!(view.read_with(cx, |view, _| view.document().expect("doc").is_read_only()));
    toggle_wrap(&view, cx);
    view.read_with(cx, |view, _| {
        let cols = view.scroll.wrap_key().expect("wrapped").cols;
        let map = view.scroll.map();
        assert_eq!(map.text_rows(1), giant.len().div_ceil(cols));
        assert_eq!(map.line_top(2), 1 + map.text_rows(1));
    });
}

#[gpui::test]
fn a_remembered_position_lands_on_the_same_line_when_the_tab_wraps(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/memory.md", &wrap_file(400));
    toggle_wrap(&view, cx);
    view.update(cx, |view, cx| {
        let top = view.scroll.map().line_top(150) as f64;
        view.scroll.set_rows(top);
        cx.notify();
    });
    cx.run_until_parked();
    let position = view.read_with(cx, |view, _| view.position().expect("a position"));
    assert_eq!(position.scroll_line.floor(), 150.0);

    view.update(cx, |view, cx| {
        let lines = view.document().expect("doc").line_count();
        view.scroll.install_map(rows::RowMap::uniform(lines), None);
        view.scroll.reset_rows();
        view.restore_position(position);
        cx.notify();
    });
    cx.run_until_parked();
    frame(&view, cx);
    frame(&view, cx);

    let top = view.read_with(cx, |view, _| view.scroll.line_position());
    assert_eq!(
        top.floor(),
        150.0,
        "a reopened wrapped tab starts on line 151, not at its count of visual rows"
    );
}
