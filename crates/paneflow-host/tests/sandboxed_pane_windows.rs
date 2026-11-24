#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(windows)]

use std::collections::BTreeMap;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::{FromRawHandle as _, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::server::{ServerHandle, bind_owner_only, bind_with_sddl};
use paneflow_host::{CreateSession, HostError, SessionHost};
use serde_json::{Value, json};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_GROUPS, TOKEN_QUERY, TokenRestrictedSids,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_READ_ATTRIBUTES, OPEN_EXISTING,
    PIPE_ACCESS_INBOUND,
};
use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const PROBE_OUT: &str = "PANEFLOW_SANDBOX_PROBE_OUT";
const PROBE_SHARED: &str = "PANEFLOW_SANDBOX_PROBE_SHARED";
const PROBE_OUTSIDE_DIR: &str = "PANEFLOW_SANDBOX_PROBE_OUTSIDE_DIR";
const PROBE_TOOLS: &str = "PANEFLOW_SANDBOX_PROBE_TOOLS";
const TOOLS_REPORT: &str = "PANEFLOW_SANDBOX_TOOLS_REPORT";
const PROBE_CLAUDE: &str = "PANEFLOW_SANDBOX_PROBE_CLAUDE";
const PROBE_CODEX: &str = "PANEFLOW_SANDBOX_PROBE_CODEX";
const PROBE_NEIGHBOR_DIR: &str = "PANEFLOW_SANDBOX_PROBE_NEIGHBOR_DIR";
const SANDBOX_SID_OVERRIDE: &str = "PANEFLOW_TEST_SANDBOX_SID";
const OUTCOME_DEADLINE: Duration = Duration::from_secs(120);
const WINDOW_PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)";

fn restricted_sids() -> Vec<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    assert_ne!(
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) },
        0
    );
    let mut needed = 0_u32;
    unsafe {
        GetTokenInformation(
            token,
            TokenRestrictedSids,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    let mut buffer = vec![0_u64; (needed as usize).div_ceil(8).max(1)];
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenRestrictedSids,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut needed,
        )
    };
    assert_ne!(read, 0, "{}", std::io::Error::last_os_error());
    let groups = buffer.as_ptr().cast::<TOKEN_GROUPS>();
    let count = unsafe { (*groups).GroupCount } as usize;
    let first = unsafe {
        std::ptr::addr_of!((*groups).Groups)
            .cast::<windows_sys::Win32::Security::SID_AND_ATTRIBUTES>()
    };
    (0..count)
        .map(|index| {
            let sid = unsafe { (*first.add(index)).Sid };
            let mut text: *mut u16 = std::ptr::null_mut();
            assert_ne!(unsafe { ConvertSidToStringSidW(sid, &mut text) }, 0);
            let length = (0..)
                .take_while(|&at| unsafe { *text.add(at) } != 0)
                .count();
            let rendered =
                String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) }).unwrap();
            unsafe { LocalFree(text.cast()) };
            rendered
        })
        .collect()
}

fn outcome<T>(result: std::io::Result<T>) -> Value {
    match result {
        Ok(_) => json!("ok"),
        Err(error) => json!(error.raw_os_error()),
    }
}

fn open_pipe(path: &str, write: bool) -> Value {
    outcome(
        std::fs::OpenOptions::new()
            .read(true)
            .write(write)
            .open(path),
    )
}

fn run_tool(program: &str, args: &[&str], cwd: &Path) -> Value {
    run_command(Command::new(program).args(args).current_dir(cwd))
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
        "claude_print": run_tool(
            &std::env::var(PROBE_CLAUDE).unwrap_or_else(|_| "claude.exe".into()),
            &["-p", "Reply with the single word OK", "--max-turns", "1"],
            cwd,
        ),
        "claude_bash_tool": run_tool(
            &std::env::var(PROBE_CLAUDE).unwrap_or_else(|_| "claude.exe".into()),
            &[
                "-p",
                "Use the Bash tool to run exactly: echo sandbox-bash-ran. Then reply with its output, or with the exact error the tool returned.",
                "--allowedTools",
                "Bash",
                "--max-turns",
                "3",
            ],
            cwd,
        ),
        "codex_sandbox": run_tool(
            &std::env::var(PROBE_CODEX).unwrap_or_else(|_| "codex.exe".into()),
            &["sandbox", "--", "cmd.exe", "/d", "/c", "echo codex-sandbox-ran"],
            cwd,
        ),
        "user_profile_write": outcome(std::fs::write(
            PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(".paneflow-sandbox-probe"),
            b"x",
        )),
        "cargo_build_inherited_target_dir": std::env::var("CARGO_TARGET_DIR").ok().map(|dir| {
            json!({
                "dir": dir,
                "outcome": run_tool("cargo", &["build", "--offline"], &crate_dir),
            })
        }),
        "node_version": run_tool("node", &["-v"], cwd),
    })
}

