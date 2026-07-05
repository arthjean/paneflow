//! Git plumbing for the multi-worktree diff viewer (US-005,
//! prd-multi-worktree-diff-2026-Q3.md).
//!
//! All heavy git operations run via `std::process::Command` subprocesses
//! (matching Zed's actual diff/worktree path and Paneflow's existing
//! `GitDiffStats::from_cwd`), never a library. Every command sets
//! `.current_dir()` to the worktree root - never the live shell cwd - and
//! returns a structured error instead of panicking on a non-zero exit or a
//! missing ref. Callers run these off the GPUI main thread (US-007).
//!
//! Diff semantic: `merge-base(HEAD, <base>)..working-tree`, including
//! uncommitted (tracked) changes - "what this branch adds since it diverged
//! from base". Base text comes from `git show <merge-base>:<path>`, new text
//! from the working-tree file on disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::engine::{DiffHunk, compute_hunks};

/// A git worktree as reported by `git worktree list --porcelain`.
///
/// On the live Worktree-scope discovery path (US-013): the porcelain parser
/// feeds [`list_repo_worktrees`], which the GUI invokes to enumerate worktrees
/// not open as workspaces. `is_main` / `is_bare` are parsed for completeness but
/// only exercised by the unit tests today, hence the field-level `allow`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub ref_name: Option<String>,
    pub sha: String,
    #[allow(dead_code)]
    pub is_main: bool,
    #[allow(dead_code)]
    pub is_bare: bool,
}

/// How a file changed between the merge-base and the working tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChange {
    Added,
    Modified,
    Deleted,
    /// Detected rename (`git diff -M`). The file's `path` is the new
    /// destination; the original path is carried in [`FileDiff::old_path`] /
    /// [`super::view::FileEntry::old_path`] so the UI can show `old → new` as a
    /// single entry instead of a delete + add pair.
    Renamed,
}

/// Per-file diff payload consumed by the renderer (US-006). Carries the raw
/// base/new text so the unified/side-by-side views can shape the actual lines.
#[derive(Clone, Debug)]
pub struct FileDiff {
    /// Path relative to the worktree root (the destination/new path for a
    /// rename).
    pub path: String,
    pub change: FileChange,
    /// Original path for a detected rename ([`FileChange::Renamed`]); `None`
    /// otherwise. Lets the UI render `old → new` and load the base text from the
    /// pre-rename path.
    pub old_path: Option<String>,
    pub base_text: String,
    pub new_text: String,
    pub hunks: Vec<DiffHunk>,
    /// Binary files are listed but not shown (no text rendering).
    pub is_binary: bool,
}

impl FileDiff {
    /// Total added / removed line counts across the file's hunks.
    pub fn line_counts(&self) -> (u32, u32) {
        let mut added = 0;
        let mut removed = 0;
        for h in &self.hunks {
            added += h.new_row_range.end - h.new_row_range.start;
            removed += h.base_row_range.end - h.base_row_range.start;
        }
        (added, removed)
    }
}

/// The diff of one worktree against a resolved base ref. `error` is `Some` when
/// the diff could not be computed (e.g. base ref not found, no merge base).
#[derive(Clone, Debug, Default)]
pub struct WorktreeDiff {
    pub files: Vec<FileDiff>,
    pub error: Option<String>,
}

/// Git-native per-file diffstat for one file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileDiffStat {
    pub added: u32,
    pub removed: u32,
}

