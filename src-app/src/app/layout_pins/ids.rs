use gpui::TestAppContext;
use paneflow_config::schema::{DetachedPaneLocation, DetachedPaneSession};
use serde_json::json;

use super::Rig;
use crate::pane::PaneEvent;
use crate::{SplitVertically, ToggleZoom};

pub(super) const PINS: usize = 8;

fn split_v(rig: &mut Rig<'_>) {
    rig.act(|app, window, cx| app.handle_split_v(&SplitVertically, window, cx));
}

fn two_panes_and_a_stacked_surface(rig: &mut Rig<'_>) {
    rig.workspace("alpha");
    rig.focus("a");
    split_v(rig);
    rig.emit("b", PaneEvent::NewTab);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b+c*)");
    rig.requests();
}

fn detach(rig: &mut Rig<'_>, name: &str) {
    let pane = rig.pane(name);
    rig.act(|app, window, cx| app.toggle_detached_pane(pane, window, cx));
}

fn is_detached(rig: &mut Rig<'_>, name: &str) -> bool {
    let pane = rig.pane(name);
    rig.cx.update(|_, cx| pane.read(cx).is_detached())
}

fn model_pane_id(rig: &mut Rig<'_>, name: &str) -> u64 {
    let pane = rig.pane(name);
    rig.cx.update(|_, cx| {
        rig.app
            .read(cx)
            .layout_pane_id(&pane)
            .expect("the pane is in the model")
            .0
    })
}

