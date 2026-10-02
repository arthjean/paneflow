use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::{Subcommand, ValueEnum};
use paneflow_agent_config::runtime_catalog::{Runtime, runtime_by_id};
use paneflow_ipc_client::IpcTransport;
use paneflow_ipc_client::host_control::HostTransport;
use regex::Regex;
use serde_json::{Value, json};

use super::selector::resolve_target;
use super::{CliError, EXIT_OK};

pub const CAPTURE_FORMAT: &str = "paneflow-screen-capture: 1";

const CAPTURE_SEPARATOR: &str = "---";

const CLI_VERSION_TIMEOUT: Duration = Duration::from_secs(5);

const REDACTED: &str = "<redacted>";

static SECRET_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\bsk-[A-Za-z0-9_\-]{16,}",
        r"\bghp_[A-Za-z0-9]{20,}",
        r"\bxox[abposr]-[A-Za-z0-9\-]{10,}",
        r"\bAKIA[0-9A-Z]{16}\b",
        r"\beyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
    ]
    .into_iter()
    .filter_map(|pattern| Regex::new(pattern).ok())
    .collect()
});

#[derive(Subcommand, Debug)]
pub enum AgentCommand {
    #[command(
        about = "Capture the agent screen of a pane, read from the host viewport, as a reference screen for the rule corpus"
    )]
    Capture {
        #[arg(help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`")]
        target: String,
        #[arg(long, value_enum, help = "State the agent shows on this screen")]
        state: CaptureState,
        #[arg(
            long,
            value_name = "DIR",
            help = "Directory to write the capture into, for instance `runtimes/<slug>/fixtures/screens` in the repository (default: `cache/captures/<slug>` under the Paneflow home)"
        )]
        out: Option<PathBuf>,
    },
    #[command(
        about = "Explain how Paneflow classifies a pane's agent: signal source, every screen rule with its origin and result, the winning rule and the final state"
    )]
    Explain {
        #[arg(help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`")]
        target: String,
        #[arg(long, help = "Emit the explanation as JSON instead of a report")]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CaptureState {
    Working,
    Idle,
    Blocked,
}

impl CaptureState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Blocked => "blocked",
        }
    }
}

pub fn run(client: &impl IpcTransport, command: AgentCommand) -> Result<i32, CliError> {
    match command {
        AgentCommand::Capture { target, state, out } => capture(client, &target, state, out),
        AgentCommand::Explain { target, json } => explain(client, &target, json),
    }
}

fn explain(client: &impl IpcTransport, target: &str, json_out: bool) -> Result<i32, CliError> {
    let surface_id = resolve_target(client, target)?;
    let status = super::reject_legacy_error(
        client
            .call("surface.status", json!({ "surface_id": surface_id }))
            .map_err(CliError::runtime)?,
    )?;
    let session = status
        .get("session")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CliError::target(format!(
                "surface {surface_id} is not backed by a Paneflow host session"
            ))
        })?;
    let mut explained = host_call("agent.explain", json!({ "session": session }))?;
    explained["surface_id"] = json!(surface_id);
    explained["signal_source"] = json!(signal_source(&status));
    explained["final_state"] = json!(super::worker_state::reduced_state(&status));
    explained["worker_status"] = status.get("state").cloned().unwrap_or(Value::Null);
    if json_out {
        super::print_json(&explained)?;
    } else {
        print!("{}", explain_report(&explained));
    }
    Ok(EXIT_OK)
}

fn signal_source(status: &Value) -> &'static str {
    if status.get("attention_reason").and_then(Value::as_str) == Some("bell") {
        return "bell";
    }
    match status.get("activity_source").and_then(Value::as_str) {
        Some("hooks") => "hook",
        Some("screen") => "screen",
        _ => "none",
    }
}

