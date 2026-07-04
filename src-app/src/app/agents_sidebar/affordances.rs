//! Affordance handlers for the Agents-mode sidebar (US-011 of
//! `tasks/prd-agents-view.md`): create / rename / delete /
//! duplicate, plus the cross-cutting reveal-in-file-manager and
//! open-in-editor entry points.
//!
//! Every handler mutates `PaneFlowApp` state in-place, calls
//! `cx.notify()` and `save_session(cx)` so the change is persisted
//! across restarts, and -- when a deletion involves a row that has
//! been written to `threads.db` -- cascades the SQL DELETE so the
//! durable store does not accumulate orphan rows (AC #9).

use gpui::{
    AppContext, ClickEvent, Context, MouseButton, PathPromptOptions, Pixels, Point, Window,
};

use super::state::{AgentsContextMenu, AgentsDeleteTarget, AgentsRenameTarget};
use crate::PaneFlowApp;
use crate::app::workspace_ops::{resolve_editor_binary, reveal_in_file_manager};
use crate::widgets::text_area::TextArea;

impl PaneFlowApp {
    // ------------------------------------------------------------------
    // Open / close context menus + confirmation dialog
    // ------------------------------------------------------------------

    /// Open the project-row context menu at the click position.
    /// Closes any prior menu and cancels any pending rename so the
    /// menu opens on a clean slate.
    pub(crate) fn open_agents_project_menu(
        &mut self,
        project_idx: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.cancel_agents_rename(cx);
        self.agents_view.agents_menu_open = Some(AgentsContextMenu::Project {
            project_idx,
            position,
        });
        cx.notify();
    }

    /// Open the thread-row context menu at the click position.
    pub(crate) fn open_agents_thread_menu(
        &mut self,
        project_idx: usize,
        thread_idx: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.cancel_agents_rename(cx);
        self.agents_view.agents_menu_open = Some(AgentsContextMenu::Thread {
            project_idx,
            thread_idx,
            position,
        });
        cx.notify();
    }

    /// US-008: open the context menu for a free chat row.
    pub(crate) fn open_agents_chat_menu(
        &mut self,
        chat_idx: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.cancel_agents_rename(cx);
        self.agents_view.agents_menu_open = Some(AgentsContextMenu::Chat { chat_idx, position });
        cx.notify();
    }

    pub(crate) fn close_agents_menu(&mut self, cx: &mut Context<Self>) {
        if self.agents_view.agents_menu_open.take().is_some() {
            cx.notify();
        }
    }

    /// Queue a confirmation dialog for `target`. Idempotent: replacing
    /// a pending confirm with a fresh one is fine.
    pub(crate) fn request_agents_confirm_delete(
        &mut self,
        target: AgentsDeleteTarget,
        cx: &mut Context<Self>,
    ) {
        self.agents_view.agents_menu_open = None;
        self.agents_view.agents_confirm_delete = Some(target);
        cx.notify();
    }

    pub(crate) fn cancel_agents_confirm_delete(&mut self, cx: &mut Context<Self>) {
        if self.agents_view.agents_confirm_delete.take().is_some() {
            cx.notify();
        }
    }

    // ------------------------------------------------------------------
    // Rename machinery
    // ------------------------------------------------------------------

