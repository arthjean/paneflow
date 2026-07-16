#![cfg(all(target_os = "linux", feature = "native"))]

use paneflow_terminal_ghostty::{
    BackendEvent, Color, DisplayTerminal, FocusEvent, Key, KeyAction, KeyInput, Modifiers,
    MouseAction, MouseButton, MouseInput, Point, Rgb, Scroll, SelectionRange, WideCell, WindowSize,
};

fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
    DisplayTerminal::new(WindowSize::new(cols, rows, 8, 16).unwrap(), 10_000).unwrap()
}

#[test]
fn feed_produces_owned_snapshot_and_ordered_effects() {
    let mut terminal = terminal(16, 4);
    terminal
        .feed(b"\x1b]0;owned title\x07\x07\x1b[31mhello \x1b[0m")
        .unwrap();
    terminal.feed("中e\u{301}".as_bytes()).unwrap();
    terminal.feed(b"\x1b[6n").unwrap();

    let snapshot = terminal.snapshot().unwrap();
    assert_eq!(snapshot.cols, 16);
    assert_eq!(snapshot.rows, 4);
    assert!(
        snapshot
            .cells
            .iter()
            .any(|cell| cell.character == '中' && cell.wide == WideCell::Wide)
    );
    assert!(snapshot.cells.iter().any(|cell| {
        cell.character == 'e' && cell.zerowidth.as_deref() == Some(['\u{301}'].as_slice())
    }));
    assert!(
        snapshot
            .cells
            .iter()
            .take(5)
            .all(|cell| cell.flags.bold == false)
    );

    let events = terminal.drain_events();
    assert!(matches!(events.first(), Some(BackendEvent::Title(title)) if title == "owned title"));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, BackendEvent::Bell))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, BackendEvent::WritePty(bytes) if !bytes.is_empty()))
    );
}

#[test]
fn erased_cells_keep_the_active_rgb_background() {
    let mut terminal = terminal(8, 2);
    terminal.feed(b"\x1b[48;2;10;20;30m\x1b[2K").unwrap();

    let snapshot = terminal.snapshot().unwrap();
    assert!(snapshot.cells.iter().take(8).all(|cell| {
        cell.background
            == Color::Rgb(Rgb {
                r: 10,
                g: 20,
                b: 30,
            })
    }));
}

#[test]
fn encoders_follow_live_terminal_modes() {
    let mut terminal = terminal(80, 24);
    terminal
        .feed(b"\x1b[?1h\x1b[?1000h\x1b[?1006h\x1b[?1004h\x1b[?2004h")
        .unwrap();
    let key = terminal
        .encode_key(&KeyInput {
            key: Key::Up,
            action: KeyAction::Press,
            modifiers: Modifiers::empty(),
            consumed_modifiers: Modifiers::empty(),
            text: String::new(),
            unshifted_codepoint: None,
            composing: false,
        })
        .unwrap();
    assert_eq!(key, b"\x1bOA");
    assert_eq!(
        terminal.encode_focus(FocusEvent::Gained).unwrap(),
        b"\x1b[I"
    );
    assert_eq!(
        terminal.encode_paste("safe", false).unwrap(),
        b"\x1b[200~safe\x1b[201~"
    );

    let mouse = terminal
        .encode_mouse(MouseInput {
            action: MouseAction::Press,
            button: Some(MouseButton::Left),
            modifiers: Modifiers::CONTROL,
            x: 8.0,
            y: 16.0,
            screen_width: 640,
            screen_height: 384,
            padding_top: 0,
            padding_bottom: 0,
            padding_left: 0,
            padding_right: 0,
            any_button_pressed: true,
        })
        .unwrap();
    assert!(mouse.starts_with(b"\x1b[<"));
}

