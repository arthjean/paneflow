#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::protocol::{ClientHello, METHOD_AGENT_EVENT};
use paneflow_host::{HostClient, SessionSummary};
use paneflow_ipc_client::host_control::ControlConnectError;
use paneflow_serve::controller::{Controller, ControllerError, FollowFrame, FollowSession};
use paneflow_serve::protocol::{
    CAPABILITY_AGENT_UNREAD, CAPABILITY_SESSION_RUNTIME_RESUME, WORKER_PROTOCOL_VERSION,
};
use serde_json::{Value, json};

const CONFORMANCE: &str = include_str!("../../../protocol/controller-conformance-v1.json");

const FRAME_WAIT: Duration = Duration::from_secs(10);

fn listed_cases() -> Vec<String> {
    let document: Value = serde_json::from_str(CONFORMANCE).expect("the conformance file parses");
    assert_eq!(document["protocol"], "paneflow-worker");
    assert_eq!(
        document["version"].as_u64(),
        Some(u64::from(WORKER_PROTOCOL_VERSION))
    );
    document["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .map(|case| {
            case["id"]
                .as_str()
                .expect("every case carries an id")
                .to_owned()
        })
        .collect()
}

fn shell_params() -> Value {
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

fn next_event(stream: &mut FollowSession) -> Value {
    let deadline = Instant::now() + FRAME_WAIT;
    while Instant::now() < deadline {
        match stream.next(Duration::from_millis(200)) {
            FollowFrame::Event(event) => return *event,
            FollowFrame::Disconnected(reason) => {
                panic!("the follow stream dropped while an event was expected: {reason}")
            }
            _ => {}
        }
    }
    panic!("no agent.event frame arrived within {FRAME_WAIT:?}");
}

fn first_bootstrap(stream: &mut FollowSession) -> paneflow_serve::Bootstrap {
    match stream.next(Duration::from_millis(200)) {
        FollowFrame::Bootstrap(bootstrap) => *bootstrap,
        other => panic!("the first frame of a follow is a bootstrap, not {other:?}"),
    }
}

fn row<'a>(sessions: &'a [Value], session: &str) -> &'a Value {
    sessions
        .iter()
        .find(|row| row["session"] == session)
        .unwrap_or_else(|| panic!("the projection carries {session}: {sessions:?}"))
}

fn assert_projection_fields(value: &Value) {
    for field in [
        "status",
        "activity_source",
        "unread",
        "updated_at_ms",
        "runtime_id",
    ] {
        assert!(
            value.get(field).is_some(),
            "a Controller row carries {field}: {value}"
        );
    }
    assert!(
        value.get("activity").is_some() || value.get("agent").is_some(),
        "a Controller row carries the reduced activity: {value}"
    );
}

#[test]
fn the_cli_controller_passes_every_listed_conformance_case() {
    let mut listed = listed_cases();
    listed.sort();
    let mut ran: Vec<String> = Vec::new();

    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var(paneflow_home::HOME_ENV, home.path()) };

    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a core opens");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint.clone())
        .expect("the core serves its endpoint");

    let hello = ClientHello::local("controller-conformance");
    let mut owner =
        HostClient::connect(&core_endpoint, &hello).expect("the core accepts a controller");
    let created: SessionSummary = serde_json::from_value(
        owner
            .call("session.create", shell_params())
            .expect("a session starts"),
    )
    .unwrap();
    let session = created.manifest.session.clone();
    let generation = created.manifest.generation;

    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let worker_endpoint = paneflow_home::serve_endpoint_path(home.path());

    ran.push(case_endpoint_is_owner_only(&worker_endpoint));

    let mut stream =
        FollowSession::open(&worker_endpoint).expect("a Controller follows the worker");
    let bootstrap = first_bootstrap(&mut stream);
    ran.push(case_bootstrap_capabilities(&bootstrap));
    ran.push(case_capability_gate_refuses_resume(&worker_endpoint));

    ran.push(case_session_projection_fields(
        &mut owner,
        &mut stream,
        &bootstrap,
        &session,
        generation,
    ));
    ran.push(case_unread_raised_and_acknowledged(
        &mut owner,
        &mut stream,
        &worker_endpoint,
        &session,
        generation,
    ));

    worker.stop();
    ran.push(case_reconnect_without_duplicate_rows(
        &mut stream,
        home.path(),
        &session,
    ));

    let _ = core_server.stop();
    let _ = core;

    ran.sort();
    assert_eq!(
        ran, listed,
        "every case listed in protocol/controller-conformance-v1.json has a runner"
    );
}

