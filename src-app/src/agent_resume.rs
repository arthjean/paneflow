use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use paneflow_agent_config::{RuntimeResume, SESSION_ID_PLACEHOLDER, runtime_resume};
use paneflow_config::schema::PaneFlowConfig;

use crate::agent_launcher::{AgentCommandSpec, TerminalAgent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConversationTemplate {
    Resume,
    Fork,
}

pub(crate) fn runtime_resume_of(agent: TerminalAgent) -> Option<&'static RuntimeResume> {
    runtime_resume(agent.runtime().id)
}

pub(crate) fn can_fork(agent: TerminalAgent) -> bool {
    runtime_resume_of(agent).is_some_and(|resume| resume.fork_argv.is_some())
}

fn compiled_pattern(pattern: &'static str) -> Option<regex::Regex> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<regex::Regex>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = match cache.lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    cache
        .entry(pattern)
        .or_insert_with(|| regex::Regex::new(pattern).ok())
        .clone()
}

pub(crate) fn accepts_session_id(agent: TerminalAgent, session_id: &str) -> bool {
    let Some(resume) = runtime_resume_of(agent) else {
        return false;
    };
    crate::agent_sessions::is_valid_session_id(session_id)
        && compiled_pattern(resume.session_id_pattern)
            .is_some_and(|pattern| pattern.is_match(session_id))
}

pub(crate) fn conversation_spec(
    agent: TerminalAgent,
    template: ConversationTemplate,
    session_id: &str,
    config: &PaneFlowConfig,
) -> Option<AgentCommandSpec> {
    let resume = runtime_resume_of(agent)?;
    let argv = match template {
        ConversationTemplate::Resume => resume.session_argv,
        ConversationTemplate::Fork => resume.fork_argv?,
    };
    if !accepts_session_id(agent, session_id) {
        tracing::warn!(
            target: "paneflow_app::agent_resume",
            runtime = agent.runtime().id,
            "refused a session id outside the runtime's session_id_pattern; no command built"
        );
        return None;
    }
    let (program, args) = argv.split_first()?;
    let mut spec = AgentCommandSpec::new(program);
    for arg in args {
        if *arg == SESSION_ID_PLACEHOLDER {
            spec.push_arg(session_id);
        } else {
            spec.push_arg(*arg);
        }
    }
    agent.push_launch_flags(&mut spec, config);
    debug_assert!(crate::agent_launcher::is_plain_shell_token(session_id));
    Some(spec)
}

pub(crate) fn conversation_command(
    agent: TerminalAgent,
    template: ConversationTemplate,
    session_id: &str,
    config: &PaneFlowConfig,
) -> Option<String> {
    conversation_spec(agent, template, session_id, config).map(|spec| spec.render_shell_command())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: &str = "019dc9ea-38d7-7372-9cc4-253ce944d41b";

    #[test]
    fn fork_commands_use_the_verified_cli_flags() {
        let config = PaneFlowConfig::default();
        assert_eq!(
            conversation_command(
                TerminalAgent::ClaudeCode,
                ConversationTemplate::Fork,
                UUID,
                &config
            ),
            Some(format!("claude --resume {UUID} --fork-session"))
        );
        assert_eq!(
            conversation_command(
                TerminalAgent::Codex,
                ConversationTemplate::Fork,
                UUID,
                &config
            ),
            Some(format!("codex fork {UUID}"))
        );
        assert_eq!(
            conversation_command(
                TerminalAgent::Gemini,
                ConversationTemplate::Fork,
                UUID,
                &config
            ),
            None
        );
        assert!(can_fork(TerminalAgent::ClaudeCode));
        assert!(!can_fork(TerminalAgent::Opencode));
        assert!(!can_fork(TerminalAgent::Amp));
    }

    #[test]
    fn the_runtime_pattern_applies_on_top_of_the_generic_session_id_check() {
        assert!(accepts_session_id(TerminalAgent::ClaudeCode, UUID));
        assert!(!accepts_session_id(TerminalAgent::ClaudeCode, "ses_abc"));
        assert!(accepts_session_id(TerminalAgent::Opencode, "ses_abc"));
        assert!(!accepts_session_id(TerminalAgent::Opencode, UUID));
        assert!(!accepts_session_id(TerminalAgent::Amp, UUID));
        for agent in [TerminalAgent::Gemini, TerminalAgent::Opencode] {
            assert!(!accepts_session_id(agent, "--dangerously-skip-permissions"));
        }
    }

    #[tracing_test::traced_test]
    #[test]
    fn a_flag_shaped_session_id_builds_no_command_and_logs_a_warning() {
        let config = PaneFlowConfig::default();
        for template in [ConversationTemplate::Resume, ConversationTemplate::Fork] {
            assert_eq!(
                conversation_command(
                    TerminalAgent::ClaudeCode,
                    template,
                    "--dangerously-skip-permissions",
                    &config
                ),
                None
            );
        }
        assert!(logs_contain("WARN"));
        assert!(logs_contain(
            "refused a session id outside the runtime's session_id_pattern"
        ));
    }
}
