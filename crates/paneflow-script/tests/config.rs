#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use paneflow_api::methods::config::{BindingTarget, InvokeContext, Severity};
use paneflow_script::{CallError, FakeClock, LoadReport, Outcome, Runtime};
use serde_json::{Value, json};
use support::{CaptureSink, FakeBackend, Harness, harness_with};

fn config_harness(backend: FakeBackend) -> (Harness, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_path_buf();
    let harness = harness_with(backend, move |options| {
        options.config_root = Some(path.clone());
        options.lua_dir = Some(path.join("lua"));
        options.serving = true;
    });
    (harness, root)
}

fn load(harness: &Harness, source: &str) -> LoadReport {
    let chunk = harness
        .runtime
        .compile(source.as_bytes(), "@init.lua")
        .expect("compiles");
    harness.runtime.attach().expect("attach");
    harness.runtime.load(chunk)
}

fn cli(surface: Option<u64>) -> InvokeContext {
    InvokeContext {
        origin: Some("cli".to_string()),
        surface_id: surface,
        ..InvokeContext::default()
    }
}

#[test]
fn an_action_is_registered_with_its_defaults_and_its_source() {
    let (harness, _root) = config_harness(FakeBackend::new());
    let report = load(
        &harness,
        "paneflow.action{ name = 'dev_layout', title = 'Dev layout', run = function() end }\n\
         paneflow.action{ name = 'tail', title = 'Tail', description = 'Follow a log', category = 'Logs',\n\
           keywords = {'log', 'follow'}, agents = true, args = { file = 'string', ratio = 'number?' },\n\
           run = function() end }",
    );
    assert!(report.ok, "{report:?}");
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    let first = &report.actions[0];
    assert_eq!(first.name, "dev_layout");
    assert_eq!(first.category, "Actions");
    assert!(!first.agents);
    assert!(first.keywords.is_empty());
    assert_eq!(first.source, "init.lua:1");
    let tail = &report.actions[1];
    assert_eq!(tail.category, "Logs");
    assert_eq!(tail.description.as_deref(), Some("Follow a log"));
    assert_eq!(tail.keywords, ["log", "follow"]);
    assert!(tail.agents);
    assert_eq!(tail.source, "init.lua:2");
    assert_eq!(
        tail.args,
        json!({
            "type": "object",
            "properties": {"file": {"type": "string"}, "ratio": {"type": "number"}},
            "required": ["file"],
            "additionalProperties": false
        })
    );
}

#[test]
fn a_bad_declaration_is_reported_with_its_line_and_the_rest_still_loads() {
    let (harness, _root) = config_harness(FakeBackend::new());
    let report = load(
        &harness,
        "paneflow.action{ name = 'Dev.Layout', title = 'x', run = function() end }\n\
         paneflow.action{ name = 'ok', title = 'Ok', run = function() end }\n\
         paneflow.action{ name = 'no_run', title = 'No run' }",
    );
    assert!(report.ok);
    assert_eq!(report.actions.len(), 1);
    assert_eq!(report.actions[0].name, "ok");
    assert_eq!(report.problems.len(), 2, "{:?}", report.problems);
    assert_eq!(report.problems[0].severity, Severity::Error);
    assert_eq!(report.problems[0].file.as_deref(), Some("init.lua"));
    assert_eq!(report.problems[0].line, Some(1));
    assert!(report.problems[0].message.contains("Dev.Layout"));
    assert_eq!(report.problems[1].line, Some(3));
    assert!(report.problems[1].message.contains("run is required"));
}

#[test]
fn every_argument_type_and_a_schema_table_are_accepted() {
    let (harness, _root) = config_harness(FakeBackend::new());
    let report = load(
        &harness,
        "paneflow.action{ name = 'all', title = 'All', run = function() end, args = {\n\
           s = 'string', n = 'number', i = 'integer', b = 'boolean', o = 'object', a = 'array', x = 'any?',\n\
           mode = { type = 'string', enum = {'fast', 'slow'} }, ['level?'] = { type = 'integer', minimum = 1 } } }\n\
         paneflow.action{ name = 'bad', title = 'Bad', run = function() end, args = { s = 'text' } }",
    );
    let schema = &report.actions[0].args;
    assert_eq!(schema["properties"]["i"], json!({"type": "integer"}));
    assert_eq!(schema["properties"]["x"], json!({}));
    assert_eq!(
        schema["properties"]["mode"],
        json!({"type": "string", "enum": ["fast", "slow"]})
    );
    assert_eq!(
        schema["required"],
        json!(["a", "b", "i", "mode", "n", "o", "s"])
    );
    assert_eq!(report.actions.len(), 1);
    assert!(report.problems[0].message.contains("'text' is not a type"));
}

