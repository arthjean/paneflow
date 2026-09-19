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

mod detect;
mod exec;

use detect::{detect_tool, find_real_binary};
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

    let Some(real) = find_real_binary(tool) else {
        eprintln!("paneflow-shim: could not find real '{tool}' on PATH after self-exclusion");
        return ExitCode::from(127);
    };

    let args: Vec<OsString> = env::args_os().skip(1).collect();

    notify_session_start(tool);

    let (code, agent_exit) = run_real(tool, &real, &args);

    let interrupted_exit = agent_exit.is_some_and(is_interrupt_exit_code);
    if let Some(exit_code) = agent_exit {
        notify_exit(tool, exit_code, interrupted_exit);
    }

    notify_session_end(tool, interrupted_exit);

    code
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

fn notify_session_start(tool: &str) {
    let Some(hook_path) = locate_sibling_hook_binary() else {
        return;
    };
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
                let _ = stdin.write_all(b"{}");
            }
            let _ = child.wait();
        });
    let _ = spawned;
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
    #[cfg(unix)]
    let name = "paneflow-ai-hook";
    #[cfg(windows)]
    let name = "paneflow-ai-hook.exe";
    let candidate = dir.join(name);
    candidate.is_file().then_some(candidate)
}

#[cfg(test)]
#[path = "tests/detect.rs"]
mod detect_tests;
