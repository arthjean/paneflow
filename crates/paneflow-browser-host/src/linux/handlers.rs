use cef::*;
use serde_json::json;

use super::{clipboard, clipboard_renderer, editing, emit, presentation, HOST};

const OZONE_ENV: &str = "PANEFLOW_BROWSER_OZONE";

fn ozone_platform() -> &'static str {
    match std::env::var(OZONE_ENV).as_deref() {
        Ok("x11") => "x11",
        _ => "wayland",
    }
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
    pub struct WitnessClient;

    impl Client {
        fn on_process_message_received(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, source_process: ProcessId, message: Option<&mut ProcessMessage>) -> i32 {
            clipboard::receive(frame, source_process, message)
        }

        fn context_menu_handler(&self) -> Option<ContextMenuHandler> { Some(editing::Menus::new()) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(Life::new()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(Load::new()) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(RequestPolicy::new()) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(FixtureDisplay::new()) }
        fn render_handler(&self) -> Option<RenderHandler> { presentation::active().then(presentation::Renderer::new) }
    }
}

wrap_life_span_handler! {
    struct Life;

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let browser = browser.cloned();
            let identifier = browser.as_ref().map(|browser| browser.identifier());
            let native_window = browser.as_ref().and_then(|browser| browser.host()).map(|host| host.window_handle());
            HOST.with(|state| { if let Some(host) = state.borrow_mut().as_mut() { host.browser = browser; } });
            emit(json!({ "native": "created", "cef_browser_id": identifier, "native_window": native_window }));
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> i32 {
            emit(json!({ "native": "close_ready" }));
            0
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            HOST.with(|state| { if let Some(host) = state.borrow_mut().as_mut() { host.browser = None; } });
            emit(json!({ "native": "closed" }));
            quit_message_loop();
        }

        fn on_before_popup(&self, browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _popup_id: i32, target_url: Option<&CefString>, _target_frame_name: Option<&CefString>, _target_disposition: WindowOpenDisposition, user_gesture: i32, _popup_features: Option<&PopupFeatures>, _window_info: Option<&mut WindowInfo>, _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>, _extra_info: Option<&mut Option<DictionaryValue>>, _no_javascript_access: Option<&mut i32>) -> i32 {
            let url = target_url.map(ToString::to_string).unwrap_or_default();
            if user_gesture != 0 && presentation::active() && paneflow_browser_protocol::validate_url(&url).is_ok() {
                if let Some(frame) = browser.and_then(|browser| browser.main_frame()) {
                    post_task(ThreadId::UI, Some(&mut OpenLink::new(frame, url)));
                }
            }
            1
        }
    }
}

wrap_load_handler! {
    struct Load;

    impl LoadHandler {
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status_code: i32) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() != 0) {
                emit(json!({ "native": "loaded", "http_status": http_status_code }));
                frame.execute_java_script(Some(&r#"(() => {
                    const counters = { clicks: 0, keys: 0, wheel: 0 };
                    addEventListener('click', () => { counters.clicks += 1; report(); }, true);
                    addEventListener('keydown', () => { counters.keys += 1; report(); }, true);
                    addEventListener('wheel', () => { counters.wheel += 1; }, true);
                    const report = () => console.log('PANEFLOW_FIXTURE:' + JSON.stringify({
                        state: document.documentElement.dataset.fixtureState || (location.pathname === '/empty' ? 'ready' : 'missing'),
                        width: innerWidth, height: innerHeight, scale: devicePixelRatio, visibility: document.visibilityState,
                        clicks: counters.clicks, keys: counters.keys, wheel: counters.wheel
                    }));
                    report(); setInterval(report, 1000);
                })()"#.into()), None, 0);
            }
        }

        fn on_load_error(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, error_code: Errorcode, error_text: Option<&CefString>, _failed_url: Option<&CefString>) {
            let main = frame.is_none_or(|frame| frame.is_main() != 0);
            let text: String = error_text.map(ToString::to_string).unwrap_or_default().chars().take(256).collect();
            emit(json!({ "native": "load_failed", "main": main, "error_code": error_code.get_raw(), "error_text": text }));
        }

        fn on_loading_state_change(&self, _browser: Option<&mut Browser>, is_loading: i32, can_go_back: i32, can_go_forward: i32) {
            emit(json!({ "native": "loading", "is_loading": is_loading != 0, "can_go_back": can_go_back != 0, "can_go_forward": can_go_forward != 0 }));
        }
    }
}

wrap_display_handler! {
    struct FixtureDisplay;

    impl DisplayHandler {
        fn on_cursor_change(&self, _browser: Option<&mut Browser>, _cursor: std::os::raw::c_ulong, type_: CursorType, _custom_cursor_info: Option<&CursorInfo>) -> i32 {
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
            emit(json!({ "native": "title", "title": title.map(ToString::to_string).unwrap_or_default() }));
        }

        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.is_some_and(|frame| frame.is_main() != 0) {
                emit(json!({ "native": "address", "url": url.map(ToString::to_string).unwrap_or_default() }));
            }
        }

        fn on_console_message(&self, _browser: Option<&mut Browser>, _level: LogSeverity, message: Option<&CefString>, _source: Option<&CefString>, _line: i32) -> i32 {
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
    struct RequestPolicy;

    impl RequestHandler {
        fn on_before_browse(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>, _user_gesture: i32, _is_redirect: i32) -> i32 {
            clipboard::clear();
            if _frame.is_some_and(|frame| frame.is_main() != 0) { editing::clear(); }
            let url = request.map(|request| CefString::from(&request.url()).to_string()).unwrap_or_default();
            let allowed = (presentation::active() && paneflow_browser_protocol::validate_url(&url).is_ok()) || HOST.with(|state| state.borrow().as_ref().is_some_and(|host| url.starts_with(&format!("{}/", host.origin))));
            i32::from(!allowed)
        }
    }
}

wrap_task! {
    struct OpenLink {
        frame: Frame,
        url: String,
    }

    impl Task {
        fn execute(&self) {
            if self.frame.is_valid() != 0 {
                self.frame.load_url(Some(&self.url.as_str().into()));
            }
        }
    }
}