#[test]
fn a_name_declared_twice_warns_once_and_the_later_declaration_wins() {
    let (harness, _root) = config_harness(FakeBackend::new());
    let report = load(
        &harness,
        "paneflow.action{ name = 'a', title = 'First', run = function() end }\n\
         paneflow.action{ name = 'a', title = 'Second', run = function() end }",
    );
    assert_eq!(report.actions.len(), 1);
    assert_eq!(report.actions[0].title, "Second");
    assert_eq!(report.problems.len(), 1);
    assert_eq!(report.problems[0].severity, Severity::Warning);
    assert_eq!(report.problems[0].line, Some(2));
}

#[test]
fn declaring_or_binding_after_load_raises() {
    let (harness, _root) = config_harness(FakeBackend::new());
    load(
        &harness,
        "paneflow.action{ name = 'late', title = 'Late', run = function()\n\
           local ok, message = pcall(paneflow.action, { name = 'x', title = 'x', run = function() end })\n\
           local bound, refusal = pcall(paneflow.bind, 'ctrl-x', 'late')\n\
           return { ok = ok, message = message, bound = bound, refusal = refusal } end }",
    );
    let outcome = harness
        .runtime
        .run_action("late".into(), json!({}), cli(None));
    let Outcome::Returned(Some(value)) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(value["ok"], false);
    assert!(
        value["message"]
            .as_str()
            .unwrap()
            .contains("only be called while init.lua loads"),
        "{value}"
    );
    assert_eq!(value["bound"], false);
    assert!(value["refusal"].as_str().unwrap().contains("paneflow.bind"));
}

#[test]
fn run_returns_its_value_and_nothing_nil_or_true_yield_no_result() {
    for (body, expected) in [
        ("return { id = 4 }", Some(json!({"id": 4}))),
        ("return", None),
        ("return nil", None),
        ("return true", None),
        ("return 'done'", Some(json!("done"))),
    ] {
        let (harness, _root) = config_harness(FakeBackend::new());
        load(
            &harness,
            &format!("paneflow.action{{ name = 'a', title = 'A', run = function() {body} end }}"),
        );
        let outcome = harness.runtime.run_action("a".into(), json!({}), cli(None));
        assert_eq!(outcome, Outcome::Returned(expected), "{body}");
    }
}

#[test]
fn arguments_outside_the_schema_are_refused_before_run_starts() {
    let (harness, _root) = config_harness(FakeBackend::new());
    let recorded = harness.recorder();
    load(
        &harness,
        "paneflow.action{ name = 'dev_layout', title = 'Dev', args = { file = 'string', ratio = 'number?' },\n\
           run = function() record('ran') end }",
    );
    let outcome = harness.runtime.run_action(
        "dev_layout".into(),
        json!({"file": "a", "bogus": 1}),
        cli(None),
    );
    assert_eq!(
        outcome,
        Outcome::Failed("no argument named \"bogus\"; the action takes file, ratio?".to_string())
    );
    assert!(recorded.borrow().is_empty());
}

#[test]
fn a_missing_or_mistyped_argument_names_what_is_wrong() {
    let (harness, _root) = config_harness(FakeBackend::new());
    load(
        &harness,
        "paneflow.action{ name = 'a', title = 'A', args = { file = 'string' }, run = function() end }",
    );
    let Outcome::Failed(missing) = harness.runtime.run_action("a".into(), json!({}), cli(None))
    else {
        panic!("refused");
    };
    assert_eq!(missing, "missing argument \"file\"; the action takes file");
    let (harness, _root) = config_harness(FakeBackend::new());
    load(
        &harness,
        "paneflow.action{ name = 'a', title = 'A', args = { file = 'string' }, run = function() end }",
    );
    let Outcome::Failed(mistyped) =
        harness
            .runtime
            .run_action("a".into(), json!({"file": 3}), cli(None))
    else {
        panic!("refused");
    };
    assert!(mistyped.contains("file"), "{mistyped}");
}

