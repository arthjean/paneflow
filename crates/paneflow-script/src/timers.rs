use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use mlua::{Function, Lua, Table, UserData, UserDataMethods};

use crate::error::{Halt, located};
use crate::executor::{TaskId, TaskKind, yield_now};
use crate::runtime::{Shared, context_table, shared};

pub const MAX_TIMERS: usize = 1024;

#[derive(Debug, Default)]
pub struct Timers {
    active: Cell<usize>,
}

impl Timers {
    pub fn active(&self) -> usize {
        self.active.get()
    }
}

pub fn seconds_label(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds.fract() == 0.0 {
        format!("{}", duration.as_secs())
    } else {
        format!("{seconds}")
    }
}

pub fn duration(lua: &Lua, what: &str, seconds: f64) -> mlua::Result<Duration> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(located(
            lua,
            format!(
                "{what}: the duration must be a number of seconds of zero or more, not {seconds}"
            ),
        ));
    }
    Duration::try_from_secs_f64(seconds)
        .map_err(|_| located(lua, format!("{what}: {seconds} seconds is too long")))
}

struct TimerState {
    active: Cell<bool>,
    running: Cell<bool>,
    counted: Cell<bool>,
    task: Cell<Option<TaskId>>,
}

impl TimerState {
    fn release(&self, shared: &Shared) {
        self.active.set(false);
        if self.counted.replace(false) {
            shared
                .timers
                .active
                .set(shared.timers.active.get().saturating_sub(1));
        }
    }
}

struct Timer {
    state: Rc<TimerState>,
}

impl UserData for Timer {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("cancel", |lua, timer, ()| {
            let shared = shared(lua)?;
            let was_active = timer.state.active.get();
            timer.state.release(&shared);
            if was_active
                && !timer.state.running.get()
                && let Some(task) = timer.state.task.get()
            {
                shared.executor.cancel(task);
            }
            Ok(was_active)
        });
        methods.add_method("active", |_, timer, ()| Ok(timer.state.active.get()));
    }
}

fn schedule(
    lua: &Lua,
    what: &str,
    delay: Duration,
    interval: Option<Duration>,
    function: Function,
) -> mlua::Result<Timer> {
    let shared = shared(lua)?;
    if shared.timers.active.get() >= MAX_TIMERS {
        let message = format!(
            "{}{what}: the script already holds {MAX_TIMERS} active timers, the most a script may hold; cancel timers it no longer needs",
            crate::error::location(lua)
        );
        shared.halt_with(Halt::Fatal(message.clone()));
        return Err(crate::error::halt(Halt::Fatal(message)));
    }
    shared.timers.active.set(shared.timers.active.get() + 1);
    let state = Rc::new(TimerState {
        active: Cell::new(true),
        running: Cell::new(false),
        counted: Cell::new(true),
        task: Cell::new(None),
    });
    let task_state = state.clone();
    let executor = shared.executor.clone();
    let start = executor.now();
    let weak = Rc::downgrade(&shared);
    let weak_lua = lua.weak();
    let task = shared.executor.spawn(TaskKind::Internal, async move {
        let mut deadline = start + delay;
        loop {
            executor.sleep_until(deadline).await;
            let Some(shared) = weak.upgrade() else {
                return;
            };
            if !task_state.active.get() || shared.halted() {
                break;
            }
            task_state.running.set(true);
            let tick_function = function.clone();
            let tick_shared = shared.clone();
            let tick_lua = weak_lua.clone();
            let tick = executor.spawn(TaskKind::Timer, async move {
                let lua = tick_lua.upgrade();
                let context = context_table(&lua, &tick_shared.context.borrow(), "timer");
                let result = match context {
                    Ok(context) => tick_function.call_async::<()>(context).await,
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    tick_shared.report_failure("a timer", &error);
                }
            });
            drop(shared);
            executor.join(tick).await;
            task_state.running.set(false);
            if !task_state.active.get() {
                break;
            }
            let Some(interval) = interval else {
                break;
            };
            let now = executor.now();
            deadline += interval;
            while deadline <= now {
                deadline += interval;
            }
        }
        if let Some(shared) = weak.upgrade() {
            task_state.release(&shared);
        }
    });
    state.task.set(Some(task));
    Ok(Timer { state })
}

pub fn install(lua: &Lua, paneflow: &Table, guard: &Function) -> mlua::Result<()> {
    let sleep = lua.create_async_function(|lua, seconds: f64| async move {
        let wait = duration(&lua, "paneflow.sleep", seconds)?;
        let shared = shared(&lua)?;
        let executor = shared.executor.clone();
        drop(shared);
        yield_now().await;
        executor.sleep_until(executor.now() + wait).await;
        Ok(())
    })?;
    paneflow.set(
        "sleep",
        crate::guard::guarded(guard, "paneflow.sleep", sleep)?,
    )?;
    paneflow.set(
        "after",
        lua.create_function(|lua, (seconds, function): (f64, Function)| {
            let delay = duration(lua, "paneflow.after", seconds)?;
            schedule(lua, "paneflow.after", delay, None, function)
        })?,
    )?;
    paneflow.set(
        "every",
        lua.create_function(|lua, (seconds, function): (f64, Function)| {
            let interval = duration(lua, "paneflow.every", seconds)?;
            if interval.is_zero() {
                return Err(located(
                    lua,
                    "paneflow.every: the interval must be more than 0 seconds",
                ));
            }
            schedule(lua, "paneflow.every", interval, Some(interval), function)
        })?,
    )?;
    Ok(())
}
