use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;
use std::os::fd::FromRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use cef::*;
use paneflow_browser_protocol::{
    read_message, write_message, BrowserError, BrowserId, Command, Controller, Document, Envelope,
    Event, FrameChannel, HistoryDirection, OperationId, Owner, ProfileId, CONTRACT_VERSION,
    FRAME_CHANNEL_ENV,
};
use serde_json::json;

mod accessibility;
mod clipboard;
mod clipboard_renderer;
mod devtools;
mod devtools_renderer;
mod drag;
mod editing;
mod external_protocols;
mod gpu_contract;
mod handlers;
mod permissions;
mod presentation;
mod transfers;
mod views;
mod vulkan;
mod web_interactions;

const GPU_ENV: &str = "PANEFLOW_BROWSER_GPU";

struct Host {
    controller: Controller,
    owner: Owner,
    profile: Option<ProfileId>,
    receiver: Receiver<Option<Envelope>>,
    pages: BTreeMap<BrowserId, Page>,
    origin: String,
    closing: bool,
    tracing: bool,
    trace_pending: bool,
    trace_path: PathBuf,
}

struct Page {
    document: Document,
    browser: Option<Browser>,
    window: Option<Window>,
    pending_close: Option<Envelope>,
    inspected: Option<Document>,
    expected_navigation: Option<String>,
    pending_agent_navigation: Option<OperationId>,
    creating: bool,
}

thread_local! {
    #[cfg(test)]
    static EMITTED: RefCell<Vec<serde_json::Value>> = const { RefCell::new(Vec::new()) };
    static CONTROL_OUTPUT: RefCell<Option<std::fs::File>> = const { RefCell::new(None) };
    static CONTEXT: RefCell<Option<Document>> = const { RefCell::new(None) };
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

fn current_document() -> Option<Document> {
    CONTEXT.with(|state| state.borrow().clone())
}

fn latest_document(document: &Document) -> Option<Document> {
    HOST.with(|state| {
        state
            .borrow()
            .as_ref()?
            .pages
            .get(&document.browser)
            .map(|page| page.document.clone())
    })
}

struct Context(Option<Document>);

impl Context {
    fn enter(document: Option<Document>) -> Self {
        Self(CONTEXT.with(|state| state.replace(document)))
    }

    fn browser(browser: Option<&Browser>) -> Self {
        let document = browser.and_then(|browser| {
            HOST.with(|state| {
                state
                    .borrow()
                    .as_ref()?
                    .pages
                    .values()
                    .find(|page| {
                        page.browser
                            .as_ref()
                            .is_some_and(|candidate| candidate.identifier() == browser.identifier())
                    })
                    .map(|page| page.document.clone())
            })
        });
        Self::enter(document)
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        CONTEXT.with(|state| state.replace(self.0.take()));
    }
}

pub(crate) fn now_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) };
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

fn parse_gpu(value: &str) -> Result<(u32, u32), String> {
    let (vendor, device) = value
        .split_once(':')
        .ok_or_else(|| format!("{GPU_ENV} must be vendor:device in hexadecimal"))?;
    let parse = |part: &str| {
        u32::from_str_radix(part.trim_start_matches("0x"), 16)
            .map_err(|_| format!("{GPU_ENV} must be vendor:device in hexadecimal"))
    };
    Ok((parse(vendor)?, parse(device)?))
}

fn emit(mut value: serde_json::Value) {
    if value.get("native").is_some() && value.get("document").is_none() {
        if let Some(document) = current_document() {
            value["document"] = json!(document);
        }
    }
    value["trace_us"] = json!(now_from_system_trace_time());
    #[cfg(test)]
    EMITTED.with(|events| events.borrow_mut().push(value.clone()));
    let result = CONTROL_OUTPUT.with(|output| {
        let mut output = output.borrow_mut();
        match output.as_mut() {
            Some(output) => write_message(output, &value),
            None => write_message(&mut io::stdout().lock(), &value),
        }
    });
    if result.is_err() {
        quit_message_loop();
    }
}

