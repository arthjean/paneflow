use std::sync::Arc;

use paneflow_lsp::client::file_uri;
use paneflow_lsp::semantic_tokens::SemanticToken;
use paneflow_lsp::{ClientEvent, DocumentId, OpenDocument, ServerState, TextChange};
use ropey::Rope;
use serde_json::{json, Value};

use crate::fake_server::default_capabilities;
use crate::support::{
    capture_debug_logs, count, debug_lines_containing, executable_runs, fixture, messages,
    method_order, Fixture,
};

const TEXT: &str = "fn main() {}\n";

fn token(line: u32, start: u32, end: u32, token_type: u32) -> SemanticToken {
    SemanticToken {
        line,
        start,
        end,
        token_type,
        modifiers: 0,
    }
}

fn next_tokens(f: &Fixture) -> (DocumentId, u64, Arc<[SemanticToken]>) {
    match f.wait_for_event("semantic tokens", |event| {
        matches!(event, ClientEvent::Tokens { .. })
    }) {
        ClientEvent::Tokens {
            document,
            revision,
            tokens,
            ..
        } => (document, revision, tokens),
        _ => unreachable!(),
    }
}

fn open(f: &Fixture, scenario: &str, capabilities: Option<Value>) -> DocumentId {
    f.client.open_document(
        f.key("fake"),
        f.spec(scenario, capabilities),
        f.document("main.rs", TEXT),
    )
}

fn append_newline(f: &Fixture, id: DocumentId, text: &mut String) {
    let at = text.len();
    text.push('\n');
    f.client.change_document(
        id,
        vec![TextChange {
            start_byte: at,
            old_end_byte: at,
            text: "\n".to_string(),
        }],
        Rope::from_str(text),
    );
}

fn close_and_settle(f: &Fixture, id: DocumentId) -> Vec<Value> {
    f.client.close_document(id);
    f.wait_for_log("a shutdown", |entries| count(entries, "shutdown") == 1)
}

pub fn did_open_is_followed_by_a_full_request_whose_tokens_reach_the_document() {
    let f = fixture();
    let id = open(&f, "normal", None);
    let (document, revision, tokens) = next_tokens(&f);
    assert_eq!((document, revision), (id, 0));
    assert_eq!(&*tokens, &[token(0, 0, 2, 0)]);
    let entries = close_and_settle(&f, id);
    let order = method_order(&entries);
    let opened = order.iter().position(|m| m == "textDocument/didOpen");
    let requested = order
        .iter()
        .position(|m| m == "textDocument/semanticTokens/full");
    assert!(opened < requested && opened.is_some(), "{order:?}");
    let uri = file_uri(&f.root.join("main.rs")).unwrap();
    let full = messages(&entries, "textDocument/semanticTokens/full")
        .next()
        .unwrap();
    assert_eq!(full["params"], json!({"textDocument": {"uri": uri}}));
}

pub fn a_range_only_server_gets_a_range_request_for_the_visible_rows() {
    let f = fixture();
    let mut caps = default_capabilities();
    caps["semanticTokensProvider"] = json!({
        "legend": {"tokenTypes": ["function"], "tokenModifiers": []},
        "range": true
    });
    let id = open(&f, "normal", Some(caps));
    f.wait_for_log("a didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 1
    });
    f.client.set_visible_lines(id, 3..10);
    let (_, revision, _) = next_tokens(&f);
    assert_eq!(revision, 0);
    let entries = close_and_settle(&f, id);
    let ranges: Vec<&Value> = messages(&entries, "textDocument/semanticTokens/range").collect();
    assert_eq!(ranges.len(), 1);
    assert_eq!(
        ranges[0]["params"]["range"],
        json!({"start": {"line": 3, "character": 0}, "end": {"line": 10, "character": 0}})
    );
    assert_eq!(count(&entries, "textDocument/semanticTokens/full"), 0);
}

