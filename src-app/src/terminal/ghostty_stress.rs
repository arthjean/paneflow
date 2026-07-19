use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedReceiver;
use paneflow_config::schema::TerminalSurfaceProfile;

use super::ghostty_session::{GhosttySession, GhosttyUiEvent};
use super::pty_session::SpawnParams;
use super::types::{ShellQuoting, TerminalWindowSize};

const CYCLES: usize = 200;
const WARMUP_CYCLES: usize = 5;
#[cfg(target_os = "windows")]
const PANES: usize = 32;
// QG-007 fixes one release sample set at five warmups followed by 100
// sequential host creations on the same controlled runner.
#[cfg(target_os = "windows")]
const HOST_CREATION_WARMUP_SAMPLES: usize = 5;
#[cfg(target_os = "windows")]
const HOST_CREATION_SAMPLES: usize = 100;
#[cfg(target_os = "windows")]
const HOST_CREATION_P95_LIMIT: Duration = Duration::from_millis(500);
const RESIZES_PER_CYCLE: usize = 200;
const RESOURCE_LIMIT_PERCENT: usize = 5;
const CYCLE_TIMEOUT: Duration = Duration::from_secs(8);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(8);
const POLL_INTERVAL: Duration = Duration::from_millis(5);
#[cfg(target_os = "windows")]
const JOB_HELPER_ENV: &str = "PANEFLOW_GHOSTTY_JOB_ABORT_HELPER";
#[cfg(target_os = "windows")]
const JOB_MARKER_ENV: &str = "PANEFLOW_GHOSTTY_JOB_ABORT_MARKER";

#[derive(Clone)]
struct SpawnSpec {
    shell: &'static str,
    quoting: ShellQuoting,
    args: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitFailureKind {
    Timeout,
    DuplicateExit,
    UnexpectedRuntimeFailure,
    MissingRuntimeFailure,
    CleanupTimeout,
}

#[derive(Debug)]
struct WaitFailure {
    kind: WaitFailureKind,
    surface_id: u64,
    pid: u32,
    elapsed_ms: u128,
    exits: usize,
    runtime_failures: usize,
}

impl std::fmt::Display for WaitFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "kind={:?} surface={} pid={} elapsed_ms={} exits={} runtime_failures={}",
            self.kind,
            self.surface_id,
            self.pid,
            self.elapsed_ms,
            self.exits,
            self.runtime_failures,
        )
    }
}

#[derive(Clone, Copy, Debug)]
struct ExitObservation {
    code: i32,
    elapsed: Duration,
}

#[derive(Clone, Copy, Debug)]
struct ResourceSnapshot {
    handles: u64,
    rss: u64,
}

struct StressPane {
    surface_id: u64,
    pid: u32,
    session: GhosttySession,
    events: UnboundedReceiver<GhosttyUiEvent>,
}

impl StressPane {
    fn spawn(surface_id: u64, spec: SpawnSpec) -> Self {
        let params = SpawnParams {
            shell: spec.shell.into(),
            shell_quoting: spec.quoting,
            extra_args: spec.args,
            env: HashMap::from([
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
                ("TERM_PROGRAM".into(), "paneflow".into()),
            ]),
            cwd: std::env::current_dir()
                .unwrap_or_else(|_| panic!("scenario=spawn surface={surface_id} phase=cwd")),
            cols: 80,
            rows: 24,
            profile: TerminalSurfaceProfile::Normal,
            surface_id,
        };
        let (session, pending, events) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let spawned = session
            .start(pending, params, None, 10_000)
            .unwrap_or_else(|_| panic!("scenario=spawn surface={surface_id} phase=start"));
        assert!(
            spawned.child_pid > 0,
            "scenario=spawn surface={surface_id} phase=pid"
        );
        session.promote();
        Self {
            surface_id,
            pid: spawned.child_pid,
            session,
            events,
        }
    }

    fn resize_storm(&self) {
        for index in 0..RESIZES_PER_CYCLE {
            self.session.resize(TerminalWindowSize::new(
                1 + index % 160,
                1 + index % 80,
                8,
                16,
            ));
        }
    }

    fn write(&self, bytes: Vec<u8>) {
        assert!(
            self.session.write(bytes).is_sent(),
            "scenario=write surface={} pid={} phase=admission",
            self.surface_id,
            self.pid,
        );
    }

