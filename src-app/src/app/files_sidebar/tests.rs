use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AppContext, Context, Entity, IntoElement, Modifiers, ParentElement, Render, StyleRefinement,
    Styled, TestAppContext, VisualTestContext, Window, div, point, px, size,
};

use super::FilesSidebar;
use super::model::{DirChange, TreeModel};
use super::projection::{FileRow, SearchRows};
use crate::app::files_git::GitStatuses;
use crate::app::files_tree::{FileNode, compare_nodes};

fn node(path: PathBuf, is_dir: bool) -> FileNode {
    FileNode {
        path,
        is_dir,
        is_hidden: false,
        is_ignored: false,
        size: 0,
    }
}

struct Snapshot {
    root: PathBuf,
    expanded: std::collections::HashSet<PathBuf>,
    children: std::collections::HashMap<PathBuf, Vec<FileNode>>,
    unreadable: std::collections::HashSet<PathBuf>,
}

impl Snapshot {
    fn root_shell(root: PathBuf) -> Self {
        Self {
            expanded: std::collections::HashSet::from([root.clone()]),
            root,
            children: std::collections::HashMap::new(),
            unreadable: std::collections::HashSet::new(),
        }
    }

    fn model(&self) -> TreeModel {
        let mut model = TreeModel::new(self.root.clone(), self.expanded.iter().cloned());
        model.apply(
            self.children
                .iter()
                .map(|(dir, nodes)| {
                    let mut nodes = nodes.clone();
                    nodes.sort_by(compare_nodes);
                    DirChange::Listing {
                        dir: dir.clone(),
                        nodes: nodes.into(),
                        diff: None,
                        unreadable: self.unreadable.contains(dir),
                    }
                })
                .collect(),
        );
        model
    }
}

fn all_rows(panel: &FilesSidebar) -> Vec<FileRow> {
    panel.rows_in(0..panel.row_count())
}

fn tree(count: usize) -> Snapshot {
    let root = PathBuf::from("workspace");
    let mut tree = Snapshot::root_shell(root.clone());
    tree.children.insert(
        root.clone(),
        (0..count)
            .map(|ix| node(root.join(format!("file-{ix:04}.rs")), false))
            .collect(),
    );
    tree
}

fn install(panel: &mut FilesSidebar, tree: Snapshot) {
    panel.active = true;
    panel.model = tree.model();
    panel.search_rows = None;
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
    tree: Snapshot,
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
    assert!(!panel.read_with(cx, |panel, _| panel.model.is_expanded(&dir)));
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
    assert!(panel.read_with(cx, |panel, _| panel.model.is_expanded(&dir)));
}

fn leak(id: String) -> &'static str {
    Box::leak(id.into_boxed_str())
}

fn project_files(root: &str, paths: &[String]) -> crate::terminal::project_files::ProjectFiles {
    crate::terminal::project_files::ProjectFiles {
        root: PathBuf::from(root),
        paths: paths.to_vec(),
        complete: true,
    }
}

fn type_query(cx: &mut VisualTestContext, panel: &Entity<FilesSidebar>, query: &str) {
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.filter_input.update(cx, |input, cx| {
                input.set_value(query.to_string(), cx);
                input.focus_handle.focus(window, cx);
            });
        })
    });
}

fn settle_search(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.executor().advance_clock(super::search::SEARCH_DEBOUNCE);
    cx.run_until_parked();
}

