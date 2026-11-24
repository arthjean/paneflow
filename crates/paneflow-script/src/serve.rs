use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

use mlua::MultiValue;
use serde_json::Value;

use crate::clock::Wake;
use crate::config::{ActionFailure, InvokeContext, LoadReport, Reply};
use crate::error::{self, Halt};
use crate::executor::TaskKind;
use crate::runtime::{Chunk, Runtime, Shared};

struct Invocation {
    name: String,
    args: Value,
    context: InvokeContext,
    reply: Reply,
}

enum Request {
    Invoke(Box<Invocation>),
    Retire,
}

#[derive(Default)]
struct Slot {
    requests: VecDeque<Request>,
    waker: Option<Waker>,
    closed: bool,
}

#[derive(Clone)]
pub struct Mailbox {
    slot: Arc<Mutex<Slot>>,
    wake: Arc<Wake>,
}

impl Mailbox {
    fn send(&self, request: Request) -> Option<Request> {
        let waker = {
            let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
            if slot.closed {
                return Some(request);
            }
            slot.requests.push_back(request);
            slot.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        self.wake.notify();
        None
    }

    pub fn invoke(&self, name: String, args: Value, context: InvokeContext, reply: Reply) {
        let request = Request::Invoke(Box::new(Invocation {
            name,
            args,
            context,
            reply,
        }));
        if let Some(Request::Invoke(invocation)) = self.send(request) {
            (invocation.reply)(Err(ActionFailure {
                message: "the configuration that declared this action is no longer loaded"
                    .to_string(),
                timeout: false,
            }));
        }
    }

    pub fn retire(&self) {
        let _ = self.send(Request::Retire);
    }

    fn close(&self) -> Vec<Request> {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        slot.closed = true;
        slot.requests.drain(..).collect()
    }
}

struct NextRequest {
    slot: Arc<Mutex<Slot>>,
}

impl Future for NextRequest {
    type Output = Request;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Request> {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        match slot.requests.pop_front() {
            Some(request) => Poll::Ready(request),
            None => {
                slot.waker = Some(context.waker().clone());
                Poll::Pending
            }
        }
    }
}

fn refuse(requests: Vec<Request>) {
    for request in requests {
        if let Request::Invoke(invocation) = request {
            (invocation.reply)(Err(ActionFailure {
                message: "the configuration runtime stopped before the action ran".to_string(),
                timeout: false,
            }));
        }
    }
}

fn retire(shared: &Rc<Shared>) {
    shared.timers.cancel_all(shared);
    shared.events.clear_handlers(shared);
}

impl Runtime {
    pub fn mailbox(&self) -> Mailbox {
        self.shared.mailbox.clone()
    }

    pub fn serve(&self, chunk: Chunk, loaded: impl FnOnce(LoadReport) + 'static) {
        let shared = self.shared.clone();
        let function = chunk.function;
        let load_shared = shared.clone();
        let lua = self.lua.weak();
        self.shared.executor.spawn(TaskKind::TopLevel, async move {
            let result = function.call_async::<MultiValue>(()).await;
            let error = result.err().map(|error| match error::halt_of(&error) {
                Some(halt) => halt.to_string(),
                None => error::describe(&error),
            });
            let failed = error.is_some();
            let report = crate::config::report(&load_shared, error.clone());
            loaded(report);
            if failed {
                load_shared.halt_with(Halt::Fatal(error.unwrap_or_default()));
            }
        });
        let pump_shared = shared.clone();
        let slot = shared.mailbox.slot.clone();
        let guard = shared.executor.external().begin();
        self.shared.executor.spawn(TaskKind::Internal, async move {
            let _guard = guard;
            loop {
                let request = NextRequest { slot: slot.clone() }.await;
                match request {
                    Request::Invoke(invocation) => {
                        let Invocation {
                            name,
                            args,
                            context,
                            reply,
                        } = *invocation;
                        let lua = lua.upgrade();
                        crate::config::spawn_action(&lua, &pump_shared, name, args, context, reply);
                    }
                    Request::Retire => {
                        refuse(pump_shared.mailbox.close());
                        retire(&pump_shared);
                        break;
                    }
                }
            }
        });
        let stop_shared = shared.clone();
        let stop = move || stop_shared.halted();
        self.shared.executor.run(&stop);
        refuse(shared.mailbox.close());
        self.shutdown();
    }
}

pub(crate) fn new_mailbox(wake: Arc<Wake>) -> Mailbox {
    Mailbox {
        slot: Arc::new(Mutex::new(Slot::default())),
        wake,
    }
}

impl Runtime {
    pub fn load(&self, chunk: Chunk) -> LoadReport {
        let slot: Rc<std::cell::RefCell<Option<LoadReport>>> = Rc::default();
        let filled = slot.clone();
        let shared = self.shared.clone();
        let function = chunk.function;
        self.shared.executor.spawn(TaskKind::TopLevel, async move {
            let result = function.call_async::<MultiValue>(()).await;
            let error = result.err().map(|error| match error::halt_of(&error) {
                Some(halt) => halt.to_string(),
                None => error::describe(&error),
            });
            *filled.borrow_mut() = Some(crate::config::report(&shared, error));
        });
        let stop_shared = self.shared.clone();
        let done = slot.clone();
        let stop = move || stop_shared.halted() || done.borrow().is_some();
        self.shared.executor.run(&stop);
        let report = slot.borrow_mut().take();
        report.unwrap_or_else(|| {
            let message = match self.shared.halt.borrow().clone() {
                Some(halt) => halt.to_string(),
                None => "the configuration stopped before it finished loading".to_string(),
            };
            crate::config::report(&self.shared, Some(message))
        })
    }

    pub fn run_action(
        &self,
        name: String,
        args: Value,
        context: InvokeContext,
    ) -> crate::runtime::Outcome {
        let slot: Arc<Mutex<Option<crate::config::ActionResult>>> = Arc::default();
        let filled = slot.clone();
        let lua = self.lua.clone();
        crate::config::spawn_action(
            &lua,
            &self.shared,
            name,
            args,
            context,
            Box::new(move |result| {
                *filled.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
            }),
        );
        let stop_shared = self.shared.clone();
        let done = slot.clone();
        let stop = move || {
            stop_shared.halted()
                || done
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_some()
        };
        self.shared.executor.run(&stop);
        self.shutdown();
        let result = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
        match result {
            Some(Ok(value)) => crate::runtime::Outcome::Returned(value),
            Some(Err(failure)) => crate::runtime::Outcome::Failed(failure.message),
            None => match self.shared.halt.borrow().clone() {
                Some(Halt::Stop(code)) => crate::runtime::Outcome::Stopped(code),
                Some(Halt::Interrupted) => crate::runtime::Outcome::Interrupted,
                Some(halt) => crate::runtime::Outcome::Failed(halt.to_string()),
                None => crate::runtime::Outcome::Failed(
                    "the action stopped before it returned".to_string(),
                ),
            },
        }
    }
}
