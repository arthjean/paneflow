//! Launch Pad (EP-002 US-005, prd-cli-cockpit-ergonomics-2026-Q3.md).
//!
//! A modal (custom-buttons modal scaffold) that compresses the
//! worktree-per-agent ritual into one gesture: pick an agent, name a NEW
//! branch, optionally write a prompt - confirm runs the existing
//! orchestration-v2 worktree engine OFF the render thread
//! (`smol::unblock` + `worktree::add_worktree`, 120 s deadline, sibling
//! `<repo>.worktrees/<slug>` or hashed collision fallback, then
//! `copy_env_files` no-clobber), and only
//! on success splits the focused pane at the worktree path, launches the
//! agent CLI (`TerminalAgent::launch_command`, honors the Claude bypass
//! setting) and pre-fills the prompt through the existing settle-poll -
//! never submitted (FR-01).
//!
//! Atomicity (US-005 AC4): a worktree failure surfaces git's error verbatim
//! in the modal and creates NO pane. Branch names are NOT validated locally
//! git is the single authority (AC7). The created worktree is registered
//! as a [`ManagedWorktree`] so teardown parity with `paneflow up` holds
//! (AC5 - no second worktree population).

use gpui::{
    AnyElement, ClickEvent, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, SharedString, Styled, WeakEntity, Window, deferred, div,
    prelude::*, px, svg,
};
use paneflow_config::schema::TerminalSurfaceProfile;

use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;
use crate::layout::{MAX_PANES, SplitDirection};
use crate::pane::Pane;
use crate::terminal::TerminalView;
use crate::widgets::text_area::TextArea;
use crate::widgets::text_input::TextInput;
use crate::workspace::worktree::{self, ManagedWorktree};

/// Live Launch Pad modal state, owned by `PaneFlowApp`.
pub(crate) struct LaunchPadState {
    /// Workspace the launch targets, by stable id (survives reorders and
    /// closes - re-resolved when the background work returns).
    pub(crate) ws_id: u64,
    /// Pane to split next to; weak so a close while the modal is open (or
    /// while git runs) degrades to splitting the first leaf.
    pub(crate) target: WeakEntity<Pane>,
    /// Index into [`TerminalAgent::ALL`].
    pub(crate) agent_idx: usize,
    pub(crate) branch_input: Entity<TextInput>,
    pub(crate) prompt_input: Entity<TextArea>,
    /// `true` while the worktree creation runs - disables re-submission
    /// (US-005 AC8: no double worktree) and Escape.
    pub(crate) running: bool,
    /// Last failure, shown verbatim in the modal (git stderr included).
    pub(crate) error: Option<String>,
}

/// Everything the background-completion handler needs to build the pane.
struct LaunchPlan {
    ws_id: u64,
    repo_root: std::path::PathBuf,
    worktree_path: std::path::PathBuf,
    branch: String,
    agent: TerminalAgent,
    prompt: String,
}

fn launch_pad_worktree_plan(
    repo_root: &std::path::Path,
    branch: &str,
) -> Result<(std::path::PathBuf, bool), String> {
    let legacy_path = worktree::worktree_dir(repo_root, branch);
    let hashed_path = worktree::worktree_dir_hashed(repo_root, branch);
    let entries = worktree::list_worktrees(repo_root)?;
    let mut path = legacy_path.clone();
    for entry in &entries {
        if entry.branch.as_deref() == Some(branch) {
            return Err(format!(
                "branch '{branch}' is already checked out at {}",
                entry.path.display()
            ));
        }
        if entry.path == legacy_path {
            path = hashed_path.clone();
        }
    }
    for entry in &entries {
        if entry.path == path {
            return Err(format!(
                "{} exists but holds another branch ({})",
                path.display(),
                entry.branch.as_deref().unwrap_or("detached")
            ));
        }
    }
    if path.exists() {
        return Err(format!(
            "{} exists but is not a registered worktree; remove it first",
            path.display()
        ));
    }
    Ok((path, !worktree::branch_exists(repo_root, branch)))
}

