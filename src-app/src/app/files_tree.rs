//! File-tree model + pure fs helpers for the docked Files sidebar
//! (PRD `prd-files-tree-sidebar-2026-Q3`, EP-001).
//!
//! Holds the in-memory tree state ([`FilesTreeState`]) and the
//! interaction-time directory read ([`read_dir_sorted`]), plus the pure
//! functions the render path leans on: the markdown-actionability predicate
//! ([`is_markdown`]), the folders-first comparator ([`compare_nodes`]), and the
//! flatten that turns (root + expanded set + cached listings) into the ordered
//! list of visible rows ([`flatten_visible`]). The pure functions carry the
//! crate's `cargo test` coverage; the render + watch wiring lives in
//! `files_sidebar.rs`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One entry in a directory listing. Ignored and hidden entries are filtered
/// before becoming nodes; the flags remain for callers that construct nodes in
/// tests or future "show ignored" UI.
///
/// `size` is the byte length reported by the directory read (0 for
/// directories, and 0 when the stat fails). US-019 needs it to tell, without
/// touching the disk again on the render thread, whether the editor would
/// refuse the file for being over [`code::load::MAX_FILE_BYTES`].
///
/// [`code::load::MAX_FILE_BYTES`]: crate::app::agents_diff::code::load::MAX_FILE_BYTES
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileNode {
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_ignored: bool,
    pub is_hidden: bool,
    pub size: u64,
}

/// A flattened, render-ready row: the node, its indentation depth (component
/// distance from the root's children), and whether it is an expanded directory
/// (drives the chevron direction). Pure output of [`flatten_visible`].
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VisibleRow {
    pub node: FileNode,
    pub depth: usize,
    pub expanded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VisibleRowRef<'a> {
    pub node: &'a FileNode,
    pub depth: usize,
    pub expanded: bool,
}

/// In-memory tree state for the open Files sidebar. Rebuilt on open and on
/// workspace re-root; cleared on close. `children` is a lazy cache keyed by
/// directory path - a directory is read on first expand and kept thereafter.
#[derive(Default)]
pub(crate) struct FilesTreeState {
    pub root: PathBuf,
    pub expanded: HashSet<PathBuf>,
    pub children: HashMap<PathBuf, Vec<FileNode>>,
}

impl FilesTreeState {
    /// US-018: a root-only shell - the root marked expanded but with no
    /// directory listings read yet. Shown synchronously the instant the sidebar
    /// opens so the panel renders without blocking, while
    /// [`crate::PaneFlowApp::spawn_files_hydration`] fills the listings and
    /// installs the non-recursive watcher on a background task.
    pub(crate) fn root_shell(root: PathBuf) -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        Self {
            root,
            expanded,
            children: HashMap::new(),
        }
    }

    /// Whether the root directory has completed its first listing read. A
    /// root shell intentionally starts without this entry so the UI can show
    /// a transient loading state instead of a false empty folder.
    pub(crate) fn root_listing_ready(&self) -> bool {
        self.children.contains_key(&self.root)
    }

    /// Build a state rooted at `root`, restoring `persisted` expanded
    /// directories (US-007). The root is always expanded; each persisted path
    /// is restored only if it still resolves to a directory under the root -
    /// stale paths (deleted folders) are silently dropped. Every restored dir's
    /// listing is read so the flatten has a cache to walk.
    pub(crate) fn hydrated(root: PathBuf, persisted: &[PathBuf]) -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        for p in persisted {
            if *p != root && p.starts_with(&root) && p.is_dir() {
                expanded.insert(p.clone());
            }
        }
        let mut children = HashMap::new();
        for dir in &expanded {
            children.insert(dir.clone(), read_dir_sorted(&root, dir));
        }
        Self {
            root,
            expanded,
            children,
        }
    }
}

/// Case-insensitive `.md` / `.markdown` / `.mdx` predicate. Gates
/// click-to-open + drag actionability - everything else is inert in v1.
pub(crate) fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            e == "md" || e == "markdown" || e == "mdx"
        })
        .unwrap_or(false)
}

