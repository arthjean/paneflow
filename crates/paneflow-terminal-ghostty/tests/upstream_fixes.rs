#![cfg(all(
    feature = "native",
    any(
        target_os = "linux",
        all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")
    )
))]

use paneflow_terminal_ghostty::{
    BackendEvent, DecodedImage, DisplayTerminal, GestureBehavior, GestureBehaviors, PALETTE_LEN,
    Point, PressOptions, Rgb, TerminalAppearance, WideCell, WindowSize, default_palette,
    set_png_decoder,
};

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
    DisplayTerminal::new(
        WindowSize::new(cols, rows, 8, 16).unwrap(),
        1_000,
        TerminalAppearance::default(),
    )
    .unwrap()
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

#[test]
fn upstream_e6db5b633_narrowing_a_wide_tail_survives_a_thousand_resizes() {
    let mut terminal = terminal(3, 3);
    for _ in 0..1_000 {
        feed(
            &mut terminal,
            "\x1b[?7l\x1b[Ha一\x1b[1;2H\x1b[@\x1b[1K".as_bytes(),
        );
        resize(&mut terminal, 2, 3);
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
