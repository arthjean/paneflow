//! `paneflow watch` - stream lifecycle events from the running instance (EP-002
//! US-007).
//!
//! Opens a persistent `events.subscribe` connection and prints each pushed event
//! as a line of JSONL, until the server closes the stream or the user interrupts
//! with Ctrl-C (a clean stop, exit 0). This is the in-pane conductor's read
//! channel: the same push an external orchestrator gets over the raw socket,
//! reached through the CLI.

use paneflow_ipc_client::IpcClient;
use serde_json::{Value, json};

use super::selector::resolve_target;
use super::{CliError, EXIT_OK};

/// `paneflow watch [--surface <sel>] [--type <t>]...`.
pub fn watch(client: &IpcClient, surface: Option<&str>, types: &[String]) -> Result<i32, CliError> {
    let mut params = serde_json::Map::new();
    if let Some(sel) = surface {
        // Force EXIT_TARGET for any resolution failure (no instance OR no match)
        // so "is Paneflow running?" surfaces as a target error per US-007 AC3.
        let id = resolve_target(client, sel).map_err(|e| CliError::target(e.message))?;
        params.insert("surfaces".into(), json!([id]));
    }
    if !types.is_empty() {
        params.insert("types".into(), json!(types));
    }

    let socket = paneflow_ipc_client::resolve_socket_path().ok_or_else(|| {
        CliError::target(
            "cannot locate the IPC socket; is Paneflow running? \
             (set PANEFLOW_SOCKET_PATH if you launched the CLI outside a Paneflow pane)",
        )
    })?;

    // A watch runs until interrupted; treat Ctrl-C as a clean stop (exit 0). The
    // dropped socket lets the server reap the subscription on its next write.
    let _ = ctrlc::set_handler(|| std::process::exit(EXIT_OK));

    let mut stream_error = None;
    match paneflow_ipc_client::subscribe_stream(&socket, Value::Object(params), |line| {
        if let Some(err) = paneflow_ipc_client::jsonrpc_error_message(line) {
            stream_error = Some(err);
            return false;
        }
        println!("{line}");
        true
    }) {
        Ok(()) => {
            if let Some(err) = stream_error {
                Err(CliError::target(format!("watch failed: {err}")))
            } else {
                Ok(EXIT_OK)
            }
        }
        Err(e) => Err(CliError::target(format!(
            "watch failed: {e}; is Paneflow running?"
        ))),
    }
}
