use std::collections::HashSet;
use std::ops::Range;
use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::fuzzy;

pub(super) const LOCAL_CAP: usize = 100_000;
pub(super) const GLOBAL_CAP: usize = 1_000_000;

const NO_PARENT: u32 = u32::MAX;
const DIR_FLAG: u16 = 1 << 15;
const HIDDEN_FLAG: u16 = 1 << 15;
const MAX_DEPTH: usize = (DIR_FLAG - 1) as usize;
const MAX_NAME: usize = (HIDDEN_FLAG - 1) as usize;
const SEALED_NAMES: [&str; 1] = ["node_modules"];
const SKIPPED_NAMES: [&str; 1] = [".git"];
const RECORD_LEN: usize = 4;

#[derive(Clone, Copy)]
struct Node {
    parent: u32,
    name: u32,
    mask: u32,
    name_len: u16,
    depth: u16,
}

pub(super) struct Index {
    root: PathBuf,
    names: String,
    nodes: Vec<Node>,
    built: SystemTime,
    complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Located {
    Root,
    Node(u32),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Push {
    Added,
    Skipped,
    Full,
}

pub(super) struct Builder {
    index: Index,
    cap: usize,
    ancestors: Vec<u32>,
    skip_below: Option<usize>,
}

impl Builder {
    pub(super) fn new(root: PathBuf, cap: usize) -> Self {
        Self {
            index: Index {
                root,
                names: String::new(),
                nodes: Vec::new(),
                built: SystemTime::now(),
                complete: true,
            },
            cap: cap.min(NO_PARENT as usize),
            ancestors: Vec::new(),
            skip_below: None,
        }
    }

    pub(super) fn push(
        &mut self,
        depth: usize,
        name: Option<&str>,
        is_dir: bool,
        hidden: bool,
    ) -> Push {
        if let Some(limit) = self.skip_below {
            if depth > limit {
                return Push::Skipped;
            }
            self.skip_below = None;
        }
        if self.index.nodes.len() >= self.cap {
            self.index.complete = false;
            return Push::Full;
        }
        self.ancestors.truncate(depth.saturating_sub(1));
        let name = name.filter(|name| {
            (1..=MAX_DEPTH).contains(&depth)
                && self.ancestors.len() == depth - 1
                && !name.is_empty()
                && name.len() <= MAX_NAME
                && !name
                    .chars()
                    .any(|ch| ch.is_control() || std::path::is_separator(ch))
        });
        let (Some(name), Ok(offset)) = (name, u32::try_from(self.index.names.len())) else {
            if is_dir {
                self.skip_below = Some(depth);
            }
            return Push::Skipped;
        };
        let parent = self.ancestors.last().copied().unwrap_or(NO_PARENT);
        let inherited = self
            .index
            .nodes
            .get(parent as usize)
            .map_or(0, |node| node.mask);
        let id = self.index.nodes.len() as u32;
        self.index.names.push_str(name);
        self.index.nodes.push(Node {
            parent,
            name: offset,
            mask: inherited | fuzzy::mask_of(name),
            name_len: name.len() as u16 | if hidden { HIDDEN_FLAG } else { 0 },
            depth: depth as u16 | if is_dir { DIR_FLAG } else { 0 },
        });
        if is_dir {
            self.ancestors.push(id);
        }
        Push::Added
    }

    pub(super) fn finish(mut self, built: SystemTime) -> Index {
        self.index.built = built;
        self.index.names.shrink_to_fit();
        self.index.nodes.shrink_to_fit();
        self.index
    }
}

impl Index {
    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    pub(super) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub(super) fn built(&self) -> SystemTime {
        self.built
    }

    pub(super) fn complete(&self) -> bool {
        self.complete
    }

    pub(super) fn name(&self, node: u32) -> &str {
        let node = &self.nodes[node as usize];
        let start = node.name as usize;
        &self.names[start..start + (node.name_len & !HIDDEN_FLAG) as usize]
    }

    pub(super) fn is_dir(&self, node: u32) -> bool {
        self.nodes[node as usize].depth & DIR_FLAG != 0
    }

    pub(super) fn is_hidden(&self, node: u32) -> bool {
        self.nodes[node as usize].name_len & HIDDEN_FLAG != 0
    }

    pub(super) fn depth(&self, node: u32) -> usize {
        (self.nodes[node as usize].depth & !DIR_FLAG) as usize
    }

    pub(super) fn parent(&self, node: u32) -> Option<u32> {
        let parent = self.nodes[node as usize].parent;
        (parent != NO_PARENT).then_some(parent)
    }

    pub(super) fn mask(&self, node: u32) -> u32 {
        self.nodes[node as usize].mask
    }

    fn located_depth(&self, at: Located) -> usize {
        match at {
            Located::Root => 0,
            Located::Node(node) => self.depth(node),
        }
    }

    pub(super) fn subtree(&self, at: Located) -> Range<u32> {
        match at {
            Located::Root => 0..self.len() as u32,
            Located::Node(node) => {
                let depth = self.depth(node);
                let end = (node + 1..self.nodes.len() as u32)
                    .find(|&next| self.depth(next) <= depth)
                    .unwrap_or(self.nodes.len() as u32);
                node + 1..end
            }
        }
    }

    pub(super) fn locate(&self, path: &Path) -> Option<Located> {
        let relative = strip_root(path, &self.root)?;
        let mut at = Located::Root;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return None;
            };
            at = Located::Node(self.child(at, name.to_str()?)?);
        }
        Some(at)
    }

