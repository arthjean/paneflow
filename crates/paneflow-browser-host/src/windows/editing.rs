use super::{emit, Presenter};
use cef::*;
use paneflow_browser_protocol::{BrowserId, Document};
use serde_json::json;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

struct PendingMenu {
    document: Document,
    request: u64,
    commands: Vec<i32>,
    callback: RunContextMenuCallback,
}

thread_local! {
    static MENUS: RefCell<BTreeMap<BrowserId, PendingMenu>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT_REQUEST: RefCell<u64> = const { RefCell::new(0) };
}

pub(super) fn cancel(browser: &BrowserId) {
    let pending = MENUS.with(|menus| menus.borrow_mut().remove(browser));
    if let Some(pending) = pending {
        pending.callback.cancel();
        let _ = emit(
            json!({"native":"context_menu_closed", "document":pending.document, "request":pending.request}),
        );
    }
}

pub(super) fn choose(document: &Document, request: u64, command: Option<i32>) {
    let pending = MENUS.with(|menus| {
        let mut menus = menus.borrow_mut();
        if menus
            .get(&document.browser)
            .is_some_and(|menu| menu.document == *document && menu.request == request)
        {
            menus.remove(&document.browser)
        } else {
            None
        }
    });
    if let Some(pending) = pending {
        match command.filter(|command| pending.commands.contains(command)) {
            Some(command) => pending.callback.cont(command, EventFlags::default()),
            None => pending.callback.cancel(),
        }
        let _ =
            emit(json!({"native":"context_menu_closed", "document":document, "request":request}));
    }
}

wrap_context_menu_handler! {
    pub(super) struct Menus { presenter: Rc<RefCell<Presenter>> }

    impl ContextMenuHandler {
        fn run_context_menu(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, params: Option<&mut ContextMenuParams>, model: Option<&mut MenuModel>, callback: Option<&mut RunContextMenuCallback>) -> i32 {
            let Some(callback) = callback else { return 1 };
            let document = match self.presenter.try_borrow() {
                Ok(presenter) => presenter.document().clone(),
                Err(_) => { callback.cancel(); return 1; }
            };
            cancel(&document.browser);
            let (Some(params), Some(model)) = (params, model) else {
                callback.cancel(); return 1;
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
                items.push(json!({"command":command,"label":label,"enabled":enabled,"checked":model.is_checked_at(index) != 0}));
            }
            if items.is_empty() { callback.cancel(); return 1; }
            let request = NEXT_REQUEST.with(|next| {
                let mut next = next.borrow_mut();
                *next = next.wrapping_add(1).max(1);
                *next
            });
            MENUS.with(|menus| menus.borrow_mut().insert(document.browser.clone(), PendingMenu {
                document:document.clone(), request, commands, callback:callback.clone(),
            }));
            if emit(json!({"native":"context_menu","document":document,"request":request,"x":params.xcoord(),"y":params.ycoord(),"items":items})).is_err() {
                cancel(&document.browser);
            }
            1
        }
    }
}
