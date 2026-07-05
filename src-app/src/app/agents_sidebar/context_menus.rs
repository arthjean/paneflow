//! Deferred-layer rendering for the Agents-mode context menus and the
//! delete-confirmation dialog (US-011 of `tasks/prd-agents-view.md`).
//!
//! Lives in its own file so [`super::PaneFlowApp::render_agents_sidebar`]
//! does not have to host another 200 lines of menu plumbing. Both
//! renderers reuse [`PaneFlowApp::render_context_menu_item`] +
//! [`PaneFlowApp::shortcut_for_action`] from the workspace
//! context-menu module so the visual language stays identical (AC #8).

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, IntoElement, MouseButton, ParentElement,
    SharedString, Styled, Window, deferred, div, prelude::*, px,
};

use super::state::{AgentsContextMenu, AgentsDeleteTarget};
use crate::PaneFlowApp;
use crate::app::sidebar::context_menu::{EDITOR_CONTEXT_MENU_ITEMS, clamped_context_menu_position};
use crate::settings::components::{menu_divider_color, select_menu};

impl PaneFlowApp {
    /// Build the deferred element for the project-row right-click
    /// menu. Caller is responsible for the
    /// `if let Some(AgentsContextMenu::Project { project_idx, position }) = self.agents_view.agents_menu_open
    ///   && project_idx < self.projects.len()`
    /// guard so the menu never points at a removed row.
    pub(crate) fn render_agents_project_context_menu(
        &self,
        project_idx: usize,
        position: gpui::Point<gpui::Pixels>,
        ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // ~9 items, ~28px tall, 2 separators ~9px, 8px padding => ~280px
        let menu_height = px(280.);
        let menu_pos = clamped_context_menu_position(position, px(248.), menu_height, window);

        let mut menu = select_menu("agents-project-context-menu", ui)
            .occlude()
            .absolute()
            .left(menu_pos.x)
            .top(menu_pos.y)
            .w(px(248.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_agents_menu(cx);
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());

        // New thread
        menu = menu.child(self.render_select_menu_item(
            "agents-project-new-thread".into(),
            "New thread",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.close_agents_menu(cx);
                this.create_agents_thread_in(project_idx, cx);
                cx.stop_propagation();
            }),
        ));

