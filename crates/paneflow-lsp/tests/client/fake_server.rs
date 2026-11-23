use std::fs::OpenOptions;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use paneflow_lsp::framing::{write_message, Frame, FrameReader};
use serde_json::{json, Value};

pub const SCENARIO_ENV: &str = "PANEFLOW_LSP_FAKE_SERVER";
pub const LOG_ENV: &str = "PANEFLOW_LSP_FAKE_LOG";
pub const CAPS_ENV: &str = "PANEFLOW_LSP_FAKE_CAPS";
pub const TOKENS_ENV: &str = "PANEFLOW_LSP_FAKE_TOKENS";
pub const RELAY_ENV: &str = "PANEFLOW_LSP_FAKE_RELAY";
pub const DEFAULT_TOKENS: [u32; 5] = [0, 0, 2, 0, 0];

pub fn default_capabilities() -> Value {
    json!({
        "positionEncoding": "utf-8",
        "textDocumentSync": {"openClose": true, "change": 2},
        "semanticTokensProvider": {
            "legend": {"tokenTypes": ["function", "variable"], "tokenModifiers": ["declaration"]},
            "full": {"delta": true},
            "range": true
        }
    })
}

#[derive(Clone)]
struct Log {
    path: String,
}

impl Log {
    fn record(&self, entry: Value) {
        let mut line = serde_json::to_vec(&entry).unwrap_or_default();
        line.push(b'\n');
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = file.write_all(&line);
        }
    }
}

fn relay(log: &Log, pid: u32) {
    let program = std::env::var(RELAY_ENV).unwrap_or_default();
    let Ok(mut child) = Command::new(&program)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        log.record(json!({"pid": pid, "event": "relay-failed", "program": program}));
        return;
    };
    let (Some(mut server_in), Some(server_out)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return;
    };
    let server_log = log.clone();
    let from_server = std::thread::spawn(move || {
        let mut reader = FrameReader::new(server_out);
        let mut stdout = io::stdout();
        loop {
            match reader.read_frame() {
                Ok(Frame::Message(message)) => {
                    server_log.record(json!({"pid": pid, "event": "server", "message": message}));
                    if write_message(&mut stdout, &message).is_err() {
                        return;
                    }
                }
                Ok(Frame::Invalid(_)) => {}
                Err(_) => return,
            }
        }
    });
    let mut reader = FrameReader::new(io::stdin());
    loop {
        match reader.read_frame() {
            Ok(Frame::Message(message)) => {
                log.record(json!({"pid": pid, "event": "message", "message": message}));
                if write_message(&mut server_in, &message).is_err() || message["method"] == "exit" {
                    break;
                }
            }
            Ok(Frame::Invalid(_)) => {}
            Err(_) => break,
        }
    }
    drop(server_in);
    let _ = child.wait();
    let _ = from_server.join();
    log.record(json!({"pid": pid, "event": "eof"}));
}

pub fn run(scenario: &str) {
    let log = Log {
        path: std::env::var(LOG_ENV).unwrap_or_default(),
    };
    let pid = std::process::id();
    log.record(json!({"pid": pid, "event": "spawn"}));
    if scenario == "relay" {
        relay(&log, pid);
        return;
    }
    let capabilities = std::env::var(CAPS_ENV)
        .ok()
        .and_then(|caps| serde_json::from_str(&caps).ok())
        .unwrap_or_else(default_capabilities);
    let tokens: Value = std::env::var(TOKENS_ENV)
        .ok()
        .and_then(|tokens| serde_json::from_str(&tokens).ok())
        .unwrap_or_else(|| json!(DEFAULT_TOKENS));
    let mut reader = FrameReader::new(io::stdin());
    let mut stdout = io::stdout();
    let mut answered = 0u64;
    let mut changes = 0u64;
    let mut held: Option<Value> = None;
    let mut refreshed = false;
    loop {
        let message = match reader.read_frame() {
            Ok(Frame::Message(message)) => message,
            Ok(Frame::Invalid(_)) => continue,
            Err(_) => {
                log.record(json!({"pid": pid, "event": "eof"}));
                return;
            }
        };
        log.record(json!({"pid": pid, "event": "message", "message": message}));
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let id = message.get("id").cloned();
        match method {
            "initialize" if scenario == "init-error" => {
                eprintln!("fatal: refusing to initialize");
                reply(
                    &mut stdout,
                    id,
                    Err(json!({"code": -32603, "message": "cannot index this root"})),
                );
            }
            "initialize" => reply(&mut stdout, id, Ok(json!({"capabilities": capabilities}))),
            "initialized" if scenario == "ask-configuration" => {
                let _ = write_message(
                    &mut stdout,
                    &json!({"jsonrpc": "2.0", "id": "cfg-1", "method": "workspace/configuration", "params": {"items": [{"section": "fake"}, {}]}}),
                );
            }
            "shutdown" => reply(&mut stdout, id, Ok(Value::Null)),
            "exit" if scenario == "linger-after-exit" => loop {
                std::thread::sleep(Duration::from_secs(1));
            },
            "exit" => std::process::exit(0),
            "textDocument/didChange" => {
                changes += 1;
                if scenario == "hold-until-two-changes" && changes == 2 {
                    if let Some(id) = held.take() {
                        answered += 1;
                        reply(
                            &mut stdout,
                            Some(id),
                            Ok(json!({"resultId": answered.to_string(), "data": tokens})),
                        );
                    }
                }
            }
            "textDocument/semanticTokens/full" | "textDocument/semanticTokens/range" => {
                let first = answered == 0 && held.is_none();
                if scenario == "hold-tokens" || (scenario == "hold-until-two-changes" && first) {
                    held = id;
                    continue;
                }
                if scenario == "tokens-error" && first {
                    answered += 1;
                    reply(
                        &mut stdout,
                        id,
                        Err(json!({"code": -32802, "message": "the server cancelled the request"})),
                    );
                    continue;
                }
                answered += 1;
                reply(
                    &mut stdout,
                    id,
                    Ok(json!({"resultId": answered.to_string(), "data": tokens})),
                );
                if scenario == "refresh-after-two" && answered == 2 && !refreshed {
                    refreshed = true;
                    let _ = write_message(
                        &mut stdout,
                        &json!({"jsonrpc": "2.0", "id": "refresh-1", "method": "workspace/semanticTokens/refresh"}),
                    );
                }
            }
            "textDocument/semanticTokens/full/delta" => {
                answered += 1;
                let edits = if scenario == "delta-beyond" {
                    json!([{"start": 999, "deleteCount": 0, "data": [0, 0, 1, 0, 0]}])
                } else {
                    json!([{"start": 5, "deleteCount": 0, "data": [0, 3, 4, 1, 0]}])
                };
                reply(
                    &mut stdout,
                    id,
                    Ok(json!({"resultId": answered.to_string(), "edits": edits})),
                );
            }
            "textDocument/didOpen" if scenario == "crash-on-open" => {
                eprintln!("fatal: the fake server crashed on open");
                std::process::exit(70);
            }
            "textDocument/didOpen" if scenario == "garbage-on-open" => {
                eprintln!("fatal: the fake server sent garbage");
                let _ = stdout.write_all(b"Bogus: 1\r\n\r\n");
                let _ = stdout.flush();
            }
            _ => {}
        }
    }
}

fn reply(stdout: &mut io::Stdout, id: Option<Value>, outcome: Result<Value, Value>) {
    let message = match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    };
    let _ = write_message(stdout, &message);
}
