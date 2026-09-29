use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Pixels, SharedString, Styled, Window, div, prelude::*, px,
};
use paneflow_config::schema::SessionId;

use crate::PaneFlowApp;
use crate::settings::components::{
    ModalKey, confirmation_list, confirmation_warning, destructive_button, modal_backdrop,
    modal_card, modal_footer, modal_header, modal_key, secondary_button,
};
use crate::terminal::host_link::{LiveSession, LiveSessionProbe};
use crate::ui_primitives::LABEL_SM;
use crate::workspace::worktree::Snapshot;

const DIALOG_WIDTH: Pixels = px(460.);
const CARD_RADIUS: Pixels = crate::app::constants::PANE_CARD_RADIUS;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemovalOrigin {
    Settings,
    TabMenu,
}

pub(crate) fn needs_confirmation(
    origin: RemovalOrigin,
    blockers: &[WorktreeBlocker],
    unlisted_sessions: Option<&str>,
) -> bool {
    origin == RemovalOrigin::TabMenu || !blockers.is_empty() || unlisted_sessions.is_some()
}

pub(crate) struct RemovalRecheck {
    pub(crate) path: PathBuf,
    pub(crate) open_workspaces: Vec<WorktreeBlocker>,
    pub(crate) consented_sessions: Vec<SessionId>,
    pub(crate) accepts_unlisted_sessions: bool,
}

#[derive(Debug)]
pub(crate) enum RemovalOutcome {
    Removed(Option<Snapshot>),
    Blocked(Vec<WorktreeBlocker>),
    Failed(String),
}

pub(crate) fn remove_unless_blocked(
    recheck: RemovalRecheck,
    probe: impl FnOnce() -> LiveSessionProbe,
    remove: impl FnOnce() -> Result<Option<Snapshot>, String>,
) -> RemovalOutcome {
    let sessions = match probe() {
        LiveSessionProbe::Sessions(sessions) => sessions,
        LiveSessionProbe::NoHost => Vec::new(),
        LiveSessionProbe::Unknown(_) if recheck.accepts_unlisted_sessions => Vec::new(),
        LiveSessionProbe::Unknown(error) => {
            return RemovalOutcome::Failed(format!(
                "Removal of {} canceled: running sessions could not be listed ({error})",
                recheck.path.display()
            ));
        }
    };
    let mut blockers = recheck.open_workspaces;
    blockers.extend(
        blockers_for(&recheck.path, &[], &[], &sessions)
            .into_iter()
            .filter(|blocker| match blocker {
                WorktreeBlocker::Session { session, .. } => {
                    !recheck.consented_sessions.contains(session)
                }
                _ => true,
            }),
    );
    if !blockers.is_empty() {
        return RemovalOutcome::Blocked(blockers);
    }
    match remove() {
        Ok(snapshot) => RemovalOutcome::Removed(snapshot),
        Err(message) => RemovalOutcome::Failed(message),
    }
}

fn blocked_removal_message(path: &Path, blockers: &[WorktreeBlocker]) -> String {
    format!(
        "Removal of {} canceled: {}",
        path.display(),
        blocker_summary(blockers).to_lowercase()
    )
}

