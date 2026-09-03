#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::error::Error;
use std::fmt;
use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

const STDERR_CAP: u64 = 64 * 1024;

const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug)]
pub struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

impl fmt::Display for OutputStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdout => f.write_str("stdout"),
            Self::Stderr => f.write_str("stderr"),
        }
    }
}

#[derive(Debug)]
pub enum ProcError {
    Spawn(io::Error),
    ProcessTree(io::Error),
    InvalidOutputLimit(u64),
    Supervision(io::Error),
    ReaderSpawn {
        stream: OutputStream,
        source: io::Error,
    },
    Wait(io::Error),
    Read {
        stream: OutputStream,
        source: io::Error,
    },
    OutputLimitExceeded {
        stream: OutputStream,
        cap: u64,
    },
    Timeout,
}

impl fmt::Display for ProcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcError::Spawn(e) => write!(f, "failed to spawn process: {e}"),
            ProcError::ProcessTree(e) => {
                write!(f, "failed to configure process-tree supervision: {e}")
            }
            ProcError::InvalidOutputLimit(cap) => {
                write!(f, "capture limit {cap} cannot be represented safely")
            }
            ProcError::Supervision(e) => write!(f, "process supervision failed: {e}"),
            ProcError::ReaderSpawn { stream, source } => {
                write!(f, "failed to start {stream} reader: {source}")
            }
            ProcError::Wait(e) => write!(f, "failed to poll process status: {e}"),
            ProcError::Read { stream, source } => {
                write!(f, "failed to capture process {stream}: {source}")
            }
            ProcError::OutputLimitExceeded { stream, cap } => {
                write!(f, "process {stream} exceeded its {cap}-byte capture limit")
            }
            ProcError::Timeout => write!(f, "process exceeded its deadline; termination requested"),
        }
    }
}

impl Error for ProcError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ProcError::Spawn(e)
            | ProcError::ProcessTree(e)
            | ProcError::Supervision(e)
            | ProcError::Wait(e) => Some(e),
            ProcError::ReaderSpawn { source, .. } | ProcError::Read { source, .. } => Some(source),
            ProcError::InvalidOutputLimit(_)
            | ProcError::OutputLimitExceeded { .. }
            | ProcError::Timeout => None,
        }
    }
}

pub fn run_with_timeout(
    mut cmd: Command,
    deadline: Duration,
    stdout_cap: u64,
) -> Result<BoundedOutput, ProcError> {
    let stdout_cap = validate_capture_cap(stdout_cap)?;
    let stderr_cap = validate_capture_cap(STDERR_CAP)?;

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    configure_process_tree(&mut cmd);
    let cleanup = spawn_cleanup_worker()?;
    let child = cmd.spawn().map_err(ProcError::Spawn)?;
    let start = Instant::now();
    let mut process = RunningProcess::new(child, cleanup)?;

    let stdout_pipe = process
        .child_mut()?
        .stdout
        .take()
        .ok_or_else(|| supervision_error("stdout capture pipe unavailable after spawn"))?;
    let stderr_pipe = process
        .child_mut()?
        .stderr
        .take()
        .ok_or_else(|| supervision_error("stderr capture pipe unavailable after spawn"))?;

    let (reader_tx, reader_rx) = mpsc::channel();
    process.attach_reader(reader_rx);
    spawn_bounded_reader(
        stdout_pipe,
        stdout_cap,
        OutputStream::Stdout,
        reader_tx.clone(),
    )?;
    spawn_bounded_reader(stderr_pipe, stderr_cap, OutputStream::Stderr, reader_tx)?;

    let mut capture = CaptureState::default();
    let status = loop {
        drain_ready_reader_messages(process.reader()?, &mut capture)?;
        match process.child_mut()?.try_wait().map_err(ProcError::Wait)? {
            Some(status) => break status,
            None => {
                let Some(sleep_for) = poll_sleep_duration(start, deadline) else {
                    return Err(ProcError::Timeout);
                };
                thread::sleep(sleep_for);
            }
        }
    };

    while !capture.is_complete() {
        let remaining = remaining_until(start, deadline).unwrap_or(Duration::ZERO);
        match process.reader()?.recv_timeout(remaining) {
            Ok(message) => capture.record(message)?,
            Err(RecvTimeoutError::Timeout) => return Err(ProcError::Timeout),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(supervision_error(
                    "output readers disconnected before reporting both streams",
                ));
            }
        }
    }

    let (stdout, stderr) = capture.finish()?;
    process.complete()?;

    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
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

