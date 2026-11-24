#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Fixture, effects, session};
use paneflow_layout::{Direction, MAX_PANES, Op, OpError};

fn close_kept(fx: &mut Fixture, name: &str) {
    fx.apply(Op::ClosePane {
        pane: fx.pane_id(name),
        keep_undo: true,
    });
}

#[test]
fn a_closed_pane_returns_to_its_own_workspace_not_the_shown_one() {
    let mut fx = Fixture::new();
    fx.workspace("first", "a");
    fx.split("a", Direction::Vertical, "b");
    close_kept(&mut fx, "b");
    fx.workspace("second", "c");
    assert_eq!(fx.model.state().active_workspace, 1);
    let applied = fx.apply(Op::UndoClose { tab: None });
    assert_eq!(effects(&applied, &fx), ["reattach b"]);
    assert_eq!(fx.model.state().active_workspace, 0);
    assert_eq!(fx.shape(0, 0), "H(0.500 a, 0.500 b)");
    assert_eq!(fx.shape(1, 0), "c");
}

#[test]
fn a_pane_whose_tab_left_returns_to_the_active_tab_of_its_workspace() {
    let mut fx = Fixture::new();
    let alpha = fx.workspace("alpha", "a");
    let moved = fx.tab(alpha, "two", "b");
    fx.split("b", Direction::Vertical, "c");
    close_kept(&mut fx, "c");
    let beta = fx.workspace("beta", "d");
    fx.apply(Op::MoveTab {
        tab: moved,
        workspace: beta,
        index: None,
    });
    let applied = fx.apply(Op::UndoClose { tab: None });
    assert_eq!(effects(&applied, &fx), ["reattach c"]);
    assert_eq!(fx.model.state().active_workspace, 0);
    assert_eq!(fx.shape(0, 0), "H(0.500 a, 0.500 c)");
}

#[test]
fn a_pane_cannot_return_to_a_full_tab_and_keeps_its_record() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "p0");
    fx.split("p0", Direction::Vertical, "gone");
    close_kept(&mut fx, "gone");
    for idx in 1..MAX_PANES {
        let previous = format!("p{}", idx - 1);
        fx.split(&previous, Direction::Horizontal, &format!("p{idx}"));
    }
    let history = fx.model.undo_history().len();
    assert_eq!(
        fx.try_apply(Op::UndoClose { tab: None }).err(),
        Some(OpError::PaneLimit)
    );
    assert_eq!(fx.model.undo_history().len(), history);
}

#[test]
fn a_pane_of_a_closed_workspace_returns_to_the_shown_workspace_and_keeps_its_session() {
    let mut fx = Fixture::new();
    fx.workspace("first", "a");
    let second = fx.workspace("second", "b");
    fx.split("b", Direction::Vertical, "c");
    close_kept(&mut fx, "c");
    fx.apply(Op::CloseWorkspace {
        workspace: second,
        focus: true,
    });
    let applied = fx.apply(Op::UndoClose { tab: None });
    assert_eq!(effects(&applied, &fx), ["reattach c"]);
    assert_eq!(fx.model.state().workspaces.len(), 1);
    assert_eq!(fx.model.state().workspaces[0].tabs[0].pane_count(), 2);
}

#[test]
fn a_pane_restored_into_another_worktree_stops_its_session_and_opens_a_fresh_shell() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    let tab = fx.tab_id(0, 0);
    fx.apply(Op::BindWorktree {
        tab,
        worktree: Some("/repo/wt-a".to_string()),
    });
    close_kept(&mut fx, "b");
    fx.apply(Op::BindWorktree {
        tab,
        worktree: Some("/repo/wt-b".to_string()),
    });
    let applied = fx.apply(Op::UndoClose { tab: None });
    assert_eq!(effects(&applied, &fx), ["stop b", "spawn b"]);
    assert!(fx.model.held_sessions().is_empty());
}

#[test]
fn a_pane_restored_into_its_own_worktree_reattaches() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    let tab = fx.tab_id(0, 0);
    fx.apply(Op::BindWorktree {
        tab,
        worktree: Some("/repo/wt-a".to_string()),
    });
    close_kept(&mut fx, "b");
    let applied = fx.apply(Op::UndoClose { tab: None });
    assert_eq!(effects(&applied, &fx), ["reattach b"]);
}

