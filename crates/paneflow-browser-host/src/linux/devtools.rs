use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeMap;

use base64::Engine;
use cef::*;
use paneflow_browser_protocol::{
    BrowserId, Document, OperationId, MAX_AGENT_CAPTURE_BYTES, MAX_AGENT_CAPTURE_CHUNK_BYTES,
    MAX_AGENT_CAPTURE_PIXELS,
};

use super::HOST;

pub(super) const URL: &str = "devtools://devtools/bundled/inspector.html";
pub(super) const MARKER: &str = "paneflow_devtools";
pub(super) const MESSAGE: &str = "paneflow.devtools.command";
pub(super) const MAX_MESSAGE: usize = 8 * 1024 * 1024;

struct Binding {
    target: Document,
    _registration: Registration,
}

struct CaptureBinding {
    target: Document,
    operation: OperationId,
    _registration: Registration,
}

thread_local! {
    static OBSERVERS: RefCell<BTreeMap<BrowserId, Binding>> = const { RefCell::new(BTreeMap::new()) };
    static CAPTURES: RefCell<BTreeMap<(BrowserId, i32), CaptureBinding>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT_CAPTURE_MESSAGE_ID: Cell<i32> = const { Cell::new(1) };
}

pub(super) fn is_inspector() -> bool {
    HOST.with(|state| {
        state
            .borrow()
            .as_ref()
            .and_then(|host| host.pages.get(&super::current_document()?.browser))
            .is_some_and(|page| page.inspected.is_some())
    })
}

pub(super) fn created(browser: &Browser) -> bool {
    if !is_inspector() {
        return true;
    }
    let pair = HOST.with(|state| {
        let state = state.borrow();
        let host = state.as_ref()?;
        let document = super::current_document()?;
        let inspected = host.pages.get(&document.browser)?.inspected.as_ref()?;
        let target = host.pages.get(&inspected.browser)?;
        if !same_live_document(inspected, &target.document, target.browser.is_some()) {
            return None;
        }
        Some((document.browser, target.browser.clone()?, inspected.clone()))
    });
    let Some((inspector, target, document)) = pair else {
        return false;
    };
    if let Some(registration) = target.host().and_then(|host| {
        host.add_dev_tools_message_observer(Some(&mut Messages::new(
            browser.clone(),
            document.clone(),
        )))
    }) {
        OBSERVERS.with(|state| {
            state.borrow_mut().insert(
                inspector,
                Binding {
                    target: document,
                    _registration: registration,
                },
            )
        });
        true
    } else {
        false
    }
}

pub(super) fn invalidate_target(target: &BrowserId) {
    let removed = OBSERVERS.with(|state| {
        let mut bindings = state.borrow_mut();
        let inspectors = bindings
            .iter()
            .filter(|(_, binding)| &binding.target.browser == target)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        inspectors
            .into_iter()
            .filter_map(|id| bindings.remove(&id).map(|binding| (id, binding)))
            .collect::<Vec<_>>()
    });
    let frontends = HOST.with(|state| {
        let state = state.borrow();
        removed
            .iter()
            .filter_map(|(id, _)| state.as_ref()?.pages.get(id)?.browser.clone())
            .collect::<Vec<_>>()
    });
    drop(removed);
    CAPTURES.with(|state| {
        state
            .borrow_mut()
            .retain(|_, binding| &binding.target.browser != target);
    });
    for frontend in frontends {
        if let Some(host) = frontend.host() {
            host.close_browser(1);
        }
    }
}

fn same_live_document(bound: &Document, current: &Document, browser_live: bool) -> bool {
    browser_live && bound == current
}

fn document_is_live(document: &Document) -> bool {
    HOST.with(|state| {
        state
            .borrow()
            .as_ref()
            .and_then(|host| host.pages.get(&document.browser))
            .is_some_and(|page| {
                same_live_document(document, &page.document, page.browser.is_some())
            })
    })
}

