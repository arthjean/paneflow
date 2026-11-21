#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::time::{Duration, Instant};

use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

const PATIENCE: Duration = Duration::from_secs(30);

fn call(home: &Path, method: &str, params: Value) -> Result<Value, String> {
    let endpoint = paneflow_home::serve_endpoint_path(home);
    HostControl::connect(&endpoint, "worker-config-test")?.request_with_deadline(
        method,
        params,
        Duration::from_secs(60),
    )
}

fn status_until(home: &Path, accept: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let status = call(home, "config.status", json!({})).expect("config.status");
        if accept(&status) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "config.status never settled: {status}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn loaded(home: &Path) -> Value {
    status_until(home, |status| {
        status["loaded_at_ms"].is_u64() || !status["problems"].as_array().unwrap().is_empty()
    })
}

fn reloaded_events(home: &Path) -> Vec<Value> {
    let endpoint = paneflow_home::serve_endpoint_path(home);
    let mut follower = HostControl::connect(&endpoint, "worker-config-events").unwrap();
    let header = follower
        .request(
            "events.follow",
            json!({"since": 0, "types": ["config.reloaded"]}),
        )
        .expect("events.follow");
    assert_eq!(header["type"], "following");
    let mut events = Vec::new();
    while let Ok(Some(line)) = follower.read_stream_line(Duration::from_millis(300)) {
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["type"] == "config.reloaded" {
            events.push(frame);
        }
    }
    events
}

fn write_init(home: &Path, source: &str) {
    std::fs::write(paneflow_home::init_lua_path_in(home), source).unwrap();
}

#[test]
fn with_no_init_lua_nothing_loads_and_no_event_is_appended() {
    let home = tempfile::tempdir().unwrap();
    let worker = paneflow_serve::open(home.path()).unwrap();
    let status = call(home.path(), "config.status", json!({})).unwrap();
    assert_eq!(status["found"], false);
    assert_eq!(status["ok"], true);
    assert_eq!(status["actions"], 0);
    assert_eq!(status["problems"], json!([]));
    assert_eq!(
        status["path"],
        home.path().join("init.lua").display().to_string()
    );
    assert_eq!(
        call(home.path(), "action.list", json!({})).unwrap(),
        json!({"actions": []})
    );
    assert_eq!(worker.worker().events.last_seq(), 0);
    worker.stop();
}

#[test]
fn init_lua_actions_and_bindings_are_served_and_run_with_their_arguments() {
    let home = tempfile::tempdir().unwrap();
    write_init(
        home.path(),
        "paneflow.action{ name = 'dev_layout', title = 'Dev layout', args = { file = 'string' },\n\
           run = function(ctx, args) return { file = args.file, origin = ctx.origin } end }\n\
         paneflow.action{ name = 'broken', title = 'Broken', run = function() error('it broke') end }\n\
         paneflow.bind('ctrl-alt-d', 'dev_layout', { file = 'app.log' })\n\
         paneflow.unbind('ctrl-shift-w')",
    );
    let worker = paneflow_serve::open(home.path()).unwrap();
    let status = loaded(home.path());
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(status["found"], true);
    assert_eq!(status["actions"], 2);
    assert_eq!(status["bindings"], 1);
    let listed = call(home.path(), "action.list", json!({})).unwrap();
    assert_eq!(listed["actions"][0]["name"], "dev_layout");
    assert_eq!(listed["actions"][0]["source"], "init.lua:1");
    let keymap = call(home.path(), "keymap.list", json!({})).unwrap();
    assert_eq!(keymap["bindings"][0]["key"], "ctrl-alt-d");
    assert_eq!(keymap["bindings"][0]["target"], "lua");
    assert_eq!(keymap["unbound"][0]["key"], "ctrl-shift-w");
    let invoked = call(
        home.path(),
        "action.invoke",
        json!({"name": "dev_layout", "args": {"file": "app.log"}, "ctx": {"origin": "cli"}}),
    )
    .unwrap();
    assert_eq!(
        invoked,
        json!({"result": {"file": "app.log", "origin": "cli"}})
    );
    let refused = call(
        home.path(),
        "action.invoke",
        json!({"name": "dev_layout", "args": {"bogus": 1}}),
    )
    .unwrap_err();
    assert!(
        refused.contains("no argument named \"bogus\"; the action takes file"),
        "{refused}"
    );
    let failed = call(home.path(), "action.invoke", json!({"name": "broken"})).unwrap_err();
    assert!(failed.contains("init.lua:3: it broke"), "{failed}");
    let unknown = call(home.path(), "action.invoke", json!({"name": "nope"})).unwrap_err();
    assert!(unknown.contains("no action named 'nope'"), "{unknown}");
    worker.stop();
}

