#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Fixture, effects, session};
use paneflow_config::schema::{
    DetachedPaneLocation, DetachedPaneSession, LayoutNode, SessionState, SpaceId,
    SurfaceDefinition, TabLayoutIds, TabSession, TabTitleSource, WorkspaceSession,
};
use paneflow_layout::{
    Direction, Edge, Effect, Event, MAX_PANES, NewNode, Node, Op, OpError, PaneId, Preset, SplitId,
    SurfaceId, TabId, WorkspaceId, export, load,
};

#[test]
fn the_crate_links_neither_gpui_nor_the_app() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("the manifest");
    let dependencies = manifest
        .split("[dependencies]")
        .nth(1)
        .and_then(|rest| rest.split("\n[").next())
        .expect("a dependency table");
    let names: Vec<&str> = dependencies
        .lines()
        .filter_map(|line| line.split(['.', '=']).next())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    assert_eq!(names, ["paneflow-config", "serde", "tracing"]);
}

#[test]
fn ids_come_from_one_increasing_counter_and_a_closed_id_never_returns() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let first = fx.model.next_id();
    fx.split("a", Direction::Vertical, "b");
    let closed = fx.pane_id("b");
    fx.apply(Op::ClosePane {
        pane: closed,
        keep_undo: false,
    });
    let mut seen = Vec::new();
    for _ in 0..50 {
        seen.push(fx.model.reserve());
    }
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(seen.iter().all(|id| *id > first && *id != closed.get()));
}

fn current_release_session() -> SessionState {
    let terminal = |name: &str, focus: bool| SurfaceDefinition {
        surface_type: Some("terminal".into()),
        name: Some(name.into()),
        cwd: Some(format!("/work/{name}")),
        focus: Some(focus),
        session: Some(session(name)),
        ..SurfaceDefinition::default()
    };
    let tab = |title: &str, layout: LayoutNode| TabSession {
        title: title.into(),
        title_source: Some(TabTitleSource::User),
        layout: Some(layout),
        ..TabSession::default()
    };
    SessionState {
        version: 3,
        active_workspace: 1,
        workspaces: vec![
            WorkspaceSession {
                id: Some(paneflow_config::schema::WorkspaceId::new()),
                layout_id: None,
                title: "alpha".into(),
                cwd: "/work/alpha".into(),
                tabs: vec![
                    tab(
                        "one",
                        LayoutNode::Split {
                            direction: "vertical".into(),
                            ratio: None,
                            ratios: Some(vec![f64::from(0.25_f32), f64::from(0.75_f32)]),
                            children: vec![
                                LayoutNode::Pane {
                                    surfaces: vec![terminal("a", false), terminal("b", true)],
                                },
                                LayoutNode::Pane {
                                    surfaces: vec![terminal("c", true)],
                                },
                            ],
                        },
                    ),
                    TabSession {
                        title_source: Some(TabTitleSource::Preset),
                        ..TabSession::empty()
                    },
                ],
                active_tab: 0,
                legacy_layout: None,
                custom_buttons: Vec::new(),
                expanded_paths: vec!["src".into()],
                managed_worktrees: Vec::new(),
                sidebar_collapsed: true,
                muted: false,
                space: None,
            },
            WorkspaceSession {
                id: Some(paneflow_config::schema::WorkspaceId::new()),
                layout_id: None,
                title: "beta".into(),
                cwd: "/work/beta".into(),
                tabs: vec![tab(
                    "two",
                    LayoutNode::Pane {
                        surfaces: vec![terminal("d", true)],
                    },
                )],
                active_tab: 0,
                legacy_layout: None,
                custom_buttons: Vec::new(),
                expanded_paths: Vec::new(),
                managed_worktrees: Vec::new(),
                sidebar_collapsed: false,
                muted: true,
                space: None,
            },
        ],
        detached_panes: vec![DetachedPaneSession {
            location: DetachedPaneLocation {
                workspace: 0,
                tab: 0,
                leaf: 1,
            },
            layout_leaf_count: Some(2),
            pane: None,
            x: 10.0,
            y: 20.0,
            width: 300.0,
            height: 200.0,
        }],
        active_space: None,
        next_layout_id: None,
    }
}

fn without_ids(mut state: SessionState) -> SessionState {
    state.next_layout_id = None;
    for ws in &mut state.workspaces {
        ws.layout_id = None;
        for tab in &mut ws.tabs {
            tab.layout_ids = None;
        }
    }
    for detached in &mut state.detached_panes {
        detached.pane = None;
    }
    state
}

#[test]
fn a_current_release_session_gets_ids_and_exports_back_unchanged() {
    let original = current_release_session();
    let loaded = load(&original);
    assert_eq!(loaded.reassigned, 0);
    let state = loaded.model.state();
    let mut ids = Vec::new();
    for ws in &state.workspaces {
        ids.push(ws.id.get());
        for tab in &ws.tabs {
            ids.push(tab.id.get());
            for pane in tab.root.as_ref().map(Node::panes).unwrap_or_default() {
                ids.push(pane.id.get());
                ids.extend(pane.surfaces.iter().map(|surface| surface.id.get()));
            }
        }
    }
    assert_eq!(ids.len(), 2 + 3 + 3 + 4);
    let unique: std::collections::BTreeSet<u64> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len());
    assert!(ids.iter().all(|id| *id > 0 && *id < loaded.model.next_id()));
    let detached_pane = state.workspaces[0].tabs[0]
        .root
        .as_ref()
        .unwrap()
        .pane_ids()[1];
    assert_eq!(loaded.detached.len(), 1);
    assert_eq!(loaded.detached[0].pane, detached_pane);

    let exported = export(&loaded.model, &loaded.detached);
    assert_eq!(without_ids(exported), original);
}

#[test]
fn stated_ids_and_counter_are_kept() {
    let first = load(&current_release_session());
    let saved = export(&first.model, &first.detached);
    let second = load(&saved);
    assert_eq!(second.reassigned, 0);
    let mut again = export(&second.model, &second.detached);
    assert!(again.next_layout_id >= saved.next_layout_id);
    again.next_layout_id = saved.next_layout_id;
    assert_eq!(again, saved);

    let mut ahead = saved.clone();
    ahead.next_layout_id = Some(10_000);
    assert!(load(&ahead).model.next_id() >= 10_000);
    let mut behind = saved;
    behind.next_layout_id = Some(1);
    let highest = behind.workspaces[1].layout_id.unwrap();
    assert!(load(&behind).model.next_id() > highest);
}

#[test]
fn a_duplicated_or_corrupt_id_gets_a_fresh_one_and_loading_succeeds() {
    let first = load(&current_release_session());
    let mut saved = export(&first.model, &first.detached);
    let tab_one = saved.workspaces[0].tabs[0].layout_ids.clone().unwrap();
    saved.workspaces[1].layout_id = Some(tab_one.tab);
    saved.workspaces[0].tabs[1].layout_ids = Some(TabLayoutIds {
        tab: 0,
        panes: Vec::new(),
        surfaces: Vec::new(),
    });
    let loaded = load(&saved);
    assert_eq!(loaded.reassigned, 2);
    let state = loaded.model.state();
    assert_eq!(state.workspaces[0].tabs[0].id.get(), tab_one.tab);
    assert_ne!(state.workspaces[1].id.get(), tab_one.tab);
    assert_ne!(state.workspaces[0].tabs[1].id.get(), 0);
    assert!(state.workspaces[1].id.get() >= first.model.next_id());
}

