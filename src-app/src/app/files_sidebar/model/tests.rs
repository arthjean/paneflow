use super::*;
use crate::app::files_tree::compare_nodes;

fn dir(path: &Path) -> FileNode {
    FileNode {
        path: path.to_path_buf(),
        is_dir: true,
        is_ignored: false,
        is_hidden: false,
        size: 0,
    }
}

fn file(path: &Path) -> FileNode {
    FileNode {
        path: path.to_path_buf(),
        is_dir: false,
        is_ignored: false,
        is_hidden: false,
        size: 0,
    }
}

fn sorted(mut nodes: Vec<FileNode>) -> Vec<FileNode> {
    nodes.sort_by(compare_nodes);
    nodes
}

struct Disk {
    root: PathBuf,
    listings: HashMap<PathBuf, Vec<FileNode>>,
}

impl Disk {
    fn listing(&self, model: &TreeModel, dir: &Path, diff: bool) -> DirChange {
        let nodes = self.listings[dir].clone();
        let diff = diff
            .then(|| {
                model
                    .listing(dir)
                    .map(|old| ListingDiff::between(old, &nodes))
            })
            .flatten();
        DirChange::Listing {
            dir: dir.to_path_buf(),
            nodes: nodes.into(),
            diff,
            unreadable: false,
        }
    }

