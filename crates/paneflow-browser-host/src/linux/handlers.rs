use cef::*;
use paneflow_browser_protocol::Document;
use serde_json::json;

use super::{clipboard, clipboard_renderer, editing, emit, presentation, HOST};

const OZONE_ENV: &str = "PANEFLOW_BROWSER_OZONE";

fn ozone_platform() -> &'static str {
    match std::env::var(OZONE_ENV).as_deref() {
        Ok("x11") => "x11",
        _ => "wayland",
    }
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

wrap_app! {
    pub struct WitnessApp;

    impl App {
        fn render_process_handler(&self) -> Option<RenderProcessHandler> { Some(clipboard_renderer::ClipboardRenderer::new()) }
        fn on_before_command_line_processing(&self, process_type: Option<&CefString>, command_line: Option<&mut CommandLine>) {
            if process_type.is_none_or(|value| value.to_string().is_empty()) {
                if let Some(command_line) = command_line {
                    command_line.append_switch_with_value(Some(&"ozone-platform".into()), Some(&ozone_platform().into()));
                    command_line.append_switch_with_value(Some(&"password-store".into()), Some(&"basic".into()));
                    if let Ok(render_node) = std::env::var("PANEFLOW_BROWSER_RENDER_NODE") {
                        command_line.append_switch_with_value(Some(&"render-node-override".into()), Some(&render_node.as_str().into()));
                    }
                    if ozone_platform() == "x11" {
                        command_line.append_switch(Some(&"enable-native-gpu-memory-buffers".into()));
                    }
                    command_line.append_switch_with_value(Some(&"use-gl".into()), Some(&"angle".into()));
                    command_line.append_switch_with_value(Some(&"use-angle".into()), Some(&"vulkan".into()));
                    command_line.append_switch(Some(&"disable-skia-graphite".into()));
                    command_line.append_switch_with_value(Some(&"disable-features".into()), Some(&"Vulkan,VulkanFromANGLE".into()));
                    command_line.append_switch_with_value(Some(&"gpu-sandbox-failures-fatal".into()), Some(&"yes".into()));
                    for name in ["no-first-run", "disable-extensions", "disable-background-networking", "disable-sync"] {
                        command_line.append_switch(Some(&name.into()));
                    }
                }
            }
        }
    }
}

