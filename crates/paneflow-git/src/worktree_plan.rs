use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::worktree;

pub const DEFAULT_SETUP_TIMEOUT: Duration = Duration::from_secs(300);
const SETUP_STDOUT_CAP: u64 = 8 * 1024 * 1024;
const SETUP_STDERR_TAIL_BYTES: usize = 64 * 1024;
const SETUP_STDERR_TAIL_LINES: usize = 20;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub branch: String,
    pub cwd: Option<String>,
    pub base: Option<String>,
    pub copy_env: Option<bool>,
    pub setup: Option<String>,
    pub setup_timeout_secs: Option<u64>,
    pub teardown: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub pane_idx: usize,
    pub repo_root: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    pub create: Option<bool>,
    pub base: Option<String>,
    pub copy_env: bool,
    pub setup: Option<String>,
    pub setup_timeout: Duration,
    pub teardown: String,
}

impl Plan {
    pub fn creates_worktree(&self) -> bool {
        self.create.is_some()
    }

    pub fn runs_setup(&self) -> bool {
        self.setup.as_deref().is_some_and(|setup| !setup.is_empty())
    }
}

pub fn plan(idx: usize, request: &Request) -> Result<Plan, String> {
    let branch = request.branch.as_str();
    if let Some(base) = request
        .base
        .as_deref()
        .map(str::trim)
        .filter(|base| base.starts_with('-'))
    {
        return Err(format!("pane {idx}: '{base}' is not a valid base"));
    }
    let cwd = expand_tilde(request.cwd.as_deref().unwrap_or_default());
    let git_dir = crate::repo::find_git_dir(&cwd).ok_or_else(|| {
        format!("pane {idx}: cwd '{cwd}' is not inside a git repository (required by `worktree`)")
    })?;
    let (repo_root, _) = crate::repo::resolve_repo_root(&git_dir);
    let repo_root = repo_root
        .ok_or_else(|| format!("pane {idx}: cannot resolve the repository root from '{cwd}'"))?;
    let legacy_path = worktree::worktree_dir(&repo_root, branch);
    let hashed_path = worktree::worktree_dir_hashed(&repo_root, branch);
    let mut path = legacy_path.clone();

    let entries = worktree::list_worktrees(&repo_root).map_err(|e| format!("pane {idx}: {e}"))?;
    let mut create: Option<bool> = Some(!worktree::branch_exists(&repo_root, branch));
    for entry in &entries {
        if entry.branch.as_deref() == Some(branch) {
            if worktree::is_owned_worktree(&repo_root, &entry.path) {
                path = entry.path.clone();
                create = None;
            } else {
                return Err(format!(
                    "pane {idx}: branch '{branch}' already checked out at {}",
                    entry.path.display()
                ));
            }
        }
    }
    if create.is_some()
        && entries
            .iter()
            .any(|entry| entry.path == legacy_path && entry.branch.as_deref() != Some(branch))
    {
        path = hashed_path;
    }
    validate_target(idx, &mut path, &repo_root, branch, &entries, &mut create)?;
    if create.is_some() && path.exists() {
        return Err(format!(
            "pane {idx}: {} exists but is not a registered worktree; remove it first",
            path.display()
        ));
    }

    Ok(Plan {
        pane_idx: idx,
        repo_root,
        path,
        branch: branch.to_string(),
        create,
        base: request.base.clone(),
        copy_env: request.copy_env.unwrap_or(true),
        setup: request.setup.clone(),
        setup_timeout: Duration::from_secs(
            request
                .setup_timeout_secs
                .unwrap_or(DEFAULT_SETUP_TIMEOUT.as_secs()),
        ),
        teardown: request
            .teardown
            .clone()
            .unwrap_or_else(|| "auto".to_string()),
    })
}

fn validate_target(
    idx: usize,
    path: &mut PathBuf,
    repo_root: &Path,
    branch: &str,
    entries: &[worktree::WorktreeEntry],
    create: &mut Option<bool>,
) -> Result<(), String> {
    for entry in entries {
        if entry.path != *path {
            continue;
        }
        if entry.branch.as_deref() == Some(branch) {
            *path = entry.path.clone();
            *create = None;
            return Ok(());
        }
        return Err(format!(
            "pane {idx}: {} exists but holds another branch ({}); \
             remove it or pick another worktree branch",
            path.display(),
            entry.branch.as_deref().unwrap_or("detached")
        ));
    }
    if !worktree::is_paneflow_worktree_dir(repo_root, branch, path) {
        return Err(format!(
            "pane {idx}: planned worktree path {} is outside Paneflow's worktree dirs",
            path.display()
        ));
    }
    Ok(())
}

