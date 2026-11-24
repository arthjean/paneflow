#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use paneflow_script::{EXIT_INTERRUPTED, FakeClock, Options, Outcome};
use serde_json::json;
use support::{FakeBackend, failed, harness, harness_with, returned};

#[test]
fn an_interrupt_stops_a_loop_that_never_yields() {
    let harness = harness(FakeBackend::new());
    let interrupter = harness.runtime.interrupter();
    harness.define("press_ctrl_c", move |_, _| {
        interrupter.interrupt();
        Ok(mlua::MultiValue::new())
    });
    let outcome =
        harness.run("local i = 0 while true do i = i + 1 if i == 5000 then press_ctrl_c() end end");
    assert_eq!(outcome, Outcome::Interrupted);
    assert_eq!(outcome.status(), EXIT_INTERRUPTED);
}

#[test]
fn a_deadline_stops_only_the_coroutine_that_overran_it() {
    let harness = harness_with(FakeBackend::new(), |options| {
        options.clock = Arc::new(FakeClock::ticking(Duration::from_millis(1)));
        options.deadline = Some(Duration::from_millis(200));
    });
    let value = returned(harness.run(
        "local done = false \
         paneflow.after(0, function() while true do end end) \
         paneflow.after(0.5, function() done = true end) \
         paneflow.sleep(1) \
         return { done = done }",
    ));
    assert_eq!(value, json!({"done": true}));
    let reports = harness.sink.reports();
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert!(reports[0].contains("timeout"), "{reports:?}");
}

#[test]
fn a_memory_limit_fails_the_chunk_and_leaves_the_runtime_usable() {
    let harness = harness_with(FakeBackend::new(), |options| {
        options.memory_limit = Some(8 * 1024 * 1024);
    });
    let message = failed(harness.run("local t = {} for i = 1, 1e8 do t[i] = i end"));
    assert!(message.contains("not enough memory"), "{message}");
    let value = returned(harness.run(
        "collectgarbage() local ok, err = pcall(function() local t = {} for i = 1, 1e8 do t[i] = i end end) \
         collectgarbage() local small = {} for i = 1, 100 do small[i] = i end \
         return { ok = ok, err = tostring(err), small = #small }",
    ));
    assert_eq!(value["ok"], false);
    assert!(
        value["err"].as_str().unwrap().contains("not enough memory"),
        "{value}"
    );
    assert_eq!(value["small"], 100);
}

#[test]
fn an_async_call_in_a_script_coroutine_raises_an_error_and_never_leaks_a_value() {
    let value = returned(harness(FakeBackend::new()).run(
        "local co = coroutine.create(function() local slept = paneflow.sleep(0.1) return slept end) \
         local ok, err = coroutine.resume(co) \
         local wrapped = coroutine.wrap(function() return paneflow.call('system.ping') end) \
         local wok, werr = pcall(wrapped) \
         local inner = coroutine.wrap(function() return paneflow.surface.list() end) \
         local iok, ierr = pcall(inner) \
         return { ok = ok, err = err, wok = wok, werr = werr, iok = iok, ierr = ierr }",
    ));
    for (flag, message) in [("ok", "err"), ("wok", "werr"), ("iok", "ierr")] {
        assert_eq!(value[flag], false, "{value}");
        let text = value[message].as_str().unwrap();
        assert!(
            text.contains("coroutine.create or coroutine.wrap"),
            "{text}"
        );
    }
    assert!(
        value["err"].as_str().unwrap().starts_with("script.lua:1:"),
        "{value}"
    );
}

#[test]
fn an_async_call_in_a_sort_comparator_names_the_limitation_and_the_fix() {
    let message = failed(harness(FakeBackend::new()).run(
        "local t = {3, 1, 2}\ntable.sort(t, function(a, b) paneflow.sleep(0) return a < b end)",
    ));
    assert!(message.contains("table.sort comparator"), "{message}");
    assert!(
        message.contains("Move the call out of the callback"),
        "{message}"
    );
    assert!(message.starts_with("script.lua:2:"), "{message}");
    let message =
        failed(harness(FakeBackend::new()).run(
            "return (string.gsub('abc', '%w', function(c) return paneflow.exec({'true'}) end))",
        ));
    assert!(
        message.contains("Move the call out of the callback"),
        "{message}"
    );
}

#[test]
fn a_runtime_has_no_memory_limit_and_no_deadline_unless_asked() {
    let options = Options::new(Arc::new(support::CaptureSink::default()));
    assert_eq!(options.memory_limit, None);
    assert_eq!(options.deadline, None);
}
