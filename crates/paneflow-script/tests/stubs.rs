#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::Value;

const STUBS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../protocol/paneflow.d.lua");

fn descriptions() -> Vec<Value> {
    paneflow_api::catalog()
        .unwrap()
        .iter()
        .map(paneflow_api::Descriptor::describe)
        .collect()
}

#[test]
fn the_checked_in_lua_stubs_match_the_registry() {
    let generated = paneflow_script::stubs::lua_stubs(&descriptions());
    if std::env::var("PANEFLOW_API_SNAPSHOT").as_deref() == Ok("write") {
        std::fs::write(STUBS_PATH, &generated).expect("protocol/paneflow.d.lua is writable");
    }
    let checked_in = std::fs::read_to_string(STUBS_PATH).unwrap_or_default();
    assert!(
        checked_in == generated,
        "protocol/paneflow.d.lua differs from the method registry. Regenerate it with \
         `PANEFLOW_API_SNAPSHOT=write cargo test -p paneflow-script --test stubs` and review its diff."
    );
}

#[test]
fn the_stubs_declare_every_method_and_every_runtime_function() {
    let stubs = paneflow_script::stubs::lua_stubs(&descriptions());
    assert!(stubs.starts_with("---@meta"));
    for descriptor in paneflow_api::catalog().unwrap().iter() {
        let name = descriptor.name();
        assert!(
            stubs.contains(&format!("paneflow.{name} = function(params) end")),
            "{name}"
        );
    }
    for runtime in [
        "function paneflow.call(",
        "function paneflow.on(",
        "function paneflow.wait(",
        "function paneflow.sleep(",
        "function paneflow.stop(",
        "function paneflow.after(",
        "function paneflow.every(",
        "paneflow.exec = {}",
        "function paneflow.exec.start(",
        "paneflow.json = {}",
        "function paneflow.log(",
        "paneflow.layout = {}",
        "paneflow.args = {}",
        "paneflow.ctx = {",
        "paneflow.action = {}",
        "function paneflow.invoke(",
        "function paneflow.bind(",
        "function paneflow.unbind(",
        "function paneflow.bindings(",
        "function paneflow.client.queue(",
    ] {
        assert!(stubs.contains(runtime), "{runtime}");
    }
}

#[test]
fn the_stub_file_is_valid_lua_that_loads() {
    let stubs = paneflow_script::stubs::lua_stubs(&descriptions());
    let lua = paneflow_script::stdlib::new_state().unwrap();
    lua.load(stubs.as_str())
        .set_name("@paneflow.d.lua")
        .exec()
        .expect("the stub file runs as Lua");
    let defined: bool = lua
        .load("return type(paneflow.surface.read) == 'function' and type(paneflow.layout.pane) == 'function'")
        .eval()
        .unwrap();
    assert!(defined);
}

#[test]
fn the_markdown_reference_has_a_section_per_method() {
    let reference = paneflow_script::stubs::markdown_reference(&descriptions());
    for descriptor in paneflow_api::catalog().unwrap().iter() {
        assert!(
            reference.contains(&format!("\n## `{}`\n", descriptor.name())),
            "{}",
            descriptor.name()
        );
    }
    assert!(reference.contains("### Parameters"));
    assert!(reference.contains("### Result"));
    assert!(
        reference.contains("local result = paneflow.surface.read{"),
        "{reference}"
    );
}
