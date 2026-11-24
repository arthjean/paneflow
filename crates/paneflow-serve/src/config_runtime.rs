use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use paneflow_api::methods::config::{
    ActionList, ActionSummary, CONFIG_RELOADED, ConfigProblem, ConfigStatus, InvokeParams, Invoked,
    Keymap, Severity,
};
use paneflow_api::methods::events::EventIds;
use paneflow_script::config::problem_from_message;
use paneflow_script::{
    ActionFailure, Interrupter, Level, LoadReport, Mailbox, Options, Runtime, Sink, Stats,
};
use serde_json::{Map, Value, json};

use crate::registry::ServeError;
use crate::server::Worker;

pub const MEMORY_LIMIT: usize = 256 * 1024 * 1024;
pub const RELOAD_SETTLE: Duration = Duration::from_millis(300);
pub const ERR_ACTION_FAILED: i64 = -32050;
pub const ERR_ACTION_TIMEOUT: i64 = -32051;
const LOAD_POLL: Duration = Duration::from_millis(200);
const JOIN_POLL: Duration = Duration::from_millis(10);

struct LogSink;

impl Sink for LogSink {
    fn log(&self, level: Level, message: &str) {
        match level {
            Level::Debug => log::debug!("init.lua: {message}"),
            Level::Info => log::info!("init.lua: {message}"),
            Level::Warn => log::warn!("init.lua: {message}"),
            Level::Error => log::error!("init.lua: {message}"),
        }
    }

    fn report(&self, message: &str) {
        log::warn!("init.lua: {message}");
    }
}

