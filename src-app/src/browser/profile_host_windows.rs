use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use paneflow_browser_protocol::{BrowserError, BrowserId, Command, Document, Event, OperationId};
use serde_json::Value;

use super::supervisor::{HostConfig, HostEvent, HostInfo, HostSupervisor, SHUTDOWN_GRACE};

const EVENT_CAPACITY: usize = 256;
const OPERATION_CAPACITY: usize = 4096;
type Registry = BTreeMap<PathBuf, Weak<ProfileHost>>;

struct Subscriber {
    sender: smol::channel::Sender<HostEvent>,
    receiver: smol::channel::Receiver<HostEvent>,
}

struct Routes {
    pages: BTreeMap<BrowserId, Subscriber>,
    stopping: bool,
    operations: BTreeMap<OperationId, BrowserId>,
    documents: BTreeMap<BrowserId, Document>,
    ready: Option<HostInfo>,
}

pub(super) struct ProfileHost {
    pub supervisor: HostSupervisor,
    routes: Mutex<Routes>,
}

impl ProfileHost {
    pub fn subscribe(
        config: HostConfig,
        browser: BrowserId,
    ) -> Result<(Arc<Self>, smol::channel::Receiver<HostEvent>, bool), String> {
        static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
        let mut registry = REGISTRY
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| "profile registry unavailable")?;
        registry.retain(|_, host| host.strong_count() != 0);
        let (sender, receiver) = smol::channel::bounded(EVENT_CAPACITY);
        if let Some(host) = registry.get(&config.profile_dir).and_then(Weak::upgrade) {
            let mut routes = host
                .routes
                .lock()
                .map_err(|_| "profile routes unavailable")?;
            if routes.stopping {
                return Err("profile host is stopping; retry after shutdown".into());
            }
            if routes.pages.contains_key(&browser) {
                return Err("browser already subscribed".into());
            }
            if let Some(info) = &routes.ready {
                sender
                    .try_send(HostEvent::Ready(info.clone()))
                    .map_err(|_| "page queue unavailable")?;
            }
            routes.pages.insert(
                browser,
                Subscriber {
                    sender,
                    receiver: receiver.clone(),
                },
            );
            drop(routes);
            return Ok((host, receiver, false));
        }
        let (supervisor, events) = HostSupervisor::channel();
        let host = Arc::new(Self {
            supervisor,
            routes: Mutex::new(Routes {
                pages: BTreeMap::from([(
                    browser.clone(),
                    Subscriber {
                        sender,
                        receiver: receiver.clone(),
                    },
                )]),
                stopping: false,
                operations: BTreeMap::new(),
                documents: BTreeMap::new(),
                ready: None,
            }),
        });
        let weak = Arc::downgrade(&host);
        std::thread::Builder::new()
            .name("browser-profile-events".into())
            .spawn(move || {
                while let Ok(event) = events.recv_blocking() {
                    let Some(host) = weak.upgrade() else { break };
                    let finished = matches!(&event, HostEvent::Lost(_) | HostEvent::Stopped);
                    if !host.route(event) {
                        host.fail_queues();
                        host.supervisor.terminate();
                        break;
                    }
                    if finished {
                        break;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        registry.insert(config.profile_dir.clone(), Arc::downgrade(&host));
        host.supervisor.activate(config);
        Ok((host, receiver, true))
    }

    pub fn send(&self, browser: &BrowserId, command: Command) -> Result<OperationId, BrowserError> {
        let mut routes = self.routes.lock().map_err(|_| BrowserError::Unavailable)?;
        if !routes.pages.contains_key(browser) {
            return Err(BrowserError::Unavailable);
        }
        if routes.operations.len() >= OPERATION_CAPACITY {
            return Err(BrowserError::Busy);
        }
        let operation = self.supervisor.send(command)?;
        routes.operations.insert(operation.clone(), browser.clone());
        Ok(operation)
    }

    fn route(&self, event: HostEvent) -> bool {
        let Ok(mut routes) = self.routes.lock() else {
            return false;
        };
        match &event {
            HostEvent::Ready(info) => {
                routes.ready = Some(info.clone());
                return routes
                    .pages
                    .values()
                    .all(|page| page.sender.try_send(HostEvent::Ready(info.clone())).is_ok());
            }
            HostEvent::Lost(reason) => {
                routes.stopping = true;
                routes.ready = None;
                routes.operations.clear();
                return routes.pages.values().all(|page| {
                    page.sender
                        .try_send(HostEvent::Lost(reason.clone()))
                        .is_ok()
                });
            }
            HostEvent::Stopped => {
                routes.stopping = true;
                routes.ready = None;
                return routes
                    .pages
                    .values()
                    .all(|page| page.sender.try_send(HostEvent::Stopped).is_ok());
            }
            _ => {}
        }
        let browser = match &event {
            HostEvent::Reply(reply) => {
                let operation_browser =
                    if matches!(&reply.result, Ok(Event::NavigationStarted { .. })) {
                        routes.operations.get(&reply.operation).cloned()
                    } else {
                        routes.operations.remove(&reply.operation)
                    };
                match &reply.result {
                    Ok(Event::State { session }) => {
                        if !routes.pages.contains_key(&session.document.browser) {
                            return self
                                .supervisor
                                .send(Command::Close {
                                    document: session.document.clone(),
                                })
                                .is_ok();
                        }
                        routes
                            .documents
                            .insert(session.document.browser.clone(), session.document.clone());
                        Some(session.document.browser.clone())
                    }
                    Ok(Event::Closed { document }) => {
                        routes.documents.remove(&document.browser);
                        Some(document.browser.clone())
                    }
                    _ => operation_browser,
                }
            }
            HostEvent::Native(value) => {
                let document = value
                    .get("document")
                    .and_then(|value| serde_json::from_value::<Document>(value.clone()).ok());
                if matches!(
                    value.get("native").and_then(Value::as_str),
                    Some(
                        "agent_navigation_committed"
                            | "agent_navigation_failed"
                            | "agent_navigation_cancelled"
                    )
                ) && let Some(operation) = value
                    .get("operation")
                    .and_then(Value::as_str)
                    .and_then(|operation| OperationId::try_from(operation.to_owned()).ok())
                {
                    routes.operations.remove(&operation);
                }
                if let Some(document) = &document
                    && value.get("native").and_then(Value::as_str)
                        == Some("agent_navigation_committed")
                {
                    routes
                        .documents
                        .insert(document.browser.clone(), document.clone());
                }
                document.map(|document| document.browser)
            }
            HostEvent::Ready(_) | HostEvent::Lost(_) | HostEvent::Stopped => None,
        };
        let Some(page) = browser
            .as_ref()
            .and_then(|browser| routes.pages.get(browser))
        else {
            return true;
        };
        page.sender.try_send(event).is_ok()
    }

    fn fail_queues(&self) {
        if let Ok(mut routes) = self.routes.lock() {
            routes.stopping = true;
            routes.ready = None;
            for page in routes.pages.values() {
                while page.receiver.try_recv().is_ok() {}
                let _ = page.sender.try_send(HostEvent::Lost(
                    "browser profile event queue saturated".into(),
                ));
            }
        }
    }

    pub fn unsubscribe(self: Arc<Self>, browser: &BrowserId, document: Option<Document>) {
        let last = if let Ok(mut routes) = self.routes.lock() {
            if let Some(document) = routes.documents.remove(browser).or(document) {
                let _ = self.supervisor.send(Command::Close { document });
            }
            routes.pages.remove(browser);
            routes.operations.retain(|_, target| target != browser);
            let last = routes.pages.is_empty();
            routes.stopping |= last;
            last
        } else {
            true
        };
        if last {
            std::thread::Builder::new()
                .name("browser-profile-shutdown".into())
                .spawn(move || {
                    let grace = if std::env::var_os("PANEFLOW_BROWSER_TRACE_DIR").is_some() {
                        Duration::from_secs(60)
                    } else {
                        SHUTDOWN_GRACE
                    };
                    self.supervisor.shutdown(grace);
                })
                .ok();
        }
    }
}
