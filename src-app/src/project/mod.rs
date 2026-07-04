// US-007 (prd-agents-view.md) is the foundation story for the Agents
// view domain model. Many items here are deliberately "unused" until
// follow-up stories activate them:
// - ID counters + `bump_id_counters_to`: US-009 restore path.
// - `project_from_session` / `thread_from_session` / `agent_kind_from_str`:
//   US-009 restore from session.json.
// - `Thread::new` / `Project::new` constructors: US-011 sidebar
//   create-new affordances.
// - `ThreadStatus` non-`Idle` variants + `Thread::status`: US-013/US-015
//   streaming state machine.
//
// The PRD's dependency chain (US-007 -> US-008 -> US-009 -> US-010 -> US-011)
// only takes meaning when activated end-to-end; the foundation is what
// US-007 ships. The allow is module-scoped (not per-item) so the
// review surface stays one comment-line, not 12 attributes.
#![allow(dead_code)]

//! Runtime domain model for the Agents view (US-007 of
//! `tasks/prd-agents-view.md`).
//!
//! Mirrors the existing [`crate::workspace::Workspace`] shape:
//! - Monotonic `AtomicU64` ID counters per type
//! - A small struct holding the live state
//! - Conversion to/from the matching [`paneflow_config::schema`]
//!   "session" struct so save/restore round-trips cleanly
//!
//! The split between runtime types (here) and session types (in
//! `paneflow-config`) is intentional: session types are pure data and
//! belong to a leaf crate so the schema can evolve without dragging the
//! whole app crate into the same compile unit. Runtime types carry the
//! richer enums and conversion helpers.

use paneflow_acp::AgentKind;
use paneflow_config::schema::{ProjectSession, ThreadSession};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic project ID counter -- one per process. Mirrors
/// [`crate::workspace::next_workspace_id`] (US-007 AC).
static NEXT_PROJECT_ID: AtomicU64 = AtomicU64::new(1);

/// Monotonic thread ID counter -- one per process. Mirrors
/// [`crate::workspace::next_workspace_id`] (US-007 AC).
static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);

/// Restore caps for `session.json`. The file-size cap protects memory, but a
/// compact session can still encode thousands of projects/chats that fan out
/// into git probes, sidebar rows, and terminal-cache work after boot.
pub const MAX_RESTORED_PROJECTS: usize = 128;
pub const MAX_RESTORED_THREADS_PER_PROJECT: usize = 256;
pub const MAX_RESTORED_CHATS: usize = 512;
pub const MAX_RESTORED_TOTAL_THREADS: usize = 2048;

pub fn next_project_id() -> u64 {
    NEXT_PROJECT_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn next_thread_id() -> u64 {
    NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed)
}

/// On restore, advance both counters past any IDs reloaded from
/// `session.json` so newly-created projects/threads cannot collide
/// with restored ones.
///
/// US-002 (prd-agents-ui-codex-redesign-2026-Q3.md): free `chats` are
/// `Thread`s allocated from the SAME `next_thread_id` counter as project
/// threads, so they MUST be folded into the thread-ID max here. Omitting
/// them would leave the counter below the highest restored chat ID and
/// the next `next_thread_id()` would collide with a live chat.
pub fn bump_id_counters_to(projects: &[Project], chats: &[Thread]) {
    let max_project = projects.iter().map(|p| p.id).max().unwrap_or(0);
    let max_thread = projects
        .iter()
        .flat_map(|p| p.threads.iter().map(|t| t.id))
        .chain(chats.iter().map(|t| t.id))
        .max()
        .unwrap_or(0);
    bump_counter(&NEXT_PROJECT_ID, max_project + 1);
    bump_counter(&NEXT_THREAD_ID, max_thread + 1);
}

/// Namespace offset applied to a [`Thread::id`] before it is handed to
/// [`crate::terminal::view::TerminalView`] as the PTY `PANEFLOW_WORKSPACE_ID`.
///
/// Thread IDs and CLI-mode workspace IDs come from two independent
/// counters that both start at 1, so the raw values collide: an `ai.*`
/// hook frame emitted from thread 3's PTY used to be indistinguishable
/// from one emitted from workspace 3 and was routed to the wrong
/// surface (or dropped). Offsetting the thread namespace into the high
/// half of u64 keeps the env var a single opaque id for the hook shim
/// while letting the IPC handler route unambiguously: `< BASE` →
/// workspace, `>= BASE` → Agents thread.
pub const AGENTS_THREAD_ENV_ID_BASE: u64 = 1 << 32;

