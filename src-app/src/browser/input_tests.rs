use std::{cell::RefCell, ops::Range, rc::Rc};

use gpui::{
    App, Bounds, Context, FocusHandle, InputHandler, IntoElement, KeyDownEvent, Keystroke,
    Modifiers, Pixels, Point, Render, TestAppContext, UTF16Selection, Window, canvas, div,
    prelude::*,
};
use paneflow_browser_protocol::{InputEvent, KeyKind};

use super::input::consume_key_down;

type Events = Rc<RefCell<Vec<InputEvent>>>;

struct InputHarness {
    focus: FocusHandle,
    events: Events,
}

impl Render for InputHarness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.focus.clone();
        let events = self.events.clone();
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                view.events.borrow_mut().extend(consume_key_down(event, cx));
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, (), window, cx| {
                        window.handle_input(&focus, RecordingInput(events.clone()), cx);
                    },
                )
                .size_full(),
            )
    }
}

struct RecordingInput(Events);

impl InputHandler for RecordingInput {
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut App,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _: &mut Window, _: &mut App) -> Option<Range<usize>> {
        None
    }

    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        Some(String::new())
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        _: &mut App,
    ) {
        self.0.borrow_mut().push(InputEvent::ImeCommit {
            replacement: None,
            text: text.to_string(),
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut App) {}

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<usize> {
        None
    }
}

fn characters(events: &[InputEvent]) -> String {
    let units: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            InputEvent::Key {
                kind: KeyKind::Char,
                character,
                ..
            } => Some(*character),
            _ => None,
        })
        .collect();
    String::from_utf16(&units).unwrap()
}

#[gpui::test]
fn printable_browser_key_does_not_also_commit_through_gpui_input(cx: &mut TestAppContext) {
    let events = Events::default();
    let recorded = events.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| InputHarness {
        focus: cx.focus_handle(),
        events,
    });
    cx.update(|window, cx| {
        let focus = view.read(cx).focus.clone();
        focus.focus(window, cx);
        window.draw(cx).clear(cx);
        assert!(window.dispatch_keystroke(Keystroke::parse("a").unwrap(), cx));
    });
    let events = recorded.borrow();
    assert_eq!(characters(&events), "a");
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0],
        InputEvent::Key {
            kind: KeyKind::RawDown,
            ..
        }
    ));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, InputEvent::ImeCommit { .. }))
    );
}

#[gpui::test]
fn browser_copy_shortcut_produces_no_character(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let events = consume_key_down(
            &KeyDownEvent {
                keystroke: Keystroke {
                    modifiers: Modifiers {
                        control: true,
                        ..Modifiers::default()
                    },
                    key: "c".into(),
                    key_char: Some("c".into()),
                },
                is_held: false,
                prefer_character_input: false,
            },
            cx,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(characters(&events), "");
    });
}

#[gpui::test]
fn browser_character_preference_preserves_altgr_and_astral_text(cx: &mut TestAppContext) {
    cx.update(|cx| {
        for text in ["@", "\u{1f600}"] {
            let events = consume_key_down(
                &KeyDownEvent {
                    keystroke: Keystroke {
                        modifiers: Modifiers {
                            control: true,
                            alt: true,
                            ..Modifiers::default()
                        },
                        key: "a".into(),
                        key_char: Some(text.into()),
                    },
                    is_held: false,
                    prefer_character_input: true,
                },
                cx,
            );
            assert_eq!(characters(&events), text);
        }
    });
}