pub(super) fn closed() {
    if let Some(document) = super::current_document() {
        OBSERVERS.with(|state| state.borrow_mut().remove(&document.browser));
        CAPTURES.with(|state| {
            state
                .borrow_mut()
                .retain(|_, binding| binding.target.browser != document.browser);
        });
    }
}

pub(super) fn capture(document: &Document, operation: &OperationId) -> bool {
    if !document_is_live(document) {
        return false;
    }
    let browser = HOST.with(|state| {
        state
            .borrow()
            .as_ref()?
            .pages
            .get(&document.browser)?
            .browser
            .clone()
    });
    let Some(browser) = browser else {
        return false;
    };
    let Some(host) = browser.host() else {
        return false;
    };
    let message_id = NEXT_CAPTURE_MESSAGE_ID.with(|next| {
        let current = next.get();
        next.set(current.wrapping_add(1).max(1));
        current
    });
    let Some(registration) = host.add_dev_tools_message_observer(Some(&mut CaptureMessages::new(
        document.clone(),
        operation.clone(),
    ))) else {
        return false;
    };
    CAPTURES.with(|state| {
        state.borrow_mut().insert(
            (document.browser.clone(), message_id),
            CaptureBinding {
                target: document.clone(),
                operation: operation.clone(),
                _registration: registration,
            },
        );
    });
    let mut params = dictionary_value_create();
    let Some(params) = params.as_mut() else {
        CAPTURES.with(|state| {
            state
                .borrow_mut()
                .remove(&(document.browser.clone(), message_id));
        });
        return false;
    };
    params.set_string(Some(&"format".into()), Some(&"png".into()));
    params.set_bool(Some(&"fromSurface".into()), 1);
    params.set_bool(Some(&"captureBeyondViewport".into()), 0);
    let accepted = host.execute_dev_tools_method(
        message_id,
        Some(&"Page.captureScreenshot".into()),
        Some(params),
    ) != 0;
    if !accepted {
        CAPTURES.with(|state| {
            state
                .borrow_mut()
                .remove(&(document.browser.clone(), message_id));
        });
    }
    accepted
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width > 0
        && height > 0
        && u64::from(width).saturating_mul(u64::from(height)) <= MAX_AGENT_CAPTURE_PIXELS)
        .then_some((width, height))
}

fn capture_failure(binding: &CaptureBinding, reason: &str) {
    super::emit(serde_json::json!({
        "native": "screenshot_failed",
        "document": binding.target,
        "operation": binding.operation,
        "reason": reason
    }));
}

pub(super) fn receive(
    browser: Option<&Browser>,
    frame: Option<&Frame>,
    source: ProcessId,
    message: &ProcessMessage,
) -> bool {
    if CefString::from(&message.name()).to_string() != MESSAGE {
        return false;
    }
    let (Some(browser), Some(frame), Some(args)) = (browser, frame, message.argument_list()) else {
        return true;
    };
    let target = HOST.with(|state| {
        let state = state.borrow();
        let host = state.as_ref()?;
        let inspector = host.pages.values().find(|page| {
            page.browser
                .as_ref()
                .is_some_and(|native| native.identifier() == browser.identifier())
        })?;
        let binding = OBSERVERS.with(|state| {
            state
                .borrow()
                .get(&inspector.document.browser)
                .map(|binding| binding.target.clone())
        })?;
        let target = host.pages.get(&inspector.inspected.as_ref()?.browser)?;
        if !same_live_document(&binding, &target.document, target.browser.is_some()) {
            return None;
        }
        target.browser.clone()
    });
    if !authorized_frame(
        target.is_some(),
        source == ProcessId::RENDERER,
        frame.is_main() != 0,
        &CefString::from(&frame.url()).to_string(),
    ) || args.size() != 1
        || args.get_type(0) != ValueType::STRING
    {
        return true;
    }
    let payload = CefString::from(&args.string(0)).to_string();
    if !valid_command(&payload) {
        return true;
    }
    if let Some(host) = target.and_then(|browser| browser.host()) {
        host.send_dev_tools_message(Some(payload.as_bytes()));
    }
    true
}

