use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::engine::{DiffHunk, DiffOptions, compute_hunk_report};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileChange {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Clone, Debug)]
pub struct FileDiff {
    pub path: String,
    pub change: FileChange,
    pub old_path: Option<String>,
    pub base_text: String,
    pub new_text: String,
    pub hunks: Vec<DiffHunk>,
    pub is_binary: bool,
}

impl FileDiff {
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

#[derive(Clone, Debug, Default)]
pub struct WorktreeDiff {
    pub files: Vec<FileDiff>,
    pub error: Option<String>,
    pub toplevel: Option<PathBuf>,
    pub head_sha: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileDiffStat {
    pub added: u32,
    pub removed: u32,
}

const GIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

const GIT_STDOUT_CAP: u64 = 16 * 1024 * 1024;

fn run_git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0");
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

pub fn ref_exists(worktree_dir: &Path, ref_name: &str) -> bool {
    run_git(
        worktree_dir,
        &["rev-parse", "--verify", "--quiet", ref_name],
    )
    .is_ok()
}

pub fn default_base_ref(worktree_dir: &Path) -> Option<String> {
    if ref_exists(worktree_dir, "develop") {
        return Some("develop".to_string());
    }
    if let Some(remote_head) = default_origin_head(worktree_dir) {
        return Some(remote_head);
    }
    for candidate in [
        "main",
        "master",
        "origin/develop",
        "origin/main",
        "origin/master",
    ] {
        if ref_exists(worktree_dir, candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

fn default_origin_head(worktree_dir: &Path) -> Option<String> {
    let out = run_git(
        worktree_dir,
        &["rev-parse", "--abbrev-ref", "refs/remotes/origin/HEAD"],
    )
    .ok()?;
    let branch = String::from_utf8_lossy(&out).trim().to_string();
    (!branch.is_empty() && branch != "origin/HEAD" && ref_exists(worktree_dir, &branch))
        .then_some(branch)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnFingerprint {
    head: String,
    base: String,
    diff_hash: u64,
    untracked_hash: u64,
}

pub fn column_fingerprint(worktree_dir: &Path, base_ref: &str) -> ColumnFingerprint {
    let toplevel = worktree_toplevel(worktree_dir);
    let worktree_dir = toplevel.as_path();
    let rev = |r: &str| {
        run_git(worktree_dir, &["rev-parse", r])
            .ok()
            .map(|o| String::from_utf8_lossy(&o).trim().to_string())
            .unwrap_or_default()
    };
    let merge_base = merge_base(worktree_dir, base_ref).unwrap_or_default();
    let diff_hash = if merge_base.is_empty() {
        0
    } else {
        run_git(
            worktree_dir,
            &["diff", "--binary", "--no-color", &merge_base, "--"],
        )
        .ok()
        .map(|out| hash_bytes(&out))
        .unwrap_or(0)
    };
    ColumnFingerprint {
        head: rev("HEAD"),
        base: rev(base_ref),
        diff_hash,
        untracked_hash: hash_untracked_inputs(worktree_dir),
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::Hasher as _;

    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(bytes);
    h.finish()
}

fn hash_untracked_inputs(worktree_dir: &Path) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    use std::io::Read as _;

    let mut h = std::collections::hash_map::DefaultHasher::new();
    let (paths, truncated) = list_untracked_limited(worktree_dir, MAX_FILE_COUNT + 1);
    truncated.hash(&mut h);
    for path in paths {
        path.hash(&mut h);
        if is_skipped_name(&path) || is_too_large(worktree_dir, &path) {
            "stub".hash(&mut h);
            continue;
        }
        let abs = worktree_dir.join(&path);
        match std::fs::symlink_metadata(&abs) {
            Ok(meta) if meta.file_type().is_symlink() => {
                "symlink".hash(&mut h);
                if let Ok(target) = std::fs::read_link(&abs) {
                    target.to_string_lossy().hash(&mut h);
                }
            }
            Ok(_) => match std::fs::File::open(&abs) {
                Ok(file) => {
                    let mut bytes = Vec::new();
                    let read_ok = file
                        .take(MAX_FILE_BYTES + 1)
                        .read_to_end(&mut bytes)
                        .is_ok();
                    read_ok.hash(&mut h);
                    (bytes.len() as u64 > MAX_FILE_BYTES).hash(&mut h);
                    h.write(&bytes);
                }
                Err(err) => {
                    err.kind().hash(&mut h);
                }
            },
            Err(err) => {
                err.kind().hash(&mut h);
            }
        }
    }
    h.finish()
}

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

pub(crate) fn worktree_toplevel(dir: &Path) -> PathBuf {
    try_worktree_toplevel(dir)
        .ok()
        .flatten()
        .unwrap_or_else(|| dir.to_path_buf())
}

pub(crate) fn try_worktree_toplevel(dir: &Path) -> Result<Option<PathBuf>, String> {
    match run_git(dir, &["rev-parse", "--show-toplevel"]) {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out).trim().to_string();
            Ok((!s.is_empty()).then(|| PathBuf::from(s)))
        }
        Err(err) if err.contains("not a git repository") => Ok(None),
        Err(err) => Err(err),
    }
}

pub(crate) fn head_sha(worktree_dir: &Path) -> Option<String> {
    run_git(worktree_dir, &["rev-parse", "--verify", "HEAD"])
        .ok()
        .map(|out| String::from_utf8_lossy(&out).trim().to_string())
        .filter(|sha| !sha.is_empty())
}

pub(crate) enum HeadFile {
    Content(Vec<u8>),
    Missing,
}

pub(crate) fn show_head_file(worktree_dir: &Path, rel_path: &str) -> Result<HeadFile, String> {
    let spec = format!("HEAD:{rel_path}");
    match run_git(worktree_dir, &["show", &spec]) {
        Ok(bytes) => Ok(HeadFile::Content(bytes)),
        Err(show_err) => match base_path_exists(worktree_dir, "HEAD", rel_path) {
            Ok(false) => Ok(HeadFile::Missing),
            Ok(true) => Err(show_err),
            Err(exists_err) if exists_err.contains("Needed a single revision") => {
                Ok(HeadFile::Missing)
            }
            Err(exists_err) => Err(format!("{show_err}; {exists_err}")),
        },
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

fn merge_base(worktree_dir: &Path, base_ref: &str) -> Result<String, String> {
    let out = run_git(worktree_dir, &["merge-base", "HEAD", base_ref])?;
    let sha = String::from_utf8_lossy(&out).trim().to_string();
    if sha.is_empty() {
        return Err(format!("no common ancestor with '{base_ref}'"));
    }
    Ok(sha)
}

fn normalize_git_text(text: String) -> String {
    if text.as_bytes().contains(&b'\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text
    }
}

pub(crate) fn classify(bytes: Vec<u8>) -> (String, bool) {
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

fn load_working_text(worktree_dir: &Path, rel_path: &str) -> (String, bool) {
    let path = worktree_dir.join(rel_path);
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

fn parse_name_status_z(stdout: &[u8]) -> Vec<(FileChange, String, Option<String>)> {
    let mut fields = stdout.split(|&b| b == 0).filter(|f| !f.is_empty());
    let mut out = Vec::new();
    while let Some(status) = fields.next() {
        let code = status.first().copied().unwrap_or(b'M') as char;
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
            _ => FileChange::Modified,
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

pub(crate) const MAX_FILE_BYTES: u64 = 512 * 1024;

const MAX_FILE_COUNT: usize = 200;

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

pub fn is_skipped_name(path: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| SKIP_FILENAMES.contains(&n))
}

fn is_too_large(worktree_dir: &Path, rel_path: &str) -> bool {
    std::fs::metadata(worktree_dir.join(rel_path))
        .map(|m| m.len() > MAX_FILE_BYTES)
        .unwrap_or(false)
}

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

pub fn compute_worktree_diff(
    worktree_dir: &Path,
    base_ref: &str,
    options: DiffOptions,
) -> WorktreeDiff {
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
                ..Default::default()
            };
        }
    };
    log::debug!("git: merge_base={merge_base}");

    compute_diff_against(worktree_dir, &merge_base, options)
}

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

const EMPTY_TREE_SHA: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

pub fn compute_head_diff(worktree_dir: &Path, options: DiffOptions) -> WorktreeDiff {
    let toplevel = worktree_toplevel(worktree_dir);
    let worktree_dir = toplevel.as_path();
    log::debug!("git: compute_head_diff dir={}", worktree_dir.display());
    let head = run_git(worktree_dir, &["rev-parse", "--verify", "HEAD"])
        .ok()
        .map(|out| String::from_utf8_lossy(&out).trim().to_string())
        .filter(|sha| !sha.is_empty());
    let base = head.clone().unwrap_or_else(|| EMPTY_TREE_SHA.to_string());
    let mut diff = compute_diff_against(worktree_dir, &base, options);
    diff.toplevel = Some(toplevel);
    diff.head_sha = head;
    diff
}

fn compute_diff_against(worktree_dir: &Path, base: &str, options: DiffOptions) -> WorktreeDiff {
    let name_status = match run_git(
        worktree_dir,
        &["diff", "--name-status", "-M", "-z", "--no-color", base],
    ) {
        Ok(out) => out,
        Err(e) => {
            log::warn!("git: name-status failed: {e}");
            return WorktreeDiff {
                files: Vec::new(),
                error: Some(e),
                ..Default::default()
            };
        }
    };

    let mut changes = parse_name_status_z(&name_status);
    let mut truncated = changes.len() > MAX_FILE_COUNT;
    if changes.len() > MAX_FILE_COUNT + 1 {
        changes.truncate(MAX_FILE_COUNT + 1);
    }
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
        if is_skipped_name(&path) || is_too_large(worktree_dir, &path) {
            log::warn!("diff: {path}: skipped (lockfile or too large), no inline change runs");
            files.push(stub_file(path, change));
            continue;
        }
        log::debug!("git: load {path}");
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
        if base_text.len() as u64 > MAX_FILE_BYTES || new_text.len() as u64 > MAX_FILE_BYTES {
            log::debug!("git: skip (oversized post-load) {path}");
            files.push(stub_file(path, change));
            continue;
        }
        let is_binary = base_bin || new_bin;
        let hunks = if is_binary {
            log::warn!("diff: {path}: binary content, no inline change runs");
            Vec::new()
        } else {
            let report = compute_hunk_report(&base_text, &new_text, options);
            if report.too_big_blocks > 0 {
                log::warn!(
                    "diff: {path}: {} block(s) too big for word diff, line hunks only",
                    report.too_big_blocks
                );
            }
            report.hunks
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
        files.push(stub_file(
            format!("… more files not shown (truncated at {MAX_FILE_COUNT})"),
            FileChange::Modified,
        ));
    }

    WorktreeDiff {
        files,
        error: None,
        ..Default::default()
    }
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
    fn name_status_z_parsing() {
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
        let raw = b"1\t0\tsrc/\xff.rs\x002\t0\tsrc/ok.rs\0";
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
    fn column_fingerprint_changes_when_modified_file_content_changes() {
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
        let first = column_fingerprint(root, "HEAD");
        std::fs::write(root.join("tracked.txt"), "one\nthree\n").unwrap();
        let second = column_fingerprint(root, "HEAD");

        assert_ne!(first, second);
    }

    #[test]
    fn line_counts_sums_hunks() {
        let fd = FileDiff {
            path: "x".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: String::new(),
            new_text: String::new(),
            hunks: vec![DiffHunk::plain(0..1, 0..2), DiffHunk::plain(5..5, 9..12)],
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
