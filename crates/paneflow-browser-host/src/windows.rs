use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{self, BufReader};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cef::*;
use interprocess::local_socket::{prelude::*, GenericFilePath, Stream};
use interprocess::TryClone;
use paneflow_browser_protocol::{
    read_value, write_message, BrowserPresentation, Command, Controller, Document, EditAction,
    Envelope, Event, FrameAck, InputEvent, KeyKind, MouseButton, OperationId, Owner, Reply,
    CONTRACT_VERSION,
};
use serde_json::{json, Value};

#[path = "linux/accessibility.rs"]
mod accessibility;
mod devtools;
#[path = "linux/devtools_renderer.rs"]
mod devtools_renderer;
mod editing;
#[path = "linux/external_protocols.rs"]
mod external_protocols;
#[path = "linux/permissions.rs"]
mod permissions;
mod presentation;
#[path = "linux/transfers.rs"]
mod transfers;
#[path = "linux/web_interactions.rs"]
mod web_interactions;

use presentation::Presenter;

static CONTROL_OUTPUT: OnceLock<Arc<Mutex<Stream>>> = OnceLock::new();
static CONTROL_SESSION: OnceLock<(Arc<Mutex<Controller>>, Owner)> = OnceLock::new();
static UI_COMMANDS: OnceLock<SyncSender<UiCommand>> = OnceLock::new();
static RESIZE_PUMP_SCHEDULED: AtomicBool = AtomicBool::new(false);
static LAST_PUMP: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static UI_RECEIVER: RefCell<Option<Receiver<UiCommand>>> = const { RefCell::new(None) };
    static PAGES: RefCell<BTreeMap<paneflow_browser_protocol::BrowserId, UiPage>> = const { RefCell::new(BTreeMap::new()) };
}

#[derive(Clone)]
struct ViewState {
    width: u32,
    height: u32,
    scale_percent: u32,
    visible: bool,
    refresh_ticks: u8,
    base_frame_rate: i32,
    resize_rate_until: Option<Instant>,
}

const RESIZE_REFRESH_TICKS: u8 = 16;

fn benchmark_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("PANEFLOW_BROWSER_BENCH").is_some())
}

fn resize_refresh_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("PANEFLOW_BROWSER_RESIZE_REFRESH").as_deref() != Ok("0"))
}

fn paint_probe_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("PANEFLOW_BROWSER_PAINT_PROBE").is_some())
}

pub(super) fn record_span(label: &str, start_ns: u64, threshold_ms: u64) {
    if !paint_probe_enabled() {
        return;
    }
    let end_ns = now_ns();
    if end_ns.saturating_sub(start_ns) < threshold_ms * 1_000_000 {
        return;
    }
    emit(json!({"native":"ui_span", "label":label,
        "start_ns":start_ns, "end_ns":end_ns}));
}

impl ViewState {
    fn begin_resize(&mut self, now: Instant) -> bool {
        self.refresh_ticks = RESIZE_REFRESH_TICKS;
        if !self.visible || self.base_frame_rate >= 120 {
            return false;
        }
        let boost = self.resize_rate_until.is_none();
        self.resize_rate_until = Some(now + Duration::from_millis(200));
        boost
    }

    fn tick(&mut self, now: Instant) -> (bool, Option<i32>) {
        let refresh = self.visible && self.refresh_ticks > 0;
        self.refresh_ticks = if self.visible {
            self.refresh_ticks.saturating_sub(1)
        } else {
            0
        };
        let restore = self
            .resize_rate_until
            .is_some_and(|deadline| !self.visible || now >= deadline);
        if restore {
            self.resize_rate_until = None;
        }
        (refresh, restore.then_some(self.base_frame_rate))
    }

    fn needs_tick(&self) -> bool {
        (self.visible && self.refresh_ticks > 0) || self.resize_rate_until.is_some()
    }

    fn physical_size(&self) -> (u32, u32) {
        (
            (self.width.max(1) * self.scale_percent.max(1)).div_ceil(100),
            (self.height.max(1) * self.scale_percent.max(1)).div_ceil(100),
        )
    }
}

struct UiPage {
    document: Document,
    browser: Option<Browser>,
    presenter: Rc<RefCell<Presenter>>,
    view: Arc<Mutex<ViewState>>,
    pending_agent_navigation: Option<OperationId>,
    expected_navigation: Option<String>,
}

enum UiCommand {
    BindInspector {
        inspector: Document,
        target: Document,
    },
    Create {
        session: paneflow_browser_protocol::BrowserSession,
        inspected: Option<Document>,
    },
    Present {
        document: Document,
        presentation: BrowserPresentation,
    },
    Navigate {
        document: Document,
        url: String,
        operation: Option<OperationId>,
    },
    Input {
        document: Document,
        input: InputEvent,
    },
    Action {
        document: Document,
        command: Command,
    },
    FrameAck(FrameAck),
    Shutdown,
}

pub(super) fn now_ns() -> u64 {
    use ::windows::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };
    let mut counter = 0;
    let mut frequency = 0;
    if unsafe { QueryPerformanceCounter(&mut counter) }.is_err()
        || unsafe { QueryPerformanceFrequency(&mut frequency) }.is_err()
        || counter < 0
        || frequency <= 0
    {
        return 0;
    }
    ((counter as u128 * 1_000_000_000) / frequency as u128) as u64
}

fn write_control(value: Value) -> io::Result<()> {
    let Some(output) = CONTROL_OUTPUT.get() else {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "browser control output is not initialized",
        ));
    };
    let mut output = output
        .lock()
        .map_err(|_| io::Error::other("browser control output lock poisoned"))?;
    write_message(&mut *output, &value)
}

fn emit(value: Value) {
    let _ = write_control(value);
}

pub(super) fn emit_frame(message: &paneflow_browser_protocol::FrameMessage) -> io::Result<()> {
    write_control(json!({ "frame": message }))
}

pub(super) fn emit_adapter(
    name: &str,
    vendor: u32,
    device: u32,
    dedicated_video_memory: u64,
    luid_high: i64,
    luid_low: u32,
    pinned: bool,
) {
    let value = json!({
        "description": name,
        "vendor_id": format!("0x{vendor:04x}"),
        "device_id": format!("0x{device:04x}"),
        "dedicated_video_memory_bytes": dedicated_video_memory,
        "luid": format!("{luid_high:x}-{luid_low:x}"),
        "pinned_to_application": pinned,
    });
    eprintln!("paneflow-browser-adapter {value}");
    emit(json!({ "native": "adapter", "value": value }));
}

fn latest_document(document: &Document) -> Option<Document> {
    PAGES.with(|pages| {
        pages
            .borrow()
            .get(&document.browser)
            .map(|page| page.document.clone())
    })
}

fn cancel_close(document: &Document) {
    emit(json!({ "native": "close_cancelled", "document": document }));
}

fn clear_web_state(document: &Document) {
    web_interactions::clear(document);
    permissions::clear(document);
    transfers::clear(document);
    external_protocols::clear(document);
}