/// Parse `git worktree list --porcelain`. Ported from Zed's
/// `git::repository::parse_worktrees_from_str` (`crates/git/src/repository.rs:363`).
pub fn parse_worktrees_from_str(raw: &str, main_worktree_path: Option<&Path>) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let normalized = raw.replace("\r\n", "\n");
    for entry in normalized.split("\n\n") {
        let mut path = None;
        let mut sha = None;
        let mut ref_name = None;
        let mut is_bare = false;

        for line in entry.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("worktree ") {
                path = Some(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("HEAD ") {
                sha = Some(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("branch ") {
                ref_name = Some(rest.to_string());
            } else if line == "bare" {
                is_bare = true;
            }
            // Ignore detached / locked / prunable / etc.
        }

        if let (Some(path), Some(sha)) = (path, sha) {
            let path = PathBuf::from(path);
            let is_main = main_worktree_path.is_some_and(|main| path == main);
            worktrees.push(Worktree {
                path,
                ref_name,
                sha,
                is_main,
                is_bare,
            });
        }
    }
    worktrees
}

/// Wall-clock deadline for every diff-viewer git call (U-035). Generous enough
/// for a large but healthy repo, short enough that a dead/slow mount or a
/// hanging `.git/config` helper fails instead of wedging the blocking-pool task.
const GIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// stdout cap for diff-viewer git calls. Comfortably above [`MAX_FILE_BYTES`]
/// (512 KiB) so a legitimate `git show` of an accepted file is never truncated,
/// while bounding a runaway/hijacked git that streams unbounded output. A
/// too-large file is read up to this ceiling and then rejected downstream by
/// the `MAX_FILE_BYTES` check, so truncated bytes are never displayed.
const GIT_STDOUT_CAP: u64 = 16 * 1024 * 1024;

/// Run a git subprocess in `dir`, returning captured stdout bytes on success.
/// A non-zero exit (or a timeout) returns `Err` with the trimmed stderr (or a
/// generic message); the caller renders the diff's "unavailable" state. Never
/// panics, never blocks past [`GIT_DEADLINE`].
fn run_git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        // U-035: never block on a credential/helper prompt.
        .env("GIT_TERMINAL_PROMPT", "0");
    // U-035: bound the subprocess (run_with_timeout also nulls stdin + caps
    // stdout) so a hung git can't pin the diff viewer's blocking-pool task.
    let output =
        paneflow_process::run_with_timeout(cmd, GIT_DEADLINE, GIT_STDOUT_CAP).map_err(|e| {
            format!(
                "git {} failed: {e}",
                args.first().copied().unwrap_or("command")
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr.trim();
        return Err(if msg.is_empty() {
            format!("git {} failed", args.first().copied().unwrap_or("command"))
        } else {
            msg.to_string()
        });
    }
    Ok(output.stdout)
}

/// List all worktrees of the repository containing `repo_dir`. Live path:
/// [`list_repo_worktrees`] calls this for the US-013 Worktree-scope "include
/// worktrees not open as workspaces" enumeration.
pub fn list_worktrees(repo_dir: &Path) -> Result<Vec<Worktree>, String> {
    let out = run_git(repo_dir, &["worktree", "list", "--porcelain"])?;
    let text = String::from_utf8_lossy(&out);
    Ok(parse_worktrees_from_str(&text, Some(repo_dir)))
}

/// US-013 (prd-git-diff-mode-2026-Q3.md): every worktree of the repo as
/// `(path, short-branch)`, for the Worktree scope's "include worktrees not open
/// as workspaces" enumeration. Reuses the tested porcelain parser; returns an
/// empty vec on error (the caller falls back to the open-workspace set). Runs a
/// git subprocess, so callers invoke it off the GPUI main thread.
pub fn list_repo_worktrees(repo_dir: &Path) -> Vec<(PathBuf, String)> {
    let worktrees = match list_worktrees(repo_dir) {
        Ok(w) => w,
        Err(_) => return Vec::new(),
    };
    worktrees
        .into_iter()
        .map(|w| {
            let branch = w
                .ref_name
                .as_deref()
                .map(short_ref)
                .unwrap_or_else(|| w.sha.chars().take(7).collect());
            (w.path, branch)
        })
        .collect()
}

/// Short branch name from a full ref (`refs/heads/develop` → `develop`).
fn short_ref(ref_name: &str) -> String {
    ref_name
        .strip_prefix("refs/heads/")
        .unwrap_or(ref_name)
        .to_string()
}

/// Whether `ref_name` resolves to a commit in `worktree_dir`. Public so the
/// multi-project shared-base seed can verify a base carried from another repo
/// actually exists here before using it (else it falls back to this repo's
/// default).
pub fn ref_exists(worktree_dir: &Path, ref_name: &str) -> bool {
    run_git(
        worktree_dir,
        &["rev-parse", "--verify", "--quiet", ref_name],
    )
    .is_ok()
}

/// Pick a sensible default base ref for `worktree_dir`: `develop`, else the
/// repo's default branch (`main` / `master`). Returns `None` when none resolve.
pub fn default_base_ref(worktree_dir: &Path) -> Option<String> {
    for candidate in ["develop", "main", "master"] {
        if ref_exists(worktree_dir, candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Cheap content fingerprint of a worktree's diff inputs, used on diff-mode
/// re-entry to decide whether a column's already-loaded rows are still valid or
/// must be re-diffed (US-016 warm-resume). It captures the three things a
/// `merge-base..working-tree` diff depends on: the worktree `HEAD`, the resolved
/// `base_ref` commit, and a hash of `git status` (staged + unstaged + untracked)
/// so a commit, a `git add`, a working-tree edit, an untracked file, or a
/// base-ref advance an AI agent made from a CLI pane while the diff was hidden is
/// all detected. One subprocess set, no per-file blob reads - orders of magnitude
/// cheaper than the full pipeline it gates.
#[derive(Clone, PartialEq, Eq)]
pub struct ColumnFingerprint {
    head: String,
    base: String,
    status_hash: u64,
}

/// Compute a [`ColumnFingerprint`] for `worktree_dir` against `base_ref`. Runs
/// git subprocesses, so callers invoke it off the GPUI main thread (inside the
/// column build closure / a `smol::unblock`). A failed `rev-parse` yields an
/// empty component and a failed `status` yields `0`, so an unborn/detached HEAD
/// or a mid-rebase tree simply never matches a prior fingerprint - forcing a
/// (correct) reload rather than showing stale rows.
pub fn column_fingerprint(worktree_dir: &Path, base_ref: &str) -> ColumnFingerprint {
    use std::hash::Hasher as _;
    // Resolve the worktree's own root first, exactly as `compute_worktree_diff`
    // does. The seed `worktree_dir` may be a SUBDIRECTORY (the workspace opened
    // after a shell `cd`); running `git status` from there would report only that
    // subtree's changes, so a change outside it would leave the fingerprint
    // unchanged and `revalidate` would falsely keep stale rows. Keying off the
    // toplevel makes the fingerprint cover the same scope the diff does.
    let toplevel = worktree_toplevel(worktree_dir);
    let worktree_dir = toplevel.as_path();
    let rev = |r: &str| {
        run_git(worktree_dir, &["rev-parse", r])
            .ok()
            .map(|o| String::from_utf8_lossy(&o).trim().to_string())
            .unwrap_or_default()
    };
    let status_hash = run_git(
        worktree_dir,
        &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
    )
    .ok()
    .map(|o| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        h.write(&o);
        h.finish()
    })
    .unwrap_or(0);
    ColumnFingerprint {
        head: rev("HEAD"),
        base: rev(base_ref),
        status_hash,
    }
}

/// Candidate base refs for the selector (US-013): local branches *and*
/// remote-tracking branches (`origin/develop`, `origin/main`, …), so the user
/// can diff a worktree against an upstream base, not just locals. The
/// `origin/HEAD` alias is filtered out. Sorted, deduplicated; empty on error
/// (the selector then just shows the resolved default).
pub fn list_base_ref_candidates(worktree_dir: &Path) -> Vec<String> {
    let out = match run_git(
        worktree_dir,
        &["branch", "-a", "--format=%(refname:short)", "--list"],
    ) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let mut names: Vec<String> = String::from_utf8_lossy(&out)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.ends_with("/HEAD"))
        .collect();
    // Tags too, so a worktree can be diffed against a release tag (e.g.
    // `v0.3.6`), not just a branch. Arbitrary SHAs are handled separately by the
    // picker's free-text resolution (see `DiffView::resolve_and_set_base`).
    if let Ok(out) = run_git(worktree_dir, &["tag", "--list"]) {
        names.extend(
            String::from_utf8_lossy(&out)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty()),
        );
    }
    names.sort();
    names.dedup();
    names
}

/// Resolve the working-tree root of `dir` via `git rev-parse --show-toplevel`.
///
/// `dir` is the column's seed path, which may be a *subdirectory* of the
/// worktree (the workspace was opened after a shell `cd`, or seeded from the
/// shell cwd). git resolves the toplevel from any subdir, so this returns the
/// worktree's own root - never the shared repo root (that would diff the main
/// checkout for every column). All file reads + git calls then key off this so
/// `worktree_dir.join(repo_root_relative_path)` lands on the right file.
/// Falls back to `dir` when git can't resolve (non-repo, error).
fn worktree_toplevel(dir: &Path) -> PathBuf {
    match run_git(dir, &["rev-parse", "--show-toplevel"]) {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out).trim().to_string();
            if s.is_empty() {
                dir.to_path_buf()
            } else {
                PathBuf::from(s)
            }
        }
        Err(_) => dir.to_path_buf(),
    }
}

fn list_untracked_limited(dir: &Path, limit: usize) -> (Vec<String>, bool) {
    if limit == 0 {
        return (Vec::new(), false);
    }
    let Ok(out) = run_git(dir, &["ls-files", "--others", "--exclude-standard", "-z"]) else {
        return (Vec::new(), false);
    };
    let mut paths = Vec::new();
    let mut truncated = false;
    for raw_path in out.split(|&b| b == 0).filter(|s| !s.is_empty()) {
        if paths.len() >= limit {
            truncated = true;
            break;
        }
        let Some(path) = decode_git_path(raw_path, "ls-files --others") else {
            continue;
        };
        paths.push(path);
    }
    (paths, truncated)
}

/// Resolve the merge-base SHA between `HEAD` and `base_ref` in `worktree_dir`.
fn merge_base(worktree_dir: &Path, base_ref: &str) -> Result<String, String> {
    let out = run_git(worktree_dir, &["merge-base", "HEAD", base_ref])?;
    let sha = String::from_utf8_lossy(&out).trim().to_string();
    if sha.is_empty() {
        return Err(format!("no common ancestor with '{base_ref}'"));
    }
    Ok(sha)
}

/// Normalize text the way git's diff stats do for text files: repository blobs
/// are LF-normalized, while a Windows worktree may contain CRLF due
/// `core.autocrlf`. The renderer should not turn that checkout detail into a
/// whole-file edit.
fn normalize_git_text(text: String) -> String {
    if text.as_bytes().contains(&b'\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text
    }
}

/// Bytes look textual if they contain no NUL and decode as UTF-8.
fn classify(bytes: Vec<u8>) -> (String, bool) {
    if bytes.contains(&0) {
        return (String::new(), true);
    }
    match String::from_utf8(bytes) {
        Ok(s) => (normalize_git_text(s), false),
        Err(_) => (String::new(), true),
    }
}

fn base_path_exists(worktree_dir: &Path, merge_base: &str, rel_path: &str) -> Result<bool, String> {
    let out = run_git(
        worktree_dir,
        &["ls-tree", "-z", "--name-only", merge_base, "--", rel_path],
    )?;
    Ok(out
        .split(|&b| b == 0)
        .any(|path| path == rel_path.as_bytes()))
}

/// Load the base-side text of `rel_path` at the merge-base commit. Returns
/// `(text, is_binary)`; a path absent at the merge-base (file added since
/// divergence) yields empty text, not an error.
fn load_base_text(worktree_dir: &Path, merge_base: &str, rel_path: &str) -> (String, bool) {
    let spec = format!("{merge_base}:{rel_path}");
    match run_git(worktree_dir, &["show", &spec]) {
        Ok(bytes) => classify(bytes),
        Err(show_err) => match base_path_exists(worktree_dir, merge_base, rel_path) {
            Ok(false) => (String::new(), false),
            Ok(true) => {
                log::warn!("git: failed to load base-side file {rel_path}: {show_err}");
                (String::new(), true)
            }
            Err(exists_err) => {
                log::warn!(
                    "git: failed to verify base-side file {rel_path}: {show_err}; {exists_err}"
                );
                (String::new(), true)
            }
        },
    }
}

/// Read the working-tree text of `rel_path`. Returns `(text, is_binary)`; a
/// missing file (deleted in the working tree) yields empty text. Any other I/O
/// error (permission denied, device error) is logged and rendered as an
/// unreadable (binary) stub rather than masquerading as a deletion.
fn load_working_text(worktree_dir: &Path, rel_path: &str) -> (String, bool) {
    let path = worktree_dir.join(rel_path);
    // U-041: lstat first. A tracked/untracked symlink in a crafted repo could
    // point outside the worktree; `fs::read` would dereference it and pull an
    // out-of-tree file into `new_text`. Render the LINK TARGET instead of
    // following it - this also matches git's own symlink-blob semantics (the
    // base side via `git show` returns the target path, not the pointee's
    // content), so an unchanged symlink produces no spurious diff.
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(&path)
                .map(|t| t.to_string_lossy().into_owned())
                .unwrap_or_default();
            (target, false)
        }
        Ok(_) => match std::fs::read(&path) {
            Ok(bytes) => classify(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
            Err(e) => {
                log::warn!("git: failed to read working-tree file {rel_path}: {e}");
                (String::new(), true)
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
        Err(e) => {
            log::warn!("git: failed to lstat working-tree file {rel_path}: {e}");
            (String::new(), true)
        }
    }
}

/// Parse `git diff --name-status -z <merge_base>` (NUL-delimited records).
/// Each record is a status field followed by its path(s); renames/copies carry
/// a source and a destination path - we key on the destination.
fn parse_name_status_z(stdout: &[u8]) -> Vec<(FileChange, String, Option<String>)> {
    let mut fields = stdout.split(|&b| b == 0).filter(|f| !f.is_empty());
    let mut out = Vec::new();
    while let Some(status) = fields.next() {
        let code = status.first().copied().unwrap_or(b'M') as char;
        // Rename/copy: status is followed by <src>\0<dst>; key on dst, keep src.
        let (path, old) = if matches!(code, 'R' | 'C') {
            let Some(src) = fields.next() else {
                break;
            };
            let Some(dst) = fields.next() else {
                break;
            };
            let Some(src) = decode_git_path(src, "diff --name-status source") else {
                continue;
            };
            let Some(dst) = decode_git_path(dst, "diff --name-status destination") else {
                continue;
            };
            (dst, Some(src))
        } else {
            let Some(path) = fields.next() else {
                break;
            };
            let Some(path) = decode_git_path(path, "diff --name-status") else {
                continue;
            };
            (path, None)
        };
        let change = match code {
            'A' => FileChange::Added,
            'D' => FileChange::Deleted,
            'R' => FileChange::Renamed,
            _ => FileChange::Modified, // M, C, T → modified content
        };
        out.push((change, path, old));
    }
    out
}

fn parse_numstat_z(stdout: &[u8]) -> HashMap<String, FileDiffStat> {
    let mut out = HashMap::new();
    let mut fields = stdout.split(|&b| b == 0).filter(|f| !f.is_empty());
    while let Some(record) = fields.next() {
        let Some((added, removed, path)) = split_numstat_record(record) else {
            continue;
        };
        let path = if path.is_empty() {
            let _old_path = fields.next();
            let Some(new_path) = fields.next() else {
                break;
            };
            new_path
        } else {
            path
        };
        let Some(path) = decode_git_path(path, "diff --numstat") else {
            continue;
        };
        let stat = out.entry(path).or_insert(FileDiffStat {
            added: 0,
            removed: 0,
        });
        stat.added = stat.added.saturating_add(parse_numstat_count(added));
        stat.removed = stat.removed.saturating_add(parse_numstat_count(removed));
    }
    out
}

fn decode_git_path(path: &[u8], source: &str) -> Option<String> {
    match std::str::from_utf8(path) {
        Ok(path) if !path.is_empty() => Some(path.to_string()),
        Ok(_) => None,
        Err(_) => {
            log::warn!("git: skipping non-UTF-8 path from {source}");
            None
        }
    }
}

fn split_numstat_record(record: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let first_tab = record.iter().position(|&b| b == b'\t')?;
    let rest = &record[first_tab + 1..];
    let second_tab = rest.iter().position(|&b| b == b'\t')?;
    Some((
        &record[..first_tab],
        &rest[..second_tab],
        &rest[second_tab + 1..],
    ))
}

fn parse_numstat_count(raw: &[u8]) -> u32 {
    if raw == b"-" {
        return 0;
    }
    std::str::from_utf8(raw)
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
}

/// Files above this size (either side, bytes) are shown as a stub instead of
/// being loaded + diffed + highlighted. Without this a single huge generated
/// file (minified bundle, vendored blob) loads megabytes into RAM, runs
/// `imara-diff` + a full syntect pass over it, and - across N columns - OOMs the
/// process. 512 KiB comfortably covers hand-written source.
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// Hard cap on changed files diffed per worktree. A 1000-file refactor would
/// otherwise load every file into RAM (×N columns); beyond this the column
/// stops and shows a truncation row.
const MAX_FILE_COUNT: usize = 200;

/// Lockfiles and other large, low-signal generated files - never worth a
/// line-by-line diff and a prime OOM trigger (`Cargo.lock` alone is ~12k lines).
const SKIP_FILENAMES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "bun.lockb",
    "yarn.lock",
    "pnpm-lock.yaml",
    "composer.lock",
    "poetry.lock",
    "Gemfile.lock",
];

/// Whether `path`'s final component is a known generated/lockfile name. Public
/// so the file watcher ([`super::view`]) shares this single source of truth and
/// cannot drift from the diff-time skip list.
pub fn is_skipped_name(path: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| SKIP_FILENAMES.contains(&n))
}

