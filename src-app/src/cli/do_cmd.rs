use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use clap::Args;
use paneflow_api::Catalog;
use paneflow_script::{
    Backend, CallError, Discovery, EventFeed, FeedFrame, Level, Options, Outcome, PaneContext,
    Runtime, SessionIds, Sink,
};
use paneflow_serve::controller::{EventFrame, EventRequest, EventStream};
use serde_json::{Map, Value, json};

use super::router::{self, Endpoints};
use super::{CliError, EXIT_RUNTIME};

#[derive(Args, Debug)]
pub(super) struct DoArgs {
    #[arg(
        short = 'e',
        value_name = "CHUNK",
        help = "Run this Lua chunk instead of a file; every argument after it is a key=value pair"
    )]
    chunk: Option<String>,
    #[arg(
        value_name = "SCRIPT",
        help = "A Lua file (ends in .lua or contains a path separator), - for stdin, or an action name, then key=value pairs for paneflow.args"
    )]
    items: Vec<String>,
    #[arg(
        long,
        value_name = "JSON",
        help = "Raw JSON for paneflow.args, or @path to read it from a file; replaces the key=value pairs"
    )]
    args: Option<String>,
    #[arg(long, help = "Indent the JSON the script returns")]
    json: bool,
    #[arg(
        long,
        help = "Hear the events of every workspace, not only the one of the pane the script runs in"
    )]
    all: bool,
    #[arg(
        long = "for",
        value_name = "DURATION",
        help = "Stop after this long and exit 0, such as 30s, 10m or 2h"
    )]
    run_for: Option<String>,
    #[arg(
        long,
        help = "Exit 1 as soon as an event handler or a timer raises an error"
    )]
    fail_fast: bool,
    #[arg(
        long,
        value_name = "MIB",
        help = "Cap the memory of the Lua state at this many MiB; no cap when absent"
    )]
    memory_limit: Option<u64>,
    #[arg(
        long,
        value_name = "SECONDS",
        help = "Stop Lua code that runs this long without waiting; no deadline when absent"
    )]
    deadline: Option<f64>,
    #[arg(
        long,
        value_name = "NAME",
        help = "Load the script as a configuration such as init.lua and run this action from it, in this process; the worker's loaded configuration is untouched"
    )]
    action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Script {
    File(PathBuf),
    Inline(String),
    Stdin,
    Action(String),
}

pub(super) fn is_file_target(target: &str) -> bool {
    target.ends_with(".lua") || target.contains('/') || target.contains(std::path::MAIN_SEPARATOR)
}

pub(super) fn classify(
    chunk: Option<&str>,
    items: &[String],
) -> Result<(Script, Vec<String>), CliError> {
    if let Some(chunk) = chunk {
        return Ok((Script::Inline(chunk.to_string()), items.to_vec()));
    }
    let Some((target, pairs)) = items.split_first() else {
        return Err(CliError::runtime(
            "paneflow do needs a script file, -e '<chunk>', or - to read the script from stdin",
        ));
    };
    let script = if target == "-" {
        Script::Stdin
    } else if is_file_target(target) {
        Script::File(PathBuf::from(target))
    } else {
        Script::Action(target.clone())
    };
    Ok((script, pairs.to_vec()))
}

pub(super) fn typed_pair(raw: &str) -> Value {
    if let Ok(integer) = raw.parse::<i64>() {
        return Value::from(integer);
    }
    if let Some(number) = raw
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
        .and_then(serde_json::Number::from_f64)
        && raw.chars().any(|c| c.is_ascii_digit())
    {
        return Value::Number(number);
    }
    match raw {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => Value::String(raw.to_string()),
    }
}

pub(super) fn script_args(pairs: &[String], raw: Option<&str>) -> Result<Value, CliError> {
    if let Some(raw) = raw {
        if !pairs.is_empty() {
            return Err(CliError::runtime(
                "use either key=value pairs or --args, not both",
            ));
        }
        let text = match raw.strip_prefix('@') {
            Some(path) => std::fs::read_to_string(path)
                .map_err(|error| CliError::runtime(format!("cannot read {path}: {error}")))?,
            None => raw.to_string(),
        };
        return serde_json::from_str(&text)
            .map_err(|error| CliError::runtime(format!("--args is not valid JSON: {error}")));
    }
    let mut args = Map::new();
    for pair in pairs {
        let (key, value) = pair
            .split_once('=')
            .filter(|(key, _)| !key.is_empty())
            .ok_or_else(|| CliError::runtime(format!("'{pair}' is not a key=value pair")))?;
        args.insert(key.to_string(), typed_pair(value));
    }
    Ok(Value::Object(args))
}

