#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_host::bootstrap::{self, Probe};
use paneflow_host::protocol::{ClientHello, ERR_SESSION_LIVE};
use paneflow_host::{HostClient, SessionReconnection, SessionSummary};
use serde_json::json;

#[cfg(windows)]
use paneflow_host::protocol::{METHOD_AGENT_EVENT, METHOD_AGENT_FOLLOW, METHOD_AGENT_SNAPSHOT};
#[cfg(windows)]
use paneflow_ipc_client::IpcTransport;
#[cfg(windows)]
use paneflow_ipc_client::host_control::{HostControl, HostTransport};

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
fn a_gpu_free_client_drives_agents_and_surfaces_while_no_window_is_open() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("paneflow.json"),
        br#"{"ai_unrestricted": true}"#,
    )
    .unwrap();
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let hello = ClientHello::local("agent-test");
    let controller = host_executable();
    allow_breakaway_like_the_desktop_does();

    bootstrap::ensure_host_running(home.path(), &controller, "agent-test")
        .expect("a host starts from the sibling executable");

    let mut owner = HostClient::connect(&endpoint, &hello).unwrap();
    let summary: SessionSummary =
        serde_json::from_value(owner.call("session.create", shell_params()).unwrap()).unwrap();
    let session = summary.manifest.session.to_string();
    let generation = summary.manifest.generation;

    let mut follower = HostControl::connect(&endpoint, "agent-follower")
        .expect("a client with no engine connects");
    let refused = follower
        .request(
            "session.attach",
            json!({"session": session, "generation": 1}),
        )
        .expect_err("a client with no engine cannot attach a grid");
    assert!(refused.contains("terminal engine"), "{refused}");
    let windowless = follower
        .request("workspace.list", json!({}))
        .expect_err("window actions need a controller");
    assert!(windowless.contains("Paneflow window"), "{windowless}");

    let surfaces = follower.request("surface.list", json!({})).unwrap();
    let listed = surfaces["surfaces"]
        .as_array()
        .expect("surfaces")
        .iter()
        .any(|surface| surface["session"] == session);
    assert!(listed, "the host lists its own sessions: {surfaces}");
    assert!(
        follower
            .request("session.text", json!({"session": session}))
            .is_ok(),
        "scrollback is readable with no window open"
    );

    let follow_id = follower
        .write_request(METHOD_AGENT_FOLLOW, json!({}))
        .unwrap();
    let header: serde_json::Value = serde_json::from_str(
        &follower
            .read_stream_line(Duration::from_secs(10))
            .unwrap()
            .expect("a follow header"),
    )
    .unwrap();
    assert_eq!(header["id"].as_u64(), Some(follow_id));
    assert!(header["result"].get("following").is_none());
    assert!(header["result"].get("host_instance").is_none());
    let known = header["result"]["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["session"] == session)
        .expect("the snapshot carries the live session");
    assert!(
        known["last_hook"].is_null(),
        "no event means no hook record"
    );

    let mut hook = HostControl::connect(&endpoint, "agent-hook").expect("the hook connects");
    let accepted = hook
        .request(
            METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "kind": "ai.prompt_submit",
                "runtime_generation": generation,
                "tool": "claude",
                "event_source": "hook",
                "emitted_at_ms": 1_000,
            }),
        )
        .unwrap();
    assert_eq!(accepted["accepted"], true);
    assert_eq!(accepted["revision"], 1);
    assert!(
        accepted["agent"].is_null(),
        "the core answers with the raw record, never with a reduced state"
    );

    let frame = next_agent_event(&mut follower);
    assert_eq!(frame["session"], session.as_str());
    assert_eq!(frame["kind"], "ai.prompt_submit");
    assert!(frame.get("source").is_none());
    assert!(
        frame["agent"].is_null(),
        "a core frame carries the event, never a controller state"
    );

    let waiting = hook
        .request(
            METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "kind": "ai.notification",
                "runtime_generation": generation,
                "tool": "claude",
                "event_source": "hook",
                "source": "retired-source-from-an-older-hook",
                "result": "a retired summary alias",
                "emitted_at_ms": 2_000,
                "hook_payload": {"message": "Needs your approval", "result": "a retired summary alias"},
            }),
        )
        .unwrap();
    assert_eq!(
        waiting["accepted"], true,
        "a retired source key or result alias is ignored, never a reason to refuse"
    );
    let frame = next_agent_event(&mut follower);
    assert_eq!(frame["kind"], "ai.notification");
    assert_eq!(frame["hook_payload"]["message"], "Needs your approval");
    assert!(frame.get("source").is_none());
    assert!(frame["summary"].is_null());

    let stale = hook
        .request(
            METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "kind": "ai.stop",
                "tool": "claude",
                "event_source": "hook",
                "emitted_at_ms": 1_500,
                "runtime_generation": 0,
            }),
        )
        .unwrap();
    assert_eq!(
        stale["accepted"], false,
        "a frame below the manifest generation is refused"
    );
    assert!(
        stale["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("has left")),
        "the refusal names the reason: {stale}"
    );

    let multiline = hook
        .request(
            "surface.send_text",
            json!({"session": session, "text": "line one\nline two"}),
        )
        .expect_err("a bare multiline write is refused without bracketed paste");
    assert!(multiline.contains("bracketed paste"), "{multiline}");
    let sent = hook
        .request(
            "surface.send_text",
            json!({"session": session, "text": "rem hosted", "submit": true}),
        )
        .expect("a single line reaches the hosted shell");
    assert_eq!(sent["sent"], true);
    assert_eq!(sent["agent_target"], true);
    assert_eq!(sent["terminal_bracketed_paste"], false);
    assert_eq!(
        sent["paste"], true,
        "an agent target submits through the paste policy"
    );
    assert_eq!(
        sent["submit_mode"], "deferred_paste_cr",
        "the carriage return follows the configured paste delay, as in the desktop"
    );

    let interrupted = hook
        .request(
            METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "kind": "ai.stop",
                "tool": "claude",
                "event_source": "interrupt",
                "runtime_generation": generation,
                "emitted_at_ms": 3_000,
                "hook_payload": {"last_result": "partial"},
            }),
        )
        .unwrap();
    assert_eq!(interrupted["accepted"], true);
    let frame = next_agent_event(&mut follower);
    assert_eq!(frame["event_source"], "interrupt");

    let transport = HostTransport::connect(&endpoint, "agent-transport")
        .expect("a control transport connects without an engine");
    for _ in 0..2 {
        let status = transport
            .call("host.status", json!({}))
            .expect("each call opens its own short-lived connection");
        assert_eq!(status["resources"]["live_runtimes"], 1);
    }
    assert_eq!(transport.calls(), 2);

    let persisted: SessionSummary = serde_json::from_value(
        owner
            .call("session.inspect", json!({"session": session}))
            .unwrap(),
    )
    .unwrap();
    let hook_record = persisted
        .manifest
        .last_hook
        .expect("the manifest owns the seed");
    assert_eq!(hook_record.tool, "claude");
    assert_eq!(hook_record.hook_event_name, "ai.stop");
    assert_eq!(
        persisted.manifest.host_protocol_version,
        paneflow_host::HOST_PROTOCOL_VERSION
    );
    assert!(
        !persisted.manifest.host_build_id.is_empty(),
        "the manifest carries a diagnostic build id"
    );

    let status = hook.request("host.status", json!({})).unwrap();
    assert!(
        status["helpers"].get("ai_hook_dir").is_some(),
        "the host reports where it looked for the hook helper: {status}"
    );

    let snapshot = hook.request(METHOD_AGENT_SNAPSHOT, json!({})).unwrap();
    let entry = snapshot["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["session"] == session)
        .expect("the session is in the snapshot");
    assert_eq!(entry["last_hook"]["hook_event_name"], "ai.stop");
    assert!(
        entry["agent"].is_null(),
        "the core snapshot never carries a reduced state"
    );

    let _ = owner.call(
        "session.stop",
        json!({"session": session, "generation": summary.manifest.generation.get()}),
    );
    let _ = owner.call("host.shutdown", json!({}));
    assert!(wait_unreachable(home.path(), &endpoint, &hello));
}

#[cfg(windows)]
fn next_agent_event(follower: &mut HostControl) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let line = follower
            .read_stream_line(Duration::from_secs(5))
            .expect("the follow stream stays readable")
            .expect("the follow stream stays open");
        let frame: serde_json::Value = serde_json::from_str(&line).expect("a JSON frame");
        if frame["type"] == "event" {
            return frame;
        }
    }
    panic!("no agent event arrived on the follow stream");
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
