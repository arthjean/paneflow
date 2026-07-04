//! Shared types and helpers for the AI-agent session readers. Each reader
//! sources sessions from its agent's documented native contract - JSONL
//! transcripts for agents that publish a file format, or CLI list commands
//! where the vendor exposes one. All readers normalise to the unified
//! [`SessionMeta`] below so the docked sidebar can render rows with a single
//! template.

/// Which AI agent created the session. Drives the row icon, the
/// `--resume` command shape, and the popover tab the row sits under.
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

    /// Brand glyph path (multicolor SVG - render via `img()`, not a tinted
    /// `svg()`). Shared by the sessions popover and the Review attribution badge
    /// (EP-004 US-015).
    pub(crate) fn icon_path(self) -> &'static str {
        self.terminal_agent().icon_path()
    }

    /// Human-readable agent name.
    pub(crate) fn label(self) -> &'static str {
        self.terminal_agent().display_name()
    }
}

pub(crate) const SESSION_AGENT_COUNT: usize = SessionAgent::ALL.len();
pub(crate) const MAX_SESSION_ID_CHARS: usize = 128;

/// EP-004 US-016: token usage aggregated across a session's assistant turns.
/// Additive on [`SessionMeta`]; `None` when the agent records no usage
/// (OpenCode) or the deeper scan was not run (the title-only popover path).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssistantUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

