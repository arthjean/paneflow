use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use paneflow_agent_config::usage::accounts::{UsageAgent, account_dir_key, canonical_account_dir};
use paneflow_agent_config::usage::model::{
    PlanLimits, plausible_claude_rate_limits, plausible_codex_rate_limits,
};
use paneflow_process::{LineSession, ProcError, spawn_line_session};
use serde_json::{Value, json};

pub const PROBE_FLOOR: Duration = Duration::from_secs(30);
pub const PROBE_DEADLINE: Duration = Duration::from_secs(10);
pub const PROBE_EXIT_GRACE: Duration = Duration::from_secs(2);
pub const MAX_CONCURRENT_PROBES: usize = 2;
pub const MAX_PROBED_ACCOUNTS: usize = 64;
const PROBE_LINE_CAP: usize = 4 * 1024 * 1024;
const MAX_VERSION_CHARS: usize = 64;
const PANEFLOW_ENV_PREFIX: &str = "PANEFLOW_";
const CLAUDE_INIT_ID: &str = "paneflow-initialize";
const CLAUDE_USAGE_ID: &str = "paneflow-get-usage";
const CODEX_INIT_ID: u64 = 1;
const CODEX_RATE_LIMITS_ID: u64 = 2;
const CLAUDE_PROBE_ARGS: &[&str] = &[
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--no-session-persistence",
    "--strict-mcp-config",
    "--settings",
    r#"{"disableAllHooks":true}"#,
];
const CLAUDE_PROBE_ENV: &[(&str, &str)] = &[
    ("ENABLE_CLAUDEAI_MCP_SERVERS", "false"),
    ("CLAUDE_CODE_AUTO_CONNECT_IDE", "0"),
    ("CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL", "1"),
];
const CLAUDE_PROBE_ENV_REMOVED: &[&str] = &["FORCE_CODE_TERMINAL"];

#[derive(Clone, Debug, PartialEq)]
pub enum ProbeOutcome {
    Limits {
        limits: PlanLimits,
        agent_version: Option<String>,
    },
    NoLimitsReported {
        agent_version: Option<String>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("no {0} binary was found on PATH outside Paneflow's own directories")]
    Missing(&'static str),
    #[error("{} is not a directory", .0.display())]
    NoConfigDir(PathBuf),
    #[error("the private probe directory cannot be prepared: {0}")]
    WorkDir(std::io::Error),
    #[error("the probe could not start: {0}")]
    Spawn(ProcError),
    #[error("the probe gave no answer within {0:?}")]
    Timeout(Duration),
    #[error("the probe exited before it answered")]
    Closed,
    #[error("the probe could not be written to: {0}")]
    Write(std::io::Error),
    #[error("the probe answered with an error: {0}")]
    Answered(String),
    #[error("the probe answered with an unknown schema")]
    UnknownSchema,
}

pub struct ProbeProgram {
    pub program: PathBuf,
    pub leading_args: Vec<OsString>,
    pub work_dir: PathBuf,
}

impl ProbeProgram {
    pub fn binary(program: PathBuf, work_dir: PathBuf) -> Self {
        Self {
            program,
            leading_args: Vec::new(),
            work_dir,
        }
    }
}

pub fn probe_work_dir(paneflow_home: &Path) -> Result<PathBuf, ProbeError> {
    let dir = paneflow_home::usage_dir_in(paneflow_home).join("probe");
    paneflow_home::create_private_dir_all(&dir).map_err(ProbeError::WorkDir)?;
    Ok(dir)
}

pub const fn binary_name(agent: UsageAgent) -> &'static str {
    match agent {
        UsageAgent::ClaudeCode => "claude",
        UsageAgent::Codex => "codex",
    }
}

const fn default_dir_name(agent: UsageAgent) -> &'static str {
    match agent {
        UsageAgent::ClaudeCode => ".claude",
        UsageAgent::Codex => ".codex",
    }
}

