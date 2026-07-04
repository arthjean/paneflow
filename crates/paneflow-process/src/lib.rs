//! Bounded external-process execution shared across Paneflow crates.
//!
//! `std`-only, zero external dependencies, so it can be a dependency of the
//! embedded `paneflow-shim` without inflating the binary that ships inside the
//! main executable (EP-002, US-005).
//!
//! Every external subprocess in Paneflow must run under a wall-clock deadline
//! with a bounded stdout buffer and a null stdin, so that a hung mirror, a
//! PATH-hijacked agent binary, or a slow/dead network mount can never freeze a
//! caller or exhaust memory. [`run_with_timeout`] is that one primitive: it is
//! synchronous and is meant to run on a background thread (the codebase calls
//! it from inside `smol::unblock`), never on the GPUI render thread.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::error::Error;
use std::fmt;
use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// Upper bound on captured stderr. stderr is diagnostics-only, so a small fixed
/// cap is enough; like stdout it is drained past the cap so the child never
/// blocks on a full pipe.
const STDERR_CAP: u64 = 64 * 1024;

/// How often [`run_with_timeout`] polls the child for exit. Small enough that a
/// fast command returns promptly, large enough that a multi-minute deadline
/// does not spin the CPU.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Windows `CREATE_NO_WINDOW` process-creation flag (`winbase.h`, `0x0800_0000`).
/// Paneflow runs as a GUI-subsystem process with no console of its own, so every
/// console subprocess it spawns (the `git diff --shortstat` poller, agent CLIs,
/// MCP probes) would otherwise get a freshly-allocated, *visible* console window
/// that flashes open and shut for the child's lifetime. Suppressing it is always
/// correct for `run_with_timeout` callers: they pipe stdout/stderr and null
/// stdin, so the child is non-interactive by construction and never needs a
/// console. Kept as a raw literal so this crate stays `std`-only / zero-dep (it
/// is embedded in `paneflow-shim`).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Output from a bounded process run.
///
/// `stdout` and `stderr` are always bounded. The truncation flags tell callers
/// whether bytes were discarded after the configured caps while the pipe was
/// still drained so the child could exit.
#[derive(Debug)]
pub struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

impl From<BoundedOutput> for Output {
    fn from(value: BoundedOutput) -> Self {
        Self {
            status: value.status,
            stdout: value.stdout,
            stderr: value.stderr,
        }
    }
}

/// Why a bounded run did not produce a normal [`BoundedOutput`].
#[derive(Debug)]
pub enum ProcError {
    /// The child could not be spawned.
    Spawn(io::Error),
    /// Polling the child's status failed.
    Wait(io::Error),
    /// The deadline elapsed before the child and its inherited pipes completed.
    /// The child tree was terminated best-effort and cleanup was detached so the
    /// caller is released by the deadline.
    Timeout,
}

impl fmt::Display for ProcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcError::Spawn(e) => write!(f, "failed to spawn process: {e}"),
            ProcError::Wait(e) => write!(f, "failed to poll process status: {e}"),
            ProcError::Timeout => {
                write!(f, "process exceeded its deadline and was killed")
            }
        }
    }
}

impl Error for ProcError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ProcError::Spawn(e) | ProcError::Wait(e) => Some(e),
            ProcError::Timeout => None,
        }
    }
}

