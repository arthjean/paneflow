#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use paneflow_api::methods::config::InvokeContext;
use paneflow_script::{CallError, FakeClock, Outcome, Runtime};
use serde_json::{Value, json};
use support::{CaptureSink, FakeBackend, Harness, harness as fresh, harness_with, returned};

fn answering(answer: Value) -> FakeBackend {
    FakeBackend::new().answering(move |method, _| match method {
        "prompt.request" => Ok(json!({"status": "answered", "id": 1, "answer": answer})),
        other => Ok(json!({"method": other})),
    })
}

fn ending(outcome: Value) -> FakeBackend {
    FakeBackend::new().answering(move |method, _| match method {
        "prompt.request" => Ok(outcome.clone()),
        other => Ok(json!({"method": other})),
    })
}

fn requests(harness: &Harness) -> Vec<Value> {
    harness
        .backend
        .calls()
        .into_iter()
        .filter(|(method, _)| method.starts_with("prompt."))
        .map(|(_, params)| params)
        .collect()
}

fn served(backend: FakeBackend) -> (Harness, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_path_buf();
    let harness = harness_with(backend, move |options| {
        options.config_root = Some(path.clone());
        options.serving = true;
    });
    (harness, root)
}

fn load(harness: &Harness, source: &str) -> paneflow_script::LoadReport {
    let chunk = harness
        .runtime
        .compile(source.as_bytes(), "@init.lua")
        .unwrap();
    harness.runtime.attach().unwrap();
    harness.runtime.load(chunk)
}

fn key(surface: u64) -> InvokeContext {
    InvokeContext {
        origin: Some("key".into()),
        surface_id: Some(surface),
        workspace_id: Some(2),
        client_id: Some("window".into()),
        ..InvokeContext::default()
    }
}

#[test]
fn input_returns_the_text_and_pick_the_id_the_ids_or_the_typed_text() {
    let harness = fresh(answering(json!(" feature/x ")));
    let outcome =
        harness.run("return paneflow.ui.input{ title = 'New branch', value = 'feature/' }");
    assert_eq!(returned(outcome), json!(" feature/x "));
    let sent = &requests(&harness)[0];
    assert_eq!(sent["kind"], "input");
    assert_eq!(sent["title"], "New branch");
    assert_eq!(sent["value"], "feature/");
    assert_eq!(sent["wait_ms"], 20_000);
    assert_eq!(sent["origin"]["kind"], "script");

    for (answer, spec) in [
        (json!("web"), "items = { 'api', 'web' }"),
        (
            json!(["api", "web"]),
            "items = { 'api', 'web' }, multiple = true",
        ),
        (
            json!({"text": "cli"}),
            "items = { 'api' }, allow_other = true",
        ),
    ] {
        let harness = fresh(answering(answer.clone()));
        let outcome = harness.run(&format!(
            "return paneflow.ui.pick{{ title = 'Project', {spec} }}"
        ));
        assert_eq!(returned(outcome), answer);
        assert_eq!(requests(&harness)[0]["kind"], "pick");
    }
}

#[test]
fn no_answer_returns_nil_and_the_reason_with_a_detail_only_for_failed() {
    for status in ["cancelled", "timeout", "no_client", "unsupported", "busy"] {
        let harness = fresh(ending(json!({"status": status})));
        let outcome = harness.run(
            "local answer, reason, detail = paneflow.ui.input{ title = 'Name' }\n\
             return { answer = answer == nil, reason = reason, detail = detail == nil }",
        );
        assert_eq!(
            returned(outcome),
            json!({"answer": true, "reason": status, "detail": true})
        );
    }
    let harness = fresh(ending(
        json!({"status": "failed", "id": 3, "detail": "the Paneflow worker stopped"}),
    ));
    let outcome = harness.run(
        "local answer, reason, detail = paneflow.ui.input{ title = 'Name' }\n\
         return { reason = reason, detail = detail }",
    );
    assert_eq!(
        returned(outcome),
        json!({"reason": "failed", "detail": "the Paneflow worker stopped"})
    );
    let harness = fresh(FakeBackend::new().answering(|_, _| {
        Err(CallError::new(
            "prompt.request is served by the Paneflow worker, which is not reachable",
            CallError::UNAVAILABLE,
        ))
    }));
    let outcome = harness.run(
        "local answer, reason, detail = paneflow.ui.pick{ title = 'Project', items = { 'a' } }\n\
         return { reason = reason, detail = detail }",
    );
    let result = returned(outcome);
    assert_eq!(result["reason"], "failed");
    assert!(result["detail"].as_str().unwrap().contains("not reachable"));
}

