#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::server::ServerHandle;
use paneflow_host::{CreateSession, HostError, SessionHost};
use serde_json::{Value, json};

const PROBE_OUT: &str = "PANEFLOW_SANDBOX_PROBE_OUT";
const PROBE_SHARED: &str = "PANEFLOW_SANDBOX_PROBE_SHARED";
const PROBE_NEIGHBOR_SOCKET: &str = "PANEFLOW_SANDBOX_PROBE_NEIGHBOR_SOCKET";
const PROBE_OUTSIDE_SOCKET: &str = "PANEFLOW_SANDBOX_PROBE_OUTSIDE_SOCKET";
const PROBE_OUTSIDE_PID: &str = "PANEFLOW_SANDBOX_PROBE_OUTSIDE_PID";
const PROBE_TOOLS: &str = "PANEFLOW_SANDBOX_PROBE_TOOLS";
const TOOLS_REPORT: &str = "PANEFLOW_SANDBOX_TOOLS_REPORT";
const PROBE_CLAUDE: &str = "PANEFLOW_SANDBOX_PROBE_CLAUDE";
const PROBE_CODEX: &str = "PANEFLOW_SANDBOX_PROBE_CODEX";
const NESTED_HOST: &str = "PANEFLOW_SANDBOX_NESTED_HOST_OUT";
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
const OUTCOME_DEADLINE: Duration = Duration::from_secs(120);
const SEPARATOR: char = ';';

fn errno_of(status: libc::c_int) -> Value {
    if status == 0 {
        json!("ok")
    } else {
        json!(std::io::Error::last_os_error().raw_os_error())
    }
}

fn connect(path: &str) -> Value {
    match UnixStream::connect(path) {
        Ok(_) => json!("connected"),
        Err(error) => json!(error.raw_os_error()),
    }
}

fn whoami(endpoint: &str) -> Value {
    let stream = UnixStream::connect(endpoint).expect("the pane endpoint answers");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    for (id, method, params) in [
        (
            1,
            "host.hello",
            json!({"client": "sandbox-probe", "protocol": paneflow_host::HOST_PROTOCOL_VERSION}),
        ),
        (2, "system.whoami", json!({})),
    ] {
        writeln!(
            writer,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .unwrap();
        reply.clear();
        reader.read_line(&mut reply).unwrap();
    }
    serde_json::from_str::<Value>(&reply).unwrap()["result"].clone()
}

fn process_path(pid: i32) -> Value {
    let mut buffer = vec![0u8; 4096];
    let written = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), 4096) };
    if written > 0 {
        json!("ok")
    } else {
        json!(std::io::Error::last_os_error().raw_os_error())
    }
}

fn run_command(command: &mut Command) -> Value {
    match command.stdin(Stdio::null()).output() {
        Ok(output) => {
            let tail = |bytes: &[u8]| {
                let text = String::from_utf8_lossy(bytes).trim().to_string();
                let start = text.len().saturating_sub(600);
                text.get(start..).unwrap_or_default().to_string()
            };
            json!({
                "code": output.status.code(),
                "stdout": tail(&output.stdout),
                "stderr": tail(&output.stderr),
            })
        }
        Err(error) => json!({"spawn_error": error.to_string()}),
    }
}

fn run_tool(program: &str, args: &[&str], cwd: &Path) -> Value {
    run_command(Command::new(program).args(args).current_dir(cwd))
}

fn own_child_signal() -> Value {
    let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let pid = child.id() as i32;
    let probe = errno_of(unsafe { libc::kill(pid, 0) });
    let info = process_path(pid);
    let _ = child.kill();
    let _ = child.wait();
    json!({"kill_0": probe, "process_path": info})
}

fn tools_report(cwd: &Path) -> Value {
    let crate_dir = cwd.join("sandbox-probe-crate");
    let created = run_tool(
        "cargo",
        &[
            "new",
            "--vcs",
            "none",
            "--name",
            "sandbox_probe",
            "sandbox-probe-crate",
        ],
        cwd,
    );
    let claude = std::env::var(PROBE_CLAUDE).unwrap_or_else(|_| "claude".into());
    let codex = std::env::var(PROBE_CODEX).unwrap_or_else(|_| "codex".into());
    json!({
        "git_init": run_tool("git", &["init", "-q"], cwd),
        "git_status": run_tool("git", &["status", "--short"], cwd),
        "cargo_new": created,
        "cargo_build": run_command(
            Command::new("cargo")
                .args(["build", "--offline"])
                .current_dir(&crate_dir)
                .env("CARGO_TARGET_DIR", crate_dir.join("target")),
        ),
        "node_version": run_tool("node", &["-v"], cwd),
        "bun_version": run_tool("bun", &["-v"], cwd),
        "claude_sandboxed_bash": run_tool(
            &claude,
            &[
                "-p",
                "--settings",
                r#"{"sandbox":{"enabled":true,"allowUnsandboxedCommands":false}}"#,
                "--allowedTools",
                "Bash",
                "--max-turns",
                "3",
                "Use the Bash tool to run exactly: echo hello-from-claude-sandbox; touch $HOME/pf-claude-sandbox-probe. Then reply with its output, or with the exact error the tool returned.",
            ],
            cwd,
        ),
        "codex_sandbox": run_tool(
            &codex,
            &["sandbox", "--", "/bin/sh", "-c", "echo codex-sandbox-ran"],
            cwd,
        ),
    })
}