/// The PTY-env id for an Agents thread (see [`AGENTS_THREAD_ENV_ID_BASE`]).
pub fn thread_env_id(thread_id: u64) -> u64 {
    AGENTS_THREAD_ENV_ID_BASE + thread_id
}

/// Decode a PTY-env id back to a [`Thread::id`]. Returns `None` for ids
/// below the namespace base (CLI-mode workspace ids).
pub fn thread_id_from_env_id(env_id: u64) -> Option<u64> {
    env_id.checked_sub(AGENTS_THREAD_ENV_ID_BASE)
}

/// Explicit selection target for the Agents-view center surface (US-003
/// of `prd-agents-ui-codex-redesign-2026-Q3.md`). Replaces the positional
/// `active_thread_idx: Option<usize>` so the center can address a thread of
/// a project OR a free chat without the index-stale bug class that a second
/// parallel `Option<usize>` would reintroduce. `None` (the `Option` wrapper
/// on `PaneFlowApp`) is the picker/home state; the project anchor for that
/// state stays `active_project_idx`.
///
/// Both arms are positional indices into the live `projects` / `chats`
/// vectors, kept in range by the data-plane ops (`select_thread`,
/// `remove_thread`, `select_chat`, `remove_chat`, `close_project`). The PTY
/// warm-resume cache is keyed by the stable `Thread::id`, not by this
/// target, so navigating between sources never tears down a running shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentsTarget {
    /// A thread inside `projects[project_idx].threads[thread_idx]`.
    Thread {
        project_idx: usize,
        thread_idx: usize,
    },
    /// A free chat inside `chats[chat_idx]` (anchored on the home dir).
    Chat { chat_idx: usize },
}

/// What a newly-launched agent is created into when no target is selected
/// (the picker/home state, US-005 of
/// `prd-agents-ui-codex-redesign-2026-Q3.md`). `Project` → a thread in the
/// active project (the legacy "select a project → agent picker" flow);
/// `NewChat` → a free chat in the home dir (the rail's "New chat" row). The
/// render branch reads this to pick the launcher title + create path; every
/// concrete selection (`select_thread`/`select_chat`) resets it to
/// `Project` so a later deselect never lands back in the new-chat picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentsPickerContext {
    #[default]
    Project,
    NewChat,
}

