use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gpui::TestAppContext;
use paneflow_git::fixture;

use super::Rig;
use crate::PaneFlowApp;
use crate::workspace::worktree::{ManagedWorktree, TeardownPolicy};

pub(super) const PINS: usize = 2;

const OUTCOME_WAIT: Duration = Duration::from_secs(60);
const PUMP_INTERVAL: Duration = Duration::from_millis(5);

fn committed_repo(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    fixture::committed_repo(&repo, &[("README.md", "init\n")]);
    repo
}

fn managed(repo: &Path, branch: &str, age: Duration) -> ManagedWorktree {
    let path = crate::workspace::worktree::create_branch_checkout(repo, branch, None)
        .expect("the worktree is created")
        .path;
    let marker = crate::workspace::worktree::owner_marker_path(&path).expect("an owner marker");
    std::fs::File::options()
        .write(true)
        .open(&marker)
        .and_then(|file| file.set_modified(SystemTime::now() - age))
        .expect("the marker is aged");
    ManagedWorktree {
        path,
        repo_root: repo.to_path_buf(),
        branch: branch.to_string(),
        teardown: TeardownPolicy::Auto,
    }
}

fn wait_for(rig: &mut Rig<'_>, what: &str, done: impl Fn(&PaneFlowApp) -> bool) {
    let deadline = Instant::now() + OUTCOME_WAIT;
    loop {
        rig.cx.run_until_parked();
        let app = rig.app.clone();
        if rig.cx.update(|_, cx| done(app.read(cx))) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(PUMP_INTERVAL);
    }
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_the_keep_limit_trims_the_oldest_worktrees_while_the_window_is_inactive(
    cx: &mut TestAppContext,
) {
    let root = tempfile::tempdir().expect("a repository root");
    let repo = committed_repo(root.path());
    let oldest = managed(&repo, "lim/a", Duration::from_secs(300));
    let older = managed(&repo, "lim/b", Duration::from_secs(200));
    let newest = managed(&repo, "lim/c", Duration::from_secs(100));
    let mut rig = Rig::mount(cx);
    let ws = rig.workspace("alpha");
    rig.act(|app, _, _| {
        app.workspaces[ws].managed_worktrees = vec![oldest.clone(), older.clone(), newest.clone()];
        app.cached_config.worktrees.keep_limit = Some(1);
    });
    crate::agents::notifications::forget_active_windows();
    assert!(!crate::agents::notifications::window_active());

    rig.act(|app, _, cx| app.git_poll_tick(Instant::now(), cx));

    wait_for(&mut rig, "the keep-limit trim", |app| {
        app.workspaces[ws].managed_worktrees.len() == 1
    });
    let kept: Vec<PathBuf> = rig.act(|app, _, _| {
        app.workspaces[ws]
            .managed_worktrees
            .iter()
            .map(|wt| wt.path.clone())
            .collect()
    });
    assert_eq!(kept, vec![newest.path.clone()]);
    assert!(!oldest.path.exists(), "{}", oldest.path.display());
    assert!(!older.path.exists(), "{}", older.path.display());
    assert!(newest.path.exists(), "{}", newest.path.display());
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_worktree_kept_when_the_last_workspace_closes_stays_listed_and_removable(
    cx: &mut TestAppContext,
) {
    let root = tempfile::tempdir().expect("a repository root");
    let repo = committed_repo(root.path());
    let mut kept = managed(&repo, "kept/one", Duration::from_secs(10));
    kept.teardown = TeardownPolicy::Keep;
    let mut rig = Rig::mount(cx);
    let ws = rig.workspace("alpha");
    rig.act(|app, _, _| app.workspaces[ws].managed_worktrees = vec![kept.clone()]);

    rig.act(|app, window, cx| app.remove_workspace(ws, Some(window), cx));

    let path = kept.path.clone();
    wait_for(&mut rig, "the kept worktree to be listed", |app| {
        app.workspaces.is_empty() && app.managed_worktrees_snapshot() == vec![(None, kept.clone())]
    });
    assert!(path.exists());
    let ledger = paneflow_home::kept_worktrees_path().expect("a ledger path");
    wait_for(&mut rig, "the ledger to name the kept worktree", |_| {
        crate::app::kept_worktrees::read_ledger(&ledger) == vec![kept.clone()]
    });

    rig.act(|app, _, cx| app.request_worktree_removal(path.clone(), cx));

    wait_for(&mut rig, "the removal from Settings", |app| {
        app.managed_worktrees_snapshot().is_empty() && !path.exists()
    });
    wait_for(&mut rig, "the ledger to forget it", |_| !ledger.exists());
}
