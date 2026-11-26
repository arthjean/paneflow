use std::time::Instant;

use super::*;
use crate::terminal::program_resume::{
    RESUME_OUTCOME_DEADLINE, RESUME_OUTCOME_POLL, ResumePolicy, ResumeState, launcher_input,
    restore_state,
};

pub(crate) const RESUME_BUTTON: &str = "program-resume-accept";
pub(crate) const RESUME_DISMISS_BUTTON: &str = "program-resume-dismiss";
pub(crate) const RESUME_BUTTON_LABEL: &str = "Resume";
pub(crate) const RESUME_DISMISS_LABEL: &str = "Dismiss";

impl TerminalView {
    #[cfg(test)]
    pub(crate) fn program_resume(&self) -> &ResumeState {
        &self.program_resume
    }

    pub(super) fn plan_program_resume(
        &mut self,
        recover: bool,
        end: &host_link::HostLinkEnd,
        cx: &mut Context<Self>,
    ) {
        if !recover {
            self.program_resume = std::mem::take(&mut self.program_resume).after_attempt(None);
            return;
        }
        let policy = ResumePolicy::from_config(&crate::config_snapshot::current(cx));
        self.apply_restore_state(restore_state(&policy, end));
    }

    pub(crate) fn apply_restore_state(&mut self, state: ResumeState) {
        if state.yields_conversation() {
            self.conversation_yield_to_program_resume();
        }
        self.program_resume = state;
    }

    pub(super) fn resume_request_for(
        &self,
        intent: SessionIntent,
    ) -> Option<paneflow_host::host::ResumeRequest> {
        match intent {
            SessionIntent::Restart { expected: Some(_) } => self.program_resume.restart_request(),
            SessionIntent::Create
            | SessionIntent::Reattach
            | SessionIntent::Restart { expected: None } => None,
        }
    }

    pub(super) fn program_resume_attached(
        &mut self,
        report: Option<&paneflow_host::host::ResumeAttemptReport>,
        cx: &mut Context<Self>,
    ) {
        self.program_resume = std::mem::take(&mut self.program_resume).after_attempt(report);
        if self.program_resume.awaited_attempt().is_some() {
            self.watch_resume_attempt(cx);
        }
    }

    pub(crate) fn note_resume_attempt(
        &mut self,
        report: &paneflow_host::host::ResumeAttemptReport,
    ) -> bool {
        if !self.program_resume.settled_outcome(report) {
            return false;
        }
        self.program_resume = std::mem::take(&mut self.program_resume).after_attempt(Some(report));
        true
    }