fn named_pipe_with_default_security() -> Value {
    let name: Vec<u16> = std::ffi::OsStr::new(&format!(
        r"\\.\pipe\paneflow-sandbox-probe-{}",
        std::process::id()
    ))
    .encode_wide()
    .chain(Some(0))
    .collect();
    let server = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_WAIT,
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    };
    if server == INVALID_HANDLE_VALUE {
        return json!({"server": std::io::Error::last_os_error().raw_os_error()});
    }
    let server = unsafe { OwnedHandle::from_raw_handle(server) };
    let client = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE | FILE_READ_ATTRIBUTES,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    let opened = if client == INVALID_HANDLE_VALUE {
        json!(std::io::Error::last_os_error().raw_os_error())
    } else {
        drop(unsafe { OwnedHandle::from_raw_handle(client) });
        json!("ok")
    };
    drop(server);
    opened
}

#[test]
fn probe_inside_the_windows_sandbox() {
    let Some(out) = std::env::var_os(PROBE_OUT) else {
        return;
    };
    let cwd = std::env::current_dir().unwrap();
    let outside = PathBuf::from(std::env::var_os(PROBE_OUTSIDE_DIR).unwrap());
    let shared: BTreeMap<String, Value> = std::env::var(PROBE_SHARED)
        .unwrap()
        .split(';')
        .map(|path| {
            (
                path.to_string(),
                json!({"write": open_pipe(path, true), "read": open_pipe(path, false)}),
            )
        })
        .collect();
    let pane_host = std::env::var("PANEFLOW_PANE_HOST_ENDPOINT").unwrap();
    let whoami = paneflow_ipc_client::host_control::HostControl::connect(
        Path::new(&pane_host),
        "sandbox-probe",
    )
    .and_then(|mut control| control.request("system.whoami", json!({})));
    let temp = std::env::temp_dir().join(format!("pf-sandbox-{}", std::process::id()));
    let child = Command::new("cmd.exe")
        .args(["/d", "/c", "echo child-ran"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .map_err(|error| error.raw_os_error());
    let report = json!({
        "restricted_sids": restricted_sids(),
        "whoami": whoami.unwrap_or_else(|error| json!({"error": error})),
        "host_endpoint_env": std::env::var("PANEFLOW_HOST_ENDPOINT").ok(),
        "shared": shared,
        "pane_host": open_pipe(&pane_host, true),
        "cwd_write": outcome(std::fs::write(cwd.join("written-in-cwd"), b"x")),
        "temp_write": outcome(std::fs::write(&temp, b"x")),
        "temp_dir": std::env::temp_dir(),
        "outside_read": outcome(std::fs::read(outside.join("readable"))),
        "outside_write": outcome(std::fs::write(outside.join("written"), b"x")),
        "nul_write": outcome(std::fs::write("NUL", b"x")),
        "named_pipe_client": named_pipe_with_default_security(),
        "child_pipe": match child {
            Ok(text) => json!(text),
            Err(code) => json!({"error": code}),
        },
        "tools": std::env::var_os(PROBE_TOOLS).map(|_| tools_report(&cwd)),
        "neighbor_writes": std::env::var_os(PROBE_NEIGHBOR_DIR).map(|dir| {
            let dir = PathBuf::from(dir);
            json!({
                "data_dir": outcome(std::fs::write(dir.join("planted-by-a-neighbor"), b"x")),
                "temp": outcome(std::fs::write(dir.join("tmp").join("planted-by-a-neighbor"), b"x")),
            })
        }),
    });
    let _ = std::fs::remove_file(&temp);
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

fn probe_shell() -> String {
    std::env::current_exe().unwrap().display().to_string()
}

struct Fixture {
    home: tempfile::TempDir,
    cwd: tempfile::TempDir,
    outside: tempfile::TempDir,
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
        let outside = tempfile::Builder::new().prefix("pfo").tempdir().unwrap();
        std::fs::write(outside.path().join("readable"), b"x").unwrap();
        Self {
            home,
            cwd: tempfile::Builder::new().prefix("pfc").tempdir().unwrap(),
            outside,
            endpoint,
            host,
            server: Some(server),
        }
    }

    fn probe_env(&self, out: &Path, shared: &[&Path]) -> BTreeMap<String, String> {
        BTreeMap::from([
            (PROBE_OUT.to_string(), out.display().to_string()),
            (
                PROBE_SHARED.to_string(),
                shared
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(";"),
            ),
            (
                PROBE_OUTSIDE_DIR.to_string(),
                self.outside.path().display().to_string(),
            ),
        ])
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
                "probe_inside_the_windows_sandbox".into(),
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

#[test]
fn a_sandboxed_windows_pane_writes_only_where_it_is_granted_and_reaches_only_its_own_endpoints() {
    let fixture = Fixture::open();
    let worker_endpoint = paneflow_home::serve_endpoint_path(fixture.home.path());
    let _worker = bind_owner_only(&worker_endpoint, "paneflow-serve").unwrap();
    let window_endpoint = PathBuf::from(format!(
        r"\\.\pipe\paneflow-window-probe-{}",
        std::process::id()
    ));
    let _window = bind_with_sddl(&window_endpoint, "paneflow", WINDOW_PIPE_SDDL).unwrap();
    let shared = [
        fixture.endpoint.as_path(),
        worker_endpoint.as_path(),
        window_endpoint.as_path(),
    ];
    let neighbor_out = fixture.cwd.path().join("neighbor-probe.json");
    let neighbor = fixture
        .create(fixture.probe_env(&neighbor_out, &shared))
        .unwrap()
        .manifest
        .session
        .to_string();
    wait_for(&neighbor_out, &fixture.host, &neighbor);
    let neighbor_data = paneflow_home::host_session_data_dir_in(fixture.home.path(), &neighbor);
    let out = fixture.cwd.path().join("probe.json");
    let mut env = fixture.probe_env(&out, &shared);
    env.insert(
        PROBE_NEIGHBOR_DIR.to_string(),
        neighbor_data.display().to_string(),
    );
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
    let restricting = report["restricted_sids"].as_array().unwrap();
    assert_eq!(
        restricting[..2],
        [
            json!(paneflow_home::pane_sandbox_sid(&session)),
            json!(paneflow_home::SANDBOX_SHARED_SID)
        ],
        "{report}"
    );
    assert_eq!(restricting.len(), 4, "{report}");
    assert!(
        restricting[2].as_str().unwrap().starts_with("S-1-5-5-"),
        "the third restricting SID is the logon session, which the desktop grants: {report}"
    );
    assert_eq!(
        restricting[3],
        json!("S-1-1-0"),
        "the fourth is Everyone, which the NUL device grants: {report}"
    );
    assert_eq!(report["nul_write"], json!("ok"), "{report}");
    assert_eq!(report["whoami"]["principal"], "pane", "{report}");
    assert_eq!(report["whoami"]["session"], session.as_str(), "{report}");
    let pane_host = paneflow_home::pane_endpoint_path(
        fixture.home.path(),
        &session,
        paneflow_home::PaneServer::Host,
    );
    assert_eq!(report["host_endpoint_env"], pane_host.display().to_string());
    for (path, opened) in report["shared"].as_object().unwrap() {
        assert_eq!(
            opened["write"],
            json!(ERROR_ACCESS_DENIED),
            "{path} must refuse a write open from the sandbox: {report}"
        );
    }
    assert_eq!(report["pane_host"], json!("ok"), "{report}");
    assert_eq!(report["cwd_write"], json!("ok"), "{report}");
    assert_eq!(report["temp_write"], json!("ok"), "{report}");
    let session_data = paneflow_home::host_session_data_dir_in(fixture.home.path(), &session);
    assert!(
        Path::new(report["temp_dir"].as_str().unwrap()).starts_with(&session_data),
        "the pane's TEMP is its own directory, so no launch rewrites the access list of the user's TEMP: {report}"
    );
    for (place, opened) in report["neighbor_writes"].as_object().unwrap() {
        assert_eq!(
            opened,
            &json!(ERROR_ACCESS_DENIED),
            "another sandboxed pane's {place} grants only that pane's SID, so this pane cannot plant a file there: {report}"
        );
    }
    assert_eq!(report["outside_read"], json!("ok"), "{report}");
    assert_eq!(
        report["outside_write"],
        json!(ERROR_ACCESS_DENIED),
        "{report}"
    );
    assert_eq!(report["child_pipe"], json!("child-ran"), "{report}");
    assert_eq!(
        report["named_pipe_client"],
        json!(ERROR_ACCESS_DENIED),
        "a named pipe made in the sandbox without an access list gets the pipe file system's default, which no restricting SID may write; libuv makes its child pipes that way, so Node and Bun cannot start a piped child and Windows stays unsupported (US-033): {report}"
    );
}

#[test]
fn a_sandboxed_windows_pane_whose_token_cannot_be_made_fails_its_launch_with_the_os_error() {
    let fixture = Fixture::open();
    let created = fixture.create(BTreeMap::from([(
        SANDBOX_SID_OVERRIDE.to_string(),
        "S-1-not-a-sid".to_string(),
    )]));
    match created {
        Err(HostError::SpawnFailed { reason, .. }) => {
            assert!(reason.contains("S-1-not-a-sid"), "{reason}");
            assert!(
                reason.contains(&std::io::Error::from_raw_os_error(1337).to_string()),
                "the launch failure carries the OS error: {reason}"
            );
        }
        other => panic!("a pane without its restricted token must not start: {other:?}"),
    }
    let live = fixture
        .host
        .list(None)
        .into_iter()
        .filter(|row| row.live)
        .count();
    assert_eq!(live, 0, "no shell runs without the restricted token");
}
