use std::time::{Duration, Instant};

use paneflow_config::schema::AgentSessionRef;

use super::*;
use crate::agent_launcher::TerminalAgent;
use crate::agent_resume::ConversationTemplate;
use crate::terminal::host_link::HostLinkEndKind;
use crate::terminal::types::ShellQuoting;

pub(crate) const RESUME_SETTLE_QUIET: Duration = Duration::from_millis(300);
pub(crate) const RESUME_SETTLE_CAP: Duration = Duration::from_secs(5);
pub(crate) const RESUME_FAILURE_WINDOW: Duration = Duration::from_secs(15);
const RESUME_SETTLE_POLL: Duration = Duration::from_millis(50);
const RESUME_FAILURE_POLL: Duration = Duration::from_millis(500);
const SIGNAL_TERMINATED_REASON: &str = "other";
const READLINE_LINE_RESET: &[u8] = b"\x15";
const CONSOLE_LINE_RESET: &[u8] = b"\x1b[F\x1b[1;5H";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentSessionSignal<'a> {
    Started {
        runtime: &'static str,
        id: &'a str,
        cwd: Option<&'a str>,
    },
    Ended {
        reason: Option<&'a str>,
    },
    ReturnedToShell,
}

pub(crate) fn next_agent_session(
    current: Option<AgentSessionRef>,
    signal: AgentSessionSignal<'_>,
) -> Option<AgentSessionRef> {
    match signal {
        AgentSessionSignal::Started { runtime, id, cwd } => Some(AgentSessionRef {
            runtime: runtime.to_string(),
            id: id.to_string(),
            cwd: cwd.map(str::to_string),
        }),
        AgentSessionSignal::Ended {
            reason: Some(SIGNAL_TERMINATED_REASON),
        } => current,
        AgentSessionSignal::Ended { .. } | AgentSessionSignal::ReturnedToShell => None,
    }
}

pub(crate) fn session_start_is_storable(runtime: &str, id: &str) -> bool {
    let accepted = TerminalAgent::from_runtime_id(runtime)
        .is_some_and(|agent| crate::agent_resume::accepts_session_id(agent, id));
    if !accepted {
        tracing::warn!(
            target: "paneflow_app::conversation",
            runtime,
            "ignored a SessionStart id outside the runtime's session_id_pattern"
        );
    }
    accepted
}

pub(crate) fn restorable_agent_session(
    recorded: Option<AgentSessionRef>,
) -> Option<AgentSessionRef> {
    recorded.filter(|recorded| {
        TerminalAgent::from_runtime_id(&recorded.runtime)
            .is_some_and(|agent| crate::agent_resume::accepts_recorded_session(agent, &recorded.id))
    })
}

pub(crate) fn line_reset_for(quoting: ShellQuoting) -> &'static [u8] {
    match quoting {
        ShellQuoting::Posix | ShellQuoting::Wsl => READLINE_LINE_RESET,
        ShellQuoting::PowerShell if !cfg!(windows) => READLINE_LINE_RESET,
        ShellQuoting::PowerShell | ShellQuoting::Cmd => CONSOLE_LINE_RESET,
    }
}

pub(crate) fn resume_input(quoting: ShellQuoting, command: &str) -> Vec<u8> {
    let reset = line_reset_for(quoting);
    let mut bytes = Vec::with_capacity(reset.len() + command.len() + 1);
    bytes.extend_from_slice(reset);
    bytes.extend_from_slice(command.as_bytes());
    bytes.push(b'\r');
    bytes
}