/// Extensions whose bytes are binary by construction. Presentation-only
/// heuristic: the editor still sniffs the actual bytes when the file is opened
/// ([`code::load::looks_binary`]), so a mis-classified extension costs a dimmed
/// row, never a wrong read.
///
/// [`code::load::looks_binary`]: crate::app::agents_diff::code::load
const BINARY_EXTENSIONS: &[&str] = &[
    // images
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "avif", "tiff", "tif", "psd", "heic",
    // audio / video
    "mp3", "wav", "flac", "ogg", "m4a", "aac", "mp4", "mkv", "mov", "avi", "webm", "wmv",
    // archives
    "zip", "gz", "bz2", "xz", "zst", "7z", "rar", "tar", "tgz", "jar", "whl",
    // executables / objects
    "exe", "dll", "so", "dylib", "o", "a", "lib", "obj", "pdb", "class", "wasm", "bin", "msi",
    "appimage", "dmg", "deb", "rpm", // fonts / documents / databases
    "ttf", "otf", "woff", "woff2", "eot", "pdf", "db", "sqlite", "sqlite3", "bcmap", "pyc",
];

/// Case-insensitive "this extension is binary" predicate.
pub(crate) fn is_binary_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            BINARY_EXTENSIONS.contains(&e.as_str())
        })
        .unwrap_or(false)
}

/// US-019: would the editor refuse this file? Refused rows stay dimmed in the
/// sidebar but remain clickable - opening one surfaces the US-003 error inside
/// the tab rather than silently doing nothing.
pub(crate) fn editor_refuses(node: &FileNode) -> bool {
    !node.is_dir
        && (is_binary_extension(&node.path)
            || node.size > crate::app::agents_diff::code::load::MAX_FILE_BYTES as u64)
}

/// Display name (the final path component) of a node, lossy for non-UTF-8.
pub(crate) fn node_name(node: &FileNode) -> String {
    node.path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Folders first, then case-insensitive by name (Zed / VS Code convention).
pub(crate) fn compare_nodes(a: &FileNode, b: &FileNode) -> std::cmp::Ordering {
    // `true > false`, so directories (true) sort ahead of files.
    b.is_dir.cmp(&a.is_dir).then_with(|| {
        node_name(a)
            .to_ascii_lowercase()
            .cmp(&node_name(b).to_ascii_lowercase())
    })
}

/// Read a directory into sorted [`FileNode`]s. Non-panicking: an unreadable
/// directory (permissions / removed) yields an empty listing rather than an
/// error. `root` anchors the gitignore matcher so root-level patterns
/// (`target/`, `node_modules/`, …) tint nested entries too.
pub(crate) fn read_dir_sorted(root: &Path, dir: &Path) -> Vec<FileNode> {
    let gitignore = build_gitignore(root, dir);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut nodes: Vec<FileNode> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let is_hidden = is_hidden_name(&path) || has_windows_hidden_attribute(&entry);
            let is_ignored = gitignore
                .as_ref()
                .map(|gi| gi.matched(&path, is_dir).is_ignore())
                .unwrap_or(false);
            if is_hidden || is_ignored {
                return None;
            }
            // Stat only what survives the filter, and only for files: the
            // sort, the gitignore pass and the watcher are untouched, this
            // just carries the size US-019 dims on. A failed stat reads as 0,
            // i.e. "not too large" - `code::load` still checks the real length
            // when the file is actually opened.
            let size = if is_dir {
                0
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            };
            Some(FileNode {
                path,
                is_dir,
                is_ignored,
                is_hidden,
                size,
            })
        })
        .collect();
    nodes.sort_by(compare_nodes);
    nodes
}

fn is_hidden_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
}