    fn naive(&self, model: &TreeModel) -> Vec<(PathBuf, usize)> {
        fn walk(model: &TreeModel, dir: &Path, depth: usize, out: &mut Vec<(PathBuf, usize)>) {
            let Some(listing) = model.listing(dir) else {
                return;
            };
            for node in listing {
                out.push((node.path.clone(), depth));
                if node.is_dir && model.is_expanded(&node.path) {
                    walk(model, &node.path, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(model, &self.root, 0, &mut out);
        out
    }
}

fn assert_matches_naive(disk: &Disk, model: &TreeModel) {
    let expected = disk.naive(model);
    assert_eq!(model.row_count(), expected.len());
    let rows: Vec<(PathBuf, usize)> = model
        .rows(0..expected.len())
        .into_iter()
        .map(|row| (row.node.path.clone(), row.depth))
        .collect();
    assert_eq!(rows, expected);
    for (index, (path, _)) in expected.iter().enumerate() {
        assert_eq!(
            model.index(path),
            Some(index),
            "index of {}",
            path.display()
        );
        let row = model.row(index).expect("row");
        assert_eq!(&row.node.path, path);
    }
    for start in [0, expected.len() / 3, expected.len().saturating_sub(5)] {
        let window: Vec<PathBuf> = model
            .rows(start..start + 7)
            .into_iter()
            .map(|row| row.node.path.clone())
            .collect();
        let naive: Vec<PathBuf> = expected
            .iter()
            .skip(start)
            .take(7)
            .map(|(path, _)| path.clone())
            .collect();
        assert_eq!(window, naive, "window from {start}");
    }
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % bound.max(1)
    }
}

fn nested_disk() -> Disk {
    let root = PathBuf::from("workspace");
    let mut listings = HashMap::new();
    let mut dirs = vec![root.clone()];
    for level in 0..3 {
        let mut next = Vec::new();
        for parent in &dirs {
            let mut nodes = Vec::new();
            for index in 0..4 {
                let child = parent.join(format!("dir{level}{index}"));
                nodes.push(dir(&child));
                next.push(child);
            }
            for index in 0..150 {
                nodes.push(file(&parent.join(format!("file{index:03}.rs"))));
            }
            listings.insert(parent.clone(), sorted(nodes));
        }
        dirs = next;
    }
    for leaf in dirs {
        listings.insert(
            leaf.clone(),
            sorted(
                (0..3)
                    .map(|i| file(&leaf.join(format!("leaf{i}.md"))))
                    .collect(),
            ),
        );
    }
    Disk { root, listings }
}

#[test]
fn folder_counts_follow_expansion_collapse_and_content_changes_like_a_full_flatten() {
    let mut disk = nested_disk();
    let mut model = TreeModel::new(disk.root.clone(), Vec::new());
    let mut dirs: Vec<PathBuf> = disk.listings.keys().cloned().collect();
    dirs.sort();
    let mut lcg = Lcg(0x5eed);
    for dir in dirs.iter().rev() {
        let change = disk.listing(&model, dir, false);
        model.apply(vec![change]);
    }
    assert_matches_naive(&disk, &model);
    for step in 0..400 {
        let target = dirs[lcg.next(dirs.len())].clone();
        match lcg.next(4) {
            0 | 1 => {
                let expand = !model.is_expanded(&target);
                model.set_expanded(&target, expand);
            }
            2 => {
                let listing = disk.listings.get_mut(&target).expect("listing");
                let files: Vec<usize> = (0..listing.len())
                    .filter(|&ix| !listing[ix].is_dir)
                    .collect();
                if !files.is_empty() {
                    listing.remove(files[lcg.next(files.len())]);
                }
                listing.push(file(&target.join(format!("added{step:04}.ts"))));
                listing.sort_by(compare_nodes);
                let change = disk.listing(&model, &target, true);
                model.apply(vec![change]);
            }
            _ => {
                if target != disk.root {
                    model.apply(vec![DirChange::Dropped(target.clone())]);
                    let change = disk.listing(&model, &target, true);
                    model.apply(vec![change]);
                }
            }
        }
        if step % 25 == 0 {
            assert_matches_naive(&disk, &model);
        }
    }
    assert_matches_naive(&disk, &model);
    let mut opened: HashSet<PathBuf> = dirs.iter().cloned().collect();
    model.replace_expanded(opened.clone());
    assert_matches_naive(&disk, &model);
    opened.retain(|path| path.components().count() < 3);
    model.replace_expanded(opened);
    assert_matches_naive(&disk, &model);
}

#[test]
fn a_thousand_watcher_changes_carry_the_counts_of_unchanged_entries_over() {
    let root = PathBuf::from("workspace");
    let open = root.join("aaa-open");
    let mut root_listing: Vec<FileNode> = (0..50_000)
        .map(|index| file(&root.join(format!("file_{index:06}.rs"))))
        .collect();
    root_listing.push(dir(&open));
    let mut disk = Disk {
        root: root.clone(),
        listings: HashMap::from([
            (root.clone(), sorted(root_listing)),
            (
                open.clone(),
                sorted(
                    (0..10)
                        .map(|i| file(&open.join(format!("{i}.rs"))))
                        .collect(),
                ),
            ),
        ]),
    };
    let mut model = TreeModel::new(root.clone(), [open.clone()]);
    for dir in [&root, &open] {
        let change = disk.listing(&model, dir, false);
        model.apply(vec![change]);
    }
    assert_eq!(model.row_count(), 50_011);

    let listing = disk.listings.get_mut(&root).expect("root listing");
    let mut removed = 0;
    listing.retain(|node| {
        let drop = !node.is_dir && removed < 500 && node.path.to_string_lossy().ends_with("7.rs");
        removed += usize::from(drop);
        !drop
    });
    listing.extend((0..500).map(|i| file(&root.join(format!("new_{i:04}.rs")))));
    listing.sort_by(compare_nodes);
    let change = disk.listing(&model, &root, true);
    let diff = match &change {
        DirChange::Listing { diff, .. } => diff.clone(),
        DirChange::Dropped(_) => None,
    }
    .expect("a diff");
    assert_eq!((diff.removed.len(), diff.inserted.len()), (500, 500));
    model.apply(vec![change]);

    assert_eq!(model.row_count(), 50_011);
    assert_eq!(model.visible_rows_of(&root), Some(50_011));
    assert_eq!(model.index(&open.join("9.rs")), Some(10));
    assert_eq!(
        model.row(10).map(|row| row.node.path.clone()),
        Some(open.join("9.rs"))
    );
    let tail = model.rows(50_009..50_100);
    assert_eq!(tail.len(), 2);
    assert_eq!(model.index(&root.join("new_0499.rs")), Some(50_010));
}

#[test]
fn rows_outside_the_window_are_never_built() {
    let root = PathBuf::from("workspace");
    let listing: Arc<[FileNode]> = sorted(
        (0..300_000)
            .map(|index| file(&root.join(format!("file_{index:06}.rs"))))
            .collect(),
    )
    .into();
    let mut model = TreeModel::new(root.clone(), Vec::new());
    let git = crate::app::files_git::GitStatuses::default();
    let (before, _) = crate::bench_harness::allocation_counters();
    model.apply(vec![DirChange::Listing {
        dir: root.clone(),
        nodes: listing,
        diff: None,
        unreadable: false,
    }]);
    let window: Vec<super::super::projection::FileRow> = model
        .rows(150_000..150_040)
        .into_iter()
        .map(|row| super::super::projection::FileRow::from_ref(row, &git))
        .collect();
    let (after, _) = crate::bench_harness::allocation_counters();
    assert_eq!(window.len(), 40);
    assert_eq!(window[0].label.as_ref(), "file_150000.rs");
    assert!(
        after - before < 16 * 1024 * 1024,
        "installing 300,000 entries and building 40 rows allocated {} bytes",
        after - before
    );
}

#[test]
fn an_expanded_folder_without_a_listing_is_one_loading_row_and_an_unreadable_one_fails() {
    let root = PathBuf::from("workspace");
    let slow = root.join("slow");
    let broken = root.join("broken");
    let mut model = TreeModel::new(root.clone(), [slow.clone(), broken.clone()]);
    model.apply(vec![
        DirChange::Listing {
            dir: root.clone(),
            nodes: sorted(vec![dir(&slow), dir(&broken), file(&root.join("a.rs"))]).into(),
            diff: None,
            unreadable: false,
        },
        DirChange::Listing {
            dir: broken.clone(),
            nodes: Arc::from([]),
            diff: None,
            unreadable: true,
        },
    ]);
    let states: Vec<(PathBuf, DirState)> = model
        .rows(0..10)
        .into_iter()
        .map(|row| (row.node.path.clone(), row.state))
        .collect();
    assert_eq!(
        states,
        vec![
            (broken, DirState::Failed),
            (slow.clone(), DirState::Loading),
            (root.join("a.rs"), DirState::Ready),
        ]
    );
    model.apply(vec![DirChange::Listing {
        dir: slow.clone(),
        nodes: vec![file(&slow.join("late.rs"))].into(),
        diff: None,
        unreadable: false,
    }]);
    assert_eq!(model.row_count(), 4);
    assert_eq!(model.index(&slow.join("late.rs")), Some(2));
}

#[test]
fn listing_diff_reports_replaced_attributes_and_folders_that_differ_only_by_case() {
    let root = PathBuf::from("w");
    let old = sorted(vec![dir(&root.join("Docs")), file(&root.join("a.rs"))]);
    let mut changed = file(&root.join("a.rs"));
    changed.size = 9;
    let new = sorted(vec![
        dir(&root.join("Docs")),
        dir(&root.join("docs")),
        changed,
    ]);
    let diff = ListingDiff::between(&old, &new);
    assert_eq!(diff.removed, vec![1]);
    assert_eq!(diff.inserted, vec![1, 2]);
    assert!(ListingDiff::between(&new, &new).is_empty());
}

#[test]
fn selection_falls_back_to_the_nearest_visible_ancestor() {
    let root = PathBuf::from("workspace");
    let src = root.join("src");
    let mut model = TreeModel::new(root.clone(), [src.clone()]);
    model.apply(vec![
        DirChange::Listing {
            dir: root.clone(),
            nodes: sorted(vec![dir(&src), file(&root.join("z.rs"))]).into(),
            diff: None,
            unreadable: false,
        },
        DirChange::Listing {
            dir: src.clone(),
            nodes: vec![file(&src.join("main.rs"))].into(),
            diff: None,
            unreadable: false,
        },
    ]);
    let main = src.join("main.rs");
    assert_eq!(
        model.reconcile_selection(Some(&main), 0),
        Some(main.clone())
    );
    model.set_expanded(&src, false);
    assert_eq!(model.reconcile_selection(Some(&main), 0), Some(src.clone()));
    assert_eq!(
        model.reconcile_selection(Some(&root.join("gone.rs")), 99),
        Some(root.join("z.rs"))
    );
}