#[test]
fn search_selection_links_and_scrollback_use_fresh_owned_data() {
    let mut terminal = terminal(24, 5);
    terminal.feed(b"first\r\nsecond\r\n").unwrap();
    terminal
        .feed(b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\")
        .unwrap();

    let search = terminal.search("SECOND", false).unwrap();
    assert_eq!(search.matches.len(), 1);
    assert_eq!(search.matches[0].start, Point::new(1, 0));

    terminal
        .set_selection(SelectionRange {
            start: Point::new(0, 0),
            end: Point::new(0, 4),
            rectangle: false,
        })
        .unwrap();
    assert_eq!(terminal.selection_text().unwrap().as_deref(), Some("first"));
    terminal.clear_selection().unwrap();

    assert_eq!(
        terminal
            .hyperlink_at(Point::new(2, 0))
            .unwrap()
            .map(|link| link.uri),
        Some("https://example.com".into())
    );

    // Push `first` and `second` into real history while keeping the newer
    // lines in the active viewport.
    terminal
        .feed(b"\r\nthird\r\nfourth\r\nfifth\r\nsixth")
        .unwrap();
    let scrollback = terminal.extract_scrollback().unwrap().unwrap();
    assert!(scrollback.contains("first"));
    assert!(scrollback.contains("second"));
    assert!(!scrollback.contains("fifth"));
    assert!(!scrollback.contains("sixth"));
}

#[test]
fn positive_scroll_delta_moves_into_history() {
    let mut terminal = terminal(5, 2);
    terminal
        .feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix")
        .unwrap();
    assert!(
        !terminal
            .snapshot()
            .unwrap()
            .cells
            .iter()
            .any(|cell| cell.character == 'h')
    );

    terminal.scroll(Scroll::Delta(3));
    assert!(
        terminal
            .snapshot()
            .unwrap()
            .cells
            .iter()
            .any(|cell| cell.character == 'h')
    );

    terminal.scroll(Scroll::Delta(-3));
    assert!(
        !terminal
            .snapshot()
            .unwrap()
            .cells
            .iter()
            .any(|cell| cell.character == 'h')
    );
}

#[test]
fn absolute_scroll_rows_rebase_on_the_live_viewport() {
    let mut terminal = terminal(5, 2);
    terminal.feed(b"hello\x1bD\x1bD\x1bD").unwrap();
    let history_size = terminal.snapshot().unwrap().history_size;
    assert!(history_size >= 2, "fixture needs at least two history rows");

    // Do not snapshot between these commands. Each row must be converted from
    // Ghostty's live viewport, not from the UI snapshot captured above.
    for row in [0, 1, history_size] {
        terminal.scroll_to_viewport_row(row).unwrap();
    }
    assert_eq!(terminal.snapshot().unwrap().display_offset, 0);

    // Row coordinates stay pinned to the old content when output extends the
    // history between two drag targets.
    terminal.scroll_to_viewport_row(1).unwrap();
    terminal.feed(b"\r\nnew").unwrap();
    terminal.scroll_to_viewport_row(1).unwrap();
    let snapshot = terminal.snapshot().unwrap();
    assert_eq!(snapshot.display_offset + 1, snapshot.history_size);

    terminal.scroll_to_viewport_row(usize::MAX).unwrap();
    assert_eq!(terminal.snapshot().unwrap().display_offset, 0);
    terminal.scroll_to_viewport_row(0).unwrap();
    let snapshot = terminal.snapshot().unwrap();
    assert_eq!(snapshot.display_offset, snapshot.history_size);
}

fn terminal_with_wrapped_scrollback(prompt_redraw: bool) -> DisplayTerminal {
    let mut terminal = terminal(140, 49);
    let prompt_start = if prompt_redraw {
        b"\x1b]133;A;redraw=1\x07".as_slice()
    } else {
        b"\x1b]133;A;redraw=0\x07".as_slice()
    };
    terminal
        .feed(prompt_start)
        .and_then(|()| terminal.feed(b"prompt> \x1b]133;B\x07codex\r\n\x1b]133;C\x07"))
        .unwrap();
    for index in 0..370 {
        let line = format!("{index:04}{}\r\n", "x".repeat(136));
        terminal.feed(line.as_bytes()).unwrap();
    }
    terminal
        .feed(b"\x1b]133;D;0\x07")
        .and_then(|()| terminal.feed(prompt_start))
        .and_then(|()| terminal.feed(b"prompt> "))
        .unwrap();
    terminal
}

fn visible_non_blank(terminal: &mut DisplayTerminal) -> (usize, usize) {
    let snapshot = terminal.snapshot().unwrap();
    let non_blank = snapshot
        .cells
        .iter()
        .filter(|cell| !cell.character.is_whitespace())
        .count();
    (non_blank, snapshot.history_size)
}

#[test]
fn shrinking_wrapped_output_without_prompt_redraw_preserves_the_viewport() {
    let mut terminal = terminal_with_wrapped_scrollback(false);
    let (before_non_blank, before_history) = visible_non_blank(&mut terminal);
    assert!(before_history > 300, "fixture needs real scrollback");
    assert!(before_non_blank > 1_000, "fixture needs visible output");

    terminal
        .resize(WindowSize::new(139, 48, 8, 16).unwrap())
        .unwrap();

    let (after_non_blank, after_history) = visible_non_blank(&mut terminal);
    assert!(
        after_non_blank > 1_000,
        "shrink emptied the viewport: before={before_non_blank}, after={after_non_blank}, history={before_history} -> {after_history}",
    );
}

#[test]
fn shrinking_wrapped_output_with_prompt_redraw_preserves_the_viewport() {
    let mut terminal = terminal_with_wrapped_scrollback(true);
    let (before_non_blank, before_history) = visible_non_blank(&mut terminal);
    assert!(before_history > 300, "fixture needs real scrollback");
    assert!(before_non_blank > 1_000, "fixture needs visible output");

    terminal
        .resize(WindowSize::new(139, 48, 8, 16).unwrap())
        .unwrap();

    let (after_non_blank, after_history) = visible_non_blank(&mut terminal);
    assert!(
        after_non_blank > 1_000,
        "shrink emptied the viewport: before={before_non_blank}, after={after_non_blank}, history={before_history} -> {after_history}",
    );
}

#[test]
fn restore_neutralizes_live_control_sequences() {
    let mut terminal = terminal(40, 4);
    terminal
        .restore_scrollback("safe\u{1b}]0;spoof\u{7}\n\u{9b}31mplain")
        .unwrap();
    let events = terminal.drain_events();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, BackendEvent::Title(_)))
    );
    assert!(terminal.search("spoof", false).unwrap().matches.len() == 1);
    assert!(terminal.search("plain", false).unwrap().matches.len() == 1);
}

#[test]
fn dimensions_reject_zero_and_values_above_u16() {
    assert!(WindowSize::new(0, 24, 8, 16).is_err());
    assert!(WindowSize::new(80, 0, 8, 16).is_err());
    assert!(WindowSize::new(usize::from(u16::MAX) + 1, 24, 8, 16).is_err());
}
