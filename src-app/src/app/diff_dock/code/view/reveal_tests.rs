use gpui::{
    Entity, Modifiers, MouseButton, MouseDownEvent, TestAppContext, TouchPhase, VisualTestContext,
};

use super::tests::*;
use super::*;

fn numbered(lines: usize) -> String {
    (0..lines)
        .map(|row| format!("line number {row}\n"))
        .collect()
}

fn reveal(view: &Entity<CodeView>, cx: &mut VisualTestContext, reveal: FileReveal) {
    view.update(cx, |view, cx| view.reveal(reveal, cx));
    cx.run_until_parked();
    frame(view, cx);
}

fn centre_line(view: &CodeView) -> f64 {
    view.scroll
        .map()
        .line_position(view.scroll.rows() + view.scroll.visible_rows() / 2.0)
}

#[gpui::test]
fn a_requested_line_is_centred_highlighted_and_holds_the_caret(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/reveal.rs", &numbered(300));
    reveal(
        &view,
        cx,
        FileReveal {
            line: 120,
            column: Some(6),
            end_line: None,
        },
    );
    view.read_with(cx, |view, _| {
        assert_eq!(view.revealed_lines(), Some(119..120));
        assert_eq!(view.cursor_line_column(), (120, 6));
        let centre = centre_line(view);
        assert!(
            (centre - 119.5).abs() <= 1.0,
            "line 120 sits mid-screen, centre {centre}"
        );
        let doc = view.document().expect("doc");
        assert_eq!(
            view.line_bands(doc),
            vec![LineBand {
                lines: 119..120,
                color: reveal_color()
            }]
        );
        assert!(
            reveal_color().a < 0.3,
            "the wash leaves the selection readable"
        );
    });
}

#[gpui::test]
fn a_range_highlights_every_line_it_spans(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/range.rs", &numbered(300));
    reveal(
        &view,
        cx,
        FileReveal {
            line: 200,
            column: None,
            end_line: Some(206),
        },
    );
    view.read_with(cx, |view, _| {
        assert_eq!(view.revealed_lines(), Some(199..206));
        let visible = view.visible_row_range();
        assert!(visible.start <= 199 && visible.end >= 206, "{visible:?}");
    });
}

#[gpui::test]
fn a_line_past_the_end_reveals_the_last_line(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/past.rs", &numbered(50));
    reveal(&view, cx, FileReveal::line(9_999));
    view.read_with(cx, |view, _| {
        let last = view.document().expect("doc").line_count() - 1;
        assert_eq!(view.revealed_lines(), Some(last..last + 1));
        assert_eq!(view.cursor_row(), last);
    });
}

#[gpui::test]
fn reopening_with_another_line_moves_the_highlight(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/again.rs", &numbered(300));
    reveal(&view, cx, FileReveal::line(20));
    reveal(&view, cx, FileReveal::line(250));
    view.read_with(cx, |view, _| {
        assert_eq!(view.revealed_lines(), Some(249..250));
        assert_eq!(view.cursor_row(), 249);
    });
}

#[gpui::test]
fn a_keystroke_a_click_or_a_scroll_clears_the_highlight(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/dismiss.rs", &numbered(300));
    reveal(&view, cx, FileReveal::line(120));
    view.update_in(cx, |view, window, cx| {
        view.replace_text_in_range(None, "x", window, cx)
    });
    assert_eq!(view.read_with(cx, |view, _| view.revealed_lines()), None);

    reveal(&view, cx, FileReveal::line(120));
    let centre = point(VIEWPORT.x / 2., VIEWPORT.y / 2.);
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: centre,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.revealed_lines()), None);

    reveal(&view, cx, FileReveal::line(120));
    cx.simulate_event(ScrollEvent {
        position: centre,
        delta: ScrollDelta::Lines(point(0.0, -3.0)),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
        ..Default::default()
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.revealed_lines()), None);
}

#[gpui::test]
fn a_remembered_position_brings_back_the_scroll_and_the_caret(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/memory.rs", &numbered(400));
    view.update(cx, |view, cx| {
        view.restore_position(FilePosition {
            scroll_line: 150.0,
            line: 160,
            column: 4,
        });
        cx.notify();
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.scroll_rows(), 150.0);
        assert_eq!(view.cursor_line_column(), (161, 5));
        let position = view.position().expect("a position");
        assert_eq!((position.line, position.column), (160, 4));
    });
}

#[gpui::test]
fn a_file_that_shrank_brings_the_position_back_to_its_last_line(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/shrunk.rs", &numbered(30));
    view.update(cx, |view, cx| {
        view.restore_position(FilePosition {
            scroll_line: 900.0,
            line: 950,
            column: 3,
        });
        cx.notify();
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        let last = view.document().expect("doc").line_count() - 1;
        assert_eq!(view.cursor_row(), last);
        assert_eq!(view.scroll_rows(), view.scroll.max_rows());
    });
}

#[gpui::test]
fn a_requested_line_wins_over_the_remembered_position(cx: &mut TestAppContext) {
    let (view, cx) = scrolled(cx, "/nonexistent/wins.rs", &numbered(400));
    view.update(cx, |view, cx| {
        view.restore_position(FilePosition {
            scroll_line: 10.0,
            line: 12,
            column: 0,
        });
        view.reveal(FileReveal::line(300), cx);
    });
    cx.run_until_parked();
    frame(&view, cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.cursor_row(), 299);
        assert!((centre_line(view) - 299.5).abs() <= 1.0);
    });
}
