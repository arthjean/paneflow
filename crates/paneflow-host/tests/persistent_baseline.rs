#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use paneflow_host::protocol::ClientHello;
use paneflow_host::{CreateSession, HostClient, SessionId, bootstrap};
use paneflow_ipc_client::host_control::HostControl;
use paneflow_ipc_client::{IpcClient, IpcTransport};
use serde_json::{Value, json};

const SCENARIOS: [usize; 4] = [0, 1, 10, 50];
const SETTLE: Duration = Duration::from_secs(4);
const WINDOW: Duration = Duration::from_secs(10);

fn host_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_paneflow-host"))
}

fn fixture_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_paneflow-session-fixture"))
}

#[cfg(windows)]
fn allow_breakaway_like_the_desktop_does() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        assert!(!job.is_null());
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        let set = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        assert!(set != 0);
        assert!(unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) } != 0);
    });
}

#[cfg(not(windows))]
fn allow_breakaway_like_the_desktop_does() {}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn executable_identity(path: &Path) -> Value {
    let bytes = std::fs::read(path).unwrap();
    json!({
        "path": path.display().to_string(),
        "bytes": bytes.len(),
        "fnv1a64": format!("{:016x}", fnv1a64(&bytes)),
    })
}

fn diff_fingerprint() -> Value {
    let Some(root) = git(&["rev-parse", "--show-toplevel"]) else {
        return json!({"dirty": null, "note": "repository root unavailable"});
    };
    let diff = Command::new("git")
        .args(["diff", "HEAD", "--binary"])
        .current_dir(&root)
        .output();
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .current_dir(&root)
        .output();
    let (Ok(diff), Ok(untracked)) = (diff, untracked) else {
        return json!({"dirty": null, "note": "git inventory unavailable"});
    };
    if !diff.status.success() || !untracked.status.success() {
        return json!({"dirty": null, "note": "git inventory failed"});
    }
    let mut bytes = diff.stdout;
    let mut paths = Vec::new();
    for name in untracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let path = String::from_utf8_lossy(name).into_owned();
        let Ok(contents) = std::fs::read(Path::new(&root).join(&path)) else {
            return json!({"dirty": true, "note": "untracked content unreadable", "path": path});
        };
        bytes.extend_from_slice(name);
        bytes.push(0);
        bytes.extend_from_slice(&(contents.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&contents);
        paths.push(path);
    }
    json!({
        "dirty": !bytes.is_empty(),
        "fnv1a64": format!("{:016x}", fnv1a64(&bytes)),
        "bytes": bytes.len(),
        "untracked_paths": paths,
    })
}

fn toolchain() -> Value {
    let rustc = Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    json!({
        "rustc": rustc,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
    })
}

fn machine() -> Value {
    json!({
        "os": std::env::consts::OS,
        "os_build": os_build(),
        "arch": std::env::consts::ARCH,
        "cpu_model": cpu_model(),
        "logical_cpus": std::thread::available_parallelism().map(usize::from).ok(),
        "total_ram_bytes": total_ram_bytes(),
        "hostname_hash": std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .ok()
            .map(|name| format!("{:016x}", fnv1a64(name.as_bytes()))),
    })
}

fn os_build() -> Option<String> {
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("cmd");
        command
            .args(["/d", "/c", "ver"])
            .creation_flags(0x0800_0000);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new("uname");
        command.arg("-a");
        command
    };
    command
        .output()
        .ok()
        .filter(|result| result.status.success())
        .map(|result| String::from_utf8_lossy(&result.stdout).trim().to_string())
}

#[cfg(target_os = "linux")]
fn cpu_model() -> Option<String> {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()?
        .lines()
        .find(|line| line.starts_with("model name"))
        .and_then(|line| line.split(':').nth(1))
        .map(|model| model.trim().to_string())
}

#[cfg(windows)]
fn cpu_model() -> Option<String> {
    std::env::var("PROCESSOR_IDENTIFIER").ok()
}

