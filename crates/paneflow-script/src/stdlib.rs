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