fn enforce_certificate_policy(browser: &Browser) -> bool {
    let Some(context) = browser.host().and_then(|host| host.request_context()) else {
        return false;
    };
    let Some(mut denied) = value_create() else {
        return false;
    };
    denied.set_bool(0);
    let name: CefString = "ssl.error_override_allowed".into();
    let mut error = CefString::from("certificate policy preference");
    if context.set_preference(Some(&name), Some(&mut denied), Some(&mut error)) == 0 {
        return false;
    }
    context
        .preference(Some(&name))
        .is_some_and(|value| value.get_type() == ValueType::BOOL && value.bool() == 0)
}

fn owner() -> Result<Owner, String> {
    let value = std::env::var("PANEFLOW_BROWSER_OWNER")
        .map_err(|_| "PANEFLOW_BROWSER_OWNER is required".to_string())?;
    let (workspace, session) = value
        .split_once('/')
        .ok_or("PANEFLOW_BROWSER_OWNER must be workspace/session")?;
    Ok(Owner {
        workspace: workspace
            .to_owned()
            .try_into()
            .map_err(|_| "invalid browser workspace identity")?,
        session: session
            .to_owned()
            .try_into()
            .map_err(|_| "invalid browser session identity")?,
    })
}

fn enqueue(command: UiCommand) -> bool {
    let Some(sender) = UI_COMMANDS.get() else {
        return false;
    };
    if sender.send(command).is_err() {
        return false;
    }
    let _ = post_task(ThreadId::UI, Some(&mut DrainControl::new()));
    true
}

fn schedule_resize_pump() {
    if RESIZE_PUMP_SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut task = ResizePump::new();
    if post_delayed_task(ThreadId::UI, Some(&mut task), 16) == 0 {
        RESIZE_PUMP_SCHEDULED.store(false, Ordering::Release);
    }
}

fn dispatch(controller: &mut Controller, caller: &Owner, message: Envelope) -> Reply {
    controller.dispatch(caller, message)
}

fn queue_after_reply(
    command: Command,
    operation: OperationId,
    result: &Result<Event, paneflow_browser_protocol::BrowserError>,
) {
    if result.is_err() {
        return;
    }
    let queued_command = command.clone();
    let agent = matches!(command, Command::AgentNavigate { .. });
    match command {
        Command::Start { .. } => {
            if let Ok(Event::State { session }) = result {
                let _ = enqueue(UiCommand::Create {
                    session: session.clone(),
                    inspected: None,
                });
            }
        }
        Command::CreateDevTools { document, .. } => {
            if let Ok(Event::State { session }) = result {
                let _ = enqueue(UiCommand::BindInspector {
                    inspector: session.document.clone(),
                    target: document,
                });
            }
        }
        Command::Navigate { url, .. } | Command::AgentNavigate { url, .. } => {
            if let Ok(Event::State { session } | Event::NavigationStarted { session }) = result {
                let _ = enqueue(UiCommand::Navigate {
                    document: session.document.clone(),
                    url,
                    operation: agent.then_some(operation),
                });
            }
        }
        Command::Present {
            document,
            presentation,
        } => {
            let _ = enqueue(UiCommand::Present {
                document,
                presentation,
            });
        }
        Command::Input { document, input } => {
            let _ = enqueue(UiCommand::Input { document, input });
        }
        Command::History { document, .. }
        | Command::Reload { document, .. }
        | Command::Stop { document }
        | Command::Find { document, .. }
        | Command::StopFinding { document }
        | Command::Zoom { document, .. }
        | Command::Mute { document, .. }
        | Command::Screenshot { document }
        | Command::Sleep { document }
        | Command::Close { document } => {
            let _ = enqueue(UiCommand::Action {
                document,
                command: queued_command,
            });
        }
        _ => {}
    }
}

fn run_control(stream: Stream, owner: Owner) {
    let mut input = BufReader::new(stream);
    let controller = Arc::new(Mutex::new(Controller::new(
        format!("{}-windows", std::env::consts::ARCH),
        true,
    )));
    let _ = CONTROL_SESSION.set((controller.clone(), owner.clone()));
    loop {
        let value = match read_value(&mut input) {
            Ok(Some(value)) => value,
            Ok(None) | Err(_) => {
                let _ = enqueue(UiCommand::Shutdown);
                quit_message_loop();
                break;
            }
        };
        if let Some(frame) = value.get("frame") {
            if let Ok(ack) = serde_json::from_value::<FrameAck>(frame.clone()) {
                if !enqueue(UiCommand::FrameAck(ack)) {
                    quit_message_loop();
                    break;
                }
                continue;
            }
            quit_message_loop();
            break;
        }
        let Ok(message) = serde_json::from_value::<Envelope>(value) else {
            quit_message_loop();
            break;
        };
        let command = message.command.clone();
        let operation = message.operation.clone();
        let reply = {
            let Ok(mut controller) = controller.lock() else {
                quit_message_loop();
                break;
            };
            dispatch(&mut controller, &owner, message)
        };
        let result = reply.result.clone();
        if write_control(json!({ "protocol": reply })).is_err() {
            quit_message_loop();
            break;
        }
        queue_after_reply(command, operation, &result);
    }
}

fn page_browser(document: &Document) -> Option<Browser> {
    PAGES.with(|pages| {
        pages
            .borrow()
            .get(&document.browser)
            .filter(|page| page.document == *document)
            .and_then(|page| page.browser.clone())
    })
}

