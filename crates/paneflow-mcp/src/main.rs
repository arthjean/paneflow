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

use paneflow_ipc_client::host_control::{
    resolve_control_target, resolve_host_endpoint, ControlTarget, HostTransport,
};
use paneflow_ipc_client::IpcTransport;

use crate::bridge::HostLink;

const CLIENT_NAME: &str = "paneflow-mcp";

fn main() -> ExitCode {
    let controller = paneflow_home::ipc_endpoint();
    let host_endpoint = resolve_host_endpoint(
        controller.as_ref(),
        paneflow_home::host_endpoint_path_for_current_home(),
        paneflow_home::reserved_host_endpoint(),
    );
    let Some(target) = resolve_control_target(
        controller,
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

    let host_link = match (&target, host_endpoint) {
        (ControlTarget::Host(_), _) => HostLink::Shared,
        (ControlTarget::Controller(_), Some(endpoint)) => HostLink::Connect(Box::new(move || {
            HostTransport::connect(&endpoint, CLIENT_NAME)
                .map(|transport| Box::new(transport) as Box<dyn IpcTransport>)
                .map_err(|error| error.to_string())
        })),
        (ControlTarget::Controller(_), None) => HostLink::Missing(
            "no Paneflow host endpoint is known, so write_pane cannot reach the host that approves writes"
                .to_string(),
        ),
    };
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
    let scope = scope::BridgeScope::from_env();
    if let Some(error) = scope.error() {
        eprintln!("paneflow-mcp: every read will be refused: {error}");
    }
    let bridge = bridge::Bridge::new(transport.as_ref(), scope)
        .with_writer(scope::calling_session(), host_link);

    match mcp::serve(stdin, stdout, &bridge) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("paneflow-mcp: stdio loop terminated: {e}");
            ExitCode::FAILURE
        }
    }
}
