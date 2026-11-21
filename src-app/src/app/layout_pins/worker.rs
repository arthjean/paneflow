use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::TestAppContext;
use paneflow_ipc_client::host_control::HostControl;
use paneflow_layout::{Op, Snapshot};
use serde_json::{Value, json};

use super::Rig;
use crate::PaneFlowApp;
use crate::{ClosePane, SplitVertically};

pub(super) const PINS: usize = 7;

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

struct Running(Option<paneflow_serve::worker::RunningWorker>);

impl Running {
    fn open(home: &Path) -> Self {
        Self(Some(paneflow_serve::open(home).expect("a worker")))
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
    control(&home)
        .request("layout.apply", json!({"op": op}))
        .expect("the worker closes b");
    rig.focus("b");
    split(&mut rig);
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
    finish(&mut rig, Running(None), &home);
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
    finish(&mut rig, Running(None), &home);
}
