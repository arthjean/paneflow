use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use mlua::{Function, Lua, MultiValue, Table, Value as LuaValue};
use serde_json::{Map, Value};

use crate::error::located;
use crate::executor::{Sleep, TaskKind};
use crate::json::{to_json, to_lua};
use crate::runtime::{
    CallError, EventScope, FeedFrame, PaneContext, SessionIds, Shared, context_table, shared,
};

pub const INBOX_CAPACITY: usize = 4096;
pub const HISTORY_CAPACITY: usize = 1024;
pub const RESYNC_EVENT: &str = "events.resync";
pub const FEED_POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    Exact(String),
    Family(String),
    Everything,
}

impl Pattern {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw == "*" {
            return Ok(Self::Everything);
        }
        if let Some(family) = raw.strip_suffix(".*") {
            if family.is_empty() || family.contains('*') {
                return Err(format!(
                    "'{raw}' is not an event type or a family such as terminal.*"
                ));
            }
            return Ok(Self::Family(format!("{family}.")));
        }
        if raw.is_empty() || raw.contains('*') {
            return Err(format!(
                "'{raw}' is not an event type or a family such as terminal.*"
            ));
        }
        Ok(Self::Exact(raw.to_string()))
    }

    pub fn matches(&self, kind: &str) -> bool {
        match self {
            Self::Exact(name) => name == kind,
            Self::Family(prefix) => kind.starts_with(prefix.as_str()),
            Self::Everything => kind != RESYNC_EVENT,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Delivered {
    pub seq: u64,
    pub kind: String,
    pub ids: PaneContext,
    pub session: Option<String>,
    pub data: Map<String, Value>,
}

impl Delivered {
    pub fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            seq: value["seq"].as_u64()?,
            kind: value["type"].as_str()?.to_string(),
            ids: PaneContext {
                surface_id: value["surface_id"].as_u64(),
                pane_id: value["pane_id"].as_u64(),
                tab_id: value["tab_id"].as_u64(),
                workspace_id: value["workspace_id"].as_u64(),
            },
            session: value["session"].as_str().map(str::to_string),
            data: value["data"].as_object().cloned().unwrap_or_default(),
        })
    }

    fn is_global(&self) -> bool {
        self.ids == PaneContext::default() && self.session.is_none()
    }

    pub fn in_scope(&self, scope: EventScope) -> bool {
        match scope {
            EventScope::All => true,
            EventScope::Workspace(workspace) => {
                self.ids.workspace_id == Some(workspace) || self.is_global()
            }
        }
    }

    fn field(&self, key: &str) -> Value {
        match key {
            "seq" => Value::from(self.seq),
            "type" => Value::String(self.kind.clone()),
            "surface_id" => self.ids.surface_id.map_or(Value::Null, Value::from),
            "pane_id" => self.ids.pane_id.map_or(Value::Null, Value::from),
            "tab_id" => self.ids.tab_id.map_or(Value::Null, Value::from),
            "workspace_id" => self.ids.workspace_id.map_or(Value::Null, Value::from),
            "session" => self.session.clone().map_or(Value::Null, Value::String),
            other => self.data.get(other).cloned().unwrap_or(Value::Null),
        }
    }

    pub fn event_table(&self, lua: &Lua) -> mlua::Result<Table> {
        let table = lua.create_table()?;
        for (key, value) in &self.data {
            table.set(key.as_str(), to_lua(lua, value)?)?;
        }
        if table.get::<LuaValue>("type")?.is_nil() {
            table.set("type", self.kind.as_str())?;
        }
        if table.get::<LuaValue>("seq")?.is_nil() {
            table.set("seq", self.seq)?;
        }
        if let Some(session) = &self.session
            && table.get::<LuaValue>("session")?.is_nil()
        {
            table.set("session", session.as_str())?;
        }
        Ok(table)
    }
}

fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => match (left.as_i64(), right.as_i64()) {
            (Some(left), Some(right)) => left == right,
            _ => left.as_f64() == right.as_f64(),
        },
        _ => left == right,
    }
}

enum Filter {
    Any,
    Fields(Vec<(String, Value)>),
    Function(Function),
}