wrap_client! {
    pub struct WitnessClient { document: Option<Document>, inspector: bool }

    impl Client {
        fn on_process_message_received(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, source_process: ProcessId, message: Option<&mut ProcessMessage>) -> i32 {
            let _context = super::Context::browser(_browser.as_deref());
            if message.as_deref().is_some_and(|message| super::devtools::receive(_browser.as_deref(), frame.as_deref(), source_process, message)) { return 1; }
            clipboard::receive(frame, source_process, message)
        }

        fn dialog_handler(&self) -> Option<DialogHandler> { self.document.clone().map(super::transfers::Uploads::new) }
        fn download_handler(&self) -> Option<DownloadHandler> { self.document.clone().map(super::transfers::Downloads::new) }
        fn permission_handler(&self) -> Option<PermissionHandler> { self.document.clone().map(super::permissions::Permissions::new) }
        fn jsdialog_handler(&self) -> Option<JsdialogHandler> { self.document.clone().map(super::web_interactions::Dialogs::new) }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> { Some(editing::Menus::new()) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(Life::new(self.document.clone())) }
        fn find_handler(&self) -> Option<FindHandler> { Some(FindResults::new()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(Load::new()) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(RequestPolicy::new(self.inspector)) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(FixtureDisplay::new()) }
        fn render_handler(&self) -> Option<RenderHandler> { presentation::active().then(|| presentation::Renderer::new(self.document.clone())) }
    }
}

wrap_life_span_handler! {
    struct Life { document: Option<Document> }

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let _context = super::Context::browser(browser.as_deref());
            let _context = super::Context::enter(self.document.clone());
            let browser = browser.cloned();
            if let Some(host) = browser.as_ref().and_then(|browser| browser.host()) { host.set_accessibility_state(State::ENABLED); }
            let identifier = browser.as_ref().map(|browser| browser.identifier());
            let native_window = browser.as_ref().and_then(|browser| browser.host()).map(|host| host.window_handle());
            HOST.with(|state| { if let Some(host) = state.borrow_mut().as_mut() { if let Some(document) = super::current_document() { if let Some(page) = host.pages.get_mut(&document.browser) { page.browser = browser; page.creating = false; }} } });
            if let Some(browser) = super::current_browser() {
                if !enforce_certificate_policy(&browser) {
                    emit(json!({ "native": "create_failed", "reason": "Strict certificate policy unavailable" }));
                    if let Some(host) = browser.host() { host.close_browser(1); }
                    return;
                }
                if !super::devtools::created(&browser) {
                    emit(json!({ "native": "create_failed", "reason": "DevTools target observer unavailable" }));
                    if let Some(host) = browser.host() { host.close_browser(1); }
                    return;
                }
            }
            emit(json!({ "native": "created", "cef_browser_id": identifier, "native_window": native_window }));
            let closing = HOST.with(|state| state.borrow().as_ref().and_then(|host| host.pages.get(&super::current_document()?.browser)).is_some_and(|page| page.pending_close.is_some()));
            if closing { if let Some(host) = super::current_browser().and_then(|browser| browser.host()) { host.close_browser(0); } }
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> i32 {
            let _context = super::Context::browser(_browser.as_deref());
            emit(json!({ "native": "close_ready" }));
            0
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            let _context = super::Context::browser(_browser.as_deref());
            if let Some(browser) = _browser.as_deref() { super::transfers::closed_drag(browser); }
            super::devtools::closed();
            let children = HOST.with(|state| { let state = state.borrow(); let Some(document) = super::current_document() else { return Vec::new(); }; state.as_ref().map(|host| host.pages.values().filter(|page| page.inspected.as_ref().is_some_and(|target| target.browser == document.browser)).filter_map(|page| page.browser.clone()).collect::<Vec<_>>()).unwrap_or_default() });
            for child in children { if let Some(host) = child.host() { host.close_browser(1); } }
            let reply = HOST.with(|state| {
                let mut state = state.borrow_mut();
                let host = state.as_mut()?;
                let document = super::current_document()?;
                let pending = host.pages.get_mut(&document.browser)?.pending_close.take().unwrap_or(paneflow_browser_protocol::Envelope {
                    version: paneflow_browser_protocol::CONTRACT_VERSION,
                    operation: "native-close".to_owned().try_into().ok()?,
                    command: paneflow_browser_protocol::Command::Close { document: document.clone() },
                });
                Some(host.controller.dispatch(&document.owner, pending))
            });
            if let Some(reply) = reply { emit(json!({ "protocol": reply })); }
            if let Some(document) = super::current_document() { super::web_interactions::clear(&document);
                super::permissions::clear(&document);
                super::transfers::clear(&document);
                    super::external_protocols::clear(&document); }
            super::editing::clear();
            HOST.with(|state| { if let Some(host) = state.borrow_mut().as_mut() { if let Some(document) = super::current_document() { if let Some(page) = host.pages.get_mut(&document.browser) { page.browser = None; page.window = None; } } } });
            presentation::detach();
            emit(json!({ "native": "closed" }));
            if !presentation::active() { super::close(); }
            if HOST.with(|state| state.borrow().as_ref().is_some_and(|host| host.closing && !host.trace_pending && host.pages.values().all(|page| page.browser.is_none()))) { quit_message_loop(); }
        }

        fn on_before_popup(&self, browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _popup_id: i32, target_url: Option<&CefString>, _target_frame_name: Option<&CefString>, _target_disposition: WindowOpenDisposition, user_gesture: i32, _popup_features: Option<&PopupFeatures>, _window_info: Option<&mut WindowInfo>, _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>, _extra_info: Option<&mut Option<DictionaryValue>>, _no_javascript_access: Option<&mut i32>) -> i32 {
            let _context = super::Context::browser(browser.as_deref());
            let url = target_url.map(ToString::to_string).unwrap_or_default();
            if user_gesture != 0 && presentation::active() && paneflow_browser_protocol::validate_url(&url).is_ok() {
                emit(json!({ "native": "popup_requested", "url": url }));
            }
            1
        }
    }
}

