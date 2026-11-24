#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_in_result
)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_config::schema::{SessionId, WorkspaceId};
use paneflow_host::protocol::ClientHello;
use paneflow_host::{CreateSession, HostClient};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

const MODE_ENV: &str = "PANEFLOW_TEST_PANE_CLIENT";
const HOME_ENV: &str = "PANEFLOW_TEST_PANE_HOME";
const REPLY_WAIT: Duration = Duration::from_secs(30);

fn worker_call(control: &mut HostControl, method: &str, params: Value) -> Result<Value, Value> {
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

fn connect(home: &Path, client: &str) -> HostControl {
    HostControl::connect(&paneflow_home::serve_endpoint_path(home), client).expect("connects")
}

fn report(label: &str, answer: &Result<Value, Value>) {
    let line = match answer {
        Ok(result) => json!({"label": label, "ok": result}),
        Err(error) => json!({"label": label, "error": error}),
    };
    println!("pane-client {line}");
}

fn split_op(target: u64, pane: u64, surface: u64) -> Value {
    json!({
        "op": "split",
        "target": target,
        "direction": "vertical",
        "edge": "after",
        "pane": {
            "id": pane,
            "surfaces": [{"id": surface, "definition": {"surface_type": "terminal", "command": "make test"}}],
            "active": 0
        },
        "focus": true
    })
}

#[test]
fn pane_client() {
    let Ok(mode) = std::env::var(MODE_ENV) else {
        return;
    };
    let home = PathBuf::from(std::env::var(HOME_ENV).expect("the test names the home"));
    let mut control = connect(&home, "pane-client");
    match mode.split_once(':') {
        Some(("attach", _)) => {
            let answer = worker_call(&mut control, "layout.attach", json!({"client": "window"}));
            report("attach", &answer);
            let launch = worker_call(
                &mut control,
                "layout.launch",
                json!({"surface_id": 1, "session": SessionId::new()}),
            );
            report("launch", &launch);
        }
        Some(("apply", target)) => {
            let lease = worker_call(&mut control, "layout.lease_ids", json!({})).unwrap();
            let start = lease["start"].as_u64().unwrap();
            let target: u64 = target.parse().unwrap();
            let answer = worker_call(
                &mut control,
                "layout.apply",
                json!({"op": split_op(target, start, start + 1), "ids_from": start + 2}),
            );
            report("apply", &answer);
        }
        Some(("invoke", action)) => {
            let answer = worker_call(
                &mut control,
                "action.invoke",
                json!({"name": action, "ctx": {"origin": "cli"}}),
            );
            report("invoke", &answer);
        }
        _ => panic!("unknown pane client mode {mode}"),
    }
    println!("pane-client done");
    let _ = std::io::stdin().read_line(&mut String::new());
}

struct Instance {
    home: tempfile::TempDir,
    host: Arc<paneflow_host::SessionHost>,
    server: Option<paneflow_host::ServerHandle>,
    worker: Option<paneflow_serve::worker::RunningWorker>,
}

impl Instance {
    fn start(init_lua: Option<&str>) -> Self {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("paneflow.json"),
            serde_json::to_vec(&json!({
                "shell_integration": false,
                "ipc": {"scripting": false, "orchestration": false}
            }))
            .unwrap(),
        )
        .unwrap();
        if let Some(source) = init_lua {
            std::fs::write(paneflow_home::init_lua_path_in(home.path()), source).unwrap();
        }
        let endpoint = paneflow_home::host_endpoint_path(home.path());
        let host = paneflow_host::SessionHost::open(home.path(), &endpoint).unwrap();
        let server = paneflow_host::ServerHandle::spawn(Arc::clone(&host), endpoint).unwrap();
        let worker = common::open_starting_terminals(home.path());
        Self {
            home,
            host,
            server: Some(server),
            worker: Some(worker),
        }
    }

    fn host_client(&self) -> HostClient {
        HostClient::connect(
            &paneflow_home::host_endpoint_path(self.home.path()),
            &ClientHello::local("pane-authorization-test"),
        )
        .unwrap()
    }

    fn run_in_pane(&self, mode: &str) -> Vec<Value> {
        let mut env = std::collections::BTreeMap::new();
        env.insert(MODE_ENV.to_string(), mode.to_string());
        env.insert(HOME_ENV.to_string(), self.home.path().display().to_string());
        let request = CreateSession {
            workspace: Some(WorkspaceId::new()),
            shell: Some(std::env::current_exe().unwrap().display().to_string()),
            args: ["pane_client", "--exact", "--nocapture", "--test-threads=1"]
                .map(str::to_string)
                .to_vec(),
            env,
            cwd: Some(std::env::temp_dir().display().to_string()),
            cols: Some(400),
            rows: Some(60),
            ..CreateSession::default()
        };
        let mut client = self.host_client();
        let session = client.create(&request).unwrap().manifest.session;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let text = client.text(&session).unwrap().text;
            if text.contains("pane-client done") {
                return reports(&text);
            }
            assert!(
                Instant::now() < deadline,
                "the pane client never finished: {text}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn view(&self) -> Value {
        worker_call(
            &mut connect(self.home.path(), "pane-authorization-view"),
            "layout.view",
            json!({}),
        )
        .unwrap()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.stop();
        }
        let _ = self.host.request_shutdown(true);
        if let Some(server) = self.server.take() {
            let _ = server.stop();
        }
    }
}