fn explain_report(explained: &Value) -> String {
    let text = |key: &str| explained.get(key).and_then(Value::as_str);
    let mut report = String::new();
    let mut line = |label: &str, value: String| {
        report.push_str(&format!("{label:<16}{value}\n"));
    };
    line(
        "runtime",
        match (text("runtime_label"), text("runtime_slug")) {
            (Some(label), Some(slug)) => format!("{label} ({slug})"),
            _ => "none recognized in the foreground, so no screen rule runs".to_string(),
        },
    );
    line(
        "signal",
        text("signal_source").unwrap_or("none").to_string(),
    );
    line(
        "final state",
        text("final_state").unwrap_or("unknown").to_string(),
    );
    line(
        "screen",
        match (text("screen_state"), text("winner")) {
            (Some(state), Some(rule)) => format!("{state} by rule {rule}"),
            _ => "no rule matches".to_string(),
        },
    );
    line(
        "visible blocker",
        text("visible_blocker").map_or_else(|| "none".to_string(), |rule| format!("rule {rule}")),
    );
    line(
        "last hook",
        explained
            .get("last_hook")
            .filter(|hook| hook.is_object())
            .map_or_else(
                || "none".to_string(),
                |hook| {
                    let age = hook.get("age_ms").and_then(Value::as_u64).unwrap_or(0);
                    let stale =
                        if hook.get("current_generation").and_then(Value::as_bool) == Some(true) {
                            ""
                        } else {
                            ", from a previous launch"
                        };
                    format!(
                        "{} from {}, {:.1} s ago{stale}",
                        hook.get("event").and_then(Value::as_str).unwrap_or("?"),
                        hook.get("tool").and_then(Value::as_str).unwrap_or("?"),
                        age as f64 / 1_000.0
                    )
                },
            ),
    );
    if let Some(title) = text("title") {
        line("title", title.to_string());
    }
    if let Some(sources) = explained.get("sources") {
        let source = |key: &str| sources.get(key).and_then(Value::as_str);
        if let Some(version) = sources.get("remote_version").and_then(Value::as_u64) {
            line("remote catalog", format!("v{version} applied"));
        }
        if let Some(reason) = source("remote_rejection") {
            line(
                "remote catalog",
                format!("remote catalog rejected: {reason}"),
            );
        }
        if let Some(path) = source("local_path") {
            line("local rules", path.to_string());
        }
        if let Some(error) = source("local_error") {
            line(
                "local rules",
                format!("rejected, previous rules kept: {error}"),
            );
        }
    }
    let rules = explained
        .get("rules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if rules.is_empty() {
        report.push_str("rules           none for this runtime\n");
        return report;
    }
    report.push_str("rules\n");
    for rule in rules {
        let field = |key: &str| rule.get(key).and_then(Value::as_str).unwrap_or("?");
        report.push_str(&format!(
            "  {} {:<28} {:<8} p{:<4} {:<9} {}{}\n",
            if rule.get("matched").and_then(Value::as_bool) == Some(true) {
                "match"
            } else {
                "  -  "
            },
            field("id"),
            field("state"),
            rule.get("priority").and_then(Value::as_i64).unwrap_or(0),
            field("region"),
            field("origin"),
            if rule.get("visible_blocker").and_then(Value::as_bool) == Some(true) {
                ", visible blocker"
            } else {
                ""
            }
        ));
    }
    report
}

fn capture(
    client: &impl IpcTransport,
    target: &str,
    state: CaptureState,
    out: Option<PathBuf>,
) -> Result<i32, CliError> {
    let surface_id = resolve_target(client, target)?;
    let session = surface_session(client, surface_id)?;
    let captured = host_call("agent.capture", json!({ "session": session }))?;
    let Some(runtime) = captured
        .get("runtime_id")
        .and_then(Value::as_str)
        .and_then(runtime_by_id)
    else {
        return Err(CliError::target(format!(
            "no recognized agent in surface {surface_id}"
        )));
    };
    let home = dirs::home_dir();
    let document = capture_document(&CaptureFacts {
        state,
        runtime,
        cols: captured.get("cols").and_then(Value::as_u64).unwrap_or(0),
        rows: captured.get("rows").and_then(Value::as_u64).unwrap_or(0),
        cli_version: cli_version(runtime),
        captured_at: SystemTime::now(),
        title: captured.get("title").and_then(Value::as_str),
        progress: captured.get("progress").and_then(Value::as_str),
        screen: captured
            .get("screen")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        home: home.as_deref(),
    });
    let dir = match out {
        Some(dir) => dir,
        None => paneflow_home::cache_dir()
            .ok_or_else(|| CliError::runtime("cannot resolve the Paneflow home"))?
            .join("captures")
            .join(runtime.slug),
    };
    std::fs::create_dir_all(&dir)
        .map_err(|error| CliError::runtime(format!("cannot create {}: {error}", dir.display())))?;
    let path = unique_capture_path(&dir, state, SystemTime::now());
    std::fs::write(&path, document)
        .map_err(|error| CliError::runtime(format!("cannot write {}: {error}", path.display())))?;
    println!("{}", path.display());
    Ok(EXIT_OK)
}

pub(super) fn surface_session(
    client: &impl IpcTransport,
    surface_id: u64,
) -> Result<String, CliError> {
    let status = super::reject_legacy_error(
        client
            .call("surface.status", json!({ "surface_id": surface_id }))
            .map_err(CliError::runtime)?,
    )?;
    status
        .get("session")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            CliError::target(format!(
                "surface {surface_id} is not backed by a Paneflow host session"
            ))
        })
}