/// Run `cmd` to completion under a wall-clock `deadline`, capturing at most
/// `stdout_cap` bytes of stdout (and a small fixed cap of stderr).
///
/// The deadline starts after the child is successfully spawned. Process creation
/// itself is owned by the OS and may still block on platform-level executable
/// lookup or antivirus hooks.
///
/// - stdin is `/dev/null` so the child can never block waiting on a prompt.
/// - stdout/stderr are read on dedicated threads so a child that writes more
///   than the cap is drained (and discarded past the cap) instead of blocking
///   on a full pipe - bounded memory, no deadlock.
/// - the child is placed in a process group/job where the platform supports it;
///   if `deadline` elapses before the direct child and inherited pipes finish,
///   the process tree is terminated best-effort and [`ProcError::Timeout`] is
///   returned.
///
/// The returned [`BoundedOutput::stdout`] is at most `stdout_cap` bytes; a child
/// that produced more exited normally (its excess was discarded), so the status
/// is still its real exit status and [`BoundedOutput::stdout_truncated`] is set.
pub fn run_with_timeout(
    mut cmd: Command,
    deadline: Duration,
    stdout_cap: u64,
) -> Result<BoundedOutput, ProcError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // GUI-subsystem callers have no console of their own, so a console child
    // spawned without this flag pops a visible window that flashes open and shut
    // for the child's lifetime. The crate-level note on CREATE_NO_WINDOW explains
    // why suppression is always correct here (piped stdio + null stdin → the child
    // is non-interactive by construction and never needs a console).
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    configure_process_tree(&mut cmd);
    let mut child = cmd.spawn().map_err(ProcError::Spawn)?;
    let tree = ProcessTree::for_child(&child);

    // Hand the pipe ends to reader threads BEFORE polling: if we polled while
    // the child filled a ~64 KiB pipe buffer it would block on write and we'd
    // kill a child that was not actually hung.
    let stdout_reader = child
        .stdout
        .take()
        .map(|pipe| spawn_bounded_reader(pipe, stdout_cap));
    let stderr_reader = child
        .stderr
        .take()
        .map(|pipe| spawn_bounded_reader(pipe, STDERR_CAP));

    let mut stdout_reader = stdout_reader;
    let mut stderr_reader = stderr_reader;
    let start = Instant::now();
    let status = loop {
        match child.try_wait().map_err(ProcError::Wait)? {
            Some(status) => break status,
            None => {
                let Some(sleep_for) = poll_sleep_duration(start, deadline) else {
                    terminate_and_detach_cleanup(
                        child,
                        tree,
                        stdout_reader.take(),
                        stderr_reader.take(),
                    );
                    return Err(ProcError::Timeout);
                };
                thread::sleep(sleep_for);
            }
        }
    };

    let stdout = match wait_for_reader(stdout_reader.take(), start, deadline) {
        Ok(read) => read,
        Err(reader) => {
            terminate_and_detach_cleanup(child, tree, reader, stderr_reader.take());
            return Err(ProcError::Timeout);
        }
    };
    let stderr = match wait_for_reader(stderr_reader.take(), start, deadline) {
        Ok(read) => read,
        Err(reader) => {
            terminate_and_detach_cleanup(child, tree, None, reader);
            return Err(ProcError::Timeout);
        }
    };

    Ok(BoundedOutput {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    })
}

#[cfg(unix)]
fn configure_process_tree(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_tree(_cmd: &mut Command) {}

fn poll_sleep_duration(start: Instant, deadline: Duration) -> Option<Duration> {
    let elapsed = start.elapsed();
    if elapsed >= deadline {
        None
    } else {
        Some((deadline - elapsed).min(POLL_INTERVAL))
    }
}

fn remaining_until(start: Instant, deadline: Duration) -> Option<Duration> {
    let elapsed = start.elapsed();
    if elapsed >= deadline {
        None
    } else {
        Some(deadline - elapsed)
    }
}

fn terminate_and_detach_cleanup(
    mut child: Child,
    tree: ProcessTree,
    stdout_reader: Option<Receiver<BoundedRead>>,
    stderr_reader: Option<Receiver<BoundedRead>>,
) {
    tree.terminate(&mut child);
    thread::spawn(move || {
        let _ = child.wait();
        drain_reader(stdout_reader);
        drain_reader(stderr_reader);
    });
}

struct ProcessTree {
    #[cfg(unix)]
    pid: u32,
    #[cfg(windows)]
    job: Option<windows_job::Job>,
}

impl ProcessTree {
    fn for_child(child: &Child) -> Self {
        Self {
            #[cfg(unix)]
            pid: child.id(),
            #[cfg(windows)]
            job: windows_job::Job::for_child(child).ok(),
        }
    }

    fn terminate(&self, child: &mut Child) {
        #[cfg(unix)]
        kill_process_group(self.pid);
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate();
        }
        let _ = child.kill();
    }
}

