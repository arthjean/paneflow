#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use paneflow_host::protocol::ClientHello;
use paneflow_host::{
    CreateSession, HostClient, SessionLifecycle, SessionReconnection, SessionSummary, bootstrap,
};
use paneflow_ipc_client::host_control::HostControl;
use portable_pty::{CommandBuilder, PtySize};
use serde_json::json;

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
        assert!(!job.is_null(), "the test needs its own Job Object");
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
        assert!(set != 0, "job limits apply");
        let assigned = unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) };
        assert!(assigned != 0, "the test process joins its breakaway-ok job");
    });
}

#[cfg(not(windows))]
fn allow_breakaway_like_the_desktop_does() {}

fn fixture_request(args: &[&str]) -> CreateSession {
    CreateSession {
        shell: Some(fixture_executable().display().to_string()),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: Some(std::env::temp_dir().display().to_string()),
        cols: Some(100),
        rows: Some(30),
        ..CreateSession::default()
    }
}

fn wait_until(deadline: Duration, mut check: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + deadline;
    while Instant::now() < until {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    false
}

struct DetachedHost {
    home: tempfile::TempDir,
    endpoint: PathBuf,
    hello: ClientHello,
}

impl DetachedHost {
    fn start(name: &str) -> Self {
        allow_breakaway_like_the_desktop_does();
        let home = tempfile::tempdir().unwrap();
        let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
        bootstrap::ensure_host_running(home.path(), &host_executable(), name)
            .expect("a detached host starts from the sibling executable");
        Self {
            home,
            endpoint,
            hello: ClientHello::local(name),
        }
    }

    fn client(&self) -> HostClient {
        HostClient::connect(&self.endpoint, &self.hello).unwrap()
    }

    fn stop(self) {
        let mut client = self.client();
        for row in client.list(None).unwrap() {
            if row.live {
                let _ = client.stop(&row.session, None);
            }
        }
        let _ = client.call("host.shutdown", json!({"force": true}));
        drop(client);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !matches!(
            bootstrap::probe(self.home.path(), &self.endpoint, &self.hello),
            bootstrap::Probe::Unreachable(_)
        ) && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[test]
fn h01_a_blocking_wait_and_independent_killer_run_concurrently() {
    let pair = paneflow_host::pty::open(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })
    .expect("the PTY the host uses opens");
    let mut command = CommandBuilder::new(fixture_executable());
    command.arg("delayed-exit");
    command.arg("30000");
    command.arg("0");
    command.cwd(std::env::temp_dir());
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let killer = child.clone_killer();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer = pair.master.take_writer().unwrap();
    let (exit_tx, exit_rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let started = Instant::now();
        let status = child.wait();
        let _ = exit_tx.send((status, started.elapsed()));
    });
    let (eof_tx, eof_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        let started = Instant::now();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = eof_tx.send(started.elapsed());
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        exit_rx.try_recv().is_err(),
        "blocking wait must still own the live root"
    );
    let killed_at = Instant::now();
    let mut killer = killer;
    let kill_result = killer.kill();
    eprintln!("H01 independent killer result: {kill_result:?}");
    #[cfg(unix)]
    kill_result.expect("Unix independent killer signals the retained child");
    let (status, waited) = exit_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("blocking wait reports the signaled process");
    status.expect("the wait handle returns a native exit status");
    assert!(waited >= Duration::from_millis(150));
    assert!(killed_at.elapsed() < Duration::from_secs(5));
    waiter.join().unwrap();
    drop(writer);
    drop(pair.master);
    assert!(
        eof_rx.recv_timeout(Duration::from_secs(10)).is_ok(),
        "EOF is the master release, not the exit"
    );
}

#[test]
fn h01_root_exit_and_pty_hangup_do_not_prove_descendant_exit() {
    let pair = paneflow_host::pty::open(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })
    .unwrap();
    let mut command = CommandBuilder::new(fixture_executable());
    command.args(["descendants-orphan", "1", "600"]);
    command.cwd(std::env::temp_dir());
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer = pair.master.take_writer().unwrap();
    let (pids_tx, pids_rx) = mpsc::channel();
    let (eof_tx, eof_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let mut buffer = [0; 4096];
        let mut sent = false;
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    text.push_str(&String::from_utf8_lossy(&buffer[..count]));
                    if !sent
                        && let Some(line) = text
                            .split_once("fixture descendants ")
                            .map(|(_, rest)| rest)
                    {
                        let pids: Vec<u32> = line
                            .split(|character: char| {
                                !character.is_ascii_digit() && character != ','
                            })
                            .next()
                            .unwrap_or_default()
                            .split(',')
                            .filter_map(|pid| pid.parse().ok())
                            .collect();
                        if !pids.is_empty() {
                            let _ = pids_tx.send(pids);
                            sent = true;
                        }
                    }
                }
            }
        }
        let _ = eof_tx.send(());
    });
    let pids = pids_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let identities: Vec<_> = pids
        .iter()
        .map(|pid| paneflow_host::process::ProcessIdentity::capture(*pid))
        .collect();
    let status = child.wait().unwrap();
    #[cfg(target_os = "macos")]
    let eof_before_cleanup = eof_rx.recv_timeout(Duration::from_secs(5)).is_ok();
    #[cfg(not(target_os = "macos"))]
    let eof_before_cleanup = eof_rx.try_recv().is_ok();
    let descendant_alive = identities
        .iter()
        .all(|identity| identity.is_provably_live());
    for identity in &identities {
        if !identity.is_provably_live() {
            continue;
        }
        #[cfg(windows)]
        let _ = paneflow_host::process::terminate_windows_process_tree(
            identity.pid,
            Instant::now() + Duration::from_secs(2),
        );
        #[cfg(unix)]
        unsafe {
            libc::kill(identity.pid as i32, libc::SIGKILL);
        }
    }
    drop(writer);
    drop(pair.master);
    assert!(status.success());
    assert!(
        descendant_alive,
        "the descendant survives the direct root exit"
    );
    #[cfg(not(target_os = "macos"))]
    assert!(
        !eof_before_cleanup,
        "native child wait completes independently of PTY EOF"
    );
    #[cfg(target_os = "macos")]
    assert!(
        eof_before_cleanup,
        "macOS hangs up the session leader's terminal while the descendant remains alive"
    );
    assert!(wait_until(Duration::from_secs(5), || identities
        .iter()
        .all(|identity| !identity.is_provably_live())));
    if !eof_before_cleanup {
        assert!(eof_rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }
}