pub(super) fn host_call(method: &str, params: Value) -> Result<Value, CliError> {
    let endpoint = paneflow_host::endpoint::host_endpoint_path_for_current_home()
        .ok_or_else(|| CliError::runtime("cannot locate the Paneflow host endpoint"))?;
    let host = HostTransport::connect(&endpoint, super::CLIENT_NAME).map_err(|error| {
        CliError::runtime(format!("{error}; start it with `paneflow host start`"))
    })?;
    super::reject_legacy_error(host.call(method, params).map_err(CliError::runtime)?)
}

struct CaptureFacts<'a> {
    state: CaptureState,
    runtime: &'static Runtime,
    cols: u64,
    rows: u64,
    cli_version: String,
    captured_at: SystemTime,
    title: Option<&'a str>,
    progress: Option<&'a str>,
    screen: &'a str,
    home: Option<&'a Path>,
}

fn capture_document(facts: &CaptureFacts<'_>) -> String {
    let mut document = String::new();
    document.push_str(CAPTURE_FORMAT);
    document.push('\n');
    let mut field = |key: &str, value: &str| {
        document.push_str(key);
        document.push_str(": ");
        document.push_str(value);
        document.push('\n');
    };
    field("state", facts.state.as_str());
    field("runtime", facts.runtime.slug);
    field("cols", &facts.cols.to_string());
    field("rows", &facts.rows.to_string());
    field("cli", &one_line(&facts.cli_version));
    field("paneflow", env!("CARGO_PKG_VERSION"));
    field("captured", &utc_timestamp(facts.captured_at));
    if let Some(title) = facts.title.filter(|title| !title.trim().is_empty()) {
        field("title", &one_line(&redact(title, facts.home)));
    }
    if let Some(progress) = facts.progress {
        field("progress", progress);
    }
    document.push_str(CAPTURE_SEPARATOR);
    document.push('\n');
    document.push_str(&plain_screen(&redact(facts.screen, facts.home)));
    document
}

fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn plain_screen(screen: &str) -> String {
    let mut lines: Vec<&str> = screen.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

pub(super) fn redact(text: &str, home: Option<&Path>) -> String {
    let mut text = text.to_string();
    if let Some(home) = home.and_then(Path::to_str).filter(|home| home.len() > 1) {
        let home = home.trim_end_matches(['/', '\\']);
        for spelling in [home.to_string(), home.replace('\\', "/")] {
            text = text.replace(&spelling, "~");
        }
    }
    for pattern in SECRET_PATTERNS.iter() {
        text = pattern.replace_all(&text, REDACTED).into_owned();
    }
    text
}

fn cli_version(runtime: &Runtime) -> String {
    let Some(alias) = runtime.detection.command_aliases.first() else {
        return "unknown".to_string();
    };
    let Ok(program) = which::which(alias) else {
        return "unknown".to_string();
    };
    let Ok(mut child) = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return "unknown".to_string();
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < CLI_VERSION_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return "unknown".to_string();
            }
        }
    }
    let mut output = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut stdout, &mut output);
    }
    one_line(&output)
}

fn unique_capture_path(dir: &Path, state: CaptureState, now: SystemTime) -> PathBuf {
    let stamp = utc_timestamp(now).replace([':', '-'], "");
    let mut path = dir.join(format!("{}-{stamp}.txt", state.as_str()));
    let mut suffix = 2;
    while path.exists() {
        path = dir.join(format!("{}-{stamp}-{suffix}.txt", state.as_str()));
        suffix += 1;
    }
    path
}

