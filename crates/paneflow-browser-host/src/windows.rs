use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{self, BufReader};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};

use cef::*;
use interprocess::local_socket::{prelude::*, GenericFilePath, Stream};
use interprocess::TryClone;
use paneflow_browser_protocol::{
    read_value, write_message, BrowserPresentation, Command, Controller, Document, EditAction,
    Envelope, Event, FrameAck, InputEvent, KeyKind, MouseButton, Owner, Reply, CONTRACT_VERSION,
};
use serde_json::{json, Value};

mod devtools;
#[path = "linux/devtools_renderer.rs"]
mod devtools_renderer;
mod editing;
mod presentation;

use presentation::Presenter;

static CONTROL_OUTPUT: OnceLock<Arc<Mutex<Stream>>> = OnceLock::new();
static UI_COMMANDS: OnceLock<SyncSender<UiCommand>> = OnceLock::new();
static FRAME_PUMP_SCHEDULED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static UI_RECEIVER: RefCell<Option<Receiver<UiCommand>>> = const { RefCell::new(None) };
    static PAGES: RefCell<BTreeMap<paneflow_browser_protocol::BrowserId, UiPage>> = const { RefCell::new(BTreeMap::new()) };
}

#[derive(Clone)]
struct ViewState {
    width: u32,
    height: u32,
    scale_percent: u32,
}

