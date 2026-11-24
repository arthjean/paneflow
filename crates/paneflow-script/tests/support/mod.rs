#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use paneflow_script::{
    Backend, CallError, Discovery, EventFeed, FakeClock, FeedFrame, Level, Options, Outcome,
    PaneContext, Runtime, SessionIds, Sink,
};
use serde_json::{Value, json};

type Responder = Box<dyn Fn(&str, &Value) -> Result<Value, CallError> + Send + Sync>;

pub struct FakeBackend {
    pub unavailable: Option<CallError>,
    pub context: PaneContext,
    pub calls: Mutex<Vec<(String, Value)>>,
    pub grants: Mutex<Vec<(String, Option<String>)>>,
    responder: Responder,
    feed: Mutex<Option<Receiver<FeedFrame>>>,
    pub sessions: HashMap<String, SessionIds>,
    pub follow_refused: Option<CallError>,
    pub extra_methods: Vec<Value>,
    pub window: bool,
}

impl FakeBackend {
    pub fn new() -> Self {
        Self {
            unavailable: None,
            context: PaneContext::default(),
            calls: Mutex::new(Vec::new()),
            grants: Mutex::new(Vec::new()),
            responder: Box::new(|method, _| Ok(json!({"method": method}))),
            feed: Mutex::new(None),
            sessions: HashMap::new(),
            follow_refused: None,
            extra_methods: Vec::new(),
            window: true,
        }
    }

    pub fn answering(
        mut self,
        responder: impl Fn(&str, &Value) -> Result<Value, CallError> + Send + Sync + 'static,
    ) -> Self {
        self.responder = Box::new(responder);
        self
    }

    pub fn with_feed(self) -> (Self, Sender<FeedFrame>) {
        let (sender, receiver) = channel();
        *self.feed.lock().unwrap() = Some(receiver);
        (self, sender)
    }

    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}

struct ChannelFeed {
    receiver: Receiver<FeedFrame>,
}

impl EventFeed for ChannelFeed {
    fn next(&mut self, timeout: Duration) -> FeedFrame {
        match self.receiver.recv_timeout(timeout) {
            Ok(frame) => frame,
            Err(RecvTimeoutError::Timeout) => FeedFrame::Idle,
            Err(RecvTimeoutError::Disconnected) => {
                std::thread::sleep(timeout);
                FeedFrame::Idle
            }
        }
    }
}

impl Backend for FakeBackend {
    fn discover(&self) -> Discovery {
        let catalog = paneflow_api::catalog().unwrap();
        Discovery {
            methods: catalog
                .iter()
                .map(|d| d.describe_from(true))
                .chain(self.extra_methods.iter().cloned())
                .collect(),
            unavailable: self.unavailable.clone(),
            context: self.context.clone(),
        }
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, CallError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((method.to_string(), params.clone()));
        (self.responder)(method, &params)
    }

    fn call_granted(
        &self,
        method: &str,
        params: Value,
        grant: Option<&str>,
    ) -> Result<Value, CallError> {
        self.grants
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((method.to_string(), grant.map(str::to_string)));
        self.call(method, params)
    }

    fn follow(&self) -> Result<Box<dyn EventFeed>, CallError> {
        if let Some(error) = &self.follow_refused {
            return Err(error.clone());
        }
        match self
            .feed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            Some(receiver) => Ok(Box::new(ChannelFeed { receiver })),
            None => Err(CallError::new("no worker in this test", -32000)),
        }
    }

    fn resolve_session(&self, session: &str) -> Option<SessionIds> {
        self.sessions.get(session).cloned()
    }

    fn window_attached(&self) -> bool {
        self.window
    }
}

#[derive(Default)]
pub struct CaptureSink {
    pub logs: Mutex<Vec<(Level, String)>>,
    pub reports: Mutex<Vec<String>>,
}

impl Sink for CaptureSink {
    fn log(&self, level: Level, message: &str) {
        self.logs.lock().unwrap().push((level, message.to_string()));
    }

