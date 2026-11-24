#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::linux::net::SocketAddrExt as _;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::server::ServerHandle;
use paneflow_host::{CreateSession, HostError, SessionHost};
use serde_json::{Value, json};

const PROBE_OUT: &str = "PANEFLOW_SANDBOX_PROBE_OUT";
const PROBE_SHARED: &str = "PANEFLOW_SANDBOX_PROBE_SHARED";
const PROBE_ABSTRACT: &str = "PANEFLOW_SANDBOX_PROBE_ABSTRACT";
const PROBE_OUTSIDE_SOCKET: &str = "PANEFLOW_SANDBOX_PROBE_OUTSIDE_SOCKET";
const PROBE_AGENT_DIR: &str = "PANEFLOW_SANDBOX_PROBE_AGENT_DIR";

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

fn connect_error(outcome: std::io::Result<UnixStream>) -> Value {
    match outcome {
        Ok(_) => json!("connected"),
        Err(error) => json!(error.raw_os_error()),
    }
}

#[test]
fn probe_inside_the_sandbox() {
    let Some(out) = std::env::var_os(PROBE_OUT) else {
        return;
    };
    let home = PathBuf::from(std::env::var_os("PANEFLOW_HOME").unwrap());
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let runtime_brokers: Vec<&str> = ["bus", "systemd", "at-spi", ".dbus-proxy"]
        .into_iter()
        .filter(|name| {
            Path::new(&std::env::var(PROBE_SHARED).unwrap())
                .parent()
                .is_some_and(|runtime| runtime.join(name).exists())
        })
        .collect();
    let report = json!({
        "whoami": whoami(&std::env::var("PANEFLOW_PANE_HOST_ENDPOINT").unwrap()),
        "host_endpoint_env": std::env::var("PANEFLOW_HOST_ENDPOINT").ok(),
        "shared": connect_error(UnixStream::connect(std::env::var(PROBE_SHARED).unwrap())),
        "abstract": connect_error(
            SocketAddr::from_abstract_name(std::env::var(PROBE_ABSTRACT).unwrap().as_bytes())
                .and_then(|address| UnixStream::connect_addr(&address)),
        ),
        "outside_socket": connect_error(UnixStream::connect(std::env::var(PROBE_OUTSIDE_SOCKET).unwrap())),
        "processes": std::fs::read_dir("/proc").unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().bytes().all(|byte| byte.is_ascii_digit()))
            .count(),
        "home_write": std::fs::write(home.join("written"), b"x").err().map(|error| error.raw_os_error()),
        "cwd_write": std::fs::write("written-in-cwd", b"x").is_ok(),
        "agent_dir_write": std::fs::write(
            PathBuf::from(std::env::var_os(PROBE_AGENT_DIR).unwrap()).join("settings.json"),
            b"{}",
        )
        .is_ok(),
        "panes": std::fs::read_dir(home.join("panes")).unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        "runtime_brokers": runtime_brokers,
        "no_new_privs": status.lines().any(|line| line.split_whitespace().eq(["NoNewPrivs:", "1"])),
    });
    std::fs::write(out, report.to_string()).unwrap();
}

fn wait_for(path: &Path) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
        {
            return value;
        }
        assert!(Instant::now() < deadline, "the probe never reported");
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct Fixture {
    home: tempfile::TempDir,
    cwd: tempfile::TempDir,
    agent_dir: tempfile::TempDir,
    endpoint: PathBuf,
    host: Arc<SessionHost>,
    server: Option<ServerHandle>,
}

