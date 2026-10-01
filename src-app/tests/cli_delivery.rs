#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use paneflow_config::schema::{SessionGeneration, SessionId, WorkspaceId};
use paneflow_host::protocol::{ClientHello, METHOD_AGENT_EVENT};
use paneflow_host::{HostClient, ServerHandle, SessionHost, SessionSummary};
use serde_json::{Value, json};

const STATE_WAIT: Duration = Duration::from_secs(10);

struct Instance {
    home: tempfile::TempDir,
    core: Arc<SessionHost>,
    server: Option<ServerHandle>,
    worker: Option<paneflow_serve::worker::RunningWorker>,
    owner: HostClient,
    emitted_at: AtomicU64,
}

impl Instance {
    fn open() -> Self {
        let home = tempfile::tempdir().expect("a temporary home");
        for (name, contents) in [
            ("paneflow.json", r#"{"ai_unrestricted": true}"#),
            ("session.json", "{}"),
            ("window-state.json", "{}"),
            ("telemetry_id", "7f03d6ba-1249-4a78-92dc-96f77e8d10a2"),
        ] {
            std::fs::write(home.path().join(name), contents).expect("isolated instance state");
        }
        let endpoint = paneflow_home::host_endpoint_path(home.path());
        let core = SessionHost::open(home.path(), &endpoint).expect("a core opens");
        let server =
            ServerHandle::spawn(Arc::clone(&core), endpoint.clone()).expect("the core serves");
        let owner = HostClient::connect(&endpoint, &ClientHello::local("cli-delivery-test"))
            .expect("the core accepts the window");
        Self {
            home,
            core,
            server: Some(server),
            worker: None,
            owner,
            emitted_at: AtomicU64::new(1_000),
        }
    }

    fn start_worker(&mut self) {
        self.worker =
            Some(paneflow_serve::open(self.home.path()).expect("the worker takes the home"));
    }

    fn create(
        &mut self,
        title: &str,
        workspace: Option<&WorkspaceId>,
    ) -> (SessionId, SessionGeneration) {
        #[cfg(windows)]
        let (shell, args) = ("cmd.exe", vec!["/Q", "/D"]);
        #[cfg(unix)]
        let (shell, args) = ("/bin/sh", Vec::<&str>::new());
        let mut params = json!({
            "shell": shell,
            "args": args,
            "cwd": std::env::temp_dir().display().to_string(),
            "cols": 80,
            "rows": 24,
            "title": title,
        });
        if let Some(workspace) = workspace {
            params["workspace"] = json!(workspace);
        }
        let created: SessionSummary = serde_json::from_value(
            self.owner
                .call("session.create", params)
                .expect("a session starts"),
        )
        .expect("a session summary");
        (created.manifest.session, created.manifest.generation)
    }

    fn hook(&mut self, session: &SessionId, generation: SessionGeneration, kind: &str, name: &str) {
        let emitted_at_ms = self.emitted_at.fetch_add(100, Ordering::Relaxed);
        let reply = self
            .owner
            .call(
                METHOD_AGENT_EVENT,
                json!({
                    "session": session,
                    "kind": kind,
                    "tool": "claude",
                    "emitted_at_ms": emitted_at_ms,
                    "runtime_generation": generation,
                    "hook_payload": {"hook_event_name": name, "tool_name": "Bash"},
                }),
            )
            .expect("the core accepts the hook");
        assert_eq!(reply["accepted"], true, "{reply}");
    }

    fn text(&self, session: &SessionId) -> String {
        self.core
            .text(session)
            .map(|text| text.text)
            .unwrap_or_default()
    }

    fn shows(&self, session: &SessionId, needle: &str) -> bool {
        let deadline = Instant::now() + STATE_WAIT;
        while Instant::now() < deadline {
            if self.text(session).contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn cli(&self, args: &[&str], caller: Option<&SessionId>) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_paneflow"));
        command
            .args(args)
            .env(paneflow_home::HOME_ENV, self.home.path());
        for key in paneflow_host::env::PANE_CONTEXT_ENV {
            command.env_remove(key);
        }
        command.env_remove("PANEFLOW_SOCKET_PATH");
        if let Some(caller) = caller {
            command.env("PANEFLOW_SESSION_ID", caller.to_string());
        }
        command
    }

    fn run(&self, args: &[&str], caller: Option<&SessionId>) -> Output {
        self.cli(args, caller)
            .output()
            .expect("the paneflow CLI runs")
    }

    fn await_state(&self, target: &str, state: &str) -> Value {
        let deadline = Instant::now() + STATE_WAIT;
        loop {
            let output = self.run(&["status", target, "--json"], None);
            let status: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
            if status["state"] == state {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "{target} never reached {state}: {status} {}",
                String::from_utf8_lossy(&output.stderr)
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn stop(mut self, sessions: &[&SessionId]) {
        for session in sessions {
            let _ = self.core.stop(session, None);
        }
        if let Some(worker) = self.worker.take() {
            worker.stop();
        }
        if let Some(server) = self.server.take() {
            let _ = server.stop();
        }
    }
}

fn reply(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "no JSON on stdout: {} / {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn send_refuses_an_agent_that_waits_for_a_decision_or_left_the_foreground() {
    let mut instance = Instance::open();
    let (blocked, blocked_gen) = instance.create("agent-blocked", None);
    let (departed, departed_gen) = instance.create("agent-departed", None);
    instance.start_worker();

    instance.hook(
        &blocked,
        blocked_gen,
        "ai.prompt_submit",
        "UserPromptSubmit",
    );
    instance.hook(
        &blocked,
        blocked_gen,
        "ai.notification",
        "PermissionRequest",
    );
    instance.await_state("agent-blocked", "waiting_for_input");
    let refused = instance.run(
        &["send", "agent-blocked", "echo pf-blocked-text", "--submit"],
        None,
    );
    assert_eq!(refused.status.code(), Some(3), "{}", stderr(&refused));
    assert!(
        stderr(&refused).contains("is waiting for a decision"),
        "{}",
        stderr(&refused)
    );
    assert!(stderr(&refused).contains("--force"));

    instance.hook(
        &departed,
        departed_gen,
        "ai.prompt_submit",
        "UserPromptSubmit",
    );
    instance.hook(&departed, departed_gen, "ai.stop", "Stop");
    instance.await_state("agent-departed", "finished");
    let refused = instance.run(&["send", "agent-departed", "pf-departed-text"], None);
    assert_eq!(refused.status.code(), Some(3), "{}", stderr(&refused));
    assert!(
        stderr(&refused).contains("no longer runs Claude Code in the foreground"),
        "{}",
        stderr(&refused)
    );

    let forced = instance.run(
        &["send", "agent-departed", "pf-forced-text", "--force"],
        None,
    );
    assert_eq!(forced.status.code(), Some(0), "{}", stderr(&forced));
    assert!(instance.shows(&departed, "pf-forced-text"));
    assert!(!instance.text(&blocked).contains("pf-blocked-text"));
    assert!(!instance.text(&departed).contains("pf-departed-text"));

    instance.stop(&[&blocked, &departed]);
}

#[test]
fn a_submit_reports_a_start_only_after_a_state_transition() {
    let mut instance = Instance::open();
    let (session, generation) = instance.create("agent-turn", None);
    instance.start_worker();
    instance.hook(&session, generation, "ai.prompt_submit", "UserPromptSubmit");
    instance.hook(&session, generation, "ai.stop", "Stop");
    instance.await_state("agent-turn", "finished");

    let echo_only = instance.run(
        &["send", "1", "echo pf-echo-only", "--submit", "--force"],
        None,
    );
    assert_eq!(echo_only.status.code(), Some(1), "{}", stderr(&echo_only));
    let unconfirmed = reply(&echo_only);
    assert_eq!(unconfirmed["delivered"], true);
    assert_eq!(unconfirmed["started"], false);
    assert_eq!(unconfirmed["reason"], "no_state_transition");
    assert_eq!(unconfirmed["state"], "idle");
    assert!(
        instance.shows(&session, "pf-echo-only"),
        "the echo moved the output, not the state"
    );

    let child = instance
        .cli(&["send", "1", "pf-real-turn", "--submit", "--force"], None)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the CLI starts");
    std::thread::sleep(Duration::from_millis(700));
    instance.hook(&session, generation, "ai.prompt_submit", "UserPromptSubmit");
    let started = child.wait_with_output().expect("the CLI ends");
    assert_eq!(started.status.code(), Some(0), "{}", stderr(&started));
    let confirmed = reply(&started);
    assert_eq!(confirmed["started"], true, "{confirmed}");
    assert_eq!(confirmed["reason"], "state_transition");
    assert_eq!(confirmed["state"], "working");

    instance.stop(&[&session]);
}

#[test]
fn wait_idle_returns_on_the_stop_hook_and_not_on_a_silent_tool() {
    let mut instance = Instance::open();
    let (session, generation) = instance.create("agent-silent", None);
    instance.start_worker();
    instance.hook(&session, generation, "ai.prompt_submit", "UserPromptSubmit");
    instance.hook(&session, generation, "ai.tool_use", "PreToolUse");
    instance.await_state("agent-silent", "thinking");

    let mut waiter = instance
        .cli(
            &[
                "wait",
                "--match",
                "agent-silent",
                "--idle",
                "--for",
                "5",
                "--timeout",
                "30",
            ],
            None,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the waiter starts");
    std::thread::sleep(Duration::from_millis(1_500));
    assert!(
        waiter.try_wait().expect("the waiter is alive").is_none(),
        "1.5 s of silence past a 5 ms window did not end the turn"
    );
    instance.hook(&session, generation, "ai.stop", "Stop");
    let deadline = Instant::now() + STATE_WAIT;
    while waiter.try_wait().expect("the waiter is alive").is_none() {
        assert!(
            Instant::now() < deadline,
            "the Stop hook never ended the wait"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = waiter.wait_with_output().expect("the waiter output");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let ended = reply(&output);
    assert_eq!(ended["idle"], true);
    assert_eq!(ended["state"], "idle");
    assert_eq!(ended["signal"], "agent_state");

    instance.stop(&[&session]);
}

#[test]
fn a_pane_cannot_write_into_another_workspace_without_scope_all() {
    let mut instance = Instance::open();
    let mine = WorkspaceId::new();
    let theirs = WorkspaceId::new();
    let (caller, _) = instance.create("caller", Some(&mine));
    let (target, _) = instance.create("foreign", Some(&theirs));

    let refused = instance.run(&["send", "foreign", "pf-cross"], Some(&caller));
    assert_ne!(refused.status.code(), Some(0));
    assert!(
        stderr(&refused).contains(&theirs.to_string()),
        "{}",
        stderr(&refused)
    );
    assert!(
        stderr(&refused).contains("--scope all"),
        "{}",
        stderr(&refused)
    );

    let unknown = instance.run(&["send", "foreign", "pf-unknown"], Some(&SessionId::new()));
    assert_ne!(unknown.status.code(), Some(0));
    assert!(
        stderr(&unknown).contains("unknown caller session"),
        "{}",
        stderr(&unknown)
    );

    let widened = instance.run(
        &["send", "foreign", "pf-scope-all", "--scope", "all"],
        Some(&caller),
    );
    assert_eq!(widened.status.code(), Some(0), "{}", stderr(&widened));
    let outside = instance.run(&["send", "foreign", "pf-outside"], None);
    assert_eq!(outside.status.code(), Some(0), "{}", stderr(&outside));
    assert!(instance.shows(&target, "pf-outside"));
    let text = instance.text(&target);
    assert!(text.contains("pf-scope-all"));
    assert!(!text.contains("pf-cross"));
    assert!(!text.contains("pf-unknown"));

    instance.stop(&[&caller, &target]);
}
