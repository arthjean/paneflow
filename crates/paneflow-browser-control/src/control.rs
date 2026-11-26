use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use paneflow_api::methods::browser::{
    ActParams, Acted, Action, BrowserStatus, BrowserTab, Closed, ConsoleHistory, EvaluateParams,
    Evaluated, HistoryParams, InstallResult, NavigateParams, Navigated, NetworkHistory, OpenParams,
    Presentation, Screenshot, Snapshot, TabList, TabOwner, TabParams, TabState, WaitParams, Waited,
};
use paneflow_browser_client::ClientEvent;
use paneflow_browser_proto::limits::{
    EVALUATE_TIMEOUT, MAX_URL_CHARS, NETWORK_IDLE_QUIET, SNAPSHOT_TEXT_BYTES,
};
use paneflow_browser_proto::navigation::{self, Frame, Verdict};
use paneflow_browser_proto::{CommandError, Event, OriginDecision, ProfileName, Reply, TabId};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::approvals::{Approvals, EvaluateRequest, SiteChoice, SiteRequest};
use crate::engine::{Engine, TabCommand};
use crate::error::ControlError;
use crate::keys::key_event;
use crate::snapshot;
use crate::state::{Decide, Marks, TabEntry};
use crate::surface::{
    ANNOTATION_BINDING, FileChooser, HeldSite, PageDialog, SurfaceEvent, TabView,
};

pub const SNAPSHOT_ENTRIES: usize = 200;
pub const MAX_HISTORY_ENTRIES: usize = 1000;
pub const DEFAULT_HISTORY_ENTRIES: usize = 200;
pub const MAX_SCRIPT_BYTES: usize = 64 * 1024;
pub const DEFAULT_WAIT: Duration = Duration::from_secs(10);
pub const MAX_WAIT: Duration = Duration::from_secs(60);
pub const DEFAULT_SCROLL: i64 = 400;

