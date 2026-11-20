use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::AppContext as _;
use interprocess::local_socket::{GenericFilePath, Stream, prelude::*};
use serde_json::{Value, json};

use crate::PaneFlowApp;
use crate::app::bootstrap::Startup;
use crate::terminal::TerminalView;
use crate::terminal::view::DisplayOnlyTerminals;
use crate::workspace::{Workspace, next_workspace_id_in};

pub(crate) const CASE_FILE: &str = include_str!("../../../../protocol/app-conformance-v1.json");

const CHILD_ENV: &str = "PANEFLOW_APP_CONFORMANCE_CHILD";
const READY_MARKER: &str = "paneflow-app-conformance ready";
const CHILD_TEST: &str = "app::ipc_handler::conformance::serve_the_conformance_fixture";
const FIXTURE_TEXT: &[u8] = b"conformance line one\r\nconformance line two\r\n";
const REPLY_WAIT: Duration = Duration::from_secs(60);
const PUMP_INTERVAL: Duration = Duration::from_millis(2);
const SURFACE_PLACEHOLDER: &str = "$surface";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gates {
    Closed,
    Open,
}

impl Gates {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "closed" => Some(Self::Closed),
            "open" => Some(Self::Open),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
        }
    }
}

#[derive(Debug)]
pub(crate) enum Expectation {
    Result { shape: Value, values: Value },
    Error { code: i64, message: Option<String> },
    Stream { frames: Vec<Value> },
}

#[derive(Debug)]
pub(crate) struct Case {
    pub(crate) id: String,
    pub(crate) method: String,
    pub(crate) gates: Gates,
    pub(crate) params: Value,
    pub(crate) expect: Expectation,
}

pub(crate) fn document() -> Value {
    serde_json::from_str(CASE_FILE).expect("the app case file is valid JSON")
}

pub(crate) fn parse_case(raw: &Value) -> Result<Case, String> {
    let id = raw["id"]
        .as_str()
        .ok_or("a case carries no id")?
        .to_string();
    let method = raw["method"]
        .as_str()
        .ok_or_else(|| format!("case {id}: no method"))?
        .to_string();
    let gates = raw
        .get("gates")
        .map_or(Some(Gates::Closed), |gates| {
            gates.as_str().and_then(Gates::parse)
        })
        .ok_or_else(|| format!("case {id}: no runner for gates {}", raw["gates"]))?;
    let params = raw.get("params").cloned().unwrap_or_else(|| json!({}));
    let expect = raw
        .get("expect")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("case {id}: no expectation"))?;
    let expect = match expect
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["result"] | ["result", "values"] | ["values", "result"] => Expectation::Result {
            shape: expect["result"].clone(),
            values: expect.get("values").cloned().unwrap_or_else(|| json!({})),
        },
        ["error"] => Expectation::Error {
            code: expect["error"]["code"]
                .as_i64()
                .ok_or_else(|| format!("case {id}: an error expectation needs a code"))?,
            message: expect["error"]["message"].as_str().map(str::to_string),
        },
        ["stream"] => Expectation::Stream {
            frames: expect["stream"]
                .as_array()
                .ok_or_else(|| format!("case {id}: a stream expectation lists frames"))?
                .clone(),
        },
        other => return Err(format!("case {id}: no runner for expectation {other:?}")),
    };
    Ok(Case {
        id,
        method,
        gates,
        params,
        expect,
    })
}

pub(crate) fn cases() -> Vec<Case> {
    document()["cases"]
        .as_array()
        .expect("the case file lists cases")
        .iter()
        .map(|raw| parse_case(raw).unwrap_or_else(|error| panic!("{error}")))
        .collect()
}

pub(crate) fn shape(value: &Value) -> Value {
    match value {
        Value::Null => json!("null"),
        Value::Bool(_) => json!("boolean"),
        Value::Number(number) if number.is_u64() || number.is_i64() => json!("integer"),
        Value::Number(_) => json!("number"),
        Value::String(_) => json!("string"),
        Value::Array(items) => Value::Array(items.first().map(shape).into_iter().collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), shape(value)))
                .collect(),
        ),
    }
}

