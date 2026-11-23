use std::fs::OpenOptions;
use std::io::{self, Write};
use std::time::Duration;

use paneflow_lsp::framing::{write_message, Frame, FrameReader};
use serde_json::{json, Value};

pub const SCENARIO_ENV: &str = "PANEFLOW_LSP_FAKE_SERVER";
pub const LOG_ENV: &str = "PANEFLOW_LSP_FAKE_LOG";
pub const CAPS_ENV: &str = "PANEFLOW_LSP_FAKE_CAPS";

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

pub fn run(scenario: &str) {
    let log = Log {
        path: std::env::var(LOG_ENV).unwrap_or_default(),
    };
    let pid = std::process::id();
    log.record(json!({"pid": pid, "event": "spawn"}));
    let capabilities = std::env::var(CAPS_ENV)
        .ok()
        .and_then(|caps| serde_json::from_str(&caps).ok())
        .unwrap_or_else(default_capabilities);
    let mut reader = FrameReader::new(io::stdin());
    let mut stdout = io::stdout();
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
