use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::engine::{DiffHunk, DiffOptions, compute_hunk_report};
use crate::workspace::git::RawChange;

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
    compute_head_diff_through(
        paneflow_git::engine::Selector::global(),
        &paneflow_git::Options::dock_listing(),
        worktree_dir,
        options,
    )
}

fn compute_head_diff_through(
    selector: &paneflow_git::engine::Selector,
    probe: &paneflow_git::Options,
    worktree_dir: &Path,
    options: DiffOptions,
) -> WorktreeDiff {
    selector.run(
        paneflow_git::engine::Probe::DockListing,
        worktree_dir,
        || head_diff_with_gix(worktree_dir, probe, options),
        || head_diff_with_cli(worktree_dir, options),
    )
}

fn failed_diff(error: String) -> WorktreeDiff {
    WorktreeDiff {
        error: Some(error),
        ..Default::default()
    }
}

fn head_diff_with_cli(worktree_dir: &Path, options: DiffOptions) -> WorktreeDiff {
    let toplevel = match try_worktree_toplevel(worktree_dir) {
        Ok(Some(toplevel)) => toplevel,
        Ok(None) => return failed_diff(NOT_A_GIT_REPOSITORY.to_string()),
        Err(error) => return failed_diff(error),
    };
    let worktree_dir = toplevel.as_path();
    log::debug!("git: compute_head_diff dir={}", worktree_dir.display());
    let head = match crate::workspace::git::resolve_head(worktree_dir, GIT_DEADLINE) {
        Ok(head) => head,
        Err(error) => return failed_diff(error),
    };
    let base = head.clone().unwrap_or_else(|| EMPTY_TREE_SHA.to_string());
    let mut diff = compute_diff_against(worktree_dir, &base, options);
    diff.toplevel = Some(toplevel);
    diff.head_sha = head;
    diff
}

fn head_diff_with_gix(
    worktree_dir: &Path,
    probe: &paneflow_git::Options,
    options: DiffOptions,
) -> Result<WorktreeDiff, paneflow_git::Error> {
    let result = paneflow_git::with_snapshot(worktree_dir, probe, |snapshot, objects| {
        let Some(workdir) = objects.workdir() else {
            return Ok(failed_diff(NOT_A_GIT_REPOSITORY.to_string()));
        };
        let toplevel = std::path::absolute(workdir).unwrap_or_else(|_| workdir.to_path_buf());
        log::debug!(
            "git: compute_head_diff in process dir={}",
            toplevel.display()
        );
        let head_sha = objects.head_commit()?;
        let listing = gix_listing(&snapshot, probe.entry_cap, objects, &toplevel)?;
        let base_ids = listing.base_ids;
        let untracked = listing.untracked;
        let mut diff = assemble_diff(
            &toplevel,
            listing.changes,
            |limit| take_limited(untracked, limit),
            |index, path| {
                gix_base_text(
                    objects,
                    base_ids.get(index).and_then(Option::as_deref),
                    path,
                )
            },
            options,
        );
        diff.toplevel = Some(toplevel);
        diff.head_sha = head_sha;
        Ok(diff)
    });
    match result {
        Err(paneflow_git::Error::NotARepository { .. }) => {
            Ok(failed_diff(NOT_A_GIT_REPOSITORY.to_string()))
        }
        other => other,
    }
}

type Listed = (FileChange, String, Option<String>);

struct GixListing {
    changes: Vec<Listed>,
    base_ids: Vec<Option<String>>,
    untracked: Vec<String>,
}

fn change_kind(status: u8) -> FileChange {
    match status {
        b'A' => FileChange::Added,
        b'D' => FileChange::Deleted,
        b'T' => FileChange::TypeChanged,
        _ => FileChange::Modified,
    }
}

fn unmerged_against(
    entry: &paneflow_git::Entry,
    objects: &paneflow_git::Objects<'_>,
    workdir: &Path,
    base_tree: Option<&str>,
) -> Result<Option<RawChange>, paneflow_git::Error> {
    use crate::workspace::git::{null_oid, path_from_git_output, raw_change, stored_side};
    let base = match base_tree {
        Some(tree) => objects.tree_entry(tree, &entry.path)?,
        None => None,
    };
    let base = stored_side(base.as_ref());
    let worktree_mode = crate::workspace::git::worktree_mode_of(
        &workdir.join(path_from_git_output(&entry.path)),
        base.0,
    );
    Ok(raw_change(base, (worktree_mode, null_oid()), &entry.path))
}

