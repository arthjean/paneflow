#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use paneflow_api::methods::events::{EventIds, PublishedEvent};
use paneflow_serve::Controller;
use serde_json::{Value, json};

const LINE_WAIT: Duration = Duration::from_secs(30);

fn command(home: &Path, args: &[&str]) -> Command {
    let mut command = verb(home, "do");
    command.args(args);
    command
}

fn paneflow(home: &Path, args: &[&str]) -> Output {
    let (verb_name, rest) = args.split_first().expect("a verb");
    let mut command = verb(home, verb_name);
    command.args(rest);
    command.output().expect("paneflow runs")
}

fn verb(home: &Path, verb: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_paneflow"));
    command
        .arg(verb)
        .env(paneflow_home::HOME_ENV, home)
        .env_remove("PANEFLOW_SOCKET_PATH")
        .env_remove("PANEFLOW_ALLOW_SOCKET_OVERRIDE");
    for key in paneflow_host::env::PANE_CONTEXT_ENV {
        command.env_remove(key);
    }
    command
}

fn run(home: &Path, args: &[&str]) -> Output {
    command(home, args).output().expect("paneflow do runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

struct Script {
    child: Child,
    out: Receiver<String>,
    err: Receiver<String>,
}

fn lines(pipe: impl std::io::Read + Send + 'static) -> Receiver<String> {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                return;
            }
        }
    });
    receiver
}

impl Script {
    fn spawn(home: &Path, args: &[&str]) -> Self {
        let mut child = command(home, args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("paneflow do starts");
        let out = lines(child.stdout.take().unwrap());
        let err = lines(child.stderr.take().unwrap());
        Self { child, out, err }
    }

    fn next(receiver: &Receiver<String>, what: &str) -> String {
        match receiver.recv_timeout(LINE_WAIT) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => panic!("paneflow do printed no {what} line"),
            Err(RecvTimeoutError::Disconnected) => panic!("paneflow do closed {what}"),
        }
    }

    fn stdout_line(&self) -> String {
        Self::next(&self.out, "stdout")
    }

    fn stderr_until(&self, needle: &str) -> String {
        loop {
            let line = Self::next(&self.err, "stderr");
            if line.contains(needle) {
                return line;
            }
        }
    }

