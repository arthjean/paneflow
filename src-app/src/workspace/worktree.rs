//! Git worktree-per-agent management (EP-002, prd-orchestration-v2).
//!
//! `paneflow up` panes can declare `worktree = "branch"`: the CLI process
//! creates (or reuses) a git worktree in a SIBLING directory of the repo -
//! `<repo>.worktrees/<branch-slug>`, or `<branch-slug>-<hash>` on slug
//! collision - copies the top-level gitignored `.env*` files, optionally runs
//! a `setup` command, and the pane spawns with the worktree as its cwd. The
//! app side records ownership ([`ManagedWorktree`]) so closing the workspace
//! tears the worktree down - IF it is clean.
//!
//! Invariants (US-006/US-009):
//! - a branch is NEVER deleted, only the worktree directory;
//! - a worktree with uncommitted changes is NEVER removed;
//! - only worktrees Paneflow created (tracked in `managed_worktrees`) are
//!   ever torn down - a pre-existing worktree pointed at by `cwd` is not ours;
//! - every git invocation is a subprocess with argv (no shell interpolation)
//!   under [`paneflow_process::run_with_timeout`], and on the app side it runs
//!   off the render thread (`smol::unblock`).
//!
//! Sibling (not in-repo) placement keeps recursive file watchers - including
//! Paneflow's own diff watcher - from descending into N extra checkouts.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Wall-clock bound for plumbing git calls (list/status/remove/prune).
const GIT_DEADLINE: Duration = Duration::from_secs(10);
/// `worktree add` checks out a full tree - give it more room on big repos.
const ADD_DEADLINE: Duration = Duration::from_secs(120);
const STDOUT_CAP: u64 = 256 * 1024;
const OWNER_MARKER_FILE: &str = ".paneflow-worktree";

/// Teardown policy for a managed worktree (US-009). `Auto` removes the
/// worktree at workspace close when it has no uncommitted changes; `Keep`
/// opts out entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TeardownPolicy {
    #[default]
    Auto,
    Keep,
}

impl TeardownPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            TeardownPolicy::Auto => "auto",
            TeardownPolicy::Keep => "keep",
        }
    }
}

/// A worktree Paneflow created for a pane and therefore owns the lifecycle of.
/// Carried by `Workspace`, persisted in `session.json` (so a crash does not
/// orphan the ownership record), torn down at workspace close.
#[derive(Debug, Clone, PartialEq)]
pub struct ManagedWorktree {
    /// Worktree checkout directory (`<repo>.worktrees/<slug>`).
    pub path: PathBuf,
    /// Main repository root (where `git worktree …` commands run).
    pub repo_root: PathBuf,
    /// Branch checked out in the worktree. Recorded for diagnostics only -
    /// teardown never touches the branch.
    pub branch: String,
    pub teardown: TeardownPolicy,
}

pub fn owner_marker_path(worktree_path: &Path) -> PathBuf {
    worktree_path.join(OWNER_MARKER_FILE)
}

pub fn has_owner_marker(worktree_path: &Path) -> bool {
    owner_marker_path(worktree_path).is_file()
}

fn write_owner_marker(worktree_path: &Path, repo_root: &Path, branch: &str) -> Result<(), String> {
    let marker = owner_marker_path(worktree_path);
    let contents = format!(
        "owner=paneflow\nrepo_root={}\nbranch={}\n",
        repo_root.display(),
        branch
    );
    std::fs::write(&marker, contents)
        .map_err(|e| format!("cannot write owner marker {}: {e}", marker.display()))
}