#[cfg(windows)]
fn has_windows_hidden_attribute(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    entry
        .metadata()
        .map(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn has_windows_hidden_attribute(_entry: &std::fs::DirEntry) -> bool {
    false
}

/// Build a gitignore matcher rooted at `root` that folds in every `.gitignore`
/// from the root down to `dir`. Approximation: all globs are evaluated against
/// `root` (a nested `.gitignore`'s dir-relative semantics aren't fully
/// reproduced), which is sufficient for the dominant case: the repo-root
/// `.gitignore` filtering `target/` / `node_modules/` before render and watch
/// registration.
fn build_gitignore(root: &Path, dir: &Path) -> Option<ignore::gitignore::Gitignore> {
    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    let mut cur = root.to_path_buf();
    let _ = builder.add(cur.join(".gitignore"));
    if let Ok(rel) = dir.strip_prefix(root) {
        for comp in rel.components() {
            cur.push(comp);
            let _ = builder.add(cur.join(".gitignore"));
        }
    }
    builder.build().ok()
}

/// Flatten (root + expanded set + cached listings) into the ordered list of
/// visible rows, skipping collapsed (or uncached) subtrees. Pure - the root
/// itself is rendered by the header, so this starts at the root's children at
/// depth 0.
#[cfg(test)]
pub(crate) fn flatten_visible(
    root: &Path,
    expanded: &HashSet<PathBuf>,
    children: &HashMap<PathBuf, Vec<FileNode>>,
) -> Vec<VisibleRow> {
    let mut out = Vec::new();
    push_children(root, 0, expanded, children, &mut out);
    out
}

/// Path relative to the workspace root for "Copy relative path" (US-009).
/// Falls back to the absolute path when `path` is not under `root` (e.g. a
/// symlink resolving outside the tree). Pure / unit-tested.
pub(crate) fn workspace_relative_path(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// Coalesce a batch of affected directory paths into the minimal set to
/// re-read (US-005): dedup, then drop any path that has an ancestor also in
/// the set - a parent re-read subsumes its queued descendants (the burst-safe
/// "parent change drops queued child events" rule). Pure / unit-tested.
pub(crate) fn coalesce_by_prefix(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut dirs = dirs;
    dirs.sort();
    dirs.dedup();

    let mut out: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if out.last().is_some_and(|parent| dir.starts_with(parent)) {
            continue;
        }
        out.push(dir);
    }
    out
}

pub(crate) fn flatten_visible_refs<'a>(
    root: &Path,
    expanded: &HashSet<PathBuf>,
    children: &'a HashMap<PathBuf, Vec<FileNode>>,
) -> Vec<VisibleRowRef<'a>> {
    let mut out = Vec::new();
    push_children_refs(root, 0, expanded, children, &mut out);
    out
}

pub(crate) fn visible_len(
    root: &Path,
    expanded: &HashSet<PathBuf>,
    children: &HashMap<PathBuf, Vec<FileNode>>,
) -> usize {
    count_children(root, expanded, children)
}

fn push_children_refs<'a>(
    dir: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    children: &'a HashMap<PathBuf, Vec<FileNode>>,
    out: &mut Vec<VisibleRowRef<'a>>,
) {
    let Some(listing) = children.get(dir) else {
        return;
    };
    for node in listing {
        let is_expanded = node.is_dir && expanded.contains(&node.path);
        out.push(VisibleRowRef {
            node,
            depth,
            expanded: is_expanded,
        });
        if is_expanded {
            push_children_refs(&node.path, depth + 1, expanded, children, out);
        }
    }
}

fn count_children(
    dir: &Path,
    expanded: &HashSet<PathBuf>,
    children: &HashMap<PathBuf, Vec<FileNode>>,
) -> usize {
    let Some(listing) = children.get(dir) else {
        return 0;
    };
    listing
        .iter()
        .map(|node| {
            1 + if node.is_dir && expanded.contains(&node.path) {
                count_children(&node.path, expanded, children)
            } else {
                0
            }
        })
        .sum()
}

