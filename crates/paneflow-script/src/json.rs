use std::collections::HashSet;

use mlua::{Lua, LuaSerdeExt, Table, Value as LuaValue};
use serde_json::{Map, Number, Value};

pub const MAX_DEPTH: usize = 128;

pub fn to_json(lua: &Lua, value: &LuaValue) -> Result<Value, String> {
    let mut visiting = HashSet::new();
    convert(lua, value, &mut visiting, 0)
}

fn convert(
    lua: &Lua,
    value: &LuaValue,
    visiting: &mut HashSet<usize>,
    depth: usize,
) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "a value nested deeper than {MAX_DEPTH} levels cannot be encoded"
        ));
    }
    match value {
        LuaValue::Nil => Ok(Value::Null),
        LuaValue::Boolean(flag) => Ok(Value::Bool(*flag)),
        LuaValue::Integer(number) => Ok(Value::from(*number)),
        LuaValue::Number(number) => Number::from_f64(*number)
            .map(Value::Number)
            .ok_or_else(|| format!("the number {number} cannot be encoded as JSON")),
        LuaValue::String(text) => Ok(Value::String(text.to_string_lossy())),
        LuaValue::Table(table) => {
            let address = table.to_pointer() as usize;
            if !visiting.insert(address) {
                return Err("a table that contains itself cannot be encoded".to_string());
            }
            let converted = convert_table(lua, table, visiting, depth);
            visiting.remove(&address);
            converted
        }
        LuaValue::LightUserData(pointer) if pointer.0.is_null() => Ok(Value::Null),
        other => Err(format!("a {} cannot be encoded as JSON", other.type_name())),
    }
}

fn is_marked_array(lua: &Lua, table: &Table) -> bool {
    table
        .metatable()
        .is_some_and(|metatable| metatable == lua.array_metatable())
}

fn sequence_length(table: &Table) -> Option<usize> {
    let mut count = 0usize;
    let mut highest = 0i64;
    for pair in table.pairs::<LuaValue, LuaValue>() {
        let Ok((key, _)) = pair else {
            return None;
        };
        match key {
            LuaValue::Integer(index) if index >= 1 => {
                count += 1;
                highest = highest.max(index);
            }
            _ => return None,
        }
    }
    (count > 0 && usize::try_from(highest).ok() == Some(count)).then_some(count)
}

fn convert_table(
    lua: &Lua,
    table: &Table,
    visiting: &mut HashSet<usize>,
    depth: usize,
) -> Result<Value, String> {
    let length = sequence_length(table);
    if let Some(length) = length.or_else(|| is_marked_array(lua, table).then(|| table.raw_len())) {
        let mut items = Vec::with_capacity(length);
        for index in 1..=length {
            let item: LuaValue = table.raw_get(index).map_err(|error| error.to_string())?;
            items.push(convert(lua, &item, visiting, depth + 1)?);
        }
        return Ok(Value::Array(items));
    }
    let mut entries = Vec::new();
    for pair in table.pairs::<LuaValue, LuaValue>() {
        entries.push(pair.map_err(|error| error.to_string())?);
    }
    let mut keyed = Vec::with_capacity(entries.len());
    for (key, item) in entries {
        let key = match key {
            LuaValue::String(text) => text.to_string_lossy(),
            LuaValue::Integer(number) => number.to_string(),
            LuaValue::Number(number) => number.to_string(),
            other => {
                return Err(format!(
                    "a table key of type {} cannot be encoded as JSON",
                    other.type_name()
                ));
            }
        };
        keyed.push((key, item));
    }
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    let mut object = Map::new();
    for (key, item) in keyed {
        object.insert(key, convert(lua, &item, visiting, depth + 1)?);
    }
    Ok(Value::Object(object))
}

pub fn to_lua(lua: &Lua, value: &Value) -> mlua::Result<LuaValue> {
    Ok(match value {
        Value::Null => LuaValue::Nil,
        Value::Bool(flag) => LuaValue::Boolean(*flag),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => LuaValue::Integer(integer),
            None => LuaValue::Number(number.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(text) => LuaValue::String(lua.create_string(text)?),
        Value::Array(items) => {
            let table = lua.create_table_with_capacity(items.len(), 0)?;
            for (index, item) in items.iter().enumerate() {
                table.raw_set(index + 1, to_lua(lua, item)?)?;
            }
            table.set_metatable(Some(lua.array_metatable()))?;
            LuaValue::Table(table)
        }
        Value::Object(fields) => {
            let table = lua.create_table_with_capacity(0, fields.len())?;
            for (key, item) in fields {
                table.raw_set(key.as_str(), to_lua(lua, item)?)?;
            }
            LuaValue::Table(table)
        }
    })
}

pub fn install(lua: &Lua, paneflow: &Table) -> mlua::Result<()> {
    let json = lua.create_table()?;
    json.set(
        "encode",
        lua.create_function(|lua, (value, pretty): (LuaValue, Option<bool>)| {
            match to_json(lua, &value) {
                Ok(json) => {
                    let text = if pretty.unwrap_or(false) {
                        serde_json::to_string_pretty(&json)
                    } else {
                        serde_json::to_string(&json)
                    };
                    match text {
                        Ok(text) => Ok((LuaValue::String(lua.create_string(text)?), None)),
                        Err(error) => Ok((LuaValue::Nil, Some(error.to_string()))),
                    }
                }
                Err(message) => Ok((LuaValue::Nil, Some(message))),
            }
        })?,
    )?;
    json.set(
        "decode",
        lua.create_function(|lua, text: mlua::LuaString| {
            match serde_json::from_slice::<Value>(&text.as_bytes()) {
                Ok(value) => Ok((to_lua(lua, &value)?, None)),
                Err(error) => Ok((LuaValue::Nil, Some(format!("invalid JSON: {error}")))),
            }
        })?,
    )?;
    paneflow.set("json", json)
}