#[test]
fn the_view_is_a_snapshot_with_the_revision() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let one = fx.model.view();
    let two = fx.model.view();
    assert_eq!(one, two);
    assert_eq!(one.revision, fx.model.revision());
    fx.split("a", Direction::Vertical, "b");
    assert_ne!(fx.model.view(), one);
}

#[test]
fn a_successful_op_bumps_the_revision_once_and_reports_its_effects() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let before = fx.model.revision();
    let applied = fx.split("a", Direction::Vertical, "b");
    assert_eq!(fx.model.revision(), before + 1);
    assert_eq!(applied.revision, before + 1);
    assert_eq!(effects(&applied, &fx), ["spawn b"]);
    assert!(
        applied
            .events
            .iter()
            .any(|event| matches!(event, Event::PaneCreated { .. }))
    );
    assert_eq!(applied.focus, Some(fx.pane_id("b")));
}

#[test]
fn a_failed_op_leaves_the_model_unchanged_with_a_typed_reason() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    let tab = fx.tab_id(0, 0);
    fx.apply(Op::SetZoom {
        tab,
        pane: Some(fx.pane_id("a")),
    });
    let pane = fx.pane(&["c"]);
    let target = fx.pane_id("a");
    let before = fx.model.clone();
    let error = fx
        .try_apply(Op::Split {
            target,
            direction: Direction::Vertical,
            edge: Edge::After,
            pane: pane.clone(),
            focus: true,
        })
        .unwrap_err();
    assert_eq!(error, OpError::Zoomed);
    assert_eq!(error.reason(), "zoomed");
    assert_eq!(fx.model, before);
    assert_eq!(
        fx.try_apply(Op::FocusPane {
            pane: PaneId(999_999)
        }),
        Err(OpError::NotFound)
    );
    assert_eq!(fx.model, before);
}

#[test]
fn split_inserts_after_the_target_and_halves_its_share() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    assert_eq!(fx.shape(0, 0), "V(0.500 a, 0.500 b)");
    fx.split("a", Direction::Horizontal, "c");
    assert_eq!(fx.shape(0, 0), "V(0.500 H(0.500 a, 0.500 c), 0.500 b)");
    fx.split("b", Direction::Vertical, "d");
    assert_eq!(
        fx.shape(0, 0),
        "V(0.500 H(0.500 a, 0.500 c), 0.250 b, 0.250 d)"
    );
    assert_eq!(fx.focused(0, 0).as_deref(), Some("d"));
}

#[test]
fn split_before_puts_the_new_pane_first() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let target = fx.pane_id("a");
    let pane = fx.pane(&["b"]);
    fx.apply(Op::Split {
        target,
        direction: Direction::Vertical,
        edge: Edge::Before,
        pane,
        focus: false,
    });
    assert_eq!(fx.shape(0, 0), "V(0.500 b, 0.500 a)");
    assert_eq!(fx.focused(0, 0).as_deref(), Some("a"));
}

#[test]
fn split_is_refused_while_zoomed_and_at_the_pane_limit() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "p0");
    for i in 1..MAX_PANES {
        fx.split(
            &format!("p{}", i - 1),
            Direction::Vertical,
            &format!("p{i}"),
        );
    }
    let target = fx.pane_id("p0");
    let pane = fx.pane(&["extra"]);
    assert_eq!(
        fx.try_apply(Op::Split {
            target,
            direction: Direction::Vertical,
            edge: Edge::After,
            pane,
            focus: true,
        }),
        Err(OpError::PaneLimit)
    );
}

#[test]
fn closing_the_focused_pane_holds_its_session_and_focuses_the_previous_sibling() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    fx.split("b", Direction::Vertical, "c");
    assert_eq!(fx.shape(0, 0), "V(0.500 a, 0.250 b, 0.250 c)");
    let applied = fx.apply(Op::ClosePane {
        pane: fx.pane_id("b"),
        keep_undo: true,
    });
    assert_eq!(fx.shape(0, 0), "V(0.625 a, 0.375 c)");
    assert_eq!(fx.focused(0, 0).as_deref(), Some("a"));
    assert_eq!(effects(&applied, &fx), ["hold b"]);
    assert_eq!(applied.focus, Some(fx.pane_id("a")));
    let first = fx.pane_id("a");
    fx.apply(Op::ClosePane {
        pane: first,
        keep_undo: true,
    });
    assert_eq!(fx.focused(0, 0).as_deref(), Some("c"));
}

#[test]
fn closing_a_pane_with_several_surfaces_holds_only_the_active_one() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let target = fx.pane_id("a");
    let pane = fx.pane(&["b", "c"]);
    fx.apply(Op::Split {
        target,
        direction: Direction::Vertical,
        edge: Edge::After,
        pane,
        focus: true,
    });
    let applied = fx.apply(Op::ClosePane {
        pane: fx.pane_id("b"),
        keep_undo: true,
    });
    assert_eq!(effects(&applied, &fx), ["stop b", "hold c"]);
}

#[test]
fn removing_a_pane_stops_its_sessions_at_once_and_records_nothing() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    let applied = fx.apply(Op::ClosePane {
        pane: fx.pane_id("b"),
        keep_undo: false,
    });
    assert_eq!(fx.shape(0, 0), "a");
    assert_eq!(effects(&applied, &fx), ["stop b"]);
    assert!(fx.model.undo_history().is_empty());
}

#[test]
fn a_close_that_empties_a_tab_gives_it_a_fresh_pane() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let applied = fx.apply(Op::ClosePane {
        pane: fx.pane_id("a"),
        keep_undo: true,
    });
    assert_eq!(fx.shape(0, 0), "fresh");
    let fresh = fx.model.state().workspaces[0].tabs[0]
        .root
        .as_ref()
        .unwrap()
        .pane_ids()[0];
    assert_eq!(effects(&applied, &fx), ["spawn fresh", "hold a"]);
    assert_eq!(applied.focus, Some(fresh));
}

#[test]
fn closing_a_surface_keeps_the_neighbour_active_and_stops_its_session() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let pane = fx.pane_id("a");
    let surface = fx.surface("b");
    fx.apply(Op::AddSurface {
        pane,
        surface: Box::new(surface),
    });
    assert_eq!(fx.shape(0, 0), "a+b*");
    let b = fx.model.state().pane(pane).unwrap().surfaces[1].id;
    let applied = fx.apply(Op::CloseSurface { surface: b });
    assert_eq!(fx.shape(0, 0), "a");
    assert_eq!(effects(&applied, &fx), ["stop b"]);
}

#[test]
fn closing_a_tab_records_it_and_holds_one_surface_per_pane() {
    let mut fx = Fixture::new();
    let ws = fx.workspace("alpha", "a");
    let second = fx.tab(ws, "two", "b");
    fx.split("b", Direction::Vertical, "c");
    let applied = fx.apply(Op::CloseTab {
        tab: second,
        keep_sessions: true,
    });
    assert_eq!(effects(&applied, &fx), ["hold b", "hold c"]);
    assert_eq!(fx.model.state().workspaces[0].tabs.len(), 1);
    assert_eq!(applied.focus, Some(fx.pane_id("a")));
    assert!(
        applied
            .events
            .iter()
            .any(|event| matches!(event, Event::TabClosed { tab, .. } if *tab == second))
    );
}

