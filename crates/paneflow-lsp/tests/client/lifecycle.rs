use std::time::Duration;

use paneflow_lsp::client::directory_uri;
use paneflow_lsp::protocol::PositionEncoding;
use paneflow_lsp::{ClientEvent, ServerKey, ServerState};
use serde_json::json;

use crate::support::{count, fixture, messages, method_order, spawns};

pub fn initialize_carries_the_root_and_the_capabilities_then_initialized_precedes_documents() {
    let f = fixture();
    let key = f.key("fake");
    f.client.open_document(
        key.clone(),
        f.spec("normal", None),
        f.document("a.rs", "fn a() {}\n"),
    );
    let entries = f.wait_for_log("a didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 1
    });
    let initialize = messages(&entries, "initialize").next().unwrap();
    let params = &initialize["params"];
    let root_uri = directory_uri(&f.root).unwrap();
    assert_eq!(params["rootUri"], json!(root_uri));
    assert_eq!(
        params["workspaceFolders"],
        json!([{"uri": root_uri, "name": "project"}])
    );
    let tokens = &params["capabilities"]["textDocument"]["semanticTokens"];
    assert_eq!(tokens["formats"], json!(["relative"]));
    assert_eq!(tokens["augmentsSyntaxTokens"], json!(true));
    assert_eq!(tokens["multilineTokenSupport"], json!(false));
    assert_eq!(tokens["overlappingTokenSupport"], json!(false));
    assert_eq!(
        params["capabilities"]["workspace"]["semanticTokens"]["refreshSupport"],
        json!(true)
    );
    assert_eq!(
        params["capabilities"]["general"]["positionEncodings"],
        json!(["utf-8", "utf-16"])
    );
    assert_eq!(
        method_order(&entries)[..3],
        ["initialize", "initialized", "textDocument/didOpen"]
    );
    let negotiated = f.client.negotiated(&key).unwrap();
    assert_eq!(negotiated.encoding, PositionEncoding::Utf8);
    let legend = negotiated.semantic_tokens.unwrap().legend;
    assert_eq!(legend.token_types, vec!["function", "variable"]);
    assert_eq!(legend.token_modifiers, vec!["declaration"]);
    assert_eq!(f.client.state(&key), Some(ServerState::Running));
}

pub fn a_second_document_of_the_same_server_and_root_starts_no_second_process() {
    let f = fixture();
    let key = f.key("fake");
    f.client
        .open_document(key.clone(), f.spec("normal", None), f.document("a.rs", "a"));
    f.client
        .open_document(key, f.spec("normal", None), f.document("b.rs", "b"));
    let entries = f.wait_for_log("two didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 2
    });
    assert_eq!(spawns(&entries).len(), 1);
}

pub fn closing_the_last_document_sends_shutdown_then_exit() {
    let f = fixture();
    let key = f.key("fake");
    let first =
        f.client
            .open_document(key.clone(), f.spec("normal", None), f.document("a.rs", "a"));
    let second =
        f.client
            .open_document(key.clone(), f.spec("normal", None), f.document("b.rs", "b"));
    f.wait_for_log("two didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 2
    });
    f.client.close_document(first);
    f.wait_for_log("a didClose", |entries| {
        count(entries, "textDocument/didClose") == 1
    });
    assert_eq!(count(&f.entries(), "shutdown"), 0);
    f.client.close_document(second);
    let stopped = f.wait_for_event("the server stopping", |event| {
        matches!(event, ClientEvent::Stopped { .. })
    });
    assert_eq!(
        stopped,
        ClientEvent::Stopped {
            key: key.clone(),
            killed: false
        }
    );
    let order = method_order(&f.entries());
    assert_eq!(
        &order[order.len() - 3..],
        ["textDocument/didClose", "shutdown", "exit"]
    );
    assert_eq!(f.client.state(&key), Some(ServerState::Stopped));
}

pub fn a_server_still_alive_after_exit_is_killed() {
    let f = fixture();
    let key = f.key("fake");
    let document = f.client.open_document(
        key.clone(),
        f.spec("linger-after-exit", None),
        f.document("a.rs", "a"),
    );
    f.wait_for_log("a didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 1
    });
    f.client.close_document(document);
    let stopped = f.wait_for_event("the server stopping", |event| {
        matches!(event, ClientEvent::Stopped { .. })
    });
    assert_eq!(stopped, ClientEvent::Stopped { key, killed: true });
    assert_eq!(count(&f.entries(), "exit"), 1);
}