struct Generation {
    mailbox: Mailbox,
    interrupter: Interrupter,
    stats: Arc<Stats>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Active {
    generation: Option<Generation>,
    retiring: Vec<JoinHandle<()>>,
    status: ConfigStatus,
    actions: Vec<ActionSummary>,
    keymap: Keymap,
    required: Vec<PathBuf>,
    loaded: bool,
}

pub struct ConfigRuntime {
    path: PathBuf,
    home: PathBuf,
    active: Mutex<Active>,
    reloading: Mutex<()>,
    stopping: Arc<AtomicBool>,
    watcher: Mutex<Option<JoinHandle<()>>>,
    signals: Mutex<Option<std::sync::mpsc::Sender<Signal>>>,
}

enum Started {
    Serving {
        mailbox: Mailbox,
        interrupter: Interrupter,
        stats: Arc<Stats>,
    },
    Failed(LoadReport),
}

fn failed_report(message: String) -> LoadReport {
    LoadReport {
        ok: false,
        problems: vec![problem_from_message(Severity::Error, &message)],
        error: Some(message),
        ..LoadReport::default()
    }
}

fn now_ms() -> u64 {
    crate::state::now_ms()
}

impl ConfigRuntime {
    pub fn new(home: &Path) -> Self {
        let path = paneflow_home::init_lua_path_in(home);
        Self {
            active: Mutex::new(Active {
                status: ConfigStatus {
                    path: path.display().to_string(),
                    ok: true,
                    ..ConfigStatus::default()
                },
                ..Active::default()
            }),
            path,
            home: home.to_path_buf(),
            reloading: Mutex::new(()),
            stopping: Arc::new(AtomicBool::new(false)),
            watcher: Mutex::new(None),
            signals: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Active> {
        self.active.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn status(&self) -> ConfigStatus {
        let active = self.lock();
        let mut status = active.status.clone();
        if let Some(generation) = &active.generation {
            status.handlers = generation.stats.handlers();
            status.timers = generation.stats.timers();
            status.handler_errors = generation.stats.handler_errors();
            status.timer_errors = generation.stats.timer_errors();
        }
        status
    }

    pub fn actions(&self) -> ActionList {
        ActionList {
            actions: self.lock().actions.clone(),
        }
    }

    pub fn keymap(&self) -> Keymap {
        self.lock().keymap.clone()
    }

    pub fn watched_files(&self) -> Vec<PathBuf> {
        let mut files = vec![self.path.clone()];
        for required in &self.lock().required {
            if !files.contains(required) {
                files.push(required.clone());
            }
        }
        files
    }

    fn start(&self, worker: &Arc<Worker>) -> (Option<Generation>, LoadReport) {
        let (started_tx, started_rx) = channel::<Started>();
        let (report_tx, report_rx) = channel::<LoadReport>();
        let path = self.path.clone();
        let home = self.home.clone();
        let backend = Arc::new(crate::script_backend::WorkerBackend::new(worker));
        let spawned = std::thread::Builder::new()
            .name("paneflow-serve-lua".into())
            .spawn(move || {
                let mut options = Options::new(Arc::new(LogSink));
                options.config_root = Some(home.clone());
                options.lua_dir = Some(paneflow_home::lua_dir_in(&home));
                options.serving = true;
                options.origin = "event".to_string();
                options.memory_limit = Some(MEMORY_LIMIT);
                let runtime = match Runtime::new(backend, options) {
                    Ok(runtime) => runtime,
                    Err(message) => {
                        let _ = started_tx.send(Started::Failed(failed_report(message)));
                        return;
                    }
                };
                let source = match std::fs::read(&path) {
                    Ok(source) => source,
                    Err(error) => {
                        let _ = started_tx.send(Started::Failed(failed_report(format!(
                            "{}: {error}",
                            paneflow_home::INIT_LUA_FILE_NAME
                        ))));
                        return;
                    }
                };
                let chunk = match runtime.compile(&source, "@init.lua") {
                    Ok(chunk) => chunk,
                    Err(message) => {
                        let _ = started_tx.send(Started::Failed(failed_report(message)));
                        return;
                    }
                };
                if let Err(message) = runtime.attach() {
                    let _ = started_tx.send(Started::Failed(failed_report(message)));
                    return;
                }
                let _ = started_tx.send(Started::Serving {
                    mailbox: runtime.mailbox(),
                    interrupter: runtime.interrupter(),
                    stats: runtime.stats(),
                });
                runtime.serve(chunk, move |report| {
                    let _ = report_tx.send(report);
                });
            });
        let thread = match spawned {
            Ok(thread) => thread,
            Err(error) => {
                return (
                    None,
                    failed_report(format!("no thread could be started for init.lua: {error}")),
                );
            }
        };
        let started = self.wait(&started_rx, &thread);
        let (mailbox, interrupter, stats) = match started {
            Some(Started::Serving {
                mailbox,
                interrupter,
                stats,
            }) => (mailbox, interrupter, stats),
            Some(Started::Failed(report)) => {
                self.retire_thread(thread);
                return (None, report);
            }
            None => {
                self.retire_thread(thread);
                return (
                    None,
                    failed_report("the init.lua runtime stopped before it loaded".to_string()),
                );
            }
        };
        let report = self.wait(&report_rx, &thread);
        let generation = Generation {
            mailbox,
            interrupter,
            stats,
            thread: Some(thread),
        };
        match report {
            Some(report) if report.ok => (Some(generation), report),
            Some(report) => {
                self.stop_generation(generation);
                (None, report)
            }
            None => {
                self.stop_generation(generation);
                (
                    None,
                    failed_report(if self.stopping.load(Ordering::Acquire) {
                        "the worker stopped while init.lua loaded".to_string()
                    } else {
                        "the init.lua runtime stopped before it loaded".to_string()
                    }),
                )
            }
        }
    }

    fn wait<T>(&self, receiver: &Receiver<T>, thread: &JoinHandle<()>) -> Option<T> {
        loop {
            match receiver.recv_timeout(LOAD_POLL) {
                Ok(value) => return Some(value),
                Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => {
                    if thread.is_finished() {
                        return receiver.try_recv().ok();
                    }
                    if self.stopping.load(Ordering::Acquire) {
                        return None;
                    }
                }
            }
        }
    }

    fn retire_thread(&self, thread: JoinHandle<()>) {
        self.lock().retiring.push(thread);
    }

    fn stop_generation(&self, mut generation: Generation) {
        generation.interrupter.interrupt();
        if let Some(thread) = generation.thread.take() {
            self.retire_thread(thread);
        }
    }

    pub fn reload(&self, worker: &Arc<Worker>) -> ConfigStatus {
        self.reload_as(worker, true)
    }

    fn reload_as(&self, worker: &Arc<Worker>, announce: bool) -> ConfigStatus {
        let _serial = self
            .reloading
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.stopping.load(Ordering::Acquire) {
            return self.status();
        }
        if !self.path.exists() {
            let had_config = {
                let mut active = self.lock();
                let had_config = active.loaded || active.status.found;
                if let Some(mut previous) = active.generation.take() {
                    previous.mailbox.retire();
                    if let Some(thread) = previous.thread.take() {
                        active.retiring.push(thread);
                    }
                }
                active.actions.clear();
                active.keymap = Keymap::default();
                active.required.clear();
                active.loaded = false;
                active.status = ConfigStatus {
                    path: self.path.display().to_string(),
                    found: false,
                    ok: true,
                    loaded_at_ms: Some(now_ms()),
                    ..ConfigStatus::default()
                };
                had_config
            };
            if announce || had_config {
                self.announce(worker);
            }
            return self.status();
        }
        let (generation, report) = self.start(worker);
        {
            let mut active = self.lock();
            match generation {
                Some(generation) => {
                    if let Some(mut previous) = active.generation.replace(generation) {
                        previous.mailbox.retire();
                        if let Some(thread) = previous.thread.take() {
                            active.retiring.push(thread);
                        }
                    }
                    active.status = ConfigStatus {
                        path: self.path.display().to_string(),
                        found: true,
                        loaded_at_ms: Some(now_ms()),
                        ok: true,
                        actions: report.actions.len(),
                        bindings: report.keymap.bindings.len(),
                        handlers: report.handlers,
                        timers: report.timers,
                        handler_errors: 0,
                        timer_errors: 0,
                        problems: report.problems.clone(),
                    };
                    active.actions = report.actions;
                    active.keymap = report.keymap;
                    active.required = report.required;
                    active.loaded = true;
                }
                None => {
                    for problem in &report.problems {
                        log::warn!(
                            "paneflow-serve: init.lua did not load: {}",
                            describe_problem(problem)
                        );
                    }
                    active.status.found = true;
                    active.status.ok = false;
                    active.status.problems = report.problems;
                    for required in report.required {
                        if !active.required.contains(&required) {
                            active.required.push(required);
                        }
                    }
                    if !active.loaded {
                        active.status.actions = 0;
                        active.status.bindings = 0;
                    }
                }
            }
            reap(&mut active.retiring);
        }
        self.signal(Signal::Rescan);
        self.announce(worker);
        self.status()
    }

    fn announce(&self, worker: &Worker) {
        let status = self.status();
        let errors = status
            .problems
            .iter()
            .filter(|problem| problem.severity == Severity::Error)
            .count();
        let warnings = status.problems.len() - errors;
        let mut data = Map::new();
        data.insert("ok".into(), json!(status.ok));
        data.insert("found".into(), json!(status.found));
        data.insert("path".into(), json!(status.path));
        data.insert("actions".into(), json!(status.actions));
        data.insert("bindings".into(), json!(status.bindings));
        data.insert("errors".into(), json!(errors));
        data.insert("warnings".into(), json!(warnings));
        if let Some(first) = status.problems.first() {
            data.insert(
                "problem".into(),
                serde_json::to_value(first).unwrap_or(Value::Null),
            );
        }
        worker
            .events
            .append(CONFIG_RELOADED, EventIds::default(), data);
    }

    pub fn agent_may_run(&self, name: &str) -> bool {
        self.lock()
            .actions
            .iter()
            .any(|action| action.name == name && action.agents)
    }

    pub fn no_action_named(name: &str) -> ServeError {
        ServeError::invalid_params(format!(
            "no action named '{name}'; paneflow actions lists the actions init.lua declares"
        ))
    }

    pub fn begin(&self, params: InvokeParams, done: ActionDone) -> Result<(), ServeError> {
        let mailbox = {
            let active = self.lock();
            if !active
                .actions
                .iter()
                .any(|action| action.name == params.name)
            {
                return Err(Self::no_action_named(&params.name));
            }
            active
                .generation
                .as_ref()
                .map(|generation| generation.mailbox.clone())
        };
        let Some(mailbox) = mailbox else {
            return Err(ServeError::invalid_params(format!(
                "no action named '{}'",
                params.name
            )));
        };
        mailbox.invoke(
            params.name,
            Value::Object(params.args.unwrap_or_default()),
            params.ctx.unwrap_or_default(),
            Box::new(move |result| {
                done(match result {
                    Ok(result) => Ok(Invoked {
                        result,
                        ..Invoked::default()
                    }),
                    Err(failure) => Err(action_error(failure)),
                });
            }),
        );
        Ok(())
    }

    pub fn invoke(&self, params: InvokeParams) -> Result<Invoked, ServeError> {
        let (reply_tx, reply_rx) = channel();
        self.begin(
            params,
            Box::new(move |result| {
                let _ = reply_tx.send(result);
            }),
        )?;
        reply_rx.recv().unwrap_or_else(|_| Err(runtime_stopped()))
    }

    pub fn start_watching(self: &Arc<Self>, worker: &Arc<Worker>) {
        let runtime = Arc::clone(self);
        let worker = Arc::downgrade(worker);
        let (sender, signals) = channel();
        *self.signals.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
        let spawned = std::thread::Builder::new()
            .name("paneflow-serve-lua-watch".into())
            .spawn(move || watch(&runtime, &worker, &signals));
        match spawned {
            Ok(handle) => {
                *self.watcher.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
            }
            Err(error) => log::warn!("paneflow-serve: cannot watch init.lua: {error}"),
        }
    }

    pub fn initial_load(&self, worker: &Arc<Worker>) {
        if self.path.exists() {
            self.reload_as(worker, true);
        }
    }

    fn signal(&self, signal: Signal) {
        if let Some(sender) = self
            .signals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            let _ = sender.send(signal);
        }
    }

    pub fn stop(&self, within: Duration) {
        self.stopping.store(true, Ordering::Release);
        self.signal(Signal::Stop);
        self.signals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let watcher = self
            .watcher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let mut threads = Vec::new();
        {
            let mut active = self.lock();
            if let Some(mut generation) = active.generation.take() {
                generation.interrupter.interrupt();
                generation.mailbox.retire();
                threads.extend(generation.thread.take());
            }
            threads.append(&mut active.retiring);
        }
        let deadline = Instant::now() + within;
        for thread in threads.into_iter().chain(watcher) {
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(JOIN_POLL);
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

fn reap(threads: &mut Vec<JoinHandle<()>>) {
    let mut kept = Vec::new();
    for thread in threads.drain(..) {
        if thread.is_finished() {
            let _ = thread.join();
        } else {
            kept.push(thread);
        }
    }
    *threads = kept;
}

pub fn describe_problem(problem: &ConfigProblem) -> String {
    match (&problem.file, problem.line) {
        (Some(file), Some(line)) => format!("{file}:{line}: {}", problem.message),
        (Some(file), None) => format!("{file}: {}", problem.message),
        _ => problem.message.clone(),
    }
}

pub type ActionDone = Box<dyn FnOnce(Result<Invoked, ServeError>) + Send>;

pub fn runtime_stopped() -> ServeError {
    ServeError {
        code: ERR_ACTION_FAILED,
        message: "the configuration runtime stopped before the action returned".to_string(),
        data: None,
    }
}

fn action_error(failure: ActionFailure) -> ServeError {
    ServeError {
        code: if failure.timeout {
            ERR_ACTION_TIMEOUT
        } else {
            ERR_ACTION_FAILED
        },
        data: Some(json!({"reason": if failure.timeout { "timeout" } else { "failed" }})),
        message: failure.message,
    }
}

fn watch_key(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    let parent = path.parent()?;
    let parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    Some(parent.join(name))
}

fn changes_content(kind: &notify::EventKind) -> bool {
    matches!(
        kind,
        notify::EventKind::Create(_) | notify::EventKind::Modify(_) | notify::EventKind::Remove(_)
    )
}

enum Signal {
    Changed(notify::Result<notify::Event>),
    Rescan,
    Stop,
}

fn watch(
    runtime: &Arc<ConfigRuntime>,
    worker: &std::sync::Weak<Worker>,
    signals: &Receiver<Signal>,
) {
    use notify::Watcher;
    let sender = runtime
        .signals
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let Some(sender) = sender else {
        return;
    };
    let mut watcher = match notify::recommended_watcher(move |event| {
        let _ = sender.send(Signal::Changed(event));
    }) {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            log::warn!("paneflow-serve: cannot watch init.lua: {error}");
            None
        }
    };
    let mut watched_dirs: Vec<PathBuf> = Vec::new();
    let mut watch_dirs_of = |files: &[PathBuf]| {
        let Some(watcher) = watcher.as_mut() else {
            return;
        };
        for file in files {
            let Some(dir) = file.parent() else {
                continue;
            };
            if dir.is_dir() && !watched_dirs.iter().any(|held| held == dir) {
                match watcher.watch(dir, notify::RecursiveMode::NonRecursive) {
                    Ok(()) => watched_dirs.push(dir.to_path_buf()),
                    Err(error) => {
                        log::debug!("paneflow-serve: cannot watch {}: {error}", dir.display())
                    }
                }
            }
        }
    };
    let before_load: Vec<PathBuf> = [
        runtime.path.clone(),
        paneflow_home::lua_dir_in(&runtime.home).join(paneflow_home::INIT_LUA_FILE_NAME),
    ]
    .iter()
    .filter_map(|path| watch_key(path))
    .collect();
    watch_dirs_of(&before_load);
    let settings_file = paneflow_home::config_path_in(&runtime.home);
    let settings_key = watch_key(&settings_file);
    let mut agent_prompts =
        paneflow_config::loader::load_config_from_path(&settings_file).agent_prompts_in_paneflow();
    let mut settings_pending: Option<Instant> = None;
    if let Some(worker) = worker.upgrade() {
        runtime.initial_load(&worker);
    }
    let mut pending: Option<Instant> = None;
    loop {
        let tracked: Vec<PathBuf> = runtime
            .watched_files()
            .iter()
            .filter_map(|path| watch_key(path))
            .collect();
        watch_dirs_of(&tracked);
        let wake = match (pending, settings_pending) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let signal = match wake {
            Some(at) => signals.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => signals.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match signal {
            Ok(Signal::Changed(Ok(event))) => {
                if changes_content(&event.kind) {
                    let keys: Vec<PathBuf> = event
                        .paths
                        .iter()
                        .filter_map(|path| watch_key(path))
                        .collect();
                    if keys.iter().any(|path| tracked.contains(path)) {
                        pending = Some(Instant::now() + RELOAD_SETTLE);
                    }
                    if settings_key
                        .as_ref()
                        .is_some_and(|settings| keys.contains(settings))
                    {
                        settings_pending = Some(Instant::now() + RELOAD_SETTLE);
                    }
                }
            }
            Ok(Signal::Changed(Err(error))) => {
                log::debug!("paneflow-serve: init.lua watcher error: {error}");
            }
            Ok(Signal::Rescan) => {}
            Ok(Signal::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if runtime.stopping.load(Ordering::Acquire) {
            return;
        }
        if settings_pending.is_some_and(|at| Instant::now() >= at) {
            settings_pending = None;
            let now = paneflow_config::loader::load_config_from_path(&settings_file)
                .agent_prompts_in_paneflow();
            if now != agent_prompts {
                agent_prompts = now;
                if let Some(worker) = worker.upgrade() {
                    worker.agent_prompts_changed();
                }
            }
        }
        if pending.is_some_and(|at| Instant::now() >= at) {
            pending = None;
            let Some(worker) = worker.upgrade() else {
                return;
            };
            runtime.reload(&worker);
        }
    }
}
