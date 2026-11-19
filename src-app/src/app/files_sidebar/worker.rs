use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use smol::channel::{Receiver, Sender};

use super::model::{DirChange, ListingDiff};
use crate::app::files_git::{self, GitStatuses};
use crate::app::files_tree::{FileNode, FilesTreeState, read_dir_listing};

const DEBOUNCE: Duration = Duration::from_millis(40);
const MAX_LATENCY: Duration = Duration::from_millis(160);
const FALLBACK_INTERVAL: Duration = Duration::from_secs(2);
pub(super) const EXPAND_ALL_LIMIT: usize = 2_000;
const WATCH_ROOT_RECURSIVELY: bool = cfg!(target_os = "macos");

pub(super) struct TreeUpdate {
    pub revision: u64,
    pub changes: Vec<DirChange>,
    pub expanded: Option<HashSet<PathBuf>>,
    pub git: Arc<GitStatuses>,
    pub watcher_available: bool,
    pub refreshed: u64,
    pub expanded_all: Option<ExpandAllOutcome>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ExpandAllOutcome {
    pub ticket: u64,
    pub capped: bool,
}

pub(super) struct FilesWorker {
    requests: Sender<Request>,
}

enum Request {
    Expanded(u64, Vec<PathBuf>),
    ExpandAll { revision: u64, ticket: u64 },
    Refresh(u64),
    Reload(PathBuf),
    Changed(notify::Result<Event>),
}

impl FilesWorker {
    pub(super) fn start(root: PathBuf, expanded: Vec<PathBuf>) -> (Self, Receiver<TreeUpdate>) {
        let (requests, incoming) = smol::channel::unbounded();
        let (updates, receiver) = smol::channel::unbounded();
        let events = requests.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("files-tree".into())
            .spawn(move || {
                smol::block_on(run(Scanner::new(root), expanded, incoming, updates, events));
            })
        {
            log::error!("files worker unavailable: {error}");
        }
        (Self { requests }, receiver)
    }

    pub(super) fn set_expanded(&self, revision: u64, expanded: Vec<PathBuf>) {
        let _ = self
            .requests
            .try_send(Request::Expanded(revision, expanded));
    }

    pub(super) fn expand_all(&self, revision: u64, ticket: u64) {
        let _ = self
            .requests
            .try_send(Request::ExpandAll { revision, ticket });
    }

    pub(super) fn refresh(&self, ticket: u64) {
        let _ = self.requests.try_send(Request::Refresh(ticket));
    }

    pub(super) fn reload(&self, dir: PathBuf) {
        let _ = self.requests.try_send(Request::Reload(dir));
    }
}

impl Drop for FilesWorker {
    fn drop(&mut self) {
        self.requests.close();
    }
}

struct Scanner {
    tree: FilesTreeState,
    watcher: Option<RecommendedWatcher>,
    recursive_root: Option<PathBuf>,
    real_root: Option<PathBuf>,
    watched: HashSet<PathBuf>,
    dirty: HashSet<PathBuf>,
    git: Arc<GitStatuses>,
    git_dir: Option<PathBuf>,
    git_dirty: bool,
    changes: Vec<DirChange>,
}

impl Scanner {
    fn new(root: PathBuf) -> Self {
        Self {
            tree: FilesTreeState::root_shell(root),
            watcher: None,
            recursive_root: None,
            real_root: None,
            watched: HashSet::new(),
            dirty: HashSet::new(),
            git: Arc::default(),
            git_dir: None,
            git_dirty: true,
            changes: Vec::new(),
        }
    }

    fn resolve_real_root(&mut self) {
        self.real_root = std::fs::canonicalize(&self.tree.root)
            .ok()
            .map(crate::runtime_paths::strip_verbatim_prefix)
            .filter(|real| *real != self.tree.root);
    }

    fn start_watching(&mut self, events: Sender<Request>) {
        self.resolve_real_root();
        self.watcher = notify::recommended_watcher(move |event| {
            let _ = events.try_send(Request::Changed(event));
        })
        .inspect_err(|error| log::warn!("files watcher unavailable: {error}"))
        .ok();
        self.recursive_root = None;
        if WATCH_ROOT_RECURSIVELY
            && let Some(watcher) = self.watcher.as_mut()
            && watcher
                .watch(&self.tree.root, RecursiveMode::Recursive)
                .is_ok()
        {
            self.recursive_root = Some(self.tree.root.clone());
        }
    }