pub(crate) fn host_session_was_lost(kind: &HostLinkEndKind) -> bool {
    match kind {
        HostLinkEndKind::Missing | HostLinkEndKind::Lost | HostLinkEndKind::HostReplaced => true,
        HostLinkEndKind::Exited { code, signal } => *code != 0 || signal.is_some(),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteRequest {
    pub(crate) id: u64,
    pub(crate) source: String,
}

impl WriteRequest {
    pub(crate) fn message(&self) -> String {
        format!("{} wants to write into this pane", self.source)
    }
}

pub(crate) const ALLOW_WRITE_LABEL: &str = "Allow for this agent session";
pub(crate) const DENY_WRITE_LABEL: &str = "Deny";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConversationBanner {
    AlreadyResumed { location: String },
    OfferResume,
    FolderMissing { path: String },
    ResumeFailed { reason: String },
    SharedFolder { agent: &'static str },
}

impl ConversationBanner {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::AlreadyResumed { location } => {
                format!("Conversation already resumed in {location}")
            }
            Self::OfferResume => {
                "Input was typed before the conversation could be resumed".to_string()
            }
            Self::SharedFolder { agent } => {
                format!("Several {agent} panes share this folder, so none resumed on its own")
            }
            Self::FolderMissing { path } => format!("Folder not found: {path}"),
            Self::ResumeFailed { reason } => format!("Resume failed: {reason}"),
        }
    }

    pub(crate) fn resumes_on_accept(&self) -> bool {
        matches!(self, Self::OfferResume | Self::SharedFolder { .. })
    }

    pub(crate) fn primary_label(&self) -> Option<&'static str> {
        match self {
            Self::OfferResume | Self::SharedFolder { .. } => Some("Resume conversation"),
            Self::ResumeFailed { .. } => Some("New session"),
            Self::AlreadyResumed { .. } | Self::FolderMissing { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResumeWrite {
    Skip,
    Offer(ConversationBanner),
    Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RestorePhase {
    AwaitingHost { duplicate_of: Option<String> },
    AwaitingFreshShell,
    AwaitingShell,
    Settling,
    Ready,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SettleClock {
    attached_at: Instant,
    quiet_since: Instant,
    generation: u64,
}

impl SettleClock {
    pub(crate) fn new(now: Instant, generation: u64) -> Self {
        Self {
            attached_at: now,
            quiet_since: now,
            generation,
        }
    }

    pub(crate) fn observe(&mut self, now: Instant, generation: u64, has_geometry: bool) -> bool {
        if generation != self.generation || !has_geometry {
            self.generation = generation;
            self.quiet_since = now;
        }
        let quiet = has_geometry && now.duration_since(self.quiet_since) >= RESUME_SETTLE_QUIET;
        quiet || now.duration_since(self.attached_at) >= RESUME_SETTLE_CAP
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FailureWatch {
    agent: TerminalAgent,
    started_at: Instant,
    baseline: usize,
    markers: &'static [&'static str],
    transition_seen: bool,
}

impl FailureWatch {
    pub(crate) fn new(started_at: Instant, baseline: usize, agent: TerminalAgent) -> Self {
        Self {
            agent,
            started_at,
            baseline,
            markers: crate::agent_resume::runtime_resume_of(agent)
                .map_or(&[], |resume| resume.failure_markers),
            transition_seen: false,
        }
    }

    pub(crate) fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.started_at) > RESUME_FAILURE_WINDOW
    }

    pub(crate) fn marker_in(&self, now: Instant, text: &str) -> Option<&'static str> {
        if self.expired(now) {
            return None;
        }
        let fresh = text.get(self.baseline..).unwrap_or(text);
        self.markers
            .iter()
            .copied()
            .find(|marker| fresh.contains(marker))
    }

    pub(crate) fn exit_failed(&self, now: Instant, code: i32) -> bool {
        code != 0 && !self.transition_seen && !self.expired(now)
    }

    pub(crate) fn note_transition(&mut self) {
        self.transition_seen = true;
    }
}

#[derive(Default)]
pub(crate) struct Conversation {
    recorded: Option<AgentSessionRef>,
    live: bool,
    restore: Option<RestorePhase>,
    banner: Option<ConversationBanner>,
    write_request: Option<WriteRequest>,
    watch: Option<FailureWatch>,
    watch_epoch: u64,
    failed_agent: Option<TerminalAgent>,
    shared_folder: bool,
}

impl TerminalView {
    pub(crate) fn agent_session(&self) -> Option<&AgentSessionRef> {
        self.conversation.recorded.as_ref()
    }

    pub(crate) fn conversation_agent(&self) -> Option<TerminalAgent> {
        self.conversation
            .recorded
            .as_ref()
            .and_then(|recorded| TerminalAgent::from_runtime_id(&recorded.runtime))
    }

    pub(crate) fn conversation_is_live(&self) -> bool {
        self.conversation.live
    }

    pub(crate) fn conversation_banner(&self) -> Option<&ConversationBanner> {
        self.conversation.banner.as_ref()
    }

    pub(crate) fn can_fork_conversation(&self) -> bool {
        self.conversation_agent()
            .is_some_and(crate::agent_resume::can_fork)
    }

    pub(crate) fn restore_agent_session(
        &mut self,
        recorded: Option<AgentSessionRef>,
        restore_conversations: bool,
    ) {
        let recorded = restorable_agent_session(recorded);
        self.conversation.restore = (restore_conversations && recorded.is_some())
            .then_some(RestorePhase::AwaitingHost { duplicate_of: None });
        self.conversation.recorded = recorded;
    }

    pub(crate) fn record_observed_conversation(
        &mut self,
        agent: TerminalAgent,
        cx: &mut Context<Self>,
    ) {
        if !crate::agent_resume::continues_by_observation(agent) {
            return;
        }
        let runtime = agent.runtime().id;
        if self
            .conversation
            .recorded
            .as_ref()
            .is_some_and(|recorded| recorded.runtime == runtime && recorded.continues_latest())
        {
            return;
        }
        self.conversation.live = true;
        let cwd = self.terminal.current_cwd.clone();
        self.set_recorded_agent_session(
            Some(AgentSessionRef {
                runtime: runtime.to_string(),
                id: String::new(),
                cwd,
            }),
            cx,
        );
    }

    pub(crate) fn mark_conversation_shared_folder(&mut self) {
        if self.conversation.restore.is_some() {
            self.conversation.shared_folder = true;
        }
    }

    pub(crate) fn expect_fresh_shell_for_conversation(&mut self) {
        if let Some(RestorePhase::AwaitingHost { duplicate_of: None }) = self.conversation.restore {
            self.conversation.restore = Some(RestorePhase::AwaitingFreshShell);
        }
    }

    pub(crate) fn apply_agent_session_signal(
        &mut self,
        signal: AgentSessionSignal<'_>,
        cx: &mut Context<Self>,
    ) {
        if let AgentSessionSignal::Started { runtime, id, .. } = signal {
            if !session_start_is_storable(runtime, id) {
                return;
            }
            self.conversation.live = true;
            if let Some(watch) = self.conversation.watch.as_mut() {
                watch.note_transition();
            }
        }
        if matches!(
            signal,
            AgentSessionSignal::Ended { .. } | AgentSessionSignal::ReturnedToShell
        ) {
            self.conversation.live = false;
        }
        let next = next_agent_session(self.conversation.recorded.clone(), signal);
        self.set_recorded_agent_session(next, cx);
    }

    fn set_recorded_agent_session(
        &mut self,
        next: Option<AgentSessionRef>,
        cx: &mut Context<Self>,
    ) {
        if next != self.conversation.recorded {
            self.conversation.recorded = next;
            cx.emit(TerminalEvent::AgentSessionChanged);
        }
    }

    pub(crate) fn mark_conversation_duplicate(&mut self, location: String, cx: &mut Context<Self>) {
        if let Some(RestorePhase::AwaitingHost { duplicate_of }) =
            self.conversation.restore.as_mut()
        {
            *duplicate_of = Some(location);
        } else if self.conversation.restore == Some(RestorePhase::AwaitingFreshShell) {
            self.mark_conversation_live_owner(location, cx);
        }
    }

    pub(crate) fn conversation_restore_pending(&self) -> bool {
        self.conversation.restore.is_some()
    }

    pub(crate) fn conversation_restore_ready(&self) -> bool {
        self.conversation.restore == Some(RestorePhase::Ready)
    }

    pub(super) fn conversation_host_attached(&mut self, cx: &mut Context<Self>) {
        match self.conversation.restore {
            Some(RestorePhase::AwaitingHost { .. }) => self.conversation.restore = None,
            Some(RestorePhase::AwaitingFreshShell) => {
                self.check_recorded_cwd(Self::fresh_shell_checked, cx);
            }
            Some(RestorePhase::AwaitingShell) => {
                self.conversation.restore = Some(RestorePhase::Settling);
                self.spawn_resume_settle(cx);
            }
            _ => {}
        }
    }

    pub(super) fn conversation_host_ended(
        &mut self,
        kind: &HostLinkEndKind,
        cx: &mut Context<Self>,
    ) {
        let Some(RestorePhase::AwaitingHost { duplicate_of }) = self.conversation.restore.clone()
        else {
            return;
        };
        if !host_session_was_lost(kind) {
            self.conversation.restore = None;
            return;
        }
        if let Some(location) = duplicate_of {
            self.conversation.restore = None;
            self.conversation.banner = Some(ConversationBanner::AlreadyResumed { location });
            self.set_recorded_agent_session(None, cx);
            self.start_hosted_session(SessionIntent::Create, true, cx);
            return;
        }
        self.check_recorded_cwd(Self::reopen_for_conversation, cx);
    }

    fn check_recorded_cwd(
        &mut self,
        then: fn(&mut Self, Option<std::path::PathBuf>, bool, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) {
        let recorded_cwd = self
            .conversation
            .recorded
            .as_ref()
            .and_then(|recorded| recorded.cwd.clone())
            .map(std::path::PathBuf::from);
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let checked = recorded_cwd.clone();
                let exists = smol::unblock(move || checked.is_none_or(|cwd| cwd.is_dir())).await;
                let _ = this.update(cx, |view, cx| then(view, recorded_cwd, exists, cx));
            },
        )
        .detach();
    }

    fn forget_conversation_of_missing_folder(
        &mut self,
        cwd: &std::path::Path,
        cx: &mut Context<Self>,
    ) {
        self.conversation.restore = None;
        self.conversation.banner = Some(ConversationBanner::FolderMissing {
            path: cwd.display().to_string(),
        });
        self.set_recorded_agent_session(None, cx);
        cx.notify();
    }

    fn fresh_shell_checked(
        &mut self,
        recorded_cwd: Option<std::path::PathBuf>,
        exists: bool,
        cx: &mut Context<Self>,
    ) {
        if self.conversation.restore != Some(RestorePhase::AwaitingFreshShell) {
            return;
        }
        match recorded_cwd {
            Some(cwd) if !exists => self.forget_conversation_of_missing_folder(&cwd, cx),
            _ => {
                self.conversation.restore = Some(RestorePhase::Settling);
                self.spawn_resume_settle(cx);
            }
        }
    }

    fn reopen_for_conversation(
        &mut self,
        recorded_cwd: Option<std::path::PathBuf>,
        exists: bool,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            self.conversation.restore,
            Some(RestorePhase::AwaitingHost { .. })
        ) {
            return;
        }
        match recorded_cwd {
            Some(cwd) if !exists => {
                self.forget_conversation_of_missing_folder(&cwd, cx);
                self.launch.cwd = dirs::home_dir();
            }
            Some(cwd) => {
                self.conversation.restore = Some(RestorePhase::AwaitingShell);
                self.launch.cwd = Some(cwd);
            }
            None => self.conversation.restore = Some(RestorePhase::AwaitingShell),
        }
        self.launch.confine_to = None;
        self.launch.fallback_to = None;
        self.start_hosted_session(SessionIntent::Create, true, cx);
    }

    fn spawn_resume_settle(&mut self, cx: &mut Context<Self>) {
        let mut clock = SettleClock::new(Instant::now(), self.terminal.output_generation);
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(RESUME_SETTLE_POLL).await;
                    let settled = this.update(cx, |view, cx| {
                        if view.conversation.restore != Some(RestorePhase::Settling) {
                            return true;
                        }
                        let ready = clock.observe(
                            Instant::now(),
                            view.terminal.output_generation,
                            view.recorded_window_size().is_some(),
                        );
                        if ready {
                            view.conversation.restore = Some(RestorePhase::Ready);
                            cx.emit(TerminalEvent::ConversationReady);
                        }
                        ready
                    });
                    if !matches!(settled, Ok(false)) {
                        break;
                    }
                }
            },
        )
        .detach();
    }

    pub(crate) fn write_conversation_resume(&mut self, cx: &mut Context<Self>) {
        match self.take_resume_write() {
            ResumeWrite::Skip => {}
            ResumeWrite::Offer(banner) => {
                self.conversation.banner = Some(banner);
                cx.notify();
            }
            ResumeWrite::Type => self.type_conversation_resume(cx),
        }
    }

    fn take_resume_write(&mut self) -> ResumeWrite {
        if self.conversation.restore.take().is_none() {
            return ResumeWrite::Skip;
        }
        if std::mem::take(&mut self.conversation.shared_folder)
            && let Some(agent) = self.conversation_agent()
        {
            return ResumeWrite::Offer(ConversationBanner::SharedFolder {
                agent: agent.display_name(),
            });
        }
        if self.terminal.input_sent() {
            return ResumeWrite::Offer(ConversationBanner::OfferResume);
        }
        ResumeWrite::Type
    }

    fn conversation_resume_command(&self, cx: &gpui::App) -> Option<(TerminalAgent, String)> {
        let agent = self.conversation_agent()?;
        let recorded = self.conversation.recorded.as_ref()?;
        let config = crate::config_snapshot::current(cx);
        let command = crate::agent_resume::conversation_command(
            agent,
            ConversationTemplate::Resume,
            &recorded.id,
            &config,
        )?;
        Some((agent, command))
    }

    fn type_conversation_resume(&mut self, cx: &mut Context<Self>) {
        let Some((agent, command)) = self.conversation_resume_command(cx) else {
            return;
        };
        let epoch = self.next_watch_epoch();
        let backend = self.terminal.session_backend();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let baseline = smol::unblock(move || backend.select_all_text())
                    .await
                    .map_or(0, |text| text.len());
                let _ = this.update(cx, |view, cx| {
                    if view.conversation.watch_epoch != epoch {
                        return;
                    }
                    view.terminal
                        .write_to_pty(resume_input(view.terminal.shell_quoting, &command));
                    view.declare_launched_agent(agent);
                    view.conversation.live = true;
                    tracing::info!(
                        target: "paneflow_app::conversation",
                        runtime = agent.runtime().id,
                        "typed the resume command of a recorded conversation"
                    );
                    view.arm_failure_watch(agent, baseline, epoch, cx);
                });
            },
        )
        .detach();
    }

    pub(crate) fn mark_conversation_live_owner(
        &mut self,
        location: String,
        cx: &mut Context<Self>,
    ) {
        self.conversation.restore = None;
        self.conversation.banner = Some(ConversationBanner::AlreadyResumed { location });
        self.set_recorded_agent_session(None, cx);
        cx.notify();
    }

    pub(crate) fn watch_conversation_start(
        &mut self,
        agent: TerminalAgent,
        cx: &mut Context<Self>,
    ) {
        let epoch = self.next_watch_epoch();
        self.arm_failure_watch(agent, 0, epoch, cx);
    }

    fn next_watch_epoch(&mut self) -> u64 {
        self.conversation.watch_epoch = self.conversation.watch_epoch.wrapping_add(1);
        self.conversation.watch = None;
        self.conversation.watch_epoch
    }

    fn arm_failure_watch(
        &mut self,
        agent: TerminalAgent,
        baseline: usize,
        epoch: u64,
        cx: &mut Context<Self>,
    ) {
        self.conversation.watch = Some(FailureWatch::new(Instant::now(), baseline, agent));
        let backend = self.terminal.session_backend();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(RESUME_FAILURE_POLL).await;
                    let scan_backend = backend.clone();
                    let text = smol::unblock(move || scan_backend.select_all_text())
                        .await
                        .unwrap_or_default();
                    let done = this.update(cx, |view, cx| {
                        if view.conversation.watch_epoch != epoch {
                            return true;
                        }
                        let Some(watch) = view.conversation.watch.as_ref() else {
                            return true;
                        };
                        let now = Instant::now();
                        if let Some(marker) = watch.marker_in(now, &text) {
                            view.fail_conversation_start(marker.to_string(), cx);
                            return true;
                        }
                        if watch.expired(now) {
                            view.conversation.watch = None;
                            return true;
                        }
                        false
                    });
                    if !matches!(done, Ok(false)) {
                        break;
                    }
                }
            },
        )
        .detach();
    }

    pub(crate) fn note_conversation_transition(&mut self) {
        if let Some(watch) = self.conversation.watch.as_mut() {
            watch.note_transition();
        }
    }

    pub(crate) fn note_conversation_exit(&mut self, code: i32, cx: &mut Context<Self>) {
        if self
            .conversation
            .watch
            .as_ref()
            .is_some_and(|watch| watch.exit_failed(Instant::now(), code))
        {
            self.fail_conversation_start(format!("exit code {code}"), cx);
        }
    }

    fn fail_conversation_start(&mut self, reason: String, cx: &mut Context<Self>) {
        self.conversation.failed_agent = self
            .conversation
            .watch
            .take()
            .map(|watch| watch.agent)
            .or_else(|| self.conversation_agent());
        self.conversation.live = false;
        self.conversation.banner = Some(ConversationBanner::ResumeFailed { reason });
        self.set_recorded_agent_session(None, cx);
        cx.notify();
    }

    pub(crate) fn write_request(&self) -> Option<&WriteRequest> {
        self.conversation.write_request.as_ref()
    }

    pub(crate) fn show_write_request(
        &mut self,
        request: Option<WriteRequest>,
        cx: &mut Context<Self>,
    ) {
        if self.conversation.write_request != request {
            self.conversation.write_request = request;
            cx.notify();
        }
    }

    pub(crate) fn decide_write_request(&mut self, allow: bool, cx: &mut Context<Self>) -> bool {
        let Some(request) = self.conversation.write_request.take() else {
            return false;
        };
        crate::app::write_approvals::decide(request.id, allow);
        cx.notify();
        true
    }

    pub(crate) fn accept_conversation_banner(&mut self, cx: &mut Context<Self>) {
        if self.decide_write_request(true, cx) {
            return;
        }
        match self.conversation.banner.take() {
            Some(banner) if banner.resumes_on_accept() => self.type_conversation_resume(cx),
            Some(ConversationBanner::ResumeFailed { .. }) => self.start_new_agent_session(cx),
            other => self.conversation.banner = other,
        }
        cx.notify();
    }

    pub(crate) fn dismiss_conversation_banner(&mut self, cx: &mut Context<Self>) {
        if self.decide_write_request(false, cx) {
            return;
        }
        if self.conversation.banner.take().is_some() {
            cx.notify();
        }
    }

    fn start_new_agent_session(&mut self, cx: &mut Context<Self>) {
        let Some(agent) = self
            .conversation
            .failed_agent
            .take()
            .or(self.terminal.detected_agent)
        else {
            return;
        };
        let config = crate::config_snapshot::current(cx);
        let command = agent.launch_command(&config);
        self.terminal
            .write_to_pty(resume_input(self.terminal.shell_quoting, &command));
        self.declare_launched_agent(agent);
    }

    pub(super) fn render_conversation_banner(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if let Some(request) = self.conversation.write_request.as_ref() {
            let bar = banner_bar(request.message(), ui)
                .child(crate::settings::components::secondary_button(
                    "write-request-allow",
                    ALLOW_WRITE_LABEL,
                    ui,
                    cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                        this.decide_write_request(true, cx);
                    }),
                ))
                .child(crate::settings::components::secondary_button(
                    "write-request-deny",
                    DENY_WRITE_LABEL,
                    ui,
                    cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                        this.decide_write_request(false, cx);
                    }),
                ));
            return Some(banner_frame(bar, ui));
        }
        let banner = self.conversation.banner.as_ref()?;
        let mut bar = banner_bar(banner.message(), ui);
        if let Some(label) = banner.primary_label() {
            bar = bar.child(crate::settings::components::secondary_button(
                "conversation-banner-action",
                label,
                ui,
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    this.accept_conversation_banner(cx);
                }),
            ));
        }
        bar = bar.child(crate::settings::components::secondary_button(
            "conversation-banner-dismiss",
            "Dismiss",
            ui,
            cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                this.dismiss_conversation_banner(cx);
            }),
        ));
        Some(banner_frame(bar, ui))
    }
}