fn validate_capture_cap(cap: u64) -> Result<usize, ProcError> {
    if cap.checked_add(1).is_none() {
        return Err(ProcError::InvalidOutputLimit(cap));
    }
    usize::try_from(cap).map_err(|_| ProcError::InvalidOutputLimit(cap))
}

fn supervision_error(message: &'static str) -> ProcError {
    ProcError::Supervision(io::Error::other(message))
}

struct RunningProcess {
    child: Option<Child>,
    tree: ProcessTree,
    reader: Option<Receiver<ReaderMessage>>,
    cleanup: mpsc::Sender<CleanupResources>,
}

impl RunningProcess {
    fn new(mut child: Child, cleanup: mpsc::Sender<CleanupResources>) -> Result<Self, ProcError> {
        let tree = match ProcessTree::for_child(&child) {
            Ok(tree) => tree,
            Err(source) => {
                let _ = child.kill();
                send_cleanup(&cleanup, child, None);
                return Err(ProcError::ProcessTree(source));
            }
        };
        Ok(Self {
            child: Some(child),
            tree,
            reader: None,
            cleanup,
        })
    }

    fn child_mut(&mut self) -> Result<&mut Child, ProcError> {
        self.child
            .as_mut()
            .ok_or_else(|| supervision_error("child already consumed"))
    }

    fn attach_reader(&mut self, reader: Receiver<ReaderMessage>) {
        self.reader = Some(reader);
    }

    fn reader(&self) -> Result<&Receiver<ReaderMessage>, ProcError> {
        self.reader
            .as_ref()
            .ok_or_else(|| supervision_error("reader channel not attached"))
    }

    fn complete(mut self) -> Result<(), ProcError> {
        self.tree.disarm().map_err(ProcError::ProcessTree)?;
        self.child = None;
        Ok(())
    }

    fn terminate_and_detach(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        self.tree.terminate(&mut child);
        send_cleanup(&self.cleanup, child, self.reader.take());
    }
}

impl Drop for RunningProcess {
    fn drop(&mut self) {
        self.terminate_and_detach();
    }
}

struct CleanupResources {
    child: Child,
    reader: Option<Receiver<ReaderMessage>>,
}

fn spawn_cleanup_worker() -> Result<mpsc::Sender<CleanupResources>, ProcError> {
    let (sender, receiver) = mpsc::channel::<CleanupResources>();
    thread::Builder::new()
        .name("paneflow-process-cleanup".to_string())
        .spawn(move || {
            let Ok(mut resources) = receiver.recv() else {
                return;
            };
            let _ = resources.child.wait();
            if let Some(reader) = resources.reader {
                while reader.recv().is_ok() {}
            }
        })
        .map_err(ProcError::Supervision)?;
    Ok(sender)
}

fn send_cleanup(
    sender: &mpsc::Sender<CleanupResources>,
    child: Child,
    reader: Option<Receiver<ReaderMessage>>,
) {
    let resources = CleanupResources { child, reader };
    if let Err(mpsc::SendError(mut resources)) = sender.send(resources) {
        let _ = resources.child.try_wait();
    }
}

struct ProcessTree {
    #[cfg(unix)]
    pid: u32,
    #[cfg(windows)]
    job: windows_job::Job,
}

