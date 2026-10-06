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

#[path = "persistent_baseline/ab.rs"]
mod ab;
#[path = "persistent_baseline/active.rs"]
mod active;
#[path = "persistent_baseline/endurance.rs"]
mod endurance;
#[path = "persistent_baseline/gate_runs.rs"]
mod gate_runs;
#[path = "persistent_baseline/gates.rs"]
mod gates;
#[path = "persistent_baseline/hardware.rs"]
mod hardware;
#[path = "persistent_baseline/headless.rs"]
mod headless;
#[path = "persistent_baseline/metrics.rs"]
mod metrics;
#[path = "persistent_baseline/processes.rs"]
mod processes;
#[path = "persistent_baseline/provenance.rs"]
mod provenance;
#[path = "persistent_baseline/report.rs"]
mod report;
#[path = "persistent_baseline/workloads.rs"]
mod workloads;

use endurance::*;
use metrics::*;
use processes::*;
use provenance::*;
use report::*;

use workloads::{Decision, FixtureLedger, automated_only, seed_failure, verdict};

pub const SCHEMA_VERSION: u64 = 4;

const SCENARIOS: [usize; 4] = [0, 1, 10, 50];
const SETTLE: Duration = Duration::from_secs(4);
const WINDOW: Duration = Duration::from_secs(10);

fn packaged_or_built(variable: &str, built: &str) -> PathBuf {
    std::env::var_os(variable).map_or_else(|| PathBuf::from(built), PathBuf::from)
}

fn host_executable() -> PathBuf {
    packaged_or_built("PANEFLOW_BENCH_HOST", env!("CARGO_BIN_EXE_paneflow-host"))
}

