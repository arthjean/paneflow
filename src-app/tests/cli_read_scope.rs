#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(unix)]

use std::process::{Command, Output};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_config::schema::{SessionId, WorkspaceId};
use paneflow_host::protocol::ClientHello;
use paneflow_host::{HostClient, ServerHandle, SessionHost, SessionSummary};
use serde_json::{Value, json};

const OUTPUT_WAIT: Duration = Duration::from_secs(60);
const FOREIGN_LINE: &str = "beta-secret-scrollback";
const OWN_LINE: &str = "alpha-visible-scrollback";

struct Instance {
    home: tempfile::TempDir,
    core: Arc<SessionHost>,
    server: Option<ServerHandle>,
    owner: HostClient,
    own: WorkspaceId,
    alpha: SessionId,
    beta: SessionId,
}

impl Instance {
    fn open(config: &str) -> Self {
        let home = tempfile::tempdir().expect("a temporary home");
        for (name, contents) in [
            ("paneflow.json", config),
            ("session.json", "{}"),
            ("window-state.json", "{}"),
        ] {
            std::fs::write(home.path().join(name), contents).expect("isolated instance state");
        }
        let endpoint = paneflow_home::host_endpoint_path(home.path());
        let core = SessionHost::open(home.path(), &endpoint).expect("a core opens");
        let server =
            ServerHandle::spawn(Arc::clone(&core), endpoint.clone()).expect("the core serves");
        let owner = HostClient::connect(&endpoint, &ClientHello::local("cli-read-scope-test"))
            .expect("the core accepts the owner");
        let own = WorkspaceId::new();
        let theirs = WorkspaceId::new();
        let mut instance = Self {
            home,
            core,
            server: Some(server),
            owner,
            own: own.clone(),
            alpha: SessionId::new(),
            beta: SessionId::new(),
        };
        instance.alpha = instance.shell("alpha-pane", &own, &format!("echo {OWN_LINE}"));
        instance.beta = instance.shell("beta-pane", &theirs, &format!("echo {FOREIGN_LINE}"));
        assert!(instance.shows(&instance.alpha.clone(), OWN_LINE));
        assert!(instance.shows(&instance.beta.clone(), FOREIGN_LINE));
        instance
    }

    fn shell(&mut self, title: &str, workspace: &WorkspaceId, script: &str) -> SessionId {
        let created: SessionSummary = serde_json::from_value(
            self.owner
                .call(
                    "session.create",
                    json!({
                        "shell": "/bin/sh",
                        "args": ["-c", format!("{script}; sleep 120")],
                        "cwd": std::env::temp_dir().display().to_string(),
                        "cols": 240,
                        "rows": 60,
                        "title": title,
                        "workspace": workspace,
                        "env": {paneflow_home::HOME_ENV: self.home.path().display().to_string()},
                    }),
                )
                .expect("a session starts"),
        )
        .expect("a session summary");
        created.manifest.session
    }

    fn text(&self, session: &SessionId) -> String {
        self.core
            .text(session)
            .map(|text| text.text)
            .unwrap_or_default()
    }

