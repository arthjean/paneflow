#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use paneflow_git::fixture;
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

mod common;

const REPLY_WAIT: Duration = Duration::from_secs(120);
const LOCAL_FILE: &str = "local.settings";

static WORKTREES_ROOT_IS_PROCESS_WIDE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn one_worker_plans_worktrees_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    WORKTREES_ROOT_IS_PROCESS_WIDE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn call(home: &Path, method: &str, params: Value) -> Result<Value, String> {
    HostControl::connect(
        &paneflow_home::serve_endpoint_path(home),
        "worker-worktrees-test",
    )
    .map_err(|error| format!("the worker does not answer: {error}"))?
    .request_with_deadline(method, params, REPLY_WAIT)
}

struct Repo {
    _root: tempfile::TempDir,
    repo: PathBuf,
    worktrees: PathBuf,
}

fn repo_with_a_local_file(home: &Path) -> Repo {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    fixture::committed_repo(
        &repo,
        &[
            ("README.md", "init\n"),
            (".gitignore", "local.settings\n"),
            (".worktreeinclude", "local.settings\n"),
        ],
    );
    std::fs::write(repo.join(LOCAL_FILE), "mode=local\n").unwrap();
    let worktrees = root.path().join("worktrees");
    std::fs::write(
        home.join("paneflow.json"),
        serde_json::to_vec(&json!({"worktrees": {"dir": worktrees}})).unwrap(),
    )
    .unwrap();
    Repo {
        _root: root,
        repo,
        worktrees,
    }
}

fn workspaces(home: &Path) -> Vec<Value> {
    call(home, "layout.view", json!({})).unwrap()["view"]["state"]["workspaces"]
        .as_array()
        .unwrap()
        .clone()
}

fn branches(repo: &Path) -> String {
    fixture::run(repo, &["branch", "--list"])
}

#[test]
fn workspace_up_creates_the_worktree_starts_the_pane_in_it_and_records_it() {
    let _serial = one_worker_plans_worktrees_at_a_time();
    let home = tempfile::tempdir().unwrap();
    let repo = repo_with_a_local_file(home.path());
    let worker = common::open_starting_terminals(home.path());

    let opened = call(
        home.path(),
        "workspace.up",
        json!({"name": "feature", "panes": [
            {"cwd": repo.repo, "worktree": {"branch": "feat/login"}},
            {"cwd": repo.repo}
        ]}),
    )
    .expect("the worker creates the worktree");

    assert!(branches(&repo.repo).contains("feat/login"));
    let index = opened["index"].as_u64().unwrap() as usize;
    let ws = workspaces(home.path())[index].clone();
    let recorded = ws["extras"]["managed_worktrees"].as_array().unwrap();
    assert_eq!(recorded.len(), 1, "{ws}");
    let path = PathBuf::from(recorded[0]["path"].as_str().unwrap());
    assert!(
        path.starts_with(&repo.worktrees),
        "{} is under the configured worktree dir {}",
        path.display(),
        repo.worktrees.display()
    );
    assert!(path.join("README.md").is_file(), "the checkout exists");
    assert_eq!(
        std::fs::read_to_string(path.join(LOCAL_FILE)).unwrap(),
        "mode=local\n",
        "the file .worktreeinclude names is copied into the new worktree"
    );
    assert_eq!(recorded[0]["branch"], "feat/login");
    assert_eq!(recorded[0]["teardown"], "auto");
    let tabs = ws["tabs"].as_array().unwrap();
    let bound = tabs
        .iter()
        .find(|tab| tab["worktree"].as_str().map(PathBuf::from) == Some(path.clone()))
        .expect("the worktree has its own tab");
    assert_eq!(bound["title"], "feat/login");
    let text = bound.to_string();
    let canonical =
        paneflow_launch::cwd::strip_verbatim_prefix(std::fs::canonicalize(&path).unwrap());
    let quoted = serde_json::to_string(&canonical.display().to_string()).unwrap();
    assert!(
        text.contains(quoted.trim_matches('"')),
        "the pane starts in the worktree: {text}"
    );
    let notices = opened["notices"].as_array().expect("notices");
    assert!(
        notices
            .iter()
            .any(|notice| notice.as_str().unwrap().contains(LOCAL_FILE)),
        "{notices:?}"
    );

    let reused = call(
        home.path(),
        "workspace.up",
        json!({"panes": [{"cwd": repo.repo, "worktree": {"branch": "feat/login"}}]}),
    )
    .expect("a second up reuses the worktree Paneflow owns");
    let index = reused["index"].as_u64().unwrap() as usize;
    assert_eq!(
        workspaces(home.path())[index]["extras"]["managed_worktrees"][0]["path"]
            .as_str()
            .map(PathBuf::from),
        Some(path)
    );
    worker.stop();
}

#[test]
fn a_failing_worktree_rolls_back_the_ones_created_before_it_and_changes_no_layout() {
    let _serial = one_worker_plans_worktrees_at_a_time();
    let home = tempfile::tempdir().unwrap();
    let repo = repo_with_a_local_file(home.path());
    let worker = common::open_starting_terminals(home.path());
    let before = workspaces(home.path()).len();

    let refused = call(
        home.path(),
        "workspace.up",
        json!({"panes": [
            {"cwd": repo.repo, "worktree": {"branch": "feat/first"}},
            {"cwd": repo.repo, "worktree": {"branch": "feat/second", "from": "no-such-base"}}
        ]}),
    )
    .expect_err("the second worktree cannot be created");

    assert!(refused.contains("pane 1"), "{refused}");
    let listed = fixture::run(&repo.repo, &["worktree", "list", "--porcelain"]);
    assert!(
        !listed.contains("feat/first") && !listed.contains("feat/second"),
        "every worktree of the call is gone: {listed}"
    );
    assert_eq!(
        workspaces(home.path()).len(),
        before,
        "the layout is unchanged"
    );
    worker.stop();
}

