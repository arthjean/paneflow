use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::TestAppContext;

use super::Rig;
use crate::PaneFlowApp;
use crate::workspace::worktree::{ManagedWorktree, TeardownPolicy};

pub(super) const PINS: usize = 4;

const PATIENCE: Duration = Duration::from_secs(20);

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a repository directory");
        let root = dir.path().join("repo");
        paneflow_git::fixture::committed_repo(&root, &[("README.md", "init\n")]);
        Self { _dir: dir, root }
    }

    fn checkout(&self, branch: &str) -> PathBuf {
        paneflow_git::worktree::create_branch_checkout(&self.root, branch, None)
            .expect("a managed worktree")
            .path
    }

    fn managed(&self, path: &Path, branch: &str) -> ManagedWorktree {
        ManagedWorktree {
            path: path.to_path_buf(),
            repo_root: self.root.clone(),
            branch: branch.to_string(),
            teardown: TeardownPolicy::Auto,
        }
    }
}

fn listed(rig: &mut Rig<'_>) -> Vec<(String, PathBuf)> {
    let app = rig.app.clone();
    rig.cx.update(|_, cx| {
        let app = app.read(cx);
        app.managed_worktrees_snapshot()
            .into_iter()
            .filter_map(|(ws_id, wt)| {
                let ws = app.workspaces.iter().find(|ws| ws.id == ws_id)?;
                Some((ws.title.clone(), wt.path))
            })
            .collect()
    })
}

fn wait_until(rig: &mut Rig<'_>, what: &str, done: impl Fn(&mut Rig<'_>) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done(rig) {
        assert!(Instant::now() < deadline, "{what} never happened");
        rig.act(|_, _, _| {});
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn close(rig: &mut Rig<'_>, title: &str) {
    rig.act(|app: &mut PaneFlowApp, window, cx| {
        let idx = app
            .workspaces
            .iter()
            .position(|ws| ws.title == title)
            .expect("the workspace to close");
        app.remove_workspace(idx, Some(window), cx);
    });
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_closing_a_workspace_keeps_the_worktree_a_tab_of_another_workspace_uses(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    let repo = Repo::new();
    let shared = repo.checkout("feat/shared");
    let spare = repo.checkout("feat/spare");
    let alpha = rig.workspace("alpha");
    let beta = rig.workspace("beta");
    let managed = vec![
        repo.managed(&shared, "feat/shared"),
        repo.managed(&spare, "feat/spare"),
    ];
    let bound = shared.clone();
    rig.act(move |app, _, cx| {
        app.set_tab_worktree(alpha, 0, Some(bound.clone()), cx);
        app.set_tab_worktree(beta, 0, Some(bound), cx);
        app.workspaces[beta].managed_worktrees = managed;
    });

    close(&mut rig, "beta");
    wait_until(&mut rig, "the unused worktree is removed", |_| {
        !spare.exists()
    });

    assert!(
        shared.join("README.md").is_file(),
        "the worktree the tab of alpha uses is still on disk"
    );
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_worktree_kept_when_its_workspace_closes_stays_listed_and_removable(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    let repo = Repo::new();
    let kept = repo.checkout("feat/kept");
    rig.workspace("alpha");
    let beta = rig.workspace("beta");
    let managed = vec![repo.managed(&kept, "feat/kept")];
    let bound = kept.clone();
    rig.act(move |app, _, cx| {
        app.cached_config.worktrees.auto_remove = Some(false);
        app.set_tab_worktree(beta, 0, Some(bound), cx);
        app.workspaces[beta].managed_worktrees = managed;
    });

    close(&mut rig, "beta");
    wait_until(&mut rig, "the kept worktree is listed by alpha", |rig| {
        listed(rig) == [("alpha".to_string(), kept.clone())]
    });
    assert!(kept.join("README.md").is_file(), "automatic removal is off");

    let path = kept.clone();
    rig.act(move |app, _, cx| {
        let ws_id = app.workspaces[0].id;
        app.remove_managed_worktree(ws_id, path, cx);
    });
    wait_until(&mut rig, "the listed worktree is removed by hand", |rig| {
        !kept.exists() && listed(rig).is_empty()
    });
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_opening_the_worktrees_settings_lists_the_snapshots_already_on_disk(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    let repo = Repo::new();
    let dirty = repo.checkout("feat/dirty");
    std::fs::write(dirty.join("wip.txt"), "unsaved work\n").expect("a dirty file");
    paneflow_git::worktree::snapshot_and_remove(&repo.root, &dirty)
        .expect("the worktree is removed")
        .expect("its changes are snapshotted");
    let alpha = rig.workspace("alpha");
    let root = repo.root.clone();
    rig.act(move |app, _, _| {
        app.workspaces[alpha].cwd = root.display().to_string();
        app.workspaces[alpha].repo_root = Some(root);
    });
    assert!(
        rig.cx
            .update(|_, cx| rig.app.read(cx).worktree_snapshots().is_empty()),
        "nothing listed the repository yet"
    );

    rig.act(|app, window, cx| {
        app.open_settings_at(crate::SettingsSection::Worktrees, window, cx);
    });
    wait_until(&mut rig, "the snapshot is listed", |rig| {
        let app = rig.app.clone();
        rig.cx
            .update(|_, cx| app.read(cx).worktree_snapshots().len() == 1)
    });
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_closing_a_workspace_lets_its_sessions_end_before_removing_its_worktree(
    cx: &mut TestAppContext,
) {
    let mut rig = Rig::mount(cx);
    let repo = Repo::new();
    let owned = repo.checkout("feat/owned");
    rig.workspace("alpha");
    let beta = rig.workspace("beta");
    let managed = vec![repo.managed(&owned, "feat/owned")];
    let bound = owned.clone();
    rig.act(move |app, _, cx| {
        app.set_tab_worktree(beta, 0, Some(bound), cx);
        app.workspaces[beta].managed_worktrees = managed;
    });
    rig.requests();

    close(&mut rig, "beta");
    wait_until(
        &mut rig,
        "the worktree of the closed workspace is removed",
        |_| !owned.exists(),
    );

    assert_eq!(
        rig.requests(),
        ["stop b"],
        "the session of beta was stopped"
    );
    assert!(listed(&mut rig).is_empty(), "nothing is left to list");
}
