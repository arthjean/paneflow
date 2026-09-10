use cef::*;
use paneflow_browser_protocol::{Document, InputEvent};
use serde_json::json;
use std::cell::RefCell;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Enter,
    Over,
    Cancelled,
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Over,
    Drop,
    Leave,
}

impl Phase {
    fn acknowledge(self, accepted: bool) -> Step {
        match self {
            Self::Enter => Step::Over,
            Self::Over if accepted => Step::Drop,
            Self::Over | Self::Cancelled => Step::Leave,
        }
    }
}

struct Pending {
    document: Document,
    host: BrowserHost,
    event: MouseEvent,
    phase: Phase,
    token: u64,
}

thread_local! {
    static PENDING: RefCell<BTreeMap<i32, Pending>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT: RefCell<u64> = const { RefCell::new(0) };
}

pub(super) fn begin(host: &BrowserHost, paths: &[String], x: i32, y: i32) {
    if !(InputEvent::DropFiles {
        paths: paths.to_vec(),
        x,
        y,
    })
    .is_valid()
    {
        return;
    }
    let (Some(document), Some(browser)) = (super::current_document(), host.browser()) else {
        return;
    };
    let id = browser.identifier();
    if PENDING.with(|pending| pending.borrow().contains_key(&id)) {
        return;
    }
    let Some(mut data) = drag_data_create() else {
        return;
    };
    for path in paths {
        data.add_file(Some(&path.as_str().into()), None);
    }
    let event = MouseEvent { x, y, modifiers: 0 };
    let token = NEXT.with(|next| {
        let mut next = next.borrow_mut();
        *next = next.saturating_add(1);
        *next
    });
    PENDING.with(|pending| {
        pending.borrow_mut().insert(
            id,
            Pending {
                document: document.clone(),
                host: host.clone(),
                event: event.clone(),
                phase: Phase::Enter,
                token,
            },
        )
    });
    super::emit(json!({"native":"drop_enter","document":document,"count":paths.len(),"x":x,"y":y}));
    host.drag_target_drag_enter(
        Some(&mut data),
        Some(&event),
        cef::sys::cef_drag_operations_mask_t::DRAG_OPERATION_COPY.into(),
    );
    post_delayed_task(ThreadId::UI, Some(&mut Deadline::new(id, token)), 2000);
}

pub(super) fn cursor(browser: &Browser, operation: DragOperationsMask) {
    let id = browser.identifier();
    let Some(mut pending) = PENDING.with(|state| state.borrow_mut().remove(&id)) else {
        return;
    };
    let current = super::current_document();
    if current.as_ref() != Some(&pending.document) {
        pending.phase = Phase::Cancelled;
    }
    let accepted =
        operation.as_ref().0 & cef::sys::cef_drag_operations_mask_t::DRAG_OPERATION_COPY.0 != 0;
    let step = pending.phase.acknowledge(accepted);
    super::emit(
        json!({"native":"drop_ack","document":pending.document,"accepted":accepted,"phase":format!("{:?}", pending.phase)}),
    );
    match step {
        Step::Over => {
            pending.phase = Phase::Over;
            let host = pending.host.clone();
            let event = pending.event.clone();
            PENDING.with(|state| state.borrow_mut().insert(id, pending));
            host.drag_target_drag_over(
                Some(&event),
                cef::sys::cef_drag_operations_mask_t::DRAG_OPERATION_COPY.into(),
            );
        }
        Step::Drop => {
            pending.host.drag_target_drop(Some(&pending.event));
            super::emit(json!({"native":"drop_dispatched","document":pending.document}));
        }
        Step::Leave => pending.host.drag_target_drag_leave(),
    }
}

pub(super) fn cancel(document: &Document) {
    let hosts = PENDING.with(|state| {
        state
            .borrow_mut()
            .values_mut()
            .filter(|pending| pending.document.browser == document.browser)
            .map(|pending| {
                pending.phase = Phase::Cancelled;
                pending.host.clone()
            })
            .collect::<Vec<_>>()
    });
    for host in hosts {
        host.drag_target_drag_leave();
    }
}

pub(super) fn closed(browser: &Browser) {
    PENDING.with(|state| state.borrow_mut().remove(&browser.identifier()));
}

wrap_task! {
    struct Deadline { browser: i32, token: u64 }

    impl Task {
        fn execute(&self) {
            let expired = PENDING.with(|state| {
                let mut state = state.borrow_mut();
                let pending = state.get_mut(&self.browser).filter(|pending| pending.token == self.token && pending.phase != Phase::Cancelled)?;
                pending.phase = Phase::Cancelled;
                Some((pending.host.clone(), pending.document.clone()))
            });
            if let Some((host, document)) = expired {
                host.drag_target_drag_leave();
                super::emit(json!({"native":"drop_timeout","document":document}));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_requires_a_positive_over_ack_and_never_an_enter_ack() {
        assert_eq!(Phase::Enter.acknowledge(false), Step::Over);
        assert_eq!(Phase::Enter.acknowledge(true), Step::Over);
        assert_eq!(Phase::Over.acknowledge(true), Step::Drop);
        assert_eq!(Phase::Over.acknowledge(false), Step::Leave);
        assert_eq!(Phase::Cancelled.acknowledge(true), Step::Leave);
    }
}