/// Working-tree size of `rel_path` exceeds [`MAX_FILE_BYTES`]. Fast pre-load
/// guard for the common added/modified case; a metadata miss (deleted file)
/// reads as not-too-large. The base side - and the metadata-miss case - is
/// caught by the post-load length check in [`compute_worktree_diff`].
fn is_too_large(worktree_dir: &Path, rel_path: &str) -> bool {
    std::fs::metadata(worktree_dir.join(rel_path))
        .map(|m| m.len() > MAX_FILE_BYTES)
        .unwrap_or(false)
}

/// A "not shown" stub: rendered as a single notice row, never loaded/diffed.
fn stub_file(path: String, change: FileChange) -> FileDiff {
    FileDiff {
        path,
        change,
        old_path: None,
        base_text: String::new(),
        new_text: String::new(),
        hunks: Vec::new(),
        is_binary: true,
    }
}

/// Compute the diff of `worktree_dir` against `base_ref`:
/// `merge-base(HEAD, base_ref)..working-tree`, including uncommitted changes.
///
/// Runs entirely via subprocess; safe to call off the main thread. Returns a
/// `WorktreeDiff` whose `error` is set (rather than panicking) when the base
/// ref or merge base cannot be resolved. Oversized / lockfile / over-count
/// files are shown as stubs rather than loaded, bounding peak RAM.
pub fn compute_worktree_diff(worktree_dir: &Path, base_ref: &str) -> WorktreeDiff {
    // Resolve the worktree's own root once: the seed path may be a subdirectory
    // (shell `cd`), which would make `worktree_dir.join(rel_path)` miss every
    // file. Everything below - merge-base, name-status, file reads - keys off
    // this so the diff is correct regardless of the seed path's depth.
    let toplevel = worktree_toplevel(worktree_dir);
    let worktree_dir = toplevel.as_path();
    log::debug!(
        "git: compute_worktree_diff dir={} base={base_ref}",
        worktree_dir.display()
    );
    let merge_base = match merge_base(worktree_dir, base_ref) {
        Ok(mb) => mb,
        Err(e) => {
            log::warn!("git: merge_base failed (base={base_ref}): {e}");
            return WorktreeDiff {
                files: Vec::new(),
                error: Some(e),
            };
        }
    };
    log::debug!("git: merge_base={merge_base}");

    compute_diff_against(worktree_dir, &merge_base)
}

