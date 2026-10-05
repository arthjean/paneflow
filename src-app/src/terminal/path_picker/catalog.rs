use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use super::index::{self, Index};

pub(super) const REFRESH_AFTER: Duration = Duration::from_secs(10 * 60);
pub(super) const IDLE_RELEASE: Duration = Duration::from_secs(10 * 60);
const MAX_VISITED: usize = 4;
const MAGIC: &[u8; 8] = b"PFPATHIX";
const VERSION: u32 = 1;

pub(super) type Shared = Arc<Mutex<Catalog>>;

static MACHINE: LazyLock<Shared> = LazyLock::new(|| {
    let catalog = if cfg!(test) {
        Catalog::new(Discovery::Fixed(Vec::new()), None)
    } else {
        Catalog::new(Discovery::Machine, paneflow_home::path_index_path())
    };
    Arc::new(Mutex::new(catalog))
});
static WRITING: Mutex<()> = Mutex::new(());

pub(super) fn machine() -> Shared {
    MACHINE.clone()
}

pub(super) fn lock(catalog: &Shared) -> MutexGuard<'_, Catalog> {
    catalog.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
pub(super) enum Discovery {
    Machine,
    Fixed(Vec<PathBuf>),
}

#[derive(Default)]
struct Slot {
    index: Option<Arc<Index>>,
    walk: Option<Arc<AtomicBool>>,
}

impl Slot {
    fn walking(&self) -> bool {
        self.walk
            .as_ref()
            .is_some_and(|cancelled| !cancelled.load(Ordering::Relaxed))
    }

