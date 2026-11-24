use mlua::{Function, Lua, Table, Value as LuaValue};

pub const REGISTRY_KEY: &str = "paneflow.guard";
pub const PLAIN_KEY: &str = "paneflow.plain";

const GUARD_SOURCE: &str = r#"
local clean = ...
local create, wrap, running, isyieldable = coroutine.create, coroutine.wrap, coroutine.running, coroutine.isyieldable
local error, type, pcall, pack, unpack = error, type, pcall, table.pack, table.unpack
local created = setmetatable({}, { __mode = "k" })

coroutine.create = function(f)
  if type(f) ~= "function" then
    return create(f)
  end
  return create(function(...)
    created[running()] = true
    return f(...)
  end)
end

coroutine.wrap = function(f)
  if type(f) ~= "function" then
    return wrap(f)
  end
  return wrap(function(...)
    created[running()] = true
    return f(...)
  end)
end

local wrapped = setmetatable({}, { __mode = "k" })

local function raise(err)
  local message, located = clean(err)
  if located then
    error(message, 0)
  end
  error(message, 3)
end

local function plain(f)
  if wrapped[f] then
    return f
  end
  local function plain_call(...)
    local results = pack(pcall(f, ...))
    if results[1] then
      return unpack(results, 2, results.n)
    end
    raise(results[2])
  end
  wrapped[plain_call] = true
  return plain_call
end

local function guard(name, f)
  local function guarded_call(...)
    if created[running()] then
      error(name .. " cannot run inside a coroutine the script created with coroutine.create or coroutine.wrap: it waits, and waiting would suspend that coroutine instead of the script. Call it outside the coroutine", 2)
    end
    if not isyieldable() then
      error(name .. " cannot wait inside a callback that the Lua core calls, such as a table.sort comparator, a string.gsub replacement or a __gc metamethod. Move the call out of the callback", 2)
    end
    local results = pack(pcall(f, ...))
    if results[1] then
      return unpack(results, 2, results.n)
    end
    raise(results[2])
  end
  wrapped[guarded_call] = true
  return guarded_call
end

return guard, plain
"#;

pub fn has_location(message: &str) -> bool {
    let mut parts = message.splitn(3, ':');
    let (Some(source), Some(line), Some(_)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !source.is_empty() && !line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit())
}

fn clean(lua: &Lua, value: LuaValue) -> mlua::Result<(LuaValue, bool)> {
    match value {
        LuaValue::Error(error) if crate::error::halt_of(&error).is_none() => {
            let message = crate::error::describe(&error);
            let located = has_location(&message);
            Ok((LuaValue::String(lua.create_string(message)?), located))
        }
        other => Ok((other, true)),
    }
}

pub fn install(lua: &Lua) -> mlua::Result<(Function, Function)> {
    let clean = lua.create_function(clean)?;
    lua.load(GUARD_SOURCE)
        .set_name(format!("={}", crate::error::INTERNAL_CHUNK))
        .call(clean)
}

pub fn guarded(guard: &Function, name: &str, function: Function) -> mlua::Result<Function> {
    guard.call((name, function))
}

pub fn plain_tree(lua: &Lua, table: &Table) -> mlua::Result<()> {
    let plain: Function = lua.named_registry_value(PLAIN_KEY)?;
    let entries: Vec<(LuaValue, LuaValue)> = table
        .pairs::<LuaValue, LuaValue>()
        .collect::<mlua::Result<_>>()?;
    for (key, value) in entries {
        match value {
            LuaValue::Function(function) => {
                table.raw_set(key, plain.call::<Function>(function)?)?;
            }
            LuaValue::Table(inner) if !matches!(&key, LuaValue::String(name) if name.as_bytes().as_ref() == b"args" || name.as_bytes().as_ref() == b"ctx") =>
            {
                plain_tree(lua, &inner)?;
            }
            _ => {}
        }
    }
    Ok(())
}