    fn shows(&self, session: &SessionId, needle: &str) -> bool {
        let deadline = Instant::now() + OUTPUT_WAIT;
        while Instant::now() < deadline {
            if self.text(session).contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn run_in_a_pane(&mut self, command: &str) -> (i32, String) {
        let marker = format!("pf-done-{}", SessionId::new());
        let script = format!(
            "{command}; echo {marker}=$?",
            command = command.replace(
                "paneflow ",
                &format!("'{}' ", env!("CARGO_BIN_EXE_paneflow"))
            )
        );
        let own = self.own.clone();
        let caller = self.shell("reader-pane", &own, &script);
        let exit = format!("{marker}=");
        assert!(
            self.shows(&caller, &exit),
            "the pane command never finished: {}",
            self.text(&caller)
        );
        let text = self.text(&caller);
        let _ = self.core.stop(&caller, None);
        let _ = self.core.remove(&caller);
        let code = text
            .split(&exit)
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|code| code.parse().ok())
            .unwrap_or(-1);
        let output = text.split(&exit).next().unwrap_or_default().to_string();
        (code, output.replace("\r\n", "\n"))
    }

    fn outside(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_paneflow"));
        command
            .args(args)
            .env(paneflow_home::HOME_ENV, self.home.path())
            .env_remove("PANEFLOW_SOCKET_PATH")
            .env_remove("PANEFLOW_ALLOW_SOCKET_OVERRIDE");
        for key in paneflow_host::env::PANE_CONTEXT_ENV {
            command.env_remove(key);
        }
        command.output().expect("the paneflow CLI runs")
    }

    fn stop(mut self) {
        for session in [&self.alpha, &self.beta] {
            let _ = self.core.stop(session, None);
        }
        if let Some(server) = self.server.take() {
            let _ = server.stop();
        }
    }
}

fn unwrapped(text: &str) -> String {
    text.lines().map(str::trim_end).collect::<Vec<_>>().join("")
}

fn skipped_by_environment() -> bool {
    std::env::var_os("PANEFLOW_IPC_SCRIPTING").is_some()
        || std::env::var_os("PANEFLOW_IPC_ORCHESTRATION").is_some()
}

#[test]
fn a_pane_reads_lists_and_searches_only_its_own_workspace_through_the_cli() {
    if skipped_by_environment() {
        return;
    }
    let mut instance = Instance::open("{}");
    let hint = paneflow_api::policy::cross_workspace_read_hint();

    let (code, listed) = instance.run_in_a_pane("paneflow ls | cat");
    assert_eq!(code, 0, "{listed}");
    assert!(
        listed.contains("<untrusted_terminal_output source=\"surface.list\" id=\""),
        "{listed}"
    );
    assert_eq!(
        listed.matches("</untrusted_terminal_output").count(),
        1,
        "{listed}"
    );
    assert!(listed.contains("alpha-pane"), "{listed}");
    assert!(!listed.contains("beta-pane"), "{listed}");

    let (code, on_a_terminal) = instance.run_in_a_pane("paneflow ls");
    assert_eq!(code, 0, "{on_a_terminal}");
    assert!(
        !on_a_terminal.contains("untrusted_terminal_output"),
        "{on_a_terminal}"
    );
    assert!(on_a_terminal.contains("alpha-pane"), "{on_a_terminal}");

    let (code, widened) = instance.run_in_a_pane("paneflow ls --scope all --raw | cat");
    assert_eq!(code, 0, "{widened}");
    assert!(!widened.contains("untrusted_terminal_output"), "{widened}");
    assert!(widened.contains("alpha-pane"), "{widened}");
    assert!(!widened.contains("beta-pane"), "{widened}");

    let beta = instance.beta.to_string();
    for (method, extra) in [
        ("surface.read", ""),
        ("surface.search", ", \"pattern\": \"secret\""),
    ] {
        let command = format!(
            "paneflow api call {method} --on host --args '{{\"session\": \"{beta}\"{extra}}}'"
        );
        let (code, refused) = instance.run_in_a_pane(&command);
        assert_ne!(code, 0, "{method}: {refused}");
        assert!(unwrapped(&refused).contains(&hint), "{method}: {refused}");
        assert!(!refused.contains(FOREIGN_LINE), "{method}: {refused}");
    }

    let (code, missed) = instance.run_in_a_pane("paneflow read beta-pane --raw");
    assert_ne!(code, 0, "{missed}");
    assert!(unwrapped(&missed).contains("--scope all"), "{missed}");
    assert!(!missed.contains(FOREIGN_LINE), "{missed}");

    let claimed = format!(
        "paneflow api call surface.read --on host --args '{{\"session\": \"{}\", \"scope_session\": \"{beta}\", \"fenced\": false}}'",
        instance.alpha
    );
    let (code, own) = instance.run_in_a_pane(&claimed);
    assert_eq!(code, 0, "{own}");
    assert!(own.contains(OWN_LINE), "{own}");

    let everything = instance.outside(&["api", "call", "surface.list", "--on", "host"]);
    assert!(
        everything.status.success(),
        "{}",
        String::from_utf8_lossy(&everything.stderr)
    );
    let everything: Value = serde_json::from_slice(&everything.stdout).expect("a listing");
    let sessions: Vec<&str> = everything["surfaces"]
        .as_array()
        .expect("surfaces")
        .iter()
        .filter_map(|surface| surface["session"].as_str())
        .collect();
    assert!(sessions.contains(&beta.as_str()), "{everything}");
    assert!(
        sessions.contains(&instance.alpha.to_string().as_str()),
        "{everything}"
    );

    instance.stop();
}

#[test]
fn a_pane_search_prints_one_fence_and_raw_prints_plain_lines() {
    if skipped_by_environment() {
        return;
    }
    let mut instance = Instance::open("{}");
    let alpha = instance.alpha.to_string();

    let (code, fenced) = instance.run_in_a_pane("paneflow search alpha-pane alpha-visible | cat");
    assert_eq!(code, 0, "{fenced}");
    let joined = unwrapped(&fenced);
    let header = format!(
        "<untrusted_terminal_output source=\"session:{alpha}\" pattern=\"alpha-visible\" id=\""
    );
    assert!(joined.contains(&header), "{fenced}");
    assert_eq!(
        fenced.matches("</untrusted_terminal_output").count(),
        1,
        "{fenced}"
    );
    assert!(joined.contains(&format!(": {OWN_LINE}")), "{fenced}");

    let (code, raw) =
        instance.run_in_a_pane("paneflow search alpha-pane alpha-visible --raw | cat");
    assert_eq!(code, 0, "{raw}");
    assert!(!raw.contains("untrusted_terminal_output"), "{raw}");
    assert!(
        raw.lines()
            .any(|line| line.split_once(": ").is_some_and(|(n, rest)| {
                n.trim().parse::<u64>().is_ok() && rest.trim_end() == OWN_LINE
            })),
        "{raw}"
    );

    instance.stop();
}

#[test]
fn a_pane_reads_another_workspace_with_scope_all_once_orchestration_is_open() {
    if skipped_by_environment() {
        return;
    }
    let mut instance = Instance::open(r#"{"ipc": {"orchestration": true}}"#);

    let (code, scoped) = instance.run_in_a_pane("paneflow ls --raw");
    assert_eq!(code, 0, "{scoped}");
    assert!(!scoped.contains("beta-pane"), "{scoped}");

    let (code, read) = instance.run_in_a_pane("paneflow read beta-pane --scope all --raw");
    assert_eq!(code, 0, "{read}");
    assert!(read.contains(FOREIGN_LINE), "{read}");

    let (code, listed) = instance.run_in_a_pane("paneflow ls --scope all --raw");
    assert_eq!(code, 0, "{listed}");
    assert!(listed.contains("beta-pane"), "{listed}");

    instance.stop();
}