#[cfg(unix)]
fn kill_process_group(pid: u32) {
    const SIGKILL: i32 = 9;

    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    if let Ok(pid) = i32::try_from(pid) {
        let _ = unsafe { kill(-pid, SIGKILL) };
    }
}

#[cfg(windows)]
mod windows_job {
    use std::ffi::c_void;
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::ptr::null_mut;

    type Handle = *mut c_void;

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: u32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct JobObjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct JobObjectExtendedLimitInformation {
        basic_limit_information: JobObjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            info_class: u32,
            info: *const c_void,
            info_len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    pub(super) struct Job(Handle);

    impl Job {
        pub(super) fn for_child(child: &Child) -> io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(null_mut(), std::ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }

            let job = Self(handle);
            let mut limits: JobObjectExtendedLimitInformation = unsafe { zeroed() };
            limits.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            let configured = unsafe {
                SetInformationJobObject(
                    job.0,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                    (&limits as *const JobObjectExtendedLimitInformation).cast(),
                    size_of::<JobObjectExtendedLimitInformation>() as u32,
                )
            };
            if configured == 0 {
                return Err(io::Error::last_os_error());
            }

            let assigned = unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle().cast()) };
            if assigned == 0 {
                return Err(io::Error::last_os_error());
            }

            Ok(job)
        }

        pub(super) fn terminate(&self) {
            let _ = unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

#[derive(Debug, Default)]
struct BoundedRead {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Read up to `cap` bytes from `pipe`, then drain and discard the remainder so
/// the child can finish writing and exit. Never retains more than `cap` bytes.
fn spawn_bounded_reader<R>(pipe: R, cap: u64) -> Receiver<BoundedRead>
where
    R: Read + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(read_bounded(pipe, cap));
    });
    rx
}

fn read_bounded<R>(mut pipe: R, cap: u64) -> BoundedRead
where
    R: Read,
{
    let mut bytes = Vec::new();
    let retain_cap = usize::try_from(cap).unwrap_or(usize::MAX);

    let _ = pipe
        .by_ref()
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes);
    let truncated = bytes.len() > retain_cap;
    if truncated {
        bytes.truncate(retain_cap);
    }

    // Drain the rest into a small scratch buffer and throw it away: a
    // chatty-but-honest child exits cleanly while a malicious stream stays
    // bounded in memory. Throttle a saturated pipe so the drain thread does not
    // spin a core for the whole deadline.
    let mut scratch = [0u8; 8 * 1024];
    loop {
        match pipe.read(&mut scratch) {
            Ok(0) | Err(_) => break,
            Ok(n) if n == scratch.len() => thread::sleep(Duration::from_millis(1)),
            Ok(_) => {}
        }
    }

    BoundedRead { bytes, truncated }
}

fn wait_for_reader(
    reader: Option<Receiver<BoundedRead>>,
    start: Instant,
    deadline: Duration,
) -> Result<BoundedRead, Option<Receiver<BoundedRead>>> {
    let Some(reader) = reader else {
        return Ok(BoundedRead::default());
    };
    let Some(remaining) = remaining_until(start, deadline) else {
        return Err(Some(reader));
    };

    match reader.recv_timeout(remaining) {
        Ok(read) => Ok(read),
        Err(RecvTimeoutError::Disconnected) => Ok(BoundedRead::default()),
        Err(RecvTimeoutError::Timeout) => Err(Some(reader)),
    }
}