pub fn refresh_params<'a>(accounts: impl IntoIterator<Item = (UsageAgent, &'a Path)>) -> Value {
    let accounts: Vec<Value> = accounts
        .into_iter()
        .map(|(agent, dir)| {
            json!({"agent": agent.runtime_slug(), "config_dir": dir.display().to_string()})
        })
        .collect();
    json!({ "accounts": accounts })
}

pub fn requested_accounts(params: &Value) -> Vec<(UsageAgent, PathBuf)> {
    params
        .get("accounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|account| {
            let agent = UsageAgent::for_runtime_slug(account.get("agent")?.as_str()?)?;
            let dir = Path::new(account.get("config_dir")?.as_str()?);
            dir.is_absolute()
                .then(|| (agent, canonical_account_dir(dir)))
        })
        .take(MAX_PROBED_ACCOUNTS)
        .collect()
}

pub fn shim_free_path(path: &OsString, paneflow_home: &Path, excluded: &[PathBuf]) -> OsString {
    let hook = paneflow_host::helpers::file_name(paneflow_host::helpers::AI_HOOK_STEM);
    let kept: Vec<PathBuf> = std::env::split_paths(path)
        .filter(|dir| {
            dir.is_absolute()
                && !dir.starts_with(paneflow_home)
                && !excluded.iter().any(|excluded| excluded == dir)
                && !dir.join(&hook).is_file()
        })
        .collect();
    std::env::join_paths(kept).unwrap_or_default()
}

pub fn resolve_binary(agent: UsageAgent, paneflow_home: &Path) -> Result<PathBuf, ProbeError> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let excluded = paneflow_host::helpers::helper_candidates(
        std::env::current_exe().ok().as_deref(),
        paneflow_home,
    );
    let search = shim_free_path(&path, paneflow_home, &excluded);
    which::which_in(binary_name(agent), Some(search), paneflow_home)
        .map_err(|_| ProbeError::Missing(binary_name(agent)))
}

pub fn probe_account(
    agent: UsageAgent,
    config_dir: &Path,
    paneflow_home: &Path,
    user_home: Option<&Path>,
    now_secs: u64,
) -> Result<ProbeOutcome, ProbeError> {
    let program = ProbeProgram::binary(
        resolve_binary(agent, paneflow_home)?,
        probe_work_dir(paneflow_home)?,
    );
    probe(
        agent,
        &program,
        config_dir,
        user_home,
        PROBE_DEADLINE,
        now_secs,
    )
}

pub fn probe(
    agent: UsageAgent,
    program: &ProbeProgram,
    config_dir: &Path,
    user_home: Option<&Path>,
    deadline: Duration,
    now_secs: u64,
) -> Result<ProbeOutcome, ProbeError> {
    if !config_dir.is_dir() {
        return Err(ProbeError::NoConfigDir(config_dir.to_path_buf()));
    }
    let command = probe_command(agent, program, config_dir, user_home);
    let mut session = spawn_line_session(command, PROBE_LINE_CAP).map_err(ProbeError::Spawn)?;
    let started = Instant::now();
    let mut exchange = Exchange {
        session: &mut session,
        started,
        deadline,
    };
    let answered = match agent {
        UsageAgent::ClaudeCode => claude_exchange(&mut exchange, now_secs),
        UsageAgent::Codex => codex_exchange(&mut exchange, now_secs),
    };
    if !session.close(PROBE_EXIT_GRACE) {
        log::debug!(
            "paneflow-serve: the {} usage probe outlived its exit grace and was terminated",
            binary_name(agent)
        );
    }
    answered
}

fn probe_command(
    agent: UsageAgent,
    program: &ProbeProgram,
    config_dir: &Path,
    user_home: Option<&Path>,
) -> Command {
    let mut command = Command::new(&program.program);
    command
        .args(&program.leading_args)
        .current_dir(&program.work_dir);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with(PANEFLOW_ENV_PREFIX) {
            command.env_remove(key);
        }
    }
    let key = agent.config_dir_key();
    let is_default = user_home.is_some_and(|home| {
        account_dir_key(&canonical_account_dir(&home.join(default_dir_name(agent))))
            == account_dir_key(config_dir)
    });
    if is_default {
        command.env_remove(key);
    } else {
        command.env(key, config_dir);
    }
    match agent {
        UsageAgent::ClaudeCode => {
            command.args(CLAUDE_PROBE_ARGS);
            command.envs(CLAUDE_PROBE_ENV.iter().copied());
            for key in CLAUDE_PROBE_ENV_REMOVED {
                command.env_remove(key);
            }
        }
        UsageAgent::Codex => {
            command.arg("app-server");
        }
    }
    command
}

