use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::TestAppContext;
use paneflow_ipc_client::host_control::HostControl;
use paneflow_layout::{Op, Snapshot};
use serde_json::{Value, json};

use super::Rig;
use crate::PaneFlowApp;
use crate::{ClosePane, SplitVertically};

pub(super) const PINS: usize = 15;

const PATIENCE: Duration = Duration::from_secs(60);

fn home() -> PathBuf {
    let home = paneflow_home::paneflow_home().expect("the isolated home");
    let session = paneflow_home::session_path_in(&home);
    let _ = std::fs::remove_file(paneflow_config::session_file::pending_session_path(
        &session,
    ));
    let _ = std::fs::remove_file(session);
    crate::layout_ids::source().release_lease();
    home
}

type Created = std::sync::Arc<std::sync::Mutex<Vec<paneflow_host::CreateSession>>>;

#[derive(Clone, Default)]
struct RecordingStarter(Created);

struct RecordingLink(Created);

impl paneflow_serve::launcher::SessionStarter for RecordingStarter {
    fn connect(
        &self,
    ) -> Result<Box<dyn paneflow_serve::launcher::SessionLink>, paneflow_serve::launcher::LaunchError>
    {
        Ok(Box::new(RecordingLink(self.0.clone())))
    }
}

impl paneflow_serve::launcher::SessionLink for RecordingLink {
    fn exists(
        &mut self,
        session: &paneflow_config::schema::SessionId,
    ) -> Result<bool, paneflow_serve::launcher::LaunchError> {
        Ok(self
            .0
            .lock()
            .expect("the record")
            .iter()
            .any(|created| created.session.as_ref() == Some(session)))
    }

    fn create(
        &mut self,
        request: &paneflow_host::CreateSession,
    ) -> Result<(), paneflow_serve::launcher::LaunchError> {
        self.0.lock().expect("the record").push(request.clone());
        Ok(())
    }
}

struct Running(Option<paneflow_serve::worker::RunningWorker>, Created);

impl Running {
    fn open(home: &Path) -> Self {
        let starter = RecordingStarter::default();
        let created = starter.0.clone();
        let worker = paneflow_serve::open_with(
            home,
            paneflow_serve::Launching {
                support: paneflow_serve::launcher::LaunchSupport::default(),
                starter: Some(std::sync::Arc::new(starter)),
            },
        )
        .expect("a worker");
        Self(Some(worker), created)
    }

    fn created(&self) -> Vec<paneflow_host::CreateSession> {
        self.1.lock().expect("the record").clone()
    }