/// Rehydrate a persisted or IPC-provided ownership record. The record is only
/// accepted when it matches Paneflow's deterministic worktree directory and the
/// on-disk worktree carries Paneflow's owner marker.
pub fn managed_worktree_from_record(
    path_raw: &str,
    repo_root_raw: &str,
    branch_raw: &str,
    teardown_raw: &str,
) -> Option<ManagedWorktree> {
    let path = PathBuf::from(path_raw);
    let repo_root = PathBuf::from(repo_root_raw);
    if !path.is_absolute() || !repo_root.is_absolute() {
        log::warn!("managed worktree: dropping record with non-absolute path");
        return None;
    }
    let branch = branch_raw.trim();
    if branch.is_empty() || branch_slug(branch).is_empty() {
        log::warn!("managed worktree: dropping record with invalid branch");
        return None;
    }
    if !is_paneflow_worktree_dir(&repo_root, branch, &path) {
        log::warn!(
            "managed worktree: dropping record outside Paneflow worktree dir: {}",
            path.display()
        );
        return None;
    }
    if !has_owner_marker(&path) {
        log::warn!(
            "managed worktree: dropping record without owner marker: {}",
            path.display()
        );
        return None;
    }
    let teardown = match teardown_raw {
        "auto" => TeardownPolicy::Auto,
        "keep" => TeardownPolicy::Keep,
        other => {
            log::warn!("managed worktree: unknown teardown policy {other:?}; keeping");
            TeardownPolicy::Keep
        }
    };
    Some(ManagedWorktree {
        path,
        repo_root,
        branch: branch.to_string(),
        teardown,
    })
}

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    /// `None` for a detached-HEAD worktree.
    pub branch: Option<String>,
}

/// Display name for a checkout: its branch, or a directory name when HEAD is
/// detached.
///
/// Detached is not an edge case - `git worktree add --detach` leaves that
/// state, the Codex app creates every one of its worktrees that way, and agent
/// tooling that lays checkouts out as `<...>/<slug>/<repo>` produces a
/// directory whose own name is just the repository's, identical for every
/// worktree. So when the last component repeats the repository's name, the
/// parent is what actually distinguishes this checkout and the label uses it.
pub fn checkout_label(branch: Option<&str>, path: &Path, repo_root: &Path) -> String {
    if let Some(branch) = branch.filter(|b| !b.is_empty()) {
        return branch.to_string();
    }
    let name = path.file_name();
    if name.is_some()
        && name == repo_root.file_name()
        && let Some(parent) = path.parent().and_then(Path::file_name)
    {
        return parent.to_string_lossy().into_owned();
    }
    name.map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Filesystem-safe directory name for a branch (`feat/x` → `feat-x`).
/// Conservative whitelist: anything outside `[A-Za-z0-9._-]` becomes `-`.
/// Leading/trailing `-` AND `.` are trimmed: a dot-only branch (`.`/`..`)
/// would otherwise survive as a path-traversal component of the (destructive)
/// worktree path, and a leading dot would hide the directory. May return ""
/// for degenerate input - spec validation rejects that before any git call,
/// and [`worktree_dir`] falls back to a safe constant as defense in depth.
pub fn branch_slug(branch: &str) -> String {
    let slug: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    slug.trim_matches(|c: char| c == '-' || c == '.')
        .to_string()
}

fn branch_slug_or_default(branch: &str) -> String {
    let slug = branch_slug(branch);
    if slug.is_empty() {
        "branch".to_string()
    } else {
        slug
    }
}

fn branch_hash_suffix(branch: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in branch.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")[..8].to_string()
}

fn worktrees_parent(repo_root: &Path) -> PathBuf {
    let repo_name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    let parent = repo_root.parent().unwrap_or(repo_root);
    parent.join(format!("{repo_name}.worktrees"))
}

/// Sibling worktree directory for a branch: `<repo>.worktrees/<slug>`, next to
/// the repo (NOT inside it - recursive watchers must not descend into it).
/// Total function: a branch whose slug is empty (dot-only - rejected upstream
/// by spec validation) maps to the constant `branch` so the result can never
/// resolve outside `<repo>.worktrees/`.
pub fn worktree_dir(repo_root: &Path, branch: &str) -> PathBuf {
    worktrees_parent(repo_root).join(branch_slug_or_default(branch))
}

/// Collision-resistant sibling directory for a branch. Kept separate from
/// [`worktree_dir`] so existing readable paths remain valid; planners switch
/// to this path only when the slug path is already claimed by another branch.
pub fn worktree_dir_hashed(repo_root: &Path, branch: &str) -> PathBuf {
    let slug = branch_slug_or_default(branch);
    worktrees_parent(repo_root).join(format!("{slug}-{}", branch_hash_suffix(branch)))
}

pub fn is_paneflow_worktree_dir(repo_root: &Path, branch: &str, path: &Path) -> bool {
    path == worktree_dir(repo_root, branch) || path == worktree_dir_hashed(repo_root, branch)
}

/// Run a git plumbing command and return trimmed stdout, mapping every
/// failure mode (spawn, timeout, non-zero exit) to a displayable message.
fn run_git(repo: &Path, args: &[&str], deadline: Duration) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).args(args);
    let out = paneflow_process::run_with_timeout(cmd, deadline, STDOUT_CAP)
        .map_err(|e| format!("git {} failed: {e}", args.join(" ")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim().lines().last().unwrap_or("non-zero exit")
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `git worktree list --porcelain`, parsed.
pub fn list_worktrees(repo_root: &Path) -> Result<Vec<WorktreeEntry>, String> {
    let stdout = run_git(
        repo_root,
        &["worktree", "list", "--porcelain"],
        GIT_DEADLINE,
    )?;
    Ok(parse_worktree_porcelain(&stdout))
}

/// Pure porcelain parser (unit-tested). Entries are blank-line separated;
/// `branch refs/heads/<name>` is absent for detached or bare entries.
pub fn parse_worktree_porcelain(stdout: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    for line in stdout.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if let Some(p) = path.take() {
                entries.push(WorktreeEntry {
                    path: p,
                    branch: branch.take(),
                });
            }
            branch = None;
            continue;
        }
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
        }
    }
    entries
}