fn bump_counter(counter: &AtomicU64, target: u64) {
    let mut current = counter.load(Ordering::Relaxed);
    while current < target {
        match counter.compare_exchange_weak(current, target, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

/// Per-thread state machine for the Agents sidebar. It stays deliberately
/// small: the row renders compact visual indicators only, never status text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThreadStatus {
    #[default]
    Idle,
    Thinking,
    WaitingForInput,
    Failed,
}

impl ThreadStatus {
    /// Derive the Agents sidebar state from the hook-backed agent lifecycle.
    ///
    /// The Agents rail intentionally stays compact: active states render as
    /// visual indicators, not status text.
    pub fn from_agent_state(state: crate::ai_types::AgentState) -> Self {
        match state {
            crate::ai_types::AgentState::Thinking | crate::ai_types::AgentState::Stalled => {
                ThreadStatus::Thinking
            }
            crate::ai_types::AgentState::WaitingForInput => ThreadStatus::WaitingForInput,
            crate::ai_types::AgentState::Finished => ThreadStatus::Idle,
            crate::ai_types::AgentState::Errored => ThreadStatus::Failed,
        }
    }
}

/// What kind of surface a thread row drives in the main area. The
/// Agents view is terminal-only since the in-app ACP chat was removed,
/// so every live thread renders a `Terminal` PTY surface (launching the
/// thread's [`crate::agent_launcher::TerminalAgent`] CLI). `Agent` is a
/// legacy variant retained only so a pre-removal `session.json` (chat
/// threads) still deserializes; those rows are routed through the same
/// terminal path at render time and relaunch their original agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThreadKind {
    /// Legacy chat thread restored from an older session. Rendered as a
    /// terminal (relaunching the stored `agent`); never created anew.
    #[default]
    Agent,
    /// A PTY surface in the thread's cwd, optionally auto-launching a
    /// CLI agent (`terminal_agent`).
    Terminal,
}

/// One persistent thread row in the Agents sidebar. A Terminal Thread:
/// the PTY is the source of truth and only the sidebar metadata
/// round-trips through session.json.
#[derive(Debug, Clone)]
pub struct Thread {
    pub id: u64,
    pub title: String,
    pub agent: AgentKind,
    pub kind: ThreadKind,
    pub status: ThreadStatus,
    pub cwd: String,
    pub created_at: u64,
    pub model: Option<String>,
    pub mode: Option<String>,
    /// Vestigial foreign key from the removed `paneflow-threads` chat
    /// store. No longer read or written (terminal threads have no SQL
    /// rows); kept on the struct only so a pre-removal `session.json`
    /// round-trips its `store_id` field without data loss.
    pub store_id: Option<String>,
    /// Which CLI coding agent a [`ThreadKind::Terminal`] thread launches
    /// on first PTY mount. `None` for a bare shell (the plain
    /// "New terminal thread" affordance + legacy Agent rows). Always
    /// `None` for `ThreadKind::Agent`.
    pub terminal_agent: Option<crate::agent_launcher::TerminalAgent>,
    /// US-001 (prd-agents-ui-codex-redesign-2026-Q3.md): whether the user
    /// pinned this thread. Pinned threads (project threads and free chats
    /// alike) are surfaced in the rail's PINNED section. Round-trips through
    /// [`ThreadSession::pinned`]; a restored thread without the flag is
    /// `false`.
    pub pinned: bool,
    /// PID of the CLI agent currently driving [`Self::status`], reported
    /// by the `ai.*` hook frames. Transient (never persisted - a restored
    /// thread is always `Idle`): only consumed by the stale-PID sweep so a
    /// killed agent can't leave the sidebar spinner running forever.
    /// `None` for legacy hook shims that omit `pid`; the sweep then keeps
    /// the state conservatively (same policy as workspace sessions).
    pub agent_pid: Option<u32>,
    /// Opaque OS process start fingerprint for [`Self::agent_pid`].
    /// Transient and best-effort: when present, stale sweeping can detect PID
    /// reuse instead of treating a recycled PID as the original live agent.
    pub agent_proc_start: Option<u64>,
    /// Forced agent session UUID for a Claude Terminal Thread. Generated
    /// at creation for agents that support `--session-id`
    /// ([`crate::agent_launcher::TerminalAgent::supports_forced_session_id`])
    /// and injected into the launch command so the live thread maps 1:1 to
    /// its on-disk session file. Round-trips through
    /// [`ThreadSession::session_id`]; `None` for bare shells and non-Claude
    /// agents.
    pub session_id: Option<String>,
    /// Whether the user manually renamed this row. When `true`,
    /// [`PaneFlowApp::handle_terminal_thread_title_changed`] and the
    /// `ai-title` backfill leave the title untouched so a deliberate name
    /// survives agent activity. Round-trips through
    /// [`ThreadSession::title_user_set`].
    pub title_user_set: bool,
}

impl Thread {
    /// Create a fresh thread with auto-allocated ID, current timestamp,
    /// and `Idle` status.
    pub fn new(title: impl Into<String>, agent: AgentKind, cwd: impl Into<String>) -> Self {
        Self {
            id: next_thread_id(),
            title: title.into(),
            agent,
            kind: ThreadKind::Agent,
            status: ThreadStatus::Idle,
            cwd: cwd.into(),
            created_at: now_unix_millis(),
            model: None,
            mode: None,
            store_id: None,
            terminal_agent: None,
            pinned: false,
            agent_pid: None,
            agent_proc_start: None,
            session_id: None,
            title_user_set: false,
        }
    }

    /// Create a fresh Terminal Thread bound to `terminal_agent` (the CLI
    /// auto-launched on first PTY mount, or `None` for a bare shell).
    /// The `agent` slot is filled with a placeholder (`ClaudeCode`) that
    /// the Terminal dispatch never consults - see [`ThreadKind`] for the
    /// rationale.
    pub fn new_terminal(
        title: impl Into<String>,
        cwd: impl Into<String>,
        terminal_agent: Option<crate::agent_launcher::TerminalAgent>,
    ) -> Self {
        // Mint a forced session UUID for agents that accept `--session-id`
        // (Claude only today). This binds the live thread to a known
        // on-disk session file so the sidebar can adopt its `ai-title`
        // (parity with `/resume`) even when several threads share a cwd.
        let session_id = terminal_agent
            .filter(|a| a.supports_forced_session_id())
            .map(|_| uuid::Uuid::new_v4().to_string());
        Self {
            id: next_thread_id(),
            title: title.into(),
            agent: AgentKind::ClaudeCode,
            kind: ThreadKind::Terminal,
            status: ThreadStatus::Idle,
            cwd: cwd.into(),
            created_at: now_unix_millis(),
            model: None,
            mode: None,
            store_id: None,
            terminal_agent,
            pinned: false,
            agent_pid: None,
            agent_proc_start: None,
            session_id,
            title_user_set: false,
        }
    }
}

/// Sidebar grouping of threads with a shared cwd anchor.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: u64,
    pub title: String,
    pub cwd: String,
    pub threads: Vec<Thread>,
    pub is_expanded: bool,
    /// Cached `git diff --shortstat` of the project's cwd. Refreshed
    /// by the 30 s git poller in `app/bootstrap.rs` (same loop that
    /// keeps workspace stats fresh). Stays `Default::default()` for
    /// non-git directories.
    pub git_stats: crate::workspace::GitDiffStats,
}

