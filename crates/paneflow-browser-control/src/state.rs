use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::Instant;

use paneflow_api::methods::browser::{
    ConsoleEntry, Dialog, DialogKind, NetworkEntry, TabOwner, TabState,
};
use paneflow_browser_proto::limits::MAX_LOG_ENTRIES_PER_TAB;
use paneflow_browser_proto::{Event, NavigationRefusal, Permission, ProfileName};
use serde_json::Value;

pub const REDACTED_HEADERS: [&str; 4] = [
    "authorization",
    "cookie",
    "proxy-authorization",
    "set-cookie",
];

pub const ABORTED: i32 = -3;

pub struct Log<T> {
    entries: VecDeque<T>,
    next: u64,
}

impl<T: Clone> Log<T> {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            next: 1,
        }
    }

    pub fn push(&mut self, build: impl FnOnce(u64) -> T) -> u64 {
        let seq = self.next;
        self.next += 1;
        self.entries.push_back(build(seq));
        while self.entries.len() > MAX_LOG_ENTRIES_PER_TAB {
            self.entries.pop_front();
        }
        seq
    }

    pub fn last_seq(&self) -> u64 {
        self.next - 1
    }

    fn first_kept(&self) -> u64 {
        self.next - self.entries.len() as u64
    }

    pub fn after(&self, cursor: u64, limit: usize, seq: impl Fn(&T) -> u64) -> Page<T> {
        let dropped = self.first_kept().saturating_sub(cursor.saturating_add(1));
        let entries: Vec<T> = self
            .entries
            .iter()
            .filter(|entry| seq(entry) > cursor)
            .take(limit)
            .cloned()
            .collect();
        let cursor = entries
            .last()
            .map_or(cursor.max(self.first_kept().saturating_sub(1)), seq);
        Page {
            entries,
            cursor,
            dropped,
        }
    }

    pub fn find_mut(&mut self, seq: u64, entry_seq: impl Fn(&T) -> u64) -> Option<&mut T> {
        self.entries
            .iter_mut()
            .rev()
            .find(|entry| entry_seq(entry) == seq)
    }
}

pub struct Page<T> {
    pub entries: Vec<T>,
    pub cursor: u64,
    pub dropped: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Marks {
    pub loads_started: u64,
    pub loads_finished: u64,
    pub load_failures: u64,
    pub refusals: u64,
    pub navigations_requested: u64,
    pub same_document: u64,
    pub crashes: u64,
    pub dialogs: u64,
}

pub struct TabEntry {
    pub workspace: ProfileName,
    pub owner: TabOwner,
    pub url: String,
    pub title: Option<String>,
    pub state: TabState,
    pub http_status: Option<i32>,
    pub generation: u64,
    pub marks: Marks,
    pub refs: HashMap<String, i64>,
    pub refs_generation: u64,
    pub console: Log<ConsoleEntry>,
    pub network: Log<NetworkEntry>,
    pub requests: HashMap<String, u64>,
    pub inflight: HashSet<String>,
    pub last_network_activity: Instant,
    pub console_seen: u64,
    pub network_seen: u64,
    pub dialogs: Vec<(u64, Dialog)>,
    pub dialogs_seen: u64,
    pub pending: Option<Pending>,
    pub decisions: u64,
    pub asking: bool,
    pub last_refusal: Option<NavigationRefusal>,
    pub last_failure: Option<String>,
    pub crash: Option<String>,
    pub domains_enabled: bool,
    pub main_frame: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub origin: String,
    pub url: String,
    pub seq: u64,
    pub since: Instant,
}

impl TabEntry {
    pub fn new(workspace: ProfileName, owner: TabOwner, url: String) -> Self {
        Self {
            workspace,
            owner,
            url,
            title: None,
            state: TabState::Starting,
            http_status: None,
            generation: 0,
            marks: Marks::default(),
            refs: HashMap::new(),
            refs_generation: 0,
            console: Log::new(),
            network: Log::new(),
            requests: HashMap::new(),
            inflight: HashSet::new(),
            last_network_activity: Instant::now(),
            console_seen: 0,
            network_seen: 0,
            dialogs: Vec::new(),
            dialogs_seen: 0,
            pending: None,
            decisions: 0,
            asking: false,
            last_refusal: None,
            last_failure: None,
            crash: None,
            domains_enabled: false,
            main_frame: None,
        }
    }

