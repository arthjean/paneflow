//! Workspace - a named collection of terminal panes with a split layout.
//!
//! Module layout (US-030 of the src-app refactor PRD):
//! - [`git`] - git metadata probing (branch, diff stats, `.git` dir lookup)
//! - [`ports`] - cross-platform TCP listening-port detection
//!
//! The [`Workspace`] struct and its constructors live in this `mod.rs`; git
//! and port helpers are re-exported so external callers keep the flat
//! `crate::workspace::*` API.

mod git;
pub mod pid_resolve;
mod ports;
pub mod surface_naming;
pub mod worktree;

pub use git::{GitDiffStats, detect_branch, find_git_dir, resolve_repo_root};
#[cfg(test)]
pub(crate) use ports::PortEntry;
pub use ports::{PaneScan, scan_panes};

/// Hard cap on open workspaces (US-054: single source for the bound previously
/// re-declared as a local `const` at every create/IPC site).
pub(crate) const MAX_WORKSPACES: usize = 20;

use gpui::{App, Entity, Window};
use paneflow_config::schema::{ButtonCommand, LayoutNode};

use crate::ai_types::AgentSession;
use crate::launch_cwd;
use crate::layout::LayoutTree;
use crate::pane::Pane;

use self::git::parse_head;

/// Monotonic workspace ID counter. Each workspace gets a unique ID at construction.
static NEXT_WORKSPACE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn next_workspace_id() -> u64 {
    NEXT_WORKSPACE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub struct Workspace {
    /// Unique workspace identifier, assigned at construction.
    pub id: u64,
    pub title: String,
    /// Working directory at creation time. Does not update when the shell `cd`s.
    pub cwd: String,
    pub root: Option<LayoutTree>,
    /// Saved layout tree when zoomed. `Some(tree)` means the workspace is zoomed
    /// and `root` contains only the zoomed pane as a single Leaf.
    pub saved_layout: Option<LayoutTree>,
    /// Cached git diff stats, refreshed by a background poller.
    pub git_stats: GitDiffStats,
    /// Current git branch name. Empty string when not a git repo or branch unknown.
    pub git_branch: String,
    /// Whether this workspace's CWD is inside a git repository.
    pub is_git_repo: bool,
    /// Resolved `.git` directory path (for file watcher). `None` if not a git repo.
    pub git_dir: Option<std::path::PathBuf>,
    /// Working directory of the shared repository (parent of the *main* `.git`),
    /// canonicalized. Sibling worktrees of one repo share an identical value -
    /// the invariant the sidebar uses to group them. `None` when not a git repo.
    pub repo_root: Option<std::path::PathBuf>,
    /// Whether this workspace's CWD is a *linked* git worktree (as opposed to
    /// the repo's main checkout). Linked worktrees carry a `commondir` file.
    // Read by EP-002 (US-005) to target git operations at the worktree root and
    // by EP-004 column labeling; stored at construction in EP-001 (US-001).
    #[allow(dead_code)]
    pub is_worktree: bool,
    /// Active TCP listening ports from workspace terminal processes.
    pub active_ports: Vec<u16>,
    /// Generation counter for event-driven port scans - the cancellation
    /// belt for workspace close/reuse (superseded scans check it to abort).
    pub port_scan_generation: u64,
    /// True while a scan ladder (debounce + retries) is in flight for this
    /// workspace - ActivityBursts arriving meanwhile are absorbed instead of
    /// superseding the pending scan (under sustained output, the old
    /// generation-bump-per-burst starved the 500ms debounce indefinitely).
    pub port_scan_pending: bool,
    /// Service metadata for `active_ports` chips, fed from BOTH sides:
    /// OS-side argv classification (authoritative for `is_frontend`, with a
    /// synthesized localhost URL) and PTY-output detection (enrichment -
    /// exact URL with path, backend labels). Keyed by port number; pruned
    /// when ports are removed from `active_ports`.
    pub service_labels: std::collections::HashMap<u16, crate::terminal::ServiceInfo>,
    /// Registered AI agent sessions for this workspace, keyed by PID. A
    /// workspace can hold many concurrent sessions (e.g., two Claude
    /// Codes + one Codex) - the sidebar aggregates them per tool with
    /// `ai_types::aggregate_by_tool`. Cleaned up by the stale-PID sweep
    /// in `event_handlers::sweep_stale_pids`.
    pub agent_sessions: std::collections::HashMap<u32, AgentSession>,
    /// AI agent process basenames detected by walking the workspace's
    /// PTY descendants (Linux `/proc/<pid>/comm`, macOS `libproc::name`).
    /// Independent of the optional IPC hook handshake -- this is what
    /// the sidebar pastille reads so the "session active" signal works
    /// even when Claude Code is launched without the Paneflow shim.
    /// Refreshed by the per-pane `scan_panes` walk (EP-005 US-012) - the
    /// union of every pane's detected agents; the recognition vocabulary
    /// is `TerminalAgent::ALL` binaries (16), unified from the historical
    /// 3-name `AI_PROCESS_NAMES` list.
    pub detected_agents: std::collections::HashSet<String>,
    /// User-defined tab-bar buttons for this workspace.
    /// Rendered after the 2 built-in defaults (Claude / Codex).
    pub custom_buttons: Vec<ButtonCommand>,
    /// Absolute directory paths expanded in the Files tree sidebar, held
    /// per-workspace so reopening the sidebar (within a session or after a
    /// restart) restores the same expansion (PRD files-tree US-007). Excludes
    /// the implicit root. Persisted as workspace-relative paths in
    /// `session.json`; the sidebar's visibility itself is never persisted.
    pub files_expanded: Vec<std::path::PathBuf>,
    /// Git worktrees Paneflow created for this workspace's panes via
    /// `paneflow up` (`worktree = "branch"`, EP-002 orchestration-v2). Torn
    /// down - clean ones only, branch never deleted - when the workspace
    /// closes; persisted in `session.json` so a crash keeps the ownership
    /// record. Empty for every workspace not built by `up` with worktrees.
    pub managed_worktrees: Vec<worktree::ManagedWorktree>,
}

impl Workspace {
    /// US-013: shared private factory for the three public constructors (kills
    /// the verbatim triplication). Resolves the *cheap* git metadata - `.git`
    /// dir, branch (`parse_head`), repo root - synchronously, since those are
    /// direct `.git/HEAD` file reads, not subprocesses. `git_stats` is left at
    /// its `default()` (0/0): the `git diff --shortstat` subprocess is the
    /// blocking call, deferred off the render thread by
    /// [`crate::PaneFlowApp::spawn_initial_git_stats`] right after creation.
    fn build(id: u64, title: String, cwd: String, root: LayoutTree) -> Self {
        let git_dir = find_git_dir(&cwd);
        let (git_branch, is_git_repo) = match &git_dir {
            Some(dir) => parse_head(dir),
            None => (String::new(), false),
        };
        let (repo_root, is_worktree) = match &git_dir {
            Some(dir) => resolve_repo_root(dir),
            None => (None, false),
        };
        Self {
            id,
            title,
            cwd,
            root: Some(root),
            saved_layout: None,
            git_stats: GitDiffStats::default(),
            git_branch,
            is_git_repo,
            git_dir,
            repo_root,
            is_worktree,
            active_ports: vec![],
            port_scan_generation: 0,
            port_scan_pending: false,
            service_labels: std::collections::HashMap::new(),
            agent_sessions: std::collections::HashMap::new(),
            detected_agents: std::collections::HashSet::new(),
            custom_buttons: Vec::new(),
            files_expanded: Vec::new(),
            managed_worktrees: Vec::new(),
        }
    }

    /// Create a workspace with a pre-allocated ID (use `next_workspace_id()` to obtain one).
    pub fn with_id(id: u64, title: impl Into<String>, pane: Entity<Pane>) -> Self {
        let cwd = launch_cwd::implicit_launch_cwd().display().to_string();
        Self::build(id, title.into(), cwd, LayoutTree::Leaf(pane))
    }

    /// Create a workspace with a pre-allocated ID and explicit CWD.
    pub fn with_cwd_and_id(
        id: u64,
        title: impl Into<String>,
        cwd: std::path::PathBuf,
        pane: Entity<Pane>,
    ) -> Self {
        Self::build(
            id,
            title.into(),
            cwd.display().to_string(),
            LayoutTree::Leaf(pane),
        )
    }

    /// Create a workspace with a pre-allocated ID and layout tree.
    pub fn with_layout_and_id(
        id: u64,
        title: impl Into<String>,
        cwd: std::path::PathBuf,
        root: LayoutTree,
    ) -> Self {
        Self::build(id, title.into(), cwd.display().to_string(), root)
    }

    pub fn is_zoomed(&self) -> bool {
        self.saved_layout.is_some()
    }

    pub fn exit_zoom(&mut self, cx: &mut App) -> Option<Entity<Pane>> {
        let zoomed_pane = self.root.as_ref().and_then(|root| root.first_leaf());
        let saved = self.saved_layout.take()?;
        self.root = Some(saved);
        if let Some(pane) = &zoomed_pane {
            pane.update(cx, |pane, _| {
                pane.zoomed = false;
            });
        }
        zoomed_pane
    }

    pub fn pane_count(&self) -> usize {
        self.root.as_ref().map_or(0, |r| r.leaf_count())
    }

    pub fn contains_pane(&self, pane: &Entity<Pane>) -> bool {
        self.root
            .as_ref()
            .is_some_and(|root| root.contains_leaf(pane))
            || self
                .saved_layout
                .as_ref()
                .is_some_and(|saved| saved.contains_leaf(pane))
    }

    pub fn any_pane(&self, mut f: impl FnMut(&Entity<Pane>) -> bool) -> bool {
        if let Some(root) = &self.root
            && root.any_leaf(&mut f)
        {
            return true;
        }
        if let Some(saved) = &self.saved_layout
            && saved.any_leaf(&mut f)
        {
            return true;
        }
        false
    }

    pub fn collect_panes(&self) -> Vec<Entity<Pane>> {
        let mut panes = Vec::new();
        if let Some(root) = &self.root {
            panes.extend(root.collect_leaves());
        }
        if let Some(saved) = &self.saved_layout {
            for pane in saved.collect_leaves() {
                if !panes.contains(&pane) {
                    panes.push(pane);
                }
            }
        }
        panes
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        if let Some(root) = &self.root {
            root.focus_first(window, cx);
        }
    }

    /// Serialize the workspace layout to a `LayoutNode`.
    ///
    /// When zoomed, serializes the saved (un-zoomed) layout so that the full
    /// pane arrangement is captured rather than just the single zoomed pane.
    pub fn serialize_layout(&self, cx: &App) -> Option<LayoutNode> {
        let tree = self.saved_layout.as_ref().or(self.root.as_ref())?;
        Some(tree.serialize(cx))
    }

    /// US-011: like [`serialize_layout`] but defers the per-terminal scrollback
    /// drain. The terminal handles are pushed into `terms` (surface-emission
    /// order) so `save_session` can drain them off the GPUI main thread.
    pub fn serialize_layout_deferred(
        &self,
        cx: &App,
        terms: &mut Vec<crate::terminal::types::SharedTerm>,
    ) -> Option<LayoutNode> {
        let tree = self.saved_layout.as_ref().or(self.root.as_ref())?;
        Some(tree.serialize_deferred(cx, terms))
    }

    /// Push the current `custom_buttons` list to every `Pane` in the
    /// workspace's layout tree so the tab bar re-renders with the new set.
    /// Call after mutating `self.custom_buttons` (add / edit / delete).
    pub fn propagate_custom_buttons(&self, cx: &mut App) {
        if let Some(root) = &self.root {
            walk_and_push_buttons(root, &self.custom_buttons, cx);
        }
        if let Some(saved) = &self.saved_layout {
            walk_and_push_buttons(saved, &self.custom_buttons, cx);
        }
    }
}

fn walk_and_push_buttons(node: &LayoutTree, buttons: &[ButtonCommand], cx: &mut App) {
    match node {
        LayoutTree::Leaf(pane) => {
            pane.update(cx, |p, cx| {
                p.custom_buttons = buttons.to_vec();
                cx.notify();
            });
        }
        LayoutTree::Container { children, .. } => {
            for child in children {
                walk_and_push_buttons(&child.node, buttons, cx);
            }
        }
    }
}

impl Workspace {
    /// US-015: push a refreshed [`PaneFlowConfig`] to every `Pane` in the
    /// workspace's layout so the tab bar re-renders against the new config
    /// without a per-frame `load_config()`. Called from
    /// `PaneFlowApp::process_config_changes` on every ConfigWatcher reload.
    pub fn propagate_config(&self, config: &paneflow_config::schema::PaneFlowConfig, cx: &mut App) {
        if let Some(root) = &self.root {
            walk_and_push_config(root, config, cx);
        }
        if let Some(saved) = &self.saved_layout {
            walk_and_push_config(saved, config, cx);
        }
    }
}

fn walk_and_push_config(
    node: &LayoutTree,
    config: &paneflow_config::schema::PaneFlowConfig,
    cx: &mut App,
) {
    match node {
        LayoutTree::Leaf(pane) => {
            pane.update(cx, |p, cx| {
                p.apply_config(config, cx);
            });
        }
        LayoutTree::Container { children, .. } => {
            for child in children {
                walk_and_push_config(&child.node, config, cx);
            }
        }
    }
}
