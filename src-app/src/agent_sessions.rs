#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionAgent {
    Claude,
    Codex,
    OpenCode,
    Pi,
    Hermes,
    Grok,
    Cursor,
    Gemini,
    Kiro,
}

impl SessionAgent {
    pub const ALL: [SessionAgent; 9] = [
        SessionAgent::Claude,
        SessionAgent::Codex,
        SessionAgent::OpenCode,
        SessionAgent::Pi,
        SessionAgent::Hermes,
        SessionAgent::Grok,
        SessionAgent::Cursor,
        SessionAgent::Gemini,
        SessionAgent::Kiro,
    ];

    pub(crate) fn index(self) -> usize {
        match self {
            SessionAgent::Claude => 0,
            SessionAgent::Codex => 1,
            SessionAgent::OpenCode => 2,
            SessionAgent::Pi => 3,
            SessionAgent::Hermes => 4,
            SessionAgent::Grok => 5,
            SessionAgent::Cursor => 6,
            SessionAgent::Gemini => 7,
            SessionAgent::Kiro => 8,
        }
    }

    pub(crate) fn terminal_agent(self) -> crate::agent_launcher::TerminalAgent {
        use crate::agent_launcher::TerminalAgent;
        match self {
            SessionAgent::Claude => TerminalAgent::ClaudeCode,
            SessionAgent::Codex => TerminalAgent::Codex,
            SessionAgent::OpenCode => TerminalAgent::OpenCode,
            SessionAgent::Pi => TerminalAgent::Pi,
            SessionAgent::Hermes => TerminalAgent::Hermes,
            SessionAgent::Grok => TerminalAgent::Grok,
            SessionAgent::Cursor => TerminalAgent::Cursor,
            SessionAgent::Gemini => TerminalAgent::Gemini,
            SessionAgent::Kiro => TerminalAgent::Kiro,
        }
    }

    pub(crate) fn icon_path(self) -> &'static str {
        self.terminal_agent().icon_path()
    }

    pub(crate) fn label(self) -> &'static str {
        self.terminal_agent().display_name()
    }
}

pub(crate) const SESSION_AGENT_COUNT: usize = SessionAgent::ALL.len();
pub(crate) const MAX_SESSION_ID_CHARS: usize = 128;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssistantUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

impl AssistantUsage {
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_creation)
    }

    pub fn add(&mut self, other: &AssistantUsage) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_creation = self.cache_creation.saturating_add(other.cache_creation);
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

