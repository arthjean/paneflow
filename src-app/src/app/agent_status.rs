//! Hook-free agent-state sources, and the one place they are applied.
//!
//! `agent_sessions` used to have a single writer: the `ai.*` lifecycle frames
//! in [`ipc_handler`](super::ipc_handler). That works right up until the hooks
//! do not run - an organization can disable them wholesale from Claude Code's
//! managed settings (`disableAllHooks`, which takes `statusLine` down with it,
//! or `allowManagedHooksOnly`, which drops Paneflow's entries without a word).
//! The sidebar then knows an agent is running and nothing else.
//!
//! Two sources fill that in, and both were already reaching Paneflow with
//! nobody listening:
//!
//! - **The pane itself.** Paneflow presents as Ghostty
//!   (`TERM_PROGRAM=ghostty`), so Claude Code already writes OSC 9;4 progress
//!   and OSC 777 notifications into the grid, and libghostty already decodes
//!   both. They were feeding a tab chip and a desktop notification and stopping
//!   there.
//! - **Claude Code's own session registry**
//!   ([`crate::claude_session_registry`]), which reports the full turn state
//!   including *why* a session is blocked.
//!
//! Three writers on one map need an order, which is what
//! [`AgentStateSource`](crate::ai_types::AgentStateSource) is for; the rule
//! itself lives at the write choke point in `ipc_handler`. This module is the
//! entry point the non-hook sources go through, so they inherit the same
//! surface binding, the same attention sync and the same auto-clear as a hook
//! frame, rather than each growing its own copy.

use std::collections::HashMap;

use gpui::Context;

use crate::agent_launcher::TerminalAgent;
use crate::ai_types::{self, AgentLifecycleEvent, AgentStateSource};
use crate::claude_session_registry::{self, ClaudeSessionRecord, ClaudeSessionStatus};

use crate::PaneFlowApp;
use crate::app::ipc_handler::upsert_session_state;

/// How long a `Finished` session survives before it is swept.
///
/// Same 5 s the `ai.stop` handler uses: the sidebar shows a turn ended, then
/// the row goes away unless something puts the session back to work.
const FINISHED_LINGER: std::time::Duration = std::time::Duration::from_secs(5);

/// How often the Claude Code session registry is re-read while at least one
/// pane is running Claude Code.
///
/// The registry is written on transition, so this interval is pure detection
/// latency and nothing else. 400 ms keeps the sidebar feeling immediate while
/// the work per tick stays a `read_dir` plus a handful of sub-kilobyte reads
/// on a background thread - an order of magnitude below the process scan that
/// already runs on this app. The loop does no I/O at all when no pane is
/// running Claude Code, which is what keeps an idle Paneflow idle.
pub(crate) const REGISTRY_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(400);

/// What a registry record said last time, so an unchanged record costs nothing
/// beyond the read that produced it.
pub(crate) type RegistryWatermark = HashMap<u32, (ClaudeSessionStatus, Option<String>)>;

/// A record that still needs its pane resolved.
struct PendingRecord {
    record: ClaudeSessionRecord,
    surface_id: Option<u64>,
}

/// What became of an observation at the write choke point.
///
/// The caller needs to tell "already handled" from "come back later", which a
/// bare `Option` cannot: a refusal has to stay re-appliable so the state lands
/// the moment the stronger source falls silent, while everything else is
/// settled and must not be retried on every tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Observation {
    /// Written to the session map.
    Written,
    /// Nothing to write, and nothing pending: the observation only says the
    /// agent is idle, and there is no session for it to end.
    Settled,
    /// A stronger source holds this session, or the pane has no identity to
    /// key on yet. Re-apply when that changes.
    Refused,
}