#[test]
fn a_worktree_setup_needs_orchestration_and_nothing_is_created_without_it() {
    let _serial = one_worker_plans_worktrees_at_a_time();
    if std::env::var_os("PANEFLOW_IPC_ORCHESTRATION").is_some()
        || std::env::var_os("PANEFLOW_IPC_SCRIPTING").is_some()
    {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let repo = repo_with_a_local_file(home.path());
    let worker = common::open_starting_terminals(home.path());

    let refused = call(
        home.path(),
        "workspace.up",
        json!({"panes": [{"cwd": repo.repo, "worktree": {"branch": "feat/setup", "setup": "make"}}]}),
    )
    .expect_err("a setup runs a command");
    assert!(
        refused.contains("PANEFLOW_IPC_ORCHESTRATION=1"),
        "{refused}"
    );
    assert!(
        !branches(&repo.repo).contains("feat/setup"),
        "no branch was created"
    );

    let exclusive = call(
        home.path(),
        "surface.split",
        json!({"direction": "vertical", "worktree": {"branch": "feat/x"}, "managed_worktree": {"path": "/tmp"}}),
    )
    .expect_err("the two worktree forms are exclusive");
    assert!(exclusive.contains("exclusive"), "{exclusive}");
    worker.stop();
}

#[test]
fn a_split_with_a_worktree_opens_a_tab_for_it_beside_the_anchor() {
    let _serial = one_worker_plans_worktrees_at_a_time();
    let home = tempfile::tempdir().unwrap();
    let repo = repo_with_a_local_file(home.path());
    let worker = common::open_starting_terminals(home.path());
    let opened = call(
        home.path(),
        "workspace.up",
        json!({"name": "main", "panes": [{"cwd": repo.repo}]}),
    )
    .unwrap();
    let anchor = opened["surface_ids"][0].as_u64().unwrap();

    let split = call(
        home.path(),
        "surface.split",
        json!({"surface_id": anchor, "direction": "vertical", "cwd": repo.repo,
               "worktree": {"branch": "feat/side", "copy_env": false}}),
    )
    .expect("the worker creates the worktree for the split");

    assert_eq!(split["split"], true);
    let index = opened["index"].as_u64().unwrap() as usize;
    let ws = workspaces(home.path())[index].clone();
    let tabs = ws["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 2, "the worktree pane opens its own tab: {ws}");
    let path = PathBuf::from(tabs[1]["worktree"].as_str().expect("a bound tab"));
    assert!(path.join("README.md").is_file());
    assert!(
        !path.join(LOCAL_FILE).exists(),
        "copy_env false copies nothing"
    );
    assert!(branches(&repo.repo).contains("feat/side"));
    worker.stop();
}

fn worktree_paths(repo: &Path) -> Vec<PathBuf> {
    fixture::run(repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect()
}

#[test]
fn a_host_refusal_after_the_layout_took_the_worktree_keeps_the_worktree_it_points_at() {
    let _serial = one_worker_plans_worktrees_at_a_time();
    let home = tempfile::tempdir().unwrap();
    let repo = repo_with_a_local_file(home.path());
    let worker = common::open_refusing_terminals(home.path());
    let before = workspaces(home.path()).len();

    let refused = call(
        home.path(),
        "workspace.up",
        json!({"name": "refused", "panes": [{"cwd": repo.repo, "worktree": {"branch": "feat/kept"}}]}),
    )
    .expect_err("the host refuses the terminal");
    assert!(refused.contains(common::REFUSAL), "{refused}");

    let all = workspaces(home.path());
    assert_eq!(all.len(), before + 1, "the layout took the workspace");
    let recorded = all[before]["extras"]["managed_worktrees"][0]["path"]
        .as_str()
        .map(PathBuf::from)
        .expect("the workspace records its worktree");
    assert!(
        recorded.join("README.md").is_file(),
        "the worktree the layout records still exists"
    );
    let recorded_file = std::fs::canonicalize(&recorded).unwrap();
    assert!(
        worktree_paths(&repo.repo)
            .iter()
            .any(|listed| std::fs::canonicalize(listed).ok().as_ref() == Some(&recorded_file)),
        "git lists the worktree the layout records"
    );

    let split = call(
        home.path(),
        "surface.split",
        json!({"direction": "vertical", "cwd": repo.repo,
               "worktree": {"branch": "feat/split-kept", "copy_env": false}}),
    )
    .expect_err("the host refuses the split's terminal too");
    assert!(split.contains(common::REFUSAL), "{split}");
    let tabs = workspaces(home.path())[before]["tabs"]
        .as_array()
        .unwrap()
        .clone();
    let split_path = tabs
        .iter()
        .filter_map(|tab| tab["worktree"].as_str().map(PathBuf::from))
        .find(|path| *path != recorded)
        .expect("the split's worktree tab stays in the layout");
    assert!(
        split_path.join("README.md").is_file(),
        "the split's worktree still exists"
    );
    worker.stop();
}
