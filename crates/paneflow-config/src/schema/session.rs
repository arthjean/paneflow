use super::layout::{default_layout_pane, LayoutNode, SurfaceDefinition};
use serde::{Deserialize, Serialize};

/// Current on-disk schema version for [`SessionState`].
///
/// Bumped to 2 by US-018 (`prd-cli-tab-hierarchy-2026-Q3`), when the workspace
/// stopped owning one layout tree and started owning a list of tabs. A v1 file
/// is migrated by [`migrate_session_v1`] rather than rejected; see
/// [`SESSION_SCHEMA_VERSION_V1`].
pub const SESSION_SCHEMA_VERSION: u32 = 2;

/// The one legacy on-disk version Paneflow still reads: a workspace with a
/// single `layout` and the EP-003 `empty` marker, both migrated into
/// [`WorkspaceSession::tabs`].
pub const SESSION_SCHEMA_VERSION_V1: u32 = 1;

/// Cap on the tabs one migrated workspace may carry. Mirrors the app's live
/// `MAX_TABS_PER_WORKSPACE` (`src-app/src/workspace/mod.rs`), the same
/// read-cap / write-cap pairing [`super::layout::MAX_LAYOUT_LEAVES`] has
/// with the app's `MAX_PANES`: the write side refuses to open the 33rd tab,
/// so the read side must refuse to restore one.
pub const MAX_SESSION_TABS: usize = 32;

/// Top-level UI mode (US-007/US-008 of `prd-agents-view.md`;
/// `Diff` added by US-001 of `prd-git-diff-mode-2026-Q3.md`).
///
/// `Cli` is the traditional terminal-multiplexer view. `Diff` is the
/// dedicated git/worktree diff surface (left git panel + diff area).
/// `Agents` is the Agents view (project + thread sidebar + chat thread).
/// Default is `Cli` so existing users see no behaviour change on first
/// launch after upgrading. Variant order mirrors the on-screen tab order
/// (CLI / Review / Agents) in the shared sidebar footer.
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

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if hands the field by reference"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// Snapshot of one workspace tab: a title and the pane layout that tab owns
/// (US-018). `layout: None` is an *empty* tab - a folder with no pane at all -
/// which is only ever written by v2. The v1 `layout: null` meant the opposite
/// ("one default pane"), which is why the migration materializes that pane
/// explicitly instead of carrying the ambiguity forward.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TabSession {
    /// User-facing tab title. Empty means unnamed - the sidebar derives a
    /// fallback label rather than persisting one.
    #[serde(default)]
    pub title: String,
    /// Pane layout tree for this tab. `None` = the tab holds no pane.
    #[serde(default)]
    pub layout: Option<LayoutNode>,
}

impl TabSession {
    /// A tab holding no pane.
    pub fn empty() -> Self {
        Self::default()
    }

    /// An unnamed tab holding `layout`.
    pub fn with_layout(layout: LayoutNode) -> Self {
        Self {
            title: String::new(),
            layout: Some(layout),
        }
    }
}

/// Snapshot of a single workspace for session persistence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceSession {
    /// Workspace display title.
    pub title: String,
    /// Root working directory of the workspace.
    pub cwd: String,
    /// The workspace's tabs, in display order (US-018). Never written empty by
    /// Paneflow: a workspace always keeps one tab (FR-01), an empty folder
    /// being a single [`TabSession::empty`]. A v1 file has no such key, which
    /// is exactly what [`migrate_session_v1`] keys off.
    #[serde(default)]
    pub tabs: Vec<TabSession>,
    /// Index into [`WorkspaceSession::tabs`] of the tab visible at save time.
    /// Clamped at restore; this is a *persistence* index, never an address -
    /// no IPC or MCP surface exposes it (FR-07).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub active_tab: usize,
    /// v1 only: the single layout tree a workspace used to own. Always `None`
    /// once [`migrate_session_v1`] has run, so v2 never writes the key.
    #[serde(rename = "layout", default, skip_serializing_if = "Option::is_none")]
    pub legacy_layout: Option<LayoutNode>,
    /// v1 only (EP-003): the workspace deliberately held no pane, the marker
    /// that separated that state from the legacy `layout: null` meaning "one
    /// default pane". Superseded by an empty [`TabSession`]; always `false`
    /// once the migration has run, so v2 never writes the key.
    #[serde(rename = "empty", default, skip_serializing_if = "is_false")]
    pub legacy_empty: bool,
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