    #[cfg(target_os = "windows")]
    fn output_contains(&self, marker: &str) -> bool {
        self.session
            .recent_output_lines()
            .iter()
            .any(|line| line.contains(marker))
    }

    #[cfg(target_os = "windows")]
    fn wait_for_marker(&self, marker: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.output_contains(marker) {
                return true;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        false
    }

    fn wait_for_exit(
        &mut self,
        timeout: Duration,
        expect_runtime_failure: bool,
    ) -> Result<ExitObservation, WaitFailure> {
        let started = Instant::now();
        let deadline = started + timeout;
        let mut exits = 0usize;
        let mut runtime_failures = 0usize;
        let mut code = -1;

        while Instant::now() < deadline && exits == 0 {
            while let Ok(event) = self.events.try_recv() {
                match event {
                    GhosttyUiEvent::ChildExited {
                        code: exit_code, ..
                    } => {
                        exits += 1;
                        code = exit_code;
                    }
                    GhosttyUiEvent::RuntimeFailed(_) => runtime_failures += 1,
                    _ => {}
                }
            }
            if exits == 0 {
                std::thread::sleep(POLL_INTERVAL);
            }
        }
        while let Ok(event) = self.events.try_recv() {
            match event {
                GhosttyUiEvent::ChildExited {
                    code: exit_code, ..
                } => {
                    exits += 1;
                    code = exit_code;
                }
                GhosttyUiEvent::RuntimeFailed(_) => runtime_failures += 1,
                _ => {}
            }
        }

        if exits == 0 {
            self.session.shutdown();
            let cleanup_succeeded =
                wait_process_inactive(self.pid, Instant::now() + CLEANUP_TIMEOUT);
            return Err(WaitFailure {
                kind: if cleanup_succeeded {
                    WaitFailureKind::Timeout
                } else {
                    WaitFailureKind::CleanupTimeout
                },
                surface_id: self.surface_id,
                pid: self.pid,
                elapsed_ms: started.elapsed().as_millis(),
                exits,
                runtime_failures,
            });
        }

        self.session.shutdown();
        if !wait_process_inactive(self.pid, Instant::now() + CLEANUP_TIMEOUT) {
            return Err(WaitFailure {
                kind: WaitFailureKind::CleanupTimeout,
                surface_id: self.surface_id,
                pid: self.pid,
                elapsed_ms: started.elapsed().as_millis(),
                exits,
                runtime_failures,
            });
        }
        let kind = if exits != 1 {
            Some(WaitFailureKind::DuplicateExit)
        } else if expect_runtime_failure && runtime_failures == 0 {
            Some(WaitFailureKind::MissingRuntimeFailure)
        } else if !expect_runtime_failure && runtime_failures != 0 {
            Some(WaitFailureKind::UnexpectedRuntimeFailure)
        } else {
            None
        };
        if let Some(kind) = kind {
            return Err(WaitFailure {
                kind,
                surface_id: self.surface_id,
                pid: self.pid,
                elapsed_ms: started.elapsed().as_millis(),
                exits,
                runtime_failures,
            });
        }
        Ok(ExitObservation {
            code,
            elapsed: started.elapsed(),
        })
    }
}

impl Drop for StressPane {
    fn drop(&mut self) {
        self.session.shutdown();
        let _ = wait_process_inactive(self.pid, Instant::now() + CLEANUP_TIMEOUT);
    }
}

#[cfg(target_os = "linux")]
fn cycle_spec() -> SpawnSpec {
    SpawnSpec {
        shell: "/bin/sh",
        quoting: ShellQuoting::Posix,
        args: vec![
            "-c".into(),
            "IFS= read -r line; printf 'PANEFLOW_STRESS:%s\\n' \"$line\"".into(),
        ],
    }
}

#[cfg(target_os = "windows")]
fn cycle_spec() -> SpawnSpec {
    SpawnSpec {
        shell: "cmd.exe",
        quoting: ShellQuoting::Cmd,
        args: vec![
            "/D".into(),
            "/Q".into(),
            "/V:ON".into(),
            "/C".into(),
            "set /p PANEFLOW_LINE= & echo PANEFLOW_STRESS:!PANEFLOW_LINE!".into(),
        ],
    }
}

#[cfg(target_os = "windows")]
fn blocked_spec() -> SpawnSpec {
    SpawnSpec {
        shell: "cmd.exe",
        quoting: ShellQuoting::Cmd,
        args: vec!["/D".into(), "/Q".into(), "/K".into()],
    }
}

#[cfg(target_os = "windows")]
fn burst_spec() -> SpawnSpec {
    SpawnSpec {
        shell: "cmd.exe",
        quoting: ShellQuoting::Cmd,
        args: vec![
            "/D".into(),
            "/Q".into(),
            "/K".into(),
            "for /L %i in (1,1,512) do @echo PANEFLOW_BURST".into(),
        ],
    }
}

#[cfg(target_os = "windows")]
fn immediate_exit_spec() -> SpawnSpec {
    SpawnSpec {
        shell: "cmd.exe",
        quoting: ShellQuoting::Cmd,
        args: vec!["/D".into(), "/Q".into(), "/C".into(), "exit /b 7".into()],
    }
}

#[cfg(target_os = "windows")]
fn descendant_spec() -> SpawnSpec {
    SpawnSpec {
        shell: "powershell.exe",
        quoting: ShellQuoting::PowerShell,
        args: vec![
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "$p = Start-Process -FilePath 'cmd.exe' -ArgumentList '/D','/Q','/K' -WindowStyle Hidden -PassThru; Wait-Process -Id $p.Id".into(),
        ],
    }
}

fn run_cycle(surface_id: u64) -> (Duration, usize) {
    let mut pane = StressPane::spawn(surface_id, cycle_spec());
    let descendants = descendant_pids(pane.pid);
    let output_before = pane.session.processed_output_bytes_for_test();
    pane.resize_storm();
    pane.write(format!("cycle-{surface_id}\r").into_bytes());
    let observation = pane
        .wait_for_exit(CYCLE_TIMEOUT, false)
        .unwrap_or_else(|failure| panic!("scenario=cycle failure={failure}"));
    assert_eq!(
        observation.code, 0,
        "scenario=cycle surface={surface_id} pid={} phase=exit_code",
        pane.pid,
    );
    let output_after = pane.session.processed_output_bytes_for_test();
    assert!(
        output_after > output_before,
        "scenario=cycle surface={surface_id} pid={} phase=output bytes_before={output_before} bytes_after={output_after}",
        pane.pid,
    );
    for descendant in &descendants {
        assert!(
            !process_active(*descendant),
            "scenario=cycle surface={surface_id} pid={} descendant={} phase=cleanup",
            pane.pid,
            descendant,
        );
    }
    (observation.elapsed, descendants.len())
}

#[cfg(target_os = "windows")]
fn measure_host_creation(surface_id: u64) -> Duration {
    let started = Instant::now();
    let mut pane = StressPane::spawn(surface_id, blocked_spec());
    let elapsed = started.elapsed();
    // End the shell through its PTY so cleanup observes a normal child exit.
    // Forcing shutdown here can race the reader and misclassify our own
    // teardown as a runtime failure after a valid host-creation sample.
    pane.write(b"exit\r".to_vec());
    pane.wait_for_exit(CYCLE_TIMEOUT, false)
        .unwrap_or_else(|failure| panic!("scenario=host_creation failure={failure}"));
    elapsed
}

#[cfg(target_os = "windows")]
fn process_active(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};