pub mod cache {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, SystemTime};

    use super::{SessionAgent, SessionMeta};

    pub const MAX_CACHE_ENTRIES: usize = 10;

    fn next_access_seq() -> u64 {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        SEQ.fetch_add(1, Ordering::Relaxed)
    }

    const MTIME_FUZZ: Duration = Duration::from_millis(1);

    struct Entry {
        mtime: SystemTime,
        sessions: Vec<SessionMeta>,
        omitted: usize,
        access_seq: u64,
    }

    fn store() -> &'static Mutex<HashMap<(SessionAgent, String), Entry>> {
        static CACHE: OnceLock<Mutex<HashMap<(SessionAgent, String), Entry>>> = OnceLock::new();
        CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    #[allow(dead_code)]
    fn dir_mtime(dir: &Path) -> Option<SystemTime> {
        std::fs::metadata(dir).ok().and_then(|m| m.modified().ok())
    }

    fn within_fuzz(cached: SystemTime, observed: SystemTime) -> bool {
        match observed.duration_since(cached) {
            Ok(delta) => delta < MTIME_FUZZ,
            Err(_) => false,
        }
    }

    #[allow(dead_code)]
    pub fn lookup(
        agent: SessionAgent,
        cwd: &str,
        project_dir: &Path,
    ) -> Option<(Vec<SessionMeta>, usize)> {
        let observed = dir_mtime(project_dir)?;
        lookup_with_mtime(agent, cwd, observed)
    }

    pub fn lookup_with_mtime(
        agent: SessionAgent,
        cwd: &str,
        observed: SystemTime,
    ) -> Option<(Vec<SessionMeta>, usize)> {
        let mut guard = match store().lock() {
            Ok(g) => g,
            Err(p) => {
                tracing::warn!(
                    target: "paneflow_app::agent_sessions",
                    "session cache mutex poisoned on lookup; using potentially stale data \
                     (a previous thread panicked while holding the lock)"
                );
                p.into_inner()
            }
        };
        let entry = guard.get_mut(&(agent, cwd.to_string()))?;
        if within_fuzz(entry.mtime, observed) {
            entry.access_seq = next_access_seq();
            Some((entry.sessions.clone(), entry.omitted))
        } else {
            None
        }
    }

    #[allow(dead_code)]
    pub fn store_result(
        agent: SessionAgent,
        cwd: &str,
        project_dir: &Path,
        sessions: &[SessionMeta],
        omitted: usize,
    ) {
        let Some(mtime) = dir_mtime(project_dir) else {
            return;
        };
        store_result_with_mtime(agent, cwd, mtime, sessions, omitted);
    }

    pub fn store_result_with_mtime(
        agent: SessionAgent,
        cwd: &str,
        mtime: SystemTime,
        sessions: &[SessionMeta],
        omitted: usize,
    ) {
        let mut guard = match store().lock() {
            Ok(g) => g,
            Err(p) => {
                tracing::warn!(
                    target: "paneflow_app::agent_sessions",
                    "session cache mutex poisoned on store_result; overwriting entry \
                     (a previous thread panicked while holding the lock)"
                );
                p.into_inner()
            }
        };
        let key = (agent, cwd.to_string());
        if guard.len() >= MAX_CACHE_ENTRIES
            && !guard.contains_key(&key)
            && let Some((victim_key, victim_seq)) = guard
                .iter()
                .map(|(k, v)| (k.clone(), v.access_seq))
                .min_by_key(|(_, seq)| *seq)
        {
            tracing::debug!(
                target: "paneflow_app::agent_sessions",
                "session cache LRU eviction: (agent={:?}, cwd={}) seq={}",
                victim_key.0, victim_key.1, victim_seq,
            );
            guard.remove(&victim_key);
        }
        guard.insert(
            key,
            Entry {
                mtime,
                sessions: sessions.to_vec(),
                omitted,
                access_seq: next_access_seq(),
            },
        );
    }

    #[cfg(test)]
    pub fn clear() {
        let cache = store();
        match cache.lock() {
            Ok(mut g) => g.clear(),
            Err(p) => p.into_inner().clear(),
        }
        cache.clear_poison();
    }

    #[cfg(test)]
    mod tests {
        use super::{MTIME_FUZZ, within_fuzz};
        use std::time::{Duration, SystemTime};
        use tracing_test::traced_test;

        fn serial() -> std::sync::MutexGuard<'static, ()> {
            static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
            LOCK.lock().unwrap_or_else(|e| e.into_inner())
        }

        #[test]
        fn within_fuzz_accepts_subms_drift() {
            let cached = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
            let observed = cached + Duration::from_micros(500);
            assert!(
                within_fuzz(cached, observed),
                "{:?} sub-ms drift should be tolerated",
                MTIME_FUZZ,
            );
        }

        #[test]
        fn within_fuzz_rejects_real_change() {
            let cached = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
            let observed = cached + Duration::from_millis(5);
            assert!(
                !within_fuzz(cached, observed),
                "5 ms drift (well past {:?}) should invalidate",
                MTIME_FUZZ,
            );
        }

        #[test]
        fn within_fuzz_accepts_exact_match() {
            let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
            assert!(within_fuzz(t, t));
        }

        #[test]
        fn within_fuzz_rejects_backwards_drift() {
            let cached = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
            let observed = cached - Duration::from_millis(10);
            assert!(!within_fuzz(cached, observed));
        }

        #[test]
        fn session_cache_evicts_lru() {
            use super::super::SessionAgent;
            use super::Entry;
            let _serial = serial();
            super::clear();
            let dir = tempfile::tempdir().expect("tempdir");
            {
                let mut guard = super::store().lock().expect("lock");
                for i in 0..super::MAX_CACHE_ENTRIES {
                    let key = (SessionAgent::Claude, format!("/proj-{i}"));
                    guard.insert(
                        key,
                        Entry {
                            mtime: SystemTime::UNIX_EPOCH,
                            sessions: Vec::new(),
                            omitted: 0,
                            access_seq: super::next_access_seq(),
                        },
                    );
                }
                assert_eq!(guard.len(), super::MAX_CACHE_ENTRIES);
                assert!(guard.contains_key(&(SessionAgent::Claude, "/proj-0".to_string())));
            }
            super::store_result(SessionAgent::Claude, "/proj-N", dir.path(), &[], 0);
            {
                let guard = super::store().lock().expect("lock");
                assert_eq!(
                    guard.len(),
                    super::MAX_CACHE_ENTRIES,
                    "cache must stay at cap after store_result eviction"
                );
                assert!(
                    guard.contains_key(&(SessionAgent::Claude, "/proj-N".to_string())),
                    "new entry must be present"
                );
                assert!(
                    !guard.contains_key(&(SessionAgent::Claude, "/proj-0".to_string())),
                    "LRU victim (proj-0) must have been evicted"
                );
            }
            super::clear();
        }

        #[test]
        #[traced_test]
        fn poisoned_session_cache_logs_warning() {
            use super::super::SessionAgent;

            let _serial = serial();
            super::clear();
            let _ = std::thread::spawn(|| {
                let _guard = super::store().lock().expect("lock cache for poison");
                panic!("force session cache poison");
            })
            .join();

            let dir = tempfile::tempdir().expect("tempdir");
            super::store_result(SessionAgent::Claude, "/poisoned", dir.path(), &[], 0);

            assert!(
                logs_contain("session cache mutex poisoned on store_result"),
                "poison recovery warning should be emitted"
            );
            super::clear();
        }

        #[test]
        fn lookup_with_mtime_invalidates_on_leaf_file_advance() {
            use super::super::{SessionAgent, SessionMeta};

            let _serial = serial();
            super::clear();
            let cached = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
            let advanced = cached + Duration::from_secs(1);
            let sessions = vec![SessionMeta {
                agent: SessionAgent::Claude,
                session_id: "s1".into(),
                timestamp: "2026-07-03T10:00:00Z".into(),
                cwd: "/repo".into(),
                git_branch: "main".into(),
                summary: Some("old".into()),
                model: None,
                usage: None,
            }];

            super::store_result_with_mtime(SessionAgent::Claude, "/repo", cached, &sessions, 0);
            assert!(
                super::lookup_with_mtime(SessionAgent::Claude, "/repo", cached).is_some(),
                "same fingerprint should hit"
            );
            assert!(
                super::lookup_with_mtime(SessionAgent::Claude, "/repo", advanced).is_none(),
                "advanced leaf-file fingerprint should invalidate"
            );
            super::clear();
        }
    }
}

