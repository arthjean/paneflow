use gpui::{
    AnyElement, AppContext, ClickEvent, Context, CursorStyle, Entity, FocusHandle,
    InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, deferred, div, prelude::FluentBuilder, px, svg,
};

use crate::PaneFlowApp;
use crate::app::menu_icons as icon;
use crate::settings::components::with_alpha;
use crate::ui_primitives::action_menu::{
    CHECK_SIZE, GLYPH_SIZE, HIGHLIGHT_ALPHA, LABEL_TEXT, PANEL_PADDING, ROW_GAP, ROW_HEIGHT,
    ROW_INSET, ROW_RADIUS as MENU_ROW_RADIUS, SEPARATOR_ALPHA, SHORTCUT_TEXT, menu_heading,
    panel_shell,
};
use crate::ui_primitives::{AccessibleControlExt, ROW_RADIUS, squircle_skin};
use crate::widgets::text_input::TextInput;

const BRANCH_GIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
const BRANCH_GIT_OUTPUT_CAP: u64 = 512 * 1024;

pub(crate) struct DiffBranchMenuState {
    pub(crate) cwd: String,
    pub(crate) current: String,
    pub(crate) branches: Vec<String>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) query_input: Entity<TextInput>,
    pub(crate) restore_focus: Option<FocusHandle>,
}

impl PaneFlowApp {
    fn open_diff_branch_menu(
        &mut self,
        cwd: String,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cwd.trim().is_empty() {
            return;
        }

        let query_input = cx.new(|cx| TextInput::new("", "Search branches", cx));
        cx.observe(&query_input, |_, _, cx| cx.notify()).detach();
        self.diff_dock.diff_branch_menu = Some(DiffBranchMenuState {
            cwd: cwd.clone(),
            current,
            branches: Vec::new(),
            loading: true,
            error: None,
            query_input: query_input.clone(),
            restore_focus: window.focused(cx),
        });
        query_input.read(cx).focus_handle.clone().focus(window, cx);
        cx.notify();

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let result = smol::unblock({
                    let cwd = cwd.clone();
                    move || list_branches(&cwd)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app, cx| {
                        let Some(menu) = app.diff_dock.diff_branch_menu.as_mut() else {
                            return;
                        };
                        if menu.cwd != cwd {
                            return;
                        }
                        menu.loading = false;
                        match result {
                            Ok(branches) => {
                                menu.branches = branches;
                                menu.error = None;
                            }
                            Err(error) => {
                                menu.branches.clear();
                                menu.error = Some(error);
                            }
                        }
                        cx.notify();
                    })
                });
            },
        )
        .detach();
    }

    pub(crate) fn close_diff_branch_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.diff_dock.diff_branch_menu.take() {
            if let Some(handle) = menu.restore_focus {
                window.focus(&handle, cx);
            }
            cx.notify();
        }
    }

    pub(crate) fn handle_diff_branch_menu_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.diff_dock.diff_branch_menu.is_none() {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.close_diff_branch_menu(window, cx),
            "enter" => {
                let resolved = {
                    let Some(menu) = self.diff_dock.diff_branch_menu.as_ref() else {
                        return;
                    };
                    let name = menu.query_input.read(cx).value().trim().to_string();
                    if name.is_empty() || !menu.branches.contains(&name) {
                        return;
                    }
                    (menu.cwd.clone(), name)
                };
                let (cwd, branch) = resolved;
                self.close_diff_branch_menu(window, cx);
                self.spawn_switch_diff_branch(cwd, branch, cx);
            }
            _ => {}
        }
    }

    fn spawn_switch_diff_branch(&mut self, cwd: String, branch: String, cx: &mut Context<Self>) {
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let result = smol::unblock({
                    let cwd = cwd.clone();
                    let branch = branch.clone();
                    move || switch_branch(&cwd, &branch)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app, cx| match result {
                        Ok((branch_now, is_repo, stats)) => {
                            app.apply_git_state_for_cwd(&cwd, branch_now, is_repo, stats);
                            app.refresh_diff_dock_if_open_for_cwd(&cwd, cx);
                            app.show_toast(format!("Switched to {branch}"), cx);
                            cx.notify();
                        }
                        Err(error) => {
                            app.show_toast(format!("Couldn't switch to {branch}: {error}"), cx);
                        }
                    })
                });
            },
        )
        .detach();
    }

    pub(super) fn diff_branch_for_cwd(&self, cwd: &str) -> Option<(String, Option<String>)> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.cwd == cwd && workspace.is_git_repo)
            .map(|workspace| {
                let stats = &workspace.git_stats;
                (
                    workspace.git_branch.clone(),
                    (stats.files_changed > 0)
                        .then(|| format!("Uncommitted: {}", stats.files_changed_label())),
                )
            })
    }
}