#[test]
fn probe_inside_the_macos_sandbox() {
    let Some(out) = std::env::var_os(PROBE_OUT) else {
        return;
    };
    let home = PathBuf::from(std::env::var_os("PANEFLOW_HOME").unwrap());
    let cwd = std::env::current_dir().unwrap();
    let outside_pid: i32 = std::env::var(PROBE_OUTSIDE_PID).unwrap().parse().unwrap();
    let shared: BTreeMap<String, Value> = std::env::var(PROBE_SHARED)
        .unwrap()
        .split(SEPARATOR)
        .map(|path| (path.to_string(), connect(path)))
        .collect();
    let nested = run_command(Command::new(SANDBOX_EXEC).args([
        "-p",
        "(version 1)(allow default)(deny file-write* (literal \"/nonexistent-probe\"))",
        "/usr/bin/true",
    ]));
    let report = json!({
        "whoami": whoami(&std::env::var("PANEFLOW_PANE_HOST_ENDPOINT").unwrap()),
        "shared": shared,
        "neighbor": connect(&std::env::var(PROBE_NEIGHBOR_SOCKET).unwrap()),
        "outside_socket": connect(&std::env::var(PROBE_OUTSIDE_SOCKET).unwrap()),
        "outside_kill_0": errno_of(unsafe { libc::kill(outside_pid, 0) }),
        "outside_sigcont": errno_of(unsafe { libc::kill(outside_pid, libc::SIGCONT) }),
        "outside_process_path": process_path(outside_pid),
        "own_child": own_child_signal(),
        "ps_outside": run_command(Command::new("/bin/ps").args(["-o", "pid=", "-p", &outside_pid.to_string()])),
        "kill_0_command": run_command(Command::new("/bin/sh").args(["-c", &format!("kill -0 {outside_pid}")])),
        "nested_sandbox_exec": nested,
        "home_write": std::fs::write(home.join("written"), b"x").err().map(|error| error.raw_os_error()),
        "cwd_write": std::fs::write(cwd.join("written-in-cwd"), b"x").is_ok(),
        "tools": std::env::var_os(PROBE_TOOLS).map(|_| tools_report(&cwd)),
    });
    std::fs::write(out, report.to_string()).unwrap();
}