pub fn enabled_session_agents_from_config(
    cfg: &paneflow_config::schema::PaneFlowConfig,
) -> Vec<SessionAgent> {
    crate::agent_launcher::TerminalAgent::visible(cfg)
        .into_iter()
        .filter_map(|agent| agent.session_agent())
        .collect()
}

pub fn enabled_session_agents() -> Vec<SessionAgent> {
    let cfg = paneflow_config::loader::load_config();
    enabled_session_agents_from_config(&cfg)
}

pub(crate) fn read_sessions_for_cwd(agent: SessionAgent, cwd: &str) -> Vec<SessionMeta> {
    match agent {
        SessionAgent::Claude => crate::claude_sessions::read_sessions_for_cwd(cwd),
        SessionAgent::Codex => crate::codex_sessions::read_sessions_for_cwd(cwd),
        SessionAgent::OpenCode => crate::opencode_sessions::read_sessions_for_cwd(cwd),
        _ => read_sessions_for_cwd_with_omitted(agent, cwd).0,
    }
}

pub(crate) fn read_sessions_for_cwd_with_omitted(
    agent: SessionAgent,
    cwd: &str,
) -> (Vec<SessionMeta>, usize) {
    match agent {
        SessionAgent::Claude => crate::claude_sessions::read_sessions_for_cwd_with_omitted(cwd),
        SessionAgent::Codex => crate::codex_sessions::read_sessions_for_cwd_with_omitted(cwd),
        SessionAgent::OpenCode => crate::opencode_sessions::read_sessions_for_cwd_with_omitted(cwd),
        SessionAgent::Pi => crate::pi_sessions::read_sessions_for_cwd_with_omitted(cwd),
        SessionAgent::Cursor => crate::command_sessions::read_cursor_sessions_for_cwd(cwd),
        SessionAgent::Gemini => crate::command_sessions::read_gemini_sessions_for_cwd(cwd),
        SessionAgent::Kiro => crate::command_sessions::read_kiro_sessions_for_cwd(cwd),
        SessionAgent::Grok => crate::command_sessions::read_grok_sessions_for_cwd(cwd),
        SessionAgent::Hermes => crate::command_sessions::read_hermes_sessions_for_cwd(cwd),
    }
}