#[test]
fn closing_a_tab_without_its_sessions_stops_them_and_undo_spawns_fresh() {
    let mut fx = Fixture::new();
    let ws = fx.workspace("alpha", "a");
    let second = fx.tab(ws, "two", "b");
    fx.split("b", Direction::Vertical, "c");
    let applied = fx.apply(Op::CloseTab {
        tab: second,
        keep_sessions: false,
    });
    assert_eq!(effects(&applied, &fx), ["stop b", "stop c"]);
    assert!(fx.model.held_sessions().is_empty());
    let applied = fx.apply(Op::UndoClose { tab: Some(second) });
    assert_eq!(applied.effects.len(), 2);
    assert!(
        applied
            .effects
            .iter()
            .all(|effect| matches!(effect, Effect::Spawn { .. }))
    );
}

#[test]
fn a_forgotten_session_is_neither_held_nor_reattached_by_undo() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    fx.apply(Op::ClosePane {
        pane: fx.pane_id("b"),
        keep_undo: true,
    });
    assert_eq!(fx.model.held_sessions(), [session("b")]);
    fx.model
        .forget_sessions(&std::iter::once(session("b")).collect());
    assert!(fx.model.held_sessions().is_empty());
    let applied = fx.apply(Op::UndoClose { tab: None });
    assert!(matches!(applied.effects.as_slice(), [Effect::Spawn { .. }]));
}

#[test]
fn closing_the_last_tab_leaves_an_empty_placeholder() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let tab = fx.tab_id(0, 0);
    fx.apply(Op::CloseTab {
        tab,
        keep_sessions: true,
    });
    let ws = &fx.model.state().workspaces[0];
    assert_eq!(ws.tabs.len(), 1);
    assert!(ws.tabs[0].root.is_none());
    assert!(ws.is_empty_shell());
}

#[test]
fn closing_a_workspace_stops_every_session_and_moves_to_the_next_one() {
    let mut fx = Fixture::new();
    let alpha = fx.workspace("alpha", "a");
    fx.workspace("beta", "b");
    fx.workspace("gamma", "c");
    fx.apply(Op::SelectWorkspace {
        workspace: alpha,
        focus: true,
    });
    let applied = fx.apply(Op::CloseWorkspace {
        workspace: alpha,
        focus: true,
    });
    assert_eq!(effects(&applied, &fx), ["stop a"]);
    assert_eq!(fx.model.state().active_workspace, 0);
    assert_eq!(fx.model.state().workspaces[0].title, "beta");
    assert_eq!(applied.focus, Some(fx.pane_id("b")));
}

#[test]
fn a_new_tab_fills_an_empty_shell_and_otherwise_appends() {
    let mut fx = Fixture::new();
    let ws = fx.workspace("alpha", "a");
    let first = fx.tab_id(0, 0);
    fx.apply(Op::CloseTab {
        tab: first,
        keep_sessions: true,
    });
    fx.tab(ws, "filled", "b");
    assert_eq!(fx.model.state().workspaces[0].tabs.len(), 1);
    fx.tab(ws, "second", "c");
    let state = fx.model.state();
    assert_eq!(state.workspaces[0].tabs.len(), 2);
    assert_eq!(state.workspaces[0].active_tab, 1);
}

#[test]
fn a_new_tab_can_fill_the_empty_active_tab_and_keep_its_id() {
    let mut fx = Fixture::new();
    let ws = fx.workspace("alpha", "a");
    fx.tab(ws, "two", "b");
    let second = fx.tab_id(0, 1);
    fx.apply(Op::CloseTab {
        tab: fx.tab_id(0, 0),
        keep_sessions: true,
    });
    fx.apply(Op::CloseTab {
        tab: second,
        keep_sessions: true,
    });
    let shell = fx.tab_id(0, 0);
    fx.apply(Op::RenameTab {
        tab: shell,
        title: "kept".into(),
        source: TabTitleSource::User,
    });
    let pane = fx.pane(&["c"]);
    let tab = TabId(fx.model.reserve());
    fx.apply(Op::OpenTab {
        workspace: ws,
        tab: paneflow_layout::NewTab {
            id: tab,
            title: "preset".into(),
            title_source: TabTitleSource::Preset,
            worktree: None,
            root: Some(NewNode::Pane(pane)),
        },
        fill_empty_active: true,
    });
    let state = fx.model.state();
    assert_eq!(state.workspaces[0].tabs.len(), 1);
    assert_eq!(state.workspaces[0].tabs[0].id, shell);
    assert_eq!(state.workspaces[0].tabs[0].title, "kept");
    assert_eq!(fx.shape(0, 0), "c");
}

#[test]
fn the_tab_cap_refuses_a_new_tab() {
    let mut fx = Fixture::new();
    let ws = fx.workspace("alpha", "t0");
    for i in 1..paneflow_layout::MAX_TABS_PER_WORKSPACE {
        fx.tab(ws, &format!("tab {i}"), &format!("t{i}"));
    }
    let pane = fx.pane(&["over"]);
    let tab = TabId(fx.model.reserve());
    let result = fx.try_apply(Op::OpenTab {
        workspace: ws,
        tab: paneflow_layout::NewTab {
            id: tab,
            title: String::new(),
            title_source: TabTitleSource::Preset,
            worktree: None,
            root: Some(NewNode::Pane(pane)),
        },
        fill_empty_active: false,
    });
    assert_eq!(result, Err(OpError::TabLimit));
}

#[test]
fn focus_select_and_rename_change_only_what_they_name() {
    let mut fx = Fixture::new();
    let alpha = fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    let second = fx.tab(alpha, "two", "c");
    let first = fx.tab_id(0, 0);
    fx.apply(Op::SelectTab { tab: first });
    assert_eq!(fx.model.state().workspaces[0].active_tab, 0);
    assert_eq!(fx.focused(0, 0).as_deref(), Some("a"));
    fx.apply(Op::FocusPane {
        pane: fx.pane_id("b"),
    });
    assert_eq!(fx.focused(0, 0).as_deref(), Some("b"));
    let applied = fx.apply(Op::RenameTab {
        tab: second,
        title: "  renamed ".into(),
        source: TabTitleSource::User,
    });
    assert_eq!(fx.model.state().workspaces[0].tabs[1].title, "renamed");
    assert_eq!(applied.events, [Event::TabRenamed { tab: second }]);
    assert_eq!(
        fx.try_apply(Op::RenameTab {
            tab: second,
            title: "auto".into(),
            source: TabTitleSource::Process,
        }),
        Err(OpError::Invalid)
    );
    let surface = fx.surface_id("a");
    let applied = fx.apply(Op::RenameSurface {
        surface,
        name: Some("custom".into()),
    });
    assert_eq!(applied.events, [Event::SurfaceRenamed { surface }]);
    fx.apply(Op::RenameWorkspace {
        workspace: alpha,
        title: "renamed".into(),
    });
    assert_eq!(fx.model.state().workspaces[0].title, "renamed");
}

#[test]
fn binding_a_session_needs_an_unbound_surface_that_exists() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let pane = fx.pane_id("a");
    let surface = fx.model.reserve_surface(SurfaceDefinition::default());
    let unbound = surface.id;
    fx.apply(Op::AddSurface {
        pane,
        surface: Box::new(surface),
    });
    assert_eq!(
        fx.try_apply(Op::BindSession {
            surface: paneflow_layout::SurfaceId(424_242),
            session: session("z"),
        }),
        Err(OpError::NotFound)
    );
    assert_eq!(
        fx.try_apply(Op::BindSession {
            surface: fx.surface_id("a"),
            session: session("z"),
        }),
        Err(OpError::Invalid)
    );
    fx.apply(Op::BindSession {
        surface: unbound,
        session: session("z"),
    });
    assert_eq!(
        fx.model.state().surface(unbound).unwrap().session(),
        Some(&session("z"))
    );
}