fn fixture_executable() -> PathBuf {
    packaged_or_built(
        "PANEFLOW_BENCH_FIXTURE",
        env!("CARGO_BIN_EXE_paneflow-session-fixture"),
    )
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

#[test]
fn a_seeded_failure_fails_the_run_and_retains_its_artifact() {
    let mut decisions = vec![workloads::decide(
        "NFR-09.attach_p95",
        "W02",
        "sequential reattachment p95 ms",
        "<= 1000",
        Some(12.0),
        |v| v <= 1000.0,
        "",
    )];
    assert!(verdict(&decisions, &[]).is_ok());
    decisions.push(Decision {
        id: "SEEDED".to_string(),
        workload: "harness",
        metric: "seeded known failure".to_string(),
        threshold: "never passes".to_string(),
        observed: json!("test"),
        result: "fail",
        reason: "seeded".to_string(),
    });
    let failures = verdict(&decisions, &[]).unwrap_err();
    assert_eq!(failures.len(), 1);
    assert!(failures[0].starts_with("SEEDED"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("persistent-seeded.json");
    let document = json!({
        "schema_version": SCHEMA_VERSION,
        "thresholds": decisions.iter().map(Decision::to_json).collect::<Vec<_>>(),
    });
    write_document(&path, &document);
    let retained: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(retained["thresholds"][1]["result"], "fail");
    let prior = vec![retained["thresholds"][1].clone()];
    let passing = vec![decisions[0].clone()];
    let retried = verdict(&passing, &prior).unwrap_err();
    assert!(
        retried[0].starts_with("retained first failure"),
        "a retry keeps the first failure: {retried:?}"
    );
}

fn seed_home(home: &Path) {
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
        std::fs::write(home.join(name), contents).unwrap();
    }
}

fn shutdown_host(
    decisions: &mut Vec<Decision>,
    client: HostClient,
    host_identity: &paneflow_host::ProcessIdentity,
    home: &Path,
    endpoint: &Path,
    hello: &ClientHello,
    ledger: &FixtureLedger,
) -> Value {
    let mut client = client;
    let shutdown = client.call("host.shutdown", json!({}));
    drop(client);
    let deadline = Instant::now() + Duration::from_secs(10);
    while (host_identity.is_provably_live()
        || !matches!(
            bootstrap::probe(home, endpoint, hello),
            bootstrap::Probe::Unreachable(_)
        ))
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    let host_exited = !host_identity.is_provably_live();
    decisions.push(workloads::decide(
        "NFR-12.host_shutdown",
        "harness",
        "host process exited after an acknowledged shutdown",
        "acknowledged and exited within 10 s",
        Some(if shutdown.is_ok() && host_exited {
            1.0
        } else {
            0.0
        }),
        |v| v == 1.0,
        "",
    ));
    let survivors = ledger.survivors();
    decisions.push(workloads::decide(
        "NFR-12.fixture_orphans",
        "harness",
        "fixture processes still alive after host shutdown",
        "== 0",
        Some(survivors.len() as f64),
        |v| v == 0.0,
        "",
    ));
    json!({
        "owned": ledger.len(),
        "survivors_after_shutdown": survivors,
        "host_shutdown": shutdown.as_ref().map(|value| value.clone()).unwrap_or_else(|error| json!({"error": error.to_string()})),
        "host_exited": host_exited,
        "cleanup": "only recorded session/process identities are checked; the host owns and stops its fixtures on a private PANEFLOW_HOME",
    })
}

#[test]
#[ignore = "baseline measurement; run through scripts/bench-persistent.sh or .ps1"]
fn persistent_session_baseline() {
    allow_breakaway_like_the_desktop_does();
    let source_fingerprint = diff_fingerprint();
    let home = tempfile::tempdir().unwrap();
    seed_home(home.path());
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let adoption =
        bootstrap::ensure_host_running(home.path(), &host_executable(), "persistent-bench")
            .expect("the detached host starts");
    let hello = ClientHello::local("persistent-bench");
    let mut client = HostClient::connect(&endpoint, &hello).unwrap();
    let host_pid = adoption.identity.pid;
    let host_identity = paneflow_host::ProcessIdentity::capture(host_pid);
    let mut worker = WorkerProcess::start(home.path());
    let worker_pid = worker.as_ref().map(|worker| worker.child.id());
    let runner_pid = std::process::id();
    let protocol = workloads::protocol();
    let ledger = FixtureLedger::new();
    let mut decisions: Vec<Decision> = Vec::new();

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
            ledger.record(&mut client, &created.manifest.session);
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
    let w04_worker = workloads::workload_worker_replacement(
        home.path(),
        &mut worker,
        &mut client,
        &endpoint,
        &open,
        &ledger,
        &protocol,
        &mut decisions,
    );
    for follower in &followers {
        follower.stop.store(true, Ordering::Release);
    }
    for follower in &followers {
        follower.finish();
    }
    for session in &open {
        client.stop(session, None).unwrap();
    }
    let w02 = workloads::workload_history(&mut client, &endpoint, &ledger, &mut decisions);
    let baseline_topology = json!({
        "schema_version": SCHEMA_VERSION,
        "topology": topology_label(worker_pid.is_some()),
        "machine": machine(),
        "toolchain": toolchain(),
    });
    let w03 = workloads::workload_throughput(
        &mut client,
        &endpoint,
        &ledger,
        &protocol,
        baseline_throughput(&baseline_topology),
        &mut decisions,
    );
    let w05 = workloads::workload_churn(
        &mut client,
        &endpoint,
        host_pid,
        &ledger,
        &protocol,
        &mut decisions,
    );
    drop(worker);
    let fixtures = shutdown_host(
        &mut decisions,
        client,
        &host_identity,
        home.path(),
        &endpoint,
        &hello,
        &ledger,
    );
    seed_failure(&mut decisions);
    let prior = workloads::prior_failures();
    let workloads_json = json!({
        "W01": {"status": "measured", "scenarios": "see scenarios[]", "topology": topology_label(worker_pid.is_some()), "note": "short-window CPU and memory samples; this window decides NFR-01"},
        "W02": w02,
        "W03": w03,
        "W04": {
            "worker_replacement": w04_worker,
            "paused_follower": paused_follower,
            "saved_layout_restoration_after_host_loss": automated_only("W04", &["paneflow-app terminal::host_link::tests::every_retained_end_state_restores_without_creating_a_process", "paneflow-app app::hosted_sessions::tests::a_pane_restored_into_an_ended_session_resumes_without_an_attachment"], &[]),
            "control_disconnect_mid_paste": automated_only("W04", &["paneflow-host server::tests::a_connection_lost_before_the_input_ack_reports_unknown_delivery_without_a_resend", "paneflow-host server::tests::repeated_disconnects_deliver_each_input_once_and_release_every_connection_thread"], &[]),
            "output_eviction_and_generation_change": automated_only("W04", &["paneflow-host server::tests::a_follower_resumes_after_the_checkpoint_survives_idle_keepalives_and_sees_the_exit", "paneflow-app terminal::ghostty_session::tests::repeated_attach_and_detach_with_filled_scrollback_retains_no_checkpoint_bytes"], &[]),
            "gui_force_quit": automated_only("W04", &[], &["desktop forced termination with live sessions: qualification runbook cell D-11"]),
        },
        "W05": w05,
        "W06": {
            "storage_faults": automated_only("W06", &["paneflow-host persistence::tests::a_failed_final_revision_is_retained_and_retried_after_storage_recovers", "paneflow-host persistence::tests::a_stalled_exclusive_job_times_out_the_waiter_without_losing_the_queue", "paneflow-host persistence::tests::metadata_admission_respects_the_byte_budget_and_reservations", "paneflow-host host::tests::a_restart_whose_persist_fails_restores_the_prior_record_without_a_stranded_start", "paneflow-host host::tests::shutdown_keeps_unsaved_final_state_owned_until_retry_succeeds", "paneflow-host host::tests::a_duplicate_hook_retries_failed_seed_persistence_without_another_notification"], &[]),
            "lifecycle_faults": automated_only("W06", &["paneflow-host host::tests::a_stop_during_the_launch_terminates_the_child_instead_of_publishing_it", "paneflow-host host::tests::a_scan_waiting_to_persist_cannot_overwrite_a_restarted_generation", "paneflow-host host::tests::a_scan_waiting_to_persist_cannot_recreate_a_removed_record", "paneflow-host tests/ownership_probes.rs", "paneflow-host tests/lifecycle.rs"], &[]),
            "capacity_faults": automated_only("W06", &["paneflow-host host::tests::checkpoint_staging_admits_two_captures_and_the_third_waits_for_a_release", "paneflow-host host::tests::oversized_terminal_dimensions_are_refused_before_reaching_the_engine", "paneflow-host server::tests::streaming_followers_leave_reserved_slots_for_control_requests", "paneflow-host runtime::tests::a_child_that_stops_reading_its_input_never_starves_the_control_path"], &[]),
            "stop_all_and_shutdown_rpc_failure": automated_only("W06", &["paneflow-app app::quit_dialog tests", "paneflow-host host::tests::shutdown_keeps_unsaved_final_state_owned_until_retry_succeeds"], &["failed stop-all through the native quit dialog: qualification runbook cell D-09"]),
        },
        "W07": automated_only("W07", &["paneflow-app pane::tests::a_natural_exit_keeps_the_surface_as_a_passive_final_view", "paneflow-app app::hosted_sessions::tests::every_unattached_session_is_listed_exactly_once_across_workspaces_and_the_fallback_group", "paneflow-app app::hosted_sessions::tests::a_fallback_workspace_prefers_the_recorded_cwd_and_falls_back_to_home", "paneflow-app app::sidebar::tests::a_disconnected_host_reads_as_stale_never_as_idle_or_finished"], &["native desktop usage after idle, resize, paste, search: runbook cells D-01 to D-10"]),
        "W08": {"status": "pending", "reason": "the 8-hour endurance run is executed on the designated qualification machines per the runbook; this run records no endurance evidence"},
    });
    let document = json!({
        "suite": "paneflow-persistent-bench",
        "schema_version": SCHEMA_VERSION,
        "platform": platform(),
        "protocol": protocol.label,
        "acceptance_grade": protocol.acceptance_grade,
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
        "workloads": workloads_json,
        "thresholds": decisions.iter().map(Decision::to_json).collect::<Vec<_>>(),
        "prior_failures": prior,
        "retry_policy": "none: the scripts never retry; a rerun passes PANEFLOW_BENCH_PRIOR_RESULT so the first failure stays in the evidence",
        "fixtures": fixtures,
        "allocator": "no custom global allocator in the host or the fixture; native memory comes from OS counters",
        "topology": topology_label(worker_pid.is_some()),
        "controller": std::env::var_os("PANEFLOW_BENCH_CONTROLLER").map(|path| executable_identity(Path::new(&path))),
        "native_environments": {"windows": cfg!(windows), "linux": cfg!(target_os = "linux"), "macos": cfg!(target_os = "macos"), "note": "false means pending, not passed"},
        "unmeasured": [
            "keystroke to pixel latency of a hosted pane (needs the desktop; see scripts/bench-terminal)",
            "reattach time after a desktop restart with 50 sessions",
            "host memory after 24 hours of idle sessions",
        ],
    });
    let mut document = document;
    let comparison = compare(&document, &decisions);
    document["comparison"] =
        json!({"text": comparison, "baseline": baseline_path("persistent").display().to_string()});
    let path = output_path();
    write_document(&path, &document);
    println!("result: {}", path.display());
    print!("{comparison}");
    assert_eq!(
        document["source_unchanged_during_measurement"], true,
        "source changed during measurement; result is not candidate-qualified"
    );
    if let Err(failures) = verdict(&decisions, &prior) {
        panic!(
            "persistent-path thresholds failed; the artifact is retained at {}:\n{}",
            path.display(),
            failures.join("\n")
        );
    }
}

#[test]
#[ignore = "active-agents measurement; run through scripts/bench-persistent.sh --active or .ps1 -Active"]
fn persistent_session_active() {
    allow_breakaway_like_the_desktop_does();
    let path = output_path();
    let source_fingerprint = diff_fingerprint_excluding(Some(&path));
    let home = tempfile::tempdir().unwrap();
    seed_home(home.path());
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let adoption =
        bootstrap::ensure_host_running(home.path(), &host_executable(), "persistent-bench")
            .expect("the detached host starts");
    let hello = ClientHello::local("persistent-bench");
    let mut client = HostClient::connect(&endpoint, &hello).unwrap();
    let host_identity = paneflow_host::ProcessIdentity::capture(adoption.identity.pid);
    let worker = WorkerProcess::start(home.path());
    let desktop_enabled = std::env::var_os("PANEFLOW_BENCH_DESKTOP").is_some();
    let ledger = FixtureLedger::new();
    let echo = workloads::EchoProbe::start(&mut client, &endpoint, &ledger);
    let started = Instant::now();
    let mut scenarios = Vec::new();
    let mut failures = Vec::new();
    for streams in active::ACTIVE_SCENARIOS {
        let plan = active::ActivePlan {
            streams,
            stream_args: &active::STREAM_ARGS,
            flood_args: Some(&active::FLOOD_ARGS),
            settle: SETTLE,
            window: active::ACTIVE_WINDOW,
            cpu_slices: 1,
        };
        let processes = active::ActiveProcesses {
            host_pid: adoption.identity.pid,
            worker: worker.as_ref(),
            desktop_home: desktop_enabled.then_some(home.path()),
            echo: Some(&echo),
        };
        match active::run_active_scenario(&mut client, &ledger, &plan, &processes) {
            Ok(scenario) => scenarios.push(scenario),
            Err(reason) => {
                failures.push(format!("{streams} sessions: {reason}"));
                scenarios.push(json!({"sessions": streams, "failed": reason}));
                break;
            }
        }
    }
    let elapsed = started.elapsed();
    echo.finish(&mut client);
    drop(worker);
    let mut decisions = Vec::new();
    let fixtures = shutdown_host(
        &mut decisions,
        client,
        &host_identity,
        home.path(),
        &endpoint,
        &hello,
        &ledger,
    );
    let mut document = json!({
        "suite": "paneflow-persistent-active",
        "schema_version": SCHEMA_VERSION,
        "platform": platform(),
        "status": if failures.is_empty() { "complete" } else { "failed" },
        "stamp": stamp(),
        "commit": git(&["rev-parse", "HEAD"]),
        "commit_short": std::env::var("PANEFLOW_BENCH_SHA").ok(),
        "diff": source_fingerprint,
        "source_unchanged_during_measurement": source_fingerprint == diff_fingerprint_excluding(Some(&path)),
        "machine": machine(),
        "toolchain": toolchain(),
        "engine": adoption.identity.engine,
        "host": {"version": adoption.identity.version, "protocol": adoption.identity.protocol, "build_id": adoption.identity.build_id},
        "executables": {"host": executable_identity(&host_executable()), "fixture": executable_identity(&fixture_executable())},
        "controller": std::env::var_os("PANEFLOW_BENCH_CONTROLLER").map(|path| executable_identity(Path::new(&path))),
        "topology": match (desktop_enabled, std::env::var_os("PANEFLOW_BENCH_CONTROLLER").is_some()) {
            (true, _) => "host-worker-native-desktop",
            (false, true) => "host-worker",
            (false, false) => "host-only",
        },
        "invocation": {
            "test": "persistent_session_active",
            "stream": active::STREAM_ARGS,
            "flood": active::FLOOD_ARGS,
            "scenarios": active::ACTIVE_SCENARIOS,
            "settle_s": SETTLE.as_secs_f64(),
            "window_s": active::ACTIVE_WINDOW.as_secs_f64(),
            "echo_samples": active::ACTIVE_ECHO_SAMPLES,
            "args": std::env::args().collect::<Vec<_>>(),
        },
        "measurement_s": elapsed.as_secs_f64(),
        "scenarios": scenarios,
        "thresholds": decisions.iter().map(Decision::to_json).collect::<Vec<_>>(),
        "fixtures": fixtures,
        "failures": failures,
        "counters_note": "work_counters are deltas of the US-001 counters over the window; pending carries its reason and is never a measured zero",
    });
    let baseline = std::fs::read(baseline_path("persistent-active"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let comparison = active::compare_active(&document, baseline.as_ref());
    document["comparison"] = json!({"text": comparison, "baseline": baseline_path("persistent-active").display().to_string()});
    write_document(&path, &document);
    println!("result: {}", path.display());
    print!("{comparison}");
    assert!(
        failures.is_empty(),
        "the active scenario failed; the artifact is retained at {}:\n{}",
        path.display(),
        failures.join("\n")
    );
    assert_eq!(
        document["source_unchanged_during_measurement"], true,
        "source changed during measurement; result is not candidate-qualified"
    );
    if let Err(failures) = verdict(&decisions, &[]) {
        panic!(
            "host shutdown checks failed; the artifact is retained at {}:\n{}",
            path.display(),
            failures.join("\n")
        );
    }
}

#[test]
fn an_active_scenario_whose_session_dies_fails_with_its_name_and_publishes_no_average() {
    allow_breakaway_like_the_desktop_does();
    let home = tempfile::tempdir().unwrap();
    seed_home(home.path());
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let adoption = bootstrap::ensure_host_running(home.path(), &host_executable(), "active-death")
        .expect("the detached host starts");
    let mut client = HostClient::connect(&endpoint, &ClientHello::local("active-death")).unwrap();
    let ledger = FixtureLedger::new();
    let processes = active::ActiveProcesses {
        host_pid: adoption.identity.pid,
        worker: None,
        desktop_home: None,
        echo: None,
    };
    let none = active::ActivePlan {
        streams: 0,
        stream_args: &active::STREAM_ARGS,
        flood_args: None,
        settle: Duration::ZERO,
        window: Duration::ZERO,
        cpu_slices: 1,
    };
    assert_eq!(
        active::run_active_scenario(&mut client, &ledger, &none, &processes),
        Err("no active session was opened".to_string())
    );
    let dying = active::ActivePlan {
        streams: 1,
        stream_args: &["delayed-exit", "300", "0"],
        flood_args: None,
        settle: Duration::ZERO,
        window: Duration::from_secs(2),
        cpu_slices: 1,
    };
    let failure = active::run_active_scenario(&mut client, &ledger, &dying, &processes)
        .expect_err("a session that exits inside the window fails the scenario");
    let session = client
        .list(None)
        .unwrap()
        .pop()
        .expect("the dead session row");
    assert!(
        failure.contains(session.session.as_str()),
        "the failure names the session: {failure}"
    );
    assert!(failure.contains("no average is published"), "{failure}");
    let _ = client.call("host.shutdown", json!({}));
}

#[test]
fn a_pending_counter_stays_pending_and_never_reads_as_an_improvement() {
    let document = json!({
        "schema_version": SCHEMA_VERSION,
        "scenarios": [{"sessions": 8, "host": {"work_counters": {
            "process_listings": {"pending": "counters unavailable: host 0.17.5 reports no counters object"},
            "foreground_observations": 3,
        }}}],
    });
    let baseline = json!({
        "schema_version": SCHEMA_VERSION,
        "scenarios": [{"sessions": 8, "host": {"work_counters": {
            "process_listings": 480,
            "foreground_observations": 9,
        }}}],
    });
    let text = active::compare_active(&document, Some(&baseline));
    let listings = text
        .lines()
        .find(|line| line.contains("process_listings"))
        .unwrap();
    assert!(
        listings.ends_with("pending: counters unavailable: host 0.17.5 reports no counters object"),
        "{listings}"
    );
    assert!(!listings.contains("lower"), "{listings}");
    let observations = text
        .lines()
        .find(|line| line.contains("foreground_observations"))
        .unwrap();
    assert!(observations.ends_with("lower"), "{observations}");
    let reading = paneflow_host::work_counters::Reading::Pending("why".to_string());
    assert_eq!(active::reading_json(&reading), json!({"pending": "why"}));
}

#[test]
fn a_baseline_of_another_schema_is_refused_with_an_explicit_message() {
    let document = json!({"schema_version": SCHEMA_VERSION, "scenarios": []});
    let old = json!({"schema_version": 3, "scenarios": []});
    let text = active::compare_active(&document, Some(&old));
    assert!(
        text.starts_with(
            "baseline schema 3 differs from candidate schema 4; comparison refused, record a new baseline"
        ),
        "{text}"
    );
    assert!(active::schema_refusal(&document, &document).is_none());
}

#[test]
fn every_committed_persistent_baseline_is_clean_current_and_on_its_platform() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/baselines");
    let mut violations = Vec::new();
    for directory in std::fs::read_dir(&root).unwrap() {
        let directory = directory.unwrap().path();
        let platform = directory
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for name in ["persistent", "persistent-active"] {
            let path = directory.join(format!("{name}.json"));
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let document: Value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()));
            violations.extend(baseline_violations(&path, &platform, &document));
        }
    }
    assert!(
        violations.is_empty(),
        "committed persistent baselines must be at schema_version {SCHEMA_VERSION}, from a clean tree, under their own platform; record them again with scripts/bench-persistent.sh --set-baseline:\n{}",
        violations.join("\n")
    );
}

#[test]
fn an_old_schema_persistent_baseline_fails_the_coherence_check_naming_the_file_and_the_schema() {
    let path = Path::new("bench/baselines/windows-x86_64/persistent.json");
    let old = json!({
        "schema_version": 2,
        "platform": "windows-x86_64",
        "diff": {"dirty": false},
        "machine": {"os": "windows", "arch": "x86_64"},
    });
    assert_eq!(
        baseline_violations(path, "windows-x86_64", &old),
        [format!(
            "bench/baselines/windows-x86_64/persistent.json: schema_version 2, expected schema_version {SCHEMA_VERSION}"
        )]
    );
    let dirty_elsewhere = json!({
        "schema_version": SCHEMA_VERSION,
        "platform": "linux-x86_64",
        "diff": {"dirty": null},
        "machine": {"os": "linux", "arch": "x86_64"},
    });
    let violations = baseline_violations(path, "windows-x86_64", &dirty_elsewhere);
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations[0].contains("diff.dirty is null"),
        "{violations:?}"
    );
    assert!(
        violations[1].contains("recorded on linux-x86_64"),
        "{violations:?}"
    );
}