pub fn an_edit_on_a_delta_server_sends_one_delta_with_the_last_result_id() {
    let f = fixture();
    let id = open(&f, "normal", None);
    next_tokens(&f);
    let mut text = TEXT.to_string();
    append_newline(&f, id, &mut text);
    let (_, revision, tokens) = next_tokens(&f);
    assert_eq!(revision, 1);
    assert_eq!(&*tokens, &[token(0, 0, 2, 0), token(0, 3, 7, 1)]);
    let entries = close_and_settle(&f, id);
    let deltas: Vec<&Value> =
        messages(&entries, "textDocument/semanticTokens/full/delta").collect();
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0]["params"]["previousResultId"], json!("1"));
    assert_eq!(count(&entries, "textDocument/semanticTokens/full"), 1);
}

pub fn an_edit_on_a_server_without_delta_sends_one_full_request() {
    let f = fixture();
    let mut caps = default_capabilities();
    caps["semanticTokensProvider"]["full"] = json!(true);
    let id = open(&f, "normal", Some(caps));
    next_tokens(&f);
    let mut text = TEXT.to_string();
    append_newline(&f, id, &mut text);
    let (_, revision, _) = next_tokens(&f);
    assert_eq!(revision, 1);
    let entries = close_and_settle(&f, id);
    assert_eq!(count(&entries, "textDocument/semanticTokens/full"), 2);
    assert_eq!(count(&entries, "textDocument/semanticTokens/full/delta"), 0);
}

pub fn edits_during_a_request_send_exactly_one_follow_up_and_the_stale_answer_is_never_delivered() {
    let f = fixture();
    let id = open(&f, "hold-until-two-changes", None);
    f.wait_for_log("a held full request", |entries| {
        count(entries, "textDocument/semanticTokens/full") == 1
    });
    let mut text = TEXT.to_string();
    append_newline(&f, id, &mut text);
    append_newline(&f, id, &mut text);
    let (_, revision, tokens) = next_tokens(&f);
    assert_eq!(
        revision, 2,
        "the answer computed for revision 0 must not be delivered"
    );
    assert_eq!(&*tokens, &[token(0, 0, 2, 0), token(0, 3, 7, 1)]);
    let entries = close_and_settle(&f, id);
    assert_eq!(count(&entries, "textDocument/semanticTokens/full"), 1);
    assert_eq!(
        count(&entries, "textDocument/semanticTokens/full/delta"),
        1,
        "two edits during one request give exactly one follow-up"
    );
}

pub fn a_refresh_requests_every_open_document_of_that_server_again() {
    let f = fixture();
    let spec = f.spec("refresh-after-two", None);
    let first = f
        .client
        .open_document(f.key("fake"), spec.clone(), f.document("a.rs", TEXT));
    let second = f
        .client
        .open_document(f.key("fake"), spec, f.document("b.rs", TEXT));
    let requests = |entries: &[Value]| {
        count(entries, "textDocument/semanticTokens/full")
            + count(entries, "textDocument/semanticTokens/full/delta")
    };
    let entries = f.wait_for_log("a second round of token requests", |entries| {
        requests(entries) == 4
    });
    for name in ["a.rs", "b.rs"] {
        let uri = file_uri(&f.root.join(name)).unwrap();
        let of_document = |method| {
            messages(&entries, method)
                .filter(|message| message["params"]["textDocument"]["uri"] == json!(uri))
                .count()
        };
        assert_eq!(
            of_document("textDocument/semanticTokens/full")
                + of_document("textDocument/semanticTokens/full/delta"),
            2,
            "{name} is requested again after the refresh"
        );
    }
    let answered = entries.iter().any(|entry| {
        entry["message"]["id"] == json!("refresh-1") && entry["message"].get("result").is_some()
    });
    assert!(answered, "the refresh request gets a result");
    f.client.close_document(first);
    f.client.close_document(second);
}

