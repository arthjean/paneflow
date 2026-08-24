use super::layout::LayoutNode;
use serde::{Deserialize, Serialize};

/// Current on-disk schema version for [`SessionState`].
pub const SESSION_SCHEMA_VERSION: u32 = 1;

/// Top-level UI mode (US-007/US-008 of `prd-agents-view.md`;
/// `Diff` added by US-001 of `prd-git-diff-mode-2026-Q3.md`).
///
/// `Cli` is the traditional terminal-multiplexer view. `Diff` is the
/// dedicated git/worktree diff surface (left git panel + diff area).
/// `Agents` is the Agents view (project + thread sidebar + chat thread).
/// Default is `Cli` so existing users see no behaviour change on first
/// launch after upgrading. Variant order mirrors the on-screen segment
/// order (CLI / Diff / Agents) in `render_mode_toggle`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppMode {
    #[default]
    Cli,
    Diff,
    Agents,
}

/// Stable selection target for the Agents view center surface.
///
/// The runtime target uses vector indices because the UI mutates those
/// collections directly. The persisted shape stores stable IDs instead so a
/// session restore after project/thread capping or reordering never points at
/// the wrong row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentsTargetSession {
    /// A thread inside the project with `project_id`.
    Thread { project_id: u64, thread_id: u64 },
    /// A free chat thread.
    Chat { thread_id: u64 },
}

/// Persisted session state written to `~/.cache/paneflow/session.json`.
///
/// Backward-compat note: the three Agents-view fields (`projects`,
/// `active_project`, `mode`) all carry `#[serde(default)]`. Loading a
/// session.json written by a pre-US-007 build deserialises cleanly --
/// the missing keys resolve to an empty project list and `AppMode::Cli`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionState {
    /// Schema version for forward-compatible migrations.
    pub version: u32,
    /// Index of the active workspace at save time.
    pub active_workspace: usize,
    /// Ordered list of workspace snapshots.
    pub workspaces: Vec<WorkspaceSession>,
    /// Ordered list of project snapshots for the Agents view.
    /// US-007 of `tasks/prd-agents-view.md`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<ProjectSession>,
    /// Index of the active project at save time. `0` when no projects
    /// exist (the sidebar treats `projects.is_empty()` as the empty
    /// state regardless of this value).
    #[serde(default)]
    pub active_project: usize,
    /// Free chats - terminal threads not attached to any project, anchored
    /// on the user's home dir (US-002 of
    /// the Agents UI redesign). A separate list from
    /// `projects` by design (no implicit "~" project). `skip_serializing_if`
    /// mirrors the `projects` field, so a pre-refonte session.json
    /// (without this key) restores as an empty chat list without touching
    /// the project serialization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chats: Vec<ThreadSession>,
    /// Last selected Agents-view center target. Stored by stable IDs rather
    /// than indices so restore can remap through capped/filtered project and
    /// chat lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents_target: Option<AgentsTargetSession>,
    /// Last UI mode the user was in. The bootstrap reads this to
    /// reopen the Agents view if it was active at quit time (US-009).
    #[serde(default)]
    pub mode: AppMode,
    /// US-015 (prd-git-diff-mode-2026-Q3.md): the Git Diff view scope at save
    /// time, snake_case (`"project"` / `"multi_project"` / `"worktree"`),
    /// restored into `AppMode::Diff` on boot when reconstructable. Stored as a
    /// string so this config crate stays independent of the app's `DiffScope`
    /// type. Absent / `None` on sessions written before this field - defaults
    /// to the app's `DiffScope::default()` (Project).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_scope: Option<String>,
}

/// Persisted shape of one `Project` (the runtime type
/// lives in `src-app/src/project/mod.rs`). The `id` is the in-memory
/// monotonic counter at save time -- it is restored on load so the
/// counter stays monotonic across restarts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectSession {
    /// Monotonic in-memory ID at save time.
    pub id: u64,
    /// Human-readable title (sidebar header).
    pub title: String,
    /// Root cwd for new threads in this project.
    pub cwd: String,
    /// Whether the sidebar header was expanded at save time. `true`
    /// is the default for backward-compat (a missing key restores as
    /// "expanded" so an old session.json doesn't ghost the threads).
    #[serde(default = "default_true")]
    pub is_expanded: bool,
    /// Ordered list of thread snapshots in this project.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<ThreadSession>,
}