#[test]
fn a_missing_persistent_baseline_names_the_current_platform() {
    let document = json!({"schema_version": SCHEMA_VERSION, "scenarios": []});
    let text = active::compare_active(&document, None);
    assert!(
        text.starts_with(&format!("no active baseline for {}", platform())),
        "{text}"
    );
    assert!(baseline_path("persistent").ends_with(Path::new(&platform()).join("persistent.json")));
}

#[test]
#[ignore = "real hardware protocol; run through scripts/perf-hardware.sh or .ps1 against a running Paneflow"]
fn hardware_protocol() {
    let state = std::env::var("PANEFLOW_HW_STATE").unwrap_or_default();
    assert!(
        hardware::HARDWARE_STATES.contains(&state.as_str()),
        "PANEFLOW_HW_STATE must be one of {:?}, got {state:?}",
        hardware::HARDWARE_STATES
    );
    let label = std::env::var("PANEFLOW_HW_LABEL").unwrap_or_else(|_| "unlabeled".to_string());
    let window = std::env::var("PANEFLOW_HW_WINDOW_S")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(hardware::HARDWARE_WINDOW, Duration::from_secs);
    let document = hardware::measure(&state, &label, window);
    let path = std::env::var_os("PANEFLOW_BENCH_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../bench/results")
                .join(format!(
                    "hardware-{}-{}-{state}-{label}-{}.json",
                    platform(),
                    hardware::session_type(),
                    stamp()
                ))
        });
    write_document(&path, &document);
    println!("result: {}", path.display());
    println!("{}", serde_json::to_string_pretty(&document).unwrap());
}

