//! US-003: kill-on-parent-death guard for spawned agent CLIs and PTYs.
//!
//! Goal: when Paneflow dies for any reason (including `kill -9`), the
//! child processes it spawned -- `claude`, `codex`, `opencode`, the
//! shells started inside agent terminals -- must die with it. Without
//! this, orphans are reparented to PID 1 (Unix) or kept alive by the
//! kernel (Windows) and continue streaming, consuming the user's API
//! tokens until their natural timeout.
//!
//! Implementation status by OS:
//!
//! - **Windows (full)**. [`install_process_job`] creates a Job Object
//!   with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and assigns the running
//!   Paneflow process to it on startup. Every process spawned by
//!   Paneflow after that point inherits the job by default. A narrowly
//!   scoped exception exists for explicit `CREATE_BREAKAWAY_FROM_JOB`
//!   children, used by the MSI self-update relay that must survive the
//!   current GUI process exiting. Normal agent and PTY paths do not request
//!   breakaway. When Paneflow exits, the
//!   last job handle is closed and Windows kills every member -- agent
//!   CLI, ConPTY host, descendants.
//!
//! - **Linux + macOS (partial)**. [`install_process_job`] returns
//!   [`ParentGuardStatus::Unsupported`] because there is no process-wide
//!   Unix equivalent to Windows Job Objects in this app layer.
//!   Shim-wrapped agent CLIs are covered separately: `paneflow-shim`
//!   installs `prctl(PR_SET_PDEATHSIG)` on Linux and a parent-death
//!   watcher on macOS before it waits on the real agent binary. Raw PTY
//!   shells are covered by a tiny per-PTY watcher process launched through
//!   [`spawn_pty_guard`].

#[cfg(unix)]
use std::process::{ChildStdin, Command, Stdio};

const CLAUDECODE_ENV: &str = "CLAUDECODE";

/// Remove Claude Code's nesting marker from the process environment before
/// any worker thread or PTY backend starts. Alacritty 0.26 inherits the parent
/// environment and does not expose arbitrary `env_remove` entries, so this
/// process-level guard remains necessary until that spawn boundary can own the
/// exclusion directly.
///
/// # Safety
///
/// Must run before any other thread, async runtime, or foreign library can
/// concurrently read environment variables.
pub(crate) unsafe fn scrub_claudecode_env_before_threads() {
    // SAFETY: delegated to the caller by this function's contract.
    unsafe {
        std::env::remove_var(CLAUDECODE_ENV);
    }
}

#[cfg(unix)]
pub const PTY_GUARD_SUBCOMMAND: &str = "__paneflow-pty-guard";

#[cfg(unix)]
pub struct PtyGuardHandle {
    _stdin: ChildStdin,
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use win32job::{ExtendedLimitInfo, Job};

    /// Build a Job Object with `KILL_ON_JOB_CLOSE` and assign the
    /// running Paneflow process to it. Children inherit the job
    /// automatically.
    ///
    /// The job handle is deliberately leaked: the Win32 contract is
    /// "kill on last handle close", and the last handle is the one
    /// held by the running Paneflow process. Storing the `Job` in a
    /// static would risk dropping it on hot-reload or a future
    /// teardown path, which would dissociate the children before
    /// Paneflow truly exits.
    pub(super) fn install() -> Result<(), Box<dyn std::error::Error>> {
        let mut info = ExtendedLimitInfo::default();
        info.limit_kill_on_job_close().limit_breakaway_ok();
        let job = Job::create_with_limit_info(&info)?;
        job.assign_current_process()?;
        std::mem::forget(job);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ParentGuardStatus {
    Installed,
    Unsupported,
}

/// Install the process-wide kill-on-parent-death guard. Call once,
/// early in `fn main()`, before any agent CLI or PTY is spawned.
///
/// Failure is non-fatal: a hosted environment that forbids
/// `CreateJobObject` (rare; restricted container or denied ACL) means
/// orphan-on-crash is back to "best effort", but Paneflow itself
/// remains functional. Caller logs the error and proceeds.
pub fn install_process_job() -> Result<ParentGuardStatus, Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::install()?;
        Ok(ParentGuardStatus::Installed)
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Unix has no process-wide equivalent to a Windows Job Object; PTY
        // shells and shim-wrapped agents install per-child guards instead.
        Ok(ParentGuardStatus::Unsupported)
    }
}

