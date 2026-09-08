use std::{cell::RefCell, collections::BTreeMap};

use cef::*;
use paneflow_browser_protocol::{Document, InputEvent};
use serde_json::json;

#[derive(Clone, PartialEq, Eq)]
struct Scope {
    document: Document,
    origin: String,
    mask: u32,
    media: bool,
}

enum Reply {
    Media(MediaAccessCallback),
    Prompt(PermissionPromptCallback),
}

impl Reply {
    fn finish(self, accept: bool, mask: u32) {
        match self {
            Self::Media(callback) => callback.cont(if accept { mask } else { 0 }),
            Self::Prompt(callback) => callback.cont(if accept {
                PermissionRequestResult::ACCEPT
            } else {
                PermissionRequestResult::DENY
            }),
        }
    }
}

struct Pending {
    scope: Scope,
    reply: Reply,
    prompt_id: Option<u64>,
}

struct State {
    next: u64,
    pending: BTreeMap<u64, Pending>,
    grants: Vec<Scope>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            next: 1 << 63,
            pending: BTreeMap::new(),
            grants: Vec::new(),
        }
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn allowed_mask(mask: u32, media: bool) -> bool {
    let allowed = if media {
        MediaAccessPermissionTypes::DEVICE_AUDIO_CAPTURE.get_raw()
            | MediaAccessPermissionTypes::DEVICE_VIDEO_CAPTURE.get_raw()
    } else {
        PermissionRequestTypes::GEOLOCATION.get_raw() | PermissionRequestTypes::CLIPBOARD.get_raw()
    };
    mask != 0 && mask & !allowed == 0
}

struct ParsedUrlParts(cef::sys::_cef_urlparts_t);

impl Drop for ParsedUrlParts {
    fn drop(&mut self) {
        for field in [
            &mut self.0.spec,
            &mut self.0.scheme,
            &mut self.0.username,
            &mut self.0.password,
            &mut self.0.host,
            &mut self.0.port,
            &mut self.0.origin,
            &mut self.0.path,
            &mut self.0.query,
            &mut self.0.fragment,
        ] {
            unsafe { cef::sys::cef_string_utf16_clear(field) };
        }
    }
}

fn origin(value: Option<&CefString>) -> Option<String> {
    let value = value?;
    let mut raw = ParsedUrlParts(Urlparts::default().into());
    let parsed = unsafe { cef::sys::cef_parse_url(value.into(), &mut raw.0) };
    let parts = Urlparts::from(raw.0);
    let scheme = parts.scheme.to_string();
    let host = parts.host.to_string();
    let allowed = parsed != 0
        && !host.is_empty()
        && parts.username.to_string().is_empty()
        && parts.password.to_string().is_empty()
        && (scheme == "https"
            || (scheme == "http" && matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]")));
    allowed.then(|| parts.origin.to_string())
}

fn request(
    document: &Document,
    requesting_origin: Option<&CefString>,
    mask: u32,
    reply: Reply,
    prompt_id: Option<u64>,
) {
    let media = matches!(&reply, Reply::Media(_));
    let Some(document) = super::latest_document(document) else {
        reply.finish(false, mask);
        return;
    };
    let Some(origin) = origin(requesting_origin).filter(|_| allowed_mask(mask, media)) else {
        reply.finish(false, mask);
        return;
    };
    let scope = Scope {
        document,
        origin,
        mask,
        media,
    };
    let granted = STATE.with(|state| state.borrow().grants.contains(&scope));
    if granted {
        reply.finish(true, mask);
        return;
    }
    let request = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.pending.len() >= 16 || state.next == u64::MAX {
            return None;
        }
        let request = state.next;
        state.next += 1;
        Some(request)
    });
    let Some(request) = request else {
        reply.finish(false, mask);
        return;
    };
    let message = if media {
        match mask {
            value if value == MediaAccessPermissionTypes::DEVICE_AUDIO_CAPTURE.get_raw() => {
                "Autoriser le microphone pour ce document ?"
            }
            value if value == MediaAccessPermissionTypes::DEVICE_VIDEO_CAPTURE.get_raw() => {
                "Autoriser la caméra pour ce document ?"
            }
            _ => "Autoriser la caméra et le microphone pour ce document ?",
        }
    } else if mask == PermissionRequestTypes::GEOLOCATION.get_raw() {
        "Autoriser la géolocalisation pour ce document ?"
    } else if mask == PermissionRequestTypes::CLIPBOARD.get_raw() {
        "Autoriser l’accès au presse-papiers pour ce document ?"
    } else {
        "Autoriser la géolocalisation et le presse-papiers pour ce document ?"
    };
    super::emit(
        json!({"native":"web_dialog", "document":scope.document, "request":request, "kind":"permission", "origin":scope.origin, "message":message, "default_text":""}),
    );
    STATE.with(|state| {
        state.borrow_mut().pending.insert(
            request,
            Pending {
                scope,
                reply,
                prompt_id,
            },
        );
    });
}

