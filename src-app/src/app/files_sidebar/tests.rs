use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AppContext, Context, Entity, IntoElement, Modifiers, ParentElement, Render, StyleRefinement,
    Styled, TestAppContext, VisualTestContext, Window, div, point, px, size,
};

use super::FilesSidebar;
use super::projection::FilesProjection;
use crate::app::files_tree::{FileNode, FilesTreeState};

fn node(path: PathBuf, is_dir: bool) -> FileNode {
    FileNode {
        path,
        is_dir,
        is_hidden: false,
        is_ignored: false,
        size: 0,
    }
}

fn tree(count: usize) -> FilesTreeState {
    let root = PathBuf::from("workspace");
    let mut tree = FilesTreeState::root_shell(root.clone());
    tree.children.insert(
        root.clone(),
        (0..count)
            .map(|ix| node(root.join(format!("file-{ix:04}.rs")), false))
            .collect(),
    );
    tree
}

fn install(panel: &mut FilesSidebar, tree: FilesTreeState) {
    panel.active = true;
    panel.expanded = tree.expanded.clone();
    panel.projection = Arc::new(FilesProjection::build(&tree, &panel.expanded, ""));
    panel.tree = Arc::new(tree);
    panel.title = "workspace".into();
}

struct Sibling(Rc<Cell<usize>>);

impl Render for Sibling {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.set(self.0.get() + 1);
        div().size_full()
    }
}

struct Harness {
    panel: Entity<FilesSidebar>,
    sibling: Entity<Sibling>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .child(
                self.panel.clone().cached(
                    StyleRefinement::default()
                        .w(px(300.))
                        .h_full()
                        .flex_shrink_0(),
                ),
            )
            .child(
                self.sibling
                    .clone()
                    .cached(StyleRefinement::default().w(px(100.)).h_full()),
            )
    }
}

fn harness(
    cx: &mut TestAppContext,
    tree: FilesTreeState,
) -> (
    Entity<FilesSidebar>,
    Rc<Cell<usize>>,
    &mut VisualTestContext,
) {
    let sibling_renders = Rc::new(Cell::new(0));
    let renders = sibling_renders.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| Harness {
        panel: cx.new(|cx| {
            let mut panel = FilesSidebar::new(cx);
            install(&mut panel, tree);
            panel
        }),
        sibling: cx.new(|_| Sibling(renders)),
    });
    cx.simulate_resize(size(px(400.), px(350.)));
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.simulate_mouse_move(point(px(350.), px(300.)), cx);
        window.refresh();
        window.draw(cx).clear(cx);
    });
    let panel = view.read_with(cx, |view, _| view.panel.clone());
    (panel, sibling_renders, cx)
}

#[gpui::test]
fn panel_hover_repaints_immediately_without_rebuilding_projection(cx: &mut TestAppContext) {
    let (panel, sibling, cx) = harness(cx, tree(2_000));
    let first = cx
        .debug_bounds("files-row-workspace\\file-0000.rs")
        .or_else(|| cx.debug_bounds("files-row-workspace/file-0000.rs"))
        .expect("first row");
    let second = cx
        .debug_bounds("files-row-workspace\\file-0001.rs")
        .or_else(|| cx.debug_bounds("files-row-workspace/file-0001.rs"))
        .expect("second row");
    assert_eq!(first.origin.x, px(8.));
    assert_eq!(first.size.width, px(284.));
    assert_eq!(first.size.height, px(28.));
    let projection = panel.read_with(cx, |panel, _| panel.projection.clone());
    for (position, repaint) in [
        (point(px(9.), first.center().y), true),
        (point(px(291.), first.center().y), false),
        (point(px(291.), second.center().y), true),
        (point(px(9.), second.center().y), false),
        (point(px(350.), second.center().y), true),
    ] {
        let before = panel.read_with(cx, |panel, _| panel.render_count);
        let sibling_before = sibling.get();
        cx.update(|window, cx| window.simulate_mouse_move(position, cx));
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.render_count - before, usize::from(repaint));
            assert!(Arc::ptr_eq(&panel.projection, &projection));
            assert_eq!(panel.projection_count, 0);
        });
        assert_eq!(sibling.get(), sibling_before);
    }
}

