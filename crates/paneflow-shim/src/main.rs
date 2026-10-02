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

mod claude;
mod codex;
mod detect;
mod exec;

use detect::{detect_tool, find_real_binary, launched_as_another_shims_target, HOOK_BINARY_NAME};
use exec::run_real;

fn main() -> ExitCode {
    let Some(tool) = detect_tool() else {
        eprintln!(
            "paneflow-shim: invoked under an unexpected name; copy or \
             hardlink this binary under one of the Paneflow-wrapped agent \
             CLI names ('claude', 'codex', 'gemini', …) and put that \
             directory first on $PATH."
        );
        return ExitCode::from(2);
    };

    if launched_as_another_shims_target(
        env::var_os(PANEFLOW_SHIM_TARGET_ENV).as_deref(),
        env::current_exe().ok().as_deref(),
    ) {
        eprintln!(
            "paneflow-shim: refusing to run '{tool}': another Paneflow shim resolved this shim \
             as the real '{tool}'; put the real '{tool}' ahead of every Paneflow helper \
             directory on PATH"
        );
        return ExitCode::from(127);
    }

    let Some(real) = find_real_binary(tool) else {
        eprintln!("paneflow-shim: could not find real '{tool}' on PATH after self-exclusion");
        return ExitCode::from(127);
    };

    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let (args, preassigned) = match tool {
        "codex" => (codex::pane_session_args(args, in_pane_session()), None),
        "claude" if claude_preassign_enabled() && claude::starts_fresh_session(&args) => {
            let session_id = claude::new_session_id();
            (claude::with_session_id(args, &session_id), Some(session_id))
        }
        _ => (args, None),
    };

    notify_session_start(tool, preassigned);

    let (code, agent_exit) = run_real(tool, &real, &args);

    let interrupted_exit = agent_exit.is_some_and(is_interrupt_exit_code);
    if let Some(exit_code) = agent_exit {
        notify_exit(tool, exit_code, interrupted_exit);
    }

    notify_session_end(tool, interrupted_exit);

    code
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