        // New terminal thread (mirrors Zed's `NewTerminalThread` action:
        // opens a PTY surface in the main area instead of a chat, so
        // the user can run any CLI agent or long-running process as a
        // first-class sidebar entry).
        menu = menu.child(self.render_select_menu_item(
            "agents-project-new-terminal-thread".into(),
            "New terminal thread",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.close_agents_menu(cx);
                this.create_terminal_thread_in(project_idx, cx);
                cx.stop_propagation();
            }),
        ));

        // Rename
        menu = menu.child(self.render_select_menu_item(
            "agents-project-rename".into(),
            "Rename project",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, w, cx| {
                this.close_agents_menu(cx);
                this.begin_agents_rename(
                    super::state::AgentsRenameTarget::Project { project_idx },
                    w,
                    cx,
                );
                cx.stop_propagation();
            }),
        ));

        // ── Separator ──
        menu = menu.child(
            div()
                .mx(px(6.))
                .my(px(4.))
                .h(px(1.))
                .bg(menu_divider_color(ui)),
        );

        // Editor entries
        for &(id, label, command, shortcut_action) in EDITOR_CONTEXT_MENU_ITEMS {
            let shortcut = self
                .shortcut_for_action(shortcut_action)
                .map(|s| SharedString::from(s.to_string()));
            let command = command.to_string();
            let label_owned = label.to_string();
            menu = menu.child(self.render_select_menu_item(
                SharedString::from(format!("agents-project-{id}")),
                label,
                shortcut,
                ui,
                cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.open_agents_project_in_editor(project_idx, &command, &label_owned, cx);
                    cx.stop_propagation();
                }),
            ));
        }

        // Reveal in file manager
        let reveal_shortcut = self
            .shortcut_for_action("reveal_workspace_in_file_manager")
            .map(|s| SharedString::from(s.to_string()));
        menu = menu.child(self.render_select_menu_item(
            "agents-project-reveal".into(),
            "Reveal in File Manager",
            reveal_shortcut,
            ui,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.reveal_agents_project_in_file_manager(project_idx, cx);
                cx.stop_propagation();
            }),
        ));

        // ── Separator ──
        menu = menu.child(
            div()
                .mx(px(6.))
                .my(px(4.))
                .h(px(1.))
                .bg(menu_divider_color(ui)),
        );

        // Delete project (always available -- confirmation is owned by
        // `render_agents_confirm_delete_dialog`, gated on whether the
        // project has threads).
        menu = menu.child(self.render_select_menu_item(
            "agents-project-delete".into(),
            "Delete project",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                // If the project has zero threads we still go through
                // the confirmation flow for consistency -- the user
                // can always dismiss with Escape.
                this.request_agents_confirm_delete(AgentsDeleteTarget::Project { project_idx }, cx);
                cx.stop_propagation();
            }),
        ));

        deferred(menu).priority(3).into_any_element()
    }

    /// Thread/chat-row right-click menu: Pin/Unpin, Rename, Duplicate,
    /// Restart terminal, Reveal, Delete.
    ///
    /// US-008/US-014: parameterized by [`crate::project::AgentsTarget`] so the
    /// project-thread row and the free-chat row share one menu renderer (no
    /// divergent duplicate). The concrete dispatch lives in the
    /// `*_for_target` affordance helpers. Also the title-bar `⋯` overflow
    /// menu (US-011) reuses this exact renderer.
    pub(crate) fn render_agents_thread_context_menu(
        &self,
        target: crate::project::AgentsTarget,
        position: gpui::Point<gpui::Pixels>,
        ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // 6 items (Pin, Rename, Duplicate, Restart, Reveal, Delete) +
        // 1 separator + 8px padding => ~225px.
        let menu_height = px(228.);
        let menu_pos = clamped_context_menu_position(position, px(220.), menu_height, window);
        // The chat row labels its rename entry "Rename chat" so the verb
        // matches the section the user right-clicked in.
        let rename_label = match target {
            crate::project::AgentsTarget::Chat { .. } => "Rename chat",
            crate::project::AgentsTarget::Thread { .. } => "Rename thread",
        };
        // US-014: the Pin entry's label flips with the target's current
        // state, mirroring the ★/☆ hover toggle (US-006).
        let pin_label = if self
            .thread_for_target(target)
            .map(|t| t.pinned)
            .unwrap_or(false)
        {
            "Unpin"
        } else {
            "Pin"
        };

        let mut menu = select_menu("agents-thread-context-menu", ui)
            .occlude()
            .absolute()
            .left(menu_pos.x)
            .top(menu_pos.y)
            .w(px(220.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_agents_menu(cx);
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());

        // US-014: Pin/Unpin first - the primary "keep this on top" action,
        // consistent with the hover ★. Toggles `thread.pinned` + saves.
        menu = menu.child(self.render_select_menu_item(
            "agents-thread-pin".into(),
            pin_label,
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.close_agents_menu(cx);
                this.toggle_pin_for_target(target, cx);
                cx.stop_propagation();
            }),
        ));

        menu = menu.child(self.render_select_menu_item(
            "agents-thread-rename".into(),
            rename_label,
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, w, cx| {
                this.close_agents_menu(cx);
                this.begin_agents_rename_for_target(target, w, cx);
                cx.stop_propagation();
            }),
        ));

        menu = menu.child(self.render_select_menu_item(
            "agents-thread-duplicate".into(),
            "Duplicate",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.close_agents_menu(cx);
                this.duplicate_agents_target(target, cx);
                cx.stop_propagation();
            }),
        ));

        menu = menu.child(self.render_select_menu_item(
            "agents-thread-restart-terminal".into(),
            "Restart terminal",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.restart_agents_target_terminal(target, cx);
                cx.stop_propagation();
            }),
        ));

        // US-011: Reveal the target's cwd (the project dir for a thread, the
        // home dir for a chat). Surfaced both here (right-click) and in the
        // title-bar `⋯` overflow menu (same renderer).
        menu = menu.child(self.render_select_menu_item(
            "agents-thread-reveal".into(),
            "Reveal in File Manager",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.reveal_agents_target(target, cx);
                cx.stop_propagation();
            }),
        ));

        menu = menu.child(
            div()
                .mx(px(6.))
                .my(px(4.))
                .h(px(1.))
                .bg(menu_divider_color(ui)),
        );

        menu = menu.child(self.render_select_menu_item(
            "agents-thread-delete".into(),
            "Delete",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.request_delete_for_target(target, cx);
                cx.stop_propagation();
            }),
        ));

        deferred(menu).priority(3).into_any_element()
    }

    /// Center-screen confirmation dialog for a pending delete. Mirrors
    /// the existing close-confirm modal style (rounded card, ui.overlay
    /// background, primary/danger buttons).
    pub(crate) fn render_agents_confirm_delete_dialog(
        &self,
        target: AgentsDeleteTarget,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, body, danger_label) = match target {
            AgentsDeleteTarget::Project { project_idx } => {
                let (name, thread_count) = self
                    .projects
                    .get(project_idx)
                    .map(|p| (p.title.clone(), p.threads.len()))
                    .unwrap_or_default();
                let body = if thread_count == 0 {
                    format!("Delete project \"{name}\"?")
                } else if thread_count == 1 {
                    format!("Delete project \"{name}\" and its 1 thread? This cannot be undone.")
                } else {
                    format!(
                        "Delete project \"{name}\" and its {thread_count} threads? This cannot be undone."
                    )
                };
                ("Delete project".to_string(), body, "Delete".to_string())
            }
            AgentsDeleteTarget::Thread {
                project_idx,
                thread_idx,
            } => {
                let name = self
                    .projects
                    .get(project_idx)
                    .and_then(|p| p.threads.get(thread_idx))
                    .map(|t| t.title.clone())
                    .unwrap_or_else(|| "this thread".to_string());
                (
                    "Delete thread".to_string(),
                    format!("Delete thread \"{name}\"? This cannot be undone."),
                    "Delete".to_string(),
                )
            }
            AgentsDeleteTarget::Chat { chat_idx } => {
                let name = self
                    .chats
                    .get(chat_idx)
                    .map(|t| t.title.clone())
                    .unwrap_or_else(|| "this chat".to_string());
                (
                    "Delete chat".to_string(),
                    format!("Delete chat \"{name}\"? This cannot be undone."),
                    "Delete".to_string(),
                )
            }
        };

        let backdrop = div()
            .id("agents-confirm-backdrop")
            .occlude()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .bg(gpui::black().opacity(0.45))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.cancel_agents_confirm_delete(cx);
                }),
            )
            .child(
                div()
                    .id("agents-confirm-dialog")
                    .occlude()
                    .w(px(360.))
                    .bg(ui.overlay)
                    .border_1()
                    .border_color(ui.border)
                    .rounded(px(10.))
                    .shadow_lg()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui.text)
                            .child(title),
                    )
                    .child(div().text_size(px(12.)).text_color(ui.muted).child(body))
                    .child(
                        div()
                            .mt(px(6.))
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                div()
                                    .id("agents-confirm-cancel")
                                    .px(px(14.))
                                    .py(px(7.))
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .bg(ui.subtle)
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ui.text)
                                    .hover(|s| {
                                        let ui = crate::theme::ui_colors();
                                        s.bg(ui.surface)
                                    })
                                    .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                                        this.cancel_agents_confirm_delete(cx);
                                    }))
                                    .child("Cancel"),
                            )
                            .child(
                                div()
                                    .id("agents-confirm-delete")
                                    .px(px(14.))
                                    .py(px(7.))
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .bg(gpui::rgb(0xf38ba8))
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ui.base)
                                    .hover(|s| s.opacity(0.88))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                                        this.execute_agents_confirm_delete(cx);
                                    }))
                                    .child(danger_label),
                            ),
                    ),
            );

        deferred(backdrop).priority(4).into_any_element()
    }
}