fn stat_only_change(sides: &paneflow_git::Sides, path: &[u8]) -> Option<RawChange> {
    use crate::workspace::git::{null_oid, raw_change, stored_side};
    let index_mode = sides.index.as_ref().map_or(0, |stored| stored.mode);
    raw_change(
        stored_side(sides.head.as_ref()),
        (index_mode, null_oid()),
        path,
    )
}

fn tracked_changes(
    snapshot: &paneflow_git::Snapshot,
    objects: &paneflow_git::Objects<'_>,
    workdir: &Path,
) -> Result<Vec<RawChange>, paneflow_git::Error> {
    let base_tree = match &snapshot.head {
        paneflow_git::Head::Tree(tree) => Some(tree.as_str()),
        paneflow_git::Head::Unborn => None,
    };
    let mut changes = Vec::new();
    for entry in &snapshot.entries {
        let sides = || {
            snapshot
                .sides
                .get(&entry.path)
                .ok_or_else(|| paneflow_git::Error::Git {
                    context: "compose the dock listing",
                    message: format!("no object ids for {}", String::from_utf8_lossy(&entry.path)),
                })
        };
        let change = if entry.conflict.is_some() {
            unmerged_against(entry, objects, workdir, base_tree)?
        } else if entry.head_to_index.is_some() || entry.index_to_worktree.is_some() {
            crate::workspace::git::composed_change(entry, sides()?)
        } else if entry.stat_dirty {
            stat_only_change(sides()?, &entry.path)
        } else {
            None
        };
        changes.extend(change);
    }
    Ok(changes)
}

type IndexedSides = Vec<(usize, paneflow_git::renames::RenameSide)>;

fn rename_candidates(tracked: &[RawChange]) -> (IndexedSides, IndexedSides) {
    use crate::workspace::git::is_null_oid;
    use paneflow_git::renames::RenameSide;
    let mut sources = Vec::new();
    let mut destinations = Vec::new();
    for (index, change) in tracked.iter().enumerate() {
        match change.status {
            b'D' => sources.push((
                index,
                RenameSide {
                    path: change.path.clone(),
                    mode: change.old_mode,
                    id: Some(change.old_oid.clone()),
                },
            )),
            b'A' => destinations.push((
                index,
                RenameSide {
                    path: change.path.clone(),
                    mode: change.new_mode,
                    id: (!is_null_oid(&change.new_oid)).then(|| change.new_oid.clone()),
                },
            )),
            _ => {}
        }
    }
    (sources, destinations)
}

fn gix_listing(
    snapshot: &paneflow_git::Snapshot,
    entry_cap: usize,
    objects: &paneflow_git::Objects<'_>,
    workdir: &Path,
) -> Result<GixListing, paneflow_git::Error> {
    if snapshot.truncated {
        return Err(paneflow_git::Error::TooManyEntries { cap: entry_cap });
    }
    let tracked = tracked_changes(snapshot, objects, workdir)?;
    let (sources, destinations) = rename_candidates(&tracked);
    let source_sides: Vec<_> = sources.iter().map(|(_, side)| side.clone()).collect();
    let destination_sides: Vec<_> = destinations.iter().map(|(_, side)| side.clone()).collect();
    let renames =
        objects.track_renames(&source_sides, &destination_sides, objects.rename_limit())?;
    let mut renamed_from = HashMap::new();
    let mut moved = std::collections::HashSet::new();
    for rename in renames {
        let source = sources[rename.source].0;
        renamed_from.insert(destinations[rename.destination].0, source);
        moved.insert(source);
    }
    let mut changes = Vec::new();
    let mut base_ids = Vec::new();
    for (index, change) in tracked.iter().enumerate() {
        if moved.contains(&index) {
            continue;
        }
        let Some(path) = decode_git_path(&change.path, "the in-process dock listing") else {
            continue;
        };
        if let Some(&source) = renamed_from.get(&index) {
            let origin = &tracked[source];
            let Some(old_path) = decode_git_path(&origin.path, "the in-process dock listing")
            else {
                continue;
            };
            changes.push((FileChange::Renamed, path, Some(old_path)));
            base_ids.push(Some(origin.old_oid.clone()));
            continue;
        }
        changes.push((change_kind(change.status), path, None));
        base_ids.push((change.old_mode != 0).then(|| change.old_oid.clone()));
    }
    let untracked = snapshot
        .entries
        .iter()
        .filter(|entry| entry.untracked)
        .filter_map(|entry| decode_git_path(&entry.path, "the in-process untracked listing"))
        .collect();
    Ok(GixListing {
        changes,
        base_ids,
        untracked,
    })
}