    fn stop_watching(&mut self) {
        self.watcher = None;
        self.recursive_root = None;
        self.watched.clear();
    }

    fn tree_path(&self, path: PathBuf) -> PathBuf {
        match self
            .real_root
            .as_ref()
            .and_then(|real| path.strip_prefix(real).ok())
        {
            Some(rest) => self.tree.root.join(rest),
            None => path,
        }
    }

    fn set_expanded(&mut self, expanded: Vec<PathBuf>) {
        if expanded
            .iter()
            .any(|path| !self.tree.expanded.contains(path))
        {
            self.git_dirty = true;
        }
        self.tree.expanded = expanded
            .into_iter()
            .filter(|path| path.starts_with(&self.tree.root))
            .collect();
        self.tree.expanded.insert(self.tree.root.clone());
    }

    fn load(&mut self, dir: &Path) {
        if !self.watched.contains(dir)
            && watch_dir(&mut self.watcher, self.recursive_root.as_deref(), dir)
        {
            self.watched.insert(dir.to_path_buf());
            self.dirty.insert(dir.to_path_buf());
        }
        if self.dirty.remove(dir) || !self.tree.children.contains_key(dir) {
            let listing = read_dir_listing(&self.tree.root, dir);
            let unreadable = listing.is_none();
            let was_unreadable = if unreadable {
                !self.tree.unreadable.insert(dir.to_path_buf())
            } else {
                self.tree.unreadable.remove(dir)
            };
            let nodes = listing.unwrap_or_default();
            let diff = match self.tree.children.get(dir) {
                Some(previous) if **previous == *nodes && was_unreadable == unreadable => return,
                Some(previous) => Some(ListingDiff::between(previous, &nodes)),
                None => None,
            };
            let nodes: Arc<[FileNode]> = nodes.into();
            self.tree.children.insert(dir.to_path_buf(), nodes.clone());
            self.changes.push(DirChange::Listing {
                dir: dir.to_path_buf(),
                nodes,
                diff,
                unreadable,
            });
        }
    }

    fn expand_all(&mut self, limit: usize, cancelled: impl Fn() -> bool) -> bool {
        let root = self.tree.root.clone();
        let mut expanded = std::mem::take(&mut self.tree.expanded);
        expanded.insert(root.clone());
        let mut queue = VecDeque::from([root]);
        let mut capped = false;
        'walk: while let Some(dir) = queue.pop_front() {
            if cancelled() {
                break;
            }
            self.load(&dir);
            let Some(children) = self.tree.children.get(&dir) else {
                continue;
            };
            let folders: Vec<PathBuf> = children
                .iter()
                .filter(|node| node.is_dir && !node.is_ignored)
                .map(|node| node.path.clone())
                .collect();
            for folder in folders {
                if !expanded.contains(&folder) {
                    if expanded.len() > limit {
                        capped = true;
                        break 'walk;
                    }
                    expanded.insert(folder.clone());
                }
                queue.push_back(folder);
            }
        }
        self.tree.expanded = expanded;
        self.git_dirty = true;
        capped
    }

    fn scan(&mut self, cancelled: impl Fn() -> bool) -> bool {
        let root = self.tree.root.clone();
        let mut pending = vec![(root.clone(), true)];
        let mut active = HashSet::new();
        if let Some(git_dir) = &self.git_dir {
            active.insert(git_dir.clone());
        }
        while let Some((dir, expanded_ancestors)) = pending.pop() {
            if cancelled() {
                return false;
            }
            active.insert(dir.clone());
            self.load(&dir);
            if let Some(children) = self.tree.children.get(&dir) {
                for node in children.iter() {
                    let expanded = expanded_ancestors && self.tree.expanded.contains(&node.path);
                    let keeps_listing =
                        !node.is_ignored && self.tree.children.contains_key(&node.path);
                    if node.is_dir && (expanded || keeps_listing) {
                        pending.push((node.path.clone(), expanded));
                    }
                }
            }
        }
        self.tree.expanded.retain(|path| {
            path.ancestors()
                .take_while(|path| *path != root.as_path())
                .all(|path| {
                    path.parent()
                        .and_then(|parent| self.tree.children.get(parent))
                        .is_none_or(|nodes| {
                            nodes.iter().any(|node| node.is_dir && node.path == path)
                        })
                })
        });
        let changes = &mut self.changes;
        self.tree.children.retain(|dir, _| {
            let keep = active.contains(dir);
            if !keep {
                changes.push(DirChange::Dropped(dir.clone()));
            }
            keep
        });
        self.tree.unreadable.retain(|dir| active.contains(dir));
        let recursive_root = self.recursive_root.as_deref();
        let watcher = &mut self.watcher;
        self.watched.retain(|dir| {
            if active.contains(dir) {
                return true;
            }
            unwatch_dir(watcher, recursive_root, dir);
            false
        });
        self.dirty.clear();
        if self.git_dirty {
            self.refresh_git();
        }
        !cancelled()
    }

