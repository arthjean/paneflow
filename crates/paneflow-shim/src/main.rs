#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

const PANEFLOW_AI_EVENT_SOURCE_ENV: &str = "PANEFLOW_AI_EVENT_SOURCE";
const PANEFLOW_AI_EVENT_SOURCE_INTERRUPT: &str = "interrupt";
const PANEFLOW_SHIM_TARGET_ENV: &str = "PANEFLOW_SHIM_TARGET";

mod chain;
mod claude;
mod codex;
mod detect;
mod exec;
mod refusal;
mod resume;

use chain::{Chain, Launch, CHAIN_ENV, CHAIN_TARGETS_ENV, MAX_DEPTH};
use detect::{detect_tool, find_real_binary, launched_as_another_shims_target, HOOK_BINARY_NAME};
use exec::run_real;
use refusal::Refusal;

fn main() -> ExitCode {
    if detect::invoked_stem().is_some_and(|stem| resume::is_launcher_stem(&stem)) {
        let args: Vec<OsString> = env::args_os().skip(1).collect();
        return resume::run(&args);
    }
    let Some(tool) = detect_tool() else {
        eprintln!(
            "paneflow-shim: invoked under an unexpected name; copy or \
             hardlink this binary under one of the Paneflow-wrapped agent \
             CLI names ('claude', 'codex', 'gemini', …) and put that \
             directory first on $PATH."
        );
        return ExitCode::from(2);
    };

    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let now_ms = chain::now_ms();
    let session = pane_session();
    let inherited = Chain::from_env(
        env::var_os(CHAIN_ENV).as_deref(),
        env::var_os(CHAIN_TARGETS_ENV).as_deref(),
        env::var_os(PANEFLOW_SHIM_TARGET_ENV).as_deref(),
        &session,
    );
    let refusal = |reentered: bool| Refusal {
        tool,
        in_pane: !session.is_empty(),
        agents_above: chain::agents_above(inherited.as_ref(), reentered),
    };

    if launched_as_another_shims_target(
        env::var_os(PANEFLOW_SHIM_TARGET_ENV).as_deref(),
        env::current_exe().ok().as_deref(),
    ) {
        return refusal(true).exit(
            &format!("'{tool}' resolves to a Paneflow shim"),
            &format!(
                "refusing to run '{tool}': another Paneflow shim resolved this shim as the real \
                 '{tool}'; put the real '{tool}' ahead of every Paneflow helper directory on PATH"
            ),
        );
    }

    if inherited.as_ref().is_some_and(Chain::is_too_deep) {
        return refusal(false).exit(
            "launcher loop: too many nested Paneflow shims",
            &format!(
                "refusing to run '{tool}': {MAX_DEPTH} Paneflow shims are already nested above \
                 this one, which is what a launcher that keeps looking '{tool}' up on PATH \
                 produces; put the real '{tool}' ahead of every launcher on PATH"
            ),
        );
    }

    let sent_back = inherited
        .as_ref()
        .filter(|chain| chain.was_sent_back(tool, &args, now_ms));
    let real = match sent_back {
        Some(chain) if chain.targets.is_empty() => None,
        Some(chain) => find_real_binary(tool, &chain.targets),
        None => find_real_binary(tool, &[]),
    };
    let Some(real) = real else {
        return match sent_back.and_then(Chain::launcher) {
            Some(launcher) => refusal(true).exit(
                &format!("launcher loop: {}", refusal::launcher_label(launcher)),
                &format!(
                    "refusing to run '{tool}': {} looked '{tool}' up on PATH and found this \
                     Paneflow shim again, and no other '{tool}' is on PATH; install the real \
                     '{tool}' or put it ahead of that launcher on PATH",
                    launcher.display()
                ),
            ),
            None => refusal(sent_back.is_some()).exit(
                &format!("no real '{tool}' on PATH"),
                &format!("could not find real '{tool}' on PATH after self-exclusion"),
            ),
        };
    };

    let (args, preassigned) = match tool {
        "codex" => (codex::pane_session_args(args, in_pane_session()), None),
        "claude" if claude_preassign_enabled() && claude::starts_fresh_session(&args) => {
            let session_id = claude::new_session_id();
            (claude::with_session_id(args, &session_id), Some(session_id))
        }
        _ => (args, None),
    };

    let reports = inherited.is_none();
    if reports {
        notify_session_start(tool, preassigned);
    }

    let chain = Chain::extended(
        inherited.as_ref(),
        &Launch {
            tool,
            session: &session,
            target: &real,
            arguments: &args,
            now_ms,
            sent_back: sent_back.is_some(),
        },
    );
    let (code, agent_exit) = run_real(tool, &real, &args, &chain, reports);

    if reports {
        let interrupted_exit = agent_exit.is_some_and(is_interrupt_exit_code);
        if let Some(exit_code) = agent_exit {
            notify_exit(tool, exit_code, interrupted_exit);
        }
        notify_session_end(tool, interrupted_exit);
    }

    code
}

