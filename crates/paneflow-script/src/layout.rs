use mlua::{Lua, LuaSerdeExt, Table, Value as LuaValue};

use crate::error::located;

const SURFACE_KEYS: &[&str] = &[
    "surface_type",
    "name",
    "custom_name",
    "command",
    "prompt",
    "cwd",
    "path",
    "env",
    "focus",
    "scrollback",
    "agent",
    "font_size",
];

fn pane(lua: &Lua, spec: Option<Table>) -> mlua::Result<Table> {
    let node = lua.create_table()?;
    node.set("type", "pane")?;
    let surfaces = lua.create_table()?;
    surfaces.set_metatable(Some(lua.array_metatable()))?;
    if let Some(spec) = spec {
        let listed: Option<Table> = spec.get("surfaces")?;
        match listed {
            Some(listed) => {
                for (index, surface) in listed.sequence_values::<LuaValue>().enumerate() {
                    surfaces.raw_set(index + 1, surface?)?;
                }
            }
            None => {
                let surface = lua.create_table()?;
                for pair in spec.pairs::<String, LuaValue>() {
                    let (key, value) = pair?;
                    if !SURFACE_KEYS.contains(&key.as_str()) {
                        return Err(located(
                            lua,
                            format!(
                                "paneflow.layout.pane: unknown field '{key}'; a pane takes surfaces or one of {}",
                                SURFACE_KEYS.join(", ")
                            ),
                        ));
                    }
                    surface.set(key, value)?;
                }
                if !surface.is_empty() {
                    surfaces.raw_set(1, surface)?;
                }
            }
        }
    }
    node.set("surfaces", surfaces)?;
    Ok(node)
}

fn split(
    lua: &Lua,
    direction: &str,
    ratio: f64,
    first: Table,
    second: Table,
) -> mlua::Result<Table> {
    if !(ratio > 0.0 && ratio < 1.0) {
        return Err(located(
            lua,
            format!("paneflow.layout.{direction}: the ratio must be between 0 and 1, not {ratio}"),
        ));
    }
    for (position, child) in [(1, &first), (2, &second)] {
        let kind: Option<String> = child.get("type")?;
        if !matches!(kind.as_deref(), Some("pane" | "split")) {
            return Err(located(
                lua,
                format!(
                    "paneflow.layout.{direction}: child {position} is not a layout node; build it with paneflow.layout.pane, horizontal or vertical"
                ),
            ));
        }
    }
    let node = lua.create_table()?;
    node.set("type", "split")?;
    node.set("direction", direction)?;
    let ratios = lua.create_sequence_from([ratio, 1.0 - ratio])?;
    node.set("ratios", ratios)?;
    let children = lua.create_sequence_from([first, second])?;
    node.set("children", children)?;
    Ok(node)
}

pub fn install(lua: &Lua, paneflow: &Table) -> mlua::Result<()> {
    let layout = lua.create_table()?;
    layout.set(
        "pane",
        lua.create_function(|lua, spec: Option<Table>| pane(lua, spec))?,
    )?;
    layout.set(
        "horizontal",
        lua.create_function(|lua, (ratio, first, second): (f64, Table, Table)| {
            split(lua, "horizontal", ratio, first, second)
        })?,
    )?;
    layout.set(
        "vertical",
        lua.create_function(|lua, (ratio, first, second): (f64, Table, Table)| {
            split(lua, "vertical", ratio, first, second)
        })?,
    )?;
    paneflow.set("layout", layout)
}