fn create_page(session: paneflow_browser_protocol::BrowserSession, inspected: Option<Document>) {
    let inspected = inspected.or_else(|| devtools::target(&session.document.browser));
    if CONTROL_OUTPUT.get().is_none() {
        return;
    }
    let frame_rate = match std::env::var("PANEFLOW_BROWSER_FRAME_RATE") {
        Ok(value) => match value.parse::<i32>() {
            Ok(rate @ (60 | 120)) => rate,
            _ => {
                emit(json!({
                    "native": "create_failed", "document": session.document,
                    "reason": "browser frame rate must be 60 or 120",
                }));
                return;
            }
        },
        Err(std::env::VarError::NotPresent) => 60,
        Err(_) => {
            emit(json!({
                "native": "create_failed", "document": session.document,
                "reason": "browser frame rate is not Unicode",
            }));
            return;
        }
    };
    let client_pid = std::env::var("PANEFLOW_BROWSER_CLIENT_PID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let presenter = match Presenter::new(session.document.clone(), client_pid) {
        Ok(presenter) => Rc::new(RefCell::new(presenter)),
        Err(error) => {
            emit(json!({
                "native": "create_failed",
                "document": session.document,
                "reason": error,
            }));
            return;
        }
    };
    let view = Arc::new(Mutex::new(ViewState {
        width: session.presentation.width.max(1),
        height: session.presentation.height.max(1),
        scale_percent: session.presentation.scale_percent.max(1),
        visible: true,
        refresh_ticks: RESIZE_REFRESH_TICKS,
        base_frame_rate: frame_rate,
        resize_rate_until: None,
    }));
    let browser_id = session.document.browser.clone();
    PAGES.with(|pages| {
        pages.borrow_mut().insert(
            browser_id,
            UiPage {
                document: session.document.clone(),
                browser: None,
                presenter: presenter.clone(),
                view: view.clone(),
                pending_agent_navigation: None,
                expected_navigation: None,
            },
        );
    });
    if let Some(target_document) = &inspected {
        let Some(target) = page_browser(target_document) else {
            remove_page(&session.document);
            emit(json!({
                "native": "create_failed",
                "document": session.document,
                "reason": "inspected browser is not available",
            }));
            return;
        };
        let Some(_host) = target.host() else {
            remove_page(&session.document);
            return;
        };
    }
    let window_info = WindowInfo {
        windowless_rendering_enabled: 1,
        shared_texture_enabled: 1,
        external_begin_frame_enabled: 0,
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    };
    let settings = BrowserSettings {
        windowless_frame_rate: frame_rate,
        ..Default::default()
    };
    let mut client = BrowserClient::new(session.document.clone(), presenter, view);
    let mut extra = inspected.as_ref().and_then(|_| dictionary_value_create());
    if let Some(info) = extra.as_mut() {
        info.set_bool(Some(&devtools::MARKER.into()), 1);
    }
    let url = if inspected.is_some() {
        devtools::URL
    } else {
        &session.url
    };
    let created = browser_host_create_browser(
        Some(&window_info),
        Some(&mut client),
        Some(&url.into()),
        Some(&settings),
        extra.as_mut(),
        None,
    ) == 1;
    if !created {
        remove_page(&session.document);
        emit(json!({
            "native": "create_failed",
            "document": session.document,
            "reason": "CEF rejected the windowless browser",
        }));
    } else {
        schedule_resize_pump();
    }
}

fn remove_page(document: &Document) {
    PAGES.with(|pages| {
        pages.borrow_mut().remove(&document.browser);
    });
}

fn present(document: Document, presentation: BrowserPresentation) {
    let started_ns = now_ns();
    let (browser, view, presenter) = PAGES.with(|pages| {
        let pages = pages.borrow();
        let Some(page) = pages.get(&document.browser) else {
            return (None, None, None);
        };
        if page.document != document {
            return (None, None, None);
        }
        (
            page.browser.clone(),
            Some(page.view.clone()),
            Some(page.presenter.clone()),
        )
    });
    let mut resized = false;
    let mut rescaled = false;
    let mut revealed = false;
    let mut boost = false;
    let mut physical_size = None;
    if let Some(view) = view {
        if let Ok(mut view) = view.lock() {
            resized = view.width != presentation.width.max(1)
                || view.height != presentation.height.max(1);
            rescaled = view.scale_percent != presentation.scale_percent.max(1);
            revealed = presentation.visible && !view.visible;
            view.width = presentation.width.max(1);
            view.height = presentation.height.max(1);
            view.scale_percent = presentation.scale_percent.max(1);
            view.visible = presentation.visible;
            if resized || rescaled {
                boost = view.begin_resize(Instant::now());
            } else if revealed {
                view.refresh_ticks = RESIZE_REFRESH_TICKS;
            }
            if view.visible && (resized || rescaled || revealed) {
                physical_size = Some(view.physical_size());
            }
        }
    }
    if let Some(browser) = browser {
        if let Some(host) = browser.host() {
            host.was_hidden(if presentation.visible { 0 } else { 1 });
            if rescaled {
                host.notify_screen_info_changed();
            }
            if resized || rescaled {
                if benchmark_enabled() {
                    emit(json!({"native":"resize_host", "document":document,
                        "at_ns":now_ns(), "generation":presentation.generation,
                        "width":presentation.width, "height":presentation.height}));
                }
                if boost {
                    host.set_windowless_frame_rate(120);
                }
                host.was_resized();
            }
            if let (Some(presenter), Some((width, height))) = (&presenter, physical_size) {
                if let Ok(mut presenter) = presenter.try_borrow_mut() {
                    presenter.prepare_resize(width, height);
                }
            }
            if presentation.visible && (resized || rescaled || revealed) {
                host.invalidate(PaintElementType::VIEW);
            }
        }
    }
    schedule_resize_pump();
    record_span("present", started_ns, 5);
}

fn navigate(document: Document, url: String, operation: Option<OperationId>) {
    if operation.is_none() {
        cancel_agent_navigation(&document, "human browser control");
    }
    devtools::closed(&document.browser);
    editing::cancel(&document.browser);
    clear_web_state(&document);
    let browser = PAGES.with(|pages| {
        let mut pages = pages.borrow_mut();
        let page = pages.get_mut(&document.browser)?;
        page.document = document.clone();
        page.pending_agent_navigation = operation;
        page.expected_navigation = Some(url.clone());
        if let Ok(mut presenter) = page.presenter.try_borrow_mut() {
            presenter.update_document(document.clone());
        }
        page.browser.clone()
    });
    if let Some(frame) = browser.and_then(|browser| browser.main_frame()) {
        frame.load_url(Some(&url.as_str().into()));
    }
}

fn take_pending_agent_navigation(document: &Document) -> Option<(Document, OperationId)> {
    PAGES.with(|pages| {
        let mut pages = pages.borrow_mut();
        let page = pages.get_mut(&document.browser)?;
        page.expected_navigation = None;
        page.pending_agent_navigation
            .take()
            .map(|operation| (page.document.clone(), operation))
    })
}

fn agent_navigation_failure_matches(document: &Document, url: Option<&str>) -> bool {
    PAGES.with(|pages| {
        let pages = pages.borrow();
        let Some(page) = pages.get(&document.browser) else {
            return false;
        };
        page.pending_agent_navigation.is_some()
            && url.is_none_or(|url| page.expected_navigation.as_deref() == Some(url))
    })
}

fn cancel_agent_navigation(document: &Document, reason: &str) {
    if let Some((document, operation)) = take_pending_agent_navigation(document) {
        emit(json!({
            "native": "agent_navigation_cancelled",
            "document": document,
            "operation": operation,
            "reason": reason
        }));
    }
}

fn agent_navigation_failed(document: &Document, reason: &str) {
    if let Some((document, operation)) = take_pending_agent_navigation(document) {
        emit(json!({
            "native": "agent_navigation_failed",
            "document": document,
            "operation": operation,
            "reason": reason
        }));
    }
}

