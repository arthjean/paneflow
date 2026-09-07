use std::cell::RefCell;

use cef::*;
use paneflow_browser_protocol::clipboard_text_is_valid;
use serde_json::json;

use super::{current_browser, emit};

pub(super) const MESSAGE: &str = "paneflow.clipboard.snapshot.v1";
pub(super) const RESPONSE: &str = "paneflow.clipboard.response.v1";
pub(super) const COPY: i32 = 0;
pub(super) const CUT: i32 = 1;
pub(super) const COMMIT: i32 = 2;
pub(super) const CANCEL: i32 = 3;

#[derive(Default)]
struct Clipboard {
    sequence: u64,
    pending: Option<Pending>,
}

struct Pending {
    token: u64,
    request: u64,
    frame: String,
    cut: bool,
    delivered: bool,
}

thread_local! {
    static CLIPBOARD: RefCell<Clipboard> = RefCell::new(Clipboard::default());
}

fn send(frame: &Frame, token: u64, action: i32) {
    let Some(mut message) = process_message_create(Some(&MESSAGE.into())) else {
        return;
    };
    let Some(args) = message.argument_list() else {
        return;
    };
    args.set_string(0, Some(&token.to_string().as_str().into()));
    args.set_int(1, action);
    frame.send_process_message(ProcessId::RENDERER, Some(&mut message));
}

pub(super) fn clear() {
    let pending = CLIPBOARD.with(|state| state.borrow_mut().pending.take());
    if let Some(pending) = pending {
        if let Some(frame) = current_browser()
            .and_then(|browser| browser.frame_by_identifier(Some(&pending.frame.as_str().into())))
        {
            send(&frame, pending.token, CANCEL);
        }
    }
}

pub(super) fn request(frame: &Frame, request: u64, cut: bool) {
    clear();
    let token = CLIPBOARD.with(|state| {
        let mut state = state.borrow_mut();
        state.sequence = state.sequence.wrapping_add(1).max(1);
        let token = state.sequence;
        state.pending = Some(Pending {
            token,
            request,
            frame: CefString::from(&frame.identifier()).to_string(),
            cut,
            delivered: false,
        });
        token
    });
    send(frame, token, if cut { CUT } else { COPY });
}

pub(super) fn receive(
    frame: Option<&mut Frame>,
    source: ProcessId,
    message: Option<&mut ProcessMessage>,
) -> i32 {
    let Some(message) = message else {
        return 0;
    };
    if source != ProcessId::RENDERER || CefString::from(&message.name()).to_string() != RESPONSE {
        return 0;
    }
    let (Some(frame), Some(args)) = (frame, message.argument_list()) else {
        return 1;
    };
    if args.size() != 2
        || args.get_type(0) != ValueType::STRING
        || args.get_type(1) != ValueType::STRING
    {
        return 1;
    }
    let Ok(token) = CefString::from(&args.string(0)).to_string().parse::<u64>() else {
        return 1;
    };
    let identifier = CefString::from(&frame.identifier()).to_string();
    let focused = current_browser()
        .and_then(|browser| browser.focused_frame())
        .is_some_and(|focused| CefString::from(&focused.identifier()).to_string() == identifier);
    if !focused {
        return 1;
    }
    let text = CefString::from(&args.string(1)).to_string();
    if !clipboard_text_is_valid(&text) || text.is_empty() {
        return 1;
    }
    let request = CLIPBOARD.with(|state| {
        let mut state = state.borrow_mut();
        let pending = state.pending.as_mut()?;
        if pending.token != token || pending.frame != identifier || pending.delivered {
            return None;
        }
        pending.delivered = true;
        Some(pending.request)
    });
    if let Some(request) = request {
        emit(json!({ "native": "clipboard", "request": request, "text": text }));
    }
    1
}

pub(super) fn written(request: u64) {
    let pending = CLIPBOARD.with(|state| {
        let mut state = state.borrow_mut();
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.request == request && pending.delivered)
        {
            state.pending.take()
        } else {
            None
        }
    });
    if let Some(pending) = pending.filter(|pending| pending.cut) {
        if let Some(frame) = current_browser()
            .and_then(|browser| browser.focused_frame())
            .filter(|frame| CefString::from(&frame.identifier()).to_string() == pending.frame)
        {
            send(&frame, pending.token, COMMIT);
        }
    }
}