struct UiPage {
    document: Document,
    browser: Option<Browser>,
    presenter: Rc<RefCell<Presenter>>,
    view: Arc<Mutex<ViewState>>,
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

fn emit(value: Value) -> io::Result<()> {
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

pub(super) fn emit_frame(message: &paneflow_browser_protocol::FrameMessage) -> io::Result<()> {
    emit(json!({ "frame": message }))
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

fn schedule_frame_pump() {
    if FRAME_PUMP_SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut task = FramePump::new();
    if post_delayed_task(ThreadId::UI, Some(&mut task), 16) == 0 {
        FRAME_PUMP_SCHEDULED.store(false, Ordering::Release);
    }
}

fn dispatch(controller: &mut Controller, caller: &Owner, message: Envelope) -> Reply {
    controller.dispatch(caller, message)
}

fn queue_after_reply(
    command: Command,
    result: &Result<Event, paneflow_browser_protocol::BrowserError>,
) {
    if result.is_err() {
        return;
    }
    let queued_command = command.clone();
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
    let mut controller = Controller::new(format!("{}-windows", std::env::consts::ARCH), true);
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
        let reply = dispatch(&mut controller, &owner, message);
        let result = reply.result.clone();
        if emit(json!({ "protocol": reply })).is_err() {
            quit_message_loop();
            break;
        }
        queue_after_reply(command, &result);
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
    let client_pid = std::env::var("PANEFLOW_BROWSER_CLIENT_PID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let presenter = match Presenter::new(session.document.clone(), client_pid) {
        Ok(presenter) => Rc::new(RefCell::new(presenter)),
        Err(error) => {
            let _ = emit(json!({
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
            },
        );
    });
    if let Some(target_document) = &inspected {
        let Some(target) = page_browser(target_document) else {
            remove_page(&session.document);
            let _ = emit(json!({
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
        external_begin_frame_enabled: 1,
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    };
    let settings = BrowserSettings {
        windowless_frame_rate: 60,
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
        let _ = emit(json!({
            "native": "create_failed",
            "document": session.document,
            "reason": "CEF rejected the windowless browser",
        }));
    } else {
        schedule_frame_pump();
    }
}

fn remove_page(document: &Document) {
    PAGES.with(|pages| {
        pages.borrow_mut().remove(&document.browser);
    });
}

fn present(document: Document, presentation: BrowserPresentation) {
    let (browser, view) = PAGES.with(|pages| {
        let pages = pages.borrow();
        let Some(page) = pages.get(&document.browser) else {
            return (None, None);
        };
        if page.document != document {
            return (None, None);
        }
        (page.browser.clone(), Some(page.view.clone()))
    });
    if let Some(view) = view {
        if let Ok(mut view) = view.lock() {
            view.width = presentation.width.max(1);
            view.height = presentation.height.max(1);
            view.scale_percent = presentation.scale_percent.max(1);
        }
    }
    if let Some(browser) = browser {
        if let Some(host) = browser.host() {
            host.was_hidden(if presentation.visible { 0 } else { 1 });
            host.notify_screen_info_changed();
            host.was_resized();
            host.send_external_begin_frame();
        }
    }
}

fn navigate(document: Document, url: String) {
    devtools::closed(&document.browser);
    editing::cancel(&document.browser);
    let browser = PAGES.with(|pages| {
        let mut pages = pages.borrow_mut();
        let page = pages.get_mut(&document.browser)?;
        page.document = document.clone();
        if let Ok(mut presenter) = page.presenter.try_borrow_mut() {
            presenter.update_document(document.clone());
        }
        page.browser.clone()
    });
    if let Some(frame) = browser.and_then(|browser| browser.main_frame()) {
        frame.load_url(Some(&url.as_str().into()));
    }
}

fn input(document: Document, input: InputEvent) {
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
        InputEvent::DropFiles { .. }
        | InputEvent::TransferResponse { .. }
        | InputEvent::CancelDownload { .. }
        | InputEvent::WebResponse { .. }
        | InputEvent::ClipboardWritten { .. } => {}
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
            let _ = emit(json!({
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
        UiCommand::Navigate { document, url } => navigate(document, url),
        UiCommand::Input {
            document,
            input: event,
        } => input(document, event),
        UiCommand::Action { document, command } => action(document, command),
        UiCommand::FrameAck(ack) => {
            let refresh = matches!(ack, FrameAck::PoolReady { .. });
            let document = match &ack {
                FrameAck::PoolReady { document, .. }
                | FrameAck::PoolRejected { document, .. }
                | FrameAck::Release { document, .. } => document,
            }
            .clone();
            PAGES.with(|pages| {
                let pages = pages.borrow();
                if let Some(page) = pages.get(&document.browser) {
                    if page.document == document {
                        if let Ok(mut presenter) = page.presenter.try_borrow_mut() {
                            presenter.handle_ack(ack);
                        }
                        if refresh {
                            if let Some(browser) = &page.browser {
                                if let Some(host) = browser.host() {
                                    host.invalidate(PaintElementType::VIEW);
                                    host.send_external_begin_frame();
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
    struct FramePump;

    impl Task {
        fn execute(&self) {
            FRAME_PUMP_SCHEDULED.store(false, Ordering::Release);
            let active = PAGES.with(|pages| {
                let pages = pages.borrow();
                let mut active = false;
                for page in pages.values() {
                    if let Some(browser) = &page.browser {
                        if let Some(host) = browser.host() {
                            host.invalidate(PaintElementType::VIEW);
                            host.send_external_begin_frame();
                            active = true;
                        }
                    }
                }
                active
            });
            if active {
                schedule_frame_pump();
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
        fn request_handler(&self) -> Option<RequestHandler> {
            devtools::target(&self.document.browser).map(|_| devtools::Requests::new())
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
            if let Some(host) = browser.host() {
                host.was_hidden(0);
                host.notify_screen_info_changed();
                host.was_resized();
                host.invalidate(PaintElementType::VIEW);
                host.send_external_begin_frame();
            }
            schedule_frame_pump();
            let _ = emit(json!({
                "native": "created",
                "document": self.document,
                "cef_browser_id": browser.identifier().to_string(),
            }));
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> i32 { 0 }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            devtools::closed(&self.document.browser);
            editing::cancel(&self.document.browser);
            PAGES.with(|pages| {
                pages.borrow_mut().remove(&self.document.browser);
            });
            let _ = emit(json!({ "native": "closed", "document": self.document }));
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
            let _ = emit(json!({
                "native": "loading",
                "document": self.document,
                "is_loading": is_loading != 0,
                "can_go_back": can_go_back != 0,
                "can_go_forward": can_go_forward != 0,
            }));
        }

        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status_code: i32) {
            if frame.is_some_and(|frame| frame.is_main() != 0) {
                let _ = emit(json!({
                    "native": "loaded",
                    "document": self.document,
                    "http_status": http_status_code,
                }));
            }
        }

        fn on_load_error(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, error_code: Errorcode, error_text: Option<&CefString>, _failed_url: Option<&CefString>) {
            if frame.is_none_or(|frame| frame.is_main() != 0) {
                let error_text = error_text.map(ToString::to_string).unwrap_or_default();
                let _ = emit(json!({
                    "native": "load_failed",
                    "document": self.document,
                    "error_code": error_code.get_raw(),
                    "error_text": error_text,
                }));
            }
        }
    }
}

wrap_display_handler! {
    struct Display { document: Document }

    impl DisplayHandler {
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let _ = emit(json!({ "native": "title", "document": self.document, "title": title.map(ToString::to_string).unwrap_or_default() }));
        }

        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.is_some_and(|frame| frame.is_main() != 0) {
                let _ = emit(json!({ "native": "address", "document": self.document, "url": url.map(ToString::to_string).unwrap_or_default() }));
            }
        }

        fn on_fullscreen_mode_change(&self, _browser: Option<&mut Browser>, fullscreen: i32) {
            let _ = emit(json!({ "native": "fullscreen", "document": self.document, "enabled": fullscreen != 0 }));
        }

        fn on_console_message(&self, _browser: Option<&mut Browser>, _level: LogSeverity, message: Option<&CefString>, _source: Option<&CefString>, _line: i32) -> i32 {
            let text = message.map(ToString::to_string).unwrap_or_default();
            if let Some(state) = text.strip_prefix("PANEFLOW_FIXTURE:") {
                if let Ok(value) = serde_json::from_str::<Value>(state) {
                    let _ = emit(json!({ "native": "fixture_state", "document": self.document, "state": value }));
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
            if let Ok(mut presenter) = self.presenter.try_borrow_mut() {
                presenter.paint(type_, dirty_rects, info);
            }
        }

        fn on_text_selection_changed(&self, _browser: Option<&mut Browser>, selected_text: Option<&CefString>, selected_range: Option<&Range>) {
            let Some(range) = selected_range else { return };
            let Ok(presenter) = self.presenter.try_borrow() else { return };
            let _ = emit(json!({
                "native": "ime_selection",
                "document": presenter.document(),
                "snapshot": { "start": range.from, "end": range.to, "text": selected_text.map(ToString::to_string).filter(|text| text.len() <= 65536) },
            }));
        }

        fn on_ime_composition_range_changed(&self, _browser: Option<&mut Browser>, selected_range: Option<&Range>, character_bounds: Option<&[Rect]>) {
            let Some(range) = selected_range else { return };
            let Ok(presenter) = self.presenter.try_borrow() else { return };
            let bounds = character_bounds.unwrap_or_default().iter().take(4096).map(|rect| [rect.x, rect.y, rect.width, rect.height]).collect::<Vec<_>>();
            let _ = emit(json!({
                "native": "ime_bounds",
                "document": presenter.document(),
                "snapshot": { "start": range.from, "end": range.to, "bounds": bounds },
            }));
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
    if emit(json!({
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