/// Per-file Git-native diffstat for the same semantic as
/// [`compute_worktree_diff`]: `merge-base(HEAD, base_ref)..working-tree`, plus
/// untracked files. This is used for Review's sidebar/global counters so they
/// match `git diff --numstat` instead of drifting with renderer hunk details.
pub fn compute_worktree_file_stats(
    worktree_dir: &Path,
    base_ref: &str,
) -> HashMap<String, FileDiffStat> {
    let toplevel = worktree_toplevel(worktree_dir);
    let worktree_dir = toplevel.as_path();
    let Ok(merge_base) = merge_base(worktree_dir, base_ref) else {
        return HashMap::new();
    };
    compute_file_stats_against(worktree_dir, &merge_base)
}

/// Git's well-known empty-tree object hash. Diffing against it (used when `HEAD`
/// is unborn - a repo with no commits yet) shows every tracked file as a pure
/// addition, so a first changeset still renders in the Agents dock.
const EMPTY_TREE_SHA: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Compute the diff of `worktree_dir` against `HEAD`: the working tree vs the
/// last commit (staged + unstaged tracked changes) plus untracked files - the
/// "what did the agent just touch" semantic used by the Agents diff dock
/// ([`crate::app::agents_diff`]). When `HEAD` is unborn, everything is diffed
/// against the empty tree. Reuses [`compute_diff_against`], so the lockfile /
/// size / count / binary guards are identical to [`compute_worktree_diff`].
///
/// Runs entirely via subprocess; safe to call off the main thread.
pub fn compute_head_diff(worktree_dir: &Path) -> WorktreeDiff {
    let toplevel = worktree_toplevel(worktree_dir);
    let worktree_dir = toplevel.as_path();
    log::debug!("git: compute_head_diff dir={}", worktree_dir.display());
    // HEAD's commit SHA, or the empty tree when HEAD is unborn (no commits yet).
    let base = match run_git(worktree_dir, &["rev-parse", "--verify", "HEAD"]) {
        Ok(out) => String::from_utf8_lossy(&out).trim().to_string(),
        Err(_) => EMPTY_TREE_SHA.to_string(),
    };
    compute_diff_against(worktree_dir, &base)
}