pub(super) fn render_diff_branch_chip(
    cwd: String,
    branch: String,
    uncommitted: Option<String>,
    menu: Option<&DiffBranchMenuState>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let menu_open = menu.is_some_and(|menu| menu.cwd == cwd);
    let current = branch.clone();
    let chip_cwd = cwd.clone();

    let rail_hover = crate::app::constants::sidebar_tab_hover_background();

    squircle_skin(
        div()
            .id("diff-branch-chip")
            .flex_shrink(1.)
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.))
            .h(px(26.))
            .px(px(8.))
            .max_w(px(220.)),
        "diff-branch-chip-group",
        ROW_RADIUS,
        menu_open.then_some(rail_hover),
        Some(rail_hover),
    )
    .accessible_control(gpui::accesskit::Role::ComboBox, format!("Branch {branch}"))
    .aria_expanded(menu_open)
    .on_press(cx.listener(move |this, _, window, cx| {
        cx.stop_propagation();
        if menu_open {
            this.close_diff_branch_menu(window, cx);
        } else {
            this.open_diff_branch_menu(chip_cwd.clone(), current.clone(), window, cx);
        }
    }))
    .child(
        svg()
            .size(px(13.))
            .flex_none()
            .path("icons/dock/git-branch.svg")
            .text_color(ui.muted),
    )
    .child(
        div()
            .min_w_0()
            .overflow_x_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(crate::ui_primitives::LABEL_SM)
            .text_color(ui.text)
            .child(SharedString::from(branch)),
    )
    .child(
        svg()
            .size(px(12.))
            .flex_none()
            .path("icons/dock/arrow-down-01.svg")
            .text_color(ui.muted),
    )
    .when_some(menu.filter(|_| menu_open), |chip, menu| {
        chip.child(render_diff_branch_menu(menu, uncommitted, ui, cx))
    })
    .into_any_element()
}

fn render_diff_branch_menu(
    menu_state: &DiffBranchMenuState,
    uncommitted: Option<String>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let cwd = menu_state.cwd.clone();
    let query_input = menu_state.query_input.clone();
    let query_lc = query_input.read(cx).value().trim().to_lowercase();

    let mut menu = panel_shell(div().id("diff-branch-menu"), ui)
        .w(px(280.))
        .p(PANEL_PADDING)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down_out(cx.listener(|this, _: &gpui::MouseDownEvent, window, cx| {
            this.close_diff_branch_menu(window, cx);
        }))
        .on_key_down(cx.listener(PaneFlowApp::handle_diff_branch_menu_key_down))
        .child(render_diff_branch_search_row(query_input, ui));

    if menu_state.loading {
        menu = menu.child(render_diff_branch_menu_status("Loading branches…", ui));
    } else if let Some(error) = &menu_state.error {
        menu = menu.child(render_diff_branch_menu_status(error.clone(), ui));
    } else {
        menu = menu.child(menu_heading("Branches".into(), ui));

        let filtered: Vec<String> = menu_state
            .branches
            .iter()
            .filter(|branch| query_lc.is_empty() || branch.to_lowercase().contains(&query_lc))
            .cloned()
            .collect();

        if filtered.is_empty() {
            menu = menu.child(render_diff_branch_menu_status("No branches", ui));
        } else {
            let mut list = div()
                .id("diff-branch-list")
                .flex()
                .flex_col()
                .max_h(px(240.))
                .overflow_y_scroll();
            for (idx, branch) in filtered.into_iter().enumerate() {
                let selected = branch == menu_state.current;
                list = list.child(render_diff_branch_item(
                    idx,
                    branch,
                    selected,
                    uncommitted.clone().filter(|_| selected),
                    cwd.clone(),
                    ui,
                    cx,
                ));
            }
            menu = menu.child(list);
        }
    }

    deferred(crate::ui_primitives::menu_reveal(
        "diff-branch-menu-reveal",
        div()
            .absolute()
            .top(px(28.))
            .left(px(0.))
            .occlude()
            .child(menu),
    ))
    .with_priority(3)
    .into_any_element()
}