    /// Enter inline-rename mode for `target`. Creates a fresh
    /// [`TextArea`] entity (real editable input -- cursor / selection
    /// / IME / clipboard, mirrors what the chat composer uses) and
    /// focuses it so the next keystroke lands in the field.
    ///
    /// Cancels any prior rename before starting -- only one row can
    /// be renaming at a time.
    pub(crate) fn begin_agents_rename(
        &mut self,
        target: AgentsRenameTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_agents_rename(cx);
        let current = match target {
            AgentsRenameTarget::Project { project_idx } => self
                .projects
                .get(project_idx)
                .map(|p| p.title.clone())
                .unwrap_or_default(),
            AgentsRenameTarget::Thread {
                project_idx,
                thread_idx,
            } => self
                .projects
                .get(project_idx)
                .and_then(|p| p.threads.get(thread_idx))
                .map(|t| t.title.clone())
                .unwrap_or_default(),
            AgentsRenameTarget::Chat { chat_idx } => self
                .chats
                .get(chat_idx)
                .map(|t| t.title.clone())
                .unwrap_or_default(),
        };
        let app_weak = cx.weak_entity();
        let textarea = cx.new(|cx| {
            let mut ta = TextArea::new("New name", cx);
            ta.set_value(current, cx);
            // Pre-select the whole text so the user can just start typing
            // to replace the existing name - saves a manual Ctrl+A and
            // matches the inline-rename UX in mainstream editors.
            ta.select_all_text(cx);
            // on_submit fires from INSIDE the TextArea's update, so
            // any re-read of the entity (`ta.read(cx)`) from the
            // callback panics with "cannot read while it is already
            // being updated". Pass the text the callback already
            // hands us straight through to `apply_agents_rename`,
            // which never touches the entity.
            let weak_submit = app_weak.clone();
            ta.on_submit(move |text, _w, app| {
                let _ = weak_submit
                    .clone()
                    .update(app, |this, cx| this.apply_agents_rename(text, cx));
            });
            let weak_escape = app_weak;
            ta.on_escape(move |_w, app| {
                let _ = weak_escape
                    .clone()
                    .update(app, |this, cx| this.cancel_agents_rename(cx));
            });
            ta
        });
        let focus = textarea.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        self.agents_view.agents_renaming = Some(target);
        self.agents_view.agents_rename_input = Some(textarea);
        // `agents_rename_text` is kept only as a legacy bridge for
        // call sites that haven't been migrated to read from the
        // TextArea entity yet; left empty on purpose.
        self.agents_view.agents_rename_text.clear();
        cx.notify();
    }

    /// Apply the in-progress rename, reading the latest value from
    /// the TextArea entity itself. Called from non-callback paths
    /// (e.g. click-outside, context-menu open) where the entity is
    /// NOT currently in an `update()` call and can safely be read.
    ///
    /// Code coming from the TextArea's `on_submit` callback must use
    /// [`Self::apply_agents_rename`] instead (it receives the text
    /// via the callback parameter and avoids re-entering the entity).
    pub(crate) fn commit_agents_rename(&mut self, cx: &mut Context<Self>) {
        if self.agents_view.agents_renaming.is_none() {
            return;
        }
        // Re-entrancy guardrail: `commit_agents_rename` reads the
        // TextArea entity, which panics if called from inside the
        // TextArea's own `on_submit` / `on_change` callback. Callers
        // coming from a TextArea callback must route through
        // `apply_agents_rename(text, cx)` (it accepts the value as a
        // parameter and never touches the entity). Document the
        // contract in debug builds so a future caller misuse trips
        // here loudly rather than corrupting GPUI's RefCell state.
        debug_assert!(
            self.agents_view.agents_rename_input.is_some(),
            "commit_agents_rename invariant: rename input must exist when renaming is active",
        );
        let text = self
            .agents_view
            .agents_rename_input
            .as_ref()
            .map(|ta| ta.read(cx).value())
            .unwrap_or_default();
        self.apply_agents_rename(text, cx);
    }