fn bundled_resource(url: &str) -> bool {
    url.starts_with("devtools://devtools/bundled/")
        || url.starts_with("data:")
        || url.starts_with("blob:devtools://devtools/")
}

wrap_resource_request_handler! {
    pub(super) struct Resources;

    impl ResourceRequestHandler {
        fn on_before_resource_load(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>, _callback: Option<&mut Callback>) -> ReturnValue {
            if request.is_some_and(|request| bundled_resource(&CefString::from(&request.url()).to_string())) { ReturnValue::CONTINUE } else { ReturnValue::CANCEL }
        }
    }
}

fn authorized_frame(owned_inspector: bool, renderer: bool, main_frame: bool, url: &str) -> bool {
    owned_inspector && renderer && main_frame && url == URL
}

fn valid_command(payload: &str) -> bool {
    if payload.len() > MAX_MESSAGE {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(payload).is_ok_and(|value| {
        value
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|id| id >= 0)
            && value
                .get("method")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|method| !method.is_empty())
            && value.get("params").is_none_or(serde_json::Value::is_object)
    })
}

wrap_dev_tools_message_observer! {
    struct Messages { frontend: Browser, target: Document }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(&self, _browser: Option<&mut Browser>, message: Option<&[u8]>) -> i32 {
            if !document_is_live(&self.target) { return 1; }
            let Some(payload) = message.filter(|message| message.len() <= MAX_MESSAGE).and_then(|message| std::str::from_utf8(message).ok()).and_then(|message| serde_json::to_string(message).ok()) else { return 1; };
            if let Some(frame) = self.frontend.main_frame().filter(|frame| CefString::from(&frame.url()).to_string() == URL) {
                frame.execute_java_script(Some(&format!("window.InspectorFrontendAPI?.dispatchMessage({payload})").as_str().into()), Some(&URL.into()), 0);
            }
            1
        }
    }
}