    pub fn loading(&self) -> bool {
        matches!(self.state, TabState::Starting | TabState::Loading)
            || self.marks.loads_started > self.marks.loads_finished
    }

    pub fn dialogs_after(&self, mark: u64) -> Vec<Dialog> {
        self.dialogs
            .iter()
            .filter(|(seq, _)| *seq > mark)
            .map(|(_, dialog)| dialog.clone())
            .collect()
    }

    pub fn note(&mut self, level: &str, text: String) {
        self.console.push(|seq| ConsoleEntry {
            seq,
            level: level.to_string(),
            text,
            source: Some("paneflow".to_string()),
            line: None,
            column: None,
        });
    }

    pub fn apply(&mut self, event: &Event) -> Option<Decide> {
        match event {
            Event::LoadStarted { url, .. } => {
                self.state = TabState::Loading;
                self.pending = None;
                self.marks.loads_started += 1;
                self.generation += 1;
                self.set_url(url);
                self.title = None;
                self.http_status = None;
            }
            Event::Loaded {
                url, http_status, ..
            } => {
                self.marks.loads_finished =
                    self.marks.loads_started.max(self.marks.loads_finished + 1);
                self.state = TabState::Loaded;
                self.set_url(url);
                self.http_status = (*http_status != 0).then_some(*http_status);
            }
            Event::LoadFailed {
                url,
                error_code,
                error_text,
                ..
            } if *error_code != ABORTED => {
                self.marks.load_failures += 1;
                self.marks.loads_finished = self.marks.loads_started;
                self.state = TabState::Failed;
                self.last_failure =
                    Some(format!("{url} failed to load: {error_text} ({error_code})"));
            }
            Event::LoadFailed { .. } => {
                self.marks.loads_finished = self.marks.loads_started;
                if self.state == TabState::Loading {
                    self.state = TabState::Loaded;
                }
            }
            Event::OriginDecisionRequested { origin, url, .. } => {
                self.state = TabState::AwaitingDecision;
                self.decisions += 1;
                self.pending = Some(Pending {
                    origin: origin.clone(),
                    url: url.clone(),
                    seq: self.decisions,
                    since: Instant::now(),
                });
                return Some(Decide {
                    origin: origin.clone(),
                    url: url.clone(),
                    seq: self.decisions,
                });
            }
            Event::NavigationRefused { refusal, url, .. } => {
                self.marks.refusals += 1;
                self.pending = None;
                self.last_refusal = Some(refusal.clone());
                self.state = if self.marks.loads_finished > 0 {
                    TabState::Loaded
                } else {
                    TabState::Refused
                };
                self.note(
                    "warning",
                    format!("Paneflow refused a navigation to {url}: {refusal}"),
                );
            }
            Event::PermissionDenied {
                origin,
                permissions,
                ..
            } => {
                let names: Vec<&str> = permissions.iter().map(permission_name).collect();
                self.note(
                    "warning",
                    format!("Paneflow denied {} to {origin}", names.join(", ")),
                );
            }
            Event::DownloadCancelled { url, .. } => {
                self.note("warning", format!("Paneflow cancelled a download of {url}"));
            }
            Event::Crashed { reason, .. } => {
                self.marks.crashes += 1;
                self.state = TabState::Crashed;
                self.crash = Some(reason.clone());
                self.inflight.clear();
                self.pending = None;
                self.domains_enabled = false;
            }
            Event::DevTools { method, params, .. } => self.apply_devtools(method, params),
            Event::Console { .. } | Event::Closed { .. } | Event::Exiting { .. } => {}
        }
        None
    }

    fn set_url(&mut self, url: &str) {
        if !url.is_empty() && (url != "about:blank" || self.url.is_empty()) {
            self.url = url.to_string();
        }
    }