#[test]
fn a_pending_prompt_is_waited_on_by_id_until_it_ends() {
    let waits = Arc::new(Mutex::new(0));
    let counted = waits.clone();
    let harness = fresh(
        FakeBackend::new().answering(move |method, params| match method {
            "prompt.request" => Ok(json!({"status": "pending", "id": 9})),
            "prompt.wait" => {
                assert_eq!(params["id"], 9);
                let mut waits = counted.lock().unwrap();
                *waits += 1;
                if *waits < 3 {
                    Ok(json!({"status": "pending", "id": 9}))
                } else {
                    Ok(json!({"status": "answered", "id": 9, "answer": "later"}))
                }
            }
            other => Ok(json!({"method": other})),
        }),
    );
    let outcome = harness.run("return paneflow.ui.input{ title = 'Name' }");
    assert_eq!(returned(outcome), json!("later"));
    assert_eq!(*waits.lock().unwrap(), 3);
}

#[test]
fn a_mistake_in_the_call_raises_before_anyone_is_asked() {
    for (source, expected) in [
        (
            "paneflow.ui.input{ placeholder = 'x' }",
            "missing 'title' parameter",
        ),
        (
            "paneflow.ui.pick{ title = 'P', items = { 'a', { id = 'a' } } }",
            "'items[1].id' repeats the id 'a'",
        ),
        (
            "paneflow.ui.pick{ title = 'P', items = { 'a' }, multiple = true, allow_other = true }",
            "'allow_other'",
        ),
        (
            "paneflow.ui.input{ title = 'T', kind = 'pick' }",
            "'kind' is set by Paneflow",
        ),
        ("paneflow.ui.input('title')", "takes a table"),
        (
            "paneflow.ui.input{ title = 'T', colour = 'red' }",
            "'colour' is not a known field",
        ),
    ] {
        let harness = fresh(answering(json!("x")));
        let Outcome::Failed(message) = harness.run(source) else {
            panic!("{source} raises");
        };
        assert!(message.contains(expected), "{source}: {message}");
        assert!(message.starts_with("script.lua:1:"), "{message}");
        assert!(requests(&harness).is_empty(), "{source} asked nobody");
    }
}

#[test]
fn it_works_under_pcall_in_a_nested_function_a_handler_and_a_timer() {
    let harness = fresh(answering(json!("ok")));
    let outcome = harness.run(
        "local function ask() return paneflow.ui.input{ title = 'Nested' } end\n\
         local caught, answer = pcall(paneflow.ui.input, { title = 'Protected' })\n\
         local from_timer\n\
         paneflow.after(0, function() from_timer = ask() end)\n\
         paneflow.sleep(0.01)\n\
         return { caught = caught, answer = answer, nested = ask(), timer = from_timer }",
    );
    assert_eq!(
        returned(outcome),
        json!({"caught": true, "answer": "ok", "nested": "ok", "timer": "ok"})
    );

    let (backend, feed) = answering(json!("handled")).with_feed();
    feed.send(support::event(
        1,
        "pane.created",
        json!({"pane_id": 5}),
        json!({}),
    ))
    .unwrap();
    let harness = harness_with(backend, |_| {});
    let recorded = harness.recorder();
    let outcome = harness.run(
        "paneflow.on('pane.created', function() record(paneflow.ui.input{ title = 'From a handler' }) paneflow.stop(0) end)",
    );
    assert_eq!(outcome, Outcome::Stopped(0));
    assert_eq!(*recorded.borrow(), [json!("handled")]);
}

#[test]
fn a_sort_comparator_or_a_coroutine_the_script_created_raises_the_waiting_error() {
    let harness = fresh(answering(json!("x")));
    let Outcome::Failed(message) = harness.run(
        "local t = { 2, 1 } table.sort(t, function(a, b) paneflow.ui.input{ title = 'T' } return a < b end)",
    ) else {
        panic!("raises");
    };
    assert!(
        message.contains("paneflow.ui.input cannot wait inside a callback"),
        "{message}"
    );
    let Outcome::Failed(message) = harness.run(
        "local co = coroutine.wrap(function() return paneflow.ui.pick{ title = 'T', items = { 'a' } } end) co()",
    ) else {
        panic!("raises");
    };
    assert!(
        message.contains("paneflow.ui.pick cannot run inside a coroutine"),
        "{message}"
    );
}

#[test]
fn at_the_top_level_of_init_lua_it_returns_failed_without_asking() {
    let (harness, _root) = served(answering(json!("x")));
    let report = load(
        &harness,
        "local answer, reason, detail = paneflow.ui.input{ title = 'At load' }\n\
         assert(answer == nil and reason == 'failed' and detail:find('while init.lua loads'), tostring(reason))",
    );
    assert!(report.ok, "{report:?}");
    assert!(requests(&harness).is_empty());
}

