use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

use super::forget_the_previous_pin;
use crate::PaneFlowApp;
use crate::app::bootstrap::Startup;
use crate::app::fake_host::FakeHost;
use crate::app::ipc_handler::{IpcReply, promote_response};
use crate::terminal::TerminalView;
use crate::terminal::view::DisplayOnlyTerminals;
use crate::workspace::{Workspace, next_workspace_id_in};

pub(super) const PINS: usize = 3;

const PATIENCE: Duration = Duration::from_secs(60);

const INIT_LUA: &str = "local seen = { calls = 0 }\n\
paneflow.action{ name = 'record', title = 'Record the call', category = 'Dev',\n\
  description = 'Remember who asked', keywords = {'memo'}, args = { file = 'string?' },\n\
  run = function(ctx, args)\n\
    seen = { calls = seen.calls + 1, origin = ctx.origin, tab = ctx.tab_id, surface = ctx.surface_id, file = args.file }\n\
  end }\n\
paneflow.action{ name = 'seen', title = 'Seen', category = 'Dev', run = function() return seen end }\n\
paneflow.action{ name = 'needs_file', title = 'Needs a file', category = 'Dev', args = { file = 'string' }, run = function() end }\n\
paneflow.bind('ctrl-alt-shift-k', 'record', { file = 'app.log' })\n";

fn call(home: &Path, method: &str, params: Value) -> Value {
    HostControl::connect(&paneflow_home::serve_endpoint_path(home), "layout-pin-lua")
        .expect("the worker answers")
        .request_with_deadline(method, params, Duration::from_secs(60))
        .unwrap_or_else(|error| panic!("{method}: {error}"))
}

fn seen(home: &Path) -> Value {
    call(home, "action.invoke", json!({"name": "seen"}))["result"].clone()
}

fn mount(cx: &mut TestAppContext) -> (Entity<PaneFlowApp>, &mut VisualTestContext) {
    forget_the_previous_pin();
    cx.executor().allow_parking();
    cx.update(|cx| {
        cx.set_global(DisplayOnlyTerminals);
        cx.set_global(FakeHost::default());
    });
    let (app, cx) = cx.add_window_view(|_, cx| PaneFlowApp::start(Startup::Conformance, cx));
    cx.run_until_parked();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let ws_id = next_workspace_id_in(None);
            let terminal = cx.new(|cx| TerminalView::new(ws_id, cx));
            let pane = app.create_pane(terminal, ws_id, cx);
            let idx = app.add_workspace_in(Workspace::with_id(ws_id, "alpha", pane), None, cx);
            app.select_workspace_through_model(idx, cx);
            app.focus_active_pane(window, cx);
        });
    });
    cx.run_until_parked();
    (app, cx)
}

fn pump_until(
    app: &Entity<PaneFlowApp>,
    cx: &mut VisualTestContext,
    what: &str,
    done: impl Fn(&PaneFlowApp, &gpui::App) -> bool,
) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        cx.update(|_, cx| app.update(cx, |app, cx| app.process_lua_config_frames(cx)));
        cx.run_until_parked();
        if cx.update(|_, cx| done(app.read(cx), cx)) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for(what: &str, cx: &mut VisualTestContext, done: impl Fn() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        cx.run_until_parked();
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn entries(app: &Entity<PaneFlowApp>, cx: &mut VisualTestContext) -> Vec<(String, Option<String>)> {
    cx.update(|_, cx| app.read(cx).command_palette_entries(cx))
}

fn open_palette(app: &Entity<PaneFlowApp>, cx: &mut VisualTestContext, query: &str) {
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            if !app.command_palette_is_open() {
                app.open_command_palette(window, cx);
            }
            app.command_palette_filter(query, cx);
        })
    });
    cx.run_until_parked();
}

