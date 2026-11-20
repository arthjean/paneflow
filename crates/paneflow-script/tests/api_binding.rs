#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Mutex;
use std::sync::mpsc::channel;
use std::time::Duration;

use paneflow_script::{CallError, PaneContext};
use serde_json::{Value, json};
use support::{FakeBackend, harness, returned};

#[test]
fn every_discovered_method_becomes_a_function_under_its_namespace() {
    let mut backend = FakeBackend::new();
    backend.extra_methods.push(json!({
        "name": "future.thing",
        "params": {"type": "object", "properties": {"n": {"type": "integer"}}},
        "stream": false,
    }));
    let harness = harness(backend);
    let names: Vec<String> = paneflow_api::catalog()
        .unwrap()
        .iter()
        .map(|descriptor| descriptor.name().to_string())
        .chain(["future.thing".to_string()])
        .collect();
    let source = format!(
        "local missing = {{}} for _, name in ipairs({{ {} }}) do \
           local ns, leaf = name:match('^([^.]+)%.(.+)$') \
           if type(paneflow[ns]) ~= 'table' or type(paneflow[ns][leaf]) ~= 'function' then missing[#missing + 1] = name end \
         end \
         return {{ missing = #missing, first = missing[1], called = paneflow.future.thing{{ n = 2 }} }}",
        names
            .iter()
            .map(|name| format!("'{name}'"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let value = returned(harness.run(&source));
    assert_eq!(value["missing"], 0, "{value}");
    assert_eq!(value["called"]["method"], "future.thing");
}

#[test]
fn call_reaches_any_method_by_its_full_name() {
    let harness = harness(FakeBackend::new().answering(|method, _| match method {
        "system.ping" => Ok(json!({"pong": true})),
        other => Ok(json!({"other": other})),
    }));
    let value = returned(harness.run("return paneflow.call('system.ping')"));
    assert_eq!(value, json!({"pong": true}));
    let value = returned(harness.run(
        "local r, message, code = paneflow.call('surface.teleport', {}) return { r = r == nil, message = message, code = code }",
    ));
    assert_eq!(value["r"], true);
    assert_eq!(value["message"], "Method not found: surface.teleport");
    assert_eq!(value["code"], CallError::METHOD_NOT_FOUND);
}

#[test]
fn a_success_returns_a_table_and_a_failure_returns_nil_the_message_and_the_code() {
    let harness = harness(FakeBackend::new().answering(|method, _| match method {
        "surface.list" => Ok(json!({"surfaces": [{"surface_id": 3}], "pane_count": 1})),
        _ => Err(CallError::new("Surface not found: 99", -32004)),
    }));
    let value = returned(harness.run(
        "local list = paneflow.surface.list() \
         local r, message, code = paneflow.surface.focus{ surface_id = 99 } \
         return { first = list.surfaces[1].surface_id, r = r == nil, message = message, code = code }",
    ));
    assert_eq!(
        value,
        json!({"first": 3, "r": true, "message": "Surface not found: 99", "code": -32004})
    );
}

#[test]
fn inside_a_pane_an_omitted_surface_or_workspace_comes_from_the_pane() {
    let mut backend = FakeBackend::new();
    backend.context = PaneContext {
        surface_id: Some(12),
        pane_id: Some(5),
        tab_id: Some(4),
        workspace_id: Some(2),
    };
    let harness = harness(backend);
    let value = returned(harness.run(
        "paneflow.surface.read{ lines = 3 } \
         paneflow.surface.split{ direction = 'vertical' } \
         paneflow.surface.read{ surface_id = 40 } \
         return paneflow.ctx",
    ));
    assert_eq!(
        value,
        json!({"surface_id": 12, "pane_id": 5, "tab_id": 4, "workspace_id": 2, "origin": "cli"})
    );
    let calls = harness.backend.calls();
    assert_eq!(
        calls[0].1,
        json!({"lines": 3, "surface_id": 12, "workspace_id": 2})
    );
    assert_eq!(
        calls[1].1,
        json!({"direction": "vertical", "surface_id": 12})
    );
    assert_eq!(calls[2].1["surface_id"], 40);
}

#[test]
fn an_undeclared_key_is_refused_without_sending_the_request() {
    let harness = harness(FakeBackend::new());
    let value = returned(harness.run(
        "local r, message, code = paneflow.surface.read{ line = 3 } return { r = r == nil, message = message, code = code }",
    ));
    assert_eq!(value["r"], true);
    let message = value["message"].as_str().unwrap();
    assert!(message.contains("'line'"), "{message}");
    assert!(
        message.contains("lines") && message.contains("surface_id"),
        "{message}"
    );
    assert_eq!(value["code"], CallError::INVALID_PARAMS);
    assert!(harness.backend.calls().is_empty());
}

#[test]
fn a_timer_runs_while_an_api_call_is_in_flight() {
    let (release, released) = channel::<()>();
    let released = Mutex::new(released);
    let backend = FakeBackend::new().answering(move |_, _| {
        released
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| CallError::new("never released", -1))?;
        Ok(json!({"pong": true}))
    });
    let harness = support::harness_with(backend, |options| {
        options.clock = std::sync::Arc::new(paneflow_script::SystemClock::new());
    });
    let release = Mutex::new(release);
    harness
        .runtime
        .lua()
        .globals()
        .set(
            "release",
            harness
                .runtime
                .lua()
                .create_function(move |_, ()| {
                    release.lock().unwrap().send(()).unwrap();
                    Ok(())
                })
                .unwrap(),
        )
        .unwrap();
    let value = returned(harness.run(
        "local order = {} \
         paneflow.after(0.01, function() order[#order + 1] = 'timer' release() end) \
         local answer = paneflow.system.ping() \
         order[#order + 1] = 'call' \
         return { order = order, pong = answer.pong }",
    ));
    assert_eq!(value, json!({"order": ["timer", "call"], "pong": true}));
}

#[test]
fn layout_helpers_build_a_document_workspace_create_accepts_and_call_nothing() {
    let harness = harness(FakeBackend::new());
    let value = returned(harness.run(
        "local doc = paneflow.layout.horizontal(0.3, \
            paneflow.layout.pane{ command = 'htop' }, \
            paneflow.layout.vertical(0.5, paneflow.layout.pane{}, paneflow.layout.pane{ surfaces = { { command = 'ls' } } })) \
         local before = 0 \
         paneflow.workspace.create{ name = 'w', layout = doc } \
         return doc",
    ));
    let calls = harness.backend.calls();
    assert_eq!(calls.len(), 1, "the helpers called something");
    assert_eq!(calls[0].0, "workspace.create");
    let layout: paneflow_config::schema::LayoutNode =
        serde_json::from_value(calls[0].1["layout"].clone()).expect("a layout node");
    assert_eq!(layout.leaf_count(), 3);
    assert_eq!(value["type"], "split");
    assert_eq!(value["direction"], "horizontal");
    assert_eq!(value["children"][0]["surfaces"][0]["command"], "htop");
    assert_eq!(value["children"][1]["children"][0]["surfaces"], json!([]));
    let ratios: Vec<f64> = serde_json::from_value(value["ratios"].clone()).unwrap();
    assert!((ratios[0] - 0.3).abs() < 1e-9 && (ratios[1] - 0.7).abs() < 1e-9);
}

#[test]
fn with_no_instance_every_namespace_function_returns_nil_and_the_no_instance_message() {
    let mut backend = FakeBackend::new();
    backend.unavailable = Some(CallError::new(
        "cannot locate the IPC socket; is Paneflow running?",
        CallError::UNAVAILABLE,
    ));
    let harness = harness(backend);
    let value = returned(harness.run(
        "local r, message, code = paneflow.surface.list() \
         local c, cm = paneflow.call('system.ping') \
         return { r = r == nil, message = message, code = code, c = c == nil, cm = cm }",
    ));
    assert_eq!(value["r"], true);
    assert_eq!(value["c"], true);
    assert!(
        value["message"]
            .as_str()
            .unwrap()
            .contains("is Paneflow running")
    );
    assert_eq!(value["cm"], value["message"]);
    assert!(harness.backend.calls().is_empty());
    let _: Value = value;
}