pub(super) fn parse_duration(raw: &str) -> Result<Duration, CliError> {
    let raw = raw.trim();
    let split = raw
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(raw.len());
    let (number, unit) = raw.split_at(split);
    let value: f64 = number.parse().map_err(|_| {
        CliError::runtime(format!("'{raw}' is not a duration such as 30s, 10m or 2h"))
    })?;
    let seconds = match unit {
        "" | "s" => value,
        "ms" => value / 1000.0,
        "m" => value * 60.0,
        "h" => value * 3600.0,
        _ => {
            return Err(CliError::runtime(format!(
                "'{raw}' is not a duration such as 30s, 10m or 2h"
            )));
        }
    };
    Duration::try_from_secs_f64(seconds)
        .map_err(|_| CliError::runtime(format!("'{raw}' is not a duration such as 30s, 10m or 2h")))
}

struct StderrSink;

impl Sink for StderrSink {
    fn log(&self, level: Level, message: &str) {
        eprintln!("[{}] {message}", level.label());
    }

    fn report(&self, message: &str) {
        eprintln!("{message}");
    }
}

pub(super) fn error_code(message: &str) -> i64 {
    message
        .split("paneflow error ")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .and_then(|code| code.trim().parse().ok())
        .unwrap_or(CallError::UNAVAILABLE)
}

fn env_id(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
}

struct EventsFeed {
    stream: EventStream,
}

impl EventFeed for EventsFeed {
    fn next(&mut self, timeout: Duration) -> FeedFrame {
        match self.stream.next(timeout) {
            EventFrame::Following {
                restarted: true, ..
            } => FeedFrame::Resync,
            EventFrame::Following { .. } | EventFrame::Idle => FeedFrame::Idle,
            EventFrame::Event(event) => FeedFrame::Event(*event),
            EventFrame::Disconnected(reason) => FeedFrame::Disconnected(reason),
        }
    }
}

pub(crate) struct CliBackend {
    catalog: Catalog,
    endpoints: Endpoints,
    sessions: Mutex<HashMap<String, SessionIds>>,
}