struct Exchange<'a> {
    session: &'a mut LineSession,
    started: Instant,
    deadline: Duration,
}

impl Exchange<'_> {
    fn send(&mut self, message: &Value) -> Result<(), ProbeError> {
        self.session
            .send_line(message.to_string().as_bytes())
            .map_err(ProbeError::Write)
    }

    fn await_message(&mut self, accept: impl Fn(&Value) -> bool) -> Result<Value, ProbeError> {
        loop {
            let remaining = self
                .deadline
                .checked_sub(self.started.elapsed())
                .filter(|remaining| !remaining.is_zero())
                .ok_or(ProbeError::Timeout(self.deadline))?;
            let line = match self.session.next_line(remaining) {
                Ok(Some(line)) => line,
                Ok(None) => return Err(ProbeError::Closed),
                Err(ProcError::Timeout) => return Err(ProbeError::Timeout(self.deadline)),
                Err(error) => return Err(ProbeError::Spawn(error)),
            };
            let Ok(message) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if accept(&message) {
                return Ok(message);
            }
        }
    }
}

fn claude_control_response(id: &'static str) -> impl Fn(&Value) -> bool {
    move |message| {
        message.get("type").and_then(Value::as_str) == Some("control_response")
            && message
                .get("response")
                .and_then(|response| response.get("request_id"))
                .and_then(Value::as_str)
                == Some(id)
    }
}

fn claude_answer(message: &Value) -> Result<&Value, ProbeError> {
    let response = &message["response"];
    match response.get("subtype").and_then(Value::as_str) {
        Some("success") => Ok(&response["response"]),
        Some("error") => Err(ProbeError::Answered(
            response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unspecified")
                .chars()
                .take(256)
                .collect(),
        )),
        _ => Err(ProbeError::UnknownSchema),
    }
}

fn claude_exchange(exchange: &mut Exchange<'_>, now_secs: u64) -> Result<ProbeOutcome, ProbeError> {
    exchange.send(&json!({
        "type": "control_request",
        "request_id": CLAUDE_INIT_ID,
        "request": {"subtype": "initialize", "hooks": null},
    }))?;
    let initialized = exchange.await_message(claude_control_response(CLAUDE_INIT_ID))?;
    let agent_version = claude_answer(&initialized)?
        .get("claude_code_version")
        .and_then(Value::as_str)
        .and_then(version_text);
    exchange.send(&json!({
        "type": "control_request",
        "request_id": CLAUDE_USAGE_ID,
        "request": {"subtype": "get_usage", "skip_behaviors": true},
    }))?;
    let usage = exchange.await_message(claude_control_response(CLAUDE_USAGE_ID))?;
    claude_usage_outcome(claude_answer(&usage)?, agent_version, now_secs)
}

pub fn claude_usage_outcome(
    usage: &Value,
    agent_version: Option<String>,
    now_secs: u64,
) -> Result<ProbeOutcome, ProbeError> {
    match usage.get("rate_limits_available").and_then(Value::as_bool) {
        Some(false) => Ok(ProbeOutcome::NoLimitsReported { agent_version }),
        Some(true) => {
            let rate_limits = usage
                .get("rate_limits")
                .filter(|rate_limits| rate_limits.is_object())
                .ok_or(ProbeError::UnknownSchema)?;
            let status_line_shape = json!({
                "five_hour": claude_status_line_window(rate_limits.get("five_hour")),
                "seven_day": claude_status_line_window(rate_limits.get("seven_day")),
            });
            Ok(ProbeOutcome::Limits {
                limits: plausible_claude_rate_limits(&status_line_shape, now_secs),
                agent_version,
            })
        }
        None => Err(ProbeError::UnknownSchema),
    }
}

