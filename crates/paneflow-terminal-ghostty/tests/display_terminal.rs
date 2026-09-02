#![cfg(all(
    feature = "native",
    any(
        target_os = "linux",
        all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")
    )
))]

use paneflow_terminal_ghostty::{
    BackendEvent, Color, DisplayTerminal, FocusEvent, Key, KeyAction, KeyInput, Modifiers,
    MouseAction, MouseButton, MouseInput, Point, Rgb, Scroll, SelectionRange, TerminalAppearance,
    WideCell, WindowSize,
};

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
    DisplayTerminal::new(
        WindowSize::new(cols, rows, 8, 16).unwrap(),
        10_000,
        TerminalAppearance::default(),
    )
    .unwrap()
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn synchronized_output_tracks_the_dec_2026_bracket() {
    let mut terminal = terminal(16, 4);
    assert!(
        !terminal.synchronized_output().unwrap(),
        "a fresh terminal is not mid-redraw"
    );

    terminal.feed(b"[?2026h").unwrap();
    assert!(
        terminal.synchronized_output().unwrap(),
        "BSU must be observable, or the publish gate cannot suppress torn frames"
    );

    terminal.feed(b"redraw in progress").unwrap();
    assert!(terminal.synchronized_output().unwrap());

    terminal.feed(b"[?2026l").unwrap();
    assert!(
        !terminal.synchronized_output().unwrap(),
        "ESU must clear it"
    );
}

#[test]
fn feed_produces_owned_snapshot_and_ordered_effects() {
    let mut terminal = terminal(16, 4);
    terminal
        .feed(b"\x1b]0;owned title\x07\x07\x07\x1b[31mhello \x1b[0m")
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
    assert!(snapshot.cells.iter().take(5).all(|cell| !cell.flags.bold));

    let events = terminal.drain_events();
    assert!(matches!(events.first(), Some(BackendEvent::Title(title)) if title == "owned title"));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, BackendEvent::Bell))
            .count(),
        2
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

    let wheel = terminal
        .encode_mouse(MouseInput {
            action: MouseAction::Press,
            button: Some(MouseButton::Four),
            modifiers: Modifiers::empty(),
            x: 8.0,
            y: 16.0,
            screen_width: 640,
            screen_height: 384,
            padding_top: 0,
            padding_bottom: 0,
            padding_left: 0,
            padding_right: 0,
            any_button_pressed: false,
        })
        .unwrap();
    assert!(wheel.starts_with(b"\x1b[<64;"));
}

#[test]
fn bracketed_paste_preserves_payload_at_the_old_64_kib_boundary() {
    let mut terminal = terminal(80, 24);
    terminal.feed(b"\x1b[?2004h").unwrap();
    let paste = "x".repeat(64 * 1024);

    let encoded = terminal.encode_paste(&paste, true).unwrap();

    assert!(encoded.starts_with(b"\x1b[200~"));
    assert!(encoded.ends_with(b"\x1b[201~"));
    assert_eq!(&encoded[6..encoded.len() - 6], paste.as_bytes());
}

