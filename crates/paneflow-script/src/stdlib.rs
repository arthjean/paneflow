use std::path::{Path, PathBuf};

use mlua::{Lua, LuaOptions, StdLib, Table};

pub fn libraries() -> StdLib {
    StdLib::TABLE
        | StdLib::STRING
        | StdLib::UTF8
        | StdLib::MATH
        | StdLib::COROUTINE
        | StdLib::IO
        | StdLib::OS
        | StdLib::PACKAGE
}

pub fn new_state() -> mlua::Result<Lua> {
    Lua::new_with(libraries(), LuaOptions::new())
}

fn refusal(lua: &Lua, name: &'static str) -> mlua::Result<mlua::Function> {
    lua.create_function(move |lua, _: mlua::MultiValue| -> mlua::Result<()> {
        Err(crate::error::located(
            lua,
            format!(
                "{name} is not available in Paneflow scripts: run programs with paneflow.exec, which takes an argument list and never starts a shell"
            ),
        ))
    })
}

pub fn search_path(script_dir: Option<&Path>, lua_dir: Option<&Path>) -> String {
    let mut entries = Vec::new();
    for dir in [script_dir, lua_dir].into_iter().flatten() {
        let dir = dir.to_string_lossy();
        entries.push(format!("{dir}{}?.lua", std::path::MAIN_SEPARATOR));
        entries.push(format!(
            "{dir}{}?{}init.lua",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        ));
    }
    entries.join(";")
}

pub fn lua_dir() -> Option<PathBuf> {
    paneflow_home::lua_dir()
}

pub fn restrict(lua: &Lua, script_dir: Option<&Path>, lua_dir: Option<&Path>) -> mlua::Result<()> {
    let globals = lua.globals();
    let io: Table = globals.get("io")?;
    io.set("popen", refusal(lua, "io.popen")?)?;
    let os: Table = globals.get("os")?;
    os.set("execute", refusal(lua, "os.execute")?)?;
    let package: Table = globals.get("package")?;
    package.set("path", search_path(script_dir, lua_dir))?;
    package.set("cpath", "")?;
    Ok(())
}

const CONFIG_SEARCHER: &str = r#"
local root, record = ...
local searchpath, open, load, insert = package.searchpath, io.open, load, table.insert
local separator = package.config:sub(1, 1)
local prefix = root .. separator

local function chunk_name(path)
  if path:sub(1, #prefix) == prefix then
    return "@" .. path:sub(#prefix + 1)
  end
  return "@" .. path
end

local function search(name)
  local path, tried = searchpath(name, package.path)
  if not path then
    return "\n\t" .. tried
  end
  local file, message = open(path, "rb")
  if not file then
    error(message, 2)
  end
  local source = file:read("a")
  file:close()
  record(path)
  local loaded, problem = load(source, chunk_name(path), "t")
  if not loaded then
    error(problem, 0)
  end
  return loaded, path
end

insert(package.searchers, 2, search)
"#;

pub fn install_config_searcher(lua: &Lua, root: &Path) -> mlua::Result<()> {
    let record = lua.create_function(|lua, path: String| {
        let shared = crate::runtime::shared(lua)?;
        let path = PathBuf::from(path);
        let mut required = shared.required.borrow_mut();
        if !required.contains(&path) {
            required.push(path);
        }
        Ok(())
    })?;
    lua.load(CONFIG_SEARCHER)
        .set_name(format!("={}", crate::error::INTERNAL_CHUNK))
        .call::<()>((root.to_string_lossy().to_string(), record))
}
