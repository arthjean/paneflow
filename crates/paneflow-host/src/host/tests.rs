use super::*;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

#[test]
fn shutdown_deadline_is_shared_by_stalled_stops_and_keeps_inspection_responsive() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("shared-stop-deadline")).unwrap();
    let mut sessions = Vec::new();
    for _ in 0..3 {
        sessions.push(host.create(shell_request(80, 24)).unwrap().manifest.session);
    }
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let blocked = Arc::clone(&release);
    let entered = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&entered);
    host.set_barrier(Arc::new(move |point| {
        if point == Barrier::StopCommit {
            count.fetch_add(1, Ordering::SeqCst);
            let (lock, wake) = &*blocked;
            let guard = lock.lock().unwrap();
            drop(wake.wait_while(guard, |released| !*released).unwrap());
        }
    }));
    let running = Arc::clone(&host);
    let started = Instant::now();
    let stopping = std::thread::spawn(move || running.request_shutdown(true));
    assert!(wait_until(Duration::from_secs(3), || entered
        .load(Ordering::SeqCst)
        == 3));
    let inspect_started = Instant::now();
    for session in &sessions {
        assert!(host.inspect(session).is_ok());
    }
    assert!(inspect_started.elapsed() < Duration::from_secs(1));
    let result = stopping.join().unwrap();
    let elapsed = started.elapsed();
    let (lock, wake) = &*release;
    *lock.lock().unwrap() = true;
    wake.notify_all();
    host.set_barrier(Arc::new(|_| {}));
    assert!(
        result.is_err(),
        "a pending commit cannot acknowledge shutdown"
    );
    assert!(
        elapsed < STOP_ACTION_BUDGET + Duration::from_secs(1),
        "{elapsed:?}"
    );
    assert!(wait_until(Duration::from_secs(3), || !host.is_shutting_down()));
    for session in sessions {
        host.stop(&session, None).unwrap();
    }
}

fn shell_request(cols: u16, rows: u16) -> CreateSession {
    #[cfg(windows)]
    let (shell, args) = (
        Some("cmd.exe".to_string()),
        vec!["/Q".to_string(), "/D".to_string()],
    );
    #[cfg(unix)]
    let (shell, args) = (Some("/bin/sh".to_string()), Vec::new());
    CreateSession {
        shell,
        args,
        cwd: Some(std::env::temp_dir().display().to_string()),
        cols: Some(cols),
        rows: Some(rows),
        ..CreateSession::default()
    }
}

fn wait_until(deadline: Duration, mut check: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + deadline;
    while Instant::now() < until {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    false
}

#[test]
fn checkpoint_staging_admits_two_captures_and_the_third_waits_for_a_release() {
    let staging = Arc::new(CheckpointStaging::default());
    let soon = || Instant::now() + Duration::from_millis(100);
    let first = staging.admit(1024, soon()).unwrap();
    let second = staging.admit(2048, soon()).unwrap();
    let refused = staging.admit(1, soon());
    assert!(matches!(refused, Err(HostError::Busy(_))));
    assert_eq!(staging.report().active, MAX_CONCURRENT_CHECKPOINTS);
    assert_eq!(staging.report().staged_bytes, 3072);
    assert_eq!(staging.report().refused, 1);
    drop(first);
    let third = staging.admit(4096, soon()).unwrap();
    assert_eq!(staging.report().staged_bytes, 6144);
    let oversized = staging.admit(CHECKPOINT_STAGING_BUDGET_BYTES + 1, soon());
    assert!(matches!(oversized, Err(HostError::Busy(_))));
    let over_budget = staging.admit(CHECKPOINT_STAGING_BUDGET_BYTES - 6144 + 1, soon());
    assert!(matches!(over_budget, Err(HostError::Busy(_))));
    drop(second);
    drop(third);
    let report = staging.report();
    assert_eq!((report.active, report.staged_bytes), (0, 0));
    assert_eq!(report.peak_staged_bytes, 6144);
}

#[test]
fn a_staged_checkpoint_releases_its_admission_when_dropped() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("staging")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    let staged = host.checkpoint(&session, None).unwrap();
    let held = host.resource_report().checkpoints;
    assert_eq!(held.active, 1);
    assert_eq!(held.staged_bytes, staged.snapshot.len());
    assert!(held.staged_bytes > 0);
    drop(staged);
    let released = host.resource_report().checkpoints;
    assert_eq!((released.active, released.staged_bytes), (0, 0));
    let resources = host.resource_report();
    let runtime = resources
        .sessions
        .iter()
        .find(|entry| entry.session == session)
        .unwrap()
        .runtime;
    assert_eq!(
        runtime.tail_budget_bytes,
        crate::protocol::MAX_OUTPUT_TAIL_BYTES
    );
    assert!(runtime.tail_allocated_bytes >= runtime.tail_retained_bytes);
    assert!(runtime.tail_allocated_bytes <= runtime.tail_budget_bytes);
    assert_eq!(
        runtime.input_budget_bytes,
        crate::runtime::MAX_INPUT_QUEUE_BYTES
    );
    host.stop(&session, None).unwrap();
}

#[test]
fn oversized_terminal_dimensions_are_refused_before_reaching_the_engine() {
    assert!(validate_dimensions(80, 24).is_ok());
    assert!(validate_dimensions(4096, 1024).is_ok());
    assert!(matches!(
        validate_dimensions(0, 24),
        Err(HostError::InvalidRequest(_))
    ));
    assert!(matches!(
        validate_dimensions(u16::MAX, u16::MAX),
        Err(HostError::InvalidRequest(_))
    ));
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("dimensions")).unwrap();
    let mut request = shell_request(80, 24);
    request.cols = Some(u16::MAX);
    request.rows = Some(u16::MAX);
    assert!(matches!(
        host.create(request),
        Err(HostError::InvalidRequest(_))
    ));
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    assert!(matches!(
        host.resize(&session, None, u16::MAX, u16::MAX),
        Err(HostError::InvalidRequest(_))
    ));
    assert!(host.inspect(&session).unwrap().live);
    host.stop(&session, None).unwrap();
}

fn exited_manifest(home: &Path, workspace: &WorkspaceId, updated_at_ms: u64) -> SessionId {
    finished_manifest(
        home,
        workspace,
        SessionLifecycle::Exited {
            code: 0,
            signal: None,
        },
        updated_at_ms,
    )
}

fn finished_manifest(
    home: &Path,
    workspace: &WorkspaceId,
    lifecycle: SessionLifecycle,
    updated_at_ms: u64,
) -> SessionId {
    let session = SessionId::new();
    let cwd = home
        .join("worktrees")
        .join("paneflow-a1b2c3d4")
        .join("feat-a-reasonably-long-branch-name")
        .join("crates")
        .join("paneflow-host");
    let manifest = SessionManifest {
        schema: MANIFEST_SCHEMA_VERSION,
        session: session.clone(),
        workspace: Some(workspace.clone()),
        generation: SessionGeneration::FIRST,
        host_instance: HostInstanceToken::new(),
        cwd: cwd.display().to_string(),
        launch: SessionLaunch {
            shell: "sh".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cols: 80,
            rows: 24,
        },
        lifecycle,
        process: None,
        title: Some("claude \u{00b7} feat/a-reasonably-long-branch-name".to_string()),
        current_cwd: Some(cwd.display().to_string()),
        last_hook: None,
        hook_revision: 0,
        generation_started_at_ms: None,
        screen_changed_at_ms: None,
        screen_activity: None,
        menu_prompt_active: false,
        runtime: None,
        final_output: None,
        host_protocol_version: HOST_PROTOCOL_VERSION,
        host_build_id: crate::protocol::host_build_id(),
        created_at_ms: updated_at_ms,
        updated_at_ms,
    };
    crate::manifest::write_manifest(home, &manifest).unwrap();
    session
}

fn summary_at(workspace: Option<&WorkspaceId>, live: bool, updated_at_ms: u64) -> SessionSummary {
    SessionSummary {
        manifest: SessionManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            session: SessionId::new(),
            workspace: workspace.cloned(),
            generation: SessionGeneration::FIRST,
            host_instance: HostInstanceToken::new(),
            cwd: "/tmp".to_string(),
            launch: SessionLaunch {
                shell: "sh".to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
                cols: 80,
                rows: 24,
            },
            lifecycle: if live {
                SessionLifecycle::Running
            } else {
                SessionLifecycle::Exited {
                    code: 0,
                    signal: None,
                }
            },
            process: None,
            title: None,
            current_cwd: None,
            last_hook: None,
            hook_revision: 0,
            generation_started_at_ms: None,
            screen_changed_at_ms: None,
            screen_activity: None,
            menu_prompt_active: false,
            runtime: None,
            final_output: None,
            host_protocol_version: HOST_PROTOCOL_VERSION,
            host_build_id: crate::protocol::host_build_id(),
            created_at_ms: updated_at_ms,
            updated_at_ms,
        },
        live,
        owned: true,
        pending_launch: false,
        launch_operation: None,
        descendants_unresolved: 0,
        durability_error: None,
    }
}

