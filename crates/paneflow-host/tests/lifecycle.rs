#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_host::bootstrap::{self, Probe};
use paneflow_host::protocol::{ClientHello, ERR_SESSION_LIVE};
use paneflow_host::{HostClient, SessionReconnection, SessionSummary};
use serde_json::json;

fn host_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_paneflow-host"))
}

#[cfg(windows)]
fn allow_breakaway_like_the_desktop_does() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(join_breakaway_ok_job);
}

#[cfg(windows)]
fn join_breakaway_ok_job() {
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
}

#[cfg(not(windows))]
fn allow_breakaway_like_the_desktop_does() {}

fn wait_unreachable(home: &Path, endpoint: &Path, hello: &ClientHello) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if matches!(
            bootstrap::probe(home, endpoint, hello),
            Probe::Unreachable(_)
        ) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn shell_params() -> serde_json::Value {
    #[cfg(windows)]
    let (shell, args) = ("cmd.exe", vec!["/Q", "/D"]);
    #[cfg(unix)]
    let (shell, args) = ("/bin/sh", vec!["-s"]);
    json!({
        "shell": shell,
        "args": args,
        "cwd": std::env::temp_dir().display().to_string(),
        "cols": 80,
        "rows": 24,
    })
}

#[test]
fn a_detached_host_is_started_once_adopted_afterwards_and_stopped_only_when_idle() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let hello = ClientHello::local("lifecycle-test");
    let controller = host_executable();
    allow_breakaway_like_the_desktop_does();

    let first = bootstrap::ensure_host_running(home.path(), &controller, "lifecycle-test")
        .expect("a host starts from the sibling executable");
    assert!(first.started);
    assert_eq!(
        first.executable.as_deref(),
        Some(controller.display().to_string().as_str())
    );
    let record = bootstrap::read_instance_record(home.path()).expect("instance record written");
    assert_eq!(record.host_instance, first.identity.host_instance);
    assert_ne!(first.identity.pid, std::process::id());

    let second = bootstrap::ensure_host_running(home.path(), &controller, "lifecycle-test")
        .expect("a second bootstrap adopts");
    assert!(!second.started);
    assert_eq!(second.identity.host_instance, first.identity.host_instance);
    assert_eq!(second.identity.pid, first.identity.pid);

    let mut client = HostClient::connect(&endpoint, &hello).unwrap();
    let created = client.call("session.create", shell_params()).unwrap();
    let summary: SessionSummary = serde_json::from_value(created).unwrap();
    assert!(summary.live);
    assert_eq!(
        summary.reconnection(&first.identity.host_instance),
        SessionReconnection::Live
    );
    let child = summary.manifest.process.expect("process identity");
    assert!(child.is_provably_live());
    drop(client);

    let mut client = HostClient::connect(&endpoint, &hello).unwrap();
    let inspected: SessionSummary = serde_json::from_value(
        client
            .call(
                "session.inspect",
                json!({"session": summary.manifest.session}),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(
        inspected.live,
        "a closed viewer connection never ends the session"
    );
    assert_eq!(inspected.manifest.process, Some(child));

    let refused = client.call("host.shutdown", json!({})).unwrap_err();
    assert_eq!(refused.code(), Some(ERR_SESSION_LIVE));
    assert!(
        matches!(
            bootstrap::probe(home.path(), &endpoint, &hello),
            Probe::Running(_)
        ),
        "a refused shutdown leaves the host serving"
    );

    let stopped: SessionSummary = serde_json::from_value(
        client
            .call(
                "session.stop",
                json!({"session": summary.manifest.session, "generation": 1}),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(!stopped.live);
    assert!(!child.is_provably_live(), "the owned process tree is gone");

    let restarted: SessionSummary = serde_json::from_value(
        client
            .call(
                "session.restart",
                json!({"session": summary.manifest.session}),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(restarted.manifest.generation.get(), 2);
    assert!(restarted.manifest.launch.args.is_empty());
    assert!(restarted.live);
    client
        .call(
            "session.stop",
            json!({"session": summary.manifest.session, "generation": 2}),
        )
        .unwrap();

    let stopping = client.call("host.shutdown", json!({})).unwrap();
    assert_eq!(stopping["stopping"], true);
    drop(client);
    assert!(
        wait_unreachable(home.path(), &endpoint, &hello),
        "the host exits once no session is live"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while paneflow_home::host_instance_record_path_in(home.path()).exists()
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !paneflow_home::host_instance_record_path_in(home.path()).exists(),
        "a graceful stop retires the instance record"
    );
    let manifest = paneflow_home::host_session_manifest_path_in(
        home.path(),
        summary.manifest.session.as_str(),
    );
    assert!(manifest.exists(), "session records survive the host");

    let third = bootstrap::ensure_host_running(home.path(), &controller, "lifecycle-test")
        .expect("a fresh host starts after a graceful stop");
    assert!(third.started);
    assert_ne!(third.identity.host_instance, first.identity.host_instance);
    let mut client = HostClient::connect(&endpoint, &hello).unwrap();
    let adopted: SessionSummary = serde_json::from_value(
        client
            .call(
                "session.inspect",
                json!({"session": summary.manifest.session}),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(!adopted.owned);
    assert!(matches!(
        adopted.reconnection(&third.identity.host_instance),
        SessionReconnection::Exited { .. }
    ));
    client.call("host.shutdown", json!({})).unwrap();
    drop(client);
    assert!(wait_unreachable(home.path(), &endpoint, &hello));
}

#[cfg(windows)]
#[test]
fn a_detached_host_inherits_none_of_the_controllers_stray_handles() {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};

    let home = tempfile::tempdir().unwrap();
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let hello = ClientHello::local("inheritance-test");
    allow_breakaway_like_the_desktop_does();

    let exclusive_path = home.path().join("stray-inheritable.txt");
    let stray = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(0)
        .open(&exclusive_path)
        .unwrap();
    let marked = unsafe {
        SetHandleInformation(
            stray.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        )
    };
    assert!(
        marked != 0,
        "the stray handle is inheritable, like a shell pipe"
    );
    assert!(
        std::fs::File::open(&exclusive_path).is_err(),
        "the exclusive handle blocks a second open while it lives"
    );

    let adoption =
        bootstrap::ensure_host_running(home.path(), &host_executable(), "inheritance-test")
            .expect("the host starts");
    assert!(adoption.started);
    drop(stray);
    assert!(
        std::fs::File::open(&exclusive_path).is_ok(),
        "the running host holds no copy of the controller's stray handle"
    );

    let mut client = HostClient::connect(&endpoint, &hello).unwrap();
    client.call("host.shutdown", json!({})).unwrap();
    drop(client);
    assert!(wait_unreachable(home.path(), &endpoint, &hello));
}
