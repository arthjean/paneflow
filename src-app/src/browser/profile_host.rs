use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use paneflow_browser_protocol::{
    BrowserError, BrowserId, Command, Document, Event, FrameAck, FrameMessage, OperationId,
};

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
            {
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
            }
            return Ok((host, receiver, false));
        }
        let (supervisor, events) = HostSupervisor::channel();
        let host = Arc::new(Self {
            supervisor,
            routes: Mutex::new(Routes {
                pages: BTreeMap::from([(
                    browser,
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
                routes.documents.clear();
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
                let operation_browser = routes.operations.remove(&reply.operation);
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
            HostEvent::Native(value) => value
                .get("document")
                .and_then(|value| serde_json::from_value::<Document>(value.clone()).ok())
                .map(|document| document.browser),
            HostEvent::Frame(message, _) => Some(match message {
                FrameMessage::PoolCreated { document, .. }
                | FrameMessage::Frame { document, .. }
                | FrameMessage::PoolRetired { document, .. }
                | FrameMessage::Failed { document, .. } => document.browser.clone(),
            }),
            _ => None,
        };
        let Some(page) = browser
            .as_ref()
            .and_then(|browser| routes.pages.get(browser))
        else {
            if let HostEvent::Frame(message, _) = event {
                let ack = match message {
                    FrameMessage::PoolCreated {
                        document,
                        pool_generation,
                        ..
                    } => Some(FrameAck::PoolRejected {
                        document,
                        pool_generation,
                    }),
                    FrameMessage::Frame {
                        document,
                        pool_generation,
                        buffer,
                        sequence,
                        ..
                    } => Some(FrameAck::Release {
                        document,
                        pool_generation,
                        buffer,
                        sequence,
                    }),
                    _ => None,
                };
                if let Some(ack) = ack {
                    return self
                        .supervisor
                        .frame_acknowledger()
                        .and_then(|acknowledger| acknowledger.ack(ack))
                        .is_ok();
                }
            }
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
                        std::time::Duration::from_secs(60)
                    } else {
                        SHUTDOWN_GRACE
                    };
                    self.supervisor.shutdown(grace);
                })
                .ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_browser_protocol::{CONTRACT_VERSION, Owner, Reply, SessionId, WorkspaceId};

    fn fixture() -> (
        ProfileHost,
        BrowserId,
        BrowserId,
        smol::channel::Receiver<HostEvent>,
        smol::channel::Receiver<HostEvent>,
    ) {
        let first = BrowserId::try_from("first".to_owned()).unwrap();
        let second = BrowserId::try_from("second".to_owned()).unwrap();
        let (first_tx, first_rx) = smol::channel::bounded(2);
        let (second_tx, second_rx) = smol::channel::bounded(2);
        let (supervisor, _) = HostSupervisor::channel();
        let host = ProfileHost {
            supervisor,
            routes: Mutex::new(Routes {
                pages: BTreeMap::from([
                    (
                        first.clone(),
                        Subscriber {
                            sender: first_tx,
                            receiver: first_rx.clone(),
                        },
                    ),
                    (
                        second.clone(),
                        Subscriber {
                            sender: second_tx,
                            receiver: second_rx.clone(),
                        },
                    ),
                ]),
                stopping: false,
                operations: BTreeMap::new(),
                documents: BTreeMap::new(),
                ready: None,
            }),
        };
        (host, first, second, first_rx, second_rx)
    }

    fn document(browser: BrowserId) -> Document {
        Document {
            owner: Owner {
                workspace: WorkspaceId::try_from("workspace".to_owned()).unwrap(),
                session: SessionId::try_from("session".to_owned()).unwrap(),
            },
            browser,
            generation: 1,
        }
    }

    #[test]
    fn terminal_host_events_prevent_reuse() {
        for event in [HostEvent::Lost("lost".into()), HostEvent::Stopped] {
            let (host, _, _, first, second) = fixture();
            assert!(host.route(event));
            assert!(host.routes.lock().unwrap().stopping);
            assert!(host.routes.lock().unwrap().ready.is_none());
            assert!(first.try_recv().is_ok());
            assert!(second.try_recv().is_ok());
        }
    }

    #[test]
    fn profile_routes_replies_native_and_owned_frames_to_one_page() {
        use std::os::fd::AsRawFd;
        let (host, first, second, first_rx, second_rx) = fixture();
        let operation = OperationId::try_from("operation".to_owned()).unwrap();
        host.routes
            .lock()
            .unwrap()
            .operations
            .insert(operation.clone(), second.clone());
        assert!(host.route(HostEvent::Reply(Reply {
            version: CONTRACT_VERSION,
            operation,
            result: Err(BrowserError::Busy)
        })));
        assert!(matches!(second_rx.try_recv(), Ok(HostEvent::Reply(_))));
        assert!(first_rx.try_recv().is_err());
        assert!(host.route(HostEvent::Native(serde_json::json!({ "native": "title", "document": document(first.clone()), "title": "first" }))));
        assert!(matches!(first_rx.try_recv(), Ok(HostEvent::Native(_))));
        assert!(second_rx.try_recv().is_err());
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let descriptor = socket.as_raw_fd();
        assert!(host.route(HostEvent::Frame(
            FrameMessage::PoolRetired {
                document: document(second),
                pool_generation: 1
            },
            vec![socket.into()]
        )));
        let HostEvent::Frame(_, fds) = second_rx.try_recv().unwrap() else {
            panic!("expected frame")
        };
        assert_eq!(fds[0].as_raw_fd(), descriptor);
        assert!(first_rx.try_recv().is_err());
    }

    #[test]
    fn saturated_profile_queue_reports_loss_to_every_page_without_waiting() {
        let (host, first, _, first_rx, second_rx) = fixture();
        let event = || {
            HostEvent::Native(
                serde_json::json!({ "native": "title", "document": document(first.clone()) }),
            )
        };
        assert!(host.route(event()));
        assert!(host.route(event()));
        assert!(!host.route(event()));
        host.fail_queues();
        assert!(matches!(first_rx.try_recv(), Ok(HostEvent::Lost(_))));
        assert!(matches!(second_rx.try_recv(), Ok(HostEvent::Lost(_))));
        assert!(host.routes.lock().unwrap().stopping);
    }
}