#[test]
fn swap_and_move_rearrange_inside_one_tab() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    fx.split("b", Direction::Vertical, "c");
    fx.apply(Op::SwapPanes {
        a: fx.pane_id("a"),
        b: fx.pane_id("c"),
    });
    assert_eq!(fx.shape(0, 0), "V(0.500 c, 0.250 b, 0.250 a)");
    fx.apply(Op::MovePane {
        pane: fx.pane_id("a"),
        target: fx.pane_id("c"),
        direction: Direction::Horizontal,
        edge: Edge::Before,
    });
    assert_eq!(fx.shape(0, 0), "V(0.625 H(0.500 a, 0.500 c), 0.375 b)");
    assert_eq!(fx.focused(0, 0).as_deref(), Some("a"));
    assert_eq!(
        fx.try_apply(Op::SwapPanes {
            a: fx.pane_id("a"),
            b: PaneId(777_777),
        }),
        Err(OpError::NotFound)
    );
}

#[test]
fn moving_a_surface_out_of_a_pane_joins_another_pane_or_splits_it() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let target = fx.pane_id("a");
    let pane = fx.pane(&["b", "c"]);
    fx.apply(Op::Split {
        target,
        direction: Direction::Vertical,
        edge: Edge::After,
        pane,
        focus: true,
    });
    fx.apply(Op::MoveSurface {
        surface: fx.surface_id("c"),
        target: fx.pane_id("a"),
        split: None,
    });
    assert_eq!(fx.shape(0, 0), "V(0.500 a+c*, 0.500 b)");
    let new_pane = PaneId(fx.model.reserve());
    fx.apply(Op::MoveSurface {
        surface: fx.surface_id("c"),
        target: fx.pane_id("b"),
        split: Some((Direction::Horizontal, Edge::After, new_pane)),
    });
    assert_eq!(fx.shape(0, 0), "V(0.500 a, 0.500 H(0.500 b, 0.500 c))");
    fx.apply(Op::MoveSurface {
        surface: fx.surface_id("c"),
        target: fx.pane_id("a"),
        split: None,
    });
    assert_eq!(fx.shape(0, 0), "V(0.500 a+c*, 0.500 b)");
}

#[test]
fn a_move_that_empties_its_tab_closes_it_and_rehomes_the_sessions() {
    let mut fx = Fixture::new();
    let alpha = fx.workspace("alpha", "a");
    let beta = fx.workspace("beta", "b");
    fx.tab(alpha, "two", "c");
    let tab = TabId(fx.model.reserve());
    let applied = fx.apply(Op::MovePaneToNewTab {
        pane: fx.pane_id("c"),
        workspace: beta,
        tab,
        index: None,
    });
    assert_eq!(effects(&applied, &fx), ["rehome c"]);
    assert_eq!(fx.model.state().workspaces[0].tabs.len(), 1);
    assert_eq!(fx.model.state().workspaces[1].tabs.len(), 2);
    let moved = fx.tab_id(1, 1);
    let applied = fx.apply(Op::MoveTab {
        tab: moved,
        workspace: alpha,
        index: Some(0),
    });
    assert_eq!(effects(&applied, &fx), ["rehome c"]);
    assert_eq!(fx.model.state().workspaces[0].tabs[0].id, moved);
    assert_eq!(
        fx.try_apply(Op::MoveTab {
            tab: moved,
            workspace: WorkspaceId(555_555),
            index: None,
        }),
        Err(OpError::NotFound)
    );
}

#[test]
fn moving_a_pane_to_a_new_tab_leaves_zoom_first() {
    let mut fx = Fixture::new();
    let alpha = fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    let tab = fx.tab_id(0, 0);
    fx.apply(Op::SetZoom {
        tab,
        pane: Some(fx.pane_id("a")),
    });
    let new_tab = TabId(fx.model.reserve());
    let applied = fx.apply(Op::MovePaneToNewTab {
        pane: fx.pane_id("b"),
        workspace: alpha,
        tab: new_tab,
        index: None,
    });
    assert_eq!(effects(&applied, &fx), ["rehome b"]);
    let state = fx.model.state();
    assert!(state.workspaces[0].tabs[0].zoomed.is_none());
    assert_eq!(state.workspaces[0].tabs.len(), 2);
}

#[test]
fn a_reorder_past_the_end_is_ignored_and_a_reorder_to_the_end_moves() {
    let mut fx = Fixture::new();
    let ws = fx.workspace("alpha", "a");
    let b = fx.tab(ws, "b", "b");
    fx.tab(ws, "c", "c");
    let first = fx.tab_id(0, 0);
    fx.apply(Op::ReorderTab { tab: first, to: 9 });
    assert_eq!(fx.model.state().workspaces[0].tabs[0].id, first);
    fx.apply(Op::ReorderTab { tab: first, to: 3 });
    let tabs: Vec<TabId> = fx.model.state().workspaces[0]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect();
    assert_eq!(tabs[0], b);
    assert_eq!(tabs[2], first);
    assert_eq!(fx.model.state().workspaces[0].active_tab, 1);
}

#[test]
fn reordering_workspaces_keeps_the_shown_one_shown() {
    let mut fx = Fixture::new();
    let alpha = fx.workspace("alpha", "a");
    fx.workspace("beta", "b");
    let gamma = fx.workspace("gamma", "c");
    fx.apply(Op::ReorderWorkspace {
        workspace: alpha,
        to: 2,
    });
    let titles: Vec<&str> = fx
        .model
        .state()
        .workspaces
        .iter()
        .map(|ws| ws.title.as_str())
        .collect();
    assert_eq!(titles, ["beta", "gamma", "alpha"]);
    assert_eq!(
        fx.model.state().workspaces[fx.model.state().active_workspace].id,
        gamma
    );
}

fn space(n: u8) -> SpaceId {
    SpaceId::parse(&format!("550e8400-e29b-41d4-a716-4466554400{n:02}")).unwrap()
}

fn mixed_spaces() -> [(u64, Option<SpaceId>); 5] {
    [
        (10, Some(space(1))),
        (11, None),
        (12, Some(space(1))),
        (13, Some(space(2))),
        (14, None),
    ]
}

fn mixed(fx: &mut Fixture) -> Vec<WorkspaceId> {
    mixed_spaces()
        .into_iter()
        .map(|(n, space)| {
            let name = format!("ws{n}");
            fx.workspace_in(&name, &name, space)
        })
        .collect()
}

fn order(fx: &Fixture) -> Vec<String> {
    fx.model
        .state()
        .workspaces
        .iter()
        .map(|ws| ws.title.clone())
        .collect()
}

fn shown(fx: &Fixture) -> Option<usize> {
    let state = fx.model.state();
    state
        .workspaces
        .get(state.active_workspace)
        .filter(|ws| ws.space == state.active_space)
        .map(|_| state.active_workspace)
}

fn shown_title(fx: &Fixture) -> Option<String> {
    shown(fx).map(|idx| fx.model.state().workspaces[idx].title.clone())
}

fn select(fx: &mut Fixture, workspace: WorkspaceId) -> bool {
    fx.apply(Op::SelectWorkspace {
        workspace,
        focus: false,
    })
    .events
    .iter()
    .any(|event| matches!(event, Event::WorkspaceSelected { workspace: shown } if *shown == workspace))
}

