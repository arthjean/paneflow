use gpui::TestAppContext;
use paneflow_config::schema::TerminalSurfaceProfile;
use paneflow_layout::Op;
use serde_json::{Value, json};

use super::Rig;
use crate::app::workspace_ops::SurfaceLaunch;
use crate::pane::PaneEvent;
use crate::{ClosePane, FocusLeft, SplitVertically};

pub(super) const PINS: usize = 2;

fn published(rig: &mut Rig<'_>) -> Vec<Value> {
    rig.cx.update(|_, cx| {
        rig.app
            .read(cx)
            .pending_layout_events()
            .iter()
            .map(|event| serde_json::to_value(event).unwrap_or_default())
            .collect()
    })
}

fn since(rig: &mut Rig<'_>, mark: &mut usize) -> Vec<Value> {
    let all = published(rig);
    let fresh = all[*mark..].to_vec();
    *mark = all.len();
    fresh
}

fn one<'e>(events: &'e [Value], kind: &str) -> &'e Value {
    let matching: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == kind)
        .collect();
    assert_eq!(matching.len(), 1, "exactly one {kind} in {events:?}");
    matching[0]
}

fn model_pane(rig: &mut Rig<'_>, name: &str) -> u64 {
    let pane = rig.pane(name);
    rig.cx.update(|_, cx| {
        rig.app
            .read(cx)
            .layout_pane_id(&pane)
            .expect("the pane is in the model")
            .get()
    })
}

fn model_workspace(rig: &mut Rig<'_>, index: usize) -> u64 {
    rig.cx.update(|_, cx| {
        rig.app.read(cx).layout.model().state().workspaces[index]
            .id
            .get()
    })
}

