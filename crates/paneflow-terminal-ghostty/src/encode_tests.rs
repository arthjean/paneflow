use paneflow_libghostty_sys as sys;

use crate::limits::MAX_SCROLLBACK_ROWS;
use crate::{DisplayTerminal, GhosttyError, Key, KeyAction, KeyInput, Modifiers, WindowSize};

#[test]
fn key_text_pointer_is_cleared_after_encoding() {
    let mut terminal =
        DisplayTerminal::new(WindowSize::new(80, 24, 8, 16).unwrap(), 10_000).unwrap();
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
        DisplayTerminal::new(invalid, 10_000),
        Err(GhosttyError::InvalidDimensions { .. })
    ));
    assert!(matches!(
        DisplayTerminal::new(
            WindowSize::new(80, 24, 8, 16).unwrap(),
            MAX_SCROLLBACK_ROWS + 1
        ),
        Err(GhosttyError::LimitExceeded { .. })
    ));
}
