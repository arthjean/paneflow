use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

use super::forget_the_previous_pin;
use crate::PaneFlowApp;
use crate::app::bootstrap::Startup;
use crate::app::fake_host::FakeHost;
use crate::terminal::TerminalView;
use crate::terminal::view::DisplayOnlyTerminals;
use crate::workspace::{Workspace, next_workspace_id_in};

pub(super) const PINS: usize = 5;

const PATIENCE: Duration = Duration::from_secs(60);

const INIT_LUA: &str = "local last = {}\n\
paneflow.action{ name = 'new_branch', title = 'New branch', run = function(ctx)\n\
  local name, reason = paneflow.ui.input{ title = 'New branch', value = 'feature/' }\n\
  last = { name = name, reason = reason, surface = ctx.surface_id }\n\
  if name then paneflow.surface.send_text{ text = 'git switch -c ' .. name, submit = true } end\n\
end }\n\
paneflow.action{ name = 'choose', title = 'Choose', run = function()\n\
  local picked, reason = paneflow.ui.pick{ title = 'Projects', multiple = true, items = {\n\
    'api', { id = 'web', title = 'Website', subtitle = 'Next.js', icon = 'globe', selected = true },\n\
    { id = 'docs', icon = 'rocket' } } }\n\
  last = { picked = picked, reason = reason }\n\
end }\n\
paneflow.action{ name = 'last', title = 'Last', run = function() return last end }\n\
paneflow.bind('ctrl-alt-shift-b', 'new_branch')\n";

fn endpoint(home: &Path) -> PathBuf {
    paneflow_home::serve_endpoint_path(home)
}

fn call(home: &Path, method: &str, params: Value) -> Value {
    HostControl::connect(&endpoint(home), "layout-pin-prompts")
        .expect("the worker answers")
        .request_with_deadline(method, params, Duration::from_secs(120))
        .unwrap_or_else(|error| panic!("{method}: {error}"))
}

fn last(home: &Path) -> Value {
    call(home, "action.invoke", json!({"name": "last"}))["result"].clone()
}

fn invoke_later(home: &Path, name: &str, ctx: Value) -> std::thread::JoinHandle<Value> {
    let home = home.to_path_buf();
    let params = json!({"name": name, "ctx": ctx});
    std::thread::spawn(move || call(&home, "action.invoke", params))
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

fn pump(app: &Entity<PaneFlowApp>, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.process_lua_config_frames(cx);
            app.process_prompt_frames(cx);
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
}

fn pump_until(
    app: &Entity<PaneFlowApp>,
    cx: &mut VisualTestContext,
    what: &str,
    done: impl Fn(&PaneFlowApp, &gpui::App) -> bool,
) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        pump(app, cx);
        if cx.update(|_, cx| done(app.read(cx), cx)) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn start(
    cx: &mut TestAppContext,
) -> (
    Entity<PaneFlowApp>,
    &mut VisualTestContext,
    PathBuf,
    paneflow_serve::worker::RunningWorker,
) {
    let home = paneflow_home::paneflow_home().expect("the isolated home");
    std::fs::write(paneflow_home::init_lua_path_in(&home), INIT_LUA).expect("init.lua");
    let (app, cx) = mount(cx);
    let worker = paneflow_serve::open(&home).expect("a worker in the isolated home");
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.start_lua_config_stream();
            app.start_prompt_stream();
        })
    });
    pump_until(&app, cx, "the actions reaching the window", |app, _| {
        app.lua_config.actions.len() == 3
    });
    (app, cx, home, worker)
}

fn finish(cx: &mut VisualTestContext, worker: paneflow_serve::worker::RunningWorker, home: &Path) {
    worker.stop();
    cx.run_until_parked();
    let _ = std::fs::remove_file(paneflow_home::init_lua_path_in(home));
    if let Some(session) = paneflow_config::loader::session_path() {
        let _ = std::fs::remove_file(session);
    }
}

fn shown(app: &Entity<PaneFlowApp>, cx: &mut VisualTestContext) -> Option<(String, String)> {
    cx.update(|_, cx| app.read(cx).shown_prompt_title(cx))
}

