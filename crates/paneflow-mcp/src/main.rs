#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]
//! paneflow-mcp - MCP (Model Context Protocol) stdio bridge for Paneflow.
//!
//! Lets an MCP-capable CLI agent (Claude Code, Codex, Gemini CLI, opencode)
//! running inside a Paneflow pane read the terminal output of ANY other
//! surface. It speaks MCP over stdin/stdout (the agent spawns it as a
//! subprocess) and proxies each tool call to Paneflow's local IPC socket.
//!
//! Tools (all READ-ONLY): `list_panes`, `read_pane`, `search_pane`. There is
//! deliberately no write/keystroke tool - the IPC scripting gate stays the
//! sole, opt-in write surface (PRD security decision).
//!
//! Module map:
//! - [`paneflow_ipc_client`] - socket path resolution + blocking JSON-RPC
//!   client (US-005), shared with the `paneflow` CLI.
//! - [`mcp`] - MCP stdio protocol loop (US-006)
//! - [`bridge`] - typed, scope-aware Paneflow IPC adapter
//! - [`tools`] - the three MCP tool adapters (US-006/007/008)
//! - [`resources`] - MCP resource convenience layer (US-014)
//! - [`output`] - untrusted terminal-output fencing
//! - [`resolve`] - name → surface_id resolution with disambiguation (US-009)

mod bridge;
mod mcp;
mod output;
mod resolve;
mod resources;
mod scope;
#[cfg(test)]
mod test_support;
mod tools;

use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(socket) = paneflow_ipc_client::resolve_socket_path() else {
        eprintln!(
            "paneflow-mcp: cannot locate the Paneflow IPC socket. \
             Set PANEFLOW_SOCKET_PATH (normally inherited from the Paneflow PTY) \
             or launch this bridge from inside a Paneflow pane."
        );
        return ExitCode::FAILURE;
    };

    let client = paneflow_ipc_client::IpcClient::new(socket);
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    let scope = match scope::BridgeScope::from_env() {
        Ok(scope) => scope,
        Err(error) => {
            eprintln!("paneflow-mcp: invalid read scope: {error}");
            return ExitCode::FAILURE;
        }
    };
    let bridge = bridge::Bridge::new(&client, scope);

    match mcp::serve(stdin, stdout, &bridge) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("paneflow-mcp: stdio loop terminated: {e}");
            ExitCode::FAILURE
        }
    }
}