const ORPHAN_EVENTS: usize = 512;
const ORPHAN_TABS: usize = 32;
const POLL: Duration = Duration::from_millis(100);
const DOMAINS: [&str; 4] = [
    "Page.enable",
    "Runtime.enable",
    "Log.enable",
    "Network.enable",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub command: Duration,
    pub navigation: Duration,
    pub approval: Duration,
    pub evaluate: Duration,
    pub settle: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            command: Duration::from_secs(90),
            navigation: Duration::from_secs(30),
            approval: Duration::from_secs(
                paneflow_api::methods::prompt::AGENT_PROMPT_DEADLINE_SECS + 10,
            ),
            evaluate: EVALUATE_TIMEOUT,
            settle: Duration::from_millis(400),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Identity {
    Agent {
        session: String,
        agent: Option<String>,
    },
    Human,
}

impl Identity {
    fn owner(&self) -> TabOwner {
        match self {
            Self::Agent { session, agent } => TabOwner::Agent {
                session: session.clone(),
                agent: agent.clone(),
            },
            Self::Human => TabOwner::Human,
        }
    }

    fn owns(&self, owner: &TabOwner) -> bool {
        match (self, owner) {
            (Self::Agent { session, .. }, TabOwner::Agent { session: held, .. }) => session == held,
            (Self::Human, TabOwner::Human) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallContext {
    pub identity: Identity,
    pub workspace: Option<ProfileName>,
    pub visible: Option<ProfileName>,
    pub live_sessions: Option<HashSet<String>>,
}

impl CallContext {
    fn sees(&self, entry: &TabEntry) -> bool {
        self.visible
            .as_ref()
            .is_none_or(|workspace| *workspace == entry.workspace)
    }
}

#[derive(Default)]
struct State {
    tabs: BTreeMap<TabId, TabEntry>,
    orphans: HashMap<TabId, Vec<Event>>,
    inspectors: HashSet<TabId>,
    allowed: HashMap<ProfileName, HashSet<String>>,
    evaluators: HashSet<(ProfileName, String, Option<String>)>,
}

struct Inner {
    engine: Arc<dyn Engine>,
    approvals: Arc<dyn Approvals>,
    limits: Limits,
    state: Mutex<State>,
    changed: Condvar,
    subscribers: Mutex<Vec<Sender<SurfaceEvent>>>,
}

#[derive(Clone)]
pub struct Control {
    inner: Arc<Inner>,
}

impl Control {
    pub fn new(
        engine: Arc<dyn Engine>,
        events: Receiver<ClientEvent>,
        approvals: Arc<dyn Approvals>,
        limits: Limits,
    ) -> Self {
        let inner = Arc::new(Inner {
            engine,
            approvals,
            limits,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            subscribers: Mutex::new(Vec::new()),
        });
        let weak = Arc::downgrade(&inner);
        let pump = std::thread::Builder::new()
            .name("browser-control-events".into())
            .spawn(move || pump(&weak, &events));
        if let Err(error) = pump {
            log::warn!("browser control: cannot start the event thread: {error}");
        }
        Self { inner }
    }

    pub fn call(
        &self,
        method: &str,
        params: Value,
        context: &CallContext,
    ) -> Result<Value, ControlError> {
        let inner = &self.inner;
        match method {
            "browser.status" => encode(inner.status(context)),
            "browser.install" => encode(inner.install()),
            "browser.open" => encode(inner.open(decode::<OpenParams>(params)?, context)?),
            "browser.list" => encode(TabList {
                tabs: inner.visible_tabs(context),
            }),
            "browser.claim" => encode(inner.claim(decode::<TabParams>(params)?, context)?),
            "browser.close" => encode(inner.close(decode::<TabParams>(params)?, context)?),
            "browser.navigate" => {
                encode(inner.navigate(decode::<NavigateParams>(params)?, context)?)
            }
            "browser.reload" => encode(inner.reload(decode::<TabParams>(params)?, context)?),
            "browser.back" => {
                encode(inner.history_step(decode::<TabParams>(params)?, -1, context)?)
            }
            "browser.forward" => {
                encode(inner.history_step(decode::<TabParams>(params)?, 1, context)?)
            }
            "browser.snapshot" => encode(inner.snapshot(decode::<TabParams>(params)?, context)?),
            "browser.act" => encode(inner.act(decode::<ActParams>(params)?, context)?),
            "browser.wait_for" => encode(inner.wait_for(decode::<WaitParams>(params)?, context)?),
            "browser.console" => encode(inner.console(decode::<HistoryParams>(params)?, context)?),
            "browser.network" => encode(inner.network(decode::<HistoryParams>(params)?, context)?),
            "browser.screenshot" => {
                encode(inner.screenshot(decode::<TabParams>(params)?, context)?)
            }
            "browser.evaluate" => {
                encode(inner.evaluate(decode::<EvaluateParams>(params)?, context)?)
            }
            other => Err(ControlError::invalid(format!(
                "{other} is not a browser method"
            ))),
        }
    }

    pub fn subscribe(&self) -> Receiver<SurfaceEvent> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.inner
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(sender);
        receiver
    }

    pub fn view(&self, tab: TabId) -> Option<TabView> {
        let state = self.inner.lock();
        state.tabs.get(&tab).map(|entry| Inner::view_of(tab, entry))
    }

    pub fn views(&self, workspace: &ProfileName) -> Vec<TabView> {
        let state = self.inner.lock();
        state
            .tabs
            .iter()
            .filter(|(_, entry)| entry.workspace == *workspace)
            .map(|(tab, entry)| Inner::view_of(*tab, entry))
            .collect()
    }

    pub fn open_surface(
        &self,
        workspace: ProfileName,
        owner: TabOwner,
        url: &str,
    ) -> Result<TabId, ControlError> {
        validate_url(url)?;
        self.inner.require_installed()?;
        self.inner.open_owned(workspace, owner, url, true)
    }

    pub fn set_presented(&self, tab: TabId, presented: bool) {
        if let Some(entry) = self.inner.lock().tabs.get_mut(&tab) {
            entry.presented = presented;
            entry.in_window |= presented;
        }
    }

    pub fn adopt_inspector(&self, inspector: TabId) {
        let mut state = self.inner.lock();
        state.orphans.remove(&inspector);
        state.inspectors.insert(inspector);
    }

    pub fn take_over(&self, tab: TabId) -> bool {
        let changed = {
            let mut state = self.inner.lock();
            let Some(entry) = state.tabs.get_mut(&tab) else {
                return false;
            };
            match &entry.owner {
                TabOwner::Human => false,
                TabOwner::Agent { session, .. } => {
                    entry.taken_over_from = Some(session.clone());
                    self.inner.set_owner(tab, entry, TabOwner::Human);
                    true
                }
                TabOwner::Unclaimed => {
                    self.inner.set_owner(tab, entry, TabOwner::Human);
                    true
                }
            }
        };
        if changed {
            self.inner.broadcast(SurfaceEvent::Changed(tab));
        }
        changed
    }

    pub fn hand_to(&self, tab: TabId, session: String, agent: Option<String>) -> bool {
        let handed = {
            let mut state = self.inner.lock();
            let Some(entry) = state.tabs.get_mut(&tab) else {
                return false;
            };
            self.inner
                .set_owner(tab, entry, TabOwner::Agent { session, agent });
            entry.taken_over_from = None;
            true
        };
        if handed {
            self.inner.broadcast(SurfaceEvent::Changed(tab));
        }
        handed
    }
}

fn decode<T: DeserializeOwned>(params: Value) -> Result<T, ControlError> {
    serde_json::from_value(params)
        .map_err(|error| ControlError::invalid(format!("invalid params: {error}")))
}

fn encode<T: Serialize>(value: T) -> Result<Value, ControlError> {
    serde_json::to_value(value).map_err(|error| ControlError::failed(error.to_string()))
}

fn pump(inner: &Weak<Inner>, events: &Receiver<ClientEvent>) {
    for event in events {
        let Some(inner) = inner.upgrade() else {
            return;
        };
        inner.on_event(event);
    }
}

pub fn validate_url(raw: &str) -> Result<(), ControlError> {
    let length = raw.chars().count();
    if length > MAX_URL_CHARS {
        return Err(ControlError::invalid(format!(
            "the URL has {length} characters, over the limit of {MAX_URL_CHARS}"
        )));
    }
    let parsed = url::Url::parse(raw)
        .map_err(|error| ControlError::invalid(format!("the URL is not valid: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ControlError::invalid(format!(
            "only http and https URLs can be opened, not {}:",
            parsed.scheme()
        )));
    }
    match navigation::classify(raw, Frame::TopLevel) {
        Verdict::Refuse(refusal) => Err(ControlError::invalid(refusal.to_string())),
        Verdict::Allow | Verdict::External { .. } => Ok(()),
    }
}

fn owner_text(tab: TabId, owner: &TabOwner) -> String {
    match owner {
        TabOwner::Unclaimed => format!(
            "tab {} is unclaimed; call browser.claim before driving it",
            tab.0
        ),
        owner => format!(
            "tab {} is owned by {}; only its owner may drive it",
            tab.0,
            owner.describe()
        ),
    }
}

fn devtools_value(reply: Result<Reply, CommandError>) -> Result<Value, ControlError> {
    match reply {
        Ok(Reply::DevTools(value)) => Ok(value),
        Ok(Reply::Done) => Ok(Value::Null),
        Ok(Reply::Security(security)) => Ok(serde_json::to_value(security).unwrap_or(Value::Null)),
        Err(error) => Err(ControlError::from_command(error)),
    }
}

fn png_size(base64: &str) -> Option<(u32, u32)> {
    let head = decode_base64(base64.get(..32)?)?;
    let width = u32::from_be_bytes(head.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(head.get(20..24)?.try_into().ok()?);
    Some((width, height))
}

pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let value = |byte: u8| -> Option<u32> {
        Some(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let bytes = text.trim_end().as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let chunks = bytes.len() / 4;
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let padding = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && index + 1 != chunks) {
            return None;
        }
        let mut word = 0u32;
        for byte in &chunk[..4 - padding] {
            word = (word << 6) | value(*byte)?;
        }
        word <<= 6 * padding as u32;
        out.extend_from_slice(&word.to_be_bytes()[1..4 - padding]);
    }
    Some(out)
}

fn bounded(text: String, bound: usize) -> (String, u64) {
    if text.len() <= bound {
        return (text, 0);
    }
    let mut cut = bound;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let dropped = (text.len() - cut) as u64;
    (text[..cut].to_string(), dropped)
}

fn guarded(owner: &TabOwner) -> bool {
    !matches!(owner, TabOwner::Human)
}

impl Inner {
    fn set_owner(&self, tab: TabId, entry: &mut TabEntry, owner: TabOwner) {
        let guard = guarded(&owner);
        let changed = guard != guarded(&entry.owner);
        entry.owner = owner;
        if !changed {
            return;
        }
        if !guard {
            entry.pending = None;
            if entry.state == TabState::AwaitingDecision {
                entry.state = TabState::Loading;
            }
        }
        self.engine.fire(tab, TabCommand::Guard(guard));
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait_until<T>(
        &self,
        deadline: impl Fn(&State) -> Instant,
        mut done: impl FnMut(&mut State) -> Option<T>,
    ) -> Option<T> {
        let mut state = self.lock();
        loop {
            if let Some(found) = done(&mut state) {
                return Some(found);
            }
            let now = Instant::now();
            let until = deadline(&state);
            if now >= until {
                return None;
            }
            let (next, _) = self
                .changed
                .wait_timeout(state, (until - now).min(POLL))
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
        }
    }

    fn broadcast(&self, event: SurfaceEvent) {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|subscriber| subscriber.send(event.clone()).is_ok());
    }

    fn view_of(tab: TabId, entry: &TabEntry) -> TabView {
        TabView {
            tab,
            workspace: entry.workspace.clone(),
            url: entry.url.clone(),
            title: entry.title.clone(),
            state: entry.state,
            owner: entry.owner.clone(),
            loading: entry.engine_loading || entry.loading(),
            load_progress: entry.load_progress,
            can_go_back: entry.can_go_back,
            can_go_forward: entry.can_go_forward,
            held: entry.pending.as_ref().map(|pending| HeldSite {
                origin: pending.origin.clone(),
                url: pending.url.clone(),
            }),
            refusal: entry.last_refusal.clone().filter(|_| entry.notice_current),
            failure: entry.last_failure.clone().filter(|_| entry.notice_current),
            crash: entry.crash.clone(),
            presentation_failure: entry.presentation_failure.clone(),
            taken_over_from: entry.taken_over_from.clone(),
            blocked: entry.blocked.clone(),
        }
    }

    fn open_owned(
        &self,
        workspace: ProfileName,
        owner: TabOwner,
        url: &str,
        in_window: bool,
    ) -> Result<TabId, ControlError> {
        let (tab, opened) =
            self.engine
                .open_tab(workspace.clone(), url, guarded(&owner), self.limits.command);
        {
            let mut state = self.lock();
            let mut entry = TabEntry::new(workspace, owner, url.to_string());
            entry.in_window = in_window;
            for event in state.orphans.remove(&tab).unwrap_or_default() {
                entry.apply(&event);
            }
            state.tabs.insert(tab, entry);
        }
        if let Err(error) = opened {
            self.lock().tabs.remove(&tab);
            return Err(ControlError::from_command(error));
        }
        self.broadcast(SurfaceEvent::Changed(tab));
        Ok(tab)
    }

    fn surface_event(self: &Arc<Self>, tab: TabId, event: &Event) -> bool {
        let owner = self.lock().tabs.get(&tab).map(|entry| entry.owner.clone());
        let agent_owned = matches!(owner, Some(TabOwner::Agent { .. }));
        match event {
            Event::Cursor { cursor, .. } => {
                self.broadcast(SurfaceEvent::Cursor {
                    tab,
                    cursor: *cursor,
                });
                true
            }
            Event::ImeCaret { caret, .. } => {
                self.broadcast(SurfaceEvent::ImeCaret { tab, caret: *caret });
                true
            }
            Event::DevTools { method, params, .. }
                if method == "Runtime.bindingCalled"
                    && params.get("name").and_then(Value::as_str) == Some(ANNOTATION_BINDING) =>
            {
                if let (Some(context), Some(payload)) = (
                    params.get("executionContextId").and_then(Value::as_i64),
                    params.get("payload").and_then(Value::as_str),
                ) {
                    self.broadcast(SurfaceEvent::Annotation {
                        tab,
                        context,
                        payload: payload.to_string(),
                    });
                }
                true
            }
            Event::DialogOpened {
                dialog,
                kind,
                origin,
                message,
                default_text,
                ..
            } => {
                if agent_owned || owner.is_none() {
                    self.engine.fire(
                        tab,
                        TabCommand::AnswerDialog {
                            dialog: *dialog,
                            accept: false,
                            text: None,
                        },
                    );
                } else {
                    self.broadcast(SurfaceEvent::Dialog {
                        tab,
                        dialog: PageDialog {
                            dialog: *dialog,
                            kind: *kind,
                            origin: origin.clone(),
                            message: message.clone(),
                            default_text: default_text.clone(),
                        },
                    });
                }
                true
            }
            Event::FileChooserOpened {
                request,
                mode,
                title,
                accept,
                ..
            } => {
                if agent_owned || owner.is_none() {
                    self.engine.fire(
                        tab,
                        TabCommand::AnswerFiles {
                            request: *request,
                            paths: None,
                        },
                    );
                    if let Some(entry) = self.lock().tabs.get_mut(&tab) {
                        entry.marks.file_choosers_refused += 1;
                        entry.note(
                            "warning",
                            "Paneflow closed a file chooser: choosing files needs the human"
                                .to_string(),
                        );
                    }
                    self.changed.notify_all();
                } else {
                    self.broadcast(SurfaceEvent::FileChooser {
                        tab,
                        chooser: FileChooser {
                            request: *request,
                            mode: *mode,
                            title: title.clone(),
                            accept: accept.clone(),
                        },
                    });
                }
                true
            }
            Event::NewWindowRequested {
                url,
                user_gesture: true,
                ..
            } => {
                self.open_window(tab, url.clone());
                true
            }
            Event::NewWindowRequested { url, .. } => {
                if let Some(entry) = self.lock().tabs.get_mut(&tab) {
                    entry.note(
                        "warning",
                        format!(
                            "Paneflow blocked a new window for {url}: the page opened it without a click"
                        ),
                    );
                }
                true
            }
            _ => false,
        }
    }

    fn open_window(self: &Arc<Self>, opener: TabId, url: String) {
        let Some((workspace, owner, in_window)) = self.lock().tabs.get(&opener).map(|entry| {
            (
                entry.workspace.clone(),
                entry.owner.clone(),
                entry.in_window,
            )
        }) else {
            return;
        };
        if let Err(error) = validate_url(&url) {
            if let Some(entry) = self.lock().tabs.get_mut(&opener) {
                entry.note(
                    "warning",
                    format!("Paneflow did not open a new window for {url}: {error}"),
                );
            }
            return;
        }
        let inner = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("browser-new-window".into())
            .spawn(
                move || match inner.open_owned(workspace, owner, &url, in_window) {
                    Ok(tab) => inner.broadcast(SurfaceEvent::Opened { tab, opener }),
                    Err(error) => {
                        if let Some(entry) = inner.lock().tabs.get_mut(&opener) {
                            entry.note(
                                "warning",
                                format!("Paneflow could not open a new window for {url}: {error}"),
                            );
                        }
                    }
                },
            );
        if let Err(error) = spawned {
            log::warn!("browser control: cannot open a new window: {error}");
        }
    }

    fn on_event(self: &Arc<Self>, event: ClientEvent) {
        let ClientEvent::Browser(event) = event else {
            self.changed.notify_all();
            self.broadcast(SurfaceEvent::Host);
            return;
        };
        let Some(tab) = event.tab() else {
            self.changed.notify_all();
            return;
        };
        if self.surface_event(tab, &event) {
            return;
        }
        let closed = matches!(event, Event::Closed { .. });
        let decide = {
            let mut state = self.lock();
            if let Event::Closed { .. } = event {
                state.tabs.remove(&tab);
                state.orphans.remove(&tab);
                state.inspectors.remove(&tab);
                None
            } else if state.inspectors.contains(&tab) {
                None
            } else if let Some(entry) = state.tabs.get_mut(&tab) {
                entry.apply(&event).filter(|_| !entry.asking).map(|decide| {
                    entry.asking = true;
                    (decide, entry.workspace.clone(), entry.owner.clone())
                })
            } else {
                if state.orphans.len() < ORPHAN_TABS || state.orphans.contains_key(&tab) {
                    let held = state.orphans.entry(tab).or_default();
                    if held.len() < ORPHAN_EVENTS {
                        held.push(event);
                    }
                }
                None
            }
        };
        self.changed.notify_all();
        self.broadcast(if closed {
            SurfaceEvent::Closed(tab)
        } else {
            SurfaceEvent::Changed(tab)
        });
        if let Some((decide, workspace, owner)) = decide {
            self.decide(tab, decide, workspace, owner);
        }
    }

    fn decide(
        self: &Arc<Self>,
        tab: TabId,
        first: Decide,
        workspace: ProfileName,
        owner: TabOwner,
    ) {
        let inner = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("browser-site-approval".into())
            .spawn(move || {
                let mut next = Some((first, owner));
                while let Some((decide, owner)) = next {
                    let applied = inner.decide_one(tab, &decide, &workspace, owner);
                    next = inner.after_decision(tab, &decide, applied);
                    inner.changed.notify_all();
                }
            });
        if let Err(error) = spawned {
            log::warn!("browser control: cannot ask about a site: {error}");
            if let Some(entry) = self.lock().tabs.get_mut(&tab) {
                entry.asking = false;
            }
        }
    }

    fn decide_one(
        &self,
        tab: TabId,
        decide: &Decide,
        workspace: &ProfileName,
        owner: TabOwner,
    ) -> bool {
        let remembered = self
            .lock()
            .allowed
            .get(workspace)
            .is_some_and(|allowed| allowed.contains(&decide.origin));
        let decision = if remembered {
            OriginDecision::AllowAlways
        } else {
            self.ask_site(&SiteRequest {
                tab,
                owner,
                workspace: workspace.clone(),
                origin: decide.origin.clone(),
                url: decide.url.clone(),
            })
        };
        if decision == OriginDecision::AllowAlways {
            self.lock()
                .allowed
                .entry(workspace.clone())
                .or_default()
                .insert(decide.origin.clone());
        }
        let decided = self.engine.send(
            tab,
            TabCommand::Decide {
                origin: decide.origin.clone(),
                decision,
            },
            self.limits.command,
        );
        if let Err(error) = &decided {
            log::debug!(
                "browser control: the site decision for tab {} was not applied: {error}",
                tab.0
            );
        }
        decided.is_ok() && decision != OriginDecision::Deny
    }

    fn after_decision(
        &self,
        tab: TabId,
        decided: &Decide,
        applied: bool,
    ) -> Option<(Decide, TabOwner)> {
        let mut state = self.lock();
        let entry = state.tabs.get_mut(&tab)?;
        match &entry.pending {
            Some(pending) if pending.seq != decided.seq => {
                return Some((
                    Decide {
                        origin: pending.origin.clone(),
                        url: pending.url.clone(),
                        seq: pending.seq,
                    },
                    entry.owner.clone(),
                ));
            }
            Some(_) if applied => {
                entry.pending = None;
                if entry.state == TabState::AwaitingDecision {
                    entry.state = TabState::Loading;
                }
            }
            _ => {}
        }
        entry.asking = false;
        None
    }

    fn ask_site(&self, request: &SiteRequest) -> OriginDecision {
        let (decision, note) = match self.approvals.site(request) {
            Ok(SiteChoice::AllowOnce) => (OriginDecision::AllowOnce, None),
            Ok(SiteChoice::AllowAlways) => (OriginDecision::AllowAlways, None),
            Ok(SiteChoice::Deny) => (
                OriginDecision::Deny,
                Some(format!("The user denied {}.", request.origin)),
            ),
            Err(reason) => (
                OriginDecision::Deny,
                Some(format!(
                    "Navigation to {} was not approved: {reason}.",
                    request.origin
                )),
            ),
        };
        if let Some(note) = note
            && let Some(entry) = self.lock().tabs.get_mut(&request.tab)
        {
            entry.note("warning", note);
        }
        decision
    }

    fn sweep(&self, state: &mut State, context: &CallContext) {
        let Some(live) = &context.live_sessions else {
            return;
        };
        for (tab, entry) in &mut state.tabs {
            if let TabOwner::Agent { session, .. } = &entry.owner
                && !live.contains(session)
            {
                self.set_owner(*tab, entry, TabOwner::Unclaimed);
            }
        }
    }

    fn describe(tab: TabId, entry: &TabEntry, context: &CallContext) -> BrowserTab {
        BrowserTab {
            tab: tab.0,
            workspace: entry.workspace.to_string(),
            url: entry.url.clone(),
            title: entry.title.clone(),
            state: entry.state,
            owner: entry.owner.clone(),
            drivable: context.identity.owns(&entry.owner),
            presentation: if entry.presented {
                Presentation::Cpu
            } else {
                Presentation::None
            },
        }
    }

    fn visible_tabs(&self, context: &CallContext) -> Vec<BrowserTab> {
        let mut state = self.lock();
        self.sweep(&mut state, context);
        state
            .tabs
            .iter()
            .filter(|(_, entry)| context.sees(entry))
            .map(|(tab, entry)| Self::describe(*tab, entry, context))
            .collect()
    }

    fn status(&self, context: &CallContext) -> BrowserStatus {
        let status = self.engine.status();
        BrowserStatus {
            running: status.running.is_some(),
            host_pid: status.running.as_ref().map(|running| running.pid),
            installed: status.installed,
            binary: status.binary.display().to_string(),
            runtime_version: status
                .running
                .as_ref()
                .map(|running| running.engine.clone()),
            sandbox: match &status.running {
                Some(running) if running.sandbox => "enabled".to_string(),
                Some(_) => "disabled".to_string(),
                None => "unknown".to_string(),
            },
            tabs: self.visible_tabs(context),
        }
    }

    fn install(&self) -> InstallResult {
        let status = self.engine.status();
        let binary = status.binary.display().to_string();
        let message = if status.installed {
            "The browser engine is installed.".to_string()
        } else {
            format!(
                "The browser engine is not installed: {binary} does not exist. A debug build runs the host built from crates/paneflow-browser-host (after scripts/fetch-cef.sh) or the one PANEFLOW_BROWSER_HOST names."
            )
        };
        InstallResult {
            installed: status.installed,
            binary,
            message,
        }
    }

    fn require_installed(&self) -> Result<(), ControlError> {
        let status = self.engine.status();
        if status.installed {
            Ok(())
        } else {
            Err(ControlError::not_installed(format!(
                "The browser engine is not installed: {} does not exist.",
                status.binary.display()
            )))
        }
    }

    fn owned(&self, tab: TabId, context: &CallContext) -> Result<(), ControlError> {
        let mut state = self.lock();
        self.sweep(&mut state, context);
        let entry = state
            .tabs
            .get(&tab)
            .filter(|entry| context.sees(entry))
            .ok_or_else(|| ControlError::unknown_tab(tab))?;
        if context.identity.owns(&entry.owner) {
            return Ok(());
        }
        if let (Identity::Agent { session, .. }, Some(displaced)) =
            (&context.identity, &entry.taken_over_from)
            && session == displaced
        {
            return Err(ControlError::not_owner(format!(
                "the human took over tab {} in the Paneflow window, so it no longer follows this session; ask the user to hand it back, or open a new tab with browser.open",
                tab.0
            )));
        }
        Err(ControlError::not_owner(owner_text(tab, &entry.owner)))
    }

    fn marks(&self, tab: TabId) -> Result<Marks, ControlError> {
        self.lock()
            .tabs
            .get(&tab)
            .map(|entry| entry.marks)
            .ok_or_else(|| ControlError::unknown_tab(tab))
    }

    fn devtools(&self, tab: TabId, method: &str, params: Value) -> Result<Value, ControlError> {
        devtools_value(self.engine.send(
            tab,
            TabCommand::DevTools {
                method: method.to_string(),
                params,
            },
            self.limits.command,
        ))
    }

    fn ensure_domains(&self, tab: TabId) -> Result<(), ControlError> {
        let enabled = self
            .lock()
            .tabs
            .get(&tab)
            .is_some_and(|entry| entry.domains_enabled);
        if enabled {
            return Ok(());
        }
        for method in DOMAINS {
            self.devtools(tab, method, json!({}))?;
        }
        if let Some(entry) = self.lock().tabs.get_mut(&tab) {
            entry.domains_enabled = true;
        }
        Ok(())
    }

    fn navigated(tab: TabId, entry: &TabEntry) -> Navigated {
        Navigated {
            tab: tab.0,
            url: entry.url.clone(),
            title: entry.title.clone(),
            state: entry.state,
            http_status: entry.http_status,
            owner: entry.owner.clone(),
        }
    }

    fn await_navigation(&self, tab: TabId, mark: Marks) -> Result<Navigated, ControlError> {
        let started = Instant::now();
        let limits = self.limits;
        let outcome = self.wait_until(
            |state| {
                let pending = state
                    .tabs
                    .get(&tab)
                    .and_then(|entry| entry.pending.as_ref())
                    .map(|pending| pending.since + limits.approval + limits.navigation);
                let base = started + limits.navigation;
                pending.map_or(base, |pending| pending.max(base))
            },
            |state| {
                let Some(entry) = state.tabs.get(&tab) else {
                    return Some(Err(ControlError::unknown_tab(tab)));
                };
                let marks = entry.marks;
                if marks.crashes > mark.crashes {
                    let reason = entry.crash.clone().unwrap_or_default();
                    return Some(Err(ControlError::failed(format!(
                        "tab {} crashed: {reason}",
                        tab.0
                    ))));
                }
                if marks.refusals > mark.refusals {
                    return Some(Err(entry.last_refusal.as_ref().map_or_else(
                        || ControlError::failed("navigation refused"),
                        ControlError::from_refusal,
                    )));
                }
                if marks.load_failures > mark.load_failures {
                    return Some(Err(ControlError::failed(
                        entry.last_failure.clone().unwrap_or_default(),
                    )));
                }
                if entry.pending.is_some() {
                    return None;
                }
                let loaded = marks.loads_finished > mark.loads_finished && !entry.loading();
                let same_document = marks.same_document > mark.same_document
                    && marks.loads_started == mark.loads_started;
                (loaded || same_document).then(|| Ok(Self::navigated(tab, entry)))
            },
        );
        outcome.unwrap_or_else(|| {
            let url = self
                .lock()
                .tabs
                .get(&tab)
                .map(|entry| entry.url.clone())
                .unwrap_or_default();
            Err(ControlError::timed_out(format!(
                "tab {} did not finish loading {url} within {} s",
                tab.0,
                self.limits.navigation.as_secs()
            )))
        })
    }

    fn open(&self, params: OpenParams, context: &CallContext) -> Result<Navigated, ControlError> {
        validate_url(&params.url)?;
        self.require_installed()?;
        let workspace = context.workspace.clone().ok_or_else(|| {
            ControlError::invalid(
                "no workspace to open the tab in: pass the durable id of a workspace as 'workspace'",
            )
        })?;
        let owner = context.identity.owner();
        let (tab, opened) = self.engine.open_tab(
            workspace.clone(),
            "about:blank",
            guarded(&owner),
            self.limits.command,
        );
        {
            let mut state = self.lock();
            let mut entry = TabEntry::new(workspace, owner, String::new());
            for event in state.orphans.remove(&tab).unwrap_or_default() {
                entry.apply(&event);
            }
            state.tabs.insert(tab, entry);
        }
        if let Err(error) = opened {
            self.lock().tabs.remove(&tab);
            return Err(ControlError::from_command(error));
        }
        let navigation = self.limits.navigation;
        let started = Instant::now();
        let _ = self.wait_until(
            |_| started + navigation,
            |state| {
                state
                    .tabs
                    .get(&tab)
                    .is_none_or(|entry| entry.marks.loads_finished > 0)
                    .then_some(())
            },
        );
        self.ensure_domains(tab)?;
        let mark = self.marks(tab)?;
        self.engine
            .send(tab, TabCommand::Navigate(params.url), self.limits.command)
            .map_err(ControlError::from_command)?;
        self.await_navigation(tab, mark)
    }

    fn navigate(
        &self,
        params: NavigateParams,
        context: &CallContext,
    ) -> Result<Navigated, ControlError> {
        let tab = TabId(params.tab);
        validate_url(&params.url)?;
        self.owned(tab, context)?;
        self.ensure_domains(tab)?;
        let mark = self.marks(tab)?;
        self.engine
            .send(tab, TabCommand::Navigate(params.url), self.limits.command)
            .map_err(ControlError::from_command)?;
        self.await_navigation(tab, mark)
    }

    fn reload(&self, params: TabParams, context: &CallContext) -> Result<Navigated, ControlError> {
        let tab = TabId(params.tab);
        self.owned(tab, context)?;
        let mark = self.marks(tab)?;
        self.engine
            .send(tab, TabCommand::Reload, self.limits.command)
            .map_err(ControlError::from_command)?;
        let navigated = self.await_navigation(tab, mark)?;
        self.ensure_domains(tab)?;
        Ok(navigated)
    }

    fn history_step(
        &self,
        params: TabParams,
        step: i64,
        context: &CallContext,
    ) -> Result<Navigated, ControlError> {
        let tab = TabId(params.tab);
        self.owned(tab, context)?;
        self.ensure_domains(tab)?;
        let history = self.devtools(tab, "Page.getNavigationHistory", json!({}))?;
        let current = history["currentIndex"].as_i64().unwrap_or(0);
        let entries = history["entries"].as_array().cloned().unwrap_or_default();
        let target = current + step;
        let entry = usize::try_from(target)
            .ok()
            .and_then(|index| entries.get(index))
            .filter(|entry| entry["url"].as_str() != Some("about:blank"))
            .ok_or_else(|| {
                ControlError::invalid(format!(
                    "tab {} has no {} page",
                    tab.0,
                    if step < 0 { "earlier" } else { "later" }
                ))
            })?;
        let mark = self.marks(tab)?;
        self.devtools(
            tab,
            "Page.navigateToHistoryEntry",
            json!({"entryId": entry["id"]}),
        )?;
        self.await_navigation(tab, mark)
    }

    fn close(&self, params: TabParams, context: &CallContext) -> Result<Closed, ControlError> {
        let tab = TabId(params.tab);
        self.owned(tab, context)?;
        let url = self
            .lock()
            .tabs
            .get(&tab)
            .map(|entry| entry.url.clone())
            .unwrap_or_default();
        self.engine
            .send(tab, TabCommand::Close, self.limits.command)
            .map_err(ControlError::from_command)?;
        let started = Instant::now();
        let navigation = self.limits.navigation;
        let closed = self
            .wait_until(
                |_| started + navigation,
                |state| (!state.tabs.contains_key(&tab)).then_some(()),
            )
            .is_some();
        if !closed {
            self.lock().tabs.remove(&tab);
        }
        Ok(Closed {
            tab: tab.0,
            url,
            closed: true,
        })
    }

    fn claim(&self, params: TabParams, context: &CallContext) -> Result<BrowserTab, ControlError> {
        let tab = TabId(params.tab);
        let mut state = self.lock();
        self.sweep(&mut state, context);
        let entry = state
            .tabs
            .get_mut(&tab)
            .filter(|entry| context.sees(entry))
            .ok_or_else(|| ControlError::unknown_tab(tab))?;
        match &entry.owner {
            TabOwner::Unclaimed => self.set_owner(tab, entry, context.identity.owner()),
            owner if context.identity.owns(owner) => {}
            owner => {
                return Err(ControlError::not_owner(format!(
                    "tab {} is owned by {}; only an unclaimed tab can be claimed",
                    tab.0,
                    owner.describe()
                )));
            }
        }
        Ok(Self::describe(tab, entry, context))
    }

    fn snapshot(&self, params: TabParams, context: &CallContext) -> Result<Snapshot, ControlError> {
        let tab = TabId(params.tab);
        self.owned(tab, context)?;
        self.ensure_domains(tab)?;
        let generation = self
            .lock()
            .tabs
            .get(&tab)
            .map(|entry| entry.generation)
            .ok_or_else(|| ControlError::unknown_tab(tab))?;
        let tree = self.devtools(tab, "Accessibility.getFullAXTree", json!({}))?;
        let nodes = tree["nodes"].as_array().cloned().unwrap_or_default();
        let mut passwords = HashSet::new();
        for backend in snapshot::password_candidates(&nodes) {
            let described =
                self.devtools(tab, "DOM.describeNode", json!({"backendNodeId": backend}));
            let is_password = described.as_ref().map_or(true, |described| {
                let attributes = described["node"]["attributes"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                attributes.chunks(2).any(|pair| {
                    pair.first().and_then(Value::as_str) == Some("type")
                        && pair
                            .get(1)
                            .and_then(Value::as_str)
                            .is_some_and(|kind| kind.eq_ignore_ascii_case("password"))
                })
            });
            if is_password {
                passwords.insert(backend);
            }
        }
        let rendered = snapshot::render(&nodes, &passwords, SNAPSHOT_TEXT_BYTES);
        let mut state = self.lock();
        let entry = state
            .tabs
            .get_mut(&tab)
            .ok_or_else(|| ControlError::unknown_tab(tab))?;
        entry.refs = rendered.refs;
        entry.refs_generation = generation;
        if rendered.title.is_some() {
            entry.title.clone_from(&rendered.title);
        }
        let console = entry
            .console
            .after(entry.console_seen, SNAPSHOT_ENTRIES, |e| e.seq);
        let network = entry
            .network
            .after(entry.network_seen, SNAPSHOT_ENTRIES, |e| e.seq);
        let console_overflow = entry.console.last_seq().saturating_sub(console.cursor);
        let network_overflow = entry.network.last_seq().saturating_sub(network.cursor);
        entry.console_seen = entry.console.last_seq();
        entry.network_seen = entry.network.last_seq();
        let dialogs = entry.dialogs_after(entry.dialogs_seen);
        entry.dialogs_seen = entry.marks.dialogs;
        Ok(Snapshot {
            tab: tab.0,
            url: entry.url.clone(),
            title: entry.title.clone(),
            loading: entry.loading(),
            generation,
            tree: rendered.tree,
            dropped_bytes: rendered.dropped_bytes,
            console: console.entries,
            network: network.entries,
            dropped_entries: console.dropped
                + network.dropped
                + console_overflow
                + network_overflow,
            dialogs,
        })
    }

    fn reference(&self, tab: TabId, reference: &str) -> Result<i64, ControlError> {
        let state = self.lock();
        let entry = state
            .tabs
            .get(&tab)
            .ok_or_else(|| ControlError::unknown_tab(tab))?;
        if entry.refs.is_empty() || !entry.refs.contains_key(reference) {
            return Err(ControlError::stale(format!(
                "the reference {reference} is not in the latest snapshot of tab {}; take a new snapshot",
                tab.0
            )));
        }
        if entry.refs_generation != entry.generation {
            return Err(ControlError::stale(format!(
                "the reference {reference} is stale: tab {} navigated since the snapshot that issued it; take a new snapshot",
                tab.0
            )));
        }
        entry
            .refs
            .get(reference)
            .copied()
            .ok_or_else(|| ControlError::stale(format!("unknown reference {reference}")))
    }

    fn element_state(
        &self,
        tab: TabId,
        backend: i64,
    ) -> Result<(bool, bool, String), ControlError> {
        let partial = self.devtools(
            tab,
            "Accessibility.getPartialAXTree",
            json!({"backendNodeId": backend, "fetchRelatives": false}),
        )?;
        let nodes = partial["nodes"].as_array().cloned().unwrap_or_default();
        let node = nodes
            .iter()
            .find(|node| node["backendDOMNodeId"].as_i64() == Some(backend))
            .or_else(|| nodes.first())
            .cloned()
            .unwrap_or(Value::Null);
        let hidden = node["ignored"].as_bool() == Some(true);
        let disabled = node["properties"].as_array().is_some_and(|properties| {
            properties.iter().any(|property| {
                property["name"].as_str() == Some("disabled")
                    && property["value"]["value"] == Value::Bool(true)
            })
        });
        let label = format!(
            "{} \"{}\"",
            node["role"]["value"].as_str().unwrap_or("element"),
            node["name"]["value"].as_str().unwrap_or_default()
        );
        let reasons: Vec<String> = node["ignoredReasons"]
            .as_array()
            .map(|reasons| {
                reasons
                    .iter()
                    .filter_map(|reason| reason["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let label = if hidden && !reasons.is_empty() {
            format!("{label} ({})", reasons.join(", "))
        } else {
            label
        };
        Ok((hidden, disabled, label))
    }

    fn center(
        &self,
        tab: TabId,
        backend: i64,
        reference: &str,
    ) -> Result<(f64, f64), ControlError> {
        let _ = self.devtools(
            tab,
            "DOM.scrollIntoViewIfNeeded",
            json!({"backendNodeId": backend}),
        );
        let hidden = || {
            ControlError::new(
                paneflow_api::methods::browser::ERR_BROWSER_FAILED,
                "hidden",
                format!("the element {reference} is hidden: it has no box on the page"),
            )
        };
        let model = self
            .devtools(tab, "DOM.getBoxModel", json!({"backendNodeId": backend}))
            .map_err(|_| hidden())?;
        let quad: Vec<f64> = model["model"]["content"]
            .as_array()
            .map(|points| points.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        if quad.len() < 8 {
            return Err(hidden());
        }
        let xs = [quad[0], quad[2], quad[4], quad[6]];
        let ys = [quad[1], quad[3], quad[5], quad[7]];
        let width = xs.iter().cloned().fold(f64::MIN, f64::max)
            - xs.iter().cloned().fold(f64::MAX, f64::min);
        let height = ys.iter().cloned().fold(f64::MIN, f64::max)
            - ys.iter().cloned().fold(f64::MAX, f64::min);
        if width <= 0.0 || height <= 0.0 {
            return Err(hidden());
        }
        Ok((xs.iter().sum::<f64>() / 4.0, ys.iter().sum::<f64>() / 4.0))
    }

    fn mouse(
        &self,
        tab: TabId,
        kind: &str,
        x: f64,
        y: f64,
        extra: Value,
    ) -> Result<(), ControlError> {
        let mut params = json!({"type": kind, "x": x, "y": y});
        if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
            params.extend(extra.clone());
        }
        self.devtools(tab, "Input.dispatchMouseEvent", params)
            .map(|_| ())
    }

    fn act(&self, params: ActParams, context: &CallContext) -> Result<Acted, ControlError> {
        let tab = TabId(params.tab);
        self.owned(tab, context)?;
        self.ensure_domains(tab)?;
        let reference = params.reference.as_str();
        let backend = self.reference(tab, reference)?;
        let (hidden, disabled, label) = self.element_state(tab, backend)?;
        if hidden {
            return Err(ControlError::new(
                paneflow_api::methods::browser::ERR_BROWSER_FAILED,
                "hidden",
                format!("the element {reference} ({label}) is hidden"),
            ));
        }
        let needs_enabled = matches!(
            params.action,
            Action::Click | Action::Type | Action::Press | Action::Select
        );
        if needs_enabled && disabled {
            return Err(ControlError::new(
                paneflow_api::methods::browser::ERR_BROWSER_FAILED,
                "disabled",
                format!("the element {reference} ({label}) is disabled"),
            ));
        }
        let mark = self.marks(tab)?;
        match params.action {
            Action::Click => {
                let (x, y) = self.center(tab, backend, reference)?;
                self.mouse(tab, "mouseMoved", x, y, json!({}))?;
                self.mouse(
                    tab,
                    "mousePressed",
                    x,
                    y,
                    json!({"button": "left", "clickCount": 1}),
                )?;
                self.mouse(
                    tab,
                    "mouseReleased",
                    x,
                    y,
                    json!({"button": "left", "clickCount": 1}),
                )?;
            }
            Action::Hover => {
                let (x, y) = self.center(tab, backend, reference)?;
                self.mouse(tab, "mouseMoved", x, y, json!({}))?;
            }
            Action::Scroll => {
                let (x, y) = self.center(tab, backend, reference)?;
                let delta = params.delta_y.unwrap_or(DEFAULT_SCROLL);
                self.mouse(
                    tab,
                    "mouseWheel",
                    x,
                    y,
                    json!({"deltaX": 0, "deltaY": delta}),
                )?;
            }
            Action::Type => {
                let text = params
                    .text
                    .ok_or_else(|| ControlError::invalid("type needs 'text'"))?;
                self.devtools(tab, "DOM.focus", json!({"backendNodeId": backend}))?;
                self.devtools(tab, "Input.insertText", json!({"text": text}))?;
            }
            Action::Press => {
                let key = params
                    .key
                    .ok_or_else(|| ControlError::invalid("press needs 'key'"))?;
                let (down, up) = key_event(&key).ok_or_else(|| {
                    ControlError::invalid(format!("'{key}' is not a key name press knows"))
                })?;
                self.devtools(tab, "DOM.focus", json!({"backendNodeId": backend}))?;
                self.devtools(tab, "Input.dispatchKeyEvent", down)?;
                self.devtools(tab, "Input.dispatchKeyEvent", up)?;
            }
            Action::Select => {
                let value = params
                    .value
                    .ok_or_else(|| ControlError::invalid("select needs 'value'"))?;
                self.select(tab, backend, reference, &value)?;
            }
        }
        let settle = self.limits.settle;
        let started = Instant::now();
        let navigating = self
            .wait_until(
                |_| started + settle,
                |state| {
                    let entry = state.tabs.get(&tab)?;
                    let marks = entry.marks;
                    (marks.navigations_requested > mark.navigations_requested
                        || marks.loads_started > mark.loads_started
                        || marks.refusals > mark.refusals
                        || marks.file_choosers_refused > mark.file_choosers_refused
                        || entry.pending.is_some())
                    .then_some(())
                },
            )
            .is_some();
        if navigating {
            self.await_navigation(tab, mark)?;
        }
        let state = self.lock();
        let entry = state
            .tabs
            .get(&tab)
            .ok_or_else(|| ControlError::unknown_tab(tab))?;
        if entry.marks.file_choosers_refused > mark.file_choosers_refused {
            return Err(ControlError::new(
                paneflow_api::methods::browser::ERR_BROWSER_FAILED,
                "needs_human",
                format!(
                    "the action on {reference} opened a file chooser; file selection needs the human, so ask the user to choose the file in the Paneflow window"
                ),
            ));
        }
        let navigated = entry.marks.loads_started > mark.loads_started
            || entry.marks.same_document > mark.same_document;
        Ok(Acted {
            tab: tab.0,
            action: params.action,
            navigated,
            url: entry.url.clone(),
            dialogs: entry.dialogs_after(mark.dialogs),
        })
    }

    fn select(
        &self,
        tab: TabId,
        backend: i64,
        reference: &str,
        value: &str,
    ) -> Result<(), ControlError> {
        let resolved = self.devtools(tab, "DOM.resolveNode", json!({"backendNodeId": backend}))?;
        let object = resolved["object"]["objectId"]
            .as_str()
            .ok_or_else(|| {
                ControlError::failed(format!("the element {reference} cannot be reached"))
            })?
            .to_string();
        let chosen = self.devtools(
            tab,
            "Runtime.callFunctionOn",
            json!({
                "objectId": object,
                "functionDeclaration": "function(wanted){if(!(this instanceof HTMLSelectElement))return 'not_select';const option=[...this.options].find(o=>o.value===wanted||o.label===wanted||o.text.trim()===wanted);if(!option)return 'no_option';this.value=option.value;this.dispatchEvent(new Event('input',{bubbles:true}));this.dispatchEvent(new Event('change',{bubbles:true}));return 'ok';}",
                "arguments": [{"value": value}],
                "returnByValue": true,
                "userGesture": true,
            }),
        )?;
        match chosen["result"]["value"].as_str() {
            Some("ok") => Ok(()),
            Some("not_select") => Err(ControlError::invalid(format!(
                "the element {reference} is not a <select>"
            ))),
            _ => Err(ControlError::invalid(format!(
                "the element {reference} has no option with the value or label {value:?}"
            ))),
        }
    }

    fn wait_for(&self, params: WaitParams, context: &CallContext) -> Result<Waited, ControlError> {
        let tab = TabId(params.tab);
        self.owned(tab, context)?;
        self.ensure_domains(tab)?;
        let timeout =
            Duration::from_millis(params.timeout_ms.unwrap_or(DEFAULT_WAIT.as_millis() as u64))
                .min(MAX_WAIT);
        let network_idle = params.network_idle == Some(true);
        let conditions = usize::from(params.text.is_some())
            + usize::from(params.reference.is_some())
            + usize::from(network_idle);
        if conditions != 1 {
            return Err(ControlError::invalid(
                "browser.wait_for needs exactly one of 'text', 'ref' or 'network_idle'",
            ));
        }
        let started = Instant::now();
        let deadline = started + timeout;
        let condition = if let Some(text) = &params.text {
            let expression = format!(
                "(()=>{{const b=document.body;return !!b&&b.innerText.includes({})}})()",
                serde_json::to_string(text).unwrap_or_default()
            );
            let condition = format!("text {text:?}");
            loop {
                let found = self.devtools(
                    tab,
                    "Runtime.evaluate",
                    json!({"expression": expression, "returnByValue": true}),
                )?;
                if found["result"]["value"] == Value::Bool(true) {
                    break condition;
                }
                if Instant::now() >= deadline {
                    return Err(self.wait_timeout(timeout, &condition));
                }
                std::thread::sleep(POLL);
            }
        } else if let Some(reference) = &params.reference {
            let condition = format!("the element {reference} to be visible");
            loop {
                let backend = self.reference(tab, reference)?;
                let visible = !self.element_state(tab, backend)?.0
                    && self.center(tab, backend, reference).is_ok();
                if visible {
                    break condition;
                }
                if Instant::now() >= deadline {
                    return Err(self.wait_timeout(timeout, &condition));
                }
                std::thread::sleep(POLL);
            }
        } else {
            let condition = format!(
                "no request in flight for {} ms",
                NETWORK_IDLE_QUIET.as_millis()
            );
            let idle = self.wait_until(
                |_| deadline,
                |state| {
                    let entry = state.tabs.get(&tab)?;
                    (entry.inflight.is_empty()
                        && entry.last_network_activity.elapsed() >= NETWORK_IDLE_QUIET
                        && !entry.loading())
                    .then_some(())
                },
            );
            if idle.is_none() {
                return Err(self.wait_timeout(timeout, &condition));
            }
            condition
        };
        Ok(Waited {
            tab: tab.0,
            condition,
            waited_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }

    fn wait_timeout(&self, timeout: Duration, condition: &str) -> ControlError {
        ControlError::timed_out(format!(
            "waited {} ms for {condition}; it never held",
            timeout.as_millis()
        ))
    }

    fn console(
        &self,
        params: HistoryParams,
        context: &CallContext,
    ) -> Result<ConsoleHistory, ControlError> {
        let tab = TabId(params.tab);
        self.owned(tab, context)?;
        let limit = params
            .limit
            .unwrap_or(DEFAULT_HISTORY_ENTRIES)
            .clamp(1, MAX_HISTORY_ENTRIES);
        let state = self.lock();
        let entry = state
            .tabs
            .get(&tab)
            .ok_or_else(|| ControlError::unknown_tab(tab))?;
        let page = entry
            .console
            .after(params.cursor.unwrap_or(0), limit, |e| e.seq);
        Ok(ConsoleHistory {
            tab: tab.0,
            entries: page.entries,
            cursor: page.cursor,
            dropped: page.dropped,
        })
    }

    fn network(
        &self,
        params: HistoryParams,
        context: &CallContext,
    ) -> Result<NetworkHistory, ControlError> {
        let tab = TabId(params.tab);
        self.owned(tab, context)?;
        let limit = params
            .limit
            .unwrap_or(DEFAULT_HISTORY_ENTRIES)
            .clamp(1, MAX_HISTORY_ENTRIES);
        let state = self.lock();
        let entry = state
            .tabs
            .get(&tab)
            .ok_or_else(|| ControlError::unknown_tab(tab))?;
        let page = entry
            .network
            .after(params.cursor.unwrap_or(0), limit, |e| e.seq);
        Ok(NetworkHistory {
            tab: tab.0,
            entries: page.entries,
            cursor: page.cursor,
            dropped: page.dropped,
        })
    }

    fn screenshot(
        &self,
        params: TabParams,
        context: &CallContext,
    ) -> Result<Screenshot, ControlError> {
        let tab = TabId(params.tab);
        self.owned(tab, context)?;
        let shot = self.devtools(
            tab,
            "Page.captureScreenshot",
            json!({"format": "png", "captureBeyondViewport": false}),
        )?;
        let data = shot["data"]
            .as_str()
            .ok_or_else(|| ControlError::failed("the engine returned no image"))?
            .to_string();
        let (width, height) = png_size(&data).ok_or_else(|| {
            ControlError::failed("the engine returned an image that is not a PNG")
        })?;
        let metrics = self.devtools(tab, "Page.getLayoutMetrics", json!({}))?;
        let css_width = metrics["cssVisualViewport"]["clientWidth"]
            .as_f64()
            .or_else(|| metrics["cssLayoutViewport"]["clientWidth"].as_f64())
            .filter(|width| *width > 0.0)
            .unwrap_or(f64::from(width));
        Ok(Screenshot {
            tab: tab.0,
            format: "png".to_string(),
            width,
            height,
            device_scale_factor: (f64::from(width) / css_width * 100.0).round() / 100.0,
            data,
        })
    }

    fn evaluate(
        &self,
        params: EvaluateParams,
        context: &CallContext,
    ) -> Result<Evaluated, ControlError> {
        let tab = TabId(params.tab);
        self.owned(tab, context)?;
        if params.script.len() > MAX_SCRIPT_BYTES {
            return Err(ControlError::invalid(format!(
                "the script is {} bytes, over the limit of {MAX_SCRIPT_BYTES}",
                params.script.len()
            )));
        }
        let (workspace, owner, url) = {
            let state = self.lock();
            let entry = state
                .tabs
                .get(&tab)
                .ok_or_else(|| ControlError::unknown_tab(tab))?;
            (
                entry.workspace.clone(),
                entry.owner.clone(),
                entry.url.clone(),
            )
        };
        let origin = navigation::origin_of(&url).unwrap_or(url);
        if let Identity::Agent { session, agent } = &context.identity {
            let key = (workspace.clone(), session.clone(), agent.clone());
            let approved = self.lock().evaluators.contains(&key);
            if !approved {
                let request = EvaluateRequest {
                    tab,
                    owner: owner.clone(),
                    workspace,
                    origin: origin.clone(),
                    script: params.script.clone(),
                };
                match self.approvals.evaluate(&request) {
                    Ok(true) => {
                        self.lock().evaluators.insert(key);
                    }
                    Ok(false) => {
                        return Err(ControlError::denied(format!(
                            "the user denied evaluation on {origin}; the script did not run"
                        )));
                    }
                    Err(reason) => {
                        return Err(ControlError::denied(format!(
                            "evaluation on {origin} was not approved ({reason}); the script did not run"
                        )));
                    }
                }
            }
        }
        self.ensure_domains(tab)?;
        let evaluate = self.limits.evaluate;
        let started = Instant::now();
        let reply = self.engine.send(
            tab,
            TabCommand::DevTools {
                method: "Runtime.evaluate".to_string(),
                params: json!({
                    "expression": params.script,
                    "returnByValue": true,
                    "awaitPromise": true,
                    "userGesture": true,
                    "timeout": u64::try_from(evaluate.as_millis()).unwrap_or(u64::MAX),
                }),
            },
            evaluate + self.limits.command,
        );
        let timed_out = || {
            ControlError::timed_out(format!(
                "the script was still running after {} s",
                evaluate.as_secs()
            ))
        };
        let result = match reply {
            Err(CommandError::Timeout { .. }) => return Err(timed_out()),
            Err(_) if started.elapsed() >= evaluate => return Err(timed_out()),
            other => devtools_value(other)?,
        };
        if let Some(details) = result.get("exceptionDetails") {
            let message = details["exception"]["description"]
                .as_str()
                .or_else(|| details["text"].as_str())
                .unwrap_or("the script threw")
                .to_string();
            if message.contains("Execution was terminated") {
                return Err(timed_out());
            }
            let (message, dropped) = bounded(message, SNAPSHOT_TEXT_BYTES);
            return Ok(Evaluated {
                tab: tab.0,
                origin,
                value: None,
                exception: Some(message),
                dropped_bytes: dropped,
            });
        }
        let object = &result["result"];
        let text = if let Some(value) = object.get("value") {
            serde_json::to_string(value).unwrap_or_default()
        } else if let Some(text) = object["unserializableValue"].as_str() {
            text.to_string()
        } else if object["type"].as_str() == Some("undefined") {
            "undefined".to_string()
        } else {
            object["description"].as_str().unwrap_or("null").to_string()
        };
        let (text, dropped) = bounded(text, SNAPSHOT_TEXT_BYTES);
        Ok(Evaluated {
            tab: tab.0,
            origin,
            value: Some(text),
            exception: None,
            dropped_bytes: dropped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineStatus;

    #[derive(Default)]
    struct RecordingEngine {
        decided: Mutex<Vec<String>>,
        fired: Mutex<Vec<(TabId, TabCommand)>>,
        opened: Mutex<Vec<(ProfileName, String)>>,
        guarded: Mutex<Vec<bool>>,
    }

    impl Engine for RecordingEngine {
        fn status(&self) -> EngineStatus {
            EngineStatus {
                binary: std::path::PathBuf::from("paneflow-browser-host"),
                installed: true,
                running: None,
            }
        }

        fn open_tab(
            &self,
            _profile: ProfileName,
            _url: &str,
            guarded: bool,
            _deadline: Duration,
        ) -> (TabId, Result<Reply, CommandError>) {
            self.guarded.lock().unwrap().push(guarded);
            let mut opened = self.opened.lock().unwrap();
            opened.push((_profile, _url.to_string()));
            (TabId(opened.len() as u64), Ok(Reply::Done))
        }

        fn send(
            &self,
            _tab: TabId,
            command: TabCommand,
            _deadline: Duration,
        ) -> Result<Reply, CommandError> {
            if let TabCommand::Decide { origin, .. } = command {
                self.decided.lock().unwrap().push(origin);
            }
            Ok(Reply::Done)
        }

        fn fire(&self, tab: TabId, command: TabCommand) {
            self.fired.lock().unwrap().push((tab, command));
        }
    }

    #[derive(Default)]
    struct CountedApprovals {
        sites: Mutex<Vec<String>>,
    }

    impl Approvals for CountedApprovals {
        fn site(&self, request: &SiteRequest) -> Result<SiteChoice, String> {
            self.sites.lock().unwrap().push(request.origin.clone());
            Ok(SiteChoice::Deny)
        }

        fn evaluate(&self, _request: &EvaluateRequest) -> Result<bool, String> {
            Ok(false)
        }
    }

    const SESSION: &str = "a9e52660-0000-4000-8000-000000000001";

    fn agent() -> TabOwner {
        TabOwner::Agent {
            session: SESSION.to_string(),
            agent: Some("claude".to_string()),
        }
    }

    fn workspace() -> ProfileName {
        ProfileName::new("w-1").unwrap()
    }

    fn surface_control() -> (
        Control,
        Arc<RecordingEngine>,
        Arc<CountedApprovals>,
        std::sync::mpsc::Sender<ClientEvent>,
    ) {
        let engine = Arc::new(RecordingEngine::default());
        let approvals = Arc::new(CountedApprovals::default());
        let (events, received) = std::sync::mpsc::channel();
        let control = Control::new(
            Arc::clone(&engine) as Arc<dyn Engine>,
            received,
            Arc::clone(&approvals) as Arc<dyn Approvals>,
            Limits::default(),
        );
        (control, engine, approvals, events)
    }

    fn next_event(
        surface: &Receiver<SurfaceEvent>,
        what: &str,
        wanted: impl Fn(&SurfaceEvent) -> bool,
    ) -> SurfaceEvent {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match surface.recv_timeout(left) {
                Ok(event) if wanted(&event) => return event,
                Ok(_) => {}
                Err(_) => panic!("{what} never arrived"),
            }
        }
    }

    fn dialog(tab: TabId) -> ClientEvent {
        ClientEvent::Browser(Event::DialogOpened {
            tab,
            dialog: 7,
            kind: paneflow_browser_proto::surface::DialogKind::Confirm,
            origin: "http://127.0.0.1:8000".to_string(),
            message: "Delete?".to_string(),
            default_text: String::new(),
        })
    }

    fn chooser(tab: TabId) -> ClientEvent {
        ClientEvent::Browser(Event::FileChooserOpened {
            tab,
            request: 3,
            mode: paneflow_browser_proto::surface::FileMode::Open,
            title: String::new(),
            accept: Vec::new(),
        })
    }

    fn agent_call() -> CallContext {
        CallContext {
            identity: Identity::Agent {
                session: SESSION.to_string(),
                agent: Some("claude".to_string()),
            },
            workspace: Some(workspace()),
            visible: Some(workspace()),
            live_sessions: None,
        }
    }

    #[test]
    fn a_dialog_or_file_chooser_on_an_agent_tab_is_dismissed_and_never_reaches_the_window() {
        let (control, engine, _, events) = surface_control();
        let surface = control.subscribe();
        let tab = control
            .open_surface(workspace(), agent(), "http://127.0.0.1:8000/")
            .unwrap();
        events.send(dialog(tab)).unwrap();
        events.send(chooser(tab)).unwrap();
        settle(&control, "the file chooser refusal", |state| {
            state.tabs[&tab].marks.file_choosers_refused == 1
        });
        assert_eq!(
            *engine.fired.lock().unwrap(),
            [
                (
                    tab,
                    TabCommand::AnswerDialog {
                        dialog: 7,
                        accept: false,
                        text: None,
                    }
                ),
                (
                    tab,
                    TabCommand::AnswerFiles {
                        request: 3,
                        paths: None,
                    }
                ),
            ]
        );
        assert!(surface.try_iter().all(|event| !matches!(
            event,
            SurfaceEvent::Dialog { .. } | SurfaceEvent::FileChooser { .. }
        )));
        let notes: Vec<String> = control.inner.lock().tabs[&tab]
            .console
            .after(0, usize::MAX, |entry| entry.seq)
            .entries
            .into_iter()
            .map(|entry| entry.text)
            .collect();
        assert!(
            notes.iter().any(|text| text.contains("needs the human")),
            "{notes:?}"
        );
    }

    #[test]
    fn a_dialog_or_file_chooser_on_a_human_tab_goes_to_the_window_and_is_not_answered() {
        let (control, engine, _, events) = surface_control();
        let surface = control.subscribe();
        let tab = control
            .open_surface(workspace(), TabOwner::Human, "http://127.0.0.1:8000/")
            .unwrap();
        events.send(dialog(tab)).unwrap();
        let SurfaceEvent::Dialog { dialog, .. } = next_event(&surface, "the dialog", |event| {
            matches!(event, SurfaceEvent::Dialog { .. })
        }) else {
            unreachable!()
        };
        assert_eq!(
            (
                dialog.dialog,
                dialog.origin.as_str(),
                dialog.message.as_str()
            ),
            (7, "http://127.0.0.1:8000", "Delete?")
        );
        events.send(chooser(tab)).unwrap();
        next_event(&surface, "the file chooser", |event| {
            matches!(event, SurfaceEvent::FileChooser { .. })
        });
        assert!(engine.fired.lock().unwrap().is_empty());
    }

    #[test]
    fn only_the_annotation_binding_reaches_the_window_and_it_never_lands_in_the_console() {
        let (control, _, _, events) = surface_control();
        let surface = control.subscribe();
        let tab = control
            .open_surface(workspace(), TabOwner::Human, "http://127.0.0.1:8000/")
            .unwrap();
        let called = |name: &str, payload: &str| {
            ClientEvent::Browser(Event::DevTools {
                tab,
                method: "Runtime.bindingCalled".to_string(),
                params: serde_json::json!({
                    "name": name,
                    "payload": payload,
                    "executionContextId": 9,
                }),
            })
        };
        events.send(called("somethingElse", "{}")).unwrap();
        events
            .send(called(ANNOTATION_BINDING, "{\"event\":\"pick\"}"))
            .unwrap();
        let SurfaceEvent::Annotation {
            tab: from,
            context,
            payload,
        } = next_event(&surface, "the annotation", |event| {
            matches!(event, SurfaceEvent::Annotation { .. })
        })
        else {
            unreachable!()
        };
        assert_eq!(
            (from, context, payload.as_str()),
            (tab, 9, "{\"event\":\"pick\"}")
        );
        let notes = control.inner.lock().tabs[&tab]
            .console
            .after(0, usize::MAX, |entry| entry.seq)
            .entries;
        assert!(notes.iter().all(|entry| !entry.text.contains("pick")));
    }

    #[test]
    fn an_inspector_tab_keeps_its_surface_events_and_never_takes_an_orphan_slot() {
        let (control, _, _, events) = surface_control();
        let surface = control.subscribe();
        let inspector = TabId(77);
        events
            .send(ClientEvent::Browser(Event::TitleChanged {
                tab: inspector,
                title: "DevTools".to_string(),
            }))
            .unwrap();
        next_event(
            &surface,
            "the early change",
            |event| matches!(event, SurfaceEvent::Changed(tab) if *tab == inspector),
        );
        control.adopt_inspector(inspector);
        assert!(!control.inner.lock().orphans.contains_key(&inspector));
        events
            .send(ClientEvent::Browser(Event::TitleChanged {
                tab: inspector,
                title: "DevTools again".to_string(),
            }))
            .unwrap();
        events
            .send(ClientEvent::Browser(Event::Cursor {
                tab: inspector,
                cursor: paneflow_browser_proto::Cursor::Text,
            }))
            .unwrap();
        next_event(
            &surface,
            "the inspector's cursor",
            |event| matches!(event, SurfaceEvent::Cursor { tab, .. } if *tab == inspector),
        );
        assert!(!control.inner.lock().orphans.contains_key(&inspector));
        assert!(control.view(inspector).is_none());
        events
            .send(ClientEvent::Browser(Event::Closed { tab: inspector }))
            .unwrap();
        next_event(
            &surface,
            "the inspector closing",
            |event| matches!(event, SurfaceEvent::Closed(tab) if *tab == inspector),
        );
        assert!(!control.inner.lock().inspectors.contains(&inspector));
    }

    #[test]
    fn a_human_takeover_refuses_the_displaced_agent_by_name_until_the_tab_is_handed_back() {
        let (control, _, _, _) = surface_control();
        let tab = control
            .open_surface(workspace(), agent(), "http://127.0.0.1:8000/")
            .unwrap();
        assert!(control.inner.owned(tab, &agent_call()).is_ok());
        assert!(control.take_over(tab));
        assert!(!control.take_over(tab));
        let view = control.view(tab).unwrap();
        assert_eq!(view.owner, TabOwner::Human);
        assert_eq!(view.taken_over_from.as_deref(), Some(SESSION));
        let refused = control.inner.owned(tab, &agent_call()).unwrap_err();
        assert!(refused.to_string().contains("took over"), "{refused}");
        assert!(control.hand_to(tab, SESSION.to_string(), Some("claude".to_string())));
        let view = control.view(tab).unwrap();
        assert_eq!(view.agent_session(), Some(SESSION));
        assert_eq!(view.taken_over_from, None);
        assert!(control.inner.owned(tab, &agent_call()).is_ok());
    }

    #[test]
    fn a_new_window_opens_as_a_tab_with_its_openers_owner_workspace_and_place_only_after_a_click() {
        let (control, engine, _, events) = surface_control();
        let surface = control.subscribe();
        let opener = control
            .open_surface(workspace(), agent(), "http://127.0.0.1:8000/")
            .unwrap();
        events
            .send(ClientEvent::Browser(Event::NewWindowRequested {
                tab: opener,
                url: "http://127.0.0.1:8000/popup".to_string(),
                user_gesture: true,
            }))
            .unwrap();
        let SurfaceEvent::Opened { tab, opener: from } =
            next_event(&surface, "the new tab", |event| {
                matches!(event, SurfaceEvent::Opened { .. })
            })
        else {
            unreachable!()
        };
        assert_eq!(from, opener);
        let view = control.view(tab).unwrap();
        assert_eq!(
            (view.owner, view.workspace, view.url.as_str()),
            (agent(), workspace(), "http://127.0.0.1:8000/popup")
        );
        assert!(control.inner.lock().tabs[&tab].in_window);
        events
            .send(ClientEvent::Browser(Event::NewWindowRequested {
                tab: opener,
                url: "file:///etc/passwd".to_string(),
                user_gesture: true,
            }))
            .unwrap();
        settle(&control, "the refused window note", |state| {
            state.tabs[&opener]
                .console
                .after(0, usize::MAX, |entry| entry.seq)
                .entries
                .iter()
                .any(|entry| entry.text.contains("did not open a new window"))
        });
        events
            .send(ClientEvent::Browser(Event::NewWindowRequested {
                tab: opener,
                url: "http://127.0.0.1:8000/unasked".to_string(),
                user_gesture: false,
            }))
            .unwrap();
        settle(&control, "the blocked window note", |state| {
            state.tabs[&opener]
                .console
                .after(0, usize::MAX, |entry| entry.seq)
                .entries
                .iter()
                .any(|entry| entry.text.contains("without a click"))
        });
        assert_eq!(
            engine.opened.lock().unwrap().len(),
            2,
            "a window the page opened without a click never becomes a tab"
        );
    }

    #[test]
    fn a_load_failure_is_shown_until_the_next_navigation_starts() {
        let (control, _, _, events) = surface_control();
        let tab = control
            .open_surface(workspace(), TabOwner::Human, "http://127.0.0.1:8000/")
            .unwrap();
        let send = |event: Event| events.send(ClientEvent::Browser(event)).unwrap();
        let failed = "https://vercel.s/";
        send(Event::LoadFailed {
            tab,
            url: failed.to_string(),
            error_code: -105,
            error_text: "ERR_NAME_NOT_RESOLVED".to_string(),
        });
        send(Event::Loaded {
            tab,
            url: failed.to_string(),
            http_status: 0,
        });
        settle(&control, "the error page", |state| {
            state.tabs[&tab].marks.load_failures == 1 && state.tabs[&tab].state == TabState::Loaded
        });
        let shown = control.view(tab).unwrap().failure;
        assert!(
            shown
                .as_deref()
                .is_some_and(|text| text.contains("ERR_NAME_NOT_RESOLVED")),
            "the error page keeps the failure shown: {shown:?}"
        );
        send(Event::LoadStarted {
            tab,
            url: "https://vercel.com/".to_string(),
        });
        settle(&control, "the next navigation", |state| {
            state.tabs[&tab].marks.loads_started > 0
        });
        assert_eq!(control.view(tab).unwrap().failure, None);
        assert!(
            control.inner.lock().tabs[&tab].last_failure.is_some(),
            "agents still read the last failure"
        );
    }

    #[test]
    fn only_a_tab_a_person_drives_opens_a_site_outside_the_local_servers_without_a_decision() {
        let (control, engine, _, _events) = surface_control();
        let human = control
            .open_surface(workspace(), TabOwner::Human, "http://127.0.0.1:8000/")
            .unwrap();
        let unclaimed = control
            .open_surface(workspace(), TabOwner::Unclaimed, "http://127.0.0.1:8000/")
            .unwrap();
        let agent = control
            .open_surface(
                workspace(),
                TabOwner::Agent {
                    session: "s2".to_string(),
                    agent: None,
                },
                "http://127.0.0.1:8000/",
            )
            .unwrap();
        assert_eq!(*engine.guarded.lock().unwrap(), [false, true, true]);
        assert!(control.hand_to(human, "s1".to_string(), None));
        let decide = control.inner.lock().tabs.get_mut(&human).unwrap().apply(
            &Event::OriginDecisionRequested {
                tab: human,
                origin: "https://example.com".to_string(),
                url: "https://example.com/".to_string(),
            },
        );
        assert!(decide.is_some());
        let lapsed = CallContext {
            identity: Identity::Human,
            workspace: None,
            visible: None,
            live_sessions: Some(HashSet::new()),
        };
        control.inner.sweep(&mut control.inner.lock(), &lapsed);
        assert_eq!(control.view(human).unwrap().owner, TabOwner::Unclaimed);
        assert!(
            control.view(human).unwrap().held.is_some(),
            "an agent that goes away leaves its held site held"
        );
        assert!(control.take_over(human));
        assert!(control.view(human).unwrap().held.is_none());
        let guards = |wanted: TabId| -> Vec<bool> {
            engine
                .fired
                .lock()
                .unwrap()
                .iter()
                .filter_map(|(tab, command)| match command {
                    TabCommand::Guard(guarded) if *tab == wanted => Some(*guarded),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            guards(human),
            [true, false],
            "only a person lifts the guard"
        );
        assert!(guards(unclaimed).is_empty());
        assert!(guards(agent).is_empty());
    }

    #[derive(Default)]
    struct HeldQuestions {
        state: Mutex<(Vec<String>, usize, usize, bool)>,
        changed: Condvar,
    }

    impl HeldQuestions {
        fn release(&self) {
            self.state.lock().unwrap().3 = true;
            self.changed.notify_all();
        }

        fn asked(&self) -> Vec<String> {
            self.state.lock().unwrap().0.clone()
        }

        fn most_at_once(&self) -> usize {
            self.state.lock().unwrap().2
        }
    }

    impl Approvals for HeldQuestions {
        fn site(&self, request: &SiteRequest) -> Result<SiteChoice, String> {
            let mut state = self.state.lock().unwrap();
            state.0.push(request.origin.clone());
            state.1 += 1;
            state.2 = state.2.max(state.1);
            self.changed.notify_all();
            while !state.3 {
                state = self.changed.wait(state).unwrap();
            }
            state.1 -= 1;
            Ok(SiteChoice::Deny)
        }

        fn evaluate(&self, _request: &EvaluateRequest) -> Result<bool, String> {
            Ok(false)
        }
    }

    fn settle(control: &Control, what: &str, done: impl Fn(&State) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if done(&control.inner.lock()) {
                return;
            }
            assert!(Instant::now() < deadline, "{what} never happened");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_page_that_keeps_leaving_asks_one_question_at_a_time_and_then_only_the_latest() {
        let engine = Arc::new(RecordingEngine::default());
        let approvals = Arc::new(HeldQuestions::default());
        let (events, received) = std::sync::mpsc::channel();
        let control = Control::new(
            Arc::clone(&engine) as Arc<dyn Engine>,
            received,
            Arc::clone(&approvals) as Arc<dyn Approvals>,
            Limits::default(),
        );
        let owner = TabOwner::Agent {
            session: "a9e52660-0000-4000-8000-000000000001".to_string(),
            agent: None,
        };
        control.inner.lock().tabs.insert(
            TabId(1),
            TabEntry::new(
                ProfileName::new("w-1").unwrap(),
                owner,
                "http://127.0.0.1/".to_string(),
            ),
        );
        let attempts = 50;
        for index in 0..attempts {
            events
                .send(ClientEvent::Browser(Event::OriginDecisionRequested {
                    tab: TabId(1),
                    origin: format!("https://site{index}.example"),
                    url: format!("https://site{index}.example/"),
                }))
                .unwrap();
        }
        settle(&control, "every attempt reaching the tab", |state| {
            state.tabs[&TabId(1)].decisions == attempts
        });
        assert_eq!(approvals.asked(), ["https://site0.example"]);
        approvals.release();
        settle(&control, "the questions ending", |state| {
            !state.tabs[&TabId(1)].asking
        });
        let last = format!("https://site{}.example", attempts - 1);
        assert_eq!(approvals.asked(), ["https://site0.example", last.as_str()]);
        assert_eq!(approvals.most_at_once(), 1);
        assert_eq!(
            *engine.decided.lock().unwrap(),
            ["https://site0.example", last.as_str()]
        );
    }

    #[test]
    fn base64_with_padding_decodes_and_malformed_text_does_not() {
        assert_eq!(decode_base64("aGk=").as_deref(), Some(&b"hi"[..]));
        assert_eq!(decode_base64("aGVsbG8=").as_deref(), Some(&b"hello"[..]));
        assert_eq!(decode_base64("aGVsbG8h").as_deref(), Some(&b"hello!"[..]));
        assert_eq!(decode_base64("a="), None);
        assert_eq!(decode_base64("aG=k"), None);
    }

    #[test]
    fn a_png_header_gives_its_size() {
        let header = "iVBORw0KGgoAAAANSUhEUgAABQAAAAMgCAYAAAA=";
        assert_eq!(png_size(header), Some((1280, 800)));
        assert_eq!(png_size("bm90IGEgcG5n"), None);
    }

    #[test]
    fn only_bounded_http_and_https_urls_without_credentials_are_accepted() {
        assert!(validate_url("http://127.0.0.1:5173/").is_ok());
        assert!(validate_url("https://example.com/").is_ok());
        for refused in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,hi",
            "chrome://settings",
            "about:blank",
            "http://user:pw@127.0.0.1/",
            "not a url",
        ] {
            let error = validate_url(refused).unwrap_err();
            assert_eq!(error.code, crate::error::INVALID_PARAMS, "{refused}");
        }
        let long = format!("http://127.0.0.1/{}", "a".repeat(MAX_URL_CHARS));
        assert!(validate_url(&long).unwrap_err().message.contains("2097152"));
    }
}