    fn watch_resume_attempt(&mut self, cx: &mut Context<Self>) {
        let Some(attempt) = self.program_resume.awaited_attempt().cloned() else {
            return;
        };
        let session = self.terminal.session_id.clone();
        let executor = cx.background_executor().clone();
        let deadline = Instant::now() + RESUME_OUTCOME_DEADLINE;
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    executor.timer(RESUME_OUTCOME_POLL).await;
                    let inspected = session.clone();
                    let report = executor
                        .spawn(async move { host_link::resume_attempt_of(&inspected) })
                        .await;
                    let expired = Instant::now() >= deadline;
                    let done = this
                        .update(cx, |view, cx| {
                            if view.program_resume.awaited_attempt() != Some(&attempt) {
                                return true;
                            }
                            if let Ok(Some(report)) = &report
                                && view.note_resume_attempt(report)
                            {
                                cx.notify();
                                return true;
                            }
                            if expired {
                                view.program_resume = ResumeState::None;
                            }
                            expired
                        })
                        .unwrap_or(true);
                    if done {
                        break;
                    }
                }
            },
        )
        .detach();
    }

    pub(crate) fn accept_program_resume(&mut self, cx: &mut Context<Self>) {
        let ResumeState::Offered { command, .. } = &self.program_resume else {
            return;
        };
        let command = command.clone();
        let record = command.record;
        self.program_resume = ResumeState::Resuming {
            command: command.clone(),
            attempt: None,
        };
        cx.notify();
        let session = self.terminal.session_id.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let prepared = executor
                    .spawn(async move { host_link::prepare_resume(&session, record) })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    match prepared {
                        Ok(prepared) => {
                            let reset_line = view.terminal.input_sent();
                            view.terminal.write_to_pty(launcher_input(
                                view.terminal.shell_quoting,
                                &prepared.launcher,
                                &prepared.handoff,
                                reset_line,
                            ));
                            view.program_resume = ResumeState::Resuming {
                                command,
                                attempt: Some(prepared.attempt),
                            };
                            view.watch_resume_attempt(cx);
                        }
                        Err(reason) => {
                            view.program_resume = ResumeState::Failed {
                                argv: command.argv,
                                reason,
                            };
                        }
                    }
                    cx.notify();
                });
            },
        )
        .detach();
    }

    pub(crate) fn dismiss_program_resume(&mut self, cx: &mut Context<Self>) {
        let record = self.program_resume.dismissed_record();
        self.program_resume = ResumeState::None;
        cx.notify();
        let Some(record) = record else {
            return;
        };
        let session = self.terminal.session_id.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = host_link::discard_resume(&session, record) {
                    log::warn!(
                        target: "paneflow::terminal::program_resume",
                        "the host kept a dismissed resume record: {error}"
                    );
                }
            })
            .detach();
    }

    pub(super) fn render_program_resume_banner(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let message = self.program_resume.banner_message()?;
        let mut buttons = Vec::with_capacity(2);
        if self.program_resume.offers_resume() {
            buttons.push(conversation::banner_button(
                RESUME_BUTTON,
                RESUME_BUTTON_LABEL,
                ui,
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    this.accept_program_resume(cx);
                }),
            ));
        }
        buttons.push(conversation::banner_button(
            RESUME_DISMISS_BUTTON,
            RESUME_DISMISS_LABEL,
            ui,
            cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                this.dismiss_program_resume(cx);
            }),
        ));
        Some(conversation::banner_frame(
            conversation::banner_bar(message, ui, buttons),
            ui,
        ))
    }
}

#[cfg(test)]
mod tests {
    use paneflow_config::schema::{AgentSessionRef, SessionGeneration};
    use paneflow_host::host::{ResumeCandidate, ResumeRefusal, SessionReconnection};
    use paneflow_program_resume::{CwdSource, RecordId};

    use super::*;
    use crate::terminal::program_resume::{OfferReason, ResumeCommand};
    use crate::terminal::view::conversation::BANNER_TEXT;

    const CLAUDE: &str = "com.anthropic.claude-code";
    const CLAUDE_ID: &str = "3922faec-860a-47b1-8f2d-e6b9488c467c";

    fn candidate(argv: &[&str]) -> ResumeCandidate {
        ResumeCandidate {
            record: RecordId::from(3),
            argv: argv.iter().map(|item| (*item).to_string()).collect(),
            cwd: "/home/user/project".to_string(),
            cwd_source: CwdSource::Report,
            owner_pid: 4242,
            accepted_at_ms: 1,
            settled: true,
            blocked: None,
        }
    }

    fn lost(candidate: ResumeCandidate) -> host_link::HostLinkEnd {
        host_link::HostLinkEnd::from_reconnection(
            SessionReconnection::Lost,
            SessionGeneration::FIRST,
        )
        .with_resume_candidate(Some(candidate))
    }

    fn command() -> ResumeCommand {
        ResumeCommand {
            record: RecordId::from(3),
            argv: vec!["htop".to_string()],
            cwd: "/home/user/project".to_string(),
        }
    }