fn pane_session() -> String {
    env::var_os("PANEFLOW_SESSION_ID")
        .map(|session| session.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn claude_preassign_enabled() -> bool {
    env::var_os(paneflow_agent_config::CLAUDE_PREASSIGN_SESSION_ID_ENV)
        .is_some_and(|value| value == "1")
}

fn in_pane_session() -> bool {
    env::var_os("PANEFLOW_SESSION_ID").is_some_and(|session| !session.is_empty())
}

fn is_interrupt_exit_code(exit_code: i32) -> bool {
    const STATUS_CONTROL_C_EXIT: i32 = 0xC000_013Au32 as i32;
    matches!(exit_code, 129 | 130 | 137 | 143 | STATUS_CONTROL_C_EXIT)
}

fn notify_exit(tool: &str, exit_code: i32, interrupted: bool) {
    let Some(hook_path) = locate_sibling_hook_binary() else {
        return;
    };
    let mut cmd = std::process::Command::new(&hook_path);
    cmd.arg("Exit")
        .env("PANEFLOW_AI_TOOL", tool)
        .env("PANEFLOW_AI_PID", std::process::id().to_string())
        .env("PANEFLOW_AI_EXIT_CODE", exit_code.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if interrupted {
        cmd.env(
            PANEFLOW_AI_EVENT_SOURCE_ENV,
            PANEFLOW_AI_EVENT_SOURCE_INTERRUPT,
        );
    }
    let _ = cmd.status();
}

fn notify_session_start(tool: &str, preassigned: Option<String>) {
    let Some(hook_path) = locate_sibling_hook_binary() else {
        return;
    };
    let payload = session_start_payload(preassigned.as_deref());
    let tool = tool.to_owned();
    let pid = std::process::id().to_string();
    let spawned = std::thread::Builder::new()
        .name("paneflow-shim-session-start".into())
        .spawn(move || {
            let child = std::process::Command::new(&hook_path)
                .arg("SessionStart")
                .env("PANEFLOW_AI_TOOL", &tool)
                .env("PANEFLOW_AI_PID", &pid)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            let Ok(mut child) = child else {
                return;
            };
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write as _;
                let _ = stdin.write_all(payload.as_bytes());
            }
            let _ = child.wait();
        });
    let _ = spawned;
}

fn session_start_payload(preassigned: Option<&str>) -> String {
    preassigned.map_or_else(
        || "{}".to_string(),
        |session_id| format!("{{\"session_id\":\"{session_id}\"}}"),
    )
}

fn notify_session_end(tool: &str, interrupted: bool) {
    let Some(hook_path) = locate_sibling_hook_binary() else {
        return;
    };
    let mut cmd = std::process::Command::new(&hook_path);
    cmd.arg("SessionEnd")
        .env("PANEFLOW_AI_TOOL", tool)
        .env("PANEFLOW_AI_PID", std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if interrupted {
        cmd.env(
            PANEFLOW_AI_EVENT_SOURCE_ENV,
            PANEFLOW_AI_EVENT_SOURCE_INTERRUPT,
        );
    }
    let _ = cmd.status();
}

pub(crate) fn locate_sibling_hook_binary() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join(HOOK_BINARY_NAME);
    candidate.is_file().then_some(candidate)
}

#[cfg(test)]
#[path = "tests/detect.rs"]
mod detect_tests;

#[cfg(test)]
#[path = "tests/chain.rs"]
mod chain_tests;

#[cfg(test)]
#[path = "tests/refusal.rs"]
mod refusal_tests;

#[cfg(test)]
mod session_start_tests {
    #[test]
    fn a_preassigned_id_reaches_the_session_start_hook() {
        assert_eq!(super::session_start_payload(None), "{}");
        assert_eq!(
            super::session_start_payload(Some("6f1c2a8e-58b4-4c1e-9f0c-7a2d3b4c5d6e")),
            r#"{"session_id":"6f1c2a8e-58b4-4c1e-9f0c-7a2d3b4c5d6e"}"#
        );
    }
}