impl AssistantUsage {
    /// Sum across all tiers - the headline token count for a tooltip.
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_creation)
    }

    /// Fold another turn's usage in (saturating). Used by the scanners to
    /// aggregate `message.usage` across assistant turns.
    pub fn add(&mut self, other: &AssistantUsage) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_creation = self.cache_creation.saturating_add(other.cache_creation);
    }

    /// True when no tier carries a count - a parsed-but-empty usage block,
    /// treated as "no usage data" by the attribution UI.
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// US-017 (audit P2-5): module-level mtime-keyed cache for the
/// session readers. The popover scan currently re-walks the on-disk
/// JSONL store on every workspace switch, so a 100-session project
/// pays the parse cost each time. The cache stores the last
/// successful scan keyed by `(agent, cwd)` plus a caller-supplied
/// filesystem fingerprint observed at scan time. A subsequent scan
/// with an unchanged fingerprint returns the cached vector directly.
///
/// Readers with simple directory contracts can use `lookup` /
/// `store_result` directly. Readers whose root directory mtime does
/// not reflect leaf-file appends should compute a stronger snapshot
/// and call `lookup_with_mtime` / `store_result_with_mtime`.
pub mod cache {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, SystemTime};

    use super::{SessionAgent, SessionMeta};

    /// US-025 (cli-hardening-followup-2026-Q3): LRU capacity cap.
    /// The session-cache HashMap was process-global with no upper
    /// bound; switching across 20+ project directories (a common
    /// workflow for someone juggling client repos) used to grow
    /// the map monotonically. 10 entries is sized for the typical
    /// "active set" of recently-opened projects -- past that the
    /// least-recently-accessed entry is evicted on the next store.
    pub const MAX_CACHE_ENTRIES: usize = 10;

    /// US-025: monotonically-increasing access stamp. Bumped on
    /// every `lookup` hit and every `store_result` write so the
    /// smallest stamp identifies the LRU entry to evict.
    fn next_access_seq() -> u64 {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        SEQ.fetch_add(1, Ordering::Relaxed)
    }

    /// Maximum forward drift between the cached mtime and a newly
    /// observed mtime before we treat the cache as stale. APFS samples
    /// mtimes at nanosecond precision but its sampling jitter can
    /// produce drift of a few hundred nanoseconds even when no file
    /// actually changed -- a strict `==` would spuriously re-scan on
    /// every popover open (apenwarr, "mtime comparison considered
    /// harmful", 2018). 1ms is small enough that any real human-
    /// induced write (always seconds apart) is still caught and large
    /// enough to absorb all known filesystem-internal jitter.
    const MTIME_FUZZ: Duration = Duration::from_millis(1);

    struct Entry {
        mtime: SystemTime,
        sessions: Vec<SessionMeta>,
        omitted: usize,
        /// US-025: monotonic stamp of the last access (lookup hit
        /// or store_result write). Used by `store_result` to pick
        /// the LRU victim when the cache hits its cap.
        access_seq: u64,
    }

    fn store() -> &'static Mutex<HashMap<(SessionAgent, String), Entry>> {
        static CACHE: OnceLock<Mutex<HashMap<(SessionAgent, String), Entry>>> = OnceLock::new();
        CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// Read the directory mtime, returning `None` when the path does
    /// not exist or its metadata is unreadable. Both cases skip the
    /// cache (caller falls through to the scan and does not store the
    /// result).
    #[allow(dead_code)]
    fn dir_mtime(dir: &Path) -> Option<SystemTime> {
        std::fs::metadata(dir).ok().and_then(|m| m.modified().ok())
    }

    /// True when `observed` has not advanced past `cached` by more
    /// than [`MTIME_FUZZ`]. mtimes only ever move forward, so the
    /// `Err` branch of `duration_since` (clock went backwards, NTP
    /// correction, DST transition) is treated as stale -- conservative
    /// safe-direction failure.
    fn within_fuzz(cached: SystemTime, observed: SystemTime) -> bool {
        match observed.duration_since(cached) {
            Ok(delta) => delta < MTIME_FUZZ,
            Err(_) => false,
        }
    }

    /// Try to read a fresh session snapshot from the cache. Returns `Some`
    /// only when the dir's mtime is within `MTIME_FUZZ` of the cached
    /// snapshot's mtime -- catches real writes (seconds apart) without
    /// spurious invalidation on filesystem-internal jitter.
    #[allow(dead_code)]
    pub fn lookup(
        agent: SessionAgent,
        cwd: &str,
        project_dir: &Path,
    ) -> Option<(Vec<SessionMeta>, usize)> {
        let observed = dir_mtime(project_dir)?;
        lookup_with_mtime(agent, cwd, observed)
    }

    /// Try to read a fresh session snapshot from the cache using a
    /// reader-computed fingerprint. Use this when the parent
    /// directory mtime is not sufficient, for example JSONL appends
    /// inside an existing leaf file.
    pub fn lookup_with_mtime(
        agent: SessionAgent,
        cwd: &str,
        observed: SystemTime,
    ) -> Option<(Vec<SessionMeta>, usize)> {
        let mut guard = match store().lock() {
            Ok(g) => g,
            // US-008 (cli-hardening-followup-2026-Q3): a poisoned
            // session cache mutex means a prior holder panicked
            // mid-write. The recovered state may be partially-written
            // -- callers see a possibly-stale `Vec<SessionMeta>` once,
            // then `store_result` replaces it on the next scan. Log
            // so the previous panic is not hidden (matches
            // `lock_with_poison_log` in agent_terminal.rs).
            Err(p) => {
                tracing::warn!(
                    target: "paneflow_app::agent_sessions",
                    "session cache mutex poisoned on lookup; using potentially stale data \
                     (a previous thread panicked while holding the lock)"
                );
                p.into_inner()
            }
        };
        // US-025 (cli-hardening-followup-2026-Q3): bump the access
        // stamp on a cache hit so subsequent stores see this entry
        // as recently-used and pick a colder entry to evict.
        let entry = guard.get_mut(&(agent, cwd.to_string()))?;
        if within_fuzz(entry.mtime, observed) {
            entry.access_seq = next_access_seq();
            Some((entry.sessions.clone(), entry.omitted))
        } else {
            None
        }
    }

    /// Store the result of a fresh scan. The mtime is captured AFTER
    /// the scan to avoid the race where a write lands between the
    /// pre-scan mtime read and the post-scan write -- using the
    /// post-scan mtime means a follow-up write also invalidates the
    /// entry.
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

    /// Store a fresh scan using a reader-computed filesystem
    /// fingerprint. The caller should capture this after the scan so
    /// a concurrent write that lands during parsing invalidates the
    /// next lookup.
    pub fn store_result_with_mtime(
        agent: SessionAgent,
        cwd: &str,
        mtime: SystemTime,
        sessions: &[SessionMeta],
        omitted: usize,
    ) {
        let mut guard = match store().lock() {
            Ok(g) => g,
            // US-008 (cli-hardening-followup-2026-Q3): log poison
            // recovery on the write path too. The `insert` overwrites
            // the entry so the data is restored to a consistent state
            // on the next scan, but the previous panic deserves a
            // breadcrumb.
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
        // US-025 (cli-hardening-followup-2026-Q3): when the cache
        // is at capacity AND this key would be a NEW entry (not an
        // overwrite of an existing one), drop the LRU entry first.
        // The linear scan is O(N) on N=10 -- cheap enough versus
        // pulling in an `lru` crate dependency for one call site.
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

    /// Drop everything; used by tests to reset state between cases.
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

        /// Cache-stateful tests share the process-global `store()`
        /// Mutex. `poisoned_session_cache_logs_warning` deliberately
        /// poisons it from a spawned thread; run concurrently with
        /// `session_cache_evicts_lru` (which locks `store()` directly
        /// and asserts exact contents) it made the latter flaky -- the
        /// LRU test would observe the poison mid-run and panic. cargo
        /// runs tests in parallel within one process, so serialize the
        /// two so each owns the shared cache exclusively. The guard is
        /// itself poison-tolerant for the same reason.
        fn serial() -> std::sync::MutexGuard<'static, ()> {
            static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
            LOCK.lock().unwrap_or_else(|e| e.into_inner())
        }

        /// EP-004 review follow-up: a strict `==` mtime check would
        /// spuriously invalidate on APFS nanosecond jitter. With the
        /// fuzz band, observed - cached < 1ms reads as fresh.
        #[test]
        fn within_fuzz_accepts_subms_drift() {
            let cached = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
            // 500 microseconds forward drift -- well inside the fuzz band.
            let observed = cached + Duration::from_micros(500);
            assert!(
                within_fuzz(cached, observed),
                "{:?} sub-ms drift should be tolerated",
                MTIME_FUZZ,
            );
        }

        /// EP-004 review follow-up: a real human-induced write lands
        /// seconds after the cached scan and MUST invalidate the
        /// cache. A 5 ms forward drift already exceeds the 1 ms fuzz
        /// band -- representative of any real file mutation.
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

        /// Identical mtime is the no-write hot path. Must still match.
        #[test]
        fn within_fuzz_accepts_exact_match() {
            let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
            assert!(within_fuzz(t, t));
        }

        /// Backwards drift (NTP correction, DST transition) is treated
        /// as stale -- safe-direction failure since mtimes only ever
        /// move forward and a backwards observation means something
        /// underneath us is unreliable.
        #[test]
        fn within_fuzz_rejects_backwards_drift() {
            let cached = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
            let observed = cached - Duration::from_millis(10);
            assert!(!within_fuzz(cached, observed));
        }

        /// US-025 (cli-hardening-followup-2026-Q3): when the cache
        /// is at capacity, inserting a new distinct (agent, cwd)
        /// must evict the least-recently-accessed existing entry.
        ///
        /// Review 2026-05-28: switched from inlining the eviction
        /// logic into the test (which risked drifting away from the
        /// production path in `store_result`) to calling
        /// `store_result` directly with a tempdir, so the test now
        /// exercises the real production code path. The earlier
        /// shape was kept only because `store_result` reads
        /// `dir_mtime`; supplying a tempdir satisfies that without
        /// duplicating the eviction branch.
        #[test]
        fn session_cache_evicts_lru() {
            use super::super::SessionAgent;
            use super::Entry;
            // Serialize against `poisoned_session_cache_logs_warning`,
            // which poisons the shared `store()` Mutex from a parallel
            // thread; without this the direct lock below sees a
            // PoisonError and panics (CI flake, aarch64 scheduling).
            let _serial = serial();
            // Isolate from any sibling test's cache state.
            super::clear();
            // Real on-disk dir for `store_result`'s `dir_mtime` call.
            let dir = tempfile::tempdir().expect("tempdir");
            // Seed the cache directly to MAX_CACHE_ENTRIES so the
            // 11th insert exercises the production eviction path.
            // We bypass `store_result` for the seeding step because
            // `dir_mtime` returns the SAME mtime for the SAME path,
            // and we want each entry to have a distinct
            // (agent, cwd) key for the LRU comparison.
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
                // /proj-0 is the oldest entry by access_seq.
                assert!(guard.contains_key(&(SessionAgent::Claude, "/proj-0".to_string())));
            }
            // Insert the 11th distinct entry via the REAL production
            // path: `store_result` enforces the cap, picks the LRU
            // victim, evicts it, then inserts. This catches any
            // future drift in the eviction branch (line 179-192).
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

        /// US-008 (cli-hardening-followup-2026-Q3): poison recovery
        /// must leave a log breadcrumb. The cache can still recover
        /// with `PoisonError::into_inner`, but the previous panic is
        /// operationally relevant and should not disappear.
        #[test]
        #[traced_test]
        fn poisoned_session_cache_logs_warning() {
            use super::super::SessionAgent;

            // See `serial()` -- this test poisons the shared cache, so
            // it must not overlap `session_cache_evicts_lru`.
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

/// Return the session-readable agents whose AI Agent launcher is currently
/// visible, in PaneFlow's launcher display order. Render paths should use this
/// with the already-cached app config so they never hit the filesystem.
pub fn enabled_session_agents_from_config(
    cfg: &paneflow_config::schema::PaneFlowConfig,
) -> Vec<SessionAgent> {
    crate::agent_launcher::TerminalAgent::visible(cfg)
        .into_iter()
        .filter_map(|agent| agent.session_agent())
        .collect()
}

/// Read user config and return the currently enabled session agents. Use this
/// only from non-render paths that do not already hold a fresh config snapshot.
pub fn enabled_session_agents() -> Vec<SessionAgent> {
    let cfg = paneflow_config::loader::load_config();
    enabled_session_agents_from_config(&cfg)
}

/// Blocking title-only session scan for the given agent and cwd. Call from
/// inside `smol::unblock`.
pub(crate) fn read_sessions_for_cwd(agent: SessionAgent, cwd: &str) -> Vec<SessionMeta> {
    match agent {
        SessionAgent::Claude => crate::claude_sessions::read_sessions_for_cwd(cwd),
        SessionAgent::Codex => crate::codex_sessions::read_sessions_for_cwd(cwd),
        SessionAgent::OpenCode => crate::opencode_sessions::read_sessions_for_cwd(cwd),
        _ => read_sessions_for_cwd_with_omitted(agent, cwd).0,
    }
}

/// Blocking title-only session scan plus retention metadata for the docked
/// sidebar. Call from inside `smol::unblock`.
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

/// EP-003: maximum session rows retained in the docked sessions sidebar for
/// one `(agent, cwd)` source. Readers already sort newest-first, so capping is
/// a stable "keep the latest" operation.
pub(crate) const SIDEBAR_SESSION_RETAINED_PER_SOURCE: usize = 100;

/// EP-003: maximum matched sessions retained on one Review attribution column.
/// Ranking happens first, then this cap keeps the most relevant matches.
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

/// Strict allow-list guard for a session id before it is interpolated into a
/// resume command (`claude --resume <id>`, `codex resume <id>`,
/// `opencode --session <id>`, etc.) and sent to the user's PTY.
///
/// Every supported agent's id format fits `^[A-Za-z0-9_-]+$`: Claude/Codex use
/// UUIDs (`550e8400-e29b-41d4-a716-446655440000`), OpenCode uses
/// `ses_<base62>` (`ses_1f80d49aeffeaKV4Lq4mc0c3cu`), and several newer CLIs
/// use short alphanumeric IDs. Anything outside that set
/// a space, `;`, `$(…)`, a path separator, a control char - means the source
/// record is malformed or the agent binary was tampered with. Rejecting here
/// (and dropping the record at scan time) is defence-in-depth: a crafted
/// `ses_x; rm -rf ~` would pass a control-char-only check but never the
/// allow-list, so it can never reach the shell.
///
/// The **first** character is additionally required to be alphanumeric or `_`:
/// the allow-list above permits `-`, so without this a tampered record carrying
/// `--dangerously-skip-permissions` (a single token, no shell metachar) would
/// pass and be interpolated as `codex resume --dangerously-skip-permissions` /
/// `opencode --session --foo` / `claude --resume --foo`, i.e. argument
/// injection (CWE-88) flipping the agent CLI into an unexpected mode. Real ids
/// never lead with `-` (UUIDs start hex, OpenCode with `ses_`), so the
/// constraint has zero false-positive cost.
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

/// Compare a session's recorded working directory against the directory the
/// sidebar is scanning for. Case-sensitive on Unix (paths there are
/// case-sensitive); case- and separator-insensitive on Windows, where NTFS /
/// ReFS are case-insensitive and the cwd can reach us with either separator
/// (PowerShell's `(Get-Location).ProviderPath` yields `\`, while a session an
/// agent recorded under a POSIX-flavoured shell may carry `/`).
///
/// Without this the filter was a raw `==`: a workspace opened as
/// `C:\Dev\Paneflow` would silently fail to match a session Claude Code
/// recorded as `C:\dev\paneflow`, so the sidebar looked empty on Windows even
/// though the sessions exist. All three readers (`claude` / `codex` /
/// `opencode`) route their cwd filter through here.
pub(crate) fn cwd_matches(recorded: &str, scanned: &str) -> bool {
    #[cfg(windows)]
    {
        // ASCII case-fold is sufficient for real paths (drive letters + the
        // ASCII dir names that dominate); both sides fold identically so the
        // comparison stays symmetric.
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

/// Unified session metadata. Anything the UI needs to render a row +
/// resume the session is here; the heavier message payload stays on disk.
#[derive(Debug, Clone)]
pub struct SessionMeta {
    /// Which CLI created the session - drives row routing and the resume
    /// command (`claude --resume <id>` vs `codex resume <id>`).
    pub agent: SessionAgent,
    pub session_id: String,
    /// ISO 8601 timestamp from the first event. Used for sorting (lexical
    /// sort matches chronological order for ISO 8601).
    pub timestamp: String,
    /// `cwd` recorded on the first line. Files where the first line lacks
    /// `cwd` are skipped, so this is always populated.
    pub cwd: String,
    /// Git branch - empty string when the session was outside a git repo
    /// (Claude Code) or when the agent doesn't record one (Codex CLI).
    /// EP-004 US-014 consumes this in [`match_sessions_to_column`] (branch is
    /// the 2nd-tier ranking key after exact cwd), so the `#[allow(dead_code)]`
    /// it carried while only the sidebar read it is gone.
    pub git_branch: String,
    /// Human-readable session label. Sourced from an LLM-generated title
    /// when available, falling back to the cleaned first user message
    /// otherwise. `None` if neither could be extracted.
    pub summary: Option<String>,
    /// EP-004 US-016: model name from the session transcript (e.g.
    /// `claude-opus-4-...`, `gpt-5`). `None` on the title-only popover scan or
    /// when the agent doesn't record one. Drives the attribution badge label +
    /// pricing lookup.
    pub model: Option<String>,
    /// EP-004 US-016: aggregated token usage across assistant turns. `None` on
    /// the title-only popover scan, when the agent records no usage (OpenCode),
    /// or when the deeper scan found none. Drives the estimated-cost figure.
    pub usage: Option<AssistantUsage>,
}

/// Collect only the newest `cap` sessions by ISO timestamp while counting the
/// older rows omitted. This avoids building long-lived UI/cache vectors just to
/// truncate them afterwards.
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

/// Incremental newest-N collector for readers that discover sessions through a
/// callback walk instead of an iterator chain.
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

/// EP-003 review: keep the attribution candidate set bounded while preserving
/// the final `branch > recency` ranking. Claude/Codex use this before usage
/// enrichment so only the retained candidates pay the deep JSONL scan cost.
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

/// EP-004 US-014: rank already-cwd-filtered sessions for one diff column by
/// `exact-cwd > branch > recency`. A pure function (no I/O, no clock) so it is
/// unit-testable; the readers have already filtered to `col_path`'s cwd, so the
/// cwd tier is enforced defensively here and the live discriminators are branch
/// match then ISO-8601 timestamp (lexical == chronological). Most-relevant
/// session first.
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

/// EP-004 US-014: gather every enabled agent's sessions for a worktree `cwd`
/// (usage-enriched where the agent supports it - US-016) and rank them against
/// the column's `(cwd, branch)`. **Blocking I/O** - call from inside
/// `smol::unblock` (it is folded into the off-thread diff-load task so
/// attribution never blocks first paint and is re-fetched only on re-diff).
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
            // OpenCode's CLI contract carries no token usage; agent + recency
            // only (graceful degradation), so the title-only scan is enough.
            SessionAgent::OpenCode => {
                all.extend(crate::opencode_sessions::read_sessions_for_cwd(cwd))
            }
            // These readers expose title-only metadata through documented
            // local contracts. They have no token usage in PaneFlow today, so
            // attribution degrades to agent + recency like OpenCode.
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

/// Format an ISO 8601 timestamp into a short relative label. Pure string
/// math (no `chrono` dep) - parses `YYYY-MM-DDTHH:MM:SS` and computes the
/// delta against `std::time::SystemTime::now()` via a calendar-free
/// approximation good enough for "Xm ago" / "Xh ago" / "Xd ago" labels.
///
/// Falls back to the date prefix (`YYYY-MM-DD`) when parsing fails.
pub fn format_relative_time(iso8601: &str) -> String {
    // US-009 (cli-hardening-followup-2026-Q3): if the system clock
    // is before UNIX_EPOCH (impossible in practice, but
    // `duration_since` returns `Err` and the previous `unwrap_or(0)`
    // silently mapped every session to "30+ days ago"), drop to the
    // ISO-date prefix fallback instead. Future NTP step-backwards
    // beyond the cached `parse_iso8601_to_unix_secs` is already
    // safe-bounded by `saturating_sub`.
    //
    // US-026 (cli-hardening-followup-2026-Q3): the fallback path
    // additionally trims to the first 10 characters defensively --
    // a malformed JSONL field containing a newline or
    // `<script>alert(1)</script>` will not blow up the sidebar
    // layout. Well-formed `YYYY-MM-DDTHH:MM:SS` and
    // `YYYY-MM-DD` inputs are already <= 10 chars after the
    // `split('T').next()` so the clamp is a no-op for them.
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

/// US-026 (cli-hardening-followup-2026-Q3): defensive ISO-date
/// fallback. Takes the substring before the first `T` (the typical
/// `YYYY-MM-DD` prefix of an ISO-8601 timestamp), then clamps to
/// the first 10 chars on a `chars()` boundary (UTF-8 safe).
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

/// Minimal ISO 8601 → Unix-seconds parser. Accepts
/// `YYYY-MM-DDTHH:MM:SS[.fff][Z|±HH:MM]`. Treats the timestamp as UTC.
/// Calendar math via Howard Hinnant's "days from civil" algorithm; an
/// off-by-one on leap-second boundaries is acceptable for a relative-time
/// UI label.
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

    // U-011: year/month/day/hour/minute/second are parsed verbatim from
    // agent-written JSONL with NO range clamp, so the civil-days math below can
    // overflow i64 well before the trailing `era * 146_097` / `days * 86_400` /
    // `hour * 3_600` multiplies that were guarded first. The fields reach here
    // as non-negative i64 (the `split('-')` parse can't yield a signed value),
    // but each can be enormous: a huge `month` overflows `153 * month_adj`, a
    // huge `day` overflows the `+ day` step, and a huge `year` overflows the
    // later `era * 146_097`. A debug build panics; a release build silently
    // wraps and returns `Some(garbage)`, bypassing the fallback. So every op
    // that can overflow is checked and falls through to `iso8601_safe_fallback`
    // (the date-prefix render) on overflow. The `year - 1` / `era * 400` checks
    // are defensive (unreachable while years stay non-negative, but cheap and
    // they keep the path correct if a signed-year parse is ever added);
    // `month_adj` and `yoe*365 + yoe/4 - yoe/100` are provably in-range
    // (`yoe ∈ [0, 399]`) so they stay unchecked.
    let y = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = y.div_euclid(400);
    let yoe = y.checked_sub(era.checked_mul(400)?)?; // y - era*400, lands in [0,399]
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
        // U-011: a parseable-but-absurd year (well within i64's digit budget)
        // overflows the day/second multiplies. Checked arithmetic must yield
        // None so the caller falls back to the date-prefix render - in debug
        // builds this would otherwise panic, in release it would silently wrap.
        assert_eq!(
            parse_iso8601_to_unix_secs("999999999999-01-01T00:00:00Z"),
            None
        );
    }

    #[test]
    fn iso8601_absurd_time_field_returns_none_not_panic() {
        // U-011: hour/minute/second are equally unbounded in the source JSONL,
        // so the `hour * 3_600` multiply must be checked too - a valid date
        // with an absurd hour would otherwise overflow before the day math.
        assert_eq!(
            parse_iso8601_to_unix_secs("2025-01-15T9999999999999999:00:00Z"),
            None
        );
    }

    #[test]
    fn iso8601_absurd_month_or_day_returns_none_not_panic() {
        // U-011: `month` and `day` are equally unbounded in the source JSONL.
        // An absurd month overflows the `153 * month_adj` multiply, and an
        // absurd day overflows the `+ day` step - both BEFORE the trailing
        // checked path, so they must be checked too. In debug these would
        // otherwise panic; in release they would wrap and return Some(garbage).
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
        // Claude / Codex UUIDs and OpenCode `ses_<base62>` all fit the
        // allow-list so a legitimate resume is never dropped.
        assert!(is_valid_session_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_valid_session_id("019dc9ea-38d7-7372-9cc4-253ce944d41b"));
        assert!(is_valid_session_id("ses_1f80d49aeffeaKV4Lq4mc0c3cu"));
        assert!(is_valid_session_id("s"));
        assert!(is_valid_session_id(&"a".repeat(MAX_SESSION_ID_CHARS)));
    }

    #[test]
    fn valid_session_id_rejects_injection_and_control_chars() {
        // Each of these would pass a control-char-only check yet inject a
        // second shell command once interpolated into `<agent> resume <id>`.
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id("ses_x; rm -rf ~"));
        assert!(!is_valid_session_id("$(reboot)"));
        assert!(!is_valid_session_id("id with space"));
        assert!(!is_valid_session_id("a/b"));
        assert!(!is_valid_session_id("`id`"));
        // Control chars (the case the old guard already covered) stay rejected.
        assert!(!is_valid_session_id("abc\r\nrm -rf ~"));
        assert!(!is_valid_session_id(&"a".repeat(MAX_SESSION_ID_CHARS + 1)));
    }

    #[test]
    fn valid_session_id_rejects_leading_dash_argument_injection() {
        // US-044 (EP-007 review): a leading `-` passes the `[A-Za-z0-9_-]`
        // char-set but turns the id into a FLAG once interpolated into
        // `codex resume <id>` / `opencode --session <id>` / `claude --resume
        // <id>` (argument injection, CWE-88). The first char must be
        // alphanumeric or `_`.
        assert!(!is_valid_session_id("--dangerously-skip-permissions"));
        assert!(!is_valid_session_id("-x"));
        assert!(!is_valid_session_id("-"));
        // An interior dash is still fine (UUIDs rely on it).
        assert!(is_valid_session_id("a-b-c"));
        // A leading underscore stays valid (no flag confusion).
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

    /// US-026 (cli-hardening-followup-2026-Q3): the fallback now
    /// also clamps to 10 chars, so an unparseable input is truncated.
    /// The original assertion is updated accordingly; the storage
    /// shape didn't change (still ISO-date prefix shape), only its
    /// length is bounded.
    #[test]
    fn iso8601_unparseable_falls_back_to_date_prefix() {
        let label = format_relative_time("not a real timestamp");
        // "not a real timestamp" has no 'T', so `split('T').next()`
        // returns the whole string; the 10-char clamp then trims it.
        assert_eq!(label, "not a real");
    }

    /// US-026 (cli-hardening-followup-2026-Q3): the fallback must
    /// be safe against malformed inputs that would otherwise blow
    /// up the sidebar layout (newlines, oversize strings, multi-byte).
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
        // EP-004 US-014: among cwd-matched sessions, a branch match outranks a
        // non-match; within a tier, most recent first.
        let sessions = vec![
            meta("old-branch", "feature", "2026-01-01T00:00:00Z"),
            meta("new-other", "main", "2026-06-01T00:00:00Z"),
            meta("new-branch", "feature", "2026-05-01T00:00:00Z"),
        ];
        let ranked = match_sessions_to_column(sessions, "/repo", "feature");
        // Both "feature" sessions rank above the "main" one; within feature,
        // the more recent (2026-05) precedes the older (2026-01).
        let order: Vec<&str> = ranked.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(order, vec!["new-branch", "old-branch", "new-other"]);
    }

    #[test]
    fn match_drops_non_cwd_and_handles_empty_branch() {
        // A session in a different cwd is dropped; with an empty column branch
        // (Codex-style), ranking falls back to pure recency.
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
        // Well-formed: behaves as before.
        let well = format_relative_time("definitely-not-a-timestamp-2025");
        assert_eq!(well.chars().count(), 10);

        // Newline-injected malformed input: only the date prefix
        // (or the first 10 chars in the absence of 'T') is kept.
        let malicious = format_relative_time("2026-05-28\n<script>alert(1)</script>");
        assert!(!malicious.contains('\n'));
        assert!(malicious.chars().count() <= 10);

        // Multi-byte safety: a `cafétimestamp` truncates on a char
        // boundary (chars().take(10) -- not bytes().take(10)).
        let multi = format_relative_time("café-timestamp-very-long");
        assert_eq!(multi.chars().count(), 10);
    }
}