impl CliBackend {
    pub(crate) fn new(catalog: Catalog, endpoints: Endpoints) -> Self {
        Self {
            catalog,
            endpoints,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn send(&self, method: &str, params: Value) -> Result<Value, CallError> {
        let endpoint = router::route(&self.catalog, &self.endpoints, method)
            .map_err(|error| CallError::new(error.message, CallError::METHOD_NOT_FOUND))
            .and_then(|endpoint| {
                router::send(&self.catalog, &endpoint, method, params).map_err(|error| {
                    let code = error_code(&error.message);
                    CallError::new(error.message, code)
                })
            });
        if let Err(error) = &endpoint
            && error.message.starts_with("Method not found")
        {
            return Err(CallError::new(
                error.message.clone(),
                CallError::METHOD_NOT_FOUND,
            ));
        }
        endpoint
    }

    fn context(&self, reachable: bool) -> PaneContext {
        let surface_id = env_id("PANEFLOW_SURFACE_ID");
        let workspace_id = env_id("PANEFLOW_WORKSPACE_ID");
        let mut context = PaneContext {
            surface_id,
            workspace_id,
            ..PaneContext::default()
        };
        if let (true, Some(surface)) = (reachable, surface_id) {
            let params = workspace_id.map_or_else(|| json!({}), |id| json!({"workspace_id": id}));
            if let Ok(listed) = self.send("surface.list", params)
                && let Some(row) = listed["surfaces"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|row| row["surface_id"].as_u64() == Some(surface))
            {
                context.tab_id = row["tab_id"].as_u64();
                context.workspace_id = context.workspace_id.or(row["workspace_id"].as_u64());
            }
        }
        context
    }

    fn learn(&self, surface_id: u64, tab_id: Option<u64>, workspace_id: Option<u64>) {
        if let Ok(status) = self.send("surface.status", json!({"surface_id": surface_id}))
            && let Some(session) = status["session"].as_str()
        {
            self.sessions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    session.to_string(),
                    SessionIds {
                        surface_id: Some(surface_id),
                        pane_id: None,
                        tab_id,
                        workspace_id,
                    },
                );
        }
    }

    fn refresh_sessions(&self) {
        let mut queries = vec![json!({})];
        if let Some(own) = env_id("PANEFLOW_WORKSPACE_ID") {
            queries.push(json!({"workspace_id": own}));
        }
        let mut seen = HashSet::new();
        for query in queries {
            let Ok(listed) = self.send("surface.list", query) else {
                continue;
            };
            for row in listed["surfaces"].as_array().into_iter().flatten() {
                let Some(surface) = row["surface_id"].as_u64() else {
                    continue;
                };
                if seen.insert(surface) {
                    self.learn(
                        surface,
                        row["tab_id"].as_u64(),
                        row["workspace_id"].as_u64(),
                    );
                }
            }
        }
    }

    fn learn_from_result(&self, method: &str, result: &Value) {
        let created: Vec<u64> = match method {
            "surface.split" => result["surface_id"].as_u64().into_iter().collect(),
            "workspace.up" => result["surface_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .collect(),
            _ => Vec::new(),
        };
        for surface in created {
            self.learn(surface, None, env_id("PANEFLOW_WORKSPACE_ID"));
        }
    }
}

impl Backend for CliBackend {
    fn discover(&self) -> Discovery {
        let linked = || {
            self.catalog
                .iter()
                .map(|descriptor| descriptor.describe_from(false))
                .collect::<Vec<_>>()
        };
        match super::api_cmd::list_rows(&self.catalog, &self.endpoints) {
            Ok(mut rows) => {
                let served: HashSet<String> = rows
                    .iter()
                    .filter_map(|row| row["name"].as_str().map(str::to_string))
                    .collect();
                rows.extend(linked().into_iter().filter(|row| {
                    row["name"]
                        .as_str()
                        .is_some_and(|name| !served.contains(name))
                }));
                Discovery {
                    methods: rows,
                    unavailable: None,
                    context: self.context(true),
                }
            }
            Err(error) => Discovery {
                methods: linked(),
                unavailable: Some(CallError::new(error.message, CallError::UNAVAILABLE)),
                context: self.context(false),
            },
        }
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, CallError> {
        let result = self.send(method, params)?;
        self.learn_from_result(method, &result);
        Ok(result)
    }

    fn follow(&self) -> Result<Box<dyn EventFeed>, CallError> {
        let endpoint = self.endpoints.worker.as_deref().ok_or_else(|| {
            CallError::new(
                "cannot resolve the Paneflow state home; set PANEFLOW_HOME",
                CallError::UNAVAILABLE,
            )
        })?;
        let request = EventRequest {
            types: Vec::new(),
            workspace_id: None,
            since: None,
            epoch: None,
        };
        EventStream::open(endpoint, request)
            .map(|stream| Box::new(EventsFeed { stream }) as Box<dyn EventFeed>)
            .map_err(|error| {
                CallError::new(
                    format!("the Paneflow worker does not answer: {error}"),
                    CallError::UNAVAILABLE,
                )
            })
    }

    fn window_attached(&self) -> bool {
        matches!(
            self.endpoints.control,
            Some(paneflow_ipc_client::host_control::ControlTarget::Controller(_))
        )
    }

    fn resolve_session(&self, session: &str) -> Option<SessionIds> {
        let cached = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session)
            .cloned();
        if cached.is_some() {
            return cached;
        }
        self.refresh_sessions();
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session)
            .cloned()
    }
}

fn read_script(script: &Script) -> Result<(Vec<u8>, String, Option<PathBuf>), CliError> {
    match script {
        Script::File(path) => {
            let source = std::fs::read(path)
                .map_err(|error| CliError::runtime(format!("{}: {error}", path.display())))?;
            let dir = match path.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
                _ => PathBuf::from("."),
            };
            Ok((source, format!("@{}", path.display()), Some(dir)))
        }
        Script::Inline(chunk) => Ok((
            chunk.clone().into_bytes(),
            "=(command line)".to_string(),
            std::env::current_dir().ok(),
        )),
        Script::Stdin => {
            let mut source = Vec::new();
            std::io::stdin()
                .read_to_end(&mut source)
                .map_err(|error| CliError::runtime(format!("cannot read stdin: {error}")))?;
            Ok((source, "=stdin".to_string(), std::env::current_dir().ok()))
        }
        Script::Action(name) => Err(CliError::runtime(format!(
            "'{name}' is an action name, which the worker runs; give a script path ending in .lua, -e '<chunk>' or - for stdin"
        ))),
    }
}

