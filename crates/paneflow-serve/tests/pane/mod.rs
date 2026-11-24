#![allow(
    dead_code,
    reason = "each test binary uses the part of the pane harness it needs"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_config::schema::WorkspaceId;
use paneflow_host::protocol::ClientHello;
use paneflow_host::{CreateSession, HostClient};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

pub const MODE_ENV: &str = "PANEFLOW_TEST_PANE_CLIENT";
pub const HOME_ENV: &str = "PANEFLOW_TEST_PANE_HOME";
pub const DONE: &str = "pane-client done";
const REPLY_WAIT: Duration = Duration::from_secs(30);
const RUN_WAIT: Duration = Duration::from_secs(90);
const RELEASE_FILE: &str = "pane-client.release";

pub fn worker_call(control: &mut HostControl, method: &str, params: Value) -> Result<Value, Value> {
    let id = control
        .write_request(method, params)
        .expect("the request is written");
    loop {
        let line = control
            .read_stream_line(REPLY_WAIT)
            .expect("the worker answers")
            .expect("the connection stays open");
        let envelope: Value = serde_json::from_str(&line).expect("a JSON frame");
        if envelope["id"].as_u64() != Some(id) {
            continue;
        }
        if envelope["error"].is_object() {
            return Err(envelope["error"].clone());
        }
        return Ok(envelope["result"].clone());
    }
}

pub fn connect(home: &Path, client: &str) -> HostControl {
    HostControl::connect(&paneflow_home::serve_endpoint_path(home), client).expect("connects")
}

pub fn report(label: &str, answer: &Result<Value, Value>) {
    let line = match answer {
        Ok(result) => json!({"label": label, "ok": result}),
        Err(error) => json!({"label": label, "error": error}),
    };
    println!("pane-client {line}");
}

pub fn pane_mode() -> Option<(String, PathBuf)> {
    let mode = std::env::var(MODE_ENV).ok()?;
    let home = PathBuf::from(std::env::var(HOME_ENV).expect("the test names the home"));
    wait_until_the_host_recorded_the_pane_shell(&home);
    Some((mode, home))
}

fn wait_until_the_host_recorded_the_pane_shell(home: &Path) {
    let Some(session) = std::env::var("PANEFLOW_SESSION_ID")
        .ok()
        .and_then(|raw| paneflow_config::schema::SessionId::parse(&raw).ok())
    else {
        return;
    };
    let path = paneflow_host::manifest::manifest_path(home, &session);
    let deadline = Instant::now() + REPLY_WAIT;
    loop {
        let recorded = paneflow_host::manifest::read_manifest(&path)
            .ok()
            .and_then(|manifest| manifest.process)
            .is_some_and(|process| process.started_at.is_some());
        if recorded || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub fn finish(home: &Path) {
    println!("{DONE}");
    hold_until_released(home);
}

fn hold_until_released(home: &Path) {
    let release = home.join(RELEASE_FILE);
    let deadline = Instant::now() + RUN_WAIT;
    while !release.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn reports(text: &str) -> Vec<Value> {
    let joined: String = text.lines().map(str::trim_end).collect();
    joined
        .split("pane-client ")
        .filter_map(|rest| {
            serde_json::Deserializer::from_str(rest)
                .into_iter::<Value>()
                .next()
                .and_then(Result::ok)
        })
        .collect()
}

pub fn labelled<'a>(lines: &'a [Value], label: &str) -> &'a Value {
    lines
        .iter()
        .find(|line| line["label"] == label)
        .unwrap_or_else(|| panic!("no {label} line in {lines:?}"))
}

pub struct Instance {
    home: tempfile::TempDir,
    host: Arc<paneflow_host::SessionHost>,
    server: Option<paneflow_host::ServerHandle>,
    worker: Option<paneflow_serve::worker::RunningWorker>,
}

impl Instance {
    pub fn start(config: &Value, init_lua: Option<&str>) -> Self {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("paneflow.json"),
            serde_json::to_vec(config).unwrap(),
        )
        .unwrap();
        if let Some(source) = init_lua {
            std::fs::write(paneflow_home::init_lua_path_in(home.path()), source).unwrap();
        }
        let endpoint = paneflow_home::host_endpoint_path(home.path());
        let host = paneflow_host::SessionHost::open(home.path(), &endpoint).unwrap();
        let server = paneflow_host::ServerHandle::spawn(Arc::clone(&host), endpoint).unwrap();
        let worker = paneflow_serve::open(home.path()).unwrap();
        Self {
            home,
            host,
            server: Some(server),
            worker: Some(worker),
        }
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    pub fn release_panes(&self) {
        std::fs::write(self.home.path().join(RELEASE_FILE), b"").unwrap();
    }

    pub fn run_in_pane(&self, test: &str, mode: &str) -> Vec<Value> {
        self.run_program_in_pane(&std::env::current_exe().unwrap(), test, mode)
    }

    pub fn run_program_in_pane(&self, program: &Path, test: &str, mode: &str) -> Vec<Value> {
        let mut env = BTreeMap::new();
        env.insert(MODE_ENV.to_string(), mode.to_string());
        env.insert(HOME_ENV.to_string(), self.home.path().display().to_string());
        let request = CreateSession {
            workspace: Some(WorkspaceId::new()),
            shell: Some(program.display().to_string()),
            args: [test, "--exact", "--nocapture", "--test-threads=1"]
                .map(str::to_string)
                .to_vec(),
            env,
            cwd: Some(std::env::temp_dir().display().to_string()),
            cols: Some(400),
            rows: Some(60),
            ..CreateSession::default()
        };
        let mut client = HostClient::connect(
            &paneflow_home::host_endpoint_path(self.home.path()),
            &ClientHello::local("pane-test"),
        )
        .unwrap();
        let session = client.create(&request).unwrap().manifest.session;
        let deadline = Instant::now() + RUN_WAIT;
        loop {
            let text = client.text(&session).unwrap().text;
            if text.contains(DONE) {
                return reports(&text);
            }
            assert!(
                Instant::now() < deadline,
                "the pane client never finished: {text}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::write(self.home.path().join(RELEASE_FILE), b"");
        if let Some(worker) = self.worker.take() {
            worker.stop();
        }
        let _ = self.host.request_shutdown(true);
        if let Some(server) = self.server.take() {
            let _ = server.stop();
        }
    }
}