impl Filter {
    fn from_lua(lua: &Lua, value: LuaValue) -> mlua::Result<Self> {
        match value {
            LuaValue::Nil => Ok(Self::Any),
            LuaValue::Function(function) => Ok(Self::Function(function)),
            LuaValue::Table(table) => {
                let mut fields = Vec::new();
                for pair in table.pairs::<String, LuaValue>() {
                    let (key, value) = pair?;
                    let value = to_json(lua, &value)
                        .map_err(|message| located(lua, format!("paneflow.wait: {message}")))?;
                    fields.push((key, value));
                }
                Ok(Self::Fields(fields))
            }
            other => Err(located(
                lua,
                format!(
                    "paneflow.wait: the filter must be a table of fields or a function, not a {}",
                    other.type_name()
                ),
            )),
        }
    }

    fn accepts(&self, lua: &Lua, event: &Delivered, shared: &Shared) -> bool {
        match self {
            Self::Any => true,
            Self::Fields(fields) => fields
                .iter()
                .all(|(key, expected)| same(&event.field(key), expected)),
            Self::Function(function) => {
                let tables = event_tables(lua, event);
                match tables.and_then(|(ev, ctx)| function.call::<LuaValue>((ev, ctx))) {
                    Ok(answer) => !matches!(answer, LuaValue::Nil | LuaValue::Boolean(false)),
                    Err(error) => {
                        shared.report_failure("a paneflow.wait filter", &error);
                        false
                    }
                }
            }
        }
    }
}

fn event_tables(lua: &Lua, event: &Delivered) -> mlua::Result<(Table, Table)> {
    Ok((
        event.event_table(lua)?,
        context_table(lua, &event.ids, "event")?,
    ))
}

struct Handler {
    pattern: Pattern,
    function: Function,
}

struct Waiter {
    pattern: Pattern,
    filter: Filter,
    found: RefCell<Option<Delivered>>,
    waker: RefCell<Option<Waker>>,
}

#[derive(Default)]
struct Inbox {
    frames: VecDeque<FeedFrame>,
    waker: Option<Waker>,
    overflowed: bool,
}

pub struct Events {
    all: bool,
    inbox: Arc<Mutex<Inbox>>,
    stop: Arc<AtomicBool>,
    feed_started: Cell<bool>,
    feed_error: RefCell<Option<CallError>>,
    handlers: RefCell<Vec<Rc<Handler>>>,
    waiters: RefCell<Vec<Rc<Waiter>>>,
    pump_running: Cell<bool>,
    pump_waker: RefCell<Option<Waker>>,
    cursor: Cell<u64>,
    history: RefCell<VecDeque<Delivered>>,
    sessions: RefCell<HashMap<String, SessionIds>>,
}

impl Events {
    pub fn new(all: bool) -> Self {
        Self {
            all,
            inbox: Arc::new(Mutex::new(Inbox::default())),
            stop: Arc::new(AtomicBool::new(false)),
            feed_started: Cell::new(false),
            feed_error: RefCell::new(None),
            handlers: RefCell::new(Vec::new()),
            waiters: RefCell::new(Vec::new()),
            pump_running: Cell::new(false),
            pump_waker: RefCell::new(None),
            cursor: Cell::new(0),
            history: RefCell::new(VecDeque::new()),
            sessions: RefCell::new(HashMap::new()),
        }
    }

    pub fn all_workspaces(&self) -> bool {
        self.all
    }

    pub fn handler_count(&self) -> usize {
        self.handlers.borrow().len()
    }

    pub fn stop_feed(&self) {
        self.stop.store(true, Ordering::Release);
    }

    fn idle(&self) -> bool {
        self.handlers.borrow().is_empty() && self.waiters.borrow().is_empty()
    }

    fn poke_pump(&self) {
        if let Some(waker) = self.pump_waker.borrow_mut().take() {
            waker.wake();
        }
    }
}