fn model_detached(rig: &mut Rig<'_>, name: &str) -> bool {
    let id = paneflow_layout::PaneId(model_pane_id(rig, name));
    rig.cx.update(|_, cx| {
        rig.app
            .read(cx)
            .layout
            .model()
            .state()
            .pane(id)
            .is_some_and(|pane| pane.detached)
    })
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_wire_ids_are_the_model_ids_and_survive_a_restart(cx: &mut TestAppContext) {
    let (state, before, names) = {
        let mut rig = Rig::mount(cx);
        two_panes_and_a_stacked_surface(&mut rig);
        let model = rig.model_surface_ids();
        assert_eq!(rig.listed_surface_ids(), model);
        let before: Vec<(&str, u64)> = ["a", "b", "c"]
            .into_iter()
            .map(|name| (name, rig.surface_id(name)))
            .collect();
        let mut terminals: Vec<u64> = before.iter().map(|(_, id)| *id).collect();
        terminals.sort_unstable();
        assert_eq!(terminals, model, "each terminal spawned with its model id");
        let state = rig.state();
        assert!(state.next_layout_id.is_some_and(|next| next > model[2]));
        assert!(state.workspaces[0].layout_id.is_some());
        let ids = state.workspaces[0].tabs[0]
            .layout_ids
            .clone()
            .expect("the tab carries its ids");
        assert_eq!(ids.panes.len(), 2);
        assert_eq!(ids.surfaces.concat().len(), 3);
        let mut written: Vec<u64> = ids.surfaces.concat();
        written.sort_unstable();
        assert_eq!(written, model);
        (state, before, rig.names())
    };
    let mut rig = Rig::mount_restored(cx, &state, names);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b+c*)");
    for (name, id) in &before {
        assert_eq!(rig.surface_id(name), *id, "{name} keeps its surface id");
    }
    let mut restored: Vec<u64> = before.iter().map(|(_, id)| *id).collect();
    restored.sort_unstable();
    assert_eq!(rig.listed_surface_ids(), restored);
    assert_eq!(rig.model_surface_ids(), restored);
    let workspace = rig.cx.update(|_, cx| rig.app.read(cx).workspaces[0].id);
    assert_eq!(Some(workspace), state.workspaces[0].layout_id);
    let tab = rig
        .cx
        .update(|_, cx| rig.app.read(cx).workspaces[0].tabs()[0].id);
    assert_eq!(
        Some(tab),
        state.workspaces[0].tabs[0]
            .layout_ids
            .as_ref()
            .map(|ids| ids.tab)
    );
    rig.requests();
    let anchor = rig.surface_id("a");
    let reply = rig.ipc(
        "surface.split",
        json!({"surface_id": anchor, "direction": "vertical"}),
    );
    assert_eq!(rig.requests(), ["create d"]);
    let fresh = rig.surface_id("d");
    assert_eq!(reply["result"]["surface_id"], json!(fresh), "{reply}");
    assert!(
        fresh > restored[2],
        "a new surface takes an id above the counter"
    );
    assert!(rig.model_surface_ids().contains(&fresh));
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_surface_in_a_background_tab_or_behind_another_resolves_to_its_pane(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    two_panes_and_a_stacked_surface(&mut rig);
    let behind = rig.surface_id("b");
    let reply = rig.ipc("surface.focus", json!({"surface_id": behind}));
    assert_eq!(reply["result"]["focused"], json!(true), "{reply}");
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b+c*)");
    assert_eq!(
        rig.focused().as_deref(),
        Some("c"),
        "the pane of b takes focus"
    );
    let missing = rig.ipc("surface.focus", json!({"surface_id": 987_654_321u64}));
    assert_eq!(
        missing["error"]["message"],
        json!("surface_id 987654321 not found"),
        "{missing}"
    );
    assert_eq!(rig.requests(), Vec::<String>::new());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_detached_pane_is_saved_and_restored_by_its_pane_id(cx: &mut TestAppContext) {
    let (state, names, b) = {
        let mut rig = Rig::mount(cx);
        two_panes_and_a_stacked_surface(&mut rig);
        detach(&mut rig, "b");
        assert!(is_detached(&mut rig, "b"));
        assert!(model_detached(&mut rig, "b"));
        let b = model_pane_id(&mut rig, "b");
        let state = rig.state();
        assert_eq!(state.detached_panes.len(), 1);
        assert_eq!(state.detached_panes[0].pane, Some(b));
        (state, rig.names(), b)
    };
    let mut rig = Rig::mount_restored(cx, &state, names);
    rig.act(|app, window, cx| app.restore_detached_panes(window, cx));
    assert_eq!(model_pane_id(&mut rig, "b"), b);
    assert!(is_detached(&mut rig, "b"));
    assert!(!is_detached(&mut rig, "a"));
    assert_eq!(rig.state().detached_panes[0].pane, Some(b));
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_positional_placement_converts_to_its_pane_and_a_dangling_one_is_dropped(
    cx: &mut TestAppContext,
) {
    let (mut state, names) = {
        let mut rig = Rig::mount(cx);
        two_panes_and_a_stacked_surface(&mut rig);
        (rig.state(), rig.names())
    };
    let placement = |leaf: usize| DetachedPaneSession {
        location: DetachedPaneLocation {
            workspace: 0,
            tab: 0,
            leaf,
        },
        layout_leaf_count: Some(2),
        pane: None,
        x: 40.0,
        y: 40.0,
        width: 640.0,
        height: 480.0,
    };
    state.detached_panes = vec![placement(1), placement(7)];
    let mut rig = Rig::mount_restored(cx, &state, names);
    rig.act(|app, window, cx| app.restore_detached_panes(window, cx));
    assert!(is_detached(&mut rig, "b"), "the positional entry names b");
    assert!(!is_detached(&mut rig, "a"));
    let b = model_pane_id(&mut rig, "b");
    let saved = rig.state().detached_panes;
    assert_eq!(saved.len(), 1, "the dangling entry is dropped");
    assert_eq!(saved[0].pane, Some(b), "the entry is now keyed by pane id");
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_closing_a_detached_window_shows_the_pane_again_and_removing_it_closes_the_window(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    two_panes_and_a_stacked_surface(&mut rig);
    detach(&mut rig, "b");
    let windows = rig.cx.update(|_, cx| cx.windows().len());
    let shape = rig.model_shape();
    let pane = rig.pane("b");
    let window = rig
        .cx
        .update(|_, cx| pane.read(cx).detached.expect("b is detached").window);
    rig.cx.update(|_, cx| {
        window
            .update(cx, |_, window, _| window.remove_window())
            .expect("the detached window is open");
    });
    rig.cx.run_until_parked();
    assert!(!is_detached(&mut rig, "b"));
    assert!(!model_detached(&mut rig, "b"));
    assert_eq!(rig.model_shape(), shape, "no structural op is applied");
    assert_eq!(rig.cx.update(|_, cx| cx.windows().len()), windows - 1);
    assert_eq!(rig.requests(), Vec::<String>::new());

    detach(&mut rig, "a");
    assert_eq!(rig.cx.update(|_, cx| cx.windows().len()), windows);
    let pane = rig.pane("a");
    rig.act(|app, _, cx| {
        app.perform_close(
            crate::app::close_policy::CloseTarget::Pane(pane),
            crate::app::close_policy::CloseIntent::Stop,
            None,
            cx,
        );
    });
    rig.cx.run_until_parked();
    assert_eq!(
        rig.cx.update(|_, cx| cx.windows().len()),
        windows - 1,
        "the window of a pane that left the model closes"
    );
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_detaching_a_pane_cancels_divider_drags_and_clears_zoom(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    two_panes_and_a_stacked_surface(&mut rig);
    let revision = rig.revision();
    let split = rig.drag_divider(&[], 0.2, 0.8);
    assert_eq!(rig.revision(), revision);
    detach(&mut rig, "a");
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b+c*)");
    rig.release_divider(split);
    assert_eq!(rig.model_shape(), "V(0.500 a, 0.500 b+c*)");
    detach(&mut rig, "a");
    rig.focus("b");
    rig.act(|app, window, cx| app.handle_toggle_zoom(&ToggleZoom, window, cx));
    let zoomed = rig.cx.update(|_, cx| {
        rig.app
            .read(cx)
            .active_workspace()
            .is_some_and(|ws| ws.active_tab().is_zoomed())
    });
    assert!(zoomed);
    detach(&mut rig, "b");
    let zoomed = rig.cx.update(|_, cx| {
        rig.app
            .read(cx)
            .active_workspace()
            .is_some_and(|ws| ws.active_tab().is_zoomed())
    });
    assert!(!zoomed, "detaching the zoomed pane clears zoom");
    assert!(model_detached(&mut rig, "b"));
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_session_without_ids_takes_the_surface_id_its_host_recorded(cx: &mut TestAppContext) {
    let (mut state, names, a, b) = {
        let mut rig = Rig::mount(cx);
        rig.workspace("alpha");
        rig.focus("a");
        split_v(&mut rig);
        let a = rig.session_of("a");
        let b = rig.session_of("b");
        (rig.state(), rig.names(), a, b)
    };
    state.next_layout_id = None;
    for ws in &mut state.workspaces {
        ws.layout_id = None;
        for tab in &mut ws.tabs {
            tab.layout_ids = None;
        }
    }
    let recorded = 9_000_000u64;
    let host = crate::app::fake_host::FakeHost::default();
    host.spawned_with_surface(a, recorded);
    host.spawned_with_surface(b, recorded);
    let mut rig = Rig::mount_restored_on(cx, &state, names, host);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b)");
    assert_eq!(
        rig.surface_id("a"),
        recorded,
        "a takes the id it was spawned with"
    );
    let b = rig.surface_id("b");
    assert_ne!(b, recorded, "an id already claimed is not taken twice");
    assert!(rig.model_surface_ids().contains(&recorded));
    assert!(rig.model_surface_ids().contains(&b));
    rig.requests();
    rig.focus("a");
    split_v(&mut rig);
    assert!(
        rig.surface_id("c") > recorded,
        "the counter starts above the highest id"
    );
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_the_window_state_restores_a_placement_and_a_dock_and_drops_an_unknown_pane(
    cx: &mut TestAppContext,
) {
    let (state, names, b, tab) = {
        let mut rig = Rig::mount(cx);
        two_panes_and_a_stacked_surface(&mut rig);
        detach(&mut rig, "b");
        let b = model_pane_id(&mut rig, "b");
        rig.act(|app, _, cx| app.save_session(cx));
        let recorded =
            crate::window_state::recorded_layout().expect("the window recorded its layout");
        assert_eq!(recorded.detached.len(), 1);
        assert_eq!(recorded.detached[0].pane, b);
        let state = crate::app::session::shared_session_state(rig.state());
        assert!(
            state.detached_panes.is_empty(),
            "session.json no longer holds placements"
        );
        assert!(
            state
                .workspaces
                .iter()
                .flat_map(|ws| ws.tabs.iter())
                .all(|tab| tab.dock.is_none())
        );
        let tab = state.workspaces[0].tabs[0]
            .layout_ids
            .as_ref()
            .expect("the tab has ids")
            .tab;
        (state, rig.names(), b, tab)
    };
    let placed = |pane| crate::window_state::PersistedDetached {
        pane,
        x: 40.,
        y: 50.,
        width: 640.,
        height: 480.,
    };
    crate::window_state::seed_loaded_layout(Some(crate::window_state::WindowLayoutState {
        docks: vec![crate::window_state::PersistedDock {
            tab,
            dock: paneflow_config::schema::DockSession {
                open: false,
                tabs: Vec::new(),
                active: 0,
                tree: true,
            },
        }],
        detached: vec![placed(987_654), placed(b)],
    }));
    let mut rig = Rig::mount_restored(cx, &state, names);
    rig.act(|app, window, cx| app.restore_detached_panes(window, cx));
    crate::window_state::seed_loaded_layout(None);
    assert!(is_detached(&mut rig, "b"));
    assert!(!is_detached(&mut rig, "a"));
    assert_eq!(model_pane_id(&mut rig, "b"), b);
    assert!(
        rig.cx
            .update(|_, cx| rig.app.read(cx).workspaces[0].tabs()[0].files_sidebar_open),
        "the dock of the tab came back from window-state.json"
    );
    assert!(rig.toasts().is_empty(), "{:?}", rig.toasts());
}