/// True when `branch` exists locally in the repo.
pub fn branch_exists(repo_root: &Path, branch: &str) -> bool {
    run_git(
        repo_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
        GIT_DEADLINE,
    )
    .is_ok()
}

/// Local branches, most recently committed first.
///
/// The picker offers branches, not worktrees: a worktree is how git gives a
/// second branch a directory, and it is the branch the user is choosing
/// between. Ordering by commit date puts the ones actually being worked on at
/// the top, where a repository with fifty branches needs them.
pub fn list_branches(repo_root: &Path) -> Result<Vec<String>, String> {
    let stdout = run_git(
        repo_root,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "--sort=-committerdate",
            "refs/heads",
        ],
        GIT_DEADLINE,
    )?;
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Where a chosen branch's checkout is, or has to be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchCheckout {
    /// A worktree already holds the branch - bind to it, create nothing.
    Existing(PathBuf),
    /// Nothing holds it yet; this is where its worktree belongs.
    Create(PathBuf),
}

/// Resolve a branch to a checkout directory, without touching the filesystem.
///
/// Split from [`prepare_branch_checkout`] so the collision rules are testable
/// without a repository. `Existing` first: git refuses a second worktree on the
/// same branch, so reusing the one that holds it is the only way selecting an
/// already-checked-out branch can work at all.
pub fn plan_branch_checkout(
    entries: &[WorktreeEntry],
    repo_root: &Path,
    branch: &str,
) -> Result<BranchCheckout, String> {
    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.branch.as_deref() == Some(branch))
    {
        return Ok(BranchCheckout::Existing(entry.path.clone()));
    }
    let legacy = worktree_dir(repo_root, branch);
    let path = if entries.iter().any(|entry| entry.path == legacy) {
        worktree_dir_hashed(repo_root, branch)
    } else {
        legacy
    };
    if let Some(entry) = entries.iter().find(|entry| entry.path == path) {
        return Err(format!(
            "{} exists but holds another branch ({})",
            path.display(),
            entry.branch.as_deref().unwrap_or("detached")
        ));
    }
    Ok(BranchCheckout::Create(path))
}