    fn child(&self, at: Located, name: &str) -> Option<u32> {
        let wanted = self.located_depth(at) + 1;
        let start = match at {
            Located::Root => 0,
            Located::Node(node) => node + 1,
        };
        (start..self.nodes.len() as u32)
            .take_while(|&next| self.depth(next) >= wanted)
            .find(|&next| self.depth(next) == wanted && names_equal(self.name(next), name))
    }

    pub(super) fn path_of(&self, node: u32) -> PathBuf {
        let mut text = String::new();
        self.write_path(node, Located::Root, &mut text, &mut Vec::new());
        self.root.join(text)
    }

    pub(super) fn write_path(
        &self,
        node: u32,
        from: Located,
        out: &mut String,
        chain: &mut Vec<u32>,
    ) {
        out.clear();
        chain.clear();
        let stop = match from {
            Located::Root => NO_PARENT,
            Located::Node(ancestor) => ancestor,
        };
        let mut current = node;
        while current != stop && current != NO_PARENT {
            chain.push(current);
            current = self.nodes[current as usize].parent;
        }
        for (nth, &id) in chain.iter().rev().enumerate() {
            if nth > 0 {
                out.push(MAIN_SEPARATOR);
            }
            out.push_str(self.name(id));
        }
    }

    pub(super) fn path_len(&self, node: u32, from: Located) -> usize {
        let stop = match from {
            Located::Root => NO_PARENT,
            Located::Node(ancestor) => ancestor,
        };
        let mut total = 0;
        let mut current = node;
        while current != stop && current != NO_PARENT {
            let entry = &self.nodes[current as usize];
            total += (entry.name_len & !HIDDEN_FLAG) as usize + usize::from(total > 0);
            current = entry.parent;
        }
        total
    }

    pub(super) fn encode(&self, out: &mut Vec<u8>) -> Option<()> {
        let root = self.root.to_str()?;
        let built = self.built.duration_since(UNIX_EPOCH).ok()?.as_secs();
        out.extend_from_slice(&u32::try_from(root.len()).ok()?.to_le_bytes());
        out.extend_from_slice(root.as_bytes());
        out.extend_from_slice(&built.to_le_bytes());
        out.push(u8::from(self.complete));
        out.extend_from_slice(&u32::try_from(self.nodes.len()).ok()?.to_le_bytes());
        out.extend_from_slice(&u32::try_from(self.names.len()).ok()?.to_le_bytes());
        out.reserve(self.nodes.len() * RECORD_LEN + self.names.len());
        for node in &self.nodes {
            out.extend_from_slice(&node.depth.to_le_bytes());
            out.extend_from_slice(&node.name_len.to_le_bytes());
        }
        out.extend_from_slice(self.names.as_bytes());
        Some(())
    }