#[test]
fn keyboard_matrix_covers_modifiers_repeat_text_and_numpad() {
    let mut terminal = terminal(80, 24);
    let ctrl_a = terminal
        .encode_key(&KeyInput {
            key: Key::Character('a'),
            action: KeyAction::Press,
            modifiers: Modifiers::CONTROL,
            consumed_modifiers: Modifiers::empty(),
            text: String::new(),
            unshifted_codepoint: Some('a'),
            composing: false,
        })
        .unwrap();
    assert_eq!(ctrl_a, b"\x01");

    let altgr = terminal
        .encode_key(&KeyInput {
            key: Key::Character('@'),
            action: KeyAction::Press,
            modifiers: Modifiers::CONTROL | Modifiers::ALT,
            consumed_modifiers: Modifiers::CONTROL | Modifiers::ALT,
            text: "@".into(),
            unshifted_codepoint: Some('0'),
            composing: false,
        })
        .unwrap();
    assert_eq!(altgr, b"@");

    let ime_commit = terminal
        .encode_key(&KeyInput {
            key: Key::Unidentified,
            action: KeyAction::Press,
            modifiers: Modifiers::empty(),
            consumed_modifiers: Modifiers::empty(),
            text: "日本語".into(),
            unshifted_codepoint: None,
            composing: false,
        })
        .unwrap();
    assert_eq!(ime_commit, "日本語".as_bytes());

    terminal.feed(b"\x1b[>3u").unwrap();
    let press = terminal
        .encode_key(&KeyInput {
            key: Key::Function(5),
            action: KeyAction::Press,
            modifiers: Modifiers::SHIFT | Modifiers::CONTROL,
            consumed_modifiers: Modifiers::empty(),
            text: String::new(),
            unshifted_codepoint: None,
            composing: false,
        })
        .unwrap();
    let repeat = terminal
        .encode_key(&KeyInput {
            key: Key::Function(5),
            action: KeyAction::Repeat,
            modifiers: Modifiers::SHIFT | Modifiers::CONTROL,
            consumed_modifiers: Modifiers::empty(),
            text: String::new(),
            unshifted_codepoint: None,
            composing: false,
        })
        .unwrap();
    let release = terminal
        .encode_key(&KeyInput {
            key: Key::Function(5),
            action: KeyAction::Release,
            modifiers: Modifiers::SHIFT | Modifiers::CONTROL,
            consumed_modifiers: Modifiers::empty(),
            text: String::new(),
            unshifted_codepoint: None,
            composing: false,
        })
        .unwrap();
    assert!(!press.is_empty());
    assert_ne!(repeat, press);
    assert_ne!(release, press);
    assert_ne!(repeat, release);

    terminal.feed(b"\x1b[?66h").unwrap();
    assert!(terminal.modes().unwrap().application_keypad);
    let numpad = terminal
        .encode_key(&KeyInput {
            key: Key::NumpadDigit(1),
            action: KeyAction::Press,
            modifiers: Modifiers::empty(),
            consumed_modifiers: Modifiers::empty(),
            text: String::new(),
            unshifted_codepoint: Some('1'),
            composing: false,
        })
        .unwrap();
    assert!(!numpad.is_empty());
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
fn batched_line_texts_read_matches_from_real_history() {
    let mut terminal = terminal(32, 2);
    terminal
        .feed("EP003-HISTORY-é\r\nviewport-one\r\nviewport-two\r\nviewport-three".as_bytes())
        .unwrap();

    let search = terminal.search("EP003-HISTORY-é", false).unwrap();
    let rows: Vec<i32> = search
        .matches
        .iter()
        .map(|found| found.start.line)
        .collect();
    assert_eq!(rows.len(), 1);
    assert!(rows[0] < 0, "fixture marker must be in real history");

    let lines = terminal.line_texts(&rows).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].0, rows[0]);
    assert_eq!(lines[0].1.trim_end(), "EP003-HISTORY-é");
}

#[test]
fn search_chunks_never_copy_a_partial_or_over_budget_row() {
    let mut terminal = terminal(32, 2);
    terminal
        .feed(b"history-one\r\nhistory-two\r\nviewport-one\r\nviewport-two")
        .unwrap();

    let below_one_row = terminal.search_chunk(0, 31).unwrap();
    assert!(below_one_row.lines.is_empty());
    assert_eq!(below_one_row.next_row, 0);
    assert!(below_one_row.total_rows > 0);

    let one_row = terminal.search_chunk(0, 32).unwrap();
    assert_eq!(one_row.lines.len(), 1);
    assert_eq!(one_row.next_row, 1);
    assert_eq!(one_row.cols, 32);
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

    for row in [0, 1, history_size] {
        terminal.scroll_to_viewport_row(row).unwrap();
    }
    assert_eq!(terminal.snapshot().unwrap().display_offset, 0);

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

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
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

#[allow(
    clippy::unwrap_used,
    reason = "test fixture assertion must fail immediately"
)]
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

#[test]
fn repeated_headless_contract_survives_malformed_input_and_releases_every_terminal() {
    let size = WindowSize::new(40, 6, 8, 16).unwrap();
    assert!(DisplayTerminal::new(size, usize::MAX, TerminalAppearance::default()).is_err());

    for iteration in 0..64 {
        let mut terminal =
            DisplayTerminal::new(size, 2_000, TerminalAppearance::default()).unwrap();
        terminal.feed(b"\x1b]52;c;@@@\x07\xff").unwrap();
        terminal
            .feed(format!("\x1b[?1049h\x1b[2J\x1b[H\x1b[48;5;42mWIN-{iteration:02}-Ω").as_bytes())
            .unwrap();
        terminal
            .resize(WindowSize::new(41, 7, 8, 16).unwrap())
            .unwrap();

        let snapshot = terminal.snapshot().unwrap();
        assert_eq!((snapshot.cols, snapshot.rows), (41, 7));
        assert!(snapshot.cells.iter().any(|cell| cell.character == 'Ω'));
        assert!(
            snapshot
                .cells
                .iter()
                .any(|cell| cell.background == Color::Palette(42))
        );
        assert!(terminal.modes().unwrap().alternate_screen);

        let encoded = terminal
            .encode_key(&KeyInput {
                key: Key::Enter,
                action: KeyAction::Press,
                modifiers: Modifiers::empty(),
                consumed_modifiers: Modifiers::empty(),
                text: String::new(),
                unshifted_codepoint: None,
                composing: false,
            })
            .unwrap();
        assert_eq!(encoded, b"\r");
    }
}
