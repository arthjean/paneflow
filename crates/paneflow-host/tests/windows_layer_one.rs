#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(windows)]

use std::collections::{BTreeMap, HashMap};
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::{AsRawHandle as _, BorrowedHandle, FromRawHandle as _, OwnedHandle};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::server::ServerHandle;
use paneflow_host::{CreateSession, SessionHost, SessionId};
use paneflow_peer::PeerRead;
use serde_json::{Value, json};
use windows_sys::Win32::Foundation::{
    HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{IsProcessInJob, OpenJobObjectW};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::SystemServices::JOB_OBJECT_QUERY;
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

const OUTCOME_DEADLINE: Duration = Duration::from_secs(60);
const INHERITED_HANDLE_ENV: &str = "PANEFLOW_LAYER_ONE_INHERITED_PIPE";
const REPORT_ENV: &str = "PANEFLOW_LAYER_ONE_REPORT";
const FILE_DEVICE_NAMED_PIPE: u32 = 0x11;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

#[repr(C)]
struct IoStatusBlock {
    status: usize,
    information: usize,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtFsControlFile(
        file: HANDLE,
        event: HANDLE,
        apc: *mut core::ffi::c_void,
        apc_context: *mut core::ffi::c_void,
        io: *mut IoStatusBlock,
        code: u32,
        input: *const core::ffi::c_void,
        input_length: u32,
        output: *mut core::ffi::c_void,
        output_length: u32,
    ) -> i32;
}

const fn pipe_control(function: u32) -> u32 {
    (FILE_DEVICE_NAMED_PIPE << 16) | (function << 2)
}

const FSCTL_PIPE_SET_CLIENT_PROCESS: u32 = pipe_control(8);
const FSCTL_PIPE_QUERY_CLIENT_PROCESS: u32 = pipe_control(9);
const FSCTL_PIPE_GET_CONNECTION_ATTRIBUTE: u32 = pipe_control(12);
const FSCTL_PIPE_SET_CONNECTION_ATTRIBUTE: u32 = pipe_control(13);

fn wide(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

fn write_report(name: &str, report: &Value) {
    if let Some(dir) = std::env::var_os(REPORT_ENV) {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{name}.json")),
            serde_json::to_string_pretty(report).unwrap(),
        )
        .unwrap();
    }
    println!("{name}: {report}");
}

fn processes() -> HashMap<u32, (u32, String)> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    assert_ne!(snapshot, INVALID_HANDLE_VALUE);
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = HashMap::new();
    let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        found.insert(
            entry.th32ProcessID,
            (
                entry.th32ParentProcessID,
                String::from_utf16_lossy(&entry.szExeFile[..length]),
            ),
        );
        more = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    found
}

fn in_job(job: HANDLE, pid: u32) -> Value {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return json!({"open_error": std::io::Error::last_os_error().raw_os_error()});
    }
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    let mut member = 0;
    assert_ne!(
        unsafe { IsProcessInJob(process.as_raw_handle(), job, &mut member) },
        0
    );
    json!(member != 0)
}

struct Fixture {
    _home: tempfile::TempDir,
    cwd: tempfile::TempDir,
    host: Arc<SessionHost>,
    server: Option<ServerHandle>,
}