#[test]
fn a_listing_never_drops_a_running_session_however_many_are_open() {
    let workspace = WorkspaceId::new();
    let summaries: Vec<SessionSummary> = (0..40)
        .map(|index| summary_at(Some(&workspace), true, index))
        .collect();

    let windowed = inactive_window(summaries, INACTIVE_ROWS_PER_WORKSPACE);

    assert_eq!(
        windowed.len(),
        40,
        "every running session is listed whatever the window allows"
    );
}

#[test]
fn the_inactive_window_counts_each_workspace_on_its_own() {
    let first = WorkspaceId::new();
    let second = WorkspaceId::new();
    let mut summaries = Vec::new();
    for index in 0..20u64 {
        summaries.push(summary_at(Some(&first), false, index));
        summaries.push(summary_at(Some(&second), false, index));
        summaries.push(summary_at(None, false, index));
    }

    let windowed = inactive_window(summaries, INACTIVE_ROWS_PER_WORKSPACE);

    for wanted in [Some(&first), Some(&second), None] {
        let kept = windowed
            .iter()
            .filter(|summary| summary.manifest.workspace.as_ref() == wanted)
            .count();
        assert_eq!(
            kept, INACTIVE_ROWS_PER_WORKSPACE,
            "each workspace keeps its own preview of finished sessions"
        );
    }
}

#[test]
fn the_inactive_window_keeps_the_most_recent_of_a_workspace() {
    let workspace = WorkspaceId::new();
    let mut summaries: Vec<SessionSummary> = (0..20u64)
        .map(|index| summary_at(Some(&workspace), false, index))
        .collect();
    let newest = summaries[19].manifest.session.clone();
    let oldest = summaries[0].manifest.session.clone();
    summaries.rotate_left(7);

    let windowed = inactive_window(summaries, INACTIVE_ROWS_PER_WORKSPACE);

    assert!(
        windowed.iter().any(|s| s.manifest.session == newest),
        "the freshest finished session is previewed"
    );
    assert!(
        !windowed.iter().any(|s| s.manifest.session == oldest),
        "the stalest finished session falls outside the window"
    );
}

#[test]
fn a_heavy_day_of_agents_still_fits_one_frame() {
    const OPEN_TERMINALS: usize = 50;
    const WORKSPACES: usize = 8;

    let home = tempfile::tempdir().unwrap();
    let endpoint = PathBuf::from("test-endpoint");
    let host = SessionHost::open(home.path(), &endpoint).unwrap();
    let owner = host.instance().clone();

    let mut summaries = Vec::new();
    for _ in 0..OPEN_TERMINALS {
        summaries.push(summary_at(Some(&WorkspaceId::new()), true, now_ms()));
    }
    for _ in 0..WORKSPACES {
        let workspace = WorkspaceId::new();
        for index in 0..40u64 {
            summaries.push(summary_at(Some(&workspace), false, index));
        }
    }

    let rows: Vec<SessionRow> = inactive_window(summaries, INACTIVE_ROWS_PER_WORKSPACE)
        .into_iter()
        .map(|summary| SessionRow::of(summary, &owner))
        .collect();
    assert_eq!(
        rows.len(),
        OPEN_TERMINALS + WORKSPACES * INACTIVE_ROWS_PER_WORKSPACE
    );

    let frame = serde_json::to_vec(&json!({"sessions": rows})).unwrap();
    assert!(
        frame.len() * 2 < crate::protocol::MAX_CONTROL_FRAME_BYTES,
        "{OPEN_TERMINALS} agents plus a preview per workspace must fit a frame twice over, got {} bytes",
        frame.len()
    );
}

#[test]
fn a_session_finished_yesterday_is_forgotten_and_a_fresh_one_is_kept() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = PathBuf::from("test-endpoint");
    let workspace = WorkspaceId::new();
    let now = now_ms();
    let yesterday = exited_manifest(
        home.path(),
        &workspace,
        now - FINISHED_RECORD_MAX_AGE_MS - 60_000,
    );
    let recent = exited_manifest(home.path(), &workspace, now - 60_000);
    let stale_data = paneflow_home::host_session_data_dir_in(home.path(), yesterday.as_str());
    std::fs::create_dir_all(&stale_data).unwrap();
    std::fs::write(stale_data.join("last-hook-event.json"), b"{}").unwrap();

    let host = SessionHost::open(home.path(), &endpoint).unwrap();
    let listed = host.list(None);

    assert!(
        listed.iter().any(|s| s.manifest.session == recent),
        "a session that ended an hour ago is still there after a restart"
    );
    assert!(
        !listed.iter().any(|s| s.manifest.session == yesterday),
        "a session that ended more than a day ago is forgotten"
    );
    assert!(
        !crate::manifest::manifest_path(home.path(), &yesterday).exists(),
        "a forgotten record leaves no file behind"
    );
    assert!(
        !stale_data.exists(),
        "the retention sweep takes the forgotten session's hook seed with it"
    );
}

#[test]
fn a_session_interrupted_before_a_week_away_is_still_there_on_return() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = PathBuf::from("test-endpoint");
    let workspace = WorkspaceId::new();
    let week = 7 * 24 * 60 * 60 * 1000;
    let now = now_ms();
    let interrupted =
        finished_manifest(home.path(), &workspace, SessionLifecycle::Lost, now - week);
    let finished = exited_manifest(home.path(), &workspace, now - week);

    let host = SessionHost::open(home.path(), &endpoint).unwrap();
    let listed = host.list(None);

    assert!(
        listed.iter().any(|s| s.manifest.session == interrupted),
        "a session left running before a week away is waiting on return"
    );
    assert!(
        !listed.iter().any(|s| s.manifest.session == finished),
        "a session that ended on its own that week is gone"
    );
}

#[test]
fn a_session_the_machine_rebooted_under_is_kept_for_a_month() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = PathBuf::from("test-endpoint");
    let workspace = WorkspaceId::new();
    let now = now_ms();
    let kept = finished_manifest(
        home.path(),
        &workspace,
        SessionLifecycle::Lost,
        now - INTERRUPTED_RECORD_MAX_AGE_MS + 60_000,
    );
    let dropped = finished_manifest(
        home.path(),
        &workspace,
        SessionLifecycle::Lost,
        now - INTERRUPTED_RECORD_MAX_AGE_MS - 60_000,
    );

    let host = SessionHost::open(home.path(), &endpoint).unwrap();
    let listed = host.list(None);

    assert!(
        listed.iter().any(|s| s.manifest.session == kept),
        "an interrupted session is kept for a month"
    );
    assert!(
        !listed.iter().any(|s| s.manifest.session == dropped),
        "an interrupted session older than a month is finally forgotten"
    );
}

#[test]
fn a_live_session_is_never_forgotten_however_old_it_is() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = PathBuf::from("test-endpoint");
    let host = SessionHost::open(home.path(), &endpoint).unwrap();
    let live = host.create(shell_request(80, 24)).unwrap();
    let (manifest, durability) = {
        let sessions = host.lock_sessions();
        let record = &sessions[&live.manifest.session];
        (Arc::clone(&record.manifest), Arc::clone(&record.durability))
    };
    host.commit(&manifest, &durability, None, WriteClass::Metadata, |m| {
        m.updated_at_ms = 1
    });

    host.trim_terminated_records();

    assert!(
        host.list(None)
            .iter()
            .any(|s| s.manifest.session == live.manifest.session),
        "an agent that has been running for days is never forgotten"
    );

    let _ = host.stop(&live.manifest.session, None);
}

#[test]
fn a_listing_leaves_the_launch_environment_out_so_many_sessions_still_fit_a_frame() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = PathBuf::from("test-endpoint");
    let host = SessionHost::open(home.path(), &endpoint).unwrap();

    let mut request = shell_request(80, 24);
    request.env = (0..40)
        .map(|i| (format!("LAB_VAR_{i:02}"), "x".repeat(256)))
        .collect();
    let created = host.create(request).unwrap();

    let listed = host.list(None);
    assert_eq!(listed.len(), 1);
    assert!(
        listed[0].manifest.launch.env.is_empty(),
        "a listing must not carry every session's environment"
    );

    let inspected = host.inspect(&created.manifest.session).unwrap();
    assert_eq!(
        inspected.manifest.launch.env.len(),
        40,
        "inspecting one session still answers with its environment"
    );

    let frame = serde_json::to_vec(&json!({"sessions": host.list(None)})).unwrap();
    assert!(
        frame.len() * 32 < crate::protocol::MAX_CONTROL_FRAME_BYTES,
        "one listed session must leave room for many more, got {} bytes",
        frame.len()
    );

    let _ = host.stop(&created.manifest.session, None);
}