fn drain_reader(reader: Option<Receiver<BoundedRead>>) {
    if let Some(reader) = reader {
        let _ = reader.recv();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shell wrapper so the behavior tests stay readable across platforms.
    #[cfg(unix)]
    fn sh(script: &str) -> Command {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script);
        c
    }
    #[cfg(windows)]
    fn sh(script: &str) -> Command {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(script);
        c
    }

    #[cfg(unix)]
    fn stdout_command() -> Command {
        sh("printf hello")
    }

    #[cfg(windows)]
    fn stdout_command() -> Command {
        sh("echo hello")
    }

    #[cfg(unix)]
    fn sleep_command() -> Command {
        sh("sleep 30")
    }

    #[cfg(windows)]
    fn sleep_command() -> Command {
        let mut c = Command::new("powershell.exe");
        c.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 30",
        ]);
        c
    }

    #[test]
    fn bounded_reader_reports_truncation() {
        let read = read_bounded(std::io::Cursor::new(b"abcdef".to_vec()), 3);
        assert_eq!(read.bytes, b"abc");
        assert!(read.truncated);
    }

    #[test]
    fn completes_under_deadline_and_captures_stdout() {
        let out = run_with_timeout(stdout_command(), Duration::from_secs(5), 1 << 20)
            .expect("fast command should complete");
        assert!(out.status.success());
        assert!(!out.stdout_truncated);
        assert!(!out.stderr_truncated);
        // printf has no trailing newline on Unix; cmd `echo` would add CRLF, so
        // assert on a prefix to stay platform-tolerant.
        assert!(
            out.stdout.starts_with(b"hello"),
            "stdout was {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    #[test]
    fn sleeping_child_is_killed_at_the_deadline() {
        // A 30 s sleeper under a 150 ms deadline must return ~immediately with
        // Timeout, not block for 30 s.
        let start = Instant::now();
        let res = run_with_timeout(sleep_command(), Duration::from_millis(150), 1 << 20);
        assert!(
            matches!(res, Err(ProcError::Timeout)),
            "expected Timeout, got {res:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must not wait for the child to finish on its own"
        );
    }

    /// stdout cap: a 1 MB producer under a 4 KiB cap returns exactly the cap,
    /// the child still exits cleanly (drain), and we neither OOM nor hang.
    /// Unix-only because it leans on `/dev/zero`; the cap/drain logic is pure
    /// `std::io` and platform-agnostic (Windows verified by inspection).
    ///
    /// Volume is 1 MB (still 256x the cap, so a broken cap would buffer the
    /// whole megabyte and fail the length assert) rather than 10 MB: the drain
    /// deliberately throttles a pipe-saturating producer with a 1 ms sleep per
    /// 8 KiB read, so 10 MB is ~1220 sleeps whose scheduler jitter on a loaded
    /// CI runner can overshoot a tight deadline. The 30 s deadline then leaves
    /// ample headroom over the ~150 ms a 1 MB drain actually takes.
    #[cfg(unix)]
    #[test]
    fn stdout_cap_truncates_without_oom_or_hang() {
        let start = Instant::now();
        let out = run_with_timeout(
            sh("head -c 1000000 /dev/zero"),
            Duration::from_secs(30),
            4096,
        )
        .expect("producer should exit cleanly after its output is drained");
        assert!(out.status.success());
        assert_eq!(
            out.stdout.len(),
            4096,
            "stdout must be capped, not buffered"
        );
        assert!(out.stdout_truncated);
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "drain must let the child exit well under the deadline"
        );
    }

    #[test]
    fn stderr_cap_reports_truncation() {
        let read = read_bounded(std::io::Cursor::new(vec![b'x'; 128 * 1024]), STDERR_CAP);
        assert_eq!(read.bytes.len(), STDERR_CAP as usize);
        assert!(read.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn descendant_pipe_holder_is_bounded_by_deadline() {
        let start = Instant::now();
        let res = run_with_timeout(
            sh("(sleep 30) & printf parent-exited"),
            Duration::from_millis(200),
            1 << 20,
        );
        assert!(
            matches!(res, Err(ProcError::Timeout)),
            "expected Timeout, got {res:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must not wait for a descendant that inherited stdout"
        );
    }

    #[test]
    fn nonzero_exit_status_is_reported_not_an_error() {
        let out = run_with_timeout(sh("exit 3"), Duration::from_secs(5), 1 << 20)
            .expect("a clean nonzero exit is an Output, not a ProcError");
        assert!(!out.status.success());
    }
}