wrap_load_handler! {
    struct Load;

    impl LoadHandler {
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status_code: i32) {
            let _context = super::Context::browser(_browser.as_deref());
            if let Some(frame) = frame.filter(|frame| frame.is_main() != 0) {
                if !super::devtools::is_inspector() {
                    let url = CefString::from(&frame.url()).to_string();
                    super::agent_navigation_committed(&url);
                }
                emit(json!({ "native": "loaded", "http_status": http_status_code }));
                if super::devtools::is_inspector() { return; }
                frame.execute_java_script(Some(&r#"(() => {
                    const counters = { clicks: 0, keys: 0, wheel: 0, pointerdowns: 0, pointermoves: 0, pointerups: 0 };
                    let lastPointer = null;
                    addEventListener('click', event => { counters.clicks += 1; lastPointer = [event.clientX, event.clientY]; report(); }, true);
                    addEventListener('keydown', () => { counters.keys += 1; report(); }, true);
                    addEventListener('wheel', () => { counters.wheel += 1; report(); }, true);
                    addEventListener('pointerdown', event => { counters.pointerdowns += 1; lastPointer = [event.clientX, event.clientY]; }, true);
                    addEventListener('pointermove', event => { if (event.buttons) counters.pointermoves += 1; lastPointer = [event.clientX, event.clientY]; }, true);
                    addEventListener('pointerup', event => { counters.pointerups += 1; lastPointer = [event.clientX, event.clientY]; report(); }, true);
                    const report = () => console.log('PANEFLOW_FIXTURE:' + JSON.stringify({
                        state: document.documentElement.dataset.fixtureState || (location.pathname === '/empty' ? 'ready' : 'missing'),
                        width: innerWidth, height: innerHeight, scale: devicePixelRatio, visibility: document.visibilityState,
                        ...counters, last_pointer: lastPointer
                    }));
                    report(); setInterval(report, 1000);
                })()"#.into()), None, 0);
            }
        }

        fn on_load_error(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, error_code: Errorcode, error_text: Option<&CefString>, failed_url: Option<&CefString>) {
            let _context = super::Context::browser(_browser.as_deref());
            let main = frame.is_none_or(|frame| frame.is_main() != 0);
            let text: String = error_text.map(ToString::to_string).unwrap_or_default().chars().take(256).collect();
            let failed_url = failed_url.map(ToString::to_string);
            let agent_pending = main && super::agent_navigation_failure_matches(None);
            if main && !super::devtools::is_inspector() && super::agent_navigation_failure_matches(failed_url.as_deref()) {
                super::agent_navigation_failed(&text);
            }
            if !(agent_pending && error_code.get_raw() == -3) {
                emit(json!({ "native": "load_failed", "main": main, "error_code": error_code.get_raw(), "error_text": text }));
            }
        }

        fn on_loading_state_change(&self, _browser: Option<&mut Browser>, is_loading: i32, can_go_back: i32, can_go_forward: i32) {
            let _context = super::Context::browser(_browser.as_deref());
            emit(json!({ "native": "loading", "is_loading": is_loading != 0, "can_go_back": can_go_back != 0, "can_go_forward": can_go_forward != 0 }));
        }
    }
}

