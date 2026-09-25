#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::protocol::{
    ClientHello, HOST_PROTOCOL_VERSION, METHOD_AGENT_EVENT, METHOD_AGENT_SNAPSHOT,
};
use paneflow_host::{HostClient, SessionSummary};
use serde_json::{Value, json};

const CASE_FILE: &str = include_str!("../../../protocol/host-conformance-v1.json");

fn case_ids() -> Vec<String> {
    let document: Value = serde_json::from_str(CASE_FILE).expect("the case file is valid JSON");
    assert_eq!(document["protocol"], "paneflow-host");
    assert_eq!(
        document["version"].as_u64(),
        Some(u64::from(HOST_PROTOCOL_VERSION)),
        "the case file describes the protocol this core serves"
    );
    document["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .map(|case| {
            case["id"]
                .as_str()
                .expect("every case carries an id")
                .to_string()
        })
        .collect()
}

fn endpoint_for(label: &str) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(format!(
            r"\\.\pipe\paneflow-host-{label}-{}",
            std::process::id()
        ))
    }
    #[cfg(unix)]
    {
        std::env::temp_dir().join(format!("paneflow-host-{label}-{}.sock", std::process::id()))
    }
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

fn read_tail(client: &mut HostClient, session: &paneflow_host::SessionId, from: u64) -> Vec<u8> {
    let mut collected = Vec::new();
    client
        .output(
            session,
            None,
            from,
            false,
            |_, bytes| {
                collected.extend_from_slice(bytes);
                true
            },
            || true,
        )
        .expect("the output stream ends on its own");
    collected
}

