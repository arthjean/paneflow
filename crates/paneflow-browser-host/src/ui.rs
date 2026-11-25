use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cef::*;

use crate::host::Host;

type Work = Box<dyn FnOnce() + Send + 'static>;

wrap_task! {
    struct UiTask {
        work: Arc<Mutex<Option<Work>>>,
    }

    impl Task {
        fn execute(&self) {
            let work = self.work.lock().ok().and_then(|mut slot| slot.take());
            if let Some(work) = work {
                work();
            }
        }
    }
}

pub fn on_ui(work: impl FnOnce() + Send + 'static) -> bool {
    let mut task = UiTask::new(Arc::new(Mutex::new(Some(Box::new(work)))));
    post_task(ThreadId::UI, Some(&mut task)) == 1
}

pub fn on_ui_after(delay: Duration, work: impl FnOnce() + Send + 'static) -> bool {
    let mut task = UiTask::new(Arc::new(Mutex::new(Some(Box::new(work)))));
    let delay_ms = i64::try_from(delay.as_millis()).unwrap_or(i64::MAX);
    post_delayed_task(ThreadId::UI, Some(&mut task), delay_ms) == 1
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

pub fn install(host: Host) {
    HOST.with(|slot| *slot.borrow_mut() = Some(host));
}

pub fn uninstall() -> Option<Host> {
    HOST.with(|slot| slot.borrow_mut().take())
}

pub fn with_host<R>(work: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with(|slot| {
        let mut guard = slot.try_borrow_mut().ok()?;
        guard.as_mut().map(work)
    })
}

pub fn dispatch(work: impl FnOnce(&mut Host) -> Vec<crate::host::Effect>) {
    let effects = with_host(work).unwrap_or_default();
    crate::host::run_effects(effects);
}