impl PaneFlowApp {
    pub(crate) fn handle_open_launch_pad(
        &mut self,
        _: &crate::OpenLaunchPad,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.mode, paneflow_config::schema::AppMode::Cli) {
            return;
        }
        if self.launch_pad.is_some() {
            // Toggle semantics, but never abandon a run in flight.
            if !self.launch_pad.as_ref().is_some_and(|lp| lp.running) {
                self.launch_pad = None;
                cx.notify();
            }
            return;
        }
        let Some(ws) = self.active_workspace() else {
            return;
        };
        let ws_id = ws.id;
        let target = self
            .focused_or_first_pane(window, cx)
            .map(|p| p.downgrade())
            .unwrap_or_else(WeakEntity::new_invalid);

        let weak_app = cx.entity().downgrade();
        let branch_input = cx.new(|cx| TextInput::new("", "new-branch-name", cx));
        let prompt_input =
            cx.new(|cx| TextArea::new("Prompt (optional) - pre-filled, never submitted", cx));
        prompt_input.update(cx, |ta, _| {
            // The prompt is OPTIONAL: Enter in the empty field must still
            // confirm the form (review R2 - TaSubmit would otherwise
            // swallow the key without bubbling to the modal handler).
            ta.set_submit_on_empty(true);
            // Same re-entrancy discipline as the Composer: defer + weak.
            let w = weak_app.clone();
            ta.on_submit(move |_text, _window, cx| {
                let w = w.clone();
                cx.defer(move |cx| {
                    let _ = w.update(cx, |app, cx| app.launch_pad_confirm(cx));
                });
            });
            let w = weak_app.clone();
            ta.on_escape(move |_window, cx| {
                let w = w.clone();
                cx.defer(move |cx| {
                    let _ = w.update(cx, |app, cx| app.launch_pad_cancel(cx));
                });
            });
        });
        let branch_focus = branch_input.read(cx).focus_handle.clone();

        // Default to the first installed agent so confirm works out of the
        // box; fall back to 0 (the row renders grayed, confirm rejects).
        let agent_idx = TerminalAgent::ALL
            .iter()
            .position(|a| a.is_installed())
            .unwrap_or(0);

