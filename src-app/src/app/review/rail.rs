use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AppContext, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::*, px,
    svg,
};

use super::{REVIEW_WORKSPACES_RAIL_WIDTH, ReviewRailMenu};
use crate::PaneFlowApp;
use crate::diff::{DiffWorktree, ReviewSubject};
use crate::pane_drag::{DragPreview, ReviewSubjectDrag};
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};
use crate::workspace::GitDiffStats;

const RAIL_ROW_MARGIN_X: f32 = 8.0;
const RAIL_ROW_PADDING_X: f32 = 8.0;
const RAIL_ROW_HEIGHT: f32 = 30.0;
const RAIL_ROW_GAP: f32 = 4.0;
const RAIL_CHILD_INDENT: f32 = 18.0;
const RAIL_ICON_SIZE: f32 = 14.0;
const RAIL_DOT_SIZE: f32 = 6.0;

pub(crate) struct ReviewCheckout {
    pub(crate) subject: ReviewSubject,
    pub(crate) label: String,
    pub(crate) stats: Option<GitDiffStats>,
}

pub(crate) struct ReviewWorkspace {
    pub(crate) repo_root: PathBuf,
    pub(crate) name: String,
    pub(crate) checkouts: Vec<ReviewCheckout>,
}

impl ReviewWorkspace {
    pub(crate) fn primary(&self) -> Option<&ReviewCheckout> {
        self.checkouts
            .iter()
            .find(|checkout| checkout.subject.worktree.path == self.repo_root)
            .or_else(|| self.checkouts.first())
    }
}

pub(crate) fn norm_path(path: &Path) -> String {
    let resolved = normalize_lexically(path);
    let s = resolved.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        s.to_lowercase()
    } else {
        s
    }
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            _ => out.push(component.as_os_str()),
        }
    }
    out
}

fn checkout_label(branch: &str, path: &Path, repo_root: &Path) -> String {
    let label = crate::workspace::worktree::checkout_label(Some(branch), path, repo_root);
    if path == repo_root {
        label
    } else {
        format!("wt · {label}")
    }
}

fn push_checkout(
    workspace: &mut ReviewWorkspace,
    seen: &mut HashSet<String>,
    path: PathBuf,
    branch: String,
    stats: Option<GitDiffStats>,
    workspace_id: Option<u64>,
) {
    if !seen.insert(norm_path(&path)) {
        return;
    }
    let label = checkout_label(&branch, &path, &workspace.repo_root);
    workspace.checkouts.push(ReviewCheckout {
        subject: ReviewSubject {
            repo_root: workspace.repo_root.clone(),
            worktree: DiffWorktree {
                path,
                branch,
                workspace_id,
            },
        },
        label,
        stats,
    });
}

fn diffstat(stats: &GitDiffStats, ui: crate::theme::UiColors) -> Option<AnyElement> {
    if stats.insertions == 0 && stats.deletions == 0 {
        return None;
    }
    Some(
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.))
            .text_size(crate::ui_primitives::LABEL_SM)
            .when(stats.insertions > 0, |row| {
                row.child(
                    div()
                        .text_color(ui.vc_added)
                        .child(format!("+{}", stats.insertions)),
                )
            })
            .when(stats.deletions > 0, |row| {
                row.child(
                    div()
                        .text_color(ui.vc_deleted)
                        .child(format!("\u{2212}{}", stats.deletions)),
                )
            })
            .into_any_element(),
    )
}