/// Was a finished turn under the user's eyes as it ended?
///
/// `visible` is what [`PaneFlowApp::surfaces_under_user_eye`] returned:
/// `None` when the workspace itself is not on screen, otherwise the surfaces
/// of the tab being looked at. A turn whose surface never resolved has no
/// finer granularity than its workspace, so it settles for that.
///
/// Pure and shared, because the hook path and the hook-free path must never
/// disagree about which dot the sidebar shows.
pub(crate) fn completion_was_seen(
    visible: Option<&std::collections::HashSet<u64>>,
    surface_id: Option<u64>,
) -> bool {
    match surface_id {
        Some(id) => visible.is_some_and(|visible| visible.contains(&id)),
        None => visible.is_some(),
    }
}

impl PaneFlowApp {
    /// The terminal surfaces the user is actually looking at in `workspace_id`,
    /// or `None` when that workspace is not on screen at all.
    ///
    /// Being the active workspace is not enough: a turn can finish in one of
    /// its other tabs, which the user has not seen. The three conditions below
    /// are what "on screen" means for a workspace - no settings overlay, CLI
    /// mode, and a focused window.
    pub(crate) fn surfaces_under_user_eye(
        &self,
        workspace_id: u64,
        cx: &gpui::App,
    ) -> Option<std::collections::HashSet<u64>> {
        if self.settings_section.is_some()
            || !matches!(self.mode, paneflow_config::schema::AppMode::Cli)
            || !crate::agents::notifications::window_active()
        {
            return None;
        }
        self.workspaces
            .get(self.active_idx)
            .filter(|ws| ws.id == workspace_id)
            .map(|ws| ws.active_tab().surface_ids(cx))
    }

    /// Whether the pane a session lives in is under the user's eye, for the
    /// desktop notification gates: the same rule as the completion dot, so a
    /// session whose pane is not resolved yet counts as seen exactly when its
    /// workspace is on screen.
    pub(super) fn session_is_seen(&self, workspace_id: u64, key: u32, cx: &gpui::App) -> bool {
        let surface = self
            .workspaces
            .iter()
            .find(|ws| ws.id == workspace_id)
            .and_then(|ws| ws.agent_sessions.get(&key))
            .and_then(|session| session.surface_id);
        completion_was_seen(
            self.surfaces_under_user_eye(workspace_id, cx).as_ref(),
            surface,
        )
    }