    fn refresh_git(&mut self) {
        self.git_dirty = false;
        let git_dir = crate::workspace::find_git_dir(&self.tree.root.to_string_lossy());
        if git_dir != self.git_dir {
            if let Some(previous) = self.git_dir.take() {
                unwatch_dir(&mut self.watcher, self.recursive_root.as_deref(), &previous);
                self.watched.remove(&previous);
            }
            self.git_dir = git_dir;
        }
        if let Some(git_dir) = self.git_dir.clone()
            && !self.watched.contains(&git_dir)
            && watch_dir(&mut self.watcher, self.recursive_root.as_deref(), &git_dir)
        {
            self.watched.insert(git_dir);
        }
        match files_git::read(&self.tree.root) {
            Some(statuses) => self.git = Arc::new(statuses),
            None if self.git_dir.is_none() => self.git = Arc::default(),
            None => log::debug!("files: git status failed; keeping the previous statuses"),
        }
    }

    fn watcher_available(&self) -> bool {
        self.git_dir
            .as_ref()
            .is_none_or(|git_dir| self.watched.contains(git_dir))
            && self
                .tree
                .children
                .keys()
                .all(|dir| self.watched.contains(dir))
    }

    fn rescan(&mut self) {
        self.dirty.extend(self.tree.children.keys().cloned());
        self.git_dirty = true;
    }

    fn invalidate_subtree(&mut self, path: &std::path::Path) {
        self.dirty.extend(
            self.tree
                .children
                .keys()
                .filter(|dir| dir.starts_with(path))
                .cloned(),
        );
        let recursive_root = self.recursive_root.as_deref();
        let watcher = &mut self.watcher;
        self.watched.retain(|dir| {
            if !dir.starts_with(path) {
                return true;
            }
            unwatch_dir(watcher, recursive_root, dir);
            false
        });
    }

    fn changed(&mut self, event: notify::Result<Event>) -> bool {
        let event = match event {
            Ok(event) if !event.need_rescan() && !event.paths.is_empty() => event,
            Err(error) => {
                log::warn!("files watcher failed: {error}");
                self.stop_watching();
                self.rescan();
                return true;
            }
            _ => {
                self.rescan();
                return true;
            }
        };
        if matches!(event.kind, EventKind::Access(_)) {
            return false;
        }
        let mut watched_change = false;
        for path in event.paths {
            let path = self.tree_path(path);
            if let Some(git_dir) = self.git_dir.as_ref()
                && path.starts_with(git_dir)
            {
                watched_change |= path == *git_dir || path.parent() == Some(git_dir.as_path());
                continue;
            }
            let in_loaded_folder = self.tree.children.contains_key(&path)
                || path
                    .parent()
                    .is_some_and(|parent| self.tree.children.contains_key(parent));
            if !in_loaded_folder {
                continue;
            }
            watched_change = true;
            if matches!(
                event.kind,
                EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
            ) {
                self.invalidate_subtree(&path);
            }
            if self.tree.children.contains_key(&path) {
                self.dirty.insert(path.clone());
            }
            if let Some(parent) = path.parent() {
                if self.tree.children.contains_key(parent) {
                    self.dirty.insert(parent.to_path_buf());
                }
                if path.file_name().is_some_and(|name| name == ".gitignore") {
                    self.dirty.extend(
                        self.tree
                            .children
                            .keys()
                            .filter(|dir| dir.starts_with(parent))
                            .cloned(),
                    );
                }
            }
        }
        self.git_dirty |= watched_change;
        !self.dirty.is_empty() || self.git_dirty
    }
}