#[gpui::test]
fn panel_keyboard_reveals_selection_and_moves_between_parent_and_child(cx: &mut TestAppContext) {
    let mut snapshot = tree(100);
    let root = snapshot.root.clone();
    let dir = root.join("src");
    let child = dir.join("main.rs");
    snapshot
        .children
        .get_mut(&root)
        .unwrap()
        .insert(0, node(dir.clone(), true));
    snapshot
        .children
        .insert(dir.clone(), vec![node(child.clone(), false)]);
    let (panel, _, cx) = harness(cx, snapshot);
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.focus.focus(window, cx)));
    for (keys, expected) in [
        ("home", dir.clone()),
        ("right", dir.clone()),
        ("right", child),
        ("left", dir.clone()),
        ("left", dir.clone()),
        ("end", root.join("file-0099.rs")),
        ("home", dir.clone()),
    ] {
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
        assert_eq!(
            panel.read_with(cx, |panel, _| panel.selected.clone()),
            Some(expected)
        );
    }
    assert!(!panel.read_with(cx, |panel, _| panel.expanded.contains(&dir)));
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    let first = cx
        .debug_bounds("files-row-workspace\\src")
        .or_else(|| cx.debug_bounds("files-row-workspace/src"))
        .expect("home scrolls back to first row");
    cx.simulate_click(point(px(290.), first.center().y), Modifiers::default());
    cx.run_until_parked();
    assert!(panel.read_with(cx, |panel, _| panel.expanded.contains(&dir)));
}

#[gpui::test]
fn panel_filter_allows_result_navigation_and_escape_closes_when_empty(cx: &mut TestAppContext) {
    let (panel, _, cx) = harness(cx, tree(20));
    let closed = Rc::new(Cell::new(false));
    let closed_event = closed.clone();
    cx.update(|_, cx| {
        cx.subscribe(&panel, move |_, event, _| {
            if matches!(event, super::FilesEvent::Close) {
                closed_event.set(true);
            }
        })
        .detach()
    });
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.filter_input.update(cx, |input, cx| {
                input.set_value("000", cx);
                input.focus_handle.focus(window, cx);
            });
        })
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("down");
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.selected.clone()),
        Some(PathBuf::from("workspace").join("file-0001.rs"))
    );
    cx.simulate_keystrokes("escape");
    assert!(!closed.get());
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .filter_input
                .read(cx)
                .focus_handle
                .clone()
                .focus(window, cx)
        })
    });
    cx.simulate_keystrokes("escape");
    assert!(closed.get());
}

#[gpui::test]
fn panel_discards_pending_projection_on_close_and_keeps_latest_filter(cx: &mut TestAppContext) {
    let panel = cx.new(|cx| {
        let mut panel = FilesSidebar::new(cx);
        install(&mut panel, tree(200));
        panel
    });
    let original = panel.read_with(cx, |panel, _| panel.projection.clone());
    panel.update(cx, |panel, cx| {
        panel.query = "0099".into();
        panel.schedule_projection(true, cx);
        panel.deactivate();
    });
    cx.run_until_parked();
    assert!(panel.read_with(cx, |panel, _| Arc::ptr_eq(&original, &panel.projection)));
    panel.update(cx, |panel, cx| panel.release_snapshot(cx));
    cx.run_until_parked();
    panel.read_with(cx, |panel, _| {
        assert!(panel.tree.children.is_empty());
        assert!(panel.projection.rows.is_empty());
    });
    panel.update(cx, |panel, cx| {
        install(panel, tree(200));
        panel.query = "0099".into();
        panel.schedule_projection(true, cx);
        panel.query = "0001".into();
        panel.schedule_projection(false, cx);
    });
    cx.run_until_parked();
    panel.read_with(cx, |panel, _| {
        assert_eq!(panel.projection.rows.len(), 1);
        assert_eq!(panel.projection.rows[0].label.as_ref(), "file-0001.rs");
        assert!(!panel.pending_reveal);
    });
}

#[test]
fn projection_keeps_selection_on_insert_and_recovers_visible_ancestor_on_collapse() {
    let mut snapshot = tree(3);
    let root = snapshot.root.clone();
    let selected = root.join("file-0001.rs");
    snapshot
        .children
        .get_mut(&root)
        .unwrap()
        .insert(0, node(root.join("a.rs"), false));
    let projection = FilesProjection::build(&snapshot, &snapshot.expanded, "");
    assert_eq!(
        projection.reconcile_selection(Some(&selected), 1),
        Some(selected)
    );
    let dir = root.join("src");
    snapshot
        .children
        .get_mut(&root)
        .unwrap()
        .insert(0, node(dir.clone(), true));
    let projection = FilesProjection::build(&snapshot, &snapshot.expanded, "");
    assert_eq!(
        projection.reconcile_selection(Some(&dir.join("nested/main.rs")), 3),
        Some(dir)
    );
    assert_eq!(
        projection.reconcile_selection(Some(&root.join("removed.rs")), 99),
        Some(root.join("file-0002.rs"))
    );
}
