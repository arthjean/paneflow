use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, Pixels, Styled, Window, deferred, div, hsla, prelude::*, px,
};
use paneflow_config::schema::SessionId;

use crate::PaneFlowApp;
use crate::settings::components::{card_color, destructive_button, secondary_button, with_alpha};
use crate::terminal::host_link::{LiveSession, LiveSessionProbe};
use crate::ui_primitives::squircle::{squircle_border, squircle_fill};
use crate::ui_primitives::{BODY, LABEL_SM, TITLE};

const DIALOG_WIDTH: Pixels = px(460.);
const CARD_RADIUS: Pixels = crate::app::constants::PANE_CARD_RADIUS;
const CARD_PADDING: Pixels = px(20.);
const MAX_LISTED_BLOCKERS: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorktreeBlocker {
    Workspace {
        id: u64,
        title: String,
    },
    Tab {
        ws_id: u64,
        tab_id: u64,
        title: String,
    },
    Session {
        session: SessionId,
        title: String,
    },
}

impl WorktreeBlocker {
    fn title(&self) -> &str {
        match self {
            Self::Workspace { title, .. }
            | Self::Tab { title, .. }
            | Self::Session { title, .. } => title,
        }
    }

    fn kind_word(&self) -> &'static str {
        match self {
            Self::Workspace { .. } => "workspace",
            Self::Tab { .. } => "tab",
            Self::Session { .. } => "running session",
        }
    }
}

pub(crate) struct WorktreeRemoveDialog {
    path: PathBuf,
    repo_root: PathBuf,
    branch: String,
    blockers: Vec<WorktreeBlocker>,
    unlisted_sessions: Option<String>,
    focused: bool,
}

pub(crate) fn blockers_for(
    path: &Path,
    workspaces: &[(u64, String, PathBuf)],
    tabs: &[(u64, u64, String, Option<PathBuf>)],
    sessions: &[LiveSession],
) -> Vec<WorktreeBlocker> {
    let mut blockers: Vec<WorktreeBlocker> = workspaces
        .iter()
        .filter(|(_, _, root)| root == path)
        .map(|(id, title, _)| WorktreeBlocker::Workspace {
            id: *id,
            title: title.clone(),
        })
        .collect();
    blockers.extend(
        tabs.iter()
            .filter(|(_, _, _, bound)| bound.as_deref() == Some(path))
            .map(|(ws_id, tab_id, title, _)| WorktreeBlocker::Tab {
                ws_id: *ws_id,
                tab_id: *tab_id,
                title: title.clone(),
            }),
    );
    blockers.extend(
        sessions
            .iter()
            .filter(|session| session.cwd.starts_with(path))
            .map(|session| WorktreeBlocker::Session {
                session: session.session.clone(),
                title: session_label(session, path),
            }),
    );
    blockers
}

fn session_label(session: &LiveSession, worktree: &Path) -> String {
    let name = runnable_name(&session.title);
    match session.cwd.strip_prefix(worktree) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("{name} · {}", rest.display()),
        _ => name,
    }
}

fn runnable_name(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return "session".to_string();
    }
    trimmed
        .rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

fn blocker_summary(blockers: &[WorktreeBlocker]) -> String {
    let workspaces = blockers
        .iter()
        .filter(|b| matches!(b, WorktreeBlocker::Workspace { .. }))
        .count();
    let tabs = blockers
        .iter()
        .filter(|b| matches!(b, WorktreeBlocker::Tab { .. }))
        .count();
    let sessions = blockers
        .iter()
        .filter(|b| matches!(b, WorktreeBlocker::Session { .. }))
        .count();
    let mut parts = Vec::new();
    if workspaces > 0 {
        parts.push(super::plural(workspaces, "workspace", "workspaces"));
    }
    if tabs > 0 {
        parts.push(super::plural(tabs, "tab", "tabs"));
    }
    if sessions > 0 {
        parts.push(super::plural(
            sessions,
            "running session",
            "running sessions",
        ));
    }
    let verb = if workspaces + tabs + sessions == 1 {
        "is"
    } else {
        "are"
    };
    match parts.len() {
        0 => "Nothing is using it".to_string(),
        1 => format!("{} {verb} using it", parts[0]),
        2 => format!("{} and {} {verb} using it", parts[0], parts[1]),
        _ => format!(
            "{}, {} and {} {verb} using it",
            parts[0], parts[1], parts[2]
        ),
    }
}