/// The directory to work in for `branch`: the worktree that already holds it,
/// or a new sibling worktree checked out from it.
///
/// Blocking (two to three git subprocesses, one of them a full checkout) - the
/// caller runs it through `smol::unblock`.
///
/// The result is deliberately NOT recorded as a [`ManagedWorktree`]: teardown
/// is for the checkouts orchestration creates behind the user's back, and a
/// directory asked for by name, by hand, is the user's. Nothing here removes
/// it, and the branch is untouched either way.
pub fn prepare_branch_checkout(repo_root: &Path, branch: &str) -> Result<PathBuf, String> {
    let entries = list_worktrees(repo_root)?;
    match plan_branch_checkout(&entries, repo_root, branch)? {
        BranchCheckout::Existing(path) => Ok(path),
        BranchCheckout::Create(path) => {
            if path.exists() {
                return Err(format!(
                    "{} exists but is not a registered worktree; remove it first",
                    path.display()
                ));
            }
            add_worktree(repo_root, &path, branch, false)?;
            // Same courtesy `paneflow up` extends its worktrees: a checkout
            // without the repository's gitignored `.env*` cannot run the app
            // it holds.
            copy_env_files(repo_root, &path);
            Ok(path)
        }
    }
}

/// `git worktree add <path> [-b] <branch>`. `create_branch` chooses between
/// branching off HEAD (`-b`) and checking out the existing branch.
pub fn add_worktree(
    repo_root: &Path,
    path: &Path,
    branch: &str,
    create_branch: bool,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let path_s = path.to_string_lossy();
    let mut args: Vec<&str> = vec!["worktree", "add", &path_s];
    if create_branch {
        args.push("-b");
    }
    args.push(branch);
    run_git(repo_root, &args, ADD_DEADLINE)?;
    if let Err(e) = write_owner_marker(path, repo_root, branch) {
        let _ = remove_worktree(repo_root, path);
        return Err(e);
    }
    Ok(())
}

/// True when the worktree has no uncommitted changes (`status --porcelain`
/// empty). An error (worktree gone, git missing) is NOT "clean" - the caller
/// must keep its hands off when it cannot prove cleanliness.
pub fn is_clean(worktree_path: &Path) -> Result<bool, String> {
    run_git(worktree_path, &["status", "--porcelain"], GIT_DEADLINE).map(|out| out.is_empty())
}

/// `git worktree remove <path>`. Refuses dirty worktrees by itself too (git
/// native), but callers must check [`is_clean`] first to control messaging.
/// The BRANCH IS NEVER DELETED - that is the US-009 invariant, not a TODO.
pub fn remove_worktree(repo_root: &Path, path: &Path) -> Result<(), String> {
    let path_s = path.to_string_lossy();
    run_git(repo_root, &["worktree", "remove", &path_s], GIT_DEADLINE).map(|_| ())
}

/// `git worktree prune` - drops references whose directory no longer exists.
/// Git-native guarantee: a worktree whose directory still exists is untouched
/// (US-009 AC5), so this is safe to run blindly at startup.
pub fn prune(repo_root: &Path) -> Result<(), String> {
    run_git(repo_root, &["worktree", "prune"], GIT_DEADLINE).map(|_| ())
}

/// Copy top-level `.env*` FILES from `src_root` into `dst_root`, skipping any
/// that already exist there (a tracked `.env.example` arrives via checkout -
/// don't clobber it). Best-effort by design (US-007): a missing source dir or
/// an unreadable entry yields an empty/partial copy, never an error. Returns
/// the file names copied.
pub fn copy_env_files(src_root: &Path, dst_root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(src_root) else {
        return Vec::new();
    };
    let mut copied = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_s = name.to_string_lossy();
        if !name_s.starts_with(".env") {
            continue;
        }
        if !entry.path().is_file() {
            continue;
        }
        let dst = dst_root.join(&name);
        if dst.exists() {
            continue;
        }
        if std::fs::copy(entry.path(), &dst).is_ok() {
            copied.push(name_s.into_owned());
        }
    }
    copied.sort();
    copied
}

