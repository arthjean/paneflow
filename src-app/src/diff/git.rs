use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::engine::{DiffHunk, DiffOptions, compute_hunk_report};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileChange {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
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
        super::engine::hunk_line_counts(&self.hunks)
    }
}

#[derive(Clone, Debug, Default)]
pub struct WorktreeDiff {
    pub files: Vec<FileDiff>,
    pub error: Option<String>,
    pub toplevel: Option<PathBuf>,
    pub head_sha: Option<String>,
    pub working_metadata: HashMap<String, std::fs::Metadata>,
}

const GIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

const GIT_STDOUT_CAP: u64 = 16 * 1024 * 1024;

fn run_git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = crate::git_command::git(crate::git_command::GitProfile::Probe, args);
    cmd.current_dir(dir);
    let output = crate::git_command::run(cmd, GIT_DEADLINE, GIT_STDOUT_CAP).map_err(|e| {
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

#[derive(Debug, PartialEq, Eq)]
enum BaseBlob {
    Loaded { text: String, binary: bool },
    TooLarge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BaseEntry {
    Missing,
    NotABlob,
    Blob { oid: String, size: u64 },
}

const BASE_BATCH_HEADER_SLACK: u64 = 256;

fn feed_git(dir: &Path, args: &[&str], stdin: Vec<u8>, stdout_cap: u64) -> Option<Vec<u8>> {
    let mut cmd = crate::git_command::git(crate::git_command::GitProfile::Probe, args);
    cmd.current_dir(dir);
    let (output, truncated) = crate::git_command::run_feeding_stdin_keeping_stdout_head(
        cmd,
        stdin,
        GIT_DEADLINE,
        stdout_cap,
    )
    .ok()?;
    (output.status.success() && !truncated).then_some(output.stdout)
}

fn parse_base_entries(stdout: &[u8], expected: usize) -> Option<Vec<BaseEntry>> {
    let entries: Vec<BaseEntry> = stdout
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let line = String::from_utf8_lossy(line);
            let mut fields = line.rsplitn(3, ' ');
            let size = fields.next().and_then(|size| size.parse::<u64>().ok());
            let kind = fields.next();
            let oid = fields.next();
            match (oid, kind, size) {
                (Some(oid), Some("blob"), Some(size)) => BaseEntry::Blob {
                    oid: oid.to_string(),
                    size,
                },
                (Some(_), Some(_), Some(_)) => BaseEntry::NotABlob,
                _ => BaseEntry::Missing,
            }
        })
        .collect();
    (entries.len() == expected).then_some(entries)
}

fn parse_base_contents(mut rest: &[u8], expected: &[u64]) -> Option<Vec<Vec<u8>>> {
    let mut contents = Vec::with_capacity(expected.len());
    for &size in expected {
        let eol = rest.iter().position(|&byte| byte == b'\n')?;
        let header = std::str::from_utf8(&rest[..eol]).ok()?;
        let announced = header.rsplit(' ').next()?.parse::<u64>().ok()?;
        if announced != size {
            return None;
        }
        let size = usize::try_from(size).ok()?;
        rest = &rest[eol + 1..];
        if rest.len() <= size {
            return None;
        }
        contents.push(rest[..size].to_vec());
        rest = &rest[size + 1..];
    }
    Some(contents)
}

fn load_base_blobs(worktree_dir: &Path, base: &str, paths: &[&str]) -> Vec<Option<BaseBlob>> {
    let unresolved = || paths.iter().map(|_| None).collect::<Vec<_>>();
    if paths.is_empty() || paths.iter().any(|path| path.contains('\n')) {
        return unresolved();
    }
    let specs: Vec<u8> = paths
        .iter()
        .flat_map(|path| format!("{base}:{path}\n").into_bytes())
        .collect();
    let listing_cap = specs.len() as u64 + paths.len() as u64 * BASE_BATCH_HEADER_SLACK;
    let Some(entries) = feed_git(
        worktree_dir,
        &["cat-file", "--batch-check"],
        specs,
        listing_cap,
    )
    .and_then(|stdout| parse_base_entries(&stdout, paths.len())) else {
        return unresolved();
    };

    let wanted: Vec<(&str, u64)> = entries
        .iter()
        .filter_map(|entry| match entry {
            BaseEntry::Blob { oid, size } if *size <= MAX_FILE_BYTES => Some((oid.as_str(), *size)),
            _ => None,
        })
        .collect();
    let sizes: Vec<u64> = wanted.iter().map(|(_, size)| *size).collect();
    let contents = if wanted.is_empty() {
        Some(Vec::new())
    } else {
        let oids: Vec<u8> = wanted
            .iter()
            .flat_map(|(oid, _)| format!("{oid}\n").into_bytes())
            .collect();
        let contents_cap = sizes.iter().sum::<u64>() + sizes.len() as u64 * BASE_BATCH_HEADER_SLACK;
        feed_git(worktree_dir, &["cat-file", "--batch"], oids, contents_cap)
            .and_then(|stdout| parse_base_contents(&stdout, &sizes))
    };
    let Some(contents) = contents else {
        return unresolved();
    };

    let mut contents = contents.into_iter();
    entries
        .into_iter()
        .map(|entry| match entry {
            BaseEntry::Missing => Some(BaseBlob::Loaded {
                text: String::new(),
                binary: false,
            }),
            BaseEntry::NotABlob => Some(BaseBlob::Loaded {
                text: String::new(),
                binary: true,
            }),
            BaseEntry::Blob { size, .. } if size > MAX_FILE_BYTES => Some(BaseBlob::TooLarge),
            BaseEntry::Blob { .. } => contents.next().map(|bytes| {
                let (text, binary) = classify(bytes);
                BaseBlob::Loaded { text, binary }
            }),
        })
        .collect()
}

struct WorkingText {
    text: String,
    binary: bool,
    metadata: Option<std::fs::Metadata>,
}

impl WorkingText {
    fn plain(text: String, binary: bool) -> Self {
        Self {
            text,
            binary,
            metadata: None,
        }
    }
}

const WORKING_FILE_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

type FileRead = std::io::Result<(Vec<u8>, std::fs::Metadata)>;

fn read_with_metadata(path: &Path) -> FileRead {
    let (file, metadata) = paneflow_home::open_regular_for_reading(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    Ok((bytes, metadata))
}

fn read_within(path: &Path, timeout: std::time::Duration, read: fn(&Path) -> FileRead) -> FileRead {
    let owned = path.to_path_buf();
    crate::fs_probe::run_bounded(timeout, move || read(&owned)).unwrap_or_else(|| {
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("no answer within {} s", timeout.as_secs()),
        ))
    })
}