impl Fixture {
    fn open() -> Self {
        let home = tempfile::Builder::new().prefix("pfs").tempdir().unwrap();
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
            cwd: tempfile::Builder::new().prefix("pfc").tempdir().unwrap(),
            agent_dir: tempfile::Builder::new().prefix("pfg").tempdir().unwrap(),
            endpoint,
            host,
            server: Some(server),
        }
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

fn run_probe(fixture: &Fixture, unix_scope: bool) -> Option<(String, Value)> {
    let abstract_name = format!("paneflow-sandbox-test-{}-{unix_scope}", std::process::id());
    let _abstract =
        UnixListener::bind_addr(&SocketAddr::from_abstract_name(abstract_name.as_bytes()).unwrap())
            .unwrap();
    let outside = fixture.cwd.path().join("outside.sock");
    let _outside = UnixListener::bind(&outside).unwrap();
    let out = fixture.cwd.path().join("probe.json");
    let env: BTreeMap<String, String> = [
        (PROBE_OUT, out.display().to_string()),
        (PROBE_SHARED, fixture.endpoint.display().to_string()),
        (PROBE_ABSTRACT, abstract_name.clone()),
        (PROBE_OUTSIDE_SOCKET, outside.display().to_string()),
        (
            PROBE_AGENT_DIR,
            fixture.agent_dir.path().display().to_string(),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    let created = fixture.host.create(CreateSession {
        cwd: Some(fixture.cwd.path().display().to_string()),
        shell: Some(std::env::current_exe().unwrap().display().to_string()),
        args: vec![
            "--exact".into(),
            "probe_inside_the_sandbox".into(),
            "--nocapture".into(),
        ],
        env,
        cols: Some(120),
        rows: Some(30),
        sandbox: true,
        sandbox_unix_scope: unix_scope,
        ..CreateSession::default()
    });
    let summary = match created {
        Ok(summary) => summary,
        Err(HostError::SpawnFailed { reason, .. }) => {
            assert!(
                std::env::var_os("PANEFLOW_SANDBOX_REQUIRED")
                    .is_none_or(|required| required != "1"),
                "PANEFLOW_SANDBOX_REQUIRED=1 and the sandbox did not start: {reason}"
            );
            assert!(
                reason.contains("bwrap"),
                "a pane this machine cannot sandbox fails naming bwrap: {reason}"
            );
            return None;
        }
        Err(other) => panic!("unexpected launch error: {other}"),
    };
    assert!(summary.manifest.launch.sandbox);
    assert_eq!(summary.manifest.launch.sandbox_unix_scope, unix_scope);
    Some((summary.manifest.session.to_string(), wait_for(&out)))
}

#[test]
fn a_sandboxed_pane_reaches_the_host_only_through_its_own_endpoint() {
    let fixture = Fixture::open();
    let Some((session, report)) = run_probe(&fixture, false) else {
        return;
    };
    let session = paneflow_host::SessionId::parse(&session).unwrap();
    assert_eq!(report["whoami"]["principal"], "pane", "{report}");
    assert_eq!(report["whoami"]["session"], session.as_str(), "{report}");
    let pane_host = paneflow_home::pane_endpoint_path(
        fixture.home.path(),
        session.as_str(),
        paneflow_home::PaneServer::Host,
    );
    assert_eq!(report["host_endpoint_env"], pane_host.display().to_string());
    assert_eq!(report["shared"], json!(libc::ENOENT), "{report}");
    if paneflow_host::sandbox::landlock_abi() >= 6 {
        assert_eq!(report["abstract"], json!(libc::EPERM), "{report}");
    } else {
        assert_eq!(
            report["abstract"],
            json!("connected"),
            "below Landlock ABI 6 nothing scopes an abstract socket: {report}"
        );
    }
    assert_eq!(
        report["outside_socket"],
        json!("connected"),
        "without the unix scope opt-in, a socket outside Paneflow stays reachable: {report}"
    );
    assert!(report["processes"].as_u64().unwrap() < 8, "{report}");
    assert_eq!(report["home_write"], json!(libc::EROFS), "{report}");
    assert_eq!(report["cwd_write"], json!(true), "{report}");
    assert_eq!(report["agent_dir_write"], json!(true), "{report}");
    assert_eq!(report["panes"], json!([session.as_str()]), "{report}");
    assert_eq!(report["no_new_privs"], json!(true), "{report}");
    assert_eq!(
        report["runtime_brokers"],
        json!([]),
        "a session bus or a systemd socket would start a process outside the sandbox: {report}"
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    while pane_host.exists() || fixture.host.pane_endpoint_open(&session) {
        assert!(
            Instant::now() < deadline,
            "the pane endpoint outlived its session"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!paneflow_home::pane_dir_in(fixture.home.path(), session.as_str()).exists());
}

#[test]
fn the_unix_scope_opt_in_refuses_a_socket_created_outside_the_sandbox() {
    let fixture = Fixture::open();
    let Some((_, report)) = run_probe(&fixture, true) else {
        return;
    };
    assert_eq!(report["whoami"]["principal"], "pane", "{report}");
    if paneflow_host::sandbox::landlock_abi() >= 9 {
        assert_eq!(report["outside_socket"], json!(libc::EACCES), "{report}");
    } else {
        assert_eq!(report["outside_socket"], json!("connected"), "{report}");
    }
}

#[test]
fn a_launch_env_request_sandboxes_the_pane_across_a_restart_and_an_unknown_one_is_refused() {
    let fixture = Fixture::open();
    let launch = |request: &str| {
        fixture.host.create(CreateSession {
            cwd: Some(fixture.cwd.path().display().to_string()),
            shell: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 60".into()],
            env: BTreeMap::from([(
                paneflow_config::schema::SANDBOX_REQUEST_ENV.to_string(),
                request.to_string(),
            )]),
            ..CreateSession::default()
        })
    };
    match launch("docker") {
        Err(HostError::InvalidRequest(reason)) => {
            assert!(reason.contains("does not start unsandboxed"), "{reason}");
        }
        other => panic!("an unknown sandbox must refuse the launch: {other:?}"),
    }
    let summary = match launch(paneflow_config::schema::SANDBOX_REQUEST_PANEFLOW_UNIX_SCOPE) {
        Ok(summary) => summary,
        Err(HostError::SpawnFailed { reason, .. }) => {
            assert!(
                std::env::var_os("PANEFLOW_SANDBOX_REQUIRED")
                    .is_none_or(|required| required != "1"),
                "PANEFLOW_SANDBOX_REQUIRED=1 and the sandbox did not start: {reason}"
            );
            assert!(reason.contains("bwrap"), "{reason}");
            return;
        }
        Err(other) => panic!("unexpected launch error: {other}"),
    };
    let launched = &summary.manifest.launch;
    assert!(launched.sandbox && launched.sandbox_unix_scope);
    assert!(
        !launched
            .env
            .contains_key(paneflow_config::schema::SANDBOX_REQUEST_ENV)
    );
    let session = summary.manifest.session.clone();
    let mut lister =
        paneflow_ipc_client::host_control::HostControl::connect(&fixture.endpoint, "sandbox-ls")
            .unwrap();
    let listed = lister.request("surface.list", json!({})).unwrap();
    let row = listed["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["session"] == session.as_str())
        .cloned()
        .unwrap();
    assert_eq!(row["sandboxed"], json!(true), "{listed}");
    let mut child = json!({"shell": "/bin/sh", "sandbox": false});
    fixture.host.confine_spawn(
        &paneflow_api::Principal::Pane {
            session: session.to_string(),
            workspace: None,
        },
        &mut child,
    );
    assert_eq!(
        (
            child["sandbox"].clone(),
            child["sandbox_unix_scope"].clone()
        ),
        (json!(true), json!(true)),
        "a session a sandboxed pane creates through the host keeps its sandbox: {child}"
    );
    let mut outside = json!({"shell": "/bin/sh"});
    fixture
        .host
        .confine_spawn(&paneflow_api::Principal::External, &mut outside);
    assert!(outside.get("sandbox").is_none());
    fixture.host.stop(&session, None).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while fixture.host.pane_endpoint_open(&session) {
        assert!(
            Instant::now() < deadline,
            "the stopped pane kept its endpoint"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let restarted = fixture.host.restart(&session, None).unwrap();
    assert!(restarted.manifest.launch.sandbox);
    assert!(restarted.manifest.launch.sandbox_unix_scope);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !fixture.host.pane_endpoint_open(&session) {
        assert!(
            Instant::now() < deadline,
            "the restarted pane lost its endpoint"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        paneflow_home::pane_endpoint_path(
            fixture.home.path(),
            session.as_str(),
            paneflow_home::PaneServer::Host,
        )
        .exists()
    );
    fixture.host.stop(&session, None).unwrap();
}

#[test]
fn a_pane_whose_endpoint_cannot_be_created_fails_its_launch_with_the_os_error() {
    let fixture = Fixture::open();
    std::fs::write(paneflow_home::panes_dir_in(fixture.home.path()), b"").unwrap();
    let created = fixture.host.create(CreateSession {
        cwd: Some(fixture.cwd.path().display().to_string()),
        shell: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 60".into()],
        sandbox: true,
        ..CreateSession::default()
    });
    match created {
        Err(HostError::SpawnFailed { reason, .. }) => {
            assert!(
                reason.contains("cannot create the sandboxed pane's endpoint"),
                "{reason}"
            );
            assert!(
                reason.contains(&std::io::Error::from_raw_os_error(libc::ENOTDIR).to_string())
                    || reason
                        .contains(&std::io::Error::from_raw_os_error(libc::EEXIST).to_string()),
                "{reason}"
            );
        }
        other => panic!("a pane without its endpoint must not start: {other:?}"),
    }
    let live = fixture
        .host
        .list(None)
        .into_iter()
        .filter(|row| row.live)
        .count();
    assert_eq!(live, 0, "no shell runs without the pane endpoint");
}
