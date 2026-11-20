#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use paneflow_script::{FeedFrame, Outcome, PaneContext, SessionIds};
use serde_json::json;
use support::{FakeBackend, event, failed, harness, harness_with, returned};

#[test]
fn a_handler_gets_the_data_in_ev_and_the_ids_in_ctx_one_event_at_a_time_in_seq_order() {
    let (backend, feed) = FakeBackend::new().with_feed();
    feed.send(event(
        1,
        "pane.created",
        json!({"workspace_id": 1, "tab_id": 2, "pane_id": 5}),
        json!({"origin": "split"}),
    ))
    .unwrap();
    feed.send(event(
        2,
        "pane.created",
        json!({"workspace_id": 1, "tab_id": 2, "pane_id": 6}),
        json!({"origin": "split"}),
    ))
    .unwrap();
    let harness = harness(backend);
    let recorded = harness.recorder();
    let outcome = harness.run(
        "local seen = 0 \
         paneflow.on('pane.created', function(ctx, ev) \
           record({ step = 'start', pane = ctx.pane_id, tab = ctx.tab_id, origin = ctx.origin, data = ev.origin, seq = ev.seq }) \
           paneflow.sleep(1) \
           record({ step = 'end', pane = ctx.pane_id }) \
           seen = seen + 1 if seen == 2 then paneflow.stop(0) end \
         end)",
    );
    assert_eq!(outcome, Outcome::Stopped(0));
    assert_eq!(
        *recorded.borrow(),
        [
            json!({"step": "start", "pane": 5, "tab": 2, "origin": "event", "data": "split", "seq": 1}),
            json!({"step": "end", "pane": 5}),
            json!({"step": "start", "pane": 6, "tab": 2, "origin": "event", "data": "split", "seq": 2}),
            json!({"step": "end", "pane": 6}),
        ]
    );
}

#[test]
fn on_refuses_to_register_from_a_handler_or_a_timer() {
    let (backend, feed) = FakeBackend::new().with_feed();
    feed.send(event(
        1,
        "tab.created",
        json!({"workspace_id": 1}),
        json!({}),
    ))
    .unwrap();
    let harness = harness(backend);
    let recorded = harness.recorder();
    harness.run(
        "paneflow.after(0, function() local ok, err = pcall(paneflow.on, 'x', print) record(err) end) \
         paneflow.on('tab.created', function() local ok, err = pcall(paneflow.on, 'y', print) record(err) paneflow.stop(0) end)",
    );
    let recorded = recorded.borrow();
    assert_eq!(recorded.len(), 2, "{recorded:?}");
    for message in recorded.iter() {
        let message = message.as_str().unwrap();
        assert!(message.contains("top level"), "{message}");
        assert!(message.starts_with("script.lua:1:"), "{message}");
        assert!(!message.contains("traceback"), "{message}");
    }
}

#[test]
fn wait_returns_the_event_or_nil_and_timeout_and_filters_by_fields_or_function() {
    let (backend, feed) = FakeBackend::new().with_feed();
    feed.send(event(
        1,
        "surface.renamed",
        json!({"workspace_id": 1, "surface_id": 3}),
        json!({"name": "a"}),
    ))
    .unwrap();
    feed.send(event(
        2,
        "surface.renamed",
        json!({"workspace_id": 1, "surface_id": 4}),
        json!({"name": "b"}),
    ))
    .unwrap();
    feed.send(event(
        3,
        "surface.renamed",
        json!({"workspace_id": 1, "surface_id": 4}),
        json!({"name": "c"}),
    ))
    .unwrap();
    let harness = harness(backend);
    let value = returned(harness.run(
        "local by_field, ctx = paneflow.wait('surface.renamed', { surface_id = 4 }) \
         local by_function = paneflow.wait('surface.renamed', function(ev, ctx) return ev.name == 'c' end) \
         local none, why = paneflow.wait('surface.closed', nil, 2) \
         return { field = by_field.name, ctx = ctx.surface_id, origin = ctx.origin, fn = by_function.name, none = none == nil, why = why }",
    ));
    assert_eq!(
        value,
        json!({"field": "b", "ctx": 4, "origin": "event", "fn": "c", "none": true, "why": "timeout"})
    );
}

