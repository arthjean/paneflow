#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

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