fn render_diff_branch_search_row(
    query_input: Entity<TextInput>,
    ui: crate::theme::UiColors,
) -> AnyElement {
    div()
        .px(ROW_INSET)
        .pt(px(6.))
        .pb(px(4.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(ROW_GAP)
                .pb(px(6.))
                .border_b_1()
                .border_color(with_alpha(ui.text, SEPARATOR_ALPHA))
                .child(
                    svg()
                        .size(GLYPH_SIZE)
                        .flex_none()
                        .path("icons/dock/search-01.svg")
                        .text_color(with_alpha(ui.muted, 0.55)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(LABEL_TEXT)
                        .text_color(ui.text)
                        .child(query_input.into_any_element()),
                ),
        )
        .into_any_element()
}

fn render_diff_branch_item(
    idx: usize,
    branch: String,
    selected: bool,
    uncommitted: Option<String>,
    cwd: String,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let item_branch = branch.clone();

    div()
        .id(SharedString::from(format!("diff-branch-{idx}")))
        .flex_none()
        .min_h(ROW_HEIGHT)
        .py(px(4.))
        .px(ROW_INSET)
        .flex()
        .flex_row()
        .items_center()
        .gap(ROW_GAP)
        .rounded(MENU_ROW_RADIUS)
        .cursor(CursorStyle::Arrow)
        .hover(|style| style.bg(with_alpha(ui.text, HIGHLIGHT_ALPHA)))
        .role(gpui::Role::MenuItemRadio)
        .aria_label(SharedString::from(branch.clone()))
        .aria_toggled(if selected {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_activate(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.close_diff_branch_menu(window, cx);
            this.spawn_switch_diff_branch(cwd.clone(), item_branch.clone(), cx);
        }))
        .child(
            svg()
                .size(GLYPH_SIZE)
                .flex_none()
                .path(icon::GIT_BRANCH)
                .text_color(with_alpha(ui.muted, 0.8)),
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
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(LABEL_TEXT)
                        .text_color(ui.text)
                        .child(branch),
                )
                .when_some(uncommitted, |d, label| {
                    d.child(
                        div()
                            .text_size(SHORTCUT_TEXT)
                            .text_color(ui.muted)
                            .child(label),
                    )
                }),
        )
        .child(div().size(CHECK_SIZE).flex_none().when(selected, |slot| {
            slot.child(svg().size(CHECK_SIZE).path(icon::CHECK).text_color(ui.text))
        }))
        .into_any_element()
}

fn render_diff_branch_menu_status(
    label: impl Into<String>,
    ui: crate::theme::UiColors,
) -> AnyElement {
    div()
        .h(ROW_HEIGHT)
        .px(ROW_INSET)
        .flex()
        .items_center()
        .text_size(SHORTCUT_TEXT)
        .text_color(ui.muted)
        .child(label.into())
        .into_any_element()
}

fn list_branches(cwd: &str) -> Result<Vec<String>, String> {
    let mut command = crate::git_command::git(
        crate::git_command::GitProfile::Probe,
        ["branch", "--format=%(refname:lstrip=2)"],
    );
    command.current_dir(cwd);

    let output = crate::git_command::run(command, BRANCH_GIT_DEADLINE, BRANCH_GIT_OUTPUT_CAP)
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(git_output_error(&output));
    }

    let mut branches = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    branches.sort();
    branches.dedup();
    Ok(branches)
}

fn switch_branch(
    cwd: &str,
    branch: &str,
) -> Result<(String, bool, crate::workspace::GitDiffStats), String> {
    let mut command = crate::git_command::git(
        crate::git_command::GitProfile::UserAction,
        ["switch", "--", branch],
    );
    command.current_dir(cwd);

    let output = crate::git_command::run(command, BRANCH_GIT_DEADLINE, BRANCH_GIT_OUTPUT_CAP)
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(git_output_error(&output));
    }

    let (branch_now, is_repo) = crate::workspace::detect_branch(cwd);
    Ok((
        branch_now,
        is_repo,
        crate::workspace::GitDiffStats::from_cwd(cwd),
    ))
}

fn git_output_error(output: &paneflow_process::BoundedOutput) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr.trim();
    if message.is_empty() {
        format!("git exited with {}", output.status)
    } else {
        message.lines().next().unwrap_or(message).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::list_branches;

    use crate::git_fixture as fixture;

    #[test]
    fn the_branch_menu_lists_a_branch_shadowed_by_a_tag_by_its_plain_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fixture::committed_repo(root, &[("README.md", "init\n")]);
        fixture::run(root, &["branch", "release"]);
        fixture::run(root, &["tag", "release"]);

        let branches = list_branches(root.to_str().expect("utf-8 path")).expect("branches");

        assert!(branches.contains(&"release".to_string()), "{branches:?}");
        assert!(!branches.iter().any(|branch| branch.starts_with("heads/")));
    }
}