#[cfg(target_os = "macos")]
fn cpu_model() -> Option<String> {
    let output = Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn cpu_model() -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn total_ram_bytes() -> Option<u64> {
    std::fs::read_to_string("/proc/meminfo")
        .ok()?
        .lines()
        .find(|line| line.starts_with("MemTotal:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|kib| kib.parse::<u64>().ok())
        .map(|kib| kib * 1024)
}

#[cfg(windows)]
fn total_ram_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.ullTotalPhys)
}

#[cfg(target_os = "macos")]
fn total_ram_bytes() -> Option<u64> {
    let output = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn total_ram_bytes() -> Option<u64> {
    None
}

#[derive(Debug, Clone)]
struct ThreadCpu {
    name: String,
    cpu_ns: u64,
}

#[allow(dead_code)]
enum Attribution {
    Exact(Vec<ThreadCpu>),
    Prefix15(Vec<ThreadCpu>),
    Pending(String),
}

impl Attribution {
    fn samples(&self) -> Option<&[ThreadCpu]> {
        match self {
            Self::Exact(samples) | Self::Prefix15(samples) => Some(samples),
            Self::Pending(_) => None,
        }
    }

    fn quality(&self) -> Value {
        match self {
            Self::Exact(_) => json!("exact thread names"),
            Self::Prefix15(_) => {
                json!("thread names truncated to 15 bytes by procfs comm; prefixes may merge")
            }
            Self::Pending(reason) => json!({"pending": reason}),
        }
    }
}

#[cfg(windows)]
fn thread_cpu(pid: u32) -> Attribution {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{
        GetThreadDescription, GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
    };

    fn filetime_ns(time: FILETIME) -> u64 {
        ((u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)) * 100
    }

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Attribution::Pending("CreateToolhelp32Snapshot failed".to_string());
    }
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut samples = Vec::new();
    let mut unavailable = 0usize;
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            let handle =
                unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, entry.th32ThreadID) };
            if !handle.is_null() {
                let mut creation: FILETIME = unsafe { std::mem::zeroed() };
                let mut exit: FILETIME = unsafe { std::mem::zeroed() };
                let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
                let mut user: FILETIME = unsafe { std::mem::zeroed() };
                let timed = unsafe {
                    GetThreadTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user)
                };
                let mut description: *mut u16 = std::ptr::null_mut();
                let named = unsafe { GetThreadDescription(handle, &mut description) };
                let name = if named >= 0 && !description.is_null() {
                    let text =
                        unsafe { widestring::U16CStr::from_ptr_str(description) }.to_string_lossy();
                    unsafe {
                        LocalFree(description.cast());
                    }
                    text
                } else {
                    String::new()
                };
                unsafe {
                    CloseHandle(handle);
                }
                if timed != 0 {
                    samples.push(ThreadCpu {
                        name: if name.is_empty() {
                            format!("unnamed-{}", entry.th32ThreadID)
                        } else {
                            name
                        },
                        cpu_ns: filetime_ns(kernel) + filetime_ns(user),
                    });
                } else {
                    unavailable += 1;
                }
            } else {
                unavailable += 1;
            }
        }
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe {
        CloseHandle(snapshot);
    }
    if unavailable > 0 || samples.is_empty() {
        Attribution::Pending(format!(
            "thread CPU incomplete: {} measured, {unavailable} unavailable",
            samples.len()
        ))
    } else {
        Attribution::Exact(samples)
    }
}

#[cfg(target_os = "linux")]
fn thread_cpu(pid: u32) -> Attribution {
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks_per_second <= 0 {
        return Attribution::Pending("sysconf(_SC_CLK_TCK) failed".to_string());
    }
    let ns_per_tick = 1_000_000_000u64 / ticks_per_second as u64;
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return Attribution::Pending(format!("/proc/{pid}/task is not readable"));
    };
    let mut samples = Vec::new();
    for task in tasks.flatten() {
        let dir = task.path();
        let name = std::fs::read_to_string(dir.join("comm"))
            .map(|comm| comm.trim().to_string())
            .unwrap_or_default();
        let Ok(stat) = std::fs::read_to_string(dir.join("stat")) else {
            continue;
        };
        let Some(after_name) = stat.rfind(')').map(|index| &stat[index + 1..]) else {
            continue;
        };
        let fields: Vec<&str> = after_name.split_whitespace().collect();
        let utime: u64 = fields.get(11).and_then(|v| v.parse().ok()).unwrap_or(0);
        let stime: u64 = fields.get(12).and_then(|v| v.parse().ok()).unwrap_or(0);
        samples.push(ThreadCpu {
            name,
            cpu_ns: (utime + stime) * ns_per_tick,
        });
    }
    Attribution::Prefix15(samples)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn thread_cpu(_pid: u32) -> Attribution {
    Attribution::Pending(
        "per-thread CPU attribution on this platform needs proc_pidinfo(PROC_PIDTHREADINFO); not implemented"
            .to_string(),
    )
}

#[cfg(windows)]
fn resident_bytes(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let queried = unsafe { K32GetProcessMemoryInfo(handle, &mut counters, counters.cb) };
    unsafe {
        CloseHandle(handle);
    }
    (queried != 0).then_some(counters.WorkingSetSize as u64)
}