fn watch_dir(
    watcher: &mut Option<RecommendedWatcher>,
    recursive_root: Option<&Path>,
    dir: &Path,
) -> bool {
    if recursive_root.is_some_and(|root| dir.starts_with(root)) {
        return true;
    }
    watcher
        .as_mut()
        .is_some_and(|watcher| watcher.watch(dir, RecursiveMode::NonRecursive).is_ok())
}

fn unwatch_dir(
    watcher: &mut Option<RecommendedWatcher>,
    recursive_root: Option<&Path>,
    dir: &Path,
) {
    if recursive_root.is_some_and(|root| dir.starts_with(root)) {
        return;
    }
    if let Some(watcher) = watcher.as_mut() {
        let _ = watcher.unwatch(dir);
    }
}

async fn run(
    mut scanner: Scanner,
    expanded: Vec<PathBuf>,
    requests: Receiver<Request>,
    updates: Sender<TreeUpdate>,
    events: Sender<Request>,
) {
    scanner.set_expanded(expanded);
    let mut revision = 0;
    let mut sent_expanded: Option<(u64, HashSet<PathBuf>)> = None;
    let mut refreshed = 0;
    let mut expanded_all = None;
    let mut ready = true;
    let mut first_event: Option<Instant> = None;
    let mut deadline = Instant::now() + FALLBACK_INTERVAL;
    while !requests.is_closed() && !updates.is_closed() {
        if ready {
            if scanner.watcher.is_none() {
                scanner.start_watching(events.clone());
            }
            if !scanner.scan(|| requests.is_closed() || updates.is_closed()) {
                break;
            }
            let expanded = sent_expanded
                .as_ref()
                .is_none_or(|(sent_revision, sent)| {
                    *sent_revision != revision || *sent != scanner.tree.expanded
                })
                .then(|| scanner.tree.expanded.clone());
            if let Some(expanded) = &expanded {
                sent_expanded = Some((revision, expanded.clone()));
            }
            if updates
                .try_send(TreeUpdate {
                    revision,
                    changes: std::mem::take(&mut scanner.changes),
                    expanded,
                    git: scanner.git.clone(),
                    watcher_available: scanner.watcher_available(),
                    refreshed,
                    expanded_all: expanded_all.take(),
                })
                .is_err()
            {
                break;
            }
            ready = false;
            first_event = None;
            deadline = Instant::now() + FALLBACK_INTERVAL;
        }
        let mut next = smol::future::or(async { requests.recv().await.ok() }, async {
            smol::Timer::at(deadline).await;
            None
        })
        .await;
        if next.is_some() {
            for index in 0..1024 {
                let request = if index == 0 {
                    next.take()
                } else {
                    requests.try_recv().ok()
                };
                match request {
                    Some(Request::Expanded(next_revision, expanded))
                        if next_revision >= revision =>
                    {
                        revision = next_revision;
                        scanner.set_expanded(expanded);
                        ready = true;
                    }
                    Some(Request::ExpandAll {
                        revision: next_revision,
                        ticket,
                    }) if next_revision >= revision => {
                        revision = next_revision;
                        let capped = scanner.expand_all(EXPAND_ALL_LIMIT, || {
                            requests.is_closed() || updates.is_closed()
                        });
                        expanded_all = Some(ExpandAllOutcome { ticket, capped });
                        ready = true;
                    }
                    Some(Request::Refresh(ticket)) => {
                        scanner.rescan();
                        refreshed = refreshed.max(ticket);
                        ready = true;
                    }
                    Some(Request::Reload(dir)) => {
                        scanner.dirty.insert(dir);
                        ready = true;
                    }
                    Some(Request::Changed(event)) => {
                        if scanner.changed(event) {
                            let now = Instant::now();
                            let first = *first_event.get_or_insert(now);
                            deadline = (now + DEBOUNCE).min(first + MAX_LATENCY);
                        }
                    }
                    None => break,
                    _ => {}
                }
            }
        }
        if Instant::now() >= deadline {
            if first_event.is_some() || !scanner.watcher_available() {
                if first_event.is_none() {
                    scanner.rescan();
                }
                ready = true;
            } else {
                deadline = Instant::now() + FALLBACK_INTERVAL;
            }
        }
    }
}

#[cfg(test)]
mod tests;
