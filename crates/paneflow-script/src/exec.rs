use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{ChildStdin, Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use mlua::{Function, Lua, MultiValue, Table, Value as LuaValue};
use paneflow_process::ProcessTreeHandle;

use crate::error::located;
use crate::executor::{ExternalGuard, TaskId, TaskKind};
use crate::runtime::{Level, Shared, shared};

pub const TERM_GRACE: Duration = Duration::from_secs(2);
pub const READ_CHUNK: usize = 8 * 1024;
pub const SIGTERM: i32 = 15;

const CALLABLE_SOURCE: &str = "local exec, run = ...\nreturn setmetatable(exec, { __call = function(_, ...) return run(...) end })";

const OPTIONS: &[&str] = &[
    "cwd",
    "env",
    "clear_env",
    "stdin",
    "timeout",
    "stdout",
    "stderr",
    "text",
    "detach",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Exit {
    code: Option<i32>,
    signal: Option<i32>,
}

#[derive(Default)]
struct State {
    exit: Option<Exit>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    open_readers: usize,
    chunks: VecDeque<(Stream, Vec<u8>)>,
    wakers: Vec<Waker>,
}

impl State {
    fn wake(&mut self) {
        for waker in self.wakers.drain(..) {
            waker.wake();
        }
    }
}

enum Capture {
    Collect,
    Discard,
    Stream(Function),
}

struct Proc {
    pid: u32,
    tree: ProcessTreeHandle,
    state: Arc<Mutex<State>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    detached: bool,
    text: bool,
    timed_out: std::cell::Cell<bool>,
    collect_stdout: bool,
    collect_stderr: bool,
}

impl Proc {
    fn exited(&self) -> bool {
        self.lock().exit.is_some()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn stop_later(shared: &Rc<Shared>, process: Rc<Proc>) {
    if process.exited() {
        return;
    }
    process.tree.signal(SIGTERM);
    let task_shared = shared.clone();
    shared.executor.spawn(TaskKind::Internal, async move {
        let exited = Watch {
            process: process.clone(),
            readers: false,
            guard: None,
            shared: task_shared.clone(),
        };
        let grace = task_shared
            .executor
            .sleep_until(task_shared.executor.now() + TERM_GRACE);
        if !race(exited, grace).await {
            process.tree.kill();
        }
    });
}

#[derive(Default)]
pub struct Processes {
    by_scope: RefCell<HashMap<TaskId, Vec<Rc<Proc>>>>,
}

impl Processes {
    fn register(&self, scope: Option<TaskId>, process: Rc<Proc>) {
        if let Some(scope) = scope {
            self.by_scope
                .borrow_mut()
                .entry(scope)
                .or_default()
                .push(process);
        }
    }

    pub(crate) fn end_scope(&self, shared: &Rc<Shared>, scope: TaskId) {
        let ended = self.by_scope.borrow_mut().remove(&scope);
        for process in ended.into_iter().flatten() {
            if !process.detached {
                stop_later(shared, process);
            }
        }
    }

    pub fn stop_all(&self) {
        let all: Vec<Rc<Proc>> = self
            .by_scope
            .borrow_mut()
            .drain()
            .flat_map(|(_, processes)| processes)
            .collect();
        let deadline = Instant::now() + TERM_GRACE;
        let running: Vec<Rc<Proc>> = all
            .into_iter()
            .filter(|process| !process.detached && !process.exited())
            .collect();
        for process in &running {
            process.tree.signal(SIGTERM);
        }
        while Instant::now() < deadline && running.iter().any(|process| !process.exited()) {
            std::thread::sleep(Duration::from_millis(10));
        }
        for process in &running {
            if !process.exited() {
                process.tree.kill();
            }
        }
    }
}

struct Watch {
    process: Rc<Proc>,
    readers: bool,
    guard: Option<ExternalGuard>,
    shared: Rc<Shared>,
}

impl Future for Watch {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        let done = {
            let mut state = self.process.lock();
            let done = state.exit.is_some() && (!self.readers || state.open_readers == 0);
            if !done {
                state.wakers.push(context.waker().clone());
            }
            done
        };
        if done {
            self.guard = None;
            return Poll::Ready(());
        }
        if self.guard.is_none() {
            self.guard = Some(self.shared.executor.external().begin());
        }
        Poll::Pending
    }
}

struct NextChunk {
    process: Rc<Proc>,
    stream: Stream,
    guard: Option<ExternalGuard>,
    shared: Rc<Shared>,
}

impl Future for NextChunk {
    type Output = Option<Vec<u8>>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Vec<u8>>> {
        let polled = {
            let mut state = self.process.lock();
            let position = state
                .chunks
                .iter()
                .position(|(stream, _)| *stream == self.stream);
            match position.and_then(|position| state.chunks.remove(position)) {
                Some((_, chunk)) => Some(Some(chunk)),
                None if state.open_readers == 0 => Some(None),
                None => {
                    state.wakers.push(context.waker().clone());
                    None
                }
            }
        };
        match polled {
            Some(chunk) => {
                self.guard = None;
                Poll::Ready(chunk)
            }
            None => {
                if self.guard.is_none() {
                    self.guard = Some(self.shared.executor.external().begin());
                }
                Poll::Pending
            }
        }
    }
}

fn text_of(bytes: &[u8], text: bool) -> Vec<u8> {
    if !text {
        return bytes.to_vec();
    }
    String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .into_bytes()
}

struct Spec {
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    env: Vec<(String, Option<String>)>,
    clear_env: bool,
    stdin: Option<Option<Vec<u8>>>,
    timeout: Option<Duration>,
    stdout: Capture,
    stderr: Capture,
    text: bool,
    detach: bool,
}

fn capture(lua: &Lua, name: &str, value: LuaValue) -> mlua::Result<Capture> {
    match value {
        LuaValue::Nil | LuaValue::Boolean(true) => Ok(Capture::Collect),
        LuaValue::Boolean(false) => Ok(Capture::Discard),
        LuaValue::Function(function) => Ok(Capture::Stream(function)),
        other => Err(located(
            lua,
            format!(
                "paneflow.exec: {name} must be true, false or a function, not a {}",
                other.type_name()
            ),
        )),
    }
}

fn spec(
    lua: &Lua,
    what: &str,
    argv: LuaValue,
    options: Option<Table>,
    starting: bool,
) -> mlua::Result<Spec> {
    let argv: Vec<String> = match argv {
        LuaValue::Table(table) => table
            .sequence_values::<String>()
            .collect::<mlua::Result<_>>()
            .map_err(|_| located(lua, format!("{what}: argv must be a list of strings")))?,
        LuaValue::String(program) => vec![program.to_string_lossy()],
        other => {
            return Err(located(
                lua,
                format!(
                    "{what}: argv must be a list of strings, not a {}",
                    other.type_name()
                ),
            ));
        }
    };
    if argv.is_empty() {
        return Err(located(lua, format!("{what}: argv names no program")));
    }
    let mut spec = Spec {
        argv,
        cwd: None,
        env: Vec::new(),
        clear_env: false,
        stdin: if starting { Some(None) } else { None },
        timeout: None,
        stdout: Capture::Collect,
        stderr: Capture::Collect,
        text: true,
        detach: false,
    };
    let Some(options) = options else {
        return Ok(spec);
    };
    let mut unknown = Vec::new();
    for pair in options.pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair?;
        let key = match key {
            LuaValue::String(key) => key.to_string_lossy(),
            other => other.to_string()?,
        };
        if !OPTIONS.contains(&key.as_str()) {
            unknown.push(key);
        }
    }
    if !unknown.is_empty() {
        unknown.sort();
        shared(lua)?.sink.log(
            Level::Warn,
            &format!(
                "{}{what}: ignoring unknown option{} {}; it takes {}",
                crate::error::location(lua),
                if unknown.len() == 1 { "" } else { "s" },
                unknown.join(", "),
                OPTIONS.join(", ")
            ),
        );
    }
    spec.cwd = options.get::<Option<String>>("cwd")?.map(PathBuf::from);
    if let Some(env) = options.get::<Option<Table>>("env")? {
        for pair in env.pairs::<String, LuaValue>() {
            let (name, value) = pair?;
            let value = match value {
                LuaValue::Boolean(false) => None,
                LuaValue::String(text) => Some(text.to_string_lossy()),
                LuaValue::Integer(number) => Some(number.to_string()),
                LuaValue::Number(number) => Some(number.to_string()),
                other => {
                    return Err(located(
                        lua,
                        format!(
                            "{what}: env.{name} must be a string, or false to remove it, not a {}",
                            other.type_name()
                        ),
                    ));
                }
            };
            spec.env.push((name, value));
        }
        spec.env.sort();
    }
    spec.clear_env = options.get::<Option<bool>>("clear_env")?.unwrap_or(false);
    match options.get::<LuaValue>("stdin")? {
        LuaValue::Nil => {}
        LuaValue::Boolean(false) => spec.stdin = None,
        LuaValue::Boolean(true) => spec.stdin = Some(None),
        LuaValue::String(text) => spec.stdin = Some(Some(text.as_bytes().to_vec())),
        other => {
            return Err(located(
                lua,
                format!(
                    "{what}: stdin must be a string or a boolean, not a {}",
                    other.type_name()
                ),
            ));
        }
    }
    spec.timeout = options
        .get::<Option<f64>>("timeout")?
        .map(|seconds| crate::timers::duration(lua, what, seconds))
        .transpose()?;
    spec.stdout = capture(lua, "stdout", options.get("stdout")?)?;
    spec.stderr = capture(lua, "stderr", options.get("stderr")?)?;
    spec.text = options.get::<Option<bool>>("text")?.unwrap_or(true);
    spec.detach = options.get::<Option<bool>>("detach")?.unwrap_or(false);
    Ok(spec)
}

fn search_path(spec: &Spec) -> Option<std::ffi::OsString> {
    match spec
        .env
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
    {
        Some((_, Some(path))) => Some(path.into()),
        Some((_, None)) => None,
        None if spec.clear_env => None,
        None => std::env::var_os("PATH"),
    }
}

pub fn resolve_program(
    program: &str,
    path: Option<std::ffi::OsString>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    let named = Path::new(program);
    if named.components().count() > 1 || program.contains('/') || program.contains('\\') {
        return Ok(named.to_path_buf());
    }
    which::which_in(program, path, cwd)
        .map_err(|_| format!("program not found: {program} (searched PATH)"))
}

fn spawn_reader(
    mut pipe: impl Read + Send + 'static,
    stream: Stream,
    streaming: bool,
    state: Arc<Mutex<State>>,
) {
    let reader_state = state.clone();
    let spawned = std::thread::Builder::new()
        .name("paneflow-script-output".to_string())
        .spawn(move || {
            let state = reader_state;
            let mut buffer = vec![0u8; READ_CHUNK];
            loop {
                let read = pipe.read(&mut buffer);
                let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
                match read {
                    Ok(0) | Err(_) => {
                        state.open_readers = state.open_readers.saturating_sub(1);
                        state.wake();
                        return;
                    }
                    Ok(count) => {
                        let chunk = &buffer[..count];
                        if streaming {
                            state.chunks.push_back((stream, chunk.to_vec()));
                        } else if stream == Stream::Stdout {
                            state.stdout.extend_from_slice(chunk);
                        } else {
                            state.stderr.extend_from_slice(chunk);
                        }
                        state.wake();
                    }
                }
            }
        });
    if spawned.is_err() {
        let mut state = state_lock(&state);
        state.open_readers = state.open_readers.saturating_sub(1);
    }
}

fn state_lock(state: &Arc<Mutex<State>>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn launch(lua: &Lua, shared: &Rc<Shared>, spec: &mut Spec) -> Result<Rc<Proc>, String> {
    let cwd = match &spec.cwd {
        Some(cwd) => cwd.clone(),
        None => std::env::current_dir().map_err(|error| error.to_string())?,
    };
    let program = resolve_program(&spec.argv[0], search_path(spec), &cwd)?;
    let mut command = Command::new(&program);
    command.args(&spec.argv[1..]);
    if spec.cwd.is_some() {
        command.current_dir(&cwd);
    }
    if spec.clear_env {
        command.env_clear();
    }
    for (name, value) in &spec.env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    command.stdin(if spec.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let stdout_streams = matches!(spec.stdout, Capture::Stream(_));
    let stderr_streams = matches!(spec.stderr, Capture::Stream(_));
    command.stdout(match spec.stdout {
        Capture::Discard => Stdio::null(),
        _ => Stdio::piped(),
    });
    command.stderr(match spec.stderr {
        Capture::Discard => Stdio::null(),
        _ => Stdio::piped(),
    });
    paneflow_process::isolate_process_tree(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("{}: {error}", spec.argv[0]))?;
    let tree = ProcessTreeHandle::attach(&child).map_err(|error| {
        let _ = child.kill();
        format!("{}: {error}", spec.argv[0])
    })?;
    if spec.detach {
        let _ = tree.release();
    }
    let state = Arc::new(Mutex::new(State::default()));
    let stdin = Arc::new(Mutex::new(child.stdin.take()));
    if let Some(Some(bytes)) = spec.stdin.take() {
        let pipe = stdin.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(mut pipe) = pipe {
            let _ = std::thread::Builder::new()
                .name("paneflow-script-stdin".to_string())
                .spawn(move || {
                    let _ = pipe.write_all(&bytes);
                });
        }
    }
    if let Some(pipe) = child.stdout.take() {
        state_lock(&state).open_readers += 1;
        spawn_reader(pipe, Stream::Stdout, stdout_streams, state.clone());
    }
    if let Some(pipe) = child.stderr.take() {
        state_lock(&state).open_readers += 1;
        spawn_reader(pipe, Stream::Stderr, stderr_streams, state.clone());
    }
    let waiter_state = state.clone();
    let pid = child.id();
    std::thread::Builder::new()
        .name("paneflow-script-wait".to_string())
        .spawn(move || {
            let status = child.wait();
            let exit = match status {
                Ok(status) => Exit {
                    code: status.code(),
                    signal: signal_of(&status),
                },
                Err(_) => Exit {
                    code: None,
                    signal: None,
                },
            };
            let mut state = waiter_state.lock().unwrap_or_else(PoisonError::into_inner);
            state.exit = Some(exit);
            state.wake();
        })
        .map_err(|error| error.to_string())?;
    let process = Rc::new(Proc {
        pid,
        tree,
        state,
        stdin,
        detached: spec.detach,
        text: spec.text,
        timed_out: std::cell::Cell::new(false),
        collect_stdout: matches!(spec.stdout, Capture::Collect),
        collect_stderr: matches!(spec.stderr, Capture::Collect),
    });
    shared.processes.register(
        shared.executor.current().map(|(task, _)| task),
        process.clone(),
    );
    for (stream, capture) in [
        (
            Stream::Stdout,
            std::mem::replace(&mut spec.stdout, Capture::Discard),
        ),
        (
            Stream::Stderr,
            std::mem::replace(&mut spec.stderr, Capture::Discard),
        ),
    ] {
        if let Capture::Stream(function) = capture {
            pump_output(lua, shared, process.clone(), stream, function);
        }
    }
    Ok(process)
}

fn pump_output(
    lua: &Lua,
    shared: &Rc<Shared>,
    process: Rc<Proc>,
    stream: Stream,
    function: Function,
) {
    let shared_task = shared.clone();
    let weak = lua.weak();
    shared.executor.spawn(TaskKind::Callback, async move {
        loop {
            let chunk = NextChunk {
                process: process.clone(),
                stream,
                guard: None,
                shared: shared_task.clone(),
            }
            .await;
            let Some(chunk) = chunk else {
                break;
            };
            let lua = weak.upgrade();
            let text = match lua.create_string(text_of(&chunk, process.text)) {
                Ok(text) => text,
                Err(_) => break,
            };
            if let Err(error) = function.call_async::<()>(text).await {
                shared_task.report_failure("an output callback", &error);
            }
        }
    });
}

#[cfg(unix)]
fn signal_of(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn signal_of(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}

fn result_table(lua: &Lua, process: &Proc) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    let (exit, stdout, stderr) = {
        let mut state = process.lock();
        (
            state.exit,
            std::mem::take(&mut state.stdout),
            std::mem::take(&mut state.stderr),
        )
    };
    let exit = exit.unwrap_or(Exit {
        code: None,
        signal: None,
    });
    let timed_out = process.timed_out.get();
    table.set("code", exit.code)?;
    table.set("signal", exit.signal)?;
    if process.collect_stdout {
        table.set("stdout", lua.create_string(text_of(&stdout, process.text))?)?;
    }
    if process.collect_stderr {
        table.set("stderr", lua.create_string(text_of(&stderr, process.text))?)?;
    }
    table.set("timed_out", timed_out)?;
    table.set("ok", exit.code == Some(0) && !timed_out)?;
    Ok(table)
}

async fn settle(shared: &Rc<Shared>, process: &Rc<Proc>, limit: Option<Duration>) -> bool {
    let watch = Watch {
        process: process.clone(),
        readers: true,
        guard: None,
        shared: shared.clone(),
    };
    let Some(limit) = limit else {
        watch.await;
        return true;
    };
    let sleep = shared.executor.sleep_until(shared.executor.now() + limit);
    race(watch, sleep).await
}

async fn race(first: impl Future<Output = ()>, second: impl Future<Output = ()>) -> bool {
    let mut first = std::pin::pin!(first);
    let mut second = std::pin::pin!(second);
    std::future::poll_fn(move |context| {
        if first.as_mut().poll(context).is_ready() {
            return Poll::Ready(true);
        }
        if second.as_mut().poll(context).is_ready() {
            return Poll::Ready(false);
        }
        Poll::Pending
    })
    .await
}

async fn terminate(shared: &Rc<Shared>, process: &Rc<Proc>) {
    process.timed_out.set(true);
    process.tree.signal(SIGTERM);
    let exited = Watch {
        process: process.clone(),
        readers: false,
        guard: None,
        shared: shared.clone(),
    };
    let grace = shared
        .executor
        .sleep_until(shared.executor.now() + TERM_GRACE);
    if !race(exited, grace).await {
        process.tree.kill();
    }
    let _ = settle(shared, process, Some(TERM_GRACE)).await;
}

async fn run_to_end(shared: &Rc<Shared>, process: &Rc<Proc>, timeout: Option<Duration>) {
    if !settle(shared, process, timeout).await {
        terminate(shared, process).await;
    }
}

fn failure(lua: &Lua, message: String) -> mlua::Result<MultiValue> {
    Ok(MultiValue::from_iter([
        LuaValue::Nil,
        LuaValue::String(lua.create_string(message)?),
    ]))
}

async fn exec(lua: Lua, (argv, options): (LuaValue, Option<Table>)) -> mlua::Result<MultiValue> {
    let shared = shared(&lua)?;
    let mut spec = spec(&lua, "paneflow.exec", argv, options, false)?;
    let timeout = spec.timeout;
    let process = match launch(&lua, &shared, &mut spec) {
        Ok(process) => process,
        Err(message) => return failure(&lua, message),
    };
    run_to_end(&shared, &process, timeout).await;
    Ok(MultiValue::from_iter([LuaValue::Table(result_table(
        &lua, &process,
    )?)]))
}

fn signal_number(lua: &Lua, value: LuaValue) -> mlua::Result<i32> {
    match value {
        LuaValue::Nil => Ok(SIGTERM),
        LuaValue::Integer(number) => i32::try_from(number)
            .map_err(|_| located(lua, format!("h:kill: {number} is not a signal"))),
        LuaValue::String(name) => {
            let name = name.to_string_lossy();
            let bare = name.trim_start_matches("SIG");
            match bare {
                "HUP" => Ok(1),
                "INT" => Ok(2),
                "QUIT" => Ok(3),
                "KILL" => Ok(9),
                "TERM" => Ok(15),
                _ => Err(located(
                    lua,
                    format!(
                        "h:kill: unknown signal '{name}'; use TERM, KILL, INT, HUP, QUIT or a number"
                    ),
                )),
            }
        }
        other => Err(located(
            lua,
            format!(
                "h:kill: the signal must be a name or a number, not a {}",
                other.type_name()
            ),
        )),
    }
}

fn handle(lua: &Lua, guard: &Function, process: Rc<Proc>) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("pid", process.pid)?;
    let waited = process.clone();
    let wait =
        lua.create_async_function(move |lua, (_this, seconds): (LuaValue, Option<f64>)| {
            let process = waited.clone();
            async move {
                let shared = shared(&lua)?;
                let limit = seconds
                    .map(|seconds| crate::timers::duration(&lua, "h:wait", seconds))
                    .transpose()?;
                if !settle(&shared, &process, limit).await {
                    return failure(&lua, "timeout".to_string());
                }
                Ok(MultiValue::from_iter([LuaValue::Table(result_table(
                    &lua, &process,
                )?)]))
            }
        })?;
    table.set("wait", crate::guard::guarded(guard, "h:wait", wait)?)?;
    let writer = process.clone();
    let write =
        lua.create_async_function(move |lua, (_this, data): (LuaValue, mlua::LuaString)| {
            let process = writer.clone();
            let bytes = data.as_bytes().to_vec();
            async move {
                let shared = shared(&lua)?;
                let pipe = process.stdin.clone();
                let written = shared
                    .executor
                    .offload("stdin", move || {
                        let mut pipe = pipe.lock().unwrap_or_else(PoisonError::into_inner);
                        match pipe.as_mut() {
                            Some(pipe) => pipe
                                .write_all(&bytes)
                                .and_then(|()| pipe.flush())
                                .map_err(|error| error.to_string()),
                            None => Err("stdin is closed".to_string()),
                        }
                    })
                    .await;
                match written {
                    Some(Ok(())) => Ok(MultiValue::from_iter([LuaValue::Boolean(true)])),
                    Some(Err(message)) => failure(&lua, message),
                    None => failure(&lua, "no thread could be started to write".to_string()),
                }
            }
        })?;
    table.set("write", crate::guard::guarded(guard, "h:write", write)?)?;
    let closer = process.clone();
    table.set(
        "close_stdin",
        lua.create_function(move |_, _this: LuaValue| {
            let pipe = closer
                .stdin
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            Ok(pipe.is_some())
        })?,
    )?;
    let killer = process.clone();
    table.set(
        "kill",
        lua.create_function(move |lua, (_this, signal): (LuaValue, LuaValue)| {
            let signal = signal_number(lua, signal)?;
            if killer.exited() {
                return Ok(false);
            }
            if signal == 9 {
                killer.tree.kill();
            } else {
                killer.tree.signal(signal);
            }
            Ok(true)
        })?,
    )?;
    let runner = process;
    table.set(
        "running",
        lua.create_function(move |_, _this: LuaValue| Ok(!runner.exited()))?,
    )?;
    crate::guard::plain_tree(lua, &table)?;
    Ok(table)
}

pub fn install(lua: &Lua, paneflow: &Table, guard: &Function) -> mlua::Result<()> {
    let exec_table = lua.create_table()?;
    let start_guard = guard.clone();
    exec_table.set(
        "start",
        lua.create_function(move |lua, (argv, options): (LuaValue, Option<Table>)| {
            let shared = shared(lua)?;
            let mut spec = spec(lua, "paneflow.exec.start", argv, options, true)?;
            let timeout = spec.timeout;
            match launch(lua, &shared, &mut spec) {
                Ok(process) => {
                    if let Some(limit) = timeout {
                        let watched = process.clone();
                        let task_shared = shared.clone();
                        shared.executor.spawn(TaskKind::Internal, async move {
                            run_to_end(&task_shared, &watched, Some(limit)).await;
                        });
                    }
                    Ok(MultiValue::from_iter([LuaValue::Table(handle(
                        lua,
                        &start_guard,
                        process,
                    )?)]))
                }
                Err(message) => failure(lua, message),
            }
        })?,
    )?;
    let run = lua.create_async_function(exec)?;
    let guarded = crate::guard::guarded(guard, "paneflow.exec", run)?;
    let callable: Table = lua
        .load(CALLABLE_SOURCE)
        .set_name(format!("={}", crate::error::INTERNAL_CHUNK))
        .call((exec_table, guarded))?;
    paneflow.set("exec", callable)?;
    Ok(())
}