/// Shared core of [`compute_worktree_diff`] and [`compute_head_diff`]: diff the
/// working tree against the already-resolved commit-ish `base`. `worktree_dir`
/// must already be the worktree toplevel (both callers resolve it first).
/// Oversized / lockfile / over-count files are shown as stubs rather than
/// loaded, bounding peak RAM.
fn compute_diff_against(worktree_dir: &Path, base: &str) -> WorktreeDiff {
    let name_status = match run_git(
        worktree_dir,
        // `-M` enables rename detection so a moved file reads as one `R` record
        // (old → new) instead of a delete + add pair - de-noises task-branch diffs.
        &["diff", "--name-status", "-M", "-z", "--no-color", base],
    ) {
        Ok(out) => out,
        Err(e) => {
            log::warn!("git: name-status failed: {e}");
            return WorktreeDiff {
                files: Vec::new(),
                error: Some(e),
            };
        }
    };

    let mut changes = parse_name_status_z(&name_status);
    let mut truncated = changes.len() > MAX_FILE_COUNT;
    if changes.len() > MAX_FILE_COUNT + 1 {
        changes.truncate(MAX_FILE_COUNT + 1);
    }
    // Tracked changes (above) miss untracked new files; append them as Added so
    // a freshly-created file on the branch shows up (loaded from the working
    // tree, empty base → rendered as a pure addition).
    if changes.len() <= MAX_FILE_COUNT {
        let remaining = MAX_FILE_COUNT + 1 - changes.len();
        let (untracked, untracked_truncated) = list_untracked_limited(worktree_dir, remaining);
        truncated |= untracked_truncated;
        for path in untracked {
            changes.push((FileChange::Added, path, None));
        }
    }
    log::debug!("git: {} changed files", changes.len());
    let mut files = Vec::new();
    for (change, path, old_path) in changes {
        if files.len() >= MAX_FILE_COUNT {
            truncated = true;
            break;
        }
        // Skip lockfiles and oversized files: emit a stub, never load/diff/
        // highlight them. This is the primary OOM guard.
        if is_skipped_name(&path) || is_too_large(worktree_dir, &path) {
            log::debug!("git: skip (lockfile/large) {path}");
            files.push(stub_file(path, change));
            continue;
        }
        log::debug!("git: load {path}");
        // For a rename, the base text lives at the pre-rename path.
        let base_lookup = match (change, &old_path) {
            (FileChange::Renamed, Some(src)) => src.as_str(),
            _ => path.as_str(),
        };
        let (base_text, base_bin) = match change {
            FileChange::Added => (String::new(), false),
            _ => load_base_text(worktree_dir, base, base_lookup),
        };
        let (new_text, new_bin) = match change {
            FileChange::Deleted => (String::new(), false),
            _ => load_working_text(worktree_dir, &path),
        };
        // Post-load size guard. `is_too_large` only sees the working-tree side
        // via metadata, so a file that is huge at the merge-base but small or
        // deleted now (a bulk rewrite / deletion commit) would otherwise load
        // its full base blob into a retained `FileDiff` - unbounded across the N
        // columns. Stub it instead, bounding retained RAM symmetrically.
        if base_text.len() as u64 > MAX_FILE_BYTES || new_text.len() as u64 > MAX_FILE_BYTES {
            log::debug!("git: skip (oversized post-load) {path}");
            files.push(stub_file(path, change));
            continue;
        }
        let is_binary = base_bin || new_bin;
        let hunks = if is_binary {
            Vec::new()
        } else {
            compute_hunks(&base_text, &new_text)
        };
        files.push(FileDiff {
            path,
            change,
            old_path,
            base_text,
            new_text,
            hunks,
            is_binary,
        });
    }

    if truncated {
        // Visible notice, not a silent cap (NFR). Rendered as a stub row.
        files.push(stub_file(
            format!("… more files not shown (truncated at {MAX_FILE_COUNT})"),
            FileChange::Modified,
        ));
    }

    WorktreeDiff { files, error: None }
}

