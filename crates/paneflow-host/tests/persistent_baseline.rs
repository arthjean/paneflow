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

#[path = "persistent_baseline/endurance.rs"]
mod endurance;
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

pub const SCHEMA_VERSION: u64 = 3;

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
    document["comparison"] = json!({"text": comparison, "baseline": std::env::var_os("PANEFLOW_BENCH_BASELINE").map(|p| Path::new(&p).display().to_string())});
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
