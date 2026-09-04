use super::*;

fn scanner(root: PathBuf, expanded: Vec<PathBuf>) -> Scanner {
    let mut scanner = Scanner {
        tree: FilesTreeState::root_shell(root),
        watcher: None,
        watched: HashSet::new(),
        dirty: HashSet::new(),
    };
    scanner.set_expanded(expanded);
    assert!(scanner.scan(|| false));
    scanner
}

fn changed(path: PathBuf) -> notify::Result<Event> {
    Ok(Event::new(EventKind::Modify(notify::event::ModifyKind::Any)).add_path(path))
}

#[test]
fn scans_only_newly_expanded_and_dirty_directories() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let first = root.join("first");
    let second = root.join("second");
    std::fs::create_dir(&first).expect("first directory");
    std::fs::create_dir(&second).expect("second directory");
    let mut scanner = scanner(root.clone(), vec![first.clone()]);
    let new_file = first.join("new.txt");
    std::fs::write(&new_file, "new").expect("new file");

    scanner.set_expanded(vec![first.clone(), second.clone()]);
    assert!(scanner.scan(|| false));

    assert!(scanner.tree.children.contains_key(&second));
    assert!(scanner.tree.children[&first].is_empty());
    assert!(scanner.changed(changed(new_file.clone())));
    assert!(scanner.scan(|| false));
    assert_eq!(scanner.tree.children[&first][0].path, new_file);
}

#[test]
fn collapsed_ancestors_keep_loaded_listings_without_loading_new_descendants() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let outer = root.join("outer");
    let inner = outer.join("inner");
    let untouched = inner.join("untouched");
    std::fs::create_dir_all(&untouched).expect("nested directory");
    let mut scanner = scanner(root.clone(), vec![outer.clone(), inner.clone()]);
    assert!(scanner.tree.children.contains_key(&inner));

    scanner.set_expanded(vec![inner.clone(), untouched.clone()]);
    assert!(scanner.scan(|| false));

    assert_eq!(scanner.tree.children.len(), 3);
    assert!(scanner.tree.expanded.contains(&inner));
    assert!(!scanner.tree.children.contains_key(&untouched));
    scanner.set_expanded(vec![outer, inner, untouched.clone()]);
    assert!(scanner.scan(|| false));
    assert!(scanner.tree.children.contains_key(&untouched));
}

#[test]
fn removing_parent_prunes_cached_and_expanded_descendants() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let outer = root.join("outer");
    let inner = outer.join("inner");
    std::fs::create_dir_all(&inner).expect("nested directory");
    let mut scanner = scanner(root.clone(), vec![outer.clone(), inner.clone()]);
    scanner.set_expanded(vec![inner.clone()]);
    assert!(scanner.scan(|| false));
    std::fs::remove_dir(&inner).expect("remove inner directory");
    std::fs::remove_dir(&outer).expect("remove outer directory");

    assert!(scanner.changed(changed(outer.clone())));
    assert!(scanner.scan(|| false));

    assert_eq!(scanner.tree.expanded, HashSet::from([root.clone()]));
    assert_eq!(scanner.tree.children.len(), 1);
    assert!(scanner.tree.children[&root].is_empty());
}

#[test]
fn gitignore_changes_refresh_nested_cached_listings() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).expect("nested directory");
    std::fs::write(nested.join("output.log"), "log").expect("log file");
    let mut scanner = scanner(root.clone(), vec![nested.clone()]);
    assert_eq!(scanner.tree.children[&nested].len(), 1);
    std::fs::write(root.join(".gitignore"), "*.log\n").expect("ignore rules");

    assert!(scanner.changed(changed(root.join(".gitignore"))));
    assert!(scanner.scan(|| false));

    assert!(scanner.tree.children[&nested].is_empty());
}

