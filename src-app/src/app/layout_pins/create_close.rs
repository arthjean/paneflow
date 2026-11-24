use gpui::TestAppContext;
use paneflow_config::schema::{TabTitleSource, TerminalSurfaceProfile};
use serde_json::json;

use super::Rig;
use crate::app::workspace_ops::SurfaceLaunch;
use crate::layout::MAX_PANES;
use crate::pane::PaneEvent;
use crate::{
    ClosePane, FocusLeft, FocusRight, NewTab, NextTab, PreviousTab, SplitHorizontally,
    SplitVertically, ToggleZoom, UndoClosePane,
};

pub(super) const PINS: usize = 24;

fn split_v(rig: &mut Rig<'_>) {
    rig.act(|app, window, cx| app.handle_split_v(&SplitVertically, window, cx));
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_split_by_key_puts_the_new_pane_after_the_focused_one_and_focuses_it(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b)");
    assert_eq!(rig.focused().as_deref(), Some("b"));
    rig.focus("a");
    rig.act(|app, window, cx| app.handle_split_h(&SplitHorizontally, window, cx));
    assert_eq!(rig.shape(0, 0), "V(0.500 H(0.500 a, 0.500 c), 0.500 b)");
    assert_eq!(rig.focused().as_deref(), Some("c"));
    assert_eq!(rig.requests(), ["create a", "create b", "create c"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_split_by_ipc_spawns_next_to_the_anchor_and_leaves_focus(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    rig.requests();
    let anchor = rig.surface_id("a");
    let reply = rig.ipc(
        "surface.split",
        json!({"surface_id": anchor, "direction": "horizontal"}),
    );
    assert_eq!(reply["result"]["split"], json!(true), "{reply}");
    assert_eq!(rig.shape(0, 0), "H(0.500 a, 0.500 b)");
    assert_eq!(rig.focused().as_deref(), Some("a"));
    assert_eq!(rig.requests(), ["create b"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_split_is_refused_while_zoomed_and_at_the_pane_limit(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    rig.act(|app, window, cx| app.handle_toggle_zoom(&ToggleZoom, window, cx));
    rig.requests();
    split_v(&mut rig);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b)");
    assert_eq!(rig.toasts(), ["Unzoom before splitting panes"]);
    assert_eq!(rig.requests(), Vec::<String>::new());
    rig.act(|app, window, cx| app.handle_toggle_zoom(&ToggleZoom, window, cx));
    for _ in 2..MAX_PANES {
        split_v(&mut rig);
    }
    rig.requests();
    let before = rig.shape(0, 0);
    split_v(&mut rig);
    assert_eq!(rig.shape(0, 0), before);
    assert_eq!(rig.requests(), Vec::<String>::new());
    assert!(
        rig.toasts()
            .contains(&format!("Maximum pane count reached ({MAX_PANES})"))
    );
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_new_surface_joins_its_pane_and_becomes_active(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.requests();
    rig.emit("a", PaneEvent::NewTab);
    assert_eq!(rig.shape(0, 0), "a+b*");
    assert_eq!(rig.requests(), ["create b"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_new_tab_appends_and_is_shown(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.requests();
    rig.act(|app, window, cx| {
        app.open_tab_with_surface(
            0,
            "second".into(),
            TerminalSurfaceProfile::Normal,
            SurfaceLaunch::default(),
            window,
            cx,
        )
    });
    assert_eq!(rig.tabs(0), ["=a", "*second=b"]);
    assert_eq!(rig.focused().as_deref(), Some("b"));
    assert_eq!(rig.requests(), ["create b"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_new_workspace_from_a_folder_is_an_empty_shell_and_is_shown(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.requests();
    let folder = tempfile::tempdir().expect("a folder");
    let path = folder.path().to_path_buf();
    rig.act(|app, _, cx| app.open_workspace_folders(std::slice::from_ref(&path), cx));
    let title = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    assert_eq!(rig.workspaces(), ["alpha".to_string(), format!("*{title}")]);
    assert_eq!(rig.tabs(1), ["*=empty"]);
    assert_eq!(rig.requests(), Vec::<String>::new());
    let model_shown = rig
        .cx
        .update(|_, cx| rig.app.read(cx).layout.model().state().active_workspace);
    assert_eq!(model_shown, 1);
    rig.act(|app, _, cx| app.open_workspace_folders(std::slice::from_ref(&path), cx));
    assert_eq!(rig.workspaces().len(), 2);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_new_workspace_by_ipc_spawns_one_pane_and_stays_unselected(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.requests();
    let reply = rig.ipc("workspace.create", json!({"name": "gamma"}));
    assert_eq!(reply["result"]["title"], json!("gamma"), "{reply}");
    assert_eq!(rig.workspaces(), ["*alpha", "gamma"]);
    assert_eq!(rig.tabs(1), ["*=b"]);
    assert_eq!(rig.requests(), ["create b"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_close_by_key_holds_the_session_and_focuses_the_previous_sibling(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    split_v(&mut rig);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.250 b, 0.250 c)");
    rig.requests();
    rig.focus("b");
    rig.act(|app, window, cx| app.handle_close_pane(&ClosePane, window, cx));
    assert_eq!(rig.shape(0, 0), "V(0.625 a, 0.375 c)");
    assert_eq!(rig.focused().as_deref(), Some("a"));
    assert_eq!(rig.requests(), ["hold b"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_closing_the_last_pane_by_key_gives_the_tab_a_fresh_pane(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    rig.requests();
    rig.act(|app, window, cx| app.handle_close_pane(&ClosePane, window, cx));
    assert_eq!(rig.shape(0, 0), "b");
    assert_eq!(rig.focused().as_deref(), Some("b"));
    assert_eq!(rig.requests(), ["create b", "hold a"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_closing_a_zoomed_pane_by_key_leaves_zoom_and_focuses_the_first_pane(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    split_v(&mut rig);
    rig.focus("c");
    rig.act(|app, window, cx| app.handle_toggle_zoom(&ToggleZoom, window, cx));
    rig.requests();
    rig.act(|app, window, cx| app.handle_close_pane(&ClosePane, window, cx));
    assert_eq!(rig.shape(0, 0), "V(0.625 a, 0.375 b)");
    assert_eq!(rig.focused().as_deref(), Some("a"));
    assert_eq!(rig.requests(), ["hold c"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_closing_a_pane_from_its_header_stops_the_session_and_records_nothing(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    rig.requests();
    rig.emit("b", PaneEvent::CloseRequested);
    assert_eq!(rig.shape(0, 0), "a");
    assert_eq!(rig.requests(), ["stop b"]);
    rig.act(|app, window, cx| app.handle_undo_close_pane(&UndoClosePane, window, cx));
    assert_eq!(rig.shape(0, 0), "a");
    assert_eq!(rig.toasts(), ["No closed pane to restore"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_removing_the_last_pane_of_a_tab_spawns_a_fresh_one(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.requests();
    let pane = rig.pane("a");
    let terminal = rig.terminal("a");
    rig.act(|app, _, cx| {
        app.perform_close(
            crate::app::close_policy::CloseTarget::Surface { pane, terminal },
            crate::app::close_policy::CloseIntent::Stop,
            None,
            cx,
        );
    });
    assert_eq!(rig.shape(0, 0), "b");
    assert_eq!(rig.requests(), ["stop a", "create b"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_closing_a_surface_stops_it_and_keeps_its_neighbour(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.emit("a", PaneEvent::NewTab);
    rig.emit("a", PaneEvent::NewTab);
    assert_eq!(rig.shape(0, 0), "a+b+c*");
    rig.requests();
    let middle = rig.terminal("b");
    rig.emit("a", PaneEvent::CloseSurfaceRequested(middle));
    assert_eq!(rig.shape(0, 0), "a+c*");
    assert_eq!(rig.requests(), ["stop b"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_closing_a_tab_holds_one_surface_per_pane_and_shows_the_neighbour(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.act(|app, window, cx| {
        app.open_tab_with_surface(
            0,
            "second".into(),
            TerminalSurfaceProfile::Normal,
            SurfaceLaunch::default(),
            window,
            cx,
        )
    });
    split_v(&mut rig);
    rig.emit("c", PaneEvent::NewTab);
    assert_eq!(rig.tabs(0), ["=a", "*second=V(0.500 b, 0.500 c+d*)"]);
    rig.requests();
    rig.act(|app, window, cx| app.close_workspace_tab(0, 1, window, cx));
    assert_eq!(rig.tabs(0), ["*=a"]);
    assert_eq!(rig.focused().as_deref(), Some("a"));
    let mut requests = rig.requests();
    requests.sort();
    assert_eq!(requests, ["hold b", "hold d", "stop c"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_closing_a_workspace_stops_its_sessions_and_shows_the_next_one(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.workspace("beta");
    rig.workspace("gamma");
    rig.act(|app, window, cx| app.select_workspace(1, window, cx));
    rig.requests();
    rig.act(|app, window, cx| app.close_workspace_at(1, window, cx));
    assert_eq!(rig.workspaces(), ["alpha", "*gamma"]);
    assert_eq!(rig.focused().as_deref(), Some("c"));
    assert_eq!(rig.requests(), ["stop b"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_closing_a_workspace_by_ipc_stops_its_sessions_and_refuses_the_last_one(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.workspace("beta");
    rig.requests();
    let reply = rig.ipc("workspace.close", json!({"index": 0}));
    assert_eq!(reply["result"]["closed"], json!(0), "{reply}");
    assert_eq!(rig.workspaces(), ["*beta"]);
    assert_eq!(rig.requests(), ["stop a"]);
    let refused = rig.ipc("workspace.close", json!({"index": 0}));
    assert!(refused.get("error").is_some(), "{refused}");
    assert_eq!(rig.workspaces(), ["*beta"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_undo_within_the_hold_reattaches_next_to_the_first_pane(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    split_v(&mut rig);
    rig.focus("c");
    rig.act(|app, window, cx| app.handle_close_pane(&ClosePane, window, cx));
    assert_eq!(rig.shape(0, 0), "V(0.625 a, 0.375 b)");
    rig.requests();
    rig.act(|app, window, cx| app.handle_undo_close_pane(&UndoClosePane, window, cx));
    assert_eq!(rig.shape(0, 0), "V(0.625 H(0.500 a, 0.500 c), 0.375 b)");
    assert_eq!(rig.focused().as_deref(), Some("c"));
    assert_eq!(rig.requests(), ["reattach c"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_undo_after_the_hold_spawns_a_fresh_terminal(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    rig.requests();
    split_v(&mut rig);
    rig.act(|app, window, cx| app.handle_close_pane(&ClosePane, window, cx));
    rig.expire_holds();
    assert_eq!(rig.requests(), ["create b", "hold b", "stop b"]);
    rig.act(|app, window, cx| app.handle_undo_close_pane(&UndoClosePane, window, cx));
    assert_eq!(rig.shape(0, 0), "H(0.500 a, 0.500 c)");
    assert_eq!(rig.focused().as_deref(), Some("c"));
    assert_eq!(rig.requests(), ["create c"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_sixth_closed_pane_evicts_the_oldest_record_and_stops_its_session(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    let mut closed = Vec::new();
    for _ in 0..6 {
        rig.focus("a");
        split_v(&mut rig);
        let name = rig.focused().expect("the new pane is focused");
        rig.act(|app, window, cx| app.handle_close_pane(&ClosePane, window, cx));
        closed.push(name);
    }
    let requests = rig.requests();
    let last = &closed[5];
    let oldest = &closed[0];
    assert_eq!(
        requests[requests.len() - 2..],
        [format!("stop {oldest}"), format!("hold {last}")]
    );
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_defect_undo_of_a_pane_with_several_surfaces_restores_only_the_active_one(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    rig.emit("b", PaneEvent::NewTab);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b+c*)");
    rig.focus("c");
    rig.requests();
    rig.act(|app, window, cx| app.handle_close_pane(&ClosePane, window, cx));
    assert_eq!(rig.requests(), ["stop b", "hold c"]);
    rig.act(|app, window, cx| app.handle_undo_close_pane(&UndoClosePane, window, cx));
    assert_eq!(rig.shape(0, 0), "H(0.500 a, 0.500 c)");
    assert_eq!(rig.requests(), ["reattach c"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_an_ipc_split_anchored_on_a_background_surface_splits_its_pane(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.emit("a", PaneEvent::NewTab);
    assert_eq!(rig.shape(0, 0), "a+b*");
    rig.requests();
    let background = rig.surface_id("a");
    let reply = rig.ipc(
        "surface.split",
        json!({"surface_id": background, "direction": "vertical"}),
    );
    assert_eq!(reply["result"]["split"], json!(true), "{reply}");
    assert_eq!(rig.shape(0, 0), "V(0.500 a+b*, 0.500 c)");
    assert_eq!(rig.requests(), ["create c"]);
    let missing = rig.ipc(
        "surface.split",
        json!({"surface_id": 987_654_321u64, "direction": "vertical"}),
    );
    assert_eq!(
        missing["error"]["message"],
        json!("Surface not found"),
        "{missing}"
    );
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_focus_select_and_rename_move_no_session(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    rig.act(|app, window, cx| app.handle_focus_left(&FocusLeft, window, cx));
    assert_eq!(rig.focused().as_deref(), Some("a"));
    rig.act(|app, window, cx| app.handle_focus_right(&FocusRight, window, cx));
    assert_eq!(rig.focused().as_deref(), Some("b"));
    rig.act(|app, window, cx| {
        app.open_tab_with_surface(
            0,
            "second".into(),
            TerminalSurfaceProfile::Normal,
            SurfaceLaunch::default(),
            window,
            cx,
        )
    });
    rig.workspace("beta");
    rig.requests();
    rig.act(|app, window, cx| app.select_workspace_tab(0, 0, window, cx));
    assert_eq!(rig.workspaces(), ["*alpha", "beta"]);
    assert_eq!(rig.tabs(0), ["*=V(0.500 a, 0.500 b)", "second=c"]);
    assert_eq!(rig.focused().as_deref(), Some("a"));
    rig.act(|app, window, cx| app.select_workspace(1, window, cx));
    assert_eq!(rig.focused().as_deref(), Some("d"));
    rig.act(|app, _, cx| {
        app.begin_tab_rename(0, 1, cx);
        app.rename_input
            .update(cx, |input, cx| input.set_value("  renamed  ", cx));
        app.commit_rename(cx);
    });
    assert_eq!(rig.tabs(0), ["*=V(0.500 a, 0.500 b)", "renamed=c"]);
    let title_source = |rig: &mut Rig| {
        rig.cx.update(|_, cx| {
            let app = rig.app.read(cx);
            (
                app.layout.model().state().workspaces[0].tabs[1].title_source,
                app.workspaces[0].tabs()[1].title_source(),
            )
        })
    };
    assert_eq!(
        title_source(&mut rig),
        (TabTitleSource::User, TabTitleSource::User)
    );
    rig.act(|app, _, cx| app.reset_tab_name(0, 1, cx));
    assert_eq!(
        title_source(&mut rig),
        (TabTitleSource::Process, TabTitleSource::Process)
    );
    let surface = rig.surface_id("a");
    let reply = rig.ipc(
        "surface.rename",
        json!({"surface_id": surface, "name": "custom"}),
    );
    assert!(reply.get("result").is_some(), "{reply}");
    let custom = rig.state().workspaces[0].tabs[0]
        .layout
        .as_ref()
        .and_then(|layout| match layout {
            paneflow_config::schema::LayoutNode::Split { children, .. } => {
                children.first().cloned()
            }
            paneflow_config::schema::LayoutNode::Pane { .. } => None,
        })
        .and_then(|first| match first {
            paneflow_config::schema::LayoutNode::Pane { surfaces } => surfaces
                .first()
                .and_then(|surface| surface.custom_name.clone()),
            paneflow_config::schema::LayoutNode::Split { .. } => None,
        });
    assert_eq!(custom.as_deref(), Some("custom"));
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_returning_to_a_new_pane_tab_gives_its_launcher_the_keyboard(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    rig.act(|app, window, cx| app.handle_new_tab(&NewTab, window, cx));
    let launcher_focused = |rig: &mut Rig| {
        let app = rig.app.clone();
        rig.cx
            .update(|window, cx| app.read(cx).pane_palette_focus.is_focused(window))
    };
    assert!(launcher_focused(&mut rig));
    rig.act(|app, window, cx| app.handle_previous_tab(&PreviousTab, window, cx));
    assert_eq!(rig.focused().as_deref(), Some("a"));
    rig.act(|app, window, cx| app.handle_next_tab(&NextTab, window, cx));
    rig.cx.run_until_parked();
    assert!(
        launcher_focused(&mut rig),
        "the New pane launcher of the shown tab holds the keyboard"
    );
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_config_reload_to_system_mode_takes_the_variant_of_the_system_appearance(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    let config = paneflow_config::schema::PaneFlowConfig {
        theme: Some("Vercel Dark".to_string()),
        theme_mode: Some("system".to_string()),
        ..Default::default()
    };
    let theme = rig.act(move |app, _, cx| {
        *app.pending_config.lock().unwrap_or_else(|e| e.into_inner()) = Some(config);
        app.process_config_changes(cx);
        app.cached_config.theme.clone()
    });
    assert_eq!(
        theme.as_deref(),
        Some("Vercel Light"),
        "the test platform reports a light appearance"
    );
}
