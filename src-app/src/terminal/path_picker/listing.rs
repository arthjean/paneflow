use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf, is_separator};
use std::sync::Arc;
use std::sync::atomic::{self, AtomicBool};

use super::fuzzy::{self, PathMatch, Pattern};
use super::wsl::WslRoots;
use crate::terminal::types::ShellQuoting;

const MAX_INDEXED: usize = 100_000;
const MAX_RESULTS: usize = 200;
const MAX_RECENT_BROWSED: usize = 5;
const RECENT_BONUS: i32 = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct IndexEntry {
    pub(super) relative: String,
    pub(super) is_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Row {
    pub(super) path: PathBuf,
    pub(super) label: String,
    pub(super) highlights: Vec<Range<usize>>,
    pub(super) is_dir: bool,
    pub(super) recent: bool,
    pub(super) completion: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Status {
    Indexing,
    NoMatch,
    EmptyFolder,
    FolderNotFound,
    NoWorkingDirectory,
}

impl Status {
    pub(super) fn text(self) -> &'static str {
        match self {
            Status::Indexing => "Indexing files…",
            Status::NoMatch => "No matching path",
            Status::EmptyFolder => "Empty folder",
            Status::FolderNotFound => "Folder not found",
            Status::NoWorkingDirectory => "Working directory unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Listing {
    Rows(Vec<Row>),
    Status(Status),
}

pub(super) struct Request {
    pub(super) query: String,
    pub(super) base: Option<PathBuf>,
    pub(super) home: Option<PathBuf>,
    pub(super) wsl: Option<Arc<WslRoots>>,
    pub(super) index: Option<Arc<[IndexEntry]>>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Query<'a> {
    Browse,
    Search(&'a str),
    Navigate {
        typed_dir: &'a str,
        fragment: &'a str,
    },
}

pub(super) fn parse(query: &str) -> Query<'_> {
    if query.is_empty() {
        return Query::Browse;
    }
    if query == "~" {
        return Query::Navigate {
            typed_dir: query,
            fragment: "",
        };
    }
    match query.rfind(is_separator) {
        Some(index) => Query::Navigate {
            typed_dir: &query[..=index],
            fragment: &query[index + 1..],
        },
        None => Query::Search(query),
    }
}

pub(super) fn compute(request: &Request, recent: &[PathBuf]) -> Listing {
    match parse(&request.query) {
        Query::Browse => browse(request.base.as_deref(), recent),
        Query::Search(text) => search(request, text, recent),
        Query::Navigate {
            typed_dir,
            fragment,
        } => navigate(request, typed_dir, fragment, recent),
    }
}

pub(super) fn build_index(root: &Path, cancelled: &AtomicBool) -> Vec<IndexEntry> {
    let mut entries = Vec::new();
    for result in ignore::WalkBuilder::new(root).build() {
        if cancelled.load(atomic::Ordering::Relaxed) || entries.len() == MAX_INDEXED {
            break;
        }
        let Ok(entry) = result else {
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }
        let Some(relative) = entry.path().strip_prefix(root).ok().and_then(Path::to_str) else {
            continue;
        };
        if relative.chars().any(char::is_control) {
            continue;
        }
        entries.push(IndexEntry {
            relative: relative.to_owned(),
            is_dir: entry.file_type().is_some_and(|kind| kind.is_dir()),
        });
    }
    entries
}

pub(super) fn insertion_text(
    path: &Path,
    base: Option<&Path>,
    quoting: ShellQuoting,
    wsl: Option<&WslRoots>,
) -> Option<String> {
    let relative = base
        .and_then(|base| path.strip_prefix(base).ok())
        .filter(|relative| !relative.as_os_str().is_empty());
    let text = match (relative, wsl) {
        (Some(relative), _) => {
            let relative = relative.to_str()?;
            if relative.starts_with('-') {
                shell_separators(format!(".{MAIN_SEPARATOR}{relative}"), quoting)
            } else {
                shell_separators(relative.to_owned(), quoting)
            }
        }
        (None, Some(wsl)) => wsl.to_linux(path)?,
        (None, None) => shell_separators(path.to_str()?.to_owned(), quoting),
    };
    if text.chars().any(char::is_control) {
        return None;
    }
    Some(crate::terminal::input::quote_path_for_shell(&text, quoting))
}

fn shell_separators(text: String, quoting: ShellQuoting) -> String {
    if cfg!(windows) && matches!(quoting, ShellQuoting::Posix | ShellQuoting::Wsl) {
        text.replace('\\', "/")
    } else {
        text
    }
}

pub(super) fn placeholder(
    base: Option<&Path>,
    home: Option<&Path>,
    wsl: Option<&WslRoots>,
) -> String {
    let Some(base) = base else {
        return "Type a path…".to_string();
    };
    let (shown, separator) = match wsl.and_then(|wsl| wsl.display(base)) {
        Some(shown) => (shown, '/'),
        None => (
            match home.and_then(|home| base.strip_prefix(home).ok()) {
                Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
                Some(rest) => format!("~{MAIN_SEPARATOR}{}", rest.display()),
                None => base.display().to_string(),
            },
            MAIN_SEPARATOR,
        ),
    };
    if shown.ends_with(is_separator) {
        shown
    } else {
        format!("{shown}{separator}")
    }
}

fn browse(base: Option<&Path>, recent: &[PathBuf]) -> Listing {
    let Some(base) = base else {
        return Listing::Status(Status::NoWorkingDirectory);
    };
    let Some(listed) = list_dir(base, false) else {
        return Listing::Status(Status::FolderNotFound);
    };
    let recents = existing_recents(base, recent);
    let mut shown = HashSet::new();
    let mut rows = Vec::new();
    for entry in recents.into_iter().take(MAX_RECENT_BROWSED) {
        shown.insert(entry.path.clone());
        rows.push(Row {
            completion: completion(&entry.relative, entry.is_dir, MAIN_SEPARATOR),
            label: entry.relative,
            path: entry.path,
            highlights: Vec::new(),
            is_dir: entry.is_dir,
            recent: true,
        });
    }
    for entry in listed {
        if shown.contains(&entry.path) {
            continue;
        }
        rows.push(Row {
            recent: recent.contains(&entry.path),
            completion: completion(&entry.name, entry.is_dir, MAIN_SEPARATOR),
            label: entry.name,
            path: entry.path,
            highlights: Vec::new(),
            is_dir: entry.is_dir,
        });
    }
    if rows.is_empty() {
        return Listing::Status(Status::EmptyFolder);
    }
    Listing::Rows(rows)
}

struct Hit<'a> {
    relative: &'a str,
    is_dir: bool,
    recent: bool,
    found: PathMatch,
}

impl Hit<'_> {
    fn score(&self) -> i32 {
        self.found.score + if self.recent { RECENT_BONUS } else { 0 }
    }

    fn rank(&self, other: &Self) -> Ordering {
        other
            .score()
            .cmp(&self.score())
            .then_with(|| self.relative.len().cmp(&other.relative.len()))
            .then_with(|| self.relative.cmp(other.relative))
    }
}

fn search(request: &Request, text: &str, recent: &[PathBuf]) -> Listing {
    let Some(base) = request.base.as_deref() else {
        return Listing::Status(Status::NoWorkingDirectory);
    };
    let Some(index) = request.index.as_deref() else {
        return Listing::Status(Status::Indexing);
    };
    let pattern = Pattern::new(text);
    let recents = existing_recents(base, recent);
    let mut recent_seen: HashMap<&str, bool> = recents
        .iter()
        .map(|entry| (entry.relative.as_str(), false))
        .collect();
    let mut hits = Vec::new();
    for entry in index {
        let Some(found) = fuzzy::match_path(&entry.relative, &pattern) else {
            continue;
        };
        let recent = match recent_seen.get_mut(entry.relative.as_str()) {
            Some(seen) => {
                *seen = true;
                true
            }
            None => false,
        };
        hits.push(Hit {
            relative: &entry.relative,
            is_dir: entry.is_dir,
            recent,
            found,
        });
    }
    for entry in &recents {
        if recent_seen.get(entry.relative.as_str()) == Some(&false)
            && let Some(found) = fuzzy::match_path(&entry.relative, &pattern)
        {
            hits.push(Hit {
                relative: &entry.relative,
                is_dir: entry.is_dir,
                recent: true,
                found,
            });
        }
    }
    if hits.len() > MAX_RESULTS {
        hits.select_nth_unstable_by(MAX_RESULTS, Hit::rank);
        hits.truncate(MAX_RESULTS);
    }
    hits.sort_by(Hit::rank);
    if hits.is_empty() {
        return Listing::Status(Status::NoMatch);
    }
    Listing::Rows(
        hits.into_iter()
            .map(|hit| Row {
                path: base.join(hit.relative),
                label: hit.relative.to_owned(),
                highlights: hit.found.highlights,
                is_dir: hit.is_dir,
                recent: hit.recent,
                completion: completion(hit.relative, hit.is_dir, MAIN_SEPARATOR),
            })
            .collect(),
    )
}

fn navigate(request: &Request, typed_dir: &str, fragment: &str, recent: &[PathBuf]) -> Listing {
    let Some(dir) = resolve_dir(
        typed_dir,
        request.base.as_deref(),
        request.home.as_deref(),
        request.wsl.as_deref(),
    ) else {
        return Listing::Status(Status::NoWorkingDirectory);
    };
    let Some(listed) = list_dir(&dir, fragment.starts_with('.')) else {
        return Listing::Status(Status::FolderNotFound);
    };
    let separator = typed_dir
        .chars()
        .next_back()
        .filter(|last| is_separator(*last))
        .unwrap_or(MAIN_SEPARATOR);
    let prefix = if typed_dir == "~" {
        format!("~{separator}")
    } else {
        typed_dir.to_owned()
    };
    let pattern = Pattern::new(fragment);
    let mut hits: Vec<(i32, Row)> = listed
        .into_iter()
        .filter_map(|entry| {
            let found = fuzzy::match_path(&entry.name, &pattern)?;
            let label = format!("{prefix}{}", entry.name);
            Some((
                found.score,
                Row {
                    recent: recent.contains(&entry.path),
                    completion: completion(&label, entry.is_dir, separator),
                    highlights: found
                        .highlights
                        .into_iter()
                        .map(|range| range.start + prefix.len()..range.end + prefix.len())
                        .collect(),
                    label,
                    path: entry.path,
                    is_dir: entry.is_dir,
                },
            ))
        })
        .collect();
    hits.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    hits.truncate(MAX_RESULTS);
    if hits.is_empty() {
        return Listing::Status(if pattern.is_empty() {
            Status::EmptyFolder
        } else {
            Status::NoMatch
        });
    }
    Listing::Rows(hits.into_iter().map(|(_, row)| row).collect())
}

fn completion(label: &str, is_dir: bool, separator: char) -> String {
    if is_dir {
        format!("{label}{separator}")
    } else {
        label.to_owned()
    }
}

fn resolve_dir(
    typed_dir: &str,
    base: Option<&Path>,
    home: Option<&Path>,
    wsl: Option<&WslRoots>,
) -> Option<PathBuf> {
    if let Some(wsl) = wsl.filter(|_| typed_dir.starts_with('/')) {
        return Some(normalize(&wsl.to_windows(typed_dir)));
    }
    let joined = match typed_dir.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(is_separator) => {
            home?.join(rest.trim_start_matches(is_separator))
        }
        _ => {
            let typed = Path::new(typed_dir);
            match base {
                Some(base) => base.join(typed),
                None if typed.is_absolute() => typed.to_path_buf(),
                None => return None,
            }
        }
    };
    Some(normalize(&joined))
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push(component);
                }
            }
            component => normalized.push(component),
        }
    }
    normalized
}

