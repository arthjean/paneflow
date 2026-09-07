use std::cell::RefCell;

use cef::*;
use paneflow_browser_protocol::clipboard_text_is_valid;

use super::clipboard::{CANCEL, COMMIT, COPY, CUT, MESSAGE, RESPONSE};

#[derive(PartialEq, Eq)]
struct Snapshot {
    frame: String,
    text: String,
    start: i32,
    end: i32,
    focused_path: Vec<usize>,
}

thread_local! {
    static CUT_SNAPSHOT: RefCell<Option<(u64, Snapshot)>> = const { RefCell::new(None) };
}

fn focused_path(document: &Domdocument) -> Option<Vec<usize>> {
    let mut node = document.focused_node();
    let mut path = Vec::new();
    let mut remaining = 4096usize;
    while let Some(current) = node {
        if path.len() >= 128 {
            return None;
        }
        let mut index = 0;
        let mut sibling = current.previous_sibling();
        while let Some(previous) = sibling {
            remaining = remaining.checked_sub(1)?;
            index += 1;
            sibling = previous.previous_sibling();
        }
        path.push(index);
        node = current.parent();
    }
    Some(path)
}

fn snapshot(frame: &Frame, document: &Domdocument) -> Option<Snapshot> {
    if document.focused_node().is_some_and(|node| {
        node.is_form_control_element() != 0
            && node.form_control_element_type() == DomFormControlType::INPUT_PASSWORD
    }) {
        return None;
    }
    let text = CefString::from(&document.selection_as_text()).to_string();
    if text.is_empty() || !clipboard_text_is_valid(&text) {
        return None;
    }
    Some(Snapshot {
        frame: CefString::from(&frame.identifier()).to_string(),
        text,
        start: document.selection_start_offset(),
        end: document.selection_end_offset(),
        focused_path: focused_path(document)?,
    })
}

wrap_domvisitor! {
    struct SelectionVisitor {
        frame: Frame,
        token: u64,
        action: i32,
    }

    impl Domvisitor {
        fn visit(&self, document: Option<&mut Domdocument>) {
            let current = document.and_then(|document| snapshot(&self.frame, document));
            if self.action == COMMIT {
                let saved = CUT_SNAPSHOT.with(|state| state.borrow_mut().take());
                if saved.zip(current).is_some_and(|((token, saved), current)| token == self.token && saved == current) {
                    self.frame.cut();
                }
                return;
            }
            let _ = CUT_SNAPSHOT.with(|state| state.borrow_mut().take());
            let Some(current) = current else { return; };
            let Some(mut message) = process_message_create(Some(&RESPONSE.into())) else { return; };
            let Some(args) = message.argument_list() else { return; };
            args.set_string(0, Some(&self.token.to_string().as_str().into()));
            args.set_string(1, Some(&current.text.as_str().into()));
            if self.action == CUT {
                CUT_SNAPSHOT.with(|state| *state.borrow_mut() = Some((self.token, current)));
            }
            self.frame.send_process_message(ProcessId::BROWSER, Some(&mut message));
        }
    }
}

wrap_render_process_handler! {
    pub(super) struct ClipboardRenderer;

    impl RenderProcessHandler {
        fn on_context_released(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _context: Option<&mut V8Context>) {
            let _ = CUT_SNAPSHOT.with(|state| state.borrow_mut().take());
        }

        fn on_process_message_received(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, source: ProcessId, message: Option<&mut ProcessMessage>) -> i32 {
            let Some(message) = message else { return 0; };
            if source != ProcessId::BROWSER || CefString::from(&message.name()).to_string() != MESSAGE { return 0; }
            let (Some(browser), Some(frame), Some(args)) = (browser, frame, message.argument_list()) else { return 1; };
            if args.size() != 2 || args.get_type(0) != ValueType::STRING || args.get_type(1) != ValueType::INT { return 1; }
            let Ok(token) = CefString::from(&args.string(0)).to_string().parse::<u64>() else { return 1; };
            let action = args.int(1);
            if action == CANCEL {
                CUT_SNAPSHOT.with(|state| {
                    let mut state = state.borrow_mut();
                    if state.as_ref().is_some_and(|(saved, _)| *saved == token) { *state = None; }
                });
                return 1;
            }
            if !matches!(action, COPY | CUT | COMMIT) { return 1; }
            if browser.focused_frame().is_none_or(|focused| CefString::from(&focused.identifier()).to_string() != CefString::from(&frame.identifier()).to_string()) { return 1; }
            frame.visit_dom(Some(&mut SelectionVisitor::new(frame.clone(), token, action)));
            1
        }
    }
}
