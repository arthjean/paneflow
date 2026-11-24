#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mlua::{Lua, Value};
use paneflow_script::executor::{Executor, TaskKind};
use paneflow_script::{FakeClock, Wake};

struct Probe {
    lua: Lua,
    executor: Rc<Executor>,
}

fn probe() -> Probe {
    let lua = paneflow_script::stdlib::new_state().unwrap();
    let executor = Executor::new(Arc::new(FakeClock::new()), Arc::new(Wake::default()));
    let sleeper = executor.clone();
    let wait = lua
        .create_async_function(move |_, milliseconds: u64| {
            let sleep = sleeper.sleep_until(sleeper.now() + Duration::from_millis(milliseconds));
            async move {
                sleep.await;
                Ok(milliseconds)
            }
        })
        .unwrap();
    lua.globals().set("wait", wait).unwrap();
    Probe { lua, executor }
}

impl Probe {
    fn run(&self, sources: &[&str]) -> Vec<mlua::Result<Value>> {
        let results = Rc::new(RefCell::new(vec![None; sources.len()]));
        for (index, source) in sources.iter().enumerate() {
            let function = self.lua.load(*source).into_function().unwrap();
            let slot = results.clone();
            self.executor.spawn(TaskKind::TopLevel, async move {
                let result = function.call_async::<Value>(()).await;
                slot.borrow_mut()[index] = Some(result);
            });
        }
        self.executor.run(&|| false);
        Rc::try_unwrap(results)
            .ok()
            .unwrap()
            .into_inner()
            .into_iter()
            .map(|result| result.expect("every chunk finished"))
            .collect()
    }

    fn one(&self, source: &str) -> mlua::Result<Value> {
        self.run(&[source]).remove(0)
    }

    fn elapsed(&self) -> Duration {
        self.executor.now()
    }
}

#[test]
fn a_direct_async_call_suspends_and_returns_its_value() {
    let probe = probe();
    let value = probe.one("return wait(200)").unwrap();
    assert_eq!(value, Value::Integer(200));
    assert_eq!(probe.elapsed(), Duration::from_millis(200));
}

#[test]
fn a_nested_async_call_returns_through_lua_frames() {
    let probe = probe();
    let value = probe
        .one("local function twice() return wait(100) + wait(100) end return twice()")
        .unwrap();
    assert_eq!(value, Value::Integer(200));
}

#[test]
fn an_async_call_under_pcall_suspends_on_lua_5_4() {
    let probe = probe();
    let value = probe
        .one("local ok, value = pcall(wait, 200) assert(ok, value) return value")
        .unwrap();
    assert_eq!(value, Value::Integer(200));
}

#[test]
fn an_async_call_inside_an_index_metamethod_suspends_on_lua_5_4() {
    let probe = probe();
    let value = probe
        .one(
            "local t = setmetatable({}, { __index = function(_, key) return wait(200) end }) return t.anything",
        )
        .unwrap();
    assert_eq!(value, Value::Integer(200));
}

#[test]
fn two_pending_calls_and_a_computation_share_the_same_200_ms() {
    let probe = probe();
    let results = probe.run(&[
        "return wait(200)",
        "return wait(200)",
        "local sum = 0 for i = 1, 100000 do sum = sum + i end return sum",
    ]);
    assert_eq!(results[0].as_ref().unwrap(), &Value::Integer(200));
    assert_eq!(results[1].as_ref().unwrap(), &Value::Integer(200));
    assert_eq!(results[2].as_ref().unwrap(), &Value::Integer(5_000_050_000));
    assert_eq!(probe.elapsed(), Duration::from_millis(200));
}

#[test]
fn an_async_call_inside_a_sort_comparator_fails_at_the_c_boundary() {
    let probe = probe();
    let error = probe
        .one("local t = {3, 1, 2} table.sort(t, function(a, b) wait(1) return a < b end) return t[1]")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("attempt to yield across a C-call boundary"),
        "{error}"
    );
}

#[test]
fn an_async_call_inside_a_script_coroutine_leaks_an_internal_value_to_the_script() {
    let probe = probe();
    let value = probe
        .one(
            "local co = coroutine.create(function() return wait(200) end) local ok, value = coroutine.resume(co) return tostring(ok) .. ' ' .. type(value)",
        )
        .unwrap();
    assert_eq!(
        value.to_string().unwrap(),
        "true userdata",
        "mlua no longer leaks its pending marker into a script coroutine: the guard of US-033 may be simplified"
    );
}