impl ProcessTree {
    fn for_child(child: &Child) -> io::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            pid: child.id(),
            #[cfg(windows)]
            job: windows_job::Job::for_child(child)?,
        })
    }

    fn terminate(&self, child: &mut Child) {
        #[cfg(unix)]
        kill_process_group(self.pid);
        #[cfg(windows)]
        let _ = self.job.terminate();
        let _ = child.kill();
    }

    fn disarm(&self) -> io::Result<()> {
        #[cfg(windows)]
        self.job.disarm()?;
        Ok(())
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
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use win32job::{ExtendedLimitInfo, Job as Win32Job};
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;

    pub(super) struct Job(Win32Job);

    impl Job {
        pub(super) fn for_child(child: &Child) -> io::Result<Self> {
            let mut limits = ExtendedLimitInfo::default();
            limits.limit_kill_on_job_close();
            let job = Win32Job::create_with_limit_info(&limits)
                .map_err(|error| io::Error::other(error.to_string()))?;
            job.assign_process(child.as_raw_handle() as isize)
                .map_err(|error| io::Error::other(error.to_string()))?;
            Ok(Self(job))
        }

        pub(super) fn disarm(&self) -> io::Result<()> {
            let mut limits = self
                .0
                .query_extended_limit_info()
                .map_err(|error| io::Error::other(error.to_string()))?;
            limits.clear_limits();
            self.0
                .set_extended_limit_info(&limits)
                .map_err(|error| io::Error::other(error.to_string()))
        }

        pub(super) fn terminate(&self) -> io::Result<()> {
            let handle = self.0.handle() as HANDLE;
            if unsafe { TerminateJobObject(handle, 1) } == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }
}

#[derive(Debug)]
enum ReaderFailure {
    Read(io::Error),
    LimitExceeded { cap: u64 },
}

#[derive(Debug)]
struct ReaderMessage {
    stream: OutputStream,
    result: Result<Vec<u8>, ReaderFailure>,
}

fn spawn_bounded_reader<R>(
    pipe: R,
    cap: usize,
    stream: OutputStream,
    sender: mpsc::Sender<ReaderMessage>,
) -> Result<(), ProcError>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name(format!("paneflow-process-{stream}"))
        .spawn(move || {
            let result = read_bounded(pipe, cap);
            let _ = sender.send(ReaderMessage { stream, result });
        })
        .map(|_| ())
        .map_err(|source| ProcError::ReaderSpawn { stream, source })
}

fn read_bounded<R>(mut pipe: R, cap: usize) -> Result<Vec<u8>, ReaderFailure>
where
    R: Read,
{
    let mut bytes = Vec::new();
    pipe.by_ref()
        .take((cap as u64) + 1)
        .read_to_end(&mut bytes)
        .map_err(ReaderFailure::Read)?;
    if bytes.len() > cap {
        return Err(ReaderFailure::LimitExceeded { cap: cap as u64 });
    }
    Ok(bytes)
}

#[derive(Default)]
struct CaptureState {
    stdout: Option<Vec<u8>>,
    stderr: Option<Vec<u8>>,
}

impl CaptureState {
    fn record(&mut self, message: ReaderMessage) -> Result<(), ProcError> {
        let bytes = match message.result {
            Ok(bytes) => bytes,
            Err(ReaderFailure::Read(source)) => {
                return Err(ProcError::Read {
                    stream: message.stream,
                    source,
                });
            }
            Err(ReaderFailure::LimitExceeded { cap }) => {
                return Err(ProcError::OutputLimitExceeded {
                    stream: message.stream,
                    cap,
                });
            }
        };
        let slot = match message.stream {
            OutputStream::Stdout => &mut self.stdout,
            OutputStream::Stderr => &mut self.stderr,
        };
        if slot.is_some() {
            return Err(supervision_error("reader reported the same stream twice"));
        }
        *slot = Some(bytes);
        Ok(())
    }

    fn is_complete(&self) -> bool {
        self.stdout.is_some() && self.stderr.is_some()
    }

    fn finish(mut self) -> Result<(Vec<u8>, Vec<u8>), ProcError> {
        let stdout = self
            .stdout
            .take()
            .ok_or_else(|| supervision_error("stdout reader result missing"))?;
        let stderr = self
            .stderr
            .take()
            .ok_or_else(|| supervision_error("stderr reader result missing"))?;
        Ok((stdout, stderr))
    }
}