fn agent_navigation_committed(document: &Document, url: &str) {
    if !agent_navigation_failure_matches(document, None) {
        return;
    }
    if paneflow_browser_protocol::validate_url(url).is_err() {
        agent_navigation_failed(document, "native navigation committed an invalid URL");
        return;
    }
    let Some((committed, operation)) = take_pending_agent_navigation(document) else {
        return;
    };
    let Some((controller, owner)) = CONTROL_SESSION.get() else {
        emit(json!({
            "native": "agent_navigation_failed",
            "document": committed,
            "operation": operation,
            "reason": "navigation commit controller unavailable"
        }));
        return;
    };
    let Ok(commit) = "native-agent-commit".to_owned().try_into() else {
        return;
    };
    let reply = controller.lock().ok().map(|mut controller| {
        controller.dispatch(
            owner,
            Envelope {
                version: CONTRACT_VERSION,
                operation: commit,
                command: Command::Navigate {
                    document: committed.clone(),
                    url: url.to_owned(),
                },
            },
        )
    });
    let Some(Ok(Event::State { session })) = reply.map(|reply| reply.result) else {
        emit(json!({
            "native": "agent_navigation_failed",
            "document": committed,
            "operation": operation,
            "reason": "navigation commit rejected"
        }));
        return;
    };
    PAGES.with(|pages| {
        let mut pages = pages.borrow_mut();
        if let Some(page) = pages.get_mut(&session.document.browser) {
            page.document = session.document.clone();
            page.expected_navigation = None;
            if let Ok(mut presenter) = page.presenter.try_borrow_mut() {
                presenter.update_document(session.document.clone());
            }
        }
    });
    emit(json!({
        "native": "agent_navigation_committed",
        "document": session.document,
        "session": session,
        "operation": operation,
        "url": url
    }));
}

fn input(document: Document, input: InputEvent) {
    cancel_agent_navigation(&document, "human browser control");
    let Some(browser) = page_browser(&document) else {
        return;
    };
    let Some(host) = browser.host() else {
        return;
    };
    match input {
        InputEvent::MouseMove { x, y, modifiers } => {
            host.send_mouse_move_event(Some(&MouseEvent { x, y, modifiers }), 0);
        }
        InputEvent::MouseLeave { x, y, modifiers } => {
            host.send_mouse_move_event(Some(&MouseEvent { x, y, modifiers }), 1);
        }
        InputEvent::MouseButton {
            x,
            y,
            button,
            down,
            clicks,
            modifiers,
        } => {
            host.send_mouse_click_event(
                Some(&MouseEvent { x, y, modifiers }),
                mouse_button(button),
                if down { 0 } else { 1 },
                clicks.into(),
            );
        }
        InputEvent::MouseWheel {
            x,
            y,
            delta_x,
            delta_y,
            modifiers,
        } => {
            host.send_mouse_wheel_event(Some(&MouseEvent { x, y, modifiers }), delta_x, delta_y);
        }
        InputEvent::Key {
            kind,
            key_code,
            native_key_code,
            character,
            unmodified_character,
            modifiers,
        } => {
            let type_ = match kind {
                KeyKind::RawDown => KeyEventType::RAWKEYDOWN,
                KeyKind::Down => KeyEventType::KEYDOWN,
                KeyKind::Up => KeyEventType::KEYUP,
                KeyKind::Char => KeyEventType::CHAR,
            };
            host.send_key_event(Some(&KeyEvent {
                type_,
                modifiers,
                windows_key_code: key_code,
                native_key_code,
                character,
                unmodified_character,
                ..Default::default()
            }));
        }
        InputEvent::Focus { focused } => {
            if !focused {
                editing::cancel(&document.browser);
                host.ime_cancel_composition();
                host.send_capture_lost_event();
            }
            host.set_focus(if focused { 1 } else { 0 });
        }
        InputEvent::ImeComposition {
            text,
            cursor,
            selection_start,
            replacement,
        } => {
            let selection = Range {
                from: selection_start.unwrap_or(cursor),
                to: cursor,
            };
            let replacement = replacement
                .map(|[from, to]| Range { from, to })
                .unwrap_or(Range {
                    from: u32::MAX,
                    to: u32::MAX,
                });
            host.ime_set_composition(
                Some(&text.as_str().into()),
                None,
                Some(&replacement),
                Some(&selection),
            );
        }
        InputEvent::ImeCommit { text, replacement } => {
            let replacement = replacement
                .map(|[from, to]| Range { from, to })
                .unwrap_or(Range {
                    from: u32::MAX,
                    to: u32::MAX,
                });
            host.ime_commit_text(Some(&text.as_str().into()), Some(&replacement), 0);
        }
        InputEvent::ImeCancel => host.ime_cancel_composition(),
        InputEvent::ImeFinish => host.ime_finish_composing_text(1),
        InputEvent::Edit { action, .. } => {
            let Some(frame) = browser.focused_frame() else {
                return;
            };
            match action {
                EditAction::Copy => frame.copy(),
                EditAction::Cut => frame.cut(),
                EditAction::Paste { text } => {
                    host.ime_commit_text(
                        Some(&text.as_str().into()),
                        Some(&Range {
                            from: u32::MAX,
                            to: u32::MAX,
                        }),
                        0,
                    );
                }
                EditAction::SelectAll => frame.select_all(),
                EditAction::Undo => frame.undo(),
                EditAction::Redo => frame.redo(),
            }
        }
        InputEvent::CaptureLost => {
            editing::cancel(&document.browser);
            host.ime_cancel_composition();
            host.send_capture_lost_event();
        }
        InputEvent::ContextMenu { request, command } => {
            editing::choose(&document, request, command)
        }
        response @ InputEvent::WebResponse { .. } => {
            web_interactions::handle(&document, &response);
            permissions::handle(&document, &response);
            external_protocols::handle(&document, &response);
        }
        response @ (InputEvent::TransferResponse { .. } | InputEvent::CancelDownload { .. }) => {
            transfers::handle(&document, &response);
        }
        InputEvent::DropFiles { .. } | InputEvent::ClipboardWritten { .. } => {}
    }
}

fn mouse_button(button: MouseButton) -> MouseButtonType {
    match button {
        MouseButton::Left => MouseButtonType::LEFT,
        MouseButton::Middle => MouseButtonType::MIDDLE,
        MouseButton::Right => MouseButtonType::RIGHT,
    }
}

fn action(document: Document, command: Command) {
    if matches!(
        command,
        Command::History { .. }
            | Command::Reload { .. }
            | Command::Stop { .. }
            | Command::Close { .. }
            | Command::Sleep { .. }
    ) {
        cancel_agent_navigation(&document, "human browser control");
    }
    let Some(browser) = page_browser(&document) else {
        return;
    };
    match command {
        Command::History { direction, .. } => match direction {
            paneflow_browser_protocol::HistoryDirection::Back => browser.go_back(),
            paneflow_browser_protocol::HistoryDirection::Forward => browser.go_forward(),
        },
        Command::Reload { ignore_cache, .. } => {
            if ignore_cache {
                browser.reload_ignore_cache();
            } else {
                browser.reload();
            }
        }
        Command::Stop { .. } => browser.stop_load(),
        Command::Find {
            text,
            forward,
            find_next,
            ..
        } => {
            if let Some(host) = browser.host() {
                host.find(
                    Some(&text.as_str().into()),
                    if forward { 1 } else { 0 },
                    0,
                    if find_next { 1 } else { 0 },
                );
            }
        }
        Command::StopFinding { .. } => {
            if let Some(host) = browser.host() {
                host.stop_finding(1);
            }
        }
        Command::Zoom { percent, .. } => {
            if let Some(host) = browser.host() {
                host.set_zoom_level((f64::from(percent) / 100.0).log(1.2));
            }
        }
        Command::Mute { muted, .. } => {
            if let Some(host) = browser.host() {
                host.set_audio_muted(if muted { 1 } else { 0 });
            }
        }
        Command::Screenshot { document } => {
            emit(json!({
                "native": "screenshot_failed",
                "document": document,
                "reason": "native capture adapter unavailable",
            }));
        }
        Command::Sleep { document } | Command::Close { document } => {
            editing::cancel(&document.browser);
            if let Some(host) = browser.host() {
                host.close_browser(0);
            } else {
                remove_page(&document);
            }
        }
        _ => {}
    }
}

