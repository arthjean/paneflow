use super::*;
use crate::git_fixture as fixture;

fn scanner(root: PathBuf, expanded: Vec<PathBuf>) -> Scanner {
    let mut scanner = Scanner::new(root);
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

    assert_eq!(scanner.tree.children[&nested].len(), 1);
    assert!(scanner.tree.children[&nested][0].is_ignored);
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
    scanner.set_expanded(vec![inner.clone()]);
    assert!(scanner.scan(|| false));
    std::fs::write(root.join(".gitignore"), "outer/\n").expect("ignore rules");

    assert!(scanner.changed(changed(root.join(".gitignore"))));
    assert!(scanner.scan(|| false));

    assert_eq!(scanner.tree.children.len(), 1);
    let listed = &scanner.tree.children[&root];
    assert_eq!(listed.len(), 2);
    assert!(
        listed
            .iter()
            .any(|node| node.path == root.join("outer") && node.is_ignored)
    );
    assert!(!scanner.tree.children.contains_key(&root.join("outer")));
    assert!(!scanner.tree.children.contains_key(&inner));
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

#[test]
fn expanding_a_directory_refreshes_the_git_statuses() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let folder = root.join("folder");
    std::fs::create_dir(&folder).expect("folder");
    let mut scanner = scanner(root.clone(), Vec::new());
    assert!(!scanner.git_dirty);

    scanner.set_expanded(vec![folder.clone()]);
    assert!(scanner.git_dirty);
    assert!(scanner.scan(|| false));
    assert!(!scanner.git_dirty);

    scanner.set_expanded(vec![folder]);
    assert!(
        !scanner.git_dirty,
        "an unchanged expansion costs no git run"
    );
}

#[test]
fn an_unwatched_git_dir_falls_back_to_polling() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    fixture::init(&root);
    let scanner_state = scanner(root.clone(), Vec::new());
    let mut scanner = scanner_state;
    let git_dir = scanner.git_dir.clone().expect("the repository git dir");
    scanner.watched = scanner.tree.children.keys().cloned().collect();
    assert!(!scanner.watcher_available());

    scanner.watched.insert(git_dir);
    assert!(scanner.watcher_available());
}

#[test]
fn a_failed_git_status_keeps_the_previous_statuses() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    fixture::init(&root);
    let mut scanner = scanner(root.clone(), Vec::new());
    let tracked = root.join("tracked.txt");
    let previous = Arc::new(GitStatuses::parse(&root, "", b" M tracked.txt\0"));
    scanner.git = previous.clone();
    std::fs::write(root.join(".git").join("HEAD"), "not a head\n").expect("corrupt HEAD");

    scanner.refresh_git();

    assert!(Arc::ptr_eq(&scanner.git, &previous));
    assert_eq!(scanner.git.summary(&tracked).worktree.modified, 1);
}

#[test]
fn an_unreadable_root_is_reported_and_clears_once_it_can_be_read() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().join("not-yet");
    let mut scanner = scanner(root.clone(), Vec::new());
    assert!(scanner.tree.root_unreadable());
    assert!(scanner.tree.root_listing_ready());
    assert!(scanner.tree.children[&root].is_empty());

    std::fs::create_dir(&root).expect("root directory");
    std::fs::write(root.join("main.rs"), "fn main() {}").expect("file");
    scanner.rescan();
    assert!(scanner.scan(|| false));
    assert!(!scanner.tree.root_unreadable());
    assert_eq!(scanner.tree.children[&root].len(), 1);
}

