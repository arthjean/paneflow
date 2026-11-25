use gpui::TestAppContext;
use paneflow_config::schema::TerminalSurfaceProfile;
use serde_json::json;

use super::Rig;
use crate::agent_sessions::SessionAgent;
use crate::app::drag::TabDrag;
use crate::app::workspace_ops::SurfaceLaunch;
use crate::pane::PaneEvent;
use crate::pane_drag::DropEdge;
use crate::{
    LayoutEvenHorizontal, LayoutEvenVertical, LayoutMainVertical, LayoutTiled, SplitEqualize,
    SplitHorizontally, SplitVertically, ToggleZoom,
};

pub(super) const PINS: usize = 30;

fn split_v(rig: &mut Rig<'_>) {
    rig.act(|app, window, cx| app.handle_split_v(&SplitVertically, window, cx));
}

fn three_panes(rig: &mut Rig<'_>) {
    rig.workspace("alpha");
    rig.focus("a");
    split_v(rig);
    split_v(rig);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.250 b, 0.250 c)");
    rig.requests();
}

fn open_tab(rig: &mut Rig<'_>, ws: usize, title: &str) {
    rig.act(|app, window, cx| {
        app.open_tab_with_surface(
            ws,
            title.into(),
            TerminalSurfaceProfile::Normal,
            SurfaceLaunch::default(),
            window,
            cx,
        )
    });
}

fn tab_drag(rig: &mut Rig<'_>, ws: usize, tab: usize) -> TabDrag {
    rig.act(|app, _, _| TabDrag {
        workspace_id: app.workspaces[ws].id,
        tab_id: app.workspaces[ws].tabs()[tab].id,
        title: app.workspaces[ws].tabs()[tab].title().to_string().into(),
    })
}

