use std::sync::Mutex;
use std::time::{Duration, Instant};

use paneflow_config::schema::{PaneFlowConfig, ProgramResumeMode, ProgramResumeSettings};
use paneflow_host::host::{
    AttemptOutcome, ResumeAttemptReport, ResumeCandidate, ResumeRefusal, ResumeRequest,
};
use paneflow_program_resume::{AttemptId, RecordId, escape_for_display};

use crate::agent_launcher::TerminalAgent;
use crate::app::conversation_restore::RESUME_SPACING;
use crate::terminal::host_link::HostLinkEnd;
use crate::terminal::types::ShellQuoting;

pub(crate) const RESUME_OUTCOME_POLL: Duration = Duration::from_millis(250);
pub(crate) const RESUME_OUTCOME_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResumePolicy {
    pub(crate) settings: ProgramResumeSettings,
    pub(crate) restore_conversations: bool,
}

impl ResumePolicy {
    pub(crate) fn from_config(config: &PaneFlowConfig) -> Self {
        Self {
            settings: config.program_resume_settings(),
            restore_conversations: config
                .agents
                .as_ref()
                .is_none_or(paneflow_config::schema::AgentsConfig::resolved_restore_conversations),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OfferReason {
    Untrusted,
    Unsettled,
    AgentShape,
    AgentsDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResumeCommand {
    pub(crate) record: RecordId,
    pub(crate) argv: Vec<String>,
    pub(crate) cwd: String,
}

impl ResumeCommand {
    fn of(candidate: &ResumeCandidate) -> Self {
        Self {
            record: candidate.record,
            argv: candidate.argv.clone(),
            cwd: candidate.cwd.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum ResumeState {
    #[default]
    None,
    Resuming {
        command: ResumeCommand,
        attempt: Option<AttemptId>,
    },
    Offered {
        command: ResumeCommand,
        reason: OfferReason,
    },
    Blocked {
        command: ResumeCommand,
        reason: ResumeRefusal,
    },
    Failed {
        argv: Vec<String>,
        reason: String,
    },
}

pub(crate) fn agent_of(argv: &[String]) -> Option<TerminalAgent> {
    let program = argv.first()?;
    if !paneflow_config::schema::is_bare_program_name(program) {
        return None;
    }
    TerminalAgent::from_binary(program)
}

pub(crate) fn matches_native_argv(agent: TerminalAgent, argv: &[String]) -> bool {
    crate::agent_resume::runtime_resume_of(agent).is_some_and(|resume| {
        resume
            .native_argv
            .iter()
            .any(|template| matches_template(agent, template, argv))
    })
}

fn matches_template(agent: TerminalAgent, template: &[&str], argv: &[String]) -> bool {
    template.len() == argv.len()
        && template
            .iter()
            .zip(argv)
            .all(|(expected, element)| match *expected {
                paneflow_agent_config::SESSION_ID_PLACEHOLDER => {
                    crate::agent_resume::accepts_session_id(agent, element)
                }
                paneflow_agent_config::ABSOLUTE_PATH_PLACEHOLDER => {
                    std::path::Path::new(element).is_absolute()
                }
                literal => literal == element,
            })
}

pub(crate) fn restore_state(policy: &ResumePolicy, end: &HostLinkEnd) -> ResumeState {
    if !policy.settings.mode.advertises() || !end.kind.lost_by_the_host() {
        return ResumeState::None;
    }
    let Some(candidate) = end.resume_candidate() else {
        return ResumeState::None;
    };
    let command = ResumeCommand::of(candidate);
    if let Some(reason) = candidate.blocked {
        return ResumeState::Blocked { command, reason };
    }
    let offer = |reason| ResumeState::Offered {
        command: ResumeCommand::of(candidate),
        reason,
    };
    if !candidate.settled {
        return offer(OfferReason::Unsettled);
    }
    let resumes = match agent_of(&candidate.argv) {
        Some(_) if !policy.restore_conversations => return offer(OfferReason::AgentsDisabled),
        Some(agent) if matches_native_argv(agent, &candidate.argv) => true,
        Some(_) => return offer(OfferReason::AgentShape),
        None => {
            policy.settings.mode == ProgramResumeMode::Auto
                || policy.settings.trusts(&candidate.argv)
        }
    };
    if resumes {
        ResumeState::Resuming {
            command,
            attempt: None,
        }
    } else {
        offer(OfferReason::Untrusted)
    }
}

impl ResumeState {
    pub(crate) fn restart_request(&self) -> Option<ResumeRequest> {
        match self {
            Self::Resuming {
                command,
                attempt: None,
            } => Some(ResumeRequest {
                record: Some(command.record),
                ..ResumeRequest::default()
            }),
            _ => None,
        }
    }

    pub(crate) fn yields_conversation(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub(crate) fn awaited_attempt(&self) -> Option<&AttemptId> {
        match self {
            Self::Resuming {
                attempt: Some(attempt),
                ..
            } => Some(attempt),
            _ => None,
        }
    }

    pub(crate) fn dismissed_record(&self) -> Option<RecordId> {
        match self {
            Self::Offered { command, .. } | Self::Blocked { command, .. } => Some(command.record),
            _ => None,
        }
    }

    pub(crate) fn after_attempt(self, report: Option<&ResumeAttemptReport>) -> Self {
        let Self::Resuming { command, attempt } = self else {
            return self;
        };
        let Some(report) = report else {
            return match attempt {
                Some(attempt) => Self::Resuming {
                    command,
                    attempt: Some(attempt),
                },
                None => Self::None,
            };
        };
        if attempt
            .as_ref()
            .is_some_and(|awaited| report.attempt.as_ref() != Some(awaited))
        {
            return Self::Resuming { command, attempt };
        }
        let reason = report.reason.clone().unwrap_or_default();
        match report.outcome {
            AttemptOutcome::Prepared => Self::Resuming {
                command,
                attempt: report.attempt.clone(),
            },
            AttemptOutcome::Started | AttemptOutcome::Settled => Self::None,
            AttemptOutcome::Failed if reason == ResumeRefusal::CwdMissing.reason() => {
                Self::Blocked {
                    command,
                    reason: ResumeRefusal::CwdMissing,
                }
            }
            AttemptOutcome::Failed => Self::Failed {
                argv: command.argv,
                reason,
            },
            AttemptOutcome::Refused => match refusal_named(&reason) {
                Some(refusal) if refusal.blocks_the_record() => Self::Blocked {
                    command,
                    reason: refusal,
                },
                Some(ResumeRefusal::Unsettled) => Self::Offered {
                    command,
                    reason: OfferReason::Unsettled,
                },
                Some(ResumeRefusal::UnknownRecord) => Self::None,
                _ => Self::Failed {
                    argv: command.argv,
                    reason,
                },
            },
        }
    }

    pub(crate) fn settled_outcome(&self, report: &ResumeAttemptReport) -> bool {
        self.awaited_attempt()
            .is_some_and(|awaited| report.attempt.as_ref() == Some(awaited))
            && report.outcome != AttemptOutcome::Prepared
    }

    pub(crate) fn banner_message(&self) -> Option<String> {
        match self {
            Self::None | Self::Resuming { .. } => None,
            Self::Offered { command, reason } => {
                let lead = match reason {
                    OfferReason::Untrusted => "Resume this program?",
                    OfferReason::Unsettled => "Resume did not complete last time",
                    OfferReason::AgentShape => {
                        "This agent asked to resume with arguments Paneflow does not recognize"
                    }
                    OfferReason::AgentsDisabled => {
                        "Conversation restore is off, so this agent did not resume on its own"
                    }
                };
                Some(format!(
                    "{lead}: {} in {}",
                    display_argv(&command.argv),
                    escape_for_display(&command.cwd)
                ))
            }
            Self::Blocked { command, reason } => Some(match reason {
                ResumeRefusal::CwdMissing => format!(
                    "Not resumed: `{}` no longer exists",
                    escape_for_display(&command.cwd)
                ),
                ResumeRefusal::ShellUnsupported => "This shell cannot resume programs".to_string(),
                ResumeRefusal::SandboxChanged => "Sandbox changed; not resumed".to_string(),
                ResumeRefusal::FirstProcess => {
                    "The program that started this pane cannot be resumed".to_string()
                }
                other => format!("Not resumed: {other}"),
            }),
            Self::Failed { argv, reason } => Some(format!(
                "Could not resume `{}`: {}",
                argv.first()
                    .map(|program| escape_for_display(program))
                    .unwrap_or_default(),
                failure_wording(reason)
            )),
        }
    }

    pub(crate) fn offers_resume(&self) -> bool {
        matches!(self, Self::Offered { .. })
    }
}

fn refusal_named(reason: &str) -> Option<ResumeRefusal> {
    [
        ResumeRefusal::CwdMissing,
        ResumeRefusal::ShellUnsupported,
        ResumeRefusal::SandboxChanged,
        ResumeRefusal::FirstProcess,
        ResumeRefusal::Unsettled,
        ResumeRefusal::UnknownRecord,
        ResumeRefusal::UnknownAttempt,
        ResumeRefusal::LauncherMissing,
        ResumeRefusal::InvalidCommand,
        ResumeRefusal::HandoffUnwritable,
        ResumeRefusal::NotInPane,
    ]
    .into_iter()
    .find(|refusal| refusal.reason() == reason)
}

fn failure_wording(reason: &str) -> String {
    match reason {
        "not_found" => "not found".to_string(),
        "not_executable" => "not executable".to_string(),
        "cwd_missing" => "its directory no longer exists".to_string(),
        "handoff_invalid" => "the resume file was invalid".to_string(),
        other => match refusal_named(other) {
            Some(refusal) => refusal.to_string(),
            None => escape_for_display(&other.replace('_', " ")),
        },
    }
}

pub(crate) fn display_element(element: &str) -> String {
    let escaped = escape_for_display(element);
    let plain = !escaped.is_empty()
        && escaped.chars().all(|character| {
            character.is_alphanumeric()
                || matches!(
                    character,
                    '/' | '.' | '_' | '-' | ':' | '=' | '@' | '+' | ','
                )
        });
    if plain {
        escaped
    } else {
        format!("'{}'", escaped.replace('\'', "'\\''"))
    }
}

pub(crate) fn display_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|element| display_element(element))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn launcher_input(
    quoting: ShellQuoting,
    launcher: &str,
    handoff: &str,
    reset_line: bool,
) -> Vec<u8> {
    let launcher_word = super::input::quote_path_for_shell(launcher, quoting);
    let handoff_word = super::input::quote_path_for_shell(handoff, quoting);
    let call = match quoting {
        ShellQuoting::PowerShell => format!("& {launcher_word} {handoff_word}"),
        ShellQuoting::Posix | ShellQuoting::Wsl | ShellQuoting::Cmd => {
            format!("{launcher_word} {handoff_word}")
        }
    };
    let mut bytes = Vec::new();
    if reset_line {
        bytes.extend_from_slice(crate::terminal::view::conversation::line_reset_for(quoting));
    }
    bytes.extend_from_slice(call.as_bytes());
    bytes.push(b'\r');
    bytes
}

#[derive(Debug, Default)]
pub(crate) struct ResumePacer {
    last: Mutex<Option<Instant>>,
}

impl ResumePacer {
    pub(crate) fn reserve(&self, now: Instant) -> Instant {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let slot = match *last {
            Some(previous) if now < previous + RESUME_SPACING => previous + RESUME_SPACING,
            _ => now,
        };
        *last = Some(slot);
        slot
    }
}

pub(crate) fn resume_pacer() -> &'static ResumePacer {
    static PACER: std::sync::OnceLock<ResumePacer> = std::sync::OnceLock::new();
    PACER.get_or_init(ResumePacer::default)
}

#[cfg(test)]
#[path = "program_resume_tests.rs"]
mod tests;