        self.launch_pad = Some(LaunchPadState {
            ws_id,
            target,
            agent_idx,
            branch_input,
            prompt_input,
            running: false,
            error: None,
        });
        window.focus(&branch_focus, cx);
        cx.notify();
    }

    /// Escape path - only honored before confirmation (US-005 AC8: the
    /// in-flight run keeps the modal up with its "Creating…" state).
    pub(crate) fn launch_pad_cancel(&mut self, cx: &mut Context<Self>) {
        if self.launch_pad.as_ref().is_some_and(|lp| lp.running) {
            return;
        }
        self.launch_pad = None;
        cx.notify();
    }

    fn launch_pad_set_error(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        if let Some(lp) = self.launch_pad.as_mut() {
            lp.running = false;
            lp.error = Some(message.into());
            cx.notify();
        }
    }

    /// Validate the form and run the worktree engine off-thread. Nothing is
    /// executed when a guard fails (US-005 AC6) - the error shows in the
    /// modal and the form stays editable.
    pub(crate) fn launch_pad_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(lp) = self.launch_pad.as_ref() else {
            return;
        };
        if lp.running {
            // AC8: a click/Enter during the run never double-creates.
            return;
        }
        let ws_id = lp.ws_id;
        let agent_idx = lp.agent_idx;
        let branch = lp.branch_input.read(cx).value().trim().to_string();
        // Same delivery profile as the Composer (security review): LF-only,
        // trailing newlines trimmed, 64 KiB cap before the PTY write.
        let (prompt, _truncated) =
            crate::app::composer::normalize_composer_text(&lp.prompt_input.read(cx).value());

        let Some(agent) = TerminalAgent::ALL.get(agent_idx).copied() else {
            self.launch_pad_set_error("No agent selected", cx);
            return;
        };
        if !agent.is_installed() {
            self.launch_pad_set_error(format!("{} is not installed", agent.display_name()), cx);
            return;
        }
        if branch.is_empty() {
            self.launch_pad_set_error("Branch name is empty", cx);
            return;
        }
        let Some(ws) = self.workspaces.iter().find(|w| w.id == ws_id) else {
            self.launch_pad_set_error("Workspace was closed", cx);
            return;
        };
        // AC6: cwd without a git repo → explicit error, nothing executed.
        let Some(repo_root) = ws.repo_root.clone() else {
            self.launch_pad_set_error("No git repository for this workspace", cx);
            return;
        };
        if ws.root.as_ref().is_none_or(|r| r.leaf_count() >= MAX_PANES) {
            self.launch_pad_set_error(format!("Maximum pane count reached ({MAX_PANES})"), cx);
            return;
        }

        let worktree_path = worktree::worktree_dir(&repo_root, &branch);
        if let Some(lp) = self.launch_pad.as_mut() {
            lp.running = true;
            lp.error = None;
        }
        cx.notify();

        let plan = LaunchPlan {
            ws_id,
            repo_root: repo_root.clone(),
            worktree_path: worktree_path.clone(),
            branch: branch.clone(),
            agent,
            prompt,
        };
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                // The engine is synchronous (git subprocess, 120 s deadline)
                // - run it on the blocking pool, never the render thread.
                let result = smol::unblock(move || {
                    let (worktree_path, create_branch) =
                        launch_pad_worktree_plan(&repo_root, &branch)?;
                    worktree::add_worktree(&repo_root, &worktree_path, &branch, create_branch)?;
                    // Best-effort by design (US-007 orchestration-v2): a
                    // partial copy is not a failure.
                    let _ = worktree::copy_env_files(&repo_root, &worktree_path);
                    Ok::<std::path::PathBuf, String>(worktree_path)
                })
                .await;
                cx.update(|cx| {
                    let _ = this.update(cx, |app, cx| {
                        app.launch_pad_finish(result, plan, cx);
                    });
                });
            },
        )
        .detach();
    }

    /// Main-thread completion: error → verbatim in the modal, zero panes
    /// (AC4); success → split + launch + prefill + ManagedWorktree
    /// registration, then close.
    fn launch_pad_finish(
        &mut self,
        result: Result<std::path::PathBuf, String>,
        mut plan: LaunchPlan,
        cx: &mut Context<Self>,
    ) {
        let worktree_path = match result {
            Ok(path) => path,
            Err(e) => {
                if self.launch_pad.is_some() {
                    self.launch_pad_set_error(e, cx);
                } else {
                    self.show_toast(format!("Launch Pad: {e}"), cx);
                }
                return;
            }
        };
        plan.worktree_path = worktree_path;

        let Some(ws_idx) = self.workspaces.iter().position(|w| w.id == plan.ws_id) else {
            // Workspace closed during the run: the worktree exists on disk
            // but there is nothing to attach it to. Surface it instead of
            // silently leaking - `git worktree prune`/manual removal applies.
            log::warn!(
                "launch pad: workspace closed during worktree creation; {} left on disk",
                plan.worktree_path.display()
            );
            self.show_toast(
                format!(
                    "Workspace closed - worktree left at {}",
                    plan.worktree_path.display()
                ),
                cx,
            );
            self.launch_pad = None;
            cx.notify();
            return;
        };

        // AC5: register ownership FIRST so teardown parity holds even if a
        // later step fails - the workspace now owns this worktree exactly
        // like a `paneflow up` one.
        self.workspaces[ws_idx]
            .managed_worktrees
            .push(ManagedWorktree {
                path: plan.worktree_path.clone(),
                repo_root: plan.repo_root.clone(),
                branch: plan.branch.clone(),
                teardown: Default::default(),
            });

        // Re-check the pane budget - it may have filled during the run.
        if self.workspaces[ws_idx]
            .root
            .as_ref()
            .is_none_or(|r| r.leaf_count() >= MAX_PANES)
        {
            self.launch_pad_set_error(
                format!(
                    "Maximum pane count reached ({MAX_PANES}) - worktree created at {}",
                    plan.worktree_path.display()
                ),
                cx,
            );
            return;
        }

        let target = self
            .launch_pad
            .as_ref()
            .and_then(|lp| lp.target.upgrade())
            .filter(|t| {
                self.workspaces[ws_idx]
                    .root
                    .as_ref()
                    .is_some_and(|r| r.contains_leaf(t))
            });
        let new_terminal = cx.new(|cx| {
            TerminalView::with_cwd_env_and_profile(
                plan.ws_id,
                Some(plan.worktree_path.clone()),
                None,
                None,
                TerminalSurfaceProfile::Agent,
                cx,
            )
        });
        let new_pane = self.create_pane(new_terminal.clone(), plan.ws_id, cx);
        let Some(root) = self.workspaces[ws_idx].root.as_mut() else {
            self.launch_pad_set_error("Workspace has no layout root", cx);
            return;
        };
        // PRD: split in the active preset's direction, fallback Vertical.
        // No active preset is tracked anywhere (LayoutPreset is a one-shot
        // `workspace.up` input), so the documented fallback IS the default:
        // Vertical = side-by-side, the natural cockpit arrangement.
        match target {
            Some(t) => {
                if !root.split_at_pane(&t, SplitDirection::Vertical, new_pane.clone()) {
                    root.split_first_leaf(SplitDirection::Vertical, new_pane.clone());
                }
            }
            None => root.split_first_leaf(SplitDirection::Vertical, new_pane.clone()),
        }

        new_terminal
            .read(cx)
            .send_command(&plan.agent.launch_command(&self.cached_config));
        if !plan.prompt.trim().is_empty() {
            // Existing settle-poll: waits for the CLI to go quiet, then
            // pre-fills WITHOUT a carriage return - human-in-loop (FR-01).
            Self::schedule_prompt_prefill(&new_terminal, plan.prompt, usize::MAX, cx);
        }

        self.launch_pad = None;
        self.pending_pane_focus = Some(new_pane);
        self.activate_workspace_without_window(ws_idx, cx);
    }

    pub(crate) fn handle_launch_pad_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let Some(lp) = self.launch_pad.as_ref() else {
            return;
        };
        match key {
            "escape" => self.launch_pad_cancel(cx),
            "enter" => self.launch_pad_confirm(cx),
            "tab" => {
                // Toggle focus between the two text fields (custom-buttons
                // modal convention). The agent list is mouse-driven.
                let branch_focused = lp.branch_input.read(cx).focus_handle.is_focused(window);
                let next = if branch_focused {
                    lp.prompt_input.read(cx).focus_handle.clone()
                } else {
                    lp.branch_input.read(cx).focus_handle.clone()
                };
                window.focus(&next, cx);
                cx.notify();
            }
            _ => {}
        }
    }

    pub(crate) fn render_launch_pad(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(lp) = self.launch_pad.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();
        let running = lp.running;

        // Agent picker: every entry of TerminalAgent::ALL, the missing ones
        // grayed out and inert (US-005 AC1).
        let mut agent_list = div()
            .id("launch-pad-agents")
            .flex()
            .flex_col()
            .max_h(px(180.))
            .overflow_y_scroll()
            .border_1()
            .border_color(ui.border)
            .rounded(px(6.));
        for (idx, agent) in TerminalAgent::ALL.iter().enumerate() {
            let installed = agent.is_installed();
            let is_selected = idx == lp.agent_idx;
            let mut row = div()
                .id(SharedString::from(format!("launch-pad-agent-{idx}")))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(5.))
                .text_size(px(12.))
                .when(is_selected, |d| d.bg(ui.subtle))
                // Multi-color logos render via `img()` (resvg keeps every
                // native fill); monochrome logos stay a tinted `svg()` mask
                // - same split as the agents-view launcher.
                .child(if agent.icon_multicolor() {
                    gpui::img(agent.icon_path())
                        .size(px(13.))
                        .flex_none()
                        .when(!installed, |d| d.opacity(0.5))
                        .into_any_element()
                } else {
                    svg()
                        .size(px(13.))
                        .flex_none()
                        .path(agent.icon_path())
                        .text_color(if installed { ui.text } else { ui.muted })
                        .into_any_element()
                })
                .child(
                    div()
                        .flex_1()
                        .text_color(if installed { ui.text } else { ui.muted })
                        .when(!installed, |d| d.opacity(0.5))
                        .child(agent.display_name()),
                );
            if installed {
                row = row
                    .cursor_pointer()
                    .when(!is_selected, |d| d.hover(|s| s.bg(ui.subtle)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        if let Some(lp) = this.launch_pad.as_mut()
                            && !lp.running
                        {
                            lp.agent_idx = idx;
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }));
            } else {
                row = row.child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(ui.muted)
                        .child("not installed"),
                );
            }
            agent_list = agent_list.child(row);
        }

        let field_label =
            |label: &'static str| div().text_size(px(11.)).text_color(ui.muted).child(label);

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(16.))
            .py(px(10.))
            .child(field_label("Agent"))
            .child(agent_list)
            .child(field_label("New branch"))
            .child(
                div()
                    .border_1()
                    .border_color(ui.border)
                    .rounded(px(6.))
                    .px(px(8.))
                    .py(px(4.))
                    .child(lp.branch_input.clone()),
            )
            .child(field_label("Prompt"))
            .child(
                div()
                    .border_1()
                    .border_color(ui.border)
                    .rounded(px(6.))
                    .px(px(8.))
                    .py(px(4.))
                    .max_h(px(140.))
                    .child(lp.prompt_input.clone()),
            );

        if let Some(err) = &lp.error {
            // AC4/AC7: git's failure verbatim - inert text, never parsed.
            body = body.child(
                div()
                    .text_size(px(11.))
                    .text_color(ui.vc_deleted)
                    .child(err.clone()),
            );
        }

        let confirm_label: SharedString = if running {
            "Creating…".into()
        } else {
            "Create worktree + launch".into()
        };
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px(px(16.))
            .py(px(10.))
            .border_t_1()
            .border_color(ui.border)
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .child("Enter creates · Tab switches fields · Esc cancels"),
            )
            .child(
                div()
                    .id("launch-pad-confirm")
                    .px(px(12.))
                    .py(px(5.))
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .when(!running, |d| {
                        d.bg(ui.accent.opacity(0.15))
                            .text_color(ui.accent)
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.8))
                    })
                    .when(running, |d| d.bg(ui.subtle).text_color(ui.muted))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                        this.launch_pad_confirm(cx);
                        cx.stop_propagation();
                    }))
                    .child(confirm_label),
            );

        let card = div()
            .id("launch-pad")
            .occlude()
            .track_focus(&self.launch_pad_focus)
            .on_key_down(cx.listener(Self::handle_launch_pad_key_down))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.launch_pad_cancel(cx);
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .w(px(520.))
            .flex()
            .flex_col()
            .bg(ui.overlay)
            .border_1()
            .border_color(ui.border)
            .rounded(px(10.))
            .overflow_hidden()
            .child(
                div()
                    .px(px(16.))
                    .pt(px(14.))
                    .pb(px(6.))
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(ui.text)
                    .child("Launch Pad"),
            )
            .child(body)
            .child(footer);

        deferred(
            div()
                .id("launch-pad-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(72.))
                .bg(gpui::hsla(0., 0., 0., 0.4))
                .child(card),
        )
        .with_priority(8)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_pad_plan_uses_hashed_path_when_slug_path_is_claimed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir(&repo_root).expect("repo dir");
        if !test_git(&repo_root, &["init"]) {
            return;
        }
        assert!(test_git(&repo_root, &["config", "core.autocrlf", "false"]));
        std::fs::write(repo_root.join("README.md"), "init\n").expect("readme");
        assert!(test_git(&repo_root, &["add", "README.md"]));
        assert!(test_git(
            &repo_root,
            &[
                "-c",
                "user.email=paneflow@example.com",
                "-c",
                "user.name=Paneflow",
                "commit",
                "-m",
                "init",
            ],
        ));

        let branch_a = "feat/a b";
        let branch_b = "feat/a-b";
        let legacy = worktree::worktree_dir(&repo_root, branch_a);
        std::fs::create_dir_all(legacy.parent().expect("worktree parent")).expect("parent dir");
        if !test_git(
            &repo_root,
            &[
                "worktree",
                "add",
                legacy.to_str().expect("utf8 path"),
                "-b",
                branch_a,
            ],
        ) {
            return;
        }

        let (path, create_branch) = launch_pad_worktree_plan(&repo_root, branch_b).expect("plan");
        assert_eq!(path, worktree::worktree_dir_hashed(&repo_root, branch_b));
        assert!(create_branch);
    }

    fn test_git(cwd: &std::path::Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }
}