    /// Apply a state observation that did not come from a hook.
    ///
    /// `surface_id` is the pane the observation belongs to and is always
    /// known here - a terminal event carries its own surface, and a registry
    /// record has already been resolved to one. That is the difference from
    /// the hook path, which has to walk a process tree to find out.
    ///
    /// See [`Observation`] for what the return value distinguishes.
    pub(crate) fn apply_observed_agent_state(
        &mut self,
        surface_id: u64,
        tool: TerminalAgent,
        pid: Option<u32>,
        event: AgentLifecycleEvent,
        source: AgentStateSource,
        cx: &mut Context<Self>,
    ) -> Observation {
        let Some(ws_id) = self.workspace_id_for_surface(surface_id, cx) else {
            return Observation::Refused;
        };
        // Key resolution, in order. The surface lookup comes FIRST and is the
        // reason this is not inlined at each call site: a hook keys its
        // session on the PID that ran the hook, a registry record on the PID
        // of the agent, and a terminal event on neither. Keying each on what
        // it happens to know would put three rows in the sidebar for one pane.
        let bound = self
            .workspaces
            .iter()
            .find(|ws| ws.id == ws_id)
            .and_then(|ws| {
                ws.agent_sessions
                    .iter()
                    .find(|(_, session)| session.surface_id == Some(surface_id))
                    .map(|(key, _)| *key)
            });
        if bound.is_none() && !opens_a_session(&event) {
            return Observation::Settled;
        }
        let Some(key_hint) = bound.or(pid).or_else(|| {
            self.surface_child_pid(surface_id, cx)
                .filter(|pid| *pid > 0)
        }) else {
            return Observation::Refused;
        };

        let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == ws_id) else {
            return Observation::Refused;
        };
        let Some(key) = upsert_session_state(
            &mut ws.agent_sessions,
            Some(key_hint),
            tool,
            ai_types::reduce_lifecycle_event(event),
            None,
            source,
        ) else {
            return Observation::Refused;
        };
        cx.notify();
        self.set_session_surface(ws_id, key, surface_id, cx);
        self.sync_attention(cx);
        self.agent_sessions_changed(cx);
        // A turn that ended has to stop being a row, exactly as `ai.stop`
        // arranges. Without this the sidebar keeps a `Finished` session alive
        // for as long as the pane exists.
        if matches!(
            self.workspaces
                .iter()
                .find(|ws| ws.id == ws_id)
                .and_then(|ws| ws.agent_sessions.get(&key))
                .map(|session| &session.state),
            Some(ai_types::AgentState::Finished)
        ) {
            // The sidebar's completion dot is the whole point of this path for
            // a user whose hooks never run: `ai.stop` is what normally raises
            // it, and it is exactly what will not arrive. Raised here on the
            // same terms, keyed on the same surface.
            let visible = self.surfaces_under_user_eye(ws_id, cx);
            let seen = completion_was_seen(visible.as_ref(), Some(surface_id));
            if let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == ws_id) {
                ws.agent_completion_notification
                    .record_finished(seen, Some(surface_id));
            }
            self.schedule_finished_sweep(ws_id, key, cx);
        }
        Observation::Written
    }

    /// Drop a `Finished` session after [`FINISHED_LINGER`], unless something
    /// put it back to work first. Mirrors the timer the `ai.stop` handler
    /// arms, keyed on the exact session so siblings are untouched.
    fn schedule_finished_sweep(&mut self, ws_id: u64, key: u32, cx: &mut Context<Self>) {
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                smol::Timer::after(FINISHED_LINGER).await;
                cx.update(|cx| {
                    let _ = this.update(cx, |app, cx| {
                        if let Some(ws) = app.workspaces.iter_mut().find(|ws| ws.id == ws_id)
                            && matches!(
                                ws.agent_sessions.get(&key).map(|s| &s.state),
                                Some(ai_types::AgentState::Finished)
                            )
                        {
                            ws.agent_sessions.remove(&key);
                            app.sync_attention(cx);
                            app.agent_sessions_changed(cx);
                            cx.notify();
                        }
                    });
                });
            },
        )
        .detach();
    }

    /// The workspace a surface belongs to, across every tab.
    pub(super) fn workspace_id_for_surface(&self, surface_id: u64, cx: &gpui::App) -> Option<u64> {
        self.workspaces
            .iter()
            .find(|ws| {
                ws.collect_panes().iter().any(|pane| {
                    pane.read(cx)
                        .terminals()
                        .any(|terminal| terminal.entity_id().as_u64() == surface_id)
                })
            })
            .map(|ws| ws.id)
    }

    /// The PTY child of a surface - the pane's own process identity, used as
    /// the session key of last resort so a terminal-sourced observation never
    /// has to invent a synthetic one.
    fn surface_child_pid(&self, surface_id: u64, cx: &gpui::App) -> Option<u32> {
        self.workspaces.iter().find_map(|ws| {
            ws.collect_panes().iter().find_map(|pane| {
                pane.read(cx)
                    .terminals()
                    .find(|terminal| terminal.entity_id().as_u64() == surface_id)
                    .map(|terminal| terminal.read(cx).terminal.child_pid)
            })
        })
    }

    /// The `child_pid → surface id` map the ancestor walk resolves a record's
    /// PID against, and whether any pane is running an agent the registry can
    /// describe at all.
    ///
    /// One traversal answers both because the sweep runs on a timer: asking
    /// the two questions separately walked every pane of every tab twice per
    /// tick for no reason. `None` means no such agent is running, which is
    /// what keeps an idle Paneflow from touching the filesystem.
    fn registry_surface_candidates(&self, cx: &gpui::App) -> Option<HashMap<u32, u64>> {
        let mut candidates = HashMap::new();
        let mut backed = false;
        for ws in &self.workspaces {
            for pane in ws.collect_panes() {
                for terminal in pane.read(cx).terminals() {
                    let view = terminal.read(cx);
                    backed |= matches!(
                        view.terminal.detected_agent,
                        Some(TerminalAgent::ClaudeCode)
                    );
                    if view.terminal.child_pid > 0 {
                        candidates.insert(view.terminal.child_pid, terminal.entity_id().as_u64());
                    }
                }
            }
        }
        backed.then_some(candidates)
    }

    /// One registry sweep: read, resolve, apply.
    ///
    /// Both the directory read and the ancestor walk are filesystem work and
    /// run through `smol::unblock`; the main thread only ever sees resolved
    /// records. Records whose PID resolves to no pane are dropped rather than
    /// attributed anywhere - an agent running in a terminal Paneflow does not
    /// own has no row to update.
    pub(crate) fn sweep_claude_session_registry(&mut self, cx: &mut Context<Self>) {
        let Some(candidates) = self.registry_surface_candidates(cx) else {
            // Nothing to describe. Forget the watermark too, so an agent
            // started later is applied from its first observed state instead
            // of being skipped as unchanged.
            self.claude_registry_seen.clear();
            return;
        };
        let Some(dir) = claude_session_registry::sessions_dir() else {
            return;
        };
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let records = smol::unblock(move || {
                    let records = claude_session_registry::read_live_sessions(&dir);
                    // The ancestor walk reads `/proc` (or the platform
                    // equivalent), so it belongs on this thread with the
                    // directory read rather than in a second hop.
                    records
                        .into_iter()
                        .map(|record| {
                            let surface_id = candidates.get(&record.pid).copied().or_else(|| {
                                crate::workspace::pid_resolve::resolve_surface_for_pid(
                                    record.pid,
                                    &candidates,
                                )
                            });
                            PendingRecord { record, surface_id }
                        })
                        .collect::<Vec<_>>()
                })
                .await;
                cx.update(|cx| {
                    let _ = this.update(cx, |app, cx| {
                        app.apply_registry_records(records, cx);
                    });
                });
            },
        )
        .detach();
    }

    fn apply_registry_records(&mut self, records: Vec<PendingRecord>, cx: &mut Context<Self>) {
        let live: std::collections::HashSet<u32> =
            records.iter().map(|pending| pending.record.pid).collect();
        self.claude_registry_seen
            .retain(|pid, _| live.contains(pid));

        for pending in records {
            let Some(surface_id) = pending.surface_id else {
                continue;
            };
            let record = pending.record;
            let observation = (record.status, record.waiting_for.clone());
            // Claude Code writes the file on transition, but Paneflow reads it
            // on a clock: without this the same state would be re-applied
            // every tick and keep resetting the stall clock on a session that
            // has not moved.
            if self.claude_registry_seen.get(&record.pid) == Some(&observation) {
                continue;
            }
            let applied = self.apply_observed_agent_state(
                surface_id,
                TerminalAgent::ClaudeCode,
                Some(record.pid),
                record.lifecycle_event(),
                AgentStateSource::SessionRegistry,
                cx,
            );
            // A refusal (a live hook holds this session) must stay
            // re-appliable, so that the moment the hook falls silent the
            // current registry state lands instead of waiting for Claude
            // Code's next transition. Everything else is settled and must not
            // be re-walked on every tick.
            if applied != Observation::Refused {
                self.claude_registry_seen.insert(record.pid, observation);
            }
        }
    }
}