impl Fixture {
    fn open() -> Self {
        let home = tempfile::Builder::new().prefix("pfl").tempdir().unwrap();
        std::fs::write(
            home.path().join("paneflow.json"),
            r#"{"shell_integration": false}"#,
        )
        .unwrap();
        let endpoint = paneflow_home::host_endpoint_path(home.path());
        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let server = ServerHandle::spawn(Arc::clone(&host), endpoint).unwrap();
        Self {
            _home: home,
            cwd: tempfile::Builder::new().prefix("pfw").tempdir().unwrap(),
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

#[test]
fn a_conpty_pane_job_holds_the_shell_and_its_child_but_not_the_console_host() {
    let fixture = Fixture::open();
    let before: Vec<u32> = processes().into_keys().collect();
    let summary = fixture
        .host
        .create(CreateSession {
            cwd: Some(fixture.cwd.path().display().to_string()),
            shell: Some("cmd.exe".into()),
            args: vec![
                "/d".into(),
                "/c".into(),
                "ping -n 120 127.0.0.1 >NUL".into(),
            ],
            env: BTreeMap::new(),
            ..CreateSession::default()
        })
        .unwrap();
    let session = summary.manifest.session.to_string();
    let shell = summary.manifest.process.unwrap().pid;
    let deadline = Instant::now() + OUTCOME_DEADLINE;
    let (child, console_hosts) = loop {
        let table = processes();
        let child = table
            .iter()
            .find(|(_, (parent, _))| *parent == shell)
            .map(|(pid, (_, name))| (*pid, name.clone()));
        let console_hosts: Vec<(u32, String)> = table
            .iter()
            .filter(|(pid, (parent, name))| {
                *parent == std::process::id()
                    && !before.contains(pid)
                    && ["openconsole.exe", "conhost.exe"].contains(&name.to_lowercase().as_str())
            })
            .map(|(pid, (_, name))| (*pid, name.clone()))
            .collect();
        if let Some(child) = child.filter(|_| !console_hosts.is_empty()) {
            break (child, console_hosts);
        }
        assert!(
            Instant::now() < deadline,
            "the shell's child or the console host never appeared"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let name = wide(&paneflow_peer::pane_job_name(&session));
    let job = unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, 0, name.as_ptr()) };
    assert!(!job.is_null(), "{}", std::io::Error::last_os_error());
    let job = unsafe { OwnedHandle::from_raw_handle(job) };
    let report = json!({
        "shell": {"pid": shell, "in_job": in_job(job.as_raw_handle(), shell)},
        "child": {"pid": child.0, "name": child.1, "in_job": in_job(job.as_raw_handle(), child.0)},
        "console_hosts": console_hosts
            .iter()
            .map(|(pid, name)| json!({"pid": pid, "name": name, "in_job": in_job(job.as_raw_handle(), *pid)}))
            .collect::<Vec<_>>(),
    });
    write_report("pane_job_membership", &report);
    assert_eq!(report["shell"]["in_job"], json!(true), "{report}");
    assert_eq!(report["child"]["in_job"], json!(true), "{report}");
    for host in report["console_hosts"].as_array().unwrap() {
        assert_eq!(
            host["in_job"],
            json!(false),
            "ConPTY starts the console host from the host process, before the shell joins its job: {report}"
        );
    }
    let stop = SessionId::parse(&session).unwrap();
    fixture.host.stop(&stop, None).unwrap();
}

struct PipePair {
    server: OwnedHandle,
    client: OwnedHandle,
}

fn pipe_pair(label: &str) -> PipePair {
    let name = wide(&format!(
        r"\\.\pipe\paneflow-layer-one-{}-{label}",
        std::process::id()
    ));
    let server = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_WAIT,
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(server, INVALID_HANDLE_VALUE);
    let server = unsafe { OwnedHandle::from_raw_handle(server) };
    let client = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(client, INVALID_HANDLE_VALUE);
    unsafe { ConnectNamedPipe(server.as_raw_handle(), std::ptr::null_mut()) };
    PipePair {
        server,
        client: unsafe { OwnedHandle::from_raw_handle(client) },
    }
}

fn client_pid(server: &OwnedHandle) -> u32 {
    let mut pid = 0;
    assert_ne!(
        unsafe { GetNamedPipeClientProcessId(server.as_raw_handle(), &mut pid) },
        0
    );
    pid
}

fn control(handle: &OwnedHandle, code: u32, input: &[u8], output: &mut [u8]) -> (i32, usize) {
    let mut io = IoStatusBlock {
        status: 0,
        information: 0,
    };
    let status = unsafe {
        NtFsControlFile(
            handle.as_raw_handle(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut io,
            code,
            input.as_ptr().cast(),
            input.len() as u32,
            output.as_mut_ptr().cast(),
            output.len() as u32,
        )
    };
    (status, io.information)
}

fn exchange_then_read(pair: &PipePair) -> Value {
    let mut client = std::fs::File::from(pair.client.try_clone().unwrap());
    std::io::Write::write_all(&mut client, b"x").unwrap();
    let mut server = std::fs::File::from(pair.server.try_clone().unwrap());
    let mut byte = [0u8; 1];
    std::io::Read::read_exact(&mut server, &mut byte).unwrap();
    peer_read(&pair.server)
}

fn peer_read(server: &OwnedHandle) -> Value {
    match paneflow_peer::read_pipe_peer(unsafe {
        BorrowedHandle::borrow_raw(server.as_raw_handle())
    }) {
        PeerRead::Identified(identity) => json!({"pid": identity.pid()}),
        other => json!({"unverified": format!("{other:?}")}),
    }
}

fn sleeper() -> std::process::Child {
    Command::new("ping.exe")
        .args(["-n", "60", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn child_writes_on_an_inherited_pipe_handle() {
    let Ok(raw) = std::env::var(INHERITED_HANDLE_ENV) else {
        return;
    };
    let handle = raw.parse::<usize>().unwrap() as HANDLE;
    let mut pipe = std::fs::File::from(unsafe { OwnedHandle::from_raw_handle(handle) });
    std::io::Write::write_all(&mut pipe, b"from-child").unwrap();
}

#[test]
fn a_pipe_client_cannot_forge_its_pid_through_the_pipe_controls_and_an_inherited_handle_presents_its_opener()
 {
    let me = std::process::id();
    let mut other = sleeper();
    let forged = other.id();

    let set_client = pipe_pair("set-client");
    let mut current = [0u8; 16];
    let (query_status, _) = control(
        &set_client.client,
        FSCTL_PIPE_QUERY_CLIENT_PROCESS,
        &[],
        &mut current,
    );
    let mut buffer = [0u8; 16];
    buffer[8..16].copy_from_slice(&(forged as u64).to_le_bytes());
    let (set_status, _) = control(
        &set_client.client,
        FSCTL_PIPE_SET_CLIENT_PROCESS,
        &buffer,
        &mut [],
    );
    let set_client_report = json!({
        "query_status": format!("{query_status:#x}"),
        "set_status": format!("{set_status:#x}"),
        "server_reads": client_pid(&set_client.server),
        "peer_read": exchange_then_read(&set_client),
    });

    let attribute = pipe_pair("attribute");
    let mut name = b"ClientProcessId\0".to_vec();
    let mut read_back = [0u8; 8];
    let (get_status, got) = control(
        &attribute.server,
        FSCTL_PIPE_GET_CONNECTION_ATTRIBUTE,
        &name,
        &mut read_back,
    );
    name.extend_from_slice(&(forged as u64).to_le_bytes());
    let (set_attribute_status, _) = control(
        &attribute.client,
        FSCTL_PIPE_SET_CONNECTION_ATTRIBUTE,
        &name,
        &mut [],
    );
    let attribute_report = json!({
        "get_status": format!("{get_status:#x}"),
        "get_value": (got >= 4).then(|| u32::from_le_bytes(read_back[..4].try_into().unwrap())),
        "set_status": format!("{set_attribute_status:#x}"),
        "server_reads": client_pid(&attribute.server),
        "peer_read": exchange_then_read(&attribute),
    });

    let inherited = pipe_pair("inherited");
    assert_ne!(
        unsafe {
            SetHandleInformation(
                inherited.client.as_raw_handle(),
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            )
        },
        0
    );
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_writes_on_an_inherited_pipe_handle",
            "--nocapture",
        ])
        .env(
            INHERITED_HANDLE_ENV,
            (inherited.client.as_raw_handle() as usize).to_string(),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let writer = child.id();
    drop(inherited.client);
    let mut server = std::fs::File::from(inherited.server.try_clone().unwrap());
    let mut received = [0u8; 10];
    std::io::Read::read_exact(&mut server, &mut received).unwrap();
    let inherited_report = json!({
        "writer": writer,
        "received": String::from_utf8_lossy(&received),
        "server_reads": client_pid(&inherited.server),
        "peer_read": peer_read(&inherited.server),
    });
    let _ = child.wait();
    let _ = other.kill();
    let _ = other.wait();

    let report = json!({
        "this_process": me,
        "forged_pid": forged,
        "fsctl_pipe_set_client_process": set_client_report,
        "fsctl_pipe_set_connection_attribute": attribute_report,
        "inherited_handle": inherited_report,
    });
    write_report("pipe_pid_forgery", &report);
    assert_eq!(
        report["fsctl_pipe_set_connection_attribute"]["get_value"],
        json!(me),
        "the attribute read names the opener, which proves the control code: {report}"
    );
    for technique in [
        "fsctl_pipe_set_client_process",
        "fsctl_pipe_set_connection_attribute",
    ] {
        assert_ne!(
            report[technique]["set_status"],
            json!("0x0"),
            "a user-mode client must not set its pipe PID: {report}"
        );
        assert_eq!(report[technique]["server_reads"], json!(me), "{report}");
        assert_eq!(report[technique]["peer_read"]["pid"], json!(me), "{report}");
    }
    assert_eq!(
        report["inherited_handle"]["server_reads"],
        json!(me),
        "a handle passed to another process keeps naming the process that opened it: {report}"
    );
    assert_eq!(
        report["inherited_handle"]["peer_read"]["pid"],
        json!(me),
        "the token-user check accepts the same user, so it does not refuse the writer: {report}"
    );
}