impl Project {
    /// Create a fresh project with auto-allocated ID, no threads, and
    /// `is_expanded = true` (matches the new-project-just-clicked
    /// expectation in the sidebar UX -- US-010).
    pub fn new(title: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            id: next_project_id(),
            title: title.into(),
            cwd: cwd.into(),
            threads: Vec::new(),
            is_expanded: true,
            git_stats: crate::workspace::GitDiffStats::default(),
        }
    }
}

/// Convert an in-memory [`Project`] to its persisted shape.
pub fn project_to_session(p: &Project) -> ProjectSession {
    ProjectSession {
        id: p.id,
        title: p.title.clone(),
        cwd: p.cwd.clone(),
        is_expanded: p.is_expanded,
        threads: p.threads.iter().map(thread_to_session).collect(),
    }
}

/// Convert an in-memory [`Thread`] to its persisted shape. The
/// runtime `status` is intentionally not persisted -- every thread
/// restores as `Idle`; live state is rebuilt from hook/IPC events.
pub fn thread_to_session(t: &Thread) -> ThreadSession {
    ThreadSession {
        id: t.id,
        title: clean_sidebar_title(&t.title).unwrap_or_else(|| "Terminal".to_string()),
        agent: agent_kind_to_str(t.agent).to_string(),
        cwd: t.cwd.clone(),
        created_at: t.created_at,
        model: t.model.clone(),
        mode: t.mode.clone(),
        // `store_id` is set by US-011's create-thread side-effect
        // (after inserting the `threads.db` row). Restoring a thread
        // from session without a `store_id` is legal -- the cascade
        // delete checks for `Some` before calling the store.
        store_id: t.store_id.clone(),
        // None for the legacy Agent kind so pre-Terminal-Thread
        // session.json files round-trip byte-identically.
        kind: match t.kind {
            ThreadKind::Agent => None,
            ThreadKind::Terminal => Some(THREAD_KIND_TAG_TERMINAL.to_string()),
        },
        terminal_agent: t.terminal_agent.map(|a| a.tag().to_string()),
        // US-001: persist the pin flag so a restart restores the
        // PINNED section.
        pinned: t.pinned,
        // Persist the forced Claude session id so a restart relaunches the
        // SAME session (resume + append), and the manual-rename lock so a
        // deliberate title is never re-clobbered after restore.
        session_id: t.session_id.clone(),
        title_user_set: t.title_user_set,
    }
}

/// Inverse of [`project_to_session`]. Unknown `agent` strings are
/// dropped (the thread is skipped) -- the AC says "OpenCode
/// deliberately excluded for v1", so any future tag we don't yet
/// recognise should surface as a missing thread, not a crash.
pub fn project_from_session(s: &ProjectSession) -> Project {
    let mut remaining_threads = usize::MAX;
    project_from_session_with_budget(s, &mut remaining_threads)
}

/// Budgeted inverse of [`project_to_session`] used by bootstrap restore.
pub fn project_from_session_with_budget(
    s: &ProjectSession,
    remaining_threads: &mut usize,
) -> Project {
    if s.threads.len() > MAX_RESTORED_THREADS_PER_PROJECT {
        log::warn!(
            "session restore: project `{}` has {} thread(s), capping at {MAX_RESTORED_THREADS_PER_PROJECT}",
            s.title,
            s.threads.len()
        );
    }
    let mut threads = Vec::new();
    for thread in s.threads.iter().take(MAX_RESTORED_THREADS_PER_PROJECT) {
        if *remaining_threads == 0 {
            break;
        }
        if let Some(thread) = thread_from_session(thread) {
            threads.push(thread);
            *remaining_threads = remaining_threads.saturating_sub(1);
        }
    }

    Project {
        id: s.id,
        title: s.title.clone(),
        cwd: s.cwd.clone(),
        is_expanded: s.is_expanded,
        threads,
        git_stats: crate::workspace::GitDiffStats::default(),
    }
}