fn claude_status_line_window(window: Option<&Value>) -> Value {
    let Some(window) = window.filter(|window| window.is_object()) else {
        return Value::Null;
    };
    let utilization = window.get("utilization").filter(|value| value.is_number());
    let resets_at = window
        .get("resets_at")
        .and_then(Value::as_str)
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|at| at.timestamp());
    match (utilization, resets_at) {
        (Some(utilization), Some(resets_at)) => {
            json!({"used_percentage": utilization, "resets_at": resets_at})
        }
        _ => Value::Null,
    }
}

fn codex_reply(id: u64) -> impl Fn(&Value) -> bool {
    move |message| message.get("id").and_then(Value::as_u64) == Some(id)
}

fn codex_result(message: &Value) -> Result<&Value, ProbeError> {
    if let Some(error) = message.get("error") {
        return Err(ProbeError::Answered(
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unspecified")
                .chars()
                .take(256)
                .collect(),
        ));
    }
    message.get("result").ok_or(ProbeError::UnknownSchema)
}

fn codex_exchange(exchange: &mut Exchange<'_>, now_secs: u64) -> Result<ProbeOutcome, ProbeError> {
    exchange.send(&json!({
        "jsonrpc": "2.0",
        "id": CODEX_INIT_ID,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "paneflow",
                "title": "Paneflow",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {"experimentalApi": true},
        },
    }))?;
    let initialized = exchange.await_message(codex_reply(CODEX_INIT_ID))?;
    let agent_version = codex_result(&initialized)?
        .get("userAgent")
        .and_then(Value::as_str)
        .and_then(codex_version);
    exchange.send(&json!({"jsonrpc": "2.0", "method": "initialized"}))?;
    exchange.send(&json!({
        "jsonrpc": "2.0",
        "id": CODEX_RATE_LIMITS_ID,
        "method": "account/rateLimits/read",
        "params": null,
    }))?;
    let read = exchange.await_message(codex_reply(CODEX_RATE_LIMITS_ID))?;
    codex_rate_limits_outcome(codex_result(&read)?, agent_version, now_secs)
}

pub fn codex_rate_limits_outcome(
    result: &Value,
    agent_version: Option<String>,
    now_secs: u64,
) -> Result<ProbeOutcome, ProbeError> {
    let rate_limits = result
        .get("rateLimits")
        .filter(|rate_limits| rate_limits.is_object())
        .ok_or(ProbeError::UnknownSchema)?;
    let rollout_shape = json!({
        "primary": codex_rollout_window(rate_limits.get("primary")),
        "secondary": codex_rollout_window(rate_limits.get("secondary")),
    });
    Ok(ProbeOutcome::Limits {
        limits: plausible_codex_rate_limits(&rollout_shape, now_secs),
        agent_version,
    })
}

fn codex_rollout_window(window: Option<&Value>) -> Value {
    let Some(window) = window.filter(|window| window.is_object()) else {
        return Value::Null;
    };
    json!({
        "used_percent": window.get("usedPercent"),
        "window_minutes": window.get("windowDurationMins"),
        "resets_at": window.get("resetsAt"),
    })
}

fn version_text(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()
        && text.chars().count() <= MAX_VERSION_CHARS
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')))
    .then(|| text.to_string())
}

fn codex_version(user_agent: &str) -> Option<String> {
    let (_, rest) = user_agent.split_once('/')?;
    version_text(rest.split_whitespace().next()?)
}

#[derive(Default)]
pub struct ProbeSlots {
    busy: Mutex<usize>,
    freed: Condvar,
}

pub struct ProbeSlot<'a> {
    slots: &'a ProbeSlots,
}

impl ProbeSlots {
    pub fn acquire(&self) -> ProbeSlot<'_> {
        let mut busy = self.busy.lock().unwrap_or_else(PoisonError::into_inner);
        while *busy >= MAX_CONCURRENT_PROBES {
            busy = self
                .freed
                .wait(busy)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *busy += 1;
        ProbeSlot { slots: self }
    }
}