#[test]
fn an_error_raised_by_run_carries_its_file_and_line() {
    let (harness, _root) = config_harness(FakeBackend::new());
    load(
        &harness,
        "paneflow.action{ name = 'boom', title = 'Boom', run = function()\n\
           error('the build is broken') end }",
    );
    let outcome = harness
        .runtime
        .run_action("boom".into(), json!({}), cli(None));
    assert_eq!(
        outcome,
        Outcome::Failed("init.lua:2: the build is broken".to_string())
    );
}

#[test]
fn an_action_that_waits_past_thirty_seconds_is_stopped_with_a_timeout() {
    let (harness, _root) = config_harness(FakeBackend::new());
    load(
        &harness,
        "paneflow.action{ name = 'slow', title = 'Slow', run = function() paneflow.sleep(60) return 'late' end }",
    );
    let Outcome::Failed(message) = harness
        .runtime
        .run_action("slow".into(), json!({}), cli(None))
    else {
        panic!("stopped");
    };
    assert!(message.starts_with("timeout:"), "{message}");
    assert!(message.contains("30 s"), "{message}");
}

#[test]
fn a_busy_loop_in_an_action_is_stopped_at_the_limit() {
    let backend = Arc::new(FakeBackend::new());
    let sink = Arc::new(CaptureSink::default());
    let root = tempfile::tempdir().unwrap();
    let mut options = paneflow_script::Options::new(sink);
    options.clock = Arc::new(FakeClock::ticking(Duration::from_millis(5)));
    options.config_root = Some(root.path().to_path_buf());
    let runtime = Runtime::new(backend, options).unwrap();
    let chunk = runtime
        .compile(
            b"paneflow.action{ name = 'spin', title = 'Spin', run = function() while true do end end }",
            "@init.lua",
        )
        .unwrap();
    runtime.attach().unwrap();
    assert!(runtime.load(chunk).ok);
    let Outcome::Failed(message) = runtime.run_action("spin".into(), json!({}), cli(None)) else {
        panic!("stopped");
    };
    assert!(message.contains("timeout"), "{message}");
}

#[test]
fn ctx_carries_the_origin_and_ids_and_api_calls_default_to_them() {
    let (harness, _root) = config_harness(FakeBackend::new());
    load(
        &harness,
        "paneflow.action{ name = 'look', title = 'Look', run = function(ctx)\n\
           paneflow.surface.read{}\n\
           return { origin = ctx.origin, surface = ctx.surface_id, tab = ctx.tab_id, client = ctx.client_id } end }",
    );
    let context = InvokeContext {
        origin: Some("palette".to_string()),
        surface_id: Some(9),
        tab_id: Some(3),
        workspace_id: Some(2),
        client_id: Some("window".to_string()),
        ..InvokeContext::default()
    };
    let outcome = harness
        .runtime
        .run_action("look".into(), json!({}), context);
    assert_eq!(
        outcome,
        Outcome::Returned(Some(
            json!({"origin": "palette", "surface": 9, "tab": 3, "client": "window"})
        ))
    );
    let calls = harness.backend.calls();
    let read = calls
        .iter()
        .find(|(method, _)| method == "surface.read")
        .unwrap();
    assert_eq!(read.1["surface_id"], 9);
    assert_eq!(read.1["workspace_id"], 2);
}