fn values_match(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => expected
            .iter()
            .all(|(key, value)| actual.get(key).is_some_and(|got| values_match(value, got))),
        _ => expected == actual,
    }
}

pub(crate) fn check(case: &Case, reply: &Value) -> Result<(), String> {
    let id = &case.id;
    match &case.expect {
        Expectation::Result {
            shape: want,
            values,
        } => {
            let Some(result) = reply.get("result") else {
                return Err(format!("case {id}: expected a result, got {reply}"));
            };
            let got = shape(result);
            if &got != want {
                return Err(format!(
                    "case {id}: the result shape changed\n  expected {want}\n  actual   {got}"
                ));
            }
            if !values_match(values, result) {
                return Err(format!(
                    "case {id}: the result values changed\n  expected {values}\n  actual   {result}"
                ));
            }
            Ok(())
        }
        Expectation::Error { code, message } => {
            let error = reply
                .get("error")
                .ok_or_else(|| format!("case {id}: expected error {code}, got {reply}"))?;
            if error["code"].as_i64() != Some(*code) {
                return Err(format!(
                    "case {id}: expected error code {code}, got {error}"
                ));
            }
            if let Some(message) = message
                && error["message"].as_str() != Some(message.as_str())
            {
                return Err(format!(
                    "case {id}: expected message {message:?}, got {}",
                    error["message"]
                ));
            }
            Ok(())
        }
        Expectation::Stream { .. } => Err(format!("case {id}: a stream case reads frames")),
    }
}

fn check_frames(case: &Case, frames: &[Value]) -> Result<(), String> {
    let Expectation::Stream { frames: want } = &case.expect else {
        return Err(format!("case {}: not a stream case", case.id));
    };
    let got: Vec<Value> = frames.iter().map(shape).collect();
    let typed = |frames: &[Value]| -> Vec<Value> {
        frames.iter().map(|frame| frame["type"].clone()).collect()
    };
    if &got != want {
        return Err(format!(
            "case {}: the stream frames changed\n  expected {want:?}\n  actual   {got:?} {:?}",
            case.id,
            typed(frames)
        ));
    }
    Ok(())
}

fn connect(endpoint: &Path) -> std::io::Result<Stream> {
    let name = endpoint.to_fs_name::<GenericFilePath>()?;
    Stream::connect(name)
}

fn connect_with_retry(endpoint: &Path) -> Stream {
    let deadline = Instant::now() + REPLY_WAIT;
    loop {
        match connect(endpoint) {
            Ok(stream) => return stream,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => panic!(
                "the app endpoint {} never answered: {error}",
                endpoint.display()
            ),
        }
    }
}

fn request_line(method: &str, params: &Value) -> Vec<u8> {
    let mut line = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    }))
    .expect("a request serializes");
    line.push(b'\n');
    line
}

pub(crate) fn call(endpoint: &Path, method: &str, params: &Value) -> Value {
    let mut stream = connect_with_retry(endpoint);
    stream
        .write_all(&request_line(method, params))
        .expect("the request is written");
    let mut line = String::new();
    BufReader::new(&mut stream)
        .read_line(&mut line)
        .expect("the reply arrives");
    serde_json::from_str(line.trim()).unwrap_or_else(|error| panic!("{method}: {error}: {line:?}"))
}

fn read_frames(endpoint: &Path, method: &str, params: &Value, count: usize) -> Vec<Value> {
    let mut stream = connect_with_retry(endpoint);
    stream
        .write_all(&request_line(method, params))
        .expect("the request is written");
    let mut reader = BufReader::new(stream);
    (0..count)
        .map(|_| {
            let mut line = String::new();
            reader.read_line(&mut line).expect("a frame arrives");
            serde_json::from_str(line.trim()).unwrap_or_else(|error| panic!("{error}: {line:?}"))
        })
        .collect()
}

fn first_surface(endpoint: &Path) -> Value {
    let listed = call(endpoint, "surface.list", &json!({}));
    listed["result"]["surfaces"][0]["surface_id"].clone()
}