#[cfg(target_os = "linux")]
fn resident_bytes(pid: u32) -> Option<u64> {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|kib| kib.parse::<u64>().ok())
        .map(|kib| kib * 1024)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn resident_bytes(_pid: u32) -> Option<u64> {
    None
}

fn role_of(thread_name: &str) -> &'static str {
    let roles = [
        ("paneflow-host-session", "host.session"),
        ("paneflow-host-pty-reader", "host.pty_reader"),
        ("paneflow-host-pty-writer", "host.pty_writer"),
        ("paneflow-host-viewport", "host.viewport_scan"),
        ("paneflow-host-cancellation", "host.cancellation_scan"),
        ("paneflow-host-conn", "host.ipc_connection"),
        ("paneflow-host-accept", "host.ipc_accept"),
        ("paneflow-host-launch-owner", "host.launch_owner"),
        ("paneflow-serve-core", "worker.core_link"),
        ("paneflow-ghostty-runtime", "desktop.runtime"),
        ("paneflow-ghostty-attached", "desktop.attached_runtime"),
        ("paneflow-ghostty-display", "desktop.display"),
        ("paneflow-ghostty-follower", "desktop.follower"),
        ("baseline-follower", "headless.follower"),
    ];
    if let Some((_, role)) = roles
        .iter()
        .find(|(prefix, _)| thread_name.starts_with(prefix))
    {
        return role;
    }
    let mut truncated = roles
        .iter()
        .filter(|(prefix, _)| prefix.starts_with(thread_name) && thread_name.len() >= 15);
    if let Some((_, role)) = truncated.next() {
        return if truncated.next().is_some() {
            "merged_truncated_names"
        } else {
            role
        };
    }
    if thread_name == "main" || thread_name.is_empty() {
        return "main";
    }
    "other"
}

fn attribute(before: &[ThreadCpu], after: &[ThreadCpu], window: Duration) -> Value {
    let mut by_role: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut raw: BTreeMap<String, u64> = BTreeMap::new();
    let aggregate = |samples: &[ThreadCpu]| {
        let mut names = BTreeMap::<String, u64>::new();
        for sample in samples {
            *names.entry(sample.name.clone()).or_default() += sample.cpu_ns;
        }
        names
    };
    let baseline = aggregate(before);
    for (name, cpu_ns) in aggregate(after) {
        let started = baseline.get(&name).copied().unwrap_or(0);
        let delta = cpu_ns.saturating_sub(started);
        *by_role.entry(role_of(&name)).or_default() += delta;
        raw.insert(name, delta);
    }
    let window_ns = window.as_nanos() as f64;
    let percent = |ns: u64| (ns as f64 / window_ns) * 100.0;
    json!({
        "by_role_cpu_percent": by_role.iter().map(|(role, ns)| (role.to_string(), percent(*ns))).collect::<BTreeMap<_, _>>(),
        "by_thread_cpu_ns": raw,
        "total_cpu_percent": percent(by_role.values().sum()),
    })
}

fn stamp() -> String {
    std::env::var("PANEFLOW_BENCH_STAMP").unwrap_or_else(|_| {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        format!("unix{seconds}")
    })
}

fn output_path() -> PathBuf {
    std::env::var_os("PANEFLOW_BENCH_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../bench/results")
                .join(format!("persistent-{}-local.json", stamp()))
        })
}

struct WorkerProcess {
    child: std::process::Child,
    endpoint: PathBuf,
}

impl WorkerProcess {
    fn start(home: &Path) -> Option<Self> {
        let executable = std::env::var_os("PANEFLOW_BENCH_CONTROLLER")?;
        Some(Self::start_enabled(home, executable))
    }