    const SYNCHRONIZE: u32 = 0x0010_0000;
    if pid == 0 {
        return false;
    }
    // SAFETY: the handle is read-only for synchronization and closed below.
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return false;
    }
    // SAFETY: `handle` is valid and the zero timeout is non-blocking.
    let wait = unsafe { WaitForSingleObject(handle, 0) };
    // SAFETY: close the handle exactly once.
    unsafe {
        CloseHandle(handle);
    }
    wait != WAIT_OBJECT_0
}

#[cfg(target_os = "linux")]
fn process_active(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only probes process existence.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn wait_process_inactive(pid: u32, deadline: Instant) -> bool {
    while Instant::now() < deadline {
        if !process_active(pid) {
            return true;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    !process_active(pid)
}

#[cfg(target_os = "windows")]
fn process_entries() -> Vec<(u32, u32)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: the snapshot handle is closed on every valid-handle path.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut entries = Vec::with_capacity(256);
    // SAFETY: the Win32 structure is zero-initialized and its size set before use.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: snapshot and entry satisfy the ToolHelp iteration contract.
    if unsafe { Process32FirstW(snapshot, &mut entry) } != 0 {
        loop {
            entries.push((entry.th32ProcessID, entry.th32ParentProcessID));
            // SAFETY: same snapshot and initialized entry as above.
            if unsafe { Process32NextW(snapshot, &mut entry) } == 0 {
                break;
            }
        }
    }
    // SAFETY: close the snapshot exactly once.
    unsafe {
        CloseHandle(snapshot);
    }
    entries
}

#[cfg(target_os = "windows")]
fn descendant_pids(root_pid: u32) -> Vec<u32> {
    fn visit(
        pid: u32,
        entries: &[(u32, u32)],
        seen: &mut std::collections::HashSet<u32>,
        output: &mut Vec<u32>,
    ) {
        for child in entries
            .iter()
            .filter_map(|(child, parent)| (*parent == pid).then_some(*child))
        {
            if seen.insert(child) {
                visit(child, entries, seen, output);
                output.push(child);
            }
        }
    }

    let entries = process_entries();
    let mut seen = std::collections::HashSet::new();
    let mut output = Vec::new();
    visit(root_pid, &entries, &mut seen, &mut output);
    output
}

#[cfg(target_os = "linux")]
fn descendant_pids(_root_pid: u32) -> Vec<u32> {
    Vec::new()
}

#[cfg(target_os = "windows")]
fn wait_for_descendants(root_pid: u32, deadline: Instant) -> Vec<u32> {
    while Instant::now() < deadline {
        let descendants = descendant_pids(root_pid);
        if !descendants.is_empty() {
            return descendants;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Vec::new()
}

#[cfg(target_os = "windows")]
fn resource_snapshot() -> ResourceSnapshot {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};

    let mut handles = 0u32;
    // SAFETY: pseudo handle is always valid; output pointer references initialized storage.
    let handle_result = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut handles) };
    assert_ne!(
        handle_result,
        0,
        "scenario=resources phase=handles os_error={:?}",
        std::io::Error::last_os_error().raw_os_error(),
    );
    // SAFETY: zeroed C POD with the documented byte size passed to the API.
    let mut memory: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    memory.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: pseudo handle and writable counter buffer satisfy the API contract.
    let memory_result = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut memory,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    assert_ne!(
        memory_result,
        0,
        "scenario=resources phase=rss os_error={:?}",
        std::io::Error::last_os_error().raw_os_error(),
    );
    ResourceSnapshot {
        handles: u64::from(handles),
        rss: u64::try_from(memory.WorkingSetSize).unwrap_or(u64::MAX),
    }
}

#[cfg(target_os = "linux")]
fn resource_snapshot() -> ResourceSnapshot {
    ResourceSnapshot {
        handles: std::fs::read_dir("/proc/self/fd")
            .map(|entries| entries.count() as u64)
            .unwrap_or(0),
        rss: super::backend_corpus::resident_set_bytes(),
    }
}

fn resources_within_budget(baseline: ResourceSnapshot, current: ResourceSnapshot) -> bool {
    let limits = resource_limits(baseline);
    current.handles <= limits.handles && current.rss <= limits.rss
}

fn resource_limits(baseline: ResourceSnapshot) -> ResourceSnapshot {
    ResourceSnapshot {
        handles: baseline
            .handles
            .saturating_add(baseline.handles.saturating_sub(1) / 20),
        rss: baseline
            .rss
            .saturating_add(baseline.rss.saturating_sub(1) / 20),
    }
}

fn wait_for_resource_recovery(baseline: ResourceSnapshot) -> ResourceSnapshot {
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    let mut current = resource_snapshot();
    while Instant::now() < deadline && !resources_within_budget(baseline, current) {
        std::thread::sleep(Duration::from_millis(20));
        current = resource_snapshot();
    }
    current
}

fn assert_resource_recovery(
    scenario: &'static str,
    baseline: ResourceSnapshot,
    current: ResourceSnapshot,
) {
    let limits = resource_limits(baseline);
    assert!(
        resources_within_budget(baseline, current),
        "scenario={scenario} phase=resources handles_start={} handles_end={} rss_start={} rss_end={} handle_limit={} rss_limit={}",
        baseline.handles,
        current.handles,
        baseline.rss,
        current.rss,
        limits.handles,
        limits.rss,
    );
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "EP-004 performance gate: 5 warmups and 100 sequential ConPTY host creations"]
#[allow(
    clippy::assertions_on_constants,
    reason = "the ignored performance gate must reject accidental debug-profile execution"
)]
fn windows_ghostty_host_creation_performance_gate() {
    assert!(
        !cfg!(debug_assertions),
        "run the host creation performance gate in release"
    );
    for warmup in 0..HOST_CREATION_WARMUP_SAMPLES {
        let _ = measure_host_creation(40_000 + warmup as u64);
    }
    let mut durations = Vec::with_capacity(HOST_CREATION_SAMPLES);
    for sample in 0..HOST_CREATION_SAMPLES {
        durations.push(measure_host_creation(
            40_000 + HOST_CREATION_WARMUP_SAMPLES as u64 + sample as u64,
        ));
    }
    durations.sort_unstable();
    let median = super::backend_corpus::percentile_duration(&durations, 50);
    let p95 = super::backend_corpus::percentile_duration(&durations, 95);
    println!(
        "{{\"scenario\":\"windows_ghostty_host_creation\",\"warmup_samples\":{HOST_CREATION_WARMUP_SAMPLES},\"samples\":{HOST_CREATION_SAMPLES},\"median_us\":{},\"p95_us\":{},\"p95_limit_ms\":{},\"profile\":\"release\"}}",
        median.as_micros(),
        p95.as_micros(),
        HOST_CREATION_P95_LIMIT.as_millis(),
    );
    assert!(
        p95 < HOST_CREATION_P95_LIMIT,
        "Ghostty host creation p95 {} ms must remain below {} ms",
        p95.as_secs_f64() * 1_000.0,
        HOST_CREATION_P95_LIMIT.as_millis(),
    );
}

