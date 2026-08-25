//! Manage-custom-buttons modal - opened from the workspace card context
//! menu in the sidebar. Lets the user add / edit / delete user-defined
//! command buttons that appear in the workspace's tab bar, to the right
//! of the built-in defaults (Claude / Codex).
//!
//! Modal overlay pattern follows the theme picker (`app/theme_picker.rs`):
//! `deferred()` backdrop + centered card + focus-handled key input.
//!
//! Form text inputs are full cursor-aware `widgets::text_input::TextInput`
//! instances, so arrow-key navigation, mouse click/drag selection, Home/End,
//! Ctrl-A/C/V/X and IME all work as in any native single-line input.

use gpui::{
    AnyElement, ClickEvent, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Point, SharedString,
    Styled, Window, deferred, div, hsla, prelude::*, px, svg,
};
use paneflow_config::schema::ButtonCommand;

use crate::PaneFlowApp;
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};
use crate::widgets::scrollbar;
use crate::widgets::text_input::TextInput;

// ---------------------------------------------------------------------------
// Icon picker inventory - curated subset of the bundled tabler icons that
// make sense for dev / terminal / git / deploy / ops commands. Claude and
// Codex colour icons are intentionally excluded (reserved for the defaults).
// ---------------------------------------------------------------------------

pub(crate) const AVAILABLE_ICONS: &[&str] = &[
    "icons/player-play.svg",
    "icons/player-stop.svg",
    "icons/refresh.svg",
    "icons/rocket.svg",
    "icons/package.svg",
    "icons/flask.svg",
    "icons/bug.svg",
    "icons/brand-git.svg",
    "icons/git-branch.svg",
    "icons/git-pull-request.svg",
    "icons/database.svg",
    "icons/server.svg",
    "icons/cloud-upload.svg",
    "icons/download.svg",
    "icons/bolt.svg",
    "icons/flame.svg",
    "icons/brand-docker.svg",
    "icons/tool.svg",
    "icons/hammer.svg",
    "icons/code.svg",
    "icons/world.svg",
    "icons/eye.svg",
    "icons/checks.svg",
    "icons/file-text.svg",
    "icons/list.svg",
    "icons/terminal.svg",
    "icons/settings.svg",
];

fn default_icon() -> String {
    // US-058: `.first()` instead of `[0]` - the const is non-empty today, but a
    // future trim of the list must degrade gracefully, not panic.
    AVAILABLE_ICONS
        .first()
        .copied()
        .unwrap_or("icons/player-play.svg")
        .to_string()
}

// ---------------------------------------------------------------------------
// Modal state
// ---------------------------------------------------------------------------

pub(crate) struct CustomButtonsModal {
    /// Target workspace, identified by stable id (survives reorder / session
    /// reload; we lookup the index lazily when we need to mutate).
    pub workspace_id: u64,
    pub view: ModalView,
    /// Scroll state for the button list (`ModalView::List`).
    pub list_scroll: gpui::ScrollHandle,
    pub list_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
    /// Scroll state for the edit form body (`ModalView::Form`).
    pub form_scroll: gpui::ScrollHandle,
    pub form_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
}

pub(crate) enum ModalView {
    List,
    Form {
        /// `Some(id)` when editing an existing button, `None` when creating.
        editing_id: Option<String>,
        /// Currently picked icon path (e.g. `"icons/rocket.svg"`).
        icon: String,
        /// Live text input entity for the "Name" field.
        name_input: Entity<TextInput>,
        /// Live text input entity for the "Command" field.
        command_input: Entity<TextInput>,
    },
}

fn new_button_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("b{millis:x}")
}

// ---------------------------------------------------------------------------
// PaneFlowApp impl
// ---------------------------------------------------------------------------