#[test]
fn the_viewport_scan_stamps_the_screen_and_flags_an_agent_drawn_menu() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = home.path().join("host.sock");
    let host = SessionHost::open(home.path(), &endpoint).unwrap();
    let created = host.create(shell_request(100, 24)).unwrap();
    let session = created.manifest.session.clone();
    assert!(created.manifest.screen_changed_at_ms.is_none());
    assert!(!created.manifest.menu_prompt_active);

    assert!(
        wait_until(Duration::from_secs(15), || {
            host.inspect(&session)
                .is_ok_and(|summary| summary.manifest.screen_changed_at_ms.is_some())
        }),
        "the scan stamps the first painted screen"
    );
    let stamped = host.inspect(&session).unwrap().manifest;
    assert!(!stamped.menu_prompt_active);
    assert!(
        stamped.runtime.is_none(),
        "a plain shell is never mistaken for an agent runtime"
    );

    let manifest = Arc::clone(&host.lock_sessions()[&session].manifest);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let once = AtomicBool::new(false);
    host.set_barrier(Arc::new(move |point| {
        if point == Barrier::ManifestPersist
            && manifest.lock().unwrap().menu_prompt_active
            && !once.swap(true, Ordering::SeqCst)
        {
            entered_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(15))
                .unwrap();
        }
    }));
    host.input(
        &session,
        Some(SessionGeneration::FIRST),
        b"echo Enter to select - up/down to navigate - Esc to cancel
"
        .to_vec(),
    )
    .unwrap();
    assert!(
        wait_until(Duration::from_secs(15), || {
            host.inspect(&session)
                .is_ok_and(|summary| summary.manifest.menu_prompt_active)
        }),
        "an agent-drawn menu footer reaches the manifest without any hook"
    );
    let asking = host.inspect(&session).unwrap().manifest;
    assert!(asking.screen_changed_at_ms >= stamped.screen_changed_at_ms);
    entered_rx.recv_timeout(Duration::from_secs(15)).unwrap();
    let path = crate::manifest::manifest_path(home.path(), &session);
    let persisted_while_paused = read_manifest(&path).unwrap().menu_prompt_active;
    release_tx.send(()).unwrap();
    host.set_barrier(Arc::new(|_| {}));
    assert!(
        !persisted_while_paused,
        "inspection observes the viewport edge before its persistence completes"
    );
    assert!(wait_until(Duration::from_secs(15), || {
        read_manifest(&path).is_ok_and(|manifest| manifest.menu_prompt_active)
    }));
    assert!(
        read_manifest(&path).unwrap().menu_prompt_active,
        "the edge is persisted, not only held in memory"
    );

    host.stop(&session, None).unwrap();
}

#[test]
fn a_bare_escape_in_a_bound_claude_pane_fences_the_turn_and_the_next_enter_resumes_it() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = home.path().join("host.sock");
    let host = SessionHost::open(home.path(), &endpoint).unwrap();
    let created = host.create(shell_request(100, 24)).unwrap();
    let session = created.manifest.session.clone();
    let directory = host.session_data_dir(&session);
    let subscription = host.subscribe_agents();

    host.input(&session, None, b"\x1b".to_vec()).unwrap();
    std::thread::sleep(crate::session_input::ESCAPE_SETTLE * 2);
    assert_eq!(
        crate::hook_assets::read_cancellation(&directory),
        None,
        "an unbound pane never fences a turn"
    );

    host.bind_runtime(&session, None, Some("com.anthropic.claude-code"))
        .unwrap();
    assert_eq!(
        host.inspect(&session)
            .unwrap()
            .manifest
            .runtime
            .and_then(|runtime| runtime.launch_binding)
            .as_deref(),
        Some("com.anthropic.claude-code")
    );

    host.input(&session, None, b"\x1b".to_vec()).unwrap();
    assert!(
        wait_until(Duration::from_secs(5), || {
            crate::hook_assets::read_cancellation(&directory).is_some()
        }),
        "a bare escape settles into a cancellation marker"
    );
    let fenced = crate::hook_assets::read_cancellation(&directory).unwrap();
    assert_eq!(fenced.runtime_generation, SessionGeneration::FIRST.get());
    assert_eq!(fenced.submitted_at, None);

    let announced = wait_until(Duration::from_secs(5), || {
        matches!(
            subscription.frames.try_recv(),
            Ok(frame) if frame["type"] == "cancellation"
                && frame["session"] == session.to_string()
        )
    });
    assert!(announced, "the fence is announced on the agent bus");

    host.input(&session, None, b"retry\r".to_vec()).unwrap();
    assert!(
        wait_until(Duration::from_secs(5), || {
            crate::hook_assets::read_cancellation(&directory)
                .is_some_and(|marker| marker.submitted_at.is_some())
        }),
        "the next Enter records the resumption in the same marker"
    );

    host.bind_runtime(&session, None, None).unwrap();
    host.input(&session, None, b"\x1b".to_vec()).unwrap();
    std::thread::sleep(crate::session_input::ESCAPE_SETTLE * 2);
    let unbound = crate::hook_assets::read_cancellation(&directory).unwrap();
    assert_eq!(
        unbound.cancelled_at, fenced.cancelled_at,
        "unbinding the runtime retires the fence"
    );

    assert!(
        host.bind_runtime(&session, None, Some("com.example.nope"))
            .is_err(),
        "only a catalog runtime can be bound"
    );

    host.stop(&session, None).unwrap();
}

#[test]
fn a_codex_pane_never_fences_an_escape_because_its_interrupt_hook_settles_the_turn() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = home.path().join("host.sock");
    let host = SessionHost::open(home.path(), &endpoint).unwrap();
    let created = host.create(shell_request(80, 24)).unwrap();
    let session = created.manifest.session.clone();
    host.bind_runtime(&session, None, Some("com.openai.codex"))
        .unwrap();

    host.input(&session, None, b"\x1b".to_vec()).unwrap();
    std::thread::sleep(crate::session_input::ESCAPE_SETTLE * 3);
    assert_eq!(
        crate::hook_assets::read_cancellation(&host.session_data_dir(&session)),
        None
    );
    host.stop(&session, None).unwrap();
}

#[test]
fn the_host_assigns_durable_ids_persists_manifests_and_stops_owned_processes() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = PathBuf::from("test-endpoint");
    let host = SessionHost::open(home.path(), &endpoint).unwrap();
    let record = paneflow_home::host_instance_record_path_in(home.path());
    let identity: HostIdentity = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    assert_eq!(&identity.host_instance, host.instance());
    assert_eq!(identity.protocol, HOST_PROTOCOL_VERSION);
    assert!(host.list(None).is_empty());

    let workspace = WorkspaceId::new();
    let mut request = shell_request(80, 24);
    request.workspace = Some(workspace.clone());
    request.title = Some("shell".to_string());
    let created = host.create(request).unwrap();
    let session = created.manifest.session.clone();
    assert!(created.live);
    assert!(created.owned);
    assert_eq!(created.manifest.lifecycle, SessionLifecycle::Running);
    assert_eq!(created.manifest.generation, SessionGeneration::FIRST);
    assert_eq!(&created.manifest.host_instance, host.instance());
    let process = created.manifest.process.expect("process identity recorded");
    assert!(process.pid != 0);
    assert!(
        process.started_at.is_some(),
        "kernel start time is captured"
    );
    assert_ne!(
        session.to_string(),
        process.pid.to_string(),
        "the durable id is not the pid"
    );
    assert_eq!(
        created.manifest.launch.env.get("PANEFLOW_SESSION_ID"),
        None,
        "host-set variables are not stored as user launch metadata"
    );

    let manifest_path = crate::manifest::manifest_path(home.path(), &session);
    let on_disk = read_manifest(&manifest_path).unwrap();
    assert_eq!(
        SessionManifest {
            title: None,
            updated_at_ms: 0,
            ..on_disk
        },
        SessionManifest {
            title: None,
            updated_at_ms: 0,
            ..created.manifest.clone()
        },
        "the manifest on disk carries the same durable identity as the one in memory"
    );
    assert_eq!(host.list(Some(&workspace)).len(), 1);
    assert!(host.list(Some(&WorkspaceId::new())).is_empty());

    host.input(
        &session,
        Some(SessionGeneration::FIRST),
        b"echo HOST_ROUNDTRIP\r\n".to_vec(),
    )
    .unwrap();
    assert!(wait_until(Duration::from_secs(15), || {
        let slice = host.output(&session, None, 0, 1 << 20).unwrap();
        String::from_utf8_lossy(&slice.data).contains("HOST_ROUNDTRIP")
    }));
    let checkpoint = host
        .checkpoint(&session, Some(SessionGeneration::FIRST))
        .unwrap();
    assert!(checkpoint.offset > 0);
    assert!(matches!(
        host.checkpoint(&session, Some(SessionGeneration::FIRST.next())),
        Err(HostError::GenerationMismatch { .. })
    ));
    assert!(matches!(
        host.inspect(&SessionId::new()),
        Err(HostError::SessionNotFound(_))
    ));

    let stopped = host.stop(&session, Some(SessionGeneration::FIRST)).unwrap();
    assert!(!stopped.live);
    assert!(matches!(
        stopped.manifest.lifecycle,
        SessionLifecycle::Exited { .. }
    ));
    assert!(
        !process.is_provably_live(),
        "the owned process tree is gone"
    );
    assert!(
        wait_until(Duration::from_secs(5), || {
            read_manifest(&manifest_path)
                .is_ok_and(|on_disk| matches!(on_disk.lifecycle, SessionLifecycle::Exited { .. }))
        }),
        "the final lifecycle revision reaches disk without blocking the stop"
    );
    let on_disk = read_manifest(&manifest_path).unwrap();
    assert_eq!(on_disk.session, session, "identity survives the exit");
    let again = host.stop(&session, None).unwrap();
    assert!(!again.live, "stopping an exited session is idempotent");
    assert!(matches!(
        host.input(&session, None, b"x".to_vec()),
        Err(HostError::Runtime(RuntimeError::NotLive)) | Err(HostError::SessionNotLive(_))
    ));
}

