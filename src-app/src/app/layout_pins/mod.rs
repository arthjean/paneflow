mod create_close;
mod events;
mod ids;
mod lua;
mod parity;
mod prompts;
mod rearrange;
mod sidebar;
mod worker;
mod worktrees;

use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AppContext as _, Context, Entity, Focusable as _, TestAppContext, VisualTestContext, Window,
};
use paneflow_config::schema::{LayoutNode, SessionId, SessionState};
use serde_json::Value;

use crate::PaneFlowApp;
use crate::app::bootstrap::Startup;
use crate::app::fake_host::{FakeHost, HostRequest};
use crate::app::ipc_handler::{IpcReply, promote_response};
use crate::pane::{Pane, PaneEvent};
use crate::terminal::TerminalView;
use crate::terminal::view::DisplayOnlyTerminals;
use crate::workspace::{Workspace, next_workspace_id_in};

const CHILD_ENV: &str = "PANEFLOW_LAYOUT_PINS_CHILD";
const PIN_FILTER: &str = "app::layout_pins::";
const IPC_WAIT: Duration = Duration::from_secs(30);
const PUMP_INTERVAL: Duration = Duration::from_millis(2);

pub(crate) fn isolated_child(home: &std::path::Path, args: &[&str]) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command
        .args(args)
        .env(CHILD_ENV, "1")
        .env(paneflow_home::HOME_ENV, home)
        .env("PANEFLOW_ALLOW_MULTIPLE", "1")
        .env_remove("PANEFLOW_SOCKET_PATH")
        .env_remove("PANEFLOW_ALLOW_SOCKET_OVERRIDE")
        .env_remove("PANEFLOW_IPC_SCRIPTING")
        .env_remove("PANEFLOW_IPC_ORCHESTRATION")
        .env_remove("PANEFLOW_SURFACE_ID")
        .env_remove("PANEFLOW_SESSION_ID");
    command
}

#[test]
fn every_layout_pin_holds_in_an_isolated_home() {
    let home = tempfile::tempdir().expect("a pin home");
    let output = isolated_child(home.path(), &["--ignored", "--test-threads=1", PIN_FILTER])
        .output()
        .expect("the pin runner starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "a layout pin failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
    );
    let passed = stdout
        .lines()
        .filter(|line| line.starts_with("test app::layout_pins::") && line.ends_with(" ok"))
        .count();
    assert!(
        passed
            >= create_close::PINS
                + events::PINS
                + ids::PINS
                + lua::PINS
                + parity::PINS
                + prompts::PINS
                + rearrange::PINS
                + sidebar::PINS
                + worker::PINS
                + worktrees::PINS,
        "only {passed} layout pins ran\n{stdout}"
    );
}