#[test]
fn hardware_sources_parse_their_tools_and_never_turn_a_missing_reading_into_zero() {
    let nvidia = hardware::parse_nvidia_smi("0, 12\n1, 0\n0, 30\n0, [N/A]\n");
    assert_eq!(nvidia["nvidia-smi gpu0"], [12.0, 30.0]);
    assert_eq!(nvidia["nvidia-smi gpu1"], [0.0]);
    let intel = "{\n\t\"engines\": {\n\t\t\"Render/3D/0\": {\n\t\t\t\"busy\": 7.25,\n\t\t\t\"sema\": 0.0\n\t\t},\n\t\t\"Blitter/0\": {\n\t\t\t\"busy\": 99.0\n\t\t}\n\t}\n}\n";
    assert_eq!(hardware::parse_intel_gpu_top(intel), [7.25]);
    let powermetrics = "**** GPU usage ****\n\nGPU HW active frequency: 389 MHz\nGPU HW active residency:  12.34% (389 MHz: 12%)\nGPU idle residency:  87.66%\n";
    assert_eq!(hardware::parse_powermetrics(powermetrics), [12.34]);
    let typeperf = "\n\"(PDH-CSV 4.0)\",\"\\\\PC\\GPU Engine(pid_42_luid_0x0_0x1_phys_0_eng_0_engtype_3D)\\Utilization Percentage\",\"\\\\PC\\GPU Engine(pid_42_luid_0x0_0x1_phys_0_eng_3_engtype_Copy)\\Utilization Percentage\",\"\\\\PC\\GPU Engine(pid_7_luid_0x0_0x1_phys_0_eng_0_engtype_3D)\\Utilization Percentage\"\n\"10/06/2026 10:00:00.000\",\"4.5\",\"50\",\"80\"\n\"10/06/2026 10:00:01.000\",\"1.5\",\"50\",\"80\"\nExiting, please wait...\n";
    assert_eq!(hardware::parse_typeperf(typeperf, 42), [4.5, 1.5]);
    assert!(hardware::parse_typeperf(typeperf, 9).is_empty());
    let mangohud = "os,cpu,gpu,ram,kernel,driver,cpuscheduler\nFedora,AMD,NVIDIA,32,6.0,580,\nfps,frametime,cpu_load,gpu_load\n144,6.94,3,5\n120,8.33,3,5\n";
    assert_eq!(hardware::parse_frame_log(mangohud).unwrap(), [6.94, 8.33]);
    let presentmon = "Application,ProcessID,MsBetweenPresents\npaneflow.exe,42,16.6\n";
    assert_eq!(hardware::parse_frame_log(presentmon).unwrap(), [16.6]);
    assert!(
        hardware::parse_frame_log("a,b\n1,2\n")
            .unwrap_err()
            .contains("no header")
    );
    let empty = hardware::distribution(&[], "percent busy");
    assert!(empty["not_measured"].is_string(), "{empty}");
    let measured = hardware::distribution(&[1.0, 2.0, 3.0, 4.0], "ms");
    assert_eq!(measured["p50"], 2.0);
    assert_eq!(measured["p95"], 4.0);
    assert_eq!(
        hardware::not_measured("why"),
        json!({"not_measured": "why"})
    );
    assert_eq!(
        hardware::display_backend(b"HOME=/h\0WAYLAND_DISPLAY=wayland-0\0DISPLAY=:0\0"),
        "wayland"
    );
    assert_eq!(
        hardware::display_backend(b"WAYLAND_DISPLAY=\0DISPLAY=:0\0"),
        "x11"
    );
    assert_eq!(hardware::display_backend(b"HOME=/h\0"), "unknown");
}

