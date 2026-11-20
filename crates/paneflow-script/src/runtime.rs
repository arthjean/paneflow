use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mlua::{Function, HookTriggers, Lua, MultiValue, Table, Value as LuaValue, VmState};
use serde_json::Value;

use crate::clock::{Clock, SystemClock, Wake};
use crate::error::{self, Halt};
use crate::executor::{Executor, RunEnd, TaskKind};

pub use crate::config::LoadReport;

pub const HOOK_INSTRUCTIONS: u32 = 1000;

pub const EXIT_INTERRUPTED: i32 = 130;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneContext {
    pub surface_id: Option<u64>,
    pub pane_id: Option<u64>,
    pub tab_id: Option<u64>,
    pub workspace_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallError {
    pub message: String,
    pub code: i64,
}

impl CallError {
    pub const INVALID_PARAMS: i64 = -32602;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const UNAVAILABLE: i64 = -32000;

    pub fn new(message: impl Into<String>, code: i64) -> Self {
        Self {
            message: message.into(),
            code,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Discovery {
    pub methods: Vec<Value>,
    pub unavailable: Option<CallError>,
    pub context: PaneContext,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FeedFrame {
    Event(Value),
    Resync,
    Idle,
    Disconnected(String),
}

pub trait EventFeed: Send {
    fn next(&mut self, timeout: Duration) -> FeedFrame;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionIds {
    pub surface_id: Option<u64>,
    pub pane_id: Option<u64>,
    pub tab_id: Option<u64>,
    pub workspace_id: Option<u64>,
}

pub trait Backend: Send + Sync {
    fn discover(&self) -> Discovery;
    fn call(&self, method: &str, params: Value) -> Result<Value, CallError>;
    fn follow(&self) -> Result<Box<dyn EventFeed>, CallError>;
    fn resolve_session(&self, session: &str) -> Option<SessionIds>;
    fn window_attached(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

pub trait Sink: Send + Sync {
    fn log(&self, level: Level, message: &str);
    fn report(&self, message: &str);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventScope {
    All,
    Workspace(u64),
}

pub struct Options {
    pub clock: Arc<dyn Clock>,
    pub sink: Arc<dyn Sink>,
    pub script_dir: Option<PathBuf>,
    pub lua_dir: Option<PathBuf>,
    pub args: Value,
    pub origin: String,
    pub all_workspaces: bool,
    pub fail_fast: bool,
    pub run_for: Option<Duration>,
    pub memory_limit: Option<usize>,
    pub deadline: Option<Duration>,
    pub config_root: Option<PathBuf>,
    pub serving: bool,
}

impl Options {
    pub fn new(sink: Arc<dyn Sink>) -> Self {
        Self {
            clock: Arc::new(SystemClock::new()),
            sink,
            script_dir: None,
            lua_dir: None,
            args: Value::Object(serde_json::Map::new()),
            origin: "cli".to_string(),
            all_workspaces: false,
            fail_fast: false,
            run_for: None,
            memory_limit: None,
            deadline: None,
            config_root: None,
            serving: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Returned(Option<Value>),
    Failed(String),
    Stopped(i32),
    Interrupted,
}

impl Outcome {
    pub fn status(&self) -> i32 {
        match self {
            Self::Returned(_) => 0,
            Self::Failed(_) => 1,
            Self::Stopped(code) => *code,
            Self::Interrupted => EXIT_INTERRUPTED,
        }
    }
}

#[derive(Clone)]
pub struct Interrupter {
    flag: Arc<AtomicBool>,
    wake: Arc<Wake>,
}

impl Interrupter {
    pub fn interrupt(&self) {
        self.flag.store(true, Ordering::Release);
        self.wake.notify();
    }
}

pub(crate) struct Shared {
    pub executor: Rc<Executor>,
    pub backend: Arc<dyn Backend>,
    pub sink: Arc<dyn Sink>,
    pub halt: RefCell<Option<Halt>>,
    pub interrupt: Arc<AtomicBool>,
    pub deadline: Option<Duration>,
    pub expires_at: std::cell::Cell<Option<Duration>>,
    pub fail_fast: bool,
    pub origin: String,
    pub scope: RefCell<EventScope>,
    pub context: RefCell<PaneContext>,
    pub methods: RefCell<crate::api::Methods>,
    pub timers: crate::timers::Timers,
    pub events: crate::events::Events,
    pub processes: crate::exec::Processes,
    pub config: crate::config::Config,
    pub tasks:
        RefCell<std::collections::HashMap<crate::executor::TaskId, crate::config::TaskScope>>,
    pub stats: Arc<crate::config::Stats>,
    pub serving: bool,
    pub required: RefCell<Vec<PathBuf>>,
    pub mailbox: crate::serve::Mailbox,
}

impl Shared {
    pub fn halt_with(&self, halt: Halt) {
        let mut current = self.halt.borrow_mut();
        if current.is_none() {
            *current = Some(halt);
        }
        drop(current);
        self.executor.wake_handle().notify();
    }

    pub fn halted(&self) -> bool {
        self.halt.borrow().is_some() || self.interrupt.load(Ordering::Acquire)
    }

    pub fn report_failure(&self, what: &str, error: &mlua::Error) {
        if error::halt_of(error).is_some() || self.halted() {
            return;
        }
        let message = error::describe(error);
        match self.executor.current().map(|(_, kind)| kind) {
            Some(TaskKind::Timer) => {
                self.stats.timer_errors.fetch_add(1, Ordering::AcqRel);
            }
            Some(TaskKind::Handler) => {
                self.stats.handler_errors.fetch_add(1, Ordering::AcqRel);
            }
            _ => {}
        }
        self.sink.report(&format!("Error in {what}: {message}"));
        if self.fail_fast {
            self.halt_with(Halt::Fatal(message));
        }
    }
}

pub(crate) fn shared(lua: &Lua) -> mlua::Result<Rc<Shared>> {
    lua.app_data_ref::<Rc<Shared>>()
        .map(|shared| shared.clone())
        .ok_or_else(|| mlua::Error::RuntimeError("the Paneflow runtime is not attached".into()))
}

pub(crate) fn context_table(lua: &Lua, ids: &PaneContext, origin: &str) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("surface_id", ids.surface_id)?;
    table.set("pane_id", ids.pane_id)?;
    table.set("tab_id", ids.tab_id)?;
    table.set("workspace_id", ids.workspace_id)?;
    table.set("origin", origin)?;
    Ok(table)
}

pub struct Runtime {
    pub(crate) lua: Lua,
    pub(crate) shared: Rc<Shared>,
    run_for: Option<Duration>,
}

pub struct Chunk {
    pub(crate) function: Function,
}

fn install_hook(lua: &Lua, shared: &Rc<Shared>) -> mlua::Result<()> {
    let weak = Rc::downgrade(shared);
    let hook = move |_lua: &Lua, _debug: &mlua::debug::Debug| -> mlua::Result<VmState> {
        let Some(shared) = weak.upgrade() else {
            return Ok(VmState::Continue);
        };
        if shared.interrupt.load(Ordering::Acquire) {
            return Err(error::halt(Halt::Interrupted));
        }
        if let Some(halt) = shared.halt.borrow().clone() {
            return Err(error::halt(halt));
        }
        if shared
            .expires_at
            .get()
            .is_some_and(|at| shared.executor.now() >= at)
        {
            shared.halt_with(Halt::Expired);
            return Err(error::halt(Halt::Expired));
        }
        if crate::config::current_deadline(&shared).is_some_and(|at| shared.executor.now() >= at) {
            return Err(crate::config::deadline_error(_lua));
        }
        if let Some(limit) = shared.deadline {
            let ran = shared
                .executor
                .now()
                .saturating_sub(shared.executor.slice_started());
            if ran > limit {
                return Err(mlua::Error::RuntimeError(format!(
                    "timeout: the Lua code ran for more than {} s without waiting",
                    crate::timers::seconds_label(limit)
                )));
            }
        }
        Ok(VmState::Continue)
    };
    let triggers = HookTriggers::new().every_nth_instruction(HOOK_INSTRUCTIONS);
    lua.set_global_hook(triggers, hook)
}

impl Runtime {
    pub fn new(backend: Arc<dyn Backend>, options: Options) -> Result<Self, String> {
        let lua = crate::stdlib::new_state().map_err(|error| error::describe(&error))?;
        let wake = Arc::new(Wake::default());
        let executor = Executor::new(options.clock.clone(), wake.clone());
        let shared = Rc::new(Shared {
            executor,
            backend,
            sink: options.sink.clone(),
            halt: RefCell::new(None),
            interrupt: Arc::new(AtomicBool::new(false)),
            deadline: options.deadline,
            expires_at: std::cell::Cell::new(None),
            fail_fast: options.fail_fast,
            origin: options.origin.clone(),
            scope: RefCell::new(EventScope::All),
            context: RefCell::new(PaneContext::default()),
            methods: RefCell::new(crate::api::Methods::default()),
            timers: crate::timers::Timers::default(),
            events: crate::events::Events::new(options.all_workspaces),
            processes: crate::exec::Processes::default(),
            config: crate::config::Config::default(),
            tasks: RefCell::new(std::collections::HashMap::new()),
            stats: Arc::new(crate::config::Stats::default()),
            serving: options.serving,
            required: RefCell::new(Vec::new()),
            mailbox: crate::serve::new_mailbox(wake),
        });
        lua.set_app_data(shared.clone());
        let setup = || -> mlua::Result<()> {
            if let Some(limit) = options.memory_limit {
                lua.set_memory_limit(limit)?;
            }
            crate::stdlib::restrict(
                &lua,
                options.script_dir.as_deref(),
                options.lua_dir.as_deref(),
            )?;
            install_hook(&lua, &shared)?;
            let paneflow = lua.create_table()?;
            let (guard, plain) = crate::guard::install(&lua)?;
            lua.set_named_registry_value(crate::guard::PLAIN_KEY, plain)?;
            crate::json::install(&lua, &paneflow)?;
            crate::layout::install(&lua, &paneflow)?;
            install_log(&lua, &paneflow)?;
            install_stop(&lua, &paneflow)?;
            paneflow.set("args", crate::json::to_lua(&lua, &options.args)?)?;
            paneflow.set(
                "ctx",
                context_table(&lua, &PaneContext::default(), &options.origin)?,
            )?;
            crate::timers::install(&lua, &paneflow, &guard)?;
            crate::events::install(&lua, &paneflow, &guard)?;
            crate::exec::install(&lua, &paneflow, &guard)?;
            crate::api::install_call(&lua, &paneflow, &guard)?;
            crate::config::install(&lua, &paneflow, &guard)?;
            if let Some(root) = options.config_root.as_deref() {
                crate::stdlib::install_config_searcher(&lua, root)?;
            }
            lua.set_named_registry_value(crate::guard::REGISTRY_KEY, guard)?;
            crate::guard::plain_tree(&lua, &paneflow)?;
            lua.globals().set("paneflow", paneflow)?;
            Ok(())
        };
        setup().map_err(|error| error::describe(&error))?;
        let processes_shared = Rc::downgrade(&shared);
        shared.executor.on_task_end(move |task| {
            if let Some(shared) = processes_shared.upgrade() {
                shared.processes.end_scope(&shared, task);
            }
        });
        Ok(Self {
            lua,
            shared,
            run_for: options.run_for,
        })
    }

    pub fn interrupter(&self) -> Interrupter {
        Interrupter {
            flag: self.shared.interrupt.clone(),
            wake: self.shared.executor.wake_handle(),
        }
    }

    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    pub fn compile(&self, source: &[u8], name: &str) -> Result<Chunk, String> {
        self.lua
            .load(source)
            .set_name(name)
            .into_function()
            .map(|function| Chunk { function })
            .map_err(|error| error::describe(&error))
    }

    pub fn attach(&self) -> Result<(), String> {
        let discovery = self.shared.backend.discover();
        let context = discovery.context.clone();
        *self.shared.context.borrow_mut() = context.clone();
        *self.shared.scope.borrow_mut() = match context.workspace_id {
            Some(workspace) if !self.shared.events.all_workspaces() => {
                EventScope::Workspace(workspace)
            }
            _ => EventScope::All,
        };
        let paneflow: Table = self
            .lua
            .globals()
            .get("paneflow")
            .map_err(|error| error::describe(&error))?;
        let guard: Function = self
            .lua
            .named_registry_value(crate::guard::REGISTRY_KEY)
            .map_err(|error| error::describe(&error))?;
        paneflow
            .set(
                "ctx",
                context_table(&self.lua, &context, &self.shared.origin)
                    .map_err(|error| error::describe(&error))?,
            )
            .map_err(|error| error::describe(&error))?;
        crate::api::install(&self.lua, &paneflow, &guard, discovery)
            .map_err(|error| error::describe(&error))?;
        crate::events::start_feed(&self.shared);
        Ok(())
    }

    pub fn run(&self, chunk: Chunk) -> Outcome {
        *self.shared.halt.borrow_mut() = None;
        let returned: Rc<RefCell<Option<mlua::Result<MultiValue>>>> = Rc::new(RefCell::new(None));
        let slot = returned.clone();
        let shared = self.shared.clone();
        let function = chunk.function;
        self.shared.executor.spawn(TaskKind::TopLevel, async move {
            let result = function.call_async::<MultiValue>(()).await;
            let failed = result.as_ref().err().cloned();
            *slot.borrow_mut() = Some(result);
            match failed {
                Some(error) => {
                    if error::halt_of(&error).is_none() && !shared.halted() {
                        shared.halt_with(Halt::Fatal(error::describe(&error)));
                    }
                }
                None => announce_waiting(&shared),
            }
        });
        if let Some(limit) = self.run_for {
            let expires_at = self.shared.executor.now() + limit;
            self.shared.expires_at.set(Some(expires_at));
            let sleep = self.shared.executor.sleep_until(expires_at);
            let shared = self.shared.clone();
            self.shared.executor.spawn(TaskKind::Daemon, async move {
                sleep.await;
                shared.halt_with(Halt::Expired);
            });
        }
        let shared = self.shared.clone();
        let stop = move || shared.halted();
        let end = self.shared.executor.run(&stop);
        let outcome = self.outcome(end, &returned);
        self.shutdown();
        outcome
    }

    fn outcome(
        &self,
        end: RunEnd,
        returned: &Rc<RefCell<Option<mlua::Result<MultiValue>>>>,
    ) -> Outcome {
        if self.shared.interrupt.load(Ordering::Acquire) {
            return Outcome::Interrupted;
        }
        let halt = self.shared.halt.borrow().clone();
        match halt {
            Some(Halt::Interrupted) => return Outcome::Interrupted,
            Some(Halt::Fatal(message)) => return Outcome::Failed(message),
            Some(Halt::Stop(code)) => return Outcome::Stopped(code),
            Some(Halt::Expired) | None => {}
        }
        let _ = end;
        let value = match returned.borrow_mut().take() {
            Some(Ok(values)) => values.into_iter().next(),
            Some(Err(error)) => match error::halt_of(&error) {
                Some(Halt::Stop(code)) => return Outcome::Stopped(code),
                Some(Halt::Interrupted) => return Outcome::Interrupted,
                Some(Halt::Fatal(message)) => return Outcome::Failed(message),
                Some(Halt::Expired) => None,
                None => return Outcome::Failed(error::describe(&error)),
            },
            None => None,
        };
        match value {
            None | Some(LuaValue::Nil) => Outcome::Returned(None),
            Some(value) => match crate::json::to_json(&self.lua, &value) {
                Ok(json) => Outcome::Returned(Some(json)),
                Err(message) => Outcome::Failed(format!(
                    "the script returned a value that cannot be printed as JSON: {message}"
                )),
            },
        }
    }

    pub fn stats(&self) -> Arc<crate::config::Stats> {
        self.shared.stats.clone()
    }

    pub(crate) fn shutdown(&self) {
        self.shared.events.stop_feed();
        self.shared.processes.stop_all();
        self.shared.executor.cancel_all();
    }
}

fn announce_waiting(shared: &Rc<Shared>) {
    let timers = shared.timers.active();
    let handlers = shared.events.handler_count();
    if timers == 0 && handlers == 0 {
        return;
    }
    let mut parts = Vec::new();
    if timers > 0 {
        parts.push(format!(
            "{timers} active timer{}",
            if timers == 1 { "" } else { "s" }
        ));
    }
    if handlers > 0 {
        parts.push(format!(
            "{handlers} event handler{}",
            if handlers == 1 { "" } else { "s" }
        ));
    }
    shared.sink.report(&format!(
        "paneflow do: the script returned; running with {} (Ctrl-C to stop)",
        parts.join(" and ")
    ));
}

fn install_log(lua: &Lua, paneflow: &Table) -> mlua::Result<()> {
    paneflow.set(
        "log",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args.into_iter();
            let first = args.next().unwrap_or(LuaValue::Nil);
            let second = args.next();
            let (level, message) = match second {
                Some(message) => {
                    let name = match &first {
                        LuaValue::String(name) => name.to_string_lossy(),
                        other => {
                            return Err(error::located(
                                lua,
                                format!(
                                    "paneflow.log: the level must be a string, not a {}",
                                    other.type_name()
                                ),
                            ));
                        }
                    };
                    let level = Level::parse(&name).ok_or_else(|| {
                        error::located(
                            lua,
                            format!(
                                "paneflow.log: unknown level '{name}'; use debug, info, warn or error"
                            ),
                        )
                    })?;
                    (level, message)
                }
                None => (Level::Info, first),
            };
            let text = match &message {
                LuaValue::String(text) => text.to_string_lossy(),
                other => other.to_string()?,
            };
            shared(lua)?.sink.log(level, &text);
            Ok(())
        })?,
    )
}

fn install_stop(lua: &Lua, paneflow: &Table) -> mlua::Result<()> {
    paneflow.set(
        "stop",
        lua.create_function(|lua, code: Option<i64>| -> mlua::Result<()> {
            let code = code.unwrap_or(0);
            let code = i32::try_from(code).map_err(|_| {
                error::located(lua, format!("paneflow.stop: {code} is not an exit status"))
            })?;
            let halt = Halt::Stop(code);
            shared(lua)?.halt_with(halt.clone());
            Err(error::halt(halt))
        })?,
    )
}