fn apply_ui_command(command: UiCommand) {
    match command {
        UiCommand::BindInspector { inspector, target } => devtools::bind(inspector.browser, target),
        UiCommand::Create { session, inspected } => create_page(session, inspected),
        UiCommand::Present {
            document,
            presentation,
        } => present(document, presentation),
        UiCommand::Navigate {
            document,
            url,
            operation,
        } => navigate(document, url, operation),
        UiCommand::Input {
            document,
            input: event,
        } => input(document, event),
        UiCommand::Action { document, command } => action(document, command),
        UiCommand::FrameAck(ack) => {
            let mut refresh = matches!(ack, FrameAck::PoolReady { .. });
            let document = match &ack {
                FrameAck::PoolReady { document, .. }
                | FrameAck::PoolRejected { document, .. }
                | FrameAck::Release { document, .. } => document,
            }
            .clone();
            PAGES.with(|pages| {
                let pages = pages.borrow();
                if let Some(page) = pages.get(&document.browser) {
                    if page.document.owner == document.owner
                        && page.document.generation >= document.generation
                    {
                        if let Ok(mut presenter) = page.presenter.try_borrow_mut() {
                            refresh |= presenter.handle_ack(ack);
                        }
                        if refresh {
                            if let Some(browser) = &page.browser {
                                if let Some(host) = browser.host() {
                                    host.invalidate(PaintElementType::VIEW);
                                }
                            }
                        }
                    }
                }
            });
        }
        UiCommand::Shutdown => {
            let browsers = PAGES.with(|pages| {
                pages
                    .borrow()
                    .values()
                    .filter_map(|page| page.browser.clone())
                    .collect::<Vec<_>>()
            });
            if browsers.is_empty() {
                quit_message_loop();
            } else {
                for browser in browsers {
                    if let Some(host) = browser.host() {
                        host.close_browser(1);
                    }
                }
            }
        }
    }
}

wrap_task! {
    struct DrainControl;

    impl Task {
        fn execute(&self) {
            for _ in 0..128 {
                let next = UI_RECEIVER.with(|receiver| {
                    receiver
                        .borrow()
                        .as_ref()
                        .map(Receiver::try_recv)
                });
                match next {
                    Some(Ok(command)) => apply_ui_command(command),
                    Some(Err(TryRecvError::Empty)) => return,
                    Some(Err(TryRecvError::Disconnected)) | None => {
                        quit_message_loop();
                        return;
                    }
                }
            }
            let _ = post_task(ThreadId::UI, Some(&mut DrainControl::new()));
        }
    }
}

wrap_task! {
    struct ResizePump;

    impl Task {
        fn execute(&self) {
            RESIZE_PUMP_SCHEDULED.store(false, Ordering::Release);
            if paint_probe_enabled() {
                let now = now_ns();
                let previous = LAST_PUMP.swap(now, Ordering::AcqRel);
                if previous > 0 && now.saturating_sub(previous) > 25_000_000 {
                    emit(json!({"native":"ui_gap", "label":"resize_pump",
                        "start_ns":previous, "end_ns":now}));
                }
            }
            let active = PAGES.with(|pages| {
                let pages = pages.borrow();
                let mut active = false;
                for page in pages.values() {
                    if let Some(browser) = &page.browser {
                        if let Some(host) = browser.host() {
                            let (refresh, restore) = match page.view.lock() {
                                Ok(mut view) => {
                                    let actions = view.tick(Instant::now());
                                    active |= view.needs_tick();
                                    actions
                                }
                                Err(_) => (false, None),
                            };
                            if let Some(rate) = restore {
                                host.set_windowless_frame_rate(rate);
                            }
                            if refresh && resize_refresh_enabled() {
                                host.invalidate(PaintElementType::VIEW);
                            }
                        }
                    }
                }
                active
            });
            if active {
                schedule_resize_pump();
            }
        }
    }
}

wrap_client! {
    struct BrowserClient {
        document: Document,
        presenter: Rc<RefCell<Presenter>>,
        view: Arc<Mutex<ViewState>>,
    }

    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(Life::new(self.document.clone())) }
        fn render_handler(&self) -> Option<RenderHandler> { Some(Renderer::new(self.presenter.clone(), self.view.clone())) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(Load::new(self.document.clone())) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(Display::new(self.document.clone())) }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> { Some(editing::Menus::new(self.presenter.clone())) }
        fn dialog_handler(&self) -> Option<DialogHandler> { Some(transfers::Uploads::new(self.document.clone())) }
        fn download_handler(&self) -> Option<DownloadHandler> { Some(transfers::Downloads::new(self.document.clone())) }
        fn permission_handler(&self) -> Option<PermissionHandler> { Some(permissions::Permissions::new(self.document.clone())) }
        fn jsdialog_handler(&self) -> Option<JsdialogHandler> { Some(web_interactions::Dialogs::new(self.document.clone())) }
        fn find_handler(&self) -> Option<FindHandler> { Some(FindResults::new(self.document.clone())) }
        fn request_handler(&self) -> Option<RequestHandler> {
            match devtools::target(&self.document.browser) {
                Some(_) => Some(devtools::Requests::new()),
                None => Some(RequestPolicy::new(self.document.clone())),
            }
        }
        fn on_process_message_received(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, source: ProcessId, message: Option<&mut ProcessMessage>) -> i32 {
            message.is_some_and(|message| devtools::message(&self.document, browser.as_deref(), frame.as_deref(), source, message)) as i32
        }
    }
}