impl PaneFlowApp {
    fn worktree_blockers(&self, path: &Path, sessions: &[LiveSession]) -> Vec<WorktreeBlocker> {
        let workspaces: Vec<(u64, String, PathBuf)> = self
            .workspaces
            .iter()
            .map(|ws| (ws.id, ws.title.clone(), ws.worktree_root.clone()))
            .collect();
        let tabs: Vec<(u64, u64, String, Option<PathBuf>)> = self
            .workspaces
            .iter()
            .flat_map(|ws| {
                ws.tabs().iter().enumerate().map(|(tab_idx, tab)| {
                    (
                        ws.id,
                        tab.id,
                        format!(
                            "{} › {}",
                            ws.title,
                            crate::app::sidebar::tab_display_title(tab, tab_idx)
                        ),
                        tab.worktree.clone(),
                    )
                })
            })
            .collect();
        blockers_for(path, &workspaces, &tabs, sessions)
    }

    pub(crate) fn request_worktree_removal(
        &mut self,
        ws_id: u64,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if self.worktree_remove_dialog.is_some() {
            return;
        }
        let Some((repo_root, branch)) = self
            .workspaces
            .iter()
            .find(|ws| ws.id == ws_id)
            .and_then(|ws| ws.managed_worktrees.iter().find(|wt| wt.path == path))
            .map(|wt| (wt.repo_root.clone(), wt.branch.clone()))
        else {
            return;
        };
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let probe = smol::unblock(crate::terminal::host_link::live_sessions).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        let (sessions, unlisted_sessions) = match probe {
                            LiveSessionProbe::Sessions(sessions) => (sessions, None),
                            LiveSessionProbe::NoHost => (Vec::new(), None),
                            LiveSessionProbe::Unknown(error) => {
                                log::warn!("worktree removal: the session probe failed: {error}");
                                (Vec::new(), Some(error))
                            }
                        };
                        let blockers = app.worktree_blockers(&path, &sessions);
                        if blockers.is_empty() && unlisted_sessions.is_none() {
                            app.remove_managed_worktree(ws_id, path.clone(), cx);
                            return;
                        }
                        app.dismiss_transient_surfaces();
                        app.worktree_remove_dialog = Some(WorktreeRemoveDialog {
                            path: path.clone(),
                            repo_root: repo_root.clone(),
                            branch: branch.clone(),
                            blockers,
                            unlisted_sessions,
                            focused: false,
                        });
                        cx.notify();
                    })
                });
            },
        )
        .detach();
    }

    pub(crate) fn close_worktree_remove_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.worktree_remove_dialog.take().is_some() {
            if self.settings_section.is_some() {
                self.settings_focus.focus(window, cx);
            } else if let Some(ws) = self.workspaces.get_mut(self.active_idx) {
                ws.focus_first(window, cx);
            }
            cx.notify();
        }
    }

    fn confirm_worktree_removal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.worktree_remove_dialog.take() else {
            return;
        };
        for blocker in &dialog.blockers {
            if let WorktreeBlocker::Session { session, .. } = blocker {
                self.stop_listed_session(session, cx);
            }
        }
        self.forget_managed_paths(std::slice::from_ref(&dialog.path), cx);
        for blocker in &dialog.blockers {
            if let WorktreeBlocker::Tab { ws_id, tab_id, .. } = blocker
                && let Some((ws_idx, tab_idx)) = self.tab_position(*ws_id, *tab_id)
            {
                self.remove_workspace_tab(ws_idx, tab_idx, window, cx);
            }
        }
        for blocker in &dialog.blockers {
            if let WorktreeBlocker::Workspace { id, .. } = blocker
                && let Some(idx) = self.workspaces.iter().position(|ws| ws.id == *id)
            {
                self.remove_workspace(idx, window, cx);
            }
        }
        self.forget_removed_worktree(&dialog.path, cx);
        self.spawn_worktree_checkout_removal(dialog.repo_root, dialog.path, cx);
        cx.notify();
    }

    fn spawn_worktree_checkout_removal(
        &mut self,
        repo_root: PathBuf,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (probe_root, probe_path) = (repo_root, path.clone());
                let removed = smol::unblock(move || {
                    crate::workspace::worktree::snapshot_and_remove(&probe_root, &probe_path)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        match removed {
                            Ok(snapshot) => {
                                app.forget_removed_worktree(&path, cx);
                                app.notify_snapshot_kept(snapshot, cx);
                            }
                            Err(message) => app.show_toast(message, cx),
                        }
                        cx.notify();
                    })
                });
            },
        )
        .detach();
    }

    fn handle_worktree_remove_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.worktree_remove_dialog.is_none() {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.close_worktree_remove_dialog(window, cx);
                cx.stop_propagation();
            }
            "enter" => {
                self.confirm_worktree_removal(window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    pub(crate) fn render_worktree_remove_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(dialog) = self.worktree_remove_dialog.as_mut() else {
            return div().into_any_element();
        };
        if !dialog.focused {
            dialog.focused = true;
            self.worktree_remove_focus.focus(window, cx);
        }
        let Some(dialog) = self.worktree_remove_dialog.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();
        let hidden = dialog.blockers.len().saturating_sub(MAX_LISTED_BLOCKERS);

        let header = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(CARD_PADDING)
            .pt(px(16.))
            .pb(px(12.))
            .child(
                div()
                    .text_size(TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui.text)
                    .child(format!("Remove the worktree of {}?", dialog.branch)),
            )
            .child(
                div()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child(blocker_summary(&dialog.blockers)),
            )
            .when_some(dialog.unlisted_sessions.clone(), |header, error| {
                header.child(
                    div()
                        .text_size(LABEL_SM)
                        .text_color(ui.vc_deleted)
                        .child(format!(
                            "Running sessions could not be listed, so this may remove more than \
                             it shows: {error}"
                        )),
                )
            });

        let mut list = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .mx(CARD_PADDING)
            .px(px(12.))
            .py(px(10.))
            .rounded(px(8.))
            .bg(with_alpha(ui.subtle, 0.5));
        for blocker in dialog.blockers.iter().take(MAX_LISTED_BLOCKERS) {
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(BODY)
                            .text_color(ui.text)
                            .child(blocker.title().to_string()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(LABEL_SM)
                            .text_color(ui.muted)
                            .child(blocker.kind_word()),
                    ),
            );
        }
        if hidden > 0 {
            list = list.child(
                div()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child(format!("and {hidden} more")),
            );
        }

        let explanation = div()
            .px(CARD_PADDING)
            .pt(px(12.))
            .pb(px(2.))
            .text_size(BODY)
            .line_height(px(18.))
            .text_color(ui.muted)
            .child(
                "Removing closes everything listed above and stops those sessions. Uncommitted \
                 changes in the worktree are saved as a snapshot you can restore, and the branch \
                 itself is kept.",
            );

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap(px(8.))
            .px(CARD_PADDING)
            .pt(px(18.))
            .pb(px(16.))
            .child(secondary_button(
                "worktree-remove-cancel",
                "Cancel",
                ui,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_worktree_remove_dialog(window, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(
                destructive_button("worktree-remove-confirm", "Remove").on_click(cx.listener(
                    |this, _: &ClickEvent, window, cx| {
                        this.confirm_worktree_removal(window, cx);
                        cx.stop_propagation();
                    },
                )),
            );

        let card = div()
            .id("worktree-remove-dialog")
            .occlude()
            .track_focus(&self.worktree_remove_focus)
            .on_key_down(cx.listener(Self::handle_worktree_remove_key_down))
            .relative()
            .w(DIALOG_WIDTH)
            .rounded(CARD_RADIUS)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(squircle_fill(CARD_RADIUS, card_color()))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(list)
                    .child(explanation)
                    .child(footer),
            )
            .child(squircle_border(
                CARD_RADIUS,
                px(1.),
                with_alpha(ui.border, 0.6),
            ));

        deferred(
            div()
                .id("worktree-remove-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.55))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.close_worktree_remove_dialog(window, cx);
                    }),
                )
                .child(card),
        )
        .with_priority(10)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(title: &str, cwd: &str) -> LiveSession {
        LiveSession {
            session: SessionId::new(),
            title: title.to_string(),
            cwd: PathBuf::from(cwd),
        }
    }

    #[test]
    fn a_free_worktree_has_no_blockers() {
        let path = PathBuf::from("/wt/feat-x");
        let workspaces = vec![(1, "Project".to_string(), PathBuf::from("/repo"))];
        let tabs = vec![(1, 10, "Terminal".to_string(), None)];
        let sessions = vec![session("shell", "/repo")];

        assert!(blockers_for(&path, &workspaces, &tabs, &sessions).is_empty());
    }

    #[test]
    fn a_tab_of_another_workspace_blocks_the_removal() {
        let path = PathBuf::from("/wt/feat-x");
        let workspaces = vec![
            (1, "Project".to_string(), PathBuf::from("/repo")),
            (2, "Other".to_string(), PathBuf::from("/repo")),
        ];
        let tabs = vec![
            (1, 10, "Terminal".to_string(), None),
            (2, 20, "Agent".to_string(), Some(path.clone())),
        ];

        let blockers = blockers_for(&path, &workspaces, &tabs, &[]);

        assert_eq!(
            blockers,
            vec![WorktreeBlocker::Tab {
                ws_id: 2,
                tab_id: 20,
                title: "Agent".to_string(),
            }]
        );
    }

    #[test]
    fn a_session_deeper_in_the_worktree_blocks_the_removal() {
        let path = PathBuf::from("/wt/feat-x");
        let sessions = vec![
            session("claude", "/wt/feat-x/src-app/src"),
            session("elsewhere", "/wt/feat-y"),
        ];

        let blockers = blockers_for(&path, &[], &[], &sessions);

        assert_eq!(blockers.len(), 1, "only the session inside the worktree");
        assert_eq!(blockers[0].kind_word(), "running session");
        assert_eq!(
            blockers[0].title(),
            "claude · src-app/src",
            "a session below the root says where it sits"
        );
    }

    #[test]
    fn a_session_is_named_by_its_program_not_by_its_full_path() {
        let path = PathBuf::from("/wt/feat-x");
        let sessions = vec![
            session(r"C:\Program Files\PowerShell\7\pwsh.exe", "/wt/feat-x"),
            session("/usr/bin/zsh", "/wt/feat-x"),
            session("   ", "/wt/feat-x"),
        ];

        let blockers = blockers_for(&path, &[], &[], &sessions);

        assert_eq!(
            blockers[0].title(),
            "pwsh.exe",
            "a windows shell path reads as its executable wherever the app runs"
        );
        assert_eq!(
            blockers[1].title(),
            "zsh",
            "a unix shell path reads as its executable too"
        );
        assert_eq!(
            blockers[2].title(),
            "session",
            "a session that set no title still reads as something"
        );
    }

    #[test]
    fn a_workspace_rooted_at_the_worktree_is_listed_first() {
        let path = PathBuf::from("/wt/feat-x");
        let workspaces = vec![(7, "feat-x".to_string(), path.clone())];
        let tabs = vec![(7, 70, "Lab › Terminal".to_string(), Some(path.clone()))];
        let sessions = vec![session("claude", "/wt/feat-x")];

        let blockers = blockers_for(&path, &workspaces, &tabs, &sessions);

        assert_eq!(blockers.len(), 3);
        assert_eq!(blockers[0].kind_word(), "workspace");
        assert_eq!(blockers[0].title(), "feat-x");
        assert_eq!(
            blockers[1].title(),
            "Lab › Terminal",
            "a tab row names its workspace so two windows stay apart"
        );
        assert_eq!(
            blocker_summary(&blockers),
            "1 workspace, 1 tab and 1 running session are using it"
        );
    }

    #[test]
    fn the_summary_counts_each_kind() {
        let path = PathBuf::from("/wt/feat-x");
        let tabs = vec![
            (1, 10, "a".to_string(), Some(path.clone())),
            (1, 11, "b".to_string(), Some(path.clone())),
        ];

        let blockers = blockers_for(&path, &[], &tabs, &[]);

        assert_eq!(blocker_summary(&blockers), "2 tabs are using it");
    }
}