impl PaneFlowApp {
    pub(crate) fn review_workspaces(&self) -> Vec<ReviewWorkspace> {
        let mut workspaces: Vec<ReviewWorkspace> = Vec::new();
        let mut seen: Vec<HashSet<String>> = Vec::new();
        for ws in &self.workspaces {
            let Some(root) = ws.repo_root.clone() else {
                continue;
            };
            let index = match workspaces.iter().position(|p| p.repo_root == root) {
                Some(index) => index,
                None => {
                    let name = root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| root.display().to_string());
                    workspaces.push(ReviewWorkspace {
                        repo_root: root.clone(),
                        name,
                        checkouts: Vec::new(),
                    });
                    seen.push(HashSet::new());
                    workspaces.len() - 1
                }
            };
            push_checkout(
                &mut workspaces[index],
                &mut seen[index],
                ws.worktree_root.clone(),
                ws.git_branch.clone(),
                ws.is_git_repo.then(|| ws.git_stats.clone()),
                Some(ws.id),
            );
            for tab in ws.tabs() {
                let Some(path) = tab.worktree.clone() else {
                    continue;
                };
                let Some(git) = self.tab_checkout_git(tab).filter(|git| git.is_repo) else {
                    continue;
                };
                push_checkout(
                    &mut workspaces[index],
                    &mut seen[index],
                    path,
                    git.branch.clone(),
                    Some(git.stats.clone()),
                    Some(ws.id),
                );
            }
        }
        for (index, workspace) in workspaces.iter_mut().enumerate() {
            let listing = self
                .worktree_states
                .listing(&workspace.repo_root.to_string_lossy())
                .to_vec();
            for entry in listing {
                push_checkout(
                    workspace,
                    &mut seen[index],
                    entry.path,
                    entry.branch.unwrap_or_default(),
                    None,
                    None,
                );
            }
        }
        workspaces
    }

    pub(crate) fn render_review_workspaces_rail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();
        let workspaces = self.review_workspaces();
        let focused = self.review_focused_subject(cx);
        let in_grid = self.review_grid_subjects(cx);

        let mut list = div()
            .id("review-workspaces-list")
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(RAIL_ROW_GAP))
            .pb(px(4.));

        if workspaces.is_empty() {
            list = list.child(crate::ui_primitives::panel_empty_state(
                ui,
                Some("icons/git-branch.svg"),
                Some("No Git repository".into()),
                "Open a workspace backed by a Git repo to review its changes here.",
                false,
            ));
        }

        for workspace in &workspaces {
            let expanded = !self.review.collapsed.contains(&workspace.repo_root);
            list = list.child(self.render_review_workspace_row(workspace, expanded, ui, cx));
            if expanded {
                for checkout in &workspace.checkouts {
                    list = list.child(self.render_review_checkout_row(
                        checkout,
                        focused.as_ref(),
                        &in_grid,
                        ui,
                        cx,
                    ));
                }
            }
        }

        div()
            .relative()
            .w(px(REVIEW_WORKSPACES_RAIL_WIDTH))
            .flex_shrink_0()
            .h_full()
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                window.is_window_active(),
                self.cached_config.cockpit_chrome_material_enabled(),
            ))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(36.))
                    .flex_none()
                    .px(px(RAIL_ROW_MARGIN_X))
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .pl(px(RAIL_ROW_PADDING_X))
                            .text_size(px(13.))
                            .text_color(ui.muted)
                            .child("Workspaces"),
                    ),
            )
            .child(list)
            .child(self.render_sidebar_settings_footer(cx))
            .into_any_element()
    }

    fn render_review_workspace_row(
        &self,
        workspace: &ReviewWorkspace,
        expanded: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(primary) = workspace.primary() else {
            return div().into_any_element();
        };
        let subject = primary.subject.clone();
        let key = norm_path(&workspace.repo_root);
        let group = SharedString::from(format!("review-workspace-{key}"));
        let repo_root = workspace.repo_root.clone();
        let title: SharedString = workspace.name.clone().into();
        let hover = Some(crate::app::constants::sidebar_tab_hover_background());

        let body = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .h(px(RAIL_ROW_HEIGHT))
            .px(px(RAIL_ROW_PADDING_X))
            .min_w_0()
            .child(
                svg()
                    .size(px(RAIL_ICON_SIZE))
                    .flex_none()
                    .path(if expanded {
                        "icons/folder-open.svg"
                    } else {
                        "icons/folder.svg"
                    })
                    .text_color(ui.muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui.text)
                    .child(title.clone()),
            );

        let menu_subject = subject.clone();
        let drag_subject = subject;
        let shell = div()
            .id(SharedString::from(format!("review-workspace-{key}")))
            .flex_none()
            .mx(px(RAIL_ROW_MARGIN_X))
            .on_drag(
                ReviewSubjectDrag {
                    subject: drag_subject,
                    title: title.clone(),
                },
                |drag, _offset, _window, cx| {
                    cx.new(|_| DragPreview {
                        title: drag.title.clone(),
                        icon: "icons/git-branch.svg".into(),
                    })
                },
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.dismiss_transient_surfaces();
                if !this.review.collapsed.remove(&repo_root) {
                    this.review.collapsed.insert(repo_root.clone());
                }
                this.save_session(cx);
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, e: &ClickEvent, _w, cx| {
                if e.is_right_click()
                    && let Some(position) = e.mouse_position()
                {
                    this.dismiss_transient_surfaces();
                    this.review.rail_menu = Some(ReviewRailMenu {
                        subject: menu_subject.clone(),
                        position,
                    });
                    cx.stop_propagation();
                    cx.notify();
                }
            }));
        squircle_skin(shell, group, ROW_RADIUS, None, hover)
            .child(body)
            .into_any_element()
    }

    fn render_review_checkout_row(
        &self,
        checkout: &ReviewCheckout,
        focused: Option<&ReviewSubject>,
        in_grid: &[ReviewSubject],
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let subject = checkout.subject.clone();
        let is_focused = focused.is_some_and(|f| f.same_worktree(&subject));
        let is_in_grid = in_grid.iter().any(|s| s.same_worktree(&subject));
        let key = norm_path(&subject.worktree.path);
        let group = SharedString::from(format!("review-checkout-{key}"));
        let title: SharedString = subject.label().into();

        let resting = is_focused.then(crate::app::constants::sidebar_tab_active_background);
        let hover = (!is_focused).then(crate::app::constants::sidebar_tab_hover_background);

        let body = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .h(px(RAIL_ROW_HEIGHT))
            .pl(px(RAIL_ROW_PADDING_X + RAIL_CHILD_INDENT))
            .pr(px(RAIL_ROW_PADDING_X))
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .size(px(RAIL_DOT_SIZE))
                    .rounded_full()
                    .bg(if is_in_grid {
                        ui.accent
                    } else {
                        ui.accent.opacity(0.0)
                    }),
            )
            .child(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path("icons/git-branch-sidebar.svg")
                    .text_color(ui.muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_sm()
                    .text_color(if is_focused { ui.text } else { ui.muted })
                    .child(checkout.label.clone()),
            )
            .children(
                checkout
                    .stats
                    .as_ref()
                    .and_then(|stats| diffstat(stats, ui)),
            );

        let click_subject = subject.clone();
        let menu_subject = subject.clone();
        let drag_subject = subject;
        let shell = div()
            .id(SharedString::from(format!("review-checkout-{key}")))
            .flex_none()
            .mx(px(RAIL_ROW_MARGIN_X))
            .on_drag(
                ReviewSubjectDrag {
                    subject: drag_subject,
                    title,
                },
                |drag, _offset, _window, cx| {
                    cx.new(|_| DragPreview {
                        title: drag.title.clone(),
                        icon: "icons/git-branch.svg".into(),
                    })
                },
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.dismiss_transient_surfaces();
                this.review_show_subject(click_subject.clone(), cx);
            }))
            .on_aux_click(cx.listener(move |this, e: &ClickEvent, _w, cx| {
                if e.is_right_click()
                    && let Some(position) = e.mouse_position()
                {
                    this.dismiss_transient_surfaces();
                    this.review.rail_menu = Some(ReviewRailMenu {
                        subject: menu_subject.clone(),
                        position,
                    });
                    cx.stop_propagation();
                    cx.notify();
                }
            }));
        squircle_skin(shell, group, ROW_RADIUS, resting, hover)
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(root: &str) -> ReviewWorkspace {
        ReviewWorkspace {
            repo_root: PathBuf::from(root),
            name: "repo".into(),
            checkouts: Vec::new(),
        }
    }

    #[test]
    fn push_checkout_dedups_equivalent_paths() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();

        let mut out = workspace(&repo.to_string_lossy());
        let mut seen = HashSet::new();
        push_checkout(
            &mut out,
            &mut seen,
            repo.clone(),
            "main".into(),
            None,
            Some(1),
        );
        push_checkout(
            &mut out,
            &mut seen,
            repo.join("."),
            "main".into(),
            None,
            Some(2),
        );

        assert_eq!(out.checkouts.len(), 1);
        assert_eq!(out.checkouts[0].subject.worktree.workspace_id, Some(1));
        assert_eq!(out.checkouts[0].label, "main");
    }

    #[test]
    fn worktree_checkouts_are_labeled_apart_from_the_root() {
        let mut out = workspace("/repo");
        let mut seen = HashSet::new();
        push_checkout(
            &mut out,
            &mut seen,
            PathBuf::from("/repo/.worktrees/fix"),
            "fix/windows".into(),
            None,
            None,
        );
        assert_eq!(out.checkouts[0].label, "wt · fix/windows");
        assert!(out.primary().is_some());
    }
}