/// Persisted shape of one thread (the runtime type lives in
/// `src-app/src/project/mod.rs`). Thread *content* (messages, tool
/// calls, attachments) is NOT stored here -- that lives in the
/// `paneflow-threads` SQLite DB (US-006). This struct holds only the
/// sidebar-visible metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadSession {
    /// Monotonic in-memory ID at save time.
    pub id: u64,
    /// Human-readable title (sidebar row).
    pub title: String,
    /// Legacy wire-format agent tag. Pre-Terminal-Thread sessions use
    /// `"claude_code"` or `"codex"` to recover the launcher. Modern sessions
    /// retain the field for backward compatibility while `terminal_agent`
    /// carries the canonical runtime tag.
    pub agent: String,
    /// Per-thread cwd. May differ from the parent project's cwd if the
    /// user explicitly forked into a subdirectory.
    pub cwd: String,
    /// Creation timestamp (unix-epoch milliseconds UTC). Used by the
    /// sidebar for relative-time labels.
    pub created_at: u64,
    /// Last selected model name from the agent's `session/new` response.
    /// `None` means "use the agent's default".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Last selected ACP mode (Claude: `default`/`acceptEdits`/`plan`...).
    /// `None` means "use the agent's default".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Foreign key into the `paneflow-threads` SQLite DB. `None` for
    /// threads that have never been persisted (the runtime layer sets
    /// this on first `append_message`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    /// Persistence discriminant introduced with Terminal Threads. `None` marks
    /// a legacy ACP row; the app migrates it immediately into the terminal
    /// runtime model using `agent`. Modern writers emit `"terminal"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Which CLI coding agent a Terminal Thread launches on first mount.
    /// Valid values are runtime `TerminalAgent::tag()` strings such as
    /// `"claude_code"`, `"codex"`, `"opencode"`, `"pi"`, `"hermes"`,
    /// `"grok"`, `"cursor"`, `"gemini"`, `"kiro"`, and other launcher tags.
    /// Drives the sidebar row icon and the auto-run command. `None`
    /// restores as a bare shell for modern rows. Stored as a tag string so this crate
    /// stays free of the runtime `TerminalAgent` enum (which lives in
    /// `src-app`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_agent: Option<String>,
    /// Whether the user pinned this thread (US-001 of
    /// the Agents UI redesign). Pinned threads are
    /// surfaced in the rail's PINNED section across both projects and
    /// free chats. `#[serde(default)]` so a session.json written before
    /// this field deserialises cleanly as `false` - no migration.
    #[serde(default)]
    pub pinned: bool,
    /// Forced agent session UUID for a Claude Terminal Thread, passed as
    /// `claude --session-id <uuid>` on launch so the thread binds 1:1 to
    /// its on-disk session file (`~/.claude/projects/<slug>/<uuid>.jsonl`).
    /// Persisting it means a restart relaunches the SAME session (Claude
    /// resumes + appends on an existing id) and the sidebar can backfill
    /// that session's LLM `ai-title`. `None` for agents that don't support
    /// a forced id (everything but Claude) and pre-feature sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Whether the user manually renamed this row. Once set, OSC title
    /// updates and the `ai-title` backfill stop overwriting the label so a
    /// deliberate name is never clobbered by agent activity. `#[serde(default)]`
    /// so older session.json files restore as `false`.
    #[serde(default)]
    pub title_user_set: bool,
}

fn default_true() -> bool {
    true
}

/// Snapshot of a single workspace for session persistence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceSession {
    /// Workspace display title.
    pub title: String,
    /// Root working directory of the workspace.
    pub cwd: String,
    /// Layout tree (splits + panes). `None` means a single default pane.
    pub layout: Option<LayoutNode>,
    /// User-defined command buttons rendered in this workspace's tab bar.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_buttons: Vec<ButtonCommand>,
    /// Workspace-relative directory paths expanded in the Files tree sidebar
    /// (PRD files-tree US-007). Additive + optional: absent in older
    /// `session.json` files, which deserialize to an empty list and never
    /// break restore of the other fields. The sidebar's open/closed state is
    /// deliberately NOT persisted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded_paths: Vec<String>,
    /// Git worktrees Paneflow created for this workspace via `paneflow up`
    /// (EP-002, prd-orchestration-v2). Persisted so a crash/restart keeps the
    /// ownership record (teardown at close, `git worktree prune` at startup).
    /// Additive + optional like `expanded_paths`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub managed_worktrees: Vec<ManagedWorktreeDef>,
}

/// A git worktree created (and therefore owned) by Paneflow for one pane of a
/// `paneflow up` workspace. Paths are stored absolute; `teardown` is `"auto"`
/// (remove at close when clean) or `"keep"`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ManagedWorktreeDef {
    /// Worktree checkout directory.
    pub path: String,
    /// Main repository root (where `git worktree` commands run).
    pub repo_root: String,
    /// Branch checked out in the worktree (diagnostics only - never deleted).
    pub branch: String,
    /// Teardown policy: `"auto"` | `"keep"`. Unknown values read as `"auto"`;
    /// the data-loss protection is the clean-check, not this flag.
    pub teardown: String,
}

/// A user-defined command button rendered in a workspace's tab bar.
/// Clicking the button sends `{command}\r` to the active terminal.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ButtonCommand {
    /// Stable identifier (opaque string) - survives reorderings and renames.
    pub id: String,
    /// Display name (also used as hover tooltip).
    pub name: String,
    /// Icon asset path relative to the `assets/` folder (e.g. `"icons/rocket.svg"`).
    pub icon: String,
    /// Shell command string, executed verbatim in the active terminal
    /// with a trailing `\r` appended (no bracketed-paste wrapping).
    pub command: String,
}
