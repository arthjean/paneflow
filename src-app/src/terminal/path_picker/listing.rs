use std::cmp::Reverse;
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf, is_separator};
use std::sync::Arc;
use std::sync::atomic::{self, AtomicU64};

use super::fuzzy::{self, Pattern};
use super::index::{self, Index, Located};
use super::search::{self, LOCAL_BONUS, RECENT_BONUS, RecentTarget, Source};
use super::wsl::WslRoots;
use crate::terminal::types::ShellQuoting;

const MAX_RESULTS: usize = 200;
const MAX_RECENT_BROWSED: usize = 5;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Recent {
    pub(super) path: PathBuf,
    pub(super) is_dir: bool,
}

pub(super) struct Request {
    pub(super) query: String,
    pub(super) base: Option<PathBuf>,
    pub(super) home: Option<PathBuf>,
    pub(super) wsl: Option<Arc<WslRoots>>,
    pub(super) local: Option<Arc<Index>>,
    pub(super) globals: Vec<Arc<Index>>,
    pub(super) visited: Vec<Arc<Index>>,
    pub(super) recents: Arc<[Recent]>,
    pub(super) pending: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Outcome {
    pub(super) listing: Listing,
    pub(super) walk: Option<PathBuf>,
}

pub(super) struct Cancel {
    generation: Arc<AtomicU64>,
    mine: u64,
}

impl Cancel {
    pub(super) fn new(generation: Arc<AtomicU64>, mine: u64) -> Self {
        Self { generation, mine }
    }

    #[cfg(test)]
    pub(super) fn never() -> Self {
        Self::new(Arc::new(AtomicU64::new(0)), 0)
    }