#[test]
fn invoke_runs_another_action_with_the_same_ctx_and_stops_past_sixteen_deep() {
    let (harness, _root) = config_harness(FakeBackend::new());
    load(
        &harness,
        "paneflow.action{ name = 'inner', title = 'Inner', args = { n = 'integer' }, run = function(ctx, args)\n\
           return { origin = ctx.origin, surface = ctx.surface_id, n = args.n } end }\n\
         paneflow.action{ name = 'outer', title = 'Outer', run = function()\n\
           local value = paneflow.invoke('inner', { n = 2 })\n\
           local missing, why = paneflow.invoke('nowhere')\n\
           return { value = value, missing = missing == nil, why = why } end }\n\
         paneflow.action{ name = 'deep', title = 'Deep', run = function(ctx, args)\n\
           local depth = (args.depth or 1)\n\
           local value, message = paneflow.invoke('deep', { depth = depth + 1 })\n\
           if value == nil then return { reached = depth, message = message } end\n\
           return value end, args = { ['depth?'] = 'integer' } }",
    );
    let outcome = harness
        .runtime
        .run_action("outer".into(), json!({}), cli(Some(5)));
    let Outcome::Returned(Some(value)) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        value["value"],
        json!({"origin": "cli", "surface": 5, "n": 2})
    );
    assert_eq!(value["missing"], true);
    assert!(value["why"].as_str().unwrap().contains("no action named"));

    let (harness, _root) = config_harness(FakeBackend::new());
    load(
        &harness,
        "paneflow.action{ name = 'deep', title = 'Deep', args = { ['depth?'] = 'integer' }, run = function(ctx, args)\n\
           local depth = (args.depth or 1)\n\
           local value, message = paneflow.invoke('deep', { depth = depth + 1 })\n\
           if value == nil then return { reached = depth, message = message } end\n\
           return value end }",
    );
    let Outcome::Returned(Some(value)) =
        harness
            .runtime
            .run_action("deep".into(), json!({}), cli(None))
    else {
        panic!("deep");
    };
    assert_eq!(value["reached"], 16);
    assert!(value["message"].as_str().unwrap().contains("16"), "{value}");
}

#[test]
fn bindings_are_recorded_with_their_source_and_unbind_removes_them() {
    let (harness, _root) = config_harness(FakeBackend::new());
    let report = load(
        &harness,
        "paneflow.action{ name = 'dev', title = 'Dev', args = { file = 'string' }, run = function() end }\n\
         paneflow.bind('ctrl-alt-d', 'dev', { file = 'app.log' })\n\
         paneflow.bind('ctrl-alt-z', 'toggle_zoom')\n\
         paneflow.bind('ctrl-alt-x', 'dev', { nope = 1 })\n\
         paneflow.bind('ctrl-alt-q', 'dev', { file = 'a' })\n\
         paneflow.unbind('ctrl+alt+q')\n\
         paneflow.unbind('ctrl-shift-w')\n\
         paneflow.bind('ctrl-alt-f', function() end)\n\
         seen = paneflow.bindings()",
    );
    let keymap = &report.keymap;
    assert_eq!(keymap.bindings.len(), 2, "{keymap:?}");
    assert_eq!(keymap.bindings[0].key, "ctrl-alt-d");
    assert_eq!(keymap.bindings[0].source, "init.lua:2");
    assert_eq!(keymap.bindings[0].target, BindingTarget::Lua);
    assert_eq!(
        keymap.bindings[0].args,
        Some(json!({"file": "app.log"}).as_object().unwrap().clone())
    );
    assert_eq!(keymap.bindings[1].target, BindingTarget::Builtin);
    assert_eq!(keymap.unbound.len(), 2);
    assert_eq!(keymap.unbound[1].key, "ctrl-shift-w");
    let warning = report
        .problems
        .iter()
        .find(|problem| problem.severity == Severity::Warning)
        .unwrap();
    assert_eq!(warning.line, Some(4));
    assert!(warning.message.contains("nope"), "{warning:?}");
    let error = report
        .problems
        .iter()
        .find(|problem| problem.severity == Severity::Error)
        .unwrap();
    assert_eq!(error.line, Some(8));
    assert!(error.message.contains("define the function as an action"));
    let seen: Value = paneflow_script::json::to_json(
        harness.runtime.lua(),
        &harness.runtime.lua().globals().get("seen").unwrap(),
    )
    .unwrap();
    assert_eq!(seen.as_array().unwrap().len(), 3);
}