pub fn resolve_conflicts(plans: &mut [Option<Plan>]) -> Result<(), String> {
    let mut groups: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for (idx, plan) in plans.iter().enumerate() {
        if let Some(plan) = plan {
            groups.entry(plan.path.clone()).or_default().push(idx);
        }
    }

    for indices in groups.values().filter(|indices| indices.len() > 1) {
        let mut branches: HashMap<String, usize> = HashMap::new();
        for &idx in indices {
            let Some(plan) = plans[idx].as_ref() else {
                continue;
            };
            if let Some(&first) = branches.get(&plan.branch) {
                return Err(format!(
                    "pane {} and pane {} both target worktree '{}' ({}) - \
                     a branch can only be checked out in one worktree",
                    first,
                    plan.pane_idx,
                    plan.branch,
                    plan.path.display()
                ));
            }
            branches.insert(plan.branch.clone(), plan.pane_idx);
        }

        let keep = indices
            .iter()
            .copied()
            .find(|&idx| {
                plans[idx]
                    .as_ref()
                    .is_some_and(|plan| plan.create.is_none())
            })
            .unwrap_or(indices[0]);
        for &idx in indices {
            if idx == keep {
                continue;
            }
            let Some(plan) = plans[idx].as_mut() else {
                continue;
            };
            let hashed = worktree::worktree_dir_hashed(&plan.repo_root, &plan.branch);
            if plan.path == hashed {
                continue;
            }
            plan.path = hashed;
            let entries = worktree::list_worktrees(&plan.repo_root)
                .map_err(|e| format!("pane {}: {e}", plan.pane_idx))?;
            let mut create = plan.create;
            validate_target(
                plan.pane_idx,
                &mut plan.path,
                &plan.repo_root,
                &plan.branch,
                &entries,
                &mut create,
            )?;
            plan.create = create;
            if plan.create.is_some() && plan.path.exists() {
                return Err(format!(
                    "pane {}: {} exists but is not a registered worktree; remove it first",
                    plan.pane_idx,
                    plan.path.display()
                ));
            }
        }
    }

    let mut seen: HashMap<&PathBuf, usize> = HashMap::new();
    for plan in plans.iter().flatten() {
        if let Some(&first) = seen.get(&plan.path) {
            return Err(format!(
                "pane {} and pane {} both target worktree '{}' ({}) - \
                 a branch can only be checked out in one worktree",
                first,
                plan.pane_idx,
                plan.branch,
                plan.path.display()
            ));
        }
        seen.insert(&plan.path, plan.pane_idx);
    }
    Ok(())
}

pub fn execute(plan: &Plan) -> Result<Vec<String>, String> {
    let mut notices = Vec::new();
    let Some(create_branch) = plan.create else {
        return Ok(notices);
    };
    worktree::add_worktree(
        &plan.repo_root,
        &plan.path,
        &plan.branch,
        create_branch,
        plan.base.as_deref(),
    )
    .map_err(|e| format!("pane {}: {e}", plan.pane_idx))?;

    if plan.copy_env {
        let outcome = worktree::copy_include_files(&plan.repo_root, &plan.path);
        if !outcome.copied.is_empty() {
            notices.push(format!(
                "pane {}: copied {} into {}",
                plan.pane_idx,
                outcome.copied.join(", "),
                plan.path.display()
            ));
        }
        for failure in outcome.failures {
            notices.push(format!("pane {}: {failure}", plan.pane_idx));
        }
    }

    if let Some(setup) = plan.setup.as_deref().filter(|s| !s.is_empty())
        && let Err(reason) = run_setup(setup, &plan.path, plan.setup_timeout)
    {
        notices.push(format!(
            "pane {}: setup failed in {} ({reason}) - agent started anyway",
            plan.pane_idx,
            plan.path.display()
        ));
    }
    Ok(notices)
}

#[derive(Debug, Default)]
pub struct Prepared {
    pub created: Vec<Plan>,
    pub notices: Vec<String>,
}

pub fn execute_all<'a>(plans: impl IntoIterator<Item = &'a Plan>) -> Result<Prepared, String> {
    let mut prepared = Prepared::default();
    for plan in plans {
        match execute(plan) {
            Ok(notices) => prepared.notices.extend(notices),
            Err(error) => {
                let created: Vec<&Plan> = prepared.created.iter().collect();
                for notice in rollback(&created) {
                    log::warn!("{notice}");
                }
                return Err(error);
            }
        }
        if plan.creates_worktree() {
            prepared.created.push(plan.clone());
        }
    }
    Ok(prepared)
}

fn setup_command(setup: &str) -> std::process::Command {
    #[cfg(unix)]
    let mut command = std::process::Command::new("sh");
    #[cfg(unix)]
    command.arg("-c");
    #[cfg(windows)]
    let mut command = std::process::Command::new("cmd");
    #[cfg(windows)]
    command.arg("/C");
    command.arg(setup);
    command
}