fn drain_ready_reader_messages(
    reader: &Receiver<ReaderMessage>,
    capture: &mut CaptureState,
) -> Result<(), ProcError> {
    loop {
        match reader.try_recv() {
            Ok(message) => capture.record(message)?,
            Err(TryRecvError::Empty) => return Ok(()),
            Err(TryRecvError::Disconnected) if capture.is_complete() => return Ok(()),
            Err(TryRecvError::Disconnected) => {
                return Err(supervision_error(
                    "output readers disconnected before reporting both streams",
                ));
            }
        }
    }
}

const DETACHED_REAP_INTERVAL: Duration = Duration::from_millis(500);

static DETACHED_REAPER: OnceLock<Option<mpsc::Sender<Child>>> = OnceLock::new();

pub fn spawn_detached(command: &mut Command) -> io::Result<()> {
    let child = command.spawn()?;
    if let Some(sender) = DETACHED_REAPER.get_or_init(start_detached_reaper).as_ref() {
        let _ = sender.send(child);
    }
    Ok(())
}

fn start_detached_reaper() -> Option<mpsc::Sender<Child>> {
    let (sender, receiver) = mpsc::channel::<Child>();
    thread::Builder::new()
        .name("paneflow-detached-reaper".to_owned())
        .spawn(move || reap_detached_children(&receiver))
        .ok()
        .map(|_| sender)
}