fn rows(app: &Entity<PaneFlowApp>, cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|_, cx| app.read(cx).shown_prompt_rows())
}

fn typed(app: &Entity<PaneFlowApp>, cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx| app.read(cx).prompt_input.read(cx).value())
}

fn type_into_prompt(app: &Entity<PaneFlowApp>, cx: &mut VisualTestContext, text: &str) {
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let text = text.to_string();
            app.prompt_input
                .update(cx, |input, cx| input.set_value(text, cx));
        })
    });
    pump(app, cx);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_key_bound_action_asks_in_the_window_and_runs_with_the_typed_text(cx: &mut TestAppContext) {
    let (app, cx, home, worker) = start(cx);
    let surface = cx.update(|window, cx| {
        app.read(cx)
            .focused_invoke_context("key", window, cx)
            .surface_id
    });
    cx.simulate_keystrokes("ctrl-alt-shift-b");
    pump_until(&app, cx, "the prompt opening", |app, _| {
        app.prompts.shown.is_some()
    });
    let granted = cx.update(|_, cx| app.read(cx).lua_config.grants.live_count());
    assert_eq!(granted, 1, "a key press mints one grant for its run");
    assert_eq!(
        shown(&app, cx),
        Some((
            "New branch".to_string(),
            "Asked by the action New branch".to_string()
        ))
    );
    assert_eq!(typed(&app, cx), "feature/");
    assert_eq!(rows(&app, cx), ["Use \u{201c}feature/\u{201d}"]);
    let input_focused = cx.update(|window, cx| {
        app.read(cx)
            .prompt_input
            .read(cx)
            .focus_handle
            .is_focused(window)
    });
    assert!(input_focused, "the field takes the keyboard");
    let attention = cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.attention_items(cx)
                .into_iter()
                .filter(|item| item.surface_id == surface)
                .count()
        })
    });
    assert_eq!(attention, 1, "the waiting prompt is in the attention queue");

    type_into_prompt(&app, cx, "fix/login");
    assert_eq!(rows(&app, cx), ["Use \u{201c}fix/login\u{201d}"]);
    cx.simulate_keystrokes("enter");
    pump_until(&app, cx, "the prompt closing", |app, _| {
        app.prompts.shown.is_none()
    });
    let deadline = Instant::now() + PATIENCE;
    while last(&home)["name"] != "fix/login" {
        pump(&app, cx);
        assert!(Instant::now() < deadline, "the action never got the answer");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(last(&home)["surface"], json!(surface));
    pump_until(&app, cx, "the grant ending with its run", |app, _| {
        app.lua_config.grants.live_count() == 0
    });
    let attention = cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.attention_items(cx)
                .into_iter()
                .filter(|item| item.surface_id == surface)
                .count()
        })
    });
    assert_eq!(attention, 0, "an answered prompt leaves the queue");

    cx.update(|window, cx| app.update(cx, |app, cx| app.open_command_palette(window, cx)));
    let asking = invoke_later(&home, "new_branch", json!({"origin": "api"}));
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        pump(&app, cx);
        assert!(
            shown(&app, cx).is_none(),
            "a prompt never replaces the open palette"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(cx.update(|_, cx| app.read(cx).command_palette_is_open()));
    cx.update(|window, cx| app.update(cx, |app, cx| app.close_command_palette(window, cx)));
    pump_until(&app, cx, "the prompt after the palette closed", |app, _| {
        app.prompts.shown.is_some()
    });
    cx.simulate_keystrokes("escape");
    asking.join().expect("the cancelled run returns");
    assert_eq!(last(&home)["reason"], "cancelled");
    pump(&app, cx);
    assert!(shown(&app, cx).is_none());

    let asking = invoke_later(&home, "new_branch", json!({"origin": "api"}));
    pump_until(&app, cx, "the prompt answered elsewhere", |app, _| {
        app.prompts.shown.is_some()
    });
    let id = cx
        .update(|_, cx| app.read(cx).prompts.shown_id())
        .expect("an id");
    call(
        &home,
        "prompt.reply",
        json!({"id": id, "answer": "from-another-client"}),
    );
    pump_until(&app, cx, "the ended frame closing the prompt", |app, _| {
        app.prompts.shown.is_none() && app.prompts.pending.is_empty()
    });
    asking.join().expect("the run returns");
    assert_eq!(last(&home)["name"], "from-another-client");
    finish(cx, worker, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_choice_prompt_filters_off_the_render_thread_and_sends_the_ticked_ids(
    cx: &mut TestAppContext,
) {
    let (app, cx, home, worker) = start(cx);
    let asking = invoke_later(&home, "choose", json!({"origin": "palette"}));
    pump_until(&app, cx, "the choice prompt", |app, _| {
        app.prompts.shown.is_some()
    });
    assert_eq!(rows(&app, cx), ["api", "Website", "docs", "Confirm"]);
    type_into_prompt(&app, cx, "web");
    pump_until(&app, cx, "the filtered rows", |app, _| {
        !app.shown_prompt_filtering() && app.shown_prompt_rows().len() == 2
    });
    assert_eq!(rows(&app, cx), ["Website", "Confirm"]);
    type_into_prompt(&app, cx, "");
    pump_until(&app, cx, "the full list again", |app, _| {
        app.shown_prompt_rows().len() == 4
    });
    cx.update(|_, cx| app.update(cx, |app, cx| app.activate_prompt_row(0, cx)));
    cx.simulate_keystrokes("down down down enter");
    asking.join().expect("the run returns");
    assert_eq!(last(&home)["picked"], json!(["api", "web"]));
    pump(&app, cx);
    assert!(shown(&app, cx).is_none());
    finish(cx, worker, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_activating_a_prompt_from_the_attention_queue_shows_it_before_an_older_one(
    cx: &mut TestAppContext,
) {
    let (app, cx, home, worker) = start(cx);
    let surface = cx
        .update(|window, cx| {
            app.read(cx)
                .focused_invoke_context("key", window, cx)
                .surface_id
        })
        .expect("a focused terminal");
    let older = invoke_later(&home, "choose", json!({"origin": "api"}));
    pump_until(&app, cx, "the older prompt", |app, _| {
        app.prompts.shown.is_some()
    });
    assert_eq!(
        shown(&app, cx).map(|(title, _)| title).as_deref(),
        Some("Projects")
    );
    let newer = invoke_later(
        &home,
        "new_branch",
        json!({"origin": "api", "surface_id": surface}),
    );
    pump_until(&app, cx, "the second prompt arriving", |app, _| {
        app.prompts.pending.len() == 2
    });
    assert_eq!(
        shown(&app, cx).map(|(title, _)| title).as_deref(),
        Some("Projects"),
        "the older prompt stays shown"
    );
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.activate_attention_surface(surface, window, cx)
        })
    });
    pump_until(&app, cx, "the activated prompt", |app, cx| {
        app.shown_prompt_title(cx)
            .is_some_and(|(title, _)| title == "New branch")
    });
    cx.simulate_keystrokes("escape");
    newer.join().expect("the newer run returns");
    pump_until(&app, cx, "the older prompt coming back", |app, cx| {
        app.shown_prompt_title(cx)
            .is_some_and(|(title, _)| title == "Projects")
    });
    cx.simulate_keystrokes("escape");
    older.join().expect("the older run returns");
    assert_eq!(last(&home)["reason"], "cancelled");
    finish(cx, worker, &home);
}

