#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use paneflow_host::bootstrap::Probe;
use paneflow_host::protocol::{ClientHello, METHOD_AGENT_FOLLOW};
use paneflow_host::{CreateSession, HostClient, SessionGeneration, SessionId};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

struct HostGuard {
    child: Child,
}

impl Drop for HostGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn host_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_paneflow-host"))
}

fn claude_settings_path() -> PathBuf {
    PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"))
        .join(".claude")
        .join("settings.json")
}

fn wait_for_host(home: &Path, endpoint: &Path, hello: &ClientHello) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if matches!(
            paneflow_host::probe(home, endpoint, hello),
            Probe::Running(_)
        ) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the real-hook host did not start at {}", endpoint.display());
}

fn start_follow(endpoint: &Path) -> HostControl {
    let mut follower = HostControl::connect(endpoint, "real-claude-hook-follower")
        .expect("the agent follower connects");
    let id = follower
        .write_request(METHOD_AGENT_FOLLOW, json!({}))
        .expect("agent follow starts");
    let header = follower
        .read_stream_line(Duration::from_secs(10))
        .expect("the follow header is readable")
        .expect("the follow stream stays open");
    let header: Value = serde_json::from_str(&header).expect("valid follow header");
    assert_eq!(header["id"].as_u64(), Some(id));
    follower
}

fn wait_for_event(
    follower: &mut HostControl,
    timeout: Duration,
    seen: &mut Vec<Value>,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match follower.read_stream_line(Duration::from_secs(2)) {
            Ok(Some(line)) => {
                let frame: Value = serde_json::from_str(&line).expect("valid agent frame");
                if frame["type"] == "event" {
                    eprintln!("real Claude hook frame: {frame}");
                    seen.push(frame.clone());
                    if predicate(&frame) {
                        return frame;
                    }
                }
            }
            Ok(None) => panic!("the agent follow stream closed"),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => panic!("the agent follow stream failed: {error}"),
        }
    }
    panic!("no matching real Claude hook arrived; observed {seen:#?}");
}

fn create_shell(client: &mut HostClient, cwd: &Path) -> (SessionId, SessionGeneration) {
    let summary = client
        .create(&CreateSession {
            cwd: Some(cwd.display().to_string()),
            shell: Some("cmd.exe".to_string()),
            args: vec!["/Q".to_string(), "/D".to_string()],
            cols: Some(120),
            rows: Some(36),
            ..CreateSession::default()
        })
        .expect("a hosted Windows shell starts");
    (summary.manifest.session, summary.manifest.generation)
}

fn wait_for_claude_prompt(control: &mut HostControl, session: &SessionId) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last_text = String::new();
    while Instant::now() < deadline {
        let response = control
            .request("session.text", json!({"session": session}))
            .expect("Claude terminal text is readable");
        last_text = response["text"].as_str().unwrap_or_default().to_string();
        if last_text.contains("Claude Code") && (last_text.contains('❯') || last_text.contains('>'))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("Claude prompt did not render; final terminal text: {last_text}");
}

