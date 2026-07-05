// US-007 (prd-agents-view.md): these ops are the data-plane primitives
// the Agents sidebar (US-010) and create-affordances (US-011) call
// into. They mutate `PaneFlowApp` state and notify the view, but no
// caller exists yet -- US-008 wires the mode toggle, US-010/US-011
// wire the buttons and context menus that invoke each op. The module-
// scoped allow keeps that staging visible in one line instead of
// twelve `#[allow(dead_code)]` attributes.
#![allow(dead_code)]

//! Project + thread lifecycle operations for `PaneFlowApp`.
//!
//! Mirrors the existing [`crate::app::workspace_ops`] module: every
//! method is a thin state mutation on `PaneFlowApp` that updates the
//! `projects` vector and the `active_project_idx`, then calls
//! [`cx.notify`] so the next render sees the change. UI rendering
//! (sidebar headers, thread rows, context menus) lives in US-010;
//! these ops are the data-plane primitives the sidebar will call into.
//!
//! No GPUI focus / persistence side effects yet. `save_session` is
//! invoked at the call site (matching the workspace_ops pattern --
//! some op chains save once at the end, not after every step).
//!
//! See US-007 of `tasks/prd-agents-view.md`.

use gpui::Context;
use paneflow_acp::AgentKind;

use crate::PaneFlowApp;
use crate::project::{
    AgentsTarget, Project, Thread, ThreadStatus, next_project_id, next_thread_id,
};

/// Hard caps from the PRD's Non-Functional Requirements: at most 50
/// projects per session and 100 threads per project. Exceeding either
/// cap is a no-op (the UI surfaces a toast at the call site -- the op
/// layer just refuses to mutate state).
pub const MAX_PROJECTS: usize = 50;
pub const MAX_THREADS_PER_PROJECT: usize = 100;

/// Outcome of an op that may fail with a user-facing reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpError {
    /// Too many projects already exist.
    ProjectLimitReached,
    /// Too many threads already exist in the target project.
    ThreadLimitReached,
    /// The target project index does not exist.
    ProjectNotFound,
    /// The target thread does not exist within the target project.
    ThreadNotFound,
}

impl PaneFlowApp {
    /// Currently active project, or `None` if no project is selected
    /// (because none have been created yet).
    pub(crate) fn active_project(&self) -> Option<&Project> {
        self.projects.get(self.active_project_idx)
    }

    pub(crate) fn active_project_mut(&mut self) -> Option<&mut Project> {
        self.projects.get_mut(self.active_project_idx)
    }

