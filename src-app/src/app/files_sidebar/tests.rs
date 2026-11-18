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
use crate::app::files_git::GitStatuses;
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
    panel.projection = Arc::new(FilesProjection::build(
        &tree,
        &panel.expanded,
        "",
        &GitStatuses::default(),
    ));
    panel.tree = Arc::new(tree);
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
            if matches!(event, super::FilesEvent::Close(_)) {
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
fn projection_stamps_git_status_onto_files_and_their_parent_directories() {
    let mut snapshot = tree(2);
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
    snapshot.expanded.insert(dir.clone());
    let git = GitStatuses::parse(&root, "", b" M src/main.rs\0");

    let projection = FilesProjection::build(&snapshot, &snapshot.expanded, "", &git);

    let status = |path: &PathBuf| {
        projection
            .rows
            .iter()
            .find(|row| &row.node.path == path)
            .expect("row")
            .status
    };
    assert_eq!(status(&child).worktree.modified, 1);
    assert_eq!(status(&dir).worktree.modified, 1);
    assert!(status(&root.join("file-0000.rs")).is_unchanged());
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
    let projection =
        FilesProjection::build(&snapshot, &snapshot.expanded, "", &GitStatuses::default());
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
    let projection =
        FilesProjection::build(&snapshot, &snapshot.expanded, "", &GitStatuses::default());
    assert_eq!(
        projection.reconcile_selection(Some(&dir.join("nested/main.rs")), 3),
        Some(dir)
    );
    assert_eq!(
        projection.reconcile_selection(Some(&root.join("removed.rs")), 99),
        Some(root.join("file-0002.rs"))
    );
}

#[test]
fn the_tree_width_follows_the_pointer_within_its_bounds_and_spares_the_content() {
    use super::{FILES_TREE_DEFAULT_WIDTH, files_tree_fit};
    assert_eq!(FILES_TREE_DEFAULT_WIDTH, 256.);
    assert_eq!(files_tree_fit(300., 1200.), 300.);
    assert_eq!(files_tree_fit(100., 1200.), 160.);
    assert_eq!(files_tree_fit(900., 1200.), 560.);
    assert_eq!(
        files_tree_fit(400., 600.),
        344.,
        "the content keeps 256 px beside the tree"
    );
    assert_eq!(
        files_tree_fit(400., 300.),
        160.,
        "a dock too narrow for both keeps the tree at its minimum"
    );
    assert_eq!(files_tree_fit(f32::NAN, 1200.), FILES_TREE_DEFAULT_WIDTH);
}

#[test]
fn the_tree_handle_steps_by_sixteen_and_jumps_to_its_bounds() {
    use super::{FilesTreeStep, stepped_files_tree_width};
    let step = |key| FilesTreeStep::from_key(key).expect("bound key");
    assert_eq!(stepped_files_tree_width(256., step("left")), 272.);
    assert_eq!(stepped_files_tree_width(256., step("right")), 240.);
    assert_eq!(stepped_files_tree_width(256., step("home")), 160.);
    assert_eq!(stepped_files_tree_width(256., step("end")), 560.);
    assert_eq!(stepped_files_tree_width(552., step("left")), 560.);
    assert_eq!(stepped_files_tree_width(168., step("right")), 160.);
    assert_eq!(FilesTreeStep::from_key("up"), None);
}

#[gpui::test]
fn an_unreadable_root_offers_a_retry_instead_of_an_empty_tree(cx: &mut TestAppContext) {
    let mut snapshot = FilesTreeState::root_shell(PathBuf::from("workspace"));
    snapshot
        .children
        .insert(PathBuf::from("workspace"), Vec::new());
    snapshot.unreadable.insert(PathBuf::from("workspace"));
    let (_panel, _, cx) = harness(cx, snapshot);
    assert!(
        cx.debug_bounds("files-sidebar-retry").is_some(),
        "the Retry button is shown"
    );
}

#[gpui::test]
fn revealing_a_folder_expands_its_ancestors_and_selects_it(cx: &mut TestAppContext) {
    let mut snapshot = tree(400);
    let root = snapshot.root.clone();
    let src = root.join("src");
    let inner = src.join("inner");
    snapshot
        .children
        .get_mut(&root)
        .unwrap()
        .push(node(src.clone(), true));
    snapshot
        .children
        .insert(src.clone(), vec![node(inner.clone(), true)]);
    snapshot
        .children
        .insert(inner.clone(), vec![node(inner.join("deep.rs"), false)]);
    let (panel, _, cx) = harness(cx, snapshot);
    assert!(!panel.read_with(cx, |panel, _| panel.expanded.contains(&src)));

    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.reveal_dir(&inner, cx)));
    cx.run_until_parked();

    panel.read_with(cx, |panel, _| {
        assert!(panel.expanded.contains(&src));
        assert!(panel.expanded.contains(&inner));
        assert_eq!(panel.selected.as_deref(), Some(inner.as_path()));
        assert!(panel.reveal_target.is_none(), "the reveal has landed");
        assert!(panel.projection.index(&inner).is_some());
    });

    let outside = PathBuf::from("elsewhere").join("dir");
    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.reveal_dir(&outside, cx)));
    cx.run_until_parked();
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.selected.clone()),
        Some(inner),
        "a folder outside the tree leaves the selection alone"
    );
}