#[cfg(unix)]
pub fn run_pty_guard_from_args(args: &[String]) -> i32 {
    let Some(parent_pid) = args.get(2).and_then(|arg| arg.parse::<u32>().ok()) else {
        return 2;
    };
    let Some(child_pgid) = args.get(3).and_then(|arg| arg.parse::<u32>().ok()) else {
        return 2;
    };
    if parent_pid <= 1 || child_pgid <= 1 {
        return 2;
    }

    set_control_pipe_nonblocking();
    while parent_still_attached(parent_pid) && process_group_alive(child_pgid) {
        if control_pipe_closed() {
            return 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    if !parent_still_attached(parent_pid) && process_group_alive(child_pgid) {
        terminate_process_group(child_pgid);
    }
    0
}

#[cfg(unix)]
#[cfg_attr(test, allow(dead_code))]
pub fn spawn_pty_guard(child_pgid: u32) -> Option<PtyGuardHandle> {
    if child_pgid <= 1 {
        return None;
    }
    let Ok(exe) = std::env::current_exe() else {
        log::debug!("parent_guard: current_exe unavailable; PTY guard not started");
        return None;
    };

    let mut cmd = Command::new(exe);
    cmd.arg(PTY_GUARD_SUBCOMMAND)
        .arg(std::process::id().to_string())
        .arg(child_pgid.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);

    match cmd.spawn() {
        Ok(mut child) => {
            let Some(stdin) = child.stdin.take() else {
                log::warn!("parent_guard: PTY guard for pgid {child_pgid} has no control pipe");
                let _ = child.kill();
                return None;
            };
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Some(PtyGuardHandle { _stdin: stdin })
        }
        Err(err) => {
            log::warn!("parent_guard: failed to start PTY guard for pgid {child_pgid}: {err}");
            None
        }
    }
}

#[cfg(unix)]
fn parent_still_attached(parent_pid: u32) -> bool {
    // SAFETY: getppid has no preconditions.
    unsafe { libc::getppid() as u32 == parent_pid }
}

#[cfg(unix)]
fn process_group_alive(pgid: u32) -> bool {
    let Ok(pgid) = i32::try_from(pgid) else {
        return false;
    };
    // SAFETY: kill with signal 0 only probes process-group existence.
    let rc = unsafe { libc::kill(-pgid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(unix)]
fn terminate_process_group(pgid: u32) {
    let Ok(pgid) = i32::try_from(pgid) else {
        return;
    };
    // SAFETY: negative pid targets the process group. The pgid is checked above.
    unsafe {
        libc::kill(-pgid, libc::SIGTERM);
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    if process_group_alive(pgid as u32) {
        // SAFETY: same process-group target after a liveness probe.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

#[cfg(unix)]
fn set_control_pipe_nonblocking() {
    // SAFETY: fcntl on stdin fd 0. Failure is non-fatal; the guard still has
    // parent/process-group polling and will exit on parent death.
    unsafe {
        let flags = libc::fcntl(0, libc::F_GETFL);
        if flags >= 0 {
            let _ = libc::fcntl(0, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

#[cfg(unix)]
fn control_pipe_closed() -> bool {
    let mut byte = [0u8; 1];
    // SAFETY: reads at most one byte into a valid stack buffer from stdin fd 0.
    let rc = unsafe { libc::read(0, byte.as_mut_ptr().cast(), 1) };
    if rc == 0 {
        return true;
    }
    if rc > 0 {
        return false;
    }
    let err = std::io::Error::last_os_error().raw_os_error();
    !matches!(err, Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK || code == libc::EINTR)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The call must NOT panic on any OS. On Windows it attempts a
    /// real Job Object install; everywhere else the unsupported shim
    /// short-circuits cleanly. We treat the Windows return value as
    /// best-effort: some hosted CI runners (GitHub Actions Windows,
    /// Azure DevOps) put the test process inside a parent Job Object
    /// with `JOB_OBJECT_LIMIT_BREAKAWAY_OK` cleared and an ACL that
    /// denies `AssignProcessToJobObject`. Win8+ allows nested jobs in
    /// general, but those restricted parent jobs still reject the
    /// assignment with `ERROR_ACCESS_DENIED`. A test panic in that
    /// case would just be CI noise -- the production call site at
    /// `main.rs:1030` already logs the error and proceeds without
    /// blocking startup.
    #[test]
    fn install_process_job_does_not_panic() {
        // Calling twice is also safe -- on Windows the second call
        // creates a second job and the OS handles the case where the
        // process is already a member. On Linux/macOS both calls are
        // no-ops.
        let _ = install_process_job();
        let _ = install_process_job();
    }

    /// Linux/macOS contract: the call must report unsupported explicitly. The
    /// behavioural assertion is that we did not silently fall through to a panic
    /// or to a `unimplemented!()`.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unix_install_is_documented_unsupported() {
        assert_eq!(
            install_process_job().unwrap(),
            ParentGuardStatus::Unsupported
        );
    }

    #[cfg(unix)]
    #[test]
    fn pty_guard_rejects_invalid_args() {
        let args = vec![
            "paneflow".to_string(),
            PTY_GUARD_SUBCOMMAND.to_string(),
            "bad".to_string(),
            "2".to_string(),
        ];
        assert_eq!(run_pty_guard_from_args(&args), 2);
    }
}