    /// Create a new project at the end of the list and make it the
    /// active project. Returns the new project's monotonic ID, or
    /// `OpError::ProjectLimitReached` if we are at the cap.
    pub(crate) fn create_project(
        &mut self,
        title: impl Into<String>,
        cwd: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Result<u64, OpError> {
        if self.projects.len() >= MAX_PROJECTS {
            return Err(OpError::ProjectLimitReached);
        }
        let cwd = cwd.into();
        let mut project = Project {
            id: next_project_id(),
            title: title.into(),
            cwd: cwd.clone(),
            threads: Vec::new(),
            is_expanded: true,
            git_stats: crate::workspace::GitDiffStats::default(),
        };
        // Defensive: in case some future caller seeds threads via
        // `..Default::default()`, clamp.
        project.threads.truncate(MAX_THREADS_PER_PROJECT);
        let id = project.id;
        self.projects.push(project);
        self.active_project_idx = self.projects.len() - 1;
        self.spawn_agents_environment_git_refresh(cwd, cx);
        cx.notify();
        Ok(id)
    }

    /// Switch to the project at `idx`, no-op if out of bounds or
    /// already active.
    pub(crate) fn select_project(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx < self.projects.len() && idx != self.active_project_idx {
            self.active_project_idx = idx;
            cx.notify();
        }
    }

    /// Close the project at `idx`. Adjusts `active_project_idx` to
    /// stay in range. Returns the removed project (so a caller can
    /// cascade-delete its threads from `paneflow-threads`).
    pub(crate) fn close_project(
        &mut self,
        idx: usize,
        cx: &mut Context<Self>,
    ) -> Result<Project, OpError> {
        if idx >= self.projects.len() {
            return Err(OpError::ProjectNotFound);
        }
        let removed = self.projects.remove(idx);
        // Cascade the in-memory terminal cache so threads belonging to a
        // closed project don't keep their PTY entity alive until the
        // next restart.
        for thread in &removed.threads {
            self.agents_view
                .agents_terminal_view_cache
                .remove(&thread.id);
        }
        // US-003: re-map the unified center target across the removal. A
        // target into the closed project falls back to the picker (`None`);
        // a target into a project that shifted down by one is re-indexed; a
        // chat target is unaffected (chats are not part of any project).
        self.agents_target = remap_target_after_project_removal(self.agents_target, idx);
        // Keep active_project_idx valid: if we removed at or before
        // the active idx, shift it down (clamped to 0). If projects
        // is now empty, reset to 0 (the sidebar reads `is_empty()`
        // to decide whether to render anything).
        self.active_project_idx = if self.projects.is_empty() {
            0
        } else if idx <= self.active_project_idx {
            self.active_project_idx.saturating_sub(1)
        } else {
            self.active_project_idx
        };
        cx.notify();
        Ok(removed)
    }

    /// Rename the project at `idx`. No-op if out of bounds.
    pub(crate) fn rename_project(
        &mut self,
        idx: usize,
        new_title: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), OpError> {
        let Some(project) = self.projects.get_mut(idx) else {
            return Err(OpError::ProjectNotFound);
        };
        project.title = new_title.into();
        cx.notify();
        Ok(())
    }

    /// Move the project at `from` to position `to`. Keeps
    /// `active_project_idx` pointing at the same project. No-op if
    /// either index is out of range or they are equal.
    pub(crate) fn reorder_project(
        &mut self,
        from: usize,
        to: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), OpError> {
        let len = self.projects.len();
        if from >= len || to >= len {
            return Err(OpError::ProjectNotFound);
        }
        if from == to {
            return Ok(());
        }
        let moved_id = self.projects[from].id;
        let project = self.projects.remove(from);
        self.projects.insert(to, project);
        // US-003: re-map a project-thread target's `project_idx` so the
        // center keeps pointing at the same thread after the move (chats
        // are unaffected).
        self.agents_target = remap_target_after_project_move(self.agents_target, from, to);
        // Re-find the moved project to update active_project_idx so
        // the user's selection follows the drag, not the index.
        if let Some(new_idx) = self.projects.iter().position(|p| p.id == moved_id) {
            // Only re-anchor if the active project IS the moved one;
            // otherwise leave the active index pointing at whatever
            // project sits there post-shift.
            if self.projects.get(self.active_project_idx).map(|p| p.id) != Some(moved_id) {
                // Different project was active; preserve its identity.
                // (No-op here because identity is implicit in the
                // shifted vector; we just don't move the index.)
            } else {
                self.active_project_idx = new_idx;
            }
        }
        cx.notify();
        Ok(())
    }

    /// Add a thread to the project at `project_idx`. The thread is
    /// always added to the end of the project's list (the sidebar
    /// will sort by `updated_at DESC` at render time via the threads
    /// DB query -- US-010). Returns the new thread's ID.
    pub(crate) fn add_thread(
        &mut self,
        project_idx: usize,
        title: impl Into<String>,
        agent: AgentKind,
        cx: &mut Context<Self>,
    ) -> Result<u64, OpError> {
        let project = self
            .projects
            .get_mut(project_idx)
            .ok_or(OpError::ProjectNotFound)?;
        if project.threads.len() >= MAX_THREADS_PER_PROJECT {
            return Err(OpError::ThreadLimitReached);
        }
        let thread = Thread {
            id: next_thread_id(),
            title: title.into(),
            agent,
            kind: crate::project::ThreadKind::Agent,
            status: ThreadStatus::Idle,
            cwd: project.cwd.clone(),
            created_at: now_unix_millis(),
            model: None,
            mode: None,
            // US-011: filled in by the affordance handler after the
            // threads.db INSERT succeeds (the op layer stays
            // persistence-agnostic so it can be unit-tested without
            // a SQLite store).
            store_id: None,
            terminal_agent: None,
            pinned: false,
            agent_pid: None,
            agent_proc_start: None,
            // Legacy Agent-kind row (not a Terminal Thread): no forced
            // session id, no manual-rename lock.
            session_id: None,
            title_user_set: false,
        };
        let id = thread.id;
        project.threads.push(thread);
        cx.notify();
        Ok(id)
    }

    /// Append a Terminal Thread to `project_idx`. Same shape as
    /// [`Self::add_thread`] but stamps [`crate::project::ThreadKind::Terminal`]
    /// and never touches `threads.db` - the PTY is the source of truth
    /// for Terminal Threads and no message rows exist to persist.
    /// `terminal_agent` is the CLI auto-launched on first PTY mount
    /// (`None` for a bare shell).
    pub(crate) fn add_terminal_thread(
        &mut self,
        project_idx: usize,
        title: impl Into<String>,
        terminal_agent: Option<crate::agent_launcher::TerminalAgent>,
        cx: &mut Context<Self>,
    ) -> Result<u64, OpError> {
        let project = self
            .projects
            .get_mut(project_idx)
            .ok_or(OpError::ProjectNotFound)?;
        if project.threads.len() >= MAX_THREADS_PER_PROJECT {
            return Err(OpError::ThreadLimitReached);
        }
        let thread =
            crate::project::Thread::new_terminal(title, project.cwd.clone(), terminal_agent);
        let id = thread.id;
        project.threads.push(thread);
        cx.notify();
        Ok(id)
    }

    /// Select a project thread as the center target (US-003). Sets both the
    /// focused-project anchor (`active_project_idx`) and the unified target
    /// so the rail highlight and the center stay in sync. Returns `Err` if
    /// either index is out of bounds.
    pub(crate) fn select_thread(
        &mut self,
        project_idx: usize,
        thread_idx: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), OpError> {
        let project = self
            .projects
            .get(project_idx)
            .ok_or(OpError::ProjectNotFound)?;
        if thread_idx >= project.threads.len() {
            return Err(OpError::ThreadNotFound);
        }
        let cwd = project.cwd.clone();
        self.active_project_idx = project_idx;
        let target = AgentsTarget::Thread {
            project_idx,
            thread_idx,
        };
        self.agents_target = Some(target);
        // US-005: a concrete selection always returns the picker context to
        // the project default (so a later deselect doesn't reopen the chat
        // picker).
        self.agents_picker_context = crate::project::AgentsPickerContext::Project;
        // Picking a thread leaves the Skills page.
        self.agents_view.agents_skills_visible = false;
        // Selecting a row cancels any armed inline delete-confirm.
        self.agents_view.agents_delete_armed = None;
        self.spawn_agents_environment_git_refresh(cwd, cx);
        self.mount_agents_terminal_for_target(target, cx);
        self.save_session(cx);
        cx.notify();
        Ok(())
    }

    /// Select a free chat as the center target (US-003). Leaves
    /// `active_project_idx` untouched - a chat is not part of any project,
    /// so the rail's focused-project anchor does not move. Returns `Err`
    /// if `chat_idx` is out of bounds.
    pub(crate) fn select_chat(
        &mut self,
        chat_idx: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), OpError> {
        if chat_idx >= self.chats.len() {
            return Err(OpError::ThreadNotFound);
        }
        let cwd = self.chats[chat_idx].cwd.clone();
        let target = AgentsTarget::Chat { chat_idx };
        self.agents_target = Some(target);
        // US-005: reset picker context on any concrete selection.
        self.agents_picker_context = crate::project::AgentsPickerContext::Project;
        self.agents_view.agents_skills_visible = false;
        // Selecting a row cancels any armed inline delete-confirm.
        self.agents_view.agents_delete_armed = None;
        self.spawn_agents_environment_git_refresh(cwd, cx);
        self.mount_agents_terminal_for_target(target, cx);
        self.save_session(cx);
        cx.notify();
        Ok(())
    }

    /// Remove a thread by index within the given project. Returns
    /// the removed [`Thread`] so the caller can cascade-delete its
    /// row from `paneflow-threads`.
    pub(crate) fn remove_thread(
        &mut self,
        project_idx: usize,
        thread_idx: usize,
        cx: &mut Context<Self>,
    ) -> Result<Thread, OpError> {
        let project = self
            .projects
            .get_mut(project_idx)
            .ok_or(OpError::ProjectNotFound)?;
        if thread_idx >= project.threads.len() {
            return Err(OpError::ThreadNotFound);
        }
        let removed = project.threads.remove(thread_idx);
        // Drop the cached Terminal Thread entity so its PTY is torn down
        // (the alacritty event loop sends `Msg::Shutdown` in `Drop`, see
        // `src/terminal/pty_session.rs`).
        self.agents_view
            .agents_terminal_view_cache
            .remove(&removed.id);
        // US-003: keep the unified target in range. Only a target pointing
        // into THIS project's thread list is affected; a chat target or a
        // different project's target is left untouched. Removing the
        // selected thread clears the target (falls back to the picker);
        // removing an earlier sibling shifts the index down.
        self.agents_target =
            remap_target_after_thread_removal(self.agents_target, project_idx, thread_idx);
        cx.notify();
        Ok(removed)
    }

    /// Remove a free chat by index (US-003). Drops its cached PTY entity
    /// and re-maps the unified target the same way [`Self::remove_thread`]
    /// does for project threads: clearing it when the removed chat was the
    /// selection, shifting it down for an earlier sibling, leaving project
    /// targets untouched. Returns the removed [`Thread`].
    pub(crate) fn remove_chat(
        &mut self,
        chat_idx: usize,
        cx: &mut Context<Self>,
    ) -> Result<Thread, OpError> {
        if chat_idx >= self.chats.len() {
            return Err(OpError::ThreadNotFound);
        }
        let removed = self.chats.remove(chat_idx);
        self.agents_view
            .agents_terminal_view_cache
            .remove(&removed.id);
        self.agents_target = remap_target_after_chat_removal(self.agents_target, chat_idx);
        cx.notify();
        Ok(removed)
    }

    /// Append a free chat (terminal thread) anchored on the user's home dir
    /// (US-002/US-003). Mirrors [`Self::add_terminal_thread`] but targets the
    /// project-less `chats` list. `terminal_agent` is the CLI auto-launched
    /// on first PTY mount (`None` for a bare shell). Chats are uncapped for
    /// v1 (the cache has no eviction; cap is tracked as a follow-up). Returns
    /// the new chat's ID.
    pub(crate) fn add_chat_thread(
        &mut self,
        title: impl Into<String>,
        terminal_agent: Option<crate::agent_launcher::TerminalAgent>,
        cx: &mut Context<Self>,
    ) -> u64 {
        let cwd = chat_home_cwd();
        let thread = Thread::new_terminal(title, cwd, terminal_agent);
        let id = thread.id;
        self.chats.push(thread);
        cx.notify();
        id
    }
}

/// Resolve the cwd for a free chat: the user's home directory (US-002).
/// Cross-platform via `dirs::home_dir()` - never `$HOME` raw, never a
/// hardcoded POSIX path. Documented fallback chain when home cannot be
/// resolved (Edge Case #1): the current working directory, then `"."`
/// (always a valid relative cwd). Never panics.
pub(crate) fn chat_home_cwd() -> String {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| ".".to_string())
}

fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// US-003: pure center-target re-mapping (free functions, no `&self`/`cx`).
//
// Extracted from the `cx`-bound ops so the index arithmetic - the part that
// must never produce a stale target after a removal/move - is unit-testable
// without a live GPUI `App`. Each takes the current target and returns the
// re-mapped one; callers assign the result back to `self.agents_target`.
// ---------------------------------------------------------------------------

/// Re-map the target after removing `projects[project_idx].threads[removed]`.
/// Only a target into the SAME project's thread list moves: it clears to the
/// picker (`None`) when the removed thread WAS the selection, and shifts down
/// by one when an earlier sibling was removed. A chat target or a different
/// project's target passes through unchanged.
fn remap_target_after_thread_removal(
    target: Option<AgentsTarget>,
    project_idx: usize,
    removed: usize,
) -> Option<AgentsTarget> {
    match target {
        Some(AgentsTarget::Thread {
            project_idx: tp,
            thread_idx: tt,
        }) if tp == project_idx => {
            if removed == tt {
                None
            } else if removed < tt {
                Some(AgentsTarget::Thread {
                    project_idx: tp,
                    thread_idx: tt - 1,
                })
            } else {
                Some(AgentsTarget::Thread {
                    project_idx: tp,
                    thread_idx: tt,
                })
            }
        }
        other => other,
    }
}

