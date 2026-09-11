use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::RwLock;
use std::time::{Duration, SystemTime};

const GIT_DEADLINE: Duration = Duration::from_secs(10);
const ADD_DEADLINE: Duration = Duration::from_secs(120);
const STDOUT_CAP: u64 = 256 * 1024;
const OWNER_MARKER_FILE: &str = "paneflow-owner";
const LEGACY_OWNER_MARKER_FILE: &str = ".paneflow-worktree";

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

#[derive(Debug, Clone, PartialEq)]
pub struct ManagedWorktree {
    pub path: PathBuf,
    pub repo_root: PathBuf,
    pub branch: String,
    pub teardown: TeardownPolicy,
}

pub fn worktree_git_dir(worktree_path: &Path) -> Option<PathBuf> {
    let dot_git = worktree_path.join(".git");
    if !dot_git.is_file() {
        return None;
    }
    let contents = std::fs::read_to_string(&dot_git).ok()?;
    let target = contents.lines().next()?.strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    let git_dir = PathBuf::from(target);
    Some(if git_dir.is_absolute() {
        git_dir
    } else {
        worktree_path.join(git_dir)
    })
}

pub fn owner_marker_path(worktree_path: &Path) -> Option<PathBuf> {
    worktree_git_dir(worktree_path).map(|dir| dir.join(OWNER_MARKER_FILE))
}

pub fn legacy_owner_marker_path(worktree_path: &Path) -> PathBuf {
    worktree_path.join(LEGACY_OWNER_MARKER_FILE)
}

pub fn has_owner_marker(worktree_path: &Path) -> bool {
    owner_marker_path(worktree_path).is_some_and(|marker| marker.is_file())
        || legacy_owner_marker_path(worktree_path).is_file()
}

pub fn migrate_owner_marker(worktree_path: &Path) -> bool {
    let legacy = legacy_owner_marker_path(worktree_path);
    if !legacy.is_file() {
        return false;
    }
    let Some(marker) = owner_marker_path(worktree_path) else {
        return false;
    };
    if !marker.is_file() {
        let Ok(contents) = std::fs::read(&legacy) else {
            return false;
        };
        if let Err(e) = std::fs::write(&marker, contents) {
            log::warn!(
                "owner marker: cannot move {} to {}: {e}",
                legacy.display(),
                marker.display()
            );
            return false;
        }
    }
    match std::fs::remove_file(&legacy) {
        Ok(()) => {
            log::info!(
                "owner marker moved out of the checkout: {}",
                marker.display()
            );
            true
        }
        Err(e) => {
            log::warn!("owner marker: cannot remove {}: {e}", legacy.display());
            false
        }
    }
}

pub fn migrate_owner_markers(entries: &[WorktreeEntry]) {
    for entry in entries {
        migrate_owner_marker(&entry.path);
    }
}

fn write_owner_marker(worktree_path: &Path, repo_root: &Path, branch: &str) -> Result<(), String> {
    let marker = owner_marker_path(worktree_path).ok_or_else(|| {
        format!(
            "cannot locate the git dir of {}: no `.git` pointer file",
            worktree_path.display()
        )
    })?;
    let contents = format!(
        "owner=paneflow\nrepo_root={}\nbranch={}\n",
        repo_root.display(),
        branch
    );
    std::fs::write(&marker, contents)
        .map_err(|e| format!("cannot write owner marker {}: {e}", marker.display()))
}

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

#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: Option<String>,
}

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

static WORKTREES_ROOT: RwLock<Option<PathBuf>> = RwLock::new(None);

pub fn set_worktrees_root(dir: Option<PathBuf>) {
    let mut root = WORKTREES_ROOT.write().unwrap_or_else(|e| e.into_inner());
    *root = dir;
}

pub fn default_worktrees_root() -> Option<PathBuf> {
    paneflow_home::worktrees_dir()
}

pub fn worktrees_root() -> Option<PathBuf> {
    WORKTREES_ROOT
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .or_else(default_worktrees_root)
}

fn repo_name(repo_root: &Path) -> String {
    repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string())
}

fn repo_dir_name(repo_root: &Path) -> String {
    let key = repo_root.to_string_lossy();
    format!("{}-{}", repo_name(repo_root), branch_hash_suffix(&key))
}

fn legacy_worktrees_parent(repo_root: &Path) -> PathBuf {
    let parent = repo_root.parent().unwrap_or(repo_root);
    parent.join(format!("{}.worktrees", repo_name(repo_root)))
}

fn worktrees_parent(repo_root: &Path) -> PathBuf {
    match worktrees_root() {
        Some(root) => root.join(repo_dir_name(repo_root)),
        None => legacy_worktrees_parent(repo_root),
    }
}

fn in_paneflow_path_form(repo_root: &Path, path: PathBuf) -> PathBuf {
    let roots = [
        worktrees_parent(repo_root),
        legacy_worktrees_parent(repo_root),
        repo_root.to_path_buf(),
    ];
    for root in roots {
        if path.starts_with(&root) {
            return path;
        }
        let Ok(resolved) = std::fs::canonicalize(&root) else {
            continue;
        };
        if let Ok(rest) = path.strip_prefix(without_verbatim_prefix(resolved)) {
            return root.join(rest);
        }
    }
    path
}

#[cfg(windows)]
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

#[cfg(not(windows))]
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

pub fn worktree_dir(repo_root: &Path, branch: &str) -> PathBuf {
    worktrees_parent(repo_root).join(branch_slug_or_default(branch))
}

pub fn worktree_dir_hashed(repo_root: &Path, branch: &str) -> PathBuf {
    let slug = branch_slug_or_default(branch);
    worktrees_parent(repo_root).join(format!("{slug}-{}", branch_hash_suffix(branch)))
}

pub fn legacy_worktree_dir(repo_root: &Path, branch: &str) -> PathBuf {
    legacy_worktrees_parent(repo_root).join(branch_slug_or_default(branch))
}

pub fn legacy_worktree_dir_hashed(repo_root: &Path, branch: &str) -> PathBuf {
    let slug = branch_slug_or_default(branch);
    legacy_worktrees_parent(repo_root).join(format!("{slug}-{}", branch_hash_suffix(branch)))
}