fn close_palette(app: &Entity<PaneFlowApp>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| app.update(cx, |app, cx| app.close_command_palette(window, cx)));
    cx.run_until_parked();
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_init_lua_actions_join_the_palette_and_run_from_it(cx: &mut TestAppContext) {
    let home = paneflow_home::paneflow_home().expect("the isolated home");
    let (app, cx) = mount(cx);

    open_palette(&app, cx, "");
    let built_in = entries(&app, cx);
    open_palette(&app, cx, "split");
    let built_in_split = entries(&app, cx);
    let options = cx.update(|_, cx| app.read(cx).command_palette_exposed_options(cx));
    assert_eq!(options.len(), built_in_split.len());
    assert!(!options.is_empty());
    for (idx, (node, (label, _))) in options.iter().zip(&built_in_split).enumerate() {
        assert_eq!(node.role(), gpui::accesskit::Role::ListBoxOption, "{label}");
        assert_eq!(node.label(), Some(label.as_str()));
        assert_eq!(node.position_in_set(), Some(idx + 1), "{label}");
        assert_eq!(node.size_of_set(), Some(built_in_split.len()), "{label}");
        assert_eq!(node.is_selected(), Some(idx == 0), "{label}");
        assert!(
            node.supports_action(gpui::accesskit::Action::Click),
            "{label}"
        );
    }
    close_palette(&app, cx);

    let worker = paneflow_serve::open(&home).expect("a worker in the isolated home");
    cx.update(|_, cx| app.update(cx, |app, _| app.start_lua_config_stream()));
    pump_until(&app, cx, "the window reaching the worker", |app, _| {
        app.lua_config_reachable()
    });
    open_palette(&app, cx, "");
    assert_eq!(
        entries(&app, cx),
        built_in,
        "no init.lua leaves the palette as it was"
    );
    open_palette(&app, cx, "split");
    assert_eq!(entries(&app, cx), built_in_split);
    close_palette(&app, cx);

    worker.stop();
    pump_until(
        &app,
        cx,
        "the window noticing the worker leaving",
        |app, _| !app.lua_config_reachable(),
    );
    std::fs::write(paneflow_home::init_lua_path_in(&home), INIT_LUA).expect("init.lua");
    let worker = paneflow_serve::open(&home).expect("a worker that finds init.lua");
    pump_until(
        &app,
        cx,
        "the actions of init.lua reaching the window",
        |app, _| !app.lua_config.actions.is_empty(),
    );

    let (tab, surface) = cx.update(|window, cx| {
        let context = app.read(cx).focused_invoke_context("palette", window, cx);
        (context.tab_id, context.surface_id)
    });
    assert!(
        tab.is_some() && surface.is_some(),
        "a focused terminal gives its ids"
    );

    open_palette(&app, cx, "");
    let browsing = entries(&app, cx);
    assert!(
        browsing.contains(&("Dev".to_string(), None)),
        "{browsing:?}"
    );
    assert_eq!(browsing.len(), built_in.len() + 1);
    for query in ["memo", "remember who", "record", "Record the"] {
        open_palette(&app, cx, query);
        assert!(
            entries(&app, cx).contains(&("Record the call".to_string(), Some("Dev".to_string()))),
            "{query} must find the action: {:?}",
            entries(&app, cx)
        );
    }

    open_palette(&app, cx, "memo");
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.command_palette_choose("Record the call", window, cx)
        })
    });
    assert!(!cx.update(|_, cx| app.read(cx).command_palette_is_open()));
    wait_for("the palette run of record", cx, || {
        seen(&home)["calls"] == 1
    });
    let recorded = seen(&home);
    assert_eq!(recorded["origin"], "palette");
    assert_eq!(recorded["tab"], json!(tab));
    assert_eq!(recorded["surface"], json!(surface));

    open_palette(&app, cx, "needs a file");
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.command_palette_choose("Needs a file", window, cx)
        })
    });
    pump_until(&app, cx, "the missing-argument toast", |app, _| {
        app.toast_messages()
            .iter()
            .any(|message| message.starts_with("Needs a file: missing argument \"file\""))
    });

    cx.update(|window, cx| app.update(cx, |app, cx| app.focus_active_pane(window, cx)));
    cx.simulate_keystrokes("ctrl-alt-shift-k");
    wait_for("the key run of record", cx, || seen(&home)["calls"] == 2);
    let recorded = seen(&home);
    assert_eq!(recorded["origin"], "key");
    assert_eq!(recorded["file"], "app.log");

    worker.stop();
    pump_until(&app, cx, "the window noticing the outage", |app, _| {
        !app.lua_config_reachable()
    });
    open_palette(&app, cx, "");
    assert_eq!(
        entries(&app, cx),
        built_in,
        "an unreachable worker leaves the built-in entries"
    );
    close_palette(&app, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.focus_active_pane(window, cx)));
    cx.simulate_keystrokes("ctrl-alt-shift-k");
    cx.simulate_keystrokes("ctrl-alt-shift-k");
    cx.run_until_parked();
    let outage = cx.update(|_, cx| {
        app.read(cx)
            .toast_messages()
            .into_iter()
            .filter(|message| message == crate::app::lua_config::SCRIPTING_UNAVAILABLE)
            .count()
    });
    assert_eq!(outage, 1, "one outage toast per outage");
    let _ = std::fs::remove_file(paneflow_home::init_lua_path_in(&home));
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_broken_init_lua_shows_one_toast_with_its_line(cx: &mut TestAppContext) {
    let home = paneflow_home::paneflow_home().expect("the isolated home");
    std::fs::write(
        paneflow_home::init_lua_path_in(&home),
        "paneflow.action{ name = 'ok', title = 'Ok', run = function() end }\nlocal x = = 1\n",
    )
    .expect("init.lua");
    let (app, cx) = mount(cx);
    let worker = paneflow_serve::open(&home).expect("a worker in the isolated home");
    cx.update(|_, cx| app.update(cx, |app, _| app.start_lua_config_stream()));
    pump_until(&app, cx, "the load error toast", |app, _| {
        !app.toast_messages().is_empty()
    });
    let count = |app: &Entity<PaneFlowApp>, cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            app.read(cx)
                .toast_messages()
                .into_iter()
                .filter(|message| message.starts_with("init.lua:2: "))
                .count()
        })
    };
    assert_eq!(
        count(&app, cx),
        1,
        "{:?}",
        cx.update(|_, cx| app.read(cx).toast_messages())
    );

    call(&home, "config.reload", json!({}));
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        cx.update(|_, cx| app.update(cx, |app, cx| app.process_lua_config_frames(cx)));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        count(&app, cx),
        1,
        "the same error twice queues no second toast"
    );

    std::fs::write(
        paneflow_home::init_lua_path_in(&home),
        "paneflow.action{ name = 'ok', title = 'Ok', run = function() end }\n\
         paneflow.action{ name = 'ok', title = 'Ok again', run = function() end }\n",
    )
    .expect("init.lua");
    call(&home, "config.reload", json!({}));
    pump_until(&app, cx, "the warning count toast", |app, _| {
        app.toast_messages()
            .iter()
            .any(|message| message == "init.lua: 1 warning")
    });
    worker.stop();
    let _ = std::fs::remove_file(paneflow_home::init_lua_path_in(&home));
}

