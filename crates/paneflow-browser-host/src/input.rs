use cef::*;
use paneflow_browser_proto::surface::{Cursor, Input, KeyKind, MouseButton};

const NO_RANGE: Range = Range {
    from: u32::MAX,
    to: u32::MAX,
};

pub fn deliver(host: &BrowserHost, input: Input) {
    match input {
        Input::MouseMove { x, y, modifiers } => {
            host.send_mouse_move_event(Some(&mouse(x, y, modifiers.0)), 0);
        }
        Input::MouseLeave { x, y, modifiers } => {
            host.send_mouse_move_event(Some(&mouse(x, y, modifiers.0)), 1);
        }
        Input::MouseButton {
            x,
            y,
            button,
            pressed,
            clicks,
            modifiers,
        } => {
            let kind = match button {
                MouseButton::Left => MouseButtonType::LEFT,
                MouseButton::Middle => MouseButtonType::MIDDLE,
                MouseButton::Right => MouseButtonType::RIGHT,
            };
            host.send_mouse_click_event(
                Some(&mouse(x, y, modifiers.0)),
                kind,
                i32::from(!pressed),
                i32::try_from(clicks.clamp(1, 3)).unwrap_or(1),
            );
        }
        Input::Wheel {
            x,
            y,
            delta_x,
            delta_y,
            modifiers,
        } => {
            host.send_mouse_wheel_event(Some(&mouse(x, y, modifiers.0)), delta_x, delta_y);
        }
        Input::Key {
            kind,
            windows_key_code,
            native_key_code,
            character,
            unmodified_character,
            modifiers,
        } => {
            let event = KeyEvent {
                size: std::mem::size_of::<KeyEvent>(),
                type_: match kind {
                    KeyKind::RawDown => KeyEventType::RAWKEYDOWN,
                    KeyKind::Up => KeyEventType::KEYUP,
                    KeyKind::Char => KeyEventType::CHAR,
                },
                modifiers: modifiers.0,
                windows_key_code,
                native_key_code,
                is_system_key: 0,
                character,
                unmodified_character,
                focus_on_editable_field: 0,
            };
            host.send_key_event(Some(&event));
        }
        Input::ImeSetComposition {
            text,
            selection_start,
            selection_end,
        } => {
            let text = CefString::from(text.as_str());
            let selection = Range {
                from: selection_start,
                to: selection_end,
            };
            host.ime_set_composition(Some(&text), Some(&[]), Some(&NO_RANGE), Some(&selection));
        }
        Input::ImeCommit { text } => {
            let text = CefString::from(text.as_str());
            host.ime_commit_text(Some(&text), Some(&NO_RANGE), 0);
        }
        Input::ImeFinish => host.ime_finish_composing_text(0),
        Input::ImeCancel => host.ime_cancel_composition(),
        Input::Focus { focused } => host.set_focus(i32::from(focused)),
        Input::CaptureLost => host.send_capture_lost_event(),
    }
}

fn mouse(x: i32, y: i32, modifiers: u32) -> MouseEvent {
    MouseEvent { x, y, modifiers }
}

pub fn cursor(kind: CursorType) -> Cursor {
    let table = [
        (CursorType::POINTER, Cursor::Default),
        (CursorType::CROSS, Cursor::Crosshair),
        (CursorType::HAND, Cursor::Pointer),
        (CursorType::IBEAM, Cursor::Text),
        (CursorType::VERTICALTEXT, Cursor::Text),
        (CursorType::WAIT, Cursor::Wait),
        (CursorType::PROGRESS, Cursor::Progress),
        (CursorType::HELP, Cursor::Help),
        (CursorType::EASTRESIZE, Cursor::ResizeEast),
        (CursorType::WESTRESIZE, Cursor::ResizeWest),
        (CursorType::NORTHRESIZE, Cursor::ResizeNorth),
        (CursorType::SOUTHRESIZE, Cursor::ResizeSouth),
        (CursorType::EASTWESTRESIZE, Cursor::ResizeEastWest),
        (CursorType::NORTHSOUTHRESIZE, Cursor::ResizeNorthSouth),
        (
            CursorType::NORTHEASTRESIZE,
            Cursor::ResizeNorthEastSouthWest,
        ),
        (
            CursorType::SOUTHWESTRESIZE,
            Cursor::ResizeNorthEastSouthWest,
        ),
        (
            CursorType::NORTHEASTSOUTHWESTRESIZE,
            Cursor::ResizeNorthEastSouthWest,
        ),
        (
            CursorType::NORTHWESTRESIZE,
            Cursor::ResizeNorthWestSouthEast,
        ),
        (
            CursorType::SOUTHEASTRESIZE,
            Cursor::ResizeNorthWestSouthEast,
        ),
        (
            CursorType::NORTHWESTSOUTHEASTRESIZE,
            Cursor::ResizeNorthWestSouthEast,
        ),
        (CursorType::COLUMNRESIZE, Cursor::ResizeColumn),
        (CursorType::ROWRESIZE, Cursor::ResizeRow),
        (CursorType::MOVE, Cursor::Move),
        (CursorType::NOTALLOWED, Cursor::NotAllowed),
        (CursorType::NODROP, Cursor::NoDrop),
        (CursorType::GRAB, Cursor::Grab),
        (CursorType::GRABBING, Cursor::Grabbing),
        (CursorType::ZOOMIN, Cursor::ZoomIn),
        (CursorType::ZOOMOUT, Cursor::ZoomOut),
        (CursorType::CONTEXTMENU, Cursor::ContextMenu),
        (CursorType::COPY, Cursor::Copy),
        (CursorType::ALIAS, Cursor::Alias),
        (CursorType::NONE, Cursor::None),
    ];
    table
        .iter()
        .find(|(candidate, _)| *candidate == kind)
        .map_or(Cursor::Default, |(_, cursor)| *cursor)
}