fn close_browser() {
    editing::clear();
    let browsers = HOST.with(|state| {
        let mut state = state.borrow_mut();
        state
            .as_mut()
            .map(|host| {
                host.closing = true;
                host.pages
                    .values()
                    .filter_map(|page| page.browser.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    if browsers.is_empty() {
        quit_message_loop();
    }
    for browser in browsers {
        let _context = Context::browser(Some(&browser));
        if let Some(host) = browser.host() {
            host.close_browser(1);
        }
    }
}

fn close() {
    if HOST.with(|state| state.borrow().as_ref().is_some_and(|host| host.closing)) {
        return;
    }
    let trace_path = HOST.with(|state| {
        let mut state = state.borrow_mut();
        state.as_mut().and_then(|host| {
            if !host.tracing {
                return None;
            }
            host.tracing = false;
            host.closing = true;
            Some(host.trace_path.clone())
        })
    });
    if let Some(path) = trace_path {
        let path = CefString::from(path.to_string_lossy().as_ref());
        emit(json!({ "native": "trace_stop_requested" }));
        HOST.with(|state| {
            if let Some(host) = state.borrow_mut().as_mut() {
                host.trace_pending = true;
            }
        });
        if end_tracing(Some(&path), Some(&mut TraceFinished::new())) == 1 {
            return;
        }
        HOST.with(|state| {
            if let Some(host) = state.borrow_mut().as_mut() {
                host.trace_pending = false;
            }
        });
        emit(json!({ "native": "trace_failed", "reason": "end_tracing rejected" }));
    }
    close_browser();
}

wrap_completion_callback! {
    struct TraceStarted;

    impl CompletionCallback {
        fn on_complete(&self) {
            emit(json!({ "native": "trace_started", "trace_us": now_from_system_trace_time() }));
            post_task(ThreadId::UI, Some(&mut DrainControl::new()));
        }
    }
}

wrap_end_tracing_callback! {
    struct TraceFinished;

    impl EndTracingCallback {
        fn on_end_tracing_complete(&self, tracing_file: Option<&CefString>) {
            HOST.with(|state| {
                if let Some(host) = state.borrow_mut().as_mut() {
                    host.trace_pending = false;
                }
            });
            emit(json!({ "native": "trace_completed", "path": tracing_file.map(ToString::to_string), "trace_us": now_from_system_trace_time() }));
            close_browser();
        }
    }
}

fn cancel_close(document: &Document) {
    let pending = HOST.with(|state| {
        state
            .borrow_mut()
            .as_mut()?
            .pages
            .get_mut(&document.browser)?
            .pending_close
            .take()
    });
    if let Some(pending) = pending {
        emit(
            json!({ "protocol": paneflow_browser_protocol::Reply { version: CONTRACT_VERSION, operation: pending.operation, result: Err(BrowserError::Unavailable) } }),
        );
        emit(json!({ "native": "close_cancelled", "document": document }));
    }
}

fn cancel_agent_navigation(reason: &str) {
    let pending = HOST.with(|state| {
        let mut state = state.borrow_mut();
        let host = state.as_mut()?;
        let document = current_document()?;
        let page = host.pages.get_mut(&document.browser)?;
        page.pending_agent_navigation
            .take()
            .map(|operation| (document, operation))
    });
    if let Some((document, operation)) = pending {
        emit(json!({
            "native": "agent_navigation_cancelled",
            "document": document,
            "operation": operation,
            "reason": reason
        }));
    }
}

fn agent_navigation_failed(reason: &str) {
    let pending = HOST.with(|state| {
        let mut state = state.borrow_mut();
        let host = state.as_mut()?;
        let document = current_document()?;
        let page = host.pages.get_mut(&document.browser)?;
        page.pending_agent_navigation
            .take()
            .map(|operation| (document, operation))
    });
    if let Some((document, operation)) = pending {
        emit(json!({
            "native": "agent_navigation_failed",
            "document": document,
            "operation": operation,
            "reason": reason
        }));
    }
}

fn agent_navigation_failure_matches(url: Option<&str>) -> bool {
    HOST.with(|state| {
        let state = state.borrow();
        let Some(host) = state.as_ref() else {
            return false;
        };
        let Some(document) = current_document() else {
            return false;
        };
        let Some(page) = host.pages.get(&document.browser) else {
            return false;
        };
        page.pending_agent_navigation.is_some()
            && url.is_none_or(|url| page.expected_navigation.as_deref() == Some(url))
    })
}

fn agent_navigation_committed(url: &str) {
    let Some(url) = paneflow_browser_protocol::validate_url(url)
        .ok()
        .map(|_| url.to_owned())
    else {
        agent_navigation_failed("native navigation committed an invalid URL");
        return;
    };
    let Some((document, operation, owner)) = HOST.with(|state| {
        let mut state = state.borrow_mut();
        let host = state.as_mut()?;
        let document = current_document()?;
        let page = host.pages.get_mut(&document.browser)?;
        page.pending_agent_navigation
            .take()
            .map(|operation| (page.document.clone(), operation, host.owner.clone()))
    }) else {
        return;
    };
    let reply = HOST.with(|state| {
        let mut state = state.borrow_mut();
        let host = state.as_mut()?;
        Some(host.controller.dispatch(
            &owner,
            Envelope {
                version: CONTRACT_VERSION,
                operation: "native-agent-commit".to_owned().try_into().ok()?,
                command: Command::Navigate {
                    document: document.clone(),
                    url: url.clone(),
                },
            },
        ))
    });
    let Some(reply) = reply else {
        emit(json!({
            "native": "agent_navigation_failed",
            "document": document,
            "operation": operation,
            "reason": "navigation commit controller unavailable"
        }));
        return;
    };
    let Ok(Event::State { session }) = reply.result else {
        emit(json!({
            "native": "agent_navigation_failed",
            "document": document,
            "operation": operation,
            "reason": "navigation commit rejected"
        }));
        return;
    };
    devtools::invalidate_target(&document.browser);
    HOST.with(|state| {
        if let Some(host) = state.borrow_mut().as_mut() {
            if let Some(page) = host.pages.get_mut(&session.document.browser) {
                page.document = session.document.clone();
                page.expected_navigation = None;
            }
        }
    });
    CONTEXT.with(|state| state.replace(Some(session.document.clone())));
    if presentation::set_document(&session.document).is_err() {
        emit(json!({
            "native": "agent_navigation_failed",
            "document": session.document,
            "operation": operation,
            "reason": "navigation commit presentation unavailable"
        }));
        return;
    }
    emit(json!({
        "native": "agent_navigation_committed",
        "document": session.document,
        "session": session,
        "operation": operation,
        "url": url
    }));
}

fn renderer_crashed() {
    if let Some(document) = current_document() {
        devtools::invalidate_target(&document.browser);
    }
    let result = HOST.with(|state| {
        let mut state = state.borrow_mut();
        let host = state.as_mut()?;
        let document = current_document()?;
        Some(host.controller.renderer_crashed(&document.owner, &document))
    });
    if let Some(Ok(Event::State { session })) = result {
        HOST.with(|state| {
            if let Some(host) = state.borrow_mut().as_mut() {
                if let Some(page) = host.pages.get_mut(&session.document.browser) {
                    page.document = session.document.clone();
                }
            }
        });
        CONTEXT.with(|state| state.replace(Some(session.document.clone())));
        let _ = presentation::set_document(&session.document);
        if let Ok(operation) = "native-crash".to_owned().try_into() {
            emit(
                json!({ "protocol": paneflow_browser_protocol::Reply { version: CONTRACT_VERSION, operation, result: Ok(Event::State { session }) } }),
            );
        }
    }
}

fn accept_navigation(url: &str) -> bool {
    let reply = HOST.with(|state| {
        let mut state = state.borrow_mut();
        let host = state.as_mut()?;
        let document = current_document()?;
        let page = host.pages.get_mut(&document.browser)?;
        if page.pending_agent_navigation.is_some() {
            if page.expected_navigation.as_deref() == Some(url) {
                return None;
            }
            return Some(paneflow_browser_protocol::Reply {
                version: CONTRACT_VERSION,
                operation: "native-navigation".to_owned().try_into().ok()?,
                result: Err(BrowserError::Unavailable),
            });
        }
        if page.expected_navigation.take().as_deref() == Some(url) {
            return None;
        }
        let message = Envelope {
            version: CONTRACT_VERSION,
            operation: "native-navigation".to_owned().try_into().ok()?,
            command: Command::Navigate {
                document: document.clone(),
                url: url.to_owned(),
            },
        };
        Some(host.controller.dispatch(&document.owner, message))
    });
    let Some(reply) = reply else {
        return true;
    };
    let Ok(Event::State { session }) = &reply.result else {
        return false;
    };
    if let Some(document) = current_document() {
        devtools::invalidate_target(&document.browser);
    }
    HOST.with(|state| {
        if let Some(host) = state.borrow_mut().as_mut() {
            if let Some(page) = host.pages.get_mut(&session.document.browser) {
                page.document = session.document.clone();
            }
        }
    });
    CONTEXT.with(|state| state.replace(Some(session.document.clone())));
    if presentation::set_document(&session.document).is_err() {
        return false;
    }
    emit(json!({ "protocol": reply }));
    true
}

fn process(message: Envelope) {
    let command = message.command.clone();
    let transport_operation = message.operation.clone();
    let document = serde_json::to_value(&command)
        .ok()
        .and_then(|value| value.get("document").cloned())
        .and_then(|value| serde_json::from_value::<Document>(value).ok());
    let _context = Context::enter(document.clone());
    if matches!(
        &command,
        Command::Navigate { .. }
            | Command::History { .. }
            | Command::Reload { .. }
            | Command::Stop { .. }
            | Command::Close { .. }
            | Command::Sleep { .. }
            | Command::Input { .. }
    ) {
        cancel_agent_navigation("human browser control");
    }
    let caller = match &command {
        Command::Create { owner, .. } => Some(owner.clone()),
        _ => document.as_ref().map(|document| document.owner.clone()),
    };
    let has_browser = current_browser().is_some();
    let creating = HOST.with(|state| {
        state
            .borrow()
            .as_ref()
            .and_then(|host| host.pages.get(&document.as_ref()?.browser))
            .is_some_and(|page| page.creating)
    });
    if matches!(&command, Command::Close { .. }) && (has_browser || creating) {
        let validation = HOST.with(|state| {
            let mut state = state.borrow_mut();
            let host = state.as_mut()?;
            let document = document.as_ref()?;
            if document.owner.workspace != host.owner.workspace {
                return None;
            }
            let check = Envelope {
                command: Command::State {
                    document: document.clone(),
                },
                ..message.clone()
            };
            let reply = host.controller.dispatch(&document.owner, check);
            if reply.result.is_err() {
                return Some(Err(reply));
            }
            let page = host.pages.get_mut(&document.browser)?;
            page.pending_close = Some(message.clone());
            Some(Ok(()))
        });
        match validation {
            Some(Ok(())) => {
                if let Some(host) = current_browser().and_then(|browser| browser.host()) {
                    host.close_browser(0);
                }
                return;
            }
            Some(Err(reply)) => {
                emit(json!({ "protocol": reply }));
                return;
            }
            None => (),
        }
    }
    let reply = HOST.with(|state| {
        let mut state = state.borrow_mut();
        state.as_mut().map(|host| {
            let permitted = match &command {
                Command::Create { url, profile, .. } => {
                    host.profile
                        .as_ref()
                        .is_none_or(|current| current == profile)
                        && (presentation::active() || url.starts_with(&format!("{}/", host.origin)))
                }
                Command::Navigate { url, .. } => {
                    presentation::active() || url.starts_with(&format!("{}/", host.origin))
                }
                Command::AgentNavigate { url, .. } => {
                    has_browser
                        && (presentation::active() || url.starts_with(&format!("{}/", host.origin)))
                }
                Command::Capabilities | Command::State { .. } | Command::Close { .. } => true,
                Command::CreateDevTools { .. } => has_browser && presentation::active(),
                Command::Start { .. } => !has_browser,
                Command::Present { .. } | Command::Input { .. } => presentation::active(),
                Command::Screenshot { .. } => has_browser,
                Command::History { .. }
                | Command::Reload { .. }
                | Command::Stop { .. }
                | Command::Zoom { .. }
                | Command::Mute { .. }
                | Command::Find { .. }
                | Command::StopFinding { .. } => has_browser,
                _ => false,
            };
            if permitted
                && !host.closing
                && caller
                    .as_ref()
                    .is_none_or(|caller| caller.workspace == host.owner.workspace)
            {
                let reply = host
                    .controller
                    .dispatch(caller.as_ref().unwrap_or(&host.owner), message);
                if reply.result.is_ok() {
                    if let Command::Create { profile, .. } = &command {
                        host.profile = Some(profile.clone());
                    }
                }
                reply
            } else {
                paneflow_browser_protocol::Reply {
                    version: CONTRACT_VERSION,
                    operation: message.operation,
                    result: Err(BrowserError::Unavailable),
                }
            }
        })
    });
    let Some(mut reply) = reply else {
        return;
    };
    if let Ok(Event::State { session } | Event::NavigationStarted { session }) = &reply.result {
        if let Some(previous) = document
            .as_ref()
            .filter(|previous| **previous != session.document)
        {
            devtools::invalidate_target(&previous.browser);
        }
        CONTEXT.with(|state| state.replace(Some(session.document.clone())));
        HOST.with(|state| {
            if let Some(host) = state.borrow_mut().as_mut() {
                host.pages
                    .entry(session.document.browser.clone())
                    .and_modify(|page| {
                        page.document = session.document.clone();
                        if matches!(&command, Command::Navigate { .. }) {
                            page.expected_navigation = Some(session.url.clone());
                        }
                        if matches!(&command, Command::AgentNavigate { .. }) {
                            page.expected_navigation = Some(session.url.clone());
                        }
                    })
                    .or_insert(Page {
                        document: session.document.clone(),
                        browser: None,
                        window: None,
                        pending_close: None,
                        inspected: None,
                        expected_navigation: Some(session.url.clone()),
                        pending_agent_navigation: None,
                        creating: false,
                    });
            }
        });
        if matches!(&command, Command::AgentNavigate { .. }) && reply.result.is_ok() {
            HOST.with(|state| {
                if let Some(host) = state.borrow_mut().as_mut() {
                    if let Some(page) = host.pages.get_mut(&session.document.browser) {
                        page.pending_agent_navigation = Some(transport_operation.clone());
                        page.expected_navigation = Some(session.url.clone());
                    }
                }
            });
        }
        if let Command::CreateDevTools { document, .. } = &command {
            HOST.with(|state| {
                if let Some(host) = state.borrow_mut().as_mut() {
                    if let Some(page) = host.pages.get_mut(&session.document.browser) {
                        page.inspected = Some(document.clone());
                    }
                }
            });
        }
        if let Err(error) = presentation::set_document(&session.document) {
            emit(json!({ "native": "create_failed", "reason": error }));
            emit(
                json!({ "protocol": paneflow_browser_protocol::Reply { version: CONTRACT_VERSION, operation: transport_operation.clone(), result: Err(BrowserError::Unavailable) } }),
            );
            return;
        }
        if matches!(&command, Command::Start { .. }) {
            HOST.with(|state| {
                if let Some(page) = state
                    .borrow_mut()
                    .as_mut()
                    .and_then(|host| host.pages.get_mut(&session.document.browser))
                {
                    page.creating = true;
                }
            });
            emit(json!({ "native": "browser_create_requested", "document": session.document }));
            let inspected = HOST.with(|state| {
                let state = state.borrow();
                let host = state.as_ref()?;
                host.pages.get(&session.document.browser)?.inspected.clone()
            });
            let created = if inspected.is_some() {
                presentation::create(devtools::URL)
            } else if presentation::active() {
                presentation::create(&session.url)
            } else {
                views::create(&session.url)
            };
            if !created {
                HOST.with(|state| {
                    if let Some(page) = state
                        .borrow_mut()
                        .as_mut()
                        .and_then(|host| host.pages.get_mut(&session.document.browser))
                    {
                        page.creating = false;
                    }
                });
                reply.result = Err(BrowserError::Unavailable);
            }
        }
    }
    let succeeded = reply.result.is_ok();
    if let Ok(value) = serde_json::to_value(&reply) {
        emit(json!({ "protocol": value }));
    }
    if succeeded {
        match command {
            Command::Close { .. } => {
                editing::clear();
                presentation::unmount();
                if let Some(host) = current_browser().and_then(|browser| browser.host()) {
                    host.close_browser(1);
                }
                if current_browser().is_none() {
                    presentation::detach();
                    emit(json!({ "native": "closed" }));
                }
                if !presentation::active() {
                    close();
                }
            }
            Command::Find {
                text,
                forward,
                find_next,
                ..
            } => {
                if let Some(host) = current_browser().and_then(|browser| browser.host()) {
                    host.find(
                        Some(&text.as_str().into()),
                        i32::from(forward),
                        0,
                        i32::from(find_next),
                    );
                }
            }
            Command::StopFinding { .. } => {
                if let Some(host) = current_browser().and_then(|browser| browser.host()) {
                    host.stop_finding(1);
                }
            }
            Command::Navigate { url, .. } => {
                if let Some(document) = current_document() {
                    web_interactions::clear(&document);
                    permissions::clear(&document);
                    transfers::clear(&document);
                    external_protocols::clear(&document);
                }
                if let Some(frame) = current_browser().and_then(|browser| browser.main_frame()) {
                    frame.load_url(Some(&url.as_str().into()));
                }
                presentation::unmount();
            }
            Command::AgentNavigate { url, .. } => {
                if let Some(document) = current_document() {
                    web_interactions::clear(&document);
                    permissions::clear(&document);
                    transfers::clear(&document);
                    external_protocols::clear(&document);
                }
                if let Some(frame) = current_browser().and_then(|browser| browser.main_frame()) {
                    frame.load_url(Some(&url.as_str().into()));
                }
                presentation::unmount();
            }
            Command::Screenshot { document } => {
                if !devtools::capture(&document, &transport_operation) {
                    emit(json!({
                        "native": "screenshot_failed",
                        "document": document,
                        "operation": transport_operation,
                        "reason": "native capture adapter unavailable"
                    }));
                }
            }
            Command::Present { presentation, .. } => presentation::present(&presentation),
            Command::Input { input, .. } => presentation::input(input),
            Command::History { direction, .. } => {
                if let Some(browser) = current_browser() {
                    match direction {
                        HistoryDirection::Back => browser.go_back(),
                        HistoryDirection::Forward => browser.go_forward(),
                    }
                }
            }
            Command::Reload { ignore_cache, .. } => {
                if let Some(browser) = current_browser() {
                    if ignore_cache {
                        browser.reload_ignore_cache();
                    } else {
                        browser.reload();
                    }
                }
            }
            Command::Stop { .. } => {
                if let Some(browser) = current_browser() {
                    browser.stop_load();
                }
            }
            Command::Zoom { percent, .. } => {
                if let Some(host) = current_browser().and_then(|browser| browser.host()) {
                    host.set_zoom_level((f64::from(percent) / 100.0).log(1.2));
                }
            }
            Command::Mute { muted, .. } => {
                if let Some(host) = current_browser().and_then(|browser| browser.host()) {
                    host.set_audio_muted(i32::from(muted));
                }
            }
            _ => (),
        }
    }
}

fn current_browser() -> Option<Browser> {
    HOST.with(|state| {
        state.borrow().as_ref().and_then(|host| {
            let document = current_document()?;
            let page = host.pages.get(&document.browser)?;
            (page.document == document)
                .then(|| page.browser.clone())
                .flatten()
        })
    })
}

wrap_task! {
    struct DrainControl;

    impl Task {
        fn execute(&self) {
            for _ in 0..64 {
                let next = HOST.with(|state| {
                    let state = state.borrow();
                    state.as_ref().filter(|host| !host.closing).map(|host| host.receiver.try_recv())
                });
                match next {
                    Some(Ok(Some(message))) => process(message),
                    Some(Ok(None) | Err(TryRecvError::Disconnected)) => { close(); return; }
                    _ => return,
                }
            }
            post_task(ThreadId::UI, Some(&mut DrainControl::new()));
        }
    }
}

fn isolate_control_output() -> io::Result<()> {
    let fd = unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let output = unsafe { std::fs::File::from_raw_fd(fd) };
    if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
        return Err(io::Error::last_os_error());
    }
    CONTROL_OUTPUT.with(|slot| slot.replace(Some(output)));
    Ok(())
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    isolate_control_output()?;
    if std::env::args().any(|arg| crate::qualification::sandbox_disabled(&arg)) {
        return Err("sandbox disabling switches are forbidden".into());
    }
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = cef::args::Args::new();
    let mut app = handlers::WitnessApp::new();
    let result = execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if result >= 0 {
        std::process::exit(result);
    }
    let root =
        PathBuf::from(std::env::var_os("PANEFLOW_CEF_ROOT").ok_or("runtime root is required")?)
            .canonicalize()?;
    let profile = PathBuf::from(
        std::env::var_os("PANEFLOW_CEF_PROFILE").ok_or("isolated witness profile is required")?,
    );
    let metadata = std::fs::symlink_metadata(&profile)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err("witness profile must be a private directory (0700)".into());
    }
    let frame_channel = match FrameChannel::from_environment() {
        Ok(channel) => Some(channel),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("{FRAME_CHANNEL_ENV}: {error}").into()),
    };
    let expected_gpu = match std::env::var(GPU_ENV) {
        Ok(value) => Some(parse_gpu(&value)?),
        Err(_) => None,
    };
    let origin = std::env::var("PANEFLOW_CEF_ORIGIN")?;
    match origin.strip_prefix("http://127.0.0.1:") {
        Some(port) => {
            if port.parse::<u16>()? == 0 {
                return Err("origin requires a bound port".into());
            }
        }
        None => {
            let secure = frame_channel.is_some()
                && origin
                    .strip_prefix("https://")
                    .is_some_and(|host| !host.is_empty() && !host.contains('/'));
            if !secure {
                return Err("origin must be a loopback http origin or an https origin".into());
            }
        }
    }
    let owner = match std::env::var("PANEFLOW_BROWSER_OWNER") {
        Ok(value) => {
            let (workspace, session) = value
                .split_once('/')
                .ok_or("PANEFLOW_BROWSER_OWNER must be workspace/session")?;
            Owner {
                workspace: workspace.to_string().try_into()?,
                session: session.to_string().try_into()?,
            }
        }
        Err(_) => Owner {
            workspace: "witness".to_string().try_into()?,
            session: "qualification".to_string().try_into()?,
        },
    };
    let handshake = read_message(&mut io::stdin().lock())
        .map_err(|error| format!("handshake: {error:?}"))?
        .ok_or("missing handshake")?;
    if !matches!(handshake.command, Command::Capabilities) {
        return Err("first control message must negotiate capabilities".into());
    }
    let (sender, receiver) = mpsc::sync_channel(256);
    let mut controller = Controller::new(format!("{}-linux", std::env::consts::ARCH), true);
    let hello = controller.dispatch(&owner, handshake);
    HOST.with(|state| {
        *state.borrow_mut() = Some(Host {
            controller,
            owner,
            profile: None,
            receiver,
            pages: BTreeMap::new(),
            origin,
            closing: false,
            tracing: false,
            trace_pending: false,
            trace_path: std::env::var_os("PANEFLOW_BROWSER_TRACE_DIR")
                .map(PathBuf::from)
                .map(|dir| dir.join(format!("cef-{}.json", std::process::id())))
                .unwrap_or_else(|| profile.join("chromium-trace.json")),
        })
    });
    let settings = Settings {
        no_sandbox: 0,
        command_line_args_disabled: 1,
        windowless_rendering_enabled: i32::from(frame_channel.is_some()),
        log_severity: if std::env::var_os("PANEFLOW_BROWSER_LOG_VERBOSE").is_some() {
            LogSeverity::VERBOSE
        } else {
            LogSeverity::default()
        },
        root_cache_path: profile.to_string_lossy().as_ref().into(),
        cache_path: profile.join("profile").to_string_lossy().as_ref().into(),
        resources_dir_path: root.join("Resources").to_string_lossy().as_ref().into(),
        locales_dir_path: root
            .join("Resources/locales")
            .to_string_lossy()
            .as_ref()
            .into(),
        log_file: profile.join("cef.log").to_string_lossy().as_ref().into(),
        ..Default::default()
    };
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        return Err("CEF initialize failed; inspect witness cef.log".into());
    }
    let presenting = frame_channel.is_some();
    if let Some(channel) = frame_channel {
        presentation::install(channel, expected_gpu)?;
    }
    emit(
        json!({ "native": "initialized", "pid": std::process::id(), "sandbox_requested": true, "contract_version": CONTRACT_VERSION, "presentation": presenting, "protocol": hello }),
    );
    if !std::env::var("PANEFLOW_BROWSER_TRACE").is_ok_and(|value| value == "0") {
        let categories = CefString::from(
            if std::env::var_os("PANEFLOW_BROWSER_TRACE_DIR").is_some() {
                "cef,cc,viz,gpu.capture,blink,devtools.timeline,renderer.scheduler"
            } else {
                "viz,gpu.capture"
            },
        );
        if begin_tracing(Some(&categories), Some(&mut TraceStarted::new())) != 1 {
            return Err("CEF tracing initialization failed".into());
        }
        HOST.with(|state| {
            if let Some(host) = state.borrow_mut().as_mut() {
                host.tracing = true;
            }
        });
    }
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            match read_message(&mut input) {
                Ok(Some(message)) => {
                    if sender.send(Some(message)).is_err() {
                        return;
                    }
                    post_task(ThreadId::UI, Some(&mut DrainControl::new()));
                }
                Ok(None) => {
                    if sender.send(None).is_ok() {
                        post_task(ThreadId::UI, Some(&mut DrainControl::new()));
                    }
                    return;
                }
                Err(error) => {
                    eprintln!("control rejected: {error:?}");
                    if sender.send(None).is_ok() {
                        post_task(ThreadId::UI, Some(&mut DrainControl::new()));
                    }
                    return;
                }
            }
        }
    });
    run_message_loop();
    presentation::uninstall();
    HOST.with(|state| *state.borrow_mut() = None);
    shutdown();
    emit(json!({ "native": "shutdown" }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_context_restores_nested_page_identity() {
        let owner = Owner {
            workspace: "workspace".to_owned().try_into().unwrap(),
            session: "session".to_owned().try_into().unwrap(),
        };
        let first = Document {
            owner: owner.clone(),
            browser: "first".to_owned().try_into().unwrap(),
            generation: 1,
        };
        let second = Document {
            owner,
            browser: "second".to_owned().try_into().unwrap(),
            generation: 2,
        };
        assert_eq!(current_document(), None);
        {
            let _first = Context::enter(Some(first.clone()));
            assert_eq!(current_document(), Some(first.clone()));
            {
                let _second = Context::enter(Some(second.clone()));
                assert_eq!(current_document(), Some(second));
            }
            assert_eq!(current_document(), Some(first));
        }
        assert_eq!(current_document(), None);
    }
    #[test]
    fn control_and_native_navigation_share_one_generation_transition_per_page() {
        let owner = Owner {
            workspace: "workspace".to_owned().try_into().unwrap(),
            session: "session".to_owned().try_into().unwrap(),
        };
        let mut controller = Controller::new("test".to_owned(), true);
        let mut pages = BTreeMap::new();
        let mut documents = Vec::new();
        for name in ["first", "second"] {
            let command = Command::Create {
                owner: owner.clone(),
                browser: name.to_owned().try_into().unwrap(),
                profile: "profile".to_owned().try_into().unwrap(),
                url: "http://127.0.0.1:3000/start".to_owned(),
                title: String::new(),
            };
            let reply = controller.dispatch(
                &owner,
                Envelope {
                    version: CONTRACT_VERSION,
                    operation: name.to_owned().try_into().unwrap(),
                    command,
                },
            );
            let session = match reply.result {
                Ok(Event::State { session }) => Some(session),
                _ => None,
            }
            .expect("expected session");
            controller.dispatch(
                &owner,
                Envelope {
                    version: CONTRACT_VERSION,
                    operation: name.to_owned().try_into().unwrap(),
                    command: Command::Start {
                        document: session.document.clone(),
                    },
                },
            );
            documents.push(session.document.clone());
            pages.insert(
                session.document.browser.clone(),
                Page {
                    document: session.document,
                    browser: None,
                    window: None,
                    pending_close: None,
                    inspected: None,
                    expected_navigation: Some(session.url),
                    pending_agent_navigation: None,
                    creating: false,
                },
            );
        }
        let (_, receiver) = mpsc::channel();
        HOST.with(|state| {
            state.replace(Some(Host {
                controller,
                owner: owner.clone(),
                profile: Some("profile".to_owned().try_into().unwrap()),
                receiver,
                pages,
                origin: "http://127.0.0.1:3000".to_owned(),
                closing: false,
                tracing: false,
                trace_pending: false,
                trace_path: PathBuf::new(),
            }))
        });
        {
            let _context = Context::enter(Some(documents[0].clone()));
            assert!(accept_navigation("http://127.0.0.1:3000/start"));
            assert_eq!(latest_document(&documents[0]), Some(documents[0].clone()));
            process(Envelope {
                version: CONTRACT_VERSION,
                operation: "navigate".to_owned().try_into().unwrap(),
                command: Command::Navigate {
                    document: documents[0].clone(),
                    url: "http://127.0.0.1:3000/next".to_owned(),
                },
            });
            let controlled = latest_document(&documents[0]).unwrap();
            assert_ne!(controlled.generation, documents[0].generation);
            let _native_context = Context::enter(Some(controlled.clone()));
            assert!(accept_navigation("http://127.0.0.1:3000/next"));
            assert_eq!(latest_document(&documents[0]), Some(controlled.clone()));
            assert!(accept_navigation("http://127.0.0.1:3000/link"));
            assert_ne!(
                latest_document(&documents[0]).unwrap().generation,
                controlled.generation
            );
            assert_eq!(latest_document(&documents[1]), Some(documents[1].clone()));
        }
        HOST.with(|state| state.borrow_mut().take());
    }
    #[test]
    fn devtools_without_native_target_preserves_profile_pages() {
        let owner = Owner {
            workspace: "workspace".to_owned().try_into().unwrap(),
            session: "session".to_owned().try_into().unwrap(),
        };
        let profile: ProfileId = "profile".to_owned().try_into().unwrap();
        let mut controller = Controller::new("test".to_owned(), true);
        let reply = controller.dispatch(
            &owner,
            Envelope {
                version: CONTRACT_VERSION,
                operation: "create".to_owned().try_into().unwrap(),
                command: Command::Create {
                    owner: owner.clone(),
                    browser: "page".to_owned().try_into().unwrap(),
                    profile: profile.clone(),
                    url: "https://example.com/".to_owned(),
                    title: String::new(),
                },
            },
        );
        let session = match reply.result {
            Ok(Event::State { session }) => Some(session),
            _ => None,
        }
        .expect("created target");
        let document = session.document.clone();
        let mut pages = BTreeMap::new();
        pages.insert(
            document.browser.clone(),
            Page {
                document: document.clone(),
                browser: None,
                window: None,
                pending_close: None,
                inspected: None,
                expected_navigation: Some(session.url),
                pending_agent_navigation: None,
                creating: false,
            },
        );
        let (_, receiver) = mpsc::channel();
        HOST.with(|state| {
            state.replace(Some(Host {
                controller,
                owner: owner.clone(),
                profile: Some(profile.clone()),
                receiver,
                pages,
                origin: "https://example.com".to_owned(),
                closing: false,
                tracing: false,
                trace_pending: false,
                trace_path: PathBuf::new(),
            }))
        });
        EMITTED.with(|events| events.borrow_mut().clear());
        process(Envelope {
            version: CONTRACT_VERSION,
            operation: "inspect".to_owned().try_into().unwrap(),
            command: Command::CreateDevTools {
                document: document.clone(),
                browser: "inspector".to_owned().try_into().unwrap(),
            },
        });
        HOST.with(|state| {
            let mut state = state.borrow_mut();
            let host = state.as_mut().expect("host preserved");
            assert!(!host.closing);
            assert_eq!(host.profile, Some(profile));
            assert_eq!(host.pages.len(), 1);
            assert_eq!(
                host.pages
                    .get(&document.browser)
                    .expect("target preserved")
                    .document,
                document
            );
            let unknown = host.controller.dispatch(
                &owner,
                Envelope {
                    version: CONTRACT_VERSION,
                    operation: "state".to_owned().try_into().unwrap(),
                    command: Command::State {
                        document: Document {
                            browser: "inspector".to_owned().try_into().unwrap(),
                            ..document.clone()
                        },
                    },
                },
            );
            assert_eq!(unknown.result.unwrap_err(), BrowserError::UnknownIdentity);
        });
        EMITTED.with(|events| {
            let events = events.borrow();
            let reply = events
                .iter()
                .find_map(|event| event.get("protocol"))
                .expect("explicit protocol reply");
            let reply: paneflow_browser_protocol::Reply =
                serde_json::from_value(reply.clone()).unwrap();
            assert_eq!(reply.result.unwrap_err(), BrowserError::Unavailable);
        });
        HOST.with(|state| state.borrow_mut().take());
    }
}
