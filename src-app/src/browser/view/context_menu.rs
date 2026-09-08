use super::super::page::ContextMenuItem;
use super::*;
use gpui::{anchored, point};
use paneflow_browser_protocol::EditAction;

pub(super) struct ContextMenuState {
    pub request: u64,
    position: gpui::Point<Pixels>,
    items: Vec<ContextMenuItem>,
    selected: Option<usize>,
}

impl BrowserView {
    pub(super) fn receive_context_menu(
        &mut self,
        request: u64,
        x: i32,
        y: i32,
        items: Vec<ContextMenuItem>,
        cx: &mut Context<Self>,
    ) {
        if !self.interaction.menu_requested
            || !self.visible
            || self.interaction.focused != Some(true)
        {
            self.input(
                InputEvent::ContextMenu {
                    request,
                    command: None,
                },
                cx,
            );
            return;
        }
        self.interaction.menu_requested = false;
        let selected = items.iter().position(|item| item.enabled);
        self.interaction.menu = Some(ContextMenuState {
            request,
            position: self.viewport_origin() + point(px(x as f32), px(y as f32)),
            items,
            selected,
        });
        cx.notify();
    }

    pub(super) fn dismiss_context_menu(&mut self, cx: &mut Context<Self>) {
        self.interaction.menu_requested = false;
        if let Some(menu) = self.interaction.menu.take() {
            self.input(
                InputEvent::ContextMenu {
                    request: menu.request,
                    command: None,
                },
                cx,
            );
            cx.notify();
        }
    }

    fn choose_context_item(&mut self, command: i32, cx: &mut Context<Self>) {
        let Some(menu) = self.interaction.menu.take() else {
            return;
        };
        let allowed = menu
            .items
            .iter()
            .any(|item| item.command == command && item.enabled);
        if !allowed {
            self.input(
                InputEvent::ContextMenu {
                    request: menu.request,
                    command: None,
                },
                cx,
            );
            cx.notify();
            return;
        }
        let native = match command {
            112 => {
                self.edit_document(EditAction::Cut, cx);
                false
            }
            113 => {
                self.edit_document(EditAction::Copy, cx);
                false
            }
            114 | 115 => {
                self.paste_from_system(cx);
                false
            }
            _ => true,
        };
        self.input(
            InputEvent::ContextMenu {
                request: menu.request,
                command: native.then_some(command),
            },
            cx,
        );
        cx.notify();
    }

    pub(super) fn context_menu_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.interaction.menu.as_mut() else {
            return false;
        };
        match key {
            "up" | "down" => {
                let count = menu.items.len();
                let start = menu.selected.unwrap_or(if key == "up" {
                    0
                } else {
                    count.saturating_sub(1)
                });
                for offset in 1..=count {
                    let index = if key == "up" {
                        (start + count - offset) % count
                    } else {
                        (start + offset) % count
                    };
                    if menu.items[index].enabled {
                        menu.selected = Some(index);
                        break;
                    }
                }
                cx.notify();
            }
            "enter" | "space" => {
                if let Some(item) = menu.selected.and_then(|index| menu.items.get(index)) {
                    let command = item.command;
                    self.choose_context_item(command, cx);
                }
            }
            "escape" | "tab" => self.dismiss_context_menu(cx),
            _ => {}
        }
        true
    }

    pub(super) fn render_context_menu(
        &self,
        ui: crate::theme::UiColors,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.web_dialog.is_some() {
            return None;
        }
        let state = self.interaction.menu.as_ref()?;
        let viewport = self.viewport?;
        let height = px(state.items.len() as f32 * 28. + 10.).min(viewport.size.height);
        let width = px(MENU_WIDTH).min(viewport.size.width);
        let position = gpui::point(
            state
                .position
                .x
                .max(viewport.left())
                .min(viewport.right() - width),
            state
                .position
                .y
                .max(viewport.top())
                .min(viewport.bottom() - height),
        );
        let mut menu = menu_surface(div().id("browser-document-menu"), ui)
            .flex()
            .flex_col()
            .p(px(4.))
            .w(width)
            .max_h(height)
            .overflow_y_scroll()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|view, _, _, cx| view.dismiss_context_menu(cx)));
        for (index, item) in state.items.iter().enumerate() {
            let command = item.command;
            let label = item
                .label
                .replace("&&", "\u{0}")
                .replace('&', "")
                .replace('\u{0}', "&");
            let label = if item.checked {
                format!("✓ {label}")
            } else {
                label
            };
            let row = select_item(
                SharedString::from(format!("browser-document-command-{command}")),
                state.selected == Some(index),
                ui,
            )
            .h(px(28.))
            .flex_none()
            .px(px(8.))
            .text_color(if item.enabled { ui.text } else { ui.muted })
            .child(
                div()
                    .text_size(crate::ui_primitives::BODY)
                    .whitespace_nowrap()
                    .overflow_x_hidden()
                    .text_ellipsis()
                    .child(label),
            )
            .when(item.enabled, |row| {
                row.cursor(CursorStyle::PointingHand)
                    .on_mouse_move(cx.listener(move |view, _, _, cx| {
                        if let Some(menu) = &mut view.interaction.menu
                            && menu.selected != Some(index)
                        {
                            menu.selected = Some(index);
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }))
                    .on_click(
                        cx.listener(move |view, _, _, cx| view.choose_context_item(command, cx)),
                    )
            });
            menu = menu.child(row);
        }
        Some(
            deferred(anchored().position(position).snap_to_window().child(menu))
                .priority(3)
                .into_any_element(),
        )
    }
}