pub(crate) const SIDEBAR_SESSION_RETAINED_PER_SOURCE: usize = 100;

pub(crate) const DIFF_ATTRIBUTION_MATCH_CAP: usize = 50;

pub(crate) fn clean_session_label(raw: &str, max_chars: usize) -> Option<String> {
    let filtered: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = filtered.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }

    let mut chars = collapsed.chars();
    let mut label: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        label.push('…');
    }
    Some(label)
}

pub(crate) fn is_valid_session_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_SESSION_ID_CHARS {
        return false;
    }
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn trim_trailing_path_separators(path: &str) -> &str {
    let mut end = path.len();
    while end > 0 {
        let current = &path[..end];
        let Some(ch) = current.chars().next_back() else {
            break;
        };
        if ch != '/' && ch != '\\' {
            break;
        }
        if end <= 1 || (end == 3 && path.as_bytes().get(1) == Some(&b':')) {
            break;
        }
        end -= ch.len_utf8();
    }
    &path[..end]
}

pub(crate) fn cwd_matches(recorded: &str, scanned: &str) -> bool {
    #[cfg(windows)]
    {
        fn normalize(path: &str) -> String {
            trim_trailing_path_separators(path)
                .replace('/', "\\")
                .to_ascii_lowercase()
        }
        normalize(recorded) == normalize(scanned)
    }
    #[cfg(not(windows))]
    {
        trim_trailing_path_separators(recorded) == trim_trailing_path_separators(scanned)
    }
}

#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub agent: SessionAgent,
    pub session_id: String,
    pub timestamp: String,
    pub cwd: String,
    pub git_branch: String,
    pub summary: Option<String>,
    pub model: Option<String>,
    pub usage: Option<AssistantUsage>,
}

pub(crate) fn collect_recent_sessions<I>(sessions: I, cap: usize) -> (Vec<SessionMeta>, usize)
where
    I: IntoIterator<Item = SessionMeta>,
{
    let mut collector = RecentSessionCollector::new(cap);
    for session in sessions {
        collector.push(session);
    }
    collector.finish()
}

pub(crate) struct RecentSessionCollector {
    retained: Vec<SessionMeta>,
    omitted: usize,
    cap: usize,
    sorted: bool,
}

