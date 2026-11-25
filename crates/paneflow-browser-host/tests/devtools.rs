#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{DEADLINE, FixtureServer, Harness};
use paneflow_browser_client::ClientEvent;
use paneflow_browser_proto::limits::DEVTOOLS_METHOD_TIMEOUT;
use paneflow_browser_proto::{CommandError, ConsoleLevel, Event, Reply};
use serde_json::json;

#[test]
fn results_return_matched_to_their_request() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = harness.open("devtools", &server.url("/form.html"));
    harness.wait_loaded(tab, "/form.html");

    let tickets: Vec<_> = (0..8)
        .map(|index| {
            let ticket = harness.client.devtools(
                tab,
                "Runtime.evaluate",
                json!({"expression": format!("'answer-' + {index}"), "returnByValue": true}),
            );
            (index, ticket)
        })
        .collect();
    let title = harness.client.devtools(
        tab,
        "Runtime.evaluate",
        json!({"expression": "document.title", "returnByValue": true}),
    );
    for (index, ticket) in tickets.into_iter().rev() {
        let Ok(Reply::DevTools(result)) = ticket.wait(DEADLINE) else {
            panic!("no result for {index}");
        };
        assert_eq!(result["result"]["value"], format!("answer-{index}"));
    }
    let Ok(Reply::DevTools(result)) = title.wait(DEADLINE) else {
        panic!("no title");
    };
    assert_eq!(result["result"]["value"], "Checkout form");

    let unknown = harness.client.devtools(tab, "Nope.doesNotExist", json!({}));
    assert!(matches!(
        unknown.wait(DEADLINE),
        Err(CommandError::DevTools { method, .. }) if method == "Nope.doesNotExist"
    ));
    harness.shutdown();
}

#[test]
fn console_and_devtools_events_reach_the_window() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = harness.open("events", &server.url("/form.html"));
    harness.wait_loaded(tab, "/form.html");
    let enabled = harness.client.devtools(tab, "Runtime.enable", json!({}));
    assert!(enabled.wait(DEADLINE).is_ok());
    assert_eq!(
        harness
            .client
            .navigate(tab, server.url("/app.html"))
            .wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_for("the console warning of the failing request", |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::Console { tab: t, level: ConsoleLevel::Warning, text, .. })
                if *t == tab && text.contains("prices failed")
        )
    });
    harness.wait_for("a Runtime.exceptionThrown event", |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::DevTools { tab: t, method, params })
                if *t == tab && method == "Runtime.exceptionThrown"
                    && params.to_string().contains("TypeError")
        )
    });
    harness.shutdown();
}

#[test]
fn a_method_that_never_answers_times_out_and_the_host_stays_responsive() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = harness.open("timeout", &server.url("/form.html"));
    harness.wait_loaded(tab, "/form.html");

    let stuck = harness.client.devtools(
        tab,
        "Runtime.evaluate",
        json!({"expression": "new Promise(() => {})", "awaitPromise": true}),
    );
    let outcome = stuck.wait(DEVTOOLS_METHOD_TIMEOUT + DEADLINE);
    assert_eq!(
        outcome,
        Err(CommandError::Timeout {
            method: "Runtime.evaluate".to_string(),
            after_ms: DEVTOOLS_METHOD_TIMEOUT.as_millis() as u64,
        })
    );
    assert_eq!(harness.evaluate(tab, "6 * 7"), json!(42));
    harness.shutdown();
}

#[test]
fn a_method_for_a_closed_tab_is_refused_naming_the_tab() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = harness.open("closed", &server.url("/form.html"));
    let other = harness.open("closed", &server.url("/form.html"));
    harness.wait_loaded(tab, "/form.html");
    assert_eq!(
        harness.client.close_tab(tab).wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_for("the tab to close", |event| {
        matches!(event, ClientEvent::Browser(Event::Closed { tab: closed }) if *closed == tab)
    });
    assert!(harness.client.tab_status(tab).is_none());
    assert!(harness.client.host_pid().is_some());
    let after = harness
        .client
        .devtools(tab, "Runtime.evaluate", json!({"expression": "1"}));
    let error = after.wait(DEADLINE).unwrap_err();
    assert_eq!(error, CommandError::TabClosed { tab });
    assert!(
        error
            .to_string()
            .contains(&format!("tab {} is closed", tab.0))
    );

    harness.wait_loaded(other, "/form.html");
    assert_eq!(harness.evaluate(other, "1 + 1"), json!(2));
    harness.shutdown();
}
