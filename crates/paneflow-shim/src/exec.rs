#[cfg(any(unix, windows))]
use crate::{
    locate_sibling_hook_binary, PANEFLOW_AI_EVENT_SOURCE_ENV, PANEFLOW_AI_EVENT_SOURCE_INTERRUPT,
};
use std::env;
use std::ffi::OsString;
#[cfg(any(unix, windows))]
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

pub(crate) fn run_real(tool: &str, path: &Path, args: &[OsString]) -> (ExitCode, Option<i32>) {
    let mut cmd = std::process::Command::new(path);
    cmd.args(args)
        .envs(env::vars_os())
        .env("PANEFLOW_AI_TOOL", tool)
        .env("PANEFLOW_AI_PID", std::process::id().to_string());

    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        #[cfg(target_os = "linux")]
        let shim_pid = std::process::id();
        cmd.pre_exec(move || {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGINT);
            libc::sigaddset(&mut set, libc::SIGHUP);
            libc::sigaddset(&mut set, libc::SIGTERM);
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());

            #[cfg(target_os = "linux")]
            {
                let _ = libc::prctl(
                    libc::PR_SET_PDEATHSIG,
                    libc::SIGKILL as libc::c_ulong,
                    0,
                    0,
                    0,
                );
                if libc::getppid() as u32 != shim_pid {
                    libc::raise(libc::SIGKILL);
                }
            }
            Ok(())
        });
    }

    #[cfg(unix)]
    ignore_terminal_signals();
    #[cfg(unix)]
    install_sigint_watcher(tool);
    #[cfg(windows)]
    install_ctrl_c_handler(tool);

    #[cfg(target_os = "macos")]
    let parent_pid = unsafe { libc::getppid() } as u32;
    #[cfg(target_os = "macos")]
    let child_reaped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("paneflow-shim: spawn '{}' failed: {e}", path.display());
            return (ExitCode::from(127), None);
        }
    };

    #[cfg(target_os = "macos")]
    spawn_parent_death_guard(child.id(), parent_pid, std::sync::Arc::clone(&child_reaped));

    let wait_result = child.wait();
    #[cfg(target_os = "macos")]
    child_reaped.store(true, std::sync::atomic::Ordering::Release);
    match wait_result {
        Ok(status) => (
            exit_code_from_status(&status),
            Some(raw_exit_code_from_status(&status)),
        ),
        Err(e) => {
            eprintln!("paneflow-shim: wait on '{}' failed: {e}", path.display());
            (ExitCode::from(1), None)
        }
    }
}

pub(crate) fn exit_code_from_status(status: &std::process::ExitStatus) -> ExitCode {
    if let Some(code) = status.code() {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            let code = 128i32.saturating_add(sig);
            return ExitCode::from(u8::try_from(code).unwrap_or(1));
        }
    }
    ExitCode::from(1)
}

pub(crate) fn raw_exit_code_from_status(status: &std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128i32.saturating_add(sig);
        }
    }
    1
}

#[cfg(unix)]
pub(crate) fn ignore_terminal_signals() {
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
}

#[cfg(unix)]
pub(crate) fn install_sigint_watcher(tool: &str) {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }

    let tool = tool.to_owned();
    let hook_path = locate_sibling_hook_binary();
    std::thread::spawn(move || {
        let Some(hook_path) = hook_path else {
            return;
        };
        loop {
            let sig = unsafe {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGINT);
                let mut sig: libc::c_int = 0;
                if libc::sigwait(&set, &mut sig) != 0 {
                    return;
                }
                sig
            };
            if sig == libc::SIGINT {
                send_interrupt_stop(&hook_path, &tool);
            }
        }
    });
}

#[cfg(any(unix, windows))]
const MAX_INFLIGHT_REAPERS: usize = 8;

#[cfg(any(unix, windows))]
static INFLIGHT_REAPERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(any(unix, windows))]
pub(crate) fn send_interrupt_stop(hook_path: &Path, tool: &str) {
    use std::sync::atomic::Ordering;

    if INFLIGHT_REAPERS.fetch_add(1, Ordering::AcqRel) >= MAX_INFLIGHT_REAPERS {
        INFLIGHT_REAPERS.fetch_sub(1, Ordering::AcqRel);
        return;
    }

    let spawned = std::process::Command::new(hook_path)
        .arg("Stop")
        .env("PANEFLOW_AI_TOOL", tool)
        .env("PANEFLOW_AI_PID", std::process::id().to_string())
        .env(
            PANEFLOW_AI_EVENT_SOURCE_ENV,
            PANEFLOW_AI_EVENT_SOURCE_INTERRUPT,
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        INFLIGHT_REAPERS.fetch_sub(1, Ordering::AcqRel);
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"{}");
    }
    std::thread::spawn(move || {
        let _ = child.wait();
        INFLIGHT_REAPERS.fetch_sub(1, Ordering::AcqRel);
    });
}

#[cfg(windows)]
pub(crate) fn install_ctrl_c_handler(tool: &str) {
    let tool = tool.to_owned();
    let Some(hook_path) = locate_sibling_hook_binary() else {
        return;
    };
    let _ = ctrlc::set_handler(move || {
        send_interrupt_stop(&hook_path, &tool);
    });
}

#[cfg(target_os = "macos")]
pub(crate) fn spawn_parent_death_guard(
    child_pid: u32,
    parent_pid: u32,
    child_reaped: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if child_reaped.load(Ordering::Acquire) {
            return;
        }
        let reparented = unsafe { libc::getppid() } as u32 != parent_pid;
        if reparented {
            unsafe {
                libc::kill(child_pid as libc::pid_t, libc::SIGKILL);
            }
            return;
        }
        let agent_gone = unsafe { libc::kill(child_pid as libc::pid_t, 0) } != 0;
        if agent_gone {
            return;
        }
    });
}