pub fn is_paneflow_worktree_dir(repo_root: &Path, branch: &str, path: &Path) -> bool {
    path == worktree_dir(repo_root, branch)
        || path == worktree_dir_hashed(repo_root, branch)
        || path == legacy_worktree_dir(repo_root, branch)
        || path == legacy_worktree_dir_hashed(repo_root, branch)
}

pub fn created_at(worktree_path: &Path) -> Option<SystemTime> {
    owner_marker_path(worktree_path)
        .and_then(|marker| std::fs::metadata(marker).ok())
        .or_else(|| std::fs::metadata(legacy_owner_marker_path(worktree_path)).ok())
        .and_then(|meta| meta.modified().ok())
}

const SNAPSHOT_REF_PREFIX: &str = "refs/paneflow/snapshots/";
const SNAPSHOT_IDENTITY: &[&str] = &[
    "-c",
    "user.name=Paneflow",
    "-c",
    "user.email=paneflow@localhost",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub reference: String,
    pub commit: String,
    pub head: String,
    pub branch: Option<String>,
    pub path: PathBuf,
    pub taken_at: u64,
}

impl Snapshot {
    pub fn label(&self) -> String {
        self.branch.clone().unwrap_or_else(|| {
            self.path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.head.chars().take(7).collect())
        })
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn current_branch(worktree_path: &Path) -> Option<String> {
    run_git(
        worktree_path,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        GIT_DEADLINE,
    )
    .ok()
    .filter(|b| !b.is_empty())
}

pub fn snapshot_worktree(repo_root: &Path, worktree_path: &Path) -> Result<Snapshot, String> {
    let head = run_git(
        worktree_path,
        &["rev-parse", "--verify", "HEAD"],
        GIT_DEADLINE,
    )?;
    let branch = current_branch(worktree_path);
    let git_dir = worktree_git_dir(worktree_path).ok_or_else(|| {
        format!(
            "cannot locate the git dir of {}: no `.git` pointer file",
            worktree_path.display()
        )
    })?;
    let index = git_dir.join("paneflow-snapshot-index");
    let _ = std::fs::remove_file(&index);
    let index_s = index.to_string_lossy().into_owned();
    let mut add = Command::new("git");
    add.arg("-C")
        .arg(worktree_path)
        .env("GIT_INDEX_FILE", &index_s)
        .args(["add", "-A", "--", "."]);
    let out = paneflow_process::run_with_timeout(add, ADD_DEADLINE, STDOUT_CAP)
        .map_err(|e| format!("git add -A failed: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&index);
        return Err(format!(
            "git add -A failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut write_tree = Command::new("git");
    write_tree
        .arg("-C")
        .arg(worktree_path)
        .env("GIT_INDEX_FILE", &index_s)
        .arg("write-tree");
    let tree = paneflow_process::run_with_timeout(write_tree, ADD_DEADLINE, STDOUT_CAP)
        .map_err(|e| format!("git write-tree failed: {e}"))
        .and_then(|out| {
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                Err(format!(
                    "git write-tree failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            }
        });
    let _ = std::fs::remove_file(&index);
    let tree = tree?;
    let taken_at = unix_now();
    let label = branch
        .as_deref()
        .map(branch_slug_or_default)
        .unwrap_or_else(|| branch_slug_or_default(&worktree_path.to_string_lossy()));
    let message = format!(
        "Paneflow snapshot of {}\n\nPaneflow-Path: {}\nPaneflow-Branch: {}\nPaneflow-Head: {head}\nPaneflow-Taken-At: {taken_at}\n",
        branch.as_deref().unwrap_or("detached HEAD"),
        worktree_path.display(),
        branch.as_deref().unwrap_or(""),
    );
    let mut args: Vec<&str> = SNAPSHOT_IDENTITY.to_vec();
    args.extend(["commit-tree", &tree, "-p", &head, "-m", &message]);
    let commit = run_git(repo_root, &args, GIT_DEADLINE)?;
    let reference = format!("{SNAPSHOT_REF_PREFIX}{label}-{taken_at}");
    run_git(
        repo_root,
        &["update-ref", &reference, &commit],
        GIT_DEADLINE,
    )?;
    Ok(Snapshot {
        reference,
        commit,
        head,
        branch,
        path: worktree_path.to_path_buf(),
        taken_at,
    })
}

pub fn list_snapshots(repo_root: &Path) -> Result<Vec<Snapshot>, String> {
    let stdout = run_git(
        repo_root,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(contents:body)%01",
            SNAPSHOT_REF_PREFIX,
        ],
        GIT_DEADLINE,
    )?;
    Ok(parse_snapshot_refs(&stdout))
}

pub fn parse_snapshot_refs(stdout: &str) -> Vec<Snapshot> {
    let mut out = Vec::new();
    for record in stdout.split('\u{1}') {
        let mut fields = record.trim_start_matches('\n').splitn(3, '\0');
        let (Some(reference), Some(commit), Some(body)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if reference.is_empty() {
            continue;
        }
        let mut path = None;
        let mut branch = None;
        let mut head = None;
        let mut taken_at = 0;
        for line in body.lines() {
            if let Some(v) = line.strip_prefix("Paneflow-Path: ") {
                path = Some(PathBuf::from(v.trim()));
            } else if let Some(v) = line.strip_prefix("Paneflow-Branch: ") {
                branch = Some(v.trim().to_string()).filter(|b| !b.is_empty());
            } else if let Some(v) = line.strip_prefix("Paneflow-Head: ") {
                head = Some(v.trim().to_string());
            } else if let Some(v) = line.strip_prefix("Paneflow-Taken-At: ") {
                taken_at = v.trim().parse().unwrap_or(0);
            }
        }
        let (Some(path), Some(head)) = (path, head) else {
            continue;
        };
        out.push(Snapshot {
            reference: reference.to_string(),
            commit: commit.to_string(),
            head,
            branch,
            path,
            taken_at,
        });
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.taken_at));
    out
}

pub fn delete_snapshot(repo_root: &Path, snapshot: &Snapshot) -> Result<(), String> {
    run_git(
        repo_root,
        &["update-ref", "-d", &snapshot.reference],
        GIT_DEADLINE,
    )
    .map(|_| ())
}

pub fn restore_snapshot(repo_root: &Path, snapshot: &Snapshot) -> Result<PathBuf, String> {
    let entries = list_worktrees(repo_root)?;
    let label = snapshot.label();
    let path =
        if !snapshot.path.exists() && is_paneflow_worktree_dir(repo_root, &label, &snapshot.path) {
            snapshot.path.clone()
        } else {
            match plan_branch_checkout(&entries, repo_root, &label)? {
                BranchCheckout::Existing(path) | BranchCheckout::Create(path) => path,
            }
        };
    if path.exists() {
        return Err(format!(
            "{} exists; remove it first, then restore again",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let path_s = path.to_string_lossy().into_owned();
    run_git(
        repo_root,
        &["worktree", "add", "--detach", &path_s, &snapshot.head],
        ADD_DEADLINE,
    )?;
    let finish = || -> Result<(), String> {
        if let Some(branch) = snapshot.branch.as_deref()
            && !entries
                .iter()
                .any(|entry| entry.branch.as_deref() == Some(branch))
            && run_git(
                repo_root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ],
                GIT_DEADLINE,
            )
            .is_ok_and(|sha| sha == snapshot.head)
        {
            run_git(&path, &["switch", "--quiet", branch], GIT_DEADLINE)?;
        }
        let tree = format!("{}^{{tree}}", snapshot.commit);
        run_git(&path, &["read-tree", "-m", "-u", &tree], ADD_DEADLINE)?;
        run_git(&path, &["reset", "--quiet", "--", "."], ADD_DEADLINE)?;
        write_owner_marker(&path, repo_root, &label)?;
        copy_include_files(repo_root, &path);
        Ok(())
    };
    if let Err(e) = finish() {
        let _ = run_git(
            repo_root,
            &["worktree", "remove", "--force", &path_s],
            GIT_DEADLINE,
        );
        return Err(e);
    }
    let _ = delete_snapshot(repo_root, snapshot);
    Ok(path)
}

pub fn snapshot_and_remove(
    repo_root: &Path,
    worktree_path: &Path,
) -> Result<Option<Snapshot>, String> {
    migrate_owner_marker(worktree_path);
    if !has_owner_marker(worktree_path) {
        return Err(format!(
            "{} was not created by Paneflow - remove it with git worktree remove",
            worktree_path.display()
        ));
    }
    let snapshot = match is_clean(worktree_path)? {
        true => None,
        false => Some(snapshot_worktree(repo_root, worktree_path)?),
    };
    let path_s = worktree_path.to_string_lossy();
    run_git(
        repo_root,
        &["worktree", "remove", "--force", &path_s],
        GIT_DEADLINE,
    )?;
    let _ = prune(repo_root);
    remove_empty_repo_dir(repo_root, worktree_path);
    Ok(snapshot)
}

fn remove_empty_repo_dir(repo_root: &Path, worktree_path: &Path) {
    let Some(parent) = worktree_path.parent() else {
        return;
    };
    let managed =
        parent == worktrees_parent(repo_root) || parent == legacy_worktrees_parent(repo_root);
    let empty = std::fs::read_dir(parent).is_ok_and(|mut entries| entries.next().is_none());
    if managed && empty {
        let _ = std::fs::remove_dir(parent);
    }
}

pub fn trim_to_limit(
    candidates: Vec<ManagedWorktree>,
    keep: usize,
    bound: &HashSet<PathBuf>,
) -> Vec<PathBuf> {
    let mut aged: Vec<(SystemTime, ManagedWorktree)> = candidates
        .into_iter()
        .filter(|wt| wt.teardown == TeardownPolicy::Auto && wt.path.exists())
        .map(|wt| (created_at(&wt.path).unwrap_or(SystemTime::UNIX_EPOCH), wt))
        .collect();
    if aged.len() <= keep {
        return Vec::new();
    }
    aged.sort_by_key(|(age, _)| *age);
    let mut excess = aged.len() - keep;
    let mut removed = Vec::new();
    for (_, wt) in aged {
        if excess == 0 {
            break;
        }
        if bound.contains(&wt.path) {
            continue;
        }
        match snapshot_and_remove(&wt.repo_root, &wt.path) {
            Ok(snapshot) => {
                log::info!(
                    "worktree trimmed past keep limit: {}{}",
                    wt.path.display(),
                    snapshot
                        .map(|s| format!(" (snapshot {})", s.reference))
                        .unwrap_or_default()
                );
                removed.push(wt.path);
                excess -= 1;
            }
            Err(e) => log::warn!("worktree kept ({}): {e}", wt.path.display()),
        }
    }
    removed
}

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

pub fn list_worktrees(repo_root: &Path) -> Result<Vec<WorktreeEntry>, String> {
    let stdout = run_git(
        repo_root,
        &["worktree", "list", "--porcelain"],
        GIT_DEADLINE,
    )?;
    Ok(parse_worktree_porcelain(&stdout)
        .into_iter()
        .map(|entry| WorktreeEntry {
            path: in_paneflow_path_form(repo_root, entry.path),
            branch: entry.branch,
        })
        .collect())
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchCheckout {
    Existing(PathBuf),
    Create(PathBuf),
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCheckout {
    pub path: PathBuf,
    pub created: bool,
}

pub fn prepare_branch_checkout(repo_root: &Path, branch: &str) -> Result<PreparedCheckout, String> {
    let entries = list_worktrees(repo_root)?;
    match plan_branch_checkout(&entries, repo_root, branch)? {
        BranchCheckout::Existing(path) => Ok(PreparedCheckout {
            path,
            created: false,
        }),
        BranchCheckout::Create(path) => {
            if path.exists() {
                return Err(format!(
                    "{} exists but is not a registered worktree; remove it first",
                    path.display()
                ));
            }
            add_worktree(repo_root, &path, branch, false, None)?;
            copy_include_files(repo_root, &path);
            Ok(PreparedCheckout {
                path,
                created: true,
            })
        }
    }
}

pub fn validate_branch_name(repo_root: &Path, branch: &str) -> Result<(), String> {
    if branch.is_empty() {
        return Err("Branch name is empty".to_string());
    }
    run_git(
        repo_root,
        &["check-ref-format", "--branch", branch],
        GIT_DEADLINE,
    )
    .map(|_| ())
    .map_err(|_| format!("'{branch}' is not a valid branch name"))
}

pub fn validate_new_branch_name(repo_root: &Path, branch: &str) -> Result<(), String> {
    validate_branch_name(repo_root, branch)?;
    if branch_exists(repo_root, branch) {
        return Err(format!("branch '{branch}' already exists"));
    }
    Ok(())
}

pub fn create_branch_checkout(
    repo_root: &Path,
    branch: &str,
    base: Option<&str>,
) -> Result<PreparedCheckout, String> {
    validate_branch_name(repo_root, branch)?;
    if branch_exists(repo_root, branch) {
        return prepare_branch_checkout(repo_root, branch);
    }
    let entries = list_worktrees(repo_root)?;
    let path = match plan_branch_checkout(&entries, repo_root, branch)? {
        BranchCheckout::Existing(path) | BranchCheckout::Create(path) => path,
    };
    if path.exists() {
        return Err(format!(
            "{} exists but is not a registered worktree; remove it first",
            path.display()
        ));
    }
    add_worktree(repo_root, &path, branch, true, base)?;
    copy_include_files(repo_root, &path);
    Ok(PreparedCheckout {
        path,
        created: true,
    })
}

pub fn resolve_base_commit(repo_root: &Path, base: Option<&str>) -> Result<String, String> {
    let base = base
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .unwrap_or("HEAD");
    if base.starts_with('-') {
        return Err(format!("'{base}' is not a valid base"));
    }
    run_git(
        repo_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{base}^{{commit}}"),
        ],
        GIT_DEADLINE,
    )
    .map_err(|_| format!("'{base}' does not name a commit"))
}

pub fn detached_checkout_label(base: Option<&str>, sha: &str) -> String {
    let base = base
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .unwrap_or("HEAD");
    let short: String = sha.chars().take(7).collect();
    format!("{}-{short}", branch_slug_or_default(base))
}

pub fn create_detached_checkout(repo_root: &Path, base: Option<&str>) -> Result<PathBuf, String> {
    let sha = resolve_base_commit(repo_root, base)?;
    let label = detached_checkout_label(base, &sha);
    let entries = list_worktrees(repo_root)?;
    let path = match plan_branch_checkout(&entries, repo_root, &label)? {
        BranchCheckout::Existing(path) | BranchCheckout::Create(path) => path,
    };
    if path.exists() {
        return Err(format!(
            "{} exists; remove it first or start from another commit",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let path_s = path.to_string_lossy();
    run_git(
        repo_root,
        &["worktree", "add", "--detach", &path_s, &sha],
        ADD_DEADLINE,
    )?;
    if let Err(e) = write_owner_marker(&path, repo_root, &label) {
        let _ = remove_worktree(repo_root, &path);
        return Err(e);
    }
    copy_include_files(repo_root, &path);
    Ok(path)
}

pub fn switch_checkout(repo_root: &Path, branch: &str, base: Option<&str>) -> Result<(), String> {
    let branch = branch.trim();
    if branch.is_empty() {
        let commit = resolve_base_commit(repo_root, base)?;
        return run_git(repo_root, &["switch", "--detach", &commit], ADD_DEADLINE).map(|_| ());
    }
    validate_branch_name(repo_root, branch)?;
    if branch_exists(repo_root, branch) {
        return run_git(repo_root, &["switch", branch], ADD_DEADLINE).map(|_| ());
    }
    let commit = resolve_base_commit(repo_root, base)?;
    run_git(repo_root, &["switch", "-c", branch, &commit], ADD_DEADLINE).map(|_| ())
}

pub fn create_branch_here(
    repo_root: &Path,
    worktree_path: &Path,
    branch: &str,
) -> Result<(), String> {
    validate_new_branch_name(repo_root, branch)?;
    run_git(worktree_path, &["switch", "-c", branch], ADD_DEADLINE).map(|_| ())
}

pub fn add_worktree(
    repo_root: &Path,
    path: &Path,
    branch: &str,
    create_branch: bool,
    base: Option<&str>,
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
    if create_branch && let Some(base) = base.map(str::trim).filter(|b| !b.is_empty()) {
        args.push(base);
    }
    run_git(repo_root, &args, ADD_DEADLINE)?;
    if let Err(e) = write_owner_marker(path, repo_root, branch) {
        let _ = remove_worktree(repo_root, path);
        return Err(e);
    }
    Ok(())
}

pub fn is_clean(worktree_path: &Path) -> Result<bool, String> {
    run_git(worktree_path, &["status", "--porcelain"], GIT_DEADLINE).map(|out| out.is_empty())
}

pub fn remove_worktree(repo_root: &Path, path: &Path) -> Result<(), String> {
    let path_s = path.to_string_lossy();
    run_git(repo_root, &["worktree", "remove", &path_s], GIT_DEADLINE)?;
    remove_empty_repo_dir(repo_root, path);
    Ok(())
}

pub fn prune(repo_root: &Path) -> Result<(), String> {
    run_git(repo_root, &["worktree", "prune"], GIT_DEADLINE).map(|_| ())
}

const INCLUDE_FILE: &str = ".worktreeinclude";
const DEFAULT_INCLUDES: &[&str] = &["AGENTS.override.md"];

pub fn parse_worktree_include(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.trim_start_matches("./").to_string())
        .filter(|line| {
            let path = Path::new(line);
            !path.is_absolute()
                && !path.components().any(|c| {
                    matches!(
                        c,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                })
        })
        .collect()
}

fn default_include_entries(src_root: &Path) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    if let Ok(dir) = std::fs::read_dir(src_root) {
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".env") && entry.path().is_file() {
                entries.push(name);
            }
        }
    }
    entries.extend(DEFAULT_INCLUDES.iter().map(|s| s.to_string()));
    entries
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)?.flatten() {
            let name = entry.file_name();
            copy_tree(&entry.path(), &dst.join(name))?;
        }
        Ok(())
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst).map(|_| ())
    }
}

pub fn copy_include_files(src_root: &Path, dst_root: &Path) -> Vec<String> {
    let listed = std::fs::read_to_string(src_root.join(INCLUDE_FILE))
        .ok()
        .map(|contents| parse_worktree_include(&contents));
    let mut entries = match listed {
        Some(listed) => listed,
        None => default_include_entries(src_root),
    };
    entries.sort();
    entries.dedup();
    let mut copied = Vec::new();
    for rel in entries {
        let rel_path = Path::new(rel.trim_end_matches(['/', '\\']));
        let src = src_root.join(rel_path);
        let dst = dst_root.join(rel_path);
        if !src.exists() || dst.exists() {
            continue;
        }
        match copy_tree(&src, &dst) {
            Ok(()) => copied.push(rel),
            Err(e) => log::warn!("worktree include: cannot copy {}: {e}", src.display()),
        }
    }
    copied
}

pub fn teardown_all(worktrees: Vec<ManagedWorktree>) {
    for wt in worktrees {
        if wt.teardown == TeardownPolicy::Keep {
            continue;
        }
        if !wt.path.exists() {
            let _ = prune(&wt.repo_root);
            continue;
        }
        match snapshot_and_remove(&wt.repo_root, &wt.path) {
            Ok(snapshot) => log::info!(
                "worktree removed: {}{}",
                wt.path.display(),
                snapshot
                    .map(|s| format!(" (snapshot {})", s.reference))
                    .unwrap_or_default()
            ),
            Err(e) => log::warn!("worktree kept ({}): {e}", wt.path.display()),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};

    static ROOT_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) struct RootGuard {
        _lock: MutexGuard<'static, ()>,
    }

    impl Drop for RootGuard {
        fn drop(&mut self) {
            super::set_worktrees_root(None);
        }
    }

    pub(crate) fn scoped_root(dir: PathBuf) -> RootGuard {
        let lock = ROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        super::set_worktrees_root(Some(dir));
        RootGuard { _lock: lock }
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
        assert_eq!(branch_slug("/weird/"), "weird");
        assert_eq!(branch_slug(".hidden"), "hidden");
        assert_eq!(branch_slug("release/v1.2.3"), "release-v1.2.3");
    }

    #[test]
    fn branch_slug_neutralizes_dot_only_traversal() {
        assert_eq!(branch_slug(".."), "");
        assert_eq!(branch_slug("."), "");
        assert_eq!(branch_slug("..."), "");
        assert_eq!(branch_slug("-..-"), "");
    }

    #[test]
    fn a_resolved_git_path_comes_back_in_paneflow_path_form() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _root = test_support::scoped_root(tmp.path().join("worktrees"));
        let repo_root = tmp.path().join("repo");
        let dir = worktree_dir(&repo_root, "feat/x");
        std::fs::create_dir_all(&dir).expect("worktree dir");

        let parent = worktrees_parent(&repo_root);
        let resolved = without_verbatim_prefix(std::fs::canonicalize(&parent).expect("canonical"));
        let as_git_reports_it = resolved.join("feat-x");

        assert_eq!(in_paneflow_path_form(&repo_root, as_git_reports_it), dir);
        assert_eq!(
            in_paneflow_path_form(&repo_root, dir.clone()),
            dir,
            "a path already in Paneflow form is untouched"
        );
    }

    #[test]
    fn a_path_outside_every_paneflow_root_is_left_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _root = test_support::scoped_root(tmp.path().join("worktrees"));
        let repo_root = tmp.path().join("repo");
        let outside = tmp.path().join("somewhere-else").join("checkout");

        assert_eq!(in_paneflow_path_form(&repo_root, outside.clone()), outside);
    }

    #[test]
    fn worktree_dir_never_escapes_the_worktrees_dir() {
        let _root = test_support::scoped_root(PathBuf::from("/home/a/paneflow-worktrees"));
        let repo = Path::new("/home/a/dev/paneflow");
        let dir = worktree_dir(repo, "..");
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some("branch"));
        assert!(dir.starts_with(worktrees_parent(repo)));
        assert_eq!(
            legacy_worktree_dir(repo, ".."),
            PathBuf::from("/home/a/dev/paneflow.worktrees/branch")
        );
    }

    #[test]
    fn managed_worktrees_live_under_the_paneflow_root_keyed_by_repository() {
        let _root = test_support::scoped_root(PathBuf::from("/home/a/paneflow-worktrees"));
        let repo = Path::new("/home/a/dev/paneflow");
        let other = Path::new("/home/a/other/paneflow");
        let root = worktrees_root().expect("a data dir exists on the test host");
        let dir = worktree_dir(repo, "feat/x");
        assert!(dir.starts_with(&root));
        assert!(!dir.starts_with("/home/a/dev/"));
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some("feat-x"));
        let repo_dir = dir.parent().expect("repo dir");
        assert!(
            repo_dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("paneflow-")),
            "the repository directory carries the repository name"
        );
        assert_ne!(
            worktree_dir(repo, "feat/x"),
            worktree_dir(other, "feat/x"),
            "two clones with the same name never share a directory"
        );
        assert!(is_paneflow_worktree_dir(repo, "feat/x", &dir));
        assert!(is_paneflow_worktree_dir(
            repo,
            "feat/x",
            &legacy_worktree_dir(repo, "feat/x")
        ));
    }

    #[test]
    fn a_branch_already_checked_out_is_reused_never_recreated() {
        let _root = test_support::scoped_root(PathBuf::from("/home/a/paneflow-worktrees"));
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
            Ok(BranchCheckout::Create(worktree_dir(
                repo,
                "chore/rust-1.98"
            )))
        );
    }

