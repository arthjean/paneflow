//! Branch switcher for the diff dock.
//!
//! A Cursor-style chip in the dock's toolbar row (`git-branch  main  v`) opens a
//! searchable list of local branches and runs `git switch` on the picked one.
//! Both git calls are bounded subprocesses run off the render thread; the
//! resulting branch/stat state is folded back into the workspaces and projects
//! rooted at the dock's folder, and the dock's diff is recomputed.

use gpui::{
    AnyElement, AppContext, ClickEvent, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    Window, deferred, div, prelude::FluentBuilder, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::with_alpha;
use crate::ui_primitives::{AnimatedHoverExt, ROW_RADIUS, squircle_skin};
use crate::widgets::text_input::TextInput;

/// Wall-clock ceiling and stdout/stderr cap for the picker's `git` calls.
/// `branch --format` and `switch` are both fast; the bounds only exist so a
/// wedged git (a hook prompting for input, a stale lock) cannot pin a thread.
const BRANCH_GIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
const BRANCH_GIT_OUTPUT_CAP: u64 = 512 * 1024;

/// Open branch picker: the folder it targets, the checked-out branch, the local
/// branches once listed, and the search field. `restore_focus` is whatever held
/// keyboard focus when the chip was clicked, handed back on close (the picker's
/// `TextInput` takes focus while open).
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
    /// Open the picker for `cwd`, listing branches off-thread. Closing is the
    /// chip's job (see [`render_diff_branch_chip`]).
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
        self.agents_view.diff_branch_menu = Some(DiffBranchMenuState {
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
                        let Some(menu) = app.agents_view.diff_branch_menu.as_mut() else {
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

    /// Dismiss the picker and hand keyboard focus back to whoever held it.
    pub(crate) fn close_diff_branch_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.agents_view.diff_branch_menu.take() {
            if let Some(handle) = menu.restore_focus {
                window.focus(&handle, cx);
            }
            cx.notify();
        }
    }

    /// Keyboard commands bubbling out of the focused search field: Enter
    /// switches to an exact match, Escape dismisses. A non-matching query is a
    /// no-op (keep filtering, or click a row).
    pub(crate) fn handle_diff_branch_menu_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agents_view.diff_branch_menu.is_none() {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.close_diff_branch_menu(window, cx),
            "enter" => {
                let resolved = {
                    let Some(menu) = self.agents_view.diff_branch_menu.as_ref() else {
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

    /// Background `git switch` to an existing branch, then refresh the cached git
    /// state for every workspace/project rooted at `cwd` and the dock's diff.
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
                            app.refresh_agents_diff_if_open_for_cwd(&cwd, cx);
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

    /// The checked-out branch of the dock's folder, plus whether it is a repo at
    /// all. Read from the workspaces rooted there, which the git refresh keeps
    /// current (see `spawn_agents_environment_git_refresh`).
    pub(super) fn diff_branch_for_cwd(&self, cwd: &str) -> Option<(String, usize)> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.cwd == cwd && workspace.is_git_repo)
            .map(|workspace| {
                (
                    workspace.git_branch.clone(),
                    workspace.git_stats.files_changed,
                )
            })
    }
}

/// The toolbar chip: `git-branch  <name>  v`, with the picker deferred over it
/// when open. Renders nothing when the folder is not a git repository.
pub(super) fn render_diff_branch_chip(
    cwd: String,
    branch: String,
    files_changed: usize,
    menu: Option<&DiffBranchMenuState>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let menu_open = menu.is_some_and(|menu| menu.cwd == cwd);
    let current = branch.clone();
    let chip_cwd = cwd.clone();

    // The rail's row skin: `ROW_RADIUS` superellipse instead of GPUI's circular
    // `rounded()`, the rail's own hover tint, and its 26 px row box. While the
    // picker is up that hover fill is pinned on as the resting fill, so the chip
    // stays lit for as long as the menu it owns.
    let rail_hover = crate::app::constants::sidebar_tab_hover_background();

    squircle_skin(
        div()
            .id("diff-branch-chip")
            .flex_none()
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
    // Toggle on press, off the render-time `menu_open` snapshot. Both parts
    // matter: the picker dismisses on `on_mouse_down_out` (capture phase of
    // this very press), which repaints before the release - so an `on_click`
    // would run against a *newer* frame whose snapshot already reads closed,
    // and the chip would re-open the menu it just dismissed.
    .on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, _: &gpui::MouseDownEvent, window, cx| {
            cx.stop_propagation();
            if menu_open {
                this.close_diff_branch_menu(window, cx);
            } else {
                this.open_diff_branch_menu(chip_cwd.clone(), current.clone(), window, cx);
            }
        }),
    )
    .child(
        svg()
            .size(px(13.))
            .flex_none()
            .path("icons/git-branch.svg")
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
            .path("icons/chevron-down.svg")
            .text_color(ui.muted),
    )
    .when_some(menu.filter(|_| menu_open), |chip, menu| {
        chip.child(render_diff_branch_menu(menu, files_changed, ui, cx))
    })
    .into_any_element()
}

fn render_diff_branch_menu(
    menu_state: &DiffBranchMenuState,
    files_changed: usize,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let cwd = menu_state.cwd.clone();
    let query_input = menu_state.query_input.clone();
    let query_lc = query_input.read(cx).value().trim().to_lowercase();

    let mut menu = crate::settings::components::menu_surface(div().id("diff-branch-menu"), ui)
        .flex()
        .flex_col()
        .gap(px(2.))
        .p(px(6.))
        .w(px(280.))
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
        menu = menu.child(
            div()
                .px(px(8.))
                .pt(px(4.))
                .pb(px(2.))
                .text_size(px(11.))
                .text_color(ui.muted)
                .child("Branches"),
        );

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
                .gap(px(1.))
                .max_h(px(240.))
                .overflow_y_scroll();
            for (idx, branch) in filtered.into_iter().enumerate() {
                let selected = branch == menu_state.current;
                list = list.child(render_diff_branch_item(
                    idx,
                    branch,
                    selected,
                    if selected { files_changed } else { 0 },
                    cwd.clone(),
                    ui,
                    cx,
                ));
            }
            menu = menu.child(list);
        }
    }

    deferred(
        div()
            .absolute()
            .top(px(28.))
            .left(px(0.))
            .occlude()
            .child(menu),
    )
    .with_priority(3)
    .into_any_element()
}

/// Branch-picker search field. Editing is handled by `TextInput`, while the
/// parent menu handles only Enter/Escape after those keys bubble.
fn render_diff_branch_search_row(
    query_input: Entity<TextInput>,
    ui: crate::theme::UiColors,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .px(px(8.))
        .h(px(30.))
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path("icons/tool_search.svg")
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(13.))
                .text_color(ui.text)
                .child(query_input.into_any_element()),
        )
        .into_any_element()
}