fn reap_detached_children(receiver: &Receiver<Child>) {
    let mut pending: Vec<Child> = Vec::new();
    let mut connected = true;
    while connected || !pending.is_empty() {
        if pending.is_empty() {
            match receiver.recv() {
                Ok(child) => pending.push(child),
                Err(_) => return,
            }
        } else {
            match receiver.recv_timeout(DETACHED_REAP_INTERVAL) {
                Ok(child) => pending.push(child),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => connected = false,
            }
        }
        pending.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(windows)]
    fn descendant_marker_command(
        marker: &std::path::Path,
        marker_delay_ms: u32,
        parent_lingers: bool,
    ) -> Command {
        let started = marker.with_extension("started");
        let stdout = marker.with_extension("stdout");
        let stderr = marker.with_extension("stderr");
        let mut script = String::from(
            "Start-Process -FilePath powershell.exe -WindowStyle Hidden \
             -ArgumentList @('-NoLogo','-NoProfile','-NonInteractive','-Command',\
             'Set-Content -LiteralPath $env:PANEFLOW_PROCESS_TEST_STARTED -Value started; \
             Start-Sleep -Milliseconds ([int]$env:PANEFLOW_PROCESS_TEST_DELAY); \
             Set-Content -LiteralPath \
             $env:PANEFLOW_PROCESS_TEST_MARKER -Value alive') \
             -RedirectStandardOutput $env:PANEFLOW_PROCESS_TEST_STDOUT \
             -RedirectStandardError $env:PANEFLOW_PROCESS_TEST_STDERR",
        );
        if parent_lingers {
            script.push_str("; Start-Sleep -Seconds 30");
        }

        let mut command = Command::new("powershell.exe");
        command
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
            .arg(script)
            .env("PANEFLOW_PROCESS_TEST_MARKER", marker)
            .env("PANEFLOW_PROCESS_TEST_STARTED", started)
            .env("PANEFLOW_PROCESS_TEST_DELAY", marker_delay_ms.to_string())
            .env("PANEFLOW_PROCESS_TEST_STDOUT", stdout)
            .env("PANEFLOW_PROCESS_TEST_STDERR", stderr);
        command
    }

    #[cfg(windows)]
    fn unique_marker(name: &str) -> std::path::PathBuf {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "paneflow-process-{name}-{}-{id}.marker",
            std::process::id()
        ))
    }

    #[cfg(windows)]
    fn wait_for_marker(marker: &std::path::Path, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if marker.exists() {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        marker.exists()
    }

    #[cfg(windows)]
    fn remove_marker_artifacts(marker: &std::path::Path) {
        let _ = std::fs::remove_file(marker);
        let _ = std::fs::remove_file(marker.with_extension("started"));
        let _ = std::fs::remove_file(marker.with_extension("stdout"));
        let _ = std::fs::remove_file(marker.with_extension("stderr"));
    }

    #[test]
    fn bounded_reader_rejects_overflow() {
        let read = read_bounded(std::io::Cursor::new(b"abcdef".to_vec()), 3);
        assert!(matches!(read, Err(ReaderFailure::LimitExceeded { cap: 3 })));
    }

    #[test]
    fn completes_under_deadline_and_captures_stdout() {
        let out = run_with_timeout(stdout_command(), Duration::from_secs(5), 1 << 20)
            .expect("fast command should complete");
        assert!(out.status.success());
        assert!(
            out.stdout.starts_with(b"hello"),
            "stdout was {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    #[test]
    fn sleeping_child_is_killed_at_the_deadline() {
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

    #[cfg(windows)]
    #[test]
    fn successful_run_disarms_job_before_closing_it() {
        let marker = unique_marker("success");
        remove_marker_artifacts(&marker);

        let output = run_with_timeout(
            descendant_marker_command(&marker, 1_000, false),
            Duration::from_secs(5),
            1 << 20,
        )
        .expect("parent should exit successfully before its detached descendant");
        assert!(output.status.success());
        assert!(
            wait_for_marker(&marker, Duration::from_secs(5)),
            "closing a successful run's Job Object must not kill its descendant"
        );

        remove_marker_artifacts(&marker);
    }

    #[cfg(windows)]
    #[test]
    fn timed_out_run_kills_job_descendants() {
        let marker = unique_marker("timeout");
        remove_marker_artifacts(&marker);

        let result = run_with_timeout(
            descendant_marker_command(&marker, 4_000, true),
            Duration::from_secs(3),
            1 << 20,
        );
        assert!(matches!(result, Err(ProcError::Timeout)));
        assert!(
            marker.with_extension("started").exists(),
            "the descendant must start before the timeout exercises Job Object termination"
        );
        thread::sleep(Duration::from_secs(5));
        assert!(
            !marker.exists(),
            "a descendant in the timed-out Job Object must not survive to write the marker"
        );

        remove_marker_artifacts(&marker);
    }

    #[cfg(unix)]
    #[test]
    fn stdout_cap_fails_without_oom_or_hang() {
        let start = Instant::now();
        let result = run_with_timeout(
            sh("head -c 1000000 /dev/zero"),
            Duration::from_secs(30),
            4096,
        );
        assert!(matches!(
            result,
            Err(ProcError::OutputLimitExceeded {
                stream: OutputStream::Stdout,
                cap: 4096
            })
        ));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "overflow must terminate the run promptly"
        );
    }

    #[test]
    fn stderr_cap_rejects_overflow() {
        let read = read_bounded(
            std::io::Cursor::new(vec![b'x'; 128 * 1024]),
            STDERR_CAP as usize,
        );
        assert!(matches!(
            read,
            Err(ReaderFailure::LimitExceeded { cap: STDERR_CAP })
        ));
    }

    #[test]
    fn bounded_reader_preserves_read_errors() {
        struct FailingReader;

        impl Read for FailingReader {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("forced read failure"))
            }
        }

        let read = read_bounded(FailingReader, 16);
        assert!(matches!(read, Err(ReaderFailure::Read(_))));
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

    #[test]
    fn spawn_detached_reports_spawn_failure() {
        let err = spawn_detached(&mut Command::new("paneflow-no-such-binary-4f2a"))
            .expect_err("a missing binary must surface as a spawn error");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[cfg(target_os = "linux")]
    fn zombie_child_count() -> usize {
        let me = std::process::id();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .filter(|entry| {
                let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                    return false;
                };
                let Some(close) = stat.rfind(')') else {
                    return false;
                };
                let mut fields = stat[close + 1..].split_whitespace();
                let state = fields.next();
                let ppid = fields.next().and_then(|value| value.parse::<u32>().ok());
                state == Some("Z") && ppid == Some(me)
            })
            .count()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn spawn_detached_reaps_short_lived_children() {
        for _ in 0..4 {
            spawn_detached(&mut Command::new("true")).expect("`true` must be spawnable");
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let zombies = zombie_child_count();
            if zombies == 0 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "spawn_detached left {zombies} zombie children unreaped"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}