wrap_life_span_handler! {
    struct Life { document: Document }

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            PAGES.with(|pages| {
                if let Some(page) = pages.borrow_mut().get_mut(&self.document.browser) {
                    page.browser = Some(browser.clone());
                }
            });
            if !devtools::created(&self.document, browser) {
                if let Some(host) = browser.host() { host.close_browser(1); }
                return;
            }
            if devtools::target(&self.document.browser).is_none() && !enforce_certificate_policy(browser) {
                emit(json!({
                    "native": "create_failed",
                    "document": self.document,
                    "reason": "Strict certificate policy unavailable",
                }));
                if let Some(host) = browser.host() { host.close_browser(1); }
                return;
            }
            if let Some(host) = browser.host() {
                host.set_accessibility_state(State::ENABLED);
                host.was_hidden(0);
                host.notify_screen_info_changed();
                host.was_resized();
                host.invalidate(PaintElementType::VIEW);
            }
            schedule_resize_pump();
            emit(json!({
                "native": "created",
                "document": self.document,
                "cef_browser_id": browser.identifier().to_string(),
            }));
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> i32 { 0 }

        fn on_before_popup(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _popup_id: i32, target_url: Option<&CefString>, _target_frame_name: Option<&CefString>, _target_disposition: WindowOpenDisposition, user_gesture: i32, _popup_features: Option<&PopupFeatures>, _window_info: Option<&mut WindowInfo>, _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>, _extra_info: Option<&mut Option<DictionaryValue>>, _no_javascript_access: Option<&mut i32>) -> i32 {
            let url = target_url.map(ToString::to_string).unwrap_or_default();
            let allowed = user_gesture != 0
                && devtools::target(&self.document.browser).is_none()
                && paneflow_browser_protocol::validate_url(&url).is_ok();
            if let Some(document) = latest_document(&self.document).filter(|_| allowed) {
                emit(json!({ "native": "popup_requested", "document": document, "url": url }));
            }
            1
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            devtools::closed(&self.document.browser);
            editing::cancel(&self.document.browser);
            clear_web_state(&self.document);
            PAGES.with(|pages| {
                pages.borrow_mut().remove(&self.document.browser);
            });
            emit(json!({ "native": "closed", "document": self.document }));
            let remaining = PAGES.with(|pages| pages.borrow().values().any(|page| page.browser.is_some()));
            if !remaining {
                quit_message_loop();
            }
        }
    }
}

wrap_load_handler! {
    struct Load { document: Document }

    impl LoadHandler {
        fn on_loading_state_change(&self, _browser: Option<&mut Browser>, is_loading: i32, can_go_back: i32, can_go_forward: i32) {
            emit(json!({
                "native": "loading",
                "document": self.document,
                "is_loading": is_loading != 0,
                "can_go_back": can_go_back != 0,
                "can_go_forward": can_go_forward != 0,
            }));
        }

        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status_code: i32) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() != 0) {
                agent_navigation_committed(&self.document, &CefString::from(&frame.url()).to_string());
                emit(json!({
                    "native": "loaded",
                    "document": self.document,
                    "http_status": http_status_code,
                }));
            }
        }

        fn on_load_error(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, error_code: Errorcode, error_text: Option<&CefString>, failed_url: Option<&CefString>) {
            if frame.is_none_or(|frame| frame.is_main() != 0) {
                let error_text: String = error_text.map(ToString::to_string).unwrap_or_default().chars().take(256).collect();
                let failed_url = failed_url.map(ToString::to_string);
                let agent_pending = agent_navigation_failure_matches(&self.document, None);
                if agent_navigation_failure_matches(&self.document, failed_url.as_deref()) {
                    agent_navigation_failed(&self.document, &error_text);
                }
                if !(agent_pending && error_code.get_raw() == -3) {
                    emit(json!({
                        "native": "load_failed",
                        "document": self.document,
                        "error_code": error_code.get_raw(),
                        "error_text": error_text,
                    }));
                }
            }
        }
    }
}

wrap_display_handler! {
    struct Display { document: Document }

    impl DisplayHandler {
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            emit(json!({ "native": "title", "document": self.document, "title": title.map(ToString::to_string).unwrap_or_default() }));
        }

        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.is_some_and(|frame| frame.is_main() != 0) {
                emit(json!({ "native": "address", "document": self.document, "url": url.map(ToString::to_string).unwrap_or_default() }));
            }
        }

        fn on_fullscreen_mode_change(&self, _browser: Option<&mut Browser>, fullscreen: i32) {
            emit(json!({ "native": "fullscreen", "document": self.document, "enabled": fullscreen != 0 }));
        }

        fn on_cursor_change(&self, _browser: Option<&mut Browser>, _cursor: sys::HCURSOR, type_: CursorType, _custom_cursor_info: Option<&CursorInfo>) -> i32 {
            emit(json!({ "native": "cursor", "document": self.document, "style": cursor_style(type_) }));
            1
        }

        fn on_console_message(&self, _browser: Option<&mut Browser>, level: LogSeverity, message: Option<&CefString>, source: Option<&CefString>, line: i32) -> i32 {
            let text = message.map(ToString::to_string).unwrap_or_default();
            if let Some(document) = latest_document(&self.document) {
                emit(json!({
                    "native": "agent_console",
                    "document": document,
                    "value": {
                        "level": level.get_raw(),
                        "message": text.chars().take(4096).collect::<String>(),
                        "source": source.map(ToString::to_string).unwrap_or_default().chars().take(1024).collect::<String>(),
                        "line": line.max(0),
                    }
                }));
            }
            if let Some(state) = text.strip_prefix("PANEFLOW_FIXTURE:") {
                if let Ok(value) = serde_json::from_str::<Value>(state) {
                    emit(json!({ "native": "fixture_state", "document": self.document, "state": value }));
                }
            }
            0
        }
    }
}