pub fn closing_a_document_with_a_request_in_flight_sends_cancel_request_for_it() {
    let f = fixture();
    let id = open(&f, "hold-tokens", None);
    let entries = f.wait_for_log("a held full request", |entries| {
        count(entries, "textDocument/semanticTokens/full") == 1
    });
    let request = messages(&entries, "textDocument/semanticTokens/full")
        .next()
        .unwrap()["id"]
        .clone();
    let entries = close_and_settle(&f, id);
    let cancels: Vec<&Value> = messages(&entries, "$/cancelRequest").collect();
    assert_eq!(cancels.len(), 1);
    assert_eq!(cancels[0]["params"], json!({"id": request}));
    let order = method_order(&entries);
    let cancel = order.iter().position(|m| m == "$/cancelRequest");
    let close = order.iter().position(|m| m == "textDocument/didClose");
    assert!(cancel < close, "{order:?}");
}

pub fn an_error_answer_delivers_nothing_and_the_next_edit_requests_again() {
    let f = fixture();
    let id = open(&f, "tokens-error", None);
    f.wait_for_log("the failed full request", |entries| {
        count(entries, "textDocument/semanticTokens/full") == 1
    });
    let mut text = TEXT.to_string();
    append_newline(&f, id, &mut text);
    let (_, revision, tokens) = next_tokens(&f);
    assert_eq!(
        revision, 1,
        "the error gave no tokens, so the previous ones stay"
    );
    assert_eq!(&*tokens, &[token(0, 0, 2, 0)]);
    let entries = close_and_settle(&f, id);
    assert_eq!(count(&entries, "textDocument/semanticTokens/full"), 2);
}

pub fn a_delta_beyond_the_cached_tokens_discards_them_and_requests_all_tokens() {
    let f = fixture();
    let id = open(&f, "delta-beyond", None);
    next_tokens(&f);
    let mut text = TEXT.to_string();
    append_newline(&f, id, &mut text);
    let (_, revision, tokens) = next_tokens(&f);
    assert_eq!(revision, 1);
    assert_eq!(&*tokens, &[token(0, 0, 2, 0)]);
    let entries = close_and_settle(&f, id);
    assert_eq!(count(&entries, "textDocument/semanticTokens/full/delta"), 1);
    assert_eq!(count(&entries, "textDocument/semanticTokens/full"), 2);
}

pub fn a_server_without_semantic_tokens_gets_no_request_and_one_debug_line() {
    capture_debug_logs();
    let f = fixture();
    let mut caps = default_capabilities();
    caps.as_object_mut()
        .unwrap()
        .remove("semanticTokensProvider");
    let id = open(&f, "normal", Some(caps));
    f.wait_for_log("a didOpen", |entries| {
        count(entries, "textDocument/didOpen") == 1
    });
    let uri = file_uri(&f.root.join("main.rs")).unwrap();
    assert_eq!(
        debug_lines_containing(&format!("offers no semantic tokens for {uri}")),
        1
    );
    let entries = close_and_settle(&f, id);
    assert_eq!(count(&entries, "textDocument/semanticTokens/full"), 0);
}

pub fn restart_after_a_crash_starts_the_server_again_and_reopens_its_documents() {
    let f = fixture();
    let key = f.key("fake");
    let id = open(&f, "crash-on-open", None);
    f.wait_for_state(&key, |state| matches!(state, ServerState::Crashed { .. }));
    f.client.restart(key.clone());
    f.wait_for_state(&key, |state| matches!(state, ServerState::Starting));
    let entries = f.wait_for_log("a third didOpen", |entries| {
        count(entries, "textDocument/didOpen") >= 3
    });
    assert!(crate::support::spawns(&entries).len() >= 3);
    f.client.close_document(id);
}

const ZIG_SAMPLE: &str = "const std = @import(\"std\");\nconst Point = struct {\n    x: f32,\n    fn len(self: Point) f32 {\n        return self.x;\n    }\n};\npub fn main() void {\n    const p = Point{ .x = 1.5 };\n    std.debug.print(\"{d}\\n\", .{p.len()});\n}\n";

