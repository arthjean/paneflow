#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::Fixture;
use paneflow_layout::{Direction, Edge, IdBlock, IdSource, Model, Op, Snapshot};

fn block(start: u64, end: u64) -> IdBlock {
    IdBlock { start, end }
}

#[test]
fn a_leased_source_walks_its_blocks_in_order_and_reports_an_overdraft() {
    let ids = IdSource::starting_at(1);
    assert_eq!(ids.remaining(), None);
    ids.grant(block(100, 102));
    ids.grant(block(500, 501));
    assert_eq!(ids.remaining(), Some(3));
    let taken: Vec<u64> = (0..3).map(|_| ids.allocate()).collect();
    assert_eq!(taken, [100, 101, 500]);
    assert!(!ids.is_overdrawn());
    assert_eq!(ids.remaining(), Some(0));
    ids.allocate();
    assert!(ids.is_overdrawn());
    ids.grant(block(900, 910));
    assert!(!ids.is_overdrawn());
    assert_eq!(ids.allocate(), 900);
}

#[test]
fn a_mirror_positioned_on_a_cursor_allocates_the_same_ids_as_the_client() {
    let client = IdSource::starting_at(1);
    let blocks = [block(10, 13), block(40, 50)];
    for leased in blocks {
        client.grant(leased);
    }
    client.allocate();
    let cursor = client.next_value();
    let mirror = IdSource::leased(&blocks, cursor);
    let from_client: Vec<u64> = (0..5).map(|_| client.allocate()).collect();
    let from_mirror: Vec<u64> = (0..5).map(|_| mirror.allocate()).collect();
    assert_eq!(from_client, from_mirror);
    assert_eq!(from_client, [11, 12, 40, 41, 42]);
    assert!(IdSource::leased(&blocks, 30).is_overdrawn());
}

#[test]
fn an_op_replayed_by_the_authority_with_the_client_cursor_gives_the_same_state() {
    let mut window = Fixture::new();
    window.workspace("alpha", "a");
    let authority_snapshot = window.model.snapshot();
    let mut authority = Model::new();
    authority.restore(authority_snapshot);

    let blocks = [block(1_000, 1_000 + paneflow_layout::LEASE_BLOCK)];
    let leased = window.model.ids();
    leased.grant(blocks[0]);
    let target = window.pane_id("a");
    let pane = window.pane(&["b"]);
    let ids_from = window.model.next_id();
    let op = Op::Split {
        target,
        direction: Direction::Vertical,
        edge: Edge::After,
        pane,
        focus: true,
    };
    let wire = serde_json::to_value(&op).expect("an op encodes");
    window
        .model
        .apply(op, 1)
        .expect("the window applies the split");

    let received: Op = serde_json::from_value(wire).expect("an op decodes");
    for id in received.new_ids() {
        assert!(blocks[0].contains(id), "{id} lies outside the lease");
        assert!(authority.reserve_exact(id));
    }
    let mirror = IdSource::leased(&blocks, ids_from);
    authority
        .apply_with_ids(received, 2, &mirror)
        .expect("the authority applies the same split");
    assert!(!mirror.is_overdrawn());
    assert_eq!(authority.state(), window.model.state());
    assert_eq!(authority.revision(), window.model.revision());
}

#[test]
fn a_snapshot_keeps_zoom_focus_and_undo_through_its_wire_form() {
    let mut fx = Fixture::new();
    fx.workspace("alpha", "a");
    fx.split("a", Direction::Vertical, "b");
    fx.split("b", Direction::Horizontal, "c");
    let tab = fx.model.state().workspaces[0].tabs[0].id;
    let b = fx.pane_id("b");
    fx.apply(Op::FocusPane { pane: b });
    fx.apply(Op::SetZoom { tab, pane: Some(b) });
    fx.apply(Op::SetZoom { tab, pane: None });
    let c = fx.pane_id("c");
    fx.apply(Op::ClosePane {
        pane: c,
        keep_undo: true,
    });
    fx.apply(Op::SetZoom { tab, pane: Some(b) });
    let snapshot = fx.model.snapshot();
    assert!(!snapshot.undo.is_empty());
    let encoded = serde_json::to_string(&snapshot).expect("a snapshot encodes");
    let decoded: Snapshot = serde_json::from_str(&encoded).expect("a snapshot decodes");
    assert_eq!(decoded, snapshot);
    let mut restored = Model::new();
    restored.restore(decoded);
    assert_eq!(restored.state(), fx.model.state());
    assert_eq!(restored.undo_history(), fx.model.undo_history());
    assert!(restored.next_id() > snapshot.undo.highest_id());
}