#[test]
#[ignore = "US-003 headless desktop spike; run through .github/workflows/perf-desktop-spike.yml"]
fn desktop_headless_spike() {
    allow_breakaway_like_the_desktop_does();
    let started = Instant::now();
    let path = output_path();
    let display = std::env::var("PANEFLOW_SPIKE_DISPLAY").unwrap_or_else(|_| "native".to_string());
    let home = tempfile::tempdir().unwrap();
    seed_home(home.path());
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let adoption =
        bootstrap::ensure_host_running(home.path(), &host_executable(), "headless-spike")
            .expect("the detached host starts");
    let hello = ClientHello::local("headless-spike");
    let mut client = HostClient::connect(&endpoint, &hello).unwrap();
    let host_identity = paneflow_host::ProcessIdentity::capture(adoption.identity.pid);
    let ledger = FixtureLedger::new();
    let open = |client: &mut HostClient, mode: &[&str]| -> Vec<SessionId> {
        (0..4)
            .map(|_| {
                let created = client
                    .create(&workloads::fixture(mode))
                    .expect("the fixture session starts");
                ledger.record(client, &created.manifest.session);
                created.manifest.session
            })
            .collect()
    };

    let idle = open(&mut client, &["idle"]);
    let desktop_started = Instant::now();
    let mut states = json!({});
    let mut failure = None;
    let mut first_frame = None;
    match headless::start_desktop(home.path(), &idle, true) {
        Ok(desktop) => {
            first_frame = Some(desktop_started.elapsed());
            states["idle_4_panes"] =
                headless::measure_state(&desktop, headless::SPIKE_RUNS, headless::SPIKE_WINDOW);
            let submitted = headless::submit_prompt(&endpoint, &idle[0]);
            std::thread::sleep(SETTLE);
            states["one_agent_thinking"] =
                headless::measure_state(&desktop, headless::SPIKE_RUNS, headless::SPIKE_WINDOW);
            states["one_agent_thinking"]["prompt_submit"] = submitted;
        }
        Err(error) => failure = Some(error),
    }
    for session in &idle {
        let _ = client.stop(session, None);
    }
    if failure.is_none() {
        let streams = open(&mut client, &["stream", "16384", "600"]);
        match headless::start_desktop(home.path(), &streams, false) {
            Ok(desktop) => {
                std::thread::sleep(SETTLE);
                states["four_streams"] =
                    headless::measure_state(&desktop, headless::SPIKE_RUNS, headless::SPIKE_WINDOW);
            }
            Err(error) => failure = Some(error),
        }
        for session in &streams {
            let _ = client.stop(session, None);
        }
    }
    let mut decisions = Vec::new();
    let fixtures = shutdown_host(
        &mut decisions,
        client,
        &host_identity,
        home.path(),
        &endpoint,
        &hello,
        &ledger,
    );
    let (validated, mut reasons) = headless::conclusion(first_frame, &states);
    if let Some(error) = &failure {
        reasons.insert(0, format!("desktop or Vulkan adapter failure: {error}"));
    }
    let validated = validated && failure.is_none();
    let document = json!({
        "suite": "paneflow-desktop-headless-spike",
        "schema_version": SCHEMA_VERSION,
        "display": display,
        "validated": validated,
        "conclusion": if validated { "validated" } else { "not validated" },
        "reasons": reasons,
        "first_frame_s": first_frame.map(|elapsed| elapsed.as_secs_f64()),
        "elapsed_s": started.elapsed().as_secs_f64(),
        "unstable_counters_cv_above_10_percent": headless::unstable_counters(&states),
        "states": states,
        "stamp": stamp(),
        "commit": git(&["rev-parse", "HEAD"]),
        "machine": machine(),
        "toolchain": toolchain(),
        "executables": {"host": executable_identity(&host_executable()), "fixture": executable_identity(&fixture_executable())},
        "controller": std::env::var_os("PANEFLOW_BENCH_CONTROLLER").map(|path| executable_identity(Path::new(&path))),
        "fixtures": fixtures,
        "thresholds": decisions.iter().map(Decision::to_json).collect::<Vec<_>>(),
    });
    write_document(&path, &document);
    println!("result: {}", path.display());
    println!(
        "{}",
        serde_json::to_string_pretty(&document["reasons"]).unwrap()
    );
    assert!(
        validated,
        "headless desktop spike not validated on {display}: {}",
        document["reasons"]
    );
}

const TAB_BADGE_TABS: usize = 12;

#[cfg(target_os = "linux")]
fn stat_cpu_ns(stat_path: &str) -> Option<u64> {
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let stat = std::fs::read_to_string(stat_path).ok()?;
    let fields: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    let ticks: u64 = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    Some(ticks * (1_000_000_000 / u64::try_from(ticks_per_second).ok().filter(|t| *t > 0)?))
}

#[cfg(target_os = "linux")]
fn main_thread_cpu_ns(pid: u32) -> Option<u64> {
    stat_cpu_ns(&format!("/proc/{pid}/task/{pid}/stat"))
}

#[cfg(target_os = "linux")]
fn process_cpu_ns(pid: u32) -> Option<u64> {
    stat_cpu_ns(&format!("/proc/{pid}/stat"))
}

#[cfg(not(target_os = "linux"))]
fn main_thread_cpu_ns(_pid: u32) -> Option<u64> {
    None
}

#[cfg(not(target_os = "linux"))]
fn process_cpu_ns(_pid: u32) -> Option<u64> {
    None
}

#[test]
#[ignore = "US-005 tab badge cost; run under a display with PANEFLOW_BENCH_CONTROLLER, see bench/README.md"]
fn desktop_tab_badges_cpu() {
    allow_breakaway_like_the_desktop_does();
    let path = output_path();
    let home = tempfile::tempdir().unwrap();
    seed_home(home.path());
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let adoption = bootstrap::ensure_host_running(home.path(), &host_executable(), "tab-badges")
        .expect("the detached host starts");
    let hello = ClientHello::local("tab-badges");
    let mut client = HostClient::connect(&endpoint, &hello).unwrap();
    let host_identity = paneflow_host::ProcessIdentity::capture(adoption.identity.pid);
    let ledger = FixtureLedger::new();
    let sessions: Vec<SessionId> = (0..TAB_BADGE_TABS)
        .map(|_| {
            let created = client
                .create(&workloads::fixture(&["idle"]))
                .expect("the fixture session starts");
            ledger.record(&mut client, &created.manifest.session);
            created.manifest.session
        })
        .collect();
    let desktop =
        DesktopProcess::start_in_two_panes(home.path(), &sessions, headless::FIRST_FRAME_LIMIT);
    let submitted = headless::submit_prompt(&endpoint, &sessions[0]);
    std::thread::sleep(SETTLE);
    let pid = desktop.child.id();
    let runs: Vec<Value> = (0..headless::SPIKE_RUNS)
        .map(|_| {
            let counters_before = active::desktop_counters(&desktop);
            let cpu_before = main_thread_cpu_ns(pid);
            let started = Instant::now();
            std::thread::sleep(headless::SPIKE_WINDOW);
            let cpu_after = main_thread_cpu_ns(pid);
            let window = started.elapsed();
            let counters_after = active::desktop_counters(&desktop);
            json!({
                "window_s": window.as_secs_f64(),
                "main_thread_cpu_percent": cpu_before.zip(cpu_after).map(|(before, after)| {
                    after.saturating_sub(before) as f64 / window.as_nanos() as f64 * 100.0
                }),
                "work_counters": active::window_json(&counters_before, &counters_after),
            })
        })
        .collect();
    drop(desktop);
    for session in &sessions {
        let _ = client.stop(session, None);
    }
    let mut decisions = Vec::new();
    let fixtures = shutdown_host(
        &mut decisions,
        client,
        &host_identity,
        home.path(),
        &endpoint,
        &hello,
        &ledger,
    );
    let cpu: Vec<f64> = runs
        .iter()
        .filter_map(|run| run["main_thread_cpu_percent"].as_f64())
        .collect();
    let document = json!({
        "suite": "paneflow-desktop-tab-badges",
        "schema_version": SCHEMA_VERSION,
        "tabs": TAB_BADGE_TABS,
        "state": "two side by side panes of 6 terminal tabs each (12 tab badges, a pane holds at most 8), one agent thinking",
        "prompt_submit": submitted,
        "main_thread_cpu_percent": workloads::stats(&cpu, headless::SPIKE_RUNS),
        "runs": runs,
        "stamp": stamp(),
        "commit": git(&["rev-parse", "HEAD"]),
        "commit_short": std::env::var("PANEFLOW_BENCH_SHA").ok(),
        "machine": machine(),
        "toolchain": toolchain(),
        "controller": std::env::var_os("PANEFLOW_BENCH_CONTROLLER").map(|path| executable_identity(Path::new(&path))),
        "fixtures": fixtures,
        "thresholds": decisions.iter().map(Decision::to_json).collect::<Vec<_>>(),
    });
    write_document(&path, &document);
    println!("result: {}", path.display());
    println!(
        "{}",
        serde_json::to_string_pretty(&document["main_thread_cpu_percent"]).unwrap()
    );
    assert_eq!(
        cpu.len(),
        headless::SPIKE_RUNS,
        "a window lost its CPU sample"
    );
}