/// One branch row: leading branch glyph, the name, an optional "Uncommitted: N
/// files" sub-label on the checked-out branch, and a trailing check.
fn render_diff_branch_item(
    idx: usize,
    branch: String,
    selected: bool,
    files_changed: usize,
    cwd: String,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let item_branch = branch.clone();
    let selected_background = with_alpha(ui.text, 0.10);
    let resting_background = if selected {
        selected_background
    } else {
        with_alpha(ui.text, 0.0)
    };
    let hover_background = if selected {
        selected_background
    } else {
        with_alpha(ui.text, 0.05)
    };

    div()
        .id(SharedString::from(format!("diff-branch-{idx}")))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(8.))
        .bg(resting_background)
        .animated_hover_bg(resting_background, hover_background)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.close_diff_branch_menu(window, cx);
            this.spawn_switch_diff_branch(cwd.clone(), item_branch.clone(), cx);
        }))
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path("icons/git-branch.svg")
                .text_color(ui.muted),
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
                        .text_size(px(13.))
                        .text_color(ui.text)
                        .child(branch),
                )
                .when(files_changed > 0, |d| {
                    d.child(div().text_size(px(11.)).text_color(ui.muted).child(format!(
                        "Uncommitted: {files_changed} file{}",
                        if files_changed > 1 { "s" } else { "" }
                    )))
                }),
        )
        .child(div().w(px(14.)).flex_none().child(if selected {
            svg()
                .size(px(14.))
                .path("icons/check.svg")
                .text_color(ui.text)
                .into_any_element()
        } else {
            div().size(px(14.)).into_any_element()
        }))
        .into_any_element()
}

fn render_diff_branch_menu_status(
    label: impl Into<String>,
    ui: crate::theme::UiColors,
) -> AnyElement {
    div()
        .h(px(28.))
        .px(px(8.))
        .flex()
        .items_center()
        .text_size(px(12.))
        .text_color(ui.muted)
        .child(label.into())
        .into_any_element()
}

fn list_branches(cwd: &str) -> Result<Vec<String>, String> {
    let mut command = std::process::Command::new("git");
    command
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0");

    let output =
        paneflow_process::run_with_timeout(command, BRANCH_GIT_DEADLINE, BRANCH_GIT_OUTPUT_CAP)
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
    let mut command = std::process::Command::new("git");
    command
        .args(["switch", "--", branch])
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0");

    let output =
        paneflow_process::run_with_timeout(command, BRANCH_GIT_DEADLINE, BRANCH_GIT_OUTPUT_CAP)
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