    /// Apply `text` as the new title for whatever row is currently
    /// in rename mode, then exit rename mode. Empty / whitespace-only
    /// text is treated as "user gave up" and rolls back without
    /// touching the title.
    ///
    /// Safe to call from inside the TextArea entity's `update` (the
    /// `on_submit` callback path): the text is taken as a parameter
    /// instead of read from the entity, and the entity is dropped on
    /// the next event-loop tick via `cx.defer` so we never re-enter
    /// the in-flight update.
    pub(crate) fn apply_agents_rename(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(target) = self.agents_view.agents_renaming.take() else {
            return;
        };
        // Drop the TextArea entity on the next tick to avoid any
        // re-entrancy when this is invoked from on_submit (where we
        // are still inside the entity's update).
        let weak = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = weak.update(cx, |app, cx| {
                app.agents_view.agents_rename_input = None;
                app.agents_view.agents_rename_text.clear();
                cx.notify();
            });
        });
        let text = text.trim().to_string();
        if text.is_empty() {
            cx.notify();
            return;
        }
        match target {
            AgentsRenameTarget::Project { project_idx } => {
                if let Some(project) = self.projects.get_mut(project_idx) {
                    project.title = text;
                    self.save_session(cx);
                }
            }
            AgentsRenameTarget::Thread {
                project_idx,
                thread_idx,
            } => {
                if let Some(thread) = self
                    .projects
                    .get_mut(project_idx)
                    .and_then(|p| p.threads.get_mut(thread_idx))
                {
                    thread.title = text;
                    // Lock the label: OSC updates and the ai-title backfill
                    // must not clobber a deliberate name.
                    thread.title_user_set = true;
                }
                self.save_session(cx);
            }
            AgentsRenameTarget::Chat { chat_idx } => {
                if let Some(chat) = self.chats.get_mut(chat_idx) {
                    chat.title = text;
                    chat.title_user_set = true;
                }
                self.save_session(cx);
            }
        }
        cx.notify();
    }

    /// Drop the in-progress rename without applying. Used when the
    /// user presses Escape, opens a context menu, or clicks elsewhere.
    /// Defers the entity drop to the next tick so a call from the
    /// TextArea's `on_escape` callback never re-enters the in-flight
    /// entity update.
    pub(crate) fn cancel_agents_rename(&mut self, cx: &mut Context<Self>) {
        if self.agents_view.agents_renaming.take().is_some() {
            let weak = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |app, cx| {
                    app.agents_view.agents_rename_input = None;
                    app.agents_view.agents_rename_text.clear();
                    cx.notify();
                });
            });
        }
    }

    // ------------------------------------------------------------------
    // Create / Duplicate
    // ------------------------------------------------------------------

    /// Create a new project by prompting the user for one or more
    /// directories via the OS folder picker. Mirrors the CLI sidebar's
    /// `create_workspace_with_picker`: each chosen folder becomes a
    /// fresh project, with the folder's basename as the title. The
    /// most recently created project is selected so the threads list
    /// opens onto it.
    pub(crate) fn create_agents_project_with_picker(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: None,
        });
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                if let Ok(Ok(Some(paths))) = receiver.await {
                    let _ = cx.update(|cx| {
                        this.update(cx, |app, cx| {
                            let mut last_created: Option<usize> = None;
                            for path in paths {
                                let title = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| "New project".to_string());
                                let cwd = path.to_string_lossy().into_owned();
                                match app.create_project(title, cwd.as_str(), cx) {
                                    Ok(_id) => {
                                        last_created = Some(app.projects.len() - 1);
                                    }
                                    Err(err) => {
                                        app.show_toast(
                                            format!("Could not create project: {err:?}"),
                                            cx,
                                        );
                                    }
                                }
                            }
                            if let Some(idx) = last_created {
                                // Select the new project with no thread so
                                // the main area shows the agent picker for
                                // it: "New threads -> pick folder -> pick
                                // agent -> terminal launches".
                                app.active_project_idx = idx;
                                // US-003: picker/home state (no target).
                                app.agents_target = None;
                                // US-005: project picker, not chat.
                                app.agents_picker_context =
                                    crate::project::AgentsPickerContext::Project;
                            }
                            app.save_session(cx);
                            cx.notify();
                        })
                    });
                }
            },
        )
        .detach();
    }

    /// "New thread" affordance: select `project_idx` with no thread so
    /// the main area shows the agent picker for it. Selecting an agent
    /// there creates a Terminal Thread (see
    /// [`Self::create_agent_terminal_thread_in`]). No thread is created
    /// until the user picks an agent.
    pub(crate) fn create_agents_thread_in(&mut self, project_idx: usize, cx: &mut Context<Self>) {
        if project_idx >= self.projects.len() {
            return;
        }
        self.agents_view.agents_skills_visible = false;
        // US-003: picker/home state for `project_idx` (no target selected).
        self.agents_target = None;
        // US-005: this picker creates into a project, not a free chat.
        self.agents_picker_context = crate::project::AgentsPickerContext::Project;
        self.active_project_idx = project_idx;
        cx.notify();
    }

    /// Picker selection: create a Terminal Thread bound to `agent` in
    /// `project_idx` and select it. The agent CLI is auto-launched on
    /// the first PTY mount in
    /// [`PaneFlowApp::ensure_terminal_view_mounted`] (which reads the
    /// thread's `terminal_agent` and honors the bypass-permission flag).
    pub(crate) fn create_agent_terminal_thread_in(
        &mut self,
        project_idx: usize,
        agent: crate::agent_launcher::TerminalAgent,
        cx: &mut Context<Self>,
    ) {
        if !agent.is_installed() {
            self.show_toast(format!("{} is not installed", agent.display_name()), cx);
            return;
        }
        let new_thread_id =
            match self.add_terminal_thread(project_idx, agent.display_name(), Some(agent), cx) {
                Ok(id) => id,
                Err(err) => {
                    self.show_toast(format!("Could not create thread: {err:?}"), cx);
                    return;
                }
            };
        if let Some(project) = self.projects.get(project_idx)
            && let Some(thread_idx) = project.threads.iter().position(|t| t.id == new_thread_id)
        {
            let _ = self.select_thread(project_idx, thread_idx, cx);
        }
        self.save_session(cx);
    }

    /// Create a fresh bare Terminal Thread (no agent) in `project_idx`:
    /// a plain shell in the project's cwd. Backs the secondary
    /// "New terminal thread" affordance for when the user wants a raw
    /// terminal rather than a launched agent.
    pub(crate) fn create_terminal_thread_in(&mut self, project_idx: usize, cx: &mut Context<Self>) {
        let new_thread_id = match self.add_terminal_thread(project_idx, "Terminal", None, cx) {
            Ok(id) => id,
            Err(err) => {
                self.show_toast(format!("Could not create terminal thread: {err:?}"), cx);
                return;
            }
        };
        if let Some(project) = self.projects.get(project_idx)
            && let Some(thread_idx) = project.threads.iter().position(|t| t.id == new_thread_id)
        {
            let _ = self.select_thread(project_idx, thread_idx, cx);
        }
        self.save_session(cx);
    }

    /// Duplicate a thread: same agent + cwd, fresh ID. The copy is a
    /// Terminal Thread bound to the source's `terminal_agent` (so it
    /// relaunches the same CLI on first open).
    pub(crate) fn duplicate_agents_thread(
        &mut self,
        project_idx: usize,
        thread_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let (terminal_agent, base_title) = match self
            .projects
            .get(project_idx)
            .and_then(|p| p.threads.get(thread_idx))
        {
            Some(t) => (t.terminal_agent, t.title.clone()),
            None => return,
        };

        let new_title = format!("{base_title} (copy)");
        if let Err(err) = self.add_terminal_thread(project_idx, new_title, terminal_agent, cx) {
            self.show_toast(format!("Could not duplicate thread: {err:?}"), cx);
            return;
        }
        self.save_session(cx);
    }

    /// Duplicate a free chat: same agent + home cwd, fresh ID (US-008).
    pub(crate) fn duplicate_agents_chat(&mut self, chat_idx: usize, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get(chat_idx) else {
            return;
        };
        let terminal_agent = chat.terminal_agent;
        let new_title = format!("{} (copy)", chat.title);
        self.add_chat_thread(new_title, terminal_agent, cx);
        self.save_session(cx);
    }

    // ------------------------------------------------------------------
    // US-005/US-006/US-008: target-parameterized dispatch
    //
    // The Pinned and Chats sections render rows that may back either a
    // project thread OR a free chat. These helpers map a unified
    // [`crate::project::AgentsTarget`] to the right concrete handler so the
    // row widgets stay source-agnostic (no duplicated project/chat render
    // logic, per the PRD maintainability requirement).
    // ------------------------------------------------------------------

    /// Select whatever the target points at (US-006: a Pinned row routes to
    /// its original source).
    pub(crate) fn select_agents_target(
        &mut self,
        target: crate::project::AgentsTarget,
        cx: &mut Context<Self>,
    ) {
        use crate::project::AgentsTarget;
        let _ = match target {
            AgentsTarget::Thread {
                project_idx,
                thread_idx,
            } => self.select_thread(project_idx, thread_idx, cx),
            AgentsTarget::Chat { chat_idx } => self.select_chat(chat_idx, cx),
        };
    }

    /// Open the context menu for the target's row.
    pub(crate) fn open_agents_menu_for_target(
        &mut self,
        target: crate::project::AgentsTarget,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        use crate::project::AgentsTarget;
        // Opening a context menu cancels any armed inline delete-confirm.
        self.agents_view.agents_delete_armed = None;
        match target {
            AgentsTarget::Thread {
                project_idx,
                thread_idx,
            } => self.open_agents_thread_menu(project_idx, thread_idx, position, cx),
            AgentsTarget::Chat { chat_idx } => self.open_agents_chat_menu(chat_idx, position, cx),
        }
    }

    /// Begin inline rename for the target's row.
    pub(crate) fn begin_agents_rename_for_target(
        &mut self,
        target: crate::project::AgentsTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::project::AgentsTarget;
        let rename_target = match target {
            AgentsTarget::Thread {
                project_idx,
                thread_idx,
            } => AgentsRenameTarget::Thread {
                project_idx,
                thread_idx,
            },
            AgentsTarget::Chat { chat_idx } => AgentsRenameTarget::Chat { chat_idx },
        };
        self.begin_agents_rename(rename_target, window, cx);
    }

    /// Queue the delete confirmation for the target's row.
    pub(crate) fn request_delete_for_target(
        &mut self,
        target: crate::project::AgentsTarget,
        cx: &mut Context<Self>,
    ) {
        use crate::project::AgentsTarget;
        let delete_target = match target {
            AgentsTarget::Thread {
                project_idx,
                thread_idx,
            } => AgentsDeleteTarget::Thread {
                project_idx,
                thread_idx,
            },
            AgentsTarget::Chat { chat_idx } => AgentsDeleteTarget::Chat { chat_idx },
        };
        self.request_agents_confirm_delete(delete_target, cx);
    }

    /// Arm the inline delete-confirm for `target`'s row (ergonomics): clicking
    /// the trash icon does NOT open a dialog - it flips the row's action
    /// cluster to a red "Delete" button. A second click on that button runs
    /// the delete; selecting a row / opening a menu / arming another cancels it.
    pub(crate) fn arm_delete_for_target(
        &mut self,
        target: crate::project::AgentsTarget,
        cx: &mut Context<Self>,
    ) {
        self.agents_view.agents_delete_armed = Some(target);
        cx.notify();
    }

    /// Run the armed inline delete. Funnels the armed target through
    /// `agents_confirm_delete` so it reuses the exact confirmed-delete path
    /// (toast + cache cleanup + session save) - no dialog is ever shown.
    pub(crate) fn execute_armed_delete(&mut self, cx: &mut Context<Self>) {
        use crate::project::AgentsTarget;
        let Some(target) = self.agents_view.agents_delete_armed.take() else {
            return;
        };
        let delete_target = match target {
            AgentsTarget::Thread {
                project_idx,
                thread_idx,
            } => AgentsDeleteTarget::Thread {
                project_idx,
                thread_idx,
            },
            AgentsTarget::Chat { chat_idx } => AgentsDeleteTarget::Chat { chat_idx },
        };
        self.agents_view.agents_confirm_delete = Some(delete_target);
        self.execute_agents_confirm_delete(cx);
        cx.notify();
    }

    /// Duplicate the target's row.
    pub(crate) fn duplicate_agents_target(
        &mut self,
        target: crate::project::AgentsTarget,
        cx: &mut Context<Self>,
    ) {
        use crate::project::AgentsTarget;
        match target {
            AgentsTarget::Thread {
                project_idx,
                thread_idx,
            } => self.duplicate_agents_thread(project_idx, thread_idx, cx),
            AgentsTarget::Chat { chat_idx } => self.duplicate_agents_chat(chat_idx, cx),
        }
    }

    /// Restart the target's mounted PTY so the next render spawns it with the
    /// current terminal settings, including `default_shell`.
    pub(crate) fn restart_agents_target_terminal(
        &mut self,
        target: crate::project::AgentsTarget,
        cx: &mut Context<Self>,
    ) {
        let Some(thread_id) = self.thread_for_target(target).map(|thread| thread.id) else {
            return;
        };
        let selected = self.agents_target == Some(target);
        let mounted = self
            .agents_view
            .agents_terminal_view_cache
            .contains_key(&thread_id);
        self.agents_view.agents_menu_open = None;
        self.remove_agents_terminal_cache_entry(thread_id);
        if selected || mounted {
            self.show_toast("Terminal restarting with current shell", cx);
        } else {
            self.show_toast("Terminal will start with current shell", cx);
        }
        cx.notify();
    }

    /// US-006: toggle the pin flag on the target's thread/chat and persist.
    /// Idempotent per-click (a re-pin just flips back). Pinned threads are
    /// aggregated into the rail's PINNED section across both sources.
    pub(crate) fn toggle_pin_for_target(
        &mut self,
        target: crate::project::AgentsTarget,
        cx: &mut Context<Self>,
    ) {
        use crate::project::AgentsTarget;
        let pinned = match target {
            AgentsTarget::Thread {
                project_idx,
                thread_idx,
            } => self
                .projects
                .get_mut(project_idx)
                .and_then(|p| p.threads.get_mut(thread_idx))
                .map(|t| {
                    t.pinned = !t.pinned;
                    t.pinned
                }),
            AgentsTarget::Chat { chat_idx } => self.chats.get_mut(chat_idx).map(|c| {
                c.pinned = !c.pinned;
                c.pinned
            }),
        };
        if pinned.is_some() {
            self.save_session(cx);
            cx.notify();
        }
    }

    // ------------------------------------------------------------------
    // US-005: New chat (free chat in the home dir)
    // ------------------------------------------------------------------

    /// "New chat" affordance: drop to the picker/home state in
    /// new-chat context so the center shows the chat agent picker (cwd =
    /// home). Picking an agent there creates a free chat (see
    /// [`Self::create_agent_chat`]). No chat is created until an agent is
    /// picked - mirrors the project "New thread" flow.
    pub(crate) fn start_new_chat(&mut self, cx: &mut Context<Self>) {
        self.agents_view.agents_skills_visible = false;
        self.agents_target = None;
        self.agents_picker_context = crate::project::AgentsPickerContext::NewChat;
        cx.notify();
    }

    /// Picker selection in new-chat context: create a free chat bound to
    /// `agent` (cwd = home) and select it. The CLI auto-launches on the
    /// first PTY mount in [`PaneFlowApp::ensure_terminal_view_mounted`]
    /// (which resolves the chat target and honors the bypass flag),
    /// preserving the human-in-loop invariant (only the launch command is
    /// sent; no user prompt is auto-submitted).
    pub(crate) fn create_agent_chat(
        &mut self,
        agent: crate::agent_launcher::TerminalAgent,
        cx: &mut Context<Self>,
    ) {
        if !agent.is_installed() {
            self.show_toast(format!("{} is not installed", agent.display_name()), cx);
            return;
        }
        let new_chat_id = self.add_chat_thread(agent.display_name(), Some(agent), cx);
        if let Some(chat_idx) = self.chats.iter().position(|t| t.id == new_chat_id) {
            let _ = self.select_chat(chat_idx, cx);
        }
        self.save_session(cx);
    }

    // ------------------------------------------------------------------
    // Execute confirmed delete
    // ------------------------------------------------------------------

    /// Apply the pending delete (project or thread). The in-memory
    /// caches are cascaded by `close_project` / `remove_thread`; Terminal
    /// Threads have no durable rows to clean up.
    pub(crate) fn execute_agents_confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.agents_view.agents_confirm_delete.take() else {
            return;
        };
        match target {
            AgentsDeleteTarget::Project { project_idx } => {
                let Ok(removed) = self.close_project(project_idx, cx) else {
                    return;
                };
                let count = removed.threads.len();
                self.show_toast(
                    if count == 0 {
                        "Project deleted".to_string()
                    } else if count == 1 {
                        "Project + 1 thread deleted".to_string()
                    } else {
                        format!("Project + {count} threads deleted")
                    },
                    cx,
                );
            }
            AgentsDeleteTarget::Thread {
                project_idx,
                thread_idx,
            } => {
                if self.remove_thread(project_idx, thread_idx, cx).is_err() {
                    return;
                }
                self.show_toast("Thread deleted", cx);
            }
            AgentsDeleteTarget::Chat { chat_idx } => {
                if self.remove_chat(chat_idx, cx).is_err() {
                    return;
                }
                self.show_toast("Chat deleted", cx);
            }
        }
        self.save_session(cx);
    }

    // ------------------------------------------------------------------
    // Reveal / Open in editor (project rows)
    // ------------------------------------------------------------------

    /// US-011: reveal the cwd of a thread/chat target in the OS file
    /// manager. A project thread reveals its own cwd (defaults to the
    /// project cwd); a chat reveals its home cwd ("adapté au home" per the
    /// overflow-menu AC). Closes any open menu first.
    pub(crate) fn reveal_agents_target(
        &mut self,
        target: crate::project::AgentsTarget,
        cx: &mut Context<Self>,
    ) {
        let cwd = self.thread_for_target(target).map(|t| t.cwd.clone());
        self.agents_view.agents_menu_open = None;
        let Some(cwd) = cwd else {
            cx.notify();
            return;
        };
        if let Err(msg) = reveal_in_file_manager(std::path::Path::new(&cwd)) {
            log::warn!("agents-overflow: reveal failed: {msg}");
            self.show_toast(msg, cx);
        }
        cx.notify();
    }

    pub(crate) fn reveal_agents_project_in_file_manager(
        &mut self,
        project_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.projects.get(project_idx) else {
            return;
        };
        let cwd = project.cwd.clone();
        self.agents_view.agents_menu_open = None;
        if let Err(msg) = reveal_in_file_manager(std::path::Path::new(&cwd)) {
            log::warn!("agents-sidebar: reveal failed: {msg}");
            self.show_toast(msg, cx);
        }
        cx.notify();
    }

    pub(crate) fn open_agents_project_in_editor(
        &mut self,
        project_idx: usize,
        command: &str,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.projects.get(project_idx) else {
            return;
        };
        let cwd = project.cwd.clone();
        let bin = resolve_editor_binary(command);
        if let Err(err) = std::process::Command::new(&bin)
            .current_dir(&cwd)
            .arg(".")
            .spawn()
        {
            log::warn!("agents-sidebar: open in {label} failed: {err}");
            self.show_toast(format!("Couldn't open in {label}: {err}"), cx);
        }
        self.agents_view.agents_menu_open = None;
        cx.notify();
    }
}

/// Convenience for the sidebar `on_aux_click` listener: a single line
/// that opens the right context menu given the right-click hit. Lives
/// here (not inline at the call site) so the sidebar render stays
/// readable and the menu dispatch logic has one home.
pub(crate) fn handle_right_click_for_project(
    this: &mut PaneFlowApp,
    project_idx: usize,
    e: &ClickEvent,
    cx: &mut Context<PaneFlowApp>,
) {
    if let Some(position) = e.mouse_position()
        && matches!(e, ClickEvent::Mouse(m) if m.down.button == MouseButton::Right)
    {
        this.open_agents_project_menu(project_idx, position, cx);
    }
}

pub(crate) fn handle_right_click_for_thread(
    this: &mut PaneFlowApp,
    project_idx: usize,
    thread_idx: usize,
    e: &ClickEvent,
    cx: &mut Context<PaneFlowApp>,
) {
    if let Some(position) = e.mouse_position()
        && matches!(e, ClickEvent::Mouse(m) if m.down.button == MouseButton::Right)
    {
        this.open_agents_thread_menu(project_idx, thread_idx, position, cx);
    }
}
