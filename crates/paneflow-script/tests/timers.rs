#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use paneflow_script::Clock;
use serde_json::json;
use support::{FakeBackend, failed, harness, returned};

#[test]
fn sleep_takes_fractional_seconds_and_lets_other_timers_run_meanwhile() {
    let harness = harness(FakeBackend::new());
    harness.define_clock();
    let value = returned(harness.run(
        "local order = {} \
         paneflow.after(0.1, function() order[#order + 1] = 'timer' end) \
         local before = now() paneflow.sleep(0.25) order[#order + 1] = 'woke' \
         return { order = order, slept = now() - before }",
    ));
    assert_eq!(value["order"], json!(["timer", "woke"]));
    assert!(
        (value["slept"].as_f64().unwrap() - 0.25).abs() < 1e-9,
        "{value}"
    );
}

#[test]
fn after_and_every_return_a_timer_and_pass_a_timer_context() {
    let harness = harness(FakeBackend::new());
    let value = returned(harness.run(
        "local seen = {} \
         local once = paneflow.after(1, function(ctx) seen.after = ctx.origin end) \
         local ticks = 0 \
         local repeating repeating = paneflow.every(1, function(ctx) ticks = ticks + 1 seen.every = ctx.origin if ticks == 2 then repeating:cancel() end end) \
         local cancelled = paneflow.after(5, function() seen.cancelled = true end) \
         local was = cancelled:active() cancelled:cancel() \
         paneflow.sleep(3) \
         return { seen = seen, ticks = ticks, once_active = once:active(), repeating_active = repeating:active(), was_active = was, now_active = cancelled:active() }",
    ));
    assert_eq!(
        value,
        json!({
            "seen": {"after": "timer", "every": "timer"},
            "ticks": 2,
            "once_active": false,
            "repeating_active": false,
            "was_active": true,
            "now_active": false,
        })
    );
}

#[test]
fn the_run_continues_past_the_top_level_while_a_timer_is_active() {
    let harness = harness(FakeBackend::new());
    let value = returned(harness.run(
        "local n = 0 local t t = paneflow.every(1, function() n = n + 1 if n == 3 then t:cancel() end end) return 'started'",
    ));
    assert_eq!(value, "started");
    assert_eq!(harness.clock.now(), Duration::from_secs(3));
    let reports = harness.sink.reports();
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert!(reports[0].contains("1 active timer"), "{reports:?}");
}

#[test]
fn a_tick_missed_while_the_script_was_busy_runs_once_and_the_rest_are_dropped() {
    let harness = harness(FakeBackend::new());
    harness.define_clock();
    let value = returned(harness.run(
        "local n = 0 local t = paneflow.every(1, function() n = n + 1 end) \
         advance(5.5) \
         local during = n \
         paneflow.sleep(0.1) local after_busy = n \
         paneflow.sleep(0.3) local before_next = n \
         paneflow.sleep(0.2) local next_tick = n \
         t:cancel() \
         return { during = during, after_busy = after_busy, before_next = before_next, next_tick = next_tick }",
    ));
    assert_eq!(
        value,
        json!({"during": 0, "after_busy": 1, "before_next": 1, "next_tick": 2})
    );
}

#[test]
fn an_error_in_a_timer_is_printed_and_the_timer_keeps_running() {
    let harness = harness(FakeBackend::new());
    let value = returned(harness.run(
        "local n = 0 local t t = paneflow.every(1, function() n = n + 1 if n == 3 then t:cancel() end error('boom ' .. n) end) paneflow.sleep(4) return n",
    ));
    assert_eq!(value, 3);
    let reports = harness.sink.reports();
    assert_eq!(
        reports,
        [
            "Error in a timer: script.lua:1: boom 1",
            "Error in a timer: script.lua:1: boom 2",
            "Error in a timer: script.lua:1: boom 3",
        ]
    );
}

#[test]
fn every_zero_raises_an_error() {
    let message = failed(harness(FakeBackend::new()).run("paneflow.every(0, function() end)"));
    assert!(message.contains("more than 0 seconds"), "{message}");
    let message = failed(harness(FakeBackend::new()).run("paneflow.after(-1, function() end)"));
    assert!(message.contains("zero or more"), "{message}");
}

#[test]
fn the_timer_past_the_limit_stops_the_script_even_inside_pcall() {
    let harness = harness(FakeBackend::new());
    let outcome = harness.run(
        "for i = 1, 1024 do paneflow.after(60, function() end) end \
         local ok, message = pcall(paneflow.after, 60, function() end) \
         return 'survived'",
    );
    let message = failed(outcome);
    assert!(message.contains("1024 active timers"), "{message}");
    assert!(message.starts_with("script.lua:1:"), "{message}");
}