pub(crate) fn start_feed(shared: &Rc<Shared>) {
    let events = &shared.events;
    if events.feed_started.replace(true) {
        return;
    }
    let mut feed = match shared.backend.follow() {
        Ok(feed) => feed,
        Err(error) => {
            *events.feed_error.borrow_mut() = Some(error);
            events.feed_started.set(false);
            return;
        }
    };
    *events.feed_error.borrow_mut() = None;
    {
        let mut inbox = events.inbox.lock().unwrap_or_else(PoisonError::into_inner);
        while inbox.frames.len() < INBOX_CAPACITY {
            match feed.next(Duration::ZERO) {
                FeedFrame::Idle => break,
                frame => inbox.frames.push_back(frame),
            }
        }
    }
    let inbox = events.inbox.clone();
    let stop = events.stop.clone();
    let wake = shared.executor.wake_handle();
    let spawned = std::thread::Builder::new()
        .name("paneflow-script-events".to_string())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let frame = feed.next(FEED_POLL);
                if matches!(frame, FeedFrame::Idle) {
                    continue;
                }
                let waker = {
                    let mut inbox = inbox.lock().unwrap_or_else(PoisonError::into_inner);
                    if inbox.frames.len() >= INBOX_CAPACITY {
                        inbox.frames.pop_front();
                        inbox.overflowed = true;
                    }
                    inbox.frames.push_back(frame);
                    inbox.waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
                wake.notify();
            }
        });
    if spawned.is_err() {
        *events.feed_error.borrow_mut() = Some(CallError::new(
            "no thread could be started to follow events",
            CallError::UNAVAILABLE,
        ));
        events.feed_started.set(false);
    }
}

fn ensure_feed(shared: &Rc<Shared>) -> Option<CallError> {
    start_feed(shared);
    shared.events.feed_error.borrow().clone()
}

enum Next {
    Frame(FeedFrame),
    Resync,
    Done,
}

struct NextFrame {
    shared: Rc<Shared>,
}

impl Future for NextFrame {
    type Output = Next;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Next> {
        let events = &self.shared.events;
        if events.idle() || self.shared.halted() {
            return Poll::Ready(Next::Done);
        }
        let mut inbox = events.inbox.lock().unwrap_or_else(PoisonError::into_inner);
        if inbox.overflowed {
            inbox.overflowed = false;
            return Poll::Ready(Next::Resync);
        }
        if let Some(frame) = inbox.frames.pop_front() {
            return Poll::Ready(Next::Frame(frame));
        }
        inbox.waker = Some(context.waker().clone());
        drop(inbox);
        *events.pump_waker.borrow_mut() = Some(context.waker().clone());
        Poll::Pending
    }
}

async fn enrich(shared: &Rc<Shared>, event: &mut Delivered) {
    let Some(session) = event.session.clone() else {
        return;
    };
    if event.ids.surface_id.is_some() && event.ids.workspace_id.is_some() {
        return;
    }
    let cached = shared.events.sessions.borrow().get(&session).cloned();
    let ids = match cached {
        Some(ids) => Some(ids),
        None => {
            let backend = shared.backend.clone();
            let lookup = session.clone();
            let resolved = shared
                .executor
                .offload("session", move || backend.resolve_session(&lookup))
                .await
                .flatten();
            if let Some(ids) = &resolved {
                shared
                    .events
                    .sessions
                    .borrow_mut()
                    .insert(session, ids.clone());
            }
            resolved
        }
    };
    if let Some(ids) = ids {
        event.ids.surface_id = event.ids.surface_id.or(ids.surface_id);
        event.ids.pane_id = event.ids.pane_id.or(ids.pane_id);
        event.ids.tab_id = event.ids.tab_id.or(ids.tab_id);
        event.ids.workspace_id = event.ids.workspace_id.or(ids.workspace_id);
    }
}

fn record(shared: &Shared, event: &Delivered) {
    let mut history = shared.events.history.borrow_mut();
    if history.len() >= HISTORY_CAPACITY {
        history.pop_front();
    }
    history.push_back(event.clone());
}

fn settle_waiters(lua: &Lua, shared: &Shared, event: &Delivered) {
    let waiters: Vec<Rc<Waiter>> = shared.events.waiters.borrow().clone();
    for waiter in waiters {
        if waiter.found.borrow().is_some()
            || event.seq <= shared.events.cursor.get()
            || !waiter.pattern.matches(&event.kind)
            || !waiter.filter.accepts(lua, event, shared)
        {
            continue;
        }
        *waiter.found.borrow_mut() = Some(event.clone());
        if let Some(waker) = waiter.waker.borrow_mut().take() {
            waker.wake();
        }
        break;
    }
}

