use std::rc::Rc;

use mlua::{Function, Lua, MultiValue, Table, Value as LuaValue};
use paneflow_api::Descriptor;
use paneflow_api::methods::prompt::{
    OriginKind, PromptOrigin, PromptOutcome, PromptStatus, Request, RequestParams, check_spec,
};
use serde_json::{Map, Value, json};

use crate::config::{PausedDeadline, at_load, current_task};
use crate::error::located;
use crate::json::{to_json, to_lua};
use crate::runtime::{CallError, Shared, shared};

pub const WAIT_SLICE_MS: u64 = 20_000;
pub const SET_BY_THE_FUNCTION: &[&str] = &["kind", "origin", "wait_ms"];

fn spec_params(
    lua: &Lua,
    name: &str,
    kind: &str,
    spec: &LuaValue,
) -> mlua::Result<Map<String, Value>> {
    let mut params = match spec {
        LuaValue::Table(_) => match to_json(lua, spec) {
            Ok(Value::Object(map)) => map,
            Ok(Value::Array(items)) if items.is_empty() => Map::new(),
            Ok(_) => {
                return Err(located(
                    lua,
                    format!("{name} takes a table of named fields such as {{ title = \"...\" }}"),
                ));
            }
            Err(message) => return Err(located(lua, format!("{name}: {message}"))),
        },
        other => {
            return Err(located(
                lua,
                format!("{name} takes a table, not a {}", other.type_name()),
            ));
        }
    };
    if let Some(field) = SET_BY_THE_FUNCTION
        .iter()
        .find(|field| params.contains_key(**field))
    {
        return Err(located(
            lua,
            format!("{name}: '{field}' is set by Paneflow and cannot be given"),
        ));
    }
    if params
        .get("items")
        .is_some_and(|items| items.as_object().is_some_and(Map::is_empty))
    {
        params.insert("items".into(), Value::Array(Vec::new()));
    }
    params.insert("kind".into(), Value::from(kind));
    let schema = Descriptor::of::<Request>().params_schema();
    paneflow_api::schema::validate(&schema, &Value::Object(params.clone()))
        .map_err(|violation| located(lua, format!("{name}: {}", violation.message)))?;
    let request: RequestParams = serde_json::from_value(Value::Object(params.clone()))
        .map_err(|error| located(lua, format!("{name}: {error}")))?;
    check_spec(&request.spec()).map_err(|message| located(lua, format!("{name}: {message}")))?;
    Ok(params)
}

fn origin(shared: &Shared) -> PromptOrigin {
    let scope = current_task(shared).and_then(|task| shared.tasks.borrow().get(&task).cloned());
    let ids = scope.as_ref().map_or_else(
        || shared.context.borrow().clone(),
        |scope| scope.ids.clone(),
    );
    let agent = scope.as_ref().and_then(|scope| scope.agent.clone());
    let action = scope.and_then(|scope| scope.action);
    let (kind, title, name, file) = match action {
        Some(action) => (
            OriginKind::Action,
            Some(action.title),
            Some(action.name),
            None,
        ),
        None => (OriginKind::Script, None, None, shared.script_name.clone()),
    };
    PromptOrigin {
        kind,
        title,
        name,
        file,
        agent: if kind == OriginKind::Action {
            agent
        } else {
            None
        },
        surface_id: ids.surface_id,
        workspace_id: ids.workspace_id,
        session: None,
    }
}

fn no_answer(lua: &Lua, status: PromptStatus, detail: Option<String>) -> mlua::Result<MultiValue> {
    let mut values = vec![
        LuaValue::Nil,
        LuaValue::String(lua.create_string(status.label())?),
    ];
    if status == PromptStatus::Failed {
        let detail = detail.unwrap_or_else(|| "the prompt failed".to_string());
        values.push(LuaValue::String(lua.create_string(detail)?));
    }
    Ok(MultiValue::from_iter(values))
}

async fn call(
    shared: &Rc<Shared>,
    method: &'static str,
    params: Value,
) -> Result<Value, CallError> {
    let backend = shared.backend.clone();
    shared
        .executor
        .offload("prompt", move || backend.call(method, params))
        .await
        .unwrap_or_else(|| {
            Err(CallError::new(
                "no thread could be started for the prompt",
                CallError::UNAVAILABLE,
            ))
        })
}

async fn ask(lua: Lua, kind: &'static str, spec: LuaValue) -> mlua::Result<MultiValue> {
    let shared = shared(&lua)?;
    let name = format!("paneflow.ui.{kind}");
    let mut params = spec_params(&lua, &name, kind, &spec)?;
    if shared.serving && at_load(&shared) {
        return no_answer(
            &lua,
            PromptStatus::Failed,
            Some(format!(
                "{name} cannot ask while init.lua loads; call it from an action, a handler or a timer"
            )),
        );
    }
    params.insert(
        "origin".into(),
        serde_json::to_value(origin(&shared)).unwrap_or(Value::Null),
    );
    params.insert("wait_ms".into(), Value::from(WAIT_SLICE_MS));
    let paused = PausedDeadline::pause(&shared);
    let mut answer = call(&shared, "prompt.request", Value::Object(params)).await;
    let outcome = loop {
        let outcome = match answer {
            Ok(value) => serde_json::from_value::<PromptOutcome>(value).unwrap_or_else(|error| {
                PromptOutcome::failed(
                    None,
                    format!("the prompt answered an unreadable outcome: {error}"),
                )
            }),
            Err(error) if error.code == CallError::INVALID_PARAMS => {
                drop(paused);
                return Err(located(&lua, format!("{name}: {}", error.message)));
            }
            Err(error) => PromptOutcome::failed(None, error.message),
        };
        match (outcome.status, outcome.id) {
            (PromptStatus::Pending, Some(id)) => {
                answer = call(
                    &shared,
                    "prompt.wait",
                    json!({"id": id, "wait_ms": WAIT_SLICE_MS}),
                )
                .await;
            }
            _ => break outcome,
        }
    };
    drop(paused);
    match (outcome.status, outcome.answer) {
        (PromptStatus::Answered, Some(answer)) => {
            let value = serde_json::to_value(answer).unwrap_or(Value::Null);
            Ok(MultiValue::from_iter([to_lua(&lua, &value)?]))
        }
        (PromptStatus::Answered, None) => no_answer(
            &lua,
            PromptStatus::Failed,
            Some("the prompt was answered with nothing".to_string()),
        ),
        (status, _) => no_answer(&lua, status, outcome.detail),
    }
}

pub(crate) fn install(lua: &Lua, paneflow: &Table, guard: &Function) -> mlua::Result<()> {
    let ui = lua.create_table()?;
    let input = lua.create_async_function(|lua, spec: LuaValue| ask(lua, "input", spec))?;
    ui.set(
        "input",
        crate::guard::guarded(guard, "paneflow.ui.input", input)?,
    )?;
    let pick = lua.create_async_function(|lua, spec: LuaValue| ask(lua, "pick", spec))?;
    ui.set(
        "pick",
        crate::guard::guarded(guard, "paneflow.ui.pick", pick)?,
    )?;
    paneflow.set("ui", ui)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fields_paneflow_sets_are_the_request_fields_a_script_does_not_write() {
        let schema = Descriptor::of::<Request>().params_schema();
        for field in SET_BY_THE_FUNCTION {
            assert!(
                schema["properties"].get(*field).is_some(),
                "{field} is a prompt.request parameter"
            );
        }
    }
}