#[test]
fn agent_ingress_rejects_old_generations_and_persists_the_accepted_seed() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("agent-ingress")).unwrap();
    let created = host.create(shell_request(80, 24)).unwrap();
    let session = created.manifest.session.clone();
    let accepted = crate::agent::AgentEvent::from_params(&json!({
        "session": session,
        "runtime_generation": 1,
        "kind": "ai.notification",
        "tool": "claude",
        "tool_name": "AskUserQuestion",
        "hook_payload": {
            "hook_event_name": "PermissionRequest",
            "tool_name": "AskUserQuestion",
            "message": "Choose one"
        }
    }))
    .unwrap();
    let subscription = host.subscribe_agents();
    let response = host.ingest_agent_event(&accepted).unwrap();
    assert_eq!(response["accepted"], true);
    assert_eq!(response["durable"], true);
    assert_eq!(response["revision"], 1);
    assert!(
        subscription.frames.try_recv().is_ok(),
        "a fresh event is broadcast"
    );
    let mut unfenced = accepted.clone();
    unfenced.generation = None;
    let rejected = host.ingest_agent_event(&unfenced).unwrap();
    assert_eq!(rejected["accepted"], false);
    assert_eq!(rejected["revision"], 1);
    assert!(subscription.frames.try_recv().is_err());
    let seed_path = paneflow_home::host_session_data_dir_in(home.path(), session.as_str())
        .join("last-hook-event.json");
    let seed: Value = serde_json::from_slice(&std::fs::read(seed_path).unwrap()).unwrap();
    assert_eq!(
        seed,
        json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "AskUserQuestion",
            "runtime_generation": 1,
            "revision": 1
        })
    );

    host.stop(&session, None).unwrap();
    host.restart(&session, None).unwrap();
    while subscription.frames.try_recv().is_ok() {}
    let rejected = host.ingest_agent_event(&accepted).unwrap();
    assert_eq!(rejected["accepted"], false);
    assert_eq!(
        rejected["reason"],
        "the event names a generation this session has left"
    );
    assert!(
        subscription.frames.try_recv().is_err(),
        "a rejected event is never broadcast"
    );
    host.stop(&session, None).unwrap();
}

#[test]
fn forgetting_a_session_takes_its_hook_seed_directory_with_it() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("agent-seed-cleanup")).unwrap();
    let created = host.create(shell_request(80, 24)).unwrap();
    let session = created.manifest.session.clone();
    let event = crate::agent::AgentEvent::from_params(&json!({
        "session": session,
        "runtime_generation": 1,
        "kind": "ai.stop",
        "tool": "claude",
        "hook_payload": {"hook_event_name": "Stop"}
    }))
    .unwrap();
    host.ingest_agent_event(&event).unwrap();
    let session_dir = paneflow_home::host_session_data_dir_in(home.path(), session.as_str());
    assert!(session_dir.join("last-hook-event.json").is_file());

    host.stop(&session, None).unwrap();
    host.remove(&session).unwrap();
    assert!(
        !session_dir.exists(),
        "a forgotten session leaves no hook seed behind at {}",
        session_dir.display()
    );
}

#[test]
fn a_requested_session_id_is_created_once_and_never_silently_replaced() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("test-endpoint")).unwrap();
    let referenced = SessionId::new();
    let created = host
        .create(CreateSession {
            session: Some(referenced.clone()),
            ..shell_request(80, 24)
        })
        .unwrap();
    assert_eq!(created.manifest.session, referenced);
    assert!(created.live);
    assert!(matches!(
        host.create(CreateSession {
            session: Some(referenced.clone()),
            ..shell_request(80, 24)
        }),
        Err(HostError::SessionExists { .. })
    ));
    assert_eq!(host.list(None).len(), 1);
    host.stop(&referenced, None).unwrap();
}

#[test]
fn a_new_host_instance_marks_inherited_running_records_lost_and_never_signals_them() {
    let home = tempfile::tempdir().unwrap();
    let session = {
        let host = SessionHost::open(home.path(), Path::new("first")).unwrap();
        let created = host.create(shell_request(80, 24)).unwrap();
        created.manifest.session
    };
    let manifest_path = crate::manifest::manifest_path(home.path(), &session);
    let before = read_manifest(&manifest_path).unwrap();
    assert_eq!(before.lifecycle, SessionLifecycle::Running);

    let host = SessionHost::open(home.path(), Path::new("second")).unwrap();
    let adopted = host.inspect(&session).unwrap();
    assert!(!adopted.live);
    assert!(!adopted.owned);
    assert_eq!(adopted.manifest.lifecycle, SessionLifecycle::Lost);
    assert_ne!(&adopted.manifest.host_instance, host.instance());
    let subscription = host.subscribe_agents();
    let event = AgentEvent::from_params(&json!({
        "session": session,
        "runtime_generation": adopted.manifest.generation,
        "kind": "ai.prompt_submit",
        "tool": "claude",
    }))
    .unwrap();
    let rejected = host.ingest_agent_event(&event).unwrap();
    assert_eq!(rejected["accepted"], false);
    assert_eq!(
        rejected["reason"],
        "the event belongs to a previous host instance"
    );
    assert_eq!(host.inspect(&session).unwrap().manifest.hook_revision, 0);
    assert!(read_manifest(&manifest_path).unwrap().last_hook.is_none());
    assert!(subscription.frames.try_recv().is_err());
    assert_eq!(
        read_manifest(&manifest_path).unwrap().lifecycle,
        SessionLifecycle::Lost
    );
    assert!(matches!(
        host.stop(&session, None),
        Err(HostError::ProcessUnverified(_))
    ));
    assert!(matches!(
        host.checkpoint(&session, None),
        Err(HostError::SessionNotLive(_))
    ));
    assert!(manifest_path.exists(), "the record stays for inspection");
}

