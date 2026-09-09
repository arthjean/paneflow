use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    AnyElement, AppContext, Bounds, Context, Entity, EventEmitter, FocusHandle, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Render, StatefulInteractiveElement, Styled,
    Window, canvas, deferred, div, prelude::FluentBuilder, px, svg,
};

use super::view::CodeView;
use crate::settings::components::{menu_surface, select_item};
use crate::ui_primitives::{AnimatedHoverExt, TooltipDelayExt, text_tooltip};

const MENU_WIDTH: f32 = 180.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EditorDisplay {
    pub(crate) minimap: bool,
    pub(crate) scrollbar: bool,
}

impl EditorDisplay {
    pub(crate) fn from_config(config: &paneflow_config::schema::EditorDisplayConfig) -> Self {
        Self {
            minimap: config.minimap_enabled(),
            scrollbar: config.scrollbar_enabled(),
        }
    }

    pub(crate) fn to_config_value(self) -> serde_json::Value {
        serde_json::json!({
            "minimap": self.minimap,
            "scrollbar": self.scrollbar,
        })
    }
}

static EDITOR_MINIMAP: AtomicBool = AtomicBool::new(false);
static EDITOR_SCROLLBAR: AtomicBool = AtomicBool::new(true);

pub(crate) fn set_editor_display(display: EditorDisplay) {
    EDITOR_MINIMAP.store(display.minimap, Ordering::Relaxed);
    EDITOR_SCROLLBAR.store(display.scrollbar, Ordering::Relaxed);
}

pub(crate) fn editor_display() -> EditorDisplay {
    EditorDisplay {
        minimap: EDITOR_MINIMAP.load(Ordering::Relaxed),
        scrollbar: EDITOR_SCROLLBAR.load(Ordering::Relaxed),
    }
}

pub(crate) struct EditorControls {
    open: bool,
    selected: Option<usize>,
    trigger_bounds: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
    editor_focus: FocusHandle,
}

impl EditorControls {
    pub(crate) fn attach(editor_focus: FocusHandle, cx: &mut Context<CodeView>) -> Entity<Self> {
        let controls = cx.new(|cx| Self {
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
        let mut display = editor_display();
        if index == 0 {
            display.minimap = !display.minimap;
        } else {
            display.scrollbar = !display.scrollbar;
        }
        cx.emit(display);
        self.close(true, window, cx);
    }

    fn menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let display = editor_display();
        let mut menu = menu_surface(div().id("code-editor-controls-menu"), ui)
            .role(gpui::Role::Menu)
            .aria_label("Editor Controls")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap(px(1.))
            .p(px(4.))
            .w(px(MENU_WIDTH))
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
            ("Minimap", display.minimap),
            ("Scrollbar", display.scrollbar),
        ]
        .into_iter()
        .enumerate()
        {
            menu = menu.child(
                select_item(
                    ("code-editor-control", index),
                    self.selected == Some(index),
                    ui,
                )
                .role(gpui::Role::MenuItemCheckBox)
                .aria_label(label)
                .aria_toggled(if checked {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .aria_selected(self.selected == Some(index))
                .on_hover(cx.listener(move |this, hovered, _, cx| {
                    if *hovered {
                        this.selected = Some(index);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.choose(index, window, cx);
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_size(px(13.))
                        .text_color(ui.text)
                        .child(label),
                )
                .child(div().w(px(14.)).flex_none().child(if checked {
                    svg()
                        .size(px(13.))
                        .path("icons/check.svg")
                        .text_color(ui.text)
                        .into_any_element()
                } else {
                    div().size(px(13.)).into_any_element()
                })),
            );
        }

        deferred(crate::ui_primitives::menu_reveal(
            "code-editor-controls-menu-reveal",
            div()
                .absolute()
                .top(px(32.))
                .right_0()
                .occlude()
                .child(menu),
        ))
        .with_priority(3)
        .into_any_element()
    }
}

impl EventEmitter<EditorDisplay> for EditorControls {}

impl Render for EditorControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.open && !self.focus.is_focused(window) {
            self.open = false;
        }
        let ui = crate::theme::ui_colors();
        let hover = crate::app::constants::sidebar_tab_hover_background();
        let trigger_bounds = self.trigger_bounds.clone();
        div()
            .id("code-editor-controls")
            .role(gpui::Role::Button)
            .aria_label("Editor Controls")
            .aria_expanded(self.open)
            .relative()
            .flex_none()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .animated_hover_bg(
                if self.open {
                    hover
                } else {
                    gpui::transparent_black()
                },
                hover,
            )
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
                    .size(px(16.))
                    .path("icons/editor-controls.svg")
                    .text_color(if self.open { ui.text } else { ui.muted }),
            )
            .when(self.open, |button| button.child(self.menu(cx)))
    }
}
