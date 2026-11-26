use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

use cef::*;
use paneflow_browser_proto::limits::DEVTOOLS_METHOD_TIMEOUT;
use paneflow_browser_proto::navigation::{self, Frame as FrameKind, Verdict};
use paneflow_browser_proto::surface::{HistoryStep, Input, Rect, View};
use paneflow_browser_proto::{
    Command, CommandError, ConsoleLevel, Event, ExitReason, HostMessage, NavigationRefusal,
    OriginDecision, Outcome, Permission, ProfileName, Reply, RequestId, TabId,
};

use crate::connection::Outbound;
use crate::handlers;
use crate::inspector::{self, Bridge};
use crate::present::{Out, Presentation};
use crate::ui::{dispatch, on_ui_after, with_host};

const QUIT_GRACE: Duration = Duration::from_secs(5);
const PLACEHOLDER_URL: &str = "about:blank";
const OPAQUE_WHITE: u32 = 0xFFFF_FFFF;

pub enum Effect {
    Reply(RequestId, Result<Reply, CommandError>),
    Emit(Event),
    CreateBrowser {
        tab: TabId,
        context: RequestContext,
    },
    Load(Browser, String, LoadKind),
    Reload(Browser),
    Close(Browser),
    SendDevTools {
        tab: TabId,
        call: i32,
        browser: Browser,
        message: Vec<u8>,
    },
    Flush(RequestContext),
    Resize(Browser),
    Input(Browser, Input),
    History(Browser, HistoryStep),
    Stop(Browser),
    Zoom(Browser, f64),
    ReportSecurity(RequestId, Browser),
    SendRaw {
        browser: Browser,
        message: Vec<u8>,
    },
    RunScript {
        browser: Browser,
        script: String,
    },
    AnswerDialog {
        callback: JsdialogCallback,
        accept: bool,
        text: Option<String>,
    },
    AnswerFiles {
        callback: FileDialogCallback,
        paths: Option<Vec<String>>,
    },
    Present(Vec<Out>),
    Quit,
}

struct PendingCall {
    request: RequestId,
    method: String,
}

struct Held {
    origin: String,
    url: String,
}

struct Tab {
    browser: Browser,
    profile: ProfileName,
    _registration: Option<Registration>,
    next_call: i32,
    calls: HashMap<i32, PendingCall>,
    held: Option<Held>,
    allow_once: Option<String>,
    crashed: Option<String>,
    presentation: Presentation,
    dialogs: HashMap<u64, JsdialogCallback>,
    files: HashMap<u64, FileDialogCallback>,
    on_placeholder: bool,
    first_page: FirstPage,
    guarded: bool,
    inspects: Option<TabId>,
    bridge: Option<Bridge>,
    internal: HashMap<i32, Internal>,
}