/// Whether an observation may CREATE a session, as opposed to only updating
/// one that already exists.
///
/// Idle observes an absence: it can end a turn, never open one. Without this,
/// opening Paneflow next to an agent that has been sitting at its prompt for
/// an hour flashes a "finished" row for a turn nobody watched - the registry
/// reports `idle` on the very first sweep, and the terminal channel does the
/// same when a pane's progress bit is read as cleared.
///
/// Every other event is evidence of activity and is allowed to open a row,
/// which is what makes the hook-free path work at all: on a machine where no
/// hook runs, `Working` from the registry IS the first thing anyone says
/// about the session.
pub(crate) fn opens_a_session(event: &AgentLifecycleEvent) -> bool {
    !matches!(event, AgentLifecycleEvent::Idle)
}

/// Map a terminal-sourced observation of a pane to the lifecycle event it
/// implies.
///
/// `busy` is the OSC 9;4 progress bit: Claude Code publishes `indeterminate`
/// while a turn or a tool is in flight and clears it otherwise, which is
/// exactly [`Working`](AgentLifecycleEvent::Working) and
/// [`Idle`](AgentLifecycleEvent::Idle). Pure, so the mapping is testable
/// without a terminal.
///
/// Known limit: OSC 9;4 belongs to the pane, not to the agent. A build the
/// agent started through a shell tool can publish its own progress and clear
/// it while the turn is still running, which reads here as a turn that ended.
/// The cost is bounded - a `Finished` row for at most one poll interval on a
/// registry-backed agent, or until the next signal otherwise - and the fix
/// would be to distrust the channel Paneflow is relying on precisely because
/// it survives a hook policy. Left as a documented edge rather than papered
/// over with heuristics about which program wrote the sequence.
pub(crate) fn progress_lifecycle_event(busy: bool) -> AgentLifecycleEvent {
    if busy {
        AgentLifecycleEvent::Working
    } else {
        AgentLifecycleEvent::Idle
    }
}