#[test]
fn an_init_lua_that_raises_leaves_the_worker_serving_and_reports_the_line() {
    let home = tempfile::tempdir().unwrap();
    write_init(home.path(), "local x = 1\nerror('bad config')\n");
    let worker = paneflow_serve::open(home.path()).unwrap();
    let status = status_until(home.path(), |status| status["ok"] == false);
    assert_eq!(status["problems"][0]["file"], "init.lua");
    assert_eq!(status["problems"][0]["line"], 2);
    assert_eq!(status["problems"][0]["severity"], "error");
    assert_eq!(status["problems"][0]["message"], "bad config");
    assert!(call(home.path(), "worker.status", json!({})).is_ok());
    assert!(call(home.path(), "usage.snapshot", json!({})).is_ok());
    worker.stop();
}

#[test]
fn a_syntax_error_is_reported_with_its_file_and_line() {
    let home = tempfile::tempdir().unwrap();
    write_init(home.path(), "paneflow.action{\n name = 'x' title = 'y' }\n");
    let worker = paneflow_serve::open(home.path()).unwrap();
    let status = status_until(home.path(), |status| status["ok"] == false);
    assert_eq!(status["problems"][0]["file"], "init.lua");
    assert_eq!(status["problems"][0]["line"], 2);
    worker.stop();
}

#[test]
fn a_save_reloads_a_broken_edit_keeps_the_previous_configuration_and_a_delete_clears_it() {
    let home = tempfile::tempdir().unwrap();
    write_init(
        home.path(),
        "paneflow.action{ name = 'first', title = 'First', run = function() return 1 end }",
    );
    let worker = paneflow_serve::open(home.path()).unwrap();
    assert_eq!(loaded(home.path())["actions"], 1);
    write_init(
        home.path(),
        "paneflow.action{ name = 'first', title = 'First', run = function() return 1 end }\n\
         paneflow.action{ name = 'second', title = 'Second', run = function() return 2 end }",
    );
    status_until(home.path(), |status| status["actions"] == 2);
    assert_eq!(
        call(home.path(), "action.invoke", json!({"name": "second"})).unwrap(),
        json!({"result": 2})
    );
    write_init(home.path(), "paneflow.action{ name = 'third' title = 'x' }");
    let failed = status_until(home.path(), |status| status["ok"] == false);
    assert_eq!(failed["problems"][0]["line"], 1);
    assert_eq!(failed["actions"], 2, "{failed}");
    assert_eq!(
        call(home.path(), "action.invoke", json!({"name": "second"})).unwrap(),
        json!({"result": 2}),
        "the previous configuration stays active"
    );
    std::fs::remove_file(paneflow_home::init_lua_path_in(home.path())).unwrap();
    let cleared = status_until(home.path(), |status| status["found"] == false);
    assert_eq!(cleared["ok"], true);
    assert_eq!(cleared["actions"], 0);
    let events = reloaded_events(home.path());
    let oks: Vec<&Value> = events.iter().map(|event| &event["data"]["ok"]).collect();
    assert_eq!(oks.first(), Some(&&json!(true)));
    assert!(oks.contains(&&json!(false)), "{events:?}");
    assert_eq!(oks.last(), Some(&&json!(true)));
    assert_eq!(events.last().unwrap()["data"]["found"], false);
    worker.stop();
}

#[test]
fn config_reload_forces_a_load_and_a_required_file_is_watched() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("lua")).unwrap();
    std::fs::write(
        home.path().join("lua").join("extra.lua"),
        "paneflow.action{ name = 'extra', title = 'Extra', run = function() return 'one' end }",
    )
    .unwrap();
    write_init(home.path(), "require('extra')");
    let worker = paneflow_serve::open(home.path()).unwrap();
    assert_eq!(loaded(home.path())["actions"], 1);
    let forced = call(home.path(), "config.reload", json!({})).unwrap();
    assert_eq!(forced["ok"], true);
    assert_eq!(forced["actions"], 1);
    std::fs::write(
        home.path().join("lua").join("extra.lua"),
        "paneflow.action{ name = 'extra', title = 'Extra', run = function() return 'two' end }",
    )
    .unwrap();
    let deadline = Instant::now() + PATIENCE;
    loop {
        let answer = call(home.path(), "action.invoke", json!({"name": "extra"})).unwrap();
        if answer == json!({"result": "two"}) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the required file was never reloaded"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    worker.stop();
}