fn zoom(rig: &mut Rig<'_>, name: &str) {
    rig.focus(name);
    rig.act(|app, window, cx| app.handle_toggle_zoom(&ToggleZoom, window, cx));
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_dropping_a_pane_on_a_pane_centre_swaps_them_and_focuses_the_source(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    three_panes(&mut rig);
    let source = rig.pane_id("a");
    rig.emit(
        "c",
        PaneEvent::DropPaneMove {
            source_pane_id: source,
            edge: None,
        },
    );
    assert_eq!(rig.shape(0, 0), "V(0.500 c, 0.250 b, 0.250 a)");
    assert_eq!(rig.focused().as_deref(), Some("a"));
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_dropping_a_pane_on_an_edge_moves_it_beside_the_target(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    three_panes(&mut rig);
    let source = rig.pane_id("a");
    rig.emit(
        "c",
        PaneEvent::DropPaneMove {
            source_pane_id: source,
            edge: Some(DropEdge::Up),
        },
    );
    assert_eq!(rig.shape(0, 0), "V(0.500 b, 0.500 H(0.500 a, 0.500 c))");
    assert_eq!(rig.focused().as_deref(), Some("a"));
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_pane_drop_on_a_zoomed_tab_finds_no_hidden_source_and_changes_nothing(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    three_panes(&mut rig);
    zoom(&mut rig, "a");
    let source = rig.pane_id("b");
    rig.emit(
        "a",
        PaneEvent::DropPaneMove {
            source_pane_id: source,
            edge: None,
        },
    );
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.250 b, 0.250 c)");
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_dropping_a_surface_joins_the_target_pane_or_splits_it(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    rig.emit("b", PaneEvent::NewTab);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b+c*)");
    rig.requests();
    let source = rig.pane_id("b");
    let surface = rig.drag_surface_id("c");
    rig.emit(
        "a",
        PaneEvent::DropSurfaceMove {
            source_pane_id: source,
            surface_id: surface,
            edge: None,
        },
    );
    assert_eq!(rig.shape(0, 0), "V(0.500 a+c*, 0.500 b)");
    assert_eq!(rig.focused().as_deref(), Some("c"));
    let source = rig.pane_id("a");
    let surface = rig.drag_surface_id("c");
    rig.emit(
        "b",
        PaneEvent::DropSurfaceMove {
            source_pane_id: source,
            surface_id: surface,
            edge: Some(DropEdge::Down),
        },
    );
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 H(0.500 b, 0.500 c))");
    assert_eq!(rig.focused().as_deref(), Some("c"));
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_surface_dropped_on_a_full_pane_is_refused_with_the_tab_limit_toast(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    for _ in 1..crate::pane::MAX_PANE_TABS {
        rig.emit("b", PaneEvent::NewTab);
    }
    let full = rig.shape(0, 0);
    rig.toasts();
    let source = rig.pane_id("a");
    let surface = rig.drag_surface_id("a");
    rig.emit(
        "b",
        PaneEvent::DropSurfaceMove {
            source_pane_id: source,
            surface_id: surface,
            edge: None,
        },
    );
    assert_eq!(rig.shape(0, 0), full);
    assert_eq!(
        rig.toasts(),
        [format!(
            "Maximum tab count reached ({})",
            crate::pane::MAX_PANE_TABS
        )]
    );
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_surface_drop_on_a_zoomed_tab_is_refused(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    rig.emit("a", PaneEvent::NewTab);
    zoom(&mut rig, "c");
    rig.requests();
    let source = rig.pane_id("c");
    let surface = rig.drag_surface_id("c");
    rig.emit(
        "b",
        PaneEvent::DropSurfaceMove {
            source_pane_id: source,
            surface_id: surface,
            edge: None,
        },
    );
    assert_eq!(rig.shape(0, 0), "V(0.500 a+c*, 0.500 b)");
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_dropping_an_agent_session_splits_at_the_edge_or_opens_a_tab(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.requests();
    let agent = SessionAgent::all().next().expect("an agent with sessions");
    rig.emit(
        "a",
        PaneEvent::DropSessionSplit {
            edge: Some(DropEdge::Right),
            agent,
            session_id: "abc123".into(),
            cwd: String::new(),
        },
    );
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b)");
    assert_eq!(rig.focused().as_deref(), Some("b"));
    rig.emit(
        "a",
        PaneEvent::DropSessionSplit {
            edge: None,
            agent,
            session_id: "abc124".into(),
            cwd: String::new(),
        },
    );
    assert_eq!(rig.tabs(0).len(), 2);
    assert_eq!(rig.focused().as_deref(), Some("c"));
    assert_eq!(rig.requests(), ["create b", "create c"]);
    zoom(&mut rig, "c");
    rig.act(|app, window, cx| app.select_workspace_tab(0, 0, window, cx));
    zoom(&mut rig, "a");
    rig.requests();
    rig.emit(
        "a",
        PaneEvent::DropSessionSplit {
            edge: Some(DropEdge::Left),
            agent,
            session_id: "abc125".into(),
            cwd: String::new(),
        },
    );
    assert_eq!(rig.toasts(), ["Unzoom before splitting panes"]);
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_moving_a_pane_to_a_new_tab_leaves_zoom_and_rehomes_across_workspaces(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    three_panes(&mut rig);
    zoom(&mut rig, "a");
    let pane = rig.pane_id("c");
    rig.act(|app, window, cx| app.move_pane_to_new_tab(pane, 0, 1, window, cx));
    assert_eq!(rig.tabs(0), ["=V(0.625 a, 0.375 b)", "*=c"]);
    assert_eq!(rig.focused().as_deref(), Some("c"));
    assert_eq!(rig.requests(), ["assign c"]);
    rig.workspace("beta");
    rig.requests();
    let pane = rig.pane_id("b");
    rig.act(|app, window, cx| app.move_pane_to_new_tab(pane, 1, 1, window, cx));
    assert_eq!(rig.tabs(0), ["=a", "*=c"]);
    assert_eq!(rig.tabs(1), ["=d", "*=b"]);
    assert_eq!(rig.requests(), ["assign b"]);
    let only = rig.pane_id("c");
    rig.act(|app, window, cx| app.move_pane_to_new_tab(only, 0, 0, window, cx));
    assert_eq!(rig.tabs(0), ["=a", "*=c"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_moving_a_tab_to_another_workspace_rehomes_and_shows_it(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    open_tab(&mut rig, 0, "second");
    rig.workspace("beta");
    rig.requests();
    let drag = tab_drag(&mut rig, 0, 1);
    rig.act(|app, window, cx| app.move_tab_to_workspace(&drag, 1, 0, window, cx));
    assert_eq!(rig.tabs(0), ["*=a"]);
    assert_eq!(rig.tabs(1), ["*second=b", "=c"]);
    assert_eq!(rig.workspaces(), ["alpha", "*beta"]);
    assert_eq!(rig.focused().as_deref(), Some("b"));
    assert_eq!(rig.requests(), ["assign b"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_moving_the_last_tab_out_leaves_an_empty_workspace(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.workspace("beta");
    rig.requests();
    let drag = tab_drag(&mut rig, 0, 0);
    rig.act(|app, window, cx| app.move_tab_to_workspace(&drag, 1, 1, window, cx));
    assert_eq!(rig.tabs(0), ["*=empty"]);
    assert_eq!(rig.tabs(1), ["=b", "*=a"]);
    assert_eq!(rig.requests(), ["assign a"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_reordering_tabs_moves_within_the_strip_and_ignores_an_index_past_the_end(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    open_tab(&mut rig, 0, "two");
    open_tab(&mut rig, 0, "three");
    rig.requests();
    let drag = tab_drag(&mut rig, 0, 0);
    rig.act(|app, _, cx| app.reorder_workspace_tab(&drag, 0, 9, cx));
    assert_eq!(rig.tabs(0), ["=a", "two=b", "*three=c"]);
    rig.act(|app, _, cx| app.reorder_workspace_tab(&drag, 0, 3, cx));
    assert_eq!(rig.tabs(0), ["two=b", "*three=c", "=a"]);
    let drag = tab_drag(&mut rig, 0, 2);
    rig.act(|app, _, cx| app.reorder_workspace_tab(&drag, 0, 0, cx));
    assert_eq!(rig.tabs(0), ["=a", "two=b", "*three=c"]);
    assert_eq!(rig.focused().as_deref(), Some("c"));
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_reordering_workspaces_keeps_the_shown_one_and_clamps_past_the_end(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.workspace("beta");
    rig.workspace("gamma");
    rig.focus("c");
    rig.requests();
    let alpha = rig.act(|app, _, _| app.workspaces[0].id);
    rig.act(|app, _, cx| app.reorder_workspace(alpha, 2, cx));
    assert_eq!(rig.workspaces(), ["beta", "*gamma", "alpha"]);
    let beta = rig.act(|app, _, _| app.workspaces[0].id);
    rig.act(|app, _, cx| app.reorder_workspace(beta, 9, cx));
    assert_eq!(rig.workspaces(), ["*gamma", "alpha", "beta"]);
    assert_eq!(rig.focused().as_deref(), Some("c"));
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_equalize_evens_the_ratios_and_is_a_no_op_while_zoomed(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    three_panes(&mut rig);
    zoom(&mut rig, "b");
    rig.act(|app, window, cx| app.handle_split_equalize(&SplitEqualize, window, cx));
    rig.act(|app, window, cx| app.handle_toggle_zoom(&ToggleZoom, window, cx));
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.250 b, 0.250 c)");
    rig.act(|app, window, cx| app.handle_split_equalize(&SplitEqualize, window, cx));
    assert_eq!(rig.shape(0, 0), "V(0.333 a, 0.333 b, 0.333 c)");
    assert_eq!(rig.focused().as_deref(), Some("b"));
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_each_preset_rebuilds_the_tree_and_focuses_as_today(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    three_panes(&mut rig);
    rig.act(|app, window, cx| app.handle_layout_even_v(&LayoutEvenVertical, window, cx));
    assert_eq!(rig.shape(0, 0), "H(0.333 a, 0.333 b, 0.333 c)");
    assert_eq!(rig.focused().as_deref(), Some("a"));
    rig.act(|app, window, cx| app.handle_layout_tiled(&LayoutTiled, window, cx));
    assert_eq!(rig.shape(0, 0), "H(0.500 V(0.500 a, 0.500 b), 0.500 c)");
    assert_eq!(rig.focused().as_deref(), Some("a"));
    rig.focus("c");
    rig.act(|app, window, cx| app.handle_layout_main_v(&LayoutMainVertical, window, cx));
    assert_eq!(rig.shape(0, 0), "V(0.500 c, 0.500 H(0.500 a, 0.500 b))");
    assert_eq!(rig.focused().as_deref(), Some("c"));
    rig.act(|app, window, cx| app.handle_layout_even_h(&LayoutEvenHorizontal, window, cx));
    assert_eq!(rig.shape(0, 0), "V(0.333 c, 0.333 a, 0.333 b)");
    assert_eq!(rig.focused().as_deref(), Some("c"));
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_preset_on_a_zoomed_tab_leaves_zoom_first(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    three_panes(&mut rig);
    zoom(&mut rig, "b");
    rig.act(|app, window, cx| app.handle_layout_even_v(&LayoutEvenVertical, window, cx));
    assert_eq!(rig.shape(0, 0), "H(0.333 a, 0.333 b, 0.333 c)");
    let zoomed = rig.act(|app, _, _| app.workspaces[0].is_zoomed());
    assert!(!zoomed);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_zoom_shows_one_pane_keeps_the_saved_tree_and_needs_two_panes(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    zoom(&mut rig, "a");
    let zoomed = rig.act(|app, _, _| app.workspaces[0].is_zoomed());
    assert!(!zoomed);
    split_v(&mut rig);
    rig.requests();
    zoom(&mut rig, "a");
    let zoomed = rig.act(|app, _, _| app.workspaces[0].is_zoomed());
    assert!(zoomed);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b)");
    assert_eq!(rig.focused().as_deref(), Some("a"));
    rig.act(|app, window, cx| app.handle_toggle_zoom(&ToggleZoom, window, cx));
    let zoomed = rig.act(|app, _, _| app.workspaces[0].is_zoomed());
    assert!(!zoomed);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b)");
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_restore_layout_keeps_the_first_panes_and_stops_the_surplus(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    three_panes(&mut rig);
    let reply = rig.ipc(
        "workspace.restore_layout",
        json!({"layout": {"type": "split", "direction": "horizontal", "ratios": [0.3, 0.7],
            "children": [{"type": "pane", "surfaces": [{}]}, {"type": "pane", "surfaces": [{}]}]}}),
    );
    assert_eq!(reply["result"]["restored"], json!(true), "{reply}");
    assert_eq!(rig.shape(0, 0), "H(0.300 a, 0.700 b)");
    assert_eq!(rig.requests(), ["stop c"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_restore_layout_with_more_leaves_spawns_the_missing_ones(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.requests();
    let reply = rig.ipc(
        "workspace.restore_layout",
        json!({"layout": {"type": "split", "direction": "vertical",
            "children": [{"type": "pane", "surfaces": [{}]}, {"type": "pane", "surfaces": [{}]}]}}),
    );
    assert_eq!(reply["result"]["restored"], json!(true), "{reply}");
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b)");
    assert_eq!(rig.requests(), ["create b"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_surface_name_of_a_restored_layout_names_the_pane_it_opens(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.requests();
    let reply = rig.ipc(
        "workspace.restore_layout",
        json!({"layout": {"type": "split", "direction": "vertical", "children": [
            {"type": "pane", "surfaces": [{"name": "kept-pane"}]},
            {"type": "pane", "surfaces": [{"name": "  rl-logs  "}]},
            {"type": "pane", "surfaces": [{"name": "title", "custom_name": "chosen"}]},
            {"type": "pane", "surfaces": [{}]}]}}),
    );
    assert_eq!(reply["result"]["restored"], json!(true), "{reply}");
    assert_eq!(rig.requests(), ["create b", "create c", "create d"]);
    let names: Vec<Option<String>> = ["a", "b", "c", "d"]
        .into_iter()
        .map(|pane| {
            let terminal = rig.terminal(pane);
            rig.cx
                .read(|cx| terminal.read(cx).terminal.custom_name.clone())
        })
        .collect();
    assert_eq!(
        names,
        [
            None,
            Some("rl-logs".to_string()),
            Some("chosen".to_string()),
            None
        ],
        "a reused pane keeps its name, a new one takes the layout's name, a custom name wins"
    );
}

fn wait_for_typed(rig: &mut Rig<'_>, name: &str, expected: &str) {
    let terminal = rig.terminal(name);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let queued = rig
            .cx
            .read(|cx| terminal.read(cx).terminal.queued_input_text());
        if queued.contains(expected) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{expected:?} was never typed into {name}: {queued:?}"
        );
        rig.cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_command_and_a_prompt_of_a_layout_are_typed_into_the_panes_it_opens(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.requests();
    let reply = rig.ipc_with(
        "workspace.restore_layout",
        json!({"layout": {"type": "split", "direction": "vertical", "children": [
            {"type": "pane", "surfaces": [{}]},
            {"type": "pane", "surfaces": [{"command": "echo restored-by-layout"}]},
            {"type": "pane", "surfaces": [{"prompt": "prefilled-by-layout"}]}]}}),
        paneflow_api::Gates::OPEN,
    );
    assert_eq!(reply["result"]["restored"], json!(true), "{reply}");
    assert_eq!(rig.requests(), ["create b", "create c"]);
    wait_for_typed(&mut rig, "b", "echo restored-by-layout\r");
    wait_for_typed(&mut rig, "c", "prefilled-by-layout");
    let created = rig.ipc_with(
        "workspace.create",
        json!({"name": "made", "layout": {"type": "pane",
            "surfaces": [{"command": "echo created-by-layout"}]}}),
        paneflow_api::Gates::OPEN,
    );
    assert!(created["result"]["index"].is_u64(), "{created}");
    assert_eq!(rig.requests(), ["create d"]);
    wait_for_typed(&mut rig, "d", "echo created-by-layout\r");
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_template_closes_the_tab_it_replaces_and_is_refused_while_zoomed(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    rig.requests();
    let params = json!({"layout": "even_v", "panes": [{"label": "one"}, {"label": "two"}]});
    zoom(&mut rig, "a");
    let refused =
        rig.act(|app, _, cx| app.launch_workspace_params_in_open_workspace(&params, 0, cx));
    assert_eq!(
        refused,
        Err("unzoom the project before running the template here".to_string())
    );
    rig.act(|app, window, cx| app.handle_toggle_zoom(&ToggleZoom, window, cx));
    let launched =
        rig.act(|app, _, cx| app.launch_workspace_params_in_open_workspace(&params, 0, cx));
    assert_eq!(launched, Ok(()));
    assert_eq!(rig.tabs(0).len(), 1);
    assert_eq!(rig.shape(0, 0), "H(0.500 c, 0.500 d)");
    assert_eq!(rig.focused().as_deref(), Some("c"));
    assert_eq!(rig.requests(), ["create c", "create d", "hold a", "hold b"]);
    assert!(
        rig.toasts()
            .iter()
            .any(|toast| toast.starts_with("Tab closed")),
        "{:?}",
        rig.toasts()
    );
    rig.expire_holds();
    assert_eq!(rig.requests(), ["stop a", "stop b"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_template_launched_from_settings_opens_its_own_workspace_beside_the_project(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    rig.requests();
    let project = tempfile::tempdir().expect("a project folder");
    let cwd = project.path().to_string_lossy().into_owned();
    let template: paneflow_config::schema::CommandDefinition = serde_json::from_value(json!({
        "name": "qa-tpl",
        "workspace": {"name": "qa-space", "cwd": cwd, "layout_preset": "even_v",
            "layout": {"type": "split", "direction": "horizontal", "children": [
                {"type": "pane", "surfaces": [{"name": "t-one"}]},
                {"type": "pane", "surfaces": [{"name": "t-two"}]}]}}
    }))
    .expect("a template");
    rig.act(|app, window, cx| {
        app.workspaces[0].cwd.clone_from(&cwd);
        app.cached_config.commands = vec![template];
        app.launch_workspace_template(0, window, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while rig.act(|app, _, _| app.workspaces.len()) < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "the template never opened a workspace"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(rig.workspaces(), ["alpha", "*qa-space"]);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b)");
    assert_eq!(rig.tabs(1), ["*=H(0.500 c, 0.500 d)"]);
    assert_eq!(rig.requests(), ["create c", "create d"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_shell_command_of_the_config_joins_the_palette_and_runs_in_a_new_tab(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    rig.requests();
    let command: paneflow_config::schema::CommandDefinition = serde_json::from_value(json!({
        "name": "qa-run",
        "description": "QA simple command",
        "keywords": ["smoke"],
        "command": "echo qa-command-ran"
    }))
    .expect("a shell command");
    rig.act(|app, window, cx| {
        app.cached_config.commands = vec![command];
        app.open_command_palette(window, cx);
        app.command_palette_filter("qa-", cx);
    });
    let listed = rig.act(|app, _, cx| app.command_palette_entries(cx));
    assert_eq!(
        listed,
        [("qa-run".to_string(), Some("QA simple command".to_string()))]
    );
    rig.act(|app, _, cx| app.command_palette_filter("smoke", cx));
    let by_keyword = rig.act(|app, _, cx| app.command_palette_entries(cx));
    assert!(
        by_keyword.iter().any(|(label, _)| label == "qa-run"),
        "{by_keyword:?}"
    );
    rig.act(|app, window, cx| app.command_palette_choose("qa-run", window, cx));
    assert!(!rig.act(|app, _, _| app.command_palette_is_open()));
    assert_eq!(rig.tabs(0), ["=a", "*qa-run=b"]);
    assert_eq!(rig.focused().as_deref(), Some("b"));
    assert_eq!(rig.requests(), ["create b"]);
    wait_for_typed(&mut rig, "b", "echo qa-command-ran\r");
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_workspace_up_with_an_invalid_pane_in_the_middle_creates_nothing(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.requests();
    let missing = std::env::temp_dir().join("paneflow-layout-pin-missing-dir");
    let reply = rig.ipc(
        "workspace.up",
        json!({"name": "delta", "panes": [{"label": "one"}, {"cwd": missing}, {"label": "three"}]}),
    );
    assert_eq!(reply["error"]["code"], json!(-32602), "{reply}");
    assert!(
        reply["error"]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("pane 1: ")),
        "{reply}"
    );
    assert_eq!(rig.workspaces(), ["*alpha"]);
    assert_eq!(rig.requests(), Vec::<String>::new());
    let reply = rig.ipc(
        "workspace.up",
        json!({"name": "delta", "panes": [{"label": "one"}, {"label": "two"}]}),
    );
    assert!(reply.get("result").is_some(), "{reply}");
    assert_eq!(rig.workspaces(), ["alpha", "*delta"]);
    assert_eq!(rig.tabs(1), ["*=V(0.500 b, 0.500 c)"]);
    assert_eq!(rig.requests(), ["create b", "create c"]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_divider_drag_moves_only_the_view_and_applies_one_op_when_released(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    three_panes(&mut rig);
    let before = rig.revision();
    let split = rig.drag_divider(&[], 0.4, 0.35);
    assert_eq!(rig.shape(0, 0), "V(0.400 a, 0.350 b, 0.250 c)");
    assert_eq!(rig.model_shape(), "V(0.500 a, 0.250 b, 0.250 c)");
    rig.act(|app, _, cx| app.adopt_layout(cx));
    assert_eq!(rig.model_shape(), "V(0.500 a, 0.250 b, 0.250 c)");
    assert_eq!(rig.revision(), before);
    rig.release_divider(split);
    assert_eq!(rig.revision(), before + 1);
    assert_eq!(rig.model_shape(), "V(0.400 a, 0.350 b, 0.250 c)");
    assert_eq!(rig.shape(0, 0), "V(0.400 a, 0.350 b, 0.250 c)");
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_two_nested_dividers_each_apply_their_own_op(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split_v(&mut rig);
    rig.focus("b");
    rig.act(|app, window, cx| app.handle_split_h(&SplitHorizontally, window, cx));
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 H(0.500 b, 0.500 c))");
    let before = rig.revision();
    let outer = rig.drag_divider(&[], 0.6, 0.4);
    rig.release_divider(outer);
    assert_eq!(rig.revision(), before + 1);
    assert_eq!(rig.model_shape(), "V(0.600 a, 0.400 H(0.500 b, 0.500 c))");
    let inner = rig.drag_divider(&[1], 0.3, 0.7);
    assert_ne!(inner, outer);
    rig.release_divider(inner);
    assert_eq!(rig.revision(), before + 2);
    assert_eq!(rig.model_shape(), "V(0.600 a, 0.400 H(0.300 b, 0.700 c))");
    assert_eq!(rig.shape(0, 0), "V(0.600 a, 0.400 H(0.300 b, 0.700 c))");
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_cancelled_divider_drag_applies_no_op_and_returns_to_the_model_ratio(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    three_panes(&mut rig);
    let before = rig.revision();
    rig.drag_divider(&[], 0.4, 0.35);
    rig.act(|app, _, cx| app.cancel_divider_drags(cx));
    assert_eq!(rig.revision(), before);
    assert_eq!(rig.model_shape(), "V(0.500 a, 0.250 b, 0.250 c)");
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.250 b, 0.250 c)");
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
#[tracing_test::traced_test]
fn pin_a_panicking_op_keeps_the_last_projected_tree(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    three_panes(&mut rig);
    let before = rig.revision();
    let error = rig.act(|app, _, cx| app.apply_panicking_layout_op(cx));
    assert_eq!(error, paneflow_layout::OpError::Invalid);
    assert!(logs_contain(
        "layout op panicked: the window keeps its last layout"
    ));
    assert!(logs_contain("op=\"Split\""));
    assert_eq!(rig.revision(), before);
    assert_eq!(rig.model_shape(), "V(0.500 a, 0.250 b, 0.250 c)");
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.250 b, 0.250 c)");
    rig.focus("b");
    split_v(&mut rig);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.125 b, 0.125 d, 0.250 c)");
    assert_eq!(rig.requests(), vec!["create d".to_string()]);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_focusing_a_surface_hidden_by_zoom_leaves_zoom_and_the_zoomed_one_keeps_it(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    three_panes(&mut rig);
    zoom(&mut rig, "b");
    let b = rig.surface_id("b");
    let reply = rig.ipc("surface.focus", json!({"surface_id": b}));
    assert_eq!(reply["result"]["focused"], json!(true), "{reply}");
    assert_eq!(rig.model_shape(), "V(0.500 a, 0.250 b, 0.250 c)");
    let zoomed = rig.cx.update(|_, cx| {
        rig.app
            .read(cx)
            .active_workspace()
            .is_some_and(|ws| ws.active_tab().is_zoomed())
    });
    assert!(zoomed, "the visible zoomed pane keeps the zoom");
    let a = rig.surface_id("a");
    rig.ipc("surface.focus", json!({"surface_id": a}));
    let zoomed = rig.cx.update(|_, cx| {
        rig.app
            .read(cx)
            .active_workspace()
            .is_some_and(|ws| ws.active_tab().is_zoomed())
    });
    assert!(!zoomed, "focusing a hidden pane leaves zoom");
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_closing_a_tab_to_stop_its_sessions_stops_them_and_holds_nothing(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    open_tab(&mut rig, 0, "two");
    rig.requests();
    rig.act(|app, window, cx| {
        app.perform_close(
            crate::app::close_policy::CloseTarget::Tab {
                ws_idx: 0,
                tab_idx: 1,
            },
            crate::app::close_policy::CloseIntent::Stop,
            Some(window),
            cx,
        );
    });
    assert_eq!(rig.tabs(0), ["*=a"]);
    assert_eq!(rig.requests(), vec!["stop b".to_string()]);
}
