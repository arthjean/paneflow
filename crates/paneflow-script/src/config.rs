use std::cell::RefCell;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use mlua::{Function, Lua, LuaSerdeExt, MultiValue, Table, Value as LuaValue};
use serde_json::{Map, Value, json};

pub use paneflow_api::methods::config::{
    ActionSummary, BindingTarget, ConfigProblem, InvokeContext, KeyBindingRow, Keymap, Severity,
    UnboundKey,
};

use crate::error::{self, located};
use crate::executor::{TaskId, TaskKind};
use crate::json::{to_json, to_lua};
use crate::runtime::{PaneContext, Shared, shared};

pub const MAX_INVOKE_DEPTH: usize = 16;
pub const ACTION_TIME_LIMIT: Duration = Duration::from_secs(30);
pub const DEFAULT_CATEGORY: &str = "Actions";
pub const ARGUMENT_TYPES: &[&str] = &[
    "string", "number", "integer", "boolean", "object", "array", "any",
];

#[derive(Debug, Default)]
pub struct Stats {
    pub handler_errors: AtomicU64,
    pub timer_errors: AtomicU64,
    pub timers: AtomicUsize,
    pub handlers: AtomicUsize,
}

impl Stats {
    pub fn handler_errors(&self) -> u64 {
        self.handler_errors.load(Ordering::Acquire)
    }

    pub fn timer_errors(&self) -> u64 {
        self.timer_errors.load(Ordering::Acquire)
    }

    pub fn timers(&self) -> usize {
        self.timers.load(Ordering::Acquire)
    }

    pub fn handlers(&self) -> usize {
        self.handlers.load(Ordering::Acquire)
    }
}

pub(crate) struct ActionDef {
    pub summary: ActionSummary,
    pub labels: Vec<String>,
    pub run: Function,
}

struct BindingDef {
    row: KeyBindingRow,
}

#[derive(Default)]
pub(crate) struct Config {
    actions: RefCell<Vec<Rc<ActionDef>>>,
    bindings: RefCell<Vec<BindingDef>>,
    unbound: RefCell<Vec<UnboundKey>>,
    problems: RefCell<Vec<ConfigProblem>>,
    keymap: RefCell<Keymap>,
}

impl Config {
    pub fn action(&self, name: &str) -> Option<Rc<ActionDef>> {
        self.actions
            .borrow()
            .iter()
            .find(|action| action.summary.name == name)
            .cloned()
    }

    pub fn summaries(&self) -> Vec<ActionSummary> {
        self.actions
            .borrow()
            .iter()
            .map(|action| action.summary.clone())
            .collect()
    }

    pub fn problems(&self) -> Vec<ConfigProblem> {
        self.problems.borrow().clone()
    }

    pub fn keymap(&self) -> Keymap {
        self.keymap.borrow().clone()
    }

    fn report(&self, severity: Severity, location: &str, message: impl Into<String>) {
        let (file, line) = split_location(location);
        self.problems.borrow_mut().push(ConfigProblem {
            file,
            line,
            severity,
            message: message.into(),
        });
    }

    pub fn finalize(&self) {
        let mut rows = Vec::new();
        for binding in self.bindings.borrow().iter() {
            let mut row = binding.row.clone();
            match self.action(&row.action) {
                Some(action) => {
                    row.target = BindingTarget::Lua;
                    let args = Value::Object(row.args.clone().unwrap_or_default());
                    if let Err(message) = check_arguments(&action, &args) {
                        self.report(
                            Severity::Warning,
                            &row.source,
                            format!(
                                "the binding of {} to {} is left out: {message}",
                                row.key, row.action
                            ),
                        );
                        continue;
                    }
                }
                None => row.target = BindingTarget::Builtin,
            }
            rows.push(row);
        }
        *self.keymap.borrow_mut() = Keymap {
            bindings: rows,
            unbound: self.unbound.borrow().clone(),
        };
    }
}

pub fn split_location(location: &str) -> (Option<String>, Option<u32>) {
    let trimmed = location.trim().trim_end_matches(':');
    let Some((file, line)) = trimmed.rsplit_once(':') else {
        return (None, None);
    };
    match line.parse::<u32>() {
        Ok(line) if !file.is_empty() => (Some(file.to_string()), Some(line)),
        _ => (None, None),
    }
}

