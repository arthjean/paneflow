use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use paneflow_lsp::framing::{Frame, FrameReader, write_message};
use serde_json::{Value, json};

pub(crate) const FAKE_FLAG: &str = "--paneflow-fake-language-server";
pub(crate) const TOKENS_FILE: &str = "tokens.json";
pub(crate) const LOG_FILE: &str = "log.jsonl";
pub(crate) const ALLOW_FILE: &str = "allow";
const CRASHED_FILE: &str = "crashed";
pub(crate) const CRASH_STDERR: &str = "fatal: the fake language server crashed";
pub(crate) const INIT_ERROR: &str = "cannot index this root";
pub(crate) const LEGEND: [&str; 3] = ["property", "variable", "function"];

pub(crate) fn args(scenario: &str, state: &Path) -> Vec<String> {
    vec![
        FAKE_FLAG.to_string(),
        scenario.to_string(),
        state.to_string_lossy().into_owned(),
    ]
}

pub(crate) fn write_tokens(state: &Path, responses: &[Vec<u32>]) {
    let body = serde_json::to_vec(responses).expect("tokens json");
    std::fs::write(state.join(TOKENS_FILE), body).expect("write tokens");
}

pub(crate) fn logged_methods(state: &Path) -> Vec<String> {
    std::fs::read_to_string(state.join(LOG_FILE))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|entry| entry["method"].as_str().map(str::to_string))
        .collect()
}

#[ctor::ctor(unsafe, priority = late)]
fn serve_when_asked() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some(FAKE_FLAG) {
        return;
    }
    let scenario = args.get(2).cloned().unwrap_or_default();
    let state = PathBuf::from(args.get(3).cloned().unwrap_or_default());
    serve(&scenario, &state);
    std::process::exit(0);
}

fn record(state: &Path, message: &Value) {
    let mut line = serde_json::to_vec(message).unwrap_or_default();
    line.push(b'\n');
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join(LOG_FILE))
    {
        let _ = file.write_all(&line);
    }
}

fn capabilities(scenario: &str) -> Value {
    let mut capabilities = json!({
        "positionEncoding": "utf-8",
        "textDocumentSync": {"openClose": true, "change": 2},
        "semanticTokensProvider": {
            "legend": {"tokenTypes": LEGEND, "tokenModifiers": []},
            "full": true
        }
    });
    if scenario == "no-tokens"
        && let Some(object) = capabilities.as_object_mut()
    {
        object.remove("semanticTokensProvider");
    }
    capabilities
}

fn tokens_answer(state: &Path, answered: usize) -> Value {
    let responses: Vec<Vec<u32>> = std::fs::read(state.join(TOKENS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let data = responses
        .get(answered)
        .or_else(|| responses.last())
        .cloned()
        .unwrap_or_default();
    json!({"resultId": answered.to_string(), "data": data})
}

fn serve(scenario: &str, state: &Path) {
    let mut reader = FrameReader::new(io::stdin());
    let mut stdout = io::stdout();
    let mut answered = 0usize;
    while let Ok(frame) = reader.read_frame() {
        let Frame::Message(message) = frame else {
            continue;
        };
        record(state, &message);
        let id = message.get("id").cloned();
        let reply = match message["method"].as_str().unwrap_or_default() {
            "initialize" if scenario == "init-error" => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": INIT_ERROR}})
            }
            "initialize" => {
                json!({"jsonrpc": "2.0", "id": id, "result": {"capabilities": capabilities(scenario)}})
            }
            "textDocument/didOpen"
                if (scenario == "crash-until-allowed" && !state.join(ALLOW_FILE).exists())
                    || (scenario == "tokens-then-crash" && state.join(CRASHED_FILE).exists()) =>
            {
                eprintln!("{CRASH_STDERR}");
                std::process::exit(70);
            }
            "textDocument/didChange" if scenario == "tokens-then-crash" => {
                let _ = std::fs::write(state.join(CRASHED_FILE), b"");
                eprintln!("{CRASH_STDERR}");
                std::process::exit(70);
            }
            "textDocument/semanticTokens/full" => {
                let result = tokens_answer(state, answered);
                answered += 1;
                json!({"jsonrpc": "2.0", "id": id, "result": result})
            }
            "shutdown" => json!({"jsonrpc": "2.0", "id": id, "result": null}),
            "exit" => return,
            _ => continue,
        };
        let _ = write_message(&mut stdout, &reply);
    }
}
