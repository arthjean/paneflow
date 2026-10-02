use super::*;

fn classify_ghostty_start_error(error: GhosttyStartError) -> TerminalBackendFailureDiagnostics {
    let GhosttyStartError::Initialization(error) = error;
    TerminalBackendFailureDiagnostics::new(
        TerminalBackendFailurePhase::Initialization,
        TerminalBackendFailureDiagnostics::GHOSTTY_INITIALIZATION_FAILED,
        raw_os_error_from_anyhow(&error),
    )
}

static BACKEND_START_FAILED_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn claim_backend_failure_report(reported: &std::sync::atomic::AtomicBool) -> bool {
    !reported.swap(true, std::sync::atomic::Ordering::Relaxed)
}

fn backend_failure_level(reported: &std::sync::atomic::AtomicBool) -> log::Level {
    if claim_backend_failure_report(reported) {
        log::Level::Error
    } else {
        log::Level::Debug
    }
}

fn log_backend_diagnostics(terminal: &TerminalState) {
    let diagnostics = terminal.backend_diagnostics();
    log::info!(
        target: "paneflow::terminal::backend",
        "Terminal backend selected: {diagnostics}"
    );
}

fn spawn_error_message(failure: &TerminalBackendFailureDiagnostics) -> String {
    format!(
        "\x1b[1;31mError\x1b[0m: failed to start the terminal.\r\n\
         \r\n\
         Common causes:\r\n\
         \x20 \x20- PTY pool exhausted\r\n\
         \x20 \x20- Shell binary not found ($SHELL / default_shell)\r\n\
         \x20 \x20- Permission denied on /dev/ptmx\r\n\
         \r\n\
         \x1b[2mfailure_phase={} reason_code={} os_error={:?}\x1b[0m\r\n",
        failure.phase.as_str(),
        failure.reason_code,
        failure.os_error,
    )
}

enum AttachOutcome {
    Attached(Box<HostedAttachment>),
    Ended(HostLinkEnd, Option<FinalText>),
    Unavailable(String),
    MirrorFailed(GhosttyStartError),
}