fn rel(parts: &[&str]) -> String {
    parts
        .iter()
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

#[gpui::test]
fn project_search_finds_files_of_never_expanded_folders_with_their_parents(
    cx: &mut TestAppContext,
) {
    let (panel, _, cx) = harness(cx, tree(20));
    let deep = rel(&["src", "deep", "needle.rs"]);
    panel.update(cx, |panel, _| {
        panel.search.install_files(project_files(
            "workspace",
            &[deep.clone(), rel(&["README.md"]), rel(&["file-0001.rs"])],
        ))
    });
    let before = panel.read_with(cx, |panel, _| panel.model.expanded().clone());

    type_query(cx, &panel, "needle");
    cx.run_until_parked();
    assert!(
        panel.read_with(cx, |panel, _| panel.search.hits.is_none()),
        "nothing is searched before the typing pause"
    );
    cx.executor()
        .advance_clock(super::search::SEARCH_DEBOUNCE - std::time::Duration::from_millis(20));
    cx.run_until_parked();
    assert!(panel.read_with(cx, |panel, _| panel.search.hits.is_none()));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(20));
    cx.run_until_parked();

    let root = PathBuf::from("workspace");
    panel.read_with(cx, |panel, _| {
        let rows: Vec<(PathBuf, usize, bool)> = all_rows(panel)
            .iter()
            .map(|row| (row.node.path.clone(), row.depth, row.expanded))
            .collect();
        assert_eq!(
            rows,
            vec![
                (root.join("src"), 0, true),
                (root.join("src").join("deep"), 1, true),
                (root.join("src").join("deep").join("needle.rs"), 2, false),
            ]
        );
        let file = &all_rows(panel)[2];
        assert_eq!(&file.label[file.highlights[0].clone()], "needle");
        assert_eq!(
            *panel.model.expanded(),
            before,
            "searching never touches the fold state"
        );
    });

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    panel.read_with(cx, |panel, _| {
        assert!(panel.query.is_empty());
        assert_eq!(*panel.model.expanded(), before);
        assert_eq!(panel.row_count(), 20, "the tree is back as it was");
    });
}

#[gpui::test]
fn project_search_caps_its_results_and_says_when_more_or_none_match(cx: &mut TestAppContext) {
    let (panel, _, cx) = harness(cx, tree(5));
    let many: Vec<String> = (0..250)
        .map(|ix| rel(&["src", &format!("view_{ix:03}.rs")]))
        .collect();
    panel.update(cx, |panel, _| {
        panel
            .search
            .install_files(project_files("workspace", &many))
    });

    type_query(cx, &panel, "view");
    settle_search(cx);
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    panel.read_with(cx, |panel, _| {
        let hits = panel.search.hits.as_ref().expect("results");
        assert_eq!(hits.len(), super::search::SEARCH_RESULT_LIMIT);
        assert!(panel.search.truncated);
        assert_eq!(panel.row_count(), 201, "one folder and 200 files");
    });
    assert!(cx.debug_bounds("files-search-truncated").is_some());
    assert_eq!(
        super::view::MORE_MATCHES,
        "More matches available. Refine your search."
    );

    type_query(cx, &panel, "zzzz");
    settle_search(cx);
    panel.read_with(cx, |panel, _| {
        assert_eq!(panel.row_count(), 0);
        assert!(!panel.search.truncated);
        assert_eq!(panel.empty_message(), "No matching files.");
    });
}

#[gpui::test]
fn a_search_result_that_arrives_after_a_new_keystroke_is_ignored(cx: &mut TestAppContext) {
    let (panel, _, cx) = harness(cx, tree(5));
    panel.update(cx, |panel, _| {
        panel.search.install_files(project_files(
            "workspace",
            &[rel(&["alpha.rs"]), rel(&["beta.rs"])],
        ))
    });
    type_query(cx, &panel, "alpha");
    cx.run_until_parked();
    panel.update(cx, |panel, cx| {
        let generation = panel.search.generation;
        panel.run_search(generation, cx);
    });
    type_query(cx, &panel, "beta");
    cx.run_until_parked();
    assert!(
        panel.read_with(cx, |panel, _| panel.search.hits.is_none()),
        "the alpha result belongs to an older query"
    );
    settle_search(cx);
    panel.read_with(cx, |panel, _| {
        let hits = panel.search.hits.as_ref().expect("results");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel, rel(&["beta.rs"]));
    });
}

#[gpui::test]
fn the_search_query_is_capped_at_two_hundred_fifty_six_characters(cx: &mut TestAppContext) {
    let (panel, _, cx) = harness(cx, tree(1));
    type_query(cx, &panel, &"a".repeat(300));
    cx.run_until_parked();
    panel.read_with(cx, |panel, cx| {
        assert_eq!(
            panel.query.chars().count(),
            super::panel::SEARCH_QUERY_MAX_CHARS
        );
        assert_eq!(
            panel.filter_input.read(cx).value().chars().count(),
            super::panel::SEARCH_QUERY_MAX_CHARS
        );
    });
}