fn resolve_placeholders(params: &Value, surface: &dyn Fn() -> Value) -> Value {
    match params {
        Value::String(text) if text == SURFACE_PLACEHOLDER => surface(),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve_placeholders(item, surface))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), resolve_placeholders(value, surface)))
                .collect(),
        ),
        other => other.clone(),
    }
}

pub(crate) fn run_case(endpoint: &Path, case: &Case) -> Result<(), String> {
    let surface = || first_surface(endpoint);
    let params = resolve_placeholders(&case.params, &surface);
    match &case.expect {
        Expectation::Stream { frames } => {
            let got = read_frames(endpoint, &case.method, &params, frames.len());
            check_frames(case, &got)
        }
        _ => check(case, &call(endpoint, &case.method, &params)),
    }
}

struct FixtureChild {
    child: Child,
    stdin: Option<ChildStdin>,
    endpoint: PathBuf,
    output: Arc<Mutex<Vec<String>>>,
    _home: tempfile::TempDir,
}

impl FixtureChild {
    fn spawn(gates: Gates) -> Self {
        let home = tempfile::tempdir().expect("a conformance home");
        let mut command = Command::new(std::env::current_exe().expect("the test binary"));
        command
            .args([
                CHILD_TEST,
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, "1")
            .env(paneflow_home::HOME_ENV, home.path())
            .env_remove("PANEFLOW_SOCKET_PATH")
            .env_remove("PANEFLOW_ALLOW_SOCKET_OVERRIDE")
            .env_remove("PANEFLOW_ALLOW_MULTIPLE")
            .env_remove("PANEFLOW_IPC_SCRIPTING")
            .env_remove("PANEFLOW_IPC_ORCHESTRATION")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in paneflow_host::env::PANE_CONTEXT_ENV {
            command.env_remove(key);
        }
        if gates == Gates::Open {
            command.env("PANEFLOW_IPC_SCRIPTING", "1");
        }
        let mut child = command.spawn().expect("the fixture app starts");
        let stdin = child.stdin.take();
        let output = Arc::new(Mutex::new(Vec::new()));
        let (ready_tx, ready_rx) = channel();
        let stdout = child.stdout.take().expect("piped stdout");
        let sink = Arc::clone(&output);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some((_, endpoint)) = line.split_once(READY_MARKER) {
                    let _ = ready_tx.send(PathBuf::from(endpoint.trim()));
                }
                sink.lock().expect("output lock").push(line);
            }
        });
        let stderr = child.stderr.take().expect("piped stderr");
        let sink = Arc::clone(&output);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                sink.lock().expect("output lock").push(line);
            }
        });
        let endpoint = Self::await_ready(&ready_rx, &mut child, &output);
        Self {
            child,
            stdin,
            endpoint,
            output,
            _home: home,
        }
    }

    fn await_ready(
        ready: &Receiver<PathBuf>,
        child: &mut Child,
        output: &Arc<Mutex<Vec<String>>>,
    ) -> PathBuf {
        match ready.recv_timeout(REPLY_WAIT) {
            Ok(endpoint) => endpoint,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "the fixture app never reported ready:\n{}",
                    output.lock().expect("output lock").join("\n")
                );
            }
        }
    }

    fn home(&self) -> &Path {
        self._home.path()
    }

    fn finish(mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + REPLY_WAIT;
        loop {
            match self
                .child
                .try_wait()
                .expect("the fixture app can be awaited")
            {
                Some(status) => {
                    assert!(
                        status.success(),
                        "the fixture app failed:\n{}",
                        self.output.lock().expect("output lock").join("\n")
                    );
                    return;
                }
                None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                None => {
                    let _ = self.child.kill();
                    panic!("the fixture app did not stop after its stdin closed");
                }
            }
        }
    }
}