#[test]
fn queued_built_in_actions_dispatch_in_order_after_run_and_are_dropped_when_it_fails() {
    let (harness, _root) = config_harness(FakeBackend::new());
    load(
        &harness,
        "paneflow.action{ name = 'zoom', title = 'Zoom', run = function()\n\
           paneflow.client.queue('toggle_zoom') paneflow.client.queue('split_vertically')\n\
           paneflow.surface.list{} end }\n\
         paneflow.action{ name = 'fail', title = 'Fail', run = function()\n\
           paneflow.client.queue('toggle_zoom') error('no') end }",
    );
    assert_eq!(
        harness
            .runtime
            .run_action("zoom".into(), json!({}), cli(None)),
        Outcome::Returned(None)
    );
    let names: Vec<String> = harness
        .backend
        .calls()
        .into_iter()
        .map(|(method, params)| format!("{method}:{}", params["name"].as_str().unwrap_or("")))
        .collect();
    assert_eq!(
        names,
        [
            "surface.list:",
            "client.dispatch_action:toggle_zoom",
            "client.dispatch_action:split_vertically"
        ]
    );
    let (harness, _root) = config_harness(FakeBackend::new());
    load(
        &harness,
        "paneflow.action{ name = 'fail', title = 'Fail', run = function()\n\
           paneflow.client.queue('toggle_zoom') error('no') end }",
    );
    assert!(matches!(
        harness
            .runtime
            .run_action("fail".into(), json!({}), cli(None)),
        Outcome::Failed(_)
    ));
    assert!(harness.backend.calls().is_empty());
}

#[test]
fn a_script_dispatches_a_queued_action_at_once_and_raises_with_no_window() {
    let harness = support::harness(FakeBackend::new());
    assert_eq!(
        harness.run("paneflow.client.queue('toggle_zoom') return 1"),
        Outcome::Returned(Some(json!(1)))
    );
    assert_eq!(harness.backend.calls()[0].0, "client.dispatch_action");
    let mut detached = FakeBackend::new();
    detached.window = false;
    let message =
        support::failed(support::harness(detached).run("paneflow.client.queue('toggle_zoom')"));
    assert!(
        message.contains("no Paneflow window is attached"),
        "{message}"
    );
}

#[test]
fn invoke_in_a_script_runs_its_own_action_first_and_the_worker_action_otherwise() {
    let backend = FakeBackend::new().answering(|method, params| match method {
        "action.invoke" => Ok(json!({"result": {"from": "worker", "name": params["name"]}})),
        _ => Err(CallError::new("unexpected", -32601)),
    });
    let harness = support::harness(backend);
    let value = support::returned(harness.run(
        "paneflow.action{ name = 'local_one', title = 'L', run = function() return 'local' end }\n\
         return { mine = paneflow.invoke('local_one'), theirs = paneflow.invoke('dev_layout', { file = 'x' }) }",
    ));
    assert_eq!(
        value,
        json!({"mine": "local", "theirs": {"from": "worker", "name": "dev_layout"}})
    );
    let (method, params) = &harness.backend.calls()[0];
    assert_eq!(method, "action.invoke");
    assert_eq!(params["args"], json!({"file": "x"}));
    assert_eq!(params["ctx"]["origin"], "script");
}

#[test]
fn a_script_that_invokes_its_own_action_gives_it_the_script_origin_and_dispatches_at_once() {
    let harness = support::harness(FakeBackend::new());
    let value = support::returned(harness.run(
        "paneflow.action{ name = 'who', title = 'Who', run = function(ctx)\n\
           paneflow.client.queue('toggle_zoom')\n\
           return ctx.origin end }\n\
         return paneflow.invoke('who')",
    ));
    assert_eq!(value, json!("cli"));
    let calls = harness.backend.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].0, "client.dispatch_action");
    assert_eq!(calls[0].1, json!({"name": "toggle_zoom"}));
}

#[test]
fn a_required_module_is_named_relative_to_the_configuration_and_recorded() {
    let (harness, root) = config_harness(FakeBackend::new());
    std::fs::create_dir_all(root.path().join("lua")).unwrap();
    std::fs::write(
        root.path().join("lua").join("dev.lua"),
        "paneflow.action{ name = 'from_module', title = 'M', run = function() end }\nreturn true",
    )
    .unwrap();
    let report = load(&harness, "require('dev')");
    assert!(report.ok, "{report:?}");
    assert_eq!(
        report.actions[0].source,
        format!("lua{}dev.lua:1", std::path::MAIN_SEPARATOR)
    );
    assert_eq!(report.required, [root.path().join("lua").join("dev.lua")]);
}