wrap_render_handler! {
    struct Renderer {
        presenter: Rc<RefCell<Presenter>>,
        view: Arc<Mutex<ViewState>>,
    }

    impl RenderHandler {
        fn accessibility_handler(&self) -> Option<AccessibilityHandler> {
            let Ok(presenter) = self.presenter.try_borrow() else { return None };
            Some(accessibility::Accessibility::new(presenter.document().clone()))
        }

        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            let Some(rect) = rect else { return };
            if let Ok(view) = self.view.lock() {
                rect.width = i32::try_from(view.width.max(1)).unwrap_or(i32::MAX);
                rect.height = i32::try_from(view.height.max(1)).unwrap_or(i32::MAX);
            }
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, screen_info: Option<&mut ScreenInfo>) -> i32 {
            let Some(screen_info) = screen_info else { return 0 };
            if let Ok(view) = self.view.lock() {
                screen_info.device_scale_factor = view.scale_percent.max(1) as f32 / 100.0;
                screen_info.depth = 24;
                screen_info.depth_per_component = 8;
                screen_info.is_monochrome = 0;
                screen_info.rect = Rect {
                    x: 0,
                    y: 0,
                    width: i32::try_from(view.width.max(1)).unwrap_or(i32::MAX),
                    height: i32::try_from(view.height.max(1)).unwrap_or(i32::MAX),
                };
                screen_info.available_rect = screen_info.rect.clone();
                return 1;
            }
            0
        }

        fn on_accelerated_paint(&self, _browser: Option<&mut Browser>, type_: PaintElementType, dirty_rects: Option<&[Rect]>, info: Option<&AcceleratedPaintInfo>) {
            let Some(info) = info else { return };
            let started_ns = now_ns();
            let published = match self.presenter.try_borrow_mut() {
                Ok(mut presenter) => presenter.paint(type_, dirty_rects, info),
                Err(_) => false,
            };
            record_span("paint", started_ns, 5);
            if !published {
                return;
            }
            if let Ok(mut view) = self.view.lock() {
                let (width, height) = view.physical_size();
                if u32::try_from(info.extra.coded_size.width).unwrap_or(0) == width
                    && u32::try_from(info.extra.coded_size.height).unwrap_or(0) == height
                {
                    if view.refresh_ticks > 0 && benchmark_enabled() {
                        if let Ok(presenter) = self.presenter.try_borrow() {
                            emit(json!({"native":"resize_capture", "document":presenter.document(),
                                "at_ns":now_ns(), "width":width, "height":height}));
                        }
                    }
                    view.refresh_ticks = 0;
                }
            }
        }

        fn on_text_selection_changed(&self, _browser: Option<&mut Browser>, selected_text: Option<&CefString>, selected_range: Option<&Range>) {
            let Some(range) = selected_range else { return };
            let Ok(presenter) = self.presenter.try_borrow() else { return };
            emit(json!({
                "native": "ime_selection",
                "document": presenter.document(),
                "snapshot": { "start": range.from, "end": range.to, "text": selected_text.map(ToString::to_string).filter(|text| text.len() <= 65536) },
            }));
        }

        fn on_ime_composition_range_changed(&self, _browser: Option<&mut Browser>, selected_range: Option<&Range>, character_bounds: Option<&[Rect]>) {
            let Some(range) = selected_range else { return };
            let Ok(presenter) = self.presenter.try_borrow() else { return };
            let bounds = character_bounds.unwrap_or_default().iter().take(4096).map(|rect| [rect.x, rect.y, rect.width, rect.height]).collect::<Vec<_>>();
            emit(json!({
                "native": "ime_bounds",
                "document": presenter.document(),
                "snapshot": { "start": range.from, "end": range.to, "bounds": bounds },
            }));
        }
    }
}

fn cursor_style(type_: CursorType) -> &'static str {
    match type_ {
        CursorType::HAND => "PointingHand",
        CursorType::IBEAM => "IBeam",
        CursorType::CROSS | CursorType::CELL => "Crosshair",
        CursorType::GRAB | CursorType::MOVE => "OpenHand",
        CursorType::GRABBING => "ClosedHand",
        CursorType::EASTRESIZE => "ResizeRight",
        CursorType::WESTRESIZE => "ResizeLeft",
        CursorType::NORTHRESIZE => "ResizeUp",
        CursorType::SOUTHRESIZE => "ResizeDown",
        CursorType::NORTHSOUTHRESIZE => "ResizeUpDown",
        CursorType::EASTWESTRESIZE => "ResizeLeftRight",
        CursorType::NORTHEASTRESIZE
        | CursorType::SOUTHWESTRESIZE
        | CursorType::NORTHEASTSOUTHWESTRESIZE => "ResizeUpRightDownLeft",
        CursorType::NORTHWESTRESIZE
        | CursorType::SOUTHEASTRESIZE
        | CursorType::NORTHWESTSOUTHEASTRESIZE => "ResizeUpLeftDownRight",
        CursorType::COLUMNRESIZE => "ResizeColumn",
        CursorType::ROWRESIZE => "ResizeRow",
        CursorType::VERTICALTEXT => "IBeamCursorForVerticalLayout",
        CursorType::NODROP | CursorType::NOTALLOWED => "OperationNotAllowed",
        CursorType::ALIAS => "DragLink",
        CursorType::COPY => "DragCopy",
        CursorType::CONTEXTMENU => "ContextualMenu",
        _ => "Arrow",
    }
}

wrap_find_handler! {
    struct FindResults { document: Document }

    impl FindHandler {
        fn on_find_result(&self, _browser: Option<&mut Browser>, _identifier: i32, count: i32, _selection_rect: Option<&Rect>, active_match_ordinal: i32, final_update: i32) {
            emit(json!({ "native": "find_result", "document": self.document, "count": count, "active": active_match_ordinal, "final": final_update != 0 }));
        }
    }
}

fn agent_network_event(
    document: &Document,
    request: Option<&mut Request>,
    response: Option<&mut Response>,
    status: Option<i32>,
    received_content_length: Option<i64>,
) {
    let Some(document) = latest_document(document) else {
        return;
    };
    let Some(request) = request else {
        return;
    };
    let raw_url = CefString::from(&request.url()).to_string();
    let Some(url) = paneflow_browser_protocol::exported_url(&raw_url) else {
        return;
    };
    let mut value = json!({
        "request_id": request.identifier(),
        "method": CefString::from(&request.method()).to_string(),
        "resource_type": request.resource_type().get_raw(),
        "url": url,
    });
    if let Some(status) = status {
        value["status"] = status.into();
    }
    if let Some(received_content_length) = received_content_length {
        value["received_content_length"] = received_content_length.into();
    }
    if let Some(response) = response {
        value["mime_type"] = CefString::from(&response.mime_type())
            .to_string()
            .chars()
            .take(256)
            .collect::<String>()
            .into();
    }
    emit(json!({ "native": "agent_network", "document": document, "value": value }));
}

wrap_resource_request_handler! {
    struct AgentResources { document: Document }

    impl ResourceRequestHandler {
        fn on_before_resource_load(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>, _callback: Option<&mut Callback>) -> ReturnValue {
            agent_network_event(&self.document, request, None, None, None);
            ReturnValue::CONTINUE
        }

        fn on_resource_load_complete(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>, response: Option<&mut Response>, status: UrlrequestStatus, received_content_length: i64) {
            agent_network_event(&self.document, request, response, Some(status.get_raw()), Some(received_content_length.max(0)));
        }
    }
}

wrap_request_handler! {
    struct RequestPolicy { document: Document }

    impl RequestHandler {
        fn resource_request_handler(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _request: Option<&mut Request>, _is_navigation: i32, _is_download: i32, _request_initiator: Option<&CefString>, _disable_default_handling: Option<&mut i32>) -> Option<ResourceRequestHandler> {
            Some(AgentResources::new(self.document.clone()))
        }

        fn on_certificate_error(&self, _browser: Option<&mut Browser>, cert_error: Errorcode, _request_url: Option<&CefString>, _ssl_info: Option<&mut Sslinfo>, callback: Option<&mut Callback>) -> i32 {
            emit(json!({ "native": "certificate_error", "document": self.document, "error_code": cert_error.get_raw() }));
            if let Some(callback) = callback { callback.cancel(); }
            1
        }

        fn on_render_process_terminated(&self, _browser: Option<&mut Browser>, status: TerminationStatus, error_code: i32, _error_string: Option<&CefString>) {
            editing::cancel(&self.document.browser);
            clear_web_state(&self.document);
            emit(json!({ "native": "renderer_crashed", "document": self.document, "status": status.get_raw(), "error_code": error_code }));
        }

        fn on_before_browse(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, request: Option<&mut Request>, user_gesture: i32, is_redirect: i32) -> i32 {
            let url = request.map(|request| CefString::from(&request.url()).to_string()).unwrap_or_default();
            let Some(document) = latest_document(&self.document) else { return 1 };
            if paneflow_browser_protocol::validate_url(&url).is_err() {
                if user_gesture != 0 && is_redirect == 0 {
                    let origin = browser.and_then(|browser| browser.main_frame()).map(|frame| CefString::from(&frame.url()).to_string()).unwrap_or_default();
                    external_protocols::request(&document, &origin, &url);
                }
                return 1;
            }
            if frame.is_some_and(|frame| frame.is_main() != 0) {
                editing::cancel(&document.browser);
                clear_web_state(&document);
            }
            0
        }
    }
}

