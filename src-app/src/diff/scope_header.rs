use crate::PaneFlowApp;
use crate::diff::DiffScope;
use crate::settings::components::{menu_surface, select_item};
use crate::ui_primitives::AnimatedHoverExt;
use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, deferred, div, prelude::*, px, svg,
};

impl PaneFlowApp {
    pub(crate) fn render_scope_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let active = self.diff_mode.diff_scope;
        let open = self.diff_mode.diff_scope_picker_open;
        let trigger_bg = if open {
            ui.subtle
        } else {
            ui.subtle.opacity(0.0)
        };

        let trigger = div()
            .id("diff-scope-trigger")
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.))
            .h(px(22.))
            .px(px(7.))
            .rounded(px(5.))
            .bg(trigger_bg)
            .text_size(crate::ui_primitives::BODY)
            .text_color(ui.text)
            .animated_hover_bg(trigger_bg, ui.subtle)
            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.diff_mode.diff_scope_picker_open = !this.diff_mode.diff_scope_picker_open;
                this.diff_mode.diff_project_picker_open = false;
                this.diff_mode.diff_worktree_picker_open = false;
                cx.notify();
            }))
            .child(
                svg()
                    .size(px(13.))
                    .flex_none()
                    .path("icons/git-pull-request.svg")
                    .text_color(ui.muted),
            )
            .child(active.label())
            .child(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path("icons/chevron-down.svg")
                    .text_color(ui.muted),
            );

        let popover: Option<AnyElement> = if open {
            let mut menu = menu_surface(div().id("diff-scope-popover"), ui)
                .occlude()
                .absolute()
                .left(px(8.))
                .top(px(30.))
                .flex()
                .flex_col()
                .gap(px(1.))
                .p(px(4.))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this.diff_mode.diff_scope_picker_open {
                        this.diff_mode.diff_scope_picker_open = false;
                        cx.notify();
                    }
                }));
            for scope in DiffScope::all() {
                let is_active = scope == active;
                menu = menu.child(
                    select_item(
                        SharedString::from(format!("diff-scope-{}", scope.label())),
                        is_active,
                        ui,
                    )
                    .cursor(CursorStyle::Arrow)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.diff_mode.diff_scope_picker_open = false;
                        this.diff_mode.diff_project_picker_open = false;
                        this.diff_mode.diff_worktree_picker_open = false;
                        if this.diff_mode.diff_scope != scope {
                            this.diff_mode.diff_scope = scope;
                            this.rebuild_diff_view(cx);
                            this.save_session(cx);
                        } else {
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .flex_none()
                            .w(px(12.))
                            .text_color(ui.accent)
                            .child(if is_active { "✓" } else { "" }),
                    )
                    .child(
                        div()
                            .text_color(if is_active { ui.text } else { ui.muted })
                            .child(scope.label()),
                    ),
                );
            }
            Some(deferred(menu).with_priority(4).into_any_element())
        } else {
            None
        };

        let show_project = active != DiffScope::MultiProject;
        let project_open = self.diff_mode.diff_project_picker_open;
        let project_trigger_bg = if project_open {
            ui.subtle
        } else {
            ui.subtle.opacity(0.0)
        };
        let project_label = self
            .workspaces
            .get(self.active_idx)
            .and_then(|ws| ws.repo_root.as_ref())
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "No project".to_string());

        let project_trigger = div()
            .id("diff-project-trigger")
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.))
            .h(px(22.))
            .px(px(7.))
            .rounded(px(5.))
            .bg(project_trigger_bg)
            .text_size(crate::ui_primitives::BODY)
            .text_color(ui.muted)
            .animated_hover_bg(project_trigger_bg, ui.subtle)
            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.diff_mode.diff_project_picker_open = !this.diff_mode.diff_project_picker_open;
                this.diff_mode.diff_scope_picker_open = false;
                this.diff_mode.diff_worktree_picker_open = false;
                cx.notify();
            }))
            .child(
                svg()
                    .size(px(13.))
                    .flex_none()
                    .path("icons/folder.svg")
                    .text_color(ui.muted),
            )
            .child(project_label)
            .child(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path("icons/chevron-down.svg")
                    .text_color(ui.muted),
            );

        let project_popover: Option<AnyElement> = if project_open {
            let mut menu = menu_surface(div().id("diff-project-popover"), ui)
                .occlude()
                .absolute()
                .left(px(0.))
                .top(px(28.))
                .flex()
                .flex_col()
                .gap(px(1.))
                .p(px(4.))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this.diff_mode.diff_project_picker_open {
                        this.diff_mode.diff_project_picker_open = false;
                        cx.notify();
                    }
                }));
            let repo_workspaces: Vec<(usize, String, String)> = self
                .workspaces
                .iter()
                .enumerate()
                .filter_map(|(idx, ws)| {
                    let root = ws.repo_root.as_ref()?;
                    let name = root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| root.display().to_string());
                    Some((idx, name, ws.git_branch.clone()))
                })
                .collect();
            if repo_workspaces.is_empty() {
                menu = menu.child(
                    div()
                        .px(px(8.))
                        .py(px(3.))
                        .text_size(crate::ui_primitives::BODY)
                        .text_color(ui.muted)
                        .child("No git projects open"),
                );
            } else {
                for (idx, name, branch) in repo_workspaces {
                    let is_active = idx == self.active_idx;
                    menu = menu.child(
                        select_item(
                            SharedString::from(format!("diff-project-{idx}")),
                            is_active,
                            ui,
                        )
                        .cursor(CursorStyle::Arrow)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.diff_mode.diff_project_picker_open = false;
                            this.select_workspace(idx, window, cx);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex_none()
                                .w(px(12.))
                                .text_color(ui.accent)
                                .child(if is_active { "✓" } else { "" }),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(if is_active { ui.text } else { ui.muted })
                                .child(name),
                        )
                        .when(!branch.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_size(crate::ui_primitives::LABEL_SM)
                                    .text_color(ui.muted)
                                    .child(format!("· {branch}")),
                            )
                        }),
                    );
                }
            }
            Some(deferred(menu).with_priority(4).into_any_element())
        } else {
            None
        };

        let repo_root = self
            .workspaces
            .get(self.active_idx)
            .and_then(|ws| ws.repo_root.clone());
        let show_branches = active == DiffScope::Worktree && repo_root.is_some();
        let branches_open = self.diff_mode.diff_worktree_picker_open;
        let (branches_trigger, branches_popover): (Option<AnyElement>, Option<AnyElement>) =
            match repo_root.clone().filter(|_| show_branches) {
                Some(root) => {
                    let total = (self.diff_mode.diff_available_repo.as_deref()
                        == Some(root.as_path()))
                    .then_some(self.diff_mode.diff_available_worktrees.len())
                    .filter(|n| *n > 0);
                    let label = match (self.diff_mode.diff_chosen_worktrees.get(&root), total) {
                        (Some(s), Some(t)) => format!("{}/{t} branches", s.len()),
                        (Some(s), None) => format!("{} branches", s.len()),
                        (None, Some(t)) => format!("All {t} branches"),
                        (None, None) => "All branches".to_string(),
                    };
                    let trig_root = root.clone();
                    let trigger_bg = if branches_open {
                        ui.subtle
                    } else {
                        ui.subtle.opacity(0.0)
                    };
                    let trigger = div()
                        .id("diff-branches-trigger")
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.))
                        .h(px(22.))
                        .px(px(7.))
                        .rounded(px(5.))
                        .bg(trigger_bg)
                        .text_size(crate::ui_primitives::BODY)
                        .text_color(ui.muted)
                        .animated_hover_bg(trigger_bg, ui.subtle)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            this.diff_mode.diff_worktree_picker_open =
                                !this.diff_mode.diff_worktree_picker_open;
                            this.diff_mode.diff_scope_picker_open = false;
                            this.diff_mode.diff_project_picker_open = false;
                            if this.diff_mode.diff_worktree_picker_open {
                                this.refresh_diff_available_worktrees(trig_root.clone(), cx);
                            }
                            cx.notify();
                        }))
                        .child(
                            svg()
                                .size(px(13.))
                                .flex_none()
                                .path("icons/git-branch.svg")
                                .text_color(ui.muted),
                        )
                        .child(label)
                        .child(
                            svg()
                                .size(px(12.))
                                .flex_none()
                                .path("icons/chevron-down.svg")
                                .text_color(ui.muted),
                        );

                    let popover: Option<AnyElement> = if branches_open {
                        let shell = menu_surface(div().id("diff-branches-popover"), ui)
                            .occlude()
                            .absolute()
                            .left(px(0.))
                            .top(px(28.))
                            .flex()
                            .flex_col()
                            .max_h(px(320.))
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                if this.diff_mode.diff_worktree_picker_open {
                                    this.diff_mode.diff_worktree_picker_open = false;
                                    cx.notify();
                                }
                            }));
                        let mut menu = div()
                            .id("diff-branches-popover-list")
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .p(px(4.))
                            .w_full()
                            .min_h_0()
                            .overflow_y_scroll();
                        if self.diff_mode.diff_available_worktrees.is_empty() {
                            menu = menu.child(
                                div()
                                    .px(px(8.))
                                    .py(px(4.))
                                    .text_size(crate::ui_primitives::BODY)
                                    .text_color(ui.muted)
                                    .child("Loading worktrees…"),
                            );
                        } else {
                            for w in &self.diff_mode.diff_available_worktrees {
                                let path_str = w.path.to_string_lossy().into_owned();
                                let chosen = self.diff_worktree_is_chosen(&root, &path_str);
                                let row_root = root.clone();
                                let row_path = path_str.clone();
                                let dir_tail = w
                                    .path
                                    .parent()
                                    .map(|p| p.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                menu = menu.child(
                                    select_item(
                                        SharedString::from(format!("diff-branch-opt-{path_str}")),
                                        chosen,
                                        ui,
                                    )
                                    .cursor(CursorStyle::Arrow)
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                                        this.toggle_chosen_worktree(
                                            row_root.clone(),
                                            row_path.clone(),
                                            cx,
                                        );
                                    }))
                                    .child(
                                        div()
                                            .flex_none()
                                            .w(px(13.))
                                            .text_size(crate::ui_primitives::BODY)
                                            .text_color(ui.accent)
                                            .child(if chosen { "✓" } else { "" }),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(crate::ui_primitives::BODY)
                                            .text_color(if chosen { ui.text } else { ui.muted })
                                            .child(w.branch.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(crate::ui_primitives::LABEL_SM)
                                            .text_color(ui.muted)
                                            .child(dir_tail),
                                    ),
                                );
                            }
                        }
                        Some(
                            deferred(shell.child(menu))
                                .with_priority(4)
                                .into_any_element(),
                        )
                    } else {
                        None
                    };
                    (Some(trigger.into_any_element()), popover)
                }
                None => (None, None),
            };

        div()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .flex_none()
            .child(trigger)
            .children(popover)
            .when(show_project, |d| {
                d.child(
                    svg()
                        .size(px(13.))
                        .flex_none()
                        .path("icons/chevron-right.svg")
                        .text_color(ui.muted),
                )
                .child(
                    div()
                        .relative()
                        .child(project_trigger)
                        .children(project_popover),
                )
            })
            .when(show_branches, |d| {
                d.child(
                    svg()
                        .size(px(13.))
                        .flex_none()
                        .path("icons/chevron-right.svg")
                        .text_color(ui.muted),
                )
                .child(
                    div()
                        .relative()
                        .children(branches_trigger)
                        .children(branches_popover),
                )
            })
            .into_any_element()
    }
}