#[test]
#[ignore = "EP-004 promotion gate: 200 PTY cycles with 200 resizes each"]
fn ghostty_spawn_resize_close_stress_has_no_residual_growth() {
    for warmup in 0..WARMUP_CYCLES {
        let _ = run_cycle(warmup as u64);
    }
    let baseline = resource_snapshot();
    let started = Instant::now();
    let mut max_cycle = Duration::ZERO;
    let mut cycle_durations = Vec::with_capacity(CYCLES);
    let mut descendants_observed = 0usize;
    for cycle in 0..CYCLES {
        let (duration, descendants) = run_cycle((cycle + WARMUP_CYCLES) as u64);
        max_cycle = max_cycle.max(duration);
        cycle_durations.push(duration);
        descendants_observed = descendants_observed.saturating_add(descendants);
    }
    let recovered = wait_for_resource_recovery(baseline);
    let elapsed = started.elapsed();
    let limits = resource_limits(baseline);
    cycle_durations.sort_unstable();
    println!(
        "{{\"scenario\":\"ghostty_spawn_resize_close\",\"warmup_cycles\":{WARMUP_CYCLES},\"cycles\":{CYCLES},\"resizes_per_cycle\":{RESIZES_PER_CYCLE},\"descendants_observed\":{descendants_observed},\"campaign_ms\":{},\"cycle_median_us\":{},\"cycle_p95_us\":{},\"max_cycle_ms\":{},\"handles_baseline\":{},\"handles_end\":{},\"handles_limit\":{},\"rss_baseline_bytes\":{},\"rss_end_bytes\":{},\"rss_limit_bytes\":{},\"resource_limit_percent\":{RESOURCE_LIMIT_PERCENT}}}",
        elapsed.as_millis(),
        super::backend_corpus::percentile_us(&cycle_durations, 50),
        super::backend_corpus::percentile_us(&cycle_durations, 95),
        max_cycle.as_millis(),
        baseline.handles,
        recovered.handles,
        limits.handles,
        baseline.rss,
        recovered.rss,
        limits.rss,
    );
    assert_resource_recovery("cycles", baseline, recovered);
    assert!(
        max_cycle <= CYCLE_TIMEOUT,
        "scenario=cycles phase=duration total_ms={} max_cycle_ms={} descendants={descendants_observed}",
        elapsed.as_millis(),
        max_cycle.as_millis(),
    );
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "EP-004 promotion gate: 32 concurrent ConPTY panes"]
fn windows_ghostty_32_pane_resize_and_close_orders_are_bounded() {
    for warmup in 0..WARMUP_CYCLES {
        let _ = run_cycle(30_000 + warmup as u64);
    }
    let baseline = resource_snapshot();
    let started = Instant::now();
    let mut panes = (0..PANES)
        .map(|index| StressPane::spawn(10_000 + index as u64, burst_spec()))
        .collect::<Vec<_>>();
    let descendants = panes
        .iter()
        .flat_map(|pane| descendant_pids(pane.pid))
        .collect::<Vec<_>>();
    let descendants_observed = descendants.len();
    let mut close_durations = Vec::with_capacity(PANES);
    for pane in &panes {
        pane.resize_storm();
    }
    let first_close_order = (0..PANES).step_by(2).collect::<Vec<_>>();
    let survivor_close_order = (1..PANES).rev().step_by(2).collect::<Vec<_>>();
    for index in &first_close_order {
        panes[*index].session.shutdown();
    }
    for index in first_close_order {
        let observation = panes[index]
            .wait_for_exit(CYCLE_TIMEOUT, false)
            .unwrap_or_else(|failure| panic!("scenario=panes32 failure={failure}"));
        close_durations.push(observation.elapsed);
    }
    for index in &survivor_close_order {
        assert!(
            process_active(panes[*index].pid),
            "scenario=panes32 survivor={} pid={} phase=isolation",
            index,
            panes[*index].pid,
        );
    }
    for index in &survivor_close_order {
        panes[*index].session.shutdown();
    }
    for index in survivor_close_order {
        let observation = panes[index]
            .wait_for_exit(CYCLE_TIMEOUT, false)
            .unwrap_or_else(|failure| panic!("scenario=panes32 failure={failure}"));
        close_durations.push(observation.elapsed);
    }
    drop(panes);
    for pid in descendants {
        assert!(
            wait_process_inactive(pid, Instant::now() + CLEANUP_TIMEOUT),
            "scenario=panes32 descendant={pid} phase=cleanup"
        );
    }
    let recovered = wait_for_resource_recovery(baseline);
    let elapsed = started.elapsed();
    let limits = resource_limits(baseline);
    close_durations.sort_unstable();
    println!(
        "{{\"scenario\":\"windows_ghostty_32_panes\",\"warmup_cycles\":{WARMUP_CYCLES},\"panes\":{PANES},\"resizes_per_pane\":{RESIZES_PER_CYCLE},\"descendants_observed\":{descendants_observed},\"campaign_ms\":{},\"close_median_us\":{},\"close_p95_us\":{},\"handles_baseline\":{},\"handles_end\":{},\"handles_limit\":{},\"rss_baseline_bytes\":{},\"rss_end_bytes\":{},\"rss_limit_bytes\":{},\"resource_limit_percent\":{RESOURCE_LIMIT_PERCENT}}}",
        elapsed.as_millis(),
        super::backend_corpus::percentile_us(&close_durations, 50),
        super::backend_corpus::percentile_us(&close_durations, 95),
        baseline.handles,
        recovered.handles,
        limits.handles,
        baseline.rss,
        recovered.rss,
        limits.rss,
    );
    assert_resource_recovery("panes32", baseline, recovered);
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "EP-004 promotion gate: Windows lifecycle scenario matrix"]
fn windows_ghostty_lifecycle_scenario_matrix_is_bounded() {
    let mut immediate = StressPane::spawn(20_001, immediate_exit_spec());
    let immediate_exit = immediate
        .wait_for_exit(CYCLE_TIMEOUT, false)
        .unwrap_or_else(|failure| panic!("scenario=immediate failure={failure}"));
    assert_eq!(
        immediate_exit.code, 7,
        "scenario=immediate pid={} phase=exit_code",
        immediate.pid
    );

    let mut blocked = StressPane::spawn(20_002, blocked_spec());
    blocked.session.shutdown();
    blocked
        .wait_for_exit(CYCLE_TIMEOUT, false)
        .unwrap_or_else(|failure| panic!("scenario=blocked failure={failure}"));

    let mut descendant = StressPane::spawn(20_003, descendant_spec());
    let descendant_pids = wait_for_descendants(descendant.pid, Instant::now() + CYCLE_TIMEOUT);
    assert!(
        !descendant_pids.is_empty(),
        "scenario=descendant pid={} phase=spawn",
        descendant.pid
    );
    descendant.session.shutdown();
    descendant
        .wait_for_exit(CYCLE_TIMEOUT, false)
        .unwrap_or_else(|failure| panic!("scenario=descendant failure={failure}"));
    for pid in descendant_pids {
        assert!(
            wait_process_inactive(pid, Instant::now() + CLEANUP_TIMEOUT),
            "scenario=descendant descendant={pid} phase=cleanup"
        );
    }

    let mut ctrl_c = StressPane::spawn(20_004, blocked_spec());
    ctrl_c.write(b"@echo off\rping -t 127.0.0.1 >NUL\r".to_vec());
    std::thread::sleep(Duration::from_millis(100));
    ctrl_c.write(vec![0x03]);
    ctrl_c.write(b"echo PANEFLOW_CTRL_C_OK\rexit\r".to_vec());
    assert!(
        ctrl_c.wait_for_marker("PANEFLOW_CTRL_C_OK", CYCLE_TIMEOUT),
        "scenario=ctrl_c pid={} phase=recovery",
        ctrl_c.pid
    );
    ctrl_c
        .wait_for_exit(CYCLE_TIMEOUT, false)
        .unwrap_or_else(|failure| panic!("scenario=ctrl_c failure={failure}"));

    let mut worker_failure = StressPane::spawn(20_005, blocked_spec());
    assert!(
        worker_failure.session.simulate_worker_crash_for_test(),
        "scenario=worker_failure pid={} phase=inject",
        worker_failure.pid
    );
    worker_failure
        .wait_for_exit(CYCLE_TIMEOUT, true)
        .unwrap_or_else(|failure| panic!("scenario=worker_failure failure={failure}"));

    let mut timeout = StressPane::spawn(20_006, blocked_spec());
    let failure = timeout
        .wait_for_exit(Duration::from_millis(25), false)
        .expect_err("blocked pane must exercise timeout cleanup");
    assert_eq!(failure.kind, WaitFailureKind::Timeout);
    assert!(
        !process_active(timeout.pid),
        "scenario=timeout pid={} phase=cleanup",
        timeout.pid
    );
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "helper subprocess for abrupt Job Object cleanup"]
fn ghostty_job_object_abort_helper() {
    if std::env::var_os(JOB_HELPER_ENV).is_none() {
        return;
    }
    if crate::agents::parent_guard::install_process_job().is_err() {
        std::process::exit(77);
    }
    let marker = std::env::var_os(JOB_MARKER_ENV).unwrap_or_else(|| std::process::exit(78));
    let pane = StressPane::spawn(30_001, blocked_spec());
    if std::fs::write(marker, pane.pid.to_string()).is_err() {
        std::process::exit(78);
    }
    std::process::abort();
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "EP-004 promotion gate: abrupt app close cleans ConPTY Job Object"]
fn windows_ghostty_job_object_abrupt_cleanup() {
    use std::process::{Command, Stdio};

    let temp = tempfile::tempdir().expect("scenario=job_object phase=tempdir");
    let marker = temp.path().join("child.pid");
    let status =
        Command::new(std::env::current_exe().expect("scenario=job_object phase=current_exe"))
            .arg("--ignored")
            .arg("ghostty_job_object_abort_helper")
            .arg("--test-threads=1")
            .env(JOB_HELPER_ENV, "1")
            .env(JOB_MARKER_ENV, &marker)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("scenario=job_object phase=helper_spawn");
    if status.code() == Some(77) {
        eprintln!("scenario=job_object status=skipped reason=nested_job_denied");
        return;
    }
    assert!(
        !status.success(),
        "scenario=job_object phase=helper_abort status={:?}",
        status.code()
    );
    let pid = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or_else(|| {
            panic!(
                "scenario=job_object phase=marker status={:?}",
                status.code()
            )
        });
    assert!(
        wait_process_inactive(pid, Instant::now() + CLEANUP_TIMEOUT),
        "scenario=job_object pid={pid} phase=cleanup"
    );
}