#[test]
fn an_explicit_restart_starts_a_new_generation_as_an_ordinary_shell() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("restart")).unwrap();
    #[cfg_attr(windows, allow(unused_mut))]
    let mut request = shell_request(80, 24);
    #[cfg(unix)]
    {
        request.args = vec!["-s".to_string()];
    }
    assert!(!request.args.is_empty());
    let created = host.create(request).unwrap();
    let session = created.manifest.session.clone();
    assert!(matches!(
        host.restart(&session, None),
        Err(HostError::SessionLive(_))
    ));
    let stopped = host.stop(&session, None).unwrap();
    assert!(!stopped.live);
    assert_eq!(
        stopped.reconnection(host.instance()),
        match stopped.manifest.lifecycle.clone() {
            SessionLifecycle::Exited { code, signal } =>
                SessionReconnection::Exited { code, signal },
            other => panic!("unexpected lifecycle {other:?}"),
        }
    );

    assert!(matches!(
        host.restart(&session, Some(SessionGeneration::FIRST.next())),
        Err(HostError::GenerationMismatch { .. })
    ));
    let restarted = host
        .restart(&session, Some(SessionGeneration::FIRST))
        .unwrap();
    assert_eq!(restarted.manifest.session, session);
    assert_eq!(
        restarted.manifest.generation,
        SessionGeneration::FIRST.next()
    );
    assert!(restarted.live);
    assert!(
        restarted.manifest.launch.args.is_empty(),
        "a restart never replays the recorded command"
    );
    assert_ne!(restarted.manifest.process, stopped.manifest.process);
    assert_eq!(
        restarted.reconnection(host.instance()),
        SessionReconnection::Live
    );
    let on_disk = read_manifest(&crate::manifest::manifest_path(home.path(), &session)).unwrap();
    assert_eq!(on_disk.generation, SessionGeneration::FIRST.next());
    assert!(matches!(
        host.request_shutdown(false),
        Err(HostError::SessionsLive { count: 1 })
    ));
    host.stop(&session, None).unwrap();
    assert_eq!(
        host.request_shutdown(false),
        Ok(ShutdownReport {
            ended: Vec::new(),
            unresolved: Vec::new(),
        })
    );
}

#[test]
fn a_record_owned_by_a_previous_host_reconnects_as_host_replaced() {
    let home = tempfile::tempdir().unwrap();
    let session = {
        let host = SessionHost::open(home.path(), Path::new("first")).unwrap();
        host.create(shell_request(80, 24)).unwrap().manifest.session
    };
    let host = SessionHost::open(home.path(), Path::new("second")).unwrap();
    let adopted = host.inspect(&session).unwrap();
    assert!(matches!(
        adopted.reconnection(host.instance()),
        SessionReconnection::HostReplaced { ref current_owner, ref previous_owner }
            if current_owner == host.instance() && previous_owner == &adopted.manifest.host_instance
    ));
    let restarted = host.restart(&session, None).unwrap();
    assert_eq!(
        restarted.manifest.generation,
        SessionGeneration::FIRST.next()
    );
    assert_eq!(&restarted.manifest.host_instance, host.instance());
    assert!(restarted.owned && restarted.live);
    host.stop(&session, None).unwrap();
    host.retire();
    assert!(!paneflow_home::host_instance_record_path_in(home.path()).exists());
}

#[test]
fn two_hosts_cannot_own_one_home_at_the_same_time() {
    let home = tempfile::tempdir().unwrap();
    let first = SessionHost::open(home.path(), Path::new("first")).unwrap();
    assert!(matches!(
        SessionHost::open(home.path(), Path::new("second")),
        Err(HostError::OwnerBusy(_))
    ));
    drop(first);
    SessionHost::open(home.path(), Path::new("third")).unwrap();
}

#[test]
fn unsupported_manifests_are_left_in_place() {
    let home = tempfile::tempdir().unwrap();
    let dir = paneflow_home::host_sessions_dir_in(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    let garbage = dir.join(format!("{}.json", SessionId::new()));
    std::fs::write(&garbage, b"{\"schema\": 99}").unwrap();
    let host = SessionHost::open(home.path(), Path::new("endpoint")).unwrap();
    assert!(host.list(None).is_empty());
    assert_eq!(std::fs::read(&garbage).unwrap(), b"{\"schema\": 99}");
}

#[test]
fn every_pane_marker_the_host_exports_is_shed_by_a_nested_desktop() {
    let env = launch_env(
        &SessionId::new(),
        SessionGeneration::FIRST,
        Some(&WorkspaceId::new()),
        Path::new("/home/x/.paneflow"),
        Path::new("/run/paneflow-host.sock"),
        Some(&std::env::temp_dir().join("paneflow-helpers")),
        &BTreeMap::new(),
    );
    for key in env.keys().filter(|key| key.starts_with("PANEFLOW_")) {
        assert!(
            key == "PANEFLOW_HOME" || crate::env::PANE_CONTEXT_ENV.contains(&key.as_str()),
            "{key} is exported to panes but a desktop launched from one would inherit it"
        );
    }
}

#[test]
fn launch_env_identifies_the_durable_session_and_drops_forbidden_keys() {
    let session = SessionId::new();
    let workspace = WorkspaceId::new();
    let user = BTreeMap::from([
        ("KEEP_ME".to_string(), "yes".to_string()),
        ("CLAUDECODE".to_string(), "1".to_string()),
        ("LD_PRELOAD".to_string(), "x".to_string()),
        ("TMUX".to_string(), "x".to_string()),
        ("PANEFLOW_SESSION_ID".to_string(), "forged".to_string()),
        ("BAD=NAME".to_string(), "x".to_string()),
    ]);
    let helper_dir = std::env::temp_dir().join("paneflow-helpers");
    let env = launch_env(
        &session,
        SessionGeneration::FIRST,
        Some(&workspace),
        Path::new("/home/x/.paneflow"),
        Path::new("/run/paneflow-host.sock"),
        Some(&helper_dir),
        &user,
    );
    assert_eq!(env.get("KEEP_ME").map(String::as_str), Some("yes"));
    assert_eq!(env.get("PANEFLOW_SESSION_ID"), Some(&session.to_string()));
    assert_eq!(
        env.get("PANEFLOW_SESSION_DIR").map(PathBuf::from),
        Some(
            Path::new("/home/x/.paneflow")
                .join("host")
                .join("session-data")
                .join(session.as_str())
        )
    );
    assert_eq!(
        env.get("PANEFLOW_RUNTIME_GENERATION").map(String::as_str),
        Some("1")
    );
    assert!(
        !env.contains_key("PANEFLOW_WORKSPACE_ID"),
        "the legacy integer marker is never forged from a UUID; the MCP bridge parses it as u64"
    );
    assert_eq!(
        env.get("PANEFLOW_WORKSPACE_UUID"),
        Some(&workspace.to_string()),
        "hooks address the durable workspace, not a GPUI surface"
    );
    assert_eq!(
        env.get("PANEFLOW_HOST_ENDPOINT").map(String::as_str),
        Some("/run/paneflow-host.sock")
    );
    assert_eq!(
        env.get("PANEFLOW_BIN_DIR").map(PathBuf::from),
        Some(helper_dir.clone())
    );
    assert_eq!(
        std::env::split_paths(env.get("PATH").expect("PATH"))
            .next()
            .as_deref(),
        Some(helper_dir.as_path()),
        "the host-local helper directory leads the child PATH"
    );
    assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
    for key in ["CLAUDECODE", "LD_PRELOAD", "TMUX", "BAD=NAME"] {
        assert!(!env.contains_key(key), "{key} must not reach the child");
    }

    let legacy = BTreeMap::from([
        ("PANEFLOW_WORKSPACE_ID".to_string(), "7".to_string()),
        ("PANEFLOW_WORKSPACE_UUID".to_string(), "forged".to_string()),
        (
            "PANEFLOW_HOST_ENDPOINT".to_string(),
            "/tmp/forged.sock".to_string(),
        ),
    ]);
    let env = launch_env(
        &session,
        SessionGeneration::FIRST,
        Some(&workspace),
        Path::new("/home/x/.paneflow"),
        Path::new("/run/paneflow-host.sock"),
        None,
        &legacy,
    );
    assert_eq!(
        env.get("PANEFLOW_WORKSPACE_ID").map(String::as_str),
        Some("7"),
        "a caller-provided workspace marker keeps the existing hook routing"
    );
    assert_eq!(
        env.get("PANEFLOW_WORKSPACE_UUID"),
        Some(&workspace.to_string()),
        "a forged durable workspace never reaches the child"
    );
    assert_eq!(
        env.get("PANEFLOW_HOST_ENDPOINT").map(String::as_str),
        Some("/run/paneflow-host.sock")
    );
    assert!(
        !env.contains_key("PANEFLOW_BIN_DIR"),
        "a missing helper directory is reported, never invented"
    );
}

fn unverified_record(host: &SessionHost, reason: &str) -> SessionId {
    let session = SessionId::new();
    let manifest = SessionManifest {
        schema: MANIFEST_SCHEMA_VERSION,
        session: session.clone(),
        workspace: None,
        generation: SessionGeneration::FIRST,
        host_instance: host.instance().clone(),
        cwd: std::env::temp_dir().display().to_string(),
        launch: SessionLaunch {
            shell: "sh".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cols: 80,
            rows: 24,
        },
        lifecycle: SessionLifecycle::Unverified {
            reason: reason.to_string(),
        },
        process: None,
        title: None,
        current_cwd: None,
        last_hook: None,
        hook_revision: 0,
        generation_started_at_ms: None,
        screen_changed_at_ms: None,
        screen_activity: None,
        menu_prompt_active: false,
        runtime: None,
        final_output: None,
        host_protocol_version: HOST_PROTOCOL_VERSION,
        host_build_id: crate::protocol::host_build_id(),
        created_at_ms: now_ms(),
        updated_at_ms: now_ms(),
    };
    host.lock_sessions().insert(
        session.clone(),
        SessionRecord::fresh(Arc::new(Mutex::new(manifest))),
    );
    session
}

#[test]
fn concurrent_restarts_of_one_generation_commit_at_most_one_new_generation() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("race")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    host.stop(&session, None).unwrap();
    host.set_barrier(Arc::new(|point| {
        if point == Barrier::RestartPersist {
            std::thread::sleep(Duration::from_millis(300));
        }
    }));
    let outcomes: Vec<Result<SessionSummary, HostError>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let host = &host;
                let session = &session;
                scope.spawn(move || host.restart(session, Some(SessionGeneration::FIRST)))
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect()
    });
    let committed = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
    assert_eq!(committed, 1, "exactly one restart commits: {outcomes:?}");
    assert!(
        outcomes.iter().any(|outcome| matches!(
            outcome,
            Err(HostError::GenerationMismatch { .. }) | Err(HostError::LaunchPending(_))
        )),
        "the loser is refused with a typed error: {outcomes:?}"
    );
    let settled = host.inspect(&session).unwrap();
    assert_eq!(settled.manifest.generation, SessionGeneration::FIRST.next());
    assert!(settled.live);
    host.stop(&session, None).unwrap();
}

