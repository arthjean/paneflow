use std::collections::BTreeMap;

use mlua::{Function, Lua, MultiValue, Table, Value as LuaValue};
use serde_json::{Map, Value};

use crate::json::{to_json, to_lua};
use crate::runtime::{CallError, Discovery, PaneContext, shared};

pub const FILLED_FROM_CONTEXT: &[&str] = &["surface_id", "workspace_id"];

#[derive(Debug, Clone, PartialEq)]
pub struct MethodInfo {
    pub name: String,
    pub schema: Value,
    pub declared: Vec<String>,
    pub required: Vec<String>,
    pub stream: bool,
}

impl MethodInfo {
    pub fn from_description(description: &Value) -> Option<Self> {
        let name = description["name"].as_str()?.to_string();
        let schema = description["params"].clone();
        let (declared, required) = declared_parameters(&schema);
        Some(Self {
            name,
            schema,
            declared,
            required,
            stream: description["stream"].as_bool().unwrap_or(false)
                || description["reply"] == "stream",
        })
    }
}

fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix('#'))
        .and_then(|pointer| root.pointer(pointer))
        .map_or(schema, |target| resolve(root, target))
}

pub fn declared_parameters(schema: &Value) -> (Vec<String>, Vec<String>) {
    let object = resolve(schema, schema);
    let forms: Vec<&Value> = match object.get("anyOf").or_else(|| object.get("oneOf")) {
        Some(Value::Array(forms)) => forms.iter().map(|form| resolve(schema, form)).collect(),
        _ => vec![object],
    };
    let mut declared = Vec::new();
    let mut required = Vec::new();
    for form in forms {
        if let Some(properties) = form.get("properties").and_then(Value::as_object) {
            for key in properties.keys() {
                if !declared.contains(key) {
                    declared.push(key.clone());
                }
            }
        }
        if let Some(names) = form.get("required").and_then(Value::as_array) {
            for name in names.iter().filter_map(Value::as_str) {
                if !required.iter().any(|known| known == name) {
                    required.push(name.to_string());
                }
            }
        }
    }
    (declared, required)
}

#[derive(Debug, Default)]
pub struct Methods {
    by_name: BTreeMap<String, MethodInfo>,
    unavailable: Option<CallError>,
}

impl Methods {
    pub fn get(&self, name: &str) -> Option<&MethodInfo> {
        self.by_name.get(name)
    }
}

pub fn prepare(
    info: &MethodInfo,
    mut params: Map<String, Value>,
    context: &PaneContext,
) -> Result<Value, CallError> {
    if let Some(unknown) = params
        .keys()
        .find(|key| !info.declared.iter().any(|declared| declared == *key))
    {
        let message = if info.declared.is_empty() {
            format!(
                "{}: unknown parameter '{unknown}'; it takes no parameters",
                info.name
            )
        } else {
            format!(
                "{}: unknown parameter '{unknown}'; it declares {}",
                info.name,
                info.declared.join(", ")
            )
        };
        return Err(CallError::new(message, CallError::INVALID_PARAMS));
    }
    for key in FILLED_FROM_CONTEXT {
        let declared = info.declared.iter().any(|name| name == key);
        let required = info.required.iter().any(|name| name == key);
        if !declared || required || params.get(*key).is_some_and(|value| !value.is_null()) {
            continue;
        }
        let value = match *key {
            "surface_id" => context.surface_id,
            _ => context.workspace_id,
        };
        if let Some(value) = value {
            params.insert((*key).to_string(), Value::from(value));
        }
    }
    let params = Value::Object(params);
    paneflow_api::schema::validate(&info.schema, &params).map_err(|violation| {
        CallError::new(
            format!("{}: {}", info.name, violation.message),
            CallError::INVALID_PARAMS,
        )
    })?;
    Ok(params)
}

fn failure(lua: &Lua, error: &CallError) -> mlua::Result<MultiValue> {
    Ok(MultiValue::from_iter([
        LuaValue::Nil,
        LuaValue::String(lua.create_string(&error.message)?),
        LuaValue::Integer(error.code),
    ]))
}

fn arguments(lua: &Lua, name: &str, args: &LuaValue) -> Result<Map<String, Value>, CallError> {
    match args {
        LuaValue::Nil => Ok(Map::new()),
        LuaValue::Table(_) => match to_json(lua, args) {
            Ok(Value::Object(map)) => Ok(map),
            Ok(Value::Array(items)) if items.is_empty() => Ok(Map::new()),
            Ok(_) => Err(CallError::new(
                format!("{name}: the parameters must be a table of named fields"),
                CallError::INVALID_PARAMS,
            )),
            Err(message) => Err(CallError::new(
                format!("{name}: {message}"),
                CallError::INVALID_PARAMS,
            )),
        },
        other => Err(CallError::new(
            format!(
                "{name}: the parameters must be a table, not a {}",
                other.type_name()
            ),
            CallError::INVALID_PARAMS,
        )),
    }
}