fn reports(text: &str) -> Vec<Value> {
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

fn labelled<'a>(lines: &'a [Value], label: &str) -> &'a Value {
    lines
        .iter()
        .find(|line| line["label"] == label)
        .unwrap_or_else(|| panic!("no {label} line in {lines:?}"))
}

fn first_pane(view: &Value) -> u64 {
    view["view"]["state"]["workspaces"][0]["tabs"][0]["root"]["id"]
        .as_u64()
        .unwrap_or_else(|| panic!("no pane in {view}"))
}

#[test]
fn a_pane_cannot_attach_to_the_layout_nor_launch_through_it() {
    let instance = Instance::start(None);
    let lines = instance.run_in_pane("attach:");
    let attach = labelled(&lines, "attach");
    assert_eq!(
        attach["error"]["data"]["reason"], "principal_refused",
        "{attach}"
    );
    let launch = labelled(&lines, "launch");
    assert_eq!(
        launch["error"]["data"]["reason"], "principal_refused",
        "{launch}"
    );
    assert_eq!(instance.view()["attached"], false);
}

#[test]
fn a_pane_whose_op_spawns_a_command_is_refused_and_the_layout_is_unchanged() {
    let instance = Instance::start(None);
    let cwd = tempfile::tempdir().unwrap();
    let mut cli = connect(instance.home.path(), "pane-authorization-cli");
    worker_call(
        &mut cli,
        "workspace.up",
        json!({"panes": [{"cwd": cwd.path()}]}),
    )
    .unwrap();
    let before = instance.view();
    let lines = instance.run_in_pane(&format!("apply:{}", first_pane(&before)));
    let apply = labelled(&lines, "apply");
    assert_eq!(apply["error"]["code"], -32601, "{apply}");
    assert!(
        apply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("layout.apply orchestration disabled"),
        "{apply}"
    );
    let after = instance.view();
    assert_eq!(after["revision"], before["revision"]);
    assert_eq!(after["view"], before["view"]);
}

const SPAWNING_ACTION: &str = "paneflow.action{ name = 'spawn', title = 'Spawn', agents = true, run = function()\n\
       local opened, message = paneflow.workspace.up{ panes = { { command = 'echo spawned' } } }\n\
       return { opened = opened ~= nil, message = message } end }\n\
     paneflow.action{ name = 'classify', title = 'Classify', agents = true, run = function()\n\
       local answer, message = paneflow.call('peer.classify', { pid = 1, started_at = 1 })\n\
       return { answered = answer ~= nil, message = message } end }";

fn loaded(home: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = worker_call(
            &mut connect(home, "pane-authorization-status"),
            "config.status",
            json!({}),
        )
        .unwrap();
        if status["loaded_at_ms"].is_u64() {
            assert_eq!(status["problems"], json!([]), "{status}");
            return;
        }
        assert!(Instant::now() < deadline, "init.lua never loaded: {status}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn an_action_runs_as_the_window_from_a_key_and_as_its_pane_from_a_pane() {
    let instance = Instance::start(Some(SPAWNING_ACTION));
    loaded(instance.home.path());
    let from_key = worker_call(
        &mut connect(instance.home.path(), "paneflow-window-config"),
        "action.invoke",
        json!({"name": "spawn", "ctx": {"origin": "key", "client_id": "window"}}),
    )
    .unwrap();
    assert_eq!(from_key["result"]["opened"], true, "{from_key}");

    let lines = instance.run_in_pane("invoke:spawn");
    let from_pane = labelled(&lines, "invoke");
    assert_eq!(from_pane["ok"]["result"]["opened"], false, "{from_pane}");
    assert!(
        from_pane["ok"]["result"]["message"]
            .as_str()
            .unwrap()
            .contains("workspace.up orchestration disabled"),
        "{from_pane}"
    );

    let from_cli = worker_call(
        &mut connect(instance.home.path(), "pane-authorization-cli"),
        "action.invoke",
        json!({"name": "spawn", "ctx": {"origin": "cli"}}),
    )
    .unwrap();
    assert_eq!(from_cli["result"]["opened"], false, "{from_cli}");

    let lines = instance.run_in_pane("invoke:classify");
    let as_pane = labelled(&lines, "invoke");
    assert_eq!(as_pane["ok"]["result"]["answered"], false, "{as_pane}");
    assert!(
        as_pane["ok"]["result"]["message"]
            .as_str()
            .unwrap()
            .contains("does not answer a caller identified as pane"),
        "{as_pane}"
    );
    let as_external = worker_call(
        &mut connect(instance.home.path(), "pane-authorization-cli"),
        "action.invoke",
        json!({"name": "classify", "ctx": {"origin": "cli"}}),
    )
    .unwrap();
    assert_eq!(as_external["result"]["answered"], true, "{as_external}");
}