#[test]
fn watcher_errors_invalidate_every_cached_directory() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).expect("nested directory");
    let mut scanner = scanner(root.clone(), vec![nested.clone()]);
    scanner.watched = HashSet::from([root.clone(), nested.clone()]);
    std::fs::write(nested.join("new.txt"), "new").expect("new file");

    assert!(scanner.changed(Err(notify::Error::generic("event queue overflow"))));
    assert!(scanner.watched.is_empty());
    assert!(!scanner.watcher_available());
    assert_eq!(scanner.dirty, HashSet::from([root, nested.clone()]));
    assert!(scanner.scan(|| false));

    assert_eq!(scanner.tree.children[&nested].len(), 1);
}

#[test]
fn cancellation_stops_before_reading_remaining_expanded_directories() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).expect("nested directory");
    let mut scanner = scanner(root.clone(), Vec::new());
    scanner.set_expanded(vec![nested.clone()]);
    let visits = std::cell::Cell::new(0);

    let completed = scanner.scan(|| {
        let count = visits.get();
        visits.set(count + 1);
        count > 0
    });

    assert!(!completed);
    assert_eq!(scanner.tree.children.len(), 1);
    assert!(!scanner.tree.children.contains_key(&nested));
}

#[test]
fn ignoring_loaded_collapsed_parent_prunes_descendants_and_their_preferences() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let outer = root.join("outer");
    let inner = outer.join("inner");
    std::fs::create_dir_all(&inner).expect("nested directory");
    let mut scanner = scanner(root.clone(), vec![outer, inner.clone()]);
    scanner.set_expanded(vec![inner]);
    assert!(scanner.scan(|| false));
    std::fs::write(root.join(".gitignore"), "outer/\n").expect("ignore rules");

    assert!(scanner.changed(changed(root.join(".gitignore"))));
    assert!(scanner.scan(|| false));

    assert_eq!(scanner.tree.expanded, HashSet::from([root.clone()]));
    assert_eq!(scanner.tree.children.len(), 1);
    assert!(scanner.tree.children[&root].is_empty());
}

#[test]
fn file_events_refresh_loaded_collapsed_directories() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).expect("nested directory");
    let mut scanner = scanner(root.clone(), vec![nested.clone()]);
    scanner.set_expanded(Vec::new());
    assert!(scanner.scan(|| false));
    let new_file = nested.join("new.txt");
    std::fs::write(&new_file, "new").expect("new file");

    assert!(scanner.changed(changed(new_file.clone())));
    assert!(scanner.scan(|| false));

    assert_eq!(scanner.tree.children[&nested][0].path, new_file);
    assert!(!scanner.tree.expanded.contains(&nested));
    scanner.set_expanded(vec![nested.clone()]);
    assert!(scanner.scan(|| false));
    assert_eq!(scanner.tree.children[&nested][0].path, new_file);
}

#[test]
fn replacing_loaded_directory_invalidates_descendant_watches_and_listings() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let outer = root.join("outer");
    let inner = outer.join("inner");
    std::fs::create_dir_all(&inner).expect("nested directory");
    std::fs::write(inner.join("old.txt"), "old").expect("old file");
    let mut scanner = scanner(root.clone(), vec![outer.clone(), inner.clone()]);
    scanner.set_expanded(Vec::new());
    assert!(scanner.scan(|| false));
    scanner.watched = HashSet::from([root.clone(), outer.clone(), inner.clone()]);
    std::fs::rename(&outer, root.join("archive")).expect("move old directory");
    std::fs::create_dir_all(&inner).expect("replacement directory");
    let new_file = inner.join("new.txt");
    std::fs::write(&new_file, "new").expect("new file");

    assert!(scanner.changed(Ok(
        Event::new(EventKind::Remove(notify::event::RemoveKind::Any)).add_path(outer.clone()),
    )));
    assert_eq!(scanner.watched, HashSet::from([root]));
    assert!(scanner.dirty.contains(&outer));
    assert!(scanner.dirty.contains(&inner));
    assert!(scanner.scan(|| false));

    assert_eq!(scanner.tree.children[&inner].len(), 1);
    assert_eq!(scanner.tree.children[&inner][0].path, new_file);
}