fn case_bootstrap_capabilities(bootstrap: &paneflow_serve::Bootstrap) -> String {
    assert!(!bootstrap.resumed, "the first bootstrap is not a resume");
    assert_eq!(
        bootstrap.identity.capabilities,
        paneflow_serve::advertised_capabilities(),
        "the bootstrap advertises exactly the capability file's set"
    );
    assert_eq!(bootstrap.identity.protocol, WORKER_PROTOCOL_VERSION);
    assert!(
        bootstrap
            .identity
            .capabilities
            .contains(&CAPABILITY_AGENT_UNREAD.to_string())
    );
    "bootstrap_capabilities".to_string()
}

fn case_session_projection_fields(
    owner: &mut HostClient,
    stream: &mut FollowSession,
    bootstrap: &paneflow_serve::Bootstrap,
    session: &paneflow_config::schema::SessionId,
    generation: paneflow_config::schema::SessionGeneration,
) -> String {
    let known = row(&bootstrap.sessions, &session.to_string());
    assert_projection_fields(known);
    assert_eq!(known["status"], "idle");
    assert_eq!(known["activity_source"], "none");
    assert_eq!(known["unread"], false);

    owner
        .call(
            METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "kind": "ai.prompt_submit",
                "tool": "claude",
                "emitted_at_ms": 1_000,
                "runtime_generation": generation,
                "hook_payload": {"hook_event_name": "UserPromptSubmit"},
            }),
        )
        .expect("the core accepts the opening hook");
    let event = next_event(stream);
    assert_eq!(event["session"], session.to_string());
    assert_projection_fields(&event);
    assert_eq!(event["status"], "busy");
    assert_eq!(event["activity_source"], "hooks");
    assert_eq!(event["agent"]["state"], "thinking");
    assert_eq!(event["runtime_id"], "com.anthropic.claude-code");
    assert_eq!(event["unread"], false);
    assert!(
        event["updated_at_ms"].as_u64().is_some_and(|at| at > 0),
        "recency is published so a Controller never reads local files: {event}"
    );
    "session_projection_fields".to_string()
}

fn case_unread_raised_and_acknowledged(
    owner: &mut HostClient,
    stream: &mut FollowSession,
    worker_endpoint: &Path,
    session: &paneflow_config::schema::SessionId,
    generation: paneflow_config::schema::SessionGeneration,
) -> String {
    owner
        .call(
            METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "kind": "ai.stop",
                "tool": "claude",
                "emitted_at_ms": 1_100,
                "runtime_generation": generation,
                "hook_payload": {"hook_event_name": "Stop", "last_result": "2 files changed"},
            }),
        )
        .expect("the core accepts the stop hook");
    let settled = next_event(stream);
    assert_eq!(settled["status"], "idle", "unexpected frame: {settled}");
    assert_eq!(settled["notify"]["kind"], "finished");
    assert_eq!(
        settled["unread"], true,
        "a finished notification raises unread on the session it settles: {settled}"
    );

    let mut controller =
        Controller::connect(worker_endpoint).expect("a second Controller connects");
    let snapshot = controller.snapshot().expect("agent.snapshot answers");
    assert_eq!(
        row(&snapshot.sessions, &session.to_string())["unread"],
        true,
        "every Controller sees the same attention queue"
    );

    let answered = controller
        .acknowledge(&[session.to_string()])
        .expect("agent.acknowledge answers");
    assert_eq!(
        answered["acknowledged"],
        json!([session.to_string()]),
        "the worker reports which sessions it lowered: {answered}"
    );

    let lowered = next_event(stream);
    assert_eq!(lowered["session"], session.to_string());
    assert_eq!(
        lowered["unread"], false,
        "the lowered row reaches every follower: {lowered}"
    );

    let after = controller.snapshot().expect("agent.snapshot answers again");
    assert_eq!(row(&after.sessions, &session.to_string())["unread"], false);
    "unread_raised_and_acknowledged".to_string()
}