    fn settle(&mut self, flag: &Arc<AtomicBool>, index: Option<Index>) {
        if let Some(index) = index {
            self.index = Some(Arc::new(index));
        }
        if self
            .walk
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, flag))
        {
            self.walk = None;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Disk {
    Unread,
    Reading,
    Read,
}

pub(super) struct Catalog {
    discovery: Discovery,
    store: Option<PathBuf>,
    roots: Vec<PathBuf>,
    globals: HashMap<PathBuf, Slot>,
    visited: Vec<(PathBuf, Slot)>,
    disk: Disk,
    open: usize,
    last_used: Option<Instant>,
    janitor: bool,
}

pub(super) struct Snapshot {
    pub(super) local: Option<Arc<Index>>,
    pub(super) globals: Vec<Arc<Index>>,
    pub(super) visited: Vec<Arc<Index>>,
    pub(super) pending: bool,
}

impl Catalog {
    pub(super) fn new(discovery: Discovery, store: Option<PathBuf>) -> Self {
        Self {
            discovery,
            store,
            roots: Vec::new(),
            globals: HashMap::new(),
            visited: Vec::new(),
            disk: Disk::Unread,
            open: 0,
            last_used: None,
            janitor: false,
        }
    }

    pub(super) fn discovery(&self) -> Discovery {
        self.discovery.clone()
    }

    pub(super) fn store(&self) -> Option<PathBuf> {
        self.store.clone()
    }

    pub(super) fn opened(&mut self) {
        self.open += 1;
        self.last_used = Some(Instant::now());
    }

    pub(super) fn closed(&mut self) {
        self.open = self.open.saturating_sub(1);
        self.last_used = Some(Instant::now());
    }

    pub(super) fn claim_janitor(&mut self) -> bool {
        !std::mem::replace(&mut self.janitor, true)
    }

    pub(super) fn set_roots(&mut self, roots: Vec<PathBuf>) {
        self.globals
            .retain(|root, slot| slot.walking() || roots.contains(root));
        self.roots = roots;
    }

    pub(super) fn snapshot(&mut self, base: Option<&Path>) -> Snapshot {
        self.last_used = Some(Instant::now());
        let mut local = None;
        let mut local_walking = false;
        let mut visited = Vec::new();
        for (root, slot) in &self.visited {
            if base.is_some_and(|base| index::same_path(root, base)) {
                local = slot.index.clone();
                local_walking = slot.walking();
            } else if let Some(index) = &slot.index {
                visited.push(index.clone());
            }
        }
        let globals: Vec<Arc<Index>> = self
            .roots
            .iter()
            .filter_map(|root| self.globals.get(root)?.index.clone())
            .collect();
        let pending = (local.is_none() && local_walking) || globals.len() < self.roots.len();
        Snapshot {
            local,
            globals,
            visited,
            pending,
        }
    }

    pub(super) fn begin_visit(&mut self, root: &Path, flag: &Arc<AtomicBool>) -> bool {
        let position = self
            .visited
            .iter()
            .position(|(known, _)| index::same_path(known, root));
        let mut entry = match position {
            Some(position) => self.visited.remove(position),
            None => (root.to_path_buf(), Slot::default()),
        };
        let started = !entry.1.walking();
        if started {
            entry.1.walk = Some(flag.clone());
        }
        self.visited.insert(0, entry);
        self.visited.truncate(MAX_VISITED);
        started
    }

    pub(super) fn finish_visit(
        &mut self,
        root: &Path,
        flag: &Arc<AtomicBool>,
        index: Option<Index>,
    ) {
        if let Some((_, slot)) = self
            .visited
            .iter_mut()
            .find(|(known, _)| index::same_path(known, root))
        {
            slot.settle(flag, index);
        }
    }

    pub(super) fn begin_disk_read(&mut self) -> bool {
        let starts = self.disk == Disk::Unread && self.store.is_some();
        if starts {
            self.disk = Disk::Reading;
        } else if self.store.is_none() {
            self.disk = Disk::Read;
        }
        starts
    }

    pub(super) fn finish_disk_read(&mut self, indexes: Vec<Index>) {
        for index in indexes {
            let root = index.root().to_path_buf();
            if !self.roots.contains(&root) {
                continue;
            }
            let slot = self.globals.entry(root).or_default();
            if slot.index.is_none() {
                slot.index = Some(Arc::new(index));
            }
        }
        self.disk = Disk::Read;
    }

    pub(super) fn next_stale(
        &mut self,
        now: SystemTime,
        flag: &Arc<AtomicBool>,
    ) -> Option<PathBuf> {
        if self.disk != Disk::Read {
            return None;
        }
        if self.globals.values().any(Slot::walking) {
            return None;
        }
        let index_of = |root: &PathBuf| self.globals.get(root).and_then(|slot| slot.index.as_ref());
        let root = self
            .roots
            .iter()
            .find(|root| index_of(root).is_none())
            .or_else(|| {
                self.roots.iter().find(|root| {
                    index_of(root).is_some_and(|index| {
                        !now.duration_since(index.built())
                            .is_ok_and(|age| age < REFRESH_AFTER)
                    })
                })
            })?
            .clone();
        self.globals.entry(root.clone()).or_default().walk = Some(flag.clone());
        Some(root)
    }

    pub(super) fn finish_global(
        &mut self,
        root: &Path,
        flag: &Arc<AtomicBool>,
        index: Option<Index>,
    ) -> Vec<Arc<Index>> {
        let wanted = self.roots.iter().any(|known| known == root);
        if let Some(slot) = self.globals.get_mut(root) {
            slot.settle(flag, index.filter(|_| wanted));
        }
        self.roots
            .iter()
            .filter_map(|root| self.globals.get(root)?.index.clone())
            .collect()
    }

    pub(super) fn release_if_idle(&mut self, idle: Duration, now: Instant) -> bool {
        let quiet = self.open == 0
            && self
                .last_used
                .is_some_and(|used| now.saturating_duration_since(used) >= idle)
            && !self.globals.values().any(Slot::walking)
            && !self.visited.iter().any(|(_, slot)| slot.walking());
        if quiet {
            self.globals.clear();
            self.visited.clear();
            self.disk = Disk::Unread;
            self.janitor = false;
        }
        quiet
    }
}

pub(super) fn global_roots(
    homes: &[PathBuf],
    projects: &[PathBuf],
    extra: &[PathBuf],
    temp: &Path,
) -> Vec<PathBuf> {
    let sealed: Vec<PathBuf> = homes
        .iter()
        .flat_map(|home| index::platform_data_dirs(home))
        .collect();
    let mut candidates: Vec<PathBuf> = homes.iter().chain(extra).cloned().collect();
    for project in projects {
        let inside_home = homes
            .iter()
            .any(|home| index::strip_root(project, home).is_some());
        let parent = project.parent().filter(|parent| {
            !inside_home
                && parent.parent().is_some()
                && index::strip_root(parent, temp).is_none()
                && !homes
                    .iter()
                    .any(|home| index::strip_root(home, parent).is_some())
        });
        candidates.push(parent.unwrap_or(project).to_path_buf());
    }
    candidates.retain(|candidate| candidate.parent().is_some() && candidate.is_dir());
    let mut shallow_first = candidates.clone();
    shallow_first.sort_by_key(|candidate| candidate.components().count());
    let mut kept: Vec<PathBuf> = Vec::new();
    for candidate in shallow_first {
        if !kept
            .iter()
            .any(|root| index::reaches(root, &candidate, &sealed))
        {
            kept.push(candidate);
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        if kept.contains(&candidate) && !roots.contains(&candidate) {
            roots.push(candidate);
        }
    }
    roots
}

pub(super) fn read_store(path: &Path) -> Vec<Index> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    decode_store(&bytes).unwrap_or_else(|| {
        log::warn!(
            "path index: {} is not a readable index, rebuilding it",
            path.display()
        );
        Vec::new()
    })
}

pub(super) fn write_store(path: &Path, indexes: &[Arc<Index>]) {
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    let encodable: Vec<&Arc<Index>> = indexes
        .iter()
        .filter(|index| index.root().to_str().is_some())
        .collect();
    bytes.extend_from_slice(&(encodable.len() as u32).to_le_bytes());
    for index in encodable {
        if index.encode(&mut bytes).is_none() {
            log::warn!("path index: could not encode {}", index.root().display());
            return;
        }
    }
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        log::warn!("path index: could not create {}: {error}", parent.display());
        return;
    }
    let staged = path.with_extension("bin.tmp");
    let written = std::fs::write(&staged, &bytes).and_then(|()| std::fs::rename(&staged, path));
    if let Err(error) = written {
        log::warn!("path index: could not write {}: {error}", path.display());
    }
}

