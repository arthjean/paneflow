#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use paneflow_host::protocol::ClientHello;
use paneflow_host::{CreateSession, HostClient, HostLifetime, ProcessIdentity, bootstrap};
use serde_json::json;

const ANSWER_WAIT: Duration = Duration::from_secs(30);

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

struct DetachedHost {
    _home: tempfile::TempDir,
    endpoint: PathBuf,
    process: ProcessIdentity,
}

impl DetachedHost {
    fn start() -> Self {
        allow_breakaway_like_the_desktop_does();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("paneflow.json"),
            json!({"ipc": {"scripting": false, "orchestration": false}}).to_string(),
        )
        .unwrap();
        let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
        let adoption = bootstrap::ensure_host_running_for(
            home.path(),
            &host_executable(),
            "genuine-callers-test",
            HostLifetime::EndsWithCaller,
        )
        .expect("a detached host starts from the sibling executable");
        Self {
            _home: home,
            endpoint,
            process: ProcessIdentity::capture(adoption.identity.pid),
        }
    }

    fn spawner(&self) -> HostClient {
        HostClient::connect(&self.endpoint, &ClientHello::local("genuine-callers-test")).unwrap()
    }
}

impl Drop for DetachedHost {
    fn drop(&mut self) {
        if let Ok(mut client) =
            HostClient::connect(&self.endpoint, &ClientHello::local("genuine-callers-test"))
        {
            let _ = client.call("host.shutdown", json!({"force": true}));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.process.is_provably_live() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(40));
        }
    }
}

fn fixture_lines(args: &[&str], until: &str) -> Vec<String> {
    let mut command = Command::new(fixture_executable());
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("PANEFLOW_") {
            command.env_remove(name);
        }
    }
    command.env("PANEFLOW_FIXTURE_SPAWNED_WHOAMI", "1");
    let mut child = command.spawn().expect("the fixture starts");
    let stdout = child.stdout.take().unwrap();
    let (lines, received) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(line.trim_end().to_string()).is_err() {
                return;
            }
        }
    });
    let deadline = Instant::now() + ANSWER_WAIT;
    let mut seen = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match received.recv_timeout(left) {
            Ok(line) => {
                let done = line == until;
                seen.push(line);
                if done {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        seen.iter().any(|line| line == until),
        "the fixture never printed {until}: {seen:?}"
    );
    seen
}

fn value_of<'a>(lines: &'a [String], prefix: &str) -> Option<&'a str> {
    lines.iter().find_map(|line| line.strip_prefix(prefix))
}

fn endpoint_arg(endpoint: &Path) -> String {
    endpoint.display().to_string()
}

#[test]
fn a_process_outside_every_pane_that_is_not_paneflow_is_external_whatever_its_hello_claims() {
    let host = DetachedHost::start();
    let endpoint = endpoint_arg(&host.endpoint);
    for claim in ["engine", "desktop", "worker", "control"] {
        let lines = fixture_lines(&["whoami", "host", &endpoint, claim], "whoami done");
        assert_eq!(
            value_of(&lines, "whoami principal="),
            Some("external"),
            "a {claim} hello: {lines:?}"
        );
    }
    let mut spawner = host.spawner();
    assert_eq!(
        spawner.call("system.whoami", json!({})).unwrap(),
        json!({"principal": "window"}),
        "the process that started the host keeps the window it claims"
    );
}

#[test]
fn a_foreign_process_that_claims_an_engine_cannot_decide_approvals_type_or_stop_the_host() {
    let host = DetachedHost::start();
    let endpoint = endpoint_arg(&host.endpoint);
    let manifest = host
        .spawner()
        .create(&CreateSession {
            shell: Some(fixture_executable().display().to_string()),
            args: vec!["idle".to_string()],
            cwd: Some(std::env::temp_dir().display().to_string()),
            cols: Some(100),
            rows: Some(30),
            ..CreateSession::default()
        })
        .unwrap()
        .manifest;
    let session = manifest.session;
    for (claim, method, params) in [
        (
            "engine",
            "approval.decide",
            json!({"id": 1, "decision": "allow"}),
        ),
        (
            "desktop",
            "approval.decide",
            json!({"id": 1, "decision": "allow"}),
        ),
        ("engine", "host.shutdown", json!({"force": true})),
        ("worker", "host.shutdown", json!({"force": true})),
        (
            "engine",
            "host.grants.set",
            json!({"revision": 1, "grants": []}),
        ),
    ] {
        let lines = fixture_lines(
            &[
                "call",
                "host",
                &endpoint,
                claim,
                method,
                &params.to_string(),
            ],
            "call done",
        );
        assert_eq!(
            value_of(&lines, "call reason="),
            Some("principal_refused"),
            "{method} from a {claim} hello: {lines:?}"
        );
    }
    let typed = json!({
        "session": session.as_str(),
        "generation": manifest.generation,
        "data": paneflow_host::protocol::encode_data(b"echo pf-foreign-typed\r"),
    });
    let lines = fixture_lines(
        &[
            "call",
            "host",
            &endpoint,
            "engine",
            "session.input",
            &typed.to_string(),
        ],
        "call done",
    );
    assert!(
        value_of(&lines, "call error=").is_some(),
        "session.input with the scripting gate closed: {lines:?}"
    );
    let mut spawner = host.spawner();
    assert!(
        !spawner
            .text(&session)
            .unwrap()
            .text
            .contains("pf-foreign-typed"),
        "the refused input never reached the pane"
    );
    assert!(host.process.is_provably_live(), "the host still serves");
}