fn utc_timestamp(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let days = (seconds / 86_400) as i64;
    let of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3_600,
        (of_day % 3_600) / 60,
        of_day % 60
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_agent_config::runtime_catalog::runtime_by_slug;

    #[test]
    fn a_trapped_screen_is_redacted_before_it_reaches_the_capture() {
        let home = if cfg!(windows) {
            PathBuf::from(r"C:\Users\alice")
        } else {
            PathBuf::from("/home/alice")
        };
        let forward = home.display().to_string().replace('\\', "/");
        let screen = format!(
            "cwd {home}{sep}dev{sep}app and {forward}/dev too\n\
             export OPENAI_API_KEY=sk-proj-abcdefghijklmnop1234\n\
             token ghp_abcdefghijklmnopqrstuvwxyz0123\n\
             slack xoxb-1234567890-abcdefghij\n\
             aws AKIAABCDEFGHIJKLMNOP\n\
             jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.c2lnbmF0dXJlLXZhbHVl\n\
             risk-assessment stays and so does sk-short",
            home = home.display(),
            sep = std::path::MAIN_SEPARATOR,
        );
        let redacted = redact(&screen, Some(&home));
        for secret in [
            "alice",
            "sk-proj-abcdefghijklmnop1234",
            "ghp_abcdefghijklmnopqrstuvwxyz0123",
            "xoxb-1234567890-abcdefghij",
            "AKIAABCDEFGHIJKLMNOP",
            "eyJhbGciOiJIUzI1NiJ9",
        ] {
            assert!(!redacted.contains(secret), "{secret} leaked:\n{redacted}");
        }
        let separator = std::path::MAIN_SEPARATOR;
        assert!(
            redacted.contains(&format!("cwd ~{separator}dev{separator}app")),
            "{redacted}"
        );
        assert!(redacted.contains("and ~/dev too"), "{redacted}");
        assert_eq!(redacted.matches(REDACTED).count(), 5, "{redacted}");
        assert!(redacted.contains("risk-assessment stays and so does sk-short"));
    }

    #[test]
    fn the_capture_header_carries_the_geometry_versions_and_date() {
        let runtime = runtime_by_slug("claude-code").expect("claude");
        let document = capture_document(&CaptureFacts {
            state: CaptureState::Blocked,
            runtime,
            cols: 120,
            rows: 40,
            cli_version: "2.1.285 (Claude Code)\n".to_string(),
            captured_at: UNIX_EPOCH + Duration::from_secs(1_790_942_645),
            title: Some("✳ Fix the build"),
            progress: Some("indeterminate"),
            screen: "Do you want to proceed?   \n❯ 1. Yes   \n  2. No\n\n\n",
            home: None,
        });
        let (header, screen) = document.split_once("\n---\n").expect("header separator");
        assert_eq!(
            header,
            format!(
                "{CAPTURE_FORMAT}\nstate: blocked\nruntime: claude-code\ncols: 120\nrows: 40\n\
                 cli: 2.1.285 (Claude Code)\npaneflow: {}\ncaptured: 2026-10-02T12:04:05Z\n\
                 title: ✳ Fix the build\nprogress: indeterminate",
                env!("CARGO_PKG_VERSION")
            )
        );
        assert_eq!(screen, "Do you want to proceed?\n❯ 1. Yes\n  2. No\n");
    }

    #[test]
    fn the_explain_report_names_the_signal_rules_origins_winner_and_rejections() {
        let explained = json!({
            "runtime_label": "Claude Code",
            "runtime_slug": "claude-code",
            "signal_source": "hook",
            "final_state": "blocked",
            "screen_state": "blocked",
            "winner": "approval-menu",
            "visible_blocker": "approval-menu",
            "last_hook": {"event": "PreToolUse", "tool": "claude", "age_ms": 2_500, "current_generation": true},
            "sources": {
                "remote_version": 3,
                "remote_rejection": "version 2 is not newer than the cached version 3",
                "local_path": "~/.paneflow/runtimes/claude-code/screen.toml",
                "local_error": "line 6: rule \"busy\" has an invalid regex"
            },
            "rules": [
                {"id": "approval-menu", "state": "blocked", "priority": 30, "region": "last:15", "origin": "remote v3", "visible_blocker": true, "matched": true},
                {"id": "idle-prompt", "state": "idle", "priority": 10, "region": "last:15", "origin": "local", "visible_blocker": false, "matched": false}
            ]
        });
        let report = explain_report(&explained);
        for expected in [
            "runtime         Claude Code (claude-code)",
            "signal          hook",
            "final state     blocked",
            "screen          blocked by rule approval-menu",
            "last hook       PreToolUse from claude, 2.5 s ago",
            "remote catalog  v3 applied",
            "remote catalog  remote catalog rejected: version 2 is not newer than the cached version 3",
            "local rules     rejected, previous rules kept: line 6:",
            "match approval-menu",
            "remote v3, visible blocker",
            "idle-prompt",
            "local",
        ] {
            assert!(
                report.contains(expected),
                "{expected:?} missing in:\n{report}"
            );
        }
        let unrecognized = explain_report(&json!({"rules": []}));
        assert!(unrecognized.contains("none recognized in the foreground"));
    }

    #[test]
    fn the_signal_source_prefers_the_bell_reason() {
        assert_eq!(
            signal_source(&json!({"attention_reason": "bell", "activity_source": "screen"})),
            "bell"
        );
        assert_eq!(signal_source(&json!({"activity_source": "hooks"})), "hook");
        assert_eq!(
            signal_source(&json!({"activity_source": "screen"})),
            "screen"
        );
        assert_eq!(signal_source(&json!({})), "none");
    }

    #[test]
    fn civil_dates_cross_leap_years_and_epochs() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn a_second_capture_in_the_same_second_gets_its_own_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = UNIX_EPOCH + Duration::from_secs(1_790_942_645);
        let first = unique_capture_path(dir.path(), CaptureState::Idle, now);
        assert_eq!(
            first.file_name().and_then(|name| name.to_str()),
            Some("idle-20261002T120405Z.txt")
        );
        std::fs::write(&first, "x").expect("write");
        let second = unique_capture_path(dir.path(), CaptureState::Idle, now);
        assert_eq!(
            second.file_name().and_then(|name| name.to_str()),
            Some("idle-20261002T120405Z-2.txt")
        );
    }
}