/// Tear down a batch of managed worktrees (blocking - run via `smol::unblock`
/// on the app side). Per entry: `Keep` policy → skip; dirty or unverifiable →
/// keep + warn (NEVER remove what might hold work); clean → remove. The
/// branch is never touched.
pub fn teardown_all(worktrees: Vec<ManagedWorktree>) {
    for wt in worktrees {
        if wt.teardown == TeardownPolicy::Keep {
            continue;
        }
        if !wt.path.exists() {
            // Directory already gone (user rm -rf'd it): just prune the ref.
            let _ = prune(&wt.repo_root);
            continue;
        }
        if !has_owner_marker(&wt.path) {
            log::warn!(
                "worktree kept: missing Paneflow owner marker in {}",
                wt.path.display()
            );
            continue;
        }
        match is_clean(&wt.path) {
            Ok(true) => match remove_worktree(&wt.repo_root, &wt.path) {
                Ok(()) => log::info!("worktree removed: {}", wt.path.display()),
                Err(e) => log::warn!("worktree kept ({}): {e}", wt.path.display()),
            },
            Ok(false) => log::warn!(
                "worktree kept: uncommitted changes in {}",
                wt.path.display()
            ),
            Err(e) => log::warn!(
                "worktree kept (cannot verify cleanliness): {} - {e}",
                wt.path.display()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_slug_is_filesystem_safe() {
        assert_eq!(
            branch_slug("feat/cli-orchestration"),
            "feat-cli-orchestration"
        );
        assert_eq!(branch_slug("fix/US-006_teardown"), "fix-US-006_teardown");
        assert_eq!(branch_slug("a b\\c:d"), "a-b-c-d");
        // Leading/trailing separators are trimmed so the dir never hides.
        assert_eq!(branch_slug("/weird/"), "weird");
        assert_eq!(branch_slug(".hidden"), "hidden");
        // Inner dots survive (version-style branches stay readable).
        assert_eq!(branch_slug("release/v1.2.3"), "release-v1.2.3");
    }

    #[test]
    fn branch_slug_neutralizes_dot_only_traversal() {
        // NFR (orchestration-v2): the slug is the one untrusted component of
        // a destructive path - `.`/`..` must never survive as a path segment.
        assert_eq!(branch_slug(".."), "");
        assert_eq!(branch_slug("."), "");
        assert_eq!(branch_slug("..."), "");
        assert_eq!(branch_slug("-..-"), "");
    }

    #[test]
    fn worktree_dir_never_escapes_the_worktrees_dir() {
        // Defense in depth below spec validation: even a dot-only branch maps
        // INSIDE `<repo>.worktrees/` (fallback slug), never to its parent.
        let dir = worktree_dir(Path::new("/home/a/dev/paneflow"), "..");
        assert_eq!(dir, PathBuf::from("/home/a/dev/paneflow.worktrees/branch"));
    }

    #[test]
    fn a_branch_already_checked_out_is_reused_never_recreated() {
        let repo = Path::new("/home/a/dev/paneflow");
        let entries = vec![
            WorktreeEntry {
                path: repo.to_path_buf(),
                branch: Some("main".to_string()),
            },
            WorktreeEntry {
                path: PathBuf::from("/home/a/dev/paneflow.worktrees/feat-x"),
                branch: Some("feat/x".to_string()),
            },
        ];
        // git refuses a second worktree on one branch, so the only workable
        // answer for an already-checked-out branch is the checkout it is in -
        // including the repository's own, which is how selecting `main` unbinds.
        assert_eq!(
            plan_branch_checkout(&entries, repo, "feat/x"),
            Ok(BranchCheckout::Existing(PathBuf::from(
                "/home/a/dev/paneflow.worktrees/feat-x"
            )))
        );
        assert_eq!(
            plan_branch_checkout(&entries, repo, "main"),
            Ok(BranchCheckout::Existing(repo.to_path_buf()))
        );
        assert_eq!(
            plan_branch_checkout(&entries, repo, "chore/rust-1.98"),
            Ok(BranchCheckout::Create(PathBuf::from(
                "/home/a/dev/paneflow.worktrees/chore-rust-1.98"
            )))
        );
    }

    #[test]
    fn a_slug_collision_falls_back_to_the_hashed_dir() {
        let repo = Path::new("/home/a/dev/paneflow");
        // `feat/x` and `feat-x` slugify the same; the second one asked for
        // takes the hashed path rather than the occupied one.
        let entries = vec![WorktreeEntry {
            path: worktree_dir(repo, "feat/x"),
            branch: Some("feat/x".to_string()),
        }];
        assert_eq!(
            plan_branch_checkout(&entries, repo, "feat-x"),
            Ok(BranchCheckout::Create(worktree_dir_hashed(repo, "feat-x")))
        );
    }

    #[test]
    fn a_registered_checkout_on_the_target_path_is_refused_not_overwritten() {
        let repo = Path::new("/home/a/dev/paneflow");
        // Both candidate paths are taken by checkouts of something else (a
        // detached HEAD names no branch, so neither matches by branch).
        let entries = vec![
            WorktreeEntry {
                path: worktree_dir(repo, "feat/x"),
                branch: None,
            },
            WorktreeEntry {
                path: worktree_dir_hashed(repo, "feat/x"),
                branch: None,
            },
        ];
        let planned = plan_branch_checkout(&entries, repo, "feat/x");
        assert!(
            planned.is_err(),
            "a registered checkout on the target path must never be written over: {planned:?}"
        );
    }

    #[test]
    fn worktree_dir_is_a_sibling_of_the_repo() {
        let dir = worktree_dir(Path::new("/home/a/dev/paneflow"), "feat/x");
        assert_eq!(dir, PathBuf::from("/home/a/dev/paneflow.worktrees/feat-x"));
        // NOT inside the repo: recursive watchers must not see it.
        assert!(!dir.starts_with("/home/a/dev/paneflow/"));
    }

    #[test]
    fn hashed_worktree_dir_disambiguates_slug_collisions() {
        let repo = Path::new("/home/a/dev/paneflow");
        let a = "feat/a b";
        let b = "feat/a-b";
        assert_eq!(branch_slug(a), branch_slug(b));
        assert_eq!(worktree_dir(repo, a), worktree_dir(repo, b));

        let hashed_a = worktree_dir_hashed(repo, a);
        let hashed_b = worktree_dir_hashed(repo, b);
        assert_ne!(hashed_a, hashed_b);
        assert!(is_paneflow_worktree_dir(repo, a, &hashed_a));
        assert!(is_paneflow_worktree_dir(repo, b, &hashed_b));
        assert!(!hashed_a.starts_with("/home/a/dev/paneflow/"));
    }

    #[test]
    fn parses_worktree_porcelain_with_detached_and_branches() {
        let out = "worktree /home/a/dev/repo\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/main\n\nworktree /home/a/dev/repo.worktrees/feat-x\nHEAD 2222222222222222222222222222222222222222\nbranch refs/heads/feat/x\n\nworktree /tmp/detached\nHEAD 3333333333333333333333333333333333333333\ndetached\n";
        let entries = parse_worktree_porcelain(out);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert_eq!(
            entries[1].path,
            PathBuf::from("/home/a/dev/repo.worktrees/feat-x")
        );
        assert_eq!(entries[1].branch.as_deref(), Some("feat/x"));
        assert_eq!(entries[2].branch, None, "detached HEAD has no branch");
    }

    #[test]
    fn parse_worktree_porcelain_handles_missing_trailing_blank() {
        let out = "worktree /r\nbranch refs/heads/main";
        let entries = parse_worktree_porcelain(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn managed_worktree_record_requires_marker_and_generated_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        let branch = "feat/hardening";
        let path = worktree_dir(&repo_root, branch);
        std::fs::create_dir_all(&path).expect("worktree dir");

        assert!(
            managed_worktree_from_record(
                &path.to_string_lossy(),
                &repo_root.to_string_lossy(),
                branch,
                "auto",
            )
            .is_none(),
            "a matching path without owner marker is not enough"
        );

        std::fs::write(owner_marker_path(&path), "owner=paneflow\n").expect("marker");
        let restored = managed_worktree_from_record(
            &path.to_string_lossy(),
            &repo_root.to_string_lossy(),
            branch,
            "delete",
        )
        .expect("marker-backed record restores");
        assert_eq!(restored.path, path);
        assert_eq!(restored.teardown, TeardownPolicy::Keep);

        let outside = tmp.path().join("external");
        std::fs::create_dir_all(&outside).expect("outside dir");
        std::fs::write(owner_marker_path(&outside), "owner=paneflow\n").expect("outside marker");
        assert!(
            managed_worktree_from_record(
                &outside.to_string_lossy(),
                &repo_root.to_string_lossy(),
                branch,
                "auto",
            )
            .is_none(),
            "marker cannot bless a path outside the deterministic Paneflow dir"
        );
    }

    #[test]
    fn managed_worktree_record_accepts_hashed_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        let branch = "feat/a-b";
        let path = worktree_dir_hashed(&repo_root, branch);
        std::fs::create_dir_all(&path).expect("worktree dir");
        std::fs::write(owner_marker_path(&path), "owner=paneflow\n").expect("marker");

        let restored = managed_worktree_from_record(
            &path.to_string_lossy(),
            &repo_root.to_string_lossy(),
            branch,
            "auto",
        )
        .expect("hashed path restores");

        assert_eq!(restored.path, path);
        assert_eq!(restored.branch, branch);
    }

    #[test]
    fn copy_env_files_copies_top_level_env_only_and_never_clobbers() {
        let src = tempfile::tempdir().expect("src");
        let dst = tempfile::tempdir().expect("dst");
        std::fs::write(src.path().join(".env"), "A=1").unwrap();
        std::fs::write(src.path().join(".env.local"), "B=2").unwrap();
        std::fs::write(src.path().join("notenv"), "x").unwrap();
        // Nested .env must NOT be picked up (top-level only).
        std::fs::create_dir(src.path().join("sub")).unwrap();
        std::fs::write(src.path().join("sub/.env"), "C=3").unwrap();
        // Pre-existing destination file must survive (checkout owns it).
        std::fs::write(dst.path().join(".env"), "KEEP").unwrap();

        let copied = copy_env_files(src.path(), dst.path());
        assert_eq!(copied, vec![".env.local".to_string()]);
        assert_eq!(
            std::fs::read_to_string(dst.path().join(".env")).unwrap(),
            "KEEP",
            "existing destination file is never clobbered"
        );
        assert!(dst.path().join(".env.local").exists());
        assert!(!dst.path().join("notenv").exists());
    }

    #[test]
    fn copy_env_files_missing_source_is_silent_empty() {
        let dst = tempfile::tempdir().expect("dst");
        let copied = copy_env_files(Path::new("/nonexistent-paneflow-test"), dst.path());
        assert!(copied.is_empty());
    }

    #[test]
    fn a_detached_checkout_is_named_by_what_distinguishes_it() {
        let repo = Path::new("/home/u/dev/paneflow");
        assert_eq!(
            checkout_label(Some("feat/login"), Path::new("/wt/feat-login"), repo),
            "feat/login"
        );
        // The layout agent tooling produces: every worktree directory is named
        // after the repository, so the last component says nothing.
        assert_eq!(
            checkout_label(
                None,
                Path::new("/home/u/dev/worktrees/paneflow/poplar-plume/paneflow"),
                repo
            ),
            "poplar-plume"
        );
        // A directory that already differs is kept as-is.
        assert_eq!(
            checkout_label(None, Path::new("/wt/hotfix-42"), repo),
            "hotfix-42"
        );
        // An empty branch is a detached HEAD, not a branch named "".
        assert_eq!(
            checkout_label(Some(""), Path::new("/wt/hotfix-42"), repo),
            "hotfix-42"
        );
    }
}
