use std::cell::RefCell;
use std::collections::BTreeMap;

use cef::*;
use paneflow_browser_protocol::{BrowserId, Document};

use super::HOST;

pub(super) const URL: &str = "devtools://devtools/bundled/inspector.html";
pub(super) const MARKER: &str = "paneflow_devtools";
pub(super) const MESSAGE: &str = "paneflow.devtools.command";
pub(super) const MAX_MESSAGE: usize = 8 * 1024 * 1024;

struct Binding {
    target: Document,
    _registration: Registration,
}

thread_local! {
    static OBSERVERS: RefCell<BTreeMap<BrowserId, Binding>> = const { RefCell::new(BTreeMap::new()) };
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
    }
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
}
