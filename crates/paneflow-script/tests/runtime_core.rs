#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use paneflow_script::{Level, Outcome};
use serde_json::json;
use support::{FakeBackend, failed, harness, harness_with, returned, run};

#[test]
fn the_documented_standard_libraries_are_loaded_and_debug_is_not() {
    let value = returned(run(
        "return { base = type(print), table = type(table), string = type(string), utf8 = type(utf8), \
         math = type(math), coroutine = type(coroutine), io = type(io), os = type(os), \
         package = type(package), debug = type(debug) }",
    ));
    for library in [
        "table",
        "string",
        "utf8",
        "math",
        "coroutine",
        "io",
        "os",
        "package",
    ] {
        assert_eq!(value[library], "table", "{library}");
    }
    assert_eq!(value["base"], "function");
    assert_eq!(value["debug"], "nil");
}

#[test]
fn c_modules_cannot_be_loaded() {
    let value = returned(run(
        "local ok = pcall(package.loadlib, 'libc.so.6', 'puts') \
         return { loadlib = ok, cpath = package.cpath, searchers = #package.searchers }",
    ));
    assert_eq!(value["loadlib"], false);
    assert_eq!(value["cpath"], "");
    let message = failed(run("return require('definitely_not_a_c_module_here')"));
    assert!(
        message.contains("definitely_not_a_c_module_here"),
        "{message}"
    );
}

#[test]
fn popen_and_execute_raise_an_error_that_names_paneflow_exec() {
    for call in ["io.popen('ls')", "os.execute('ls')"] {
        let message = failed(run(call));
        assert!(message.contains("paneflow.exec"), "{call}: {message}");
        assert!(message.contains("script.lua:1:"), "{call}: {message}");
    }
}

#[test]
fn require_searches_the_script_directory_then_the_home_lua_directory() {
    let script_dir = tempfile::tempdir().unwrap();
    let lua_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        script_dir.path().join("near.lua"),
        "return 'from the script directory'",
    )
    .unwrap();
    std::fs::write(
        lua_dir.path().join("shared.lua"),
        "return 'from <home>/lua'",
    )
    .unwrap();
    std::fs::create_dir(lua_dir.path().join("pkg")).unwrap();
    std::fs::write(
        lua_dir.path().join("pkg").join("init.lua"),
        "return 'a package'",
    )
    .unwrap();
    let script = script_dir.path().to_path_buf();
    let home_lua = lua_dir.path().to_path_buf();
    let harness = harness_with(FakeBackend::new(), move |options| {
        options.script_dir = Some(script);
        options.lua_dir = Some(home_lua);
    });
    let value = returned(harness.run(
        "return { near = require('near'), shared = require('shared'), pkg = require('pkg') }",
    ));
    assert_eq!(
        value,
        json!({"near": "from the script directory", "shared": "from <home>/lua", "pkg": "a package"})
    );
}

#[test]
fn the_home_lua_directory_is_resolved_by_paneflow_home() {
    let expected = paneflow_home::paneflow_home().map(|home| home.join("lua"));
    assert_eq!(paneflow_script::stdlib::lua_dir(), expected);
}

#[test]
fn json_sequences_become_arrays_and_every_other_table_an_object() {
    let value = returned(run("return { \
           list = paneflow.json.encode({ 1, 2, 3 }), \
           empty = paneflow.json.encode({}), \
           holes = paneflow.json.encode({ [1] = 'a', [3] = 'c' }), \
           mixed = paneflow.json.encode({ 1, x = 2 }), \
           integer = paneflow.json.encode(3), \
           float = paneflow.json.encode(3.5) }"));
    assert_eq!(value["list"], "[1,2,3]");
    assert_eq!(value["empty"], "{}");
    assert_eq!(value["holes"], r#"{"1":"a","3":"c"}"#);
    assert_eq!(value["mixed"], r#"{"1":1,"x":2}"#);
    assert_eq!(value["integer"], "3");
    assert_eq!(value["float"], "3.5");
}

#[test]
fn json_decode_maps_null_to_nil_and_keeps_integers_integers() {
    let value = returned(run(
        "local t = paneflow.json.decode('{\"a\": null, \"n\": 3, \"f\": 2.5, \"list\": [1, 2]}') \
         return { a_is_nil = t.a == nil, n_type = math.type(t.n), f_type = math.type(t.f), \
                  list = #t.list, again = paneflow.json.encode(paneflow.json.decode('[]')) }",
    ));
    assert_eq!(value["a_is_nil"], true);
    assert_eq!(value["n_type"], "integer");
    assert_eq!(value["f_type"], "float");
    assert_eq!(value["list"], 2);
    assert_eq!(value["again"], "[]");
}

#[test]
fn encode_returns_nil_and_a_message_for_a_function_or_a_cycle() {
    let value = returned(run("local f, fm = paneflow.json.encode({ f = print }) \
         local t = {} t.self = t \
         local c, cm = paneflow.json.encode(t) \
         local bad, bm = paneflow.json.decode('{oops') \
         return { f = f == nil, fm = fm, c = c == nil, cm = cm, bad = bad == nil, bm = bm }"));
    assert_eq!(value["f"], true);
    assert!(
        value["fm"].as_str().unwrap().contains("function"),
        "{value}"
    );
    assert_eq!(value["c"], true);
    assert!(
        value["cm"].as_str().unwrap().contains("contains itself"),
        "{value}"
    );
    assert_eq!(value["bad"], true);
    assert!(
        value["bm"].as_str().unwrap().contains("invalid JSON"),
        "{value}"
    );
}

#[test]
fn log_takes_an_optional_level_that_defaults_to_info() {
    let harness = harness(FakeBackend::new());
    let outcome = harness.run(
        "paneflow.log('plain') paneflow.log('warn', 'careful') paneflow.log('debug', 42) return true",
    );
    assert_eq!(outcome, Outcome::Returned(Some(json!(true))));
    assert_eq!(
        harness.sink.logs(),
        [
            (Level::Info, "plain".to_string()),
            (Level::Warn, "careful".to_string()),
            (Level::Debug, "42".to_string()),
        ]
    );
    let message = failed(run("paneflow.log('loud', 'x')"));
    assert!(message.contains("unknown level 'loud'"), "{message}");
    assert!(message.contains("script.lua:1:"), "{message}");
}

#[test]
fn a_lua_error_carries_the_file_name_and_the_line() {
    let message = failed(run("local x = 1\nlocal y = nil\nreturn y.field"));
    assert!(message.starts_with("script.lua:3:"), "{message}");
    let message = failed(run("\n\nerror('plain failure')"));
    assert_eq!(message, "script.lua:3: plain failure");
    let message = failed(run("paneflow.every(0, function() end)"));
    assert!(message.starts_with("script.lua:1:"), "{message}");
}

#[test]
fn the_crate_links_neither_gpui_nor_the_app() {
    let manifest = include_str!("../Cargo.toml");
    for banned in ["gpui", "paneflow-app", "src-app"] {
        assert!(!manifest.contains(banned), "{banned}");
    }
}