fn options(
    args: &DoArgs,
    script_dir: Option<PathBuf>,
    script_args: Value,
) -> Result<Options, CliError> {
    let mut options = Options::new(Arc::new(StderrSink));
    options.script_dir = script_dir;
    options.lua_dir = paneflow_script::stdlib::lua_dir();
    options.args = script_args;
    options.all_workspaces = args.all;
    options.fail_fast = args.fail_fast;
    options.run_for = args.run_for.as_deref().map(parse_duration).transpose()?;
    options.memory_limit = args
        .memory_limit
        .map(|mib| usize::try_from(mib.saturating_mul(1024 * 1024)).unwrap_or(usize::MAX));
    options.deadline = args
        .deadline
        .map(|seconds| {
            Duration::try_from_secs_f64(seconds).map_err(|_| {
                CliError::runtime(format!("--deadline {seconds} is not a number of seconds"))
            })
        })
        .transpose()?;
    Ok(options)
}

pub(super) fn with_full_path(message: &str, path: Option<&str>) -> String {
    let Some(path) = path else {
        return message.to_string();
    };
    let Some(shortened) = message.strip_prefix("...") else {
        return message.to_string();
    };
    let Some(colon) = shortened.find(':') else {
        return message.to_string();
    };
    let (tail, rest) = shortened.split_at(colon);
    if path.ends_with(tail) {
        format!("{path}{rest}")
    } else {
        message.to_string()
    }
}

pub(super) fn script_label(chunk_name: &str) -> String {
    match chunk_name.strip_prefix('@') {
        Some(path) => std::path::Path::new(path).file_name().map_or_else(
            || path.to_string(),
            |name| name.to_string_lossy().to_string(),
        ),
        None => chunk_name
            .trim_start_matches('=')
            .trim_start_matches('(')
            .trim_end_matches(')')
            .to_string(),
    }
}

fn report_error(message: &str) -> i32 {
    eprintln!("Error: {message}");
    EXIT_RUNTIME
}

fn invoke_context() -> paneflow_api::methods::config::InvokeContext {
    paneflow_api::methods::config::InvokeContext {
        origin: Some("cli".to_string()),
        surface_id: env_id("PANEFLOW_SURFACE_ID"),
        workspace_id: env_id("PANEFLOW_WORKSPACE_ID"),
        ..Default::default()
    }
}

fn print_result(value: Option<&Value>, indent: bool) {
    if let Some(value) = value {
        let rendered = if indent {
            serde_json::to_string_pretty(value)
        } else {
            serde_json::to_string(value)
        };
        println!("{}", rendered.unwrap_or_default());
    }
}

pub(super) fn invoke_on_worker(
    name: &str,
    args: Value,
    context: paneflow_api::methods::config::InvokeContext,
) -> Result<Option<Value>, CliError> {
    let catalog = paneflow_api::catalog().map_err(|error| CliError::runtime(error.to_string()))?;
    let endpoints = Endpoints::from_environment();
    let endpoint = router::route(&catalog, &endpoints, "action.invoke")?;
    let params = json!({
        "name": name,
        "args": args,
        "ctx": serde_json::to_value(context).unwrap_or_else(|_| json!({})),
    });
    let reply = router::send(&catalog, &endpoint, "action.invoke", params)?;
    Ok(reply
        .get("result")
        .filter(|value| !value.is_null())
        .cloned())
}

fn run_worker_action(name: &str, pairs: &[String], args: &DoArgs) -> i32 {
    let values = match script_args(pairs, args.args.as_deref()) {
        Ok(values) => values,
        Err(error) => return report_error(&error.message),
    };
    match invoke_on_worker(name, values, invoke_context()) {
        Ok(value) => {
            print_result(value.as_ref(), args.json);
            super::EXIT_OK
        }
        Err(error) => report_error(&error.message),
    }
}