#[test]
fn a_session_closed_again_belongs_only_to_its_newest_record() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    close_kept(&mut fx, "b");
    fx.apply(Op::UndoClose { tab: None });
    close_kept(&mut fx, "b");
    assert_eq!(fx.model.held_sessions(), [session("b")]);
    let newest: Vec<_> = fx
        .model
        .undo_history()
        .records()
        .map(|record| record.sessions())
        .collect();
    assert_eq!(newest, [vec![session("b")]]);
    let applied = fx.apply(Op::UndoClose { tab: None });
    assert_eq!(effects(&applied, &fx), ["reattach b"]);
}

#[test]
fn a_closed_tab_counts_as_one_record_toward_the_cap() {
    let mut fx = Fixture::new();
    let ws = fx.workspace("alpha", "keep");
    let wide = fx.tab(ws, "wide", "t1");
    fx.split("t1", Direction::Vertical, "t2");
    fx.split("t2", Direction::Vertical, "t3");
    fx.apply(Op::CloseTab {
        tab: wide,
        keep_sessions: true,
    });
    assert_eq!(fx.model.undo_history().len(), 1);
    for name in ["r1", "r2", "r3", "r4"] {
        fx.split("keep", Direction::Vertical, name);
        close_kept(&mut fx, name);
    }
    assert_eq!(fx.model.undo_history().len(), 5);
    fx.split("keep", Direction::Vertical, "r5");
    let applied = fx.apply(Op::ClosePane {
        pane: fx.pane_id("r5"),
        keep_undo: true,
    });
    assert_eq!(
        effects(&applied, &fx),
        ["stop t1", "stop t2", "stop t3", "hold r5"]
    );
    assert_eq!(fx.model.undo_history().len(), 5);
}

#[test]
fn reopening_a_named_tab_takes_only_its_record() {
    let mut fx = Fixture::new();
    let ws = fx.workspace("alpha", "a");
    let one = fx.tab(ws, "one", "b");
    let two = fx.tab(ws, "two", "c");
    fx.apply(Op::CloseTab {
        tab: one,
        keep_sessions: true,
    });
    fx.apply(Op::CloseTab {
        tab: two,
        keep_sessions: true,
    });
    fx.apply(Op::UndoClose { tab: Some(one) });
    let left: Vec<_> = fx
        .model
        .undo_history()
        .records()
        .map(|record| record.sessions())
        .collect();
    assert_eq!(left, [vec![session("c")]]);
    assert_eq!(
        fx.try_apply(Op::UndoClose { tab: Some(one) }).err(),
        Some(OpError::NotFound)
    );
}

#[test]
fn closing_an_empty_tab_records_nothing() {
    let mut fx = Fixture::new();
    let ws = fx.workspace("alpha", "a");
    let empty = paneflow_layout::TabId(fx.model.reserve());
    fx.apply(Op::OpenTab {
        workspace: ws,
        tab: paneflow_layout::NewTab {
            id: empty,
            title: "empty".to_string(),
            title_source: paneflow_config::schema::TabTitleSource::Preset,
            worktree: None,
            root: None,
        },
        fill_empty_active: false,
    });
    assert_eq!(fx.shape(0, 1), "empty");
    let applied = fx.apply(Op::CloseTab {
        tab: empty,
        keep_sessions: true,
    });
    assert!(applied.effects.is_empty());
    assert!(fx.model.undo_history().is_empty());
}

#[test]
fn undo_opens_a_fresh_shell_for_a_session_already_shown_elsewhere() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    close_kept(&mut fx, "b");
    let pane = fx.pane(&["b"]);
    let target = fx.pane_id("a");
    fx.apply(Op::Split {
        target,
        direction: Direction::Vertical,
        edge: paneflow_layout::Edge::After,
        pane,
        focus: false,
    });
    let applied = fx.apply(Op::UndoClose { tab: None });
    let spawned = effects(&applied, &fx);
    assert_eq!(spawned.len(), 1);
    assert!(spawned[0].starts_with("spawn"), "{spawned:?}");
}