#[test]
fn reading_init_lua_never_triggers_a_reload_of_its_own() {
    let home = tempfile::tempdir().unwrap();
    write_init(
        home.path(),
        "paneflow.action{ name = 'first', title = 'First', run = function() end }",
    );
    let worker = paneflow_serve::open(home.path()).unwrap();
    assert_eq!(loaded(home.path())["actions"], 1);
    write_init(
        home.path(),
        "paneflow.action{ name = 'first', title = 'First', run = function() end }\n\
         paneflow.action{ name = 'second', title = 'Second', run = function() end }",
    );
    status_until(home.path(), |status| status["actions"] == 2);
    std::thread::sleep(paneflow_serve::config_runtime::RELOAD_SETTLE * 5);
    let _ = std::fs::read(paneflow_home::init_lua_path_in(home.path())).unwrap();
    std::thread::sleep(paneflow_serve::config_runtime::RELOAD_SETTLE * 5);
    let events = reloaded_events(home.path());
    assert_eq!(events.len(), 2, "one load and one save: {events:?}");
    worker.stop();
}

#[test]
fn a_file_first_required_by_a_load_that_failed_reloads_when_it_is_fixed() {
    let home = tempfile::tempdir().unwrap();
    let extra = home.path().join("lua").join("extra.lua");
    std::fs::create_dir_all(home.path().join("lua")).unwrap();
    write_init(
        home.path(),
        "paneflow.action{ name = 'first', title = 'First', run = function() end }",
    );
    let worker = paneflow_serve::open(home.path()).unwrap();
    assert_eq!(loaded(home.path())["actions"], 1);
    std::fs::write(&extra, "local broken = = 1").unwrap();
    write_init(home.path(), "require('extra')");
    let failed = status_until(home.path(), |status| status["ok"] == false);
    assert_eq!(failed["actions"], 1, "{failed}");
    std::fs::write(
        &extra,
        "paneflow.action{ name = 'a', title = 'A', run = function() end }\n\
         paneflow.action{ name = 'b', title = 'B', run = function() end }",
    )
    .unwrap();
    let fixed = status_until(home.path(), |status| status["actions"] == 2);
    assert_eq!(fixed["ok"], true);
    worker.stop();
}