/// The lifecycle event an OSC 9 / OSC 777 notification implies, or `None`
/// when the notification says nothing about the agent's state.
///
/// OSC 9 is a general-purpose channel with no type tag, so the text is all
/// there is to classify on, and it is classified the way cmux's fallback
/// does (`CLI/AgentHookNotificationPolicy.swift`): a notification that reads
/// as a request for permission, approval or input is the user's turn, which
/// is what [`WaitingForInput`](crate::ai_types::AgentState::WaitingForInput)
/// means. Claude Code's "Claude needs your permission" and "Claude Code needs
/// your input" both land there, with the text kept as the session's message
/// so the sidebar and the attention queue say what is being asked.
///
/// The one Claude Code text that mentions input without asking for it is the
/// `idle_prompt`, "Claude is waiting for your input": it fires a fixed delay
/// after a turn ENDED with nothing pending, so it is the same fact as
/// `ai.stop`. The hook already drops that notification type
/// (`paneflow-ai-hook::event`); here it maps to
/// [`Idle`](AgentLifecycleEvent::Idle), which cannot open a row and cannot
/// raise the attention queue.
///
/// Everything else is dropped. A bare bell, a `notify-send` from a build, an
/// agent announcing that a task completed: none of it is a question, and
/// promoting it lit the bell for things nobody had to act on.
///
/// The text arrives already sanitized - `ghostty_session::sanitized_notification`
/// strips bidi and zero-width controls and bounds the length at the engine
/// boundary, before either the desktop notification or this ever sees it.
pub(crate) fn notification_lifecycle_event(title: &str, body: &str) -> Option<AgentLifecycleEvent> {
    let message = if body.trim().is_empty() {
        title.trim()
    } else {
        body.trim()
    };
    if message.is_empty() {
        return None;
    }
    if is_turn_ended_notification(message) {
        return Some(AgentLifecycleEvent::Idle);
    }
    reads_as_a_request(message).then(|| AgentLifecycleEvent::Notification {
        message: Some(message.to_owned()),
    })
}

/// Claude Code's `idle_prompt` text, matched loosely so a trailing period or
/// a case change in a future release does not turn it back into a question.
fn is_turn_ended_notification(message: &str) -> bool {
    message
        .trim_end_matches('.')
        .eq_ignore_ascii_case("Claude is waiting for your input")
}