const ZLS_RECORDING: &str = include_str!("zls_sample_zig.json");
const RECORD_ZLS_ENV: &str = "PANEFLOW_RECORD_ZLS";
const FIELDS: usize = 5;

fn zig_document(f: &Fixture) -> OpenDocument {
    let path = f.root.join("sample.zig");
    std::fs::write(&path, ZIG_SAMPLE).unwrap();
    OpenDocument {
        path,
        language_id: "zig".to_string(),
        text: Rope::from_str(ZIG_SAMPLE),
    }
}

fn version_of(program: &std::path::Path, argument: &str) -> Option<String> {
    let output = std::process::Command::new(program)
        .arg(argument)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn first_answer<'a>(entries: &'a [Value], method: &str) -> Option<&'a Value> {
    let id = messages(entries, method).next()?.get("id")?;
    entries
        .iter()
        .filter(|entry| entry["event"] == "server")
        .map(|entry| &entry["message"])
        .find(|message| message.get("method").is_none() && message.get("id") == Some(id))?
        .get("result")
}

fn tokens_with_legend(
    f: &Fixture,
) -> (
    Arc<[SemanticToken]>,
    Arc<paneflow_lsp::protocol::TokenLegend>,
) {
    match f.wait_for_event("semantic tokens", |event| {
        matches!(event, ClientEvent::Tokens { .. })
    }) {
        ClientEvent::Tokens { tokens, legend, .. } => (tokens, legend),
        _ => unreachable!(),
    }
}

fn role_of<'a>(
    tokens: &[SemanticToken],
    legend: &'a paneflow_lsp::protocol::TokenLegend,
    line: u32,
    word: &str,
    nth: usize,
) -> &'a str {
    let text = ZIG_SAMPLE.lines().nth(line as usize).unwrap();
    let start = text
        .match_indices(word)
        .nth(nth)
        .unwrap_or_else(|| panic!("{word} is not on line {line}"))
        .0 as u32;
    let token = tokens
        .iter()
        .find(|token| token.line == line && token.start == start)
        .unwrap_or_else(|| panic!("no token starts at {word} on line {line}"));
    assert_eq!(
        token.end,
        start + word.len() as u32,
        "{word} on line {line}"
    );
    &legend.token_types[token.token_type as usize]
}