#[gpui::test]
fn panel_discards_pending_projection_on_close_and_keeps_the_latest_search(cx: &mut TestAppContext) {
    let panel = cx.new(|cx| {
        let mut panel = FilesSidebar::new(cx);
        install(&mut panel, tree(200));
        panel
    });
    let hit = |name: &str| super::search::SearchHit {
        rel: name.to_string(),
        highlights: Vec::new(),
    };
    panel.update(cx, |panel, cx| {
        panel.query = "0099".into();
        panel.search.hits = Some(Arc::new(vec![hit("file-0099.rs")]));
        panel.schedule_projection(true, cx);
        panel.deactivate();
    });
    cx.run_until_parked();
    assert!(panel.read_with(cx, |panel, _| panel.search_rows.is_none()));
    panel.update(cx, |panel, cx| panel.release_snapshot(cx));
    cx.run_until_parked();
    panel.read_with(cx, |panel, _| {
        assert!(!panel.model.root_listing_ready());
        assert_eq!(panel.row_count(), 0);
    });
    panel.update(cx, |panel, cx| {
        install(panel, tree(200));
        panel.query = "0099".into();
        panel.search.hits = Some(Arc::new(vec![hit("file-0099.rs")]));
        panel.schedule_projection(true, cx);
        panel.query = "0001".into();
        panel.search.hits = Some(Arc::new(vec![hit("file-0001.rs")]));
        panel.schedule_projection(false, cx);
    });
    cx.run_until_parked();
    panel.read_with(cx, |panel, _| {
        assert_eq!(panel.row_count(), 1);
        assert_eq!(all_rows(panel)[0].label.as_ref(), "file-0001.rs");
        assert!(!panel.pending_reveal);
    });
}

#[gpui::test]
fn the_toolbar_holds_refresh_then_search_then_expand_all(cx: &mut TestAppContext) {
    let (panel, _, cx) = harness(cx, tree(3));
    let refresh = cx
        .debug_bounds("files-tree-refresh")
        .expect("refresh button");
    let search = cx
        .debug_bounds("files-sidebar-filter")
        .expect("search field");
    let expand = cx
        .debug_bounds("files-tree-expand-all")
        .expect("expand button");
    assert!(refresh.right() <= search.left());
    assert!(search.right() <= expand.left());
    assert_eq!(super::view::REFRESH_LABEL, "Refresh files");
    assert_eq!(super::view::SEARCH_LABEL, "Search files");
    panel.read_with(cx, |panel, _| {
        assert_eq!(panel.expand_toggle_label(), "Expand all");
    });
    panel.update(cx, |panel, cx| panel.toggle_expand_all(cx));
    panel.read_with(cx, |panel, _| {
        assert_eq!(panel.expand_toggle_label(), "Collapse all");
    });
    panel.update(cx, |panel, cx| panel.toggle_expand_all(cx));
    panel.read_with(cx, |panel, _| {
        assert_eq!(panel.expand_toggle_label(), "Expand all");
        assert_eq!(
            panel.model.expanded().len(),
            1,
            "collapse all keeps only the root"
        );
    });
}

#[gpui::test]
fn an_expanded_folder_without_a_listing_shows_a_loading_indicator_and_a_failed_one_its_error(
    cx: &mut TestAppContext,
) {
    let mut snapshot = tree(2);
    let root = snapshot.root.clone();
    let slow = root.join("slow");
    let broken = root.join("broken");
    let children = snapshot.children.get_mut(&root).expect("root listing");
    children.insert(0, node(slow.clone(), true));
    children.insert(0, node(broken.clone(), true));
    snapshot.expanded.insert(slow.clone());
    snapshot.expanded.insert(broken.clone());
    snapshot.children.insert(broken.clone(), Vec::new());
    snapshot.unreadable.insert(broken.clone());
    let (panel, _, cx) = harness(cx, snapshot);
    panel.read_with(cx, |panel, _| {
        let state = |path: &PathBuf| {
            panel
                .row_at(panel.row_index(path).expect("row"))
                .expect("row")
                .state
        };
        assert_eq!(state(&slow), super::projection::DirState::Loading);
        assert_eq!(state(&broken), super::projection::DirState::Failed);
    });
    let id = |prefix: &str, path: &PathBuf| leak(format!("{prefix}{}", path.display()));
    assert!(cx.debug_bounds(id("files-row-loading-", &slow)).is_some());
    assert!(cx.debug_bounds(id("files-row-failed-", &broken)).is_some());
    assert_eq!(super::row::FOLDER_FAILED, "Unable to load folder.");
}