enum Internal {
    Setup,
    Shim,
    TargetOf { inspector: TabId },
    Attach { inspector: TabId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadKind {
    Push,
    ReplaceBlank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FirstPage {
    Opening,
    OnBlank,
    Left,
}

impl FirstPage {
    fn after(self, event: &Event) -> Self {
        let url = match event {
            Event::LoadStarted { url, .. }
            | Event::Loaded { url, .. }
            | Event::LoadFailed { url, .. }
            | Event::AddressChanged { url, .. } => url,
            _ => return self,
        };
        match (self, url.as_str() == PLACEHOLDER_URL, event) {
            (Self::Left, _, _) | (_, false, _) => Self::Left,
            (Self::Opening, true, Event::Loaded { .. }) => Self::OnBlank,
            (state, true, _) => state,
        }
    }
}

impl Tab {
    fn load(&self, url: String) -> Effect {
        let kind = if self.first_page == FirstPage::OnBlank {
            LoadKind::ReplaceBlank
        } else {
            LoadKind::Push
        };
        Effect::Load(self.browser.clone(), url, kind)
    }
}

struct Opening {
    request: RequestId,
    profile: ProfileName,
    url: String,
    guarded: bool,
    inspects: Option<TabId>,
}

enum Readiness {
    Initializing(Vec<TabId>),
    Ready,
    Failed(String),
}

struct Profile {
    context: RequestContext,
    allowed: HashSet<String>,
    readiness: Readiness,
}

pub struct Host {
    out: Sender<Outbound>,
    root: PathBuf,
    idle_exit: Duration,
    tabs: HashMap<TabId, Tab>,
    opening: HashMap<TabId, Opening>,
    profiles: HashMap<ProfileName, Profile>,
    idle_generation: u64,
    quitting: bool,
    next_prompt: u64,
}

impl Host {
    pub fn new(out: Sender<Outbound>, root: PathBuf, idle_exit: Duration) -> Self {
        Self {
            out,
            root,
            idle_exit,
            tabs: HashMap::new(),
            opening: HashMap::new(),
            profiles: HashMap::new(),
            idle_generation: 0,
            quitting: false,
            next_prompt: 1,
        }
    }

    pub fn send(&self, message: HostMessage) {
        let _ = self.out.send(Outbound::from(message));
    }

    pub fn send_out(&self, out: Vec<Out>) {
        for out in out {
            let outbound = match out {
                Out::Event(event) => Outbound::from(HostMessage::Event(event)),
                Out::Surface(event, fds) => Outbound {
                    message: HostMessage::Event(event),
                    fds,
                },
            };
            let _ = self.out.send(outbound);
        }
    }

    pub fn handle(&mut self, request: RequestId, command: Command) -> Vec<Effect> {
        if self.quitting {
            return vec![Effect::Reply(
                request,
                Err(CommandError::HostUnavailable {
                    reason: "the browser engine is shutting down".to_string(),
                }),
            )];
        }
        match command {
            Command::OpenTab {
                tab,
                profile,
                url,
                guarded,
            } => self.open_tab(request, tab, profile, url, guarded),
            Command::Guard { tab, guarded } => self.guard(request, tab, guarded),
            Command::CloseTab { tab } => self.close_tab(request, tab),
            Command::Navigate { tab, url } => self.navigate(request, tab, url),
            Command::Reload { tab } => self.reload(request, tab),
            Command::DevTools {
                tab,
                method,
                params,
            } => self.devtools(request, tab, method, params),
            Command::DecideOrigin {
                tab,
                origin,
                decision,
            } => self.decide_origin(request, tab, origin, decision),
            Command::Present { tab, view } => self.present(request, tab, view),
            Command::ReleaseFrame {
                tab,
                generation,
                slot,
            } => self.release_frame(request, tab, generation, slot),
            Command::Input { tab, input } => self.input(request, tab, input),
            Command::History { tab, step } => {
                self.with_browser(request, tab, |browser| Effect::History(browser, step))
            }
            Command::Stop { tab } => self.with_browser(request, tab, Effect::Stop),
            Command::Zoom { tab, percent } => self.with_browser(request, tab, |browser| {
                Effect::Zoom(browser, zoom_level(percent))
            }),
            Command::Inspect { tab, inspector } => self.inspect(request, tab, inspector),
            Command::Security { tab } => match self.live_tab(tab) {
                Ok(entry) => vec![Effect::ReportSecurity(request, entry.browser.clone())],
                Err(error) => vec![Effect::Reply(request, Err(error))],
            },
            Command::AnswerDialog {
                tab,
                dialog,
                accept,
                text,
            } => self.answer_dialog(request, tab, dialog, accept, text),
            Command::AnswerFiles {
                tab,
                request: chooser,
                paths,
            } => self.answer_files(request, tab, chooser, paths),
            Command::Shutdown => {
                let mut effects = vec![Effect::Reply(request, Ok(Reply::Done))];
                effects.extend(self.begin_quit(ExitReason::Shutdown));
                effects
            }
        }
    }

    fn with_browser(
        &mut self,
        request: RequestId,
        tab: TabId,
        effect: impl FnOnce(Browser) -> Effect,
    ) -> Vec<Effect> {
        match self.live_tab(tab) {
            Ok(entry) => vec![
                effect(entry.browser.clone()),
                Effect::Reply(request, Ok(Reply::Done)),
            ],
            Err(error) => vec![Effect::Reply(request, Err(error))],
        }
    }

    fn present(&mut self, request: RequestId, tab: TabId, view: Option<View>) -> Vec<Effect> {
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return vec![Effect::Reply(request, Err(CommandError::TabClosed { tab }))];
        };
        if let Some(view) = view {
            crate::ui::set_view(tab, view);
        }
        let (resize, out) = entry.presentation.present(view);
        let mut effects = vec![Effect::Reply(request, Ok(Reply::Done))];
        if !out.is_empty() {
            effects.push(Effect::Present(out));
        }
        if resize && entry.crashed.is_none() {
            effects.push(Effect::Resize(entry.browser.clone()));
        }
        effects
    }

    fn release_frame(
        &mut self,
        request: RequestId,
        tab: TabId,
        generation: u64,
        slot: u32,
    ) -> Vec<Effect> {
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return vec![Effect::Reply(request, Err(CommandError::TabClosed { tab }))];
        };
        let out = entry.presentation.release(generation, slot);
        let mut effects = vec![Effect::Reply(request, Ok(Reply::Done))];
        if !out.is_empty() {
            effects.push(Effect::Present(out));
        }
        effects
    }

    fn input(&mut self, request: RequestId, tab: TabId, input: Input) -> Vec<Effect> {
        let entry = match self.live_tab(tab) {
            Ok(entry) => entry,
            Err(error) => return vec![Effect::Reply(request, Err(error))],
        };
        let input = offset_into_popup(input, entry.presentation.popup_offset());
        vec![
            Effect::Input(entry.browser.clone(), input),
            Effect::Reply(request, Ok(Reply::Done)),
        ]
    }

    fn answer_dialog(
        &mut self,
        request: RequestId,
        tab: TabId,
        dialog: u64,
        accept: bool,
        text: Option<String>,
    ) -> Vec<Effect> {
        let callback = self
            .tabs
            .get_mut(&tab)
            .and_then(|entry| entry.dialogs.remove(&dialog));
        match callback {
            Some(callback) => vec![
                Effect::AnswerDialog {
                    callback,
                    accept,
                    text,
                },
                Effect::Reply(request, Ok(Reply::Done)),
            ],
            None => vec![Effect::Reply(
                request,
                Err(CommandError::Rejected {
                    reason: format!("tab {} has no open dialog {dialog}", tab.0),
                }),
            )],
        }
    }

    fn answer_files(
        &mut self,
        request: RequestId,
        tab: TabId,
        chooser: u64,
        paths: Option<Vec<String>>,
    ) -> Vec<Effect> {
        let callback = self
            .tabs
            .get_mut(&tab)
            .and_then(|entry| entry.files.remove(&chooser));
        match callback {
            Some(callback) => vec![
                Effect::AnswerFiles { callback, paths },
                Effect::Reply(request, Ok(Reply::Done)),
            ],
            None => vec![Effect::Reply(
                request,
                Err(CommandError::Rejected {
                    reason: format!("tab {} has no open file chooser {chooser}", tab.0),
                }),
            )],
        }
    }

    pub fn dialog_opened(&mut self, tab: TabId, callback: JsdialogCallback) -> Option<u64> {
        let entry = self.tabs.get_mut(&tab)?;
        let id = self.next_prompt;
        self.next_prompt += 1;
        entry.dialogs.insert(id, callback);
        Some(id)
    }

    pub fn dialogs_reset(&mut self, tab: TabId) {
        if let Some(entry) = self.tabs.get_mut(&tab) {
            entry.dialogs.clear();
        }
    }

    pub fn file_chooser_opened(&mut self, tab: TabId, callback: FileDialogCallback) -> Option<u64> {
        let entry = self.tabs.get_mut(&tab)?;
        let id = self.next_prompt;
        self.next_prompt += 1;
        entry.files.insert(id, callback);
        Some(id)
    }

    pub fn painted(&mut self, tab: TabId, popup: bool, buffer: &[u8], width: u32, height: u32) {
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return;
        };
        let out = if popup {
            entry.presentation.paint_popup(buffer, width, height)
        } else {
            entry.presentation.paint_view(buffer, width, height)
        };
        self.send_out(out);
    }