impl Drop for FixtureChild {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub(crate) fn run_profile(gates: Gates, cases: &[Case]) -> Vec<String> {
    let fixture = FixtureChild::spawn(gates);
    let failures: Vec<String> = cases
        .iter()
        .filter(|case| case.gates == gates)
        .filter_map(|case| run_case(&fixture.endpoint, case).err())
        .map(|failure| format!("[gates {}] {failure}", gates.label()))
        .collect();
    fixture.finish();
    failures
}

impl PaneFlowApp {
    fn install_conformance_fixture(&mut self, cx: &mut gpui::Context<Self>) {
        for title in ["alpha", "beta"] {
            let ws_id = next_workspace_id_in(None);
            let terminal = cx.new(|cx| TerminalView::new(ws_id, cx));
            terminal.read(cx).terminal.write_output(FIXTURE_TEXT);
            let surface_id = terminal.read(cx).surface_id();
            let pane = self.create_pane(terminal, ws_id, cx);
            let idx = self.add_workspace_in(Workspace::with_id(ws_id, title, pane), None, cx);
            if title == "alpha" {
                self.workspaces[idx]
                    .agent_sessions
                    .insert(4242, conformance_agent(surface_id));
            }
        }
        self.active_idx = 0;
    }
}

fn conformance_agent(surface_id: u64) -> crate::ai_types::AgentSession {
    let mut session = crate::ai_types::AgentSession::new(
        crate::agent_launcher::TerminalAgent::ClaudeCode,
        crate::ai_types::AgentState::WaitingForInput,
    );
    session.surface_id = Some(surface_id);
    session.active_tool_name = Some("Bash".to_string());
    session.message = Some("needs a decision".to_string());
    session.last_result = Some("done".to_string());
    session.waiting_since = Some(Instant::now());
    session.state_seq = 3;
    session
}

#[gpui::test]
#[ignore = "started by every_app_conformance_case_passes_over_the_real_endpoint in its own process and home"]
fn serve_the_conformance_fixture(cx: &mut gpui::TestAppContext) {
    assert_eq!(
        std::env::var(CHILD_ENV).as_deref(),
        Ok("1"),
        "the fixture app runs only as the conformance runner's child"
    );
    cx.executor().allow_parking();
    cx.update(|cx| cx.set_global(DisplayOnlyTerminals));
    let app = cx.new(|cx| PaneFlowApp::start(Startup::Conformance, cx));
    app.update(cx, |app, cx| app.install_conformance_fixture(cx));
    cx.run_until_parked();
    let endpoint = crate::runtime_paths::socket_path().expect("the fixture home has an endpoint");
    println!("{READY_MARKER} {}", endpoint.display());
    let stdin_open = Arc::new(AtomicBool::new(true));
    let watcher = Arc::clone(&stdin_open);
    std::thread::spawn(move || {
        let mut sink = String::new();
        while std::io::stdin()
            .read_line(&mut sink)
            .is_ok_and(|read| read > 0)
        {
            sink.clear();
        }
        watcher.store(false, Ordering::SeqCst);
    });
    while stdin_open.load(Ordering::SeqCst) {
        cx.run_until_parked();
        std::thread::sleep(PUMP_INTERVAL);
    }
    drop(app);
}

#[test]
fn every_app_conformance_case_passes_over_the_real_endpoint() {
    let cases = cases();
    let mut failures = run_profile(Gates::Closed, &cases);
    failures.extend(run_profile(Gates::Open, &cases));
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn registry_validation_answers_over_the_real_endpoint() {
    let cases: Vec<Case> = [
        json!({
            "id": "extra_parameter_is_accepted",
            "method": "surface.read",
            "params": {"surface_id": SURFACE_PLACEHOLDER, "lines": 2, "fenced": false, "from_a_newer_client": true},
            "expect": {"result": {
                "text": "string", "lines": "integer", "total_lines": "integer",
                "eof": "boolean", "output_generation": "integer", "truncated": "boolean"
            }},
        }),
        json!({
            "id": "mistyped_parameter_is_named",
            "method": "surface.read",
            "params": {"surface_id": SURFACE_PLACEHOLDER, "lines": "50"},
            "expect": {"error": {"code": -32602, "message": "'lines' must be a non-negative integer"}},
        }),
        json!({
            "id": "nested_parameter_is_named",
            "method": "surface.split",
            "params": {"direction": "vertical", "env": {"PORT": 8080}},
            "expect": {"error": {"code": -32602, "message": "'env.PORT' must be a string"}},
        }),
    ]
    .iter()
    .map(|raw| parse_case(raw).unwrap_or_else(|error| panic!("{error}")))
    .collect();
    let failures = run_profile(Gates::Closed, &cases);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn system_describe_answers_over_the_real_endpoint() {
    let fixture = FixtureChild::spawn(Gates::Closed);
    let endpoint = &fixture.endpoint;
    let catalog = paneflow_api::catalog().expect("catalog");

    let all = call(endpoint, "system.describe", &json!({}));
    let methods = all["result"]["methods"].as_array().expect("a method list");
    let names: Vec<&str> = methods.iter().filter_map(|m| m["name"].as_str()).collect();
    assert_eq!(names, catalog.names());
    for method in methods {
        for field in [
            "summary", "owner", "gate", "stream", "params", "result", "examples",
        ] {
            assert!(
                method.get(field).is_some(),
                "{} has no {field}",
                method["name"]
            );
        }
        assert_eq!(
            method["params"]["$schema"],
            paneflow_api::schema::META_SCHEMA
        );
        assert_eq!(
            method["result"]["$schema"],
            paneflow_api::schema::META_SCHEMA
        );
        let served = method["owner"] == "window";
        assert_eq!(method["callable"], served, "{}", method["name"]);
    }

    let one = call(
        endpoint,
        "system.describe",
        &json!({"method": "surface.read"}),
    );
    assert_eq!(one["result"]["name"], "surface.read");
    assert_eq!(one["result"]["owner"], "window");
    assert_eq!(one["result"]["reply"], "deferred");
    assert!(one["result"]["params"]["properties"]["lines"].is_object());

    let unknown = call(
        endpoint,
        "system.describe",
        &json!({"method": "surface.teleport"}),
    );
    assert_eq!(unknown["error"]["code"], -32602);
    let message = unknown["error"]["message"].as_str().expect("a message");
    assert!(message.contains("surface.teleport"), "{message}");

    let mistyped = call(endpoint, "system.describe", &json!({"method": 5}));
    assert_eq!(mistyped["error"]["code"], -32602);

    let capabilities = call(endpoint, "system.capabilities", &json!({}));
    let advertised = capabilities["result"]["methods"]
        .as_array()
        .expect("methods");
    assert!(!advertised.iter().any(|name| name == "system.describe"));
    fixture.finish();
}

#[test]
fn the_api_cli_routes_each_method_to_the_window_or_the_worker_that_owns_it() {
    use crate::cli::api_cmd::{call_value, describe_value, list_rows};
    use paneflow_ipc_client::host_control::ControlTarget;

    let fixture = FixtureChild::spawn(Gates::Closed);
    let worker = paneflow_serve::open(fixture.home()).expect("a worker takes the fixture home");
    let endpoints = crate::cli::router::Endpoints {
        control: Some(ControlTarget::Controller(fixture.endpoint.clone())),
        host: None,
        worker: Some(paneflow_home::serve_endpoint_path(fixture.home())),
    };
    let catalog = paneflow_api::catalog().expect("catalog");

    let rows = list_rows(&catalog, &endpoints).expect("the window and the worker answer");
    let owner_of = |name: &str| {
        rows.iter()
            .find(|row| row["name"] == name)
            .map(|row| row["owner"].clone())
    };
    assert_eq!(owner_of("surface.list"), Some(json!("window")));
    assert_eq!(owner_of("worker.status"), Some(json!("worker")));
    assert_eq!(owner_of("usage.snapshot"), Some(json!("worker")));
    assert_eq!(
        owner_of("pane.write"),
        None,
        "no host runs for this fixture"
    );
    assert!(rows.iter().all(|row| row["summary"].is_string()));

    let described = describe_value(&catalog, &endpoints, "surface.read").expect("described");
    assert_eq!(described["callable"], true);
    let worker_method = describe_value(&catalog, &endpoints, "worker.status").expect("described");
    assert_eq!(worker_method["owner"], "worker");
    assert_eq!(
        worker_method["callable"], false,
        "the window does not serve it"
    );

    let listed = call_value(&catalog, &endpoints, "surface.list", &[], None).expect("listed");
    let surface = listed["surfaces"][0]["surface_id"]
        .as_u64()
        .expect("a surface");
    let read = call_value(
        &catalog,
        &endpoints,
        "surface.read",
        &[
            format!("surface_id={surface}"),
            "lines=1".into(),
            "fenced=false".into(),
        ],
        None,
    )
    .expect("read through the window");
    assert_eq!(read["lines"], 1);
    let status = call_value(&catalog, &endpoints, "worker.status", &[], None)
        .expect("worker.status goes to the worker");
    assert_eq!(status["pid"], std::process::id());

    let refused = call_value(
        &catalog,
        &endpoints,
        "surface.read",
        &["lines=many".into()],
        None,
    )
    .expect_err("the schema refuses the pair");
    assert!(refused.message.contains("'lines'"), "{}", refused.message);

    worker.stop();
    let unreachable = call_value(&catalog, &endpoints, "worker.status", &[], None)
        .expect_err("the worker is gone");
    assert!(
        unreachable.message.contains("worker.status"),
        "{}",
        unreachable.message
    );
    assert!(
        unreachable.message.contains("Paneflow worker"),
        "{}",
        unreachable.message
    );
    fixture.finish();
}

#[test]
fn the_case_file_pins_every_advertised_method() {
    let document = document();
    assert_eq!(document["protocol"], "paneflow-app");
    let cases = cases();
    let advertised = super::capabilities_value(false, false)["methods"]
        .as_array()
        .expect("methods")
        .iter()
        .map(|method| method.as_str().expect("a method name").to_string())
        .collect::<Vec<_>>();
    for method in &advertised {
        assert!(
            cases.iter().any(|case| &case.method == method
                && matches!(
                    case.expect,
                    Expectation::Result { .. } | Expectation::Stream { .. }
                )),
            "no success case pins {method}"
        );
    }
    for gated in ["surface.send_text", "surface.send_keystroke"] {
        assert!(
            cases.iter().any(|case| case.method == gated
                && case.gates == Gates::Closed
                && matches!(case.expect, Expectation::Error { code: -32601, .. })),
            "no closed-gate case pins {gated}"
        );
    }
    let mut ids = std::collections::HashSet::new();
    for case in &cases {
        assert!(
            ids.insert(case.id.as_str()),
            "case id {} is repeated",
            case.id
        );
    }
}

#[test]
fn a_case_with_no_runner_fails_naming_its_id() {
    let unknown_expectation = json!({
        "id": "invented_case",
        "method": "system.ping",
        "expect": {"telepathy": true},
    });
    let error = parse_case(&unknown_expectation).expect_err("no runner reads telepathy");
    assert!(error.contains("invented_case"), "{error}");
    let unknown_gates = json!({
        "id": "half_open",
        "method": "system.ping",
        "gates": "ajar",
        "expect": {"result": {}},
    });
    let error = parse_case(&unknown_gates).expect_err("no runner opens a gate ajar");
    assert!(error.contains("half_open"), "{error}");
}

#[test]
fn a_changed_result_shape_fails_naming_the_case() {
    let case = parse_case(&json!({
        "id": "ping_shape",
        "method": "system.ping",
        "expect": {"result": {"pong": "boolean"}},
    }))
    .expect("a valid case");
    assert!(check(&case, &json!({"result": {"pong": true}})).is_ok());
    let error = check(&case, &json!({"result": {"pong": true, "extra": 1}}))
        .expect_err("an extra field is a change");
    assert!(error.contains("ping_shape"), "{error}");
    let error = check(&case, &json!({"result": {"pong": "yes"}})).expect_err("a type change");
    assert!(error.contains("ping_shape"), "{error}");
}