fn close(fx: &mut Fixture, workspace: WorkspaceId) {
    fx.apply(Op::CloseWorkspace {
        workspace,
        focus: false,
    });
}

fn reorder(fx: &mut Fixture, workspace: WorkspaceId, to: usize) {
    fx.apply(Op::ReorderWorkspace { workspace, to });
}

#[test]
fn activating_a_workspace_of_another_space_switches_to_that_space() {
    let mut fx = Fixture::new();
    let ws = mixed(&mut fx);
    select(&mut fx, ws[0]);

    assert!(select(&mut fx, ws[3]));
    assert_eq!(fx.model.state().active_space, Some(space(2)));
    assert_eq!(shown(&fx), Some(3));

    assert!(select(&mut fx, ws[4]));
    assert_eq!(fx.model.state().active_space, None);
    assert_eq!(shown(&fx), Some(4));

    assert!(!select(&mut fx, ws[4]), "already shown");
    assert!(select(&mut fx, ws[1]));
    assert_eq!(fx.model.state().active_space, None, "same space, no switch");
    assert_eq!(shown(&fx), Some(1));
}

#[test]
fn activating_from_an_empty_space_switches_and_reports_a_change() {
    let mut fx = Fixture::new();
    let spaces = mixed_spaces();
    let mut ws = vec![fx.workspace_in("ws10", "ws10", spaces[0].1.clone())];
    let lone = fx.workspace_in("ws15", "ws15", Some(space(3)));
    for (n, space) in spaces.into_iter().skip(1) {
        let name = format!("ws{n}");
        ws.push(fx.workspace_in(&name, &name, space));
    }
    select(&mut fx, lone);
    close(&mut fx, lone);
    assert_eq!(order(&fx), ["ws10", "ws11", "ws12", "ws13", "ws14"]);
    assert_eq!(fx.model.state().active_space, Some(space(3)));
    assert_eq!(fx.model.state().active_workspace, 1);
    assert_eq!(shown(&fx), None);

    assert!(
        select(&mut fx, ws[1]),
        "the index already points at ws11, yet it was hidden, so this is a change"
    );
    assert_eq!(fx.model.state().active_space, None);
    assert_eq!(shown(&fx), Some(1));
}

#[test]
fn every_activation_path_leaves_the_shown_workspace_in_the_active_space() {
    let len = mixed_spaces().len();
    for start in 0..len {
        for target in 0..len {
            let mut fx = Fixture::new();
            let ws = mixed(&mut fx);
            select(&mut fx, ws[start]);
            select(&mut fx, ws[target]);
            let shown = shown(&fx).expect("a workspace shows");
            let state = fx.model.state();
            assert_eq!(state.workspaces[shown].space, state.active_space);
            assert_eq!(shown, target, "start {start} target {target}");
        }
    }
}

#[test]
fn closing_the_shown_workspace_shows_its_nearest_neighbor_in_the_space() {
    let mut fx = Fixture::new();
    let ws = mixed(&mut fx);
    select(&mut fx, ws[2]);
    close(&mut fx, ws[2]);
    assert_eq!(
        shown_title(&fx).as_deref(),
        Some("ws10"),
        "no following workspace of the space, so the preceding one"
    );
    assert_eq!(fx.model.state().active_workspace, 0);

    let mut fx = Fixture::new();
    let ws = mixed(&mut fx);
    select(&mut fx, ws[1]);
    close(&mut fx, ws[1]);
    assert_eq!(
        shown_title(&fx).as_deref(),
        Some("ws14"),
        "the next default workspace, skipping the other spaces"
    );
    assert_eq!(fx.model.state().active_workspace, 3);

    let mut fx = Fixture::new();
    let ws = mixed(&mut fx);
    select(&mut fx, ws[3]);
    close(&mut fx, ws[3]);
    let state = fx.model.state();
    assert!(state.active_workspace < state.workspaces.len());
    assert_eq!(state.active_space, Some(space(2)));
    assert_eq!(shown(&fx), None, "the space is now empty");
}

#[test]
fn closing_another_workspace_keeps_the_shown_one() {
    let mut fx = Fixture::new();
    let ws = mixed(&mut fx);
    select(&mut fx, ws[3]);
    close(&mut fx, ws[0]);
    assert_eq!(fx.model.state().active_workspace, 2);
    assert_eq!(shown_title(&fx).as_deref(), Some("ws13"));

    let mut fx = Fixture::new();
    let only = fx.workspace("only", "only");
    close(&mut fx, only);
    assert!(fx.model.state().workspaces.is_empty());
    assert_eq!(fx.model.state().active_workspace, 0);
}

#[test]
fn without_spaces_closing_matches_the_previous_index_rule() {
    for len in 1..5usize {
        for removed in 0..len {
            for active in 0..len {
                let mut fx = Fixture::new();
                let ws: Vec<WorkspaceId> = (0..len)
                    .map(|n| {
                        let name = format!("ws{n}");
                        fx.workspace(&name, &name)
                    })
                    .collect();
                select(&mut fx, ws[active]);
                close(&mut fx, ws[removed]);
                let remaining = len - 1;
                let previous = if remaining == 0 {
                    0
                } else if active >= remaining {
                    remaining - 1
                } else if active > removed {
                    active - 1
                } else {
                    active
                };
                assert_eq!(
                    fx.model.state().active_workspace,
                    previous,
                    "len {len} removed {removed} active {active}"
                );
            }
        }
    }
}

#[test]
fn reordering_inside_a_space_leaves_other_spaces_in_place() {
    let mut fx = Fixture::new();
    let ws = mixed(&mut fx);
    select(&mut fx, ws[3]);
    reorder(&mut fx, ws[0], 2);
    assert_eq!(order(&fx), ["ws12", "ws11", "ws10", "ws13", "ws14"]);
    assert_eq!(
        fx.model.state().active_workspace,
        3,
        "the shown workspace keeps its identity"
    );
    assert_eq!(shown_title(&fx).as_deref(), Some("ws13"));

    let mut fx = Fixture::new();
    let ws = mixed(&mut fx);
    select(&mut fx, ws[0]);
    reorder(&mut fx, ws[4], 0);
    assert_eq!(order(&fx), ["ws10", "ws14", "ws12", "ws13", "ws11"]);
    assert_eq!(shown_title(&fx).as_deref(), Some("ws10"));
}

#[test]
fn the_trailing_drop_slot_moves_a_workspace_last_in_its_space() {
    let mut fx = Fixture::new();
    let ws = mixed(&mut fx);
    select(&mut fx, ws[1]);
    let len = ws.len();
    reorder(&mut fx, ws[1], len - 1);
    assert_eq!(order(&fx), ["ws10", "ws14", "ws12", "ws13", "ws11"]);
    assert_eq!(shown_title(&fx).as_deref(), Some("ws11"));
}