    fn apply_devtools(&mut self, method: &str, params: &Value) {
        match method {
            "Runtime.consoleAPICalled" => self.console_api(params),
            "Runtime.exceptionThrown" => self.exception(params),
            "Log.entryAdded" => self.log_entry(params),
            "Network.requestWillBeSent" => self.request_sent(params),
            "Network.responseReceived" => self.response(params),
            "Network.loadingFailed" => self.request_failed(params),
            "Network.dataReceived" => self.last_network_activity = Instant::now(),
            "Network.loadingFinished" => {
                if let Some(id) = params["requestId"].as_str() {
                    self.finish_request(id);
                }
            }
            "Page.javascriptDialogOpening" => self.dialog(params),
            "Page.frameRequestedNavigation" | "Page.frameStartedNavigating"
                if self.is_main_frame(params) =>
            {
                self.marks.navigations_requested += 1;
            }
            "Page.navigatedWithinDocument" if self.is_main_frame(params) => {
                self.marks.same_document += 1;
                if let Some(url) = params["url"].as_str() {
                    self.set_url(url);
                }
            }
            "Page.frameNavigated" if params["frame"].get("parentId").is_none() => {
                if let Some(id) = params["frame"]["id"].as_str() {
                    self.main_frame = Some(id.to_string());
                }
                if let Some(url) = params["frame"]["url"].as_str() {
                    self.set_url(url);
                }
            }
            _ => {}
        }
    }

    fn is_main_frame(&self, params: &Value) -> bool {
        match (&self.main_frame, params["frameId"].as_str()) {
            (Some(main), Some(frame)) => main == frame,
            _ => true,
        }
    }