    fn status(mut self) -> Option<i32> {
        let deadline = std::time::Instant::now() + LINE_WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            if std::time::Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("paneflow do did not exit");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Script {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn layout_event(kind: &str, pane: u64) -> PublishedEvent {
    PublishedEvent {
        kind: kind.to_string(),
        ids: EventIds {
            workspace_id: Some(1),
            pane_id: Some(pane),
            ..EventIds::default()
        },
        data: Default::default(),
    }
}

fn publish(endpoint: &Path, events: &[PublishedEvent]) -> Vec<u64> {
    Controller::connect(endpoint)
        .expect("the worker answers")
        .publish_events(events)
        .expect("the worker takes layout events")
}

#[test]
fn a_chunk_prints_its_result_and_errors_and_stops_set_the_exit_status() {
    let home = tempfile::tempdir().unwrap();
    let output = run(
        home.path(),
        &[
            "-e",
            "return { n = paneflow.args.n * 2, on = paneflow.args.on }",
            "n=21",
            "on=true",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "{\"n\":42,\"on\":true}\n");

    let output = run(home.path(), &["-e", "local x = nil\nreturn x.field"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with("Error: (command line):2:"),
        "{}",
        stderr(&output)
    );
    assert!(output.stdout.is_empty());

    let output = run(
        home.path(),
        &["-e", "paneflow.stop(3) return 'unreachable'"],
    );
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());

    let output = run(home.path(), &["-e", "local nothing = 1"]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn a_file_and_stdin_run_in_the_cli_process_and_json_indents_the_result() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("helper.lua"),
        "return { twice = function(n) return n * 2 end }",
    )
    .unwrap();
    let script = dir.path().join("main.lua");
    std::fs::write(&script, "local helper = require('helper') return { pid_type = type(paneflow.ctx), value = helper.twice(paneflow.args.n) }").unwrap();
    let output = run(home.path(), &[script.to_str().unwrap(), "n=4", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value, json!({"pid_type": "table", "value": 8}));
    assert!(
        stdout(&output).contains("\n  "),
        "--json indents: {}",
        stdout(&output)
    );

    let mut child = command(home.path(), &["-", "--args", "{\"list\": [1, 2, 3]}"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"return #paneflow.args.list")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "3\n");
}

#[test]
fn a_missing_or_broken_file_exits_1_without_contacting_paneflow() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent.lua");
    let output = run(home.path(), &[missing.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains(missing.to_str().unwrap()),
        "{}",
        stderr(&output)
    );

    let broken = dir.path().join("broken.lua");
    std::fs::write(&broken, "local x = 1\nreturn 1 +").unwrap();
    let output = run(home.path(), &[broken.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with(&format!("Error: {}:2:", broken.display())),
        "{}",
        stderr(&output)
    );
    assert!(!paneflow_home::serve_dir_in(home.path()).exists());

    let output = run(home.path(), &["deploy"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("action.invoke is served by the Paneflow worker"),
        "{}",
        stderr(&output)
    );
}

const DEV_LAYOUT: &str = "paneflow.action{ name = 'dev_layout', title = 'Dev layout', args = { file = 'string' },\n\
      run = function(ctx, args) return { file = args.file, origin = ctx.origin, where = WHERE } end }\n\
    paneflow.action{ name = 'broken', title = 'Broken', run = function() error('it broke') end }";

fn wait_loaded(home: &Path) {
    let deadline = std::time::Instant::now() + LINE_WAIT;
    loop {
        let output = paneflow(home, &["actions", "--json"]);
        if output.status.success()
            && serde_json::from_str::<Value>(&stdout(&output)).is_ok_and(|listed| {
                listed["actions"]
                    .as_array()
                    .is_some_and(|actions| !actions.is_empty())
            })
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "init.lua never loaded"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn an_action_name_runs_the_worker_action_and_prints_its_result() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        paneflow_home::init_lua_path_in(home.path()),
        format!("WHERE = 'worker'\n{DEV_LAYOUT}"),
    )
    .unwrap();
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    wait_loaded(home.path());
    let output = run(home.path(), &["dev_layout", "file=app.log"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(
        value,
        json!({"file": "app.log", "origin": "cli", "where": "worker"})
    );

    let output = run(home.path(), &["broken"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("init.lua:4: it broke"),
        "{}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("not reachable"),
        "a worker that answered with an error is reachable: {}",
        stderr(&output)
    );

    let output = run(home.path(), &["nowhere"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("no action named 'nowhere'"),
        "{}",
        stderr(&output)
    );

    let listed = paneflow(home.path(), &["actions"]);
    assert!(listed.status.success());
    assert!(
        stdout(&listed).contains("dev_layout"),
        "{}",
        stdout(&listed)
    );
    assert!(
        stdout(&listed).contains("init.lua:2"),
        "{}",
        stdout(&listed)
    );

    let reloaded = paneflow(home.path(), &["config", "reload"]);
    assert!(reloaded.status.success(), "{}", stderr(&reloaded));
    assert!(
        stdout(&reloaded).contains("2 actions"),
        "{}",
        stdout(&reloaded)
    );
    worker.stop();
}

#[test]
fn action_runs_a_configuration_file_in_the_cli_process_without_the_worker() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("init.lua");
    std::fs::write(&config, format!("WHERE = 'cli'\n{DEV_LAYOUT}")).unwrap();
    let output = run(
        home.path(),
        &[
            config.to_str().unwrap(),
            "--action",
            "dev_layout",
            "file=app.log",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(
        value,
        json!({"file": "app.log", "origin": "cli", "where": "cli"})
    );
    assert!(!paneflow_home::serve_dir_in(home.path()).exists());

    let output = run(
        home.path(),
        &[
            config.to_str().unwrap(),
            "--action",
            "dev_layout",
            "bogus=1",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("no argument named \"bogus\"; the action takes file"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn config_check_prints_problems_with_their_line_and_exits_1_on_an_error() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("init.lua");
    std::fs::write(
        &config,
        "paneflow.action{ name = 'ok', title = 'Ok', run = function() end }\npaneflow.action{ name = 'Bad', title = 'x', run = function() end }",
    )
    .unwrap();
    let output = paneflow(home.path(), &["config", "check", config.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).contains("init.lua:2: error:"),
        "{}",
        stdout(&output)
    );
    std::fs::write(
        &config,
        "paneflow.action{ name = 'ok', title = 'Ok', run = function() end }",
    )
    .unwrap();
    let output = paneflow(home.path(), &["config", "check", config.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert!(!paneflow_home::serve_dir_in(home.path()).exists());
}

#[test]
fn with_no_instance_a_method_returns_nil_and_the_no_instance_message() {
    let home = tempfile::tempdir().unwrap();
    let output = run(
        home.path(),
        &[
            "-e",
            "local r, message, code = paneflow.surface.list() return { r = r == nil, message = message, code = code }",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["r"], true);
    let mut verb = Command::new(env!("CARGO_BIN_EXE_paneflow"));
    verb.args(["api", "call", "surface.list"])
        .env(paneflow_home::HOME_ENV, home.path())
        .env_remove("PANEFLOW_SOCKET_PATH");
    for key in paneflow_host::env::PANE_CONTEXT_ENV {
        verb.env_remove(key);
    }
    let existing = verb.output().unwrap();
    assert!(!existing.status.success());
    assert_eq!(
        format!("paneflow: {}\n", value["message"].as_str().unwrap()),
        stderr(&existing),
        "{value}"
    );
    assert!(value["code"].as_i64().is_some());
}

#[test]
fn a_worker_method_is_routed_to_the_worker_from_a_script() {
    let home = tempfile::tempdir().unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a core opens");
    let _core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint)
        .expect("the core serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let output = run(
        home.path(),
        &[
            "-e",
            "local status = assert(paneflow.worker.status()) local r, message = paneflow.surface.focus{ surface_id = 1 } return { pid = math.type(status.pid), focus = r == nil, message = message }",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["pid"], "integer", "{value}");
    assert_eq!(value["focus"], true, "{value}");
    worker.stop();
}

#[test]
fn handlers_hear_worker_events_and_a_worker_restart_calls_the_resync_handlers() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let script = Script::spawn(
        home.path(),
        &[
            "-e",
            "paneflow.on('events.resync', function() print('resync') io.stdout:flush() end) \
             paneflow.on('pane.created', function(ctx, ev) print('pane ' .. ctx.pane_id) io.stdout:flush() \
               if ctx.pane_id == 2 then paneflow.stop(0) end end) \
             paneflow.log('ready')",
        ],
    );
    script.stderr_until("ready");
    publish(&endpoint, &[layout_event("pane.created", 1)]);
    assert_eq!(script.stdout_line(), "pane 1");
    worker.stop();
    let worker = paneflow_serve::open(home.path()).expect("a new worker takes the home");
    assert_eq!(script.stdout_line(), "resync");
    script.stderr_until("restarted");
    publish(&endpoint, &[layout_event("pane.created", 2)]);
    assert_eq!(script.stdout_line(), "pane 2");
    assert_eq!(script.status(), Some(0));
    worker.stop();
}

#[test]
fn wait_returns_a_worker_event_and_for_ends_a_listening_script_with_status_zero() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let script = Script::spawn(
        home.path(),
        &[
            "-e",
            "paneflow.log('ready') local ev, ctx = paneflow.wait('pane.*', { pane_id = 9 }, 60) return { type = ev.type, pane = ctx.pane_id }",
        ],
    );
    script.stderr_until("ready");
    publish(
        &endpoint,
        &[
            layout_event("pane.focused", 8),
            layout_event("pane.closed", 9),
        ],
    );
    assert_eq!(
        script.stdout_line(),
        "{\"pane\":9,\"type\":\"pane.closed\"}"
    );
    assert_eq!(script.status(), Some(0));

    let output = run(
        home.path(),
        &[
            "--for",
            "300ms",
            "-e",
            "paneflow.on('pane.created', function() end) return 'listened'",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "\"listened\"\n");
    worker.stop();
}

#[cfg(unix)]
#[test]
fn ctrl_c_stops_a_loop_that_never_yields_with_the_interrupted_status() {
    let home = tempfile::tempdir().unwrap();
    let script = Script::spawn(
        home.path(),
        &["-e", "paneflow.log('ready') while true do end"],
    );
    script.stderr_until("ready");
    let interrupted = Command::new("kill")
        .args(["-INT", &script.child.id().to_string()])
        .status()
        .unwrap();
    assert!(interrupted.success());
    assert_eq!(script.status(), Some(130));
}

#[test]
fn api_stubs_and_docs_print_from_the_linked_registry_with_nothing_running() {
    let home = tempfile::tempdir().unwrap();
    let stubs = Command::new(env!("CARGO_BIN_EXE_paneflow"))
        .args(["api", "stubs"])
        .env(paneflow_home::HOME_ENV, home.path())
        .output()
        .unwrap();
    assert!(stubs.status.success(), "{}", stderr(&stubs));
    let checked_in = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../protocol/paneflow.d.lua"
    ))
    .unwrap();
    assert_eq!(stdout(&stubs), checked_in);
    let docs = Command::new(env!("CARGO_BIN_EXE_paneflow"))
        .args(["api", "docs"])
        .env(paneflow_home::HOME_ENV, home.path())
        .output()
        .unwrap();
    assert!(docs.status.success(), "{}", stderr(&docs));
    assert!(stdout(&docs).contains("## `surface.read`"));
}

#[test]
fn limit_flags_and_fail_fast_are_read_from_the_command_line() {
    let home = tempfile::tempdir().unwrap();
    let grow = "local t = {} for i = 1, 1e8 do t[i] = string.rep('x', 64) .. i end return #t";
    let output = run(home.path(), &["--memory-limit", "8", "-e", grow]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("memory"), "{}", stderr(&output));

    let spin =
        "local ok, message = pcall(function() while true do end end) return tostring(message)";
    let output = run(home.path(), &["--deadline", "0.2", "-e", spin]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("timeout"), "{}", stdout(&output));

    let failing = "paneflow.after(0, function() error('tick failed') end) paneflow.after(0.3, function() print('survived') end)";
    let output = run(home.path(), &["-e", failing]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("running with 2 active timers"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("tick failed"),
        "{}",
        stderr(&output)
    );
    assert_eq!(stdout(&output).lines().collect::<Vec<_>>(), ["survived"]);

    let output = run(home.path(), &["--fail-fast", "-e", failing]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("tick failed"),
        "{}",
        stderr(&output)
    );
    assert!(output.stdout.is_empty());
}