fn take_limited(mut paths: Vec<String>, limit: usize) -> (Vec<String>, bool) {
    if limit == 0 {
        return (Vec::new(), false);
    }
    let truncated = paths.len() > limit;
    paths.truncate(limit);
    (paths, truncated)
}

fn gix_base_text(
    objects: &paneflow_git::Objects<'_>,
    id: Option<&str>,
    rel_path: &str,
) -> (String, bool) {
    let Some(id) = id else {
        return (String::new(), false);
    };
    let read = objects.header(id).and_then(|header| match header {
        Some(header) if header.size > GIT_STDOUT_CAP => Ok(None),
        Some(_) => objects.blob(id),
        None => Ok(None),
    });
    match read {
        Ok(Some(bytes)) => classify(bytes),
        Ok(None) => {
            log::warn!(
                "git: failed to load base-side file {rel_path}: object {id} is missing, too large or not a blob"
            );
            (String::new(), true)
        }
        Err(error) => {
            log::warn!("git: failed to load base-side file {rel_path}: {error}");
            (String::new(), true)
        }
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
    assemble_diff(
        worktree_dir,
        parse_name_status_z(&name_status),
        |limit| list_untracked_limited(worktree_dir, limit),
        |_, lookup| load_base_text(worktree_dir, base, lookup),
        options,
    )
}

fn assemble_diff(
    worktree_dir: &Path,
    mut changes: Vec<Listed>,
    untracked: impl FnOnce(usize) -> (Vec<String>, bool),
    mut base_text: impl FnMut(usize, &str) -> (String, bool),
    options: DiffOptions,
) -> WorktreeDiff {
    let mut truncated = changes.len() > MAX_FILE_COUNT;
    if changes.len() > MAX_FILE_COUNT + 1 {
        changes.truncate(MAX_FILE_COUNT + 1);
    }
    if changes.len() <= MAX_FILE_COUNT {
        let remaining = MAX_FILE_COUNT + 1 - changes.len();
        let (untracked, untracked_truncated) = untracked(remaining);
        truncated |= untracked_truncated;
        for path in untracked {
            changes.push((FileChange::Added, path, None));
        }
    }
    log::debug!("git: {} changed files", changes.len());
    let mut files = Vec::new();
    let mut working_metadata = HashMap::new();
    for (index, (change, path, old_path)) in changes.into_iter().enumerate() {
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
            _ => base_text(index, base_lookup),
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

    type DockView = (
        Option<String>,
        Vec<(
            FileChange,
            String,
            Option<String>,
            String,
            String,
            bool,
            usize,
        )>,
    );

    fn dock_view(diff: &WorktreeDiff) -> DockView {
        (
            diff.error.clone(),
            diff.files
                .iter()
                .map(|file| {
                    (
                        file.change,
                        file.path.clone(),
                        file.old_path.clone(),
                        file.base_text.clone(),
                        file.new_text.clone(),
                        file.is_binary,
                        file.hunks.len(),
                    )
                })
                .collect(),
        )
    }

    fn cli_dock(root: &Path) -> WorktreeDiff {
        head_diff_with_cli(root, DiffOptions::default())
    }

    fn gix_dock(root: &Path) -> WorktreeDiff {
        head_diff_with_gix(
            root,
            &paneflow_git::Options::dock_listing(),
            DiffOptions::default(),
        )
        .expect("gix dock listing")
    }

    fn equal_on_both_engines(root: &Path) -> WorktreeDiff {
        let cli = cli_dock(root);
        let gix = gix_dock(root);
        assert_eq!(listing(&gix), listing(&cli), "gix, then the CLI");
        assert_eq!(dock_view(&gix), dock_view(&cli), "gix, then the CLI");
        assert_eq!(gix.head_sha, cli.head_sha);
        assert_eq!(
            gix.toplevel
                .as_deref()
                .map(std::fs::canonicalize)
                .transpose()
                .ok(),
            cli.toplevel
                .as_deref()
                .map(std::fs::canonicalize)
                .transpose()
                .ok(),
        );
        gix
    }

    fn listing(diff: &WorktreeDiff) -> Vec<(FileChange, &str, Option<&str>)> {
        diff.files
            .iter()
            .map(|file| (file.change, file.path.as_str(), file.old_path.as_deref()))
            .collect()
    }

    fn numbered(tag: &str, count: usize) -> String {
        (0..count)
            .map(|line| format!("{tag} line {line:03} with some ordinary text\n"))
            .collect()
    }

    fn edited(tag: &str, count: usize, kept: usize) -> String {
        let mut text: String = numbered(tag, count)
            .lines()
            .take(kept)
            .map(|line| format!("{line}\n"))
            .collect();
        text.push_str(&numbered(&format!("{tag} replaced"), count - kept));
        text
    }

    fn git_spawns_on_this_thread() -> u64 {
        use crate::work_counters::{counted_on_this_thread, git_profile_counter};
        counted_on_this_thread(git_profile_counter(true))
            + counted_on_this_thread(git_profile_counter(false))
    }

    #[test]
    fn renames_type_changes_and_edits_list_like_git_diff_m() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::committed_repo(
            root,
            &[
                ("pure/old.txt", &numbered("pure", 30)),
                ("above/old.txt", &numbered("above", 40)),
                ("below/old.txt", &numbered("below", 40)),
                ("staged/old.txt", &numbered("staged", 30)),
                ("typed.txt", "a regular file\n"),
                ("edited.txt", &numbered("edited", 10)),
            ],
        );
        std::fs::create_dir_all(root.join("moved")).unwrap();
        std::fs::rename(root.join("pure/old.txt"), root.join("moved/pure.txt")).unwrap();
        std::fs::remove_file(root.join("above/old.txt")).unwrap();
        std::fs::write(root.join("moved/above.txt"), edited("above", 40, 30)).unwrap();
        std::fs::remove_file(root.join("below/old.txt")).unwrap();
        std::fs::write(root.join("moved/below.txt"), edited("below", 40, 12)).unwrap();
        fixture::run(root, &["mv", "staged/old.txt", "moved/staged.txt"]);
        std::fs::write(root.join("moved/staged.txt"), edited("staged", 30, 27)).unwrap();
        std::fs::write(root.join("edited.txt"), edited("edited", 10, 8)).unwrap();
        fixture::run(
            root,
            &["add", "-A", "--", "moved", "pure", "above", "below"],
        );
        fixture::run(root, &["config", "core.symlinks", "true"]);
        std::fs::remove_file(root.join("typed.txt")).unwrap();
        #[cfg(unix)]
        let typed = std::os::unix::fs::symlink("edited.txt", root.join("typed.txt")).is_ok();
        #[cfg(windows)]
        let typed =
            std::os::windows::fs::symlink_file("edited.txt", root.join("typed.txt")).is_ok();
        if !typed {
            std::fs::write(root.join("typed.txt"), "a regular file\nedited\n").unwrap();
        }

        let diff = equal_on_both_engines(root);
        let listed = listing(&diff);
        for expected in [
            (FileChange::Renamed, "moved/pure.txt", Some("pure/old.txt")),
            (
                FileChange::Renamed,
                "moved/above.txt",
                Some("above/old.txt"),
            ),
            (
                FileChange::Renamed,
                "moved/staged.txt",
                Some("staged/old.txt"),
            ),
            (FileChange::Deleted, "below/old.txt", None),
            (FileChange::Added, "moved/below.txt", None),
            (FileChange::Modified, "edited.txt", None),
        ] {
            assert!(listed.contains(&expected), "{expected:?} in {listed:?}");
        }
        if typed {
            assert!(
                listed.contains(&(FileChange::TypeChanged, "typed.txt", None)),
                "{listed:?}"
            );
        } else {
            eprintln!("dock: SKIPPED the type change, symlinks need a privilege here");
        }
        let renamed = diff
            .files
            .iter()
            .find(|file| file.path == "moved/above.txt")
            .unwrap();
        assert_eq!(renamed.base_text, numbered("above", 40));
    }

    #[test]
    fn renames_past_the_rename_limit_are_skipped_like_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let names: Vec<String> = (0..3).map(|index| format!("old/file{index}.txt")).collect();
        let contents: Vec<String> = (0..3)
            .map(|index| numbered(&format!("f{index}"), 20))
            .collect();
        let files: Vec<(&str, &str)> = names
            .iter()
            .map(String::as_str)
            .zip(contents.iter().map(String::as_str))
            .collect();
        fixture::committed_repo(root, &files);
        std::fs::create_dir_all(root.join("new")).unwrap();
        for (index, name) in names.iter().enumerate() {
            std::fs::remove_file(root.join(name)).unwrap();
            std::fs::write(
                root.join(format!("new/moved{index}.txt")),
                edited(&format!("f{index}"), 20, 18),
            )
            .unwrap();
        }
        fixture::run(root, &["add", "-A"]);
        let renamed = |diff: &WorktreeDiff| {
            diff.files
                .iter()
                .filter(|file| file.change == FileChange::Renamed)
                .count()
        };
        assert_eq!(renamed(&equal_on_both_engines(root)), 3);
        fixture::run(root, &["config", "diff.renameLimit", "2"]);
        let limited = equal_on_both_engines(root);
        assert_eq!(renamed(&limited), 0, "{:?}", listing(&limited));
        assert_eq!(limited.files.len(), 6);
    }

    #[test]
    fn a_base_other_than_head_compares_that_commit_with_the_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::committed_repo(
            root,
            &[
                ("first.txt", "first\n"),
                ("moved.txt", &numbered("moved", 20)),
            ],
        );
        let base = fixture::run(root, &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        std::fs::write(root.join("second.txt"), "second\n").unwrap();
        std::fs::write(root.join("first.txt"), "first\nedited\n").unwrap();
        fixture::commit_all(root, "second");
        fixture::run(root, &["mv", "moved.txt", "renamed.txt"]);
        std::fs::write(root.join("untracked.txt"), "u\n").unwrap();

        let cli = compute_diff_against(root, &base, DiffOptions::default());
        let probe = paneflow_git::Options {
            base: Some(base.clone()),
            ..paneflow_git::Options::dock_listing()
        };
        let gix = head_diff_with_gix(root, &probe, DiffOptions::default()).expect("gix");
        assert_eq!(dock_view(&gix), dock_view(&cli));
        assert_eq!(
            listing(&gix),
            vec![
                (FileChange::Modified, "first.txt", None),
                (FileChange::Renamed, "renamed.txt", Some("moved.txt")),
                (FileChange::Added, "second.txt", None),
                (FileChange::Added, "untracked.txt", None),
            ]
        );
        let head = equal_on_both_engines(root);
        assert!(listing(&head).contains(&(FileChange::Renamed, "renamed.txt", Some("moved.txt"))));
        assert!(
            !listing(&head)
                .iter()
                .any(|(_, path, _)| *path == "second.txt")
        );
    }

    #[test]
    fn base_texts_equal_git_show_for_crlf_binary_lockfile_and_missing_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::committed_repo(
            root,
            &[
                ("crlf.txt", "one\r\ntwo\r\n"),
                ("binary.dat", "bin\0ary\n"),
                ("Cargo.lock", "locked\n"),
                ("plain.txt", "plain\n"),
            ],
        );
        let missing = "deadbeef".repeat(5);
        fixture::run(
            root,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{missing},partial.txt"),
            ],
        );
        let tree = fixture::run(root, &["write-tree", "--missing-ok"]);
        let commit = fixture::run(
            root,
            &[
                "-c",
                "user.email=paneflow@example.com",
                "-c",
                "user.name=Paneflow",
                "commit-tree",
                tree.trim(),
                "-p",
                "HEAD",
                "-m",
                "a blob the object database does not hold",
            ],
        );
        fixture::run(root, &["update-ref", "HEAD", commit.trim()]);
        std::fs::write(root.join("partial.txt"), "one\ntwo\n").unwrap();
        std::fs::write(root.join("crlf.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
        std::fs::write(root.join("binary.dat"), "bin\0ary\nmore\n").unwrap();
        std::fs::write(root.join("Cargo.lock"), "locked\nagain\n").unwrap();
        std::fs::write(root.join("plain.txt"), "plain\nedited\n").unwrap();

        let diff = equal_on_both_engines(root);
        let file = |path: &str| diff.files.iter().find(|file| file.path == path).unwrap();
        assert_eq!(file("crlf.txt").base_text, "one\ntwo\n");
        assert!(file("binary.dat").is_binary);
        assert!(file("Cargo.lock").is_binary, "a lockfile is a stub");
        let partial = file("partial.txt");
        assert!(partial.is_binary, "a missing blob reads as unreadable");
        assert!(partial.base_text.is_empty());
        assert_eq!(file("plain.txt").base_text, "plain\n");
    }

    #[test]
    fn a_stat_dirty_file_with_unchanged_content_lists_like_the_cli_probe() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::committed_repo(root, &[("touched.txt", "same\n"), ("other.txt", "o\n")]);
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
        std::fs::File::options()
            .write(true)
            .open(root.join("touched.txt"))
            .and_then(|file| file.set_modified(later))
            .unwrap();
        let diff = equal_on_both_engines(root);
        assert_eq!(
            listing(&diff),
            vec![(FileChange::Modified, "touched.txt", None)]
        );
        assert_eq!(diff.files[0].hunks.len(), 0);
    }

    #[test]
    fn a_refresh_of_two_hundred_files_on_the_gix_path_spawns_no_git_process() {
        use paneflow_git::engine::{Engine, Probe, Selector};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let names: Vec<String> = (0..200)
            .map(|index| format!("src/file{index:03}.txt"))
            .collect();
        let files: Vec<(&str, &str)> = names.iter().map(|name| (name.as_str(), "one\n")).collect();
        fixture::committed_repo(root, &files);
        for name in &names {
            std::fs::write(root.join(name), "one\ntwo\n").unwrap();
        }

        let selector = Selector::new(Engine::Gix, None);
        let before = git_spawns_on_this_thread();
        let gix = compute_head_diff_through(
            &selector,
            &paneflow_git::Options::dock_listing(),
            root,
            DiffOptions::default(),
        );
        assert_eq!(
            git_spawns_on_this_thread() - before,
            0,
            "the gix path spawns no git process"
        );
        assert_eq!(
            selector.counters().probes(Probe::DockListing, Engine::Gix),
            1
        );
        assert_eq!(gix.error, None);
        assert_eq!(gix.files.len(), 200);
        assert!(gix.files.iter().all(|file| file.base_text == "one\n"));

        let selector = Selector::new(Engine::Cli, None);
        let before = git_spawns_on_this_thread();
        let cli = compute_head_diff_through(
            &selector,
            &paneflow_git::Options::dock_listing(),
            root,
            DiffOptions::default(),
        );
        assert!(
            git_spawns_on_this_thread() - before > 200,
            "the CLI path runs one git show per file"
        );
        assert_eq!(
            selector.counters().probes(Probe::DockListing, Engine::Cli),
            1
        );
        assert_eq!(dock_view(&gix), dock_view(&cli));
    }

    #[test]
    fn a_gix_failure_is_answered_by_the_cli_in_the_same_call() {
        use paneflow_git::engine::{Engine, Fallback, Probe, Selector};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::committed_repo(root, &[("tracked.txt", "one\n")]);
        std::fs::write(root.join("tracked.txt"), "one\ntwo\n").unwrap();
        let index = root.join(".git").join("index");
        let mut bytes = std::fs::read(&index).unwrap();
        bytes[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        std::fs::write(&index, &bytes).unwrap();

        let selector = Selector::new(Engine::Gix, None);
        let diff = compute_head_diff_through(
            &selector,
            &paneflow_git::Options::dock_listing(),
            root,
            DiffOptions::default(),
        );
        let counters = selector.counters();
        assert_eq!(counters.probes(Probe::DockListing, Engine::Cli), 1);
        assert_eq!(counters.fallbacks(Fallback::Error), 1);
        assert_eq!(dock_view(&diff), dock_view(&cli_dock(root)));
    }

    #[test]
    fn a_listing_over_the_entry_cap_is_answered_by_the_cli_without_pinning_the_repository() {
        use paneflow_git::engine::{Engine, Fallback, Probe, Selector};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::committed_repo(root, &[("one.txt", "one\n"), ("two.txt", "two\n")]);
        std::fs::write(root.join("one.txt"), "one\nedited\n").unwrap();
        std::fs::write(root.join("two.txt"), "two\nedited\n").unwrap();
        std::fs::write(root.join("untracked.txt"), "u\n").unwrap();
        let capped = paneflow_git::Options {
            entry_cap: 1,
            ..paneflow_git::Options::dock_listing()
        };

        let selector = Selector::new(Engine::Gix, None);
        for _ in 0..3 {
            let diff = compute_head_diff_through(&selector, &capped, root, DiffOptions::default());
            assert_eq!(dock_view(&diff), dock_view(&cli_dock(root)));
        }
        let counters = selector.counters();
        assert_eq!(counters.probes(Probe::DockListing, Engine::Cli), 3);
        assert_eq!(counters.fallbacks(Fallback::SizeThreshold), 3);
        assert_eq!(counters.fallbacks(Fallback::Error), 0);
        assert_eq!(
            counters.pinned_directories, 0,
            "a large status is not a fault of the repository"
        );
        let diff = compute_head_diff_through(
            &selector,
            &paneflow_git::Options::dock_listing(),
            root,
            DiffOptions::default(),
        );
        assert_eq!(diff.files.len(), 3);
        assert_eq!(
            selector.counters().probes(Probe::DockListing, Engine::Gix),
            1,
            "the repository stays on gix"
        );
    }

    #[test]
    fn gix_and_cli_dock_listings_are_equal_on_every_oracle_fixture() {
        use paneflow_git::oracle::{Probe as OracleProbe, Skip, base, layouts, linked, submodules};
        paneflow_git::support::isolate_process();
        let tmp = tempfile::tempdir().expect("tempdir");
        let matrices = [
            ("base", base::matrix(), base::MAY_SKIP, &[][..]),
            ("layouts", layouts::matrix(), &[][..], &[][..]),
            ("linked", linked::matrix(), &[][..], &[][..]),
            (
                "submodules",
                submodules::matrix(OracleProbe::Files),
                &[][..],
                submodules::MUST_FAIL,
            ),
        ];
        let mut compared = 0usize;
        let mut skipped = 0usize;
        let mut total = 0usize;
        let mut failures = Vec::new();
        for (matrix, fixtures, may_skip, must_fail) in matrices {
            for fixture in fixtures {
                total += 1;
                let root = tmp.path().join(matrix).join(fixture.name).join("repo");
                let cwd = match (fixture.build)(&root) {
                    Ok(cwd) => cwd,
                    Err(Skip(reason)) if may_skip.contains(&fixture.name) => {
                        eprintln!("oracle: SKIPPED fixture {}: {reason}", fixture.name);
                        skipped += 1;
                        continue;
                    }
                    Err(Skip(reason)) => {
                        failures.push(format!(
                            "fixture {} was skipped but is not allowed to skip: {reason}",
                            fixture.name
                        ));
                        continue;
                    }
                };
                compared += 1;
                let cli = cli_dock(&cwd);
                let gix = head_diff_with_gix(
                    &cwd,
                    &paneflow_git::Options::dock_listing(),
                    DiffOptions::default(),
                );
                let expected_to_fail = must_fail.contains(&fixture.name);
                match gix {
                    Ok(gix) if !expected_to_fail && dock_view(&gix) == dock_view(&cli) => {}
                    Err(_) if expected_to_fail && cli.error.is_some() => {}
                    gix => failures.push(format!(
                        "fixture {}: expected both engines to {}, got\n  cli {:?}\n  gix {:?}",
                        fixture.name,
                        if expected_to_fail {
                            "fail"
                        } else {
                            "list the same files"
                        },
                        dock_view(&cli),
                        gix.as_ref().map(dock_view),
                    )),
                }
            }
        }
        assert!(compared > 0);
        assert_eq!(
            compared + skipped,
            total,
            "every fixture is compared or skipped by name"
        );
        assert!(
            failures.is_empty(),
            "the dock listing differs between the git CLI and gix ({compared} fixtures):\n{}",
            failures.join("\n")
        );
    }
}