pub fn run_setup(setup: &str, cwd: &Path, timeout: Duration) -> Result<(), String> {
    let mut command = setup_command(setup);
    command.current_dir(cwd);
    let tail = Arc::new(Mutex::new(Vec::new()));
    let sink = tail.clone();
    let result = paneflow_process::run_with_timeout_tapping_stderr(
        command,
        timeout,
        SETUP_STDOUT_CAP,
        move |chunk| {
            if let Ok(mut tail) = sink.lock() {
                tail.extend_from_slice(chunk);
                let excess = tail.len().saturating_sub(SETUP_STDERR_TAIL_BYTES);
                tail.drain(..excess);
            }
        },
    );
    match result {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!("exit {}", out.status.code().unwrap_or(-1))),
        Err(paneflow_process::ProcError::Timeout) => {
            let last_lines = tail
                .lock()
                .map(|tail| last_stderr_lines(&tail, SETUP_STDERR_TAIL_LINES))
                .unwrap_or_default();
            let mut reason = format!("killed after its {}s timeout", timeout.as_secs());
            if !last_lines.is_empty() {
                reason.push_str("; last stderr lines:\n");
                reason.push_str(&last_lines);
            }
            Err(reason)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn last_stderr_lines(stderr: &[u8], count: usize) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

pub fn rollback(plans: &[&Plan]) -> Vec<String> {
    let mut notices = Vec::new();
    for plan in plans.iter().rev() {
        if !plan.creates_worktree() {
            continue;
        }
        if let Err(e) = worktree::remove_worktree(&plan.repo_root, &plan.path) {
            notices.push(format!(
                "pane {}: could not roll back worktree {} ({e})",
                plan.pane_idx,
                plan.path.display()
            ));
        }
    }
    notices
}

pub fn expand_tilde(raw: &str) -> String {
    if raw == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    } else if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    } else if let Some(rest) = raw.strip_prefix("~\\")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;

    #[cfg(unix)]
    const LOUD_SETUP: &str =
        "head -c 1048576 /dev/zero | tr '\\0' x >&2; echo done > setup-finished";
    #[cfg(windows)]
    const LOUD_SETUP: &str = "powershell -NoProfile -NonInteractive -Command \
         \"[Console]::Error.Write('x' * 1048576)\" & echo done> setup-finished";

    #[cfg(unix)]
    const HANGING_SETUP: &str = "for i in $(seq 1 30); do echo line$i >&2; done; sleep 30";
    #[cfg(windows)]
    const HANGING_SETUP: &str = "(for /L %i in (1,1,30) do @echo line%i 1>&2) & \
         powershell -NoProfile -NonInteractive -Command Start-Sleep 30";

    fn dummy_plan(idx: usize, path: &str, branch: &str) -> Plan {
        Plan {
            pane_idx: idx,
            repo_root: PathBuf::from("/r"),
            path: PathBuf::from(path),
            branch: branch.to_string(),
            create: Some(true),
            base: None,
            copy_env: true,
            setup: None,
            setup_timeout: Duration::from_secs(1),
            teardown: "auto".to_string(),
        }
    }

    fn committed_repo(root: &Path) {
        std::fs::create_dir_all(root).expect("repo dir");
        fixture::init(root);
        std::fs::write(root.join("README.md"), "init\n").expect("readme");
        fixture::commit_all(root, "init");
    }

    #[test]
    fn a_request_outside_a_repo_is_an_atomic_error() {
        let request = Request {
            branch: "feat/x".to_string(),
            cwd: Some("/".to_string()),
            ..Request::default()
        };
        let err = plan(3, &request).unwrap_err();
        assert!(err.contains("pane 3"), "got: {err}");
        assert!(err.contains("not inside a git repository"), "got: {err}");
    }

    #[test]
    fn duplicate_worktree_paths_are_an_atomic_validation_error() {
        let mut plans = vec![
            Some(dummy_plan(0, "/r.worktrees/feat-x", "feat/x")),
            None,
            Some(dummy_plan(2, "/r.worktrees/feat-x", "feat/x")),
        ];
        let err = resolve_conflicts(&mut plans).unwrap_err();
        assert!(
            err.contains("pane 0") && err.contains("pane 2"),
            "both panes cited: {err}"
        );

        let mut distinct = vec![
            Some(dummy_plan(0, "/r.worktrees/feat-x", "feat/x")),
            Some(dummy_plan(1, "/r.worktrees/feat-y", "feat/y")),
        ];
        assert!(resolve_conflicts(&mut distinct).is_ok());
    }

    #[test]
    fn slug_collision_branches_move_to_hashed_fallback() {
        let _root =
            worktree::test_support::scoped_root(PathBuf::from("/home/a/paneflow-worktrees"));
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir(&repo_root).expect("repo dir");
        fixture::init(&repo_root);

        let branch_a = "feat/a b";
        let branch_b = "feat/a-b";
        let legacy = worktree::worktree_dir(&repo_root, branch_a);
        assert_eq!(legacy, worktree::worktree_dir(&repo_root, branch_b));
        let hashed_b = worktree::worktree_dir_hashed(&repo_root, branch_b);

        let colliding = |idx: usize, branch: &str| Plan {
            pane_idx: idx,
            repo_root: repo_root.clone(),
            path: legacy.clone(),
            branch: branch.to_string(),
            create: Some(true),
            base: None,
            copy_env: true,
            setup: None,
            setup_timeout: Duration::from_secs(1),
            teardown: "auto".to_string(),
        };
        let mut plans = vec![Some(colliding(0, branch_a)), Some(colliding(1, branch_b))];

        resolve_conflicts(&mut plans).expect("slug collision resolves");
        assert_eq!(plans[0].as_ref().unwrap().path, legacy);
        assert_eq!(plans[1].as_ref().unwrap().path, hashed_b);
    }

    #[test]
    fn a_setup_that_floods_stderr_still_runs_to_completion() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        committed_repo(&repo);
        let mut plan = dummy_plan(0, "", "feat/loud");
        plan.repo_root = repo;
        plan.path = tmp.path().join("checkout");
        plan.copy_env = false;
        plan.setup = Some(LOUD_SETUP.to_string());
        plan.setup_timeout = Duration::from_secs(60);

        let notices = execute(&plan).expect("worktree created");

        assert!(notices.is_empty(), "{notices:?}");
        assert!(
            plan.path.join("setup-finished").exists(),
            "1 MiB of stderr must not kill the setup before it finishes"
        );
    }

    #[test]
    fn a_setup_past_its_timeout_is_killed_and_reports_its_last_stderr_lines() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reason = run_setup(HANGING_SETUP, tmp.path(), Duration::from_secs(3))
            .expect_err("a hanging setup must be killed");
        assert!(reason.contains("3s timeout"), "got: {reason}");
        let (_, tail) = reason
            .split_once("last stderr lines:\n")
            .unwrap_or_else(|| panic!("no stderr tail in: {reason}"));
        let lines: Vec<&str> = tail.lines().map(str::trim).collect();
        let expected: Vec<String> = (11..=30).map(|i| format!("line{i}")).collect();
        assert_eq!(lines, expected);
    }

    #[test]
    fn a_failing_plan_rolls_back_the_worktrees_created_before_it() {
        let _root = worktree::test_support::scoped_root(
            tempfile::tempdir().expect("root").keep().join("worktrees"),
        );
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        committed_repo(&repo);
        let request = |branch: &str| Request {
            branch: branch.to_string(),
            cwd: Some(repo.display().to_string()),
            copy_env: Some(false),
            ..Request::default()
        };
        let first = plan(0, &request("feat/first")).expect("a plan");
        let mut broken = plan(1, &request("feat/second")).expect("a plan");
        broken.base = Some("no-such-base".to_string());

        let error = execute_all([&first, &broken]).expect_err("the second plan fails");

        assert!(error.contains("pane 1"), "{error}");
        assert!(
            !first.path.exists(),
            "the first worktree is removed when a later one fails"
        );
        let listed = worktree::list_worktrees(&repo).expect("worktrees");
        assert!(
            listed.iter().all(|entry| entry.path != first.path),
            "{listed:?}"
        );
    }

    #[test]
    fn a_base_that_reads_as_a_git_option_is_refused_before_git_runs() {
        let request = Request {
            branch: "feat/option".to_string(),
            cwd: Some("/nonexistent-paneflow-repo".to_string()),
            base: Some(" --force".to_string()),
            ..Request::default()
        };
        let error = plan(2, &request).expect_err("an option-like base is refused");
        assert_eq!(error, "pane 2: '--force' is not a valid base");
    }

    #[test]
    fn expand_tilde_expands_home_prefix_only() {
        let home = dirs::home_dir()
            .expect("home")
            .to_string_lossy()
            .into_owned();
        assert_eq!(expand_tilde("~"), home);
        assert!(expand_tilde("~/dev").starts_with(&home));
        assert!(expand_tilde("~\\dev").starts_with(&home));
        assert_eq!(expand_tilde("/abs/path"), "/abs/path");
        assert_eq!(expand_tilde("rel/~nothome"), "rel/~nothome");
    }
}