fn banner_bar(message: String, ui: crate::theme::UiColors) -> gpui::Div {
    div()
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
                .child(message),
        )
}

fn banner_frame(bar: gpui::Div, ui: crate::theme::UiColors) -> gpui::AnyElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .pt(gpui::px(10.0))
        .child(
            crate::ui_primitives::squircle_skin(
                div().id("conversation-banner").max_w(gpui::px(480.0)),
                "conversation-banner",
                crate::ui_primitives::ROW_RADIUS,
                Some(ui.overlay),
                None,
            )
            .child(bar),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = "com.anthropic.claude-code";
    const CODEX: &str = "com.openai.codex";
    const CLAUDE_STARTUP_ID: &str = "3922faec-860a-47b1-8f2d-e6b9488c467c";
    const CLAUDE_CLEAR_ID: &str = "8b0c4f6e-2d1a-4f3b-9c7e-5a6d7e8f9a0b";
    const CODEX_THREAD_ID: &str = "01a0f952-1fa0-7e91-a35c-899b9dbfe97e";

    fn started<'a>(runtime: &'static str, id: &'a str) -> AgentSessionSignal<'a> {
        AgentSessionSignal::Started {
            runtime,
            id,
            cwd: Some("C:\\dev\\paneflow"),
        }
    }

    fn recorded(runtime: &str, id: &str) -> Option<AgentSessionRef> {
        Some(AgentSessionRef {
            runtime: runtime.to_string(),
            id: id.to_string(),
            cwd: Some("C:\\dev\\paneflow".to_string()),
        })
    }

    #[test]
    fn a_new_session_start_replaces_the_recorded_conversation_for_claude_and_codex() {
        let first = next_agent_session(None, started(CLAUDE, CLAUDE_STARTUP_ID));
        assert_eq!(first, recorded(CLAUDE, CLAUDE_STARTUP_ID));
        let cleared = next_agent_session(first, started(CLAUDE, CLAUDE_CLEAR_ID));
        assert_eq!(cleared, recorded(CLAUDE, CLAUDE_CLEAR_ID));
        let codex = next_agent_session(None, started(CODEX, CODEX_THREAD_ID));
        assert_eq!(codex, recorded(CODEX, CODEX_THREAD_ID));
    }

    #[test]
    fn a_voluntary_exit_forgets_the_conversation_but_losing_the_host_keeps_it() {
        let current = recorded(CLAUDE, CLAUDE_STARTUP_ID);
        for reason in [Some("prompt_input_exit"), Some("logout"), None] {
            assert_eq!(
                next_agent_session(current.clone(), AgentSessionSignal::Ended { reason }),
                None,
                "{reason:?}"
            );
        }
        assert_eq!(
            next_agent_session(current.clone(), AgentSessionSignal::ReturnedToShell),
            None
        );
        assert_eq!(
            next_agent_session(
                current.clone(),
                AgentSessionSignal::Ended {
                    reason: Some("other")
                }
            ),
            current,
            "a runtime terminated by a signal with its host is not a voluntary exit"
        );
    }

    #[tracing_test::traced_test]
    #[test]
    fn a_session_start_id_outside_the_runtime_pattern_is_refused_with_a_warning() {
        assert!(session_start_is_storable(CLAUDE, CLAUDE_STARTUP_ID));
        assert!(session_start_is_storable(CODEX, CODEX_THREAD_ID));
        assert!(!session_start_is_storable(CLAUDE, "ses_not_a_uuid"));
        assert!(!session_start_is_storable(
            CODEX,
            "--dangerously-skip-permissions"
        ));
        assert!(logs_contain("WARN"));
        assert!(logs_contain(
            "ignored a SessionStart id outside the runtime's session_id_pattern"
        ));
    }

    #[test]
    fn the_resume_write_clears_the_line_for_each_shell_family_then_submits() {
        let command = format!("claude --resume {CLAUDE_STARTUP_ID}");
        let powershell_reset = if cfg!(windows) {
            CONSOLE_LINE_RESET
        } else {
            READLINE_LINE_RESET
        };
        for (quoting, reset) in [
            (ShellQuoting::Posix, READLINE_LINE_RESET),
            (ShellQuoting::Wsl, READLINE_LINE_RESET),
            (ShellQuoting::PowerShell, powershell_reset),
            (ShellQuoting::Cmd, CONSOLE_LINE_RESET),
        ] {
            let bytes = resume_input(quoting, &command);
            assert!(bytes.starts_with(reset), "{quoting:?}");
            assert!(
                !reset.ends_with(b"\x1b"),
                "a trailing lone ESC merges with the command into an Alt chord under ConPTY"
            );
            assert_eq!(
                &bytes[reset.len()..bytes.len() - 1],
                command.as_bytes(),
                "{quoting:?}"
            );
            assert_eq!(bytes.last(), Some(&b'\r'), "{quoting:?}");
        }
    }

    #[test]
    fn the_resume_waits_for_geometry_and_300_ms_of_quiet_but_never_beyond_5_s() {
        let start = Instant::now();
        let mut clock = SettleClock::new(start, 0);
        assert!(!clock.observe(start + Duration::from_millis(400), 0, false));
        assert!(!clock.observe(start + Duration::from_millis(500), 0, true));
        assert!(!clock.observe(start + Duration::from_millis(700), 1, true));
        assert!(!clock.observe(start + Duration::from_millis(900), 1, true));
        assert!(clock.observe(start + Duration::from_millis(1000), 1, true));

        let mut noisy = SettleClock::new(start, 0);
        for step in 1..100u64 {
            let now = start + Duration::from_millis(step * 50);
            let ready = noisy.observe(now, step, true);
            assert_eq!(
                ready,
                now.duration_since(start) >= RESUME_SETTLE_CAP,
                "{step}"
            );
        }
        let mut headless = SettleClock::new(start, 0);
        assert!(headless.observe(start + RESUME_SETTLE_CAP, 0, false));
    }

    fn fixture(slug: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("runtimes")
            .join(slug)
            .join("fixtures")
            .join("resume-failure.txt");
        std::fs::read_to_string(path).expect("resume failure fixture")
    }

    #[test]
    fn captured_resume_failures_raise_the_banner_within_15_s() {
        let start = Instant::now();
        for (agent, slug) in [
            (TerminalAgent::ClaudeCode, "claude-code"),
            (TerminalAgent::Codex, "codex"),
        ] {
            let watch = FailureWatch::new(start, 0, agent);
            let screen = fixture(slug);
            let marker = watch
                .marker_in(start + Duration::from_secs(2), &screen)
                .expect("captured failure is recognized");
            assert!(screen.contains(marker), "{slug}");
            let banner = ConversationBanner::ResumeFailed {
                reason: marker.to_string(),
            };
            assert_eq!(banner.message(), format!("Resume failed: {marker}"));
            assert_eq!(banner.primary_label(), Some("New session"));
        }
    }

    #[test]
    fn a_marker_after_15_s_or_before_the_command_never_raises_the_banner() {
        let start = Instant::now();
        let screen = fixture("claude-code");
        let late = FailureWatch::new(start, 0, TerminalAgent::ClaudeCode);
        assert_eq!(
            late.marker_in(
                start + RESUME_FAILURE_WINDOW + Duration::from_millis(1),
                &screen
            ),
            None
        );
        let history = format!("{screen}$ claude --resume {CLAUDE_STARTUP_ID}\r\n");
        let fresh = FailureWatch::new(start, screen.len(), TerminalAgent::ClaudeCode);
        assert_eq!(fresh.marker_in(start, &history), None);
    }

    #[test]
    fn a_non_zero_exit_fails_only_before_any_state_transition() {
        let start = Instant::now();
        let mut watch = FailureWatch::new(start, 0, TerminalAgent::Codex);
        let soon = start + Duration::from_secs(1);
        assert!(watch.exit_failed(soon, 1));
        assert!(!watch.exit_failed(soon, 0));
        assert!(!watch.exit_failed(start + Duration::from_secs(16), 1));
        watch.note_transition();
        assert!(!watch.exit_failed(soon, 1));
    }

    #[test]
    fn only_a_session_the_host_lost_triggers_a_reopen() {
        assert!(host_session_was_lost(&HostLinkEndKind::Missing));
        assert!(host_session_was_lost(&HostLinkEndKind::Lost));
        assert!(host_session_was_lost(&HostLinkEndKind::HostReplaced));
        assert!(
            !host_session_was_lost(&HostLinkEndKind::Exited {
                code: 0,
                signal: None
            }),
            "a shell the user exited cleanly is not a lost conversation"
        );
        assert!(
            host_session_was_lost(&HostLinkEndKind::Exited {
                code: 0xC000_013Au32 as i32,
                signal: None
            }),
            "host stop --force and a Windows shutdown end the shell with STATUS_CONTROL_C_EXIT"
        );
        assert!(host_session_was_lost(&HostLinkEndKind::Exited {
            code: 0,
            signal: Some("SIGHUP".to_string())
        }));
        assert!(!host_session_was_lost(&HostLinkEndKind::Incompatible));
    }

    fn view(cx: &mut gpui::TestAppContext) -> gpui::Entity<TerminalView> {
        cx.update(|cx| cx.new(|cx| TerminalView::display_only_for_test(1, cx)))
    }

    fn count_changes(
        view: &gpui::Entity<TerminalView>,
        cx: &mut gpui::TestAppContext,
    ) -> (std::rc::Rc<std::cell::Cell<usize>>, gpui::Subscription) {
        let hits = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let sink = hits.clone();
        let subscription = cx.update(|cx| {
            cx.subscribe(view, move |_, event: &TerminalEvent, _| {
                if matches!(event, TerminalEvent::AgentSessionChanged) {
                    sink.set(sink.get() + 1);
                }
            })
        });
        (hits, subscription)
    }

    #[gpui::test]
    fn a_pane_records_its_own_session_start_and_forgets_it_when_the_agent_exits(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = view(cx);
        let (changes, _subscription) = count_changes(&view, cx);
        view.update(cx, |view, cx| {
            assert_eq!(
                view.agent_session(),
                None,
                "a new pane never inherits an id"
            );
            view.apply_agent_session_signal(started(CODEX, CODEX_THREAD_ID), cx);
            assert_eq!(
                view.agent_session().cloned(),
                recorded(CODEX, CODEX_THREAD_ID)
            );
            assert!(view.conversation_is_live());
            view.apply_agent_session_signal(started(CODEX, CODEX_THREAD_ID), cx);
            view.apply_agent_session_signal(started(CLAUDE, "--resume"), cx);
            assert_eq!(
                view.agent_session().cloned(),
                recorded(CODEX, CODEX_THREAD_ID)
            );
            view.apply_agent_session_signal(
                AgentSessionSignal::Ended {
                    reason: Some("other"),
                },
                cx,
            );
            assert_eq!(
                view.agent_session().cloned(),
                recorded(CODEX, CODEX_THREAD_ID)
            );
            view.apply_agent_session_signal(AgentSessionSignal::ReturnedToShell, cx);
            assert_eq!(view.agent_session(), None);
        });
        cx.run_until_parked();
        assert_eq!(
            changes.get(),
            2,
            "one save for the record, one for the exit"
        );
    }

    #[gpui::test]
    fn a_restored_surface_keeps_its_conversation_and_queues_a_resume_only_when_enabled(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = view(cx);
        view.update(cx, |view, _cx| {
            view.restore_agent_session(recorded(CLAUDE, CLAUDE_STARTUP_ID), true);
            assert_eq!(
                view.agent_session().cloned(),
                recorded(CLAUDE, CLAUDE_STARTUP_ID)
            );
            assert!(view.conversation_restore_pending());
            assert!(view.can_fork_conversation());

            view.restore_agent_session(recorded(CLAUDE, CLAUDE_STARTUP_ID), false);
            assert_eq!(
                view.agent_session().cloned(),
                recorded(CLAUDE, CLAUDE_STARTUP_ID)
            );
            assert!(!view.conversation_restore_pending());

            view.restore_agent_session(recorded(CLAUDE, "-rf"), true);
            assert_eq!(view.agent_session(), None);
            assert!(!view.conversation_restore_pending());
            assert!(!view.can_fork_conversation());

            view.restore_agent_session(recorded("ai.opencode.cli", "ses_abc"), true);
            assert!(view.conversation_restore_pending());
            assert!(
                !view.can_fork_conversation(),
                "OpenCode declares no fork_argv"
            );
        });
    }

    #[gpui::test]
    fn input_typed_before_the_resume_write_offers_a_banner_instead_of_typing(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = view(cx);
        view.update(cx, |view, cx| {
            view.restore_agent_session(recorded(CLAUDE, CLAUDE_STARTUP_ID), true);
            view.conversation.restore = Some(RestorePhase::Ready);
            view.terminal.write_to_pty(b"ls".to_vec());
            view.write_conversation_resume(cx);
            assert_eq!(
                view.conversation_banner(),
                Some(&ConversationBanner::OfferResume)
            );
            assert!(!view.conversation_restore_pending());
            assert_eq!(
                view.terminal.queued_raw_input_for_test(),
                vec![b"ls".to_vec()]
            );
            view.dismiss_conversation_banner(cx);
            assert_eq!(view.conversation_banner(), None);
        });
    }

    #[gpui::test]
    fn a_failed_resume_forgets_the_conversation_and_new_session_types_the_launch_command(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = view(cx);
        let (changes, _subscription) = count_changes(&view, cx);
        view.update(cx, |view, cx| {
            view.restore_agent_session(recorded(CLAUDE, CLAUDE_STARTUP_ID), true);
            view.conversation.watch = Some(FailureWatch::new(
                Instant::now(),
                0,
                TerminalAgent::ClaudeCode,
            ));
            view.note_conversation_exit(1, cx);
            assert_eq!(view.agent_session(), None);
            assert_eq!(
                view.conversation_banner(),
                Some(&ConversationBanner::ResumeFailed {
                    reason: "exit code 1".to_string()
                })
            );
            view.accept_conversation_banner(cx);
            assert_eq!(view.conversation_banner(), None);
            let typed = view.terminal.queued_raw_input_for_test();
            let config = crate::config_snapshot::current(cx);
            let launch = TerminalAgent::ClaudeCode.launch_command(&config);
            assert_eq!(
                typed,
                vec![resume_input(view.terminal.shell_quoting, &launch)]
            );
        });
        cx.run_until_parked();
        assert_eq!(changes.get(), 1);
    }

    const FX: &str = "sh.fx.cli";

    #[gpui::test]
    fn an_observed_fx_is_recorded_as_a_conversation_to_continue_until_it_returns_to_the_shell(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = view(cx);
        let (changes, _subscription) = count_changes(&view, cx);
        view.update(cx, |view, cx| {
            view.terminal.current_cwd = Some("/home/u/project".to_string());
            view.record_observed_conversation(TerminalAgent::Fx, cx);
            view.record_observed_conversation(TerminalAgent::Fx, cx);
            assert_eq!(
                view.agent_session().cloned(),
                Some(AgentSessionRef {
                    runtime: FX.to_string(),
                    id: String::new(),
                    cwd: Some("/home/u/project".to_string()),
                })
            );
            assert!(view.conversation_is_live());
            assert!(!view.can_fork_conversation());
            view.record_observed_conversation(TerminalAgent::Amp, cx);
            assert_eq!(
                view.agent_session()
                    .map(|recorded| recorded.runtime.as_str()),
                Some(FX),
                "a runtime without continue_argv records nothing"
            );
            view.apply_agent_session_signal(AgentSessionSignal::ReturnedToShell, cx);
            assert_eq!(view.agent_session(), None);
        });
        cx.run_until_parked();
        assert_eq!(changes.get(), 2);
    }

    #[gpui::test]
    fn a_lone_fx_restored_after_a_host_loss_types_fx_continue(cx: &mut gpui::TestAppContext) {
        let view = view(cx);
        view.update(cx, |view, cx| {
            view.restore_agent_session(recorded(FX, ""), true);
            assert!(view.conversation_restore_pending());
            assert_eq!(
                view.conversation_resume_command(cx),
                Some((TerminalAgent::Fx, "fx --continue".to_string()))
            );
            view.conversation.restore = Some(RestorePhase::Ready);
            assert_eq!(view.take_resume_write(), ResumeWrite::Type);
            assert!(!view.conversation_restore_pending());
        });
    }

    #[gpui::test]
    fn an_fx_sharing_its_folder_offers_the_banner_and_types_only_when_accepted(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = view(cx);
        view.update(cx, |view, cx| {
            view.restore_agent_session(recorded(FX, ""), true);
            view.mark_conversation_shared_folder();
            view.conversation.restore = Some(RestorePhase::Ready);
            view.write_conversation_resume(cx);
            let banner = ConversationBanner::SharedFolder { agent: "fx" };
            assert_eq!(view.conversation_banner(), Some(&banner));
            assert!(!view.conversation_restore_pending());
            assert!(view.terminal.queued_raw_input_for_test().is_empty());
            assert!(banner.resumes_on_accept());
            assert_eq!(banner.primary_label(), Some("Resume conversation"));
            assert_eq!(
                view.conversation_resume_command(cx),
                Some((TerminalAgent::Fx, "fx --continue".to_string()))
            );
        });
    }

    #[gpui::test]
    fn a_duplicate_conversation_is_not_resumed_twice(cx: &mut gpui::TestAppContext) {
        let view = view(cx);
        view.update(cx, |view, cx| {
            view.restore_agent_session(recorded(CLAUDE, CLAUDE_STARTUP_ID), true);
            view.mark_conversation_duplicate("pane 1 of main".to_string(), cx);
            assert_eq!(
                view.conversation.restore,
                Some(RestorePhase::AwaitingHost {
                    duplicate_of: Some("pane 1 of main".to_string())
                })
            );
        });
    }

    #[gpui::test]
    fn a_duplicate_restored_after_a_host_stop_opens_a_shell_without_resuming(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = view(cx);
        view.update(cx, |view, cx| {
            view.restore_agent_session(recorded(CLAUDE, CLAUDE_STARTUP_ID), true);
            view.expect_fresh_shell_for_conversation();
            view.mark_conversation_duplicate("pane 1 of main".to_string(), cx);
            assert!(!view.conversation_restore_pending());
            assert_eq!(view.agent_session(), None);
            assert_eq!(
                view.conversation_banner(),
                Some(&ConversationBanner::AlreadyResumed {
                    location: "pane 1 of main".to_string()
                })
            );
        });
    }

    #[gpui::test]
    fn a_missing_folder_after_a_host_stop_skips_the_resume_and_says_so(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = view(cx);
        let missing = std::path::PathBuf::from("/gone/project");
        view.update(cx, |view, cx| {
            view.restore_agent_session(recorded(CLAUDE, CLAUDE_STARTUP_ID), true);
            view.expect_fresh_shell_for_conversation();
            view.fresh_shell_checked(Some(missing.clone()), false, cx);
            assert!(!view.conversation_restore_pending());
            assert_eq!(view.agent_session(), None);
            assert_eq!(
                view.conversation_banner(),
                Some(&ConversationBanner::FolderMissing {
                    path: missing.display().to_string()
                })
            );
        });
    }

    #[gpui::test]
    fn a_write_request_takes_the_notice_actions_until_the_human_decides(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = view(cx);
        view.update(cx, |view, cx| {
            view.conversation.banner = Some(ConversationBanner::OfferResume);
            let request = |id| WriteRequest {
                id,
                source: "conductor".to_string(),
            };
            view.show_write_request(Some(request(7)), cx);
            assert_eq!(
                view.write_request().map(WriteRequest::message).as_deref(),
                Some("conductor wants to write into this pane")
            );
            view.dismiss_conversation_banner(cx);
            assert_eq!(view.write_request(), None, "dismissing denies the write");
            assert_eq!(
                view.conversation_banner(),
                Some(&ConversationBanner::OfferResume),
                "a decision leaves the resume notice in place"
            );

            view.show_write_request(Some(request(8)), cx);
            view.accept_conversation_banner(cx);
            assert_eq!(view.write_request(), None, "accepting allows the write");
            assert_eq!(
                view.conversation_banner(),
                Some(&ConversationBanner::OfferResume)
            );
            assert!(view.terminal.queued_raw_input_for_test().is_empty());
        });
    }
}