#[test]
fn a_family_name_matches_every_type_under_its_prefix() {
    let (backend, feed) = FakeBackend::new().with_feed();
    feed.send(event(
        1,
        "pane.created",
        json!({"workspace_id": 1}),
        json!({}),
    ))
    .unwrap();
    feed.send(event(
        2,
        "terminal.bell",
        json!({"workspace_id": 1}),
        json!({}),
    ))
    .unwrap();
    feed.send(event(
        3,
        "terminal.exited",
        json!({"workspace_id": 1}),
        json!({"exit_code": 2}),
    ))
    .unwrap();
    let harness = harness(backend);
    let recorded = harness.recorder();
    harness.run(
        "paneflow.on('terminal.*', function(ctx, ev) record(ev.type) if ev.type == 'terminal.exited' then paneflow.stop(0) end end)",
    );
    assert_eq!(
        *recorded.borrow(),
        [json!("terminal.bell"), json!("terminal.exited")]
    );
}

#[test]
fn inside_a_pane_a_script_hears_its_own_workspace_unless_it_asks_for_all() {
    for (all, expected) in [
        (false, vec![json!(2), json!(2)]),
        (true, vec![json!(2), json!(3), json!(2)]),
    ] {
        let (mut backend, feed) = FakeBackend::new().with_feed();
        backend.context = PaneContext {
            surface_id: Some(9),
            workspace_id: Some(2),
            ..PaneContext::default()
        };
        feed.send(event(
            1,
            "tab.created",
            json!({"workspace_id": 2}),
            json!({}),
        ))
        .unwrap();
        feed.send(event(
            2,
            "tab.created",
            json!({"workspace_id": 3}),
            json!({}),
        ))
        .unwrap();
        feed.send(event(
            3,
            "tab.created",
            json!({"workspace_id": 2}),
            json!({"last": true}),
        ))
        .unwrap();
        let harness = harness_with(backend, |options| options.all_workspaces = all);
        let recorded = harness.recorder();
        harness.run(
            "paneflow.on('tab.created', function(ctx, ev) record(ctx.workspace_id) if ev.last then paneflow.stop(0) end end)",
        );
        assert_eq!(*recorded.borrow(), expected, "all = {all}");
    }
}

#[test]
fn for_ends_the_run_with_status_zero_and_fail_fast_turns_a_handler_error_into_status_one() {
    let (backend, _feed) = FakeBackend::new().with_feed();
    let harness = harness_with(backend, |options| {
        options.run_for = Some(Duration::from_secs(600))
    });
    let outcome = harness.run("paneflow.on('pane.created', function() end) return 'listening'");
    assert_eq!(outcome, Outcome::Returned(Some(json!("listening"))));
    assert_eq!(
        paneflow_script::Clock::now(harness.clock.as_ref()),
        Duration::from_secs(600)
    );

    for fail_fast in [false, true] {
        let (backend, feed) = FakeBackend::new().with_feed();
        feed.send(event(
            1,
            "pane.created",
            json!({"workspace_id": 1}),
            json!({}),
        ))
        .unwrap();
        feed.send(event(
            2,
            "pane.closed",
            json!({"workspace_id": 1}),
            json!({}),
        ))
        .unwrap();
        let harness = harness_with(backend, |options| options.fail_fast = fail_fast);
        let outcome = harness.run(
            "paneflow.on('pane.created', function() error('handler broke') end) \
             paneflow.on('pane.closed', function() paneflow.stop(0) end)",
        );
        if fail_fast {
            assert_eq!(failed(outcome.clone()), "script.lua:1: handler broke");
            assert_eq!(outcome.status(), 1);
        } else {
            assert_eq!(outcome, Outcome::Stopped(0));
            assert!(
                harness.sink.reports().contains(
                    &"Error in a handler for pane.created: script.lua:1: handler broke".to_string()
                ),
                "{:?}",
                harness.sink.reports()
            );
        }
    }
}

#[test]
fn a_stream_that_restarted_from_the_current_seq_calls_the_resync_handlers() {
    let (backend, feed) = FakeBackend::new().with_feed();
    feed.send(event(
        7,
        "pane.created",
        json!({"workspace_id": 1}),
        json!({}),
    ))
    .unwrap();
    feed.send(FeedFrame::Resync).unwrap();
    feed.send(event(
        1,
        "pane.closed",
        json!({"workspace_id": 1}),
        json!({}),
    ))
    .unwrap();
    let harness = harness(backend);
    let recorded = harness.recorder();
    harness.run(
        "paneflow.on('events.resync', function(ctx, ev) record(ev.type) end) \
         paneflow.on('*', function(ctx, ev) record(ev.type) if ev.type == 'pane.closed' then paneflow.stop(0) end end)",
    );
    assert_eq!(
        *recorded.borrow(),
        [
            json!("pane.created"),
            json!("events.resync"),
            json!("pane.closed")
        ]
    );
    assert!(
        harness
            .sink
            .reports()
            .iter()
            .any(|line| line.contains("restarted")),
        "{:?}",
        harness.sink.reports()
    );
}