    pub fn popup_shown(&mut self, tab: TabId, shown: bool) {
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return;
        };
        let out = entry.presentation.popup_shown(shown);
        self.send_out(out);
    }

    pub fn popup_moved(&mut self, tab: TabId, rect: Rect) {
        if let Some(entry) = self.tabs.get_mut(&tab) {
            entry.presentation.popup_moved(rect);
        }
    }

    fn open_tab(
        &mut self,
        request: RequestId,
        tab: TabId,
        profile: ProfileName,
        url: String,
        guarded: bool,
    ) -> Vec<Effect> {
        if self.tabs.contains_key(&tab) || self.opening.contains_key(&tab) {
            return vec![Effect::Reply(request, Err(CommandError::TabExists { tab }))];
        }
        if let Verdict::Refuse(refusal) = navigation::classify(&url, FrameKind::TopLevel) {
            return vec![Effect::Reply(
                request,
                Err(CommandError::NavigationRefused { refusal }),
            )];
        }
        if let Err(error) = self.profile_context(&profile) {
            return vec![Effect::Reply(request, Err(error))];
        }
        let Some(entry) = self.profiles.get_mut(&profile) else {
            return vec![Effect::Reply(
                request,
                Err(CommandError::ProfileUnavailable {
                    reason: format!("the profile {profile} vanished"),
                }),
            )];
        };
        let effect = match &mut entry.readiness {
            Readiness::Ready => Some(Effect::CreateBrowser {
                tab,
                context: entry.context.clone(),
            }),
            Readiness::Initializing(waiting) => {
                waiting.push(tab);
                None
            }
            Readiness::Failed(reason) => {
                return vec![Effect::Reply(
                    request,
                    Err(CommandError::ProfileUnavailable {
                        reason: reason.clone(),
                    }),
                )];
            }
        };
        self.idle_generation += 1;
        self.opening.insert(
            tab,
            Opening {
                request,
                profile,
                url,
                guarded,
                inspects: None,
            },
        );
        effect.into_iter().collect()
    }

    fn inspect(&mut self, request: RequestId, tab: TabId, inspector: TabId) -> Vec<Effect> {
        if self.tabs.contains_key(&inspector) || self.opening.contains_key(&inspector) {
            return vec![Effect::Reply(
                request,
                Err(CommandError::TabExists { tab: inspector }),
            )];
        }
        let entry = match self.live_tab(tab) {
            Ok(entry) if entry.inspects.is_some() => {
                return vec![Effect::Reply(
                    request,
                    Err(CommandError::Rejected {
                        reason: format!("tab {} is an inspector", tab.0),
                    }),
                )];
            }
            Ok(entry) => entry,
            Err(error) => return vec![Effect::Reply(request, Err(error))],
        };
        let inspected = self.tabs.values().any(|other| other.inspects == Some(tab))
            || self
                .opening
                .values()
                .any(|opening| opening.inspects == Some(tab));
        if inspected {
            return vec![Effect::Reply(
                request,
                Err(CommandError::Rejected {
                    reason: format!("tab {} already has an inspector", tab.0),
                }),
            )];
        }
        let profile = entry.profile.clone();
        let Some(context) = self
            .profiles
            .get(&profile)
            .map(|entry| entry.context.clone())
        else {
            return vec![Effect::Reply(request, Err(CommandError::TabClosed { tab }))];
        };
        self.idle_generation += 1;
        self.opening.insert(
            inspector,
            Opening {
                request,
                profile,
                url: PLACEHOLDER_URL.to_string(),
                guarded: false,
                inspects: Some(tab),
            },
        );
        vec![Effect::CreateBrowser {
            tab: inspector,
            context,
        }]
    }

    fn internal_call(
        &mut self,
        tab: TabId,
        method: &str,
        params: serde_json::Value,
        purpose: Internal,
    ) -> Option<Effect> {
        let entry = self.tabs.get_mut(&tab)?;
        let call = entry.next_call;
        entry.next_call = entry.next_call.wrapping_add(1).max(1);
        entry.internal.insert(call, purpose);
        let message = serde_json::json!({ "id": call, "method": method, "params": params });
        Some(Effect::SendDevTools {
            tab,
            call,
            browser: entry.browser.clone(),
            message: message.to_string().into_bytes(),
        })
    }

    fn start_inspector(&mut self, inspector: TabId, inspected: TabId) -> Vec<Effect> {
        let Some(entry) = self.tabs.get_mut(&inspector) else {
            return Vec::new();
        };
        entry.inspects = Some(inspected);
        entry.bridge = Some(Bridge::new(inspected));
        let mut effects: Vec<Effect> = [
            ("Runtime.enable", serde_json::json!({}), Internal::Setup),
            ("Page.enable", serde_json::json!({}), Internal::Setup),
            (
                "Emulation.setUserAgentOverride",
                serde_json::json!({ "userAgent": "", "acceptLanguage": inspector::FRONTEND_LOCALE }),
                Internal::Setup,
            ),
            (
                "Runtime.addBinding",
                serde_json::json!({ "name": inspector::BINDING }),
                Internal::Setup,
            ),
            (
                "Page.addScriptToEvaluateOnNewDocument",
                serde_json::json!({ "source": inspector::SHIM }),
                Internal::Shim,
            ),
        ]
        .into_iter()
        .filter_map(|(method, params, purpose)| {
            self.internal_call(inspector, method, params, purpose)
        })
        .collect();
        effects.extend(self.internal_call(
            inspected,
            "Target.getTargetInfo",
            serde_json::json!({}),
            Internal::TargetOf { inspector },
        ));
        effects
    }

    fn internal_result(
        &mut self,
        tab: TabId,
        purpose: Internal,
        success: bool,
        result: &serde_json::Value,
    ) -> Vec<Effect> {
        match purpose {
            Internal::Setup => Vec::new(),
            Internal::Shim if success => self
                .tabs
                .get(&tab)
                .map(|entry| {
                    Effect::Load(
                        entry.browser.clone(),
                        inspector::FRONTEND_URL.to_string(),
                        LoadKind::Push,
                    )
                })
                .into_iter()
                .collect(),
            Internal::Shim => self.close_inspector(tab),
            Internal::TargetOf { inspector } => {
                let target = result
                    .pointer("/targetInfo/targetId")
                    .and_then(serde_json::Value::as_str);
                match (success, target) {
                    (true, Some(target)) => self
                        .internal_call(
                            tab,
                            "Target.attachToTarget",
                            serde_json::json!({ "targetId": target, "flatten": true }),
                            Internal::Attach { inspector },
                        )
                        .into_iter()
                        .collect(),
                    _ => self.close_inspector(inspector),
                }
            }
            Internal::Attach { inspector } => {
                let session = result.get("sessionId").and_then(serde_json::Value::as_str);
                let Some(session) = session.filter(|_| success).map(str::to_string) else {
                    return self.close_inspector(inspector);
                };
                let Some(browser) = self.tabs.get(&tab).map(|entry| entry.browser.clone()) else {
                    return Vec::new();
                };
                let Some(bridge) = self
                    .tabs
                    .get_mut(&inspector)
                    .and_then(|entry| entry.bridge.as_mut())
                else {
                    return vec![detach(browser, &session)];
                };
                bridge
                    .attached(session)
                    .into_iter()
                    .map(|message| Effect::SendRaw {
                        browser: browser.clone(),
                        message,
                    })
                    .collect()
            }
        }
    }

    fn close_inspector(&mut self, inspector: TabId) -> Vec<Effect> {
        self.tabs
            .get(&inspector)
            .map(|entry| Effect::Close(entry.browser.clone()))
            .into_iter()
            .collect()
    }

    pub fn devtools_event(
        &mut self,
        tab: TabId,
        method: String,
        params: serde_json::Value,
    ) -> Vec<Effect> {
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return Vec::new();
        };
        let Some(bridge) = entry.bridge.as_mut() else {
            return vec![Effect::Emit(Event::DevTools {
                tab,
                method,
                params,
            })];
        };
        if method != "Runtime.bindingCalled"
            || params.get("name").and_then(serde_json::Value::as_str) != Some(inspector::BINDING)
        {
            return Vec::new();
        }
        let payload = params
            .get("payload")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let inspected = bridge.inspected;
        let Some(message) = bridge.outgoing(payload) else {
            return Vec::new();
        };
        self.tabs
            .get(&inspected)
            .map(|entry| Effect::SendRaw {
                browser: entry.browser.clone(),
                message,
            })
            .into_iter()
            .collect()
    }

    pub fn session_message(&mut self, tab: TabId, message: &[u8]) -> Option<Vec<Effect>> {
        if !contains(message, b"\"sessionId\"") {
            return None;
        }
        let (browser, bridge) = self
            .tabs
            .values_mut()
            .find(|entry| entry.inspects == Some(tab))
            .and_then(|entry| {
                let browser = entry.browser.clone();
                entry.bridge.as_mut().map(|bridge| (browser, bridge))
            })?;
        let script = bridge.incoming(message)?;
        Some(vec![Effect::RunScript { browser, script }])
    }

    fn guard(&mut self, request: RequestId, tab: TabId, guarded: bool) -> Vec<Effect> {
        let mut effects = vec![Effect::Reply(request, Ok(Reply::Done))];
        if let Some(opening) = self.opening.get_mut(&tab) {
            opening.guarded = guarded;
            return effects;
        }
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return vec![Effect::Reply(request, Err(CommandError::TabClosed { tab }))];
        };
        entry.guarded = guarded;
        if !guarded && let Some(held) = entry.held.take() {
            effects.push(Effect::Emit(Event::NavigationRefused {
                tab,
                url: held.url,
                refusal: NavigationRefusal::OriginUndecided {
                    origin: held.origin,
                },
            }));
        }
        effects
    }

    pub fn profile_initialized(
        &mut self,
        profile: &ProfileName,
        outcome: Result<(), String>,
    ) -> Vec<Effect> {
        let Some(entry) = self.profiles.get_mut(profile) else {
            return Vec::new();
        };
        let waiting = match std::mem::replace(&mut entry.readiness, Readiness::Ready) {
            Readiness::Initializing(waiting) => waiting,
            other => {
                entry.readiness = other;
                return Vec::new();
            }
        };
        match outcome {
            Ok(()) => waiting
                .into_iter()
                .filter(|tab| self.opening.contains_key(tab))
                .map(|tab| Effect::CreateBrowser {
                    tab,
                    context: entry.context.clone(),
                })
                .collect(),
            Err(reason) => {
                let reason =
                    format!("the profile {profile} could not deny site permissions: {reason}");
                entry.readiness = Readiness::Failed(reason.clone());
                let effects = waiting
                    .into_iter()
                    .filter_map(|tab| self.opening.remove(&tab))
                    .map(|opening| {
                        Effect::Reply(
                            opening.request,
                            Err(CommandError::ProfileUnavailable {
                                reason: reason.clone(),
                            }),
                        )
                    })
                    .collect();
                if self.tabs.is_empty() && self.opening.is_empty() && !self.quitting {
                    self.schedule_idle();
                }
                effects
            }
        }
    }

    pub fn tab_created(&mut self, tab: TabId, browser: Browser) -> Vec<Effect> {
        let Some(opening) = self.opening.remove(&tab) else {
            return vec![Effect::Close(browser)];
        };
        if self.quitting {
            return vec![
                Effect::Close(browser),
                Effect::Reply(
                    opening.request,
                    Err(CommandError::HostUnavailable {
                        reason: "the browser engine is shutting down".to_string(),
                    }),
                ),
            ];
        }
        let inspects = opening.inspects;
        let mut effects = self.insert_tab(
            opening.request,
            tab,
            opening.profile,
            browser,
            opening.url,
            opening.guarded,
        );
        if let Some(inspected) = inspects {
            effects.extend(self.start_inspector(tab, inspected));
        }
        effects
    }

    pub fn creation_failed(&mut self, tab: TabId) -> Vec<Effect> {
        let effects: Vec<Effect> = self
            .opening
            .remove(&tab)
            .map(|opening| {
                Effect::Reply(
                    opening.request,
                    Err(CommandError::Rejected {
                        reason: "the engine could not create the tab".to_string(),
                    }),
                )
            })
            .into_iter()
            .collect();
        if self.tabs.is_empty() && self.opening.is_empty() && !self.quitting {
            self.schedule_idle();
        }
        effects
    }

    fn profile_context(&mut self, profile: &ProfileName) -> Result<RequestContext, CommandError> {
        if let Some(existing) = self.profiles.get(profile) {
            return Ok(existing.context.clone());
        }
        let directory = self.root.join(profile.as_str());
        std::fs::create_dir_all(&directory).map_err(|error| CommandError::ProfileUnavailable {
            reason: format!("{}: {error}", directory.display()),
        })?;
        restrict_to_owner(&directory);
        let settings = RequestContextSettings {
            cache_path: directory.to_string_lossy().as_ref().into(),
            persist_session_cookies: 1,
            ..Default::default()
        };
        let mut handler = handlers::profile_handler(profile.clone());
        let context = request_context_create_context(Some(&settings), Some(&mut handler))
            .ok_or_else(|| CommandError::ProfileUnavailable {
                reason: format!("the engine refused the profile {}", directory.display()),
            })?;
        self.profiles.insert(
            profile.clone(),
            Profile {
                context: context.clone(),
                allowed: HashSet::new(),
                readiness: Readiness::Initializing(Vec::new()),
            },
        );
        Ok(context)
    }

    fn insert_tab(
        &mut self,
        request: RequestId,
        tab: TabId,
        profile: ProfileName,
        browser: Browser,
        url: String,
        guarded: bool,
    ) -> Vec<Effect> {
        let registration = browser.host().and_then(|host| {
            let mut observer = handlers::devtools_observer(tab);
            host.add_dev_tools_message_observer(Some(&mut observer))
        });
        self.tabs.insert(
            tab,
            Tab {
                browser,
                profile,
                _registration: registration,
                next_call: 1,
                calls: HashMap::new(),
                held: None,
                allow_once: None,
                crashed: None,
                presentation: Presentation::new(tab),
                dialogs: HashMap::new(),
                files: HashMap::new(),
                on_placeholder: url != PLACEHOLDER_URL,
                first_page: FirstPage::Opening,
                guarded,
                inspects: None,
                bridge: None,
                internal: HashMap::new(),
            },
        );
        let mut effects = if url == PLACEHOLDER_URL {
            Vec::new()
        } else {
            self.start_navigation(tab, url)
        };
        effects.insert(0, Effect::Reply(request, Ok(Reply::Done)));
        effects
    }

    fn start_navigation(&mut self, tab: TabId, url: String) -> Vec<Effect> {
        let verdict = navigation::classify(&url, FrameKind::TopLevel);
        let Some(entry) = self.tabs.get(&tab) else {
            return Vec::new();
        };
        let allowed = self
            .profiles
            .get(&entry.profile)
            .map(|profile| &profile.allowed);
        match verdict {
            Verdict::Allow => vec![entry.load(url)],
            Verdict::External { .. } if !entry.guarded => vec![entry.load(url)],
            Verdict::External { origin } if allowed.is_some_and(|set| set.contains(&origin)) => {
                vec![entry.load(url)]
            }
            Verdict::External { origin } => self.hold(tab, origin, url),
            Verdict::Refuse(refusal) => {
                vec![Effect::Emit(Event::NavigationRefused { tab, url, refusal })]
            }
        }
    }

    fn hold(&mut self, tab: TabId, origin: String, url: String) -> Vec<Effect> {
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return Vec::new();
        };
        entry.held = Some(Held {
            origin: origin.clone(),
            url: url.clone(),
        });
        vec![Effect::Emit(Event::OriginDecisionRequested {
            tab,
            origin,
            url,
        })]
    }

    fn live_tab(&self, tab: TabId) -> Result<&Tab, CommandError> {
        let entry = self.tabs.get(&tab).ok_or(CommandError::TabClosed { tab })?;
        if let Some(reason) = &entry.crashed {
            return Err(CommandError::TabCrashed {
                tab,
                reason: reason.clone(),
            });
        }
        Ok(entry)
    }

    fn navigate(&mut self, request: RequestId, tab: TabId, url: String) -> Vec<Effect> {
        if let Err(error) = self.live_tab(tab) {
            return vec![Effect::Reply(request, Err(error))];
        }
        if let Verdict::Refuse(refusal) = navigation::classify(&url, FrameKind::TopLevel) {
            return vec![Effect::Reply(
                request,
                Err(CommandError::NavigationRefused { refusal }),
            )];
        }
        if let Some(entry) = self.tabs.get_mut(&tab) {
            entry.held = None;
            entry.allow_once = None;
        }
        let mut effects = vec![Effect::Reply(request, Ok(Reply::Done))];
        effects.extend(self.start_navigation(tab, url));
        effects
    }

    fn reload(&mut self, request: RequestId, tab: TabId) -> Vec<Effect> {
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return vec![Effect::Reply(request, Err(CommandError::TabClosed { tab }))];
        };
        entry.crashed = None;
        vec![
            Effect::Reload(entry.browser.clone()),
            Effect::Reply(request, Ok(Reply::Done)),
        ]
    }

    fn close_tab(&mut self, request: RequestId, tab: TabId) -> Vec<Effect> {
        match self.tabs.get(&tab) {
            Some(entry) => vec![
                Effect::Close(entry.browser.clone()),
                Effect::Reply(request, Ok(Reply::Done)),
            ],
            None => vec![Effect::Reply(request, Err(CommandError::TabClosed { tab }))],
        }
    }

    fn devtools(
        &mut self,
        request: RequestId,
        tab: TabId,
        method: String,
        params: serde_json::Value,
    ) -> Vec<Effect> {
        if method.is_empty() || !(params.is_object() || params.is_null()) {
            return vec![Effect::Reply(
                request,
                Err(CommandError::Rejected {
                    reason: "a DevTools call needs a method name and object parameters".to_string(),
                }),
            )];
        }
        if let Err(error) = self.live_tab(tab) {
            return vec![Effect::Reply(request, Err(error))];
        }
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return vec![Effect::Reply(request, Err(CommandError::TabClosed { tab }))];
        };
        let call = entry.next_call;
        entry.next_call = entry.next_call.wrapping_add(1).max(1);
        let message = serde_json::json!({
            "id": call,
            "method": method,
            "params": if params.is_null() { serde_json::json!({}) } else { params },
        });
        entry.calls.insert(call, PendingCall { request, method });
        on_ui_after(DEVTOOLS_METHOD_TIMEOUT, move || {
            dispatch(|host| host.devtools_timeout(tab, call));
        });
        vec![Effect::SendDevTools {
            tab,
            call,
            browser: entry.browser.clone(),
            message: message.to_string().into_bytes(),
        }]
    }

    pub fn devtools_not_sent(&mut self, tab: TabId, call: i32) -> Vec<Effect> {
        self.finish_call(tab, call, |method| {
            Err(CommandError::DevTools {
                method,
                message: "the engine did not accept the message".to_string(),
            })
        })
    }

    fn devtools_timeout(&mut self, tab: TabId, call: i32) -> Vec<Effect> {
        let after_ms = u64::try_from(DEVTOOLS_METHOD_TIMEOUT.as_millis()).unwrap_or(u64::MAX);
        self.finish_call(tab, call, |method| {
            Err(CommandError::Timeout { method, after_ms })
        })
    }

    pub fn devtools_result(
        &mut self,
        tab: TabId,
        call: i32,
        success: bool,
        result: &[u8],
    ) -> Vec<Effect> {
        let parsed: serde_json::Value =
            serde_json::from_slice(result).unwrap_or(serde_json::Value::Null);
        if let Some(purpose) = self
            .tabs
            .get_mut(&tab)
            .and_then(|entry| entry.internal.remove(&call))
        {
            return self.internal_result(tab, purpose, success, &parsed);
        }
        self.finish_call(tab, call, |method| {
            if success {
                Ok(Reply::DevTools(parsed))
            } else {
                let message = parsed
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| parsed.to_string());
                Err(CommandError::DevTools { method, message })
            }
        })
    }

    fn finish_call(
        &mut self,
        tab: TabId,
        call: i32,
        outcome: impl FnOnce(String) -> Result<Reply, CommandError>,
    ) -> Vec<Effect> {
        let pending = self
            .tabs
            .get_mut(&tab)
            .and_then(|entry| entry.calls.remove(&call));
        match pending {
            Some(pending) => vec![Effect::Reply(pending.request, outcome(pending.method))],
            None => Vec::new(),
        }
    }

    fn decide_origin(
        &mut self,
        request: RequestId,
        tab: TabId,
        origin: String,
        decision: OriginDecision,
    ) -> Vec<Effect> {
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return vec![Effect::Reply(request, Err(CommandError::TabClosed { tab }))];
        };
        let held = match entry.held.take() {
            Some(held) if held.origin == origin => held,
            other => {
                entry.held = other;
                return vec![Effect::Reply(
                    request,
                    Err(CommandError::NoPendingDecision { tab, origin }),
                )];
            }
        };
        let load = entry.load(held.url.clone());
        let profile = entry.profile.clone();
        let mut effects = vec![Effect::Reply(request, Ok(Reply::Done))];
        match decision {
            OriginDecision::AllowOnce => {
                entry.allow_once = Some(held.origin);
                effects.push(load);
            }
            OriginDecision::AllowAlways => {
                if let Some(profile) = self.profiles.get_mut(&profile) {
                    profile.allowed.insert(held.origin);
                }
                effects.push(load);
            }
            OriginDecision::Deny => effects.push(Effect::Emit(Event::NavigationRefused {
                tab,
                url: held.url,
                refusal: NavigationRefusal::OriginDenied {
                    origin: held.origin,
                },
            })),
        }
        effects
    }

    pub fn before_browse(
        &mut self,
        tab: TabId,
        url: String,
        top_level: bool,
    ) -> (bool, Vec<Effect>) {
        let frame = if top_level {
            FrameKind::TopLevel
        } else {
            FrameKind::Subframe
        };
        let opening_inspector = self
            .opening
            .get(&tab)
            .is_some_and(|opening| opening.inspects.is_some());
        if opening_inspector {
            return (!inspector::is_frontend_page(&url), Vec::new());
        }
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return (true, Vec::new());
        };
        if entry.inspects.is_some() {
            return (!inspector::is_frontend_page(&url), Vec::new());
        }
        let allowed = self
            .profiles
            .get(&entry.profile)
            .map(|profile| &profile.allowed);
        match navigation::classify(&url, frame) {
            Verdict::Allow => (false, Vec::new()),
            Verdict::External { .. } if !entry.guarded => (false, Vec::new()),
            Verdict::External { origin } if allowed.is_some_and(|set| set.contains(&origin)) => {
                (false, Vec::new())
            }
            Verdict::External { origin }
                if top_level && entry.allow_once.as_deref() == Some(origin.as_str()) =>
            {
                entry.allow_once = None;
                (false, Vec::new())
            }
            Verdict::External { origin } if top_level => (true, self.hold(tab, origin, url)),
            Verdict::External { origin } => (
                true,
                vec![Effect::Emit(Event::NavigationRefused {
                    tab,
                    url,
                    refusal: NavigationRefusal::OriginUndecided { origin },
                })],
            ),
            Verdict::Refuse(refusal) => (
                true,
                vec![Effect::Emit(Event::NavigationRefused { tab, url, refusal })],
            ),
        }
    }

    pub fn tab_crashed(&mut self, tab: TabId, reason: String) -> Vec<Effect> {
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return Vec::new();
        };
        entry.crashed = Some(reason.clone());
        let mut effects: Vec<Effect> = entry
            .calls
            .drain()
            .map(|(_, pending)| {
                Effect::Reply(
                    pending.request,
                    Err(CommandError::TabCrashed {
                        tab,
                        reason: reason.clone(),
                    }),
                )
            })
            .collect();
        effects.push(Effect::Emit(Event::Crashed { tab, reason }));
        effects
    }

    pub fn tab_closed(&mut self, tab: TabId) -> Vec<Effect> {
        let Some(mut entry) = self.tabs.remove(&tab) else {
            return Vec::new();
        };
        crate::ui::forget_view(tab);
        let mut cleanup: Vec<Effect> = self
            .tabs
            .values()
            .filter(|other| other.inspects == Some(tab))
            .map(|other| Effect::Close(other.browser.clone()))
            .collect();
        if let (Some(inspected), Some(session)) = (
            entry.inspects,
            entry.bridge.as_ref().and_then(Bridge::session),
        ) && let Some(page) = self.tabs.get(&inspected)
        {
            cleanup.push(detach(page.browser.clone(), session));
        }
        let mut effects: Vec<Effect> = entry
            .calls
            .drain()
            .map(|(_, pending)| {
                Effect::Reply(pending.request, Err(CommandError::TabClosed { tab }))
            })
            .collect();
        effects.push(Effect::Emit(Event::Closed { tab }));
        effects.extend(cleanup);
        let profile_still_open = self
            .tabs
            .values()
            .any(|other| other.profile == entry.profile);
        if !profile_still_open && let Some(profile) = self.profiles.get(&entry.profile) {
            effects.push(Effect::Flush(profile.context.clone()));
        }
        if self.tabs.is_empty() && self.opening.is_empty() {
            if self.quitting {
                effects.push(Effect::Quit);
            } else {
                self.schedule_idle();
            }
        }
        effects
    }

    pub fn schedule_idle(&mut self) {
        self.idle_generation += 1;
        let generation = self.idle_generation;
        on_ui_after(self.idle_exit, move || {
            dispatch(|host| {
                if host.tabs.is_empty()
                    && host.opening.is_empty()
                    && host.idle_generation == generation
                {
                    host.begin_quit(ExitReason::Idle)
                } else {
                    Vec::new()
                }
            });
        });
    }

    pub fn begin_quit(&mut self, reason: ExitReason) -> Vec<Effect> {
        if self.quitting {
            return Vec::new();
        }
        self.quitting = true;
        let mut effects = vec![Effect::Emit(Event::Exiting { reason })];
        for profile in self.profiles.values() {
            effects.push(Effect::Flush(profile.context.clone()));
        }
        if self.tabs.is_empty() && self.opening.is_empty() {
            effects.push(Effect::Quit);
        } else {
            effects.extend(
                self.tabs
                    .values()
                    .map(|entry| Effect::Close(entry.browser.clone())),
            );
            on_ui_after(QUIT_GRACE, || run_effects(vec![Effect::Quit]));
        }
        effects
    }

    pub fn emit(&self, event: Event) {
        self.send(HostMessage::Event(event));
    }

    pub fn page_event(&mut self, event: Event) -> Vec<Effect> {
        let placeholder_url = match &event {
            Event::LoadStarted { url, .. }
            | Event::Loaded { url, .. }
            | Event::LoadFailed { url, .. }
            | Event::AddressChanged { url, .. } => Some(url.as_str() == PLACEHOLDER_URL),
            Event::TitleChanged { .. }
            | Event::LoadingState { .. }
            | Event::LoadProgress { .. } => None,
            _ => return vec![Effect::Emit(event)],
        };
        let Some(entry) = event.tab().and_then(|tab| self.tabs.get_mut(&tab)) else {
            return vec![Effect::Emit(event)];
        };
        entry.first_page = entry.first_page.after(&event);
        if !entry.on_placeholder {
            return vec![Effect::Emit(event)];
        }
        match placeholder_url {
            Some(false) => {
                entry.on_placeholder = false;
                vec![Effect::Emit(event)]
            }
            Some(true) | None => Vec::new(),
        }
    }
}