pub async fn invoke(lua: Lua, name: String, args: LuaValue) -> mlua::Result<MultiValue> {
    let shared = shared(&lua)?;
    let (info, unavailable) = {
        let methods = shared.methods.borrow();
        (methods.get(&name).cloned(), methods.unavailable.clone())
    };
    let Some(info) = info else {
        return failure(
            &lua,
            &CallError::new(
                format!("Method not found: {name}"),
                CallError::METHOD_NOT_FOUND,
            ),
        );
    };
    if let Some(unavailable) = unavailable {
        return failure(&lua, &unavailable);
    }
    if info.stream {
        return failure(
            &lua,
            &CallError::new(
                format!(
                    "{name} is a stream and answers no single call; react to events with paneflow.on or paneflow.wait"
                ),
                CallError::METHOD_NOT_FOUND,
            ),
        );
    }
    let context =
        crate::config::current_ids(&shared).unwrap_or_else(|| shared.context.borrow().clone());
    let params =
        match arguments(&lua, &name, &args).and_then(|params| prepare(&info, params, &context)) {
            Ok(params) => params,
            Err(error) => return failure(&lua, &error),
        };
    let backend = shared.backend.clone();
    let method = name.clone();
    let answer = shared
        .executor
        .offload("call", move || backend.call(&method, params))
        .await;
    match answer {
        Some(Ok(result)) => Ok(MultiValue::from_iter([to_lua(&lua, &result)?])),
        Some(Err(error)) => failure(&lua, &error),
        None => failure(
            &lua,
            &CallError::new(
                format!("{name}: no thread could be started for the call"),
                CallError::UNAVAILABLE,
            ),
        ),
    }
}

fn method_function(lua: &Lua, guard: &Function, name: &str) -> mlua::Result<Function> {
    let owned = name.to_string();
    let function =
        lua.create_async_function(move |lua, args: LuaValue| invoke(lua, owned.clone(), args))?;
    crate::guard::guarded(guard, &format!("paneflow.{name}"), function)
}

pub fn install_call(lua: &Lua, paneflow: &Table, guard: &Function) -> mlua::Result<()> {
    let call = lua.create_async_function(|lua, (method, args): (String, LuaValue)| {
        invoke(lua, method, args)
    })?;
    paneflow.set("call", crate::guard::guarded(guard, "paneflow.call", call)?)
}

fn namespace(lua: &Lua, root: &Table, parts: &[&str]) -> mlua::Result<Option<Table>> {
    let mut table = root.clone();
    for part in parts {
        table = match table.get::<LuaValue>(*part)? {
            LuaValue::Table(inner) => inner,
            LuaValue::Nil => {
                let inner = lua.create_table()?;
                table.set(*part, inner.clone())?;
                inner
            }
            _ => return Ok(None),
        };
    }
    Ok(Some(table))
}

pub fn install(
    lua: &Lua,
    paneflow: &Table,
    guard: &Function,
    discovery: Discovery,
) -> mlua::Result<()> {
    let shared = shared(lua)?;
    let mut methods = Methods {
        by_name: BTreeMap::new(),
        unavailable: discovery.unavailable,
    };
    for description in &discovery.methods {
        let Some(info) = MethodInfo::from_description(description) else {
            continue;
        };
        let mut parts: Vec<&str> = info.name.split('.').collect();
        if let Some(leaf) = parts.pop()
            && !parts.is_empty()
            && let Some(table) = namespace(lua, paneflow, &parts)?
            && table.get::<LuaValue>(leaf)?.is_nil()
        {
            table.set(leaf, method_function(lua, guard, &info.name)?)?;
        }
        methods.by_name.insert(info.name.clone(), info);
    }
    *shared.methods.borrow_mut() = methods;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn info(method: &str) -> MethodInfo {
        let catalog = paneflow_api::catalog().expect("catalog");
        MethodInfo::from_description(&catalog.get(method).expect(method).describe()).expect("info")
    }

    #[test]
    fn an_undeclared_key_is_refused_naming_it_and_the_declared_ones() {
        let mut params = Map::new();
        params.insert("line".into(), json!(3));
        let error =
            prepare(&info("surface.read"), params, &PaneContext::default()).expect_err("refused");
        assert_eq!(error.code, CallError::INVALID_PARAMS);
        assert!(error.message.contains("'line'"), "{}", error.message);
        assert!(error.message.contains("lines"), "{}", error.message);
        assert!(error.message.contains("surface_id"), "{}", error.message);
    }

    #[test]
    fn the_pane_context_fills_an_optional_surface_and_workspace() {
        let context = PaneContext {
            surface_id: Some(7),
            workspace_id: Some(3),
            ..PaneContext::default()
        };
        let read = prepare(&info("surface.read"), Map::new(), &context).expect("valid");
        assert_eq!(read["surface_id"], 7);
        assert_eq!(read["workspace_id"], 3);
        let mut explicit = Map::new();
        explicit.insert("surface_id".into(), json!(9));
        let read = prepare(&info("surface.read"), explicit, &context).expect("valid");
        assert_eq!(read["surface_id"], 9);
    }

    #[test]
    fn a_required_surface_is_never_guessed_from_the_context() {
        let context = PaneContext {
            surface_id: Some(7),
            ..PaneContext::default()
        };
        let error = prepare(&info("surface.focus"), Map::new(), &context).expect_err("required");
        assert_eq!(error.code, CallError::INVALID_PARAMS);
    }
}