fn load_working_text(worktree_dir: &Path, rel_path: &str) -> WorkingText {
    let path = worktree_dir.join(rel_path);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(&path)
                .map(|t| t.to_string_lossy().into_owned())
                .unwrap_or_default();
            WorkingText::plain(target, false)
        }
        Ok(_) => match read_within(&path, WORKING_FILE_READ_TIMEOUT, read_with_metadata) {
            Ok((bytes, metadata)) => {
                let (text, binary) = classify(bytes);
                WorkingText {
                    text,
                    binary,
                    metadata: Some(metadata),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                WorkingText::plain(String::new(), false)
            }
            Err(e) => {
                log::warn!("git: failed to read working-tree file {rel_path}: {e}");
                WorkingText::plain(String::new(), true)
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            WorkingText::plain(String::new(), false)
        }
        Err(e) => {
            log::warn!("git: failed to lstat working-tree file {rel_path}: {e}");
            WorkingText::plain(String::new(), true)
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
            'T' => FileChange::TypeChanged,
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
    std::fs::symlink_metadata(worktree_dir.join(rel_path))
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

pub(crate) const NOT_A_GIT_REPOSITORY: &str = "Not a Git repository";

pub fn compute_head_diff(worktree_dir: &Path, options: DiffOptions) -> WorktreeDiff {
    let failed = |error: String| WorktreeDiff {
        error: Some(error),
        ..Default::default()
    };
    let toplevel = match try_worktree_toplevel(worktree_dir) {
        Ok(Some(toplevel)) => toplevel,
        Ok(None) => return failed(NOT_A_GIT_REPOSITORY.to_string()),
        Err(error) => return failed(error),
    };
    let worktree_dir = toplevel.as_path();
    log::debug!("git: compute_head_diff dir={}", worktree_dir.display());
    let head = match crate::workspace::git::resolve_head(worktree_dir, GIT_DEADLINE) {
        Ok(head) => head,
        Err(error) => return failed(error),
    };
    let base = head.clone().unwrap_or_else(|| EMPTY_TREE_SHA.to_string());
    let mut diff = compute_diff_against(worktree_dir, &base, options);
    diff.toplevel = Some(toplevel);
    diff.head_sha = head;
    diff
}

fn base_lookup_path<'a>(
    change: FileChange,
    path: &'a str,
    old_path: &'a Option<String>,
) -> &'a str {
    match (change, old_path) {
        (FileChange::Renamed, Some(source)) => source,
        _ => path,
    }
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
    if changes.len() > MAX_FILE_COUNT {
        truncated = true;
        changes.truncate(MAX_FILE_COUNT);
    }
    let skipped: Vec<bool> = changes
        .iter()
        .map(|(_, path, _)| is_skipped_name(path) || is_too_large(worktree_dir, path))
        .collect();
    let base_paths: Vec<&str> = changes
        .iter()
        .zip(&skipped)
        .filter(|((change, _, _), skipped)| !**skipped && *change != FileChange::Added)
        .map(|((change, path, old_path), _)| base_lookup_path(*change, path, old_path))
        .collect();
    let mut base_blobs = load_base_blobs(worktree_dir, base, &base_paths).into_iter();

    let mut files = Vec::new();
    let mut working_metadata = HashMap::new();
    for ((change, path, old_path), skipped) in changes.into_iter().zip(skipped) {
        if skipped {
            log::debug!("diff: {path}: skipped (lockfile or too large), no inline change runs");
            files.push(stub_file(path, change));
            continue;
        }
        log::debug!("git: load {path}");
        let (base_text, base_bin) = match change {
            FileChange::Added => (String::new(), false),
            _ => match base_blobs.next().flatten() {
                Some(BaseBlob::Loaded { text, binary }) => (text, binary),
                Some(BaseBlob::TooLarge) => {
                    log::debug!("git: skip (oversized base) {path}");
                    files.push(stub_file(path, change));
                    continue;
                }
                None => load_base_text(
                    worktree_dir,
                    base,
                    base_lookup_path(change, &path, &old_path),
                ),
            },
        };
        let working = match change {
            FileChange::Deleted => WorkingText::plain(String::new(), false),
            _ => load_working_text(worktree_dir, &path),
        };
        let (new_text, new_bin) = (working.text, working.binary);
        if let Some(metadata) = working.metadata {
            working_metadata.insert(path.clone(), metadata);
        }
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
        working_metadata,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture as fixture;

    #[cfg(unix)]
    #[test]
    fn a_tracked_file_replaced_by_a_fifo_reads_as_unreadable_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(dir.path().join("pipe"))
            .status()
            .is_ok_and(|status| status.success());
        assert!(made);
        let started = std::time::Instant::now();
        let working = load_working_text(dir.path(), "pipe");
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        assert!(working.binary);
        assert!(working.text.is_empty());
    }

    #[test]
    fn a_working_file_read_that_hangs_gives_up_at_its_deadline() {
        let started = std::time::Instant::now();
        let error = read_within(
            Path::new("stuck"),
            std::time::Duration::from_millis(50),
            |_| {
                std::thread::sleep(std::time::Duration::from_secs(30));
                Err(std::io::Error::other("unreachable"))
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(
            WORKING_FILE_READ_TIMEOUT,
            std::time::Duration::from_secs(10)
        );
    }

    #[test]
    fn a_tracked_symlink_is_sized_by_its_link_not_by_the_file_it_points_to() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.bin");
        std::fs::write(&big, vec![b'x'; MAX_FILE_BYTES as usize + 1]).unwrap();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&big, dir.path().join("link")).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&big, dir.path().join("link")).is_ok();
        if !linked {
            return;
        }
        assert!(is_too_large(dir.path(), "big.bin"));
        assert!(!is_too_large(dir.path(), "link"));
    }

    #[test]
    fn a_folder_outside_any_repository_reads_as_not_a_git_repository() {
        let dir = tempfile::tempdir().unwrap();
        let diff = compute_head_diff(dir.path(), DiffOptions::default());
        assert_eq!(diff.error.as_deref(), Some(NOT_A_GIT_REPOSITORY));
    }

    #[test]
    fn name_status_z_parsing() {
        let raw = b"M\0src/main.rs\0A\0src/new.rs\0D\0old.rs\0R100\0from.rs\0to.rs\0T\0link\0";
        let parsed = parse_name_status_z(raw);
        assert_eq!(parsed.len(), 5);
        assert_eq!(
            parsed[4],
            (FileChange::TypeChanged, "link".to_string(), None)
        );
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
        fixture::init(root);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        std::fs::write(root.join("c.txt"), "c\n").unwrap();

        let (paths, truncated) = list_untracked_limited(root, 2);
        assert_eq!(paths.len(), 2);
        assert!(truncated);
    }

    fn spawned(subcommand: &str) -> u64 {
        crate::work_counters::counted_on_this_thread(crate::work_counters::git_subcommand_counter(
            Some(subcommand),
        ))
    }

    #[test]
    fn every_base_text_comes_from_two_cat_file_processes_whatever_the_file_count() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let names: Vec<String> = (0..12).map(|index| format!("file{index}.txt")).collect();
        let committed: Vec<(&str, &str)> = names
            .iter()
            .map(|name| (name.as_str(), "before\n"))
            .collect();
        fixture::committed_repo(root, &committed);
        for name in &names {
            std::fs::write(root.join(name), "after\n").unwrap();
        }

        let (shows, cat_files) = (spawned("show"), spawned("cat-file"));
        let diff = compute_head_diff(root, DiffOptions::default());

        assert_eq!(diff.error, None);
        assert_eq!(diff.files.len(), names.len());
        for file in &diff.files {
            assert_eq!(file.base_text, "before\n", "{}", file.path);
            assert_eq!(file.new_text, "after\n", "{}", file.path);
            assert!(!file.hunks.is_empty(), "{}", file.path);
        }
        assert_eq!(spawned("show") - shows, 0);
        assert_eq!(spawned("cat-file") - cat_files, 2);
    }

    #[test]
    fn the_batch_resolves_deleted_renamed_binary_and_oversized_bases() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let oversized = "a\n".repeat(MAX_FILE_BYTES as usize / 2 + 1);
        fixture::committed_repo(
            root,
            &[
                ("plain name.txt", "one\n"),
                ("gone.txt", "soon deleted\n"),
                ("bin.dat", "\0\u{1}binary"),
                ("big.txt", &oversized),
                (
                    "old.txt",
                    "moved line one\nmoved line two\nmoved line three\n",
                ),
            ],
        );
        std::fs::write(root.join("plain name.txt"), "two\n").unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        std::fs::write(root.join("bin.dat"), "now text\n").unwrap();
        std::fs::write(root.join("big.txt"), "small\n").unwrap();
        fixture::run(root, &["mv", "old.txt", "new.txt"]);

        let shows = spawned("show");
        let diff = compute_head_diff(root, DiffOptions::default());
        assert_eq!(diff.error, None);
        assert_eq!(spawned("show") - shows, 0);
        let file = |path: &str| {
            diff.files
                .iter()
                .find(|file| file.path == path)
                .unwrap_or_else(|| panic!("{path} is missing from the diff"))
        };

        assert_eq!(file("plain name.txt").base_text, "one\n");
        assert_eq!(file("plain name.txt").new_text, "two\n");
        assert_eq!(file("gone.txt").change, FileChange::Deleted);
        assert_eq!(file("gone.txt").base_text, "soon deleted\n");
        assert!(file("bin.dat").is_binary);
        assert!(file("bin.dat").hunks.is_empty());
        assert!(file("big.txt").is_binary);
        assert!(file("big.txt").base_text.is_empty());
        assert_eq!(file("new.txt").change, FileChange::Renamed);
        assert_eq!(file("new.txt").old_path.as_deref(), Some("old.txt"));
        assert_eq!(
            file("new.txt").base_text,
            "moved line one\nmoved line two\nmoved line three\n"
        );
    }

    #[test]
    fn a_batch_listing_tells_blobs_from_missing_paths_and_other_objects() {
        let listing =
            b"1111 blob 12\nHEAD:gone file.txt missing\n2222 tree 40\nHEAD:x blob 9 missing\n";
        assert_eq!(
            parse_base_entries(listing, 4),
            Some(vec![
                BaseEntry::Blob {
                    oid: "1111".to_string(),
                    size: 12
                },
                BaseEntry::Missing,
                BaseEntry::NotABlob,
                BaseEntry::Missing,
            ])
        );
        assert_eq!(parse_base_entries(listing, 5), None);
    }

    #[test]
    fn batch_contents_are_refused_when_a_size_or_the_tail_does_not_match() {
        let stdout = b"1111 blob 4\nab\nc\n2222 blob 0\n\n";
        assert_eq!(
            parse_base_contents(stdout, &[4, 0]),
            Some(vec![b"ab\nc".to_vec(), Vec::new()])
        );
        assert_eq!(parse_base_contents(stdout, &[5, 0]), None);
        assert_eq!(parse_base_contents(b"1111 blob 4\nab", &[4]), None);
        assert_eq!(parse_base_contents(stdout, &[4, 0, 1]), None);
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
}
