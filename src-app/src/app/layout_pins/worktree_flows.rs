use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gpui::TestAppContext;
use paneflow_git::fixture;

use super::Rig;
use crate::PaneFlowApp;
use crate::workspace::worktree::{ManagedWorktree, TeardownPolicy};

pub(super) const PINS: usize = 6;

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

fn project(rig: &mut Rig<'_>, repo: &Path) -> usize {
    let ws = rig.workspace("alpha");
    let repo = repo.to_path_buf();
    rig.act(move |app, _, _| {
        let workspace = &mut app.workspaces[ws];
        workspace.cwd = repo.display().to_string();
        workspace.repo_root = Some(repo.clone());
        workspace.worktree_root = repo;
    });
    ws
}

fn launch_from_palette(rig: &mut Rig<'_>, branch: &str) {
    let branch = branch.to_string();
    rig.act(move |app, window, cx| {
        app.pane_palette_open_new_branch(window, cx);
        let draft = app
            .pane_palette
            .as_ref()
            .and_then(|palette| palette.new_branch.as_ref())
            .map(|draft| draft.name.clone())
            .expect("the New branch form is open");
        draft.update(cx, |input, cx| input.set_value(branch, cx));
        app.pane_palette_launch(0, window, cx);
    });
    wait_for(rig, "the branch checkout", |app| {
        app.branch_checkout_pending.is_empty()
            && app
                .pane_palette
                .as_ref()
                .is_none_or(|palette| palette.pending_launch.is_some() || palette.error.is_some())
    });
    let error = rig.act(|app, _, _| app.pane_palette.as_ref().and_then(|p| p.error.clone()));
    assert_eq!(error, None);
    rig.act(|app, window, cx| {
        if let Some(preset) = app.take_pane_palette_pending_launch() {
            app.pane_palette_launch(preset, window, cx);
        }
    });
}

fn current_branch(worktree: &Path) -> String {
    fixture::run(worktree, &["branch", "--show-current"])
        .trim()
        .to_string()
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

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_split_palette_new_branch_binds_the_tab_and_opens_the_pane_in_its_worktree(
    cx: &mut TestAppContext,
) {
    let root = tempfile::tempdir().expect("a repository root");
    let repo = committed_repo(root.path());
    let mut rig = Rig::mount(cx);
    let ws = project(&mut rig, &repo);
    let anchor = rig.pane("a");
    rig.act(|app, _, cx| {
        app.open_split_palette(anchor, crate::layout::SplitDirection::Vertical, cx)
    });

    launch_from_palette(&mut rig, "feat/split");

    let worktree = rig
        .act(|app, _, _| app.workspaces[ws].tabs()[0].worktree.clone())
        .expect("the split's tab is bound to the new worktree");
    assert_eq!(current_branch(&worktree), "feat/split");
    assert_eq!(rig.tabs(ws).len(), 1, "the split stays in its tab");
    assert_eq!(rig.shape(ws, 0), "V(0.500 a, 0.500 b)");
    let opened = rig.terminal("b");
    assert!(
        rig.cx
            .update(|_, cx| opened.read(cx).launched_under(&worktree)),
        "the new pane starts in {}",
        worktree.display()
    );
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_the_tab_menu_new_branch_binds_the_clicked_tab_and_adds_its_pane_there(
    cx: &mut TestAppContext,
) {
    let root = tempfile::tempdir().expect("a repository root");
    let repo = committed_repo(root.path());
    let mut rig = Rig::mount(cx);
    let ws = project(&mut rig, &repo);
    rig.act(|app, window, cx| {
        app.open_tab_with_surface(
            ws,
            "second".to_string(),
            paneflow_config::schema::TerminalSurfaceProfile::Normal,
            crate::app::workspace_ops::SurfaceLaunch::default(),
            window,
            cx,
        )
    });
    assert_eq!(rig.tabs(ws).len(), 2);

    rig.act(|app, window, cx| app.open_new_branch_for_tab(ws, 0, window, cx));
    launch_from_palette(&mut rig, "feat/menu");

    let (count, clicked, other) = rig.act(|app, _, _| {
        let tabs = app.workspaces[ws].tabs();
        (
            tabs.len(),
            tabs[0].worktree.clone(),
            tabs[1].worktree.clone(),
        )
    });
    assert_eq!(count, 2, "the menu opens no tab of its own");
    assert_eq!(other, None);
    let worktree = clicked.expect("the clicked tab is bound to the new worktree");
    assert_eq!(current_branch(&worktree), "feat/menu");
    assert_eq!(rig.shape(ws, 0), "V(0.500 a, 0.500 c)");
    let opened = rig.terminal("c");
    assert!(
        rig.cx
            .update(|_, cx| opened.read(cx).launched_under(&worktree)),
        "the new pane starts in {}",
        worktree.display()
    );
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_new_branch_titles_its_tab_unless_the_user_named_it(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().expect("a repository root");
    let repo = committed_repo(root.path());
    let mut rig = Rig::mount(cx);
    let ws = project(&mut rig, &repo);
    let title = |rig: &mut Rig<'_>, tab: usize| {
        rig.act(move |app, _, _| app.workspaces[ws].tabs()[tab].title().to_string())
    };

    rig.act(|app, window, cx| app.open_pane_palette(ws, window, cx));
    launch_from_palette(&mut rig, "mouse-one");
    assert_eq!(title(&mut rig, 1), "mouse-one");

    rig.act(|app, window, cx| app.open_pane_palette(ws, window, cx));
    launch_from_palette(&mut rig, "");
    let detached = rig
        .act(|app, _, _| app.workspaces[ws].tabs()[2].worktree.clone())
        .expect("the detached tab is bound to its worktree");
    let label = detached
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .expect("a worktree folder name");
    assert_eq!(title(&mut rig, 2), label);

    rig.act(|app, _, cx| {
        app.retitle_tab_through_model(
            ws,
            0,
            "mine",
            paneflow_config::schema::TabTitleSource::User,
            cx,
        )
    });
    rig.act(|app, window, cx| app.open_new_branch_for_tab(ws, 0, window, cx));
    launch_from_palette(&mut rig, "feat/renamed");
    assert!(
        rig.act(|app, _, _| app.workspaces[ws].tabs()[0].worktree.is_some()),
        "the renamed tab is still bound"
    );
    assert_eq!(title(&mut rig, 0), "mine");
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_the_tab_menu_offers_new_branch_in_a_repository_with_a_single_branch(
    cx: &mut TestAppContext,
) {
    let root = tempfile::tempdir().expect("a repository root");
    let repo = committed_repo(root.path());
    let mut rig = Rig::mount(cx);
    let ws = project(&mut rig, &repo);
    let offers = |rig: &mut Rig<'_>, ws: usize| {
        rig.act(move |app, _, _| {
            let key = crate::app::window::TabKey::of(&app.workspaces[ws], 0).expect("a tab");
            let menu = app.tab_menu(key).expect("the tab menu");
            (0..menu.entries().len()).any(|index| {
                matches!(
                    menu.action_at(index),
                    Some(crate::app::sidebar::context_menu::TabMenuAction::NewBranch)
                )
            })
        })
    };
    assert!(
        offers(&mut rig, ws),
        "a tab of a repository with only its default branch offers New branch…"
    );
    rig.act(move |app, _, _| app.workspaces[ws].repo_root = None);
    assert!(
        !offers(&mut rig, ws),
        "a tab outside any repository has no Branch section"
    );
}