impl TerminalView {
    pub(super) fn begin_hosted_attach(
        &self,
        intent: SessionIntent,
        params: crate::terminal::pty_session::SpawnParams,
        pending: crate::terminal::pty_session::PendingTerminalBackend,
        cx: &mut Context<Self>,
    ) {
        let ghostty = self.terminal.ghostty_session();
        let ghostty_pending = pending.ghostty;
        let profile = params.profile;
        let request = AttachRequest {
            intent,
            session: self.terminal.session_id.clone(),
            workspace: crate::workspace::durable_workspace_id(self.launch.workspace_id),
            params,
            appearance: crate::terminal::ghostty_session::current_host_appearance(),
            cell: self
                .recorded_window_size()
                .filter(|size| size.cell_width > 0 && size.cell_height > 0)
                .map(|size| paneflow_host::CellSize {
                    width: size.cell_width,
                    height: size.cell_height,
                }),
        };
        let spawn_rule = crate::workspace::SpawnCwd {
            cwd: None,
            confine_to: self.launch.confine_to.clone(),
            fallback: self.launch.fallback_to.clone(),
        };
        let max_scrollback = crate::config_snapshot::current(cx)
            .terminal
            .clone()
            .unwrap_or_default()
            .resolved_scrollback_lines_for_profile(profile);
        let executor = cx.background_executor().clone();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let outcome = executor
                    .spawn(async move {
                        let mut request = request;
                        if spawn_rule.needs_resolving() {
                            let requested = request.params.cwd.clone();
                            request.params.cwd = crate::workspace::SpawnCwd {
                                cwd: Some(requested.clone()),
                                ..spawn_rule
                            }
                            .resolve(crate::workspace::CONFINE_TIMEOUT)
                            .unwrap_or(requested);
                        }
                        match host_link::resolve(request) {
                            Ok(ResolveOutcome::Attached(hosted, snapshot)) => {
                                match ghostty.start_attached(
                                    ghostty_pending,
                                    hosted.attachment.clone(),
                                    snapshot,
                                    max_scrollback,
                                ) {
                                    Ok(()) => AttachOutcome::Attached(hosted),
                                    Err(error) => AttachOutcome::MirrorFailed(error),
                                }
                            }
                            Ok(ResolveOutcome::Ended(end, final_text)) => {
                                let _ = ghostty.start_display(ghostty_pending, max_scrollback);
                                AttachOutcome::Ended(end, final_text)
                            }
                            Err(error) => {
                                let _ = ghostty.start_display(ghostty_pending, max_scrollback);
                                log::log!(
                                    target: "paneflow::terminal::backend",
                                    backend_failure_level(&BACKEND_START_FAILED_LOGGED),
                                    "hosted session resolution failed: {error}"
                                );
                                AttachOutcome::Unavailable(error.user_message())
                            }
                        }
                    })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    cx.emit(TerminalEvent::HostLinkResolved);
                    match outcome {
                        AttachOutcome::Attached(hosted) => {
                            view.saved_scrollback = None;
                            view.needs_initial_clear
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                            view.terminal.promote_hosted(*hosted);
                            if let Some(size) = view.recorded_window_size() {
                                view.terminal.notify_window_size(size);
                            }
                            view.conversation_host_attached(cx);
                        }
                        AttachOutcome::Ended(end, final_text) => {
                            match final_text {
                                Some(final_text)
                                    if final_text.available && !final_text.text.is_empty() =>
                                {
                                    view.saved_scrollback = None;
                                    view.terminal.write_output(final_text.text.as_bytes());
                                }
                                _ => view.restore_saved_scrollback(),
                            }
                            view.needs_initial_clear
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                            let kind = end.kind.clone();
                            view.terminal.mark_host_link(HostLinkState::Ended(end));
                            view.conversation_host_ended(&kind, cx);
                        }
                        AttachOutcome::Unavailable(message) => {
                            view.restore_saved_scrollback();
                            view.needs_initial_clear
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                            view.terminal
                                .mark_host_link(HostLinkState::Unavailable(message));
                        }
                        AttachOutcome::MirrorFailed(error) => {
                            let failure = classify_ghostty_start_error(error);
                            log::log!(
                                target: "paneflow::terminal::backend",
                                backend_failure_level(&BACKEND_START_FAILED_LOGGED),
                                "hosted terminal mirror failed: failure_phase={} reason_code={} os_error={:?}",
                                failure.phase.as_str(),
                                failure.reason_code,
                                failure.os_error,
                            );
                            view.needs_initial_clear
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                            let message = spawn_error_message(&failure);
                            view.terminal.report_spawn_failure(failure, &message);
                            view.saved_scrollback = None;
                            if view.terminal.has_backend_events() {
                                view.pump_epoch = view.pump_epoch.wrapping_add(1);
                                let epoch = view.pump_epoch;
                                spawn_event_pump_task(
                                    view.terminal.take_backend_events(),
                                    epoch,
                                    cx,
                                );
                            }
                            view.terminal.mark_host_link(HostLinkState::Unavailable(
                                "The terminal state could not be restored from the local host."
                                    .to_string(),
                            ));
                        }
                    }
                    log_backend_diagnostics(&view.terminal);
                    cx.notify();
                });
            },
        )
        .detach();
    }

    pub(crate) fn resume_hosted_session(&mut self, cx: &mut Context<Self>) {
        let observed = self
            .terminal
            .hosted
            .as_ref()
            .map(|hosted| hosted.generation);
        let (intent, fresh_identity) = match &self.terminal.host_link {
            HostLinkState::Ended(end) => {
                let intent = end.intent(observed);
                (intent, intent == SessionIntent::Create)
            }
            HostLinkState::Unavailable(_) => (SessionIntent::Reattach, false),
            _ => return,
        };
        self.start_hosted_session(intent, fresh_identity, cx);
    }

    pub(crate) fn launched_under(&self, root: &std::path::Path) -> bool {
        let reported = self
            .terminal
            .current_cwd
            .as_deref()
            .is_some_and(|cwd| std::path::Path::new(cwd).starts_with(root));
        reported
            || self
                .launch
                .confine_to
                .as_deref()
                .or(self.launch.cwd.as_deref())
                .is_some_and(|cwd| cwd.starts_with(root))
    }

    pub(crate) fn relaunch_in(&mut self, cwd: std::path::PathBuf, cx: &mut Context<Self>) {
        self.launch.cwd = Some(cwd);
        self.launch.confine_to = None;
        self.launch.fallback_to = None;
        self.relaunch_pending = true;
        self.relaunch_when_ended(cx);
    }

    pub(super) fn relaunch_when_ended(&mut self, cx: &mut Context<Self>) {
        if self.relaunch_pending && matches!(self.terminal.host_link, HostLinkState::Ended(_)) {
            self.relaunch_pending = false;
            self.start_hosted_session(SessionIntent::Create, true, cx);
        }
    }

    pub(super) fn start_hosted_session(
        &mut self,
        intent: SessionIntent,
        fresh_identity: bool,
        cx: &mut Context<Self>,
    ) {
        self.session_intent = intent;
        let surface_id = cx.entity_id().as_u64();
        let (params, shell_notice) = self
            .launch
            .spawn_params(surface_id, &crate::config_snapshot::current(cx));
        let (mut fresh, pending) = TerminalState::new_pending_with_shell_quoting(
            params.cols,
            params.rows,
            params.shell_quoting,
        );
        fresh.pending_host_notices.extend(shell_notice);
        fresh.osc52_policy = self.terminal.osc52_policy;
        fresh.session_id = if fresh_identity {
            paneflow_config::schema::SessionId::new()
        } else {
            self.terminal.session_id.clone()
        };
        fresh.custom_name = self.terminal.custom_name.take();
        fresh.font_size_override = self.terminal.font_size_override;
        fresh.detected_agent = self.terminal.detected_agent;
        let previous = std::mem::replace(&mut self.terminal, fresh);
        drop(previous);
        self.needs_initial_clear
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.pump_epoch = self.pump_epoch.wrapping_add(1);
        let epoch = self.pump_epoch;
        spawn_event_pump_task(self.terminal.take_backend_events(), epoch, cx);
        self.begin_hosted_attach(intent, params, pending, cx);
        cx.notify();
    }

    pub(crate) fn stop_incompatible_host_and_restart(&mut self, cx: &mut Context<Self>) {
        if !matches!(
            &self.terminal.host_link,
            HostLinkState::Ended(end) if end.kind == crate::terminal::host_link::HostLinkEndKind::Incompatible
        ) {
            return;
        }
        self.terminal.mark_host_link(HostLinkState::Unavailable(
            "Stopping the incompatible local host".to_string(),
        ));
        cx.notify();
        let executor = cx.background_executor().clone();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let stopped = executor
                    .spawn(async move { host_link::stop_incompatible_host() })
                    .await;
                let _ = this.update(cx, |view, cx| match stopped {
                    Ok(()) => {
                        view.session_intent = SessionIntent::Reattach;
                        view.resume_hosted_session(cx);
                    }
                    Err(error) => {
                        view.terminal
                            .mark_host_link(HostLinkState::Unavailable(format!(
                                "The incompatible local host could not be stopped: {error}"
                            )));
                        cx.notify();
                    }
                });
            },
        )
        .detach();
    }

    pub(super) fn render_host_link_bar(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let (text, action, secondary) = match &self.terminal.host_link {
            HostLinkState::Attaching | HostLinkState::Attached => return None,
            HostLinkState::Reconnecting => {
                ("Reconnecting to the local host".to_string(), None, None)
            }
            HostLinkState::Ended(end) => (
                end.detail.clone(),
                end.action_label(),
                end.secondary_action_label(),
            ),
            HostLinkState::Unavailable(message) => (message.clone(), Some("Retry"), None),
        };

        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(gpui::px(10.0))
            .px(gpui::px(12.0))
            .py(gpui::px(7.0))
            .child(
                div()
                    .min_w_0()
                    .text_xs()
                    .text_color(ui.muted)
                    .truncate()
                    .child(text),
            );
        if let Some(label) = action {
            bar = bar.child(crate::settings::components::secondary_button(
                "host-link-action",
                label,
                ui,
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    this.resume_hosted_session(cx);
                }),
            ));
        }
        if let Some(label) = secondary {
            bar = bar.child(crate::settings::components::secondary_button(
                "host-link-secondary-action",
                label,
                ui,
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    this.stop_incompatible_host_and_restart(cx);
                }),
            ));
        }

        Some(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .pb(gpui::px(10.0))
                .child(
                    crate::ui_primitives::squircle_skin(
                        div().id("host-link-bar").max_w(gpui::px(420.0)),
                        "host-link-bar",
                        crate::ui_primitives::ROW_RADIUS,
                        Some(ui.overlay),
                        None,
                    )
                    .child(bar),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ghostty_start_error_reports_the_initialization_phase_and_os_error() {
        let os_error = anyhow::Error::new(std::io::Error::from_raw_os_error(5));
        let failure = classify_ghostty_start_error(GhosttyStartError::Initialization(os_error));
        assert_eq!(failure.phase, TerminalBackendFailurePhase::Initialization);
        assert_eq!(
            failure.reason_code,
            TerminalBackendFailureDiagnostics::GHOSTTY_INITIALIZATION_FAILED
        );
        assert_eq!(failure.os_error, Some(5));
    }

    #[test]
    fn backend_start_failure_reports_once_per_process() {
        let reported = std::sync::atomic::AtomicBool::new(false);
        assert!(claim_backend_failure_report(&reported));
        assert!(!claim_backend_failure_report(&reported));

        let fresh = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(backend_failure_level(&fresh), log::Level::Error);
        assert_eq!(backend_failure_level(&fresh), log::Level::Debug);
        assert_eq!(backend_failure_level(&fresh), log::Level::Debug);
    }
}