#[test]
fn h02_a_requester_that_disconnects_mid_create_leaves_the_host_owning_the_child() {
    let host = DetachedHost::start("ownership-probe");
    let session = paneflow_host::SessionId::new();
    {
        let mut control = HostControl::connect(&host.endpoint, "ownership-probe").unwrap();
        let mut request = fixture_request(&["idle"]);
        request.session = Some(session.clone());
        request
            .env
            .insert("PANEFLOW_TEST_SPAWN_DELAY_MS".into(), "12000".into());
        control
            .write_request("session.create", serde_json::to_value(&request).unwrap())
            .unwrap();
    }
    let mut client = host.client();
    assert!(wait_until(Duration::from_secs(5), || client
        .inspect(&session)
        .is_ok_and(|summary| summary.pending_launch)));
    let mut duplicate = fixture_request(&["idle"]);
    duplicate.session = Some(session.clone());
    assert!(
        client.create(&duplicate).is_err(),
        "pending spawn excludes a second launch"
    );
    std::thread::sleep(Duration::from_secs(10));
    assert!(
        client.inspect(&session).unwrap().pending_launch,
        "host startup deadline retains the registered launch after requester loss"
    );
    let owned = wait_until(Duration::from_secs(20), || {
        client
            .inspect(&session)
            .is_ok_and(|summary| summary.live && summary.owned)
    });
    assert!(
        owned,
        "the launch stays registered and completes although its requester vanished: {:?}",
        client.inspect(&session)
    );
    let summary: SessionSummary = client.inspect(&session).unwrap();
    let process = summary
        .manifest
        .process
        .expect("the child identity is recorded");
    assert!(process.is_provably_live());
    assert_eq!(
        summary.reconnection(&client.identity().host_instance),
        SessionReconnection::Live
    );
    let stopped = client.stop(&session, None).unwrap();
    assert!(matches!(
        stopped.manifest.lifecycle,
        SessionLifecycle::Exited { .. }
    ));
    assert!(!process.is_provably_live());
    host.stop();
}