/// Re-map the target after removing `chats[removed]`. Mirrors
/// [`remap_target_after_thread_removal`] for the chat source; project targets
/// pass through unchanged.
fn remap_target_after_chat_removal(
    target: Option<AgentsTarget>,
    removed: usize,
) -> Option<AgentsTarget> {
    match target {
        Some(AgentsTarget::Chat { chat_idx: tc }) => {
            if removed == tc {
                None
            } else if removed < tc {
                Some(AgentsTarget::Chat { chat_idx: tc - 1 })
            } else {
                Some(AgentsTarget::Chat { chat_idx: tc })
            }
        }
        other => other,
    }
}

/// Re-map the target after closing `projects[removed]`. A target into the
/// closed project falls back to the picker (`None`); a target into a project
/// that shifted down by one is re-indexed; a chat target is unaffected.
fn remap_target_after_project_removal(
    target: Option<AgentsTarget>,
    removed: usize,
) -> Option<AgentsTarget> {
    match target {
        Some(AgentsTarget::Thread {
            project_idx: tp,
            thread_idx: tt,
        }) => {
            if tp == removed {
                None
            } else if tp > removed {
                Some(AgentsTarget::Thread {
                    project_idx: tp - 1,
                    thread_idx: tt,
                })
            } else {
                Some(AgentsTarget::Thread {
                    project_idx: tp,
                    thread_idx: tt,
                })
            }
        }
        other => other,
    }
}