#[test]
#[ignore = "performance gates; run through scripts/perf-gates.sh"]
fn perf_gates() {
    allow_breakaway_like_the_desktop_does();
    let started = Instant::now();
    let path = output_path();
    let inputs = std::env::var_os("PANEFLOW_PERF_GATES_DIR")
        .map(PathBuf::from)
        .expect("PANEFLOW_PERF_GATES_DIR names the directory of the suite results; run scripts/perf-gates.sh");
    let home = tempfile::tempdir().unwrap();
    seed_home(home.path());
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let adoption = bootstrap::ensure_host_running(home.path(), &host_executable(), "perf-gates")
        .expect("the detached host starts");
    let hello = ClientHello::local("perf-gates");
    let mut client = HostClient::connect(&endpoint, &hello).unwrap();
    let host_pid = adoption.identity.pid;
    let host_identity = paneflow_host::ProcessIdentity::capture(host_pid);
    let worker = WorkerProcess::start(home.path());
    let ledger = FixtureLedger::new();
    let mut values = BTreeMap::new();
    let mut measurements = json!({});
    measurements["host_worker_idle"] =
        gate_runs::host_worker_idle(&mut client, &ledger, host_pid, worker.as_ref(), &mut values);
    measurements["host_worker_active"] =
        gate_runs::host_worker_active(&mut client, &ledger, host_pid, worker.as_ref(), &mut values);
    measurements["desktop"] = gate_runs::desktop_idle_and_thinking(
        &mut client,
        &ledger,
        home.path(),
        &endpoint,
        &mut values,
    );
    measurements["desktop_diff_stat"] =
        gate_runs::desktop_diff_stat(&mut client, &ledger, home.path(), &mut values);
    measurements["hook_burst"] = gate_runs::hook_burst(&inputs, &mut values);
    measurements["desktop_startup"] = gate_runs::startup(&inputs, &mut values);
    measurements["terminal_trickle"] = gate_runs::trickle(&inputs, &mut values);
    drop(worker);
    let mut decisions = Vec::new();
    let fixtures = shutdown_host(
        &mut decisions,
        client,
        &host_identity,
        home.path(),
        &endpoint,
        &hello,
        &ledger,
    );
    let mut verdicts = gates::verify(gates::BUDGETS, &values);
    verdicts.extend(gate_runs::allocations(&inputs));
    let document = json!({
        "suite": "paneflow-perf-gates",
        "schema_version": SCHEMA_VERSION,
        "stamp": stamp(),
        "commit": git(&["rev-parse", "HEAD"]),
        "commit_short": std::env::var("PANEFLOW_BENCH_SHA").ok(),
        "machine": machine(),
        "toolchain": toolchain(),
        "elapsed_s": started.elapsed().as_secs_f64(),
        "gates": gates::verdicts_json(&verdicts),
        "measurements": measurements,
        "fixtures": fixtures,
        "thresholds": decisions.iter().map(Decision::to_json).collect::<Vec<_>>(),
    });
    write_document(&path, &document);
    let summary = gates::markdown(&verdicts);
    if let Some(step_summary) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(step_summary)
            .and_then(|mut file| file.write_all(summary.as_bytes()));
    }
    print!("{summary}");
    println!("report: {}", path.display());
    if let Some(report) = gates::failure_report(&verdicts) {
        panic!("{report}report: {}", path.display());
    }
    if let Err(failures) = verdict(&decisions, &[]) {
        panic!(
            "host shutdown checks failed; the report is retained at {}:\n{}",
            path.display(),
            failures.join("\n")
        );
    }
}

fn topology_label(worker: bool) -> &'static str {
    if std::env::var_os("PANEFLOW_BENCH_DESKTOP").is_some() {
        "host-worker-native-desktop"
    } else if std::env::var_os("PANEFLOW_BENCH_NO_FOLLOWERS").is_some() {
        if worker { "host-worker" } else { "host-only" }
    } else if worker {
        "host-worker-headless-followers"
    } else {
        "host-headless-followers"
    }
}