    #[test]
    fn a_slug_collision_falls_back_to_the_hashed_dir() {
        let _root = test_support::scoped_root(PathBuf::from("/home/a/paneflow-worktrees"));
        let repo = Path::new("/home/a/dev/paneflow");
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
        let _root = test_support::scoped_root(PathBuf::from("/home/a/paneflow-worktrees"));
        let repo = Path::new("/home/a/dev/paneflow");
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
    fn legacy_worktree_dir_is_a_sibling_of_the_repo() {
        let dir = legacy_worktree_dir(Path::new("/home/a/dev/paneflow"), "feat/x");
        assert_eq!(dir, PathBuf::from("/home/a/dev/paneflow.worktrees/feat-x"));
        assert!(!dir.starts_with("/home/a/dev/paneflow/"));
    }

    #[test]
    fn hashed_worktree_dir_disambiguates_slug_collisions() {
        let _root = test_support::scoped_root(PathBuf::from("/home/a/paneflow-worktrees"));
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
        assert!(is_paneflow_worktree_dir(
            repo,
            a,
            &legacy_worktree_dir_hashed(repo, a)
        ));
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

    fn link_fake_git_dir(worktree: &Path, git_dir: &Path) {
        std::fs::create_dir_all(worktree).expect("worktree dir");
        std::fs::create_dir_all(git_dir).expect("git dir");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .expect("gitdir pointer");
    }

    #[test]
    fn owner_marker_lives_in_the_worktree_git_dir_never_in_the_checkout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let git_dir = tmp.path().join("repo/.git/worktrees/feat-x");
        let worktree = tmp.path().join("repo.worktrees/feat-x");
        link_fake_git_dir(&worktree, &git_dir);

        assert_eq!(worktree_git_dir(&worktree), Some(git_dir.clone()));
        assert_eq!(
            owner_marker_path(&worktree),
            Some(git_dir.join("paneflow-owner"))
        );
        assert!(
            !owner_marker_path(&worktree)
                .expect("marker path")
                .starts_with(&worktree),
            "the marker must never be a file git status can see"
        );
        assert_eq!(
            worktree_git_dir(tmp.path()),
            None,
            "a directory without a .git pointer file has no worktree git dir"
        );
    }

    #[test]
    fn relative_gitdir_pointer_resolves_against_the_worktree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).expect("worktree dir");
        std::fs::write(worktree.join(".git"), "gitdir: ../repo/.git/worktrees/wt\n")
            .expect("pointer");
        assert_eq!(
            worktree_git_dir(&worktree),
            Some(worktree.join("../repo/.git/worktrees/wt"))
        );
    }

    #[test]
    fn legacy_marker_migrates_out_of_the_checkout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let git_dir = tmp.path().join("repo/.git/worktrees/feat-x");
        let worktree = tmp.path().join("repo.worktrees/feat-x");
        link_fake_git_dir(&worktree, &git_dir);
        let legacy = legacy_owner_marker_path(&worktree);
        std::fs::write(&legacy, "owner=paneflow\nbranch=feat/x\n").expect("legacy marker");

        assert!(has_owner_marker(&worktree), "legacy marker still counts");
        assert!(migrate_owner_marker(&worktree));
        assert!(!legacy.exists(), "the checkout is clean again");
        assert_eq!(
            std::fs::read_to_string(git_dir.join("paneflow-owner")).expect("moved marker"),
            "owner=paneflow\nbranch=feat/x\n"
        );
        assert!(has_owner_marker(&worktree));
        assert!(
            !migrate_owner_marker(&worktree),
            "a second pass has nothing to move"
        );
    }

    #[test]
    fn managed_worktree_record_requires_marker_and_generated_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _root = test_support::scoped_root(tmp.path().join("worktrees"));
        let repo_root = tmp.path().join("repo");
        let branch = "feat/hardening";
        let path = worktree_dir(&repo_root, branch);
        link_fake_git_dir(&path, &repo_root.join(".git/worktrees/feat-hardening"));

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

        std::fs::write(
            owner_marker_path(&path).expect("marker path"),
            "owner=paneflow\n",
        )
        .expect("marker");
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
        link_fake_git_dir(&outside, &repo_root.join(".git/worktrees/external"));
        std::fs::write(
            owner_marker_path(&outside).expect("marker path"),
            "owner=paneflow\n",
        )
        .expect("outside marker");
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
        let _root = test_support::scoped_root(tmp.path().join("worktrees"));
        let repo_root = tmp.path().join("repo");
        let branch = "feat/a-b";
        let path = worktree_dir_hashed(&repo_root, branch);
        link_fake_git_dir(&path, &repo_root.join(".git/worktrees/hashed"));
        std::fs::write(
            owner_marker_path(&path).expect("marker path"),
            "owner=paneflow\n",
        )
        .expect("marker");

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
    fn managed_worktree_record_still_accepts_a_legacy_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _root = test_support::scoped_root(tmp.path().join("worktrees"));
        let repo_root = tmp.path().join("repo");
        let branch = "feat/legacy";
        let path = worktree_dir(&repo_root, branch);
        link_fake_git_dir(&path, &repo_root.join(".git/worktrees/feat-legacy"));
        std::fs::write(legacy_owner_marker_path(&path), "owner=paneflow\n").expect("legacy");

        assert!(
            managed_worktree_from_record(
                &path.to_string_lossy(),
                &repo_root.to_string_lossy(),
                branch,
                "auto",
            )
            .is_some(),
            "a session saved before the marker moved must still restore"
        );
    }

    fn test_git(cwd: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    fn init_test_repo(repo_root: &Path) -> bool {
        std::fs::create_dir_all(repo_root).expect("repo dir");
        if !test_git(repo_root, &["init", "-q", "-b", "main"]) {
            return false;
        }
        assert!(test_git(repo_root, &["config", "core.autocrlf", "false"]));
        std::fs::write(repo_root.join("README.md"), "init\n").expect("readme");
        assert!(test_git(repo_root, &["add", "README.md"]));
        test_git(
            repo_root,
            &[
                "-c",
                "user.email=paneflow@example.com",
                "-c",
                "user.name=Paneflow",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        )
    }

    #[test]
    fn a_created_checkout_is_clean_and_removable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _root = test_support::scoped_root(tmp.path().join("worktrees"));
        let repo_root = tmp.path().join("repo");
        if !init_test_repo(&repo_root) {
            return;
        }

        let path = create_branch_checkout(&repo_root, "feat/clean", None)
            .expect("create")
            .path;
        assert_eq!(path, worktree_dir(&repo_root, "feat/clean"));
        assert!(has_owner_marker(&path));
        assert!(
            !legacy_owner_marker_path(&path).exists(),
            "nothing of Paneflow may sit inside the checkout"
        );
        assert_eq!(
            is_clean(&path),
            Ok(true),
            "a fresh checkout must be clean, or teardown and removal never fire"
        );
        assert_eq!(remove_worktree(&repo_root, &path), Ok(()));
        assert!(!path.exists());
        assert!(
            branch_exists(&repo_root, "feat/clean"),
            "removing the checkout never deletes the branch"
        );
    }

    #[test]
    fn a_new_branch_starts_from_the_requested_base() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _root = test_support::scoped_root(tmp.path().join("worktrees"));
        let repo_root = tmp.path().join("repo");
        if !init_test_repo(&repo_root) {
            return;
        }
        assert!(test_git(&repo_root, &["branch", "develop"]));
        std::fs::write(repo_root.join("main-only.txt"), "x\n").expect("file");
        assert!(test_git(&repo_root, &["add", "main-only.txt"]));
        assert!(test_git(
            &repo_root,
            &[
                "-c",
                "user.email=paneflow@example.com",
                "-c",
                "user.name=Paneflow",
                "commit",
                "-q",
                "-m",
                "main moves on",
            ],
        ));

        let prepared = create_branch_checkout(&repo_root, "feat/from-develop", Some("develop"))
            .expect("create");
        assert!(prepared.created);
        let path = prepared.path;
        assert!(
            !path.join("main-only.txt").exists(),
            "the branch forked from develop, not from HEAD"
        );

        let again = create_branch_checkout(&repo_root, "feat/from-develop", Some("develop"))
            .expect("an existing branch is checked out, not refused");
        assert_eq!(again.path, path, "the live checkout is reused");
        assert!(!again.created);

        remove_worktree(&repo_root, &path).expect("remove");
        let back = create_branch_checkout(&repo_root, "feat/from-develop", None)
            .expect("a branch without a checkout gets a fresh worktree");
        assert_eq!(back.path, path);
        assert!(back.created);
        assert!(
            !back.path.join("main-only.txt").exists(),
            "the existing branch keeps its own history, the base is ignored"
        );
        assert!(
            create_branch_checkout(&repo_root, "bad name", None)
                .expect_err("space is invalid")
                .contains("not a valid branch name")
        );
    }

    #[test]
    fn copy_env_files_copies_top_level_env_only_and_never_clobbers() {
        let src = tempfile::tempdir().expect("src");
        let dst = tempfile::tempdir().expect("dst");
        std::fs::write(src.path().join(".env"), "A=1").unwrap();
        std::fs::write(src.path().join(".env.local"), "B=2").unwrap();
        std::fs::write(src.path().join("notenv"), "x").unwrap();
        std::fs::create_dir(src.path().join("sub")).unwrap();
        std::fs::write(src.path().join("sub/.env"), "C=3").unwrap();
        std::fs::write(dst.path().join(".env"), "KEEP").unwrap();

        let copied = copy_include_files(src.path(), dst.path());
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
        let copied = copy_include_files(Path::new("/nonexistent-paneflow-test"), dst.path());
        assert!(copied.is_empty());
    }

    #[test]
    fn worktreeinclude_lists_what_to_copy_and_replaces_the_env_default() {
        let src = tempfile::tempdir().expect("src");
        let dst = tempfile::tempdir().expect("dst");
        std::fs::write(src.path().join(".env"), "A=1").unwrap();
        std::fs::write(src.path().join("secrets.json"), "{}").unwrap();
        std::fs::create_dir_all(src.path().join("config/local")).unwrap();
        std::fs::write(src.path().join("config/local/dev.toml"), "x=1").unwrap();
        std::fs::write(src.path().join("AGENTS.override.md"), "local").unwrap();
        std::fs::write(
            src.path().join(".worktreeinclude"),
            "# ignored files to carry over\nsecrets.json\nconfig/local/\n../escape\n/abs\nmissing.txt\n",
        )
        .unwrap();

        let copied = copy_include_files(src.path(), dst.path());
        assert_eq!(
            copied,
            vec!["config/local/".to_string(), "secrets.json".to_string()]
        );
        assert!(dst.path().join("config/local/dev.toml").exists());
        assert!(
            !dst.path().join(".env").exists(),
            "an explicit list replaces the .env default"
        );
        assert!(!dst.path().join("AGENTS.override.md").exists());
    }

    #[test]
    fn agents_override_is_carried_over_by_default() {
        let src = tempfile::tempdir().expect("src");
        let dst = tempfile::tempdir().expect("dst");
        std::fs::write(src.path().join("AGENTS.override.md"), "local").unwrap();
        let copied = copy_include_files(src.path(), dst.path());
        assert_eq!(copied, vec!["AGENTS.override.md".to_string()]);
    }

    #[test]
    fn parse_worktree_include_rejects_escapes() {
        assert_eq!(
            parse_worktree_include("a\n# c\n\n./b/c\n../x\n/etc/passwd\n"),
            vec!["a".to_string(), "b/c".to_string()]
        );
    }

    #[test]
    fn parse_snapshot_refs_reads_trailers_newest_first() {
        let out = "refs/paneflow/snapshots/feat-x-100\0aaaa\0Paneflow-Path: /w/feat-x\nPaneflow-Branch: feat/x\nPaneflow-Head: 1111\nPaneflow-Taken-At: 100\n\u{1}\nrefs/paneflow/snapshots/dev-200\0bbbb\0Paneflow-Path: /w/dev-2\nPaneflow-Branch: \nPaneflow-Head: 2222\nPaneflow-Taken-At: 200\n\u{1}\n";
        let snaps = parse_snapshot_refs(out);
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].reference, "refs/paneflow/snapshots/dev-200");
        assert_eq!(snaps[0].branch, None, "a detached snapshot has no branch");
        assert_eq!(snaps[0].label(), "dev-2");
        assert_eq!(snaps[1].branch.as_deref(), Some("feat/x"));
        assert_eq!(snaps[1].path, PathBuf::from("/w/feat-x"));
        assert_eq!(snaps[1].head, "1111");
    }

    #[test]
    fn a_dirty_worktree_is_snapshotted_removed_and_restored_intact() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _root = test_support::scoped_root(tmp.path().join("worktrees"));
        let repo_root = tmp.path().join("repo");
        if !init_test_repo(&repo_root) {
            return;
        }
        let path = create_branch_checkout(&repo_root, "feat/snap", None)
            .expect("create")
            .path;
        std::fs::write(path.join("README.md"), "edited\n").expect("tracked edit");
        std::fs::write(path.join("notes.txt"), "untracked\n").expect("untracked file");
        assert_eq!(is_clean(&path), Ok(false));

        let snapshot = snapshot_and_remove(&repo_root, &path)
            .expect("remove")
            .expect("a dirty worktree leaves a snapshot");
        assert!(!path.exists());
        assert_eq!(snapshot.branch.as_deref(), Some("feat/snap"));
        assert!(branch_exists(&repo_root, "feat/snap"));
        let listed = list_snapshots(&repo_root).expect("list");
        assert_eq!(listed, vec![snapshot.clone()]);

        let restored = restore_snapshot(&repo_root, &snapshot).expect("restore");
        assert_eq!(restored, path, "the original Paneflow path is reused");
        assert_eq!(
            std::fs::read_to_string(restored.join("README.md")).expect("tracked"),
            "edited\n"
        );
        assert_eq!(
            std::fs::read_to_string(restored.join("notes.txt")).expect("untracked"),
            "untracked\n"
        );
        assert_eq!(
            is_clean(&restored),
            Ok(false),
            "changes come back uncommitted"
        );
        assert_eq!(current_branch(&restored).as_deref(), Some("feat/snap"));
        assert!(has_owner_marker(&restored));
        assert!(
            list_snapshots(&repo_root).expect("list").is_empty(),
            "a restored snapshot is consumed"
        );

        let clean = create_branch_checkout(&repo_root, "feat/clean", None)
            .expect("create")
            .path;
        let repo_dir = clean.parent().expect("repo dir").to_path_buf();
        assert_eq!(
            snapshot_and_remove(&repo_root, &clean).expect("remove"),
            None,
            "a clean worktree needs no snapshot"
        );
        assert!(repo_dir.exists(), "feat/snap still lives there");
        snapshot_and_remove(&repo_root, &restored).expect("remove the last one");
        assert!(
            !repo_dir.exists(),
            "the per-repository directory goes with its last worktree"
        );
    }

    #[test]
    fn switching_the_checkout_creates_reuses_or_detaches_without_a_worktree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _root = test_support::scoped_root(tmp.path().join("worktrees"));
        let repo_root = tmp.path().join("repo");
        if !init_test_repo(&repo_root) {
            return;
        }
        let before = list_worktrees(&repo_root).expect("list").len();

        switch_checkout(&repo_root, "feat/local", None).expect("create in place");
        assert_eq!(current_branch(&repo_root).as_deref(), Some("feat/local"));
        assert!(branch_exists(&repo_root, "feat/local"));

        std::fs::write(repo_root.join("wip.txt"), "wip\n").expect("dirty file");
        switch_checkout(&repo_root, "main", None).expect("existing branch carries the change");
        assert_eq!(current_branch(&repo_root).as_deref(), Some("main"));
        assert!(repo_root.join("wip.txt").exists());

        switch_checkout(&repo_root, "", Some("feat/local")).expect("detach at the base");
        assert_eq!(current_branch(&repo_root), None, "detached HEAD");
        assert!(
            switch_checkout(&repo_root, "bad name", None)
                .expect_err("space is invalid")
                .contains("not a valid branch name")
        );
        assert_eq!(
            list_worktrees(&repo_root).expect("list").len(),
            before,
            "switching never adds a worktree"
        );
    }

    #[test]
    fn a_detached_checkout_starts_at_the_base_and_can_be_named_later() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _root = test_support::scoped_root(tmp.path().join("worktrees"));
        let repo_root = tmp.path().join("repo");
        if !init_test_repo(&repo_root) {
            return;
        }
        assert!(test_git(&repo_root, &["branch", "develop"]));

        let path = create_detached_checkout(&repo_root, Some("develop")).expect("detached");
        let sha = resolve_base_commit(&repo_root, Some("develop")).expect("sha");
        assert_eq!(
            path,
            worktree_dir(&repo_root, &detached_checkout_label(Some("develop"), &sha))
        );
        let entries = list_worktrees(&repo_root).expect("list");
        let entry = entries.iter().find(|e| e.path == path).expect("registered");
        assert_eq!(entry.branch, None, "detached, no branch yet");
        assert!(has_owner_marker(&path));
        assert_eq!(is_clean(&path), Ok(true));

        let second = create_detached_checkout(&repo_root, Some("develop"))
            .expect("a second detached checkout from the same base gets its own dir");
        assert_ne!(second, path);
        assert_eq!(list_worktrees(&repo_root).expect("list").len(), 3);

        create_branch_here(&repo_root, &path, "feat/named-later").expect("branch here");
        let entries = list_worktrees(&repo_root).expect("list");
        let entry = entries.iter().find(|e| e.path == path).expect("registered");
        assert_eq!(entry.branch.as_deref(), Some("feat/named-later"));
        assert!(
            create_branch_here(&repo_root, &path, "feat/named-later")
                .expect_err("existing name refused")
                .contains("already exists")
        );
    }

    #[test]
    fn a_detached_checkout_is_named_by_what_distinguishes_it() {
        let repo = Path::new("/home/u/dev/paneflow");
        assert_eq!(
            checkout_label(Some("feat/login"), Path::new("/wt/feat-login"), repo),
            "feat/login"
        );
        assert_eq!(
            checkout_label(
                None,
                Path::new("/home/u/dev/worktrees/paneflow/poplar-plume/paneflow"),
                repo
            ),
            "poplar-plume"
        );
        assert_eq!(
            checkout_label(None, Path::new("/wt/hotfix-42"), repo),
            "hotfix-42"
        );
        assert_eq!(
            checkout_label(Some(""), Path::new("/wt/hotfix-42"), repo),
            "hotfix-42"
        );
    }
}