impl PaneFlowApp {
    pub(crate) fn open_custom_buttons_modal(
        &mut self,
        workspace_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get(workspace_idx) else {
            return;
        };
        let workspace_id = ws.id;
        self.workspace_menu_open = None;
        self.custom_buttons_modal = Some(CustomButtonsModal {
            workspace_id,
            view: ModalView::List,
            list_scroll: gpui::ScrollHandle::new(),
            list_drag: None,
            form_scroll: gpui::ScrollHandle::new(),
            form_drag: None,
        });
        self.custom_buttons_modal_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn close_custom_buttons_modal(&mut self, cx: &mut Context<Self>) {
        self.custom_buttons_modal = None;
        cx.notify();
    }

    fn target_workspace_idx(&self) -> Option<usize> {
        let modal = self.custom_buttons_modal.as_ref()?;
        self.workspaces
            .iter()
            .position(|w| w.id == modal.workspace_id)
    }

    fn begin_new_button(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name_input = cx.new(|cx| TextInput::new("", "e.g. Dev Server", cx));
        let command_input = cx.new(|cx| TextInput::new("", "e.g. clear && bun dev", cx));
        let name_focus = name_input.read(cx).focus_handle.clone();
        if let Some(modal) = self.custom_buttons_modal.as_mut() {
            modal.view = ModalView::Form {
                editing_id: None,
                icon: default_icon(),
                name_input,
                command_input,
            };
        }
        window.focus(&name_focus, cx);
        cx.notify();
    }

    fn begin_edit_button(&mut self, button_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(idx) = self.target_workspace_idx() else {
            return;
        };
        let Some(btn) = self.workspaces[idx]
            .custom_buttons
            .iter()
            .find(|b| b.id == button_id)
            .cloned()
        else {
            return;
        };
        let name_input = cx.new(|cx| TextInput::new(btn.name.clone(), "Name", cx));
        let command_input = cx.new(|cx| TextInput::new(btn.command.clone(), "Command", cx));
        let name_focus = name_input.read(cx).focus_handle.clone();
        if let Some(modal) = self.custom_buttons_modal.as_mut() {
            modal.view = ModalView::Form {
                editing_id: Some(btn.id),
                icon: btn.icon,
                name_input,
                command_input,
            };
        }
        window.focus(&name_focus, cx);
        cx.notify();
    }

    fn cancel_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(modal) = self.custom_buttons_modal.as_mut() else {
            return;
        };
        modal.view = ModalView::List;
        // Return focus to the modal shell so Escape still closes the modal.
        self.custom_buttons_modal_focus.focus(window, cx);
        cx.notify();
    }

    fn delete_button(&mut self, button_id: &str, cx: &mut Context<Self>) {
        let Some(idx) = self.target_workspace_idx() else {
            return;
        };
        let ws = &mut self.workspaces[idx];
        ws.custom_buttons.retain(|b| b.id != button_id);
        self.save_session(cx);
        cx.notify();
    }

    fn save_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(idx) = self.target_workspace_idx() else {
            return;
        };

        // Extract form values while holding only an immutable borrow of the
        // modal state (we read from the entities via `cx`).
        let (editing_id, name, icon, command) = {
            let Some(modal) = self.custom_buttons_modal.as_ref() else {
                return;
            };
            match &modal.view {
                ModalView::Form {
                    editing_id,
                    icon,
                    name_input,
                    command_input,
                } => (
                    editing_id.clone(),
                    name_input.read(cx).value().trim().to_string(),
                    icon.clone(),
                    command_input.read(cx).value().trim().to_string(),
                ),
                ModalView::List => return,
            }
        };

        // Silent no-op when required fields are empty - the Save button is
        // rendered disabled in this case, so this guard is just defensive.
        if name.is_empty() || command.is_empty() {
            return;
        }

        let ws = &mut self.workspaces[idx];
        match editing_id {
            Some(id) => {
                if let Some(btn) = ws.custom_buttons.iter_mut().find(|b| b.id == id) {
                    btn.name = name;
                    btn.icon = icon;
                    btn.command = command;
                }
            }
            None => {
                ws.custom_buttons.push(ButtonCommand {
                    id: new_button_id(),
                    name,
                    icon,
                    command,
                });
            }
        }
        self.save_session(cx);