pub fn zls_reports_whether_it_returns_semantic_tokens_and_its_encoding() {
    let program = match paneflow_lsp::resolve::resolve_command("zls") {
        Ok(program) if executable_runs(&program) => program,
        _ => {
            println!("skipped: zls is not on PATH or does not run");
            return;
        }
    };
    let zls_version = version_of(&program, "--version");
    let zig_version = paneflow_lsp::resolve::resolve_command("zig")
        .ok()
        .and_then(|zig| version_of(&zig, "version"));
    let f = fixture();
    let isolated = f._dir.path().join("zls-home");
    std::fs::create_dir_all(&isolated).unwrap();
    let key = f.key("zls");
    let mut spec = f.spec("relay", None);
    spec.env.extend([
        (
            crate::fake_server::RELAY_ENV.to_string(),
            program.to_string_lossy().into_owned(),
        ),
        (
            "XDG_CACHE_HOME".to_string(),
            isolated.to_string_lossy().into_owned(),
        ),
        (
            "XDG_CONFIG_HOME".to_string(),
            isolated.to_string_lossy().into_owned(),
        ),
    ]);
    let id = f.client.open_document(key.clone(), spec, zig_document(&f));
    let state = f.wait_for_state(&key, |state| !matches!(state, ServerState::Starting));
    assert!(
        matches!(state, ServerState::Running),
        "zls did not start: {state:?}"
    );
    let negotiated = f.client.negotiated(&key).unwrap();
    let encoding = negotiated.encoding.as_str();
    if negotiated.semantic_tokens.is_none() {
        println!("zls accepted position encoding {encoding} and offers no semantic tokens");
        f.client.close_document(id);
        return;
    }
    let (tokens, _) = tokens_with_legend(&f);
    let entries = f.wait_for_log("the zls answer to semanticTokens/full", |entries| {
        first_answer(entries, "textDocument/semanticTokens/full").is_some()
    });
    let capabilities = first_answer(&entries, "initialize").unwrap()["capabilities"].clone();
    let answer = first_answer(&entries, "textDocument/semanticTokens/full").unwrap();
    let data: Vec<u32> = serde_json::from_value(answer["data"].clone()).unwrap();
    assert_eq!(data.len() % FIELDS, 0, "zls sent a partial token");
    assert_eq!(
        tokens.len(),
        data.len() / FIELDS,
        "the decoder dropped a token zls sent"
    );
    println!(
        "zls {} with zig {} accepted position encoding {encoding} and returned {} semantic tokens for sample.zig",
        zls_version.as_deref().unwrap_or("unknown"),
        zig_version.as_deref().unwrap_or("unknown"),
        tokens.len()
    );
    let live = json!({
        "zls": zls_version,
        "zig": zig_version,
        "capabilities": capabilities,
        "data": data,
    });
    let recording: Value = serde_json::from_str(ZLS_RECORDING).unwrap();
    if std::env::var_os(RECORD_ZLS_ENV).is_some() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/client/zls_sample_zig.json");
        let mut body = serde_json::to_string_pretty(&live).unwrap();
        body.push('\n');
        std::fs::write(&path, body).unwrap();
        println!("recorded the zls answer in {}", path.display());
    } else if recording["zls"] == live["zls"] && recording["zig"] == live["zig"] {
        assert_eq!(
            recording["capabilities"]["semanticTokensProvider"],
            live["capabilities"]["semanticTokensProvider"],
            "the zls legend drifted from its recording"
        );
        assert_eq!(
            recording["data"], live["data"],
            "zls answered sample.zig differently from its recording"
        );
    } else {
        println!(
            "the recording comes from zls {} with zig {}: rerun with {RECORD_ZLS_ENV}=1 to record this version",
            recording["zls"], recording["zig"]
        );
    }
    f.client.close_document(id);
}

pub fn the_recorded_zls_answer_reaches_the_document_with_every_token_and_its_role() {
    let recording: Value = serde_json::from_str(ZLS_RECORDING).unwrap();
    let data: Vec<u32> = serde_json::from_value(recording["data"].clone()).unwrap();
    assert!(!data.is_empty(), "the zls recording holds no tokens");
    let f = fixture();
    let mut spec = f.spec("normal", Some(recording["capabilities"].clone()));
    spec.env.push((
        crate::fake_server::TOKENS_ENV.to_string(),
        recording["data"].to_string(),
    ));
    let id = f.client.open_document(f.key("zls"), spec, zig_document(&f));
    let (tokens, legend) = tokens_with_legend(&f);
    assert_eq!(tokens.len(), data.len() / FIELDS);
    assert_eq!(role_of(&tokens, &legend, 1, "Point", 0), "struct");
    assert_eq!(role_of(&tokens, &legend, 2, "x", 0), "property");
    assert_eq!(role_of(&tokens, &legend, 3, "len", 0), "method");
    assert_eq!(role_of(&tokens, &legend, 3, "self", 0), "parameter");
    assert_eq!(role_of(&tokens, &legend, 4, "x", 0), "property");
    assert_eq!(role_of(&tokens, &legend, 7, "main", 0), "function");
    assert_eq!(role_of(&tokens, &legend, 8, "p", 0), "variable");
    assert_eq!(role_of(&tokens, &legend, 9, "\"{d}\\n\"", 0), "string");
    f.client.close_document(id);
}