fn ipc(
    app: &Entity<PaneFlowApp>,
    cx: &mut VisualTestContext,
    method: &str,
    params: Value,
) -> Value {
    let reply = cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.handle_ipc(method, &params, None, paneflow_api::Gates::CLOSED, cx)
        })
    });
    let slot: Rc<RefCell<Option<Value>>> = Rc::default();
    match reply {
        IpcReply::Ready(value) => *slot.borrow_mut() = Some(value),
        IpcReply::Deferred(job) => *slot.borrow_mut() = Some(job()),
        IpcReply::Async(task) => {
            let sink = slot.clone();
            cx.update(|_, cx| cx.spawn(async move |_| *sink.borrow_mut() = Some(task.await)))
                .detach();
        }
    }
    let answered = slot.clone();
    wait_for(method, cx, move || answered.borrow().is_some());
    let answer = slot.borrow_mut().take().unwrap_or(Value::Null);
    promote_response(answer, Value::from(1))
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_the_window_lists_and_dispatches_its_built_in_actions(cx: &mut TestAppContext) {
    let (app, cx) = mount(cx);
    let tabs = |app: &Entity<PaneFlowApp>, cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let app = app.read(cx);
            app.workspaces[app.active_idx].tabs().len()
        })
    };

    let listed = ipc(&app, cx, "client.list_actions", json!({}));
    let actions = listed["result"]["actions"]
        .as_array()
        .unwrap_or_else(|| panic!("{listed}"))
        .clone();
    assert_eq!(actions.len(), crate::keybindings::builtin_actions().len());
    let new_tab = actions
        .iter()
        .find(|action| action["name"] == "new_tab")
        .expect("new_tab is listed");
    assert!(
        new_tab["description"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    );
    assert!(
        new_tab["group"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    );

    let before = tabs(&app, cx);
    let reply = ipc(
        &app,
        cx,
        "client.dispatch_action",
        json!({"name": "new_tab"}),
    );
    assert_eq!(
        reply["result"],
        json!({"dispatched": true, "name": "new_tab"})
    );
    cx.run_until_parked();
    assert_eq!(tabs(&app, cx), before + 1, "the built-in action ran");

    let refused = ipc(
        &app,
        cx,
        "client.dispatch_action",
        json!({"name": "no_such_action"}),
    );
    assert_eq!(refused["error"]["code"], -32602, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("no_such_action")),
        "{refused}"
    );
}