#[test]
fn ignored_and_attribute_hidden_entries_are_dimmed_and_dotfiles_are_not() {
    let mut ignored = node(PathBuf::from("w").join("target"), true);
    ignored.is_ignored = true;
    let mut attribute_hidden = node(PathBuf::from("w").join("Thumbs.db"), false);
    attribute_hidden.is_hidden = true;
    let dotfile = node(PathBuf::from("w").join(".editorconfig"), false);
    assert!(super::row::is_dimmed(&ignored));
    assert!(super::row::is_dimmed(&attribute_hidden));
    assert!(!super::row::is_dimmed(&dotfile));
    assert_eq!(super::DIMMED_OPACITY, 0.55);
}

#[gpui::test]
fn an_ignored_file_still_opens(cx: &mut TestAppContext) {
    let mut snapshot = tree(1);
    let root = snapshot.root.clone();
    let generated = root.join("generated.log");
    let mut ignored = node(generated.clone(), false);
    ignored.is_ignored = true;
    snapshot
        .children
        .get_mut(&root)
        .expect("root")
        .insert(0, ignored);
    let (panel, _, cx) = harness(cx, snapshot);
    let opened = Rc::new(std::cell::RefCell::new(None));
    let sink = opened.clone();
    cx.update(|_, cx| {
        cx.subscribe(&panel, move |_, event, _| {
            if let super::FilesEvent::OpenFile { path, .. } = event {
                *sink.borrow_mut() = Some(path.clone());
            }
        })
        .detach()
    });
    let row = cx
        .debug_bounds(leak(format!("files-row-{}", generated.display())))
        .expect("ignored row");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(opened.borrow().clone(), Some(generated));
}

fn wait_for_reveal(cx: &mut VisualTestContext, panel: &Entity<FilesSidebar>) {
    cx.executor().allow_parking();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while panel.read_with(cx, |panel, _| panel.reveal_task.is_some()) {
        assert!(
            std::time::Instant::now() < deadline,
            "the ignore probe never finished"
        );
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cx.run_until_parked();
}

fn wait_for(
    cx: &mut VisualTestContext,
    panel: &Entity<FilesSidebar>,
    mut ready: impl FnMut(&FilesSidebar) -> bool,
) {
    cx.executor().allow_parking();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if panel.read_with(cx, |panel, _| ready(panel)) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the files worker never delivered the awaited update"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[gpui::test]
fn refresh_spins_until_the_reread_lands_and_expand_all_stops_at_two_thousand_folders(
    cx: &mut TestAppContext,
) {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    std::fs::write(root.join(".gitignore"), "ignored/\n").expect("ignore rules");
    std::fs::create_dir_all(root.join("ignored").join("inside")).expect("ignored folder");
    let wide = root.join("wide");
    for ix in 0..super::worker::EXPAND_ALL_LIMIT + 5 {
        std::fs::create_dir_all(wide.join(format!("d{ix:04}"))).expect("folder");
    }
    let (panel, _, cx) = harness(cx, Snapshot::root_shell(root.clone()));
    let toasts = Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = toasts.clone();
    cx.update(|_, cx| {
        cx.subscribe(&panel, move |_, event, _| {
            if let super::FilesEvent::Toast(message) = event {
                sink.borrow_mut().push(message.clone());
            }
        })
        .detach()
    });
    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.open(root.clone(), Vec::new(), cx)));
    wait_for(cx, &panel, |panel| panel.model.root_listing_ready());

    std::fs::write(root.join("late.txt"), "late").expect("late file");
    panel.update(cx, |panel, cx| panel.refresh(cx));
    assert!(panel.read_with(cx, |panel, _| panel.refreshing));
    wait_for(cx, &panel, |panel| !panel.refreshing);
    wait_for(cx, &panel, |panel| {
        panel.row_index(&root.join("late.txt")).is_some()
    });

    panel.update(cx, |panel, cx| panel.toggle_expand_all(cx));
    wait_for(cx, &panel, |panel| !panel.expanding_all);
    panel.read_with(cx, |panel, _| {
        assert_eq!(
            panel.model.expanded().len() - 1,
            super::worker::EXPAND_ALL_LIMIT
        );
        assert!(!panel.model.is_expanded(&root.join("ignored")));
    });
    assert_eq!(
        toasts.borrow().as_slice(),
        ["Expanded the first 2,000 folders".to_string()]
    );
}