#[test]
fn an_ignored_folder_is_read_only_while_expanded_and_unwatched_once_collapsed() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let target = root.join("target");
    std::fs::create_dir_all(target.join("debug")).expect("target directory");
    std::fs::write(root.join(".gitignore"), "target/\n").expect("ignore rules");
    let mut scanner = scanner(root.clone(), Vec::new());
    scanner.watched = HashSet::from([root.clone()]);
    assert!(
        scanner.tree.children[&root]
            .iter()
            .any(|node| node.path == target && node.is_ignored)
    );
    assert!(!scanner.tree.children.contains_key(&target));

    scanner.set_expanded(vec![target.clone()]);
    assert!(scanner.scan(|| false));
    assert_eq!(scanner.tree.children[&target].len(), 1);
    assert!(scanner.tree.children[&target][0].is_ignored);
    scanner.watched.insert(target.clone());

    scanner.set_expanded(Vec::new());
    assert!(scanner.scan(|| false));
    assert!(!scanner.tree.children.contains_key(&target));
    assert!(!scanner.watched.contains(&target));
}

#[test]
fn expand_all_skips_ignored_folders_and_stops_at_its_limit() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    std::fs::write(root.join(".gitignore"), "node_modules/\n").expect("ignore rules");
    std::fs::create_dir_all(root.join("node_modules").join("pkg")).expect("ignored tree");
    for name in ["a", "b", "c"] {
        std::fs::create_dir_all(root.join(name).join("inner")).expect("tree");
    }
    let mut scanner = scanner(root.clone(), Vec::new());

    assert!(!scanner.expand_all(EXPAND_ALL_LIMIT, || false));
    assert!(scanner.scan(|| false));
    let mut expanded: Vec<PathBuf> = scanner.tree.expanded.iter().cloned().collect();
    expanded.sort();
    let mut wanted = vec![root.clone()];
    for name in ["a", "b", "c"] {
        wanted.push(root.join(name));
        wanted.push(root.join(name).join("inner"));
    }
    wanted.sort();
    assert_eq!(expanded, wanted);
    assert!(
        !scanner
            .tree
            .children
            .contains_key(&root.join("node_modules"))
    );

    let mut capped = scanner_state_without_expansion(&root);
    assert!(capped.expand_all(4, || false));
    assert_eq!(capped.tree.expanded.len(), 5, "the root plus four folders");
}

fn scanner_state_without_expansion(root: &std::path::Path) -> Scanner {
    scanner(root.to_path_buf(), Vec::new())
}

#[test]
fn a_refresh_rereads_every_loaded_folder() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).expect("nested directory");
    let mut scanner = scanner(root.clone(), vec![nested.clone()]);
    std::fs::write(nested.join("late.txt"), "late").expect("late file");
    std::fs::write(root.join("top.txt"), "top").expect("top file");

    scanner.rescan();
    assert!(scanner.scan(|| false));

    assert_eq!(scanner.tree.children[&nested].len(), 1);
    assert_eq!(scanner.tree.children[&root].len(), 2);
}

#[test]
fn events_reported_under_the_real_path_of_an_aliased_root_refresh_the_tree() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().to_path_buf();
    let first = root.join("first");
    std::fs::create_dir(&first).expect("first directory");
    let mut scanner = scanner(root.clone(), vec![first.clone()]);
    let real = std::env::temp_dir().join("paneflow-real-path-of-an-aliased-root");
    scanner.real_root = Some(real.clone());
    let new_file = first.join("new.txt");
    std::fs::write(&new_file, "new").expect("new file");

    assert!(scanner.changed(changed(real.join("first").join("new.txt"))));
    assert!(scanner.dirty.contains(&first), "{:?}", scanner.dirty);
    assert!(scanner.scan(|| false));
    assert_eq!(scanner.tree.children[&first][0].path, new_file);
}

#[cfg(unix)]
#[test]
fn a_root_reached_through_a_symlink_knows_its_real_path() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let real = temp.path().canonicalize().expect("canonical").join("real");
    std::fs::create_dir(&real).expect("real directory");
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).expect("symlink");
    let mut scanner = Scanner::new(alias);
    scanner.resolve_real_root();
    assert_eq!(scanner.real_root, Some(real));

    let mut direct = Scanner::new(temp.path().canonicalize().expect("canonical"));
    direct.resolve_real_root();
    assert_eq!(direct.real_root, None);
}