#[test]
fn an_action_invoked_from_a_timer_or_a_handler_gets_that_origin_and_the_nesting_limit() {
    let home = tempfile::tempdir().unwrap();
    write_init(
        home.path(),
        "local seen = {}\n\
         paneflow.action{ name = 'who', title = 'Who', run = function(ctx) return ctx.origin end }\n\
         paneflow.action{ name = 'recurse', title = 'Recurse', run = function()\n\
           local value, why = paneflow.invoke('recurse')\n\
           if value == nil then return why end\n\
           return value end }\n\
         paneflow.after(0.01, function()\n\
           seen.timer = paneflow.invoke('who')\n\
           seen.deep = paneflow.invoke('recurse') end)\n\
         paneflow.on('pane.created', function() seen.event = paneflow.invoke('who') end)\n\
         paneflow.action{ name = 'seen', title = 'Seen', run = function() return seen end }",
    );
    let worker = paneflow_serve::open(home.path()).unwrap();
    loaded(home.path());
    let deadline = Instant::now() + PATIENCE;
    let seen = loop {
        call(
            home.path(),
            "events.publish",
            json!({"events": [{"type": "pane.created", "workspace_id": 1, "pane_id": 4}]}),
        )
        .unwrap();
        let answer = call(home.path(), "action.invoke", json!({"name": "seen"})).unwrap();
        let seen = answer["result"].clone();
        if seen["event"].is_string() && seen["deep"].is_string() {
            break seen;
        }
        assert!(Instant::now() < deadline, "never seen: {answer}");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(seen["timer"], "timer");
    assert_eq!(seen["event"], "event");
    assert!(seen["deep"].as_str().unwrap().contains("16"), "{seen}");
    worker.stop();
}

#[test]
fn handlers_run_in_the_worker_and_failing_timers_keep_running_and_are_counted() {
    let home = tempfile::tempdir().unwrap();
    write_init(
        home.path(),
        "local seen = 0\n\
         paneflow.on('pane.created', function(ctx, ev) seen = seen + 1 end)\n\
         paneflow.every(0.02, function() error('tick failed') end)\n\
         paneflow.action{ name = 'seen', title = 'Seen', run = function() return seen end }",
    );
    let worker = paneflow_serve::open(home.path()).unwrap();
    let status = loaded(home.path());
    assert_eq!(status["handlers"], 1);
    assert_eq!(status["timers"], 1);
    call(
        home.path(),
        "events.publish",
        json!({"events": [{"type": "pane.created", "workspace_id": 1, "pane_id": 4}]}),
    )
    .unwrap();
    let deadline = Instant::now() + PATIENCE;
    loop {
        let answer = call(home.path(), "action.invoke", json!({"name": "seen"})).unwrap();
        if answer == json!({"result": 1}) {
            break;
        }
        assert!(Instant::now() < deadline, "the handler never ran: {answer}");
        std::thread::sleep(Duration::from_millis(20));
    }
    let counted = status_until(home.path(), |status| {
        status["timer_errors"].as_u64().unwrap_or(0) >= 3
    });
    assert_eq!(counted["timers"], 1, "the failing timer keeps running");
    worker.stop();
}

#[test]
fn a_reload_cancels_the_previous_timers_and_handlers() {
    let home = tempfile::tempdir().unwrap();
    write_init(
        home.path(),
        "paneflow.every(0.02, function() end)\npaneflow.on('pane.created', function() end)",
    );
    let worker = paneflow_serve::open(home.path()).unwrap();
    assert_eq!(loaded(home.path())["timers"], 1);
    write_init(
        home.path(),
        "paneflow.action{ name = 'a', title = 'A', run = function() end }",
    );
    let status = status_until(home.path(), |status| status["actions"] == 1);
    assert_eq!(status["timers"], 0);
    assert_eq!(status["handlers"], 0);
    worker.stop();
}

#[test]
fn the_worker_stops_while_init_lua_runs_forever_at_load_or_in_an_action() {
    let home = tempfile::tempdir().unwrap();
    write_init(
        home.path(),
        "paneflow.action{ name = 'spin', title = 'Spin', run = function() while true do end end }",
    );
    let worker = paneflow_serve::open(home.path()).unwrap();
    loaded(home.path());
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    let caller = std::thread::spawn(move || {
        HostControl::connect(&endpoint, "spin")
            .unwrap()
            .request_with_deadline(
                "action.invoke",
                json!({"name": "spin"}),
                Duration::from_secs(60),
            )
    });
    std::thread::sleep(Duration::from_millis(100));
    worker.stop();
    assert!(caller.join().unwrap().is_err());

    let home = tempfile::tempdir().unwrap();
    write_init(home.path(), "while true do end");
    let worker = paneflow_serve::open(home.path()).unwrap();
    assert!(call(home.path(), "worker.status", json!({})).is_ok());
    worker.stop();
}

#[test]
fn the_lua_state_is_capped_at_256_mib() {
    let home = tempfile::tempdir().unwrap();
    write_init(
        home.path(),
        "local held = {}\nfor i = 1, 1e9 do held[i] = string.rep('x', 4096) .. i end",
    );
    let worker = paneflow_serve::open(home.path()).unwrap();
    let status = status_until(home.path(), |status| status["ok"] == false);
    let message = status["problems"][0]["message"].as_str().unwrap();
    assert!(message.contains("memory"), "{message}");
    worker.stop();
}

fn follow_prompts(home: &Path) -> HostControl {
    let endpoint = paneflow_home::serve_endpoint_path(home);
    let mut follower = HostControl::connect(&endpoint, "worker-config-prompts").unwrap();
    let header = follower
        .request("prompt.follow", json!({"kinds": ["input", "pick"]}))
        .expect("prompt.follow");
    assert_eq!(header["type"], "following");
    follower
}

fn next_prompt(follower: &mut HostControl) -> Value {
    loop {
        let line = follower
            .read_stream_line(PATIENCE)
            .expect("a prompt frame")
            .expect("the stream stays open");
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["type"] == "requested" {
            return frame;
        }
    }
}

#[test]
fn an_action_asks_through_the_worker_and_types_with_the_answer_of_the_first_client() {
    let home = tempfile::tempdir().unwrap();
    write_init(
        home.path(),
        "paneflow.action{ name = 'new_branch', title = 'New branch', run = function()\n\
           local name, reason = paneflow.ui.input{ title = 'New branch', value = 'feature/' }\n\
           if not name then return reason end\n\
           return 'git switch -c ' .. name end }",
    );
    let _worker = paneflow_serve::open(home.path()).unwrap();
    loaded(home.path());
    let invoke = json!({"name": "new_branch", "ctx": {"origin": "key", "surface_id": 4, "client_id": "window"}});
    assert_eq!(
        call(home.path(), "action.invoke", invoke.clone()).unwrap(),
        json!({"result": "no_client"}),
        "with no window attached nobody is asked"
    );
    let mut follower = follow_prompts(home.path());
    let asking = {
        let home = home.path().to_path_buf();
        std::thread::spawn(move || call(&home, "action.invoke", invoke).unwrap())
    };
    let shown = next_prompt(&mut follower);
    assert_eq!(shown["spec"]["title"], "New branch");
    assert_eq!(shown["spec"]["value"], "feature/");
    assert_eq!(shown["origin"]["kind"], "action");
    assert_eq!(shown["origin"]["title"], "New branch");
    assert_eq!(shown["origin"]["surface_id"], 4);
    call(
        home.path(),
        "prompt.reply",
        json!({"id": shown["id"], "answer": "fix/login"}),
    )
    .unwrap();
    assert_eq!(
        asking.join().unwrap(),
        json!({"result": "git switch -c fix/login"})
    );
}

#[cfg(unix)]
struct FakeWindow {
    socket: std::path::PathBuf,
    requests: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(unix)]
impl FakeWindow {
    fn listen(home: &Path) -> Self {
        use std::io::{BufRead, BufReader, Write};
        use std::sync::atomic::Ordering;
        let socket = paneflow_home::ipc_endpoint_path(home);
        let _ = std::fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (seen, halted) = (requests.clone(), stop.clone());
        std::thread::spawn(move || {
            while !halted.load(Ordering::Acquire) {
                let Ok((stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                let reply = json!({"jsonrpc": "2.0", "id": request["id"], "result": {"bytes": 2}});
                seen.lock().unwrap().push(request);
                let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
            }
        });
        Self {
            socket,
            requests,
            stop,
        }
    }

    fn take(&self) -> Vec<Value> {
        std::mem::take(&mut *self.requests.lock().unwrap())
    }
}

#[cfg(unix)]
impl Drop for FakeWindow {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        let _ = std::fs::remove_file(&self.socket);
    }
}

#[cfg(unix)]
#[test]
fn a_window_grant_rides_on_the_envelope_of_its_runs_writes_and_nowhere_else() {
    let home = tempfile::tempdir().unwrap();
    write_init(
        home.path(),
        "paneflow.action{ name = 'type_it', title = 'Type it', run = function()\n\
           local sent, message = paneflow.surface.send_text{ text = 'ls', surface_id = 4 }\n\
           return { sent = sent ~= nil, message = message } end }",
    );
    let window = FakeWindow::listen(home.path());
    let _worker = paneflow_serve::open(home.path()).unwrap();
    loaded(home.path());
    let writes = |window: &FakeWindow| {
        window
            .take()
            .into_iter()
            .filter(|request| request["method"] == "surface.send_text")
            .collect::<Vec<_>>()
    };

    let granted = json!({"name": "type_it", "ctx": {"origin": "key", "client_id": "window", "grant": "g-123"}});
    assert_eq!(
        call(home.path(), "action.invoke", granted).unwrap(),
        json!({"result": {"sent": true}})
    );
    let sent = writes(&window);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["grant"], "g-123");
    assert!(
        sent[0]["params"].get("grant").is_none(),
        "the grant is never a method parameter: {}",
        sent[0]
    );

    let claimed = json!({"name": "type_it", "ctx": {"origin": "key", "client_id": "window"}});
    assert_eq!(
        call(home.path(), "action.invoke", claimed).unwrap(),
        json!({"result": {"sent": true}})
    );
    let sent = writes(&window);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].get("grant").is_none(),
        "a run without a grant sends none: {}",
        sent[0]
    );
}