fn wait_for(path: &Path, host: &SessionHost, session: &str) -> Value {
    let deadline = Instant::now() + OUTCOME_DEADLINE;
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
        {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "the probe never reported; the pane shows: {:?}",
            paneflow_host::SessionId::parse(session)
                .ok()
                .and_then(|session| host.text(&session).ok())
                .map(|text| text.text)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn short_tempdir(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in("/tmp")
        .unwrap()
}

fn probe_shell() -> String {
    std::env::current_exe().unwrap().display().to_string()
}

struct Fixture {
    home: tempfile::TempDir,
    cwd: tempfile::TempDir,
    endpoint: PathBuf,
    host: Arc<SessionHost>,
    server: Option<ServerHandle>,
}

impl Fixture {
    fn open() -> Self {
        let home = short_tempdir("pfs");
        std::fs::write(
            home.path().join("paneflow.json"),
            r#"{"shell_integration": false}"#,
        )
        .unwrap();
        let endpoint = paneflow_home::host_endpoint_path(home.path());
        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        host.use_sandbox_helper(PathBuf::from(env!("CARGO_BIN_EXE_paneflow-host")));
        let server = ServerHandle::spawn(Arc::clone(&host), endpoint.clone()).unwrap();
        Self {
            home,
            cwd: short_tempdir("pfc"),
            endpoint,
            host,
            server: Some(server),
        }
    }

    fn create(
        &self,
        env: BTreeMap<String, String>,
    ) -> Result<paneflow_host::SessionSummary, HostError> {
        self.host.create(CreateSession {
            cwd: Some(self.cwd.path().display().to_string()),
            shell: Some(probe_shell()),
            args: vec![
                "--exact".into(),
                "probe_inside_the_macos_sandbox".into(),
                "--nocapture".into(),
            ],
            env,
            cols: Some(120),
            rows: Some(30),
            sandbox: true,
            ..CreateSession::default()
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.host.request_shutdown(true);
        if let Some(server) = self.server.take() {
            let _ = server.stop();
        }
    }
}

struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_seatbelt_pane_reaches_only_its_own_endpoints_and_signals_only_its_own_processes() {
    let fixture = Fixture::open();
    let worker_endpoint = paneflow_home::serve_endpoint_path(fixture.home.path());
    let window_endpoint = paneflow_home::ipc_endpoint_path(fixture.home.path());
    let _worker = UnixListener::bind(&worker_endpoint).unwrap();
    let _window = UnixListener::bind(&window_endpoint).unwrap();
    let neighbor_dir = paneflow_home::pane_dir_in(fixture.home.path(), "neighbor");
    std::fs::create_dir_all(&neighbor_dir).unwrap();
    let neighbor_socket = neighbor_dir.join("host.sock");
    let _neighbor = UnixListener::bind(&neighbor_socket).unwrap();
    let outside_socket = fixture.cwd.path().join("outside.sock");
    let _outside = UnixListener::bind(&outside_socket).unwrap();
    let outside = Reaped(Command::new("/bin/sleep").arg("300").spawn().unwrap());
    let out = fixture.cwd.path().join("probe.json");
    let shared = [&fixture.endpoint, &worker_endpoint, &window_endpoint];
    let mut env = BTreeMap::from([
        (PROBE_OUT.to_string(), out.display().to_string()),
        (
            PROBE_SHARED.to_string(),
            shared
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(&SEPARATOR.to_string()),
        ),
        (
            PROBE_NEIGHBOR_SOCKET.to_string(),
            neighbor_socket.display().to_string(),
        ),
        (
            PROBE_OUTSIDE_SOCKET.to_string(),
            outside_socket.display().to_string(),
        ),
        (PROBE_OUTSIDE_PID.to_string(), outside.0.id().to_string()),
    ]);
    let tools = std::env::var_os(TOOLS_REPORT);
    if tools.is_some() {
        env.insert(PROBE_TOOLS.to_string(), "1".to_string());
        for agent in [PROBE_CLAUDE, PROBE_CODEX] {
            if let Ok(path) = std::env::var(agent) {
                env.insert(agent.to_string(), path);
            }
        }
    }
    let summary = fixture.create(env).unwrap();
    assert!(summary.manifest.launch.sandbox);
    let session = summary.manifest.session.to_string();
    let report = wait_for(&out, &fixture.host, &session);
    if let Some(path) = tools {
        std::fs::write(path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    }
    assert_eq!(report["whoami"]["principal"], "pane", "{report}");
    assert_eq!(report["whoami"]["session"], session.as_str(), "{report}");
    for endpoint in shared {
        assert_eq!(
            report["shared"][endpoint.display().to_string()],
            json!(libc::EPERM),
            "the sandbox refuses the shared endpoint {}: {report}",
            endpoint.display()
        );
    }
    assert_eq!(report["neighbor"], json!(libc::EPERM), "{report}");
    assert_eq!(
        report["outside_socket"],
        json!("connected"),
        "a socket outside Paneflow stays reachable: {report}"
    );
    assert_eq!(report["outside_kill_0"], json!(libc::EPERM), "{report}");
    assert_eq!(report["outside_sigcont"], json!(libc::EPERM), "{report}");
    assert_eq!(
        report["outside_process_path"],
        json!(libc::EPERM),
        "{report}"
    );
    assert_eq!(
        report["own_child"],
        json!({"kill_0": "ok", "process_path": "ok"}),
        "{report}"
    );
    assert_ne!(report["kill_0_command"]["code"], json!(0), "{report}");
    assert_ne!(report["ps_outside"]["code"], json!(0), "{report}");
    assert_eq!(report["home_write"], json!(libc::EPERM), "{report}");
    assert_eq!(report["cwd_write"], json!(true), "{report}");
    assert_ne!(
        report["nested_sandbox_exec"]["code"],
        json!(0),
        "Seatbelt refuses a second profile inside the pane's: {report}"
    );
    assert!(
        report["nested_sandbox_exec"]["stderr"]
            .as_str()
            .unwrap()
            .contains("sandbox_apply: Operation not permitted"),
        "{report}"
    );
}

#[test]
fn probe_a_host_that_runs_under_seatbelt() {
    let Some(out) = std::env::var_os(NESTED_HOST) else {
        return;
    };
    let fixture = Fixture::open();
    let outcome = match fixture.create(BTreeMap::new()) {
        Ok(summary) => json!({"started": summary.manifest.session.to_string()}),
        Err(HostError::SpawnFailed { reason, .. }) => json!({"spawn_failed": reason}),
        Err(other) => json!({"other": other.to_string()}),
    };
    std::fs::write(out, outcome.to_string()).unwrap();
}

#[test]
fn a_pane_whose_seatbelt_profile_is_refused_fails_its_launch_with_the_error() {
    let scratch = short_tempdir("pfn");
    let out = scratch.path().join("nested.json");
    let status = Command::new(SANDBOX_EXEC)
        .args([
            "-p",
            "(version 1)(allow default)(deny file-write* (literal \"/nonexistent-probe\"))",
        ])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "probe_a_host_that_runs_under_seatbelt",
            "--nocapture",
        ])
        .env(NESTED_HOST, &out)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(status.status.success(), "{status:?}");
    let outcome: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    let reason = outcome["spawn_failed"]
        .as_str()
        .unwrap_or_else(|| panic!("the pane must not start: {outcome}"));
    assert!(
        reason.contains("sandbox-exec refused the sandbox profile"),
        "{reason}"
    );
    assert!(
        reason.contains("sandbox_apply: Operation not permitted"),
        "{reason}"
    );
}
