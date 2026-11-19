use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileNode {
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_ignored: bool,
    pub is_hidden: bool,
    pub size: u64,
}

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

#[derive(Clone, Default)]
pub(crate) struct FilesTreeState {
    pub root: PathBuf,
    pub expanded: HashSet<PathBuf>,
    pub children: HashMap<PathBuf, Vec<FileNode>>,
    pub unreadable: HashSet<PathBuf>,
}

impl FilesTreeState {
    pub(crate) fn root_shell(root: PathBuf) -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        Self {
            root,
            expanded,
            children: HashMap::new(),
            unreadable: HashSet::new(),
        }
    }

    pub(crate) fn root_listing_ready(&self) -> bool {
        self.children.contains_key(&self.root)
    }

    pub(crate) fn root_unreadable(&self) -> bool {
        self.unreadable.contains(&self.root)
    }
}

pub(crate) fn node_name(node: &FileNode) -> String {
    node.path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn compare_nodes(a: &FileNode, b: &FileNode) -> std::cmp::Ordering {
    b.is_dir.cmp(&a.is_dir).then_with(|| {
        node_name(a)
            .to_ascii_lowercase()
            .cmp(&node_name(b).to_ascii_lowercase())
    })
}

#[cfg(test)]
pub(crate) fn read_dir_sorted(root: &Path, dir: &Path) -> Vec<FileNode> {
    read_dir_listing(root, dir).unwrap_or_default()
}

pub(crate) fn read_dir_listing(root: &Path, dir: &Path) -> Option<Vec<FileNode>> {
    let gitignore = build_gitignore(root, dir);
    let entries = std::fs::read_dir(dir).ok()?;
    let mut nodes: Vec<FileNode> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            if entry.file_name() == GIT_DIR_NAME {
                return None;
            }
            let path = entry.path();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let is_hidden = has_windows_hidden_attribute(&entry);
            let is_ignored = gitignore.as_ref().is_some_and(|gi| {
                path.starts_with(gi.path())
                    && gi.matched_path_or_any_parents(&path, is_dir).is_ignore()
            });
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
    Some(nodes)
}

pub(crate) const GIT_DIR_NAME: &str = ".git";

pub(crate) fn path_is_git_ignored(root: &Path, path: &Path, is_dir: bool) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if !path.starts_with(root) || path == root {
        return false;
    }
    if path.strip_prefix(root).is_ok_and(|rel| {
        rel.components()
            .any(|part| part.as_os_str() == GIT_DIR_NAME)
    }) {
        return true;
    }
    build_gitignore(root, parent)
        .is_some_and(|gi| gi.matched_path_or_any_parents(path, is_dir).is_ignore())
}

pub(crate) fn is_hidden_entry(entry: &std::fs::DirEntry) -> bool {
    is_hidden_name(&entry.path()) || has_windows_hidden_attribute(entry)
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

#[cfg(test)]
pub(crate) fn flatten_visible(
    root: &Path,
    expanded: &HashSet<PathBuf>,
    children: &HashMap<PathBuf, Vec<FileNode>>,
) -> Vec<VisibleRow> {
    flatten_visible_refs(root, expanded, children)
        .into_iter()
        .map(|row| VisibleRow {
            node: row.node.clone(),
            depth: row.depth,
            expanded: row.expanded,
        })
        .collect()
}

pub(crate) fn workspace_relative_path(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
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
        let mut shell = FilesTreeState::root_shell(root.clone());
        assert!(!shell.root_listing_ready());
        shell.children.insert(root, Vec::new());
        assert!(shell.root_listing_ready());
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
    fn read_dir_marks_gitignored_entries_shows_dotfiles_and_hides_only_git() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join(".gitignore"), "target/\n").expect("gitignore");
        std::fs::create_dir_all(root.join("target").join("debug")).expect("target dir");
        std::fs::write(root.join(".env"), "secret").expect("dot file");
        std::fs::create_dir(root.join(".git")).expect("git dir");
        std::fs::create_dir(root.join("src")).expect("src dir");
        std::fs::write(root.join("README.md"), "").expect("readme");

        let nodes = read_dir_sorted(root, root);
        let shown: Vec<(String, bool, bool)> = nodes
            .iter()
            .map(|node| (node_name(node), node.is_ignored, node.is_hidden))
            .collect();
        assert_eq!(
            shown,
            vec![
                ("src".to_string(), false, false),
                ("target".to_string(), true, false),
                (".env".to_string(), false, false),
                (".gitignore".to_string(), false, false),
                ("README.md".to_string(), false, false),
            ]
        );

        let inside = read_dir_sorted(root, &root.join("target"));
        assert_eq!(inside.len(), 1);
        assert!(
            inside[0].is_ignored,
            "entries under an ignored folder inherit its status"
        );
        assert!(path_is_git_ignored(
            root,
            &root.join("target").join("debug").join("app"),
            false
        ));
        assert!(path_is_git_ignored(
            root,
            &root.join(".git").join("HEAD"),
            false
        ));
        assert!(!path_is_git_ignored(
            root,
            &root.join("src").join("main.rs"),
            false
        ));
        assert!(!path_is_git_ignored(
            root,
            Path::new("/elsewhere/main.rs"),
            false
        ));
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
    fn flatten_missing_root_listing_is_empty() {
        let root = PathBuf::from("/r");
        let children = HashMap::new();
        let expanded = HashSet::new();
        assert!(flatten_visible(&root, &expanded, &children).is_empty());
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
