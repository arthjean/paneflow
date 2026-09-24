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

use paneflow_ipc_client::host_control::{resolve_control_target, ControlTarget, HostTransport};
use paneflow_ipc_client::IpcTransport;

const CLIENT_NAME: &str = "paneflow-mcp";

fn main() -> ExitCode {
    let Some(target) = resolve_control_target(
        paneflow_home::isolated_ipc_endpoint_for_current_home(),
        paneflow_home::host_endpoint_path_for_current_home(),
        paneflow_home::reserved_host_endpoint(),
    ) else {
        eprintln!(
            "paneflow-mcp: cannot locate a Paneflow controller socket or local host endpoint. \
             Set PANEFLOW_SOCKET_PATH or PANEFLOW_HOST_ENDPOINT (normally inherited from the \
             Paneflow PTY) or launch this bridge from inside a Paneflow pane."
        );
        return ExitCode::FAILURE;
    };

    let hosted = matches!(target, ControlTarget::Host(_));
    let transport: Box<dyn IpcTransport> = match target {
        ControlTarget::Controller(socket) => Box::new(paneflow_ipc_client::IpcClient::new(socket)),
        ControlTarget::Host(endpoint) => match HostTransport::connect(&endpoint, CLIENT_NAME) {
            Ok(transport) => Box::new(transport),
            Err(error) => {
                eprintln!("paneflow-mcp: {error}");
                return ExitCode::FAILURE;
            }
        },
    };

    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    let scope = if hosted {
        scope::BridgeScope::from_env_for_host()
    } else {
        scope::BridgeScope::from_env()
    };
    let scope = match scope {
        Ok(scope) => scope,
        Err(error) => {
            eprintln!("paneflow-mcp: invalid read scope: {error}");
            return ExitCode::FAILURE;
        }
    };
    let bridge = bridge::Bridge::new(transport.as_ref(), scope);

    match mcp::serve(stdin, stdout, &bridge) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("paneflow-mcp: stdio loop terminated: {e}");
            ExitCode::FAILURE
        }
    }
}
