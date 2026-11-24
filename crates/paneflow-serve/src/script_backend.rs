use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use paneflow_api::{Catalog, Owner, Principal};
use paneflow_ipc_client::host_control::{ControlTarget, HostTransport, resolve_control_target};
use paneflow_ipc_client::{IpcClient, IpcTransport};
use paneflow_script::{
    Backend, CallError, Discovery, EventFeed, FeedFrame, PaneContext, SessionIds,
};
use serde_json::{Value, json};

use crate::event_log::{Delivery, Filter, Subscription};
use crate::server::Worker;

const CLIENT_NAME: &str = "paneflow-serve-lua";

pub struct WorkerBackend {
    worker: Weak<Worker>,
    home: PathBuf,
    catalog: Option<Catalog>,
    sessions: Mutex<HashMap<String, SessionIds>>,
}

impl WorkerBackend {
    pub fn new(worker: &Arc<Worker>) -> Self {
        Self {
            worker: Arc::downgrade(worker),
            home: worker.home.clone(),
            catalog: paneflow_api::catalog().ok(),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn control_target(&self) -> Option<ControlTarget> {
        let env = paneflow_home::EndpointEnv {
            paneflow_home: Some(self.home.clone().into_os_string()),
            ..paneflow_home::EndpointEnv::from_process()
        };
        resolve_control_target(
            paneflow_home::ipc_endpoint_in(&env),
            Some(paneflow_home::host_endpoint_path(&self.home)),
            paneflow_home::reserved_host_endpoint(),
        )
    }

    fn unreachable(method: &str, owner: &str, reason: impl std::fmt::Display) -> CallError {
        CallError::new(
            format!("{method} is served by {owner}, which is not reachable: {reason}"),
            CallError::UNAVAILABLE,
        )
    }

    fn call_window(
        &self,
        method: &str,
        params: Value,
        fallback: bool,
        grant: Option<&str>,
    ) -> Result<Value, CallError> {
        match self.control_target() {
            Some(ControlTarget::Controller(socket)) => {
                let client = IpcClient::new(socket);
                match grant {
                    Some(grant) => client.call_granted(method, params, grant),
                    None => client.call(method, params),
                }
                .map_err(|message| wire_error(&message))
            }
            Some(ControlTarget::Host(endpoint)) if fallback => {
                self.call_host(&endpoint, method, params)
            }
            _ => Err(Self::unreachable(
                method,
                "the Paneflow window",
                "no window is attached",
            )),
        }
    }

    fn call_host(&self, endpoint: &Path, method: &str, params: Value) -> Result<Value, CallError> {
        let transport = HostTransport::connect(endpoint, CLIENT_NAME)
            .map_err(|reason| Self::unreachable(method, "the local Paneflow host", reason))?;
        transport
            .call(method, params)
            .map_err(|message| wire_error(&message))
    }

    fn call_worker(
        &self,
        method: &str,
        params: &Value,
        acting: &Principal,
    ) -> Result<Value, CallError> {
        let worker = self.worker.upgrade().ok_or_else(|| {
            CallError::new("the Paneflow worker is stopping", CallError::UNAVAILABLE)
        })?;
        let registry = crate::registry::registry()
            .map_err(|error| CallError::new(error.message, error.code))?;
        registry
            .call(&worker, method, params, acting)
            .map_err(|error| CallError::new(error.message, error.code))
    }

    fn acting(&self, grant: Option<&str>) -> Result<(Principal, Option<String>), CallError> {
        let worker = self.worker.upgrade().ok_or_else(|| {
            CallError::new("the Paneflow worker is stopping", CallError::UNAVAILABLE)
        })?;
        let (principal, window) = worker.acting.resolve(grant);
        Ok((principal, window.map(str::to_string)))
    }

    fn authorize_forward(
        &self,
        descriptor: &paneflow_api::Descriptor,
        acting: &Principal,
        params: &mut Value,
    ) -> Result<(), CallError> {
        if matches!(acting, Principal::Worker) {
            return Ok(());
        }
        let worker = self.worker.upgrade().ok_or_else(|| {
            CallError::new("the Paneflow worker is stopping", CallError::UNAVAILABLE)
        })?;
        crate::authority::authorize_call(&worker, acting, descriptor, params)
            .map(drop)
            .map_err(|error| CallError::new(error.message, error.code))
    }

    fn learn(&self) {
        let Ok(listed) = self.call_window("surface.list", json!({}), true, None) else {
            return;
        };
        for row in listed["surfaces"].as_array().into_iter().flatten() {
            let Some(surface) = row["surface_id"].as_u64() else {
                continue;
            };
            let Ok(status) =
                self.call_window("surface.status", json!({"surface_id": surface}), true, None)
            else {
                continue;
            };
            if let Some(session) = status["session"].as_str() {
                self.sessions
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(
                        session.to_string(),
                        SessionIds {
                            surface_id: Some(surface),
                            pane_id: None,
                            tab_id: row["tab_id"].as_u64(),
                            workspace_id: row["workspace_id"].as_u64(),
                        },
                    );
            }
        }
    }
}

pub fn wire_error(message: &str) -> CallError {
    let code = message
        .split("paneflow error ")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .and_then(|code| code.trim().parse().ok())
        .unwrap_or(CallError::UNAVAILABLE);
    CallError::new(message, code)
}

impl Backend for WorkerBackend {
    fn discover(&self) -> Discovery {
        Discovery {
            methods: self
                .catalog
                .iter()
                .flat_map(Catalog::iter)
                .map(|descriptor| descriptor.describe_from(true))
                .collect(),
            unavailable: None,
            context: PaneContext::default(),
        }
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, CallError> {
        self.call_granted(method, params, None)
    }

    fn call_granted(
        &self,
        method: &str,
        params: Value,
        grant: Option<&str>,
    ) -> Result<Value, CallError> {
        let Some(descriptor) = self
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.get(method))
        else {
            return Err(CallError::new(
                format!("Method not found: {method}"),
                CallError::METHOD_NOT_FOUND,
            ));
        };
        let (acting, window_grant) = self.acting(grant)?;
        if descriptor.spec.owner == Owner::Worker {
            return self.call_worker(method, &params, &acting);
        }
        let mut params = params;
        self.authorize_forward(descriptor, &acting, &mut params)?;
        match descriptor.spec.owner {
            Owner::Window => self.call_window(
                method,
                params,
                descriptor.spec.fallback == Some(Owner::Host),
                window_grant.as_deref(),
            ),
            Owner::Host | Owner::Worker => {
                let endpoint = paneflow_home::host_endpoint_path(&self.home);
                self.call_host(&endpoint, method, params)
            }
        }
    }

    fn follow(&self) -> Result<Box<dyn EventFeed>, CallError> {
        let worker = self.worker.upgrade().ok_or_else(|| {
            CallError::new("the Paneflow worker is stopping", CallError::UNAVAILABLE)
        })?;
        let subscription = subscribe(&worker)?;
        Ok(Box::new(LogFeed {
            worker: self.worker.clone(),
            subscription: Some(subscription),
        }))
    }

    fn resolve_session(&self, session: &str) -> Option<SessionIds> {
        let cached = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session)
            .cloned();
        if cached.is_some() {
            return cached;
        }
        self.learn();
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session)
            .cloned()
    }