/// Migrate a v1 `session.json` in place to the v2 tab shape (US-018).
///
/// v1 gave each workspace one layout tree whose panes could each stack several
/// surfaces. v2 gives it a list of tabs whose panes hold exactly one surface,
/// so the extra surfaces have to go somewhere: the tree becomes the first tab
/// with every pane reduced to its focused surface, and each surface left over
/// is promoted - in traversal order - to its own single-pane tab named after
/// that surface. No surface is dropped, except past [`MAX_SESSION_TABS`],
/// which is logged with its count rather than swallowed.
///
/// Idempotent: a workspace that already carries `tabs` is left alone, and the
/// two legacy fields are always drained so a re-save writes pure v2.
pub fn migrate_session_v1(state: &mut SessionState) {
    for ws in &mut state.workspaces {
        migrate_workspace_v1(ws);
    }
    state.version = SESSION_SCHEMA_VERSION;
}

fn migrate_workspace_v1(ws: &mut WorkspaceSession) {
    let legacy_empty = std::mem::take(&mut ws.legacy_empty);
    let legacy_layout = ws.legacy_layout.take();
    if !ws.tabs.is_empty() {
        // Hand-written or already-migrated file: the tab list wins over the
        // legacy keys, which have just been drained.
        return;
    }
    let Some(mut root) = legacy_layout else {
        // v1 `layout: null` meant "one default pane"; the EP-003 `empty`
        // marker was the only way to say "no pane at all".
        ws.tabs.push(if legacy_empty {
            TabSession::empty()
        } else {
            TabSession::with_layout(default_layout_pane())
        });
        return;
    };
    let mut promoted = Vec::new();
    demote_panes_to_focused_surface(&mut root, &mut promoted);
    ws.tabs.push(TabSession::with_layout(root));
    ws.tabs.append(&mut promoted);
    if ws.tabs.len() > MAX_SESSION_TABS {
        let dropped = ws.tabs.len() - MAX_SESSION_TABS;
        tracing::warn!(
            workspace = %ws.title,
            dropped,
            cap = MAX_SESSION_TABS,
            "session v1 migration: workspace exceeds the tab cap, surplus tabs dropped"
        );
        ws.tabs.truncate(MAX_SESSION_TABS);
    }
    // The v1 file has no notion of an active tab, and the tree the user was
    // looking at is always the first one.
    ws.active_tab = 0;
}

/// Depth-first, left to right: keep each pane's focused surface where it is
/// and hand every other surface back as its own tab, in the order the panes
/// (and the surfaces inside them) appear in the tree.
fn demote_panes_to_focused_surface(node: &mut LayoutNode, promoted: &mut Vec<TabSession>) {
    match node {
        LayoutNode::Pane { surfaces } => {
            if surfaces.is_empty() {
                surfaces.push(SurfaceDefinition::default());
                return;
            }
            let focused = surfaces
                .iter()
                .position(|s| s.focus == Some(true))
                .unwrap_or(0);
            let mut drained: Vec<SurfaceDefinition> = std::mem::take(surfaces);
            surfaces.push(drained.remove(focused));
            for surface in drained {
                let title = surface_title(&surface);
                promoted.push(TabSession {
                    title,
                    layout: Some(LayoutNode::Pane {
                        surfaces: vec![surface],
                    }),
                });
            }
        }
        LayoutNode::Split { children, .. } => {
            for child in children.iter_mut() {
                demote_panes_to_focused_surface(child, promoted);
            }
        }
    }
}

fn surface_title(surface: &SurfaceDefinition) -> String {
    surface
        .custom_name
        .as_deref()
        .or(surface.name.as_deref())
        .unwrap_or_default()
        .to_string()
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