#[test]
fn descendants_remain_recoverable_after_the_parent_exits() {
    let host = DetachedHost::start("orphan-recovery");
    let mut client = host.client();
    let tree = client
        .create(&fixture_request(&["descendants-orphan", "2", "1200"]))
        .unwrap();
    let pids = wait_for_descendant_pids(&mut client, &tree.manifest.session);
    assert_eq!(pids.len(), 2);
    assert!(wait_until(Duration::from_secs(10), || !tree
        .manifest
        .process
        .unwrap()
        .is_provably_live()));
    let stopped = client
        .stop(&tree.manifest.session, Some(tree.manifest.generation))
        .unwrap();
    assert!(
        !stopped.owns_process(),
        "retry settles retained descendant ownership: {stopped:?}"
    );
    assert!(wait_until(Duration::from_secs(5), || pids
        .iter()
        .all(|pid| !pid_is_alive(*pid))));
    host.stop();
}

#[test]
fn the_fixture_modes_are_driven_through_the_real_host_ipc() {
    let host = DetachedHost::start("fixture-modes");
    let mut client = host.client();

    let echo = client.create(&fixture_request(&["echo"])).unwrap();
    client
        .input(
            &echo.manifest.session,
            echo.manifest.generation,
            b"round trip\r\n",
        )
        .unwrap();
    assert!(wait_until(Duration::from_secs(10), || {
        client
            .call("session.text", json!({"session": echo.manifest.session}))
            .is_ok_and(|text| {
                text["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("round trip"))
            })
    }));

    let split = client
        .create(&fixture_request(&["split-sequences"]))
        .unwrap();
    assert!(wait_until(Duration::from_secs(10), || {
        client
            .call("session.text", json!({"session": split.manifest.session}))
            .is_ok_and(|text| {
                text["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("red plain é done"))
            })
    }));

    let delayed = client
        .create(&fixture_request(&["delayed-exit", "300", "7"]))
        .unwrap();
    assert!(wait_until(Duration::from_secs(10), || {
        client
            .inspect(&delayed.manifest.session)
            .is_ok_and(|summary| {
                matches!(
                    summary.manifest.lifecycle,
                    SessionLifecycle::Exited { code: 7, .. }
                )
            })
    }));

    let blocked = client.create(&fixture_request(&["blocked-stdin"])).unwrap();
    let blocked_process = blocked.manifest.process.unwrap();
    let stopped = client.stop(&blocked.manifest.session, None).unwrap();
    assert!(matches!(
        stopped.manifest.lifecycle,
        SessionLifecycle::Exited { .. }
    ));
    assert!(!blocked_process.is_provably_live());

    let tree = client
        .create(&fixture_request(&["descendants", "2", "60000"]))
        .unwrap();
    let pids = wait_for_descendant_pids(&mut client, &tree.manifest.session);
    assert_eq!(pids.len(), 2, "the fixture announces its descendants");
    let stopped = client.stop(&tree.manifest.session, None).unwrap();
    assert!(
        !stopped.owns_process(),
        "the stop confirms the whole tree, never a group signal alone: {stopped:?}"
    );
    assert!(wait_until(Duration::from_secs(5), || {
        pids.iter().all(|pid| !pid_is_alive(*pid))
    }));

    let flood = client
        .create(&fixture_request(&["flood", "2097152"]))
        .unwrap();
    assert!(wait_until(Duration::from_secs(20), || {
        client
            .inspect(&flood.manifest.session)
            .is_ok_and(|summary| {
                matches!(
                    summary.manifest.lifecycle,
                    SessionLifecycle::Exited { code: 0, .. }
                )
            })
    }));

    for session in [echo.manifest.session, split.manifest.session] {
        let _ = client.stop(&session, None);
    }
    drop(client);
    host.stop();
}

fn wait_for_descendant_pids(
    client: &mut HostClient,
    session: &paneflow_host::SessionId,
) -> Vec<u32> {
    let mut pids = Vec::new();
    wait_until(Duration::from_secs(10), || {
        let Ok(text) = client.call("session.text", json!({"session": session})) else {
            return false;
        };
        let Some(line) = text["text"].as_str().and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("fixture descendants "))
        }) else {
            return false;
        };
        pids = line
            .trim_start_matches("fixture descendants ")
            .split(',')
            .filter_map(|pid| pid.trim().parse().ok())
            .collect();
        !pids.is_empty()
    });
    pids
}

#[cfg(windows)]
fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut code = 0u32;
    let queried = unsafe { GetExitCodeProcess(handle, &mut code) };
    unsafe {
        CloseHandle(handle);
    }
    queried != 0 && code == STILL_ACTIVE as u32
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
    if !alive {
        return false;
    }
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|stat| !stat.contains(") Z "))
        .unwrap_or(true)
}