fn run_local_action(action: &str, path: &std::path::Path, pairs: &[String], args: &DoArgs) -> i32 {
    let values = match script_args(pairs, args.args.as_deref()) {
        Ok(values) => values,
        Err(error) => return report_error(&error.message),
    };
    let source = match std::fs::read(path) {
        Ok(source) => source,
        Err(error) => return report_error(&format!("{}: {error}", path.display())),
    };
    let root = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut options = match options(args, Some(root.clone()), json!({})) {
        Ok(options) => options,
        Err(error) => return report_error(&error.message),
    };
    options.config_root = Some(root.clone());
    options.lua_dir = Some(root.join(paneflow_home::LUA_DIR_NAME));
    options.script_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string());
    let catalog = match paneflow_api::catalog() {
        Ok(catalog) => catalog,
        Err(error) => return report_error(&error.to_string()),
    };
    let backend = Arc::new(CliBackend::new(catalog, Endpoints::from_environment()));
    let runtime = match Runtime::new(backend, options) {
        Ok(runtime) => runtime,
        Err(message) => return report_error(&message),
    };
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().to_string(),
    );
    let chunk = match runtime.compile(&source, &format!("@{name}")) {
        Ok(chunk) => chunk,
        Err(message) => return report_error(&message),
    };
    let interrupter = runtime.interrupter();
    let _ = ctrlc::set_handler(move || interrupter.interrupt());
    if let Err(message) = runtime.attach() {
        return report_error(&message);
    }
    let report = runtime.load(chunk);
    for problem in &report.problems {
        eprintln!("{}", super::config_cmd::problem_line(problem));
    }
    if !report.ok {
        return EXIT_RUNTIME;
    }
    let outcome = runtime.run_action(action.to_string(), values, invoke_context());
    print_outcome(&outcome, args.json);
    outcome.status()
}

pub(super) fn run(args: DoArgs) -> i32 {
    match (
        classify(args.chunk.as_deref(), &args.items),
        args.action.as_deref(),
    ) {
        (Ok((Script::Action(name), pairs)), None) => {
            return run_worker_action(&name, &pairs, &args);
        }
        (Ok((Script::File(path), pairs)), Some(action)) => {
            return run_local_action(action, &path, &pairs, &args);
        }
        (Ok(_), Some(_)) => {
            return report_error(
                "--action needs a configuration file such as ~/.paneflow/init.lua",
            );
        }
        _ => {}
    }
    let prepared = classify(args.chunk.as_deref(), &args.items).and_then(|(script, pairs)| {
        let values = script_args(&pairs, args.args.as_deref())?;
        let (source, name, dir) = read_script(&script)?;
        Ok((source, name, dir, values))
    });
    let script_path = match classify(args.chunk.as_deref(), &args.items) {
        Ok((Script::File(path), _)) => Some(path.display().to_string()),
        _ => None,
    };
    let (source, name, dir, values) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return report_error(&error.message),
    };
    let mut options = match options(&args, dir, values) {
        Ok(options) => options,
        Err(error) => return report_error(&error.message),
    };
    options.script_name = Some(script_label(&name));
    let catalog = match paneflow_api::catalog() {
        Ok(catalog) => catalog,
        Err(error) => return report_error(&error.to_string()),
    };
    let backend = Arc::new(CliBackend::new(catalog, Endpoints::from_environment()));
    let runtime = match Runtime::new(backend, options) {
        Ok(runtime) => runtime,
        Err(message) => return report_error(&message),
    };
    let chunk = match runtime.compile(&source, &name) {
        Ok(chunk) => chunk,
        Err(message) => return report_error(&with_full_path(&message, script_path.as_deref())),
    };
    let interrupter = runtime.interrupter();
    let _ = ctrlc::set_handler(move || interrupter.interrupt());
    if let Err(message) = runtime.attach() {
        return report_error(&message);
    }
    let outcome = match runtime.run(chunk) {
        Outcome::Failed(message) => {
            Outcome::Failed(with_full_path(&message, script_path.as_deref()))
        }
        other => other,
    };
    print_outcome(&outcome, args.json);
    outcome.status()
}

