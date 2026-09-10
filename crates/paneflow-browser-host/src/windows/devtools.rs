use std::cell::RefCell;
use std::collections::BTreeMap;

use cef::*;
use paneflow_browser_protocol::{BrowserId, Document};

pub(super) const URL: &str = "devtools://devtools/bundled/inspector.html";
pub(super) const MARKER: &str = "paneflow_devtools";
pub(super) const MESSAGE: &str = "paneflow.devtools.command";
pub(super) const MAX_MESSAGE: usize = 8 * 1024 * 1024;

thread_local! {
    static BINDINGS: RefCell<BTreeMap<BrowserId, (Document, Option<Registration>)>> = const { RefCell::new(BTreeMap::new()) };
}

pub(super) fn bind(inspector: BrowserId, target: Document) {
    BINDINGS.with_borrow_mut(|bindings| bindings.insert(inspector, (target, None)));
}

pub(super) fn target(inspector: &BrowserId) -> Option<Document> {
    BINDINGS.with_borrow(|bindings| bindings.get(inspector).map(|binding| binding.0.clone()))
}

pub(super) fn created(document: &Document, frontend: &Browser) -> bool {
    let Some(target) = target(&document.browser) else {
        return true;
    };
    let registration = super::page_browser(&target)
        .and_then(|browser| browser.host())
        .and_then(|host| {
            host.add_dev_tools_message_observer(Some(&mut Messages::new(frontend.clone(), target)))
        });
    let Some(registration) = registration else {
        return false;
    };
    BINDINGS.with_borrow_mut(|bindings| {
        if let Some(binding) = bindings.get_mut(&document.browser) {
            binding.1 = Some(registration);
        }
    });
    true
}

pub(super) fn closed(browser: &BrowserId) {
    let removed = BINDINGS.with_borrow_mut(|bindings| {
        let keys = bindings
            .iter()
            .filter(|(id, (target, _))| *id == browser || &target.browser == browser)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter_map(|id| bindings.remove(&id).map(|binding| (id, binding)))
            .collect::<Vec<_>>()
    });
    for (id, _) in &removed {
        if id == browser {
            continue;
        }
        let frontend =
            super::PAGES.with_borrow(|pages| pages.get(id).and_then(|page| page.browser.clone()));
        if let Some(host) = frontend.and_then(|browser| browser.host()) {
            host.close_browser(1);
        }
    }
}

pub(super) fn message(
    document: &Document,
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
    if source != ProcessId::RENDERER
        || frame.is_main() == 0
        || CefString::from(&frame.url()).to_string() != URL
        || args.size() != 1
        || args.get_type(0) != ValueType::STRING
    {
        return true;
    }
    if !super::page_browser(document).is_some_and(|live| live.identifier() == browser.identifier())
    {
        return true;
    }
    let Some(target) = target(&document.browser).and_then(|target| super::page_browser(&target))
    else {
        return true;
    };
    let payload = CefString::from(&args.string(0)).to_string();
    if valid_command(&payload) {
        if let Some(host) = target.host() {
            host.send_dev_tools_message(Some(payload.as_bytes()));
        }
    }
    true
}

fn valid_command(payload: &str) -> bool {
    payload.len() <= MAX_MESSAGE
        && serde_json::from_str::<serde_json::Value>(payload).is_ok_and(|value| {
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
            if super::page_browser(&self.target).is_none() { return 1; }
            let Some(payload) = message.filter(|message| message.len() <= MAX_MESSAGE)
                .and_then(|message| std::str::from_utf8(message).ok())
                .and_then(|message| serde_json::to_string(message).ok()) else { return 1; };
            if let Some(frame) = self.frontend.main_frame().filter(|frame| CefString::from(&frame.url()).to_string() == URL) {
                frame.execute_java_script(Some(&format!("window.InspectorFrontendAPI?.dispatchMessage({payload})").as_str().into()), Some(&URL.into()), 0);
            }
            1
        }
    }
}

wrap_render_process_handler! {
    pub(super) struct Renderer;

    impl RenderProcessHandler {
        fn on_browser_created(&self, browser: Option<&mut Browser>, extra_info: Option<&mut DictionaryValue>) {
            if let Some(browser) = browser { super::devtools_renderer::created(browser, extra_info.as_deref()); }
        }
        fn on_browser_destroyed(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser { super::devtools_renderer::destroyed(browser); }
        }
        fn on_context_created(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, context: Option<&mut V8Context>) {
            if let (Some(browser), Some(frame), Some(context)) = (browser, frame, context) { super::devtools_renderer::context(browser, frame, context); }
        }
    }
}

wrap_request_handler! {
    pub(super) struct Requests;

    impl RequestHandler {
        fn resource_request_handler(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _request: Option<&mut Request>, _is_navigation: i32, _is_download: i32, _request_initiator: Option<&CefString>, _disable_default_handling: Option<&mut i32>) -> Option<ResourceRequestHandler> {
            Some(Resources::new())
        }
    }
}

wrap_resource_request_handler! {
    struct Resources;

    impl ResourceRequestHandler {
        fn on_before_resource_load(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>, _callback: Option<&mut Callback>) -> ReturnValue {
            let allowed = request.is_some_and(|request| {
                let url = CefString::from(&request.url()).to_string();
                url.starts_with("devtools://devtools/bundled/") || url.starts_with("data:") || url.starts_with("blob:devtools://devtools/")
            });
            if allowed { ReturnValue::CONTINUE } else { ReturnValue::CANCEL }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_require_bounded_protocol_objects() {
        assert!(valid_command(r#"{"id":1,"method":"Runtime.enable"}"#));
        for payload in [
            "null",
            "[]",
            r#"{"id":-1,"method":"Runtime.enable"}"#,
            r#"{"id":1,"method":""}"#,
        ] {
            assert!(!valid_command(payload));
        }
        assert!(!valid_command(&" ".repeat(MAX_MESSAGE + 1)));
    }
}