pub fn thread_from_session_with_budget(
    s: &ThreadSession,
    remaining_threads: &mut usize,
) -> Option<Thread> {
    if *remaining_threads == 0 {
        return None;
    }
    let thread = thread_from_session(s)?;
    *remaining_threads = remaining_threads.saturating_sub(1);
    Some(thread)
}

/// Inverse of [`thread_to_session`]. Returns `None` on unknown agent
/// tag (forward-compat for a future v1.x where OpenCode lands).
/// Terminal-kind rows ignore the `agent` field entirely (it carries a
/// placeholder on disk for forward-compat with pre-Terminal-Thread
/// readers, see `THREAD_KIND_TAG_TERMINAL`).
pub fn thread_from_session(s: &ThreadSession) -> Option<Thread> {
    let kind = match s.kind.as_deref() {
        Some(THREAD_KIND_TAG_TERMINAL) => ThreadKind::Terminal,
        Some(_) | None => ThreadKind::Agent,
    };
    let agent = match kind {
        ThreadKind::Agent => agent_kind_from_str(&s.agent)?,
        // Terminal threads never dispatch through the agent path; the
        // placeholder keeps the struct shape uniform.
        ThreadKind::Terminal => AgentKind::ClaudeCode,
    };
    // Strip leading spinner/bullet decoration that CLI agents may
    // have baked into the title when it was last persisted (Claude
    // Code's `✻`, Codex's braille spinner, generic `●`). Falls back
    // to the raw title if cleaning yields nothing meaningful so the
    // row never restores empty.
    let title = clean_sidebar_title(&s.title).unwrap_or_else(|| "Terminal".to_string());
    Some(Thread {
        id: s.id,
        title,
        agent,
        kind,
        status: ThreadStatus::default(),
        cwd: s.cwd.clone(),
        created_at: s.created_at,
        model: s.model.clone(),
        mode: s.mode.clone(),
        store_id: s.store_id.clone(),
        terminal_agent: s
            .terminal_agent
            .as_deref()
            .and_then(crate::agent_launcher::TerminalAgent::from_tag),
        // US-001: a pre-refonte ThreadSession defaults `pinned = false`
        // via `#[serde(default)]`, so this restores cleanly.
        pinned: s.pinned,
        agent_pid: None,
        agent_proc_start: None,
        // Re-gate the restored session id through the same allow-list the
        // PTY-injection path uses: a tampered session.json must never
        // smuggle a flag-shaped value into `claude --session-id`. An
        // invalid id collapses to `None` - the thread relaunches as a fresh
        // session rather than refusing to open.
        session_id: s
            .session_id
            .clone()
            .filter(|id| crate::agent_sessions::is_valid_session_id(id)),
        title_user_set: s.title_user_set,
    })
}

/// On-disk discriminant for [`ThreadKind::Terminal`] in
/// `ThreadSession::kind`. Lives in its own constant so the round-trip
/// helpers and the affordance handlers agree on the literal.
pub const THREAD_KIND_TAG_TERMINAL: &str = "terminal";

/// Strip leading decoration glyphs and invisible characters that CLI
/// agents (Claude Code, Codex, OpenCode, Pi, Amp) bake into their
/// session / OSC titles to indicate status. Without this:
/// - During response: "● Project overview" sits in the sidebar with
///   a literal dot in front of the label.
/// - After response: a completion glyph (`✓`, `⚡`, …) or a
///   zero-width character (`U+200B`, `U+FEFF`, …) takes its place
///   and shows as a phantom margin -- `trim()` doesn't strip these
///   because they aren't whitespace per the Unicode standard, yet
///   most fonts render them with non-zero advance width.
///
/// Implementation strategy: whitelist what *can* legitimately lead a
/// human-written title (letters, digits, common opening punctuation)
/// and strip everything else from the front in one pass. That covers
/// the entire CLI-status-decoration family in a future-proof way --
/// new spinner glyphs or completion icons get caught without code
/// changes. Trailing whitespace is also normalized.
///
/// Returns `None` when nothing meaningful remains after stripping
/// (the caller treats that the same as an empty title -- the row
/// keeps its previous label rather than flashing blank).
const MAX_SIDEBAR_TITLE_CHARS: usize = 240;

