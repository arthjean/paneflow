use super::*;
use gpui::{DispatchPhase, MouseExitEvent, Subscription};
use paneflow_browser_protocol::EditAction;

#[derive(Default)]
pub(super) struct InteractionState {
    pub focused: Option<bool>,
    pub scroll: crate::browser::input::ScrollAccumulator,
    pub subscriptions: Vec<Subscription>,
    pub buttons: u32,
    pub point: gpui::Point<Pixels>,
    pub modifiers: gpui::Modifiers,
    pub sequence: u64,
    pub clipboard: Option<u64>,
    pub menu_requested: bool,
    pub menu: Option<super::context_menu::ContextMenuState>,
}

impl BrowserView {
    pub(super) fn sync_input_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.interaction.subscriptions.is_empty() {
            let focus = self.focus.clone();
            self.interaction.subscriptions = vec![
                cx.on_focus_in(&focus, window, |view, window, cx| {
                    view.sync_input_focus(window, cx)
                }),
                cx.on_focus_out(&focus, window, |view, _, window, cx| {
                    view.sync_input_focus(window, cx)
                }),
                cx.observe_window_activation(window, |view, window, cx| {
                    view.sync_input_focus(window, cx)
                }),
            ];
        }
        let focused = self.visible && window.is_window_active() && self.focus.is_focused(window);
        if self.interaction.focused == Some(focused) {
            return;
        }
        if !focused {
            self.release_input(cx);
        }
        if self.live.as_ref().and_then(LivePage::document).is_some()
            && self.state != SessionState::Starting
        {
            self.input(InputEvent::Focus { focused }, cx);
            self.interaction.focused = Some(focused);
            cx.notify();
        }
    }

    pub(super) fn release_input(&mut self, cx: &mut Context<Self>) {
        self.interaction.scroll = Default::default();
        self.interaction.clipboard = None;
        self.interaction.sequence = self.interaction.sequence.wrapping_add(1);
        self.dismiss_context_menu(cx);
        if self.interaction.buttons != 0 {
            let point = self.interaction.point;
            let held = self.interaction.modifiers;
            for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
                if self.interaction.buttons & button_modifier(button) != 0 {
                    self.interaction.buttons &= !button_modifier(button);
                    self.mouse_button_event(button, point, false, 1, &held, cx);
                }
            }
            self.interaction.buttons = 0;
            self.input(InputEvent::CaptureLost, cx);
        }
        self.ime_cancel(cx);
    }

    pub(super) fn pointer_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_context_menu(cx);
        self.menu_open = false;
        window.focus(&self.focus, cx);
        self.sync_input_focus(window, cx);
        self.interaction.menu_requested = event.button == MouseButton::Right;
        self.interaction.buttons |= button_modifier(event.button);
        self.interaction.point = event.position;
        self.interaction.modifiers = event.modifiers;
        self.mouse_button_event(
            event.button,
            event.position,
            true,
            event.click_count,
            &event.modifiers,
            cx,
        );
        cx.stop_propagation();
    }

    pub(super) fn pointer_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        self.interaction.point = event.position;
        self.interaction.modifiers = event.modifiers;
        let (x, y) = self.browser_position(event.position);
        self.input(
            InputEvent::MouseMove {
                x,
                y,
                modifiers: modifiers(&event.modifiers) | self.interaction.buttons,
            },
            cx,
        );
    }

    pub(super) fn install_pointer_capture(view: &Entity<Self>, window: &mut Window) {
        let moving = view.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            moving.update(cx, |view, cx| {
                if view.interaction.buttons == 0 {
                    return;
                }
                if event.pressed_button.is_none() {
                    view.release_input(cx);
                } else {
                    view.pointer_move(event, cx);
                }
                cx.stop_propagation();
            });
        });
        let releasing = view.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            releasing.update(cx, |view, cx| {
                if view.interaction.buttons & button_modifier(event.button) == 0 {
                    return;
                }
                view.interaction.buttons &= !button_modifier(event.button);
                view.mouse_button_event(
                    event.button,
                    event.position,
                    false,
                    event.click_count,
                    &event.modifiers,
                    cx,
                );
                cx.stop_propagation();
            });
        });
        let leaving = view.clone();
        window.on_mouse_event(move |event: &MouseExitEvent, phase, _, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            leaving.update(cx, |view, cx| {
                if view.interaction.buttons != 0 {
                    view.release_input(cx);
                }
                let (x, y) = view.browser_position(event.position);
                view.input(
                    InputEvent::MouseLeave {
                        x,
                        y,
                        modifiers: modifiers(&event.modifiers),
                    },
                    cx,
                );
            });
        });
    }

    pub(super) fn document_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.context_menu_key(&event.keystroke.key, cx) {
            cx.stop_propagation();
            return;
        }
        if self.ime_composing {
            return;
        }
        let held = event.keystroke.modifiers;
        if held.control && !held.alt && !held.platform && !event.prefer_character_input {
            let action = match event.keystroke.key.as_str() {
                "a" => Some(EditAction::SelectAll),
                "c" | "insert" if !held.shift => Some(EditAction::Copy),
                "x" => Some(EditAction::Cut),
                "v" => {
                    self.paste_from_system(cx);
                    cx.stop_propagation();
                    return;
                }
                "z" if held.shift => Some(EditAction::Redo),
                "z" => Some(EditAction::Undo),
                "y" => Some(EditAction::Redo),
                _ => None,
            };
            if let Some(action) = action {
                self.edit_document(action, cx);
                cx.stop_propagation();
                return;
            }
        }
        if event.keystroke.key == "f10" && held.shift {
            self.interaction.menu_requested = true;
        }
        for input in super::super::input::consume_key_down(event, cx) {
            self.input(input, cx);
        }
    }
}