#[test]
fn without_spaces_reordering_matches_a_plain_move() {
    let mut fx = Fixture::new();
    let ws: Vec<WorkspaceId> = (0..4)
        .map(|n| {
            let name = format!("ws{n}");
            fx.workspace(&name, &name)
        })
        .collect();
    select(&mut fx, ws[2]);
    reorder(&mut fx, ws[0], 3);
    assert_eq!(order(&fx), ["ws1", "ws2", "ws3", "ws0"]);
    assert_eq!(fx.model.state().active_workspace, 1);

    let before = fx.model.state().clone();
    let applied = fx.apply(Op::ReorderWorkspace {
        workspace: ws[2],
        to: 1,
    });
    assert_eq!(
        fx.model.state(),
        &before,
        "a move to its own slot is a no-op"
    );
    assert!(applied.events.is_empty());

    assert_eq!(
        fx.try_apply(Op::ReorderWorkspace {
            workspace: WorkspaceId(999_999),
            to: 0,
        }),
        Err(OpError::NotFound),
        "an unknown workspace is refused"
    );
    assert_eq!(fx.model.state(), &before);
}

#[test]
fn ratios_are_clamped_equalized_and_rebuilt_by_presets() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    fx.split("b", Direction::Vertical, "c");
    let split = split_id(&fx);
    fx.apply(Op::SetRatios {
        split,
        ratios: vec![0.0, 0.5, 0.5],
    });
    assert_eq!(fx.shape(0, 0), "V(0.010 a, 0.495 b, 0.495 c)");
    let tab = fx.tab_id(0, 0);
    fx.apply(Op::Equalize { tab });
    assert_eq!(fx.shape(0, 0), "V(0.333 a, 0.333 b, 0.333 c)");
    fx.apply(Op::ApplyPreset {
        tab,
        preset: Preset::EvenVertical,
    });
    assert_eq!(fx.shape(0, 0), "H(0.333 a, 0.333 b, 0.333 c)");
    fx.apply(Op::ApplyPreset {
        tab,
        preset: Preset::Tiled,
    });
    assert_eq!(fx.shape(0, 0), "H(0.500 V(0.500 a, 0.500 b), 0.500 c)");
    fx.apply(Op::FocusPane {
        pane: fx.pane_id("c"),
    });
    fx.apply(Op::ApplyPreset {
        tab,
        preset: Preset::MainVertical,
    });
    assert_eq!(fx.shape(0, 0), "V(0.500 c, 0.500 H(0.500 a, 0.500 b))");
    assert_eq!(fx.focused(0, 0).as_deref(), Some("c"));
    fx.apply(Op::ApplyPreset {
        tab,
        preset: Preset::EvenHorizontal,
    });
    assert_eq!(fx.shape(0, 0), "V(0.333 c, 0.333 a, 0.333 b)");
}

fn split_id(fx: &Fixture) -> SplitId {
    match fx.model.state().workspaces[0].tabs[0]
        .root
        .as_ref()
        .unwrap()
    {
        Node::Split(split) => split.id,
        Node::Pane(_) => panic!("a split root"),
    }
}

#[test]
fn zoom_is_a_flag_that_leaves_the_tree_and_the_export_alone() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    let before = export(&fx.model, &[]);
    let tree = fx.model.state().workspaces[0].tabs[0].root.clone();
    let tab = fx.tab_id(0, 0);
    let applied = fx.apply(Op::SetZoom {
        tab,
        pane: Some(fx.pane_id("b")),
    });
    assert_eq!(fx.model.state().workspaces[0].tabs[0].root, tree);
    assert_eq!(export(&fx.model, &[]), before);
    assert_eq!(applied.events, [Event::LayoutChanged { tab }]);
    fx.apply(Op::ApplyPreset {
        tab,
        preset: Preset::EvenVertical,
    });
    assert!(fx.model.state().workspaces[0].tabs[0].zoomed.is_none());
}

#[test]
fn restore_layout_reuses_panes_in_order_and_stops_the_surplus() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    fx.split("b", Direction::Vertical, "c");
    let tab = fx.tab_id(0, 0);
    let layout = LayoutNode::Split {
        direction: "horizontal".into(),
        ratio: None,
        ratios: Some(vec![0.3, 0.7]),
        children: vec![
            LayoutNode::Pane {
                surfaces: vec![SurfaceDefinition::default()],
            },
            LayoutNode::Pane {
                surfaces: vec![SurfaceDefinition::default()],
            },
        ],
    };
    let node = fx.model.reserve_node(&layout);
    let applied = fx.apply(Op::RestoreLayout { tab, layout: node });
    assert_eq!(fx.shape(0, 0), "H(0.300 a, 0.700 b)");
    assert_eq!(effects(&applied, &fx), ["stop c"]);

    let mut wide = LayoutNode::Pane {
        surfaces: vec![SurfaceDefinition::default()],
    };
    for _ in 0..MAX_PANES {
        wide = LayoutNode::Split {
            direction: "vertical".into(),
            ratio: None,
            ratios: None,
            children: vec![
                wide,
                LayoutNode::Pane {
                    surfaces: vec![SurfaceDefinition::default()],
                },
            ],
        };
    }
    let before = fx.model.clone();
    let node = fx.model.reserve_node(&wide);
    let reserved = fx.model.clone();
    assert_eq!(
        fx.try_apply(Op::RestoreLayout { tab, layout: node }),
        Err(OpError::PaneLimit)
    );
    assert_eq!(fx.model, reserved);
    assert_eq!(fx.model.state(), before.state());
}

#[test]
fn undo_within_the_hold_reattaches_and_after_it_spawns() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    fx.apply(Op::ClosePane {
        pane: fx.pane_id("b"),
        keep_undo: true,
    });
    fx.advance(4_000);
    let applied = fx.apply(Op::UndoClose { tab: None });
    assert_eq!(effects(&applied, &fx), ["reattach b"]);
    assert_eq!(fx.shape(0, 0), "H(0.500 a, 0.500 b)");
    assert_eq!(fx.focused(0, 0).as_deref(), Some("b"));
    assert_eq!(
        fx.model
            .state()
            .surface(fx.surface_id("b"))
            .unwrap()
            .session(),
        Some(&session("b"))
    );

    fx.apply(Op::ClosePane {
        pane: fx.pane_id("b"),
        keep_undo: true,
    });
    fx.advance(5_000);
    assert_eq!(
        fx.model.expire_holds(fx.now),
        [Effect::Stop {
            session: session("b")
        }]
    );
    let applied = fx.apply(Op::UndoClose { tab: None });
    let restored = fx.pane_id("b");
    let surface = fx.model.state().pane(restored).unwrap().surfaces[0].clone();
    assert_eq!(surface.session(), None);
    assert_eq!(
        applied.effects,
        [Effect::Spawn {
            surface: surface.id
        }]
    );
}

#[test]
fn a_sixth_record_evicts_the_oldest_and_stops_its_held_session() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "keep");
    let names = ["r1", "r2", "r3", "r4", "r5", "r6"];
    let mut last = None;
    for name in names {
        fx.split("keep", Direction::Vertical, name);
        last = Some(fx.apply(Op::ClosePane {
            pane: fx.pane_id(name),
            keep_undo: true,
        }));
    }
    let applied = last.unwrap();
    assert_eq!(effects(&applied, &fx), ["stop r1", "hold r6"]);
    assert_eq!(fx.model.undo_history().len(), 5);
}

#[test]
fn undo_with_an_empty_history_fails_and_changes_nothing() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let before = fx.model.clone();
    assert_eq!(
        fx.try_apply(Op::UndoClose { tab: None }),
        Err(OpError::NotFound)
    );
    assert_eq!(fx.model, before);
}

