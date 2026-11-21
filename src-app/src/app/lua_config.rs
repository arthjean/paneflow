use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use gpui::{Context, Window};
use paneflow_api::methods::config::{
    ACTION_INVOKE, ACTION_REPLY_DEADLINE, ActionList, ActionSummary, CONFIG_RELOADED,
    ConfigProblem, ConfigStatus, InvokeContext, Keymap, Severity,
};
use paneflow_ipc_client::host_control::HostControl;
use paneflow_serve::controller::{EventFrame, EventRequest, EventStream};
use serde_json::{Map, Value, json};

use crate::PaneFlowApp;
use crate::app::notifications::ToastAction;
use crate::keybindings::{LuaKeymap, LuaKeymapWarnings, RunLuaAction};

const CLIENT_NAME: &str = "paneflow-window-config";
const FRAME_SLOTS: usize = 8;
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const STREAM_SLICE: Duration = Duration::from_secs(60);
const DRAIN_MAX_PER_TICK: usize = 8;
pub(crate) const SCRIPTING_UNAVAILABLE: &str =
    "Scripting is unavailable: the Paneflow worker does not answer";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConfigSnapshot {
    pub(crate) actions: Vec<ActionSummary>,
    pub(crate) keymap: Keymap,
    pub(crate) status: ConfigStatus,
    pub(crate) reloaded: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConfigFrame {
    Loaded(Box<ConfigSnapshot>),
    Disconnected,
}

#[derive(Default)]
pub(crate) struct LuaConfigView {
    frames: Option<Receiver<ConfigFrame>>,
    pub(crate) actions: Vec<ActionSummary>,
    keymap: Keymap,
    status: Option<ConfigStatus>,
    pub(crate) reachable: bool,
    outage_noticed: bool,
    last_notice: Option<String>,
    pub(crate) grants: GestureGrants,
}

pub(crate) const GESTURE_ORIGINS: [&str; 2] = ["key", "palette"];

#[derive(Default)]
pub(crate) struct GestureGrants {
    live: HashMap<String, Instant>,
}

impl GestureGrants {
    pub(crate) fn mint(&mut self, now: Instant) -> String {
        self.live.retain(|_, until| *until > now);
        let grant = uuid::Uuid::new_v4().simple().to_string();
        self.live.insert(grant.clone(), now + ACTION_REPLY_DEADLINE);
        grant
    }

    pub(crate) fn revoke(&mut self, grant: &str) {
        self.live.remove(grant);
    }

    pub(crate) fn admits(&self, grant: &str, now: Instant) -> bool {
        self.live.get(grant).is_some_and(|until| *until > now)
    }

    #[cfg(test)]
    pub(crate) fn live_count(&self) -> usize {
        self.live.len()
    }
}

impl LuaConfigView {
    pub(crate) fn categories(&self) -> Vec<String> {
        let mut categories: Vec<String> = Vec::new();
        for action in &self.actions {
            if !categories.contains(&action.category) {
                categories.push(action.category.clone());
            }
        }
        categories.sort();
        categories
    }

    pub(crate) fn title_of(&self, name: &str) -> String {
        self.actions
            .iter()
            .find(|action| action.name == name)
            .map_or_else(|| name.to_string(), |action| action.title.clone())
    }
}

fn fetch(endpoint: &Path, reloaded: bool) -> Result<ConfigSnapshot, String> {
    let mut control = HostControl::connect(endpoint, CLIENT_NAME)?;
    let decode = |value: Value, what: &str| -> Result<Value, String> {
        if value.is_null() {
            Err(format!("{what} answered nothing"))
        } else {
            Ok(value)
        }
    };
    let actions: ActionList = serde_json::from_value(decode(
        control.request("action.list", json!({}))?,
        "action.list",
    )?)
    .map_err(|error| error.to_string())?;
    let keymap: Keymap = serde_json::from_value(decode(
        control.request("keymap.list", json!({}))?,
        "keymap.list",
    )?)
    .map_err(|error| error.to_string())?;
    let status: ConfigStatus = serde_json::from_value(decode(
        control.request("config.status", json!({}))?,
        "config.status",
    )?)
    .map_err(|error| error.to_string())?;
    Ok(ConfigSnapshot {
        actions: actions.actions,
        keymap,
        status,
        reloaded,
    })
}

struct Sink {
    tx: SyncSender<ConfigFrame>,
    wake: crate::app::wake::AppWake,
}

impl Sink {
    fn send(&self, frame: ConfigFrame) -> bool {
        let sent = match self.tx.try_send(frame) {
            Ok(()) | Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Disconnected(_)) => false,
        };
        self.wake.notify();
        sent
    }
}