#[test]
#[ignore = "endurance measurement; run through scripts/bench-persistent.sh --endurance or .ps1 -Endurance"]
fn persistent_session_endurance() {
    allow_breakaway_like_the_desktop_does();
    let plan = EndurancePlan::from_env();
    let desktop_enabled = std::env::var_os("PANEFLOW_BENCH_DESKTOP").is_some();
    let path = output_path();
    let source_fingerprint = diff_fingerprint_excluding(Some(&path));
    let home = tempfile::tempdir().unwrap();
    seed_home(home.path());
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let adoption =
        bootstrap::ensure_host_running(home.path(), &host_executable(), "persistent-bench")
            .expect("the detached host starts");
    let hello = ClientHello::local("persistent-bench");
    let mut client = HostClient::connect(&endpoint, &hello).unwrap();
    let mut idle_control =
        HostClient::connect(&endpoint, &ClientHello::local("persistent-endurance-idle")).unwrap();
    let host_pid = adoption.identity.pid;
    let host_identity = paneflow_host::ProcessIdentity::capture(host_pid);
    let mut worker = WorkerProcess::start(home.path());
    let worker_executable = std::env::var_os("PANEFLOW_BENCH_CONTROLLER");
    let replacement = std::env::var_os("PANEFLOW_BENCH_CONTROLLER_REPLACEMENT");
    let ledger = FixtureLedger::new();
    let mut decisions: Vec<Decision> = Vec::new();

    let mut idle_sessions = Vec::with_capacity(ENDURANCE_RETAINED_IDLE);
    for _ in 0..ENDURANCE_RETAINED_IDLE {
        let created = client
            .create(&workloads::fixture(&["idle"]))
            .expect("retained idle fixture");
        ledger.record(&mut client, &created.manifest.session);
        idle_sessions.push(created.manifest.session);
    }
    let echo = workloads::EchoProbe::start(&mut client, &endpoint, &ledger);
    let mut retained = idle_sessions.clone();
    retained.push(echo.session.clone());
    let identities = retained_identities(&mut client, &retained);

    let started = Instant::now();
    let idle_end = started + plan.idle;
    let active = plan.duration.saturating_sub(plan.idle);
    let slot = |cycles: usize, index: usize| {
        idle_end + active.mul_f64((index as f64 + 0.5) / cycles.max(1) as f64)
    };
    let mut samples = vec![endurance_sample(
        &mut client,
        host_pid,
        worker.as_ref().map(|w| w.child.id()),
        &retained,
        Duration::ZERO,
        "start",
    )];
    let mut bursts = Vec::new();
    let mut worker_cycles = Vec::new();
    let mut desktop_cycles = Vec::new();
    let mut first_input = Value::Null;
    let mut identity_checks = Vec::new();
    let mut next_sample = started + plan.sample_interval;
    let mut next_burst = started + plan.burst_interval;
    let mut worker_index = 0usize;
    let mut desktop_index = 0usize;
    let document = |status: &str,
                    samples: &[Value],
                    bursts: &[Value],
                    worker_cycles: &[Value],
                    desktop_cycles: &[Value],
                    first_input: &Value,
                    identity_checks: &[Value],
                    decisions: &[Decision],
                    extra: Value| {
        let mut document = json!({
            "suite": "paneflow-persistent-endurance",
            "schema_version": SCHEMA_VERSION,
            "status": status,
            "plan": plan.to_json(desktop_enabled),
            "protocol": plan.to_json(desktop_enabled)["label"],
            "acceptance_grade": plan.acceptance_grade(desktop_enabled),
            "stamp": stamp(),
            "commit": git(&["rev-parse", "HEAD"]),
            "commit_short": std::env::var("PANEFLOW_BENCH_SHA").ok(),
            "diff": source_fingerprint,
            "source_unchanged_during_measurement": source_fingerprint == diff_fingerprint_excluding(Some(&path)),
            "machine": machine(),
            "toolchain": toolchain(),
            "engine": adoption.identity.engine,
            "host": {"version": adoption.identity.version, "protocol": adoption.identity.protocol, "build_id": adoption.identity.build_id},
            "executables": {"host": executable_identity(&host_executable()), "fixture": executable_identity(&fixture_executable())},
            "controller": worker_executable.as_ref().map(|path| executable_identity(Path::new(path))),
            "controller_replacement": replacement.as_ref().map(|path| executable_identity(Path::new(path))),
            "pty": if cfg!(windows) { "ConPTY via portable-pty 0.9" } else { "posix openpty via portable-pty 0.9" },
            "topology": topology_label(worker_executable.is_some()),
            "invocation": {"test": "persistent_session_endurance", "fixture": fixture_executable().display().to_string(), "args": std::env::args().collect::<Vec<_>>()},
            "retained": retained,
            "workloads": {"W08": {
                "status": if status == "complete" { "measured" } else { status },
                "elapsed_s": started.elapsed().as_secs(),
                "samples": samples,
                "bursts": bursts,
                "worker_cycles": worker_cycles,
                "desktop_cycles": if desktop_enabled { json!(desktop_cycles) } else { json!({"pending": "PANEFLOW_BENCH_DESKTOP was not supplied; the 100 desktop detach/reopen cycles need the native desktop"}) },
                "idle_first_input": first_input,
                "identity_checks": identity_checks,
            }},
            "thresholds": decisions.iter().map(Decision::to_json).collect::<Vec<_>>(),
            "prior_failures": workloads::prior_failures(),
            "retry_policy": "none: the scripts never retry; a rerun passes PANEFLOW_BENCH_PRIOR_RESULT so the first failure stays in the evidence",
            "allocator": "no custom global allocator in the host or the fixture; native memory comes from OS counters",
            "native_environments": {"windows": cfg!(windows), "linux": cfg!(target_os = "linux"), "macos": cfg!(target_os = "macos"), "note": "false means pending, not passed"},
        });
        if let Value::Object(fields) = extra {
            for (key, value) in fields {
                document[key] = value;
            }
        }
        document
    };
    write_document(
        &path,
        &document(
            "running",
            &samples,
            &bursts,
            &worker_cycles,
            &desktop_cycles,
            &first_input,
            &identity_checks,
            &decisions,
            json!({}),
        ),
    );

    loop {
        let now = Instant::now();
        if now >= started + plan.duration {
            break;
        }
        let in_idle = now < idle_end;
        if now >= next_sample {
            samples.push(endurance_sample(
                &mut client,
                host_pid,
                worker.as_ref().map(|w| w.child.id()),
                &retained,
                started.elapsed(),
                if in_idle { "idle-control" } else { "active" },
            ));
            identity_checks.push(json!({"elapsed_s": started.elapsed().as_secs(), "unchanged": identities_unchanged(&mut client, &identities)}));
            next_sample += plan.sample_interval;
            write_document(
                &path,
                &document(
                    "running",
                    &samples,
                    &bursts,
                    &worker_cycles,
                    &desktop_cycles,
                    &first_input,
                    &identity_checks,
                    &decisions,
                    json!({}),
                ),
            );
        }
        if now >= next_burst {
            bursts.push(endurance_burst(
                &mut client,
                &endpoint,
                &ledger,
                retained.len(),
                bursts.len(),
            ));
            next_burst += plan.burst_interval;
        }
        if !in_idle && first_input.is_null() {
            first_input = echo.first_input(&mut idle_control, Duration::from_secs(2));
            first_input["idle_s"] = json!(started.elapsed().as_secs());
        }
        if !in_idle
            && worker_index < plan.worker_cycles
            && now >= slot(plan.worker_cycles, worker_index)
        {
            let record = match (worker.take(), worker_executable.as_deref()) {
                (Some(live), Some(executable)) => {
                    let (next, mut record) = workloads::cycle_worker(
                        home.path(),
                        live,
                        worker_index,
                        executable,
                        replacement.as_deref(),
                    );
                    worker = Some(next);
                    record["elapsed_s"] = json!(started.elapsed().as_secs());
                    record["identities_unchanged"] =
                        json!(identities_unchanged(&mut client, &identities));
                    record
                }
                _ => {
                    json!({"cycle": worker_index, "pending": "PANEFLOW_BENCH_CONTROLLER was not supplied; worker cycles need the existing worker"})
                }
            };
            worker_cycles.push(record);
            worker_index += 1;
        }
        if !in_idle
            && desktop_enabled
            && desktop_index < plan.desktop_cycles
            && now >= slot(plan.desktop_cycles, desktop_index)
        {
            let desktop = DesktopProcess::start_enabled(home.path(), &idle_sessions);
            let ready_ms = desktop.restored_ms;
            drop(desktop);
            desktop_cycles.push(json!({
                "cycle": desktop_index,
                "elapsed_s": started.elapsed().as_secs(),
                "ready_ms": ready_ms,
                "identities_unchanged": identities_unchanged(&mut client, &identities),
            }));
            desktop_index += 1;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    samples.push(endurance_sample(
        &mut client,
        host_pid,
        worker.as_ref().map(|w| w.child.id()),
        &retained,
        started.elapsed(),
        "end",
    ));
    let final_unchanged = identities_unchanged(&mut client, &identities);
    identity_checks
        .push(json!({"elapsed_s": started.elapsed().as_secs(), "unchanged": final_unchanged}));

    let warmed = samples
        .get(1)
        .or(samples.first())
        .cloned()
        .unwrap_or(Value::Null);
    let last = samples.last().cloned().unwrap_or(Value::Null);
    let delta = |key: &str| {
        last[key]
            .as_u64()
            .zip(warmed[key].as_u64())
            .map(|(after, before)| after as f64 - before as f64)
    };
    let allowance = warmed["host_resident_bytes"]
        .as_u64()
        .map(|w| (w as f64 * 0.10).max(workloads::NFR05_FLOOR_BYTES));
    decisions.push(workloads::decide(
        "NFR-05.memory_after_endurance",
        "W08",
        "host resident bytes at the end minus the first post-warmup sample",
        "<= max(16 MiB, 10% of warmed)",
        delta("host_resident_bytes"),
        |v| allowance.is_some_and(|a| v <= a),
        "resident memory is unavailable on this platform sampler",
    ));
    decisions.push(workloads::decide(
        "NFR-05.threads_after_endurance",
        "W08",
        "host threads at the end minus the first post-warmup sample",
        &format!("<= {}", workloads::NFR05_COUNTER_SLACK),
        delta("host_threads"),
        |v| v <= workloads::NFR05_COUNTER_SLACK as f64,
        "thread count is unavailable on this platform sampler",
    ));
    decisions.push(workloads::decide(
        "NFR-05.handles_after_endurance",
        "W08",
        "host handles or file descriptors at the end minus the first post-warmup sample",
        &format!("<= {}", workloads::NFR05_COUNTER_SLACK),
        delta("host_handles"),
        |v| v <= workloads::NFR05_COUNTER_SLACK as f64,
        "handle or descriptor count is unavailable on this platform sampler",
    ));
    decisions.push(workloads::decide(
        "NFR-04.burst_release",
        "W08",
        "bursts whose runtimes released within the reclaim budget",
        &format!(
            "all {} within {} s",
            bursts.len(),
            workloads::NFR04_RECLAIM_S
        ),
        Some(
            bursts
                .iter()
                .filter(|b| !b["runtime_reclaimed_ms"].is_null())
                .count() as f64,
        ),
        |v| v == bursts.len() as f64,
        "",
    ));
    decisions.push(workloads::decide(
        "NFR-11.worker_cycles",
        "W08",
        "worker cycles with unchanged child identities and generations",
        &format!("all {} cycles", plan.worker_cycles),
        worker_executable.is_some().then(|| {
            worker_cycles
                .iter()
                .filter(|c| c["identities_unchanged"] == true)
                .count() as f64
        }),
        |v| v == plan.worker_cycles as f64,
        "PANEFLOW_BENCH_CONTROLLER was not supplied",
    ));
    decisions.push(workloads::decide(
        "NFR-11.desktop_cycles",
        "W08",
        "desktop detach/reopen cycles with unchanged child identities and generations",
        &format!("all {} cycles", plan.desktop_cycles),
        desktop_enabled.then(|| {
            desktop_cycles
                .iter()
                .filter(|c| c["identities_unchanged"] == true)
                .count() as f64
        }),
        |v| v == plan.desktop_cycles as f64,
        "PANEFLOW_BENCH_DESKTOP was not supplied",
    ));
    decisions.push(workloads::decide(
        "NFR-11.idle_first_input",
        "W08",
        "echoes of the first input on the control connection left untouched for the idle interval",
        "== 1",
        first_input["echoes"].as_u64().map(|v| v as f64),
        |v| v == 1.0,
        "the idle interval did not elapse within the run",
    ));
    decisions.push(workloads::decide(
        "NFR-12.retained_identities",
        "W08",
        "identity checks where every retained session kept its generation and process",
        &format!("all {}", identity_checks.len()),
        Some(
            identity_checks
                .iter()
                .filter(|c| c["unchanged"] == true)
                .count() as f64,
        ),
        |v| v == identity_checks.len() as f64,
        "",
    ));
    let ownership_violations = samples
        .iter()
        .filter(|sample| {
            sample["live_runtimes"]
                .as_u64()
                .is_none_or(|live| live > (retained.len() + ENDURANCE_BURST_SIZE) as u64)
                || sample["pending_launches"]
                    .as_u64()
                    .is_none_or(|pending| pending > ENDURANCE_BURST_SIZE as u64)
                || sample["retained_descendants_unresolved"]
                    .as_u64()
                    .is_none_or(|n| n > 0)
        })
        .count();
    decisions.push(workloads::decide(
        "NFR-12.ownership_counters",
        "W08",
        "samples where live runtimes, pending launches, or unresolved descendants exceeded the retained set plus one burst",
        "== 0, and the final sample owns exactly the retained set",
        Some(
            ownership_violations as f64
                + if last["live_runtimes"] == json!(retained.len()) && last["pending_launches"] == json!(0) { 0.0 } else { 1.0 },
        ),
        |v| v == 0.0,
        "",
    ));

    echo.finish(&mut client);
    for session in &idle_sessions {
        client.stop(session, None).unwrap();
    }
    drop(idle_control);
    drop(worker);
    let shutdown = shutdown_host(
        &mut decisions,
        client,
        &host_identity,
        home.path(),
        &endpoint,
        &hello,
        &ledger,
    );
    seed_failure(&mut decisions);
    let prior = workloads::prior_failures();
    let document = document(
        "complete",
        &samples,
        &bursts,
        &worker_cycles,
        &desktop_cycles,
        &first_input,
        &identity_checks,
        &decisions,
        json!({"fixtures": shutdown, "comparison": {"text": compare_endurance(&decisions), "baseline": Value::Null}}),
    );
    write_document(&path, &document);
    println!("result: {}", path.display());
    print!("{}", compare_endurance(&decisions));
    assert_eq!(
        document["source_unchanged_during_measurement"], true,
        "source changed during measurement; result is not candidate-qualified"
    );
    if let Err(failures) = verdict(&decisions, &prior) {
        panic!(
            "endurance thresholds failed; the artifact is retained at {}:\n{}",
            path.display(),
            failures.join("\n")
        );
    }
}

fn kill_signal_delivered(pid: u32) -> bool {
    let status = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .status()
    } else {
        Command::new("kill").args(["-9", &pid.to_string()]).status()
    };
    status.unwrap().success()
}

fn kill_hard(pid: u32) {
    assert!(kill_signal_delivered(pid), "process {pid} is killed");
}

fn kill_unless_already_gone(process: &paneflow_host::ProcessIdentity) {
    assert!(
        kill_signal_delivered(process.pid) || !process.is_provably_live(),
        "process {} is killed",
        process.pid
    );
}

#[test]
fn an_acknowledged_durable_hook_survives_a_host_kill() {
    allow_breakaway_like_the_desktop_does();
    let home = tempfile::tempdir().unwrap();
    seed_home(home.path());
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let adoption = bootstrap::ensure_host_running(home.path(), &host_executable(), "hook-kill")
        .expect("the detached host starts");
    let host_identity = paneflow_host::ProcessIdentity::capture(adoption.identity.pid);
    let mut client = HostClient::connect(&endpoint, &ClientHello::local("hook-kill")).unwrap();
    let session = client
        .create(&CreateSession {
            shell: Some(fixture_executable().display().to_string()),
            args: vec!["idle".to_string()],
            cwd: Some(std::env::temp_dir().display().to_string()),
            cols: Some(80),
            rows: Some(24),
            ..CreateSession::default()
        })
        .expect("the fixture session starts")
        .manifest
        .session;
    let mut hook =
        HostClient::connect(&endpoint, &ClientHello::control("hook-kill-ai-hook")).unwrap();
    let ack = hook
        .call(
            paneflow_host::protocol::METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "runtime_generation": 1,
                "kind": "ai.prompt_submit",
                "tool": "claude",
                "hook_payload": {"hook_event_name": "UserPromptSubmit", "session_id": "survivor"},
            }),
        )
        .unwrap();
    assert_eq!(ack["accepted"], true, "{ack}");
    assert_eq!(ack["durable"], true, "{ack}");
    drop(hook);
    drop(client);

    kill_hard(adoption.identity.pid);
    let deadline = Instant::now() + Duration::from_secs(10);
    while host_identity.is_provably_live() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!host_identity.is_provably_live(), "the host is gone");

    let on_disk = paneflow_host::manifest::read_manifest(&paneflow_host::manifest::manifest_path(
        home.path(),
        &session,
    ))
    .unwrap();
    if let Some(process) = on_disk
        .process
        .as_ref()
        .filter(|process| process.is_provably_live())
    {
        kill_unless_already_gone(process);
    }
    assert_eq!(on_disk.hook_revision, ack["revision"].as_u64().unwrap());
    let hook = on_disk.last_hook.expect("the acknowledged hook is on disk");
    assert_eq!(hook.hook_event_name, "UserPromptSubmit");
    assert_eq!(hook.provider_session_id.as_deref(), Some("survivor"));
}