/// Re-map a project-thread target's `project_idx` after moving the project at
/// `from` to `to` (standard list-move remap). Chats pass through unchanged.
fn remap_target_after_project_move(
    target: Option<AgentsTarget>,
    from: usize,
    to: usize,
) -> Option<AgentsTarget> {
    match target {
        Some(AgentsTarget::Thread {
            project_idx: tp,
            thread_idx: tt,
        }) => {
            let new_p = if tp == from {
                to
            } else if from < to && from < tp && tp <= to {
                tp - 1
            } else if to <= tp && tp < from {
                tp + 1
            } else {
                tp
            };
            Some(AgentsTarget::Thread {
                project_idx: new_p,
                thread_idx: tt,
            })
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(p: usize, t: usize) -> Option<AgentsTarget> {
        Some(AgentsTarget::Thread {
            project_idx: p,
            thread_idx: t,
        })
    }
    fn chat(c: usize) -> Option<AgentsTarget> {
        Some(AgentsTarget::Chat { chat_idx: c })
    }

    #[test]
    fn thread_removal_clears_selection_when_it_was_the_target() {
        // Deleting the selected thread falls back to the picker, not a
        // stale index (the index-stale bug class US-003 closes).
        assert_eq!(remap_target_after_thread_removal(thread(0, 2), 0, 2), None);
    }

    #[test]
    fn thread_removal_shifts_earlier_sibling_down() {
        assert_eq!(
            remap_target_after_thread_removal(thread(0, 3), 0, 1),
            thread(0, 2)
        );
    }

    #[test]
    fn thread_removal_leaves_later_sibling_and_other_sources() {
        // Removing a later sibling does not move the target.
        assert_eq!(
            remap_target_after_thread_removal(thread(0, 1), 0, 3),
            thread(0, 1)
        );
        // A different project is untouched.
        assert_eq!(
            remap_target_after_thread_removal(thread(1, 0), 0, 0),
            thread(1, 0)
        );
        // A chat target is untouched by a thread removal.
        assert_eq!(remap_target_after_thread_removal(chat(0), 0, 0), chat(0));
    }

    #[test]
    fn chat_removal_clears_shifts_and_ignores_threads() {
        // Selected chat removed -> picker.
        assert_eq!(remap_target_after_chat_removal(chat(2), 2), None);
        // Earlier chat removed -> shift down.
        assert_eq!(remap_target_after_chat_removal(chat(3), 1), chat(2));
        // Later chat removed -> unchanged.
        assert_eq!(remap_target_after_chat_removal(chat(1), 3), chat(1));
        // A project-thread target is untouched by a chat removal.
        assert_eq!(
            remap_target_after_chat_removal(thread(0, 0), 0),
            thread(0, 0)
        );
    }

    #[test]
    fn project_removal_clears_shifts_and_ignores_chats() {
        // Target inside the closed project -> picker.
        assert_eq!(remap_target_after_project_removal(thread(1, 4), 1), None);
        // Project after the closed one shifts down (thread_idx preserved).
        assert_eq!(
            remap_target_after_project_removal(thread(2, 4), 1),
            thread(1, 4)
        );
        // Project before the closed one is unchanged.
        assert_eq!(
            remap_target_after_project_removal(thread(0, 4), 1),
            thread(0, 4)
        );
        // A chat target survives a project close.
        assert_eq!(remap_target_after_project_removal(chat(0), 1), chat(0));
    }

    #[test]
    fn project_move_remaps_target_project_idx() {
        // The moved project itself follows the drag.
        assert_eq!(
            remap_target_after_project_move(thread(0, 1), 0, 2),
            thread(2, 1)
        );
        // A project caught in the downward shift moves up one slot.
        assert_eq!(
            remap_target_after_project_move(thread(2, 1), 0, 3),
            thread(1, 1)
        );
        // A project caught in an upward shift moves down one slot.
        assert_eq!(
            remap_target_after_project_move(thread(1, 1), 3, 0),
            thread(2, 1)
        );
        // Chats are unaffected by a project move.
        assert_eq!(remap_target_after_project_move(chat(0), 0, 2), chat(0));
    }

    #[test]
    fn chat_home_cwd_never_empty() {
        // US-002 + Edge Case #1: home resolution always yields a non-empty,
        // usable cwd (home, else current dir, else "."). Never panics, never
        // a raw `$HOME`.
        let cwd = chat_home_cwd();
        assert!(!cwd.is_empty(), "chat cwd must never be empty");
    }
}
