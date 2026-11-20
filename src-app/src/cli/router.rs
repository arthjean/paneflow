use std::path::{Path, PathBuf};

use paneflow_api::{Catalog, Descriptor, Owner};
use paneflow_ipc_client::host_control::{
    ControlTarget, HostControl, HostTransport, REQUEST_DEADLINE, resolve_control_target,
};
use paneflow_ipc_client::{IpcClient, IpcTransport};
use serde_json::Value;

use super::{CLIENT_NAME, CliError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Endpoint {
    Window(PathBuf),
    Host(PathBuf),
    Worker(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Endpoints {
    pub(crate) control: Option<ControlTarget>,
    pub(crate) host: Option<PathBuf>,
    pub(crate) worker: Option<PathBuf>,
}

impl Endpoints {
    pub(crate) fn from_environment() -> Self {
        Self {
            host: paneflow_host::endpoint::host_endpoint_path_for_current_home(),
            control: resolve_control_target(
                paneflow_home::ipc_endpoint(),
                paneflow_host::endpoint::host_endpoint_path_for_current_home(),
                paneflow_home::reserved_host_endpoint(),
            ),
            worker: paneflow_home::serve_endpoint_path_for_current_home(),
        }
    }
}

pub(super) fn no_instance() -> CliError {
    CliError::runtime(
        "cannot locate the IPC socket; is Paneflow running? \
         (set PANEFLOW_SOCKET_PATH if you launched the CLI outside a Paneflow pane)",
    )
}

pub(super) fn unknown_method(method: &str) -> CliError {
    CliError::runtime(format!("Method not found: {method}"))
}

pub(super) fn route(
    catalog: &Catalog,
    endpoints: &Endpoints,
    method: &str,
) -> Result<Endpoint, CliError> {
    let descriptor = catalog.get(method).ok_or_else(|| unknown_method(method))?;
    match descriptor.spec.owner {
        Owner::Window => match &endpoints.control {
            Some(ControlTarget::Controller(socket)) => Ok(Endpoint::Window(socket.clone())),
            Some(ControlTarget::Host(endpoint)) => Ok(Endpoint::Host(endpoint.clone())),
            None => Err(no_instance()),
        },
        Owner::Host => match &endpoints.control {
            Some(ControlTarget::Host(endpoint)) => Ok(Endpoint::Host(endpoint.clone())),
            Some(ControlTarget::Controller(_)) | None => endpoints
                .host
                .clone()
                .map(Endpoint::Host)
                .ok_or_else(|| owner_unreachable(descriptor, "no host endpoint resolves")),
        },
        Owner::Worker => endpoints
            .worker
            .clone()
            .map(Endpoint::Worker)
            .ok_or_else(|| owner_unreachable(descriptor, "no worker endpoint resolves")),
    }
}

fn owner_unreachable(descriptor: &Descriptor, reason: impl std::fmt::Display) -> CliError {
    let owner = match descriptor.spec.owner {
        Owner::Window => "the Paneflow window",
        Owner::Host => "the local Paneflow host",
        Owner::Worker => "the Paneflow worker",
    };
    CliError::runtime(format!(
        "{} is served by {owner}, which is not reachable: {reason}",
        descriptor.name()
    ))
}

fn worker_request(
    descriptor: &Descriptor,
    endpoint: &Path,
    method: &str,
    params: Value,
) -> Result<Value, CliError> {
    let mut control =
        HostControl::open(endpoint, CLIENT_NAME, REQUEST_DEADLINE).map_err(|error| {
            let reason = match error {
                paneflow_ipc_client::host_control::ControlConnectError::Transport(error) => {
                    format!("{} ({error})", endpoint.display())
                }
                paneflow_ipc_client::host_control::ControlConnectError::Handshake(message) => {
                    message
                }
            };
            owner_unreachable(descriptor, reason)
        })?;
    control
        .request_with_deadline(
            method,
            params,
            paneflow_api::methods::config::reply_deadline(method, REQUEST_DEADLINE),
        )
        .map_err(CliError::runtime)
}

pub(super) fn send(
    catalog: &Catalog,
    endpoint: &Endpoint,
    method: &str,
    params: Value,
) -> Result<Value, CliError> {
    let descriptor = catalog.get(method).ok_or_else(|| unknown_method(method))?;
    match endpoint {
        Endpoint::Window(socket) => IpcClient::new(socket.clone())
            .call(method, params)
            .map_err(CliError::runtime),
        Endpoint::Host(path) => {
            let transport = HostTransport::connect(path, CLIENT_NAME).map_err(|error| {
                CliError::runtime(format!("{error}; start it with `paneflow host start`"))
            })?;
            transport.call(method, params).map_err(CliError::runtime)
        }
        Endpoint::Worker(path) => worker_request(descriptor, path, method, params),
    }
}

pub(super) fn worker_listens(endpoints: &Endpoints) -> bool {
    endpoints
        .worker
        .as_deref()
        .is_some_and(|endpoint| HostControl::open(endpoint, CLIENT_NAME, REQUEST_DEADLINE).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        paneflow_api::catalog().expect("catalog")
    }

    fn endpoints(control: Option<ControlTarget>, worker: Option<&str>) -> Endpoints {
        Endpoints {
            control,
            host: Some("/h.sock".into()),
            worker: worker.map(PathBuf::from),
        }
    }

    #[test]
    fn a_window_method_goes_to_the_window_when_it_listens() {
        let routed = route(
            &catalog(),
            &endpoints(
                Some(ControlTarget::Controller("/run/app.sock".into())),
                Some("/w"),
            ),
            "surface.list",
        )
        .expect("routed");
        assert_eq!(routed, Endpoint::Window("/run/app.sock".into()));
    }

    #[test]
    fn a_window_method_falls_back_to_the_host_as_the_control_target_resolves() {
        let routed = route(
            &catalog(),
            &endpoints(Some(ControlTarget::Host("/h.sock".into())), None),
            "surface.read",
        )
        .expect("routed");
        assert_eq!(routed, Endpoint::Host("/h.sock".into()));
    }

    #[test]
    fn a_worker_method_goes_to_the_worker_and_never_to_the_window() {
        let routed = route(
            &catalog(),
            &endpoints(
                Some(ControlTarget::Controller("/run/app.sock".into())),
                Some("/w.sock"),
            ),
            "usage.snapshot",
        )
        .expect("routed");
        assert_eq!(routed, Endpoint::Worker("/w.sock".into()));
    }

    #[test]
    fn an_unreachable_worker_is_named_with_the_method_and_nothing_else_is_tried() {
        let dir = tempfile::tempdir().expect("tempdir");
        let absent = dir.path().join("worker.sock");
        let error = send(
            &catalog(),
            &Endpoint::Worker(absent),
            "worker.status",
            serde_json::json!({}),
        )
        .expect_err("no worker listens");
        assert!(error.message.contains("worker.status"), "{}", error.message);
        assert!(
            error.message.contains("Paneflow worker"),
            "{}",
            error.message
        );
    }

    #[test]
    fn an_unknown_method_is_refused_before_any_endpoint() {
        let error =
            route(&catalog(), &endpoints(None, None), "surface.teleport").expect_err("unknown");
        assert_eq!(error.message, "Method not found: surface.teleport");
    }

    #[test]
    fn every_host_control_method_is_declared_with_the_host_as_owner_or_fallback() {
        let catalog = catalog();
        for method in paneflow_host::control::CONTROL_METHODS {
            let descriptor = catalog
                .get(method)
                .unwrap_or_else(|| panic!("{method} has no descriptor"));
            let spec = descriptor.spec;
            assert!(
                spec.owner == Owner::Host || spec.fallback == Some(Owner::Host),
                "{method}"
            );
        }
        for method in paneflow_host::control::CONTROLLER_ONLY_METHODS {
            let spec = catalog.get(method).expect(method).spec;
            assert_eq!(spec.owner, Owner::Window, "{method}");
            assert_eq!(spec.fallback, None, "{method}");
        }
    }

    #[test]
    fn every_worker_method_is_declared_with_the_worker_as_owner() {
        use paneflow_serve::protocol as worker;
        let catalog = catalog();
        for method in [
            worker::METHOD_WORKER_HELLO,
            worker::METHOD_WORKER_STATUS,
            worker::METHOD_WORKER_SHUTDOWN,
            worker::METHOD_AGENT_FOLLOW,
            worker::METHOD_AGENT_SNAPSHOT,
            worker::METHOD_AGENT_ACTIVITY_LOG,
            worker::METHOD_AGENT_ACKNOWLEDGE,
            worker::METHOD_USAGE_SNAPSHOT,
            worker::METHOD_USAGE_REFRESH,
            worker::METHOD_USAGE_IMPORT,
        ] {
            let spec = catalog
                .get(method)
                .unwrap_or_else(|| panic!("{method} has no descriptor"))
                .spec;
            assert_eq!(spec.owner, Owner::Worker, "{method}");
        }
        assert!(
            catalog
                .get(worker::METHOD_AGENT_FOLLOW)
                .is_some_and(Descriptor::is_stream)
        );
    }
}