pub fn clean_sidebar_title(raw: &str) -> Option<String> {
    let normalized: String = raw
        .chars()
        .map(|c| {
            if is_title_invisible_or_control(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let stripped = normalized
        .trim_start_matches(|c: char| !is_title_meaningful_lead(c))
        .trim();
    if stripped.is_empty() {
        None
    } else {
        let collapsed = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
        Some(cap_sidebar_title(&collapsed))
    }
}

fn cap_sidebar_title(title: &str) -> String {
    let mut chars = title.chars();
    let mut capped: String = chars.by_ref().take(MAX_SIDEBAR_TITLE_CHARS).collect();
    if chars.next().is_some() {
        capped.push('…');
    }
    capped
}

fn is_title_invisible_or_control(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200B}'
                | '\u{200C}'
                | '\u{200D}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

/// Whitelist of characters that can legitimately *start* a sidebar
/// thread title written by a human. Everything else (CLI status
/// glyphs, emoji, zero-width characters, format/control codepoints)
/// is treated as decoration and stripped by [`clean_sidebar_title`].
fn is_title_meaningful_lead(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            // Quotes -- ASCII + Unicode opening forms
            '"' | '\'' | '`'
            | '\u{201C}' | '\u{201D}'  // "" curly double
            | '\u{2018}' | '\u{2019}'  // '' curly single
            | '\u{00AB}' | '\u{00BB}'  // « » guillemets
            // Opening brackets / parens
            | '(' | '[' | '{'
            // Common title leads (hashtag, mention, code identifier)
            | '#' | '@' | '_'
            // Path / namespace separators
            | '/' | '\\' | '~' | '.'
            // Math / numeric leads
            | '-' | '+' | '=' | '$'
            | '\u{2013}' | '\u{2014}'  // - -
            | '\u{2212}'               // − minus sign
            // Currency
            | '\u{00A3}' | '\u{00A5}' | '\u{20AC}' // £ ¥ €
        )
}

/// Canonical string tag for an [`AgentKind`]. Stable on-disk format
/// for `ThreadSession::agent`.
pub fn agent_kind_to_str(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::ClaudeCode => "claude_code",
        AgentKind::Codex => "codex",
    }
}

pub fn agent_kind_from_str(tag: &str) -> Option<AgentKind> {
    match tag {
        "claude_code" => Some(AgentKind::ClaudeCode),
        "codex" => Some(AgentKind::Codex),
        _ => None,
    }
}

fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::thread;

    #[test]
    fn project_id_counter_is_monotonic() {
        let a = next_project_id();
        let b = next_project_id();
        let c = next_project_id();
        assert!(a < b && b < c, "got {a} {b} {c}");
    }

    #[test]
    fn thread_id_counter_is_monotonic() {
        let a = next_thread_id();
        let b = next_thread_id();
        let c = next_thread_id();
        assert!(a < b && b < c, "got {a} {b} {c}");
    }

    #[test]
    fn thread_env_id_round_trips_and_rejects_workspace_ids() {
        assert_eq!(thread_id_from_env_id(thread_env_id(1)), Some(1));
        let big = u32::MAX as u64;
        assert_eq!(thread_id_from_env_id(thread_env_id(big)), Some(big));
        // CLI-mode workspace ids live below the namespace base - they must
        // never decode as a thread, or an ai.* frame from a CLI pane would
        // drive an Agents row's spinner.
        assert_eq!(thread_id_from_env_id(0), None);
        assert_eq!(thread_id_from_env_id(1), None);
        assert_eq!(thread_id_from_env_id(AGENTS_THREAD_ENV_ID_BASE - 1), None);
    }

    #[test]
    fn thread_status_maps_from_agent_state_without_text_labels() {
        assert_eq!(
            ThreadStatus::from_agent_state(crate::ai_types::AgentState::Thinking),
            ThreadStatus::Thinking
        );
        assert_eq!(
            ThreadStatus::from_agent_state(crate::ai_types::AgentState::Stalled),
            ThreadStatus::Thinking
        );
        assert_eq!(
            ThreadStatus::from_agent_state(crate::ai_types::AgentState::WaitingForInput),
            ThreadStatus::WaitingForInput
        );
        assert_eq!(
            ThreadStatus::from_agent_state(crate::ai_types::AgentState::Finished),
            ThreadStatus::Idle
        );
        assert_eq!(
            ThreadStatus::from_agent_state(crate::ai_types::AgentState::Errored),
            ThreadStatus::Failed
        );
    }

    // AC: monotonic ID atomicity across 1000 calls. We probe both
    // counters concurrently from many threads and assert (a) no
    // duplicates, (b) the full range was issued. The shared counter
    // in `super` is process-wide so other tests in the same binary
    // may have already advanced it; we work with the IDs we see, not
    // with a fixed range.
    #[test]
    fn project_id_atomic_no_duplicates_under_contention() {
        check_no_duplicates(next_project_id, 1_000);
    }

    #[test]
    fn thread_id_atomic_no_duplicates_under_contention() {
        check_no_duplicates(next_thread_id, 1_000);
    }

    fn check_no_duplicates(make_id: fn() -> u64, total: usize) {
        let issued = Arc::new(std::sync::Mutex::new(Vec::with_capacity(total)));
        let threads_n = 8;
        let per_thread = total / threads_n;

        let handles: Vec<_> = (0..threads_n)
            .map(|_| {
                let issued = Arc::clone(&issued);
                thread::spawn(move || {
                    let mut local = Vec::with_capacity(per_thread);
                    for _ in 0..per_thread {
                        local.push(make_id());
                    }
                    issued.lock().unwrap().extend(local);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let mut ids = issued.lock().unwrap().clone();
        let observed = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), observed, "duplicate IDs issued under contention");
    }

    #[test]
    fn project_roundtrip_through_session_shape() {
        let mut proj = Project::new("Paneflow", "/home/me/dev/paneflow");
        proj.threads.push(Thread::new(
            "First thread",
            AgentKind::ClaudeCode,
            &proj.cwd,
        ));
        proj.threads
            .push(Thread::new("Second thread", AgentKind::Codex, &proj.cwd));
        proj.is_expanded = false;

        let session = project_to_session(&proj);
        assert_eq!(session.id, proj.id);
        assert_eq!(session.threads.len(), 2);
        assert_eq!(session.threads[0].agent, "claude_code");
        assert_eq!(session.threads[1].agent, "codex");
        assert!(!session.is_expanded);

        let restored = project_from_session(&session);
        assert_eq!(restored.id, proj.id);
        assert_eq!(restored.title, proj.title);
        assert_eq!(restored.threads.len(), 2);
        assert_eq!(restored.threads[0].agent, AgentKind::ClaudeCode);
        assert_eq!(restored.threads[1].agent, AgentKind::Codex);
        // Status always restores Idle regardless of pre-save value.
        assert_eq!(restored.threads[0].status, ThreadStatus::Idle);
    }

    #[test]
    fn unknown_agent_tag_is_skipped_not_panicking() {
        let session = ProjectSession {
            id: 1,
            title: "P".to_string(),
            cwd: "/tmp".to_string(),
            is_expanded: true,
            threads: vec![ThreadSession {
                id: 1,
                title: "Future thread".to_string(),
                agent: "opencode".to_string(), // not in AgentKind yet
                cwd: "/tmp".to_string(),
                created_at: 0,
                model: None,
                mode: None,
                store_id: None,
                kind: None,
                terminal_agent: None,
                pinned: false,
                session_id: None,
                title_user_set: false,
            }],
        };
        let restored = project_from_session(&session);
        // The unknown thread is silently dropped (forward-compat).
        assert!(restored.threads.is_empty());
    }

    #[test]
    fn project_restore_respects_thread_budget() {
        let threads = (0..3)
            .map(|id| ThreadSession {
                id,
                title: format!("Thread {id}"),
                agent: "claude_code".to_string(),
                cwd: "/tmp".to_string(),
                created_at: 0,
                model: None,
                mode: None,
                store_id: None,
                kind: None,
                terminal_agent: None,
                pinned: false,
                session_id: None,
                title_user_set: false,
            })
            .collect();
        let session = ProjectSession {
            id: 1,
            title: "P".to_string(),
            cwd: "/tmp".to_string(),
            is_expanded: true,
            threads,
        };
        let mut remaining = 2;

        let restored = project_from_session_with_budget(&session, &mut remaining);

        assert_eq!(restored.threads.len(), 2);
        assert_eq!(remaining, 0);
    }

    /// US-001: the `pinned` flag survives a thread -> session -> thread
    /// round-trip, and a session shape without the flag restores `false`.
    #[test]
    fn pinned_flag_round_trips_through_session() {
        let mut thread = Thread::new_terminal(
            "Pinned",
            "/home/me",
            Some(crate::agent_launcher::TerminalAgent::ClaudeCode),
        );
        assert!(!thread.pinned, "fresh threads start unpinned");
        thread.pinned = true;
        let session = thread_to_session(&thread);
        assert!(session.pinned, "pin flag persists into the session shape");
        let restored = thread_from_session(&session).expect("terminal thread restores");
        assert!(restored.pinned, "pin flag restores from the session shape");
    }

    #[test]
    fn claude_thread_mints_session_id_others_do_not() {
        let claude = Thread::new_terminal(
            "c",
            "/home/me",
            Some(crate::agent_launcher::TerminalAgent::ClaudeCode),
        );
        let id = claude.session_id.as_deref().expect("Claude mints a uuid");
        assert!(
            crate::agent_sessions::is_valid_session_id(id),
            "minted id {id} must pass the PTY allow-list"
        );

        let codex = Thread::new_terminal(
            "x",
            "/home/me",
            Some(crate::agent_launcher::TerminalAgent::Codex),
        );
        assert!(
            codex.session_id.is_none(),
            "only forced-session-id agents mint an id"
        );
        let shell = Thread::new_terminal("s", "/home/me", None);
        assert!(shell.session_id.is_none());
    }

    #[test]
    fn session_id_and_rename_lock_round_trip() {
        let mut thread = Thread::new_terminal(
            "Backfilled",
            "/home/me",
            Some(crate::agent_launcher::TerminalAgent::ClaudeCode),
        );
        thread.title_user_set = true;
        let session = thread_to_session(&thread);
        let restored = thread_from_session(&session).expect("terminal thread restores");
        assert_eq!(restored.session_id, thread.session_id, "forced id persists");
        assert!(restored.title_user_set, "manual-rename lock persists");
    }

    #[test]
    fn tampered_session_id_is_dropped_on_restore() {
        // A flag-shaped id smuggled into session.json collapses to None so
        // it can never reach `claude --session-id`.
        let mut thread = Thread::new_terminal(
            "T",
            "/home/me",
            Some(crate::agent_launcher::TerminalAgent::ClaudeCode),
        );
        thread.session_id = Some("--dangerously-skip-permissions".to_string());
        let session = thread_to_session(&thread);
        let restored = thread_from_session(&session).expect("restores");
        assert!(
            restored.session_id.is_none(),
            "a flag-shaped session id must not survive restore"
        );
    }

    #[test]
    fn sidebar_titles_are_cleaned_and_bounded_before_persisting() {
        let long = "x".repeat(MAX_SIDEBAR_TITLE_CHARS + 8);
        let raw = format!("● hello\n\u{202E}world {long}");

        let cleaned = clean_sidebar_title(&raw).expect("title should survive cleanup");

        assert!(cleaned.starts_with("hello world "));
        assert!(!cleaned.contains('\n'));
        assert!(!cleaned.contains('\u{202E}'));
        assert_eq!(cleaned.chars().count(), MAX_SIDEBAR_TITLE_CHARS + 1);
        assert!(cleaned.ends_with('…'));

        let mut thread = Thread::new_terminal(raw, "/home/me", None);
        thread.title_user_set = true;
        let session = thread_to_session(&thread);
        assert_eq!(session.title, cleaned);
    }

    /// US-002: free chats draw IDs from the same `next_thread_id` counter as
    /// project threads, so `bump_id_counters_to` MUST fold chats into the
    /// thread-ID max. After a restore the next ID is `max(all IDs) + 1`,
    /// even when the highest ID belongs to a chat, not a project thread.
    #[test]
    fn bump_id_counters_covers_chats() {
        // A project thread at a low ID and a chat at a high ID. The chat's
        // ID must drive the counter, otherwise the next thread collides.
        let mut project = Project::new("P", "/tmp");
        let mut low = Thread::new_terminal("t", "/tmp", None);
        low.id = 1_000_000;
        project.threads.push(low);
        let mut chat = Thread::new_terminal("c", "/home/me", None);
        chat.id = 9_000_000;
        let chats = vec![chat];

        bump_id_counters_to(std::slice::from_ref(&project), &chats);
        let next = next_thread_id();
        assert!(
            next > 9_000_000,
            "next_thread_id ({next}) must exceed the highest restored chat ID"
        );
    }

    #[test]
    fn bump_id_counters_advances_past_restored_max() {
        let raw = AtomicU64::new(5);
        bump_counter(&raw, 10);
        assert_eq!(raw.load(Ordering::Relaxed), 10);
        // No regression: bumping to a smaller value is a no-op.
        bump_counter(&raw, 7);
        assert_eq!(raw.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn agent_kind_tags_are_bijective() {
        for kind in AgentKind::all() {
            let tag = agent_kind_to_str(kind);
            assert_eq!(agent_kind_from_str(tag), Some(kind), "round-trip {tag}");
        }
        assert_eq!(agent_kind_from_str("nope"), None);
    }
}