struct Listed {
    path: PathBuf,
    name: String,
    is_dir: bool,
}

fn list_dir(dir: &Path, show_hidden: bool) -> Option<Vec<Listed>> {
    let mut listed: Vec<Listed> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if name.chars().any(char::is_control)
                || (!show_hidden && crate::app::files_tree::is_hidden_entry(&entry))
            {
                return None;
            }
            let path = entry.path();
            let is_dir = match entry.file_type() {
                Ok(kind) if kind.is_symlink() => path.is_dir(),
                Ok(kind) => kind.is_dir(),
                Err(_) => false,
            };
            Some(Listed { path, name, is_dir })
        })
        .collect();
    listed.sort_by_cached_key(|entry| (!entry.is_dir, entry.name.to_lowercase()));
    Some(listed)
}

struct RecentEntry {
    path: PathBuf,
    relative: String,
    is_dir: bool,
}

fn existing_recents(base: &Path, recent: &[PathBuf]) -> Vec<RecentEntry> {
    recent
        .iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(base).ok()?.to_str()?;
            if relative.is_empty() {
                return None;
            }
            let metadata = std::fs::metadata(path).ok()?;
            Some(RecentEntry {
                path: path.clone(),
                relative: relative.to_owned(),
                is_dir: metadata.is_dir(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree {
        dir: tempfile::TempDir,
    }

    impl Tree {
        fn new(dirs: &[&str], files: &[&str]) -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            for relative in dirs {
                std::fs::create_dir_all(dir.path().join(relative)).expect("dir");
            }
            for relative in files {
                let path = dir.path().join(relative);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).expect("parent");
                }
                std::fs::write(path, b"").expect("file");
            }
            Self { dir }
        }

        fn root(&self) -> PathBuf {
            self.dir.path().to_path_buf()
        }

        fn request(&self, query: &str) -> Request {
            Request {
                query: query.to_string(),
                base: Some(self.root()),
                home: None,
                wsl: None,
                index: Some(build_index(&self.root(), &AtomicBool::new(false)).into()),
            }
        }
    }

    fn native(relative: &str) -> String {
        relative.replace('/', std::path::MAIN_SEPARATOR_STR)
    }

    fn labels(listing: &Listing) -> Vec<String> {
        match listing {
            Listing::Rows(rows) => rows.iter().map(|row| row.label.clone()).collect(),
            Listing::Status(status) => panic!("expected rows, got {status:?}"),
        }
    }

    fn rows(listing: Listing) -> Vec<Row> {
        match listing {
            Listing::Rows(rows) => rows,
            Listing::Status(status) => panic!("expected rows, got {status:?}"),
        }
    }

    #[test]
    fn the_query_picks_browse_search_or_navigation() {
        assert_eq!(parse(""), Query::Browse);
        assert_eq!(parse("lic"), Query::Search("lic"));
        assert_eq!(
            parse("src/"),
            Query::Navigate {
                typed_dir: "src/",
                fragment: ""
            }
        );
        assert_eq!(
            parse("~/Ap"),
            Query::Navigate {
                typed_dir: "~/",
                fragment: "Ap"
            }
        );
        assert_eq!(
            parse("~"),
            Query::Navigate {
                typed_dir: "~",
                fragment: ""
            }
        );
        assert_eq!(
            parse("../x"),
            Query::Navigate {
                typed_dir: "../",
                fragment: "x"
            }
        );
    }

    #[test]
    fn a_typed_folder_resolves_against_the_base_the_home_or_itself() {
        let tree = Tree::new(&["src", "lib"], &[]);
        let root = tree.root();
        assert_eq!(
            resolve_dir("src/../lib/", Some(&root), None, None),
            Some(root.join("lib"))
        );
        assert_eq!(
            resolve_dir("./", Some(&root), None, None),
            Some(root.clone())
        );
        assert_eq!(
            resolve_dir("~/src/", None, Some(&root), None),
            Some(root.join("src"))
        );
        assert_eq!(
            resolve_dir("~", None, Some(&root), None),
            Some(root.clone())
        );
        let absolute = format!("{}{MAIN_SEPARATOR}", root.join("lib").display());
        assert_eq!(
            resolve_dir(&absolute, Some(Path::new("unused")), None, None),
            Some(root.join("lib"))
        );
        assert_eq!(resolve_dir("src/", None, None, None), None);
    }

    #[test]
    fn browsing_lists_folders_first_and_hides_dotfiles() {
        let tree = Tree::new(&["src", "Apps"], &["README.md", "build.rs", ".env"]);
        let listing = compute(&tree.request(""), &[]);
        assert_eq!(labels(&listing), ["Apps", "src", "build.rs", "README.md"]);
    }

    #[test]
    fn browsing_puts_recent_paths_first_without_listing_them_twice() {
        let tree = Tree::new(&["src"], &["README.md", "src/main.rs"]);
        let recent = [
            tree.root().join("src").join("main.rs"),
            tree.root().join("README.md"),
        ];
        let rows = rows(compute(&tree.request(""), &recent));
        let shown: Vec<(&str, bool)> = rows
            .iter()
            .map(|row| (row.label.as_str(), row.recent))
            .collect();
        assert_eq!(
            shown,
            [
                (native("src/main.rs").as_str(), true),
                ("README.md", true),
                ("src", false)
            ]
        );
    }

    #[test]
    fn recent_paths_outside_the_base_or_gone_are_skipped() {
        let tree = Tree::new(&[], &["a.txt"]);
        let elsewhere = Tree::new(&[], &["b.txt"]);
        let recent = [
            elsewhere.root().join("b.txt"),
            tree.root().join("missing.txt"),
        ];
        let rows = rows(compute(&tree.request(""), &recent));
        assert!(rows.iter().all(|row| !row.recent));
    }

    #[test]
    fn an_empty_or_missing_base_reports_a_status() {
        let tree = Tree::new(&[], &[]);
        assert_eq!(
            compute(&tree.request(""), &[]),
            Listing::Status(Status::EmptyFolder)
        );
        let mut missing = tree.request("");
        missing.base = Some(tree.root().join("gone"));
        assert_eq!(
            compute(&missing, &[]),
            Listing::Status(Status::FolderNotFound)
        );
        missing.base = None;
        assert_eq!(
            compute(&missing, &[]),
            Listing::Status(Status::NoWorkingDirectory)
        );
    }

    #[test]
    fn searching_ranks_file_name_matches_before_directory_matches() {
        let tree = Tree::new(
            &[
                "apps/license-lookup-app/src/types",
                "apps/license-lookup-app/tests",
            ],
            &["LICENSE", "apps/license-lookup-app/src/types/License.ts"],
        );
        let listing = compute(&tree.request("licens"), &[]);
        assert_eq!(
            labels(&listing),
            [
                "LICENSE".to_string(),
                native("apps/license-lookup-app"),
                native("apps/license-lookup-app/src/types/License.ts"),
                native("apps/license-lookup-app/src"),
                native("apps/license-lookup-app/tests"),
                native("apps/license-lookup-app/src/types"),
            ]
        );
    }

    #[test]
    fn searching_boosts_and_marks_recent_paths() {
        let tree = Tree::new(&[], &["alpha.txt", "alpine.txt"]);
        let recent = [tree.root().join("alpine.txt")];
        let rows = rows(compute(&tree.request("alp"), &recent));
        assert_eq!(rows[0].label, "alpine.txt");
        assert!(rows[0].recent);
        assert!(!rows[1].recent);
    }

    #[test]
    fn searching_waits_for_the_index() {
        let tree = Tree::new(&[], &["a.txt"]);
        let mut request = tree.request("a");
        request.index = None;
        assert_eq!(compute(&request, &[]), Listing::Status(Status::Indexing));
        request.index = Some(Vec::new().into());
        assert_eq!(compute(&request, &[]), Listing::Status(Status::NoMatch));
    }

    #[test]
    fn navigating_filters_one_folder_and_keeps_the_typed_prefix() {
        let tree = Tree::new(&["src/app"], &["src/main.rs", "src/mod.rs", "src/.hidden"]);
        let rows = rows(compute(&tree.request("src/ma"), &[]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "src/main.rs");
        assert_eq!(rows[0].highlights, vec![4..6]);
        assert_eq!(rows[0].path, tree.root().join("src").join("main.rs"));
        assert_eq!(rows[0].completion, "src/main.rs");
    }

    #[test]
    fn navigating_completes_folders_with_the_typed_separator() {
        let tree = Tree::new(&["src/app"], &["src/main.rs"]);
        let rows = rows(compute(&tree.request("src/"), &[]));
        assert_eq!(rows[0].label, "src/app");
        assert_eq!(rows[0].completion, "src/app/");
    }

    #[test]
    fn navigating_shows_dotfiles_only_for_a_dot_fragment() {
        let tree = Tree::new(&[], &["src/.hidden", "src/visible"]);
        assert_eq!(
            labels(&compute(&tree.request("src/"), &[])),
            ["src/visible"]
        );
        assert_eq!(
            labels(&compute(&tree.request("src/.h"), &[])),
            ["src/.hidden"]
        );
    }

    #[test]
    fn navigating_the_home_folder_uses_a_tilde_label() {
        let tree = Tree::new(&["Apps"], &[]);
        let mut request = tree.request("~");
        request.home = Some(tree.root());
        let rows = rows(compute(&request, &[]));
        assert_eq!(rows[0].label, format!("~{MAIN_SEPARATOR}Apps"));
        assert_eq!(rows[0].path, tree.root().join("Apps"));
    }

    #[test]
    fn navigating_into_a_missing_folder_reports_it() {
        let tree = Tree::new(&[], &[]);
        assert_eq!(
            compute(&tree.request("nope/"), &[]),
            Listing::Status(Status::FolderNotFound)
        );
        let tree = Tree::new(&["src"], &["src/a.rs"]);
        assert_eq!(
            compute(&tree.request("src/zz"), &[]),
            Listing::Status(Status::NoMatch)
        );
    }

    #[test]
    fn the_index_skips_hidden_and_ignored_entries() {
        let tree = Tree::new(
            &["src", "target/debug"],
            &["src/main.rs", ".env", ".ignore"],
        );
        std::fs::write(tree.root().join(".ignore"), "target/\n").expect("ignore file");
        let mut relatives: Vec<String> = build_index(&tree.root(), &AtomicBool::new(false))
            .into_iter()
            .map(|entry| entry.relative)
            .collect();
        relatives.sort();
        assert_eq!(relatives, ["src".to_string(), native("src/main.rs")]);
    }

    #[test]
    fn a_cancelled_index_stops_before_walking() {
        let tree = Tree::new(&["src"], &["src/main.rs"]);
        assert!(build_index(&tree.root(), &AtomicBool::new(true)).is_empty());
    }

    #[test]
    fn a_path_inside_the_base_is_inserted_relative_and_quoted_for_the_shell() {
        let base = Tree::new(&[], &[]).root();
        let path = base.join("src").join("main.rs");
        let expected = if cfg!(windows) {
            r"src\main.rs"
        } else {
            "src/main.rs"
        };
        assert_eq!(
            insertion_text(&path, Some(&base), ShellQuoting::PowerShell, None).as_deref(),
            Some(expected)
        );
        let spaced = base.join("my notes.md");
        assert_eq!(
            insertion_text(&spaced, Some(&base), ShellQuoting::Posix, None).as_deref(),
            Some("'my notes.md'")
        );
        assert_eq!(
            insertion_text(&spaced, Some(&base), ShellQuoting::Cmd, None).as_deref(),
            Some("\"my notes.md\"")
        );
    }

    #[test]
    fn a_path_outside_the_base_is_inserted_absolute() {
        let base = Tree::new(&[], &[]).root();
        let elsewhere = Tree::new(&[], &["x.txt"]).root().join("x.txt");
        let inserted =
            insertion_text(&elsewhere, Some(&base), ShellQuoting::PowerShell, None).expect("text");
        assert!(Path::new(inserted.trim_matches('\'')).is_absolute());
        assert!(inserted.trim_matches('\'').ends_with("x.txt"));
    }

    #[test]
    fn a_relative_path_starting_with_a_dash_cannot_read_as_an_option() {
        let base = Tree::new(&[], &[]).root();
        assert_eq!(
            insertion_text(
                &base.join("-rf"),
                Some(&base),
                ShellQuoting::PowerShell,
                None
            )
            .as_deref(),
            Some(format!(".{MAIN_SEPARATOR}-rf").as_str())
        );
    }

    #[test]
    fn a_path_with_a_control_character_is_never_inserted() {
        let base = Tree::new(&[], &[]).root();
        assert_eq!(
            insertion_text(
                &base.join("evil\u{1b}[31m"),
                Some(&base),
                ShellQuoting::Posix,
                None
            ),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_posix_shell_on_windows_gets_forward_slashes() {
        let base = Path::new(r"C:\work");
        assert_eq!(
            insertion_text(
                Path::new(r"C:\work\src\main.rs"),
                Some(base),
                ShellQuoting::Posix,
                None
            )
            .as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            insertion_text(
                Path::new(r"D:\data\x.txt"),
                Some(base),
                ShellQuoting::Posix,
                None
            )
            .as_deref(),
            Some("D:/data/x.txt")
        );
    }

    #[test]
    fn the_placeholder_names_the_base_folder() {
        let home = Tree::new(&["Apps/web"], &[]).root();
        assert_eq!(
            placeholder(Some(&home.join("Apps").join("web")), Some(&home), None),
            format!("~{MAIN_SEPARATOR}Apps{MAIN_SEPARATOR}web{MAIN_SEPARATOR}")
        );
        assert_eq!(
            placeholder(Some(&home), Some(&home), None),
            format!("~{MAIN_SEPARATOR}")
        );
        assert_eq!(placeholder(None, Some(&home), None), "Type a path…");
    }

    #[cfg(windows)]
    fn wsl_roots() -> WslRoots {
        WslRoots::parse("\\\\wsl.localhost\\Ubuntu\\\n/home/me\n/mnt/c/\n").expect("probe output")
    }

    #[cfg(windows)]
    #[test]
    fn a_wsl_shell_gets_linux_paths_outside_the_base() {
        let wsl = wsl_roots();
        let base = Path::new(r"\\wsl.localhost\Ubuntu\home\me\app");
        let insert = |path: &Path| insertion_text(path, Some(base), ShellQuoting::Wsl, Some(&wsl));
        assert_eq!(
            insert(&base.join("src").join("main.rs")).as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            insert(Path::new(r"\\wsl.localhost\Ubuntu\etc\hosts")).as_deref(),
            Some("/etc/hosts")
        );
        assert_eq!(
            insert(Path::new(r"C:\Users\me\my notes.md")).as_deref(),
            Some("'/mnt/c/Users/me/my notes.md'")
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_wsl_shell_resolves_typed_linux_paths_and_names_its_base_in_linux_form() {
        let wsl = wsl_roots();
        let base = Path::new(r"C:\dev\app");
        assert_eq!(
            resolve_dir("/etc/", Some(base), None, Some(&wsl)),
            Some(PathBuf::from(r"\\wsl.localhost\Ubuntu\etc"))
        );
        assert_eq!(
            resolve_dir("/mnt/c/dev/../tmp/", Some(base), None, Some(&wsl)),
            Some(PathBuf::from(r"C:\tmp"))
        );
        assert_eq!(
            resolve_dir("src/", Some(base), None, Some(&wsl)),
            Some(PathBuf::from(r"C:\dev\app\src"))
        );
        assert_eq!(placeholder(Some(base), None, Some(&wsl)), "/mnt/c/dev/app/");
        let home = wsl.home();
        assert_eq!(
            placeholder(Some(&home.join("app")), Some(&home), Some(&wsl)),
            "~/app/"
        );
    }
}