pub fn problem_from_message(severity: Severity, message: &str) -> ConfigProblem {
    if crate::guard::has_location(message) {
        let mut parts = message.splitn(3, ':');
        let file = parts.next().map(str::to_string);
        let line = parts.next().and_then(|line| line.parse().ok());
        let rest = parts.next().unwrap_or_default().trim().to_string();
        return ConfigProblem {
            file,
            line,
            severity,
            message: rest,
        };
    }
    ConfigProblem {
        file: None,
        line: None,
        severity,
        message: message.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionLabel {
    pub name: String,
    pub title: String,
}

impl ActionLabel {
    fn of(action: &ActionDef) -> Self {
        Self {
            name: action.summary.name.clone(),
            title: action.summary.title.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TaskScope {
    pub ids: PaneContext,
    pub action: Option<ActionLabel>,
    pub origin: String,
    pub client_id: Option<String>,
    pub grant: Option<String>,
    pub deadline: Option<Duration>,
    pub depth: usize,
    pub queue: Vec<String>,
    pub dispatches_at_once: bool,
    pub agent: Option<String>,
}

fn open_caller_scope(shared: &Shared, task: TaskId) -> bool {
    let mut tasks = shared.tasks.borrow_mut();
    if tasks.contains_key(&task) {
        return false;
    }
    let origin = match shared.executor.current().map(|(_, kind)| kind) {
        Some(TaskKind::Timer) => "timer",
        Some(TaskKind::Handler) => "event",
        _ => shared.origin.as_str(),
    };
    tasks.insert(
        task,
        TaskScope {
            ids: shared.context.borrow().clone(),
            origin: origin.to_string(),
            dispatches_at_once: true,
            ..TaskScope::default()
        },
    );
    true
}

pub(crate) fn current_task(shared: &Shared) -> Option<TaskId> {
    shared.executor.current().map(|(id, _)| id)
}

pub(crate) fn current_ids(shared: &Shared) -> Option<PaneContext> {
    let task = current_task(shared)?;
    shared
        .tasks
        .borrow()
        .get(&task)
        .map(|scope| scope.ids.clone())
}

pub(crate) fn current_grant(shared: &Shared) -> Option<String> {
    let task = current_task(shared)?;
    shared
        .tasks
        .borrow()
        .get(&task)
        .and_then(|scope| scope.grant.clone())
}

pub(crate) fn current_deadline(shared: &Shared) -> Option<Duration> {
    let task = current_task(shared)?;
    shared
        .tasks
        .borrow()
        .get(&task)
        .and_then(|scope| scope.deadline)
}

pub(crate) fn at_load(shared: &Shared) -> bool {
    matches!(shared.executor.current(), Some((_, TaskKind::TopLevel)))
}

fn loaded_error(lua: &Lua, what: &str) -> mlua::Error {
    located(
        lua,
        format!(
            "{what} can only be called while init.lua loads, not from an action, a handler or a timer"
        ),
    )
}

fn source_of(lua: &Lua) -> String {
    error::location(lua)
        .trim_end()
        .trim_end_matches(':')
        .to_string()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn string_field(table: &Table, key: &str) -> Result<Option<String>, String> {
    match table.get::<LuaValue>(key) {
        Ok(LuaValue::Nil) => Ok(None),
        Ok(LuaValue::String(text)) => Ok(Some(text.to_string_lossy())),
        Ok(other) => Err(format!(
            "{key} must be a string, not a {}",
            other.type_name()
        )),
        Err(error) => Err(error::describe(&error)),
    }
}

struct ArgumentSpec {
    schema: Value,
    labels: Vec<String>,
}

fn type_schema(kind: &str) -> Option<Value> {
    match kind {
        "any" => Some(json!({})),
        "string" | "number" | "integer" | "boolean" | "object" | "array" => {
            Some(json!({"type": kind}))
        }
        _ => None,
    }
}

fn argument_spec(lua: &Lua, value: LuaValue) -> Result<ArgumentSpec, String> {
    let table = match value {
        LuaValue::Nil => {
            return Ok(ArgumentSpec {
                schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                labels: Vec::new(),
            });
        }
        LuaValue::Table(table) => table,
        other => {
            return Err(format!(
                "args must be a table of argument names, not a {}",
                other.type_name()
            ));
        }
    };
    let mut entries: Vec<(String, LuaValue)> = Vec::new();
    for pair in table.pairs::<LuaValue, LuaValue>() {
        let (key, value) = pair.map_err(|error| error::describe(&error))?;
        let LuaValue::String(key) = key else {
            return Err(format!(
                "args must be keyed by argument names, not by a {}",
                key.type_name()
            ));
        };
        entries.push((key.to_string_lossy(), value));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut properties = Map::new();
    let mut required = Vec::new();
    let mut labels = Vec::new();
    for (key, value) in entries {
        let (name, mut optional) = match key.strip_suffix('?') {
            Some(name) => (name.to_string(), true),
            None => (key.clone(), false),
        };
        if name.is_empty() {
            return Err("an argument needs a name".to_string());
        }
        let schema = match &value {
            LuaValue::String(kind) => {
                let kind = kind.to_string_lossy();
                let (kind, marked) = match kind.strip_suffix('?') {
                    Some(kind) => (kind.to_string(), true),
                    None => (kind, false),
                };
                optional |= marked;
                type_schema(&kind).ok_or_else(|| {
                    format!(
                        "argument {name}: '{kind}' is not a type; use one of {}, with a trailing ? for an optional argument, or a table with a type key",
                        ARGUMENT_TYPES.join(", ")
                    )
                })?
            }
            LuaValue::Table(schema)
                if !schema
                    .get::<LuaValue>("type")
                    .is_ok_and(|kind| kind.is_nil()) =>
            {
                to_json(lua, &value).map_err(|message| format!("argument {name}: {message}"))?
            }
            other => {
                return Err(format!(
                    "argument {name}: give a type name such as \"string\" or a JSON Schema table with a type key, not a {}",
                    other.type_name()
                ));
            }
        };
        let unsupported = paneflow_api::schema::unsupported_keywords(&schema);
        if !unsupported.is_empty() {
            return Err(format!(
                "argument {name}: the schema uses keywords Paneflow does not check: {}",
                unsupported.join(", ")
            ));
        }
        labels.push(if optional {
            format!("{name}?")
        } else {
            name.clone()
        });
        if !optional {
            required.push(Value::String(name.clone()));
        }
        properties.insert(name, schema);
    }
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }
    schema.insert("additionalProperties".into(), json!(false));
    Ok(ArgumentSpec {
        schema: Value::Object(schema),
        labels,
    })
}

fn takes(labels: &[String]) -> String {
    if labels.is_empty() {
        "the action takes no arguments".to_string()
    } else {
        format!("the action takes {}", labels.join(", "))
    }
}

pub(crate) fn check_arguments(action: &ActionDef, args: &Value) -> Result<(), String> {
    let Value::Object(given) = args else {
        return Err(format!(
            "the arguments must be an object of named fields; {}",
            takes(&action.labels)
        ));
    };
    let schema = &action.summary.args;
    let properties = schema["properties"].as_object();
    if let Some(unknown) = given
        .keys()
        .find(|key| !properties.is_some_and(|known| known.contains_key(*key)))
    {
        return Err(format!(
            "no argument named \"{unknown}\"; {}",
            takes(&action.labels)
        ));
    }
    if let Some(missing) = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|name| !given.contains_key(*name))
    {
        return Err(format!(
            "missing argument \"{missing}\"; {}",
            takes(&action.labels)
        ));
    }
    paneflow_api::schema::validate(schema, args).map_err(|violation| violation.message)
}

fn declare(lua: &Lua, spec: LuaValue) -> mlua::Result<LuaValue> {
    let shared = shared(lua)?;
    if !at_load(&shared) {
        return Err(loaded_error(lua, "paneflow.action"));
    }
    let source = source_of(lua);
    let LuaValue::Table(spec) = spec else {
        shared.config.report(
            Severity::Error,
            &source,
            format!(
                "paneflow.action takes a table such as {{ name = \"dev_layout\", title = \"Dev layout\", run = function(ctx, args) end }}, not a {}",
                spec.type_name()
            ),
        );
        return Ok(LuaValue::Nil);
    };
    match parse_action(lua, &spec, &source) {
        Ok(action) => {
            let name = action.summary.name.clone();
            let mut actions = shared.config.actions.borrow_mut();
            if let Some(previous) = actions.iter().position(|held| held.summary.name == name) {
                let earlier = actions.remove(previous).summary.source.clone();
                drop(actions);
                shared.config.report(
                    Severity::Warning,
                    &source,
                    format!(
                        "the action {name} is declared again; this declaration replaces the one at {earlier}"
                    ),
                );
                actions = shared.config.actions.borrow_mut();
            }
            actions.push(Rc::new(action));
            Ok(LuaValue::Boolean(true))
        }
        Err(message) => {
            shared.config.report(Severity::Error, &source, message);
            Ok(LuaValue::Nil)
        }
    }
}

fn parse_action(lua: &Lua, spec: &Table, source: &str) -> Result<ActionDef, String> {
    let name = string_field(spec, "name")
        .map_err(|message| format!("paneflow.action: {message}"))?
        .ok_or("paneflow.action: name is required")?;
    if !valid_name(&name) {
        return Err(format!(
            "paneflow.action: '{name}' is not an action name; use lowercase letters, digits and underscores, without dots"
        ));
    }
    let field = |key: &str| {
        string_field(spec, key).map_err(|message| format!("paneflow.action {name}: {message}"))
    };
    let title = field("title")?.ok_or(format!("paneflow.action {name}: title is required"))?;
    let description = field("description")?;
    let category = field("category")?.unwrap_or_else(|| DEFAULT_CATEGORY.to_string());
    let run = match spec.get::<LuaValue>("run") {
        Ok(LuaValue::Function(run)) => run,
        Ok(LuaValue::Nil) => return Err(format!("paneflow.action {name}: run is required")),
        Ok(other) => {
            return Err(format!(
                "paneflow.action {name}: run must be a function, not a {}",
                other.type_name()
            ));
        }
        Err(error) => return Err(error::describe(&error)),
    };
    let keywords = match spec.get::<LuaValue>("keywords") {
        Ok(LuaValue::Nil) => Vec::new(),
        Ok(LuaValue::Table(list)) => {
            let mut keywords = Vec::new();
            for item in list.sequence_values::<LuaValue>() {
                match item {
                    Ok(LuaValue::String(word)) => keywords.push(word.to_string_lossy()),
                    _ => {
                        return Err(format!(
                            "paneflow.action {name}: keywords must be a list of strings"
                        ));
                    }
                }
            }
            keywords
        }
        _ => {
            return Err(format!(
                "paneflow.action {name}: keywords must be a list of strings"
            ));
        }
    };
    let agents = match spec.get::<LuaValue>("agents") {
        Ok(LuaValue::Nil) => false,
        Ok(LuaValue::Boolean(flag)) => flag,
        _ => {
            return Err(format!(
                "paneflow.action {name}: agents must be true or false"
            ));
        }
    };
    let arguments = spec
        .get::<LuaValue>("args")
        .map_err(|error| error::describe(&error))
        .and_then(|args| argument_spec(lua, args))
        .map_err(|message| format!("paneflow.action {name}: {message}"))?;
    Ok(ActionDef {
        summary: ActionSummary {
            name,
            title,
            description,
            category,
            keywords,
            args: arguments.schema,
            agents,
            source: source.to_string(),
        },
        labels: arguments.labels,
        run,
    })
}

fn bind(lua: &Lua, (key, target, args): (LuaValue, LuaValue, LuaValue)) -> mlua::Result<LuaValue> {
    let shared = shared(lua)?;
    if !at_load(&shared) {
        return Err(loaded_error(lua, "paneflow.bind"));
    }
    let source = source_of(lua);
    let refuse = |message: String| {
        shared.config.report(Severity::Error, &source, message);
        Ok(LuaValue::Nil)
    };
    let key = match key {
        LuaValue::String(key) if !key.to_string_lossy().trim().is_empty() => {
            key.to_string_lossy().trim().to_string()
        }
        other => {
            return refuse(format!(
                "paneflow.bind: the key must be a string such as \"ctrl-alt-d\", not a {}",
                other.type_name()
            ));
        }
    };
    let action = match target {
        LuaValue::String(name) => name.to_string_lossy(),
        LuaValue::Function(_) => {
            return refuse(format!(
                "paneflow.bind({key}, function): a key binds an action name, not a function; define the function as an action with paneflow.action{{ name = ..., title = ..., run = ... }} and bind its name"
            ));
        }
        other => {
            return refuse(format!(
                "paneflow.bind({key}): the second argument must be an action name, not a {}",
                other.type_name()
            ));
        }
    };
    let args = match args {
        LuaValue::Nil => None,
        LuaValue::Table(_) => match to_json(lua, &args) {
            Ok(Value::Object(map)) => Some(map),
            Ok(Value::Array(items)) if items.is_empty() => Some(Map::new()),
            Ok(_) => {
                return refuse(format!(
                    "paneflow.bind({key}): the arguments must be a table of named fields"
                ));
            }
            Err(message) => return refuse(format!("paneflow.bind({key}): {message}")),
        },
        other => {
            return refuse(format!(
                "paneflow.bind({key}): the arguments must be a table, not a {}",
                other.type_name()
            ));
        }
    };
    let normalized = normalize_key(&key);
    let mut bindings = shared.config.bindings.borrow_mut();
    bindings.retain(|binding| normalize_key(&binding.row.key) != normalized);
    bindings.push(BindingDef {
        row: KeyBindingRow {
            key,
            action,
            args,
            source,
            target: BindingTarget::Builtin,
        },
    });
    Ok(LuaValue::Boolean(true))
}

pub fn normalize_key(key: &str) -> String {
    key.trim().replace('+', "-").to_lowercase()
}

fn unbind(lua: &Lua, key: String) -> mlua::Result<bool> {
    let shared = shared(lua)?;
    if !at_load(&shared) {
        return Err(loaded_error(lua, "paneflow.unbind"));
    }
    let source = source_of(lua);
    let normalized = normalize_key(&key);
    shared
        .config
        .bindings
        .borrow_mut()
        .retain(|binding| normalize_key(&binding.row.key) != normalized);
    let mut unbound = shared.config.unbound.borrow_mut();
    unbound.retain(|held| normalize_key(&held.key) != normalized);
    unbound.push(UnboundKey {
        key: key.trim().to_string(),
        source,
    });
    Ok(true)
}

fn bindings(lua: &Lua, (): ()) -> mlua::Result<Table> {
    let shared = shared(lua)?;
    let list = lua.create_table()?;
    for (index, binding) in shared.config.bindings.borrow().iter().enumerate() {
        let row = lua.create_table()?;
        row.set("key", binding.row.key.as_str())?;
        row.set("action", binding.row.action.as_str())?;
        if let Some(args) = &binding.row.args {
            row.set("args", to_lua(lua, &Value::Object(args.clone()))?)?;
        }
        row.set("source", binding.row.source.as_str())?;
        list.raw_set(index + 1, row)?;
    }
    list.set_metatable(Some(lua.array_metatable()))?;
    Ok(list)
}

pub(crate) fn scope_table(lua: &Lua, scope: &TaskScope) -> mlua::Result<Table> {
    let table = crate::runtime::context_table(lua, &scope.ids, &scope.origin)?;
    table.set("client_id", scope.client_id.as_deref())?;
    Ok(table)
}

fn arguments_of(lua: &Lua, args: &LuaValue) -> Result<Value, String> {
    match args {
        LuaValue::Nil => Ok(Value::Object(Map::new())),
        LuaValue::Table(_) => match to_json(lua, args)? {
            Value::Array(items) if items.is_empty() => Ok(Value::Object(Map::new())),
            value => Ok(value),
        },
        other => Err(format!(
            "the arguments must be a table, not a {}",
            other.type_name()
        )),
    }
}

pub(crate) async fn call_action(
    lua: &Lua,
    shared: &Rc<Shared>,
    action: &ActionDef,
    args: Value,
) -> Result<Option<Value>, String> {
    check_arguments(action, &args)?;
    let scope = current_task(shared)
        .and_then(|task| shared.tasks.borrow().get(&task).cloned())
        .unwrap_or_default();
    let ctx = scope_table(lua, &scope).map_err(|error| error::describe(&error))?;
    let args = to_lua(lua, &args).map_err(|error| error::describe(&error))?;
    let returned = action
        .run
        .call_async::<MultiValue>((ctx, args))
        .await
        .map_err(|error| match error::halt_of(&error) {
            Some(halt) => halt.to_string(),
            None => error::describe(&error),
        })?;
    match returned.into_iter().next() {
        None | Some(LuaValue::Nil) | Some(LuaValue::Boolean(true)) => Ok(None),
        Some(value) => to_json(lua, &value).map(Some).map_err(|message| {
            format!("the action returned a value that cannot be encoded as JSON: {message}")
        }),
    }
}

fn nil_and(lua: &Lua, message: String) -> mlua::Result<MultiValue> {
    Ok(MultiValue::from_iter([
        LuaValue::Nil,
        LuaValue::String(lua.create_string(message)?),
    ]))
}

async fn invoke(lua: Lua, (name, args): (String, LuaValue)) -> mlua::Result<MultiValue> {
    let shared = shared(&lua)?;
    let args = match arguments_of(&lua, &args) {
        Ok(args) => args,
        Err(message) => return nil_and(&lua, format!("paneflow.invoke {name}: {message}")),
    };
    let task = current_task(&shared);
    let depth = task
        .and_then(|task| shared.tasks.borrow().get(&task).map(|scope| scope.depth))
        .unwrap_or(0);
    if depth >= MAX_INVOKE_DEPTH {
        return nil_and(
            &lua,
            format!(
                "paneflow.invoke {name}: actions nest at most {MAX_INVOKE_DEPTH} deep; this call would be deeper"
            ),
        );
    }
    if let Some(action) = shared.config.action(&name) {
        let opened = task.is_some_and(|task| open_caller_scope(&shared, task));
        let caller = task.and_then(|task| {
            shared.tasks.borrow_mut().get_mut(&task).map(|scope| {
                scope.depth += 1;
                scope.action.replace(ActionLabel::of(&action))
            })
        });
        let result = call_action(&lua, &shared, &action, args).await;
        if let Some(caller) = caller
            && let Some(task) = task
            && let Some(scope) = shared.tasks.borrow_mut().get_mut(&task)
        {
            scope.depth = scope.depth.saturating_sub(1);
            scope.action = caller;
        }
        if opened && let Some(task) = task {
            shared.tasks.borrow_mut().remove(&task);
        }
        return match result {
            Ok(Some(value)) => Ok(MultiValue::from_iter([to_lua(&lua, &value)?])),
            Ok(None) => Ok(MultiValue::from_iter([LuaValue::Boolean(true)])),
            Err(message) => nil_and(&lua, message),
        };
    }
    if shared.serving {
        return nil_and(
            &lua,
            format!("paneflow.invoke: no action named '{name}' in this configuration"),
        );
    }
    let scope = task
        .and_then(|task| shared.tasks.borrow().get(&task).cloned())
        .unwrap_or_else(|| TaskScope {
            ids: shared.context.borrow().clone(),
            origin: "script".to_string(),
            ..TaskScope::default()
        });
    let params = json!({
        "name": name,
        "args": args,
        "ctx": serde_json::to_value(InvokeContext {
            origin: Some("script".to_string()),
            surface_id: scope.ids.surface_id,
            pane_id: scope.ids.pane_id,
            tab_id: scope.ids.tab_id,
            workspace_id: scope.ids.workspace_id,
            client_id: scope.client_id.clone(),
            grant: None,
            session: None,
            agent: None,
        })
        .unwrap_or_else(|_| json!({})),
    });
    let backend = shared.backend.clone();
    let answer = shared
        .executor
        .offload("invoke", move || backend.call("action.invoke", params))
        .await;
    match answer {
        Some(Ok(reply)) => match reply.get("result") {
            Some(value) if !value.is_null() => Ok(MultiValue::from_iter([to_lua(&lua, value)?])),
            _ => Ok(MultiValue::from_iter([LuaValue::Boolean(true)])),
        },
        Some(Err(error)) => nil_and(&lua, error.message),
        None => nil_and(
            &lua,
            format!("paneflow.invoke {name}: no thread could be started for the call"),
        ),
    }
}

async fn queue(lua: Lua, (name, args): (String, LuaValue)) -> mlua::Result<MultiValue> {
    let shared = shared(&lua)?;
    if !matches!(&args, LuaValue::Nil)
        && !matches!(arguments_of(&lua, &args), Ok(Value::Object(map)) if map.is_empty())
    {
        return Err(located(
            &lua,
            format!("paneflow.client.queue {name}: built-in actions take no arguments"),
        ));
    }
    let backend = shared.backend.clone();
    let attached = shared
        .executor
        .offload("window", move || backend.window_attached())
        .await
        .unwrap_or(false);
    if !attached {
        return Err(located(
            &lua,
            format!(
                "paneflow.client.queue {name}: no Paneflow window is attached to run built-in actions"
            ),
        ));
    }
    let task = current_task(&shared);
    let queued = task.is_some_and(|task| {
        shared
            .tasks
            .borrow_mut()
            .get_mut(&task)
            .filter(|scope| !scope.dispatches_at_once)
            .map(|scope| scope.queue.push(name.clone()))
            .is_some()
    });
    if queued {
        return Ok(MultiValue::from_iter([LuaValue::Boolean(true)]));
    }
    let backend = shared.backend.clone();
    let answer = shared
        .executor
        .offload("dispatch", move || {
            backend.call("client.dispatch_action", json!({"name": name}))
        })
        .await;
    match answer {
        Some(Ok(_)) => Ok(MultiValue::from_iter([LuaValue::Boolean(true)])),
        Some(Err(error)) => nil_and(&lua, error.message),
        None => nil_and(&lua, "no thread could be started for the call".to_string()),
    }
}

pub(crate) fn install(lua: &Lua, paneflow: &Table, guard: &Function) -> mlua::Result<()> {
    let action = lua.create_table()?;
    let metatable = lua.create_table()?;
    let plain: Function = lua.named_registry_value(crate::guard::PLAIN_KEY)?;
    let call = lua.create_function(|lua, (_, spec): (LuaValue, LuaValue)| declare(lua, spec))?;
    metatable.set("__call", plain.call::<Function>(call)?)?;
    action.set_metatable(Some(metatable))?;
    paneflow.set("action", action)?;
    paneflow.set("bind", lua.create_function(bind)?)?;
    paneflow.set("unbind", lua.create_function(unbind)?)?;
    paneflow.set("bindings", lua.create_function(bindings)?)?;
    let invoke = lua.create_async_function(invoke)?;
    paneflow.set(
        "invoke",
        crate::guard::guarded(guard, "paneflow.invoke", invoke)?,
    )?;
    let client = lua.create_table()?;
    let queue = lua.create_async_function(queue)?;
    client.set(
        "queue",
        crate::guard::guarded(guard, "paneflow.client.queue", queue)?,
    )?;
    paneflow.set("client", client)?;
    Ok(())
}

pub(crate) struct Deadline<F> {
    future: Pin<Box<F>>,
    shared: Rc<Shared>,
    task: TaskId,
    armed: Option<(Duration, crate::executor::Sleep)>,
}

impl<F: Future> Future for Deadline<F> {
    type Output = Option<F::Output>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if let Poll::Ready(value) = self.future.as_mut().poll(context) {
            return Poll::Ready(Some(value));
        }
        let at = self
            .shared
            .tasks
            .borrow()
            .get(&self.task)
            .and_then(|scope| scope.deadline);
        let Some(at) = at else {
            self.armed = None;
            return Poll::Pending;
        };
        if self.armed.as_ref().is_none_or(|(armed, _)| *armed != at) {
            let sleep = self.shared.executor.sleep_until(at);
            self.armed = Some((at, sleep));
        }
        let expired = self
            .armed
            .as_mut()
            .is_some_and(|(_, sleep)| Pin::new(sleep).poll(context).is_ready());
        if expired {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

pub(crate) struct PausedDeadline {
    shared: Rc<Shared>,
    task: TaskId,
    remaining: Duration,
}

impl PausedDeadline {
    pub(crate) fn pause(shared: &Rc<Shared>) -> Option<Self> {
        let task = current_task(shared)?;
        let at = shared.tasks.borrow_mut().get_mut(&task)?.deadline.take()?;
        Some(Self {
            shared: shared.clone(),
            task,
            remaining: at.saturating_sub(shared.executor.now()),
        })
    }
}

impl Drop for PausedDeadline {
    fn drop(&mut self) {
        let resumed = self.shared.executor.now() + self.remaining;
        if let Some(scope) = self.shared.tasks.borrow_mut().get_mut(&self.task) {
            scope.deadline = Some(resumed);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionFailure {
    pub message: String,
    pub timeout: bool,
}

pub type ActionResult = Result<Option<Value>, ActionFailure>;

pub type Reply = Box<dyn FnOnce(ActionResult) + Send>;

pub fn timeout_message(name: &str) -> String {
    format!(
        "timeout: the action {name} was stopped after {} s",
        ACTION_TIME_LIMIT.as_secs()
    )
}

pub(crate) fn spawn_action(
    lua: &Lua,
    shared: &Rc<Shared>,
    name: String,
    args: Value,
    context: InvokeContext,
    reply: Reply,
) {
    let Some(action) = shared.config.action(&name) else {
        reply(Err(ActionFailure {
            message: format!("no action named '{name}'"),
            timeout: false,
        }));
        return;
    };
    let lua = lua.weak();
    let task_shared = shared.clone();
    let executor = shared.executor.clone();
    executor.clone().spawn(TaskKind::Action, async move {
        let shared = task_shared;
        let lua = lua.upgrade();
        let Some(task) = current_task(&shared) else {
            return;
        };
        let deadline = executor.now() + ACTION_TIME_LIMIT;
        shared.tasks.borrow_mut().insert(
            task,
            TaskScope {
                ids: PaneContext {
                    surface_id: context.surface_id,
                    pane_id: context.pane_id,
                    tab_id: context.tab_id,
                    workspace_id: context.workspace_id,
                },
                action: Some(ActionLabel::of(&action)),
                origin: context.origin.clone().unwrap_or_else(|| "api".to_string()),
                client_id: context.client_id.clone(),
                grant: context.grant.clone(),
                deadline: Some(deadline),
                depth: 1,
                queue: Vec::new(),
                dispatches_at_once: false,
                agent: (context.origin.as_deref() == Some("agent"))
                    .then(|| context.agent.clone())
                    .flatten(),
            },
        );
        let run = {
            let lua = lua.clone();
            let shared = shared.clone();
            let action = action.clone();
            async move { call_action(&lua, &shared, &action, args).await }
        };
        let outcome = Deadline {
            future: Box::pin(run),
            shared: shared.clone(),
            task,
            armed: None,
        }
        .await;
        let scope = shared.tasks.borrow_mut().remove(&task);
        let result = match outcome {
            Some(Ok(value)) => {
                for queued in scope.map(|scope| scope.queue).unwrap_or_default() {
                    let backend = shared.backend.clone();
                    let answer = executor
                        .offload("dispatch", move || {
                            backend.call("client.dispatch_action", json!({"name": queued}))
                        })
                        .await;
                    if let Some(Err(error)) = answer {
                        shared.sink.report(&format!(
                            "the action {} queued a built-in action that failed: {}",
                            action.summary.name, error.message
                        ));
                    }
                }
                Ok(value)
            }
            Some(Err(message)) => Err(ActionFailure {
                timeout: message.starts_with("timeout:"),
                message,
            }),
            None => Err(ActionFailure {
                message: timeout_message(&action.summary.name),
                timeout: true,
            }),
        };
        reply(result);
    });
}

pub fn deadline_error(lua: &Lua) -> mlua::Error {
    mlua::Error::RuntimeError(format!(
        "{}timeout: the action ran for more than {} s",
        error::location(lua),
        ACTION_TIME_LIMIT.as_secs()
    ))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadReport {
    pub ok: bool,
    pub error: Option<String>,
    pub actions: Vec<ActionSummary>,
    pub keymap: Keymap,
    pub problems: Vec<ConfigProblem>,
    pub handlers: usize,
    pub timers: usize,
    pub required: Vec<PathBuf>,
}

impl LoadReport {
    pub fn has_errors(&self) -> bool {
        self.problems
            .iter()
            .any(|problem| problem.severity == Severity::Error)
    }
}

pub(crate) fn report(shared: &Shared, error: Option<String>) -> LoadReport {
    shared.config.finalize();
    let mut problems = shared.config.problems();
    if let Some(message) = &error {
        problems.push(problem_from_message(Severity::Error, message));
    }
    LoadReport {
        ok: error.is_none(),
        error,
        actions: shared.config.summaries(),
        keymap: shared.config.keymap(),
        problems,
        handlers: shared.events.handler_count(),
        timers: shared.timers.active(),
        required: shared.required.borrow().clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_location_splits_into_its_file_and_line() {
        assert_eq!(
            split_location("init.lua:12:"),
            (Some("init.lua".to_string()), Some(12))
        );
        assert_eq!(
            split_location("lua/dev.lua:3"),
            (Some("lua/dev.lua".to_string()), Some(3))
        );
        assert_eq!(split_location(""), (None, None));
        let problem = problem_from_message(Severity::Error, "init.lua:7: unexpected symbol");
        assert_eq!(problem.file.as_deref(), Some("init.lua"));
        assert_eq!(problem.line, Some(7));
        assert_eq!(problem.message, "unexpected symbol");
    }

    #[test]
    fn names_are_lowercase_letters_digits_and_underscores() {
        assert!(valid_name("dev_layout2"));
        assert!(!valid_name("Dev"));
        assert!(!valid_name("dev.layout"));
        assert!(!valid_name("dev-layout"));
        assert!(!valid_name(""));
    }

    #[test]
    fn keys_compare_with_either_separator_and_any_case() {
        assert_eq!(normalize_key("Ctrl+Alt+D"), normalize_key("ctrl-alt-d"));
    }
}