fn terminate_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[test]
fn real_claude_hooks_report_a_hand_typed_prompt_and_survive_controller_loss() {
    if std::env::var("PANEFLOW_REAL_CLAUDE").as_deref() != Ok("1") {
        return;
    }
    which::which("claude").expect("Claude Code must be installed");
    let settings_path = claude_settings_path();
    let settings_before = std::fs::read(&settings_path).expect("Claude settings are readable");
    assert!(
        String::from_utf8_lossy(&settings_before).contains("paneflow-ai-hook.exe"),
        "install the Claude integration before running the real test"
    );

    let home = tempfile::tempdir().expect("temporary Paneflow home");
    std::fs::write(
        home.path().join("paneflow.json"),
        br#"{"ai_unrestricted":true}"#,
    )
    .expect("test config");
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let hello = ClientHello::local("real-claude-hook-test");
    let child = Command::new(host_executable())
        .arg("--home")
        .arg(home.path())
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("the real-hook host starts");
    let _host = HostGuard { child };
    wait_for_host(home.path(), &endpoint, &hello);

    let mut owner = HostClient::connect(&endpoint, &hello).expect("the owner connects");
    let mut follower = start_follow(&endpoint);
    let cwd = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root");
    let mut sessions = Vec::new();
    for _ in 0..3 {
        let (session, generation) = create_shell(&mut owner, cwd);
        owner
            .input(&session, generation, b"claude\r")
            .expect("Claude is typed into the hosted shell");
        sessions.push((session, generation));
    }

    let mut seen = Vec::new();
    let expected: BTreeSet<String> = sessions
        .iter()
        .map(|(session, _)| session.to_string())
        .collect();
    let mut started = BTreeSet::new();
    while started != expected {
        let frame = wait_for_event(&mut follower, Duration::from_secs(90), &mut seen, |frame| {
            frame["kind"] == "ai.session_start"
                && frame["hook_payload"]["hook_event_name"] == "HookSeen"
        });
        started.insert(frame["session"].as_str().expect("session id").to_string());
    }

    let prompt = "Use the AskUserQuestion tool now. Ask exactly: Which option should I choose? Provide exactly two options named Alpha and Beta. Do not do anything else.";
    let mut sender =
        HostControl::connect(&endpoint, "real-claude-hook-sender").expect("the sender connects");
    wait_for_claude_prompt(&mut sender, &sessions[0].0);
    let sent = sender
        .request(
            "surface.send_text",
            json!({"session": sessions[0].0, "text": prompt, "submit": true}),
        )
        .expect("the prompt is submitted to Claude");
    assert_eq!(sent["sent"], true);
    wait_for_event(&mut follower, Duration::from_secs(30), &mut seen, |frame| {
        frame["session"].as_str() == Some(sessions[0].0.as_str())
            && frame["kind"] == "ai.prompt_submit"
    });
    wait_for_event(
        &mut follower,
        Duration::from_secs(120),
        &mut seen,
        |frame| {
            frame["session"].as_str() == Some(sessions[0].0.as_str())
                && frame["tool_name"] == "AskUserQuestion"
                && frame["kind"] == "ai.session_start"
                && frame["hook_payload"]["hook_event_name"] == "HookSeen"
        },
    );

    drop(sender);
    drop(follower);
    drop(owner);
    let settings_after = std::fs::read(&settings_path).expect("Claude settings remain readable");
    assert_eq!(
        settings_after, settings_before,
        "three hosted Claude launches and controller loss must not rewrite settings.json"
    );

    let mut cleanup = HostClient::connect(&endpoint, &hello).expect("cleanup reconnects");
    for (session, generation) in sessions {
        cleanup
            .stop(&session, Some(generation))
            .expect("the hosted Claude session stops");
    }
}

#[test]
fn a_hard_kill_of_paneflow_never_rewrites_the_claude_integration() {
    if std::env::var("PANEFLOW_REAL_CLAUDE").as_deref() != Ok("1") {
        return;
    }
    which::which("claude").expect("Claude Code must be installed");
    let settings_path = claude_settings_path();
    let settings_before = std::fs::read(&settings_path).expect("Claude settings are readable");
    assert!(
        String::from_utf8_lossy(&settings_before).contains("paneflow-ai-hook.exe"),
        "install the Claude integration before running the real test"
    );

    let home = tempfile::tempdir().expect("temporary Paneflow home");
    std::fs::write(
        home.path().join("paneflow.json"),
        br#"{"ai_unrestricted":true}"#,
    )
    .expect("test config");
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let hello = ClientHello::local("real-claude-kill-test");
    let mut child = Command::new(host_executable())
        .arg("--home")
        .arg(home.path())
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("the real-hook host starts");
    wait_for_host(home.path(), &endpoint, &hello);

    let mut owner = HostClient::connect(&endpoint, &hello).expect("the owner connects");
    let mut follower = start_follow(&endpoint);
    let cwd = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root");
    let mut shells = Vec::new();
    for _ in 0..3 {
        let summary = owner
            .create(&CreateSession {
                cwd: Some(cwd.display().to_string()),
                shell: Some("cmd.exe".to_string()),
                args: vec!["/Q".to_string(), "/D".to_string()],
                cols: Some(120),
                rows: Some(36),
                ..CreateSession::default()
            })
            .expect("a hosted Windows shell starts");
        let session = summary.manifest.session.clone();
        let generation = summary.manifest.generation;
        let shell_pid = summary.manifest.process.map(|identity| identity.pid);
        owner
            .input(&session, generation, b"claude\r")
            .expect("Claude is typed into the hosted shell");
        shells.push((session, shell_pid));
    }

    let mut seen = Vec::new();
    let expected: BTreeSet<String> = shells
        .iter()
        .map(|(session, _)| session.to_string())
        .collect();
    let mut started = BTreeSet::new();
    while started != expected {
        let frame = wait_for_event(&mut follower, Duration::from_secs(90), &mut seen, |frame| {
            frame["kind"] == "ai.session_start"
        });
        started.insert(frame["session"].as_str().expect("session id").to_string());
    }

    drop(follower);
    drop(owner);
    child
        .kill()
        .expect("the host is terminated without running any cleanup");
    let _ = child.wait();

    let settings_after = std::fs::read(&settings_path).expect("Claude settings remain readable");
    for (_, shell_pid) in &shells {
        if let Some(pid) = shell_pid {
            terminate_tree(*pid);
        }
    }
    assert_eq!(
        settings_after, settings_before,
        "a hard kill with three Claude panes running must leave settings.json byte-identical"
    );
}