fn model_tab(rig: &mut Rig<'_>, ws: usize, tab: usize) -> u64 {
    rig.cx.update(|_, cx| {
        rig.app.read(cx).layout.model().state().workspaces[ws].tabs[tab]
            .id
            .get()
    })
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_every_layout_op_publishes_exactly_one_event_of_its_type_with_the_model_ids(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    let mut mark = published(&mut rig).len();

    let reply = rig.ipc("workspace.create", json!({"name": "gamma"}));
    assert!(reply.get("result").is_some(), "{reply}");
    let events = since(&mut rig, &mut mark);
    let gamma = model_workspace(&mut rig, 1);
    assert_eq!(one(&events, "workspace.created")["workspace_id"], gamma);
    assert_eq!(one(&events, "tab.created")["workspace_id"], gamma);
    assert_eq!(one(&events, "pane.created")["workspace_id"], gamma);
    let created = one(&events, "surface.created");
    assert_eq!(created["workspace_id"], gamma);
    assert!(created["surface_id"].as_u64().is_some() && created["session"].is_string());

    rig.act(|app, _, cx| {
        app.apply_layout_op(
            Op::RenameWorkspace {
                workspace: paneflow_layout::WorkspaceId(gamma),
                title: "delta".to_string(),
            },
            cx,
        )
        .expect("the window renames a workspace through the model")
    });
    let events = since(&mut rig, &mut mark);
    assert_eq!(one(&events, "workspace.renamed")["workspace_id"], gamma);

    rig.act(|app, window, cx| app.select_workspace(1, window, cx));
    let events = since(&mut rig, &mut mark);
    assert_eq!(one(&events, "workspace.selected")["workspace_id"], gamma);
    rig.act(|app, window, cx| app.select_workspace(0, window, cx));
    let alpha = model_workspace(&mut rig, 0);
    assert_eq!(
        one(&since(&mut rig, &mut mark), "workspace.selected")["workspace_id"],
        alpha
    );

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
    let events = since(&mut rig, &mut mark);
    let second = model_tab(&mut rig, 0, 1);
    let tab = one(&events, "tab.created");
    assert_eq!(tab["tab_id"], second);
    assert_eq!(tab["workspace_id"], alpha);

    rig.act(|app, _, cx| {
        app.begin_tab_rename(0, 1, cx);
        app.rename_input
            .update(cx, |input, cx| input.set_value("renamed", cx));
        app.commit_rename(cx);
    });
    let events = since(&mut rig, &mut mark);
    let renamed = one(&events, "tab.renamed");
    assert_eq!(renamed["tab_id"], second);
    assert_eq!(renamed["workspace_id"], alpha);

    rig.act(|app, window, cx| app.select_workspace_tab(0, 0, window, cx));
    let first = model_tab(&mut rig, 0, 0);
    let events = since(&mut rig, &mut mark);
    assert_eq!(one(&events, "tab.selected")["tab_id"], first);

    rig.focus("a");
    since(&mut rig, &mut mark);
    rig.act(|app, window, cx| app.handle_split_v(&SplitVertically, window, cx));
    let events = since(&mut rig, &mut mark);
    let b = model_pane(&mut rig, "d");
    let pane = one(&events, "pane.created");
    assert_eq!(pane["pane_id"], b);
    assert_eq!(pane["tab_id"], first);
    assert_eq!(pane["workspace_id"], alpha);
    assert_eq!(one(&events, "surface.created")["pane_id"], b);
    let changed = one(&events, "layout.changed");
    assert_eq!(changed["tab_id"], first);
    assert_eq!(changed["data"]["revision"], rig.revision());
    assert_eq!(one(&events, "pane.focused")["pane_id"], b);

    rig.act(|app, window, cx| app.handle_focus_left(&FocusLeft, window, cx));
    rig.act(|app, window, cx| app.note_layout_focus(window, cx));
    let a = model_pane(&mut rig, "a");
    assert_eq!(
        one(&since(&mut rig, &mut mark), "pane.focused")["pane_id"],
        a
    );

    let surface = rig.surface_id("a");
    let reply = rig.ipc(
        "surface.rename",
        json!({"surface_id": surface, "name": "custom"}),
    );
    assert!(reply.get("result").is_some(), "{reply}");
    let events = since(&mut rig, &mut mark);
    let renamed = one(&events, "surface.renamed");
    assert_eq!(renamed["surface_id"], surface);
    assert_eq!(renamed["pane_id"], a);

    rig.emit("d", PaneEvent::NewTab);
    let events = since(&mut rig, &mut mark);
    assert_eq!(one(&events, "surface.created")["pane_id"], b);

    rig.focus("d");
    since(&mut rig, &mut mark);
    rig.act(|app, window, cx| app.handle_close_pane(&ClosePane, window, cx));
    let events = since(&mut rig, &mut mark);
    let closed = one(&events, "pane.closed");
    assert_eq!(closed["pane_id"], b);
    assert_eq!(closed["workspace_id"], alpha);
    let closed_surfaces: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == "surface.closed")
        .collect();
    assert_eq!(
        closed_surfaces.len(),
        2,
        "both surfaces of the pane: {events:?}"
    );
    assert!(closed_surfaces.iter().all(|event| event["pane_id"] == b));
    one(&events, "layout.changed");

    rig.act(|app, window, cx| app.close_workspace_tab(0, 1, window, cx));
    let events = since(&mut rig, &mut mark);
    let closed = one(&events, "tab.closed");
    assert_eq!(closed["tab_id"], second);
    assert_eq!(closed["workspace_id"], alpha);

    rig.act(|app, window, cx| app.close_workspace_at(1, window, cx));
    let events = since(&mut rig, &mut mark);
    assert_eq!(one(&events, "workspace.closed")["workspace_id"], gamma);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_divider_drag_publishes_exactly_one_layout_changed_when_it_ends(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    rig.act(|app, window, cx| app.handle_split_v(&SplitVertically, window, cx));
    let mut mark = published(&mut rig).len();
    let split = rig.drag_divider(&[], 0.4, 0.6);
    rig.act(|app, _, cx| app.adopt_layout(cx));
    assert_eq!(
        since(&mut rig, &mut mark),
        Vec::<Value>::new(),
        "a drag in progress publishes nothing"
    );
    rig.release_divider(split);
    let events = since(&mut rig, &mut mark);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["type"], "layout.changed");
    assert_eq!(events[0]["data"]["revision"], rig.revision());
}