    fn console_api(&mut self, params: &Value) {
        let level = match params["type"].as_str().unwrap_or("log") {
            "error" | "assert" => "error",
            "warning" => "warning",
            "debug" | "trace" => "debug",
            _ => "info",
        };
        let text = params["args"]
            .as_array()
            .map(|args| args.iter().map(remote_text).collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        let frame = &params["stackTrace"]["callFrames"][0];
        self.push_console(
            level,
            text,
            frame["url"].as_str(),
            frame["lineNumber"].as_u64(),
            frame["columnNumber"].as_u64(),
        );
    }

    fn exception(&mut self, params: &Value) {
        let details = &params["exceptionDetails"];
        let text = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("Uncaught exception")
            .to_string();
        self.push_console(
            "error",
            text,
            details["url"].as_str(),
            details["lineNumber"].as_u64(),
            details["columnNumber"].as_u64(),
        );
    }

    fn log_entry(&mut self, params: &Value) {
        let entry = &params["entry"];
        let level = match entry["level"].as_str().unwrap_or("info") {
            "error" => "error",
            "warning" => "warning",
            "verbose" => "debug",
            _ => "info",
        };
        let text = entry["text"].as_str().unwrap_or_default().to_string();
        self.push_console(
            level,
            text,
            entry["url"].as_str(),
            entry["lineNumber"].as_u64(),
            None,
        );
    }

    fn push_console(
        &mut self,
        level: &str,
        text: String,
        source: Option<&str>,
        line: Option<u64>,
        column: Option<u64>,
    ) {
        let one_based = |value: Option<u64>| value.and_then(|value| u32::try_from(value + 1).ok());
        self.console.push(|seq| ConsoleEntry {
            seq,
            level: level.to_string(),
            text,
            source: source
                .filter(|source| !source.is_empty())
                .map(str::to_string),
            line: one_based(line),
            column: one_based(column),
        });
    }

    fn request_sent(&mut self, params: &Value) {
        let Some(id) = params["requestId"].as_str() else {
            return;
        };
        if let Some(redirect) = params.get("redirectResponse")
            && let Some(seq) = self.requests.get(id).copied()
            && let Some(entry) = self.network.find_mut(seq, |entry| entry.seq)
        {
            entry.status = redirect["status"]
                .as_u64()
                .and_then(|status| u16::try_from(status).ok());
        }
        let request = &params["request"];
        let headers = redacted_headers(&request["headers"]);
        let seq = self.network.push(|seq| NetworkEntry {
            seq,
            method: request["method"].as_str().unwrap_or("GET").to_string(),
            url: request["url"].as_str().unwrap_or_default().to_string(),
            status: None,
            resource_type: params["type"].as_str().map(str::to_string),
            failure: None,
            request_headers: headers,
        });
        self.requests.insert(id.to_string(), seq);
        self.inflight.insert(id.to_string());
        self.last_network_activity = Instant::now();
        self.trim_requests();
    }

    fn trim_requests(&mut self) {
        if self.requests.len() > MAX_LOG_ENTRIES_PER_TAB * 2 {
            let oldest = self
                .network
                .last_seq()
                .saturating_sub(MAX_LOG_ENTRIES_PER_TAB as u64);
            self.requests.retain(|_, seq| *seq > oldest);
        }
    }

    fn response(&mut self, params: &Value) {
        let Some(seq) = params["requestId"]
            .as_str()
            .and_then(|id| self.requests.get(id))
            .copied()
        else {
            return;
        };
        if let Some(entry) = self.network.find_mut(seq, |entry| entry.seq) {
            entry.status = params["response"]["status"]
                .as_u64()
                .and_then(|status| u16::try_from(status).ok());
            if let Some(kind) = params["type"].as_str() {
                entry.resource_type = Some(kind.to_string());
            }
        }
        if let Some(id) = params["requestId"].as_str() {
            self.inflight.remove(id);
        }
        self.last_network_activity = Instant::now();
    }

    fn request_failed(&mut self, params: &Value) {
        let Some(id) = params["requestId"].as_str() else {
            return;
        };
        if let Some(seq) = self.requests.get(id).copied()
            && let Some(entry) = self.network.find_mut(seq, |entry| entry.seq)
        {
            let mut failure = params["errorText"].as_str().unwrap_or("failed").to_string();
            if let Some(reason) = params["blockedReason"].as_str() {
                failure.push_str(&format!(" (blocked: {reason})"));
            }
            if params["canceled"].as_bool() == Some(true) {
                failure.push_str(" (cancelled)");
            }
            entry.failure = Some(failure);
        }
        self.finish_request(id);
    }

    fn finish_request(&mut self, id: &str) {
        self.inflight.remove(id);
        self.last_network_activity = Instant::now();
    }

    fn dialog(&mut self, params: &Value) {
        let kind = match params["type"].as_str().unwrap_or("alert") {
            "confirm" => DialogKind::Confirm,
            "prompt" => DialogKind::Prompt,
            "beforeunload" => DialogKind::Beforeunload,
            _ => DialogKind::Alert,
        };
        self.marks.dialogs += 1;
        let seq = self.marks.dialogs;
        self.dialogs.push((
            seq,
            Dialog {
                kind,
                message: params["message"].as_str().unwrap_or_default().to_string(),
                dismissed: true,
            },
        ));
        if self.dialogs.len() > MAX_LOG_ENTRIES_PER_TAB {
            self.dialogs.remove(0);
        }
    }
}

pub struct Decide {
    pub origin: String,
    pub url: String,
    pub seq: u64,
}

fn permission_name(permission: &Permission) -> &'static str {
    match permission {
        Permission::Camera => "the camera",
        Permission::Microphone => "the microphone",
        Permission::Geolocation => "the location",
        Permission::Notifications => "notifications",
        Permission::Other => "a permission",
    }
}

fn remote_text(object: &Value) -> String {
    if let Some(text) = object["value"].as_str() {
        return text.to_string();
    }
    if let Some(value) = object.get("value").filter(|value| !value.is_null()) {
        return value.to_string();
    }
    if let Some(text) = object["unserializableValue"].as_str() {
        return text.to_string();
    }
    if let Some(text) = object["description"].as_str() {
        return text.to_string();
    }
    match object["type"].as_str() {
        Some("undefined") => "undefined".to_string(),
        Some(kind) => kind.to_string(),
        None => String::new(),
    }
}

pub fn redacted_headers(headers: &Value) -> BTreeMap<String, String> {
    headers
        .as_object()
        .map(|headers| {
            headers
                .iter()
                .filter(|(name, _)| !REDACTED_HEADERS.contains(&name.to_ascii_lowercase().as_str()))
                .map(|(name, value)| {
                    (
                        name.clone(),
                        value
                            .as_str()
                            .map_or_else(|| value.to_string(), str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_browser_proto::TabId;
    use serde_json::json;

    fn entry() -> TabEntry {
        TabEntry::new(
            ProfileName::new("w-1").unwrap(),
            TabOwner::Human,
            "about:blank".to_string(),
        )
    }

    fn devtools(method: &str, params: Value) -> Event {
        Event::DevTools {
            tab: TabId(1),
            method: method.to_string(),
            params,
        }
    }

    #[test]
    fn authorization_and_cookie_values_never_reach_a_network_entry() {
        let mut tab = entry();
        tab.apply(&devtools(
            "Network.requestWillBeSent",
            json!({"requestId": "1", "type": "Fetch", "request": {"method": "POST", "url": "http://127.0.0.1/api", "headers": {
                "Authorization": "Bearer secret-token", "Cookie": "sid=secret-cookie", "proxy-authorization": "Basic x", "Accept": "application/json"
            }}}),
        ));
        let page = tab.network.after(0, 10, |entry| entry.seq);
        let rendered = serde_json::to_string(&page.entries).unwrap();
        assert!(!rendered.contains("secret-token"), "{rendered}");
        assert!(!rendered.contains("secret-cookie"), "{rendered}");
        assert!(!rendered.contains("Basic x"), "{rendered}");
        assert_eq!(
            page.entries[0].request_headers["Accept"],
            "application/json"
        );
    }

    #[test]
    fn a_request_reports_its_status_type_and_failure_and_no_body() {
        let mut tab = entry();
        tab.apply(&devtools(
            "Network.requestWillBeSent",
            json!({"requestId": "7", "type": "Fetch", "request": {"method": "GET", "url": "http://127.0.0.1/api/prices", "headers": {}}}),
        ));
        tab.apply(&devtools(
            "Network.responseReceived",
            json!({"requestId": "7", "type": "Fetch", "response": {"status": 500, "url": "http://127.0.0.1/api/prices"}}),
        ));
        tab.apply(&devtools(
            "Network.requestWillBeSent",
            json!({"requestId": "8", "type": "Image", "request": {"method": "GET", "url": "http://127.0.0.1/x.png", "headers": {}}}),
        ));
        tab.apply(&devtools(
            "Network.loadingFailed",
            json!({"requestId": "8", "errorText": "net::ERR_CONNECTION_REFUSED"}),
        ));
        let page = tab.network.after(0, 10, |entry| entry.seq);
        assert_eq!(page.entries[0].status, Some(500));
        assert_eq!(page.entries[0].resource_type.as_deref(), Some("Fetch"));
        assert_eq!(
            page.entries[1].failure.as_deref(),
            Some("net::ERR_CONNECTION_REFUSED")
        );
        tab.apply(&devtools(
            "Network.requestWillBeSent",
            json!({"requestId": "9", "type": "Script", "request": {"method": "GET", "url": "http://127.0.0.1/app.js", "headers": {}}}),
        ));
        assert!(
            !tab.inflight.contains("7"),
            "an answered request no longer holds the network busy"
        );
        assert!(!tab.inflight.contains("8"));
        assert!(tab.inflight.contains("9"));
    }

    #[test]
    fn a_cursor_past_evicted_entries_states_how_many_were_dropped() {
        let mut tab = entry();
        let total = MAX_LOG_ENTRIES_PER_TAB as u64 + 25;
        for index in 0..total {
            tab.note("info", format!("line {index}"));
        }
        let page = tab.console.after(0, 2000, |entry| entry.seq);
        assert_eq!(page.dropped, 25);
        assert_eq!(page.entries.len(), MAX_LOG_ENTRIES_PER_TAB);
        assert_eq!(page.entries[0].seq, 26);
        assert_eq!(page.cursor, total);
        let next = tab.console.after(page.cursor, 10, |entry| entry.seq);
        assert!(next.entries.is_empty());
        assert_eq!(next.dropped, 0);
        assert_eq!(next.cursor, total);
    }

    #[test]
    fn console_sources_are_one_based_like_an_editor() {
        let mut tab = entry();
        tab.apply(&devtools(
            "Runtime.exceptionThrown",
            json!({"exceptionDetails": {"text": "Uncaught", "url": "http://127.0.0.1/", "lineNumber": 13, "columnNumber": 22,
                   "exception": {"description": "TypeError: x is null\n    at http://127.0.0.1/:14:23"}}}),
        ));
        let page = tab.console.after(0, 10, |entry| entry.seq);
        assert_eq!(page.entries[0].level, "error");
        assert_eq!(page.entries[0].line, Some(14));
        assert_eq!(page.entries[0].column, Some(23));
        assert!(page.entries[0].text.starts_with("TypeError"));
    }

    #[test]
    fn a_load_start_moves_to_a_new_generation() {
        let mut tab = entry();
        tab.apply(&Event::LoadStarted {
            tab: TabId(1),
            url: "http://127.0.0.1/".into(),
        });
        let first = tab.generation;
        tab.apply(&Event::Loaded {
            tab: TabId(1),
            url: "http://127.0.0.1/".into(),
            http_status: 200,
        });
        tab.apply(&Event::LoadStarted {
            tab: TabId(1),
            url: "http://127.0.0.1/form.html".into(),
        });
        assert_eq!(tab.generation, first + 1);
        assert!(tab.loading());
    }
}