impl RecentSessionCollector {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            retained: Vec::with_capacity(cap),
            omitted: 0,
            cap,
            sorted: false,
        }
    }

    pub(crate) fn push(&mut self, session: SessionMeta) {
        let cap = self.cap;
        let retained = &mut self.retained;
        let omitted = &mut self.omitted;
        if cap == 0 {
            *omitted = omitted.saturating_add(1);
            return;
        }

        if retained.len() < cap {
            retained.push(session);
            if retained.len() == cap {
                retained.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                self.sorted = true;
            }
            return;
        }

        if !self.sorted {
            retained.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            self.sorted = true;
        }

        if retained
            .last()
            .is_some_and(|oldest| session.timestamp > oldest.timestamp)
        {
            let insert_at =
                retained.partition_point(|existing| existing.timestamp >= session.timestamp);
            retained.insert(insert_at, session);
            retained.pop();
        }
        *omitted = omitted.saturating_add(1);
    }

    pub(crate) fn finish(mut self) -> (Vec<SessionMeta>, usize) {
        if !self.sorted {
            self.retained.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        }
        self.retained.shrink_to_fit();
        (self.retained, self.omitted)
    }
}

fn attribution_branch_rank(session: &SessionMeta, col_branch: &str) -> u8 {
    u8::from(!col_branch.is_empty() && session.git_branch == col_branch)
}

fn attribution_ordering(a: &SessionMeta, b: &SessionMeta, col_branch: &str) -> std::cmp::Ordering {
    attribution_branch_rank(b, col_branch)
        .cmp(&attribution_branch_rank(a, col_branch))
        .then_with(|| b.timestamp.cmp(&a.timestamp))
}

pub(crate) fn push_ranked_attribution<T>(
    retained: &mut Vec<(SessionMeta, T)>,
    session: SessionMeta,
    payload: T,
    col_branch: &str,
    cap: usize,
) {
    if cap == 0 {
        return;
    }

    let insert_at = retained.partition_point(|(existing, _)| {
        !matches!(
            attribution_ordering(existing, &session, col_branch),
            std::cmp::Ordering::Greater
        )
    });
    if insert_at >= cap {
        return;
    }

    retained.insert(insert_at, (session, payload));
    if retained.len() > cap {
        retained.pop();
    }
}

pub fn match_sessions_to_column(
    sessions: Vec<SessionMeta>,
    col_path: &str,
    col_branch: &str,
) -> Vec<SessionMeta> {
    let mut matched = Vec::new();
    for session in sessions
        .into_iter()
        .filter(|s| cwd_matches(&s.cwd, col_path))
    {
        push_ranked_attribution(
            &mut matched,
            session,
            (),
            col_branch,
            DIFF_ATTRIBUTION_MATCH_CAP,
        );
    }
    matched.into_iter().map(|(session, _)| session).collect()
}

pub fn attribution_for_column(cwd: &str, branch: &str) -> Vec<SessionMeta> {
    let mut all = Vec::new();
    for agent in enabled_session_agents() {
        match agent {
            SessionAgent::Claude => all.extend(
                crate::claude_sessions::read_sessions_with_usage_for_attribution(cwd, branch),
            ),
            SessionAgent::Codex => all.extend(
                crate::codex_sessions::read_sessions_with_usage_for_attribution(cwd, branch),
            ),
            SessionAgent::OpenCode => {
                all.extend(crate::opencode_sessions::read_sessions_for_cwd(cwd))
            }
            SessionAgent::Pi
            | SessionAgent::Hermes
            | SessionAgent::Grok
            | SessionAgent::Cursor
            | SessionAgent::Gemini
            | SessionAgent::Kiro => all.extend(read_sessions_for_cwd(agent, cwd)),
        }
        all = match_sessions_to_column(all, cwd, branch);
    }
    all
}

pub fn format_relative_time(iso8601: &str) -> String {
    let now_secs = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => return iso8601_safe_fallback(iso8601),
    };

    match parse_iso8601_to_unix_secs(iso8601) {
        Some(ts_secs) => {
            let delta = now_secs.saturating_sub(ts_secs);
            relative_label(delta)
        }
        None => iso8601_safe_fallback(iso8601),
    }
}

fn iso8601_safe_fallback(iso8601: &str) -> String {
    iso8601
        .split('T')
        .next()
        .unwrap_or(iso8601)
        .chars()
        .take(10)
        .collect()
}