        // Return to the list so the user sees the result of their action.
        if let Some(modal) = self.custom_buttons_modal.as_mut() {
            modal.view = ModalView::List;
        }
        self.custom_buttons_modal_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn handle_custom_buttons_modal_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let Some(modal) = self.custom_buttons_modal.as_ref() else {
            return;
        };

        match &modal.view {
            ModalView::List => {
                if key == "escape" {
                    self.close_custom_buttons_modal(cx);
                }
            }
            ModalView::Form {
                name_input,
                command_input,
                ..
            } => match key {
                "escape" => {
                    self.cancel_form(window, cx);
                }
                "enter" => {
                    self.save_form(window, cx);
                }
                "tab" => {
                    let name_focus = name_input.read(cx).focus_handle.clone();
                    let command_focus = command_input.read(cx).focus_handle.clone();
                    let target = if name_focus.is_focused(window) {
                        command_focus
                    } else {
                        name_focus
                    };
                    window.focus(&target, cx);
                    cx.notify();
                }
                _ => {}
            },
        }
    }

    pub(crate) fn render_custom_buttons_modal(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(modal) = self.custom_buttons_modal.as_ref() else {
            return div().into_any_element();
        };
        let Some(ws_idx) = self.target_workspace_idx() else {
            return div().into_any_element();
        };
        let ws = &self.workspaces[ws_idx];
        let ui = crate::theme::ui_colors();

        // Codex-quiet header: no own background, no divider - the title sits
        // directly on the card; hierarchy comes from type + spacing. The
        // workspace context rides along as a muted suffix instead of an
        // em-dash compound title.
        let (header_title, header_context) = match &modal.view {
            ModalView::List => ("Custom buttons", Some(ws.title.clone())),
            ModalView::Form { editing_id, .. } => {
                if editing_id.is_some() {
                    ("Edit button", None)
                } else {
                    ("New button", None)
                }
            }
        };
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .px(px(16.))
            .pt(px(14.))
            .pb(px(6.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap(px(7.))
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui.text)
                            .child(header_title),
                    )
                    .when_some(header_context, |d, ws_title| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(ui.muted)
                                .truncate()
                                .child(ws_title),
                        )
                    }),
            )
            .child(
                div()
                    .id("cbtn-header-close")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(22.))
                    .h(px(22.))
                    .rounded(px(4.))
                    .animated_hover(move |style, delta| {
                        style.bg(lerp_color(ui.subtle.opacity(0.0), ui.subtle, delta));
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.close_custom_buttons_modal(cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        svg()
                            .size(px(11.))
                            .flex_none()
                            .path("icons/close.svg")
                            .text_color(ui.muted),
                    ),
            );

        let body: AnyElement = match &modal.view {
            ModalView::List => self.render_modal_list(&ws.custom_buttons, modal, ui, cx),
            ModalView::Form {
                icon,
                name_input,
                command_input,
                editing_id,
            } => self.render_modal_form(
                icon,
                name_input,
                command_input,
                editing_id.as_deref(),
                modal,
                ui,
                cx,
            ),
        };

        let backdrop = div()
            .id("custom-buttons-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(72.))
            .bg(hsla(0., 0., 0., 0.45))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_custom_buttons_modal(cx);
            }))
            .child(
                div()
                    .id("custom-buttons-modal")
                    .occlude()
                    .track_focus(&self.custom_buttons_modal_focus)
                    .on_key_down(cx.listener(Self::handle_custom_buttons_modal_key_down))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        let Some(modal) = this.custom_buttons_modal.as_mut() else {
                            return;
                        };
                        // Only one drag is active at a time (one view at a
                        // time). Try list first, then form.
                        if let Some(drag) = modal.list_drag
                            && let Some(off) =
                                scrollbar::drag_offset(&modal.list_scroll, &drag, ev.position.y)
                        {
                            modal.list_scroll.set_offset(Point::new(px(0.), px(off)));
                            cx.notify();
                        } else if let Some(drag) = modal.form_drag
                            && let Some(off) =
                                scrollbar::drag_offset(&modal.form_scroll, &drag, ev.position.y)
                        {
                            modal.form_scroll.set_offset(Point::new(px(0.), px(off)));
                            cx.notify();
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if let Some(modal) = this.custom_buttons_modal.as_mut() {
                                let cleared = modal.list_drag.take().is_some()
                                    | modal.form_drag.take().is_some();
                                if cleared {
                                    cx.notify();
                                }
                            }
                        }),
                    )
                    .w(px(560.))
                    .max_h(px(560.))
                    .flex()
                    .flex_col()
                    .bg(ui.overlay)
                    .border_1()
                    .border_color(ui.border)
                    .rounded(px(10.))
                    .shadow_lg()
                    .overflow_hidden()
                    .child(header)
                    .child(body),
            );

        deferred(backdrop).with_priority(8).into_any_element()
    }

    fn render_modal_list(
        &self,
        buttons: &[ButtonCommand],
        modal: &CustomButtonsModal,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut list = div()
            .id("cbtn-list-scroll")
            .flex_1()
            .pr(scrollbar::SCROLLBAR_GUTTER)
            .overflow_y_scroll()
            .track_scroll(&modal.list_scroll)
            .flex()
            .flex_col()
            .p(px(8.));

        if buttons.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(28.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(36.))
                            .h(px(36.))
                            .rounded(px(8.))
                            .bg(ui.subtle)
                            .child(
                                svg()
                                    .size(px(16.))
                                    .flex_none()
                                    .path("icons/tool.svg")
                                    .text_color(ui.muted),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(ui.muted)
                            .child("No custom buttons yet."),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(ui.muted)
                            .child("Click \"+ New button\" to create one."),
                    ),
            );
        } else {
            for btn in buttons {
                let btn_id = btn.id.clone();
                let edit_id = btn.id.clone();
                let del_id = btn.id.clone();
                let icon_path = SharedString::from(btn.icon.clone());
                let name = btn.name.clone();
                let cmd_preview = btn.command.clone();
                let edit_button = div()
                    .id(SharedString::from(format!("cbtn-edit-{edit_id}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(24.))
                    .h(px(24.))
                    .rounded(px(4.))
                    .animated_hover(move |style, delta| {
                        style.bg(lerp_color(ui.surface.opacity(0.0), ui.surface, delta));
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.begin_edit_button(&edit_id, window, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        svg()
                            .size(px(13.))
                            .flex_none()
                            .path("icons/edit.svg")
                            .text_color(ui.muted),
                    );
                let delete_button = div()
                    .id(SharedString::from(format!("cbtn-del-{del_id}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(24.))
                    .h(px(24.))
                    .rounded(px(4.))
                    .animated_hover(move |style, delta| {
                        style.bg(lerp_color(ui.surface.opacity(0.0), ui.surface, delta));
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.delete_button(&del_id, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        svg()
                            .size(px(13.))
                            .flex_none()
                            .path("icons/trash.svg")
                            .text_color(ui.muted),
                    );
                list = list.child(
                    div()
                        .id(SharedString::from(format!("cbtn-row-{btn_id}")))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.))
                        .px(px(8.))
                        .py(px(8.))
                        .rounded(px(6.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(26.))
                                .h(px(26.))
                                .rounded(px(6.))
                                .bg(ui.subtle)
                                .flex_none()
                                .child(
                                    svg()
                                        .size(px(14.))
                                        .flex_none()
                                        .path(icon_path)
                                        .text_color(ui.text),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(ui.text)
                                        .truncate()
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(ui.muted)
                                        .font_family("monospace")
                                        .truncate()
                                        .child(cmd_preview),
                                ),
                        )
                        .animated_hover_element(move |row, delta| {
                            row.style()
                                .bg(lerp_color(ui.subtle.opacity(0.0), ui.subtle, delta));
                            let actions = div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .flex()
                                .flex_row()
                                .gap(px(10.))
                                .opacity(delta)
                                .when(delta <= f32::EPSILON, |actions| actions.hidden())
                                .child(edit_button)
                                .child(delete_button);
                            let actions_slot = div()
                                .relative()
                                .flex_none()
                                .w(px(58.))
                                .h(px(24.))
                                .child(actions)
                                .into_any_element();
                            row.extend([actions_slot]);
                        }),
                );
            }
        }

        // Quiet "New button" row (the Agents "New chat" language): no border,
        // muted at rest, fill on hover.
        let new_row = div()
            .id("cbtn-new")
            .mt(px(4.))
            .px(px(8.))
            .py(px(8.))
            .rounded(px(6.))
            .animated_hover(move |style, delta| {
                style.bg(lerp_color(ui.subtle.opacity(0.0), ui.subtle, delta));
            })
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(ui.muted)
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.begin_new_button(window, cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(26.))
                    .h(px(26.))
                    .flex_none()
                    .child(
                        svg()
                            .size(px(13.))
                            .flex_none()
                            .path("icons/plus.svg")
                            .text_color(ui.muted),
                    ),
            )
            .child("New button");

        list = list.child(new_row);

        let bar = scrollbar::render(
            &modal.list_scroll,
            ui,
            None,
            "cbtn-list-scrollbar-track",
            "cbtn-list-scrollbar-thumb",
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if let Some(modal) = this.custom_buttons_modal.as_ref()
                    && let Some(off) =
                        scrollbar::track_click_offset(&modal.list_scroll, ev.position.y)
                {
                    modal.list_scroll.set_offset(Point::new(px(0.), px(off)));
                    cx.notify();
                }
            }),
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if let Some(modal) = this.custom_buttons_modal.as_mut() {
                    modal.list_drag =
                        Some(scrollbar::begin_drag(&modal.list_scroll, ev.position.y));
                }
                cx.stop_propagation();
            }),
        );

        let list_wrapper = div()
            .id("cbtn-list-wrapper")
            .relative()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .child(list)
            .when_some(bar, |d, sb| d.child(sb));

        // No "Done" footer (Codex-minimal): ✕, Esc, and the backdrop click
        // already dismiss - a primary close button was a third affordance for
        // the same action. A small bottom pad keeps the list off the edge.
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .pb(px(6.))
            .child(list_wrapper)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_modal_form(
        &self,
        icon: &str,
        name_input: &Entity<TextInput>,
        command_input: &Entity<TextInput>,
        editing_id: Option<&str>,
        modal: &CustomButtonsModal,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name_field = Self::render_input_field("Name", name_input.clone(), ui);
        let command_field = Self::render_input_field("Command", command_input.clone(), ui);

        // Icon picker - responsive flex-wrap grid.
        let mut icon_grid = div().flex().flex_row().flex_wrap().gap(px(6.)).mt(px(4.));
        for &path in AVAILABLE_ICONS {
            let is_selected = path == icon;
            let path_owned = path.to_string();
            let tile = div()
                .id(SharedString::from(format!(
                    "cbtn-icon-{}",
                    path.replace(['/', '.'], "-")
                )))
                .flex()
                .items_center()
                .justify_center()
                .w(px(32.))
                .h(px(32.))
                .rounded(px(6.))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(modal) = this.custom_buttons_modal.as_mut()
                        && let ModalView::Form {
                            icon: ref mut cur_icon,
                            ..
                        } = modal.view
                    {
                        *cur_icon = path_owned.clone();
                    }
                    cx.notify();
                    cx.stop_propagation();
                }))
                .child(
                    svg()
                        .size(px(16.))
                        .flex_none()
                        .path(path)
                        .text_color(if is_selected { ui.text } else { ui.muted }),
                );
            // Selection by fill, not border (Codex): the picked tile gets the
            // same brightest-neutral fill as every selected surface in the app
            // (#323232); only unselected tiles animate on hover.
            let tile = if is_selected {
                tile.bg(gpui::rgb(0x323232)).into_any_element()
            } else {
                tile.animated_hover(move |style, delta| {
                    style.bg(lerp_color(ui.subtle.opacity(0.0), ui.subtle, delta));
                })
                .into_any_element()
            };
            icon_grid = icon_grid.child(tile);
        }

        // Whether the primary button is enabled depends on both fields having
        // non-whitespace content.
        let name_filled = !name_input.read(cx).value().trim().is_empty();
        let cmd_filled = !command_input.read(cx).value().trim().is_empty();
        let is_valid = name_filled && cmd_filled;

        let save_label = if editing_id.is_some() {
            "Save changes"
        } else {
            "Create"
        };

        let save_button = div()
            .id("cbtn-form-save")
            .px(px(14.))
            .py(px(6.))
            .rounded(px(6.))
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.save_form(window, cx);
                cx.stop_propagation();
            }))
            .child(save_label);
        let save_button = if is_valid {
            save_button
                .bg(ui.text)
                .text_color(ui.base)
                .animated_hover(|style, delta| {
                    style.opacity(1.0 - 0.15 * delta);
                })
                .into_any_element()
        } else {
            save_button
                .bg(ui.subtle)
                .text_color(ui.muted)
                .cursor_default()
                .into_any_element()
        };

        // Quiet form footer: no divider (separation by spacing), Cancel is a
        // bare quiet button - only the primary CTA carries weight.
        let footer = div()
            .flex()
            .flex_row()
            .justify_between()
            .items_center()
            .px(px(16.))
            .pt(px(4.))
            .pb(px(12.))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(ui.muted.opacity(0.8))
                    .child("Tab to switch field · Enter to save · Esc to cancel"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(8.))
                    .child(
                        div()
                            .id("cbtn-form-cancel")
                            .px(px(12.))
                            .py(px(6.))
                            .rounded(px(6.))
                            .text_size(px(12.))
                            .text_color(ui.muted)
                            .animated_hover(move |style, delta| {
                                style
                                    .bg(lerp_color(ui.subtle.opacity(0.0), ui.subtle, delta))
                                    .text_color(lerp_color(ui.muted, ui.text, delta));
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.cancel_form(window, cx);
                                cx.stop_propagation();
                            }))
                            .child("Cancel"),
                    )
                    .child(save_button),
            );

        let body = div()
            .id("cbtn-form-scroll")
            .flex_1()
            .pr(scrollbar::SCROLLBAR_GUTTER)
            .overflow_y_scroll()
            .track_scroll(&modal.form_scroll)
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(16.))
            .py(px(14.))
            .child(name_field)
            .child(command_field)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui.muted)
                            .child("Icon"),
                    )
                    .child(icon_grid),
            );

        let bar = scrollbar::render(
            &modal.form_scroll,
            ui,
            None,
            "cbtn-form-scrollbar-track",
            "cbtn-form-scrollbar-thumb",
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if let Some(modal) = this.custom_buttons_modal.as_ref()
                    && let Some(off) =
                        scrollbar::track_click_offset(&modal.form_scroll, ev.position.y)
                {
                    modal.form_scroll.set_offset(Point::new(px(0.), px(off)));
                    cx.notify();
                }
            }),
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if let Some(modal) = this.custom_buttons_modal.as_mut() {
                    modal.form_drag =
                        Some(scrollbar::begin_drag(&modal.form_scroll, ev.position.y));
                }
                cx.stop_propagation();
            }),
        );

        let body_wrapper = div()
            .id("cbtn-form-wrapper")
            .relative()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .child(body)
            .when_some(bar, |d, sb| d.child(sb));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(body_wrapper)
            .child(footer)
            .into_any_element()
    }

    /// Label + styled container wrapping a cursor-aware `TextInput` entity.
    /// The `TextInput` is a GPUI entity, so we hand it off as a child element
    /// directly - its own Render produces the IBeam hit area, mouse handlers
    /// and text shaping.
    fn render_input_field(
        label: &str,
        input: Entity<TextInput>,
        ui: crate::theme::UiColors,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui.muted)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(ui.border)
                    .bg(ui.surface)
                    .text_size(px(12.))
                    .text_color(ui.text)
                    .font_family("monospace")
                    .child(input),
            )
    }
}
