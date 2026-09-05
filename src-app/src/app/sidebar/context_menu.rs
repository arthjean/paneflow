use std::path::PathBuf;

use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, CursorStyle, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, SharedString, Styled, Window, deferred, div,
    point, prelude::*, px,
};

use crate::app::files_tree;
use crate::pane::PaneSurface;
use crate::settings::components::{menu_divider_color, select_item, select_menu, with_alpha};
use crate::ui_primitives::AnimatedHoverExt;
use crate::{PaneContextMenu, PaneFlowApp, TabContextMenu, WorkspaceContextMenu};

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
            .text_size(px(11.))
            .text_color(ui.text)
            .animated_hover_bg(ui.subtle.opacity(0.0), ui.subtle)
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

    pub(crate) fn render_select_menu_item(
        &self,
        id: SharedString,
        label: &str,
        shortcut: Option<SharedString>,
        ui: crate::theme::UiColors,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        select_item(id, false, ui)
            .cursor(CursorStyle::Arrow)
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
        let separator_rows = 2 + workflow_rows + usize::from(service_rows > 0);
        let menu_rows = EDITOR_CONTEXT_MENU_ITEMS.len() + 4 + workflow_rows + service_rows;
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

        if workflow_rows > 0 {
            context_menu = context_menu.child(context_menu_divider(ui));
        }

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

        let close_shortcut = self
            .shortcut_for_action("close_workspace")
            .map(|s| SharedString::from(s.to_string()));
        context_menu = context_menu.child({
            let hover_bg = with_alpha(ui.text, 0.05);
            let target_bg = if can_close {
                hover_bg
            } else {
                hover_bg.opacity(0.0)
            };
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
                .animated_hover_bg(hover_bg.opacity(0.0), target_bg)
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
                })
        });

        deferred(context_menu).priority(3).into_any_element()
    }

    pub(crate) fn render_tab_context_menu(
        &self,
        menu: TabContextMenu,
        ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let TabContextMenu {
            ws_idx,
            tab_idx,
            position,
        } = menu;
        let tab = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs().get(tab_idx));
        let can_reset_name = tab.is_some_and(|tab| tab.title_is_user_owned());
        let bound = tab.and_then(|tab| tab.worktree.clone());
        let is_bound = bound.is_some();
        let root = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.repo_root.clone())
            .unwrap_or_default();
        let listing = self.workspace_worktree_listing(ws_idx);
        let on_branch = match bound.as_ref() {
            Some(path) => listing
                .iter()
                .find(|entry| entry.path == *path)
                .and_then(|entry| entry.branch.clone()),
            None => Some(self.workspace_checkout_label(ws_idx)),
        };
        let mut branches: Vec<(Option<std::path::PathBuf>, String, bool)> = self
            .workspace_branches(ws_idx)
            .iter()
            .map(|branch| {
                let selected = on_branch.as_deref() == Some(branch.as_str());
                (None, branch.clone(), selected)
            })
            .collect();
        branches.extend(
            listing
                .iter()
                .filter(|entry| entry.branch.is_none() && entry.path != root)
                .map(|entry| {
                    let label =
                        crate::workspace::worktree::checkout_label(None, &entry.path, &root);
                    let selected = bound.as_deref() == Some(entry.path.as_path());
                    (Some(entry.path.clone()), label, selected)
                }),
        );
        let show_worktrees = branches.len() > 1;
        let worktree_rows = if show_worktrees {
            1. + branches.len() as f32
        } else {
            0.
        };
        let remove_rows = if is_bound { 1. } else { 0. };
        let rows = if can_reset_name { 3. } else { 2. } + worktree_rows + remove_rows;
        let menu_height = px(8. + rows * 28.);
        let menu_pos = clamped_context_menu_position(position, px(248.), menu_height, window);
        let close_shortcut = self
            .shortcut_for_action("close_tab")
            .map(|key| SharedString::from(key.to_string()));
        let remove_worktree_item = is_bound.then(|| {
            self.render_select_menu_item(
                "tab-context-remove-worktree".into(),
                "Remove worktree",
                None,
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.tab_menu_open = None;
                    this.remove_tab_worktree(ws_idx, tab_idx, cx);
                    cx.stop_propagation();
                }),
            )
        });

        select_menu("tab-context-menu", ui)
            .occlude()
            .absolute()
            .left(menu_pos.x)
            .top(menu_pos.y)
            .w(px(248.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.tab_menu_open = None;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(self.render_select_menu_item(
                "tab-context-rename".into(),
                "Rename",
                None,
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.tab_menu_open = None;
                    this.begin_tab_rename(ws_idx, tab_idx, cx);
                    cx.stop_propagation();
                }),
            ))
            .when(can_reset_name, |menu| {
                menu.child(self.render_select_menu_item(
                    "tab-context-reset-name".into(),
                    "Reset name",
                    None,
                    ui,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.tab_menu_open = None;
                        this.reset_tab_name(ws_idx, tab_idx, cx);
                        cx.stop_propagation();
                    }),
                ))
            })
            .child(self.render_select_menu_item(
                "tab-context-close".into(),
                "Close",
                close_shortcut,
                ui,
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.tab_menu_open = None;
                    this.close_workspace_tab(ws_idx, tab_idx, window, cx);
                    cx.stop_propagation();
                }),
            ))
            .when(show_worktrees, |menu| {
                let mut menu = menu.child(context_menu_divider(ui)).child(
                    div()
                        .px(px(8.))
                        .pb(px(4.))
                        .text_size(px(10.))
                        .text_color(ui.muted)
                        .child("Branch"),
                );
                for (detached, label, selected) in branches {
                    let branch = label.clone();
                    menu = menu.child(
                        select_item(
                            SharedString::from(format!("tab-branch-{label}")),
                            selected,
                            ui,
                        )
                        .cursor(CursorStyle::Arrow)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.tab_menu_open = None;
                            match detached.clone() {
                                Some(path) => {
                                    this.set_tab_worktree(ws_idx, tab_idx, Some(path), cx)
                                }
                                None => {
                                    this.bind_tab_to_branch(ws_idx, tab_idx, branch.clone(), cx)
                                }
                            }
                            cx.stop_propagation();
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_x_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(ui.text)
                                .child(label),
                        ),
                    );
                }
                menu
            })
            .when_some(remove_worktree_item, |menu, item| {
                menu.child(context_menu_divider(ui)).child(item)
            })
            .into_any_element()
    }

    pub(crate) fn render_pane_context_menu(
        &self,
        menu: PaneContextMenu,
        ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let source = menu.pane.clone();

        let owner_id = source.read(cx).workspace_id;
        let workspace_cwd: Option<PathBuf> = self
            .workspaces
            .iter()
            .find(|ws| ws.id == owner_id)
            .map(|ws| PathBuf::from(&ws.cwd));

        let surface_path = Self::surface_context_path(&source.read(cx).surface, cx);
        let full_path = surface_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        let relative_path = surface_path.as_ref().map(|path| {
            workspace_cwd
                .as_ref()
                .map(|root| files_tree::workspace_relative_path(root, path))
                .unwrap_or_else(|| path.to_string_lossy().into_owned())
        });

        let pending_sid = source
            .read(cx)
            .surface
            .as_terminal()
            .map(|t| t.entity_id().as_u64())
            .filter(|sid| self.broadcast.pending.contains_key(sid));

        let rows = 2 + usize::from(pending_sid.is_some()) + 1;
        let menu_height = px(8. + rows as f32 * 29. + 18.);
        let menu_pos = clamped_context_menu_position(menu.position, px(248.), menu_height, window);

        let mut context_menu = select_menu("pane-context-menu", ui)
            .occlude()
            .absolute()
            .left(menu_pos.x)
            .top(menu_pos.y)
            .w(px(248.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.pane_menu_open = None;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());

        if let Some(value) = full_path {
            context_menu = context_menu.child(self.render_select_menu_item(
                "pane-context-copy-path".into(),
                "Copy Path",
                None,
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                    this.pane_menu_open = None;
                    this.show_toast("Copied path", cx);
                    cx.stop_propagation();
                }),
            ));
        } else {
            context_menu = context_menu.child(Self::render_disabled_select_menu_item(
                "pane-context-copy-path-disabled".into(),
                "Copy Path unavailable",
                ui,
            ));
        }

        if let Some(value) = relative_path {
            context_menu = context_menu.child(self.render_select_menu_item(
                "pane-context-copy-relative-path".into(),
                "Copy Relative Path",
                None,
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                    this.pane_menu_open = None;
                    this.show_toast("Copied relative path", cx);
                    cx.stop_propagation();
                }),
            ));
        } else {
            context_menu = context_menu.child(Self::render_disabled_select_menu_item(
                "pane-context-copy-relative-path-disabled".into(),
                "Copy Relative Path unavailable",
                ui,
            ));
        }

        if let Some(sid) = pending_sid {
            context_menu = context_menu.child(self.render_select_menu_item(
                SharedString::from("pane-cancel-queued"),
                "Cancel queued prompt",
                None,
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.pane_menu_open = None;
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
        context_menu = context_menu.child(self.render_select_menu_item(
            "pane-context-close".into(),
            "Close Pane",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.pane_menu_open = None;
                source_for_close.update(cx, |pane, pane_cx| pane.close(pane_cx));
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

    fn surface_context_path(surface: &PaneSurface, cx: &App) -> Option<PathBuf> {
        match surface {
            PaneSurface::Terminal(terminal) => terminal
                .read(cx)
                .terminal
                .current_cwd
                .as_ref()
                .filter(|cwd| !cwd.is_empty())
                .map(PathBuf::from),
            PaneSurface::Markdown(markdown) => Some(markdown.read(cx).path.clone()),
            PaneSurface::Diff(diff) => Some(diff.read(cx).worktree_path().clone()),
        }
    }
}
