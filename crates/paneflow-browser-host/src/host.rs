use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

use cef::*;
use paneflow_browser_proto::limits::DEVTOOLS_METHOD_TIMEOUT;
use paneflow_browser_proto::navigation::{self, Frame as FrameKind, Verdict};
use paneflow_browser_proto::{
    Command, CommandError, ConsoleLevel, Event, ExitReason, HostMessage, NavigationRefusal,
    OriginDecision, Outcome, Permission, ProfileName, Reply, RequestId, TabId,
};

use crate::handlers;
use crate::ui::{dispatch, on_ui_after, with_host};

const QUIT_GRACE: Duration = Duration::from_secs(5);

pub enum Effect {
    Reply(RequestId, Result<Reply, CommandError>),
    Emit(Event),
    CreateBrowser {
        tab: TabId,
        context: RequestContext,
    },
    Load(Browser, String),
    Reload(Browser),
    Close(Browser),
    SendDevTools {
        tab: TabId,
        call: i32,
        browser: Browser,
        message: Vec<u8>,
    },
    Flush(RequestContext),
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
}

struct Opening {
    request: RequestId,
    profile: ProfileName,
    url: String,
}

struct Profile {
    context: RequestContext,
    allowed: HashSet<String>,
}

pub struct Host {
    out: Sender<HostMessage>,
    root: PathBuf,
    idle_exit: Duration,
    tabs: HashMap<TabId, Tab>,
    opening: HashMap<TabId, Opening>,
    profiles: HashMap<ProfileName, Profile>,
    idle_generation: u64,
    quitting: bool,
}

impl Host {
    pub fn new(out: Sender<HostMessage>, root: PathBuf, idle_exit: Duration) -> Self {
        Self {
            out,
            root,
            idle_exit,
            tabs: HashMap::new(),
            opening: HashMap::new(),
            profiles: HashMap::new(),
            idle_generation: 0,
            quitting: false,
        }
    }

    pub fn send(&self, message: HostMessage) {
        let _ = self.out.send(message);
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
            Command::OpenTab { tab, profile, url } => self.open_tab(request, tab, profile, url),
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
            Command::Shutdown => {
                let mut effects = vec![Effect::Reply(request, Ok(Reply::Done))];
                effects.extend(self.begin_quit(ExitReason::Shutdown));
                effects
            }
        }
    }

    fn open_tab(
        &mut self,
        request: RequestId,
        tab: TabId,
        profile: ProfileName,
        url: String,
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
        let context = match self.profile_context(&profile) {
            Ok(context) => context,
            Err(error) => return vec![Effect::Reply(request, Err(error))],
        };
        self.idle_generation += 1;
        self.opening.insert(
            tab,
            Opening {
                request,
                profile,
                url,
            },
        );
        vec![Effect::CreateBrowser { tab, context }]
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
        self.insert_tab(opening.request, tab, opening.profile, browser, opening.url)
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
        let context = request_context_create_context(Some(&settings), None).ok_or_else(|| {
            CommandError::ProfileUnavailable {
                reason: format!("the engine refused the profile {}", directory.display()),
            }
        })?;
        self.profiles.insert(
            profile.clone(),
            Profile {
                context: context.clone(),
                allowed: HashSet::new(),
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
            },
        );
        let mut effects = self.start_navigation(tab, url);
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
            Verdict::Allow => vec![Effect::Load(entry.browser.clone(), url)],
            Verdict::External { origin } if allowed.is_some_and(|set| set.contains(&origin)) => {
                vec![Effect::Load(entry.browser.clone(), url)]
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
        let browser = entry.browser.clone();
        let profile = entry.profile.clone();
        let mut effects = vec![Effect::Reply(request, Ok(Reply::Done))];
        match decision {
            OriginDecision::AllowOnce => {
                entry.allow_once = Some(held.origin);
                effects.push(Effect::Load(browser, held.url));
            }
            OriginDecision::AllowAlways => {
                if let Some(profile) = self.profiles.get_mut(&profile) {
                    profile.allowed.insert(held.origin);
                }
                effects.push(Effect::Load(browser, held.url));
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
        let Some(entry) = self.tabs.get_mut(&tab) else {
            return (true, Vec::new());
        };
        let allowed = self
            .profiles
            .get(&entry.profile)
            .map(|profile| &profile.allowed);
        match navigation::classify(&url, frame) {
            Verdict::Allow => (false, Vec::new()),
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
        let mut effects: Vec<Effect> = entry
            .calls
            .drain()
            .map(|(_, pending)| {
                Effect::Reply(pending.request, Err(CommandError::TabClosed { tab }))
            })
            .collect();
        effects.push(Effect::Emit(Event::Closed { tab }));
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
                ..Default::default()
            };
            let mut client = handlers::tab_client(tab);
            let created = browser_host_create_browser(
                Some(&window_info),
                Some(&mut client),
                Some(&"about:blank".into()),
                Some(&settings),
                None,
                Some(&mut context),
            );
            if created != 1 {
                dispatch(|host| host.creation_failed(tab));
            }
        }
        Effect::Load(browser, url) => {
            if let Some(frame) = browser.main_frame() {
                frame.load_url(Some(&url.as_str().into()));
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
