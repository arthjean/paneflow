use std::cell::RefCell;
use std::collections::BTreeMap;

use cef::*;
use paneflow_browser_protocol::{Document, InputEvent};
use serde_json::json;

struct Pending {
    kind: String,
    document: Document,
    callback: JsdialogCallback,
}

thread_local! {
    static PENDING: RefCell<BTreeMap<u64, Pending>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT: RefCell<u64> = const { RefCell::new(1) };
}

pub fn clear(document: &Document) {
    let removed = PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        let ids: Vec<_> = pending
            .iter()
            .filter(|(_, item)| item.document.browser == document.browser)
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter()
            .filter_map(|id| pending.remove(&id).map(|pending| (id, pending)))
            .collect::<Vec<_>>()
    });
    for (request, pending) in removed {
        super::emit(
            json!({"native":"web_dialog_closed", "document":pending.document, "request":request}),
        );
        pending.callback.cont(0, None);
    }
}

pub fn handle(document: &Document, response: &InputEvent) {
    let InputEvent::WebResponse {
        request,
        accept,
        text,
    } = response
    else {
        return;
    };
    let pending = PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        if pending
            .get(request)
            .is_some_and(|item| &item.document == document)
        {
            pending.remove(request)
        } else {
            None
        }
    });
    if let Some(pending) = pending {
        if pending.kind == "beforeunload" && !accept {
            super::cancel_close(document);
        }
        pending
            .callback
            .cont(i32::from(*accept), Some(&text.as_str().into()));
    }
}

fn request(
    document: &Document,
    kind: &str,
    origin: String,
    message: String,
    default_text: String,
    callback: Option<&mut JsdialogCallback>,
) -> i32 {
    let Some(callback) = callback else {
        return 0;
    };
    let full = PENDING.with(|pending| {
        pending.borrow().len() >= 8
            || pending
                .borrow()
                .values()
                .any(|item| item.document.browser == document.browser)
    });
    if full || message.len() > 8192 || default_text.len() > 8192 || origin.len() > 8192 {
        callback.cont(0, None);
        return 1;
    }
    let id = NEXT.with(|next| {
        let mut next = next.borrow_mut();
        let id = *next;
        *next = next.saturating_add(1);
        id
    });
    PENDING.with(|pending| {
        pending.borrow_mut().insert(
            id,
            Pending {
                kind: kind.to_string(),
                document: document.clone(),
                callback: callback.clone(),
            },
        );
    });
    super::emit(
        json!({"native":"web_dialog", "document":document, "request":id, "kind":kind, "origin":origin, "message":message, "default_text":default_text}),
    );
    1
}

wrap_jsdialog_handler! {
    pub struct Dialogs { document: Document }
    impl JsdialogHandler {
        fn on_jsdialog(&self, _browser: Option<&mut Browser>, origin_url: Option<&CefString>, dialog_type: JsdialogType, message_text: Option<&CefString>, default_prompt_text: Option<&CefString>, callback: Option<&mut JsdialogCallback>, _suppress_message: Option<&mut i32>) -> i32 {
            let kind = if dialog_type == JsdialogType::PROMPT { "prompt" } else if dialog_type == JsdialogType::CONFIRM { "confirm" } else { "alert" };
            request(&super::latest_document(&self.document).unwrap_or_else(|| self.document.clone()), kind, origin_url.map(ToString::to_string).unwrap_or_default(), message_text.map(ToString::to_string).unwrap_or_default(), default_prompt_text.map(ToString::to_string).unwrap_or_default(), callback)
        }
        fn on_before_unload_dialog(&self, browser: Option<&mut Browser>, message_text: Option<&CefString>, _is_reload: i32, callback: Option<&mut JsdialogCallback>) -> i32 {
            let origin = browser.and_then(|browser| browser.main_frame()).map(|frame| CefString::from(&frame.url()).to_string()).unwrap_or_default();
            request(&super::latest_document(&self.document).unwrap_or_else(|| self.document.clone()), "beforeunload", origin, message_text.map(ToString::to_string).unwrap_or_default(), String::new(), callback)
        }
        fn on_reset_dialog_state(&self, _browser: Option<&mut Browser>) { clear(&self.document); }
    }
}