    fn report(&self, message: &str) {
        self.reports.lock().unwrap().push(message.to_string());
    }
}

impl CaptureSink {
    pub fn reports(&self) -> Vec<String> {
        self.reports.lock().unwrap().clone()
    }

    pub fn logs(&self) -> Vec<(Level, String)> {
        self.logs.lock().unwrap().clone()
    }
}

pub struct Harness {
    pub backend: Arc<FakeBackend>,
    pub sink: Arc<CaptureSink>,
    pub clock: Arc<FakeClock>,
    pub runtime: Runtime,
}

pub fn harness_with(backend: FakeBackend, configure: impl FnOnce(&mut Options)) -> Harness {
    let backend = Arc::new(backend);
    let sink = Arc::new(CaptureSink::default());
    let clock = Arc::new(FakeClock::new());
    let mut options = Options::new(sink.clone());
    options.clock = clock.clone();
    configure(&mut options);
    let runtime = Runtime::new(backend.clone(), options).expect("runtime");
    Harness {
        backend,
        sink,
        clock,
        runtime,
    }
}

pub fn harness(backend: FakeBackend) -> Harness {
    harness_with(backend, |_| {})
}

impl Harness {
    pub fn run(&self, source: &str) -> Outcome {
        let chunk = self
            .runtime
            .compile(source.as_bytes(), "@script.lua")
            .expect("the script compiles");
        self.runtime.attach().expect("attach");
        self.runtime.run(chunk)
    }
}

pub fn run(source: &str) -> Outcome {
    harness(FakeBackend::new()).run(source)
}

pub fn returned(outcome: Outcome) -> Value {
    match outcome {
        Outcome::Returned(Some(value)) => value,
        other => panic!("expected a returned value, got {other:?}"),
    }
}

pub fn failed(outcome: Outcome) -> String {
    match outcome {
        Outcome::Failed(message) => message,
        other => panic!("expected a failure, got {other:?}"),
    }
}

impl Harness {
    pub fn define(
        &self,
        name: &str,
        function: impl Fn(&mlua::Lua, mlua::MultiValue) -> mlua::Result<mlua::MultiValue> + 'static,
    ) {
        let lua = self.runtime.lua();
        let created = lua
            .create_function(move |lua, args| function(lua, args))
            .unwrap();
        lua.globals().set(name, created).unwrap();
    }

    pub fn define_clock(&self) {
        let clock = self.clock.clone();
        self.define("now", move |_, _| {
            Ok(mlua::MultiValue::from_iter([mlua::Value::Number(
                paneflow_script::Clock::now(clock.as_ref()).as_secs_f64(),
            )]))
        });
        let clock = self.clock.clone();
        self.define("advance", move |_, args| {
            let seconds = match args.into_iter().next() {
                Some(mlua::Value::Number(seconds)) => seconds,
                Some(mlua::Value::Integer(seconds)) => seconds as f64,
                _ => 0.0,
            };
            clock.advance(Duration::from_secs_f64(seconds));
            Ok(mlua::MultiValue::new())
        });
    }
}

pub fn event(seq: u64, kind: &str, ids: Value, data: Value) -> FeedFrame {
    let mut frame = json!({"seq": seq, "ts": 1, "type": kind, "data": data});
    if let (Some(frame), Some(ids)) = (frame.as_object_mut(), ids.as_object()) {
        for (key, value) in ids {
            frame.insert(key.clone(), value.clone());
        }
    }
    FeedFrame::Event(frame)
}

pub type Recorded = std::rc::Rc<std::cell::RefCell<Vec<Value>>>;

impl Harness {
    pub fn recorder(&self) -> Recorded {
        let recorded: Recorded = Default::default();
        let sink = recorded.clone();
        self.define("record", move |lua, args| {
            let value = args.into_iter().next().unwrap_or(mlua::Value::Nil);
            sink.borrow_mut()
                .push(paneflow_script::json::to_json(lua, &value).unwrap_or(Value::Null));
            Ok(mlua::MultiValue::new())
        });
        recorded
    }
}