fn compute_file_stats_against(worktree_dir: &Path, base: &str) -> HashMap<String, FileDiffStat> {
    let mut stats = run_git(
        worktree_dir,
        &["diff", "--numstat", "-z", "--no-color", base, "--"],
    )
    .map(|out| parse_numstat_z(&out))
    .unwrap_or_default();

    let remaining = MAX_FILE_COUNT.saturating_sub(stats.len());
    if remaining == 0 {
        return stats;
    }

    let (untracked, truncated) = list_untracked_limited(worktree_dir, remaining);
    if truncated {
        log::debug!("git: untracked file stats truncated at {remaining}");
    }
    for path in untracked {
        if is_skipped_name(&path) || is_too_large(worktree_dir, &path) {
            stats.insert(
                path,
                FileDiffStat {
                    added: 0,
                    removed: 0,
                },
            );
            continue;
        }
        let (text, is_binary) = load_working_text(worktree_dir, &path);
        let added = if is_binary {
            0
        } else {
            u32::try_from(text.lines().count()).unwrap_or(u32::MAX)
        };
        stats.insert(path, FileDiffStat { added, removed: 0 });
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_worktrees_basic() {
        let raw = "worktree /repo/main\nHEAD abc123\nbranch refs/heads/develop\n\n\
                   worktree /repo/wt-a\nHEAD def456\nbranch refs/heads/feature-a\n";
        let wts = parse_worktrees_from_str(raw, Some(Path::new("/repo/main")));
        assert_eq!(wts.len(), 2);
        assert!(wts[0].is_main);
        assert_eq!(wts[0].ref_name.as_deref(), Some("refs/heads/develop"));
        assert_eq!(wts[0].sha, "abc123");
        assert!(!wts[1].is_main);
        assert_eq!(wts[1].path, PathBuf::from("/repo/wt-a"));
    }

    #[test]
    fn parse_worktrees_detached_and_bare() {
        let raw = "worktree /repo/bare\nbare\n\n\
                   worktree /repo/det\nHEAD aaa111\ndetached\n";
        let wts = parse_worktrees_from_str(raw, None);
        // Bare entry has no HEAD → skipped; detached entry kept with no ref_name.
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].ref_name, None);
        assert_eq!(wts[0].sha, "aaa111");
    }

    #[test]
    fn name_status_z_parsing() {
        // status\0path\0 records; includes an addition, a deletion, a rename.
        let raw = b"M\0src/main.rs\0A\0src/new.rs\0D\0old.rs\0R100\0from.rs\0to.rs\0";
        let parsed = parse_name_status_z(raw);
        assert_eq!(parsed.len(), 4);
        assert_eq!(
            parsed[0],
            (FileChange::Modified, "src/main.rs".to_string(), None)
        );
        assert_eq!(
            parsed[1],
            (FileChange::Added, "src/new.rs".to_string(), None)
        );
        assert_eq!(parsed[2], (FileChange::Deleted, "old.rs".to_string(), None));
        // Rename keys on destination, keeping the source as old_path.
        assert_eq!(
            parsed[3],
            (
                FileChange::Renamed,
                "to.rs".to_string(),
                Some("from.rs".to_string())
            )
        );
    }

    #[test]
    fn name_status_z_skips_non_utf8_paths() {
        let raw = b"M\0src/\xff.rs\0A\0src/ok.rs\0";
        let parsed = parse_name_status_z(raw);
        assert_eq!(
            parsed,
            vec![(FileChange::Added, "src/ok.rs".to_string(), None)]
        );
    }

    #[test]
    fn classify_binary_and_text() {
        assert_eq!(
            classify(b"hello\n".to_vec()),
            ("hello\n".to_string(), false)
        );
        assert_eq!(
            classify(b"hello\r\nworld\r\n".to_vec()),
            ("hello\nworld\n".to_string(), false)
        );
        let (_, bin) = classify(vec![0x00, 0x01, 0x02]);
        assert!(bin);
    }

    #[test]
    fn numstat_z_parsing() {
        let raw = b"3\t1\tsrc/main.rs\0-\t-\timage.png\0";
        let parsed = parse_numstat_z(raw);
        assert_eq!(
            parsed.get("src/main.rs"),
            Some(&FileDiffStat {
                added: 3,
                removed: 1
            })
        );
        assert_eq!(
            parsed.get("image.png"),
            Some(&FileDiffStat {
                added: 0,
                removed: 0
            })
        );
    }

    #[test]
    fn numstat_z_renames_key_on_destination() {
        let raw = b"2\t1\t\0src/old.rs\0src/new.rs\0";
        let parsed = parse_numstat_z(raw);
        assert_eq!(
            parsed.get("src/new.rs"),
            Some(&FileDiffStat {
                added: 2,
                removed: 1
            })
        );
        assert!(!parsed.contains_key("src/old.rs"));
    }

    #[test]
    fn numstat_z_skips_non_utf8_paths() {
        let raw = b"1\t0\tsrc/\xff.rs\02\t0\tsrc/ok.rs\0";
        let parsed = parse_numstat_z(raw);
        assert_eq!(
            parsed.get("src/ok.rs"),
            Some(&FileDiffStat {
                added: 2,
                removed: 0
            })
        );
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn worktree_file_stats_count_tracked_and_untracked() {
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
        std::fs::write(root.join("untracked.txt"), "alpha\nbeta\n").unwrap();

        let stats = compute_worktree_file_stats(root, "HEAD");
        assert_eq!(
            stats.get("tracked.txt"),
            Some(&FileDiffStat {
                added: 1,
                removed: 0
            })
        );
        assert_eq!(
            stats.get("untracked.txt"),
            Some(&FileDiffStat {
                added: 2,
                removed: 0
            })
        );
    }

    #[test]
    fn list_untracked_limited_reports_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !test_git(root, &["init"]) {
            return;
        }
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        std::fs::write(root.join("c.txt"), "c\n").unwrap();

        let (paths, truncated) = list_untracked_limited(root, 2);
        assert_eq!(paths.len(), 2);
        assert!(truncated);
    }

    #[test]
    fn line_counts_sums_hunks() {
        use super::super::engine::DiffHunkStatus;
        let fd = FileDiff {
            path: "x".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: String::new(),
            new_text: String::new(),
            hunks: vec![
                DiffHunk {
                    base_row_range: 0..1,
                    new_row_range: 0..2,
                    status: DiffHunkStatus::Modified,
                },
                DiffHunk {
                    base_row_range: 5..5,
                    new_row_range: 9..12,
                    status: DiffHunkStatus::Added,
                },
            ],
            is_binary: false,
        };
        assert_eq!(fd.line_counts(), (5, 1));
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