fn follow(endpoint: &Path, sink: &Sink) {
    let request = EventRequest {
        types: vec![CONFIG_RELOADED.to_string()],
        workspace_id: None,
        since: None,
        epoch: None,
    };
    let mut connected = false;
    loop {
        let Ok(mut stream) = EventStream::open(endpoint, request.clone()) else {
            if connected && !sink.send(ConfigFrame::Disconnected) {
                return;
            }
            connected = false;
            std::thread::sleep(RECONNECT_DELAY);
            continue;
        };
        if let Ok(snapshot) = fetch(endpoint, false) {
            connected = true;
            if !sink.send(ConfigFrame::Loaded(Box::new(snapshot))) {
                return;
            }
        }
        loop {
            let reloaded = match stream.next(STREAM_SLICE) {
                EventFrame::Following { .. } => false,
                EventFrame::Event(_) => true,
                EventFrame::Idle => continue,
                EventFrame::Disconnected(_) => {
                    if connected && !sink.send(ConfigFrame::Disconnected) {
                        return;
                    }
                    connected = false;
                    continue;
                }
            };
            match fetch(endpoint, reloaded) {
                Ok(snapshot) => {
                    connected = true;
                    if !sink.send(ConfigFrame::Loaded(Box::new(snapshot))) {
                        return;
                    }
                }
                Err(error) => log::debug!("paneflow: cannot read the worker's init.lua: {error}"),
            }
        }
    }
}

pub(crate) fn spawn_config_thread(
    wake: crate::app::wake::AppWake,
) -> Option<Receiver<ConfigFrame>> {
    let endpoint = paneflow_home::serve_endpoint_path_for_current_home()?;
    let (tx, rx) = sync_channel(FRAME_SLOTS);
    let sink = Sink { tx, wake };
    let spawned = std::thread::Builder::new()
        .name("paneflow-worker-config".into())
        .spawn(move || follow(&endpoint, &sink));
    match spawned {
        Ok(_) => Some(rx),
        Err(error) => {
            log::warn!("paneflow: cannot start the init.lua follower: {error}");
            None
        }
    }
}

pub(crate) fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message).trim()
}

pub(crate) fn wire_message(error: &str) -> &str {
    error
        .strip_prefix("paneflow error ")
        .and_then(|rest| rest.split_once(": "))
        .map_or(error, |(_, message)| message)
}

pub(crate) fn problem_notice(problem: &ConfigProblem) -> String {
    let file = problem
        .file
        .as_deref()
        .unwrap_or(paneflow_home::INIT_LUA_FILE_NAME);
    match problem.line {
        Some(line) => format!("{file}:{line}: {}", first_line(&problem.message)),
        None => format!("{file}: {}", first_line(&problem.message)),
    }
}

pub(crate) fn warnings_notice(count: usize) -> String {
    if count == 1 {
        "init.lua: 1 warning".to_string()
    } else {
        format!("init.lua: {count} warnings")
    }
}

pub(crate) fn notice_for(
    status: &ConfigStatus,
    reloaded: bool,
    window_warnings: usize,
) -> Option<(String, Option<&ConfigProblem>)> {
    let error = status
        .problems
        .iter()
        .find(|problem| problem.severity == Severity::Error);
    if !status.ok || error.is_some() {
        let problem = error.or(status.problems.first());
        return Some(match problem {
            Some(problem) => (problem_notice(problem), Some(problem)),
            None => (
                "init.lua did not load; the previous configuration stays active".to_string(),
                None,
            ),
        });
    }
    if !reloaded {
        return None;
    }
    let count = status.problems.len() + window_warnings;
    (count > 0).then(|| (warnings_notice(count), status.problems.first()))
}

fn config_file(home: &Path, problem: Option<&ConfigProblem>) -> PathBuf {
    let relative = problem
        .and_then(|problem| problem.file.as_deref())
        .unwrap_or(paneflow_home::INIT_LUA_FILE_NAME);
    home.join(relative)
}

#[derive(Debug)]
enum InvokeFailure {
    Unreachable,
    Failed(String),
}

fn invoke_blocking(
    endpoint: Option<PathBuf>,
    name: String,
    args: Option<Map<String, Value>>,
    context: InvokeContext,
) -> Result<(), InvokeFailure> {
    let endpoint = endpoint.ok_or(InvokeFailure::Unreachable)?;
    let mut control =
        HostControl::connect(&endpoint, CLIENT_NAME).map_err(|_| InvokeFailure::Unreachable)?;
    let params = json!({
        "name": name,
        "args": args.unwrap_or_default(),
        "ctx": serde_json::to_value(context).unwrap_or_else(|_| json!({})),
    });
    control
        .request_with_deadline(ACTION_INVOKE, params, ACTION_REPLY_DEADLINE)
        .map(|_| ())
        .map_err(|error| InvokeFailure::Failed(wire_message(&error).to_string()))
}