pub fn quitting_sends_shutdown_and_exit_to_every_running_server() {
    let f = fixture();
    let other_root = f.root.with_file_name("other");
    std::fs::create_dir_all(&other_root).unwrap();
    let first = f.key("fake");
    let second = ServerKey {
        server_id: "fake".to_string(),
        root: other_root.clone(),
    };
    f.client
        .open_document(first, f.spec("normal", None), f.document("a.rs", "a"));
    let mut document = f.document("b.rs", "b");
    document.path = other_root.join("b.rs");
    f.client
        .open_document(second, f.spec("normal", None), document);
    f.wait_for_log("two didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 2
    });
    f.client.shutdown_all(Duration::from_secs(10));
    let entries = f.wait_for_log("two exits", |entries| count(entries, "exit") == 2);
    let pids = spawns(&entries);
    assert_eq!(pids.len(), 2);
    for pid in pids {
        let methods: Vec<&str> = entries
            .iter()
            .filter(|entry| entry["pid"] == pid && entry["event"] == "message")
            .filter_map(|entry| entry["message"]["method"].as_str())
            .collect();
        assert_eq!(&methods[methods.len() - 2..], ["shutdown", "exit"], "{pid}");
    }
}

fn crash_twice(scenario: &str) {
    let f = fixture();
    let key = f.key("fake");
    f.client
        .open_document(key.clone(), f.spec(scenario, None), f.document("a.rs", "a"));
    f.client
        .open_document(key.clone(), f.spec(scenario, None), f.document("b.rs", "b"));
    let state = f.wait_for_state(&key, |state| matches!(state, ServerState::Crashed { .. }));
    let ServerState::Crashed { stderr } = state else {
        unreachable!()
    };
    assert!(
        stderr
            .last()
            .is_some_and(|line| line.starts_with("fatal: the fake server")),
        "{stderr:?}"
    );
    let entries = f.entries();
    let pids = spawns(&entries);
    assert_eq!(pids.len(), 2, "one automatic restart, then none");
    let reopened = entries
        .iter()
        .filter(|entry| entry["pid"] == pids[1])
        .any(|entry| entry["message"]["method"] == "textDocument/didOpen");
    assert!(reopened, "the restarted server gets its documents again");
    f.client
        .open_document(key.clone(), f.spec(scenario, None), f.document("c.rs", "c"));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(spawns(&f.entries()).len(), 2);
    assert!(matches!(
        f.client.state(&key),
        Some(ServerState::Crashed { .. })
    ));
}

pub fn a_first_unexpected_exit_restarts_once_and_a_second_leaves_it_crashed() {
    crash_twice("crash-on-open");
}

pub fn a_first_protocol_error_restarts_once_and_a_second_leaves_it_crashed() {
    crash_twice("garbage-on-open");
}

pub fn an_initialize_error_stops_the_server_in_failed_with_its_message() {
    let f = fixture();
    let key = f.key("fake");
    f.client.open_document(
        key.clone(),
        f.spec("init-error", None),
        f.document("a.rs", "a"),
    );
    let state = f.wait_for_state(&key, |state| matches!(state, ServerState::Failed { .. }));
    assert_eq!(
        state,
        ServerState::Failed {
            message: "cannot index this root".to_string()
        }
    );
    assert_eq!(spawns(&f.entries()).len(), 1);
}

pub fn a_missing_command_enters_the_missing_state_and_starts_nothing() {
    let f = fixture();
    let key = f.key("ghost");
    let mut spec = f.spec("normal", None);
    spec.command = "paneflow-no-such-language-server".to_string();
    f.client
        .open_document(key.clone(), spec, f.document("a.rs", "a"));
    let state = f.wait_for_state(&key, |state| matches!(state, ServerState::Missing { .. }));
    assert_eq!(
        state,
        ServerState::Missing {
            command: "paneflow-no-such-language-server".to_string()
        }
    );
    assert!(f.entries().is_empty());
}

pub fn workspace_configuration_is_answered_from_the_entry_settings() {
    let f = fixture();
    let key = f.key("fake");
    let mut spec = f.spec("ask-configuration", None);
    spec.settings = json!({"fake": {"answer": 42}, "other": true});
    f.client.open_document(key, spec, f.document("a.rs", "a"));
    let entries = f.wait_for_log("the configuration answer", |entries| {
        entries
            .iter()
            .any(|entry| entry["message"]["id"] == "cfg-1")
    });
    let answer = entries
        .iter()
        .find(|entry| entry["message"]["id"] == "cfg-1")
        .unwrap();
    assert_eq!(
        answer["message"]["result"],
        json!([{"answer": 42}, {"fake": {"answer": 42}, "other": true}])
    );
}

pub fn a_reported_missing_server_is_in_the_missing_state_without_a_process() {
    let f = fixture();
    let key = f.key("zls");
    f.client.report_missing(key.clone(), "zls".to_string());
    let state = f.wait_for_state(&key, |state| matches!(state, ServerState::Missing { .. }));
    assert_eq!(
        state,
        ServerState::Missing {
            command: "zls".to_string()
        }
    );
    assert!(f.entries().is_empty());
}