#[cfg(test)]
fn push_children(
    dir: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    children: &HashMap<PathBuf, Vec<FileNode>>,
    out: &mut Vec<VisibleRow>,
) {
    let Some(listing) = children.get(dir) else {
        return;
    };
    for node in listing {
        let is_expanded = node.is_dir && expanded.contains(&node.path);
        out.push(VisibleRow {
            node: node.clone(),
            depth,
            expanded: is_expanded,
        });
        if is_expanded {
            push_children(&node.path, depth + 1, expanded, children, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(p: &str) -> FileNode {
        FileNode {
            path: PathBuf::from(p),
            is_dir: true,
            is_ignored: false,
            is_hidden: false,
            size: 0,
        }
    }

    fn file(p: &str) -> FileNode {
        FileNode {
            path: PathBuf::from(p),
            is_dir: false,
            is_ignored: false,
            is_hidden: false,
            size: 0,
        }
    }

    #[test]
    fn root_shell_marks_root_listing_not_ready() {
        let root = PathBuf::from("/r");
        let shell = FilesTreeState::root_shell(root.clone());
        assert!(!shell.root_listing_ready());

        let hydrated = FilesTreeState::hydrated(root, &[]);
        assert!(hydrated.root_listing_ready());
    }

    #[test]
    fn is_markdown_matches_known_extensions() {
        assert!(is_markdown(Path::new("README.md")));
        assert!(is_markdown(Path::new("notes.markdown")));
        assert!(is_markdown(Path::new("doc.mdx")));
    }

    #[test]
    fn is_markdown_is_case_insensitive() {
        assert!(is_markdown(Path::new("README.MD")));
        assert!(is_markdown(Path::new("Doc.Markdown")));
    }

    #[test]
    fn is_markdown_rejects_non_markdown_and_extensionless() {
        assert!(!is_markdown(Path::new("main.rs")));
        assert!(!is_markdown(Path::new("notes.txt")));
        assert!(!is_markdown(Path::new("Makefile")));
        assert!(!is_markdown(Path::new("LICENSE")));
    }

    #[test]
    fn compare_nodes_puts_folders_first_then_case_insensitive() {
        let mut nodes = [file("z.txt"), dir("Tasks"), file("a.md"), dir("assets")];
        nodes.sort_by(compare_nodes);
        let names: Vec<String> = nodes.iter().map(node_name).collect();
        assert_eq!(names, vec!["assets", "Tasks", "a.md", "z.txt"]);
    }

    #[test]
    fn flatten_skips_collapsed_and_uncached_subtrees() {
        let root = PathBuf::from("/r");
        let mut children = HashMap::new();
        children.insert(
            root.clone(),
            vec![dir("/r/src"), dir("/r/docs"), file("/r/a.md")],
        );
        // Only /r/src is cached + expanded; /r/docs stays collapsed.
        children.insert("/r/src".into(), vec![file("/r/src/main.rs")]);
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        expanded.insert(PathBuf::from("/r/src"));

        let rows = flatten_visible(&root, &expanded, &children);
        let names: Vec<(String, usize)> =
            rows.iter().map(|r| (node_name(&r.node), r.depth)).collect();
        assert_eq!(
            names,
            vec![
                ("src".to_string(), 0),
                ("main.rs".to_string(), 1),
                ("docs".to_string(), 0),
                ("a.md".to_string(), 0),
            ]
        );
    }

    #[test]
    fn flatten_empty_dir_yields_no_rows() {
        let root = PathBuf::from("/r");
        let mut children = HashMap::new();
        children.insert(root.clone(), Vec::new());
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        assert!(flatten_visible(&root, &expanded, &children).is_empty());
    }

    #[test]
    fn read_dir_filters_gitignored_and_hidden_entries() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join(".gitignore"), "target/\n").expect("gitignore");
        std::fs::create_dir(root.join("target")).expect("target dir");
        std::fs::write(root.join(".env"), "secret").expect("hidden file");
        std::fs::create_dir(root.join("src")).expect("src dir");
        std::fs::write(root.join("README.md"), "").expect("readme");

        let names: Vec<String> = read_dir_sorted(root, root)
            .into_iter()
            .map(|node| node_name(&node))
            .collect();

        assert_eq!(names, vec!["src".to_string(), "README.md".to_string()]);
    }

    #[test]
    fn coalesce_parent_drops_children() {
        let out = coalesce_by_prefix(vec![
            PathBuf::from("/r"),
            PathBuf::from("/r/src"),
            PathBuf::from("/r/src/inner"),
        ]);
        assert_eq!(out, vec![PathBuf::from("/r")]);
    }

    #[test]
    fn coalesce_preserves_siblings() {
        let mut out = coalesce_by_prefix(vec![PathBuf::from("/r/a"), PathBuf::from("/r/b")]);
        out.sort();
        assert_eq!(out, vec![PathBuf::from("/r/a"), PathBuf::from("/r/b")]);
    }

    #[test]
    fn coalesce_dedups() {
        let out = coalesce_by_prefix(vec![PathBuf::from("/r/a"), PathBuf::from("/r/a")]);
        assert_eq!(out, vec![PathBuf::from("/r/a")]);
    }

    #[test]
    fn relative_path_nested() {
        assert_eq!(
            workspace_relative_path(Path::new("/r"), Path::new("/r/a/b.md")),
            "a/b.md"
        );
    }

    #[test]
    fn relative_path_root_child() {
        assert_eq!(
            workspace_relative_path(Path::new("/r"), Path::new("/r/x")),
            "x"
        );
    }

    #[test]
    fn relative_path_outside_root_falls_back_to_absolute() {
        assert_eq!(
            workspace_relative_path(Path::new("/r"), Path::new("/other/y")),
            "/other/y"
        );
    }

    #[test]
    fn coalesce_does_not_treat_name_prefix_as_ancestor() {
        // `/r/src2` is NOT under `/r/src` - string-prefix would wrongly fold
        // it, but `Path::starts_with` is component-wise so both survive.
        let mut out = coalesce_by_prefix(vec![PathBuf::from("/r/src"), PathBuf::from("/r/src2")]);
        out.sort();
        assert_eq!(out, vec![PathBuf::from("/r/src"), PathBuf::from("/r/src2")]);
    }

    #[test]
    fn flatten_missing_root_listing_is_empty() {
        let root = PathBuf::from("/r");
        let children = HashMap::new();
        let expanded = HashSet::new();
        assert!(flatten_visible(&root, &expanded, &children).is_empty());
    }

    #[test]
    fn hydrated_restores_existing_dirs_and_drops_stale_dirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let src = root.join("src");
        let stale = root.join("stale");
        std::fs::create_dir(&src).expect("src dir");
        std::fs::write(src.join("lib.rs"), "").expect("src file");

        let tree = FilesTreeState::hydrated(root.clone(), &[src.clone(), stale.clone()]);

        assert!(tree.expanded.contains(&root));
        assert!(tree.expanded.contains(&src));
        assert!(!tree.expanded.contains(&stale));
        assert!(tree.children.contains_key(&root));
        assert!(tree.children.contains_key(&src));
        assert!(
            tree.children
                .get(&src)
                .expect("src listing")
                .iter()
                .any(|node| node.path == src.join("lib.rs"))
        );
    }

    // ── US-019: editor-refusal tiers ────────────────────────────────────────

    #[test]
    fn binary_extensions_are_recognized_case_insensitively() {
        assert!(is_binary_extension(Path::new("/w/logo.PNG")));
        assert!(is_binary_extension(Path::new("/w/app.wasm")));
        assert!(is_binary_extension(Path::new("/w/lib.so")));
        assert!(!is_binary_extension(Path::new("/w/main.rs")));
        assert!(!is_binary_extension(Path::new("/w/README")));
        assert!(!is_binary_extension(Path::new("/w/notes.md")));
    }

    #[test]
    fn editor_refuses_binary_and_oversized_files_only() {
        let limit = crate::app::agents_diff::code::load::MAX_FILE_BYTES as u64;

        let mut png = file("/w/logo.png");
        png.size = 12;
        assert!(editor_refuses(&png));

        let mut huge = file("/w/dump.log");
        huge.size = limit + 1;
        assert!(editor_refuses(&huge));

        let mut at_limit = file("/w/dump.log");
        at_limit.size = limit;
        assert!(!editor_refuses(&at_limit));

        let mut source = file("/w/main.rs");
        source.size = 4_096;
        assert!(!editor_refuses(&source));

        // A directory is never "refused" - it toggles, it never opens.
        let mut d = dir("/w/target");
        d.size = limit + 1;
        assert!(!editor_refuses(&d));
    }

    #[test]
    fn read_dir_sorted_reports_file_sizes() {
        let tmp = std::env::temp_dir().join(format!("paneflow-files-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("tmp dir");
        std::fs::write(tmp.join("a.txt"), b"hello").expect("write");
        std::fs::create_dir_all(tmp.join("sub")).expect("subdir");

        let nodes = read_dir_sorted(&tmp, &tmp);
        let a = nodes
            .iter()
            .find(|n| n.path.ends_with("a.txt"))
            .expect("a.txt");
        assert_eq!(a.size, 5);
        let sub = nodes.iter().find(|n| n.path.ends_with("sub")).expect("sub");
        assert!(sub.is_dir);
        assert_eq!(sub.size, 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