#[test]
fn undo_is_refused_on_a_zoomed_tab_and_keeps_the_record() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    fx.split("b", Direction::Vertical, "c");
    fx.apply(Op::ClosePane {
        pane: fx.pane_id("c"),
        keep_undo: true,
    });
    let tab = fx.tab_id(0, 0);
    fx.apply(Op::SetZoom {
        tab,
        pane: Some(fx.pane_id("a")),
    });
    assert_eq!(
        fx.try_apply(Op::UndoClose { tab: None }),
        Err(OpError::Zoomed)
    );
    assert_eq!(fx.model.undo_history().len(), 1);
}

#[test]
fn a_closed_tab_comes_back_at_its_place_with_its_layout() {
    let mut fx = Fixture::new();
    let ws = fx.workspace("alpha", "a");
    let second = fx.tab(ws, "two", "b");
    fx.split("b", Direction::Vertical, "c");
    fx.tab(ws, "three", "d");
    fx.apply(Op::CloseTab {
        tab: second,
        keep_sessions: true,
    });
    let applied = fx.apply(Op::UndoClose { tab: Some(second) });
    assert_eq!(effects(&applied, &fx), ["reattach b", "reattach c"]);
    let state = fx.model.state();
    assert_eq!(state.workspaces[0].tabs[1].id, second);
    assert_eq!(state.workspaces[0].active_tab, 1);
    assert_eq!(fx.shape(0, 1), "V(0.500 b, 0.500 c)");
}

#[test]
fn launching_a_template_replaces_the_active_tab_tree() {
    let mut fx = Fixture::new();
    let ws = fx.workspace("alpha", "a");
    let left = fx.pane(&["left"]);
    let right = fx.pane(&["right"]);
    let focus = right.id;
    let applied = fx.apply(Op::LaunchTemplate {
        workspace: ws,
        root: NewNode::Split {
            direction: Direction::Vertical,
            ratios: vec![0.5, 0.5],
            children: vec![NewNode::Pane(left), NewNode::Pane(right)],
        },
        focus: Some(focus),
        cwd: Some("/work/template".into()),
    });
    assert_eq!(fx.shape(0, 0), "V(0.500 left, 0.500 right)");
    assert_eq!(effects(&applied, &fx), ["spawn left", "spawn right"]);
    assert_eq!(applied.focus, Some(focus));
    assert_eq!(fx.model.state().workspaces[0].cwd, "/work/template");
}

#[test]
fn an_op_that_reuses_a_consumed_id_is_invalid() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let target = fx.pane_id("a");
    let pane = fx.pane(&["b"]);
    fx.apply(Op::Split {
        target,
        direction: Direction::Vertical,
        edge: Edge::After,
        pane: pane.clone(),
        focus: true,
    });
    assert_eq!(
        fx.try_apply(Op::Split {
            target,
            direction: Direction::Vertical,
            edge: Edge::After,
            pane,
            focus: true,
        }),
        Err(OpError::Invalid)
    );
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn check_invariants(model: &paneflow_layout::Model) {
    let state = model.state();
    let mut ids = std::collections::BTreeSet::new();
    let mut claim = |id: u64| assert!(ids.insert(id), "id {id} appears twice");
    assert!(state.workspaces.is_empty() || state.active_workspace < state.workspaces.len());
    for ws in &state.workspaces {
        claim(ws.id.get());
        assert!(!ws.tabs.is_empty());
        assert!(ws.active_tab < ws.tabs.len(), "active tab out of range");
        for tab in &ws.tabs {
            claim(tab.id.get());
            let Some(root) = &tab.root else {
                assert!(tab.focused.is_none() || tab.zoomed.is_none());
                continue;
            };
            if let Some(focused) = tab.focused {
                assert!(root.contains(focused), "focused pane outside its tree");
            }
            if let Some(zoomed) = tab.zoomed {
                assert!(root.contains(zoomed), "zoomed pane outside its tree");
            }
            check_node(root, &mut claim);
        }
    }
}

fn check_node(node: &Node, claim: &mut impl FnMut(u64)) {
    match node {
        Node::Pane(pane) => {
            claim(pane.id.get());
            assert!(!pane.surfaces.is_empty());
            assert!(pane.active < pane.surfaces.len());
            for surface in &pane.surfaces {
                claim(surface.id.get());
            }
        }
        Node::Split(split) => {
            claim(split.id.get());
            assert!(split.children.len() >= 2, "a split with one child");
            let sum: f64 = split.children.iter().map(|c| f64::from(c.ratio)).sum();
            assert!((sum - 1.0).abs() < 1e-6, "ratios sum to {sum}");
            for child in &split.children {
                check_node(&child.node, claim);
            }
        }
    }
}

fn random_op(fx: &mut Fixture, rng: &mut Rng) -> Option<Op> {
    let state = fx.model.state().clone();
    let ws_idx = rng.below(state.workspaces.len());
    let ws = state.workspaces.get(ws_idx)?;
    let tab = &ws.tabs[rng.below(ws.tabs.len())];
    let panes: Vec<PaneId> = tab.root.as_ref().map(Node::pane_ids).unwrap_or_default();
    let pick = |rng: &mut Rng| panes.get(rng.below(panes.len())).copied();
    let direction = if rng.below(2) == 0 {
        Direction::Vertical
    } else {
        Direction::Horizontal
    };
    let edge = if rng.below(2) == 0 {
        Edge::Before
    } else {
        Edge::After
    };
    let name = format!("n{}", fx.model.next_id());
    Some(match rng.below(17) {
        0..=3 => {
            let target = pick(rng)?;
            let pane = fx.pane(&[name.as_str()]);
            Op::Split {
                target,
                direction,
                edge,
                pane,
                focus: rng.below(2) == 0,
            }
        }
        4 => Op::ClosePane {
            pane: pick(rng)?,
            keep_undo: rng.below(2) == 0,
        },
        5 => Op::SwapPanes {
            a: pick(rng)?,
            b: pick(rng)?,
        },
        6 => Op::MovePane {
            pane: pick(rng)?,
            target: pick(rng)?,
            direction,
            edge,
        },
        7 => Op::SetZoom {
            tab: tab.id,
            pane: if rng.below(2) == 0 { pick(rng) } else { None },
        },
        8 => Op::ApplyPreset {
            tab: tab.id,
            preset: [
                Preset::EvenHorizontal,
                Preset::EvenVertical,
                Preset::Tiled,
                Preset::MainVertical,
            ][rng.below(4)],
        },
        9 => Op::Equalize { tab: tab.id },
        10 => {
            let split = tab.root.as_ref().map(Node::split_ids).unwrap_or_default();
            let split = *split.get(rng.below(split.len()))?;
            let count = tab.root.as_ref()?.split(split)?.children.len();
            Op::SetRatios {
                split,
                ratios: (0..count).map(|_| (rng.below(100) as f32) / 50.0).collect(),
            }
        }
        11 => {
            let tab_id = TabId(fx.model.reserve());
            let pane = fx.pane(&[name.as_str()]);
            Op::OpenTab {
                workspace: ws.id,
                tab: paneflow_layout::NewTab {
                    id: tab_id,
                    title: String::new(),
                    title_source: TabTitleSource::Preset,
                    worktree: None,
                    root: Some(NewNode::Pane(pane)),
                },
                fill_empty_active: rng.below(2) == 0,
            }
        }
        12 => Op::CloseTab {
            tab: tab.id,
            keep_sessions: true,
        },
        13 => Op::UndoClose { tab: None },
        14 => {
            let target = state.workspaces.get(rng.below(state.workspaces.len()))?.id;
            let tab = TabId(fx.model.reserve());
            Op::MovePaneToNewTab {
                pane: pick(rng)?,
                workspace: target,
                tab,
                index: None,
            }
        }
        15 => {
            let pane = pick(rng)?;
            let surfaces = &tab.root.as_ref()?.pane(pane)?.surfaces;
            if rng.below(2) == 0 {
                let surface = fx.surface(&name);
                Op::AddSurface {
                    pane,
                    surface: Box::new(surface),
                }
            } else {
                Op::CloseSurface {
                    surface: surfaces[rng.below(surfaces.len())].id,
                }
            }
        }
        _ => {
            let target = state.workspaces.get(rng.below(state.workspaces.len()))?.id;
            Op::MoveTab {
                tab: tab.id,
                workspace: target,
                index: Some(rng.below(4)),
            }
        }
    })
}

