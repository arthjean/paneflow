use paneflow_lsp::client::file_uri;
use paneflow_lsp::protocol::PositionEncoding;
use paneflow_lsp::{LanguageClient, OpenDocument, ServerKey, ServerSpec, ServerState, TextChange};
use ropey::Rope;
use serde_json::{json, Value};

use crate::fake_server::default_capabilities;
use crate::support::{count, executable_runs, fixture, messages};

fn caps_with(encoding: &str, change: u64) -> Value {
    let mut caps = default_capabilities();
    caps["positionEncoding"] = json!(encoding);
    caps["textDocumentSync"] = json!({"openClose": true, "change": change});
    caps
}

pub fn did_open_sends_the_uri_the_language_id_version_one_and_the_full_text() {
    let f = fixture();
    let document = f.document("main.rs", "fn main() {}\n");
    let uri = file_uri(&document.path).unwrap();
    f.client
        .open_document(f.key("fake"), f.spec("normal", None), document);
    let entries = f.wait_for_log("a didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 1
    });
    let open = messages(&entries, "textDocument/didOpen").next().unwrap();
    assert_eq!(
        open["params"]["textDocument"],
        json!({"uri": uri, "languageId": "rust", "version": 1, "text": "fn main() {}\n"})
    );
}

fn edited(encoding: &str) -> Value {
    let f = fixture();
    let before = "let 🦀 = a;\nb\n";
    let id = f.client.open_document(
        f.key("fake"),
        f.spec("normal", Some(caps_with(encoding, 2))),
        f.document("a.rs", before),
    );
    f.wait_for_log("a didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 1
    });
    let crab_end = "let 🦀".len();
    let changes = vec![
        TextChange {
            start_byte: crab_end + 3,
            old_end_byte: crab_end + 4,
            text: "xyz".to_string(),
        },
        TextChange {
            start_byte: 0,
            old_end_byte: 0,
            text: "// top\n".to_string(),
        },
    ];
    let after = "// top\nlet 🦀 = xyz;\nb\n";
    f.client.change_document(id, changes, Rope::from_str(after));
    let entries = f.wait_for_log("a didChange", |entries| {
        count(entries, "textDocument/didChange") == 1
    });
    let params = messages(&entries, "textDocument/didChange").next().unwrap()["params"].clone();
    params
}

pub fn an_edit_batch_sends_one_incremental_did_change_in_utf16_counting_astral_characters_as_two_units(
) {
    let params = edited("utf-16");
    assert_eq!(params["textDocument"]["version"], json!(2));
    let changes = params["contentChanges"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(
        changes[0],
        json!({"range": {"start": {"line": 0, "character": 9}, "end": {"line": 0, "character": 10}}, "text": "xyz"})
    );
    assert_eq!(
        changes[1],
        json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "text": "// top\n"})
    );
}

pub fn an_edit_batch_in_utf8_uses_byte_columns() {
    let params = edited("utf-8");
    let changes = params["contentChanges"].as_array().unwrap();
    assert_eq!(changes[0]["range"]["start"]["character"], json!(11));
    assert_eq!(changes[0]["range"]["end"]["character"], json!(12));
}

pub fn successive_did_changes_increase_the_version_by_one() {
    let f = fixture();
    let id = f.client.open_document(
        f.key("fake"),
        f.spec("normal", None),
        f.document("a.rs", "ab"),
    );
    f.wait_for_log("a didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 1
    });
    f.client.change_document(
        id,
        vec![TextChange {
            start_byte: 2,
            old_end_byte: 2,
            text: "c".into(),
        }],
        Rope::from_str("abc"),
    );
    f.client.change_document(
        id,
        vec![TextChange {
            start_byte: 3,
            old_end_byte: 3,
            text: "d".into(),
        }],
        Rope::from_str("abcd"),
    );
    let entries = f.wait_for_log("two didChange", |entries| {
        count(entries, "textDocument/didChange") == 2
    });
    let versions: Vec<Value> = messages(&entries, "textDocument/didChange")
        .map(|change| change["params"]["textDocument"]["version"].clone())
        .collect();
    assert_eq!(versions, vec![json!(2), json!(3)]);
}

pub fn a_full_sync_server_gets_the_full_text_on_every_edit() {
    let f = fixture();
    let id = f.client.open_document(
        f.key("fake"),
        f.spec("normal", Some(caps_with("utf-16", 1))),
        f.document("a.rs", "ab"),
    );
    f.wait_for_log("a didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 1
    });
    f.client.change_document(
        id,
        vec![TextChange {
            start_byte: 1,
            old_end_byte: 2,
            text: "Z".into(),
        }],
        Rope::from_str("aZ"),
    );
    let entries = f.wait_for_log("a didChange", |entries| {
        count(entries, "textDocument/didChange") == 1
    });
    let change = messages(&entries, "textDocument/didChange").next().unwrap();
    assert_eq!(change["params"]["contentChanges"], json!([{"text": "aZ"}]));
}