/// Whether a notification's wording asks the user for something. Keyword
/// match on purpose: the channel carries no type, and every CLI phrases its
/// prompts around one of these words.
fn reads_as_a_request(message: &str) -> bool {
    let lower = message.to_lowercase();
    ["permission", "approv", "input", "confirm"]
        .iter()
        .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_is_only_seen_when_its_own_pane_is_the_one_on_screen() {
        let watched = std::collections::HashSet::from([7u64]);

        assert!(completion_was_seen(Some(&watched), Some(7)));
        // The workspace is on screen, but this turn ended in another tab.
        assert!(!completion_was_seen(Some(&watched), Some(8)));
        // The workspace is not on screen at all.
        assert!(!completion_was_seen(None, Some(7)));
    }

    #[test]
    fn an_unresolved_surface_falls_back_to_its_workspace() {
        // No surface means no tab row can claim the completion, so the only
        // question left to ask is whether the workspace was on screen.
        let watched = std::collections::HashSet::from([7u64]);
        assert!(completion_was_seen(Some(&watched), None));
        assert!(completion_was_seen(
            Some(&std::collections::HashSet::new()),
            None
        ));
        assert!(!completion_was_seen(None, None));
    }

    #[test]
    fn only_evidence_of_activity_opens_a_session() {
        // The failure this prevents: a first sweep next to a long-idle agent
        // flashing a "finished" row for a turn nobody watched.
        assert!(!opens_a_session(&AgentLifecycleEvent::Idle));

        // Everything else is what a hook-free machine has to build a session
        // out of, so all of it must be allowed to open one.
        for event in [
            AgentLifecycleEvent::Working,
            AgentLifecycleEvent::PromptSubmit,
            AgentLifecycleEvent::ToolUse { tool_name: None },
            AgentLifecycleEvent::Notification { message: None },
            AgentLifecycleEvent::Stop { summary: None },
            AgentLifecycleEvent::Exit { exit_code: 1 },
        ] {
            assert!(
                opens_a_session(&event),
                "{event:?} must be able to open a row"
            );
        }
    }

    #[test]
    fn progress_maps_to_the_two_states_it_can_prove() {
        assert_eq!(progress_lifecycle_event(true), AgentLifecycleEvent::Working);
        assert_eq!(progress_lifecycle_event(false), AgentLifecycleEvent::Idle);
    }

    #[test]
    fn a_notification_becomes_the_question_the_sidebar_shows() {
        assert_eq!(
            notification_lifecycle_event("Claude Code", "Claude needs your permission"),
            Some(AgentLifecycleEvent::Notification {
                message: Some("Claude needs your permission".into())
            })
        );
        // OSC 9 carries a single string, which libghostty reports as the
        // title with an empty body.
        assert_eq!(
            notification_lifecycle_event("Claude Code needs your input", ""),
            Some(AgentLifecycleEvent::Notification {
                message: Some("Claude Code needs your input".into())
            })
        );
    }

    #[test]
    fn the_idle_prompt_is_a_turn_that_ended_not_a_question() {
        // The failure this prevents: a finished turn showing the bell and the
        // attention queue a minute after the agent went quiet.
        for text in [
            "Claude is waiting for your input",
            "Claude is waiting for your input.",
            "claude is waiting for your input",
        ] {
            assert_eq!(
                notification_lifecycle_event(text, ""),
                Some(AgentLifecycleEvent::Idle),
                "{text:?}"
            );
        }
        assert_eq!(
            notification_lifecycle_event("Claude Code", "Claude is waiting for your input"),
            Some(AgentLifecycleEvent::Idle)
        );
    }

    #[test]
    fn an_empty_notification_is_not_an_agent_asking_for_something() {
        assert_eq!(notification_lifecycle_event("", ""), None);
        assert_eq!(notification_lifecycle_event("   ", "\n\t"), None);
    }

    #[test]
    fn a_notification_that_asks_for_nothing_says_nothing() {
        // The failure this prevents: a build's `notify-send`, or an agent
        // announcing a completed task, lighting the bell.
        for text in [
            "Build finished",
            "Task completed",
            "Codex turn done",
            "ding",
        ] {
            assert_eq!(notification_lifecycle_event(text, ""), None, "{text:?}");
        }
        // Other CLIs phrase their prompts the same way.
        assert!(matches!(
            notification_lifecycle_event("Codex", "Approval required to run a command"),
            Some(AgentLifecycleEvent::Notification { .. })
        ));
    }
}