#[test]
fn a_thousand_random_ops_keep_every_invariant() {
    for seed in [0x9e37_79b9_7f4a_7c15_u64, 0x2545_f491_4f6c_dd1d, 42] {
        let mut fx = Fixture::new();
        fx.workspace("alpha", "a");
        fx.workspace("beta", "b");
        let mut rng = Rng(seed);
        let mut applied = 0usize;
        let mut attempts = 0usize;
        while applied < 1_000 {
            attempts += 1;
            assert!(
                attempts < 20_000,
                "only {applied} ops applied for seed {seed}"
            );
            fx.advance(rng.below(3_000) as u64);
            let Some(op) = random_op(&mut fx, &mut rng) else {
                continue;
            };
            let (state, revision, history) = (
                fx.model.state().clone(),
                fx.model.revision(),
                fx.model.undo_history().clone(),
            );
            match fx.model.apply(op, fx.now) {
                Ok(_) => {
                    applied += 1;
                    assert_eq!(fx.model.revision(), revision + 1);
                }
                Err(_) => {
                    assert_eq!(fx.model.state(), &state);
                    assert_eq!(fx.model.revision(), revision);
                    assert_eq!(fx.model.undo_history(), &history);
                }
            }
            check_invariants(&fx.model);
        }
    }
}

#[test]
fn a_spawned_surface_takes_its_session_within_the_same_revision() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "keep");
    let applied = fx.apply(Op::ClosePane {
        pane: fx.pane_id("keep"),
        keep_undo: false,
    });
    let surface = applied
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Spawn { surface } => Some(*surface),
            _ => None,
        })
        .expect("closing the last pane spawns a fresh one");
    let revision = fx.model.revision();
    assert!(fx.model.complete_spawn(surface, session("fresh")));
    assert_eq!(fx.model.revision(), revision);
    assert_eq!(
        fx.model.state().surface(surface).and_then(|s| s.session()),
        Some(&session("fresh"))
    );
    assert!(!fx.model.complete_spawn(surface, session("other")));
    assert!(
        !fx.model
            .complete_spawn(SurfaceId(u64::MAX), session("other"))
    );
}

#[test]
fn a_model_on_a_shared_id_source_never_hands_out_an_id_taken_outside_it() {
    let ids = paneflow_layout::IdSource::starting_at(1);
    let mut model = paneflow_layout::Model::new().with_ids(ids.clone());
    let outside = ids.allocate();
    let inside = model.reserve();
    assert_ne!(inside, outside);
    assert!(ids.next_value() > inside);
    ids.claim(500);
    assert!(model.reserve() > 500);
    assert!(model.reserve_exact(42));
    assert!(
        !model.reserve_exact(42),
        "a reserved id is not reserved twice"
    );
    assert!(!model.reserve_exact(0));
}

#[test]
fn a_failed_op_leaves_the_id_counter_where_it_was() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    let (left, right) = (fx.pane(&["left"]), fx.pane(&["right"]));
    let tab = TabId(fx.model.reserve());
    let before = fx.model.clone();
    let next = fx.model.next_id();
    assert_eq!(
        fx.try_apply(Op::OpenTab {
            workspace: WorkspaceId(9_999),
            tab: paneflow_layout::NewTab {
                id: tab,
                title: String::new(),
                title_source: TabTitleSource::Preset,
                worktree: None,
                root: Some(NewNode::Split {
                    direction: Direction::Vertical,
                    ratios: vec![0.5, 0.5],
                    children: vec![NewNode::Pane(left), NewNode::Pane(right)],
                }),
            },
            fill_empty_active: false,
        }),
        Err(OpError::NotFound)
    );
    assert_eq!(fx.model.next_id(), next);
    assert_eq!(fx.model, before);
}

#[test]
fn an_id_held_by_the_undo_history_cannot_be_reserved_again() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    let closed = fx.pane_id("b");
    fx.apply(Op::ClosePane {
        pane: closed,
        keep_undo: true,
    });
    assert!(fx.model.state().pane(closed).is_none());
    assert!(!fx.model.reserve_exact(closed.get()));
    fx.apply(Op::UndoClose { tab: None });
    assert!(fx.model.state().pane(closed).is_some());
}

#[test]
fn a_reopened_tab_keeps_its_worktree() {
    let mut fx = Fixture::new();
    let ws = fx.workspace("alpha", "a");
    let tab = fx.tab(ws, "work", "b");
    fx.apply(Op::BindWorktree {
        tab,
        worktree: Some("/repo/wt-a".to_string()),
    });
    fx.apply(Op::CloseTab {
        tab,
        keep_sessions: true,
    });
    assert!(fx.model.state().tab(tab).is_none());
    fx.apply(Op::UndoClose { tab: Some(tab) });
    assert_eq!(
        fx.model
            .state()
            .tab(tab)
            .and_then(|tab| tab.worktree.clone()),
        Some("/repo/wt-a".to_string())
    );
}

#[test]
fn every_event_of_an_op_is_scoped_with_the_ids_it_concerns_even_once_closed() {
    let mut fx = Fixture::new();
    let workspace = fx.workspace("one", "a");
    let tab = fx.tab_id(0, 0);
    let split = fx.split("a", Direction::Horizontal, "b");
    assert_eq!(split.scopes.len(), split.events.len());
    let b = fx.pane_id("b");
    let b_surface = fx.surface_id("b");
    for (event, scope) in split.events.iter().zip(&split.scopes) {
        assert_eq!(scope.workspace, Some(workspace), "{event:?}");
        assert_eq!(scope.tab, Some(tab), "{event:?}");
        match event {
            Event::PaneCreated { pane, .. } | Event::PaneFocused { pane, .. } => {
                assert_eq!(scope.pane, Some(*pane));
            }
            Event::SurfaceCreated { surface, .. } => {
                assert_eq!(scope.pane, Some(b));
                assert_eq!(scope.surface, Some(*surface));
                assert_eq!(scope.session, Some(session("b")));
            }
            Event::LayoutChanged { .. } => assert_eq!(scope.pane, None),
            other => panic!("unexpected event of a split: {other:?}"),
        }
    }

    let closed = fx.apply(Op::ClosePane {
        pane: b,
        keep_undo: true,
    });
    let surface_closed = closed
        .events
        .iter()
        .position(|event| matches!(event, Event::SurfaceClosed { .. }))
        .expect("closing a pane closes its surface");
    let scope = &closed.scopes[surface_closed];
    assert_eq!(scope.workspace, Some(workspace));
    assert_eq!(scope.tab, Some(tab));
    assert_eq!(scope.pane, Some(b));
    assert_eq!(scope.surface, Some(b_surface));
    assert_eq!(
        scope.session,
        Some(session("b")),
        "a closed surface is resolved in the state before the op"
    );
}
