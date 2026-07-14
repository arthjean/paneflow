//! Context-menu row helpers shared between the sidebar workspace menu and the
//! title-bar burger menu. Includes the action-name shortcut lookup used
//! to render the keyboard-shortcut label next to each action.
//!
//! Part of the US-025 sidebar decomposition.

use std::path::PathBuf;

use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, SharedString, Styled, Window, deferred, div, point,
    prelude::*, px,
};

use crate::app::files_tree;
use crate::pane::{Pane, TabContent};
use crate::settings::components::{menu_divider_color, select_item, select_menu, with_alpha};
use crate::{PaneFlowApp, TabContextMenu, WorkspaceContextMenu};

pub(crate) const EDITOR_CONTEXT_MENU_ITEMS: &[(&str, &str, &str, &str)] = &[
    ("zed", "Open in Zed", "zed", "open_workspace_in_zed"),
    (
        "cursor",
        "Open in Cursor",
        "cursor",
        "open_workspace_in_cursor",
    ),
    (
        "vscode",
        "Open in VS Code",
        "code",
        "open_workspace_in_vscode",
    ),
    (
        "windsurf",
        "Open in Windsurf",
        "windsurf",
        "open_workspace_in_windsurf",
    ),
];

fn context_menu_divider(ui: crate::theme::UiColors) -> gpui::Div {
    div()
        .mx(px(6.))
        .my(px(4.))
        .h(px(1.))
        .bg(menu_divider_color(ui))
}

pub(crate) fn clamped_context_menu_position(
    position: gpui::Point<Pixels>,
    width: Pixels,
    height: Pixels,
    window: &Window,
) -> gpui::Point<Pixels> {
    let win_size = window.window_bounds().get_bounds().size;
    let x = if position.x + width > win_size.width {
        (position.x - width).max(px(0.))
    } else {
        position.x
    };
    let y = if position.y + height > win_size.height {
        (position.y - height).max(px(0.))
    } else {
        position.y
    };
    point(x, y)
}

impl PaneFlowApp {
    pub(crate) fn shortcut_for_action(&self, action_name: &str) -> Option<&str> {
        self.effective_shortcuts
            .iter()
            .find(|entry| entry.action_name == action_name && entry.key != "Unassigned")
            .map(|entry| entry.key.as_str())
    }