#[cfg(unix)]
fn lock_folder(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).expect("lock");
}

#[cfg(unix)]
fn unlock_folder(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("unlock");
}

#[cfg(windows)]
fn lock_folder(path: &std::path::Path) {
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(["/deny", "*S-1-1-0:(OI)(CI)(RD)"])
        .stdout(std::process::Stdio::null())
        .status()
        .expect("icacls runs");
    assert!(status.success(), "icacls /deny exited with {status}");
}

#[cfg(windows)]
fn unlock_folder(path: &std::path::Path) {
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(["/remove:d", "*S-1-1-0"])
        .stdout(std::process::Stdio::null())
        .status()
        .expect("icacls runs");
    assert!(status.success(), "icacls /remove:d exited with {status}");
}

#[gpui::test]
fn a_folder_that_fails_to_load_retries_on_click(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let locked = root.join("locked");
    std::fs::create_dir(&locked).expect("folder");
    std::fs::write(locked.join("inside.txt"), "x").expect("file");
    lock_folder(&locked);
    if std::fs::read_dir(&locked).is_ok() {
        unlock_folder(&locked);
        return;
    }
    let (panel, _, cx) = harness(cx, Snapshot::root_shell(root.clone()));
    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.open(root.clone(), vec![locked.clone()], cx)
        })
    });
    wait_for(cx, &panel, |panel| {
        panel.row_index(&locked).is_some_and(|ix| {
            panel
                .row_at(ix)
                .is_some_and(|row| row.state == super::projection::DirState::Failed)
        })
    });
    unlock_folder(&locked);
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    let row = cx
        .debug_bounds(leak(format!("files-row-{}", locked.display())))
        .expect("failed row");
    cx.simulate_click(row.center(), Modifiers::default());
    wait_for(cx, &panel, |panel| {
        panel.row_index(&locked.join("inside.txt")).is_some()
    });
    assert!(panel.read_with(cx, |panel, _| panel.model.is_expanded(&locked)));
}

#[gpui::test]
fn revealing_the_active_file_expands_its_parents_clears_the_search_and_keeps_focus(
    cx: &mut TestAppContext,
) {
    let mut snapshot = tree(300);
    let root = snapshot.root.clone();
    let src = root.join("src");
    let file = src.join("deep.rs");
    snapshot
        .children
        .get_mut(&root)
        .expect("root")
        .push(node(src.clone(), true));
    snapshot
        .children
        .insert(src.clone(), vec![node(file.clone(), false)]);
    let (panel, _, cx) = harness(cx, snapshot);
    panel.update(cx, |panel, _| {
        panel
            .search
            .install_files(project_files("workspace", &[rel(&["file-0001.rs"])]))
    });
    type_query(cx, &panel, "0001");
    settle_search(cx);
    cx.update(|window, cx| window.blur(cx));
    assert!(!panel.read_with(cx, |panel, _| panel.query.is_empty()));

    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.reveal_file(&file, cx)));
    wait_for_reveal(cx, &panel);

    cx.update(|window, cx| {
        let panel = panel.read(cx);
        assert!(panel.query.is_empty());
        assert!(panel.filter_input.read(cx).value().is_empty());
        assert!(panel.model.is_expanded(&src));
        assert!(!panel.model.is_expanded(&file), "a file is never expanded");
        assert_eq!(panel.selected.as_deref(), Some(file.as_path()));
        assert!(panel.reveal_target.is_none(), "the reveal has landed");
        assert!(
            !panel.focus.is_focused(window),
            "revealing never takes focus"
        );
    });

    panel.update(cx, |panel, cx| {
        panel.selected = Some(root.join("file-0000.rs"));
        panel.refresh(cx);
        panel.schedule_projection(false, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.selected.clone()),
        Some(root.join("file-0000.rs")),
        "a refresh does not replay the last reveal"
    );

    let outside = PathBuf::from("elsewhere").join("main.rs");
    let expanded = panel.read_with(cx, |panel, _| panel.model.expanded().clone());
    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.reveal_file(&outside, cx)));
    wait_for_reveal(cx, &panel);
    panel.read_with(cx, |panel, _| {
        assert_eq!(*panel.model.expanded(), expanded);
        assert_eq!(panel.selected, Some(root.join("file-0000.rs")));
    });
}

