use paneflow_libghostty_sys as sys;

use crate::limits::MAX_SCROLLBACK_ROWS;
use crate::{
    DisplayTerminal, GhosttyError, Key, KeyAction, KeyInput, Modifiers, MouseAction, MouseButton,
    MouseInput, TerminalAppearance, WindowSize,
};

fn click(action: MouseAction, x: f32, y: f32) -> MouseInput {
    MouseInput {
        action,
        button: Some(MouseButton::Left),
        modifiers: Modifiers::empty(),
        x,
        y,
        screen_width: 80 * 8,
        screen_height: 24 * 16,
        padding_top: 0,
        padding_bottom: 0,
        padding_left: 0,
        padding_right: 0,
        any_button_pressed: matches!(action, MouseAction::Press),
    }
}

#[test]
fn the_mouse_encoder_follows_every_tracking_and_format_mode_switch() {
    let mut terminal = DisplayTerminal::new(
        WindowSize::new(80, 24, 8, 16).unwrap(),
        1_000,
        TerminalAppearance::default(),
    )
    .unwrap();

    terminal.feed(b"\x1b[?9h").unwrap();
    assert!(
        !terminal
            .encode_mouse(click(MouseAction::Press, 12.0, 20.0))
            .unwrap()
            .is_empty()
    );
    assert!(
        terminal
            .encode_mouse(click(MouseAction::Release, 12.0, 20.0))
            .unwrap()
            .is_empty(),
        "X10 tracking reports presses only"
    );

    terminal.feed(b"\x1b[?9l\x1b[?1000h").unwrap();
    assert!(
        !terminal
            .encode_mouse(click(MouseAction::Release, 12.0, 20.0))
            .unwrap()
            .is_empty(),
        "normal tracking reports releases once the encoder follows the switch"
    );

    terminal.feed(b"\x1b[?1006h").unwrap();
    assert_eq!(
        terminal
            .encode_mouse(click(MouseAction::Press, 12.0, 20.0))
            .unwrap(),
        b"\x1b[<0;2;2M"
    );
    terminal.feed(b"\x1b[?1016h").unwrap();
    assert_eq!(
        terminal
            .encode_mouse(click(MouseAction::Press, 12.0, 20.0))
            .unwrap(),
        b"\x1b[<0;12;20M"
    );
    terminal.feed(b"\x1b[?1016l\x1b[?1006l\x1b[?1015h").unwrap();
    assert_eq!(
        terminal
            .encode_mouse(click(MouseAction::Press, 12.0, 20.0))
            .unwrap(),
        b"\x1b[32;2;2M"
    );
}

#[test]
fn key_text_pointer_is_cleared_after_encoding() {
    let mut terminal = DisplayTerminal::new(
        WindowSize::new(80, 24, 8, 16).unwrap(),
        10_000,
        TerminalAppearance::default(),
    )
    .unwrap();
    terminal
        .encode_key(&KeyInput {
            key: Key::Character('x'),
            action: KeyAction::Press,
            modifiers: Modifiers::empty(),
            consumed_modifiers: Modifiers::empty(),
            text: "x".into(),
            unshifted_codepoint: Some('x'),
            composing: false,
        })
        .unwrap();

    let mut len = usize::MAX;
    let pointer = unsafe { sys::ghostty_key_event_get_utf8(terminal.key_event.raw(), &mut len) };
    assert!(pointer.is_null());
    assert_eq!(len, 0);
}

#[test]
fn constructor_revalidates_public_dimensions_and_scrollback_cap() {
    let invalid = WindowSize {
        cols: 0,
        rows: 24,
        cell_width: 8,
        cell_height: 16,
    };
    assert!(matches!(
        DisplayTerminal::new(invalid, 10_000, TerminalAppearance::default()),
        Err(GhosttyError::InvalidDimensions { .. })
    ));
    assert!(matches!(
        DisplayTerminal::new(
            WindowSize::new(80, 24, 8, 16).unwrap(),
            MAX_SCROLLBACK_ROWS + 1,
            TerminalAppearance::default(),
        ),
        Err(GhosttyError::LimitExceeded { .. })
    ));
}