fn agent_asks_later(home: &Path, title: &str) -> std::thread::JoinHandle<Value> {
    let home = home.to_path_buf();
    let params = json!({
        "kind": "input",
        "title": title,
        "origin": {"kind": "agent", "agent": "claude", "session": "a9e52660-0000-4000-8000-000000000001"},
        "wait_ms": 25_000
    });
    std::thread::spawn(move || call(&home, "prompt.request", params))
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_an_agent_question_is_named_plain_and_waits_behind_the_users_own_prompt(
    cx: &mut TestAppContext,
) {
    let (app, cx, home, worker) = start(cx);
    let first = agent_asks_later(&home, "Deploy\u{202e}gnp.exe\u{7} to\nproduction?");
    pump_until(&app, cx, "the agent's question", |app, _| {
        app.prompts.shown.is_some()
    });
    assert_eq!(
        shown(&app, cx),
        Some((
            "Deploygnp.exe to\nproduction?".to_string(),
            "Asked by Claude Code".to_string()
        ))
    );
    let second = agent_asks_later(&home, "Second agent question");
    pump_until(&app, cx, "the second agent question", |app, _| {
        app.prompts.pending.len() == 2
    });
    let users = invoke_later(&home, "new_branch", json!({"origin": "api"}));
    pump_until(&app, cx, "the user's own prompt", |app, _| {
        app.prompts.pending.len() == 3
    });
    cx.simulate_keystrokes("escape");
    assert_eq!(
        first.join().expect("the agent waited")["status"],
        "cancelled"
    );
    pump_until(&app, cx, "the user's prompt shown first", |app, cx| {
        app.shown_prompt_title(cx)
            .is_some_and(|(title, _)| title == "New branch")
    });
    let (_, origin) = shown(&app, cx).expect("shown");
    assert_eq!(origin, "Asked by the action New branch");
    cx.simulate_keystrokes("escape");
    users.join().expect("the user's run returns");
    pump_until(&app, cx, "the older agent question last", |app, cx| {
        app.shown_prompt_title(cx)
            .is_some_and(|(title, _)| title == "Second agent question")
    });
    cx.simulate_keystrokes("escape");
    assert_eq!(
        second.join().expect("the agent waited")["status"],
        "cancelled"
    );
    finish(cx, worker, &home);
}

fn send_text_with(
    app: &Entity<PaneFlowApp>,
    cx: &mut VisualTestContext,
    surface: Option<u64>,
    grant: Option<String>,
) -> Value {
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let request = crate::ipc::IpcRequest {
        method: "surface.send_text".to_string(),
        params: json!({"surface_id": surface, "text": "ls"}),
        response_tx,
        state: std::sync::Arc::new(crate::ipc::RequestState::default()),
        caller_pid: None,
        caller: paneflow_api::Principal::Window,
        grant,
        gates: paneflow_api::Gates::CLOSED,
    };
    cx.update(|_, cx| app.update(cx, |app, cx| app.process_ipc_requests(vec![request], cx)));
    cx.run_until_parked();
    response_rx
        .recv_timeout(PATIENCE)
        .expect("surface.send_text answers")
}

fn refused_by_the_write_gate(reply: &Value) -> bool {
    let promoted = crate::app::ipc_handler::promote_response(reply.clone(), json!(1));
    promoted["error"]["code"] == -32601
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_only_a_grant_the_window_minted_and_still_holds_opens_the_write_gate(
    cx: &mut TestAppContext,
) {
    let (app, cx) = mount(cx);
    let surface = cx.update(|window, cx| {
        app.read(cx)
            .focused_invoke_context("key", window, cx)
            .surface_id
    });
    let grant =
        cx.update(|_, cx| app.update(cx, |app, _| app.lua_config.grants.mint(Instant::now())));
    let admitted = send_text_with(&app, cx, surface, Some(grant.clone()));
    assert!(
        !refused_by_the_write_gate(&admitted),
        "a live grant passes the write gate: {admitted}"
    );
    for (what, other) in [
        ("no grant", None),
        (
            "a forged grant",
            Some("0123456789abcdef0123456789abcdef".to_string()),
        ),
    ] {
        let reply = send_text_with(&app, cx, surface, other);
        assert!(refused_by_the_write_gate(&reply), "{what}: {reply}");
    }
    cx.update(|_, cx| app.update(cx, |app, _| app.lua_config.grants.revoke(&grant)));
    let revoked = send_text_with(&app, cx, surface, Some(grant));
    assert!(
        refused_by_the_write_gate(&revoked),
        "a revoked grant: {revoked}"
    );
}