pub fn block_permissions(context: &RequestContext) -> Result<(), String> {
    for kind in crate::app::BLOCKED_PERMISSIONS {
        context.set_content_setting(None, None, kind, ContentSettingValues::BLOCK);
        let applied = context.content_setting(None, None, kind);
        if applied != ContentSettingValues::BLOCK {
            return Err(format!("the engine kept {kind:?} at {applied:?}"));
        }
    }
    Ok(())
}

pub fn console_level(level: LogSeverity) -> ConsoleLevel {
    if level == LogSeverity::ERROR || level == LogSeverity::FATAL {
        ConsoleLevel::Error
    } else if level == LogSeverity::WARNING {
        ConsoleLevel::Warning
    } else if level == LogSeverity::VERBOSE {
        ConsoleLevel::Debug
    } else {
        ConsoleLevel::Info
    }
}

pub fn prompt_permissions(bits: u32) -> Vec<Permission> {
    let known = [
        (
            PermissionRequestTypes::CAMERA_STREAM.get_raw(),
            Permission::Camera,
        ),
        (
            PermissionRequestTypes::MIC_STREAM.get_raw(),
            Permission::Microphone,
        ),
        (
            PermissionRequestTypes::GEOLOCATION.get_raw(),
            Permission::Geolocation,
        ),
        (
            PermissionRequestTypes::NOTIFICATIONS.get_raw(),
            Permission::Notifications,
        ),
    ];
    collect_permissions(bits, &known)
}