    pub(super) fn decode(input: &mut &[u8]) -> Option<Self> {
        let root_len = take_u32(input)? as usize;
        let root = std::str::from_utf8(take(input, root_len)?).ok()?;
        let built = UNIX_EPOCH.checked_add(Duration::from_secs(take_u64(input)?))?;
        let complete = take(input, 1)?[0] != 0;
        let count = take_u32(input)? as usize;
        let names_len = take_u32(input)? as usize;
        let records = take(input, count.checked_mul(RECORD_LEN)?)?;
        let names = std::str::from_utf8(take(input, names_len)?).ok()?;
        let mut builder = Builder::new(PathBuf::from(root), count);
        let mut offset = 0;
        for record in records.as_chunks::<RECORD_LEN>().0 {
            let depth = u16::from_le_bytes([record[0], record[1]]);
            let name_len = u16::from_le_bytes([record[2], record[3]]);
            let length = (name_len & !HIDDEN_FLAG) as usize;
            let name = names.get(offset..offset + length)?;
            offset += length;
            let pushed = builder.push(
                (depth & !DIR_FLAG) as usize,
                Some(name),
                depth & DIR_FLAG != 0,
                name_len & HIDDEN_FLAG != 0,
            );
            if pushed != Push::Added {
                return None;
            }
        }
        if offset != names.len() {
            return None;
        }
        let mut index = builder.finish(built);
        index.complete = complete;
        Some(index)
    }
}

pub(super) fn walk(
    root: &Path,
    cap: usize,
    sealed: &[PathBuf],
    cancelled: &AtomicBool,
) -> Option<Index> {
    let inside: HashSet<PathBuf> = sealed
        .iter()
        .filter_map(|sealed| strip_root(sealed, root))
        .filter(|rest| !rest.as_os_str().is_empty())
        .map(|rest| root.join(rest))
        .collect();
    let admitted = Arc::new(Mutex::new(inside));
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .same_file_system(cfg!(unix))
        .filter_entry(move |entry| admit(entry, &admitted))
        .build();
    let mut builder = Builder::new(root.to_path_buf(), cap);
    for result in walker {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let Ok(entry) = result else {
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }
        let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
        let pushed = builder.push(
            entry.depth(),
            entry.file_name().to_str(),
            is_dir,
            is_hidden(&entry),
        );
        if pushed == Push::Full {
            break;
        }
    }
    Some(builder.finish(SystemTime::now()))
}

fn admit(entry: &ignore::DirEntry, fence: &Mutex<HashSet<PathBuf>>) -> bool {
    let name = entry.file_name();
    if SKIPPED_NAMES.iter().any(|skipped| name == *skipped) {
        return false;
    }
    let mut fence = fence.lock().unwrap_or_else(PoisonError::into_inner);
    if entry
        .path()
        .parent()
        .is_some_and(|parent| fence.contains(parent))
    {
        return false;
    }
    let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
    if is_dir && (is_hidden(entry) || SEALED_NAMES.iter().any(|sealed| name == *sealed)) {
        fence.insert(entry.path().to_path_buf());
    }
    true
}

fn is_hidden(entry: &ignore::DirEntry) -> bool {
    entry.file_name().as_encoded_bytes().starts_with(b".") || has_hidden_attribute(entry)
}

#[cfg(windows)]
fn has_hidden_attribute(entry: &ignore::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    entry
        .metadata()
        .is_ok_and(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
}

#[cfg(not(windows))]
fn has_hidden_attribute(_entry: &ignore::DirEntry) -> bool {
    false
}

pub(super) fn platform_data_dirs(home: &Path) -> Vec<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["AppData"]
    } else if cfg!(target_os = "macos") {
        &["Library"]
    } else {
        &[]
    };
    names.iter().map(|name| home.join(name)).collect()
}

pub(super) fn reaches(root: &Path, path: &Path, sealed: &[PathBuf]) -> bool {
    let Some(relative) = strip_root(path, root) else {
        return false;
    };
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return false;
        };
        current.push(name);
        if name.as_encoded_bytes().starts_with(b".")
            || SEALED_NAMES.iter().any(|sealed| name == *sealed)
            || sealed.iter().any(|sealed| same_path(sealed, &current))
        {
            return false;
        }
    }
    true
}

pub(super) fn strip_root<'a>(path: &'a Path, root: &Path) -> Option<&'a Path> {
    let mut rest = path.components();
    for wanted in root.components() {
        let got = rest.next()?;
        if !components_equal(got, wanted) {
            return None;
        }
    }
    Some(rest.as_path())
}