#[gpui::test]
fn the_active_file_is_revealed_once_so_each_frame_keeps_the_search_and_the_collapses(
    cx: &mut TestAppContext,
) {
    let mut snapshot = tree(300);
    let root = snapshot.root.clone();
    let src = root.join("src");
    let file = src.join("deep.rs");
    let other = root.join("file-0002.rs");
    snapshot
        .children
        .get_mut(&root)
        .expect("root")
        .push(node(src.clone(), true));
    snapshot
        .children
        .insert(src.clone(), vec![node(file.clone(), false)]);
    let (panel, _, cx) = harness(cx, snapshot);
    panel.update(cx, |panel, _| {
        panel
            .search
            .install_files(project_files("workspace", &[rel(&["file-0001.rs"])]))
    });
    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.reveal_file(&file, cx)));
    wait_for_reveal(cx, &panel);
    assert!(panel.read_with(cx, |panel, _| panel.model.is_expanded(&src)));

    type_query(cx, &panel, "0001");
    settle_search(cx);
    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.reveal_file(&file, cx)));
    wait_for_reveal(cx, &panel);
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.query.clone()),
        "0001",
        "a repeated reveal keeps the search"
    );

    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| panel.clear_files_filter(window, cx));
    });
    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.toggle_dir(&src, cx)));
    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.reveal_file(&file, cx)));
    wait_for_reveal(cx, &panel);
    assert!(
        !panel.read_with(cx, |panel, _| panel.model.is_expanded(&src)),
        "a repeated reveal keeps the folder the user collapsed"
    );

    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.reveal_file(&other, cx)));
    wait_for_reveal(cx, &panel);
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.selected.clone()),
        Some(other),
        "a newly active file is revealed"
    );
}

#[test]
fn search_rows_keep_folders_that_differ_only_by_case_apart() {
    let snapshot = Snapshot::root_shell(PathBuf::from("workspace"));
    let root = snapshot.root.clone();
    let hits = [
        super::search::SearchHit {
            rel: rel(&["Docs", "a.md"]),
            highlights: Vec::new(),
        },
        super::search::SearchHit {
            rel: rel(&["docs", "b.md"]),
            highlights: Vec::new(),
        },
    ];
    let projection = SearchRows::build(&root, &hits, &GitStatuses::default());
    let paths: Vec<PathBuf> = projection
        .rows
        .iter()
        .map(|row| row.node.path.clone())
        .collect();
    assert_eq!(
        paths,
        vec![
            root.join("Docs"),
            root.join("Docs").join("a.md"),
            root.join("docs"),
            root.join("docs").join("b.md"),
        ]
    );
}