    fn start_enabled(home: &Path, executable: std::ffi::OsString) -> Self {
        let log = std::fs::File::create(home.join("worker-benchmark.log")).unwrap();
        let mut command = Command::new(executable);
        command
            .args(["serve", "run", "--home"])
            .arg(home)
            .env("PANEFLOW_HOME", home)
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let child = command.spawn().expect("benchmark worker starts");
        let mut worker = Self {
            child,
            endpoint: paneflow_home::serve_endpoint_path(home),
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if HostControl::connect_with_deadline(
                &worker.endpoint,
                "persistent-bench",
                Duration::from_millis(500),
            )
            .is_ok()
            {
                return worker;
            }
            assert!(worker.child.try_wait().unwrap().is_none(), "worker exited");
            assert!(Instant::now() < deadline, "worker startup watchdog");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        if let Ok(mut control) = HostControl::connect_with_deadline(
            &self.endpoint,
            "persistent-bench",
            Duration::from_secs(2),
        ) {
            let _ = control.request("worker.shutdown", json!({"drain_ms": 1000}));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

struct Follower {
    stop: Arc<AtomicBool>,
    finished: std::sync::mpsc::Receiver<Result<paneflow_host::client::OutputEnd, String>>,
    attach_ms: f64,
    checkpoint_bytes: usize,
}

struct DesktopProcess {
    child: std::process::Child,
    endpoint: PathBuf,
    restored_ms: f64,
    surfaces: Value,
}

impl DesktopProcess {
    fn start(home: &Path, sessions: &[SessionId]) -> Option<Self> {
        std::env::var_os("PANEFLOW_BENCH_DESKTOP")?;
        Some(Self::start_enabled(home, sessions))
    }

    fn start_enabled(home: &Path, sessions: &[SessionId]) -> Self {
        let executable = std::env::var_os("PANEFLOW_BENCH_CONTROLLER")
            .expect("desktop benchmark requires PANEFLOW_BENCH_CONTROLLER");
        let workspaces: Vec<_> = sessions
            .chunks(32)
            .enumerate()
            .map(|(index, sessions)| {
                json!({
                    "title": format!("Persistent baseline {index}"),
                    "cwd": home.display().to_string(),
                    "tabs": sessions.iter().map(|session| json!({
                        "title": session.as_str(),
                        "layout": {"type": "pane", "surfaces": [{"surface_type": "terminal", "session": session, "custom_name": session.as_str()}]},
                    })).collect::<Vec<_>>(),
                    "active_tab": 0,
                })
            })
            .collect();
        let saved = json!({"version": 3, "active_workspace": 0, "workspaces": workspaces});
        let _: paneflow_config::schema::SessionState =
            serde_json::from_value(saved.clone()).unwrap();
        std::fs::write(
            home.join("session.json"),
            serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
        std::fs::write(
            home.join("paneflow.json"),
            br#"{"telemetry":{"enabled":false},"terminal":{"cursor_blink":"off"}}"#,
        )
        .unwrap();
        #[cfg(windows)]
        let endpoint = PathBuf::from(format!(
            r"\\.\pipe\paneflow-persistent-bench-{}",
            std::process::id()
        ));
        #[cfg(not(windows))]
        let endpoint = home.join("desktop.sock");
        let log =
            std::fs::File::create(home.join(format!("desktop-{}.log", sessions.len()))).unwrap();
        let started = Instant::now();
        let child = Command::new(executable)
            .env("PANEFLOW_HOME", home)
            .env("PANEFLOW_SOCKET_PATH", &endpoint)
            .env(
                "PANEFLOW_UPDATE_FEED_URL",
                "http://127.0.0.1:9/fixture.json",
            )
            .env("RUST_LOG", "info")
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("native desktop starts");
        let mut desktop = Self {
            child,
            endpoint: endpoint.clone(),
            restored_ms: 0.0,
            surfaces: Value::Null,
        };
        let ipc = IpcClient::new(endpoint.clone());
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(
                desktop.child.try_wait().unwrap().is_none(),
                "desktop exited"
            );
            assert!(Instant::now() < deadline, "desktop restoration watchdog");
            if paneflow_ipc_client::socket_is_listening(&endpoint)
                && let Ok(surfaces) = ipc.call("surface.list", json!({}))
                && let Some(entries) = surfaces["surfaces"].as_array()
                && entries.len() == sessions.len()
            {
                let ready = entries.iter().all(|surface| {
                    ipc.call(
                        "surface.read",
                        json!({"surface_id": surface["surface_id"], "lines": 24}),
                    )
                    .is_ok_and(|result| result.to_string().contains("fixture idle"))
                });
                if ready {
                    desktop.restored_ms = started.elapsed().as_secs_f64() * 1000.0;
                    desktop.surfaces = surfaces;
                    return desktop;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for DesktopProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn warm_desktop_panes(
    desktop: &DesktopProcess,
    client: &mut HostClient,
    sessions: &[SessionId],
) -> Value {
    let ipc = IpcClient::new(desktop.endpoint.clone());
    let surfaces = desktop.surfaces["surfaces"].as_array().unwrap();
    assert_eq!(surfaces.len(), sessions.len());
    let started = Instant::now();
    for (surface, session) in surfaces.iter().zip(sessions) {
        let focused = ipc
            .call(
                "surface.focus",
                json!({"surface_id": surface["surface_id"]}),
            )
            .unwrap();
        assert_eq!(focused["focused"], true);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let observed = client.inspect(session).unwrap().manifest.launch;
            if observed.cols == 80 && observed.rows == 24 {
                break;
            }
            if Instant::now() >= deadline {
                return json!({"pending": "a restored pane did not adopt the calibrated 80x24 window", "session": session, "cols": observed.cols, "rows": observed.rows});
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    if let Some(first) = surfaces.first() {
        let focused = ipc
            .call("surface.focus", json!({"surface_id": first["surface_id"]}))
            .unwrap();
        assert_eq!(focused["focused"], true);
    }
    json!({"all_80x24": true, "panes": sessions.len(), "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0, "preparation": "each pane focused once through existing IPC; first pane restored visible before sampling"})
}

#[cfg(windows)]
fn fit_desktop_grid(
    desktop: &DesktopProcess,
    client: &mut HostClient,
    session: &SessionId,
) -> Value {
    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    struct Search {
        pid: u32,
        window: isize,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(isize, isize) -> i32,
            data: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(window: isize, process: *mut u32) -> u32;
        fn IsWindowVisible(window: isize) -> i32;
        fn GetWindowRect(window: isize, rect: *mut Rect) -> i32;
        fn SetWindowPos(
            window: isize,
            after: isize,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
    }
    unsafe extern "system" fn find(window: isize, data: isize) -> i32 {
        let search = unsafe { &mut *(data as *mut Search) };
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut pid);
        }
        if pid == search.pid && unsafe { IsWindowVisible(window) } != 0 {
            search.window = window;
            0
        } else {
            1
        }
    }
    let mut search = Search {
        pid: desktop.child.id(),
        window: 0,
    };
    unsafe {
        EnumWindows(find, (&raw mut search) as isize);
    }
    if search.window == 0 {
        return json!({"pending": "the owned native window was not found"});
    }
    let mut width_range = (800, 1800);
    let mut height_range = (500, 1200);
    let started = Instant::now();
    for attempt in 0..20 {
        let current = client.inspect(session).unwrap().manifest.launch;
        let mut rect = Rect::default();
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(search.window, &mut pid);
        }
        assert_eq!(
            pid,
            desktop.child.id(),
            "only the owned desktop can be resized"
        );
        assert_ne!(unsafe { GetWindowRect(search.window, &mut rect) }, 0);
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if current.cols == 80 && current.rows == 24 {
            std::thread::sleep(Duration::from_millis(250));
            let stable = client.inspect(session).unwrap().manifest.launch;
            if stable.cols == 80 && stable.rows == 24 {
                return json!({"result": "80x24 observed", "attempts": attempt, "window_width": width, "window_height": height, "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0});
            }
        }
        let next_dimension = |observed, target, current, range: &mut (i32, i32)| {
            if observed == target {
                return current;
            }
            if observed > target {
                range.1 = current - 1;
            } else {
                range.0 = current + 1;
            }
            (range.0 + (range.1 - range.0) / 2).clamp(1, 3000)
        };
        let next_width = next_dimension(current.cols, 80, width, &mut width_range);
        let next_height = next_dimension(current.rows, 24, height, &mut height_range);
        assert_ne!(
            unsafe { SetWindowPos(search.window, 0, 0, 0, next_width, next_height, 0x16) },
            0
        );
        std::thread::sleep(Duration::from_millis(250));
    }
    json!({"pending": "80x24 window calibration did not converge within twenty bounded resize attempts"})
}

#[cfg(not(windows))]
fn fit_desktop_grid(
    _desktop: &DesktopProcess,
    _client: &mut HostClient,
    _session: &SessionId,
) -> Value {
    json!({"pending": "automatic native window calibration is implemented only on Windows; observed dimensions are recorded"})
}

impl Follower {
    fn attach(endpoint: &Path, session: &SessionId) -> Self {
        let mut client =
            HostClient::connect(endpoint, &ClientHello::local("persistent-follower")).unwrap();
        let started = Instant::now();
        let attachment = client.attach(session, None).unwrap();
        let attach_ms = started.elapsed().as_secs_f64() * 1000.0;
        let checkpoint_bytes = attachment.checkpoint.snapshot.len();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let (sender, finished) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name(format!("baseline-follower-{session}"))
            .spawn(move || {
                let generation = attachment.checkpoint.generation;
                let offset = attachment.checkpoint.offset;
                let session = attachment.session.clone();
                drop(attachment);
                let result = client
                    .output(
                        &session,
                        Some(generation),
                        offset,
                        true,
                        |_, _| true,
                        || !stopping.load(Ordering::Acquire),
                    )
                    .map_err(|error| error.to_string());
                let _ = sender.send(result);
            })
            .unwrap();
        Self {
            stop,
            finished,
            attach_ms,
            checkpoint_bytes,
        }
    }

    fn finish(&self) {
        self.stop.store(true, Ordering::Release);
        self.finished
            .recv_timeout(Duration::from_secs(5))
            .expect("follower shutdown watchdog")
            .expect("follower completes");
    }
}

fn process_sample(pid: u32, before: &Attribution, after: &Attribution, window: Duration) -> Value {
    let cpu = match (before.samples(), after.samples()) {
        (Some(before), Some(after)) => attribute(before, after, window),
        _ => json!({"pending": after.quality()}),
    };
    json!({
        "pid": pid,
        "resident_bytes": resident_bytes(pid),
        "cpu": cpu,
        "attribution": after.quality(),
    })
}

fn paused_follower_probe(client: &mut HostClient, endpoint: &Path) -> Value {
    let session = client
        .create(&CreateSession {
            shell: Some(fixture_executable().display().to_string()),
            args: vec!["echo".to_string()],
            cols: Some(80),
            rows: Some(24),
            ..CreateSession::default()
        })
        .unwrap();
    let session_id = session.manifest.session;
    let mut follower =
        HostClient::connect(endpoint, &ClientHello::local("paused-follower")).unwrap();
    let attachment = follower.attach(&session_id, None).unwrap();
    let generation = attachment.checkpoint.generation;
    let offset = attachment.checkpoint.offset;
    drop(attachment);
    let (paused_tx, paused_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let followed_id = session_id.clone();
    std::thread::spawn(move || {
        let mut first = true;
        let result = follower.output(
            &followed_id,
            Some(generation),
            offset,
            true,
            |_, _| {
                if first {
                    first = false;
                    paused_tx.send(()).unwrap();
                    resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                false
            },
            || true,
        );
        let _ = done_tx.send(result.map_err(|error| error.to_string()));
    });
    client
        .input(&session_id, generation, b"paused-follower-probe\r")
        .unwrap();
    paused_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let pause_started = Instant::now();
    std::thread::sleep(Duration::from_millis(300));
    let control_started = Instant::now();
    assert!(client.inspect(&session_id).unwrap().live);
    let control_ms = control_started.elapsed().as_secs_f64() * 1000.0;
    let attach_started = Instant::now();
    client.attach(&session_id, Some(generation)).unwrap();
    let reattach_ms = attach_started.elapsed().as_secs_f64() * 1000.0;
    resume_tx.send(()).unwrap();
    let paused_ms = pause_started.elapsed().as_secs_f64() * 1000.0;
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    client.stop(&session_id, Some(generation)).unwrap();
    json!({
        "result": "pass",
        "watchdog_s": 5,
        "paused_ms": paused_ms,
        "independent_inspect_ms": control_ms,
        "independent_reattach_ms": reattach_ms,
        "scope": "one follower pauses receipt; independent real IPC inspect and attachment complete; no queue-capacity claim",
    })
}

fn compare(document: &Value) {
    let Some(path) = std::env::var_os("PANEFLOW_BENCH_BASELINE") else {
        println!("no baseline configured; nothing to compare against");
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        println!("baseline {} is not readable", Path::new(&path).display());
        return;
    };
    let Ok(baseline) = serde_json::from_slice::<Value>(&bytes) else {
        println!("baseline {} is not JSON", Path::new(&path).display());
        return;
    };
    if baseline["schema_version"] != document["schema_version"]
        || baseline["topology"] != document["topology"]
        || baseline["machine"] != document["machine"]
        || baseline["toolchain"]["profile"] != document["toolchain"]["profile"]
    {
        println!(
            "baseline topology, schema, machine, or profile differs; no performance comparison is valid"
        );
        return;
    }
    println!(
        "{:<10} {:>14} {:>14} {:>16} {:>16}",
        "sessions", "cpu% now", "cpu% base", "rss now", "rss base"
    );
    for scenario in document["scenarios"].as_array().into_iter().flatten() {
        let sessions = scenario["sessions"].as_u64().unwrap_or(0);
        let base = baseline["scenarios"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|candidate| candidate["sessions"].as_u64() == Some(sessions));
        let now_cpu = scenario["host"]["cpu"]["total_cpu_percent"].as_f64();
        let base_cpu = base.and_then(|b| b["host"]["cpu"]["total_cpu_percent"].as_f64());
        let now_rss = scenario["host"]["resident_bytes"].as_u64();
        let base_rss = base.and_then(|b| b["host"]["resident_bytes"].as_u64());
        println!(
            "{sessions:<10} {:>14} {:>14} {:>16} {:>16}",
            now_cpu.map_or("pending".to_string(), |v| format!("{v:.3}")),
            base_cpu.map_or("n/a".to_string(), |v| format!("{v:.3}")),
            now_rss.map_or("pending".to_string(), |v| v.to_string()),
            base_rss.map_or("n/a".to_string(), |v| v.to_string()),
        );
    }
}

#[test]
#[ignore = "baseline measurement; run through scripts/bench-persistent.sh or .ps1"]
fn persistent_session_baseline() {
    allow_breakaway_like_the_desktop_does();
    let source_fingerprint = diff_fingerprint();
    let home = tempfile::tempdir().unwrap();
    for (name, contents) in [
        (
            "paneflow.json",
            r#"{"telemetry":{"enabled":false},"terminal":{"cursor_blink":"off"}}"#,
        ),
        (
            "session.json",
            r#"{"version":3,"active_workspace":0,"workspaces":[]}"#,
        ),
        ("window-state.json", r#"{"width":1200,"height":800}"#),
        ("telemetry_id", "7f03d6ba-1249-4a78-92dc-96f77e8d10a2"),
    ] {
        std::fs::write(home.path().join(name), contents).unwrap();
    }
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let adoption =
        bootstrap::ensure_host_running(home.path(), &host_executable(), "persistent-bench")
            .expect("the detached host starts");
    let hello = ClientHello::local("persistent-bench");
    let mut client = HostClient::connect(&endpoint, &hello).unwrap();
    let host_pid = adoption.identity.pid;
    let worker = WorkerProcess::start(home.path());
    let worker_pid = worker.as_ref().map(|worker| worker.child.id());
    let runner_pid = std::process::id();

    let mut scenarios = Vec::new();
    let mut open: Vec<paneflow_host::SessionId> = Vec::new();
    let mut followers = Vec::new();
    for target in SCENARIOS {
        let added = target - open.len();
        let mut create_elapsed = Duration::ZERO;
        while open.len() < target {
            let create_started = Instant::now();
            let created = client
                .create(&CreateSession {
                    shell: Some(fixture_executable().display().to_string()),
                    args: vec!["idle".to_string()],
                    cwd: Some(std::env::temp_dir().display().to_string()),
                    cols: Some(80),
                    rows: Some(24),
                    ..CreateSession::default()
                })
                .expect("the fixture session starts");
            create_elapsed += create_started.elapsed();
            if std::env::var_os("PANEFLOW_BENCH_DESKTOP").is_none()
                && std::env::var_os("PANEFLOW_BENCH_NO_FOLLOWERS").is_none()
            {
                followers.push(Follower::attach(&endpoint, &created.manifest.session));
            }
            open.push(created.manifest.session);
        }
        let identities: Vec<_> = open
            .iter()
            .map(|session| client.inspect(session).unwrap().manifest)
            .collect();
        let desktop = DesktopProcess::start(home.path(), &open);
        let geometry = desktop
            .as_ref()
            .zip(open.first())
            .map(|(desktop, session)| fit_desktop_grid(desktop, &mut client, session));
        let warmed_panes = desktop
            .as_ref()
            .map(|desktop| warm_desktop_panes(desktop, &mut client, &open));
        let desktop_pid = desktop.as_ref().map(|desktop| desktop.child.id());
        std::thread::sleep(SETTLE);
        let before = thread_cpu(host_pid);
        let worker_before = worker_pid.map(thread_cpu);
        let runner_before = thread_cpu(runner_pid);
        let desktop_before = desktop_pid.map(thread_cpu);
        let window_started = Instant::now();
        std::thread::sleep(WINDOW);
        let after = thread_cpu(host_pid);
        let worker_after = worker_pid.map(thread_cpu);
        let runner_after = thread_cpu(runner_pid);
        let desktop_after = desktop_pid.map(thread_cpu);
        let window = window_started.elapsed();
        let listing_started = Instant::now();
        let listed = client.list(None).unwrap();
        let listing_elapsed = listing_started.elapsed();
        assert_eq!(listed.iter().filter(|row| row.live).count(), target);
        scenarios.push(json!({
            "sessions": target,
            "create_all_ms": create_elapsed.as_secs_f64() * 1000.0,
            "sessions_created": added,
            "create_per_session_ms": (added > 0).then(|| create_elapsed.as_secs_f64() * 1000.0 / added as f64),
            "attachments": if desktop.is_some() { target } else { followers.len() },
            "headless_follower_count": followers.len(),
            "attach_ms": followers.iter().map(|follower| follower.attach_ms).collect::<Vec<_>>(),
            "checkpoint_bytes": followers.iter().map(|follower| follower.checkpoint_bytes).collect::<Vec<_>>(),
            "list_ms": listing_elapsed.as_secs_f64() * 1000.0,
            "window_s": window.as_secs_f64(),
            "host": process_sample(host_pid, &before, &after, window),
            "worker": match (worker_pid, &worker_before, &worker_after) {
                (Some(pid), Some(before), Some(after)) => process_sample(pid, before, after, window),
                _ => json!({"pending": "PANEFLOW_BENCH_CONTROLLER was not supplied; build paneflow and pass its executable"}),
            },
            "headless_followers": process_sample(runner_pid, &runner_before, &runner_after, window),
            "desktop_mirrors": match (desktop_pid, &desktop_before, &desktop_after) {
                (Some(pid), Some(before), Some(after)) => process_sample(pid, before, after, window),
                _ => json!({"pending": "PANEFLOW_BENCH_DESKTOP was not supplied; desktop mirrors and GPUI deadlines are unmeasured"}),
            },
            "desktop_restore": desktop.as_ref().map(|desktop| json!({"ready_ms": desktop.restored_ms, "surfaces": desktop.surfaces, "proof": "every restored surface.read contains fixture idle"})),
            "desktop_geometry": geometry,
            "desktop_panes_prepared": warmed_panes,
        }));
        drop(desktop);
        let mut observed_dimensions = Vec::new();
        for before in identities {
            let after = client.inspect(&before.session).unwrap();
            assert!(after.live, "desktop detachment preserves the live child");
            assert_eq!(after.manifest.generation, before.generation);
            assert_eq!(after.manifest.process, before.process);
            observed_dimensions.push(json!({"session": before.session, "cols": after.manifest.launch.cols, "rows": after.manifest.launch.rows}));
        }
        scenarios.last_mut().unwrap()["observed_dimensions"] = json!(observed_dimensions);
    }

    let paused_follower = paused_follower_probe(&mut client, &endpoint);
    for follower in &followers {
        follower.stop.store(true, Ordering::Release);
    }
    for follower in &followers {
        follower.finish();
    }
    drop(worker);
    for session in &open {
        client.stop(session, None).unwrap();
    }
    let _ = client.call("host.shutdown", json!({}));
    drop(client);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !matches!(
        bootstrap::probe(home.path(), &endpoint, &hello),
        bootstrap::Probe::Unreachable(_)
    ) && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(50));
    }

    let document = json!({
        "suite": "paneflow-persistent-bench",
        "schema_version": 2,
        "stamp": stamp(),
        "commit": git(&["rev-parse", "HEAD"]),
        "commit_short": std::env::var("PANEFLOW_BENCH_SHA").ok(),
        "diff": source_fingerprint,
        "source_unchanged_during_measurement": source_fingerprint == diff_fingerprint(),
        "machine": machine(),
        "toolchain": toolchain(),
        "engine": adoption.identity.engine,
        "host": {"version": adoption.identity.version, "protocol": adoption.identity.protocol, "build_id": adoption.identity.build_id},
        "executables": {"host": executable_identity(&host_executable()), "fixture": executable_identity(&fixture_executable())},
        "pty": if cfg!(windows) { "ConPTY via portable-pty 0.9" } else { "posix openpty via portable-pty 0.9" },
        "seed": Value::Null,
        "seed_note": "the fixture is deterministic and the runner draws no random input",
        "invocation": {
            "test": "persistent_session_baseline",
            "fixture": fixture_executable().display().to_string(),
            "fixture_mode": "idle",
            "scenarios": SCENARIOS,
            "settle_s": SETTLE.as_secs_f64(),
            "window_s": WINDOW.as_secs_f64(),
            "args": std::env::args().collect::<Vec<_>>(),
        },
        "scenarios": scenarios,
        "faults": {"paused_follower": paused_follower},
        "topology": if std::env::var_os("PANEFLOW_BENCH_DESKTOP").is_some() { "host-worker-native-desktop" } else if std::env::var_os("PANEFLOW_BENCH_NO_FOLLOWERS").is_some() { if worker_pid.is_some() { "host-worker" } else { "host-only" } } else if worker_pid.is_some() { "host-worker-headless-followers" } else { "host-headless-followers" },
        "controller": std::env::var_os("PANEFLOW_BENCH_CONTROLLER").map(|path| executable_identity(Path::new(&path))),
        "native_environments": {"windows": cfg!(windows), "linux": cfg!(target_os = "linux"), "macos": cfg!(target_os = "macos"), "note": "false means pending, not passed"},
        "unmeasured": [
            "keystroke to pixel latency of a hosted pane (needs the desktop; see scripts/bench-terminal)",
            "reattach time after a desktop restart with 50 sessions",
            "host memory after 24 hours of idle sessions",
        ],
    });
    let path = output_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    println!("result: {}", path.display());
    compare(&document);
    assert_eq!(
        document["source_unchanged_during_measurement"], true,
        "source changed during measurement; result is not candidate-qualified"
    );
}
