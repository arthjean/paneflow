#![cfg(all(
    feature = "native",
    any(
        target_os = "linux",
        all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")
    )
))]

use paneflow_terminal_ghostty::{
    BackendEvent, ClipboardLocation, Color, DecodedImage, DisplayTerminal, GestureBehavior,
    GestureBehaviors, GhosttyError, NativeSearchSnapshot, PALETTE_LEN, PasteRepresentation, Point,
    PressOptions, Rgb, SnapshotDecoder, SnapshotRestore, TerminalAppearance, WideCell, WindowSize,
    default_palette, set_png_decoder,
};

const LINK: &str = "http://example.com";
const BOLD_LINK: &[u8] = b"\x1b[1m\x1b]8;id=fixture;http://example.com\x1b\\";
const PLAIN_TEXT: &str = "\x1b[0m\x1b]8;;\x1b\\";
const PLAIN: &[u8] = PLAIN_TEXT.as_bytes();
const WIDEST: usize = u16::MAX as usize;

fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
    terminal_with_history(cols, rows, 1_000)
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn terminal_with_history(cols: usize, rows: usize, history: usize) -> DisplayTerminal {
    DisplayTerminal::new(
        WindowSize::new(cols, rows, 8, 16).unwrap(),
        history,
        TerminalAppearance::default(),
    )
    .unwrap()
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn assert_bold_link(
    terminal: &mut DisplayTerminal,
    point: Point,
    character: char,
    foreground: Color,
    context: &str,
) {
    let content = terminal.snapshot().unwrap();
    let cell = content
        .cells
        .iter()
        .find(|cell| cell.point == point)
        .unwrap();
    assert_eq!(cell.character, character, "{context}");
    assert!(cell.flags.bold, "{context}: the cursor style must survive");
    assert_eq!(
        cell.foreground, foreground,
        "{context}: the cursor style must survive"
    );
    assert!(
        cell.hyperlink,
        "{context}: the cursor hyperlink must survive"
    );
    let uri = terminal.hyperlink_at(point).unwrap().map(|link| link.uri);
    assert_eq!(uri.as_deref(), Some(LINK), "{context}");
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn assert_narrowed_to_two_columns(terminal: &mut DisplayTerminal, context: &str) {
    resize(terminal, 2, 1);
    let content = terminal.snapshot().unwrap();
    assert_eq!(
        (content.cols, content.rows, content.cells.len()),
        (2, 1, 2),
        "{context}: the published grid must match the narrowed size"
    );
    feed(terminal, b"\r\nef");
    assert_eq!(plain_text(terminal), "ef", "{context}");
    feed(terminal, b"\r\ngh");
    assert_eq!(plain_text(terminal), "gh", "{context}");
}

fn mode_report(terminal: &mut DisplayTerminal, query: &str) -> Vec<u8> {
    let _ = terminal.drain_events();
    feed(terminal, format!("\x1b[{query}$p").as_bytes());
    pty_writes(terminal)
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture search must fail immediately"
)]
fn search(terminal: &mut DisplayTerminal, query: &str) -> NativeSearchSnapshot {
    terminal.set_search_query(query).unwrap();
    for _ in 0..10_000 {
        let snapshot = terminal
            .search_step_with_rail_refresh(true)
            .unwrap()
            .unwrap();
        if snapshot.complete {
            return snapshot;
        }
    }
    unreachable!("native search did not complete")
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn feed(terminal: &mut DisplayTerminal, bytes: &[u8]) {
    terminal.feed(bytes).unwrap();
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn resize(terminal: &mut DisplayTerminal, cols: usize, rows: usize) {
    terminal
        .resize(WindowSize::new(cols, rows, 8, 16).unwrap())
        .unwrap();
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn plain_text(terminal: &mut DisplayTerminal) -> String {
    let rows = terminal.snapshot().unwrap().rows;
    let lines: Vec<i32> = (0..rows as i32).collect();
    let texts: Vec<String> = terminal
        .line_texts(&lines)
        .unwrap()
        .into_iter()
        .map(|(_, text)| text.trim_end().to_owned())
        .collect();
    texts.join("\n").trim_end_matches('\n').to_owned()
}

fn pty_writes(terminal: &mut DisplayTerminal) -> Vec<u8> {
    terminal
        .drain_events()
        .into_iter()
        .filter_map(|event| match event {
            BackendEvent::WritePty(bytes) => Some(bytes),
            _ => None,
        })
        .flatten()
        .collect()
}

fn titles(terminal: &mut DisplayTerminal) -> Vec<String> {
    terminal
        .drain_events()
        .into_iter()
        .filter_map(|event| match event {
            BackendEvent::Title(title) => Some(title),
            _ => None,
        })
        .collect()
}

fn palette_reply(terminal: &mut DisplayTerminal, index: u8) -> Vec<u8> {
    let _ = terminal.drain_events();
    feed(terminal, format!("\x1b]4;{index};?\x1b\\").as_bytes());
    pty_writes(terminal)
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn select_with(
    terminal: &mut DisplayTerminal,
    behavior: GestureBehavior,
    point: Point,
) -> Option<String> {
    terminal.clear_selection().unwrap();
    let options = PressOptions {
        behaviors: Some(GestureBehaviors {
            single_click: behavior,
            double_click: behavior,
            triple_click: behavior,
        }),
        word_boundaries: vec!['\0', ' '],
        ..PressOptions::default()
    };
    terminal.gesture_press(point, &options).unwrap();
    terminal.gesture_release(Some(point)).unwrap();
    terminal.gesture_reset().unwrap();
    terminal.selection_text().unwrap()
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn upstream_e6db5b633_narrowing_without_reflow_clears_an_orphaned_wide_head() {
    let mut terminal = terminal(3, 3);
    feed(&mut terminal, "\x1b[?7la一".as_bytes());
    resize(&mut terminal, 2, 3);

    let content = terminal.snapshot().unwrap();
    let last = &content.cells[1];
    assert_eq!(last.character, ' ', "the cut wide head must be cleared");
    assert_eq!(last.wide, WideCell::Narrow);

    feed(&mut terminal, b"\x1b[1;2H\x1b[@\x1b[1K");
    let content = terminal.snapshot().unwrap();
    assert_eq!(content.cols, 2);
    assert!(
        content.cells[..2].iter().all(|cell| cell.character == ' '),
        "insert and erase on the narrowed row must stay inside it"
    );
    assert_eq!(plain_text(&mut terminal), "");
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn upstream_e6db5b633_narrowing_a_wide_tail_survives_a_thousand_resizes() {
    let mut terminal = terminal(3, 3);
    for iteration in 0..1_000 {
        feed(&mut terminal, "\x1b[?7l\x1b[Ha一".as_bytes());
        let before = terminal.snapshot().unwrap();
        assert_eq!(
            before.cells[1].wide,
            WideCell::Wide,
            "iteration {iteration}: the wide head must sit in the column the shrink keeps last"
        );

        resize(&mut terminal, 2, 3);
        let after = terminal.snapshot().unwrap();
        assert_eq!(after.cols, 2);
        assert_eq!(
            after.cells[1].character, ' ',
            "iteration {iteration}: the cut wide head must be cleared"
        );
        assert_eq!(
            after.cells[1].wide,
            WideCell::Narrow,
            "iteration {iteration}: the last column must be narrow after the shrink"
        );

        feed(&mut terminal, b"\x1b[1;2H\x1b[@\x1b[1K\x1b[2K");
        resize(&mut terminal, 3, 3);
    }
}

#[test]
fn upstream_f9ab34f10_a_resize_keeps_the_live_and_saved_pending_wrap() {
    let cases: [(&str, usize, &str); 7] = [
        ("ABCD", 6, "ABCDX"),
        ("ABCD", 3, "ABC\nDX"),
        ("ABCD", 2, "AB\nCD\nX"),
        ("ABCD", 4, "ABCD\nX"),
        ("ABCDEFGH", 6, "ABCDEF\nGHX"),
        ("AB界", 6, "AB界X"),
        ("ABC", 6, "ABCX"),
    ];
    for (text, cols, expected) in cases {
        for restore in [false, true] {
            let mut terminal = terminal(4, 5);
            feed(&mut terminal, text.as_bytes());
            if restore {
                feed(&mut terminal, b"\x1b7");
            }
            resize(&mut terminal, cols, 5);
            if restore {
                feed(&mut terminal, b"\x1b8");
            }
            feed(&mut terminal, b"X");
            assert_eq!(
                plain_text(&mut terminal),
                expected,
                "{text:?} resized to {cols} columns, saved cursor restored: {restore}"
            );
        }
    }
}

#[test]
fn upstream_afded91df_the_saved_cursor_survives_repeated_widening() {
    let mut terminal = terminal(4, 5);
    feed(&mut terminal, b"abc\r\nAAA|\x1b7");
    resize(&mut terminal, 5, 5);
    resize(&mut terminal, 6, 5);
    feed(&mut terminal, b"\x1b8X");
    assert_eq!(plain_text(&mut terminal), "abc\nAAA|X");
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn upstream_d4f45bee3_reverse_wrap_above_the_top_margin_does_not_jump() {
    let mut terminal = terminal(5, 5);
    feed(
        &mut terminal,
        b"\x1b[?7h\x1b[?45h\x1b[?69h\x1b[1;2sAB\x1b7\x1b[2;5s\x1b[3;5r\x1b8\x1b[D",
    );
    let cursor = terminal.snapshot().unwrap().cursor.point;
    assert_eq!(cursor, Point::new(0, 1));
    feed(&mut terminal, b"X");
    assert_eq!(plain_text(&mut terminal), "AX");
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn upstream_6220a3617_a_codepoint_above_0xff_prints_unmapped_in_a_charset() {
    let mut terminal = terminal(10, 2);
    feed(&mut terminal, "\x1b(0`😀a".as_bytes());
    assert_eq!(plain_text(&mut terminal), "◆😀▒");
    let content = terminal.snapshot().unwrap();
    assert_eq!(content.cells[1].character, '😀');
    assert_eq!(content.cells[1].wide, WideCell::Wide);
    assert_eq!(content.cursor.point, Point::new(0, 4));
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn upstream_c706451fe_a_single_shift_maps_exactly_one_printed_character() {
    for cluster in [false, true] {
        let mut terminal = terminal(10, 2);
        let mode = if cluster { "h" } else { "l" };
        feed(
            &mut terminal,
            format!("\x1b[?2027{mode}\x1b*A#\x1bN\u{301}#").as_bytes(),
        );
        let content = terminal.snapshot().unwrap();
        assert_eq!(
            content.cells[1].character, '#',
            "the combining mark used up the single shift, grapheme clustering: {cluster}"
        );
        assert_eq!(content.cursor.point, Point::new(0, 2));
    }
}

#[test]
fn upstream_a4f0d9f4a_batched_special_graphics_match_per_character_printing() {
    let text = b"\x1b(0lqqqqk\r\nx`afgjx\r\nmqqqqj\x1b(B ascii \x1b)0\x0eqq\x0fqq";
    let mut batched = terminal(20, 4);
    feed(&mut batched, text);
    let mut scalar = terminal(20, 4);
    for byte in text {
        feed(&mut scalar, std::slice::from_ref(byte));
    }
    assert_eq!(plain_text(&mut batched), plain_text(&mut scalar));
    assert_eq!(
        plain_text(&mut batched),
        "┌────┐\n│◆▒°±┘│\n└────┘ ascii ──qq"
    );
}

#[test]
fn upstream_a3e80a685_word_selection_spans_wide_characters_from_either_half() {
    let cases: [(&str, usize, usize, usize, &str); 5] = [
        ("日本語", 10, 0, 5, "日本語"),
        ("a日b語c", 10, 0, 6, "a日b語c"),
        (" 日本語 ", 10, 1, 6, "日本語"),
        ("日本語", 4, 0, 5, "日本語"),
        ("日本語", 5, 0, 6, "日本語"),
    ];
    for (text, cols, start, end, expected) in cases {
        let mut terminal = terminal(cols, 4);
        feed(&mut terminal, text.as_bytes());
        for offset in start..=end {
            let point = Point::new((offset / cols) as i32, offset % cols);
            let selected = select_with(&mut terminal, GestureBehavior::Word, point)
                .map(|text| text.replace('\n', ""));
            assert_eq!(
                selected.as_deref(),
                Some(expected),
                "{text:?} at {cols} columns, offset {offset}"
            );
        }
    }
}

#[test]
fn upstream_c4f15c884_word_selection_stops_at_a_hard_line_break() {
    let mut terminal = terminal(5, 2);
    feed(&mut terminal, b"abcde\r\nfghij");
    for (line, expected) in [(0, "abcde"), (1, "fghij")] {
        for column in 0..5 {
            let selected = select_with(
                &mut terminal,
                GestureBehavior::Word,
                Point::new(line, column),
            );
            assert_eq!(
                selected.as_deref(),
                Some(expected),
                "line {line}, column {column}"
            );
        }
    }

    let mut wrapped = self::terminal(5, 3);
    feed(&mut wrapped, b"abcdefghij\r\nklmno");
    for line in 0..2 {
        for column in 0..5 {
            let selected = select_with(
                &mut wrapped,
                GestureBehavior::Word,
                Point::new(line, column),
            )
            .map(|text| text.replace('\n', ""));
            assert_eq!(selected.as_deref(), Some("abcdefghij"));
        }
    }
}

#[test]
fn upstream_13b5ab204_line_selection_stops_at_a_prompt_boundary_across_blank_cells() {
    for column in 1..3 {
        let mut terminal = terminal(5, 2);
        feed(&mut terminal, b"\x1b]133;A\x1b\\x\x1b[2Cm");
        let selected = select_with(&mut terminal, GestureBehavior::Line, Point::new(0, column));
        assert!(
            selected
                .as_deref()
                .is_none_or(|text| text.trim().is_empty()),
            "column {column} selected {selected:?}"
        );
    }
}

#[test]
fn upstream_520d8f55a_can_and_sub_cancel_an_osc_title() {
    let mut terminal = terminal(20, 2);
    feed(&mut terminal, b"\x1b]2;before\x07");
    assert_eq!(titles(&mut terminal), ["before"]);

    feed(&mut terminal, b"\x1b]2;can\x18");
    feed(&mut terminal, b"\x1b]2;sub\x1a");
    assert!(titles(&mut terminal).is_empty());

    feed(&mut terminal, b"\x1b]2;after\x07");
    assert_eq!(titles(&mut terminal), ["after"]);
}

#[test]
fn upstream_73768913b_an_osc_index_with_a_digit_separator_is_rejected() {
    let mut terminal = terminal(20, 2);
    let default = palette_reply(&mut terminal, 10);
    assert!(!default.is_empty(), "OSC 4 queries must be answered");

    feed(&mut terminal, b"\x1b]4;1_0;rgb:ff/00/00\x1b\\");
    assert_eq!(palette_reply(&mut terminal, 10), default);

    feed(&mut terminal, b"\x1b]4;10;rgb:ff/00/00\x1b\\");
    assert_ne!(palette_reply(&mut terminal, 10), default);
}

#[test]
fn upstream_36953bca8_osc_105_is_consumed_by_the_color_parser() {
    let mut terminal = terminal(20, 2);
    terminal
        .capture_unknown_sequences(true)
        .expect("unknown sequence capture must install");
    feed(&mut terminal, b"\x1b]105\x07\x1b]105;0\x1b\\after");
    assert!(
        !terminal
            .drain_events()
            .iter()
            .any(|event| matches!(event, BackendEvent::UnknownSequence { .. })),
        "OSC 105 must not reach the unknown sequence path"
    );
    assert_eq!(plain_text(&mut terminal), "after");
}

#[test]
fn upstream_9dc0d974e_and_3beb6d717_decrqm_answers_ansi_and_16_bit_modes() {
    let mut terminal = terminal(20, 2);
    let _ = terminal.drain_events();

    feed(&mut terminal, b"\x1b[4$p");
    assert_eq!(pty_writes(&mut terminal), b"\x1b[4;2$y");
    feed(&mut terminal, b"\x1b[4h\x1b[4$p");
    assert_eq!(pty_writes(&mut terminal), b"\x1b[4;1$y");

    feed(&mut terminal, b"\x1b[?32775$p");
    assert_eq!(pty_writes(&mut terminal), b"\x1b[?32775;0$y");
    feed(&mut terminal, b"\x1b[?7$p");
    assert_eq!(
        pty_writes(&mut terminal),
        b"\x1b[?7;1$y",
        "a large unknown mode must not alias wraparound"
    );
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn upstream_bb20f8e45_ris_restores_the_configured_palette() {
    let mut theme: [Rgb; PALETTE_LEN] = default_palette();
    theme[1] = Rgb {
        r: 0x12,
        g: 0x34,
        b: 0x56,
    };
    let mut terminal = terminal(20, 2);
    terminal.set_palette(&theme).unwrap();
    let themed = palette_reply(&mut terminal, 1);

    let mut stock = self::terminal(20, 2);
    assert_ne!(
        palette_reply(&mut stock, 1),
        themed,
        "the fixture theme must differ from Ghostty's default"
    );

    feed(&mut terminal, b"\x1b]4;1;rgb:ab/cd/ef\x1b\\");
    assert_ne!(palette_reply(&mut terminal, 1), themed);

    feed(&mut terminal, b"\x1bc");
    assert_eq!(palette_reply(&mut terminal, 1), themed);
}

fn one_pixel_png(data: &[u8]) -> Option<DecodedImage> {
    (!data.is_empty()).then(|| DecodedImage {
        width: 1,
        height: 1,
        rgba: vec![0x11, 0x22, 0x33, 0xff],
    })
}

fn refusing_png(_: &[u8]) -> Option<DecodedImage> {
    None
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn upstream_aca9bf031_a_kitty_png_decodes_through_the_trampoline_or_is_refused() {
    const TRANSMIT: &[u8] = b"\x1b_Ga=t,f=100,i=7,q=2;iVBORw0KGgo=\x1b\\";

    set_png_decoder(Some(one_pixel_png)).unwrap();
    let mut decoded = terminal(20, 4);
    decoded.enable_kitty_graphics(1 << 20, 1 << 16).unwrap();
    feed(&mut decoded, TRANSMIT);
    let info = decoded
        .kitty_graphics()
        .unwrap()
        .and_then(|graphics| graphics.image(7))
        .map(|image| image.info().unwrap());

    set_png_decoder(Some(refusing_png)).unwrap();
    let mut refused = terminal(20, 4);
    refused.enable_kitty_graphics(1 << 20, 1 << 16).unwrap();
    feed(&mut refused, TRANSMIT);
    let refused_image = refused
        .kitty_graphics()
        .unwrap()
        .and_then(|graphics| graphics.image(7))
        .is_some();
    set_png_decoder(None).unwrap();

    let info = info.expect("a decoded PNG must be stored");
    assert_eq!((info.width, info.height, info.len), (1, 1, 4));
    assert!(!refused_image, "a refused decode must store nothing");
}

#[test]
fn upstream_65ff54c67_a_single_row_prune_keeps_the_cursor_style_and_link() {
    let mut terminal = terminal_with_history(3, 1, 0);
    feed(&mut terminal, BOLD_LINK);
    feed(&mut terminal, &[b'a'; 103]);
    resize(&mut terminal, 80, 1);
    let column = terminal
        .snapshot()
        .map(|content| content.cursor.point.column);
    assert_eq!(
        column.ok(),
        Some(23),
        "the reflow must leave the cursor after the text"
    );

    for line_feed in 0..2_000 {
        feed(&mut terminal, b"\nb\x08");
        assert_bold_link(
            &mut terminal,
            Point::new(0, 23),
            'b',
            Color::Default,
            &format!("line feed {line_feed}"),
        );
    }
    assert_eq!(plain_text(&mut terminal), format!("{}b", " ".repeat(23)));
}

fn cursor_style_and_link(color: u16, link: u16) -> String {
    format!("\x1b[1;38;5;{color}m\x1b]8;id=fixture-{link};{LINK}\x1b\\")
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn lines_until_the_first_page_is_pruned() -> usize {
    let mut terminal = terminal_with_history(10, 3, 0);
    let mut history = 0;
    for line in 0..100_000 {
        feed(&mut terminal, b"x\r\n");
        let now = terminal.snapshot().unwrap().history_size;
        if now < history {
            return line;
        }
        history = now;
    }
    unreachable!("a zero line scrollback limit never pruned a page")
}

fn clear_with_the_cursor_on_the_pruned_page(clear: &[u8], at_prompt: bool) {
    let boundary = lines_until_the_first_page_is_pruned();
    let prompt: &[u8] = if at_prompt { b"\x1b]133;A\x1b\\" } else { b"" };
    for lines in boundary.saturating_sub(16)..=boundary + 16 {
        let mut terminal = terminal_with_history(10, 3, 0);
        for line in 0..lines {
            let style = cursor_style_and_link((16 + line % 96) as u16, (line % 8) as u16);
            feed(
                &mut terminal,
                format!("{style}x{PLAIN_TEXT}\r\n").as_bytes(),
            );
        }
        feed(&mut terminal, prompt);
        feed(&mut terminal, b"x\x1b[H");
        feed(&mut terminal, cursor_style_and_link(201, 8).as_bytes());
        feed(&mut terminal, clear);
        feed(&mut terminal, b"y");
        assert_bold_link(
            &mut terminal,
            Point::new(0, 0),
            'y',
            Color::Palette(201),
            &format!("{lines} lines before the clear, page boundary at {boundary}"),
        );
    }
}

fn clear_ten_thousand_times(clear: &[u8], at_prompt: bool) {
    let mut terminal = terminal_with_history(10, 3, 0);
    let prompt: &[u8] = if at_prompt { b"\x1b]133;A\x1b\\" } else { b"" };
    for iteration in 0..10_000u16 {
        let color = 16 + iteration % 96;
        feed(&mut terminal, b"\x1b[H");
        feed(
            &mut terminal,
            cursor_style_and_link(color, iteration % 8).as_bytes(),
        );
        feed(&mut terminal, clear);
        feed(&mut terminal, prompt);
        feed(&mut terminal, b"y");
        assert_bold_link(
            &mut terminal,
            Point::new(0, 0),
            'y',
            Color::Palette(color as u8),
            &format!("clear {iteration}"),
        );
        feed(&mut terminal, PLAIN);
        let _ = terminal.drain_events();
    }
    let history = terminal.snapshot().map(|content| content.history_size);
    assert!(
        history.as_ref().is_ok_and(|rows| *rows > 0),
        "every clear must scroll the screen into history, got {history:?}"
    );
}

#[test]
fn upstream_e257bb45e_a_scroll_clear_prune_keeps_the_cursor_style_and_link() {
    clear_with_the_cursor_on_the_pruned_page(b"\x1b[22J", false);
    clear_ten_thousand_times(b"\x1b[22J", false);
}

#[test]
fn upstream_e257bb45e_an_erase_display_at_a_prompt_prune_keeps_the_cursor_style_and_link() {
    clear_with_the_cursor_on_the_pruned_page(b"\x1b[2J", true);
    clear_ten_thousand_times(b"\x1b[2J", true);
}

#[test]
fn upstream_0af5c9e55_narrowing_the_widest_terminal_keeps_its_rows_consistent() {
    for iteration in 0..1_000 {
        let mut terminal = terminal_with_history(WIDEST, 1, 0);
        feed(&mut terminal, b"abcd");
        assert_narrowed_to_two_columns(&mut terminal, &format!("iteration {iteration}"));
    }
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn upstream_0af5c9e55_narrowing_a_decoded_widest_snapshot_keeps_its_rows_consistent() {
    let mut source = terminal_with_history(WIDEST, 1, 0);
    feed(&mut source, b"abcd");
    let encoded = source.encode_snapshot().unwrap();
    for iteration in 0..1_000 {
        let mut decoder = SnapshotDecoder::from_bytes(&encoded).unwrap();
        decoder
            .decode(SnapshotRestore {
                cell_width: 8,
                cell_height: 16,
                max_scrollback: 0,
                appearance: TerminalAppearance::default(),
            })
            .unwrap();
        let mut restored = decoder.into_terminal().unwrap();
        assert_eq!(restored.snapshot().unwrap().cols, WIDEST);
        assert_narrowed_to_two_columns(&mut restored, &format!("iteration {iteration}"));
    }
}

#[test]
fn upstream_0af5c9e55_the_widest_page_is_the_widest_window_the_wrapper_accepts() {
    assert!(WindowSize::new(WIDEST, 1, 8, 16).is_ok());
    assert!(matches!(
        WindowSize::new(WIDEST + 1, 1, 8, 16),
        Err(GhosttyError::InvalidDimensions { .. })
    ));
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn upstream_f473f3109_decstr_restores_the_configured_palette() {
    let mut theme: [Rgb; PALETTE_LEN] = default_palette();
    theme[1] = Rgb {
        r: 0x12,
        g: 0x34,
        b: 0x56,
    };
    let mut terminal = terminal(20, 2);
    terminal.set_palette(&theme).unwrap();
    let themed = palette_reply(&mut terminal, 1);

    let mut stock = self::terminal(20, 2);
    assert_ne!(
        palette_reply(&mut stock, 1),
        themed,
        "the fixture theme must differ from Ghostty's default"
    );

    feed(&mut terminal, b"\x1b]4;1;rgb:ab/cd/ef\x1b\\");
    assert_ne!(palette_reply(&mut terminal, 1), themed);

    feed(&mut terminal, b"\x1b[!p");
    assert_eq!(palette_reply(&mut terminal, 1), themed);
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn upstream_f473f3109_decstr_resets_modes_and_margins_but_keeps_the_text() {
    let mut terminal = terminal(10, 5);
    feed(&mut terminal, b"keep\x1b[?6h\x1b[4h\x1b[?7l\x1b[2;3r");
    assert_eq!(mode_report(&mut terminal, "?6"), b"\x1b[?6;1$y");
    assert_eq!(mode_report(&mut terminal, "4"), b"\x1b[4;1$y");
    assert_eq!(mode_report(&mut terminal, "?7"), b"\x1b[?7;2$y");
    let text = plain_text(&mut terminal);

    feed(&mut terminal, b"\x1b[!p");
    assert_eq!(plain_text(&mut terminal), text);
    assert_eq!(mode_report(&mut terminal, "?6"), b"\x1b[?6;2$y");
    assert_eq!(mode_report(&mut terminal, "4"), b"\x1b[4;2$y");
    assert_eq!(mode_report(&mut terminal, "?7"), b"\x1b[?7;1$y");

    feed(&mut terminal, b"\x1b[5;1H\n");
    assert_eq!(
        plain_text(&mut terminal),
        "",
        "a line feed on the last row must scroll the whole screen"
    );
    assert_eq!(terminal.snapshot().unwrap().history_size, 1);
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn upstream_ff779564c_erase_display_at_a_prompt_above_blank_rows_scrolls_only_text() {
    let mut terminal = terminal(10, 15);
    for line in 1..=10 {
        feed(&mut terminal, format!("out-{line}\r\n").as_bytes());
    }
    feed(&mut terminal, b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\");
    feed(&mut terminal, b"\x1b[H\x1b[2J");

    let content = terminal.snapshot().unwrap();
    assert_eq!(
        content.history_size, 11,
        "the ten output rows and the prompt scroll, the blank rows below do not"
    );
    assert_eq!(plain_text(&mut terminal), "");
    assert_eq!(search(&mut terminal, "out-3").total_matches, 1);
    assert_eq!(search(&mut terminal, "out-10").total_matches, 1);
}

#[test]
fn csi_19_t_reports_the_screen_size_in_characters() {
    let mut terminal = terminal(80, 24);
    let _ = terminal.drain_events();
    feed(&mut terminal, b"\x1b[19t");
    assert_eq!(pty_writes(&mut terminal), b"\x1b[9;24;80t");
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn csi_8_t_never_resizes_a_pane() {
    let mut terminal = terminal(80, 24);
    let _ = terminal.drain_events();
    feed(&mut terminal, b"\x1b[8;50;100t");
    assert!(pty_writes(&mut terminal).is_empty());
    let content = terminal.snapshot().unwrap();
    assert_eq!((content.cols, content.rows), (80, 24));
}

#[test]
fn upstream_d432b01a3_decrqm_reports_an_unsupported_ansi_mode_as_permanently_reset() {
    let mut terminal = terminal(20, 2);
    assert_eq!(mode_report(&mut terminal, "5"), b"\x1b[5;4$y");
}

#[test]
fn upstream_5c9a3d8a9_an_unbracketed_lone_carriage_return_paste_is_refused() {
    let mut terminal = terminal(20, 2);
    let _ = terminal.drain_events();
    let result = terminal.paste(
        &[PasteRepresentation {
            mime: "text/plain;charset=utf-8",
            data: b"a\rb",
        }],
        ClipboardLocation::Standard,
        false,
    );
    assert!(
        matches!(result, Err(GhosttyError::UnsafePaste)),
        "{result:?}"
    );
    assert!(pty_writes(&mut terminal).is_empty());
}