fn print_outcome(outcome: &Outcome, indent: bool) {
    match outcome {
        Outcome::Returned(Some(value)) => {
            let rendered = if indent {
                serde_json::to_string_pretty(value)
            } else {
                serde_json::to_string(value)
            };
            println!("{}", rendered.unwrap_or_default());
        }
        Outcome::Returned(None) | Outcome::Stopped(_) => {}
        Outcome::Failed(message) => {
            eprintln!("Error: {message}");
        }
        Outcome::Interrupted => eprintln!("Error: interrupted"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_names_a_file_script_by_its_file_name_and_the_others_by_their_source() {
        assert_eq!(script_label("@/home/me/scripts/deploy.lua"), "deploy.lua");
        assert_eq!(script_label("=stdin"), "stdin");
        assert_eq!(script_label("=(command line)"), "command line");
    }

    #[test]
    fn a_path_like_argument_is_a_file_and_anything_else_an_action() {
        let items = |list: &[&str]| {
            list.iter()
                .map(|item| (*item).to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            classify(None, &items(&["./deploy.lua", "n=2"])).unwrap(),
            (Script::File("./deploy.lua".into()), items(&["n=2"]))
        );
        assert_eq!(
            classify(None, &items(&["scripts/deploy"])).unwrap().0,
            Script::File("scripts/deploy".into())
        );
        assert_eq!(
            classify(None, &items(&["deploy.lua"])).unwrap().0,
            Script::File("deploy.lua".into())
        );
        assert_eq!(
            classify(None, &items(&["deploy"])).unwrap().0,
            Script::Action("deploy".into())
        );
        assert_eq!(classify(None, &items(&["-"])).unwrap().0, Script::Stdin);
        assert_eq!(
            classify(Some("return 1"), &items(&["n=2"])).unwrap(),
            (Script::Inline("return 1".into()), items(&["n=2"]))
        );
        assert!(classify(None, &[]).is_err());
        let refused = read_script(&Script::Action("deploy".into())).unwrap_err();
        assert!(
            refused.message.starts_with("'deploy' is an action name"),
            "{}",
            refused.message
        );
    }

    #[test]
    fn pairs_are_typed_by_their_spelling_and_args_replace_them() {
        let pairs = [
            "n=2",
            "ratio=0.5",
            "on=true",
            "off=false",
            "name=x",
            "id=007x",
            "empty=",
        ]
        .map(str::to_string);
        assert_eq!(
            script_args(&pairs, None).unwrap(),
            json!({"n": 2, "ratio": 0.5, "on": true, "off": false, "name": "x", "id": "007x", "empty": ""})
        );
        assert_eq!(
            script_args(&[], Some(r#"{"list": [1, 2]}"#)).unwrap(),
            json!({"list": [1, 2]})
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("args.json");
        std::fs::write(&file, r#"{"from": "file"}"#).unwrap();
        assert_eq!(
            script_args(&[], Some(&format!("@{}", file.display()))).unwrap(),
            json!({"from": "file"})
        );
        assert!(script_args(&["n=2".to_string()], Some("{}")).is_err());
        assert!(script_args(&["oops".to_string()], None).is_err());
    }

    #[test]
    fn durations_take_a_unit_and_default_to_seconds() {
        assert_eq!(parse_duration("10m").unwrap(), Duration::from_secs(600));
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("1.5").unwrap(), Duration::from_millis(1500));
        assert!(parse_duration("soon").is_err());
        assert!(parse_duration("5d").is_err());
    }

    #[test]
    fn a_path_lua_shortened_is_printed_in_full() {
        let path = "/home/someone/a/very/long/directory/that/lua/shortens/in/messages/script.lua";
        assert_eq!(
            with_full_path(
                "...ory/that/lua/shortens/in/messages/script.lua:2: unexpected symbol",
                Some(path)
            ),
            format!("{path}:2: unexpected symbol")
        );
        assert_eq!(
            with_full_path("short.lua:1: x", Some("short.lua")),
            "short.lua:1: x"
        );
        assert_eq!(with_full_path("...x:1: y", None), "...x:1: y");
    }

    #[test]
    fn a_wire_error_keeps_its_numeric_code() {
        assert_eq!(
            error_code("paneflow error -32004: Surface not found: 9"),
            -32004
        );
        assert_eq!(
            error_code("surface.read is served by the Paneflow window, which is not reachable: x"),
            CallError::UNAVAILABLE
        );
    }

    #[test]
    fn a_script_without_limit_flags_runs_without_a_memory_cap_or_a_deadline() {
        let args = DoArgs {
            chunk: Some("return 1".into()),
            items: Vec::new(),
            args: None,
            json: false,
            all: false,
            run_for: None,
            fail_fast: false,
            memory_limit: None,
            deadline: None,
            action: None,
        };
        let options = options(&args, None, json!({})).unwrap();
        assert_eq!(options.memory_limit, None);
        assert_eq!(options.deadline, None);
        assert_eq!(options.run_for, None);
    }
}
