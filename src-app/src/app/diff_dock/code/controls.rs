use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, AppContext, Bounds, BoxShadow, Context, CursorStyle, Entity, FocusHandle,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Render,
    StatefulInteractiveElement, Styled, Window, canvas, deferred, div, hsla,
    prelude::FluentBuilder, px, svg,
};

use super::view::CodeView;
use crate::ui_primitives::{TooltipDelayExt, text_tooltip};

#[derive(Clone, Copy)]
pub(crate) struct EditorDisplay {
    pub(crate) minimap: bool,
    pub(crate) scrollbar: bool,
}

impl Default for EditorDisplay {
    fn default() -> Self {
        Self {
            minimap: false,
            scrollbar: true,
        }
    }
}

pub(crate) struct EditorControls {
    pub(crate) display: EditorDisplay,
    open: bool,
    selected: Option<usize>,
    trigger_bounds: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
    editor_focus: FocusHandle,
}

impl EditorControls {
    pub(crate) fn attach(editor_focus: FocusHandle, cx: &mut Context<CodeView>) -> Entity<Self> {
        let controls = cx.new(|cx| Self {
            display: EditorDisplay::default(),
            open: false,
            selected: None,
            trigger_bounds: Rc::default(),
            focus: cx.focus_handle(),
            editor_focus,
        });
        cx.observe(&controls, |_, _, cx| cx.notify()).detach();
        controls
    }

    fn close(&mut self, restore_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        if restore_focus {
            window.focus(&self.editor_focus, cx);
        }
        cx.notify();
    }

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == 0 {
            self.display.minimap = !self.display.minimap;
        } else {
            self.display.scrollbar = !self.display.scrollbar;
        }
        self.close(true, window, cx);
    }

    fn menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let mut menu = div()
            .id("code-editor-controls-menu")
            .role(gpui::Role::Menu)
            .aria_label("Editor Controls")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .w(px(200.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(ui.border)
            .bg(ui.overlay)
            .font_family(".ZedSans")
            .text_size(px(14.))
            .line_height(px(14. * 1.618))
            .text_color(ui.text)
            .shadow(vec![
                BoxShadow::new(px(0.), px(2.), hsla(0., 0., 0., 0.12)).blur_radius(px(3.)),
                BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., 0.06)),
            ])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if !this.trigger_bounds.get().contains(&event.position) {
                        this.close(true, window, cx);
                    }
                }),
            )
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.close(true, window, cx),
                    "up" | "down" | "tab" => {
                        this.selected = Some(this.selected.map_or(0, |index| 1 - index));
                        cx.notify();
                    }
                    "enter" | "space" => this.choose(this.selected.unwrap_or(0), window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }));

        for (index, (label, checked)) in [
            ("Minimap", self.display.minimap),
            ("Scrollbar", self.display.scrollbar),
        ]
        .into_iter()
        .enumerate()
        {
            menu = menu.child(
                div()
                    .id(("code-editor-control", index))
                    .role(gpui::Role::MenuItemCheckBox)
                    .aria_label(label)
                    .aria_toggled(if checked {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .aria_selected(self.selected == Some(index))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .mx(px(4.))
                    .px(px(6.))
                    .rounded(px(2.))
                    .cursor(CursorStyle::PointingHand)
                    .when(self.selected == Some(index), |row| {
                        row.bg(ui.text.opacity(0.06))
                    })
                    .hover(|row| row.bg(ui.text.opacity(0.08)))
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        if *hovered {
                            this.selected = Some(index);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose(index, window, cx);
                    }))
                    .child(div().size(px(14.)).flex_none().when(checked, |slot| {
                        slot.child(
                            svg()
                                .size_full()
                                .path("icons/zed-check.svg")
                                .text_color(ui.accent),
                        )
                    }))
                    .child(label),
            );
        }

        deferred(
            div()
                .absolute()
                .top(px(22.))
                .right_0()
                .occlude()
                .child(menu),
        )
        .with_priority(3)
        .into_any_element()
    }
}

impl Render for EditorControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.open && !self.focus.is_focused(window) {
            self.open = false;
        }
        let ui = crate::theme::ui_colors();
        let trigger_bounds = self.trigger_bounds.clone();
        div()
            .id("code-editor-controls")
            .role(gpui::Role::Button)
            .aria_label("Editor Controls")
            .aria_expanded(self.open)
            .relative()
            .flex_none()
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .cursor(CursorStyle::PointingHand)
            .when(self.open, |button| button.bg(ui.text.opacity(0.08)))
            .hover(|button| button.bg(ui.text.opacity(0.08)))
            .delayed_tooltip(text_tooltip("Editor Controls"))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| {
                if this.open {
                    this.close(true, window, cx);
                } else {
                    this.open = true;
                    this.selected = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .child(
                canvas(
                    move |bounds, _, _| trigger_bounds.set(bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                svg()
                    .size(px(14.))
                    .path("icons/editor-controls.svg")
                    .text_color(if self.open { ui.accent } else { ui.muted }),
            )
            .when(self.open, |button| button.child(self.menu(cx)))
    }
}