pub(super) fn handle(document: &Document, input: &InputEvent) {
    let InputEvent::WebResponse {
        request, accept, ..
    } = input
    else {
        return;
    };
    let pending = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state
            .pending
            .get(request)
            .is_some_and(|pending| &pending.scope.document == document)
        {
            state.pending.remove(request)
        } else {
            None
        }
    });
    if let Some(pending) = pending {
        let accept = *accept && super::latest_document(document).as_ref() == Some(document);
        if accept {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                if state.grants.len() < 64 && !state.grants.contains(&pending.scope) {
                    state.grants.push(pending.scope.clone());
                }
            });
        }
        pending.reply.finish(accept, pending.scope.mask);
    }
}

pub(super) fn clear(document: &Document) {
    let removed = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let ids = state
            .pending
            .iter()
            .filter(|(_, pending)| {
                pending.scope.document.browser == document.browser
                    && pending.scope.document.owner == document.owner
            })
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        state.grants.retain(|scope| {
            scope.document.browser != document.browser || scope.document.owner != document.owner
        });
        ids.into_iter()
            .filter_map(|id| state.pending.remove(&id))
            .collect::<Vec<_>>()
    });
    for pending in removed {
        pending.reply.finish(false, pending.scope.mask);
    }
}

pub(super) fn revoke(document: &Document) {
    clear(document);
}