#[test]
fn an_action_asks_with_its_title_and_its_thirty_second_limit_waits_with_it() {
    let clock = Arc::new(FakeClock::new());
    let advancing = clock.clone();
    let backend = FakeBackend::new().answering(move |method, _| match method {
        "prompt.request" => {
            advancing.advance(Duration::from_secs(90));
            Ok(json!({"status": "answered", "id": 1, "answer": "fix/login"}))
        }
        other => Ok(json!({"method": other})),
    });
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_path_buf();
    let shared_clock = clock.clone();
    let harness = harness_with(backend, move |options| {
        options.clock = shared_clock.clone();
        options.config_root = Some(path.clone());
        options.serving = true;
    });
    let report = load(
        &harness,
        "paneflow.action{ name = 'new_branch', title = 'New branch', run = function()\n\
           local name = paneflow.ui.input{ title = 'New branch', value = 'feature/' }\n\
           if not name then return end\n\
           paneflow.sleep(20)\n\
           paneflow.surface.send_text{ text = 'git switch -c ' .. name, submit = true }\n\
           return name end }\n\
         paneflow.bind('ctrl-alt-b', 'new_branch')",
    );
    assert!(report.ok, "{report:?}");
    let outcome = harness
        .runtime
        .run_action("new_branch".into(), json!({}), key(4));
    assert_eq!(outcome, Outcome::Returned(Some(json!("fix/login"))));
    let calls = harness.backend.calls();
    let asked = &calls
        .iter()
        .find(|(method, _)| method == "prompt.request")
        .unwrap()
        .1;
    assert_eq!(
        asked["origin"],
        json!({"kind": "action", "name": "new_branch", "title": "New branch", "surface_id": 4, "workspace_id": 2})
    );
    let sent = &calls
        .iter()
        .find(|(method, _)| method == "surface.send_text")
        .unwrap()
        .1;
    assert_eq!(sent["text"], "git switch -c fix/login");
    assert_eq!(sent["submit"], true);
    assert_eq!(
        sent["surface_id"], 4,
        "the focused terminal of the key press"
    );
}

#[test]
fn a_prompt_open_in_one_action_lets_another_action_and_a_timer_run() {
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let (sender, receiver) = std::sync::mpsc::channel::<(String, Result<Option<Value>, String>)>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let backend = Arc::new(FakeBackend::new().answering(move |method, _| match method {
            "prompt.request" => {
                release_rx.lock().unwrap().recv().unwrap();
                Ok(json!({"status": "answered", "id": 1, "answer": "late"}))
            }
            other => Ok(json!({"method": other})),
        }));
        let mut options = paneflow_script::Options::new(Arc::new(CaptureSink::default()));
        options.serving = true;
        let runtime = Runtime::new(backend, options).unwrap();
        let chunk = runtime
            .compile(
                b"local ticks = 0\n\
                  paneflow.every(0.01, function() ticks = ticks + 1 end)\n\
                  paneflow.action{ name = 'ask', title = 'Ask', run = function() return paneflow.ui.input{ title = 'Wait' } end }\n\
                  paneflow.action{ name = 'other', title = 'Other', run = function() paneflow.sleep(0.05) return ticks end }",
                "@init.lua",
            )
            .unwrap();
        runtime.attach().unwrap();
        ready_tx.send(runtime.mailbox()).unwrap();
        runtime.serve(chunk, |report| assert!(report.ok));
    });
    let mailbox: paneflow_script::Mailbox = ready_rx.recv().unwrap();
    for name in ["ask", "other"] {
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
    let (first, ticks) = receiver.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(first, "other");
    assert!(
        ticks.unwrap().unwrap().as_i64().unwrap() > 0,
        "the timer ran"
    );
    release_tx.send(()).unwrap();
    let second = receiver.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(second, ("ask".to_string(), Ok(Some(json!("late")))));
    mailbox.retire();
    thread.join().unwrap();
}

#[test]
fn an_empty_items_table_is_sent_as_an_empty_list() {
    let harness = fresh(ending(json!({"status": "cancelled"})));
    let outcome = harness.run(
        "local items = {}\n\
         local picked, reason = paneflow.ui.pick{ title = 'Nothing here', items = items }\n\
         return { picked = picked, reason = reason }",
    );
    assert_eq!(returned(outcome), json!({"reason": "cancelled"}));
    assert_eq!(requests(&harness)[0]["items"], json!([]));
}
