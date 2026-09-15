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
            log::debug!("diff: {path}: skipped (lockfile or too large), no inline change runs");
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
            log::debug!("diff: {path}: binary content, no inline change runs");
            Vec::new()
        } else {
            let report = compute_hunk_report(&base_text, &new_text, options);
            if report.too_big_blocks > 0 {
                log::debug!(
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