pub(super) fn same_path(left: &Path, right: &Path) -> bool {
    strip_root(left, right).is_some_and(|rest| rest.as_os_str().is_empty())
}

fn components_equal(left: Component<'_>, right: Component<'_>) -> bool {
    if cfg!(windows) {
        left.as_os_str().eq_ignore_ascii_case(right.as_os_str())
    } else {
        left == right
    }
}

fn names_equal(left: &str, right: &str) -> bool {
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn take<'a>(input: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
    if input.len() < len {
        return None;
    }
    let (head, tail) = input.split_at(len);
    *input = tail;
    Some(head)
}

fn take_u32(input: &mut &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(take(input, 4)?.try_into().ok()?))
}

fn take_u64(input: &mut &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(take(input, 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Index {
        let mut builder = Builder::new(PathBuf::from("root"), GLOBAL_CAP);
        for (depth, name, is_dir) in [
            (1, "src", true),
            (2, "app", true),
            (3, "main.rs", false),
            (2, "lib.rs", false),
            (1, ".env", false),
            (1, "docs", true),
            (2, "guide.md", false),
        ] {
            assert_eq!(
                builder.push(depth, Some(name), is_dir, name.starts_with('.')),
                Push::Added
            );
        }
        builder.finish(UNIX_EPOCH + Duration::from_secs(42))
    }

    fn path(index: &Index, node: u32, from: Located) -> String {
        let mut text = String::new();
        index.write_path(node, from, &mut text, &mut Vec::new());
        text
    }

    fn native(relative: &str) -> String {
        relative.replace('/', std::path::MAIN_SEPARATOR_STR)
    }

    #[test]
    fn nodes_rebuild_their_paths_from_preorder_depths() {
        let index = sample();
        assert_eq!(path(&index, 2, Located::Root), native("src/app/main.rs"));
        assert_eq!(path(&index, 3, Located::Root), native("src/lib.rs"));
        assert_eq!(path(&index, 2, Located::Node(0)), native("app/main.rs"));
        assert_eq!(
            index.path_len(2, Located::Root),
            native("src/app/main.rs").len()
        );
        assert_eq!(index.path_len(2, Located::Node(1)), "main.rs".len());
        assert_eq!(
            index.path_of(6),
            Path::new("root").join("docs").join("guide.md")
        );
        assert!(index.is_dir(1) && !index.is_dir(2));
        assert!(index.is_hidden(4) && !index.is_hidden(3));
    }

    #[test]
    fn a_subtree_is_the_contiguous_run_after_its_folder() {
        let index = sample();
        assert_eq!(index.subtree(Located::Root), 0..7);
        assert_eq!(index.subtree(Located::Node(0)), 1..4);
        assert_eq!(index.subtree(Located::Node(1)), 2..3);
        assert_eq!(index.subtree(Located::Node(5)), 6..7);
        assert_eq!(index.subtree(Located::Node(4)), 5..5);
    }

    #[test]
    fn a_path_is_located_by_walking_its_folders() {
        let index = sample();
        assert_eq!(index.locate(Path::new("root")), Some(Located::Root));
        assert_eq!(
            index.locate(&Path::new("root").join("src").join("app")),
            Some(Located::Node(1))
        );
        assert_eq!(
            index.locate(&Path::new("root").join("docs")),
            Some(Located::Node(5))
        );
        assert_eq!(index.locate(&Path::new("root").join("app")), None);
        assert_eq!(index.locate(Path::new("elsewhere")), None);
        if cfg!(windows) {
            assert_eq!(
                index.locate(&Path::new("ROOT").join("SRC").join("App")),
                Some(Located::Node(1))
            );
        }
    }

    #[test]
    fn the_path_mask_covers_every_folder_above() {
        let index = sample();
        assert_eq!(
            index.mask(2),
            fuzzy::mask_of("src") | fuzzy::mask_of("app") | fuzzy::mask_of("main.rs")
        );
    }

    #[test]
    fn an_unusable_folder_drops_everything_below_it() {
        let mut builder = Builder::new(PathBuf::from("root"), GLOBAL_CAP);
        assert_eq!(builder.push(1, None, true, false), Push::Skipped);
        assert_eq!(
            builder.push(2, Some("inside.rs"), false, false),
            Push::Skipped
        );
        assert_eq!(
            builder.push(1, Some("bad\u{7}name"), false, false),
            Push::Skipped
        );
        assert_eq!(builder.push(1, Some("kept.rs"), false, false), Push::Added);
        assert_eq!(
            builder.push(3, Some("orphan.rs"), false, false),
            Push::Skipped
        );
        let index = builder.finish(SystemTime::now());
        assert_eq!(index.len(), 1);
        assert_eq!(index.name(0), "kept.rs");
    }

    #[test]
    fn the_cap_marks_the_index_incomplete() {
        let mut builder = Builder::new(PathBuf::from("root"), 1);
        assert_eq!(builder.push(1, Some("a"), false, false), Push::Added);
        assert_eq!(builder.push(1, Some("b"), false, false), Push::Full);
        let index = builder.finish(SystemTime::now());
        assert!(!index.complete());
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn an_index_round_trips_through_its_encoding() {
        let index = sample();
        let mut bytes = Vec::new();
        index.encode(&mut bytes).expect("encodable");
        let mut input = bytes.as_slice();
        let decoded = Index::decode(&mut input).expect("decodable");
        assert!(input.is_empty());
        assert_eq!(decoded.len(), index.len());
        assert_eq!(decoded.root(), index.root());
        assert_eq!(decoded.built(), index.built());
        assert!(decoded.complete());
        for node in 0..index.len() as u32 {
            assert_eq!(
                path(&decoded, node, Located::Root),
                path(&index, node, Located::Root)
            );
            assert_eq!(decoded.is_hidden(node), index.is_hidden(node));
            assert_eq!(decoded.mask(node), index.mask(node));
        }
    }

    #[test]
    fn a_damaged_encoding_is_rejected() {
        let index = sample();
        let mut bytes = Vec::new();
        index.encode(&mut bytes).expect("encodable");
        assert!(Index::decode(&mut &bytes[..bytes.len() - 1]).is_none());
        let records = bytes.len() - index.names.len() - index.len() * RECORD_LEN;
        let mut orphaned = bytes.clone();
        orphaned[records] = 5;
        assert!(Index::decode(&mut orphaned.as_slice()).is_none());
    }

    #[test]
    fn walking_indexes_hidden_and_sealed_folders_without_entering_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        for relative in [
            "src/main.rs",
            ".env",
            ".github/workflows/ci.yml",
            "node_modules/pkg/index.js",
            "AppData/Local/cache.bin",
            "target/debug/app",
            ".git/config",
        ] {
            let file = root.join(relative);
            std::fs::create_dir_all(file.parent().expect("parent")).expect("dirs");
            std::fs::write(file, b"").expect("file");
        }
        std::fs::write(root.join(".ignore"), "target/\n").expect("ignore file");
        let sealed = [root.join("AppData")];
        let index = walk(root, GLOBAL_CAP, &sealed, &AtomicBool::new(false)).expect("walk");
        let mut paths: Vec<String> = (0..index.len() as u32)
            .map(|node| path(&index, node, Located::Root))
            .collect();
        paths.sort();
        let mut expected: Vec<String> = [
            ".env",
            ".github",
            ".ignore",
            "AppData",
            "node_modules",
            "src",
            "src/main.rs",
        ]
        .iter()
        .map(|relative| native(relative))
        .collect();
        expected.sort();
        assert_eq!(paths, expected);
        let inside = walk(
            &root.join("AppData"),
            GLOBAL_CAP,
            &sealed,
            &AtomicBool::new(false),
        )
        .expect("walk");
        assert_eq!(inside.len(), 2);
    }

    #[test]
    fn a_cancelled_walk_yields_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"").expect("file");
        assert!(walk(dir.path(), GLOBAL_CAP, &[], &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn a_root_reaches_the_folders_its_walk_enters() {
        let root = Path::new("home");
        let sealed = [root.join("AppData")];
        assert!(reaches(root, root, &sealed));
        assert!(reaches(root, &root.join("code").join("app"), &sealed));
        assert!(!reaches(
            root,
            &root.join(".paneflow").join("worktrees"),
            &sealed
        ));
        assert!(!reaches(root, &root.join("AppData").join("Local"), &sealed));
        assert!(!reaches(
            root,
            &root.join("web").join("node_modules"),
            &sealed
        ));
        assert!(!reaches(root, Path::new("elsewhere"), &sealed));
    }
}