async fn run_handlers(lua: &Lua, shared: &Rc<Shared>, event: &Delivered) {
    let handlers: Vec<Rc<Handler>> = shared
        .events
        .handlers
        .borrow()
        .iter()
        .filter(|handler| handler.pattern.matches(&event.kind))
        .cloned()
        .collect();
    for handler in handlers {
        if shared.halted() {
            return;
        }
        let tables = event_tables(lua, event);
        let function = handler.function.clone();
        let task_shared = shared.clone();
        let kind = event.kind.clone();
        let task = shared.executor.spawn(TaskKind::Handler, async move {
            let result = match tables {
                Ok((ev, ctx)) => function.call_async::<()>((ctx, ev)).await,
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                task_shared.report_failure(&format!("a handler for {kind}"), &error);
            }
        });
        shared.executor.join(task).await;
    }
}

async fn resync(lua: &Lua, shared: &Rc<Shared>) {
    shared.sink.report(
        "paneflow do: the event stream restarted from the current seq; events in between were missed",
    );
    let event = Delivered {
        seq: 0,
        kind: RESYNC_EVENT.to_string(),
        ids: PaneContext::default(),
        session: None,
        data: Map::new(),
    };
    run_handlers(lua, shared, &event).await;
}

fn ensure_pump(lua: &Lua, shared: &Rc<Shared>) {
    if shared.events.pump_running.replace(true) {
        shared.events.poke_pump();
        return;
    }
    let shared = shared.clone();
    let lua = lua.weak();
    let executor = shared.executor.clone();
    executor.spawn(TaskKind::Internal, async move {
        let _hold = shared.executor.hold();
        loop {
            let next = NextFrame {
                shared: shared.clone(),
            }
            .await;
            let lua = lua.upgrade();
            match next {
                Next::Done => break,
                Next::Resync => resync(&lua, &shared).await,
                Next::Frame(FeedFrame::Resync) => resync(&lua, &shared).await,
                Next::Frame(FeedFrame::Event(value)) => {
                    let Some(mut event) = Delivered::parse(&value) else {
                        continue;
                    };
                    enrich(&shared, &mut event).await;
                    if !event.in_scope(*shared.scope.borrow()) {
                        continue;
                    }
                    record(&shared, &event);
                    settle_waiters(&lua, &shared, &event);
                    run_handlers(&lua, &shared, &event).await;
                }
                Next::Frame(FeedFrame::Idle | FeedFrame::Disconnected(_)) => {}
            }
        }
        shared.events.pump_running.set(false);
    });
}

fn pattern(lua: &Lua, what: &str, raw: &str) -> mlua::Result<Pattern> {
    Pattern::parse(raw).map_err(|message| located(lua, format!("{what}: {message}")))
}

fn on(lua: &Lua, (event, function): (String, Function)) -> mlua::Result<bool> {
    let shared = shared(lua)?;
    if let Some((_, kind)) = shared.executor.current()
        && kind != TaskKind::TopLevel
    {
        return Err(located(
            lua,
            "paneflow.on can only be called at the top level of a script, not from a handler or a timer",
        ));
    }
    let pattern = pattern(lua, "paneflow.on", &event)?;
    if pattern != Pattern::Exact(RESYNC_EVENT.to_string())
        && let Some(error) = ensure_feed(&shared)
    {
        return Err(located(
            lua,
            format!("paneflow.on: events cannot be followed: {}", error.message),
        ));
    }
    shared
        .events
        .handlers
        .borrow_mut()
        .push(Rc::new(Handler { pattern, function }));
    ensure_pump(lua, &shared);
    Ok(true)
}

struct WaitFor {
    waiter: Rc<Waiter>,
    sleep: Option<Sleep>,
}

impl Future for WaitFor {
    type Output = Option<Delivered>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Delivered>> {
        if let Some(found) = self.waiter.found.borrow_mut().take() {
            return Poll::Ready(Some(found));
        }
        if let Some(sleep) = self.sleep.as_mut()
            && Pin::new(sleep).poll(context).is_ready()
        {
            return Poll::Ready(None);
        }
        *self.waiter.waker.borrow_mut() = Some(context.waker().clone());
        Poll::Pending
    }
}