    fn stop(mut self) {
        if let Some(worker) = self.0.take() {
            worker.stop();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(worker) = self.0.take() {
            worker.stop();
        }
    }
}

fn control(home: &Path) -> HostControl {
    let mut control = HostControl::connect(
        &paneflow_home::serve_endpoint_path(home),
        "layout-pin-worker",
    )
    .expect("the worker answers");
    control.set_max_frame(paneflow_serve::layout_authority::MAX_LAYOUT_FRAME_BYTES);
    control
}

fn worker_view(home: &Path) -> Snapshot {
    let view: Value = control(home)
        .request("layout.view", json!({}))
        .expect("layout.view answers");
    serde_json::from_value(view["view"].clone()).expect("the view is a snapshot")
}

fn file_revision(home: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(paneflow_home::session_path_in(home)).ok()?;
    serde_json::from_str::<paneflow_config::schema::SessionState>(&text)
        .ok()?
        .layout_revision
}

fn exported(snapshot: Snapshot) -> paneflow_config::schema::SessionState {
    let mut model = paneflow_layout::Model::new();
    model.restore(snapshot);
    paneflow_layout::export(&model, &[])
}

fn window_snapshot(rig: &mut Rig<'_>) -> Snapshot {
    rig.cx
        .update(|_, cx| rig.app.read(cx).layout.model().snapshot())
}

fn pump(rig: &mut Rig<'_>, what: &str, done: impl Fn(&PaneFlowApp) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        rig.act(|app, _, cx| app.process_layout_client(cx));
        let app = rig.app.clone();
        if rig.cx.update(|_, cx| done(app.read(cx))) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn settled(app: &PaneFlowApp) -> bool {
    app.layout.sync.is_settled()
        && app
            .layout
            .model()
            .ids()
            .remaining()
            .is_some_and(|left| left > 0)
}

fn assert_converged(rig: &mut Rig<'_>, home: &Path) {
    pump(rig, "the window settling with the worker", settled);
    let window = window_snapshot(rig);
    let worker = worker_view(home);
    assert_eq!(window.revision, worker.revision, "the revisions agree");
    assert_eq!(
        window.state, worker.state,
        "the worker holds the window's layout"
    );
    assert_eq!(exported(window), exported(worker));
}

fn attach(rig: &mut Rig<'_>) {
    rig.act(|app, _, _| app.start_layout_client());
    pump(rig, "the window attaching", settled);
}

fn finish(rig: &mut Rig<'_>, worker: Running, home: &Path) {
    rig.act(|app, _, _| {
        app.session_exit_pending = true;
        app.save_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    worker.stop();
    rig.cx.run_until_parked();
    let _ = std::fs::remove_file(paneflow_home::session_path_in(home));
}

fn split(rig: &mut Rig<'_>) {
    rig.act(|app, window, cx| app.handle_split_v(&SplitVertically, window, cx));
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_an_attached_window_sends_every_gesture_and_the_worker_holds_the_same_layout(
    cx: &mut TestAppContext,
) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    let worker = Running::open(&home);
    attach(&mut rig);
    assert_converged(&mut rig, &home);

    rig.focus("a");
    split(&mut rig);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b)");
    assert_eq!(rig.focused().as_deref(), Some("b"));
    assert_converged(&mut rig, &home);
    let bound = worker_view(&home)
        .state
        .workspaces
        .iter()
        .flat_map(|ws| ws.tabs.iter())
        .filter_map(|tab| tab.root.as_ref())
        .flat_map(paneflow_layout::Node::panes)
        .flat_map(|pane| pane.surfaces.iter())
        .filter(|surface| surface.session().is_some())
        .count();
    assert_eq!(bound, 2, "the new terminal's session reached the worker");

    let sent = rig.cx.update(|_, cx| rig.app.read(cx).layout.sync.sent());
    let drag = rig.drag_divider(&[], 0.3, 0.7);
    rig.act(|app, _, cx| app.adopt_layout(cx));
    assert_eq!(
        rig.cx.update(|_, cx| rig.app.read(cx).layout.sync.sent()),
        sent,
        "a divider drag in progress sends nothing"
    );
    rig.release_divider(drag);
    assert_eq!(
        rig.cx.update(|_, cx| rig.app.read(cx).layout.sync.sent()),
        sent + 1,
        "the end of a drag sends one op"
    );
    assert_converged(&mut rig, &home);

    rig.focus("b");
    rig.act(|app, window, cx| app.handle_close_pane(&ClosePane, window, cx));
    assert_eq!(rig.shape(0, 0), "a");
    assert_converged(&mut rig, &home);
    assert!(
        rig.requests()
            .iter()
            .all(|request| !request.starts_with("stop")),
        "an attached window leaves stops to the worker"
    );
    finish(&mut rig, worker, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_keyboard_split_in_an_attached_window_is_started_by_the_worker(cx: &mut TestAppContext) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    assert_eq!(
        rig.requests(),
        ["create a"],
        "a window alone starts its own terminals"
    );
    let worker = Running::open(&home);
    attach(&mut rig);
    assert_converged(&mut rig, &home);

    rig.focus("a");
    split(&mut rig);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b)");
    assert_eq!(
        rig.requests(),
        ["launch b"],
        "the attached window asks the worker and creates nothing itself"
    );
    let pane = rig.pane("b");
    let (surface, session) = rig
        .cx
        .update(|_, cx| {
            pane.read(cx).active_terminal_opt().map(|view| {
                let view = view.read(cx);
                (view.surface_id(), view.terminal.session_id.clone())
            })
        })
        .expect("b holds a terminal");
    let deadline = Instant::now() + PATIENCE;
    while worker.created().is_empty() {
        assert!(Instant::now() < deadline, "the worker never started b");
        std::thread::sleep(Duration::from_millis(5));
    }
    let created = worker.created();
    assert_eq!(created.len(), 1, "exactly one creator for b");
    assert_eq!(created[0].session.as_ref(), Some(&session));
    let variables = &created[0].env;
    assert_eq!(
        variables.get("PANEFLOW_SURFACE_ID"),
        Some(&surface.to_string())
    );
    assert_converged(&mut rig, &home);
    finish(&mut rig, worker, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_focus_sent_to_the_worker_moves_the_keyboard_focus_of_the_attached_window(
    cx: &mut TestAppContext,
) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split(&mut rig);
    let worker = Running::open(&home);
    attach(&mut rig);
    assert_converged(&mut rig, &home);
    rig.focus("b");
    assert_eq!(rig.focused().as_deref(), Some("b"));
    let a = rig.surface_id("a");

    let focused: Value = control(&home)
        .request("surface.focus", json!({"surface_id": a}))
        .expect("the worker focuses a");
    assert_eq!(focused["focused"], true, "{focused}");
    let deadline = Instant::now() + PATIENCE;
    while rig.focused().as_deref() != Some("a") {
        rig.act(|app, _, cx| app.process_layout_client(cx));
        rig.requests();
        assert!(
            Instant::now() < deadline,
            "the window never moved the keyboard focus to a"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_converged(&mut rig, &home);
    finish(&mut rig, worker, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_window_opened_after_a_headless_up_draws_it_and_attaches_the_worker_sessions(
    cx: &mut TestAppContext,
) {
    let home = home();
    let worker = Running::open(&home);
    let cwd = tempfile::tempdir().expect("a pane directory");
    let opened: Value = control(&home)
        .request(
            "workspace.up",
            json!({"name": "headless", "panes": [{"cwd": cwd.path()}, {"cwd": cwd.path()}]}),
        )
        .expect("the worker opens the workspace with no window");
    assert_eq!(opened["panes"], 2, "{opened}");
    let created: Vec<paneflow_config::schema::SessionId> = worker
        .created()
        .into_iter()
        .filter_map(|request| request.session)
        .collect();
    assert_eq!(created.len(), 2, "the worker started both terminals");
    let revision = worker_view(&home).revision;
    let deadline = Instant::now() + PATIENCE;
    while file_revision(&home) != Some(revision) {
        assert!(
            Instant::now() < deadline,
            "the worker never wrote its layout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let text =
        std::fs::read_to_string(paneflow_home::session_path_in(&home)).expect("the session file");
    let state: paneflow_config::schema::SessionState =
        serde_json::from_str(&text).expect("a session file");

    let view = worker_view(&home);
    let session_of_surface = |id: u64| {
        view.state
            .surface(paneflow_layout::SurfaceId(id))
            .and_then(|known| known.definition.session.clone())
            .expect("a bound surface")
    };
    let names = vec![
        (
            session_of_surface(opened["surface_ids"][0].as_u64().expect("an id")),
            "a".to_string(),
        ),
        (
            session_of_surface(opened["surface_ids"][1].as_u64().expect("an id")),
            "b".to_string(),
        ),
    ];
    let mut rig = Rig::mount_restored(cx, &state, names);
    attach(&mut rig);
    assert_converged(&mut rig, &home);
    assert_eq!(rig.workspaces(), ["*headless"]);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.500 b)");
    let mut drawn = vec![rig.session_of("a"), rig.session_of("b")];
    drawn.sort();
    let mut expected = created;
    expected.sort();
    assert_eq!(drawn, expected, "the window attached the worker's sessions");
    assert_eq!(
        worker.created().len(),
        2,
        "nobody created a session when the window opened"
    );
    finish(&mut rig, worker, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_an_op_from_another_client_is_drawn_by_the_attached_window(cx: &mut TestAppContext) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    let worker = Running::open(&home);
    attach(&mut rig);
    let workspace = window_snapshot(&mut rig).state.workspaces[0].id;
    let op = serde_json::to_value(Op::RenameWorkspace {
        workspace,
        title: "renamed elsewhere".to_string(),
    })
    .expect("the op encodes");
    let applied: Value = control(&home)
        .request("layout.apply", json!({"op": op}))
        .expect("the worker applies the op");
    assert!(applied["revision"].as_u64().is_some(), "{applied}");
    pump(&mut rig, "the window drawing the foreign op", |app| {
        app.workspaces
            .first()
            .is_some_and(|ws| ws.title == "renamed elsewhere")
    });
    assert_converged(&mut rig, &home);
    assert_eq!(rig.workspaces(), ["*renamed elsewhere"]);
    finish(&mut rig, worker, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_workspace_another_client_opens_is_drawn_with_its_terminal(cx: &mut TestAppContext) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    let worker = Running::open(&home);
    attach(&mut rig);
    let opened: Value = control(&home)
        .request("workspace.create", json!({"name": "remote"}))
        .expect("the worker opens the workspace");
    assert_eq!(opened["panes"], 1, "{opened}");
    pump(&mut rig, "the window drawing the remote workspace", |app| {
        app.workspaces.iter().any(|ws| ws.title == "remote")
    });
    let app = rig.app.clone();
    let terminals: Vec<usize> = rig.cx.update(|_, cx| {
        let app = app.read(cx);
        let remote = app
            .workspaces
            .iter()
            .find(|ws| ws.title == "remote")
            .expect("the remote workspace");
        crate::workspace::panes_across(std::slice::from_ref(remote))
            .iter()
            .map(|pane| pane.read(cx).terminals().count())
            .collect()
    });
    assert_eq!(terminals, [1], "the remote pane shows its terminal");
    assert_converged(&mut rig, &home);
    finish(&mut rig, worker, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_every_pane_of_a_workspace_another_client_opens_shares_its_durable_id(
    cx: &mut TestAppContext,
) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    let worker = Running::open(&home);
    attach(&mut rig);
    control(&home)
        .request("workspace.create", json!({"name": "remote"}))
        .expect("the worker opens the workspace");
    pump(&mut rig, "the window drawing the remote workspace", |app| {
        app.workspaces.iter().any(|ws| ws.title == "remote")
    });
    rig.act(|app, _, cx| app.adopt_layout(cx));
    assert_converged(&mut rig, &home);
    let remote_surface = |snapshot: &Snapshot| {
        snapshot
            .state
            .workspaces
            .iter()
            .find(|ws| ws.title == "remote")
            .and_then(|ws| ws.tabs.first())
            .and_then(|tab| tab.root.as_ref())
            .and_then(|root| root.first_pane())
            .and_then(|pane| pane.surfaces.first())
            .map(|surface| surface.id.0)
            .expect("the remote workspace holds a surface")
    };
    let anchor = remote_surface(&worker_view(&home));
    let split: Value = control(&home)
        .request(
            "surface.split",
            json!({"surface_id": anchor, "direction": "vertical"}),
        )
        .expect("the worker splits the remote pane");
    assert_eq!(split["split"], true, "{split}");
    pump(&mut rig, "the window drawing the split", |app| {
        app.workspaces
            .iter()
            .find(|ws| ws.title == "remote")
            .is_some_and(|ws| crate::workspace::panes_across(std::slice::from_ref(ws)).len() == 2)
    });
    assert_converged(&mut rig, &home);

    let window_durable = {
        let app = rig.app.clone();
        rig.cx.update(|_, cx| {
            app.read(cx)
                .workspaces
                .iter()
                .find(|ws| ws.title == "remote")
                .map(|ws| ws.durable_id.clone())
                .expect("the remote workspace")
        })
    };
    let registered: Vec<Option<paneflow_config::schema::WorkspaceId>> = worker
        .created()
        .into_iter()
        .map(|request| request.workspace)
        .collect();
    assert_eq!(
        registered,
        vec![Some(window_durable.clone()), Some(window_durable.clone())],
        "the host registers both panes under the window's workspace"
    );
    let model_durable = worker_view(&home)
        .state
        .workspaces
        .iter()
        .find(|ws| ws.title == "remote")
        .and_then(|ws| ws.extras.durable_id.clone());
    assert_eq!(model_durable, Some(window_durable));
    finish(&mut rig, worker, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_worktree_the_worker_creates_is_listed_by_the_window_and_removed_when_it_closes(
    cx: &mut TestAppContext,
) {
    let home = home();
    let repo_dir = tempfile::tempdir().expect("a repository directory");
    let repo = repo_dir.path().join("repo");
    paneflow_git::fixture::committed_repo(&repo, &[("README.md", "init\n")]);
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    let worker = Running::open(&home);
    attach(&mut rig);
    control(&home)
        .request(
            "workspace.up",
            json!({"name": "remote", "panes": [{"cwd": repo, "worktree": {"branch": "feat/remote"}}]}),
        )
        .expect("the worker creates the worktree");
    pump(&mut rig, "the window drawing the remote workspace", |app| {
        app.workspaces.iter().any(|ws| ws.title == "remote")
    });
    let listed: Vec<(String, PathBuf)> = rig.cx.update(|_, cx| {
        let app = rig.app.read(cx);
        app.managed_worktrees_snapshot()
            .into_iter()
            .filter_map(|(ws_id, wt)| {
                let ws = app.workspaces.iter().find(|ws| ws.id == ws_id)?;
                Some((ws.title.clone(), wt.path))
            })
            .collect()
    });
    let [(owner, path)] = listed.as_slice() else {
        panic!("the window lists the worker's worktree once: {listed:?}");
    };
    assert_eq!(owner, "remote");
    assert!(path.join("README.md").is_file());

    let path = path.clone();
    assert_converged(&mut rig, &home);
    rig.act(|app, window, cx| {
        let idx = app
            .workspaces
            .iter()
            .position(|ws| ws.title == "remote")
            .expect("the remote workspace");
        app.remove_workspace(idx, Some(window), cx);
    });
    let deadline = Instant::now() + PATIENCE;
    while path.exists() {
        if Instant::now() >= deadline {
            let toasts = rig.toasts();
            let titles: Vec<String> = rig.cx.update(|_, cx| {
                let app = rig.app.read(cx);
                app.workspaces
                    .iter()
                    .map(|ws| format!("{} {:?}", ws.title, ws.managed_worktrees))
                    .collect()
            });
            panic!(
                "closing the workspace never removed its worktree: {toasts:?} {titles:?} {:?}",
                worker_view(&home).state.workspaces
            );
        }
        rig.act(|app, _, cx| app.process_layout_client(cx));
        std::thread::sleep(Duration::from_millis(5));
    }
    finish(&mut rig, worker, &home);
}

fn shown_name(rig: &mut Rig<'_>, surface: u64) -> Option<String> {
    let app = rig.app.clone();
    rig.cx.update(|_, cx| {
        crate::workspace::panes_across(&app.read(cx).workspaces)
            .iter()
            .flat_map(|pane| pane.read(cx).terminals().cloned().collect::<Vec<_>>())
            .find(|terminal| terminal.read(cx).surface_id() == surface)
            .and_then(|terminal| terminal.read(cx).terminal.custom_name.clone())
    })
}

fn model_name(snapshot: &Snapshot, surface: u64) -> Option<String> {
    snapshot
        .state
        .surface(paneflow_layout::SurfaceId(surface))
        .and_then(|known| known.definition.custom_name.clone())
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_surface_another_client_renames_shows_its_name_and_keeps_it_through_a_gesture(
    cx: &mut TestAppContext,
) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    let worker = Running::open(&home);
    attach(&mut rig);
    let surface = rig.model_surface_ids()[0];
    let renamed: Value = control(&home)
        .request(
            "surface.rename",
            json!({"surface_id": surface, "name": "tests"}),
        )
        .expect("the worker renames the surface");
    assert_eq!(renamed["renamed"], true, "{renamed}");
    pump(&mut rig, "the window adopting the foreign rename", |app| {
        app.layout
            .model()
            .state()
            .surface(paneflow_layout::SurfaceId(surface))
            .is_some_and(|known| known.definition.custom_name.as_deref() == Some("tests"))
    });
    assert_eq!(
        shown_name(&mut rig, surface).as_deref(),
        Some("tests"),
        "the window shows the name another client gave"
    );
    split(&mut rig);
    assert_converged(&mut rig, &home);
    assert_eq!(shown_name(&mut rig, surface).as_deref(), Some("tests"));
    assert_eq!(
        model_name(&worker_view(&home), surface).as_deref(),
        Some("tests"),
        "a gesture in the window keeps the name another client gave"
    );
    finish(&mut rig, worker, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_split_the_worker_refuses_shows_the_split_toast_and_adopts_the_worker_view(
    cx: &mut TestAppContext,
) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split(&mut rig);
    let worker = Running::open(&home);
    attach(&mut rig);
    let pane = rig.pane("b");
    let b = rig
        .cx
        .update(|_, cx| rig.app.read(cx).layout_pane_id(&pane))
        .expect("b has a model id")
        .0;
    let op = serde_json::to_value(Op::ClosePane {
        pane: paneflow_layout::PaneId(b),
        keep_undo: true,
    })
    .expect("the op encodes");
    rig.focus("b");
    pump(&mut rig, "the window settling before the race", settled);
    rig.act(|app, window, cx| {
        control(&home)
            .request("layout.apply", json!({"op": op}))
            .expect("the worker closes b");
        app.handle_split_v(&SplitVertically, window, cx);
    });
    pump(&mut rig, "the refusal reaching the window", |app| {
        app.toast
            .iter()
            .chain(app.toast_queue.iter())
            .any(|toast| toast.message == "That pane no longer exists")
    });
    assert_converged(&mut rig, &home);
    assert_eq!(rig.shape(0, 0), "a");
    finish(&mut rig, worker, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_the_window_keeps_every_gesture_while_the_worker_is_replaced_and_converges(
    cx: &mut TestAppContext,
) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    let worker = Running::open(&home);
    attach(&mut rig);
    rig.focus("a");
    split(&mut rig);
    assert_converged(&mut rig, &home);

    worker.stop();
    pump(&mut rig, "the window noticing the worker left", |app| {
        !app.layout_is_attached()
    });
    rig.focus("b");
    split(&mut rig);
    assert_eq!(rig.shape(0, 0), "V(0.500 a, 0.250 b, 0.250 c)");
    rig.act(|app, window, cx| app.handle_close_pane(&ClosePane, window, cx));
    assert_eq!(rig.shape(0, 0), "V(0.625 a, 0.375 b)");
    assert_eq!(rig.focused().as_deref(), Some("b"));
    let revision = rig.revision();
    rig.act(|app, _, cx| app.save_session(cx));
    pump(&mut rig, "the fallback write", |_| {
        file_revision(&home) == Some(revision)
    });
    let successor = Running::open(&home);
    assert_converged(&mut rig, &home);
    assert!(
        rig.toasts().is_empty(),
        "a window whose model is newest adopts without a refresh toast: {:?}",
        rig.toasts()
    );
    finish(&mut rig, successor, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_reconnect_the_worker_refuses_refreshes_the_layout_with_one_toast(cx: &mut TestAppContext) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    let worker = Running::open(&home);
    attach(&mut rig);
    let workspace = window_snapshot(&mut rig).state.workspaces[0].id;
    worker.stop();
    pump(&mut rig, "the window noticing the worker left", |app| {
        !app.layout_is_attached()
    });
    let successor = Running::open(&home);
    wait_for_the_window(&home);
    for title in ["first", "second"] {
        let op = serde_json::to_value(Op::RenameWorkspace {
            workspace,
            title: title.to_string(),
        })
        .expect("the op encodes");
        control(&home)
            .request("layout.apply", json!({"op": op}))
            .expect("the successor applies the op");
    }
    pump(&mut rig, "the refused adopt reaching the window", |app| {
        app.workspaces
            .first()
            .is_some_and(|ws| ws.title == "second")
    });
    assert_converged(&mut rig, &home);
    let refreshed = rig
        .toasts()
        .into_iter()
        .filter(|toast| toast == crate::app::layout_bridge::sync::LAYOUT_REFRESHED)
        .count();
    assert_eq!(refreshed, 1, "{:?}", rig.toasts());
    finish(&mut rig, successor, &home);
}

fn wait_for_the_window(home: &Path) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let status: Value = control(home)
            .request("worker.status", json!({}))
            .expect("worker.status answers");
        if status["layout"]["attached"]
            .as_array()
            .is_some_and(|attached| !attached.is_empty())
        {
            return;
        }
        assert!(Instant::now() < deadline, "the window never attached");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_lease_used_up_while_the_worker_is_away_refuses_a_split_with_its_toast(
    cx: &mut TestAppContext,
) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    let worker = Running::open(&home);
    attach(&mut rig);
    worker.stop();
    pump(&mut rig, "the window noticing the worker left", |app| {
        !app.layout_is_attached()
    });
    let ids = rig.cx.update(|_, cx| rig.app.read(cx).layout.model().ids());
    while ids.remaining().is_some_and(|left| left > 0) {
        ids.allocate();
    }
    rig.focus("a");
    let before = rig.shape(0, 0);
    split(&mut rig);
    assert_eq!(rig.shape(0, 0), before, "nothing was created");
    assert_eq!(
        rig.toasts(),
        [format!(
            "Maximum pane count reached ({})",
            crate::layout::MAX_PANES
        )]
    );
    finish(&mut rig, Running(None, Created::default()), &home);
}

fn worker_sessions(home: &Path) -> Vec<Option<paneflow_config::schema::SessionId>> {
    worker_view(home)
        .state
        .workspaces
        .iter()
        .flat_map(|ws| ws.tabs.iter())
        .filter_map(|tab| tab.root.as_ref())
        .flat_map(paneflow_layout::Node::panes)
        .flat_map(|pane| pane.surfaces.iter())
        .map(|surface| surface.definition.session.clone())
        .collect()
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_stopping_everything_keeps_the_worker_layout_with_its_panes_and_no_session(
    cx: &mut TestAppContext,
) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    rig.focus("a");
    split(&mut rig);
    let worker = Running::open(&home);
    attach(&mut rig);
    assert_converged(&mut rig, &home);
    assert_eq!(worker_sessions(&home).len(), 2);
    let quit = std::rc::Rc::new(std::cell::Cell::new(false));
    let done = quit.clone();
    rig.act(|app, _, cx| app.save_stopped_session_before_exit(cx, move |_, _| done.set(true)));
    pump(&mut rig, "the stopping quit", |_| quit.get());
    assert_eq!(
        worker_sessions(&home),
        vec![None, None],
        "the worker keeps both panes and forgets their sessions"
    );
    let revision = worker_view(&home).revision;
    let deadline = Instant::now() + PATIENCE;
    while file_revision(&home) != Some(revision) {
        assert!(
            Instant::now() < deadline,
            "the worker never wrote the stopped layout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let text =
        std::fs::read_to_string(paneflow_home::session_path_in(&home)).expect("the session file");
    let state: paneflow_config::schema::SessionState =
        serde_json::from_str(&text).expect("a session file");
    fn walk<'a>(
        node: &'a paneflow_config::schema::LayoutNode,
        found: &mut Vec<&'a paneflow_config::schema::SurfaceDefinition>,
    ) {
        match node {
            paneflow_config::schema::LayoutNode::Pane { surfaces } => found.extend(surfaces),
            paneflow_config::schema::LayoutNode::Split { children, .. } => {
                for child in children {
                    walk(child, found);
                }
            }
        }
    }
    let mut written = Vec::new();
    for layout in state
        .workspaces
        .iter()
        .flat_map(|ws| ws.tabs.iter())
        .filter_map(|tab| tab.layout.as_ref())
    {
        walk(layout, &mut written);
    }
    assert_eq!(written.len(), 2, "the file keeps both panes");
    assert!(
        written.iter().all(|surface| surface.session.is_none()),
        "the next start opens a fresh shell per pane"
    );
    finish(&mut rig, worker, &home);
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_quitting_leaves_the_last_model_on_disk_whoever_writes_it(cx: &mut TestAppContext) {
    let home = home();
    let mut rig = Rig::mount(cx);
    rig.workspace("alpha");
    let worker = Running::open(&home);
    attach(&mut rig);
    rig.focus("a");
    split(&mut rig);
    let attached_revision = rig.revision();
    let quit = std::rc::Rc::new(std::cell::Cell::new(false));
    let done = quit.clone();
    rig.act(|app, _, cx| app.save_session_before_exit(cx, move |_, _| done.set(true)));
    pump(&mut rig, "the attached quit", |_| quit.get());
    assert!(
        worker_sessions(&home).iter().all(Option::is_some),
        "quitting keeps every terminal bound in the worker's layout"
    );
    worker.stop();
    assert_eq!(
        file_revision(&home),
        Some(attached_revision),
        "the worker flushed the window's last model when it stopped"
    );
    pump(&mut rig, "the window noticing the worker left", |app| {
        !app.layout_is_attached()
    });
    rig.act(|app, window, cx| app.handle_close_pane(&ClosePane, window, cx));
    let away_revision = rig.revision();
    let quit = std::rc::Rc::new(std::cell::Cell::new(false));
    let done = quit.clone();
    rig.act(|app, _, cx| app.save_session_before_exit(cx, move |_, _| done.set(true)));
    pump(&mut rig, "the quit while the worker is away", |_| {
        quit.get()
    });
    assert_eq!(file_revision(&home), Some(away_revision));
    finish(&mut rig, Running(None, Created::default()), &home);
}