fn connect_control() -> Result<Stream, String> {
    let path = std::env::var_os("PANEFLOW_BROWSER_CONTROL_PIPE")
        .ok_or("PANEFLOW_BROWSER_CONTROL_PIPE is required")?;
    let path = std::path::PathBuf::from(path);
    let name = path
        .to_fs_name::<GenericFilePath>()
        .map_err(|error| format!("browser named-pipe name: {error}"))?;
    Stream::connect(name).map_err(|error| format!("browser named-pipe connect: {error}"))
}

fn run(instance: sys::HINSTANCE, sandbox_info: *mut u8) -> i32 {
    let main_args = MainArgs { instance };
    let args = cef::args::Args::from(main_args);
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let Some(_) = args.as_cmd_line() else {
        return 1;
    };
    let mut application = BrowserApp::new();
    let exit_code = execute_process(
        Some(args.as_main_args()),
        Some(&mut application),
        sandbox_info,
    );
    if exit_code >= 0 {
        return exit_code;
    }
    let profile = std::env::var_os("PANEFLOW_CEF_PROFILE").map(std::path::PathBuf::from);
    let runtime = std::env::var_os("PANEFLOW_CEF_ROOT").map(std::path::PathBuf::from);
    let release = runtime.map(|path| path.join("Release"));
    let settings = Settings {
        no_sandbox: 0,
        command_line_args_disabled: 1,
        windowless_rendering_enabled: 1,
        root_cache_path: profile
            .as_ref()
            .map(|path| path.to_string_lossy().as_ref().into())
            .unwrap_or_default(),
        cache_path: profile
            .as_ref()
            .map(|path| path.join("profile").to_string_lossy().as_ref().into())
            .unwrap_or_default(),
        resources_dir_path: release
            .as_ref()
            .map(|path| path.to_string_lossy().as_ref().into())
            .unwrap_or_default(),
        locales_dir_path: release
            .as_ref()
            .map(|path| path.join("locales").to_string_lossy().as_ref().into())
            .unwrap_or_default(),
        ..Default::default()
    };
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut application),
        sandbox_info,
    ) != 1
    {
        return 1;
    }
    let stream = match connect_control() {
        Ok(stream) => stream,
        Err(_) => {
            shutdown();
            return 1;
        }
    };
    let owner = match owner() {
        Ok(owner) => owner,
        Err(_) => {
            shutdown();
            return 1;
        }
    };
    let (ui_sender, ui_receiver) = mpsc::sync_channel(256);
    if UI_COMMANDS.set(ui_sender).is_err() {
        shutdown();
        return 1;
    }
    UI_RECEIVER.with(|receiver| receiver.replace(Some(ui_receiver)));
    let mut controller = Controller::new(format!("{}-windows", std::env::consts::ARCH), true);
    let handshake = Envelope {
        version: CONTRACT_VERSION,
        operation: "hello"
            .to_owned()
            .try_into()
            .unwrap_or_else(|_| unreachable!()),
        command: Command::Capabilities,
    };
    let hello = dispatch(&mut controller, &owner, handshake);
    let output = Arc::new(Mutex::new(match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => {
            shutdown();
            return 1;
        }
    }));
    if CONTROL_OUTPUT.set(output).is_err() {
        shutdown();
        return 1;
    }
    if write_control(json!({
        "native": "initialized",
        "pid": std::process::id(),
        "sandbox_requested": true,
        "contract_version": CONTRACT_VERSION,
        "presentation": std::env::var("PANEFLOW_BROWSER_FRAMES").as_deref() == Ok("1"),
        "protocol": hello,
    }))
    .is_err()
    {
        shutdown();
        return 1;
    }
    std::thread::spawn(move || run_control(stream, owner));
    run_message_loop();
    shutdown();
    0
}

wrap_app! {
    struct BrowserApp;

    impl App {
        fn render_process_handler(&self) -> Option<RenderProcessHandler> { Some(devtools::Renderer::new()) }
        fn on_before_command_line_processing(&self, _process_type: Option<&CefString>, command_line: Option<&mut CommandLine>) {
            let Some(adapter) = presentation::requested_adapter_switch() else { return };
            if let Some(command_line) = command_line {
                command_line.append_switch_with_value(Some(&"use-adapter-luid".into()), Some(&adapter.as_str().into()));
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn RunWinMain(
    instance: sys::HINSTANCE,
    _command_line: *const u8,
    _command_show: i32,
    sandbox_info: *mut u8,
) -> i32 {
    run(instance, sandbox_info)
}

#[cfg(test)]
mod tests {
    use super::{Duration, Instant, ViewState};

    fn view(frame_rate: i32) -> ViewState {
        ViewState {
            width: 800,
            height: 600,
            scale_percent: 100,
            visible: true,
            refresh_ticks: 0,
            base_frame_rate: frame_rate,
            resize_rate_until: None,
        }
    }

    #[test]
    fn an_idle_page_needs_no_resize_pump() {
        let mut view = view(60);
        assert!(!view.needs_tick());
        assert_eq!(view.tick(Instant::now()), (false, None));
    }

    #[test]
    fn a_resize_burst_extends_the_boost_until_the_latest_size_settles() {
        let mut view = view(60);
        let start = Instant::now();
        assert!(view.begin_resize(start));
        assert!(!view.begin_resize(start + Duration::from_millis(150)));
        view.refresh_ticks = 0;
        assert_eq!(view.tick(start + Duration::from_millis(200)), (false, None));
        assert!(view.needs_tick());
        assert_eq!(
            view.tick(start + Duration::from_millis(350)),
            (false, Some(60))
        );
        assert!(!view.needs_tick());
    }

    #[test]
    fn hiding_a_page_cancels_refresh_and_restores_its_base_rate() {
        let mut view = view(60);
        let start = Instant::now();
        assert!(view.begin_resize(start));
        view.visible = false;
        assert_eq!(view.tick(start), (false, Some(60)));
        assert!(!view.needs_tick());
    }

    #[test]
    fn a_120_hz_page_only_refreshes_until_the_matching_capture_arrives() {
        let mut view = view(120);
        let start = Instant::now();
        assert!(!view.begin_resize(start));
        assert_eq!(view.tick(start), (true, None));
        view.refresh_ticks = 0;
        assert!(!view.needs_tick());
    }
}
