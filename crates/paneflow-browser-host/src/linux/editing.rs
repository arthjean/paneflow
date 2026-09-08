use paneflow_browser_protocol::BrowserId;
use std::cell::RefCell;
use std::collections::BTreeMap;

use cef::*;
use paneflow_browser_protocol::{clipboard_text_is_valid, EditAction};
use serde_json::json;

use super::{current_browser, emit};

#[derive(Default)]
struct Editing {
    next_menu: u64,
    menu: Option<PendingMenu>,
}

struct PendingMenu {
    request: u64,
    callback: RunContextMenuCallback,
    commands: Vec<i32>,
}

thread_local! {
    static EDITING: RefCell<BTreeMap<BrowserId, RefCell<Editing>>> = const { RefCell::new(BTreeMap::new()) };
}

fn with<R: Default>(f: impl FnOnce(&RefCell<Editing>) -> R) -> R {
    let Some(document) = super::current_document() else {
        return R::default();
    };
    EDITING.with(|state| {
        let mut state = state.borrow_mut();
        f(state.entry(document.browser).or_default())
    })
}

fn cancel_menu() {
    let pending = with(|state| state.borrow_mut().menu.take());
    if let Some(pending) = pending {
        pending.callback.cancel();
        emit(json!({ "native": "context_menu_closed", "request": pending.request }));
    }
}

pub(super) fn clear() {
    super::clipboard::clear();
    cancel_menu();
}

pub(super) fn choose(request: u64, command: Option<i32>) {
    let pending = with(|state| {
        let mut state = state.borrow_mut();
        if state
            .menu
            .as_ref()
            .is_some_and(|menu| menu.request == request)
        {
            state.menu.take()
        } else {
            None
        }
    });
    if let Some(pending) = pending {
        match command.filter(|command| pending.commands.contains(command)) {
            Some(command) => pending.callback.cont(command, EventFlags::default()),
            None => pending.callback.cancel(),
        }
        emit(json!({ "native": "context_menu_closed", "request" : request }));
    }
}

pub(super) fn edit(action: EditAction, request: u64) {
    let Some(browser) = current_browser() else {
        return;
    };
    let Some(frame) = browser.focused_frame() else {
        return;
    };
    match action {
        EditAction::Copy | EditAction::Cut => {
            super::clipboard::request(&frame, request, action == EditAction::Cut);
        }
        EditAction::Paste { text } => {
            if let Some(host) = browser.host().filter(|_| clipboard_text_is_valid(&text)) {
                host.ime_commit_text(
                    Some(&CefString::from(text.as_str())),
                    Some(&Range {
                        from: u32::MAX,
                        to: u32::MAX,
                    }),
                    0,
                );
            }
        }
        EditAction::SelectAll => frame.select_all(),
        EditAction::Undo => frame.undo(),
        EditAction::Redo => frame.redo(),
    }
}

wrap_context_menu_handler! {
    pub(super) struct Menus;

    impl ContextMenuHandler {
        fn run_context_menu(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, params: Option<&mut ContextMenuParams>, model: Option<&mut MenuModel>, callback: Option<&mut RunContextMenuCallback>) -> i32 {
            let _context = super::Context::browser(_browser.as_deref());
            cancel_menu();
            let Some(callback) = callback else { return 0; };
            let (Some(params), Some(model)) = (params, model) else {
                callback.cancel();
                return 1;
            };
            let mut items = Vec::new();
            let mut commands = Vec::new();
            for index in 0..model.count().min(64) {
                let command = model.command_id_at(index);
                if command < 0 || model.is_visible_at(index) == 0 || model.sub_menu_at(index).is_some() {
                    continue;
                }
                let label: String = CefString::from(&model.label_at(index)).to_string().chars().filter(|character| !character.is_control()).take(256).collect();
                if label.is_empty() { continue; }
                let enabled = model.is_enabled_at(index) != 0;
                if enabled { commands.push(command); }
                items.push(json!({ "command": command, "label": label, "enabled": enabled, "checked": model.is_checked_at(index) != 0 }));
            }
            if items.is_empty() {
                callback.cancel();
                return 1;
            }
            let request = with(|state| {
                let mut state = state.borrow_mut();
                state.next_menu = state.next_menu.wrapping_add(1).max(1);
                let request = state.next_menu;
                state.menu = Some(PendingMenu { request, callback: callback.clone(), commands });
                request
            });
            emit(json!({ "native": "context_menu", "request": request, "x": params.xcoord(), "y": params.ycoord(), "items": items }));
            1
        }
    }
}