fn wait_for_text(
    client: &mut HostClient,
    session: &paneflow_host::SessionId,
    needle: &str,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let tail = String::from_utf8_lossy(&read_tail(client, session, 0)).into_owned();
        if tail.contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
fn every_listed_conformance_case_passes_against_a_real_core() {
    let listed: BTreeSet<String> = case_ids().into_iter().collect();
    let mut covered: BTreeSet<String> = BTreeSet::new();

    let home = tempfile::tempdir().unwrap();
    let endpoint = endpoint_for("conformance");
    let host = paneflow_host::SessionHost::open(home.path(), &endpoint).expect("a core opens");
    let server = paneflow_host::ServerHandle::spawn(Arc::clone(&host), endpoint.clone())
        .expect("the core serves its endpoint");
    let hello = ClientHello::local("conformance");

    let mut client = HostClient::connect(&endpoint, &hello).expect("a controller connects");
    let identity = client.identity().clone();
    assert_eq!(identity.name, "paneflow-host");
    assert_eq!(identity.protocol, HOST_PROTOCOL_VERSION);
    assert!(!identity.build_id.is_empty());
    assert_eq!(identity.pid, std::process::id());
    assert_eq!(identity.engine.engine, "libghostty-vt");
    assert_eq!(identity.home, home.path().display().to_string());
    assert_eq!(identity.endpoint, endpoint.display().to_string());
    covered.insert("bootstrap".to_string());

    let created: SessionSummary = serde_json::from_value(
        client
            .call("session.create", shell_params())
            .expect("a session starts"),
    )
    .unwrap();
    let session = created.manifest.session.clone();
    let generation = created.manifest.generation;

    let listed_sessions = client
        .call("session.list", json!({}))
        .expect("session.list");
    let row = listed_sessions["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["session"] == session.to_string())
        .expect("the new session is listed");
    assert_eq!(
        row["host_protocol_version"].as_u64(),
        Some(u64::from(HOST_PROTOCOL_VERSION))
    );
    assert!(
        row["host_build_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    covered.insert("session_list".to_string());

    let before = read_tail(&mut client, &session, 0).len() as u64;
    covered.insert("output_stream".to_string());

    client
        .input(
            &session,
            generation,
            b"echo paneflow-conformance-marker\r\n",
        )
        .expect("input reaches the pty");
    assert!(
        wait_for_text(&mut client, &session, "paneflow-conformance-marker"),
        "the bytes written come back through the output tail"
    );
    assert!(
        !read_tail(&mut client, &session, before).is_empty(),
        "the tail advances from the requested offset"
    );
    covered.insert("input".to_string());

    client
        .resize(&session, generation, 100, 30)
        .expect("resize applies");
    let resized: SessionSummary = serde_json::from_value(
        client
            .call("session.inspect", json!({"session": session}))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(resized.manifest.launch.cols, 100);
    assert_eq!(resized.manifest.launch.rows, 30);
    covered.insert("resize".to_string());

    let accepted = client
        .call(
            METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "kind": "ai.prompt_submit",
                "tool": "claude",
                "emitted_at_ms": 1_000,
                "runtime_generation": generation,
            }),
        )
        .expect("an in-generation hook frame is accepted");
    assert_eq!(accepted["accepted"], true);

    let snapshot = client
        .call(METHOD_AGENT_SNAPSHOT, json!({}))
        .expect("agent.snapshot");
    let entry = snapshot["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["session"] == session.to_string())
        .expect("the session is in the snapshot");
    assert_eq!(entry["last_hook"]["tool"], "claude");
    assert!(
        entry["agent"].is_null(),
        "the core snapshot carries the raw seed, never a reduced state"
    );
    covered.insert("agent_snapshot".to_string());

    let seed = paneflow_home::host_session_data_dir_in(home.path(), session.as_str())
        .join("last-hook-event.json");
    let seed_before = std::fs::read(&seed).expect("the seed is written on receipt");

    let refused = client
        .call(
            METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "kind": "ai.stop",
                "tool": "claude",
                "emitted_at_ms": 2_000,
                "runtime_generation": 0,
            }),
        )
        .expect("a rejected frame is an answer, not a transport failure");
    assert_eq!(refused["accepted"], false);
    assert!(
        refused["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "the refusal names its reason"
    );
    assert_eq!(
        std::fs::read(&seed).expect("the seed survives"),
        seed_before,
        "a refused frame is never persisted"
    );
    covered.insert("hook_ingress_rejection".to_string());

    let _ = client.call(
        "session.stop",
        json!({"session": session, "generation": generation}),
    );
    drop(client);
    let _ = server.stop();

    assert_eq!(
        covered, listed,
        "every case in protocol/host-conformance-v1.json runs against the real core"
    );
}

#[test]
fn an_incompatible_hello_is_refused_with_the_unchanged_message() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = endpoint_for("incompatible-hello");
    let host = paneflow_host::SessionHost::open(home.path(), &endpoint).expect("a core opens");
    let server = paneflow_host::ServerHandle::spawn(Arc::clone(&host), endpoint.clone())
        .expect("the core serves its endpoint");

    let mut newer = ClientHello::local("future-client");
    newer.protocol = HOST_PROTOCOL_VERSION + 1;
    let refused = HostClient::connect(&endpoint, &newer)
        .err()
        .expect("a newer protocol is refused");
    assert_eq!(
        refused.to_string(),
        format!(
            "local host incompatible: client future-client is incompatible with this host: host protocol {} does not match the expected protocol {HOST_PROTOCOL_VERSION}",
            HOST_PROTOCOL_VERSION + 1
        )
    );

    let mut foreign = ClientHello::local("foreign-engine");
    let mut engine = foreign
        .engine
        .clone()
        .expect("a local hello offers its engine");
    let expected_sha = engine.source_sha.clone();
    engine.source_sha = "0".repeat(40);
    foreign.engine = Some(engine);
    let refused = HostClient::connect(&endpoint, &foreign)
        .err()
        .expect("a foreign engine is refused");
    assert_eq!(
        refused.to_string(),
        format!(
            "local host incompatible: client foreign-engine is incompatible with this host: terminal engine source_sha mismatch: expected {expected_sha}, host offers {}",
            "0".repeat(40)
        )
    );

    drop(server);
}