wrap_display_handler! {
    struct FixtureDisplay;

    impl DisplayHandler {
        fn on_fullscreen_mode_change(&self, browser: Option<&mut Browser>, fullscreen: i32) {
            let _context = super::Context::browser(browser.as_deref());
            emit(json!({ "native": "fullscreen", "enabled": fullscreen != 0 }));
        }
        fn on_cursor_change(&self, _browser: Option<&mut Browser>, _cursor: std::os::raw::c_ulong, type_: CursorType, _custom_cursor_info: Option<&CursorInfo>) -> i32 {
            let _context = super::Context::browser(_browser.as_deref());
            let style = match type_ {
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
                CursorType::NORTHEASTRESIZE | CursorType::SOUTHWESTRESIZE | CursorType::NORTHEASTSOUTHWESTRESIZE => "ResizeUpRightDownLeft",
                CursorType::NORTHWESTRESIZE | CursorType::SOUTHEASTRESIZE | CursorType::NORTHWESTSOUTHEASTRESIZE => "ResizeUpLeftDownRight",
                CursorType::COLUMNRESIZE => "ResizeColumn",
                CursorType::ROWRESIZE => "ResizeRow",
                CursorType::VERTICALTEXT => "IBeamCursorForVerticalLayout",
                CursorType::NODROP | CursorType::NOTALLOWED => "OperationNotAllowed",
                CursorType::ALIAS => "DragLink",
                CursorType::COPY => "DragCopy",
                CursorType::CONTEXTMENU => "ContextualMenu",
                _ => "Arrow",
            };
            emit(json!({ "native": "cursor", "style": style }));
            1
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let _context = super::Context::browser(_browser.as_deref());
            emit(json!({ "native": "title", "title": title.map(ToString::to_string).unwrap_or_default() }));
        }

        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            let _context = super::Context::browser(_browser.as_deref());
            if frame.is_some_and(|frame| frame.is_main() != 0) {
                emit(json!({ "native": "address", "url": url.map(ToString::to_string).unwrap_or_default() }));
            }
        }

        fn on_console_message(&self, _browser: Option<&mut Browser>, _level: LogSeverity, message: Option<&CefString>, _source: Option<&CefString>, _line: i32) -> i32 {
            let _context = super::Context::browser(_browser.as_deref());
            let Some(document) = super::current_document() else {
                return 0;
            };
            let message_text = message.map(ToString::to_string).unwrap_or_default();
            emit(json!({
                "native": "agent_console",
                "document": document,
                "value": {
                    "level": _level.get_raw(),
                    "message": message_text.chars().take(4096).collect::<String>(),
                    "source": _source.map(ToString::to_string).unwrap_or_default().chars().take(1024).collect::<String>(),
                    "line": _line.max(0),
                }
            }));
            if super::devtools::is_inspector() && HOST.with(|state| state.borrow().as_ref().is_some_and(|host| host.tracing)) {
                emit(json!({ "native": "devtools_console", "message": message_text.chars().take(1024).collect::<String>() }));
            }
            if let Some(report) = message.and_then(|message| message.as_slice()).and_then(crate::qualification::fixture_report) {
                match report {
                    Ok(state) => emit(json!({ "native": "fixture_state", "state": state })),
                    Err(_) => emit(json!({ "native": "fixture_state_invalid" })),
                }
                return 1;
            }
            0
        }
    }
}

wrap_request_handler! {
    struct RequestPolicy { inspector: bool }

    impl RequestHandler {
        fn resource_request_handler(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _request: Option<&mut Request>, _is_navigation: i32, _is_download: i32, _request_initiator: Option<&CefString>, _disable_default_handling: Option<&mut i32>) -> Option<ResourceRequestHandler> {
            if self.inspector {
                Some(super::devtools::Resources::new())
            } else {
                Some(AgentResources::new())
            }
        }

        fn on_certificate_error(&self, browser: Option<&mut Browser>, cert_error: Errorcode, _request_url: Option<&CefString>, _ssl_info: Option<&mut Sslinfo>, _callback: Option<&mut Callback>) -> i32 {
            let _context = super::Context::browser(browser.as_deref());
            emit(json!({ "native": "certificate_error", "error_code": cert_error.get_raw() }));
            if let Some(callback) = _callback { callback.cancel(); }
            1
        }

        fn on_render_process_terminated(&self, browser: Option<&mut Browser>, status: TerminationStatus, error_code: i32, _error_string: Option<&CefString>) {
            let _context = super::Context::browser(browser.as_deref());
            super::renderer_crashed();
            editing::clear();
            presentation::unmount();
            if let Some(document) = super::current_document() { super::web_interactions::clear(&document); super::permissions::clear(&document); super::transfers::clear(&document);
                    super::external_protocols::clear(&document); }
            emit(json!({ "native": "renderer_crashed", "status": status.get_raw(), "error_code": error_code }));
        }

        fn on_before_browse(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>, _user_gesture: i32, _is_redirect: i32) -> i32 {
            let _context = super::Context::browser(_browser.as_deref());
            let url = request.map(|request| CefString::from(&request.url()).to_string()).unwrap_or_default();
            let inspector = HOST.with(|state| { let state = state.borrow(); state.as_ref().and_then(|host| host.pages.get(&super::current_document()?.browser)).is_some_and(|page| page.inspected.is_some()) });
            let allowed = if inspector { url == super::devtools::URL } else { (presentation::active() && paneflow_browser_protocol::validate_url(&url).is_ok()) || HOST.with(|state| state.borrow().as_ref().is_some_and(|host| url.starts_with(&format!("{}/", host.origin)))) };
            if !allowed {
                if _user_gesture != 0 && _is_redirect == 0 {
                    if let Some(document) = super::current_document() {
                        let origin = _browser.as_deref().and_then(|browser| browser.main_frame()).map(|frame| CefString::from(&frame.url()).to_string()).unwrap_or_default();
                        super::external_protocols::request(&document, &origin, &url);
                    }
                }
                return 1;
            }
            if _frame.is_some_and(|frame| frame.is_main() != 0) {
                if !inspector && _is_redirect == 0 && !super::accept_navigation(&url) { return 1; }
                editing::clear();
                if let Some(document) = super::current_document() {
                    super::web_interactions::clear(&document);
                    super::permissions::clear(&document);
                    super::transfers::clear(&document);
                    super::external_protocols::clear(&document);
                }
            }
            0
        }
    }
}