    #[gpui::test]
    fn a_native_record_takes_the_pane_from_its_catalog_conversation(cx: &mut gpui::TestAppContext) {
        let view = cx.update(|cx| cx.new(|cx| TerminalView::display_only_for_test(1, cx)));
        view.update(cx, |view, cx| {
            view.restore_agent_session(
                Some(AgentSessionRef {
                    runtime: CLAUDE.to_string(),
                    id: CLAUDE_ID.to_string(),
                    cwd: None,
                }),
                true,
            );
            assert!(view.conversation_restore_pending());
            view.plan_program_resume(true, &lost(candidate(&["pi", "--session", "abc_1"])), cx);
            assert!(
                !view.conversation_restore_pending(),
                "the catalog conversation is not resumed beside the record"
            );
            assert_eq!(view.conversation_banner(), None);
            assert_eq!(view.program_resume().banner_message(), None);
            let request = view
                .resume_request_for(SessionIntent::Restart {
                    expected: Some(SessionGeneration::FIRST),
                })
                .expect("the restart resumes the record");
            assert_eq!(request.record, Some(RecordId::from(3)));
            assert_eq!(view.resume_request_for(SessionIntent::Reattach), None);
        });
    }

    #[gpui::test]
    fn a_pane_not_armed_for_recovery_keeps_its_conversation_and_resumes_nothing(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = cx.update(|cx| cx.new(|cx| TerminalView::display_only_for_test(1, cx)));
        view.update(cx, |view, cx| {
            view.plan_program_resume(false, &lost(candidate(&["htop"])), cx);
            assert_eq!(view.program_resume(), &ResumeState::None);
            assert_eq!(
                view.resume_request_for(SessionIntent::Restart {
                    expected: Some(SessionGeneration::FIRST)
                }),
                None
            );
        });
    }

    #[gpui::test]
    fn an_offer_paints_resume_and_dismiss_and_dismiss_clears_it(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, px, size};

        let (view, cx) = cx.add_window_view(|_, cx| TerminalView::display_only_for_test(1, cx));
        view.update(cx, |view, cx| {
            view.plan_program_resume(true, &lost(candidate(&["htop"])), cx);
            assert!(matches!(
                view.program_resume(),
                ResumeState::Offered {
                    reason: OfferReason::Untrusted,
                    ..
                }
            ));
        });
        cx.simulate_resize(size(px(480.), px(400.)));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds(BANNER_TEXT).is_some());
        assert!(cx.debug_bounds(RESUME_BUTTON).is_some());
        let dismiss = cx
            .debug_bounds(RESUME_DISMISS_BUTTON)
            .expect("Dismiss is painted");
        cx.simulate_click(dismiss.center(), Modifiers::default());
        cx.run_until_parked();
        view.update(cx, |view, _cx| {
            assert_eq!(view.program_resume(), &ResumeState::None);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds(RESUME_BUTTON).is_none());
    }

    #[gpui::test]
    fn a_blocked_pane_paints_only_dismiss(cx: &mut gpui::TestAppContext) {
        use gpui::{px, size};

        let (view, cx) = cx.add_window_view(|_, cx| TerminalView::display_only_for_test(1, cx));
        view.update(cx, |view, _cx| {
            view.apply_restore_state(ResumeState::Blocked {
                command: command(),
                reason: ResumeRefusal::CwdMissing,
            });
        });
        cx.simulate_resize(size(px(480.), px(400.)));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds(BANNER_TEXT).is_some());
        assert!(cx.debug_bounds(RESUME_BUTTON).is_none());
        assert!(cx.debug_bounds(RESUME_DISMISS_BUTTON).is_some());
    }

    #[gpui::test]
    fn a_failed_resume_paints_its_reason_until_dismissed(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, px, size};

        let (view, cx) = cx.add_window_view(|_, cx| TerminalView::display_only_for_test(1, cx));
        view.update(cx, |view, _cx| {
            view.apply_restore_state(ResumeState::Failed {
                argv: vec!["htop".to_string()],
                reason: "not_found".to_string(),
            });
        });
        cx.simulate_resize(size(px(480.), px(400.)));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds(RESUME_BUTTON).is_none());
        let dismiss = cx
            .debug_bounds(RESUME_DISMISS_BUTTON)
            .expect("Dismiss is painted");
        cx.simulate_click(dismiss.center(), Modifiers::default());
        cx.run_until_parked();
        view.update(cx, |view, _cx| {
            assert_eq!(view.program_resume(), &ResumeState::None);
        });
    }
}