#[gpui::test]
fn an_ignored_active_file_is_never_revealed(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    std::fs::write(root.join(".gitignore"), "target/\n").expect("ignore rules");
    let target = root.join("target");
    let built = target.join("debug").join("app.d");
    std::fs::create_dir_all(built.parent().expect("parent")).expect("target tree");
    std::fs::write(&built, "").expect("file");
    let mut snapshot = Snapshot::root_shell(root.clone());
    let mut ignored = node(target.clone(), true);
    ignored.is_ignored = true;
    snapshot.children.insert(root.clone(), vec![ignored]);
    let (panel, _, cx) = harness(cx, snapshot);
    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.reveal_file(&built, cx)));
    wait_for_reveal(cx, &panel);
    panel.read_with(cx, |panel, _| {
        assert!(!panel.model.is_expanded(&target));
        assert_ne!(panel.selected.as_deref(), Some(built.as_path()));
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

    let model = snapshot.model();
    let rows: Vec<FileRow> = model
        .rows(0..model.row_count())
        .into_iter()
        .map(|row| FileRow::from_ref(row, &git))
        .collect();

    let status = |path: &PathBuf| {
        rows.iter()
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
    let projection = snapshot.model();
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
    let projection = snapshot.model();
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
    let mut snapshot = Snapshot::root_shell(PathBuf::from("workspace"));
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
    assert!(!panel.read_with(cx, |panel, _| panel.model.is_expanded(&src)));

    cx.update(|_, cx| panel.update(cx, |panel, cx| panel.reveal_dir(&inner, cx)));
    cx.run_until_parked();

    panel.read_with(cx, |panel, _| {
        assert!(panel.model.is_expanded(&src));
        assert!(panel.model.is_expanded(&inner));
        assert_eq!(panel.selected.as_deref(), Some(inner.as_path()));
        assert!(panel.reveal_target.is_none(), "the reveal has landed");
        assert!(panel.row_index(&inner).is_some());
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

#[test]
fn the_project_search_walk_skips_what_git_ignores() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    crate::git_fixture::init(&root);
    std::fs::write(root.join(".gitignore"), "target/\n").expect("ignore rules");
    std::fs::create_dir_all(root.join("target").join("debug")).expect("target");
    std::fs::write(root.join("target").join("debug").join("needle.rs"), "").expect("ignored");
    std::fs::create_dir_all(root.join("src").join("deep")).expect("src");
    std::fs::write(root.join("src").join("deep").join("needle.rs"), "").expect("kept");
    let files = super::search::walk_for_test(&root).expect("walked");
    let (hits, truncated) = super::search::rank_hits(&files, "needle");
    assert!(!truncated);
    assert_eq!(
        hits.iter().map(|hit| hit.rel.clone()).collect::<Vec<_>>(),
        vec![rel(&["src", "deep", "needle.rs"])]
    );
}

#[gpui::test]
fn a_burst_of_disk_changes_applies_only_the_changed_entries_and_builds_only_the_shown_rows(
    cx: &mut TestAppContext,
) {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let big = root.join("big");
    let other = root.join("other");
    std::fs::create_dir_all(&big).expect("big folder");
    std::fs::create_dir_all(&other).expect("other folder");
    std::fs::File::create(other.join("kept.rs")).expect("kept file");
    for ix in 0..2_000 {
        std::fs::File::create(big.join(format!("file_{ix:04}.rs"))).expect("file");
    }
    let (panel, _, cx) = harness(cx, Snapshot::root_shell(root.clone()));
    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.open(root.clone(), vec![big.clone(), other.clone()], cx)
        })
    });
    wait_for(cx, &panel, |panel| panel.row_count() == 2 + 2_000 + 1);
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    let built = panel.read_with(cx, |panel, _| panel.rows_built.get());
    assert!(
        (1..=64).contains(&built),
        "one frame of 2,003 rows built {built} rows"
    );
    let (counted, other_listing) = panel.read_with(cx, |panel, _| {
        (
            panel.model.counted_entries,
            panel.model.listing_ptr(&other).expect("other listing"),
        )
    });

    for ix in (0..2_000).step_by(4) {
        std::fs::remove_file(big.join(format!("file_{ix:04}.rs"))).expect("remove");
    }
    for ix in 0..500 {
        std::fs::File::create(big.join(format!("new_{ix:04}.rs"))).expect("create");
    }
    wait_for(cx, &panel, |panel| {
        panel.row_index(&big.join("new_0499.rs")).is_some()
            && panel.row_index(&big.join("file_1996.rs")).is_none()
            && panel.row_count() == 2 + 2_000 + 1
    });
    panel.read_with(cx, |panel, _| {
        assert_eq!(
            panel.model.counted_entries - counted,
            500,
            "only the created entries get a fresh visible count"
        );
        assert_eq!(
            panel.model.listing_ptr(&other),
            Some(other_listing),
            "an untouched folder keeps the listing it was given"
        );
        assert_eq!(panel.row_index(&big.join("file_0001.rs")), Some(1));
        assert_eq!(panel.row_index(&big.join("new_0000.rs")), Some(1 + 1_500));
    });
}