#[test]
fn a_stop_of_generation_one_never_writes_exited_into_generation_two() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("stale-stop")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    let weak = Arc::downgrade(&host);
    let racing = session.clone();
    let fired = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&fired);
    host.set_barrier(Arc::new(move |point| {
        if point == Barrier::StopCommit
            && counter.fetch_add(1, Ordering::SeqCst) == 0
            && let Some(host) = weak.upgrade()
        {
            host.restart(&racing, Some(SessionGeneration::FIRST))
                .expect("the process of generation one is gone, so the restart is admitted");
        }
    }));
    let after_stop = host.stop(&session, Some(SessionGeneration::FIRST)).unwrap();
    assert_eq!(
        after_stop.manifest.generation,
        SessionGeneration::FIRST.next()
    );
    assert_eq!(
        after_stop.manifest.lifecycle,
        SessionLifecycle::Running,
        "the stale stop outcome is dropped at commit instead of overwriting the new generation"
    );
    assert!(after_stop.live);
    let on_disk = read_manifest(&crate::manifest::manifest_path(home.path(), &session)).unwrap();
    assert_eq!(on_disk.lifecycle, SessionLifecycle::Running);
    host.set_barrier(Arc::new(|_| {}));
    host.stop(&session, None).unwrap();
}

#[test]
fn a_restart_whose_persist_fails_restores_the_prior_record_without_a_stranded_start() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("persist-failure")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    let prior = host.stop(&session, None).unwrap();
    host.persistence.drain(Duration::from_secs(5)).unwrap();
    let sessions_dir = paneflow_home::host_sessions_dir_in(home.path());
    let blocked = sessions_dir.clone();
    host.set_barrier(Arc::new(move |point| {
        if point == Barrier::RestartPersist && blocked.is_dir() {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let replaced = std::fs::remove_dir_all(&blocked)
                    .and_then(|()| std::fs::write(&blocked, b"not a directory"));
                match replaced {
                    Ok(()) => break,
                    Err(_) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => panic!("cannot replace the sessions directory: {error}"),
                }
            }
        }
    }));
    let failed = host.restart(&session, Some(SessionGeneration::FIRST));
    assert!(
        matches!(failed, Err(HostError::Storage(_))),
        "a persist failure is a typed error: {failed:?}"
    );
    let restored = host.inspect(&session).unwrap();
    assert_eq!(restored.manifest.generation, SessionGeneration::FIRST);
    assert_eq!(restored.manifest.lifecycle, prior.manifest.lifecycle);
    assert!(!restored.pending_launch, "no launch stays registered");
    assert!(!restored.live);
    host.set_barrier(Arc::new(|_| {}));
    std::fs::remove_file(&sessions_dir).unwrap();
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let restarted = host
        .restart(&session, Some(SessionGeneration::FIRST))
        .unwrap();
    assert_eq!(
        restarted.manifest.generation,
        SessionGeneration::FIRST.next()
    );
    host.stop(&session, None).unwrap();
}

#[test]
fn a_create_whose_persist_times_out_leaves_no_manifest_behind() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("persist-timeout")).unwrap();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    host.persistence.spawn_exclusive(move || {
        let _ = release_rx.recv_timeout(Duration::from_secs(30));
        Ok(())
    });
    let mut request = shell_request(80, 24);
    let session = SessionId::new();
    request.session = Some(session.clone());
    let failed = host.create(request);
    assert!(
        matches!(failed, Err(HostError::Storage(_))),
        "a stalled writer is a typed storage error: {failed:?}"
    );
    assert!(matches!(
        host.inspect(&session),
        Err(HostError::SessionNotFound(_))
    ));
    release_tx.send(()).unwrap();
    host.persistence.drain(Duration::from_secs(5)).unwrap();
    assert!(
        !crate::manifest::manifest_path(home.path(), &session).exists(),
        "the revision queued before the failure is discarded, not written later"
    );
    assert!(!host.session_data_dir(&session).exists());
}

#[test]
fn a_restart_whose_persist_times_out_keeps_the_prior_record_on_disk() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("restart-timeout")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    let prior = host.stop(&session, None).unwrap();
    host.persistence.drain(Duration::from_secs(5)).unwrap();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    host.persistence.spawn_exclusive(move || {
        let _ = release_rx.recv_timeout(Duration::from_secs(30));
        Ok(())
    });
    let failed = host.restart(&session, Some(SessionGeneration::FIRST));
    assert!(matches!(failed, Err(HostError::Storage(_))), "{failed:?}");
    release_tx.send(()).unwrap();
    host.persistence.drain(Duration::from_secs(5)).unwrap();
    let on_disk = read_manifest(&crate::manifest::manifest_path(home.path(), &session)).unwrap();
    assert_eq!(on_disk.generation, SessionGeneration::FIRST);
    assert_eq!(on_disk.lifecycle, prior.manifest.lifecycle);
    let restored = host.inspect(&session).unwrap();
    assert_eq!(restored.manifest.generation, SessionGeneration::FIRST);
    assert!(!restored.pending_launch);
}

#[test]
fn a_stop_during_the_launch_terminates_the_child_instead_of_publishing_it() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("late-child")).unwrap();
    let weak = Arc::downgrade(&host);
    let fired = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&fired);
    let id = SessionId::new();
    let racing = id.clone();
    host.set_barrier(Arc::new(move |point| {
        if point == Barrier::LaunchCommit
            && counter.fetch_add(1, Ordering::SeqCst) == 0
            && let Some(host) = weak.upgrade()
        {
            assert!(
                matches!(host.stop(&racing, None), Err(HostError::LaunchPending(_))),
                "a stop during the launch is refused as pending, never advertised as done"
            );
            assert!(matches!(
                host.remove(&racing),
                Err(HostError::LaunchPending(_))
            ));
            assert!(matches!(
                host.restart(&racing, None),
                Err(HostError::LaunchPending(_))
            ));
        }
    }));
    let created = host
        .create(CreateSession {
            session: Some(id.clone()),
            ..shell_request(80, 24)
        })
        .unwrap();
    assert!(
        !created.live,
        "the cancelled launch never publishes a live session"
    );
    assert!(!created.pending_launch);
    assert!(matches!(
        created.manifest.lifecycle,
        SessionLifecycle::Exited { .. }
    ));
    let process = created
        .manifest
        .process
        .expect("the late child identity is recorded");
    assert!(!process.is_provably_live(), "the late child was terminated");
    host.set_barrier(Arc::new(|_| {}));
}

