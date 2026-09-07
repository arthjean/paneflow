use cef::*;
use serde_json::json;

use super::{emit, presentation, HOST};

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
        fn on_before_command_line_processing(&self, process_type: Option<&CefString>, command_line: Option<&mut CommandLine>) {
            if process_type.is_none_or(|value| value.to_string().is_empty()) {
                if let Some(command_line) = command_line {
                    command_line.append_switch_with_value(Some(&"ozone-platform".into()), Some(&ozone_platform().into()));
                    command_line.append_switch_with_value(Some(&"password-store".into()), Some(&"basic".into()));
                    if let Ok(render_node) = std::env::var("PANEFLOW_BROWSER_RENDER_NODE") {
                        command_line.append_switch_with_value(Some(&"render-node-override".into()), Some(&render_node.as_str().into()));
                    }
                    if std::env::var_os(paneflow_browser_protocol::FRAME_CHANNEL_ENV).is_some() && ozone_platform() == "x11" {
                        command_line.append_switch(Some(&"enable-native-gpu-memory-buffers".into()));
                        command_line.append_switch_with_value(Some(&"use-angle".into()), Some(&"vulkan".into()));
                        command_line.append_switch_with_value(Some(&"enable-features".into()), Some(&"Vulkan,VulkanFromANGLE,DefaultANGLEVulkan".into()));
                    }
                    if std::env::var_os(paneflow_browser_protocol::FRAME_CHANNEL_ENV).is_some() {
                        command_line.append_switch_with_value(Some(&"gpu-sandbox-failures-fatal".into()), Some(&"yes".into()));
                    }
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

        fn on_before_popup(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _popup_id: i32, _target_url: Option<&CefString>, _target_frame_name: Option<&CefString>, _target_disposition: WindowOpenDisposition, _user_gesture: i32, _popup_features: Option<&PopupFeatures>, _window_info: Option<&mut WindowInfo>, _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>, _extra_info: Option<&mut Option<DictionaryValue>>, _no_javascript_access: Option<&mut i32>) -> i32 { 1 }
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

        fn on_load_error(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _error_code: Errorcode, _error_text: Option<&CefString>, _failed_url: Option<&CefString>) {
            emit(json!({ "native": "load_failed" }));
        }
    }
}

wrap_display_handler! {
    struct FixtureDisplay;

    impl DisplayHandler {
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
            let url = request.map(|request| CefString::from(&request.url()).to_string()).unwrap_or_default();
            let allowed = HOST.with(|state| state.borrow().as_ref().is_some_and(|host| url.starts_with(&format!("{}/", host.origin))));
            i32::from(!allowed)
        }
    }
}