    fn is_cancelled(&self) -> bool {
        self.generation.load(atomic::Ordering::Relaxed) != self.mine
    }
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

pub(super) fn compute(request: &Request, cancel: &Cancel) -> Option<Outcome> {
    match parse(&request.query) {
        Query::Browse => Some(settled(browse(request))),
        Query::Search(text) => search(request, text, cancel).map(settled),
        Query::Navigate {
            typed_dir,
            fragment,
        } => navigate(request, typed_dir, fragment, cancel),
    }
}

pub(super) fn existing_recents(paths: Vec<PathBuf>) -> Vec<Recent> {
    paths
        .into_iter()
        .filter_map(|path| {
            let is_dir = std::fs::metadata(&path).ok()?.is_dir();
            Some(Recent { path, is_dir })
        })
        .collect()
}

fn settled(listing: Listing) -> Outcome {
    Outcome {
        listing,
        walk: None,
    }
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

fn browse(request: &Request) -> Listing {
    let mut shown = HashSet::new();
    let mut rows = Vec::new();
    for recent in request.recents.iter().take(MAX_RECENT_BROWSED) {
        let Some(display) = display(&recent.path, request) else {
            continue;
        };
        shown.insert(recent.path.clone());
        rows.push(Row {
            completion: completion(&display.label, recent.is_dir, display.separator),
            label: display.label,
            path: recent.path.clone(),
            highlights: Vec::new(),
            is_dir: recent.is_dir,
            recent: true,
        });
    }
    let Some(base) = request.base.as_deref() else {
        return if rows.is_empty() {
            Listing::Status(Status::NoWorkingDirectory)
        } else {
            Listing::Rows(rows)
        };
    };
    let Some(listed) = list_dir(base, false) else {
        return Listing::Status(Status::FolderNotFound);
    };
    for entry in listed {
        if shown.contains(&entry.path) {
            continue;
        }
        rows.push(Row {
            recent: is_recent(request, &entry.path),
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

fn search(request: &Request, text: &str, cancel: &Cancel) -> Option<Listing> {
    let pattern = Pattern::new(text);
    let base = request.base.as_deref();
    let covered = base.is_some_and(|base| {
        request
            .globals
            .iter()
            .any(|global| global.locate(base).is_some())
    });
    let local = request
        .local
        .as_deref()
        .filter(|local| local.complete() || !covered);
    let mut sources = Vec::new();
    if let Some(local) = local {
        let skip = request
            .globals
            .iter()
            .filter_map(|global| match local.locate(global.root())? {
                Located::Root => None,
                at => Some(local.subtree(at)),
            })
            .collect();
        sources.push(search_source(
            local,
            skip,
            Some(local.subtree(Located::Root)),
            &request.recents,
        ));
    }
    for global in &request.globals {
        let at_base = base.and_then(|base| global.locate(base));
        let (skip, local_range) = match (at_base, local.is_some()) {
            (Some(Located::Root), true) => continue,
            (Some(at), true) => (vec![global.subtree(at)], None),
            (Some(at), false) => (Vec::new(), Some(global.subtree(at))),
            (None, _) => (Vec::new(), None),
        };
        sources.push(search_source(global, skip, local_range, &request.recents));
    }
    let scan = search::scan(&sources, &pattern, MAX_RESULTS, &|| cancel.is_cancelled())?;
    let mut ranked: Vec<(i32, usize, Row)> = scan
        .hits
        .iter()
        .filter_map(|hit| {
            let source = &sources[hit.source as usize];
            let path = source.index.path_of(hit.node);
            let display = display(&path, request)?;
            let is_dir = source.index.is_dir(hit.node);
            Some((
                hit.score,
                hit.len as usize,
                Row {
                    highlights: highlights(&display, &pattern),
                    completion: completion(&display.label, is_dir, display.separator),
                    label: display.label,
                    path,
                    is_dir,
                    recent: hit.recent,
                },
            ))
        })
        .collect();
    for recent in request.recents.iter().filter(|recent| {
        !sources
            .iter()
            .any(|source| index::strip_root(&recent.path, source.index.root()).is_some())
    }) {
        let Some(display) = display(&recent.path, request) else {
            continue;
        };
        let Some(found) = fuzzy::match_path(&display.label[display.tail..], &pattern) else {
            continue;
        };
        let local = base.is_some_and(|base| is_inside(&recent.path, base));
        ranked.push((
            found.score + RECENT_BONUS + if local { LOCAL_BONUS } else { 0 },
            display.label.len() - display.tail,
            Row {
                highlights: shifted(found.highlights, display.tail),
                completion: completion(&display.label, recent.is_dir, display.separator),
                label: display.label,
                path: recent.path.clone(),
                is_dir: recent.is_dir,
                recent: true,
            },
        ));
    }
    if ranked.is_empty() {
        return Some(Listing::Status(if request.pending {
            Status::Indexing
        } else if sources.is_empty() && base.is_none() {
            Status::NoWorkingDirectory
        } else {
            Status::NoMatch
        }));
    }
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.label.cmp(&right.2.label))
    });
    ranked.truncate(MAX_RESULTS);
    Some(Listing::Rows(
        ranked.into_iter().map(|(_, _, row)| row).collect(),
    ))
}

fn search_source<'a>(
    index: &'a Index,
    skip: Vec<Range<u32>>,
    local: Option<Range<u32>>,
    recents: &[Recent],
) -> Source<'a> {
    Source {
        index,
        range: index.subtree(Located::Root),
        skip,
        from: Located::Root,
        min_depth: 1,
        local,
        show_hidden: true,
        recents: recent_targets(index, recents),
    }
}

fn recent_targets(index: &Index, recents: &[Recent]) -> Vec<RecentTarget> {
    recents
        .iter()
        .filter_map(|entry| {
            let relative = index::strip_root(&entry.path, index.root())?.to_str()?;
            let name = entry.path.file_name()?.to_str()?;
            (!relative.is_empty()).then(|| RecentTarget {
                name: name.to_owned(),
                relative: relative.to_owned(),
            })
        })
        .collect()
}

fn navigate(
    request: &Request,
    typed_dir: &str,
    fragment: &str,
    cancel: &Cancel,
) -> Option<Outcome> {
    let Some(dir) = resolve_dir(
        typed_dir,
        request.base.as_deref(),
        request.home.as_deref(),
        request.wsl.as_deref(),
    ) else {
        return Some(settled(Listing::Status(Status::NoWorkingDirectory)));
    };
    let show_hidden = fragment.starts_with('.');
    let Some(listed) = list_dir(&dir, show_hidden) else {
        return Some(settled(Listing::Status(Status::FolderNotFound)));
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
    let has_entries = !listed.is_empty();
    let mut ranked: Vec<(i32, Row)> = listed
        .into_iter()
        .filter_map(|entry| {
            let found = fuzzy::match_path(&entry.name, &pattern)?;
            let label = format!("{prefix}{}", entry.name);
            Some((
                found.score,
                Row {
                    recent: is_recent(request, &entry.path),
                    completion: completion(&label, entry.is_dir, separator),
                    highlights: shifted(found.highlights, prefix.len()),
                    label,
                    path: entry.path,
                    is_dir: entry.is_dir,
                },
            ))
        })
        .collect();
    let mut walk = None;
    if !pattern.is_empty() {
        match indexed_subtree(request, &dir, has_entries) {
            Some((index, at)) => {
                let source = Source {
                    index,
                    range: index.subtree(at),
                    skip: Vec::new(),
                    from: at,
                    min_depth: match at {
                        Located::Root => 2,
                        Located::Node(node) => index.depth(node) + 2,
                    },
                    local: None,
                    show_hidden,
                    recents: Vec::new(),
                };
                let scan =
                    search::scan(&[source], &pattern, MAX_RESULTS, &|| cancel.is_cancelled())?;
                let mut text = String::new();
                let mut chain = Vec::new();
                for hit in scan.hits {
                    index.write_path(hit.node, at, &mut text, &mut chain);
                    if separator != MAIN_SEPARATOR {
                        text = text.replace(MAIN_SEPARATOR, &separator.to_string());
                    }
                    let path = index.path_of(hit.node);
                    let is_dir = index.is_dir(hit.node);
                    let label = format!("{prefix}{text}");
                    let highlights = fuzzy::match_path(&text, &pattern)
                        .map(|found| shifted(found.highlights, prefix.len()))
                        .unwrap_or_default();
                    ranked.push((
                        hit.score,
                        Row {
                            recent: is_recent(request, &path),
                            completion: completion(&label, is_dir, separator),
                            label,
                            highlights,
                            path,
                            is_dir,
                        },
                    ));
                }
            }
            None => walk = Some(dir.clone()),
        }
    }
    ranked.sort_by_key(|(score, _)| Reverse(*score));
    ranked.truncate(MAX_RESULTS);
    let listing = if ranked.is_empty() {
        Listing::Status(if walk.is_some() {
            Status::Indexing
        } else if pattern.is_empty() {
            Status::EmptyFolder
        } else {
            Status::NoMatch
        })
    } else {
        Listing::Rows(ranked.into_iter().map(|(_, row)| row).collect())
    };
    Some(Outcome { listing, walk })
}

fn indexed_subtree<'a>(
    request: &'a Request,
    dir: &Path,
    has_entries: bool,
) -> Option<(&'a Index, Located)> {
    request
        .local
        .iter()
        .chain(&request.visited)
        .chain(&request.globals)
        .find_map(|index| {
            let at = index.locate(dir)?;
            let descended = at == Located::Root || !index.subtree(at).is_empty() || !has_entries;
            descended.then_some((index.as_ref(), at))
        })
}

struct Display {
    label: String,
    tail: usize,
    separator: char,
}

fn display(path: &Path, request: &Request) -> Option<Display> {
    if let Some(relative) = request
        .base
        .as_deref()
        .and_then(|base| index::strip_root(path, base))
        .filter(|relative| !relative.as_os_str().is_empty())
    {
        return Some(Display {
            label: relative.to_str()?.to_owned(),
            tail: 0,
            separator: MAIN_SEPARATOR,
        });
    }
    if let Some(shown) = request.wsl.as_deref().and_then(|wsl| wsl.display(path)) {
        let tail = if shown.starts_with("~/") { 2 } else { 0 };
        return Some(Display {
            label: shown,
            tail,
            separator: '/',
        });
    }
    if let Some(relative) = request
        .home
        .as_deref()
        .and_then(|home| index::strip_root(path, home))
        .filter(|relative| !relative.as_os_str().is_empty())
    {
        return Some(Display {
            label: format!("~{MAIN_SEPARATOR}{}", relative.to_str()?),
            tail: 1 + MAIN_SEPARATOR.len_utf8(),
            separator: MAIN_SEPARATOR,
        });
    }
    Some(Display {
        label: path.to_str()?.to_owned(),
        tail: 0,
        separator: MAIN_SEPARATOR,
    })
}

fn highlights(display: &Display, pattern: &Pattern) -> Vec<Range<usize>> {
    fuzzy::match_path(&display.label[display.tail..], pattern)
        .map(|found| shifted(found.highlights, display.tail))
        .unwrap_or_default()
}

fn shifted(highlights: Vec<Range<usize>>, by: usize) -> Vec<Range<usize>> {
    highlights
        .into_iter()
        .map(|range| range.start + by..range.end + by)
        .collect()
}

fn is_inside(path: &Path, base: &Path) -> bool {
    index::strip_root(path, base).is_some_and(|relative| !relative.as_os_str().is_empty())
}

fn is_recent(request: &Request, path: &Path) -> bool {
    request.recents.iter().any(|recent| recent.path == path)
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::SystemTime;

    use super::*;
    use crate::terminal::path_picker::index::{Builder, LOCAL_CAP};

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

        fn index(&self) -> Arc<Index> {
            Arc::new(
                index::walk(&self.root(), LOCAL_CAP, &[], &AtomicBool::new(false)).expect("walk"),
            )
        }

        fn request(&self, query: &str) -> Request {
            Request {
                query: query.to_string(),
                base: Some(self.root()),
                home: None,
                wsl: None,
                local: Some(self.index()),
                globals: Vec::new(),
                visited: Vec::new(),
                recents: Arc::from([]),
                pending: false,
            }
        }
    }