    fn window_attached(&self) -> bool {
        matches!(self.control_target(), Some(ControlTarget::Controller(_)))
    }
}

fn subscribe(worker: &Worker) -> Result<Subscription, CallError> {
    let filter = Filter::new(None, None).map_err(|message| CallError::new(message, -32602))?;
    worker.events.follow(None, None, filter).map_err(|evicted| {
        CallError::new(
            format!("the event log refused the follower at seq {}", evicted.seq),
            CallError::UNAVAILABLE,
        )
    })
}

struct LogFeed {
    worker: Weak<Worker>,
    subscription: Option<Subscription>,
}

impl EventFeed for LogFeed {
    fn next(&mut self, timeout: Duration) -> FeedFrame {
        let Some(subscription) = self.subscription.as_ref() else {
            std::thread::sleep(timeout);
            return FeedFrame::Idle;
        };
        match subscription.next(timeout) {
            Delivery::Event(logged) => match serde_json::from_str(&logged.line) {
                Ok(value) => FeedFrame::Event(value),
                Err(_) => FeedFrame::Idle,
            },
            Delivery::Quiet => FeedFrame::Idle,
            Delivery::Overflowed => {
                let renewed = self.worker.upgrade().and_then(|worker| {
                    worker.events.unfollow(subscription.id);
                    subscribe(&worker).ok()
                });
                self.subscription = renewed;
                FeedFrame::Resync
            }
            Delivery::Closed => {
                self.subscription = None;
                FeedFrame::Disconnected("the worker closed its event log".to_string())
            }
        }
    }
}

impl Drop for LogFeed {
    fn drop(&mut self) {
        if let (Some(worker), Some(subscription)) = (self.worker.upgrade(), &self.subscription) {
            worker.events.unfollow(subscription.id);
        }
    }
}