async fn wait(
    lua: Lua,
    (event, filter, timeout): (String, LuaValue, Option<f64>),
) -> mlua::Result<MultiValue> {
    let shared = shared(&lua)?;
    let pattern = pattern(&lua, "paneflow.wait", &event)?;
    let filter = Filter::from_lua(&lua, filter)?;
    let timeout = timeout
        .map(|seconds| crate::timers::duration(&lua, "paneflow.wait", seconds))
        .transpose()?;
    if let Some(error) = ensure_feed(&shared) {
        return Ok(MultiValue::from_iter([
            LuaValue::Nil,
            LuaValue::String(
                lua.create_string(format!("events cannot be followed: {}", error.message))?,
            ),
        ]));
    }
    let cursor = shared.events.cursor.get();
    let earlier: Option<Delivered> = shared
        .events
        .history
        .borrow()
        .iter()
        .filter(|event| event.seq > cursor && pattern.matches(&event.kind))
        .find(|event| filter.accepts(&lua, event, &shared))
        .cloned();
    let found = match earlier {
        Some(found) => Some(found),
        None => {
            let waiter = Rc::new(Waiter {
                pattern,
                filter,
                found: RefCell::new(None),
                waker: RefCell::new(None),
            });
            shared.events.waiters.borrow_mut().push(waiter.clone());
            ensure_pump(&lua, &shared);
            let sleep =
                timeout.map(|timeout| shared.executor.sleep_until(shared.executor.now() + timeout));
            let found = WaitFor {
                waiter: waiter.clone(),
                sleep,
            }
            .await;
            shared
                .events
                .waiters
                .borrow_mut()
                .retain(|other| !Rc::ptr_eq(other, &waiter));
            shared.events.poke_pump();
            found
        }
    };
    match found {
        Some(event) => {
            shared
                .events
                .cursor
                .set(shared.events.cursor.get().max(event.seq));
            let (ev, ctx) = event_tables(&lua, &event)?;
            Ok(MultiValue::from_iter([
                LuaValue::Table(ev),
                LuaValue::Table(ctx),
            ]))
        }
        None => Ok(MultiValue::from_iter([
            LuaValue::Nil,
            LuaValue::String(lua.create_string("timeout")?),
        ])),
    }
}

pub fn install(lua: &Lua, paneflow: &Table, guard: &Function) -> mlua::Result<()> {
    paneflow.set("on", lua.create_function(on)?)?;
    let wait = lua.create_async_function(wait)?;
    paneflow.set("wait", crate::guard::guarded(guard, "paneflow.wait", wait)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_family_matches_every_type_under_its_prefix_and_nothing_else() {
        let family = Pattern::parse("terminal.*").expect("family");
        assert!(family.matches("terminal.exited"));
        assert!(family.matches("terminal.bell"));
        assert!(!family.matches("terminals.exited"));
        assert!(!family.matches("pane.created"));
        assert!(Pattern::parse("terminal*").is_err());
        assert!(Pattern::parse(".*").is_err());
    }

    #[test]
    fn a_scoped_follower_keeps_its_workspace_and_global_events_only() {
        let event = |value: Value| Delivered::parse(&value).expect("event");
        let scope = EventScope::Workspace(4);
        assert!(
            event(json!({"seq": 1, "type": "pane.created", "workspace_id": 4, "data": {}}))
                .in_scope(scope)
        );
        assert!(
            !event(json!({"seq": 2, "type": "pane.created", "workspace_id": 5, "data": {}}))
                .in_scope(scope)
        );
        assert!(event(json!({"seq": 3, "type": "config.reloaded", "data": {}})).in_scope(scope));
        assert!(
            !event(json!({"seq": 4, "type": "terminal.bell", "session": "s-1", "data": {}}))
                .in_scope(scope)
        );
        assert!(
            event(json!({"seq": 5, "type": "pane.created", "workspace_id": 5, "data": {}}))
                .in_scope(EventScope::All)
        );
    }

    #[test]
    fn numbers_compare_equal_across_integer_and_float_spellings() {
        assert!(same(&json!(3), &json!(3.0)));
        assert!(!same(&json!(3), &json!("3")));
    }
}