    fn outcome(request: &Request) -> Outcome {
        compute(request, &Cancel::never()).expect("never cancelled")
    }

    fn listing(request: &Request) -> Listing {
        outcome(request).listing
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

    fn recents(paths: &[PathBuf]) -> Arc<[Recent]> {
        existing_recents(paths.to_vec()).into()
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
        assert_eq!(
            labels(&listing(&tree.request(""))),
            ["Apps", "src", "build.rs", "README.md"]
        );
    }

    #[test]
    fn browsing_puts_recent_paths_first_without_listing_them_twice() {
        let tree = Tree::new(&["src"], &["README.md", "src/main.rs"]);
        let mut request = tree.request("");
        request.recents = recents(&[
            tree.root().join("src").join("main.rs"),
            tree.root().join("README.md"),
        ]);
        let rows = rows(listing(&request));
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
    fn browsing_lists_recent_paths_from_anywhere_and_drops_gone_ones() {
        let tree = Tree::new(&[], &["a.txt"]);
        let elsewhere = Tree::new(&["notes"], &["notes/b.txt"]);
        let mut request = tree.request("");
        request.home = Some(elsewhere.root());
        request.recents = recents(&[
            elsewhere.root().join("notes").join("b.txt"),
            tree.root().join("missing.txt"),
        ]);
        let rows = rows(listing(&request));
        assert_eq!(rows[0].label, native("~/notes/b.txt"));
        assert!(rows[0].recent);
        assert_eq!(rows[0].path, elsewhere.root().join("notes").join("b.txt"));
        assert_eq!(rows[1].label, "a.txt");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn an_empty_or_missing_base_reports_a_status() {
        let tree = Tree::new(&[], &[]);
        assert_eq!(
            listing(&tree.request("")),
            Listing::Status(Status::EmptyFolder)
        );
        let mut missing = tree.request("");
        missing.base = Some(tree.root().join("gone"));
        assert_eq!(listing(&missing), Listing::Status(Status::FolderNotFound));
        missing.base = None;
        assert_eq!(
            listing(&missing),
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
        assert_eq!(
            labels(&listing(&tree.request("licens"))),
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
        let mut request = tree.request("alp");
        request.recents = recents(&[tree.root().join("alpine.txt")]);
        let rows = rows(listing(&request));
        assert_eq!(rows[0].label, "alpine.txt");
        assert!(rows[0].recent);
        assert!(!rows[1].recent);
    }

    #[test]
    fn searching_waits_for_the_index() {
        let tree = Tree::new(&[], &["a.txt"]);
        let mut request = tree.request("a");
        request.local = None;
        request.pending = true;
        assert_eq!(listing(&request), Listing::Status(Status::Indexing));
        request.local = Some(Arc::new(
            Builder::new(tree.root(), LOCAL_CAP).finish(SystemTime::now()),
        ));
        request.pending = false;
        assert_eq!(listing(&request), Listing::Status(Status::NoMatch));
    }

    #[test]
    fn searching_finds_hidden_files_but_not_the_inside_of_hidden_folders() {
        let tree = Tree::new(
            &[".github/workflows"],
            &[".envrc", ".github/workflows/ci.yml"],
        );
        assert_eq!(labels(&listing(&tree.request("envrc"))), [".envrc"]);
        assert_eq!(labels(&listing(&tree.request("github"))), [".github"]);
        assert_eq!(
            listing(&tree.request("ciyml")),
            Listing::Status(Status::NoMatch)
        );
    }

    #[test]
    fn searching_reaches_a_global_root_outside_the_working_directory() {
        let tree = Tree::new(&[], &["main.rs"]);
        let home = Tree::new(&["notes"], &["notes/todo.md"]);
        let mut request = tree.request("todo");
        request.home = Some(home.root());
        request.globals = vec![home.index()];
        let rows = rows(listing(&request));
        assert_eq!(rows[0].label, native("~/notes/todo.md"));
        assert_eq!(rows[0].path, home.root().join("notes").join("todo.md"));
        let tail = native("~/").len();
        assert_eq!(rows[0].highlights, vec![tail + 6..tail + 10]);
        assert_eq!(rows[0].completion, native("~/notes/todo.md"));
    }

    #[test]
    fn a_working_directory_match_outranks_the_same_name_elsewhere() {
        let tree = Tree::new(&[], &["config.toml"]);
        let elsewhere = Tree::new(&[], &["config.toml"]);
        let mut request = tree.request("config");
        request.globals = vec![elsewhere.index()];
        let rows = rows(listing(&request));
        assert_eq!(rows[0].label, "config.toml");
        assert_eq!(rows[1].path, elsewhere.root().join("config.toml"));
    }

    #[test]
    fn a_working_directory_inside_a_global_root_is_listed_once() {
        let outer = Tree::new(&["inner", "other"], &["inner/x.rs", "other/x.rs"]);
        let inner = outer.root().join("inner");
        let mut request = outer.request("x.rs");
        request.base = Some(inner.clone());
        request.local = Some(Arc::new(
            index::walk(&inner, LOCAL_CAP, &[], &AtomicBool::new(false)).expect("walk"),
        ));
        request.globals = vec![outer.index()];
        let rows = rows(listing(&request));
        let paths: Vec<&Path> = rows.iter().map(|row| row.path.as_path()).collect();
        assert_eq!(
            paths,
            [
                inner.join("x.rs").as_path(),
                outer.root().join("other").join("x.rs").as_path()
            ]
        );
        assert_eq!(rows[0].label, "x.rs");
    }

    #[test]
    fn a_truncated_working_directory_index_defers_to_the_global_root() {
        let outer = Tree::new(&["inner"], &["inner/a.rs", "inner/b.rs"]);
        let inner = outer.root().join("inner");
        let mut request = outer.request("rs");
        request.base = Some(inner.clone());
        request.local = Some(Arc::new(
            index::walk(&inner, 1, &[], &AtomicBool::new(false)).expect("walk"),
        ));
        request.globals = vec![outer.index()];
        assert_eq!(labels(&listing(&request)), ["a.rs", "b.rs"]);
    }

    #[test]
    fn a_global_root_inside_the_working_directory_is_listed_once() {
        let outer = Tree::new(&["projects/app"], &["projects/app/lib.rs"]);
        let projects = outer.root().join("projects");
        let mut request = outer.request("lib");
        request.globals = vec![Arc::new(
            index::walk(&projects, LOCAL_CAP, &[], &AtomicBool::new(false)).expect("walk"),
        )];
        assert_eq!(labels(&listing(&request)), [native("projects/app/lib.rs")]);
    }

    #[test]
    fn a_recent_path_outside_every_index_still_matches() {
        let tree = Tree::new(&[], &["a.txt"]);
        let elsewhere = Tree::new(&[], &["report.pdf"]);
        let mut request = tree.request("report");
        request.recents = recents(&[elsewhere.root().join("report.pdf")]);
        let rows = rows(listing(&request));
        assert_eq!(rows[0].path, elsewhere.root().join("report.pdf"));
        assert!(rows[0].recent);
    }

    #[test]
    fn navigating_filters_one_folder_and_keeps_the_typed_prefix() {
        let tree = Tree::new(&["src/app"], &["src/main.rs", "src/mod.rs", "src/.hidden"]);
        let rows = rows(listing(&tree.request("src/ma")));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "src/main.rs");
        assert_eq!(rows[0].highlights, vec![4..6]);
        assert_eq!(rows[0].path, tree.root().join("src").join("main.rs"));
        assert_eq!(rows[0].completion, "src/main.rs");
    }

    #[test]
    fn navigating_with_a_fragment_searches_the_whole_folder_below() {
        let tree = Tree::new(
            &["src/deep/er", "src/.cache"],
            &[
                "src/deep/er/target.rs",
                "src/top.rs",
                "src/.cache/target.bin",
            ],
        );
        let rows = rows(listing(&tree.request("src/targ")));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "src/deep/er/target.rs");
        assert_eq!(
            rows[0].path,
            tree.root()
                .join("src")
                .join("deep")
                .join("er")
                .join("target.rs")
        );
        let start = "src/deep/er/".len();
        assert_eq!(rows[0].highlights, vec![start..start + 4]);
    }

    #[test]
    fn navigating_lists_direct_entries_before_deeper_ties() {
        let tree = Tree::new(&["src/a"], &["src/a/note.md", "src/note.md"]);
        assert_eq!(
            labels(&listing(&tree.request("src/note"))),
            ["src/note.md", "src/a/note.md"]
        );
    }

    #[test]
    fn navigating_outside_every_index_asks_for_a_walk() {
        let tree = Tree::new(&["far/nested"], &["far/nested/deep.txt", "far/deer.txt"]);
        let mut request = tree.request("far/dee");
        request.local = None;
        let first = outcome(&request);
        assert_eq!(first.walk, Some(tree.root().join("far")));
        assert_eq!(labels(&first.listing), ["far/deer.txt"]);
        request.visited = vec![Arc::new(
            index::walk(
                &tree.root().join("far"),
                LOCAL_CAP,
                &[],
                &AtomicBool::new(false),
            )
            .expect("walk"),
        )];
        let second = outcome(&request);
        assert_eq!(second.walk, None);
        assert_eq!(
            labels(&second.listing),
            ["far/deer.txt", "far/nested/deep.txt"]
        );
    }

    #[test]
    fn navigating_into_an_unindexed_hidden_folder_asks_for_a_walk() {
        let tree = Tree::new(&[".config/tool"], &[".config/tool/settings.json"]);
        let request = tree.request(".config/sett");
        let outcome = outcome(&request);
        assert_eq!(outcome.walk, Some(tree.root().join(".config")));
        assert_eq!(outcome.listing, Listing::Status(Status::Indexing));
    }

    #[test]
    fn navigating_completes_folders_with_the_typed_separator() {
        let tree = Tree::new(&["src/app"], &["src/main.rs"]);
        let rows = rows(listing(&tree.request("src/")));
        assert_eq!(rows[0].label, "src/app");
        assert_eq!(rows[0].completion, "src/app/");
    }

    #[test]
    fn navigating_shows_dotfiles_only_for_a_dot_fragment() {
        let tree = Tree::new(&[], &["src/.hidden", "src/visible"]);
        assert_eq!(labels(&listing(&tree.request("src/"))), ["src/visible"]);
        assert_eq!(labels(&listing(&tree.request("src/.h"))), ["src/.hidden"]);
    }

    #[test]
    fn navigating_the_home_folder_uses_a_tilde_label() {
        let tree = Tree::new(&["Apps"], &[]);
        let mut request = tree.request("~");
        request.home = Some(tree.root());
        let rows = rows(listing(&request));
        assert_eq!(rows[0].label, format!("~{MAIN_SEPARATOR}Apps"));
        assert_eq!(rows[0].path, tree.root().join("Apps"));
    }

    #[test]
    fn navigating_into_a_missing_folder_reports_it() {
        let tree = Tree::new(&[], &[]);
        assert_eq!(
            listing(&tree.request("nope/")),
            Listing::Status(Status::FolderNotFound)
        );
        let tree = Tree::new(&["src"], &["src/a.rs"]);
        assert_eq!(
            listing(&tree.request("src/zz")),
            Listing::Status(Status::NoMatch)
        );
    }

    #[test]
    fn a_cancelled_computation_yields_nothing() {
        let tree = Tree::new(&[], &["a.txt"]);
        let generation = Arc::new(AtomicU64::new(1));
        let cancel = Cancel::new(generation.clone(), 0);
        assert_eq!(compute(&tree.request("a"), &cancel), None);
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