wrap_permission_handler! {
    pub struct Permissions { document: Document }

    impl PermissionHandler {
        fn on_request_media_access_permission(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, requesting_origin: Option<&CefString>, requested_permissions: u32, callback: Option<&mut MediaAccessCallback>) -> i32 {
            if let Some(callback) = callback { request(&self.document, requesting_origin, requested_permissions, Reply::Media(callback.clone()), None); }
            1
        }

        fn on_show_permission_prompt(&self, _browser: Option<&mut Browser>, prompt_id: u64, requesting_origin: Option<&CefString>, requested_permissions: u32, callback: Option<&mut PermissionPromptCallback>) -> i32 {
            if let Some(callback) = callback { request(&self.document, requesting_origin, requested_permissions, Reply::Prompt(callback.clone()), Some(prompt_id)); }
            1
        }

        fn on_dismiss_permission_prompt(&self, _browser: Option<&mut Browser>, prompt_id: u64, _result: PermissionRequestResult) {
            STATE.with(|state| { state.borrow_mut().pending.retain(|_, pending| !(pending.scope.document.browser == self.document.browser && pending.scope.document.owner == self.document.owner && pending.prompt_id == Some(prompt_id))); });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_native_origin_output_without_discarding_parts() {
        assert_eq!(
            origin(Some(&CefString::from("http://127.0.0.1:18764/path"))),
            Some("http://127.0.0.1:18764/".to_owned())
        );
        assert_eq!(
            origin(Some(&CefString::from("https://example.com/path"))),
            Some("https://example.com/".to_owned())
        );
        assert!(origin(Some(&CefString::from("not a url"))).is_none());
        assert!(origin(Some(&CefString::from("http://remote.example/path"))).is_none());
        assert!(origin(Some(&CefString::from(
            "https://user:secret@example.com/path"
        )))
        .is_none());
    }

    #[test]
    fn only_explicit_supported_permissions_are_allowed() {
        assert!(allowed_mask(
            MediaAccessPermissionTypes::DEVICE_AUDIO_CAPTURE.get_raw(),
            true
        ));
        assert!(allowed_mask(
            PermissionRequestTypes::GEOLOCATION.get_raw(),
            false
        ));
        assert!(allowed_mask(
            PermissionRequestTypes::CLIPBOARD.get_raw(),
            false
        ));
        assert!(!allowed_mask(
            MediaAccessPermissionTypes::DESKTOP_VIDEO_CAPTURE.get_raw(),
            true
        ));
        assert!(!allowed_mask(0, false));
        assert!(!allowed_mask(u32::MAX, false));
    }
    thread_local! {
        static RESULTS: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn permission_result(
        _callback: *mut cef::sys::_cef_permission_prompt_callback_t,
        result: cef::sys::cef_permission_request_result_t,
    ) {
        RESULTS.with(|results| results.borrow_mut().push(result as i32));
    }

    #[test]
    fn actual_callbacks_reject_stale_revoked_closed_and_unknown_requests() {
        use cef::rc::ConvertReturnValue;
        use paneflow_browser_protocol::{Controller, Owner};
        let document = Document {
            owner: Owner {
                workspace: "workspace".to_owned().try_into().unwrap(),
                session: "session".to_owned().try_into().unwrap(),
            },
            browser: "page".to_owned().try_into().unwrap(),
            generation: 1,
        };
        let (_, receiver) = std::sync::mpsc::channel();
        super::super::HOST.with(|state| {
            state.replace(Some(super::super::Host {
                controller: Controller::new("test".to_owned(), true),
                owner: document.owner.clone(),
                profile: None,
                receiver,
                pages: BTreeMap::from([(
                    document.browser.clone(),
                    super::super::Page {
                        document: document.clone(),
                        browser: None,
                        window: None,
                        pending_close: None,
                        inspected: None,
                        expected_navigation: None,
                        creating: false,
                    },
                )]),
                origin: "https://example.com".to_owned(),
                closing: false,
                tracing: false,
                trace_path: std::path::PathBuf::new(),
            }))
        });
        STATE.with(|state| state.replace(State::default()));
        RESULTS.with(|results| results.borrow_mut().clear());
        let mut raw = cef::sys::_cef_permission_prompt_callback_t {
            base: cef::sys::_cef_base_ref_counted_t {
                size: std::mem::size_of::<cef::sys::_cef_permission_prompt_callback_t>(),
                add_ref: None,
                release: None,
                has_one_ref: None,
                has_at_least_one_ref: None,
            },
            cont: Some(permission_result),
        };
        let callback: PermissionPromptCallback = (&raw mut raw).wrap_result();
        let origin = CefString::from("https://example.com/path");
        let mask = PermissionRequestTypes::GEOLOCATION.get_raw();
        let enqueue = |mask| {
            request(
                &document,
                Some(&origin),
                mask,
                Reply::Prompt(callback.clone()),
                None,
            );
            STATE.with(|state| state.borrow().pending.keys().next().copied())
        };
        assert!(enqueue(u32::MAX).is_none());
        let denied = PermissionRequestResult::DENY.get_raw() as i32;
        assert_eq!(
            RESULTS.with(|results| results.borrow().clone()),
            vec![denied]
        );
        let id = enqueue(mask).unwrap();
        super::super::HOST.with(|state| {
            state
                .borrow_mut()
                .as_mut()
                .unwrap()
                .pages
                .get_mut(&document.browser)
                .unwrap()
                .document
                .generation = 2;
        });
        handle(
            &document,
            &InputEvent::WebResponse {
                request: id,
                accept: true,
                text: String::new(),
            },
        );
        assert!(STATE.with(|state| state.borrow().grants.is_empty()));
        assert_eq!(RESULTS.with(|results| results.borrow().len()), 2);
        super::super::HOST.with(|state| {
            state
                .borrow_mut()
                .as_mut()
                .unwrap()
                .pages
                .get_mut(&document.browser)
                .unwrap()
                .document = document.clone();
        });
        let id = enqueue(mask).unwrap();
        revoke(&document);
        handle(
            &document,
            &InputEvent::WebResponse {
                request: id,
                accept: true,
                text: String::new(),
            },
        );
        assert_eq!(RESULTS.with(|results| results.borrow().len()), 3);
        let id = enqueue(mask).unwrap();
        clear(&document);
        handle(
            &document,
            &InputEvent::WebResponse {
                request: id,
                accept: true,
                text: String::new(),
            },
        );
        assert_eq!(RESULTS.with(|results| results.borrow().len()), 4);
        let id = enqueue(mask).unwrap();
        handle(
            &document,
            &InputEvent::WebResponse {
                request: id,
                accept: false,
                text: String::new(),
            },
        );
        assert_eq!(
            RESULTS.with(|results| results.borrow().clone()),
            vec![denied; 5]
        );
        assert!(STATE
            .with(|state| state.borrow().pending.is_empty() && state.borrow().grants.is_empty()));
        super::super::HOST.with(|state| state.borrow_mut().take());
        request(
            &document,
            Some(&origin),
            mask,
            Reply::Prompt(callback.clone()),
            None,
        );
        assert_eq!(
            RESULTS.with(|results| results.borrow().clone()),
            vec![denied; 6]
        );
    }
}