fn case_capability_gate_refuses_resume(worker_endpoint: &Path) -> String {
    let controller = Controller::connect(worker_endpoint).expect("a Controller connects");
    assert!(
        !controller.advertises(CAPABILITY_SESSION_RUNTIME_RESUME),
        "the worker does not advertise resume"
    );
    let refused = controller
        .require(CAPABILITY_SESSION_RUNTIME_RESUME)
        .expect_err("an unadvertised capability is refused");
    assert!(
        matches!(refused, ControllerError::CapabilityNotAdvertised(ref name) if name == CAPABILITY_SESSION_RUNTIME_RESUME)
    );
    assert_eq!(
        refused.to_string(),
        "capability not advertised: session.runtime.resume"
    );
    "capability_gate_refuses_resume".to_string()
}

fn case_reconnect_without_duplicate_rows(
    stream: &mut FollowSession,
    home: &Path,
    session: &paneflow_config::schema::SessionId,
) -> String {
    let noticed = Instant::now() + FRAME_WAIT;
    let dropped = loop {
        assert!(
            Instant::now() < noticed,
            "the Controller notices the worker going away"
        );
        if let FollowFrame::Disconnected(reason) = stream.next(Duration::from_millis(200)) {
            break reason;
        }
    };
    assert!(!dropped.is_empty(), "the drop carries a reason");

    let replacement = paneflow_serve::open(home).expect("a replacement worker takes the home");
    let started = Instant::now();
    let resumed = loop {
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "a Controller resumes within three seconds of the worker coming back"
        );
        if let FollowFrame::Bootstrap(bootstrap) = stream.next(Duration::from_millis(200)) {
            break *bootstrap;
        }
    };
    assert!(resumed.resumed, "the second bootstrap is a resume");
    assert!(
        resumed
            .sessions
            .iter()
            .any(|entry| entry["session"] == session.to_string()),
        "the fresh bootstrap still carries the whole fleet: {:?}",
        resumed.sessions
    );
    assert!(
        !resumed
            .fresh
            .iter()
            .any(|entry| entry["session"] == session.to_string()),
        "a row whose projection did not move is not replayed after a reconnect: {:?}",
        resumed.fresh
    );
    replacement.stop();
    "reconnect_without_duplicate_rows".to_string()
}

#[cfg(windows)]
const WINDOWS_PIPE_OWNER_ONLY_DACL: &str = "D:P(A;;FA;;;OW)";

#[cfg(windows)]
fn live_pipe_dacl(endpoint: &Path) -> String {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};

    let name: Vec<u16> = endpoint
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let read = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(
        read,
        ERROR_SUCCESS,
        "the live worker pipe at {} has no readable security descriptor (win32 error {read})",
        endpoint.display()
    );

    let mut rendered: windows_sys::core::PWSTR = std::ptr::null_mut();
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut rendered,
            std::ptr::null_mut(),
        )
    };
    assert!(
        converted != 0,
        "the live worker pipe's security descriptor does not render as SDDL"
    );
    let mut end = 0usize;
    while unsafe { *rendered.add(end) } != 0 {
        end += 1;
    }
    let dacl = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(rendered, end) });
    unsafe {
        LocalFree(rendered.cast());
        LocalFree(descriptor.cast());
    }
    dacl
}

fn case_endpoint_is_owner_only(worker_endpoint: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(worker_endpoint)
            .expect("the worker endpoint exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the worker socket is owner-only");
    }
    #[cfg(windows)]
    {
        let dacl = live_pipe_dacl(worker_endpoint);
        assert_eq!(
            dacl, WINDOWS_PIPE_OWNER_ONLY_DACL,
            "the live worker pipe carries a protected DACL with one allow ACE for the owner alone;              the kernel maps the requested GA to FA when it assigns the descriptor"
        );
        for opened in [";;;WD)", ";;;AU)", ";;;BU)", ";;;SY)", ";;;BA)", ";;;IU)"] {
            assert!(
                !dacl.contains(opened),
                "the live worker pipe grants {opened} to a second principal: {dacl}"
            );
        }
    }
    let denied = ControllerError::from_connect(
        worker_endpoint,
        ControlConnectError::Transport(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
    );
    assert!(
        denied.to_string().starts_with("permission denied"),
        "a transport denial reaches the caller as permission denied: {denied}"
    );
    "endpoint_is_owner_only".to_string()
}
