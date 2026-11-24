#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_in_result
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_config::schema::{SessionId, WorkspaceId};
use paneflow_host::protocol::ClientHello;
use paneflow_host::{CreateSession, HostClient};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

const STEPS_ENV: &str = "PANEFLOW_TEST_GRANT_STEPS";
const HOME_ENV: &str = "PANEFLOW_TEST_GRANT_HOME";
const IDLE_ENV: &str = "PANEFLOW_TEST_GRANT_IDLE";
const REPLY_WAIT: Duration = Duration::from_secs(400);
const PATIENCE: Duration = Duration::from_secs(60);

fn call(control: &mut HostControl, method: &str, params: Value) -> Result<Value, Value> {
    let id = control
        .write_request(method, params)
        .expect("the request is written");
    loop {
        let line = control
            .read_stream_line(REPLY_WAIT)
            .expect("the server answers")
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

fn endpoint_of(home: &Path, server: &str) -> PathBuf {
    match server {
        "host" => paneflow_home::host_endpoint_path(home),
        _ => paneflow_home::serve_endpoint_path(home),
    }
}

#[test]
fn grant_pane_client() {
    if std::env::var_os(IDLE_ENV).is_some() {
        std::thread::sleep(Duration::from_secs(600));
        return;
    }
    let Ok(steps) = std::env::var(STEPS_ENV) else {
        return;
    };
    let home = PathBuf::from(std::env::var(HOME_ENV).expect("the test names the home"));
    let steps: Vec<Value> = serde_json::from_str(&steps).expect("steps are JSON");
    for step in steps {
        if let Some(ms) = step["sleep_ms"].as_u64() {
            std::thread::sleep(Duration::from_millis(ms));
            continue;
        }
        if let Some(path) = step["wait_file"].as_str() {
            let deadline = Instant::now() + PATIENCE;
            while !Path::new(path).exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            continue;
        }
        let server = step["server"].as_str().unwrap_or("worker");
        let mut control =
            HostControl::connect(&endpoint_of(&home, server), "grant-pane-client").unwrap();
        let answer = call(
            &mut control,
            step["method"].as_str().unwrap(),
            step["params"].clone(),
        );
        let line = match answer {
            Ok(result) => json!({"label": step["label"], "ok": result}),
            Err(error) => json!({"label": step["label"], "error": error}),
        };
        println!("grant-client {line}");
    }
    println!("grant-client done");
    std::thread::sleep(Duration::from_secs(300));
}

struct Instance {
    home: tempfile::TempDir,
    host: Arc<paneflow_host::SessionHost>,
    server: Option<paneflow_host::ServerHandle>,
    worker: Option<paneflow_serve::worker::RunningWorker>,
}

fn host_endpoint(home: &Path) -> PathBuf {
    paneflow_home::host_endpoint_path(home)
}

impl Instance {
    fn start(config: Value) -> Self {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("paneflow.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let endpoint = host_endpoint(home.path());
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

    fn closed() -> Self {
        Self::start(json!({
            "shell_integration": false,
            "ipc": {"scripting": false, "orchestration": false}
        }))
    }

    fn client(&self) -> HostClient {
        HostClient::connect(
            &host_endpoint(self.home.path()),
            &ClientHello::local("pane-grants-test"),
        )
        .unwrap()
    }

    fn create(&self, workspace: &WorkspaceId, env: Vec<(&str, String)>) -> SessionId {
        let request = CreateSession {
            workspace: Some(workspace.clone()),
            shell: Some(std::env::current_exe().unwrap().display().to_string()),
            args: [
                "grant_pane_client",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ]
            .map(str::to_string)
            .to_vec(),
            env: env
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
            cwd: Some(std::env::temp_dir().display().to_string()),
            cols: Some(400),
            rows: Some(80),
            ..CreateSession::default()
        };
        self.client().create(&request).unwrap().manifest.session
    }

    fn idle(&self, workspace: &WorkspaceId) -> SessionId {
        self.create(workspace, vec![(IDLE_ENV, "1".to_string())])
    }

    fn run(&self, workspace: &WorkspaceId, steps: &Value) -> Vec<Value> {
        let session = self.create(
            workspace,
            vec![
                (STEPS_ENV, steps.to_string()),
                (HOME_ENV, self.home.path().display().to_string()),
            ],
        );
        self.wait_done(&session)
    }

    fn wait_reported(&self, session: &SessionId, label: &str) {
        let mut client = self.client();
        let deadline = Instant::now() + PATIENCE;
        while !reports(&client.text(session).unwrap().text)
            .iter()
            .any(|line| line["label"] == label)
        {
            assert!(Instant::now() < deadline, "the pane never reported {label}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_done(&self, session: &SessionId) -> Vec<Value> {
        let mut client = self.client();
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let text = client.text(session).unwrap().text;
            if text.contains("grant-client done") {
                return reports(&text);
            }
            assert!(
                Instant::now() < deadline,
                "the pane client never finished: {text}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn worker(&self, method: &str, params: Value) -> Result<Value, Value> {
        call(
            &mut HostControl::connect(
                &paneflow_home::serve_endpoint_path(self.home.path()),
                "pane-grants-cli",
            )
            .unwrap(),
            method,
            params,
        )
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
        .split("grant-client ")
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

struct Human {
    thread: std::thread::JoinHandle<Vec<Value>>,
}

impl Human {
    fn answering(home: &Path, answers: Vec<&'static str>) -> Self {
        let home = home.to_path_buf();
        let mut follower =
            HostControl::connect(&paneflow_home::serve_endpoint_path(&home), "window").unwrap();
        let header = follower
            .request("prompt.follow", json!({"kinds": ["input", "pick"]}))
            .unwrap();
        assert_eq!(header["type"], "following");
        let thread = std::thread::spawn(move || {
            let mut shown = Vec::new();
            for answer in answers {
                let frame = loop {
                    let line = follower.read_stream_line(PATIENCE).unwrap().unwrap();
                    let frame: Value = serde_json::from_str(&line).unwrap();
                    if frame["type"] == "requested" {
                        break frame;
                    }
                };
                let prompt = frame;
                call(
                    &mut HostControl::connect(&paneflow_home::serve_endpoint_path(&home), "window")
                        .unwrap(),
                    "prompt.reply",
                    json!({"id": prompt["id"], "token": prompt["token"], "answer": answer}),
                )
                .unwrap();
                shown.push(prompt);
            }
            shown
        });
        Self { thread }
    }

    fn shown(self) -> Vec<Value> {
        self.thread.join().unwrap()
    }
}

fn input(label: &str, session: &SessionId) -> Value {
    json!({"label": label, "server": "host", "method": "session.input",
           "params": {"session": session, "data": "eA=="}})
}

fn refused_reason(line: &Value) -> String {
    line["error"]["data"]["reason"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("{line}"))
}

#[test]
fn a_granted_pane_writes_in_its_workspace_only_and_not_after_the_grant_expires() {
    let instance = Instance::closed();
    let own = WorkspaceId::new();
    let other = WorkspaceId::new();
    let neighbour = instance.idle(&own);
    let stranger = instance.idle(&other);
    let human = Human::answering(instance.home.path(), vec!["allow"]);
    let lines = instance.run(
        &own,
        &json!([
            input("before", &neighbour),
            {"label": "request", "method": "grant.request",
             "params": {"rights": ["write"], "scope": "workspace", "duration_secs": 3}},
            input("own", &neighbour),
            input("other", &stranger),
            {"sleep_ms": 3500},
            input("expired", &neighbour)
        ]),
    );
    let before = labelled(&lines, "before");
    assert_eq!(before["error"]["code"], -32601, "{before}");
    let request = labelled(&lines, "request");
    assert_eq!(request["ok"]["granted"], true, "{request}");
    assert!(request["ok"]["id"].is_string(), "{request}");
    assert!(request["ok"]["expires_at_ms"].is_u64(), "{request}");
    let own_write = labelled(&lines, "own");
    assert!(own_write["ok"].is_object(), "{own_write}");
    assert_eq!(
        refused_reason(labelled(&lines, "other")),
        "scope_refused",
        "{lines:?}"
    );
    let expired = labelled(&lines, "expired");
    assert_eq!(expired["error"]["code"], -32601, "{expired}");
    let shown = human.shown();
    let title = shown[0]["spec"]["title"].as_str().unwrap();
    assert!(title.contains("write"), "{title}");
    assert!(title.contains("in its own workspace"), "{title}");
    assert!(title.contains("for 3 s"), "{title}");
    assert!(title.contains("in pane"), "{title}");
}

#[test]
fn a_denied_or_unanswered_request_stores_no_grant_and_only_a_pane_may_ask() {
    let instance = Instance::closed();
    let workspace = WorkspaceId::new();
    let human = Human::answering(instance.home.path(), vec!["deny"]);
    let lines = instance.run(
        &workspace,
        &json!([{"label": "denied", "method": "grant.request",
                 "params": {"rights": ["write", "spawn"], "scope": "instance", "duration_secs": 0}}]),
    );
    assert_eq!(
        labelled(&lines, "denied")["ok"],
        json!({"granted": false, "reason": "denied"})
    );
    let title = human.shown()[0]["spec"]["title"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(title.contains("across every workspace"), "{title}");
    assert!(title.contains("with no expiry"), "{title}");
    let lines = instance.run(
        &workspace,
        &json!([{"label": "unanswered", "method": "grant.request",
                 "params": {"rights": ["write"]}}]),
    );
    assert_eq!(
        labelled(&lines, "unanswered")["ok"],
        json!({"granted": false, "reason": "unanswered"})
    );
    assert_eq!(
        instance.worker("grant.list", json!({})).unwrap()["grants"],
        json!([])
    );
    let outside = instance
        .worker("grant.request", json!({"rights": ["write"]}))
        .unwrap_err();
    assert_eq!(outside["data"]["reason"], "principal_refused", "{outside}");
}

#[test]
fn the_host_applies_the_set_the_worker_pushes_and_refuses_while_it_lags() {
    let instance = Instance::closed();
    let workspace = WorkspaceId::new();
    let target = instance.idle(&workspace);
    let mirrored_marker = instance.home.path().join("mirrored-marker");
    let marker = instance.home.path().join("lagging-marker");
    let human = Human::answering(instance.home.path(), vec!["allow"]);
    let session = instance.create(
        &workspace,
        vec![
            (
                STEPS_ENV,
                json!([
                    {"label": "request", "method": "grant.request",
                     "params": {"rights": ["write"], "duration_secs": 600}},
                    {"wait_file": mirrored_marker.display().to_string()},
                    input("synced", &target),
                    {"wait_file": marker.display().to_string()},
                    input("lagging", &target)
                ])
                .to_string(),
            ),
            (HOME_ENV, instance.home.path().display().to_string()),
        ],
    );
    human.shown();
    let grants_file = paneflow_home::serve_grants_path_in(instance.home.path());
    let deadline = Instant::now() + PATIENCE;
    while instance.host.pane_grants().snapshot().grants.is_empty() {
        assert!(
            Instant::now() < deadline,
            "the host never received the grant"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let mirrored = instance.host.pane_grants().snapshot();
    assert!(mirrored.synced);
    let published = paneflow_host::pane_grants::published_revision(&grants_file);
    assert_eq!(instance.host.pane_grants().held_revision(), published);
    std::fs::write(&mirrored_marker, "go").unwrap();
    instance.wait_reported(&session, "synced");
    let mut file: Value = serde_json::from_slice(&std::fs::read(&grants_file).unwrap()).unwrap();
    file["revision"] = json!(published + 1);
    std::fs::write(&grants_file, serde_json::to_vec(&file).unwrap()).unwrap();
    assert!(!instance.host.pane_grants().snapshot().synced);
    std::fs::write(&marker, "go").unwrap();
    let lines = instance.wait_done(&session);
    assert_eq!(labelled(&lines, "request")["ok"]["granted"], true);
    assert!(labelled(&lines, "synced")["ok"].is_object(), "{lines:?}");
    let lagging = labelled(&lines, "lagging");
    assert_eq!(lagging["error"]["code"], -32601, "{lagging}");
    assert_eq!(refused_reason(lagging), "grants_unsynced", "{lagging}");
    assert_eq!(
        lagging["error"]["message"],
        "Grants are being synchronized; retry the call"
    );
}

#[test]
fn a_grant_ends_when_its_holder_session_stops() {
    let instance = Instance::closed();
    let workspace = WorkspaceId::new();
    {
        let human = Human::answering(instance.home.path(), vec!["allow"]);
        let steps = json!([{"label": "request", "method": "grant.request",
                            "params": {"rights": ["write"], "duration_secs": 600}}]);
        let session = instance.create(
            &workspace,
            vec![
                (STEPS_ENV, steps.to_string()),
                (HOME_ENV, instance.home.path().display().to_string()),
            ],
        );
        let lines = instance.wait_done(&session);
        assert_eq!(labelled(&lines, "request")["ok"]["granted"], true);
        human.shown();
        let listed = instance.worker("grant.list", json!({})).unwrap();
        assert_eq!(listed["grants"].as_array().unwrap().len(), 1, "{listed}");
        assert_eq!(listed["grants"][0]["holder"]["session"], json!(session));
        let mut client = instance.client();
        client.stop(&session, None).unwrap();
        let deadline = Instant::now() + PATIENCE;
        loop {
            let listed = instance.worker("grant.list", json!({})).unwrap();
            if listed["grants"] == json!([])
                && instance.host.pane_grants().snapshot().grants.is_empty()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the grant outlived its session: {listed}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
