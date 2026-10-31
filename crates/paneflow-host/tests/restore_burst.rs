#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use paneflow_config::schema::SessionId;
use paneflow_host::protocol::{ClientHello, ERR_BUSY};
use paneflow_host::{CreateSession, HostClient, HostClientError, ProcessIdentity, bootstrap};
use serde_json::json;

const RESTORED_PANES: usize = 40;
const HISTORY_PANES: usize = 16;
const HISTORY_LINES: &str = "12000";
const SLOW_SPAWN_MS: &str = "250";

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

fn wait_until(deadline: Duration, mut check: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + deadline;
    while Instant::now() < until {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn fixture_request(session: &SessionId, args: &[&str]) -> CreateSession {
    let mut request = CreateSession {
        session: Some(session.clone()),
        shell: Some(fixture_executable().display().to_string()),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: Some(std::env::temp_dir().display().to_string()),
        cols: Some(100),
        rows: Some(30),
        ..CreateSession::default()
    };
    request
        .env
        .insert("PANEFLOW_TEST_SPAWN_DELAY_MS".into(), SLOW_SPAWN_MS.into());
    request
}

struct BurstHost {
    _home: tempfile::TempDir,
    endpoint: PathBuf,
    hello: ClientHello,
    process: ProcessIdentity,
}

impl BurstHost {
    fn start(name: &str) -> Self {
        allow_breakaway_like_the_desktop_does();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("paneflow.json"), "{}").unwrap();
        let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
        bootstrap::ensure_host_running(home.path(), &host_executable(), name)
            .expect("a detached host starts from the sibling executable");
        let hello = ClientHello::local(name);
        let pid = HostClient::connect(&endpoint, &hello)
            .unwrap()
            .identity()
            .pid;
        Self {
            _home: home,
            endpoint,
            hello,
            process: ProcessIdentity::capture(pid),
        }
    }

    fn client(&self) -> Result<HostClient, HostClientError> {
        HostClient::connect(&self.endpoint, &self.hello)
    }
}

impl Drop for BurstHost {
    fn drop(&mut self) {
        if let Ok(mut client) = self.client() {
            let _ = client.call("host.shutdown", json!({"force": true}));
        }
        if wait_until(Duration::from_secs(10), || !self.process.is_provably_live()) {
            return;
        }
        let pid = self.process.pid.to_string();
        #[cfg(windows)]
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid])
            .status();
        #[cfg(unix)]
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid])
            .status();
        wait_until(Duration::from_secs(5), || !self.process.is_provably_live());
    }
}

fn restore_like_resolve_at(
    host: &BurstHost,
    session: &SessionId,
    args: &[&str],
) -> Result<(), HostClientError> {
    let mut client = host.client()?;
    match client.inspect(session) {
        Err(error) if error.is_session_gone() => {}
        Err(error) => return Err(error),
        Ok(summary) => panic!("{session} is unknown before its restore: {summary:?}"),
    }
    let created = client.create(&fixture_request(session, args))?;
    client.attach(session, Some(created.manifest.generation))?;
    Ok(())
}

fn busy_refusals(results: &[Result<(), HostClientError>]) -> Vec<String> {
    results
        .iter()
        .filter_map(|result| result.as_ref().err())
        .filter(|error| error.code() == Some(ERR_BUSY))
        .map(ToString::to_string)
        .collect()
}

#[test]
fn a_restore_burst_against_a_real_host_is_never_refused_for_capacity() {
    let started = Instant::now();
    let host = BurstHost::start("restore-burst");
    let process = host.process;

    let restored: Vec<SessionId> = (0..RESTORED_PANES).map(|_| SessionId::new()).collect();
    let results: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = restored
            .iter()
            .map(|session| scope.spawn(|| restore_like_resolve_at(&host, session, &["idle"])))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    let refused = busy_refusals(&results);
    assert!(
        refused.is_empty(),
        "{} of {RESTORED_PANES} simultaneous restores were refused for capacity: {refused:?}",
        refused.len()
    );
    let failed: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    assert!(failed.is_empty(), "{failed:?}");

    let historic: Vec<SessionId> = (0..HISTORY_PANES).map(|_| SessionId::new()).collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = historic
            .iter()
            .map(|session| {
                scope.spawn(|| {
                    host.client()
                        .unwrap()
                        .create(&fixture_request(session, &["history", HISTORY_LINES]))
                        .map(|_| ())
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap().expect("a history session starts");
        }
    });
    let mut reader = host.client().unwrap();
    for session in &historic {
        assert!(
            wait_until(Duration::from_secs(15), || reader
                .text(session)
                .is_ok_and(|reply| reply.text.contains("fixture history done"))),
            "{session} fills its scrollback"
        );
    }
    let reattached: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = historic
            .iter()
            .map(|session| {
                scope.spawn(|| {
                    let mut client = host.client()?;
                    client.attach(session, None).map(|_| ())
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    let refused = busy_refusals(&reattached);
    assert!(
        refused.is_empty(),
        "{} of {HISTORY_PANES} simultaneous reattachments were refused for capacity: {refused:?}",
        refused.len()
    );
    let failed: Vec<_> = reattached.iter().filter_map(|r| r.as_ref().err()).collect();
    assert!(failed.is_empty(), "{failed:?}");

    drop(reader);
    drop(host);
    assert!(
        !process.is_provably_live(),
        "the host started by the test does not survive it"
    );
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
}