#[test]
fn a_load_that_raises_reports_the_error_with_its_line() {
    let (harness, _root) = config_harness(FakeBackend::new());
    let report = load(&harness, "local x = 1\nerror('broken config')");
    assert!(!report.ok);
    let problem = report.problems.last().unwrap();
    assert_eq!(problem.line, Some(2));
    assert_eq!(problem.message, "broken config");
}

#[test]
fn the_served_runtime_runs_two_actions_concurrently_on_one_thread() {
    let (sender, receiver) = std::sync::mpsc::channel::<(String, Result<Option<Value>, String>)>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let backend = Arc::new(FakeBackend::new());
        let sink = Arc::new(CaptureSink::default());
        let mut options = paneflow_script::Options::new(sink);
        options.serving = true;
        let runtime = Runtime::new(backend, options).unwrap();
        let chunk = runtime
            .compile(
                b"local order = {}\n\
                  paneflow.action{ name = 'slow', title = 'Slow', run = function()\n\
                    order[#order + 1] = 'slow-start' paneflow.sleep(0.05) order[#order + 1] = 'slow-end'\n\
                    return order end }\n\
                  paneflow.action{ name = 'fast', title = 'Fast', run = function()\n\
                    order[#order + 1] = 'fast' return 'fast' end }",
                "@init.lua",
            )
            .unwrap();
        runtime.attach().unwrap();
        let mailbox = runtime.mailbox();
        ready_tx.send(mailbox).unwrap();
        runtime.serve(chunk, |report| assert!(report.ok));
    });
    let mailbox: paneflow_script::Mailbox = ready_rx.recv().unwrap();
    for name in ["slow", "fast"] {
        let sender = sender.clone();
        let label = name.to_string();
        mailbox.invoke(
            name.to_string(),
            json!({}),
            InvokeContext::default(),
            Box::new(move |result| {
                let _ = sender.send((label, result.map_err(|failure| failure.message)));
            }),
        );
    }
    let first = receiver.recv_timeout(Duration::from_secs(30)).unwrap();
    let second = receiver.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(first, ("fast".to_string(), Ok(Some(json!("fast")))));
    assert_eq!(
        second,
        (
            "slow".to_string(),
            Ok(Some(json!(["slow-start", "fast", "slow-end"])))
        )
    );
    mailbox.retire();
    thread.join().unwrap();
}

#[test]
fn a_retired_runtime_cancels_its_timers_and_handlers_and_lets_running_actions_finish() {
    let (sender, receiver) = std::sync::mpsc::channel::<Result<Option<Value>, String>>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let (backend, _feed) = FakeBackend::new().with_feed();
        let sink = Arc::new(CaptureSink::default());
        let mut options = paneflow_script::Options::new(sink);
        options.serving = true;
        let runtime = Runtime::new(Arc::new(backend), options).unwrap();
        let stats = runtime.stats();
        let chunk = runtime
            .compile(
                b"paneflow.every(0.01, function() end)\n\
                  paneflow.on('pane.created', function() end)\n\
                  paneflow.action{ name = 'slow', title = 'Slow', run = function() paneflow.sleep(0.1) return 'finished' end }",
                "@init.lua",
            )
            .unwrap();
        runtime.attach().unwrap();
        ready_tx.send((runtime.mailbox(), stats)).unwrap();
        runtime.serve(chunk, |report| {
            assert_eq!(report.timers, 1);
            assert_eq!(report.handlers, 1);
        });
    });
    let (mailbox, stats): (paneflow_script::Mailbox, Arc<paneflow_script::Stats>) =
        ready_rx.recv().unwrap();
    mailbox.invoke(
        "slow".into(),
        json!({}),
        InvokeContext::default(),
        Box::new(move |result| {
            let _ = sender.send(result.map_err(|failure| failure.message));
        }),
    );
    mailbox.retire();
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(30)).unwrap(),
        Ok(Some(json!("finished")))
    );
    thread.join().unwrap();
    assert_eq!(stats.timers(), 0);
    assert_eq!(stats.handlers(), 0);
}