#[test]
fn a_terminal_event_that_carries_only_a_session_is_matched_by_its_surface() {
    let (mut backend, feed) = FakeBackend::new().with_feed();
    backend.sessions.insert(
        "s-17".into(),
        SessionIds {
            surface_id: Some(17),
            workspace_id: Some(2),
            ..SessionIds::default()
        },
    );
    feed.send(event(
        1,
        "terminal.exited",
        json!({"session": "s-9"}),
        json!({"exit_code": 0}),
    ))
    .unwrap();
    feed.send(event(
        2,
        "terminal.exited",
        json!({"session": "s-17"}),
        json!({"exit_code": 1, "lifecycle": "exited"}),
    ))
    .unwrap();
    let harness = harness(backend);
    let value = returned(harness.run(
        "local ev, ctx = paneflow.wait('terminal.exited', { surface_id = 17 }, 600) \
         return { exit_code = ev.exit_code, surface = ctx.surface_id, workspace = ctx.workspace_id, session = ev.session }",
    ));
    assert_eq!(
        value,
        json!({"exit_code": 1, "surface": 17, "workspace": 2, "session": "s-17"})
    );
}

#[test]
fn wait_with_no_event_stream_returns_nil_and_the_reason() {
    let harness = harness(FakeBackend::new());
    let value = returned(harness.run(
        "local ev, why = paneflow.wait('pane.created', nil, 1) return { ev = ev == nil, why = why }",
    ));
    assert_eq!(value["ev"], true);
    assert!(
        value["why"].as_str().unwrap().contains("no worker"),
        "{value}"
    );
}

#[test]
fn a_later_wait_still_gets_an_event_that_arrived_before_one_already_returned() {
    let (backend, feed) = FakeBackend::new().with_feed();
    feed.send(event(
        1,
        "terminal.exited",
        json!({"workspace_id": 1, "surface_id": 4}),
        json!({"exit_code": 4}),
    ))
    .unwrap();
    feed.send(event(
        2,
        "terminal.exited",
        json!({"workspace_id": 1, "surface_id": 3}),
        json!({"exit_code": 3}),
    ))
    .unwrap();
    let harness = harness(backend);
    let value = returned(harness.run(
        "local first = paneflow.wait('terminal.exited', { surface_id = 3 }) \
         local second = paneflow.wait('terminal.exited', { surface_id = 4 }, 2) \
         local again, why = paneflow.wait('terminal.exited', nil, 2) \
         return { first = first.exit_code, second = second and second.exit_code, again = again == nil, why = why }",
    ));
    assert_eq!(
        value,
        json!({"first": 3, "second": 4, "again": true, "why": "timeout"})
    );
}

#[test]
fn wait_keeps_hearing_events_after_a_restart_numbers_them_from_one_again() {
    let (backend, feed) = FakeBackend::new().with_feed();
    feed.send(event(
        500,
        "tab.created",
        json!({"workspace_id": 1}),
        json!({}),
    ))
    .unwrap();
    feed.send(FeedFrame::Resync).unwrap();
    feed.send(event(
        1,
        "tab.created",
        json!({"workspace_id": 1}),
        json!({}),
    ))
    .unwrap();
    let harness = harness(backend);
    let value = returned(harness.run(
        "local before = paneflow.wait('tab.created') \
         local after = paneflow.wait('tab.created', nil, 2) \
         return { before = before.seq, after = after and after.seq }",
    ));
    assert_eq!(value, json!({"before": 500, "after": 1}));
}

#[test]
fn wait_inside_a_handler_raises_an_error_that_names_the_limitation() {
    let (backend, feed) = FakeBackend::new().with_feed();
    feed.send(event(
        1,
        "tab.created",
        json!({"workspace_id": 1}),
        json!({}),
    ))
    .unwrap();
    let harness = harness(backend);
    let recorded = harness.recorder();
    let outcome = harness.run(
        "paneflow.on('tab.created', function() \
           local ok, err = pcall(function() local ev = paneflow.wait('tab.closed') return ev end) record(err) paneflow.stop(0) \
         end)",
    );
    assert_eq!(outcome, Outcome::Stopped(0));
    let recorded = recorded.borrow();
    let message = recorded[0].as_str().unwrap();
    assert!(message.contains("inside an event handler"), "{message}");
    assert!(message.starts_with("script.lua:1:"), "{message}");
}

#[test]
fn for_ends_a_loop_that_never_waits_with_status_zero() {
    let harness = harness_with(FakeBackend::new(), |options| {
        options.run_for = Some(Duration::from_secs(2));
    });
    harness.define_clock();
    let outcome = harness.run("advance(5) while true do end");
    assert_eq!(outcome, Outcome::Returned(None));
    assert_eq!(outcome.status(), 0);
}