impl PaneFlowApp {
    pub(crate) fn start_lua_config_stream(&mut self) {
        if self.lua_config.frames.is_some() {
            return;
        }
        self.lua_config.frames = spawn_config_thread(self.app_wake.clone());
    }

    pub(crate) fn process_lua_config_frames(&mut self, cx: &mut Context<Self>) {
        let mut pending = Vec::new();
        if let Some(frames) = self.lua_config.frames.as_ref() {
            while pending.len() < DRAIN_MAX_PER_TICK {
                let Ok(frame) = frames.try_recv() else {
                    break;
                };
                pending.push(frame);
            }
        }
        if pending.len() == DRAIN_MAX_PER_TICK {
            self.app_wake.notify();
        }
        for frame in pending {
            self.apply_lua_config_frame(frame, cx);
        }
    }

    pub(crate) fn apply_lua_config_frame(&mut self, frame: ConfigFrame, cx: &mut Context<Self>) {
        match frame {
            ConfigFrame::Disconnected => {
                self.lua_config.reachable = false;
                self.lua_config.actions.clear();
                cx.notify();
            }
            ConfigFrame::Loaded(snapshot) => {
                let ConfigSnapshot {
                    actions,
                    keymap,
                    status,
                    reloaded,
                } = *snapshot;
                self.lua_config.reachable = true;
                self.lua_config.outage_noticed = false;
                self.lua_config.actions = actions;
                if self.lua_config.keymap != keymap {
                    self.lua_config.keymap = keymap.clone();
                    cx.set_global(LuaKeymap {
                        bindings: keymap.bindings,
                        unbound: keymap.unbound,
                    });
                }
                crate::keybindings::apply_keybindings(cx, &self.cached_config.shortcuts);
                let window_warnings = cx
                    .try_global::<LuaKeymapWarnings>()
                    .map_or(0, |warnings| warnings.0.len());
                self.notify_config_status(&status, reloaded, window_warnings, cx);
                self.lua_config.status = Some(status);
                cx.notify();
            }
        }
    }

    fn notify_config_status(
        &mut self,
        status: &ConfigStatus,
        reloaded: bool,
        window_warnings: usize,
        cx: &mut Context<Self>,
    ) {
        let Some((message, problem)) = notice_for(status, reloaded, window_warnings) else {
            if status.ok && reloaded {
                self.lua_config.last_notice = None;
            }
            return;
        };
        if self.lua_config.last_notice.as_deref() == Some(message.as_str()) {
            return;
        }
        self.lua_config.last_notice = Some(message.clone());
        let failed = !status.ok
            || status
                .problems
                .iter()
                .any(|problem| problem.severity == Severity::Error);
        let actions = paneflow_home::paneflow_home()
            .map(|home| {
                vec![ToastAction::OpenConfig {
                    path: config_file(&home, problem),
                    root: home,
                    line: problem.and_then(|problem| problem.line),
                    failed,
                }]
            })
            .unwrap_or_default();
        self.push_toast(
            message,
            actions,
            crate::app::constants::TOAST_HOLD_MS * 3,
            cx,
        );
    }

    pub(crate) fn notify_scripting_unavailable(&mut self, cx: &mut Context<Self>) {
        if self.lua_config.outage_noticed {
            return;
        }
        self.lua_config.outage_noticed = true;
        self.show_toast(SCRIPTING_UNAVAILABLE, cx);
    }

    pub(crate) fn focused_invoke_context(
        &self,
        origin: &str,
        window: &Window,
        cx: &gpui::App,
    ) -> InvokeContext {
        let mut context = InvokeContext {
            origin: Some(origin.to_string()),
            client_id: Some("window".to_string()),
            ..InvokeContext::default()
        };
        let Some(workspace) = self.active_workspace() else {
            return context;
        };
        context.workspace_id = self.workspace_id_for_workspace_idx(self.active_idx);
        let tab = workspace.active_tab();
        context.tab_id = Some(tab.id);
        if let Some(pane) = tab
            .root
            .as_ref()
            .and_then(|root| root.focused_pane(window, cx))
        {
            let pane = pane.read(cx);
            if let Some(crate::pane::PaneSurface::Terminal(view)) =
                pane.surfaces().get(pane.active_surface_idx())
            {
                context.surface_id = Some(view.read(cx).surface_id());
            }
        }
        context
    }