fn failed_removal_message(path: &Path, message: String) -> String {
    let shown = path.display().to_string();
    if message.contains(&shown) {
        message
    } else {
        format!("{shown} is still on disk: {message}")
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
        let Some((repo_root, branch)) = self
            .workspaces
            .iter()
            .find(|ws| ws.id == ws_id)
            .and_then(|ws| ws.managed_worktrees.iter().find(|wt| wt.path == path))
            .map(|wt| (wt.repo_root.clone(), wt.branch.clone()))
        else {
            return;
        };
        self.open_worktree_removal(
            RemovalOrigin::Settings,
            Some(ws_id),
            path,
            repo_root,
            branch,
            cx,
        );
    }

    pub(crate) fn remove_tab_worktree(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return;
        };
        let Some(repo_root) = ws.repo_root.clone() else {
            return;
        };
        let Some(path) = ws.tabs().get(tab_idx).and_then(|tab| tab.worktree.clone()) else {
            return;
        };
        if self.workspaces.iter().any(|ws| ws.worktree_root == path) {
            self.show_toast(
                format!("{} is open as a workspace - close it first", path.display()),
                cx,
            );
            return;
        }
        let branch = self
            .workspace_worktree_listing(ws_idx)
            .iter()
            .find(|entry| entry.path == path)
            .and_then(|entry| entry.branch.clone())
            .unwrap_or_else(|| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string())
            });
        self.open_worktree_removal(RemovalOrigin::TabMenu, None, path, repo_root, branch, cx);
    }

    fn open_worktree_removal(
        &mut self,
        origin: RemovalOrigin,
        managed_by: Option<u64>,
        path: PathBuf,
        repo_root: PathBuf,
        branch: String,
        cx: &mut Context<Self>,
    ) {
        if self.worktree_remove_dialog.is_some() {
            return;
        }
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
                        let mut blockers = app.worktree_blockers(&path, &sessions);
                        if origin == RemovalOrigin::TabMenu {
                            blockers.retain(|b| !matches!(b, WorktreeBlocker::Tab { .. }));
                        }
                        if !needs_confirmation(origin, &blockers, unlisted_sessions.as_deref())
                            && let Some(ws_id) = managed_by
                        {
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
        let consented_sessions: Vec<SessionId> = dialog
            .blockers
            .iter()
            .filter_map(|blocker| match blocker {
                WorktreeBlocker::Session { session, .. } => Some(session.clone()),
                _ => None,
            })
            .collect();
        for session in &consented_sessions {
            self.stop_listed_session(session, cx);
        }
        for blocker in &dialog.blockers {
            if let WorktreeBlocker::Tab { ws_id, tab_id, .. } = blocker
                && let Some((ws_idx, tab_idx)) = self.tab_position(*ws_id, *tab_id)
            {
                self.remove_workspace_tab(ws_idx, tab_idx, window, cx);
            }
        }
        crate::app::workspace_ops::settle_closed_sessions(
            &mut self.closed_panes,
            &consented_sessions.iter().cloned().collect(),
            None,
        );
        for blocker in &dialog.blockers {
            if let WorktreeBlocker::Workspace { id, .. } = blocker
                && let Some(idx) = self.workspaces.iter().position(|ws| ws.id == *id)
            {
                self.remove_workspace(idx, Some(window), cx);
            }
        }
        let open_workspaces = self
            .worktree_blockers(&dialog.path, &[])
            .into_iter()
            .filter(|b| matches!(b, WorktreeBlocker::Workspace { .. }))
            .collect();
        let recheck = RemovalRecheck {
            path: dialog.path.clone(),
            open_workspaces,
            consented_sessions,
            accepts_unlisted_sessions: dialog.unlisted_sessions.is_some(),
        };
        self.spawn_worktree_checkout_removal(dialog.repo_root, recheck, cx);
        cx.notify();
    }

    fn spawn_worktree_checkout_removal(
        &mut self,
        repo_root: PathBuf,
        recheck: RemovalRecheck,
        cx: &mut Context<Self>,
    ) {
        let path = recheck.path.clone();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let probe_path = path.clone();
                let outcome = smol::unblock(move || {
                    remove_unless_blocked(
                        recheck,
                        crate::terminal::host_link::live_sessions,
                        || crate::workspace::worktree::snapshot_and_remove(&repo_root, &probe_path),
                    )
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        match outcome {
                            RemovalOutcome::Removed(snapshot) => {
                                app.forget_removed_worktree(&path, cx);
                                app.forget_managed_paths(std::slice::from_ref(&path), cx);
                                app.notify_snapshot_kept(snapshot, cx);
                            }
                            RemovalOutcome::Blocked(blockers) => {
                                app.show_toast(blocked_removal_message(&path, &blockers), cx)
                            }
                            RemovalOutcome::Failed(message) => {
                                app.show_toast(failed_removal_message(&path, message), cx)
                            }
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
        match modal_key(event) {
            Some(ModalKey::Dismiss) => self.close_worktree_remove_dialog(window, cx),
            Some(ModalKey::Confirm) => self.confirm_worktree_removal(window, cx),
            None => return,
        }
        cx.stop_propagation();
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

        let header = modal_header(
            ui,
            format!("Remove the worktree of {}?", dialog.branch),
            blocker_summary(&dialog.blockers),
        )
        .child(
            div()
                .text_size(LABEL_SM)
                .text_color(ui.muted)
                .child(dialog.path.display().to_string()),
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

        let list = confirmation_list(
            ui,
            dialog.blockers.iter().map(|blocker| {
                (
                    SharedString::from(blocker.title().to_string()),
                    SharedString::from(blocker.kind_word()),
                )
            }),
            MAX_LISTED_BLOCKERS,
        );

        let explanation = confirmation_warning(
            ui,
            "Removing closes everything listed above and stops those sessions. Uncommitted \
             changes in the worktree are saved as a snapshot you can restore, and the branch \
             itself is kept.",
        );

        let footer = modal_footer()
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

        let card = modal_card(
            "worktree-remove-dialog",
            DIALOG_WIDTH,
            CARD_RADIUS,
            ui,
            div()
                .child(header)
                .child(list)
                .child(explanation)
                .child(footer),
        )
        .track_focus(&self.worktree_remove_focus)
        .on_key_down(cx.listener(Self::handle_worktree_remove_key_down));

        modal_backdrop(
            "worktree-remove-backdrop",
            card,
            cx.listener(|this, _, window, cx| {
                this.close_worktree_remove_dialog(window, cx);
            }),
        )
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

    fn recheck(path: &Path, consented: Vec<SessionId>) -> RemovalRecheck {
        RemovalRecheck {
            path: path.to_path_buf(),
            open_workspaces: Vec::new(),
            consented_sessions: consented,
            accepts_unlisted_sessions: false,
        }
    }

    #[test]
    fn a_tab_menu_removal_always_asks_before_removing() {
        let path = PathBuf::from("/wt/feat-x");
        let live = blockers_for(&path, &[], &[], &[session("claude", "/wt/feat-x")]);

        assert!(needs_confirmation(RemovalOrigin::TabMenu, &[], None));
        assert!(needs_confirmation(RemovalOrigin::TabMenu, &live, None));
        assert!(needs_confirmation(RemovalOrigin::Settings, &live, None));
        assert!(!needs_confirmation(RemovalOrigin::Settings, &[], None));
    }

    #[test]
    fn a_live_session_that_was_not_confirmed_keeps_the_checkout() {
        let path = PathBuf::from("/wt/feat-x");
        let agent = session("claude", "/wt/feat-x/src");
        let removed = std::cell::Cell::new(false);

        let outcome = remove_unless_blocked(
            recheck(&path, Vec::new()),
            || LiveSessionProbe::Sessions(vec![agent.clone()]),
            || {
                removed.set(true);
                Ok(None)
            },
        );

        assert!(!removed.get(), "snapshot_and_remove must never run");
        assert!(matches!(outcome, RemovalOutcome::Blocked(ref b) if b.len() == 1));
    }

    #[test]
    fn a_blocker_that_appears_after_confirmation_cancels_the_removal() {
        let path = PathBuf::from("/wt/feat-x");
        let stopping = session("zsh", "/wt/feat-x");
        let newcomer = session("codex", "/wt/feat-x");
        let removed = std::cell::Cell::new(false);

        let outcome = remove_unless_blocked(
            recheck(&path, vec![stopping.session.clone()]),
            || LiveSessionProbe::Sessions(vec![stopping.clone(), newcomer.clone()]),
            || {
                removed.set(true);
                Ok(None)
            },
        );

        assert!(!removed.get(), "a session started after the dialog blocks");
        let RemovalOutcome::Blocked(blockers) = outcome else {
            panic!("expected the removal to be blocked");
        };
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].title(), "codex");
        assert_eq!(
            blocked_removal_message(&path, &blockers),
            format!(
                "Removal of {} canceled: 1 running session is using it",
                path.display()
            )
        );

        let mut reopened = recheck(&path, Vec::new());
        reopened.open_workspaces = vec![WorktreeBlocker::Workspace {
            id: 9,
            title: "feat-x".to_string(),
        }];
        let outcome = remove_unless_blocked(
            reopened,
            || LiveSessionProbe::NoHost,
            || {
                removed.set(true);
                Ok(None)
            },
        );
        assert!(!removed.get(), "a workspace opened on the path blocks");
        assert!(matches!(outcome, RemovalOutcome::Blocked(_)));
    }

    #[test]
    fn confirmed_sessions_still_shutting_down_do_not_block() {
        let path = PathBuf::from("/wt/feat-x");
        let stopping = session("zsh", "/wt/feat-x");
        let removed = std::cell::Cell::new(false);

        let outcome = remove_unless_blocked(
            recheck(&path, vec![stopping.session.clone()]),
            || LiveSessionProbe::Sessions(vec![stopping.clone()]),
            || {
                removed.set(true);
                Ok(None)
            },
        );

        assert!(removed.get());
        assert!(matches!(outcome, RemovalOutcome::Removed(None)));
    }

    #[test]
    fn an_unlistable_host_cancels_a_removal_the_user_did_not_accept_blind() {
        let path = PathBuf::from("/wt/feat-x");
        let removed = std::cell::Cell::new(false);

        let outcome = remove_unless_blocked(
            recheck(&path, Vec::new()),
            || LiveSessionProbe::Unknown("pipe closed".to_string()),
            || {
                removed.set(true);
                Ok(None)
            },
        );

        assert!(!removed.get());
        assert!(matches!(outcome, RemovalOutcome::Failed(_)));
    }

    #[test]
    fn a_failed_removal_names_the_path_left_on_disk() {
        let path = PathBuf::from("/wt/feat-x");

        let message = failed_removal_message(
            &path,
            "git worktree remove --force failed: Permission denied".to_string(),
        );

        assert!(message.starts_with(&format!("{} is still on disk", path.display())));
        let named = format!("{} was not created by Paneflow", path.display());
        assert_eq!(failed_removal_message(&path, named.clone()), named);
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