    pub(crate) fn render_context_menu_item(
        &self,
        id: SharedString,
        label: &str,
        shortcut: Option<SharedString>,
        ui: crate::theme::UiColors,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_between()
            .gap(px(10.))
            .px(px(8.))
            .py(px(5.))
            .rounded(px(4.))
            .cursor_pointer()
            .text_size(px(11.))
            .text_color(ui.text)
            .hover(|s| {
                let ui = crate::theme::ui_colors();
                s.bg(ui.subtle)
            })
            .on_click(on_click)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label.to_string()),
            )
            .when_some(shortcut, |d, shortcut| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(ui.muted)
                        .child(shortcut),
                )
            })
    }

    /// One workspace-menu row in the shared Settings "Shell" select look
    /// (`components::select_item`): 28px tall, 7px radius, 12px label flex-filled
    /// with the optional shortcut pinned right, and the whisper hover highlight
    /// (`text @ 0.05`) instead of the older flat `ui.subtle`. Keeps every app
    /// menu reading as one consistent menu language.
    pub(crate) fn render_select_menu_item(
        &self,
        id: SharedString,
        label: &str,
        shortcut: Option<SharedString>,
        ui: crate::theme::UiColors,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        select_item(id, false, ui)
            .on_click(on_click)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(ui.text)
                    .child(label.to_string()),
            )
            .when_some(shortcut, |d, shortcut| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(ui.muted)
                        .child(shortcut),
                )
            })
    }

    pub(crate) fn open_workspace_service_url(&mut self, url: &str, cx: &mut Context<Self>) {
        if let Err(err) = crate::external_open::open_url(url) {
            let message = if err.kind() == std::io::ErrorKind::NotFound {
                "Could not open URL - install xdg-utils (Linux), or check your default browser"
                    .to_string()
            } else {
                format!("Could not open URL: {err}")
            };
            log::warn!("sidebar: open URL failed: {err}");
            self.show_toast(message, cx);
        }
    }

    /// Build the deferred element that paints the right-click workspace
    /// context menu. Caller is responsible for the
    /// `if let Some(menu) = self.workspace_menu_open && menu.idx < self.workspaces.len()`
    /// guard. Extracted from `main.rs` per US-002.
    pub(crate) fn render_workspace_context_menu(
        &self,
        menu: WorkspaceContextMenu,
        ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let idx = menu.idx;
        let can_close = !self.workspaces.is_empty();
        let workflow_template = self.workspace_template_for_workspace(idx);
        let services: Vec<_> = self
            .workspaces
            .get(idx)
            .map(|workspace| {
                workspace
                    .active_ports
                    .iter()
                    .filter_map(|port| {
                        workspace
                            .service_labels
                            .get(port)
                            .cloned()
                            .map(|info| (*port, info))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let workflow_rows = usize::from(workflow_template.is_some());
        let service_rows = services.len();
        let separator_rows = 3 + usize::from(service_rows > 0);
        let menu_rows = EDITOR_CONTEXT_MENU_ITEMS.len() + 5 + workflow_rows + service_rows;
        let menu_height = px(8. + menu_rows as f32 * 28. + separator_rows as f32 * 9.);
        let menu_pos = clamped_context_menu_position(menu.position, px(248.), menu_height, window);

        let mut context_menu = select_menu("workspace-context-menu", ui)
            .occlude()
            .absolute()
            .left(menu_pos.x)
            .top(menu_pos.y)
            .w(px(248.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.workspace_menu_open = None;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());

        context_menu = context_menu.child(self.render_select_menu_item(
            "workspace-context-rename".into(),
            "Rename",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.workspace_menu_open = None;
                this.begin_workspace_rename(idx, cx);
                cx.stop_propagation();
                cx.notify();
            }),
        ));

        if let Some(template_idx) = workflow_template {
            context_menu = context_menu.child(self.render_select_menu_item(
                "workspace-context-run-workflow".into(),
                "Run Workflow",
                None,
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.workspace_menu_open = None;
                    this.run_saved_workspace_template_for_workspace(idx, template_idx, cx);
                    cx.stop_propagation();
                }),
            ));
        }

        context_menu = context_menu.child(context_menu_divider(ui));

        for (port, info) in services {
            let service_name = info
                .label
                .clone()
                .unwrap_or_else(|| "Local service".to_string());
            if info.is_frontend {
                let label = format!("Open {service_name} :{port}");
                let url = info
                    .url
                    .clone()
                    .unwrap_or_else(|| format!("http://localhost:{port}"));
                context_menu = context_menu.child(self.render_select_menu_item(
                    SharedString::from(format!("workspace-context-service-{port}")),
                    &label,
                    None,
                    ui,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.workspace_menu_open = None;
                        this.open_workspace_service_url(&url, cx);
                        cx.stop_propagation();
                    }),
                ));
            } else {
                context_menu = context_menu.child(Self::render_disabled_select_menu_item(
                    SharedString::from(format!("workspace-context-service-{port}-info")),
                    &format!("{service_name} :{port}"),
                    ui,
                ));
            }
        }

        if service_rows > 0 {
            context_menu = context_menu.child(context_menu_divider(ui));
        }

        for &(id, label, command, shortcut_action) in EDITOR_CONTEXT_MENU_ITEMS {
            let shortcut = self
                .shortcut_for_action(shortcut_action)
                .map(|s| SharedString::from(s.to_string()));
            let command = command.to_string();
            let label_owned = label.to_string();
            context_menu = context_menu.child(self.render_select_menu_item(
                SharedString::from(format!("workspace-context-{id}")),
                label,
                shortcut,
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.open_workspace_in_editor(idx, &command, &label_owned, cx);
                    cx.stop_propagation();
                }),
            ));
        }

        context_menu = context_menu.child(context_menu_divider(ui));

        // Reveal in file manager
        let reveal_shortcut = self
            .shortcut_for_action("reveal_workspace_in_file_manager")
            .map(|s| SharedString::from(s.to_string()));
        context_menu = context_menu.child(self.render_select_menu_item(
            "workspace-context-reveal".into(),
            "Reveal in File Manager",
            reveal_shortcut,
            ui,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.reveal_workspace_in_file_manager(idx, cx);
                cx.stop_propagation();
            }),
        ));

        // Copy path
        let copy_shortcut = self
            .shortcut_for_action("copy_workspace_path")
            .map(|s| SharedString::from(s.to_string()));
        context_menu = context_menu.child(self.render_select_menu_item(
            "workspace-context-copy".into(),
            "Copy Path",
            copy_shortcut,
            ui,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.copy_workspace_path(idx, cx);
                cx.stop_propagation();
            }),
        ));

        // Manage Custom Buttons - opens the per-workspace button editor modal.
        context_menu = context_menu.child(self.render_select_menu_item(
            "workspace-context-custom-buttons".into(),
            "Manage Custom Buttons…",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.open_custom_buttons_modal(idx, window, cx);
                cx.stop_propagation();
            }),
        ));

        context_menu = context_menu.child(context_menu_divider(ui));

        // Close workspace (conditionally disabled)
        let close_shortcut = self
            .shortcut_for_action("close_workspace")
            .map(|s| SharedString::from(s.to_string()));
        context_menu = context_menu.child(
            div()
                .id("workspace-context-close")
                .h(px(28.))
                .px(px(8.))
                .rounded(px(7.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .text_size(px(12.))
                .text_color(ui.muted)
                .when(can_close, |d| d.text_color(ui.text))
                .when(can_close, |d| {
                    d.cursor_pointer()
                        .hover(move |s| s.bg(with_alpha(ui.text, 0.05)))
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    if can_close {
                        this.close_workspace_at(idx, window, cx);
                    } else {
                        this.workspace_menu_open = None;
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child("Close Workspace"),
                )
                .when_some(close_shortcut, |d, shortcut| {
                    d.child(
                        div()
                            .flex_none()
                            .text_size(px(10.))
                            .text_color(ui.muted)
                            .child(shortcut),
                    )
                }),
        );

        deferred(context_menu).priority(3).into_any_element()
    }

    /// Build the deferred "Move to pane…" tab context menu (EP-002 US-006, the
    /// WCAG 2.5.7 non-drag alternative to a cross-pane drag). Lists every other
    /// pane in the source pane's workspace; selecting one moves the tab there
    /// through the same [`crate::pane_drag::move_tab_into`] path the drag uses,
    /// so the PTY is preserved and an emptied source pane is reflowed away. When
    /// the source pane is the workspace's only pane, the menu shows a disabled
    /// note instead of move targets.
    pub(crate) fn render_tab_context_menu(
        &self,
        menu: TabContextMenu,
        ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let source = menu.source_pane.clone();
        let source_idx = source.read(cx).index_for_tab_id(menu.tab_id);

        // Enumerate the panes of the workspace that owns the source pane, in
        // tree order, dropping the source itself.
        let (workspace_cwd, others): (Option<PathBuf>, Vec<(usize, Entity<Pane>)>) = self
            .workspaces
            .iter()
            .find_map(|ws| {
                let root = ws.root.as_ref()?;
                root.contains_leaf(&source).then(|| {
                    let panes = root
                        .collect_leaves()
                        .into_iter()
                        .enumerate()
                        .filter(|(_, p)| p != &source)
                        .collect();
                    (Some(PathBuf::from(&ws.cwd)), panes)
                })
            })
            .unwrap_or((None, Vec::new()));

        let tab_path = source
            .read(cx)
            .tabs
            .get(source_idx.unwrap_or(usize::MAX))
            .and_then(|tab| Self::tab_context_path(tab, cx));
        let target_tab_id =
            source_idx.and_then(|idx| source.read(cx).tabs.get(idx).map(|tab| tab.entity_id()));
        let full_path = tab_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        let relative_path = tab_path.as_ref().map(|path| {
            workspace_cwd
                .as_ref()
                .map(|root| files_tree::workspace_relative_path(root, path))
                .unwrap_or_else(|| path.to_string_lossy().into_owned())
        });

        // EP-001 US-003 (cli-cockpit): cancel this tab's queued prompt -
        // the non-Composer cancel path. Only shown when a buffer exists.
        let pending_sid = source
            .read(cx)
            .tabs
            .get(source_idx.unwrap_or(usize::MAX))
            .and_then(|t| t.as_terminal())
            .map(|t| t.entity_id().as_u64())
            .filter(|sid| self.broadcast.pending.contains_key(sid));

        let rows = 2 + others.len().max(1) + usize::from(pending_sid.is_some()) + 1;
        let menu_height = px(8. + rows as f32 * 29. + 18.);
        let menu_pos = clamped_context_menu_position(menu.position, px(248.), menu_height, window);

        let mut context_menu = select_menu("tab-context-menu", ui)
            .occlude()
            .absolute()
            .left(menu_pos.x)
            .top(menu_pos.y)
            .w(px(248.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.tab_menu_open = None;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());

        if let Some(value) = full_path {
            context_menu = context_menu.child(self.render_select_menu_item(
                "tab-context-copy-path".into(),
                "Copy Path",
                None,
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                    this.tab_menu_open = None;
                    this.show_toast("Copied path", cx);
                    cx.stop_propagation();
                }),
            ));
        } else {
            context_menu = context_menu.child(Self::render_disabled_select_menu_item(
                "tab-context-copy-path-disabled".into(),
                "Copy Path unavailable",
                ui,
            ));
        }

        if let Some(value) = relative_path {
            context_menu = context_menu.child(self.render_select_menu_item(
                "tab-context-copy-relative-path".into(),
                "Copy Relative Path",
                None,
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                    this.tab_menu_open = None;
                    this.show_toast("Copied relative path", cx);
                    cx.stop_propagation();
                }),
            ));
        } else {
            context_menu = context_menu.child(Self::render_disabled_select_menu_item(
                "tab-context-copy-relative-path-disabled".into(),
                "Copy Relative Path unavailable",
                ui,
            ));
        }

        context_menu = context_menu.child(
            div()
                .mx(px(6.))
                .my(px(4.))
                .h(px(1.))
                .bg(menu_divider_color(ui)),
        );

        if source_idx.is_none() {
            context_menu = context_menu.child(
                div()
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .text_color(ui.muted)
                    .child("Tab no longer exists"),
            );
        } else if others.is_empty() {
            // AC US-006: with a single pane there is nowhere to move to.
            context_menu = context_menu.child(
                div()
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .text_color(ui.muted)
                    .child("No other panes"),
            );
        } else {
            for (orig_idx, dest) in others {
                let label = format!(
                    "Move to Pane {} - {}",
                    orig_idx + 1,
                    dest.read(cx).active_tab_label(cx)
                );
                let dest_for_click = dest.clone();
                let source_for_click = source.clone();
                let source_tab_id = menu.tab_id;
                context_menu = context_menu.child(self.render_select_menu_item(
                    SharedString::from(format!("tab-move-{orig_idx}")),
                    &label,
                    None,
                    ui,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.tab_menu_open = None;
                        cx.stop_propagation();
                        // Both panes are held alive by strong refs, but either
                        // could have been removed from the tree while the menu
                        // was open (e.g. a background shell exited and emptied
                        // its pane). Moving into/out of an off-tree pane would
                        // be a confusing no-op, so verify both are still live
                        // leaves of the same workspace before committing.
                        let both_live = this.workspaces.iter().any(|ws| {
                            ws.root.as_ref().is_some_and(|r| {
                                let leaves = r.collect_leaves();
                                leaves.contains(&source_for_click)
                                    && leaves.contains(&dest_for_click)
                            })
                        });
                        if !both_live {
                            cx.notify();
                            return;
                        }
                        dest_for_click.update(cx, |dest_pane, dest_cx| {
                            let dest_idx = dest_pane.tabs.len();
                            crate::pane_drag::move_tab_into(
                                dest_pane,
                                dest_cx,
                                &source_for_click,
                                source_tab_id,
                                dest_idx,
                                window,
                            );
                        });
                        this.save_session(cx);
                        cx.notify();
                    }),
                ));
            }
        }

        if let Some(sid) = pending_sid {
            context_menu = context_menu.child(self.render_select_menu_item(
                SharedString::from("tab-cancel-queued"),
                "Cancel queued prompt",
                None,
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.tab_menu_open = None;
                    this.cancel_pending_for(sid, cx);
                    cx.stop_propagation();
                    cx.notify();
                }),
            ));
        }

        context_menu = context_menu.child(
            div()
                .mx(px(6.))
                .my(px(4.))
                .h(px(1.))
                .bg(menu_divider_color(ui)),
        );

        let source_for_close = source.clone();
        let target_tab_id_for_close = target_tab_id;
        context_menu = context_menu.child(self.render_select_menu_item(
            "tab-context-close".into(),
            "Close Tab",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.tab_menu_open = None;
                source_for_close.update(cx, |pane, pane_cx| {
                    if let Some(tab_id) = target_tab_id_for_close
                        && let Some(idx) =
                            pane.tabs.iter().position(|tab| tab.entity_id() == tab_id)
                    {
                        pane.close_tab_at(idx, pane_cx);
                    }
                });
                this.save_session(cx);
                cx.stop_propagation();
                cx.notify();
            }),
        ));

        deferred(context_menu).priority(3).into_any_element()
    }

    fn render_disabled_select_menu_item(
        id: SharedString,
        label: &str,
        ui: crate::theme::UiColors,
    ) -> impl IntoElement {
        div()
            .id(id)
            .h(px(28.))
            .px(px(8.))
            .rounded(px(7.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.))
            .text_color(ui.muted)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label.to_string()),
            )
    }

    fn tab_context_path(tab: &TabContent, cx: &App) -> Option<PathBuf> {
        match tab {
            TabContent::Terminal(terminal) => terminal
                .read(cx)
                .terminal
                .current_cwd
                .as_ref()
                .filter(|cwd| !cwd.is_empty())
                .map(PathBuf::from),
            TabContent::Markdown(markdown) => Some(markdown.read(cx).path.clone()),
            TabContent::Diff(diff) => diff.read(cx).column_paths().into_iter().next(),
        }
    }
}