pub fn media_permissions(bits: u32) -> Vec<Permission> {
    let known = [
        (
            MediaAccessPermissionTypes::DEVICE_VIDEO_CAPTURE.get_raw(),
            Permission::Camera,
        ),
        (
            MediaAccessPermissionTypes::DEVICE_AUDIO_CAPTURE.get_raw(),
            Permission::Microphone,
        ),
    ];
    collect_permissions(bits, &known)
}

fn collect_permissions(bits: u32, known: &[(u32, Permission)]) -> Vec<Permission> {
    let mut permissions: Vec<Permission> = known
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, permission)| *permission)
        .collect();
    let named = known.iter().fold(0, |all, (bit, _)| all | bit);
    if bits & !named != 0 {
        permissions.push(Permission::Other);
    }
    permissions
}

pub fn offset_into_popup(input: Input, popup: Option<(Rect, i32, i32)>) -> Input {
    let Some((placed, dx, dy)) = popup else {
        return input;
    };
    let shift = |x: i32, y: i32| {
        if placed.contains(x, y) {
            (x + dx, y + dy)
        } else {
            (x, y)
        }
    };
    match input {
        Input::MouseMove { x, y, modifiers } => {
            let (x, y) = shift(x, y);
            Input::MouseMove { x, y, modifiers }
        }
        Input::MouseButton {
            x,
            y,
            button,
            pressed,
            clicks,
            modifiers,
        } => {
            let (x, y) = shift(x, y);
            Input::MouseButton {
                x,
                y,
                button,
                pressed,
                clicks,
                modifiers,
            }
        }
        Input::Wheel {
            x,
            y,
            delta_x,
            delta_y,
            modifiers,
        } => {
            let (x, y) = shift(x, y);
            Input::Wheel {
                x,
                y,
                delta_x,
                delta_y,
                modifiers,
            }
        }
        other => other,
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn detach(browser: Browser, session: &str) -> Effect {
    Effect::SendRaw {
        browser,
        message: serde_json::json!({
            "id": 0,
            "method": "Target.detachFromTarget",
            "params": { "sessionId": session },
        })
        .to_string()
        .into_bytes(),
    }
}

pub fn run_effects(effects: Vec<Effect>) {
    for effect in effects {
        run_effect(effect);
    }
}

fn run_effect(effect: Effect) {
    match effect {
        Effect::Reply(id, result) => {
            with_host(|host| {
                host.send(HostMessage::Reply {
                    id,
                    outcome: Outcome::from(result),
                });
            });
        }
        Effect::Emit(event) => {
            with_host(|host| host.emit(event));
        }
        Effect::CreateBrowser { tab, mut context } => {
            let window_info = WindowInfo {
                windowless_rendering_enabled: 1,
                ..Default::default()
            };
            let settings = BrowserSettings {
                windowless_frame_rate: 30,
                background_color: OPAQUE_WHITE,
                ..Default::default()
            };
            let mut client = handlers::tab_client(tab);
            let created = browser_host_create_browser(
                Some(&window_info),
                Some(&mut client),
                Some(&PLACEHOLDER_URL.into()),
                Some(&settings),
                None,
                Some(&mut context),
            );
            if created != 1 {
                dispatch(|host| host.creation_failed(tab));
            }
        }
        Effect::Load(browser, url, kind) => {
            if let Some(frame) = browser.main_frame() {
                match kind {
                    LoadKind::Push => frame.load_url(Some(&url.as_str().into())),
                    LoadKind::ReplaceBlank => {
                        let script = format!("location.replace({})", serde_json::Value::from(url));
                        frame.execute_java_script(
                            Some(&script.as_str().into()),
                            Some(&PLACEHOLDER_URL.into()),
                            0,
                        );
                    }
                }
            }
        }
        Effect::Reload(browser) => browser.reload(),
        Effect::Close(browser) => {
            if let Some(host) = browser.host() {
                host.close_browser(1);
            }
        }
        Effect::SendDevTools {
            tab,
            call,
            browser,
            message,
        } => {
            let sent = browser
                .host()
                .is_some_and(|host| host.send_dev_tools_message(Some(&message)) == 1);
            if !sent {
                dispatch(|host| host.devtools_not_sent(tab, call));
            }
        }
        Effect::Flush(context) => {
            if let Some(manager) = context.cookie_manager(None) {
                manager.flush_store(None);
            }
        }
        Effect::Resize(browser) => {
            if let Some(host) = browser.host() {
                host.notify_screen_info_changed();
                host.was_resized();
                host.invalidate(PaintElementType::VIEW);
            }
        }
        Effect::Input(browser, input) => {
            if let Some(host) = browser.host() {
                crate::input::deliver(&host, input);
            }
        }
        Effect::History(browser, HistoryStep::Back) => browser.go_back(),
        Effect::History(browser, HistoryStep::Forward) => browser.go_forward(),
        Effect::Stop(browser) => browser.stop_load(),
        Effect::Zoom(browser, level) => {
            if let Some(host) = browser.host() {
                host.set_zoom_level(level);
            }
        }
        Effect::SendRaw { browser, message } => {
            if let Some(host) = browser.host() {
                host.send_dev_tools_message(Some(&message));
            }
        }
        Effect::RunScript { browser, script } => {
            if let Some(frame) = browser.main_frame() {
                frame.execute_java_script(Some(&script.as_str().into()), None, 0);
            }
        }
        Effect::ReportSecurity(id, browser) => {
            let security = crate::security::of(&browser);
            with_host(|host| {
                host.send(HostMessage::Reply {
                    id,
                    outcome: Outcome::Security { security },
                });
            });
        }
        Effect::AnswerDialog {
            callback,
            accept,
            text,
        } => {
            let text = text.map(|text| CefString::from(text.as_str()));
            callback.cont(i32::from(accept), text.as_ref());
        }
        Effect::AnswerFiles { callback, paths } => match paths {
            Some(paths) if !paths.is_empty() => {
                let mut list = CefStringList::new();
                for path in &paths {
                    list.append(path);
                }
                callback.cont(Some(&mut list));
            }
            _ => callback.cancel(),
        },
        Effect::Present(out) => {
            with_host(|host| host.send_out(out));
        }
        Effect::Quit => quit_message_loop(),
    }
}

#[cfg(unix)]
fn restrict_to_owner(directory: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_to_owner(_directory: &std::path::Path) {}

pub fn origin_or_url(raw: &str) -> String {
    navigation::origin_of(raw).unwrap_or_else(|| raw.to_string())
}

pub fn zoom_level(percent: u16) -> f64 {
    (f64::from(percent.clamp(25, 500)) / 100.).ln() / 1.2_f64.ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zoom_percent_becomes_chromiums_logarithmic_zoom_level() {
        assert!(zoom_level(100).abs() < 1e-9);
        assert!((zoom_level(120) - 1.).abs() < 1e-9);
        assert!((zoom_level(144) - 2.).abs() < 1e-9);
        assert!(zoom_level(50) < 0.);
    }

    fn started(url: &str) -> Event {
        Event::LoadStarted {
            tab: TabId(1),
            url: url.to_string(),
        }
    }

    fn loaded(url: &str) -> Event {
        Event::Loaded {
            tab: TabId(1),
            url: url.to_string(),
            http_status: 200,
        }
    }

    #[test]
    fn only_a_committed_blank_page_with_nothing_before_it_is_replaced() {
        let opening = FirstPage::Opening;
        assert_eq!(opening.after(&started(PLACEHOLDER_URL)), FirstPage::Opening);
        let on_blank = opening.after(&loaded(PLACEHOLDER_URL));
        assert_eq!(on_blank, FirstPage::OnBlank);
        assert_eq!(
            on_blank.after(&started(PLACEHOLDER_URL)),
            FirstPage::OnBlank
        );
        let left = on_blank.after(&started("http://127.0.0.1:3000/"));
        assert_eq!(left, FirstPage::Left);
        assert_eq!(left.after(&loaded(PLACEHOLDER_URL)), FirstPage::Left);
        assert_eq!(
            opening.after(&started("http://127.0.0.1:3000/")),
            FirstPage::Left
        );
    }
}