    pub(crate) fn invoke_lua_action(
        &mut self,
        name: String,
        args: Option<Map<String, Value>>,
        origin: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.lua_config.reachable {
            self.notify_scripting_unavailable(cx);
            return;
        }
        let mut context = self.focused_invoke_context(origin, window, cx);
        let grant = GESTURE_ORIGINS
            .contains(&origin)
            .then(|| self.lua_config.grants.mint(Instant::now()));
        context.grant = grant.clone();
        let title = self.lua_config.title_of(&name);
        let endpoint = paneflow_home::serve_endpoint_path_for_current_home();
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let result =
                smol::unblock(move || invoke_blocking(endpoint, name, args, context)).await;
            let _ = this.update(cx, |app, cx| {
                if let Some(grant) = grant.as_deref() {
                    app.lua_config.grants.revoke(grant);
                }
                let Err(failure) = result else {
                    return;
                };
                app.report_invoke_failure(&title, failure, cx);
            });
        })
        .detach();
    }

    fn report_invoke_failure(
        &mut self,
        title: &str,
        failure: InvokeFailure,
        cx: &mut Context<Self>,
    ) {
        match failure {
            InvokeFailure::Unreachable => self.notify_scripting_unavailable(cx),
            InvokeFailure::Failed(message) => {
                self.show_toast(format!("{title}: {}", first_line(&message)), cx);
            }
        }
    }

    pub(crate) fn handle_run_lua_action(
        &mut self,
        action: &RunLuaAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invoke_lua_action(action.name.clone(), action.args.clone(), "key", window, cx);
    }
}

#[cfg(test)]
impl PaneFlowApp {
    pub(crate) fn toast_messages(&self) -> Vec<String> {
        self.toast
            .iter()
            .chain(self.toast_queue.iter())
            .map(|toast| toast.message.clone())
            .collect()
    }

    pub(crate) fn lua_config_reachable(&self) -> bool {
        self.lua_config.reachable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gesture_grant_admits_its_run_until_revoked_or_past_the_reply_deadline() {
        let now = Instant::now();
        let mut grants = GestureGrants::default();
        let grant = grants.mint(now);
        let other = grants.mint(now);
        assert_ne!(grant, other, "each run gets its own grant");
        assert_eq!(grant.len(), 32, "128 random bits in hex");
        assert!(grants.admits(&grant, now));
        assert!(!grants.admits("forged", now));
        assert!(!grants.admits("", now));
        assert!(!grants.admits(&grant, now + ACTION_REPLY_DEADLINE));
        grants.revoke(&grant);
        assert!(!grants.admits(&grant, now));
        assert!(grants.admits(&other, now));
        grants.mint(now + ACTION_REPLY_DEADLINE);
        assert!(
            !grants.admits(&other, now),
            "a grant past its deadline is dropped at the next mint"
        );
        assert_eq!(GESTURE_ORIGINS, ["key", "palette"]);
    }

    fn problem(severity: Severity, line: Option<u32>, message: &str) -> ConfigProblem {
        ConfigProblem {
            file: Some("init.lua".into()),
            line,
            severity,
            message: message.into(),
        }
    }

    fn status(ok: bool, problems: Vec<ConfigProblem>) -> ConfigStatus {
        ConfigStatus {
            path: "/home/me/.paneflow/init.lua".into(),
            found: true,
            ok,
            problems,
            ..ConfigStatus::default()
        }
    }

    #[test]
    fn a_failed_load_names_the_file_line_and_first_line_of_the_message() {
        let failed = status(
            false,
            vec![problem(
                Severity::Error,
                Some(7),
                "unexpected symbol\nnear end",
            )],
        );
        let (message, problem) = notice_for(&failed, true, 0).unwrap();
        assert_eq!(message, "init.lua:7: unexpected symbol");
        assert_eq!(problem.and_then(|problem| problem.line), Some(7));
        assert_eq!(
            notice_for(&failed, false, 0).map(|(message, _)| message),
            Some("init.lua:7: unexpected symbol".to_string()),
            "a load error present at window start is shown too"
        );
    }

    #[test]
    fn warnings_give_a_count_only_after_a_reload_and_nothing_when_there_are_none() {
        let warned = status(
            true,
            vec![problem(Severity::Warning, Some(2), "declared again")],
        );
        assert_eq!(
            notice_for(&warned, true, 1).map(|(message, _)| message),
            Some("init.lua: 2 warnings".to_string())
        );
        assert_eq!(notice_for(&warned, false, 1), None);
        assert_eq!(notice_for(&status(true, Vec::new()), true, 0), None);
    }

    #[test]
    fn a_refused_declaration_in_a_file_that_loaded_is_shown_as_an_error() {
        let refused = status(
            true,
            vec![
                problem(Severity::Warning, Some(2), "declared again"),
                problem(Severity::Error, Some(5), "'Bad' is not an action name"),
            ],
        );
        for reloaded in [true, false] {
            let (message, problem) = notice_for(&refused, reloaded, 0).unwrap();
            assert_eq!(message, "init.lua:5: 'Bad' is not an action name");
            assert_eq!(problem.and_then(|problem| problem.line), Some(5));
        }
    }

    #[test]
    fn a_wire_error_prints_without_its_code() {
        assert_eq!(
            wire_message("paneflow error -32050: init.lua:3: it broke"),
            "init.lua:3: it broke"
        );
        assert_eq!(wire_message("plain"), "plain");
    }
}