#[test]
fn launch_owner_thread_failure_is_reconciled_by_the_existing_scan() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("launch-owner-failure")).unwrap();
    let created = host.create(shell_request(80, 24)).unwrap();
    let session = created.manifest.session.clone();
    host.stop(&session, Some(created.manifest.generation))
        .unwrap();
    let mut spec = SpawnSpec {
        shell: created.manifest.launch.shell.clone(),
        args: Vec::new(),
        cwd: std::env::temp_dir(),
        env: BTreeMap::new(),
        cols: 80,
        rows: 24,
        scrollback_lines: 500,
    };
    spec.env
        .insert("PANEFLOW_TEST_SPAWN_DELAY_MS".into(), "100".into());
    let handle =
        SessionRuntime::launch(spec, created.manifest.generation, Arc::new(|_| {})).unwrap();
    {
        let mut sessions = host.lock_sessions();
        let record = sessions.get_mut(&session).unwrap();
        record.runtime = None;
        record.manifest.lock().unwrap().lifecycle = SessionLifecycle::Starting;
        record.launch = Some(PendingLaunch {
            operation: 99,
            generation: created.manifest.generation,
            cancel: Some(handle.canceller()),
            cancelled: false,
            fallback_owner: None,
        });
    }
    host.fail_launch_owner_spawn.store(true, Ordering::Release);
    host.own_late_launch(session.clone(), 99, handle);
    assert!(host.inspect(&session).unwrap().pending_launch);
    assert!(wait_until(Duration::from_secs(5), || {
        let summary = host.inspect(&session).unwrap();
        !summary.pending_launch && summary.live
    }));
    let summary = host.inspect(&session).unwrap();
    let identity = summary.manifest.process.unwrap();
    assert!(identity.is_provably_live());
    assert!(
        !host
            .stop(&session, Some(summary.manifest.generation))
            .unwrap()
            .owns_process()
    );
    assert!(!identity.is_provably_live());
}

#[test]
fn admission_stops_at_eight_unresolved_launches() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("admission")).unwrap();
    {
        let mut sessions = host.lock_sessions();
        for index in 0..MAX_PENDING_LAUNCHES {
            let session = SessionId::new();
            let mut record = SessionRecord::fresh(Arc::new(Mutex::new(SessionManifest {
                schema: MANIFEST_SCHEMA_VERSION,
                session: session.clone(),
                workspace: None,
                generation: SessionGeneration::FIRST,
                host_instance: host.instance().clone(),
                cwd: std::env::temp_dir().display().to_string(),
                launch: SessionLaunch {
                    shell: "sh".to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    cols: 80,
                    rows: 24,
                },
                lifecycle: SessionLifecycle::Starting,
                process: None,
                title: None,
                current_cwd: None,
                last_hook: None,
                hook_revision: 0,
                generation_started_at_ms: None,
                screen_changed_at_ms: None,
                screen_activity: None,
                menu_prompt_active: false,
                runtime: None,
                final_output: None,
                host_protocol_version: HOST_PROTOCOL_VERSION,
                host_build_id: crate::protocol::host_build_id(),
                created_at_ms: now_ms(),
                updated_at_ms: now_ms(),
            })));
            record.launch = Some(PendingLaunch {
                operation: index as u64 + 1,
                generation: SessionGeneration::FIRST,
                cancel: None,
                cancelled: false,
                fallback_owner: None,
            });
            sessions.insert(session, record);
        }
    }
    assert_eq!(
        host.resource_report().pending_launches,
        MAX_PENDING_LAUNCHES
    );
    assert!(matches!(
        host.create(shell_request(80, 24)),
        Err(HostError::Busy(_))
    ));
    let summaries = host.list(None);
    assert!(
        summaries.iter().all(|summary| summary.pending_launch
            && summary.reconnection(host.instance()) == SessionReconnection::Starting),
        "a pending launch is reported as starting, never as a verified process"
    );
    host.lock_sessions().clear();
}

#[test]
fn a_panicked_runtime_keeps_ownership_until_a_stop_confirms_the_exit() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("panic")).unwrap();
    let created = host.create(shell_request(80, 24)).unwrap();
    let session = created.manifest.session.clone();
    let process = created.manifest.process.unwrap();
    let runtime = host.lock_sessions()[&session].runtime.clone().unwrap();
    runtime.inject_panic();
    assert!(
        wait_until(Duration::from_secs(5), || {
            matches!(
                host.inspect(&session).unwrap().manifest.lifecycle,
                SessionLifecycle::Unverified { .. }
            )
        }),
        "the panic surfaces as an unverified record"
    );
    let unverified = host.inspect(&session).unwrap();
    assert!(unverified.owns_process());
    assert!(matches!(
        unverified.reconnection(host.instance()),
        SessionReconnection::Unverified { .. }
    ));
    assert!(process.is_provably_live(), "no signal was fabricated");
    assert!(matches!(
        host.remove(&session),
        Err(HostError::OwnershipUnresolved { .. })
    ));
    assert!(matches!(
        host.restart(&session, None),
        Err(HostError::OwnershipUnresolved { .. })
    ));
    assert!(matches!(
        host.checkpoint(&session, None),
        Err(HostError::Runtime(RuntimeError::Unverified(_)))
    ));
    let stopped = host.stop(&session, None).unwrap();
    assert!(matches!(
        stopped.manifest.lifecycle,
        SessionLifecycle::Exited { .. }
    ));
    assert!(!process.is_provably_live());
    host.remove(&session).unwrap();
}

#[test]
fn a_forced_shutdown_with_unresolved_ownership_keeps_the_host_serving() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("shutdown")).unwrap();
    let session = unverified_record(&host, "the wait handle was lost");
    let refused = host.request_shutdown(true);
    assert!(
        matches!(
            &refused,
            Err(HostError::SessionsUnresolved { sessions })
                if sessions.len() == 1 && sessions[0].session == session
        ),
        "the shutdown names the unresolved session: {refused:?}"
    );
    assert!(!host.is_shutting_down());
    assert!(host.create(shell_request(80, 24)).is_ok());
    for summary in host.live_sessions() {
        host.stop(&summary.manifest.session, None).unwrap();
    }
    host.lock_sessions().remove(&session);
    assert!(host.request_shutdown(false).is_ok());
    assert!(host.is_shutting_down());
    assert!(matches!(
        host.create(shell_request(80, 24)),
        Err(HostError::ShuttingDown)
    ));
}

#[test]
fn a_scan_waiting_to_persist_cannot_overwrite_a_restarted_generation() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("scan-restart")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    let target = host
        .live_scan_targets()
        .into_iter()
        .find(|target| target.session == session)
        .unwrap();
    host.stop(&session, None).unwrap();
    let weak = Arc::downgrade(&host);
    let named = session.clone();
    let once = AtomicBool::new(false);
    host.set_barrier(Arc::new(move |point| {
        if point == Barrier::ManifestPersist && !once.swap(true, Ordering::SeqCst) {
            weak.upgrade()
                .unwrap()
                .restart(&named, Some(SessionGeneration::FIRST))
                .unwrap();
        }
    }));
    assert!(host.commit_scan(&target, |manifest| manifest.title = Some("old scan".into())));
    host.set_barrier(Arc::new(|_| {}));
    let stored = read_manifest(&crate::manifest::manifest_path(home.path(), &session)).unwrap();
    assert_eq!(stored.generation, SessionGeneration::FIRST.next());
    assert_eq!(stored.lifecycle, SessionLifecycle::Running);
    host.stop(&session, None).unwrap();
}

#[test]
fn a_scan_waiting_to_persist_cannot_recreate_a_removed_record() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("scan-remove")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    let target = host
        .live_scan_targets()
        .into_iter()
        .find(|target| target.session == session)
        .unwrap();
    host.stop(&session, None).unwrap();
    let weak = Arc::downgrade(&host);
    let named = session.clone();
    let once = AtomicBool::new(false);
    host.set_barrier(Arc::new(move |point| {
        if point == Barrier::ManifestPersist && !once.swap(true, Ordering::SeqCst) {
            weak.upgrade().unwrap().remove(&named).unwrap();
        }
    }));
    assert!(host.commit_scan(&target, |manifest| manifest.title = Some("old scan".into())));
    host.set_barrier(Arc::new(|_| {}));
    assert!(!crate::manifest::manifest_path(home.path(), &session).exists());
    assert!(!host.session_data_dir(&session).exists());
    assert!(!host.commit_scan(&target, |_| panic!("removed state cannot be updated")));
}

#[test]
fn shutdown_keeps_unsaved_final_state_owned_until_retry_succeeds() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("shutdown-durability")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    let path = crate::manifest::manifest_path(home.path(), &session);
    let blocked = path.clone();
    host.set_barrier(Arc::new(move |point| {
        if point != Barrier::StopCommit {
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let replaced = std::fs::remove_file(&blocked)
                .or_else(|error| match error.kind() {
                    std::io::ErrorKind::NotFound => Ok(()),
                    _ => Err(error),
                })
                .and_then(|()| std::fs::create_dir(&blocked));
            match replaced {
                Ok(()) => break,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("cannot replace the manifest with a directory: {error}"),
            }
        }
    }));
    host.stop(&session, None).unwrap();
    host.set_barrier(Arc::new(|_| {}));
    assert!(matches!(
        host.request_shutdown(false),
        Err(HostError::Storage(_))
    ));
    assert!(!host.is_shutting_down());
    let summary = host.inspect(&session).unwrap();
    assert!(!summary.owns_process());
    assert!(summary.durability_error.is_some());
    std::fs::remove_dir(&path).unwrap();
    assert!(host.request_shutdown(false).is_ok());
    assert!(host.is_shutting_down());
    assert!(host.inspect(&session).unwrap().durability_error.is_none());
    let stored = read_manifest(&path).unwrap();
    assert!(matches!(stored.lifecycle, SessionLifecycle::Exited { .. }));
}