wrap_dev_tools_message_observer! {
    struct CaptureMessages { target: Document, operation: OperationId }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(&self, _browser: Option<&mut Browser>, message_id: i32, success: i32, result: Option<&[u8]>) {
            let key = Some((self.target.browser.clone(), message_id));
            let binding = CAPTURES.with(|state| {
                let mut state = state.borrow_mut();
                let key = key.or_else(|| state.keys().find(|(_, id)| *id == message_id).cloned());
                key.and_then(|key| state.remove(&key))
            });
            let Some(binding) = binding else { return; };
            if success == 0 {
                capture_failure(&binding, "Page.captureScreenshot was rejected");
                return;
            }
            let Some(result) = result
                .filter(|result| result.len() <= (MAX_AGENT_CAPTURE_BYTES * 4).div_ceil(3) + 1024)
                .and_then(|result| serde_json::from_slice::<serde_json::Value>(result).ok())
            else {
                capture_failure(&binding, "native screenshot result is invalid");
                return;
            };
            let Some(data) = result.get("data").and_then(serde_json::Value::as_str) else {
                capture_failure(&binding, "native screenshot data is missing");
                return;
            };
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
                capture_failure(&binding, "native screenshot data is not base64");
                return;
            };
            let Some((width, height)) = png_dimensions(&bytes)
                .filter(|(width, height)| u64::from(*width).saturating_mul(u64::from(*height)) <= MAX_AGENT_CAPTURE_PIXELS)
            else {
                capture_failure(&binding, "native screenshot dimensions are invalid");
                return;
            };
            if bytes.len() > MAX_AGENT_CAPTURE_BYTES
                || data.len() > (MAX_AGENT_CAPTURE_BYTES * 4).div_ceil(3) + 4
                || data.is_empty()
            {
                capture_failure(&binding, "native screenshot exceeds its bound");
                return;
            }
            let count = data.len().div_ceil(MAX_AGENT_CAPTURE_CHUNK_BYTES);
            for (index, chunk) in data.as_bytes().chunks(MAX_AGENT_CAPTURE_CHUNK_BYTES).enumerate() {
                let Ok(chunk) = std::str::from_utf8(chunk) else {
                    capture_failure(&binding, "native screenshot chunk is invalid");
                    return;
                };
                super::emit(serde_json::json!({
                    "native": "screenshot_chunk",
                    "document": binding.target.clone(),
                    "operation": binding.operation.clone(),
                    "mime": "image/png",
                    "width": width,
                    "height": height,
                    "index": index,
                    "count": count,
                    "data": chunk,
                    "final": index + 1 == count
                }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inspector_binding_never_crosses_target_generation_or_owner() {
        let bound = Document {
            owner: paneflow_browser_protocol::Owner {
                workspace: "workspace".to_owned().try_into().unwrap(),
                session: "session".to_owned().try_into().unwrap(),
            },
            browser: "target".to_owned().try_into().unwrap(),
            generation: 1,
        };
        assert!(same_live_document(&bound, &bound, true));
        assert!(!same_live_document(&bound, &bound, false));
        let mut replacement = bound.clone();
        replacement.generation += 1;
        assert!(!same_live_document(&bound, &replacement, true));
        replacement = bound.clone();
        replacement.owner.session = "other-session".to_owned().try_into().unwrap();
        assert!(!same_live_document(&bound, &replacement, true));
        replacement = bound.clone();
        replacement.browser = "other-target".to_owned().try_into().unwrap();
        assert!(!same_live_document(&bound, &replacement, true));
    }

    #[test]
    fn frontend_resources_never_use_remote_or_local_network_origins() {
        assert!(bundled_resource(URL));
        assert!(bundled_resource(
            "devtools://devtools/bundled/core/sdk/sdk.js"
        ));
        for url in [
            "https://chrome-devtools-frontend.appspot.com/serve_file/test",
            "http://localhost:9222/json",
            "file:///tmp/frontend.js",
            "devtools://other/bundled/inspector.html",
        ] {
            assert!(!bundled_resource(url));
        }
    }

    #[test]
    fn trusted_url_alone_never_grants_the_inspector_capability() {
        assert!(authorized_frame(true, true, true, URL));
        for permissions in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            assert!(!authorized_frame(
                permissions.0,
                permissions.1,
                permissions.2,
                URL
            ));
        }
        for url in [
            "https://example.com",
            "devtools://devtools/bundled/other.html",
            "devtools://devtools/bundled/inspector.html?ws=localhost",
            "devtools://evil/bundled/inspector.html",
        ] {
            assert!(!authorized_frame(true, true, true, url));
        }
    }

    #[test]
    fn only_bounded_protocol_commands_cross_the_frontend_bridge() {
        assert!(valid_command(
            r#"{"id":1,"method":"Runtime.evaluate","params":{"expression":"1+1"}}"#
        ));
        for payload in [
            "null",
            "[]",
            r#"{"id":1}"#,
            r#"{"id":-1,"method":"Runtime.enable"}"#,
            r#"{"id":1,"method":"Runtime.enable","params":[]}"#,
        ] {
            assert!(!valid_command(payload));
        }
        assert!(!valid_command(&" ".repeat(MAX_MESSAGE + 1)));
    }

    #[test]
    fn screenshot_dimensions_are_checked_before_chunking() {
        let mut valid = vec![0; 24];
        valid[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        valid[12..16].copy_from_slice(b"IHDR");
        valid[16..20].copy_from_slice(&1920u32.to_be_bytes());
        valid[20..24].copy_from_slice(&1080u32.to_be_bytes());
        assert_eq!(png_dimensions(&valid), Some((1920, 1080)));

        valid[16..20].copy_from_slice(&5000u32.to_be_bytes());
        valid[20..24].copy_from_slice(&5000u32.to_be_bytes());
        assert_eq!(png_dimensions(&valid), None);
        valid[0] = 0;
        assert_eq!(png_dimensions(&valid), None);
    }
}