/// Type-erased view-side helper: given the live `agents_menu_open`,
/// build the right deferred element. Centralised so the main render
/// path is one line.
pub(crate) fn render_open_agents_menu(
    app: &PaneFlowApp,
    menu: AgentsContextMenu,
    ui: crate::theme::UiColors,
    window: &mut Window,
    cx: &mut Context<PaneFlowApp>,
) -> Option<AnyElement> {
    match menu {
        AgentsContextMenu::Project {
            project_idx,
            position,
        } if project_idx < app.projects.len() => {
            Some(app.render_agents_project_context_menu(project_idx, position, ui, window, cx))
        }
        AgentsContextMenu::Thread {
            project_idx,
            thread_idx,
            position,
        } if project_idx < app.projects.len()
            && app
                .projects
                .get(project_idx)
                .map(|p| thread_idx < p.threads.len())
                .unwrap_or(false) =>
        {
            Some(app.render_agents_thread_context_menu(
                crate::project::AgentsTarget::Thread {
                    project_idx,
                    thread_idx,
                },
                position,
                ui,
                window,
                cx,
            ))
        }
        AgentsContextMenu::Chat { chat_idx, position } if chat_idx < app.chats.len() => {
            Some(app.render_agents_thread_context_menu(
                crate::project::AgentsTarget::Chat { chat_idx },
                position,
                ui,
                window,
                cx,
            ))
        }
        _ => None,
    }
}