pub fn a_server_without_sync_gets_no_did_open_and_the_document_is_unsupported() {
    let f = fixture();
    let mut caps = default_capabilities();
    caps.as_object_mut().unwrap().remove("textDocumentSync");
    let key = f.key("fake");
    let id = f.client.open_document(
        key.clone(),
        f.spec("normal", Some(caps)),
        f.document("a.rs", "a"),
    );
    f.wait_for_event("the document marked unsupported", |event| {
        matches!(event, paneflow_lsp::ClientEvent::Unsupported { document } if *document == id)
    });
    assert!(f.client.is_unsupported(id));
    assert_eq!(count(&f.entries(), "textDocument/didOpen"), 0);
    assert_eq!(f.client.state(&key), Some(ServerState::Running));
}

pub fn a_replaced_document_sends_one_full_text_did_change() {
    let f = fixture();
    let id = f.client.open_document(
        f.key("fake"),
        f.spec("normal", None),
        f.document("a.rs", "old"),
    );
    f.wait_for_log("a didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 1
    });
    f.client
        .replace_document(id, Rope::from_str("new text from disk\n"));
    let entries = f.wait_for_log("a didChange", |entries| {
        count(entries, "textDocument/didChange") == 1
    });
    let change = messages(&entries, "textDocument/didChange").next().unwrap();
    assert_eq!(
        change["params"]["contentChanges"],
        json!([{"text": "new text from disk\n"}])
    );
    assert_eq!(change["params"]["textDocument"]["version"], json!(2));
}

pub fn closing_a_tab_sends_did_close_for_its_uri() {
    let f = fixture();
    let key = f.key("fake");
    let document = f.document("a.rs", "a");
    let uri = file_uri(&document.path).unwrap();
    let keep = f.client.open_document(
        key.clone(),
        f.spec("normal", None),
        f.document("keep.rs", "k"),
    );
    let id = f
        .client
        .open_document(key, f.spec("normal", None), document);
    f.wait_for_log("two didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 2
    });
    f.client.close_document(id);
    let entries = f.wait_for_log("a didClose", |entries| {
        count(entries, "textDocument/didClose") == 1
    });
    let close = messages(&entries, "textDocument/didClose").next().unwrap();
    assert_eq!(close["params"], json!({"textDocument": {"uri": uri}}));
    f.client.close_document(keep);
}

pub fn rust_analyzer_negotiates_a_position_encoding() {
    let program = match paneflow_lsp::resolve::resolve_command("rust-analyzer") {
        Ok(program) if executable_runs(&program) => program,
        _ => {
            println!("skipped: rust-analyzer is not on PATH or does not run");
            return;
        }
    };
    let dir = tempfile::tempdir().unwrap();
    let root = paneflow_lsp::trust::canonical_root(dir.path()).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let (sender, events) = std::sync::mpsc::channel();
    let sender = std::sync::Mutex::new(sender);
    let client = LanguageClient::start(std::sync::Arc::new(move |event| {
        let _ = sender.lock().unwrap().send(event);
    }))
    .unwrap();
    let key = ServerKey {
        server_id: "rust-analyzer".to_string(),
        root: root.clone(),
    };
    let spec = ServerSpec {
        command: program.to_string_lossy().into_owned(),
        args: Vec::new(),
        env: Vec::new(),
        initialization_options: None,
        settings: Value::Null,
    };
    let id = client.open_document(
        key.clone(),
        spec,
        OpenDocument {
            path: root.join("src/main.rs"),
            language_id: "rust".to_string(),
            text: Rope::from_str("fn main() {}\n"),
        },
    );
    loop {
        match events.recv_timeout(crate::support::OUTCOME_DEADLINE) {
            Ok(paneflow_lsp::ClientEvent::State {
                state: ServerState::Running,
                ..
            }) => break,
            Ok(paneflow_lsp::ClientEvent::State { state, .. })
                if !matches!(state, ServerState::Starting) =>
            {
                panic!("rust-analyzer did not start: {state:?}")
            }
            Ok(_) => {}
            Err(_) => panic!("rust-analyzer never finished initialize"),
        }
    }
    let negotiated = client.negotiated(&key).unwrap();
    println!(
        "rust-analyzer negotiated position encoding {}",
        negotiated.encoding.as_str()
    );
    assert!(matches!(
        negotiated.encoding,
        PositionEncoding::Utf8 | PositionEncoding::Utf16
    ));
    client.close_document(id);
    client.shutdown_all(std::time::Duration::from_secs(10));
}