impl Drop for ProbeSlot<'_> {
    fn drop(&mut self) {
        let mut busy = self
            .slots
            .busy
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *busy = busy.saturating_sub(1);
        self.slots.freed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use paneflow_agent_config::usage::model::{SESSION_WINDOW_SECS, WEEK_WINDOW_SECS};

    use super::*;

    const CLAUDE_RECORDED_AT: u64 = 1_793_523_918;
    const CODEX_RECORDED_AT: u64 = 1_793_523_924;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("usage-probe")
            .join(name)
    }

    struct Fake {
        dir: tempfile::TempDir,
        program: ProbeProgram,
    }

    impl Fake {
        fn env_dump(&self) -> String {
            std::fs::read_to_string(self.dir.path().join("env.txt")).unwrap_or_default()
        }
    }

    #[cfg(unix)]
    fn fake(replies: &[(&str, PathBuf)]) -> Fake {
        let dir = tempfile::tempdir().unwrap();
        let mut script = format!("env > '{}'\n", dir.path().join("env.txt").display());
        script.push_str("while IFS= read -r l; do\n  case \"$l\" in\n");
        for (needle, reply) in replies {
            script.push_str(&format!("    *'{needle}'*) cat '{}' ;;\n", reply.display()));
        }
        script.push_str("  esac\ndone\n");
        let path = dir.path().join("fake.sh");
        std::fs::write(&path, script).unwrap();
        Fake {
            program: ProbeProgram {
                program: PathBuf::from("/bin/sh"),
                leading_args: vec![path.into_os_string()],
                work_dir: dir.path().to_path_buf(),
            },
            dir,
        }
    }

    #[cfg(windows)]
    fn fake(replies: &[(&str, PathBuf)]) -> Fake {
        let dir = tempfile::tempdir().unwrap();
        let mut script = format!(
            "Get-ChildItem env: | ForEach-Object {{ \"$($_.Name)=$($_.Value)\" }} | Set-Content -Encoding UTF8 -LiteralPath '{}'\n",
            dir.path().join("env.txt").display()
        );
        script.push_str("while (($l = [Console]::In.ReadLine()) -ne $null) {\n");
        for (needle, reply) in replies {
            script.push_str(&format!(
                "  if ($l.Contains('{needle}')) {{ [Console]::Out.Write([IO.File]::ReadAllText('{}')); [Console]::Out.Flush() }}\n",
                reply.display()
            ));
        }
        script.push_str("}\n");
        let path = dir.path().join("fake.ps1");
        std::fs::write(&path, script).unwrap();
        Fake {
            program: ProbeProgram {
                program: PathBuf::from("powershell.exe"),
                leading_args: [
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ]
                .into_iter()
                .map(OsString::from)
                .chain([path.into_os_string()])
                .collect(),
                work_dir: dir.path().to_path_buf(),
            },
            dir,
        }
    }

    fn reply(fake_dir: &Path, name: &str, lines: &[Value]) -> PathBuf {
        let path = fake_dir.join(name);
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn config_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn the_claude_probe_replays_the_recorded_get_usage_answer() {
        let config = config_dir();
        let fake = fake(&[
            ("\"initialize\"", fixture("claude-2.1.316-initialize.jsonl")),
            ("\"get_usage\"", fixture("claude-2.1.316-get_usage.jsonl")),
        ]);
        let outcome = probe(
            UsageAgent::ClaudeCode,
            &fake.program,
            config.path(),
            None,
            Duration::from_secs(20),
            CLAUDE_RECORDED_AT,
        )
        .unwrap();
        let ProbeOutcome::Limits {
            limits,
            agent_version,
        } = outcome
        else {
            panic!("limits expected: {outcome:?}");
        };
        assert_eq!(agent_version.as_deref(), Some("2.1.316"));
        assert_eq!(limits.primary, None, "a null five_hour window is absent");
        let week = limits.secondary.unwrap();
        assert_eq!(week.used_percent, 67.0);
        assert_eq!(week.window_secs, WEEK_WINDOW_SECS);
        assert_eq!(week.resets_at, 1_793_527_199);
        let env = fake.env_dump();
        assert!(
            !env.lines()
                .any(|line| line.starts_with(PANEFLOW_ENV_PREFIX)),
            "{env}"
        );
        assert!(env.contains("ENABLE_CLAUDEAI_MCP_SERVERS=false"), "{env}");
        assert!(
            env.lines()
                .any(|line| line == format!("CLAUDE_CONFIG_DIR={}", config.path().display())),
            "{env}"
        );
    }

    #[test]
    fn the_claude_probe_reports_an_account_without_limits() {
        let config = config_dir();
        let scratch = tempfile::tempdir().unwrap();
        let usage = reply(
            scratch.path(),
            "usage.jsonl",
            &[json!({"type": "control_response", "response": {
                "subtype": "success", "request_id": CLAUDE_USAGE_ID,
                "response": {"rate_limits_available": false, "rate_limits": null}
            }})],
        );
        let fake = fake(&[
            ("\"initialize\"", fixture("claude-2.1.316-initialize.jsonl")),
            ("\"get_usage\"", usage),
        ]);
        let outcome = probe(
            UsageAgent::ClaudeCode,
            &fake.program,
            config.path(),
            None,
            Duration::from_secs(20),
            CLAUDE_RECORDED_AT,
        )
        .unwrap();
        assert_eq!(
            outcome,
            ProbeOutcome::NoLimitsReported {
                agent_version: Some("2.1.316".into())
            }
        );
    }

    #[test]
    fn the_claude_probe_surfaces_an_error_answer_and_an_unknown_schema() {
        let config = config_dir();
        let scratch = tempfile::tempdir().unwrap();
        let init = fixture("claude-2.1.316-initialize.jsonl");
        let error = reply(
            scratch.path(),
            "error.jsonl",
            &[json!({"type": "control_response", "response": {
                "subtype": "error", "request_id": CLAUDE_USAGE_ID, "error": "Unknown subtype: get_usage"
            }})],
        );
        let unknown = reply(
            scratch.path(),
            "unknown.jsonl",
            &[
                json!({"type": "system", "subtype": "noise"}),
                json!({"type": "control_response", "response": {
                    "subtype": "success", "request_id": CLAUDE_USAGE_ID, "response": {"limits": []}
                }}),
            ],
        );
        let run = |answer: &PathBuf| {
            let fake = fake(&[
                ("\"initialize\"", init.clone()),
                ("\"get_usage\"", answer.clone()),
            ]);
            probe(
                UsageAgent::ClaudeCode,
                &fake.program,
                config.path(),
                None,
                Duration::from_secs(20),
                CLAUDE_RECORDED_AT,
            )
        };
        assert!(
            matches!(run(&error), Err(ProbeError::Answered(message)) if message.contains("get_usage"))
        );
        assert!(matches!(run(&unknown), Err(ProbeError::UnknownSchema)));
    }

    #[test]
    fn a_silent_probe_times_out_and_is_terminated() {
        let config = config_dir();
        let fake = fake(&[]);
        let started = Instant::now();
        let outcome = probe(
            UsageAgent::ClaudeCode,
            &fake.program,
            config.path(),
            None,
            Duration::from_millis(1_500),
            CLAUDE_RECORDED_AT,
        );
        assert!(
            matches!(outcome, Err(ProbeError::Timeout(_))),
            "{outcome:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn a_missing_binary_or_config_dir_is_an_error_not_a_panic() {
        let config = config_dir();
        let missing = ProbeProgram::binary(
            config.path().join("no-such-claude"),
            config.path().to_path_buf(),
        );
        assert!(matches!(
            probe(
                UsageAgent::ClaudeCode,
                &missing,
                config.path(),
                None,
                Duration::from_secs(1),
                CLAUDE_RECORDED_AT
            ),
            Err(ProbeError::Spawn(_))
        ));
        assert!(matches!(
            probe(
                UsageAgent::Codex,
                &missing,
                &config.path().join("absent"),
                None,
                Duration::from_secs(1),
                CODEX_RECORDED_AT
            ),
            Err(ProbeError::NoConfigDir(_))
        ));
    }

    #[test]
    fn the_codex_probe_replays_the_recorded_rate_limits_answer() {
        let config = config_dir();
        let home = config.path().join("home");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let default_codex = canonical_account_dir(&home.join(".codex"));
        let fake = fake(&[
            ("\"initialize\"", fixture("codex-0.166.0-initialize.jsonl")),
            (
                "account/rateLimits/read",
                fixture("codex-0.166.0-rate_limits.jsonl"),
            ),
        ]);
        let outcome = probe(
            UsageAgent::Codex,
            &fake.program,
            &default_codex,
            Some(&home),
            Duration::from_secs(20),
            CODEX_RECORDED_AT,
        )
        .unwrap();
        let ProbeOutcome::Limits {
            limits,
            agent_version,
        } = outcome
        else {
            panic!("limits expected: {outcome:?}");
        };
        assert_eq!(agent_version.as_deref(), Some("0.166.0"));
        let month = limits.primary.unwrap();
        assert_eq!(month.used_percent, 3.0);
        assert_eq!(month.window_secs, 43_200 * 60);
        assert_eq!(month.resets_at, 1_796_073_944);
        assert_eq!(limits.secondary, None);
        let env = fake.env_dump();
        assert!(
            !env.lines().any(|line| line.starts_with("CODEX_HOME=")),
            "the default CODEX_HOME is left unset: {env}"
        );
    }

    #[test]
    fn the_codex_probe_surfaces_errors_and_unknown_schemas() {
        let config = config_dir();
        let scratch = tempfile::tempdir().unwrap();
        let init = fixture("codex-0.166.0-initialize.jsonl");
        let error = reply(
            scratch.path(),
            "error.jsonl",
            &[
                json!({"id": 2, "error": {"code": -32600, "message": "codex account authentication required to read rate limits"}}),
            ],
        );
        let unknown = reply(
            scratch.path(),
            "unknown.jsonl",
            &[json!({"id": 2, "result": {"limits": {}}})],
        );
        let run = |answer: &PathBuf| {
            let fake = fake(&[
                ("\"initialize\"", init.clone()),
                ("account/rateLimits/read", answer.clone()),
            ]);
            probe(
                UsageAgent::Codex,
                &fake.program,
                config.path(),
                None,
                Duration::from_secs(20),
                CODEX_RECORDED_AT,
            )
        };
        assert!(
            matches!(run(&error), Err(ProbeError::Answered(message)) if message.contains("authentication"))
        );
        assert!(matches!(run(&unknown), Err(ProbeError::UnknownSchema)));
        let fake = fake(&[("\"initialize\"", init.clone())]);
        assert!(matches!(
            probe(
                UsageAgent::Codex,
                &fake.program,
                config.path(),
                None,
                Duration::from_millis(1_500),
                CODEX_RECORDED_AT
            ),
            Err(ProbeError::Timeout(_))
        ));
    }

    #[test]
    fn implausible_or_malformed_windows_are_dropped() {
        let usage = json!({"rate_limits_available": true, "rate_limits": {
            "five_hour": {"utilization": 140, "resets_at": "2026-11-01T12:00:00Z"},
            "seven_day": {"utilization": 20, "resets_at": "not a date"},
        }});
        assert_eq!(
            claude_usage_outcome(&usage, None, CLAUDE_RECORDED_AT).unwrap(),
            ProbeOutcome::Limits {
                limits: PlanLimits::default(),
                agent_version: None
            }
        );
        let session = json!({"rate_limits_available": true, "rate_limits": {
            "five_hour": {"utilization": 12.5, "resets_at": "2026-11-01T12:00:00+00:00"},
        }});
        let ProbeOutcome::Limits { limits, .. } =
            claude_usage_outcome(&session, None, CLAUDE_RECORDED_AT).unwrap()
        else {
            panic!("limits expected");
        };
        assert_eq!(limits.primary.unwrap().window_secs, SESSION_WINDOW_SECS);
        assert!(matches!(
            claude_usage_outcome(&json!({}), None, CLAUDE_RECORDED_AT),
            Err(ProbeError::UnknownSchema)
        ));
    }

    #[test]
    fn the_refresh_params_round_trip_only_absolute_known_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let params = refresh_params([
            (UsageAgent::ClaudeCode, dir.path()),
            (UsageAgent::Codex, dir.path()),
        ]);
        let mut decoded = requested_accounts(&params);
        let mut hostile = params.clone();
        hostile["accounts"].as_array_mut().unwrap().extend([
            json!({"agent": "gemini", "config_dir": dir.path().display().to_string()}),
            json!({"agent": "codex", "config_dir": "relative/dir"}),
            json!({"agent": 3}),
        ]);
        assert_eq!(requested_accounts(&hostile), decoded);
        decoded.sort();
        assert_eq!(
            decoded,
            vec![
                (UsageAgent::ClaudeCode, canonical_account_dir(dir.path())),
                (UsageAgent::Codex, canonical_account_dir(dir.path())),
            ]
        );
        assert!(requested_accounts(&json!({})).is_empty());
    }

    #[test]
    fn the_search_path_skips_paneflow_shim_directories() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("paneflow-home");
        let installed_bin = root.path().join("Paneflow").join("bin");
        let real = root.path().join("real");
        let excluded = root.path().join("exe-bin");
        for dir in [&home.join("bin"), &installed_bin, &real, &excluded] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(
            installed_bin.join(paneflow_host::helpers::file_name(
                paneflow_host::helpers::AI_HOOK_STEM,
            )),
            b"",
        )
        .unwrap();
        let path = std::env::join_paths([
            &home.join("bin"),
            &installed_bin,
            &excluded,
            &PathBuf::from("relative-bin"),
            &real,
        ])
        .unwrap();
        let kept: Vec<PathBuf> = std::env::split_paths(&shim_free_path(
            &path,
            &home,
            std::slice::from_ref(&excluded),
        ))
        .collect();
        assert_eq!(kept, vec![real]);
    }

    #[test]
    fn the_installed_binaries_answer_the_probe_for_the_default_accounts() {
        let Some(home) = paneflow_agent_config::home_dir() else {
            return;
        };
        for (agent, gate) in [
            (UsageAgent::ClaudeCode, "PANEFLOW_REAL_CLAUDE"),
            (UsageAgent::Codex, "PANEFLOW_REAL_CODEX"),
        ] {
            if std::env::var(gate).as_deref() != Ok("1") {
                continue;
            }
            let paneflow_home = tempfile::tempdir().unwrap();
            let dir = canonical_account_dir(&home.join(default_dir_name(agent)));
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let started = Instant::now();
            let binary = resolve_binary(agent, paneflow_home.path()).unwrap();
            let outcome =
                probe_account(agent, &dir, paneflow_home.path(), Some(&home), now).unwrap();
            eprintln!(
                "{agent:?} via {} answered in {:?}: {outcome:?}",
                binary.display(),
                started.elapsed()
            );
            assert!(
                matches!(
                    &outcome,
                    ProbeOutcome::Limits {
                        agent_version: Some(_),
                        ..
                    } | ProbeOutcome::NoLimitsReported {
                        agent_version: Some(_)
                    }
                ),
                "{outcome:?}"
            );
        }
    }

    #[test]
    fn at_most_two_probes_hold_a_slot_at_once() {
        let slots = Arc::new(ProbeSlots::default());
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..6)
            .map(|_| {
                let (slots, running, peak) =
                    (Arc::clone(&slots), Arc::clone(&running), Arc::clone(&peak));
                std::thread::spawn(move || {
                    let _slot = slots.acquire();
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(40));
                    running.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), MAX_CONCURRENT_PROBES);
    }
}