#[test]
fn a_duplicate_hook_retries_failed_seed_persistence_without_another_notification() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("seed-retry")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    let path = host
        .session_data_dir(&session)
        .join(crate::manifest::LAST_HOOK_EVENT_FILE);
    std::fs::create_dir(&path).unwrap();
    let event = AgentEvent::from_params(&json!({
        "session": session, "runtime_generation": 1,
        "kind": "ai.stop", "tool": "claude", "emitted_at_ms": 42,
    }))
    .unwrap();
    let subscription = host.subscribe_agents();
    let first = host.ingest_agent_event(&event).unwrap();
    assert_eq!(first["durable"], false);
    assert!(subscription.frames.try_recv().is_ok());
    let retry = host.ingest_agent_event(&event).unwrap();
    assert_eq!(retry["durable"], false);
    assert!(subscription.frames.try_recv().is_err());
    std::fs::remove_dir(&path).unwrap();
    let recovered = host.ingest_agent_event(&event).unwrap();
    assert_eq!(recovered["durable"], true);
    assert_eq!(recovered["revision"], first["revision"]);
    assert!(subscription.frames.try_recv().is_err());
    let seed: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(seed["revision"], first["revision"]);
    assert!(host.inspect(&session).unwrap().durability_error.is_none());
    host.stop(&session, None).unwrap();
}

#[test]
fn a_retried_agent_event_after_an_ambiguous_ack_is_acknowledged_but_not_notified_twice() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("receipts")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    let event = crate::agent::AgentEvent::from_params(&json!({
        "session": session,
        "runtime_generation": 1,
        "kind": "ai.stop",
        "tool": "claude",
        "emitted_at_ms": 1_700_000_000_000u64,
        "hook_payload": {"hook_event_name": "Stop"}
    }))
    .unwrap();
    let subscription = host.subscribe_agents();
    let first = host.ingest_agent_event(&event).unwrap();
    assert_eq!(first["revision"], 1);
    assert!(subscription.frames.try_recv().is_ok());
    let retried = host.ingest_agent_event(&event).unwrap();
    assert_eq!(retried["accepted"], true);
    assert_eq!(retried["duplicate"], true);
    assert_eq!(retried["revision"], 1);
    assert_eq!(retried["durable"], true);
    assert!(
        subscription.frames.try_recv().is_err(),
        "a retry never notifies twice"
    );
    assert_eq!(host.inspect(&session).unwrap().manifest.hook_revision, 1);
    let next = crate::agent::AgentEvent::from_params(&json!({
        "session": session,
        "runtime_generation": 1,
        "kind": "ai.stop",
        "tool": "claude",
        "emitted_at_ms": 1_700_000_000_001u64,
        "hook_payload": {"hook_event_name": "Stop"}
    }))
    .unwrap();
    let advanced = host.ingest_agent_event(&next).unwrap();
    assert_eq!(advanced["revision"], 2);
    assert_eq!(subscription.frames.try_recv().unwrap()["revision"], 2);
    host.stop(&session, None).unwrap();
}

#[test]
fn parallel_hook_commits_publish_in_revision_order_before_returning_acknowledgements() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("parallel-hook-order")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    let subscription = host.subscribe_agents();
    let start = Arc::new(std::sync::Barrier::new(3));
    let mut threads = Vec::new();
    for message in ["first", "second"] {
        let host = Arc::clone(&host);
        let session = session.clone();
        let start = Arc::clone(&start);
        threads.push(std::thread::spawn(move || {
            let event = AgentEvent::from_params(&json!({
                "session": session, "runtime_generation": 1,
                "kind": "ai.notification", "tool": "claude", "message": message,
                "hook_payload": {"hook_event_name": "PermissionRequest"},
            }))
            .unwrap();
            start.wait();
            host.ingest_agent_event(&event).unwrap()
        }));
    }
    start.wait();
    let first = subscription
        .frames
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let second = subscription
        .frames
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(first["revision"], 1);
    assert_eq!(second["revision"], 2);
    for thread in threads {
        assert_eq!(thread.join().unwrap()["accepted"], true);
    }
    assert_eq!(
        host.agent_snapshot()[0]
            .last_hook
            .as_ref()
            .unwrap()
            .event
            .as_ref(),
        Some(&second)
    );
    host.stop(&session, None).unwrap();
}

#[test]
fn accepted_hook_frames_fit_the_reload_limit_and_oversized_events_never_commit() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("hook-size-limit")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    let mut event = AgentEvent::from_params(&json!({
        "session": session, "runtime_generation": 1,
        "kind": "ai.prompt_submit", "tool": "claude",
        "hook_payload": {"hook_event_name": "UserPromptSubmit", "padding": "x".repeat(28 * 1024)},
    }))
    .unwrap();
    let accepted = host.ingest_agent_event(&event).unwrap();
    assert_eq!(accepted["durable"], true);
    let path = crate::manifest::manifest_path(home.path(), &session);
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size > 48 * 1024 && size <= crate::manifest::MAX_MANIFEST_BYTES);
    let stored = read_manifest(&path).unwrap();
    assert_eq!(stored.hook_revision, 1);
    event.payload["padding"] = json!("x".repeat(40 * 1024));
    assert!(matches!(
        host.ingest_agent_event(&event),
        Err(HostError::InvalidRequest(_))
    ));
    assert_eq!(host.inspect(&session).unwrap().manifest.hook_revision, 1);
    assert_eq!(read_manifest(&path).unwrap().hook_revision, 1);
    let mut oversized = stored;
    oversized.title = Some("x".repeat(crate::manifest::MAX_MANIFEST_BYTES as usize));
    assert_eq!(
        crate::manifest::write_manifest(home.path(), &oversized)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    assert_eq!(read_manifest(&path).unwrap().hook_revision, 1);
    host.stop(&session, None).unwrap();
}

#[test]
fn a_delayed_seed_or_marker_write_never_recreates_a_removed_session_directory() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("seed-barrier")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    let directory = host.session_data_dir(&session);
    assert!(directory.is_dir());
    let event = crate::agent::AgentEvent::from_params(&json!({
        "session": session,
        "runtime_generation": 1,
        "kind": "ai.stop",
        "tool": "claude",
        "hook_payload": {"hook_event_name": "Stop"}
    }))
    .unwrap();
    host.stop(&session, None).unwrap();
    host.remove(&session).unwrap();
    assert!(!directory.exists());
    assert!(matches!(
        host.ingest_agent_event(&event),
        Err(HostError::SessionNotFound(_))
    ));
    assert!(
        host.commit_marker(&session, SessionGeneration::FIRST, |dir| {
            crate::hook_assets::record_cancellation(dir, 1, std::time::SystemTime::now())
        })
        .is_none(),
        "a marker for a removed session is dropped at the barrier"
    );
    let late_seed = crate::manifest::write_hook_seed(
        home.path(),
        &session,
        &crate::manifest::encode_hook_seed("Stop", None, SessionGeneration::FIRST, 1),
        false,
    );
    assert!(
        late_seed.is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "a late seed write is refused instead of recreating the directory"
    );
    assert!(
        crate::hook_assets::record_cancellation(&directory, 1, std::time::SystemTime::now())
            .unwrap()
            .is_none()
    );
    assert!(
        !directory.exists(),
        "nothing recreated {}",
        directory.display()
    );
}

#[test]
fn a_marker_captured_under_generation_one_is_dropped_once_generation_two_runs() {
    let home = tempfile::tempdir().unwrap();
    let host = SessionHost::open(home.path(), Path::new("marker-generation")).unwrap();
    let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
    host.stop(&session, None).unwrap();
    host.restart(&session, None).unwrap();
    let stale = host.commit_marker(&session, SessionGeneration::FIRST, |_| ());
    assert!(
        stale.is_none(),
        "the captured generation is re-checked at commit"
    );
    let current = host.commit_marker(&session, SessionGeneration::FIRST.next(), |_| ());
    assert!(current.is_some());
    host.stop(&session, None).unwrap();
}