fn decode_store(bytes: &[u8]) -> Option<Vec<Index>> {
    let rest = bytes.strip_prefix(MAGIC.as_slice())?;
    let (version, rest) = rest.split_first_chunk::<4>()?;
    if u32::from_le_bytes(*version) != VERSION {
        return None;
    }
    let (count, mut rest) = rest.split_first_chunk::<4>()?;
    let mut indexes = Vec::new();
    for _ in 0..u32::from_le_bytes(*count) {
        indexes.push(Index::decode(&mut rest)?);
    }
    rest.is_empty().then_some(indexes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::path_picker::index::{Builder, LOCAL_CAP};

    fn built(root: &Path, names: &[&str], at: SystemTime) -> Index {
        let mut builder = Builder::new(root.to_path_buf(), LOCAL_CAP);
        for name in names {
            builder.push(1, Some(name), false, false);
        }
        builder.finish(at)
    }

    fn flag() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    #[test]
    fn global_roots_keep_homes_and_the_folders_holding_outside_projects() {
        let machine = tempfile::tempdir().expect("tempdir");
        let home = machine.path().join("home");
        let dev = machine.path().join("dev");
        let temp = machine.path().join("temp");
        for dir in [
            home.join("code").join("app"),
            home.join(".paneflow").join("worktrees"),
            home.join("AppData").join("Local").join("x"),
            dev.join("site"),
            dev.join("tool"),
            temp.join("scratch"),
        ] {
            std::fs::create_dir_all(dir).expect("dir");
        }
        let worktrees = home.join(".paneflow").join("worktrees");
        let roots = global_roots(
            std::slice::from_ref(&home),
            &[
                home.join("code").join("app"),
                dev.join("site"),
                dev.join("tool"),
                temp.join("scratch"),
                machine.path().join("gone").join("x"),
            ],
            std::slice::from_ref(&worktrees),
            &temp,
        );
        let expected = vec![home.clone(), worktrees, dev.clone(), temp.join("scratch")];
        if cfg!(windows) {
            let sealed = home.join("AppData").join("Local").join("x");
            let roots = global_roots(
                std::slice::from_ref(&home),
                std::slice::from_ref(&sealed),
                &[],
                &temp,
            );
            assert_eq!(roots, [home.clone(), sealed]);
        }
        assert_eq!(roots, expected);
    }

    #[test]
    fn a_project_beside_a_home_does_not_index_every_home() {
        let machine = tempfile::tempdir().expect("tempdir");
        let home = machine.path().join("users").join("me");
        let other = machine.path().join("users").join("them");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::create_dir_all(&other).expect("other");
        let roots = global_roots(
            std::slice::from_ref(&home),
            std::slice::from_ref(&other),
            &[],
            machine.path(),
        );
        assert_eq!(roots, [home, other]);
    }

    #[test]
    fn the_store_round_trips_every_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("cache").join("path-index.bin");
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let indexes = [
            Arc::new(built(&dir.path().join("a"), &["one.txt", "two.txt"], at)),
            Arc::new(built(&dir.path().join("b"), &["three.txt"], at)),
        ];
        write_store(&file, &indexes);
        let read = read_store(&file);
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].root(), dir.path().join("a"));
        assert_eq!(read[0].name(1), "two.txt");
        assert_eq!(read[1].built(), at);
        assert!(!file.with_extension("bin.tmp").exists());
    }

    #[test]
    fn an_unreadable_store_reads_as_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("path-index.bin");
        assert!(read_store(&file).is_empty());
        std::fs::write(&file, b"PFPATHIX\x01\x00\x00\x00\x05\x00\x00\x00").expect("write");
        assert!(read_store(&file).is_empty());
        std::fs::write(&file, b"garbage").expect("write");
        assert!(read_store(&file).is_empty());
    }

    #[test]
    fn global_walks_wait_for_the_store_then_take_stale_roots_one_at_a_time() {
        let now = SystemTime::now();
        let fresh = PathBuf::from("fresh");
        let stale = PathBuf::from("stale");
        let missing = PathBuf::from("missing");
        let mut catalog = Catalog::new(Discovery::Fixed(Vec::new()), Some(PathBuf::from("store")));
        catalog.set_roots(vec![fresh.clone(), stale.clone(), missing.clone()]);
        assert_eq!(catalog.next_stale(now, &flag()), None);
        assert!(catalog.begin_disk_read());
        assert!(!catalog.begin_disk_read());
        catalog.finish_disk_read(vec![
            built(&fresh, &["a"], now),
            built(&stale, &["b"], now - REFRESH_AFTER),
            built(Path::new("unknown"), &["c"], now),
        ]);
        let first = flag();
        assert_eq!(catalog.next_stale(now, &first), Some(missing.clone()));
        assert_eq!(catalog.next_stale(now, &flag()), None);
        assert!(catalog.snapshot(None).pending);
        let persisted = catalog.finish_global(&missing, &first, Some(built(&missing, &["c"], now)));
        assert_eq!(persisted.len(), 3);
        assert!(!catalog.snapshot(None).pending);
        assert_eq!(catalog.next_stale(now, &flag()), Some(stale));
    }

    #[test]
    fn visited_folders_keep_the_most_recent_few() {
        let mut catalog = Catalog::new(Discovery::Fixed(Vec::new()), None);
        let walk = flag();
        for nth in 0..MAX_VISITED + 2 {
            let root = PathBuf::from(format!("dir{nth}"));
            assert!(catalog.begin_visit(&root, &walk));
            catalog.finish_visit(&root, &walk, Some(built(&root, &["x"], SystemTime::now())));
        }
        let snapshot = catalog.snapshot(Some(Path::new("dir5")));
        assert_eq!(snapshot.local.expect("local").root(), Path::new("dir5"));
        assert_eq!(snapshot.visited.len(), MAX_VISITED - 1);
        assert!(!snapshot.pending);
    }

    #[test]
    fn a_folder_walk_in_flight_is_not_started_twice_unless_cancelled() {
        let mut catalog = Catalog::new(Discovery::Fixed(Vec::new()), None);
        let root = PathBuf::from("dir");
        let first = flag();
        assert!(catalog.begin_visit(&root, &first));
        assert!(!catalog.begin_visit(&root, &flag()));
        assert!(catalog.snapshot(Some(&root)).pending);
        first.store(true, Ordering::Relaxed);
        let second = flag();
        assert!(catalog.begin_visit(&root, &second));
        catalog.finish_visit(&root, &first, None);
        assert!(!catalog.begin_visit(&root, &flag()));
        catalog.finish_visit(
            &root,
            &second,
            Some(built(&root, &["x"], SystemTime::now())),
        );
        assert!(catalog.snapshot(Some(&root)).local.is_some());
    }

    #[test]
    fn indexes_are_released_only_when_every_picker_is_closed_and_idle() {
        let mut catalog = Catalog::new(Discovery::Fixed(Vec::new()), None);
        let root = PathBuf::from("dir");
        let walk = flag();
        catalog.opened();
        assert!(catalog.claim_janitor());
        assert!(!catalog.claim_janitor());
        assert!(catalog.begin_visit(&root, &walk));
        catalog.finish_visit(&root, &walk, Some(built(&root, &["x"], SystemTime::now())));
        let later = Instant::now() + IDLE_RELEASE * 2;
        assert!(!catalog.release_if_idle(IDLE_RELEASE, later));
        catalog.closed();
        assert!(!catalog.release_if_idle(IDLE_RELEASE, Instant::now()));
        assert!(catalog.release_if_idle(IDLE_RELEASE, later));
        assert!(catalog.snapshot(Some(&root)).local.is_none());
        assert!(catalog.claim_janitor());
    }
}