fn relative_label(delta_secs: i64) -> String {
    if delta_secs < 60 {
        return "just now".to_string();
    }
    if delta_secs < 3_600 {
        return format!("{}m ago", delta_secs / 60);
    }
    if delta_secs < 86_400 {
        return format!("{}h ago", delta_secs / 3_600);
    }
    if delta_secs < 30 * 86_400 {
        return format!("{}d ago", delta_secs / 86_400);
    }
    if delta_secs < 365 * 86_400 {
        return format!("{}mo ago", delta_secs / (30 * 86_400));
    }
    format!("{}y ago", delta_secs / (365 * 86_400))
}

fn parse_iso8601_to_unix_secs(iso: &str) -> Option<i64> {
    let (date, rest) = iso.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;

    let time = rest
        .split_once(['Z', '+', '-'])
        .map(|(t, _)| t)
        .unwrap_or(rest);
    let time = time.split('.').next().unwrap_or(time);
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next().unwrap_or("0").parse().ok()?;

    let y = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = y.div_euclid(400);
    let yoe = y.checked_sub(era.checked_mul(400)?)?;
    let month_adj = if month > 2 { month - 3 } else { month + 9 };
    let doy = 153_i64
        .checked_mul(month_adj)?
        .checked_add(2)?
        .checked_div(5)?
        .checked_add(day)?
        .checked_sub(1)?;
    let doe = (yoe * 365 + yoe / 4 - yoe / 100).checked_add(doy)?;
    let days_since_epoch = era
        .checked_mul(146_097)?
        .checked_add(doe)?
        .checked_sub(719_468)?;

    let hms = hour
        .checked_mul(3_600)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)?;

    days_since_epoch.checked_mul(86_400)?.checked_add(hms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_session_agents_from_config_uses_visible_session_capable_agents() {
        let cfg = paneflow_config::schema::PaneFlowConfig {
            claude_code_button_visible: Some(true),
            codex_button_visible: Some(false),
            opencode_button_visible: Some(true),
            pi_button_visible: Some(false),
            hermes_agent_button_visible: Some(false),
            grok_button_visible: Some(false),
            cursor_button_visible: Some(false),
            gemini_button_visible: Some(false),
            kiro_button_visible: Some(false),
            amp_button_visible: Some(true),
            ..Default::default()
        };

        assert_eq!(
            enabled_session_agents_from_config(&cfg),
            vec![SessionAgent::Claude, SessionAgent::OpenCode],
            "visible agents without session readers must not create sidebar groups"
        );
    }

    #[test]
    fn relative_label_under_minute() {
        assert_eq!(relative_label(15), "just now");
    }

    #[test]
    fn relative_label_minutes() {
        assert_eq!(relative_label(125), "2m ago");
    }

    #[test]
    fn relative_label_hours() {
        assert_eq!(relative_label(7_400), "2h ago");
    }

    #[test]
    fn relative_label_days() {
        assert_eq!(relative_label(3 * 86_400 + 100), "3d ago");
    }

    #[test]
    fn iso8601_parses_z() {
        let secs = parse_iso8601_to_unix_secs("2025-01-15T12:30:45Z").unwrap();
        assert_eq!(secs, 1_736_944_245);
    }

    #[test]
    fn iso8601_absurd_year_returns_none_not_panic() {
        assert_eq!(
            parse_iso8601_to_unix_secs("999999999999-01-01T00:00:00Z"),
            None
        );
    }

    #[test]
    fn iso8601_absurd_time_field_returns_none_not_panic() {
        assert_eq!(
            parse_iso8601_to_unix_secs("2025-01-15T9999999999999999:00:00Z"),
            None
        );
    }

    #[test]
    fn iso8601_absurd_month_or_day_returns_none_not_panic() {
        assert_eq!(
            parse_iso8601_to_unix_secs("2025-99999999999999999-01T00:00:00Z"),
            None,
            "absurd month must overflow-guard to None"
        );
        assert_eq!(
            parse_iso8601_to_unix_secs("2025-01-9223372036854775807T00:00:00Z"),
            None,
            "absurd day (i64::MAX) must overflow-guard to None"
        );
    }

    #[test]
    fn valid_session_id_accepts_every_agent_format() {
        assert!(is_valid_session_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_valid_session_id("019dc9ea-38d7-7372-9cc4-253ce944d41b"));
        assert!(is_valid_session_id("ses_1f80d49aeffeaKV4Lq4mc0c3cu"));
        assert!(is_valid_session_id("s"));
        assert!(is_valid_session_id(&"a".repeat(MAX_SESSION_ID_CHARS)));
    }

    #[test]
    fn valid_session_id_rejects_injection_and_control_chars() {
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id("ses_x; rm -rf ~"));
        assert!(!is_valid_session_id("$(reboot)"));
        assert!(!is_valid_session_id("id with space"));
        assert!(!is_valid_session_id("a/b"));
        assert!(!is_valid_session_id("`id`"));
        assert!(!is_valid_session_id("abc\r\nrm -rf ~"));
        assert!(!is_valid_session_id(&"a".repeat(MAX_SESSION_ID_CHARS + 1)));
    }

    #[test]
    fn valid_session_id_rejects_leading_dash_argument_injection() {
        assert!(!is_valid_session_id("--dangerously-skip-permissions"));
        assert!(!is_valid_session_id("-x"));
        assert!(!is_valid_session_id("-"));
        assert!(is_valid_session_id("a-b-c"));
        assert!(is_valid_session_id("_internal"));
    }

    #[test]
    fn clean_session_label_collapses_controls_and_caps_chars() {
        assert_eq!(
            clean_session_label("  hello\n\tworld\u{1b}  ", 20).as_deref(),
            Some("hello world")
        );
        assert_eq!(clean_session_label("abcdef", 3).as_deref(), Some("abc…"));
        assert_eq!(clean_session_label("\n\t", 10), None);
    }

    #[test]
    fn cwd_matches_ignores_trailing_separators() {
        assert!(cwd_matches("/repo/", "/repo"));
        assert!(cwd_matches("/", "/"));
    }

    #[cfg(windows)]
    #[test]
    fn cwd_matches_normalizes_windows_case_and_separators() {
        assert!(cwd_matches("C:/Dev/Paneflow/", "c:\\dev\\paneflow"));
        assert!(cwd_matches("C:\\", "c:/"));
    }

    #[test]
    fn iso8601_parses_fractional_seconds() {
        let secs = parse_iso8601_to_unix_secs("2025-01-15T12:30:45.123Z").unwrap();
        assert_eq!(secs, 1_736_944_245);
    }

    #[test]
    fn iso8601_unparseable_falls_back_to_date_prefix() {
        let label = format_relative_time("not a real timestamp");
        assert_eq!(label, "not a real");
    }

    fn meta(id: &str, branch: &str, ts: &str) -> SessionMeta {
        SessionMeta {
            agent: SessionAgent::Claude,
            session_id: id.into(),
            timestamp: ts.into(),
            cwd: "/repo".into(),
            git_branch: branch.into(),
            summary: None,
            model: None,
            usage: None,
        }
    }

    fn sortable_test_ts(i: usize) -> String {
        format!("2026-06-01T{:02}:{:02}:00Z", i / 60, i % 60)
    }

    #[test]
    fn match_ranks_branch_then_recency() {
        let sessions = vec![
            meta("old-branch", "feature", "2026-01-01T00:00:00Z"),
            meta("new-other", "main", "2026-06-01T00:00:00Z"),
            meta("new-branch", "feature", "2026-05-01T00:00:00Z"),
        ];
        let ranked = match_sessions_to_column(sessions, "/repo", "feature");
        let order: Vec<&str> = ranked.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(order, vec!["new-branch", "old-branch", "new-other"]);
    }

    #[test]
    fn match_drops_non_cwd_and_handles_empty_branch() {
        let mut wrong = meta("wrong", "feature", "2026-06-01T00:00:00Z");
        wrong.cwd = "/elsewhere".into();
        let sessions = vec![
            wrong,
            meta("older", "", "2026-01-01T00:00:00Z"),
            meta("newer", "", "2026-06-01T00:00:00Z"),
        ];
        let ranked = match_sessions_to_column(sessions, "/repo", "");
        let order: Vec<&str> = ranked.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(order, vec!["newer", "older"]);
    }

    #[test]
    fn sidebar_cap_keeps_newest_rows_and_reports_omitted() {
        let sessions: Vec<SessionMeta> = (0..(SIDEBAR_SESSION_RETAINED_PER_SOURCE + 3))
            .map(|i| meta(&format!("s-{i:03}"), "", &sortable_test_ts(i)))
            .collect();

        let (capped, omitted) =
            collect_recent_sessions(sessions, SIDEBAR_SESSION_RETAINED_PER_SOURCE);

        assert_eq!(capped.len(), SIDEBAR_SESSION_RETAINED_PER_SOURCE);
        assert_eq!(omitted, 3);
        assert_eq!(capped[0].session_id, "s-102");
        assert_eq!(capped.last().map(|s| s.session_id.as_str()), Some("s-003"));
    }

    #[test]
    fn match_caps_attribution_after_relevance_ranking() {
        let sessions: Vec<SessionMeta> = (0..(DIFF_ATTRIBUTION_MATCH_CAP + 5))
            .map(|i| meta(&format!("s-{i:02}"), "feature", &sortable_test_ts(i)))
            .collect();

        let ranked = match_sessions_to_column(sessions, "/repo", "feature");

        assert_eq!(ranked.len(), DIFF_ATTRIBUTION_MATCH_CAP);
        assert_eq!(ranked[0].session_id, "s-54");
        assert_eq!(
            ranked.last().map(|s| s.session_id.as_str()),
            Some("s-05"),
            "the five oldest ranked matches should be omitted"
        );
    }

    #[test]
    fn ranked_attribution_push_caps_before_usage_enrichment() {
        let mut retained = Vec::new();
        push_ranked_attribution(
            &mut retained,
            meta("new-other", "main", "2026-06-01T00:00:00Z"),
            "new-other",
            "feature",
            2,
        );
        push_ranked_attribution(
            &mut retained,
            meta("old-branch", "feature", "2026-01-01T00:00:00Z"),
            "old-branch",
            "feature",
            2,
        );
        push_ranked_attribution(
            &mut retained,
            meta("newer-other", "main", "2026-07-01T00:00:00Z"),
            "newer-other",
            "feature",
            2,
        );

        let order: Vec<&str> = retained
            .iter()
            .map(|(s, _)| s.session_id.as_str())
            .collect();
        assert_eq!(order, vec!["old-branch", "newer-other"]);
        let payloads: Vec<&str> = retained.iter().map(|(_, payload)| *payload).collect();
        assert_eq!(payloads, vec!["old-branch", "newer-other"]);
    }

    #[test]
    fn assistant_usage_total_and_add_saturate() {
        let mut u = AssistantUsage {
            input: 10,
            output: 5,
            cache_read: 2,
            cache_creation: 1,
        };
        assert_eq!(u.total(), 18);
        u.add(&AssistantUsage {
            input: u64::MAX,
            ..Default::default()
        });
        assert_eq!(u.input, u64::MAX, "add must saturate, not overflow-panic");
        assert!(!u.is_empty());
        assert!(AssistantUsage::default().is_empty());
    }

    #[test]
    fn iso8601_date_trims_to_10_chars() {
        let well = format_relative_time("definitely-not-a-timestamp-2025");
        assert_eq!(well.chars().count(), 10);

        let malicious = format_relative_time("2026-05-28\n<script>alert(1)</script>");
        assert!(!malicious.contains('\n'));
        assert!(malicious.chars().count() <= 10);

        let multi = format_relative_time("café-timestamp-very-long");
        assert_eq!(multi.chars().count(), 10);
    }
}