fn emit_network_event(
    browser: Option<&mut Browser>,
    request: Option<&mut Request>,
    response: Option<&mut Response>,
    status: Option<u32>,
    received_content_length: Option<i64>,
) {
    let _context = super::Context::browser(browser.as_deref());
    let Some(document) = super::current_document() else {
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
    struct AgentResources;

    impl ResourceRequestHandler {
        fn on_before_resource_load(&self, browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>, _callback: Option<&mut Callback>) -> ReturnValue {
            emit_network_event(browser, request, None, None, None);
            ReturnValue::CONTINUE
        }

        fn on_resource_load_complete(&self, browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>, response: Option<&mut Response>, status: UrlrequestStatus, received_content_length: i64) {
            emit_network_event(browser, request, response, Some(status.get_raw()), Some(received_content_length.max(0)));
        }
    }
}

wrap_find_handler! {
    struct FindResults;

    impl FindHandler {
        fn on_find_result(&self, browser: Option<&mut Browser>, _identifier: i32, count: i32, _selection_rect: Option<&Rect>, active_match_ordinal: i32, final_update: i32) {
            let _context = super::Context::browser(browser.as_deref());
            emit(json!({ "native": "find_result", "count": count, "active": active_match_ordinal, "final": final_update != 0 }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cef::rc::ConvertReturnValue;
    use std::cell::Cell;

    thread_local! {
        static CANCELLED: Cell<usize> = const { Cell::new(0) };
        static CONTINUED: Cell<usize> = const { Cell::new(0) };
    }

    unsafe extern "C" fn cancel(_callback: *mut cef::sys::_cef_callback_t) {
        CANCELLED.with(|count| count.set(count.get() + 1));
    }

    unsafe extern "C" fn proceed(_callback: *mut cef::sys::_cef_callback_t) {
        CONTINUED.with(|count| count.set(count.get() + 1));
    }

    #[test]
    fn certificate_errors_cancel_once_and_never_delegate_to_chrome() {
        CANCELLED.with(|count| count.set(0));
        CONTINUED.with(|count| count.set(0));
        let mut raw = cef::sys::_cef_callback_t {
            base: cef::sys::_cef_base_ref_counted_t {
                size: std::mem::size_of::<cef::sys::_cef_callback_t>(),
                add_ref: None,
                release: None,
                has_one_ref: None,
                has_at_least_one_ref: None,
            },
            cont: Some(proceed),
            cancel: Some(cancel),
        };
        let mut callback: Callback = (&raw mut raw).wrap_result();
        let handler = RequestPolicy::new(false);
        assert_eq!(
            handler.on_certificate_error(
                None,
                Errorcode::CERT_AUTHORITY_INVALID,
                None,
                None,
                Some(&mut callback)
            ),
            1
        );
        assert_eq!(CANCELLED.with(Cell::get), 1);
        assert_eq!(CONTINUED.with(Cell::get), 0);
        assert_eq!(
            handler.on_certificate_error(None, Errorcode::CERT_AUTHORITY_INVALID, None, None, None),
            1
        );
        assert_eq!(CANCELLED.with(Cell::get), 1);
        drop(callback);
    }
}