pub(super) fn forget_the_previous_pin() {
    assert_isolated_home();
    let deadline = Instant::now() + Duration::from_secs(60);
    while crate::app::session::session_writes_in_flight() > 0 {
        assert!(
            Instant::now() < deadline,
            "a session write of an earlier pin never finished"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let Some(session) = paneflow_config::loader::session_path() else {
        return;
    };
    let _ = std::fs::remove_file(paneflow_config::session_file::pending_session_path(
        &session,
    ));
    let _ = std::fs::remove_file(&session);
}

pub(super) fn assert_isolated_home() {
    assert_eq!(
        std::env::var(CHILD_ENV).as_deref(),
        Ok("1"),
        "a layout pin runs only in the isolated home of every_layout_pin_holds_in_an_isolated_home"
    );
}

pub(crate) struct Rig<'a> {
    pub(crate) cx: &'a mut VisualTestContext,
    pub(crate) app: Entity<PaneFlowApp>,
    host: FakeHost,
    names: Vec<(SessionId, String)>,
    log: Vec<HostRequest>,
}

impl<'a> Rig<'a> {
    pub(crate) fn mount(cx: &'a mut TestAppContext) -> Self {
        forget_the_previous_pin();
        Self::mount_with(cx, FakeHost::default())
    }

    fn mount_with(cx: &'a mut TestAppContext, host: FakeHost) -> Self {
        assert_isolated_home();
        cx.executor().allow_parking();
        cx.update(|cx| {
            cx.set_global(DisplayOnlyTerminals);
            cx.set_global(host.clone());
        });
        let cx = cx.add_empty_window();
        let app = cx.new(|cx| PaneFlowApp::start(Startup::Conformance, cx));
        cx.update(|_, cx| {
            crate::app::window::observe_closed_windows(
                app.downgrade(),
                PaneFlowApp::detached_window_closed,
                cx,
            );
        });
        cx.run_until_parked();
        let mut rig = Self {
            cx,
            app,
            host,
            names: Vec::new(),
            log: Vec::new(),
        };
        rig.sync();
        rig.log.clear();
        rig
    }

    pub(super) fn mount_restored(
        cx: &'a mut TestAppContext,
        state: &SessionState,
        names: Vec<(SessionId, String)>,
    ) -> Self {
        Self::mount_restored_on(cx, state, names, FakeHost::default())
    }

    pub(super) fn mount_restored_on(
        cx: &'a mut TestAppContext,
        state: &SessionState,
        names: Vec<(SessionId, String)>,
        host: FakeHost,
    ) -> Self {
        forget_the_previous_pin();
        let path = paneflow_config::loader::session_path().expect("a session path");
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("the session directory");
        }
        std::fs::write(&path, serde_json::to_vec(state).expect("a session file"))
            .expect("the session file is written");
        let mut rig = Self::mount_with(cx, host);
        std::fs::remove_file(&path).expect("the session file is removed");
        rig.names = names;
        rig.sync();
        rig.log.clear();
        rig
    }

    pub(super) fn session_of(&self, name: &str) -> SessionId {
        self.session_named(name)
    }

    pub(super) fn names(&self) -> Vec<(SessionId, String)> {
        self.names.clone()
    }

    pub(super) fn model_surface_ids(&mut self) -> Vec<u64> {
        self.cx.update(|_, cx| {
            let state = self.app.read(cx).layout.model().state();
            let mut ids: Vec<u64> = state
                .workspaces
                .iter()
                .flat_map(|ws| ws.tabs.iter())
                .filter_map(|tab| tab.root.as_ref())
                .flat_map(paneflow_layout::Node::panes)
                .flat_map(|pane| pane.surfaces.iter().map(|surface| surface.id.0))
                .collect();
            ids.sort_unstable();
            ids
        })
    }

    pub(super) fn listed_surface_ids(&mut self) -> Vec<u64> {
        let reply = self.ipc("surface.list", serde_json::json!({}));
        let mut ids: Vec<u64> = reply["result"]["surfaces"]
            .as_array()
            .unwrap_or_else(|| panic!("surface.list lists surfaces: {reply}"))
            .iter()
            .filter_map(|surface| surface["surface_id"].as_u64())
            .collect();
        ids.sort_unstable();
        ids
    }

    pub(crate) fn workspace(&mut self, title: &str) -> usize {
        let idx = self.act(|app, _, cx| {
            let ws_id = next_workspace_id_in(None);
            let terminal = cx.new(|cx| TerminalView::new(ws_id, cx));
            let pane = app.create_pane(terminal, ws_id, cx);
            let idx = app.add_workspace_in(Workspace::with_id(ws_id, title, pane), None, cx);
            app.active_idx = idx;
            idx
        });
        self.sync();
        idx
    }

    pub(super) fn act<R>(
        &mut self,
        f: impl FnOnce(&mut PaneFlowApp, &mut Window, &mut Context<PaneFlowApp>) -> R,
    ) -> R {
        let app = self.app.clone();
        let result = self
            .cx
            .update(|window, cx| app.update(cx, |app, cx| f(app, window, cx)));
        self.cx.run_until_parked();
        self.sync();
        result
    }

    pub(super) fn emit(&mut self, pane: &str, event: PaneEvent) {
        let pane = self.pane(pane);
        self.cx
            .update(|_, cx| pane.update(cx, |_, cx| cx.emit(event)));
        self.cx.run_until_parked();
        self.sync();
    }

    pub(super) fn ipc(&mut self, method: &str, params: Value) -> Value {
        let reply = self.act(|app, _, cx| {
            app.handle_ipc(method, &params, None, paneflow_api::Gates::CLOSED, cx)
        });
        let slot: Rc<RefCell<Option<Value>>> = Rc::default();
        match reply {
            IpcReply::Ready(value) => *slot.borrow_mut() = Some(value),
            IpcReply::Deferred(job) => *slot.borrow_mut() = Some(job()),
            IpcReply::Async(task) => {
                let sink = slot.clone();
                self.cx
                    .update(|_, cx| cx.spawn(async move |_| *sink.borrow_mut() = Some(task.await)))
                    .detach();
            }
        }
        let deadline = Instant::now() + IPC_WAIT;
        while slot.borrow().is_none() {
            assert!(Instant::now() < deadline, "{method} never answered");
            self.cx.run_until_parked();
            std::thread::sleep(PUMP_INTERVAL);
        }
        self.cx.run_until_parked();
        self.sync();
        let answer = slot.borrow_mut().take().unwrap_or(Value::Null);
        promote_response(answer, Value::from(1))
    }

    fn settle_pending_focus(&mut self) {
        let app = self.app.clone();
        self.cx.update(|window, cx| {
            let pending = app.update(cx, |app, _| app.pending_pane_focus.take());
            if let Some(pane) = pending
                && !PaneFlowApp::focus_pane_window(pane.clone(), cx)
            {
                pane.read(cx).focus_handle(cx).focus(window, cx);
            }
        });
        self.cx.run_until_parked();
    }

    fn sync(&mut self) {
        self.settle_pending_focus();
        for request in self.host.take() {
            if matches!(
                request,
                HostRequest::Create { .. }
                    | HostRequest::Launch { .. }
                    | HostRequest::Reattach { .. }
            ) && self.name_of(request.session()).is_none()
            {
                let name = ((b'a' + self.names.len() as u8 % 26) as char).to_string();
                self.names.push((request.session().clone(), name));
            }
            self.log.push(request);
        }
    }

    fn name_of(&self, session: &SessionId) -> Option<&str> {
        self.names
            .iter()
            .find(|(known, _)| known == session)
            .map(|(_, name)| name.as_str())
    }

    fn named(&self, session: &SessionId) -> String {
        self.name_of(session)
            .map_or_else(|| format!("?{session}"), str::to_string)
    }

    fn session_named(&self, name: &str) -> SessionId {
        self.names
            .iter()
            .find(|(_, known)| known == name)
            .map(|(session, _)| session.clone())
            .unwrap_or_else(|| panic!("no surface is named {name}"))
    }

    pub(super) fn terminal(&mut self, name: &str) -> Entity<TerminalView> {
        let session = self.session_named(name);
        self.cx.update(|_, cx| {
            let app = self.app.read(cx);
            crate::workspace::panes_across(&app.workspaces)
                .iter()
                .flat_map(|pane| pane.read(cx).terminals().cloned().collect::<Vec<_>>())
                .find(|terminal| terminal.read(cx).terminal.session_id == session)
                .unwrap_or_else(|| panic!("surface {name} is not in the layout"))
        })
    }

    pub(super) fn pane(&mut self, name: &str) -> Entity<Pane> {
        let terminal = self.terminal(name);
        self.cx.update(|_, cx| {
            let app = self.app.read(cx);
            crate::workspace::panes_across(&app.workspaces)
                .into_iter()
                .find(|pane| pane.read(cx).contains_terminal(&terminal))
                .unwrap_or_else(|| panic!("surface {name} has no pane"))
        })
    }

    pub(super) fn surface_id(&mut self, name: &str) -> u64 {
        let terminal = self.terminal(name);
        self.cx.read(|cx| terminal.read(cx).surface_id())
    }

    pub(super) fn drag_surface_id(&mut self, name: &str) -> u64 {
        self.terminal(name).entity_id().as_u64()
    }

    pub(super) fn pane_id(&mut self, name: &str) -> u64 {
        self.pane(name).entity_id().as_u64()
    }

    pub(crate) fn focus(&mut self, name: &str) {
        let pane = self.pane(name);
        self.cx.update(|window, cx| {
            let handle = pane.read(cx).focus_handle(cx);
            handle.focus(window, cx);
        });
        self.cx.run_until_parked();
    }

    pub(super) fn focused(&mut self) -> Option<String> {
        let focused = self.cx.update(|window, cx| {
            let app = self.app.read(cx);
            crate::workspace::panes_across(&app.workspaces)
                .into_iter()
                .find(|pane| pane.read(cx).focus_handle(cx).is_focused(window))
                .and_then(|pane| pane.read(cx).active_terminal_opt().cloned())
                .map(|terminal| terminal.read(cx).terminal.session_id.clone())
        });
        focused.map(|session| self.named(&session))
    }

    pub(super) fn state(&mut self) -> SessionState {
        self.cx
            .update(|_, cx| self.app.read(cx).build_session_state(cx))
    }

    pub(super) fn shape(&mut self, ws: usize, tab: usize) -> String {
        let state = self.state();
        let Some(tab) = state.workspaces.get(ws).and_then(|ws| ws.tabs.get(tab)) else {
            return "no tab".to_string();
        };
        match &tab.layout {
            Some(layout) => self.render(layout),
            None => "empty".to_string(),
        }
    }

    pub(super) fn tabs(&mut self, ws: usize) -> Vec<String> {
        let state = self.state();
        let Some(ws) = state.workspaces.get(ws) else {
            return Vec::new();
        };
        ws.tabs
            .iter()
            .enumerate()
            .map(|(idx, tab)| {
                let marker = if idx == ws.active_tab { "*" } else { "" };
                let layout = tab
                    .layout
                    .as_ref()
                    .map_or_else(|| "empty".to_string(), |layout| self.render(layout));
                format!("{marker}{}={layout}", tab.title)
            })
            .collect()
    }

    pub(super) fn workspaces(&mut self) -> Vec<String> {
        let state = self.state();
        state
            .workspaces
            .iter()
            .enumerate()
            .map(|(idx, ws)| {
                let marker = if idx == state.active_workspace {
                    "*"
                } else {
                    ""
                };
                format!("{marker}{}", ws.title)
            })
            .collect()
    }

    fn render(&self, node: &LayoutNode) -> String {
        match node {
            LayoutNode::Pane { surfaces } => {
                let names: Vec<String> = surfaces
                    .iter()
                    .map(|surface| {
                        let name = surface.session.as_ref().map_or_else(
                            || {
                                surface
                                    .path
                                    .clone()
                                    .unwrap_or_else(|| "unbound".to_string())
                            },
                            |session| self.named(session),
                        );
                        if surfaces.len() > 1 && surface.focus == Some(true) {
                            format!("{name}*")
                        } else {
                            name
                        }
                    })
                    .collect();
                names.join("+")
            }
            LayoutNode::Split {
                direction,
                children,
                ..
            } => {
                let letter = if direction == "vertical" { "V" } else { "H" };
                let ratios = node.resolved_ratios();
                let parts: Vec<String> = children
                    .iter()
                    .zip(ratios)
                    .map(|(child, ratio)| format!("{ratio:.3} {}", self.render(child)))
                    .collect();
                format!("{letter}({})", parts.join(", "))
            }
        }
    }

    pub(super) fn revision(&mut self) -> u64 {
        self.cx
            .update(|_, cx| self.app.read(cx).layout.model().revision())
    }

    pub(super) fn model_shape(&mut self) -> String {
        let node = self.cx.update(|_, cx| {
            let state = self.app.read(cx).layout.model().state();
            state
                .workspaces
                .get(state.active_workspace)
                .and_then(|ws| ws.tabs.get(ws.active_tab))
                .and_then(|tab| tab.root.as_ref())
                .map(paneflow_layout::export_node)
        });
        node.map_or_else(|| "empty".to_string(), |node| self.render(&node))
    }

    pub(super) fn drag_divider(&mut self, path: &[usize], before: f32, after: f32) -> usize {
        let path = path.to_vec();
        self.cx.update(|_, cx| {
            let app = self.app.read(cx);
            let mut node = app
                .active_workspace()
                .and_then(|ws| ws.active_tab().root.as_ref())
                .expect("the active tab has a layout");
            for idx in path {
                let crate::layout::LayoutTree::Container { children, .. } = node else {
                    panic!("the divider path leaves the splits");
                };
                node = &children[idx].node;
            }
            let crate::layout::LayoutTree::Container { children, drag, .. } = node else {
                panic!("the divider path does not end on a split");
            };
            drag.set(Some(crate::layout::DragState {
                divider_idx: 0,
                start_pos: 0.0,
                start_ratio_before: children[0].ratio.get(),
                start_ratio_after: children[1].ratio.get(),
            }));
            children[0].ratio.set(before);
            children[1].ratio.set(after);
            Rc::as_ptr(drag) as usize
        })
    }

    pub(super) fn release_divider(&mut self, split: usize) {
        self.act(|app, _, cx| {
            let dragged = app.workspaces.iter().find_map(|ws| {
                ws.tabs()
                    .iter()
                    .filter_map(|tab| tab.root.as_ref())
                    .find_map(|root| root.drag_cell(split))
            });
            if let Some(drag) = dragged {
                drag.set(None);
            }
            app.divider_drag_ended(split, cx);
        });
    }

    pub(super) fn requests(&mut self) -> Vec<String> {
        self.sync();
        let log = std::mem::take(&mut self.log);
        log.iter()
            .map(|request| {
                let name = self.named(request.session());
                match request {
                    HostRequest::Create { .. } => format!("create {name}"),
                    HostRequest::Launch { .. } => format!("launch {name}"),
                    HostRequest::Reattach { .. } => format!("reattach {name}"),
                    HostRequest::Hold { .. } => format!("hold {name}"),
                    HostRequest::Stop { .. } => format!("stop {name}"),
                    HostRequest::Assign { .. } => format!("assign {name}"),
                }
            })
            .collect()
    }

    pub(super) fn toasts(&mut self) -> Vec<String> {
        self.cx.update(|_, cx| {
            let app = self.app.read(cx);
            app.toast
                .iter()
                .chain(app.toast_queue.iter())
                .map(|toast| toast.message.clone())
                .collect()
        })
    }

    pub(super) fn expire_holds(&mut self) {
        self.act(|app, _, cx| {
            app.expire_closed_sessions_at(
                Instant::now() + Duration::from_millis(crate::CLOSED_SESSION_GRACE_MS + 1),
                cx,
            )
        });
    }
}
