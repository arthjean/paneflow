//! Session persistence for `PaneFlowApp` - save/restore workspace layouts
//! and their per-pane metadata so relaunching rebuilds what the user had open.
//! Terminal scrollback stays process-local: a relaunched shell must not inherit
//! plain-text output from an earlier PTY.
//!
//! Extracted from `main.rs` per US-017 of the src-app refactor PRD.

use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext, Context, Entity};
use paneflow_config::schema::LayoutNode;

use crate::PaneFlowApp;
use crate::launch_cwd;
use crate::layout::{LayoutTree, MAX_PANES};
use crate::limits::MAX_SESSION_SIZE_BYTES;
use crate::pane::Pane;
use crate::terminal::TerminalView;
use crate::workspace::{MAX_WORKSPACES, Workspace, next_workspace_id};

/// Cap on the number of `session.json.corrupted-*` backup files retained
/// alongside the live session. Beyond this, the oldest are deleted on
/// rotation. Cf. risk R8 in `prd-stabilization-2026-q2.md`: every parse
/// failure produces a new backup, and without rotation a user with a
/// chronic corruption (e.g. a flaky disk) would silently fill `~/.cache`.
const MAX_CORRUPTION_BACKUPS: usize = 5;

/// US-011: debounce window for coalescing a burst of [`PaneFlowApp::save_session`]
/// calls into a single disk write. Short enough to be imperceptible, long
/// enough to absorb the multi-call bursts emitted when creating/closing many
/// workspaces in quick succession.
const SAVE_DEBOUNCE_MS: u64 = 150;

static SESSION_WRITE_LOCK: Mutex<()> = Mutex::new(());
static SESSION_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static SESSION_CORRUPTION_COUNTER: AtomicU64 = AtomicU64::new(0);

fn session_write_guard() -> MutexGuard<'static, ()> {
    SESSION_WRITE_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Forensic context emitted alongside a `session_corrupted` telemetry
/// event (US-006). Gathered by [`PaneFlowApp::load_session_at`] inside
/// the parse-failure branch *before* the empty fallback session is
/// returned, so the values reflect the file the user actually had on
/// disk - not the one we're about to overwrite. Stays a plain data
/// struct (no telemetry client coupling) because `load_session` runs
/// in bootstrap before the `TelemetryClient` is constructed; the
/// caller in `bootstrap.rs` defers the emit until after telemetry is
/// up.
#[derive(Debug, Clone)]
pub(crate) struct SessionCorruptionInfo {
    /// Canonical `serde_json::Error::classify()` bucket (`io | syntax | data
    /// | eof`) or a guarded-load bucket such as `oversize`, `non_regular`, or
    /// `unsupported_version`. Plain string keeps the telemetry schema fixed.
    pub(crate) error_category: &'static str,
    /// Size in bytes of the corrupted file, or `0` if the metadata
    /// call itself failed (rare - file just got read successfully).
    pub(crate) file_size: u64,
    /// Wall-clock age in seconds (mtime → now). `None` when the
    /// platform's modification-time call returns a value newer than
    /// `now` (clock drift) or the metadata call fails.
    pub(crate) file_age_seconds: Option<u64>,
    /// Resolved path of the freshly-written backup, or `None` if the
    /// backup write itself failed (AC6 - never block startup on
    /// backup-side errors).
    pub(crate) backup_path: Option<PathBuf>,
}

impl PaneFlowApp {
    /// Build the [`SessionState`] snapshot from live app state.
    ///
    /// Every persisted terminal surface emits `scrollback: None`, keeping PTY
    /// output local to the process that produced it.
    fn build_session_state(&self, cx: &App) -> paneflow_config::schema::SessionState {
        paneflow_config::schema::SessionState {
            version: paneflow_config::schema::SESSION_SCHEMA_VERSION,
            active_workspace: self.active_idx,
            workspaces: self
                .workspaces
                .iter()
                .map(|ws| paneflow_config::schema::WorkspaceSession {
                    title: ws.title.clone(),
                    cwd: ws.cwd.clone(),
                    layout: ws.serialize_layout_without_scrollback(cx),
                    custom_buttons: ws.custom_buttons.clone(),
                    // US-007: store expanded dirs relative to the workspace
                    // root. A path that can't be made relative (symlinked
                    // outside the root) is dropped rather than persisted absolute.
                    expanded_paths: persisted_expanded_paths(&ws.cwd, &ws.files_expanded),
                    // EP-002 (orchestration-v2): persist worktree ownership so
                    // a crash/restart keeps the teardown + prune record.
                    managed_worktrees: ws
                        .managed_worktrees
                        .iter()
                        .map(|wt| paneflow_config::schema::ManagedWorktreeDef {
                            path: wt.path.to_string_lossy().into_owned(),
                            repo_root: wt.repo_root.to_string_lossy().into_owned(),
                            branch: wt.branch.clone(),
                            teardown: wt.teardown.as_str().to_string(),
                        })
                        .collect(),
                })
                .collect(),
            // US-007 (prd-agents-view.md): persist project + thread
            // snapshots and the active project index. US-009 will
            // additionally persist the actual `AppMode` once
            // `PaneFlowApp` carries it.
            projects: self
                .projects
                .iter()
                .map(crate::project::project_to_session)
                .collect(),
            active_project: self.active_project_idx,
            // US-002 (Agents UI redesign): persist free
            // chats alongside projects. Empty list serializes to nothing
            // (`skip_serializing_if`), so a no-chats session.json is byte-
            // identical to a pre-refonte one.
            chats: self
                .chats
                .iter()
                .map(crate::project::thread_to_session)
                .collect(),
            agents_target: crate::project::agents_target_to_session(
                self.agents_target,
                &self.projects,
                &self.chats,
            ),
            // US-008 (prd-agents-view.md): persist the live UI mode
            // so US-009's restore branch can reopen Paneflow in the
            // same screen the user left.
            mode: self.mode,
            // US-015 (prd-git-diff-mode-2026-Q3.md): persist the diff scope so
            // a session that quit in Diff mode reopens on the same scope.
            diff_scope: Some(self.diff_mode.diff_scope.as_persisted().to_string()),
        }
    }

    /// US-011: persist the session WITHOUT blocking the GPUI main thread.
    ///
    /// The lightweight metadata snapshot is built here (render thread, cheap),
    /// then JSON serialization and the atomic write run on a background task.
    /// A burst of saves (e.g. closing 20 workspaces) is coalesced into a single
    /// write via a monotonic token + short debounce, so the most-recent snapshot
    /// wins.
    ///
    /// The quit / pre-update-install paths must use [`save_session_blocking`]
    /// instead - there the write has to land before the process exits or is
    /// replaced, so a deferred task would be lost.
    pub(crate) fn save_session(&self, cx: &App) {
        let state = self.build_session_state(cx);
        let Some(path) = paneflow_config::loader::session_path() else {
            return;
        };

        let seq = self
            .save_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let save_seq = std::sync::Arc::clone(&self.save_seq);

        cx.background_spawn(async move {
            smol::Timer::after(std::time::Duration::from_millis(SAVE_DEBOUNCE_MS)).await;
            if save_seq.load(std::sync::atomic::Ordering::SeqCst) != seq {
                // A newer save was scheduled meanwhile - let it carry the
                // latest state; skip this redundant write.
                return;
            }
            // `smol::unblock` keeps serialization and filesystem I/O off the
            // background executor's async threads.
            smol::unblock(move || {
                // Re-check inside the shared write lock. A quit-path blocking
                // save or a newer deferred save may have superseded this task
                // after its debounce check but before it reached the filesystem.
                write_session_json_if_current(&path, &state, &save_seq, seq);
            })
            .await;
        })
        .detach();
    }

    /// US-011: synchronous session save for the quit / pre-update-install
    /// paths, where a deferred background write would be lost when the process
    /// exits or is replaced.
    pub(crate) fn save_session_blocking(&self, cx: &App) {
        crate::window_state::save();
        // Cancel any in-flight deferred save: bump the coalescing token so a
        // background task still sleeping in its debounce wakes to a stale `seq`
        // and no-ops. Without this, a `save_session` fired moments before quit
        // could land its (older) snapshot *after* this final synchronous write,
        // resurrecting pre-quit state (e.g. a just-closed workspace).
        self.save_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let state = self.build_session_state(cx);
        let Some(path) = paneflow_config::loader::session_path() else {
            return;
        };
        write_session_json(&path, &state);
    }

    /// Restore a saved session from disk, or fall back silently to an
    /// empty session if anything goes wrong.
    ///
    /// Behaviour matrix (US-006):
    ///
    /// | State on disk           | Returned session  | Corruption info | Backup written |
    /// |-------------------------|-------------------|-----------------|----------------|
    /// | File missing            | `None`            | `None`          | no             |
    /// | Read error (perms, IO)  | `None`            | `Some(info)`    | no             |
    /// | Non-regular / oversize | `None`            | `Some(info)`    | no             |
    /// | Read OK + parse OK      | `Some(state)`     | `None`          | no             |
    /// | Read OK + parse FAIL    | `None` (fallback) | `Some(info)`    | yes            |
    /// | Unsupported version     | `None` (fallback) | `Some(info)`    | yes            |
    ///
    /// On parse failure the bad file is preserved as
    /// `session.json.corrupted-<unix-timestamp>` *before* the next
    /// `save_session` overwrites it, so we keep forensic evidence even
    /// when the user immediately moves on. The backup directory is
    /// rotated down to [`MAX_CORRUPTION_BACKUPS`] entries (R8) so a
    /// chronic-corruption case can't silently fill `~/.cache`.
    ///
    /// Telemetry: callers receive a `SessionCorruptionInfo` they can
    /// pass to `PaneFlowApp::emit_session_corrupted` once the
    /// telemetry client is up. The emit is consent-gated by the
    /// existing `TelemetryClient::Null` factory branch - opted-out
    /// users never produce a network call.
    pub(crate) fn load_session() -> (
        Option<paneflow_config::schema::SessionState>,
        Option<SessionCorruptionInfo>,
    ) {
        let Some(path) = paneflow_config::loader::session_path() else {
            return (None, None);
        };
        Self::load_session_at(&path)
    }

    /// Path-parametrised core of [`load_session`]. Direct test surface -
    /// the wrapper above resolves `paneflow_config::loader::session_path()`
    /// against the user's XDG cache dir, which is unsuitable for unit
    /// tests because every run would race against a live install.
    pub(crate) fn load_session_at(
        path: &Path,
    ) -> (
        Option<paneflow_config::schema::SessionState>,
        Option<SessionCorruptionInfo>,
    ) {
        // U-008/U-016: bound the read so a multi-hundred-MB hand-edited /
        // agent-written session.json (or a non-regular file swapped in) can't
        // OOM/stall the load before parse. Guard hits start from an empty
        // session, but still surface forensic context to telemetry.
        let bytes = match read_session_capped(path) {
            Ok(SessionRead::Data(d)) => d,
            Ok(SessionRead::Missing) => return (None, None),
            Ok(SessionRead::Rejected(category)) => {
                return (None, Some(session_corruption_info(path, category, None)));
            }
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("session load: read failed at {}: {e}", path.display());
                }
                return (None, Some(session_corruption_info(path, "io", None)));
            }
        };

        let data = match String::from_utf8(bytes) {
            Ok(data) => data,
            Err(err) => {
                log::warn!(
                    "session load: invalid UTF-8 at {}; falling back to empty session",
                    path.display()
                );
                let bytes = err.into_bytes();
                let backup_path = write_corruption_backup(path, &bytes).unwrap_or_else(|e| {
                    log::warn!(
                        "session load: backup write failed at {}: {e}",
                        path.display()
                    );
                    None
                });

                return (
                    None,
                    Some(session_corruption_info(path, "data", backup_path)),
                );
            }
        };

        match serde_json::from_str::<paneflow_config::schema::SessionState>(&data) {
            Ok(state) if state.version == paneflow_config::schema::SESSION_SCHEMA_VERSION => {
                (Some(state), None)
            }
            Ok(state) => {
                log::warn!(
                    "session load: unsupported schema version {} at {} (expected {}); falling back to empty session",
                    state.version,
                    path.display(),
                    paneflow_config::schema::SESSION_SCHEMA_VERSION
                );
                let backup_path =
                    write_corruption_backup(path, data.as_bytes()).unwrap_or_else(|e| {
                        log::warn!(
                            "session load: backup write failed at {}: {e}",
                            path.display()
                        );
                        None
                    });
                (
                    None,
                    Some(session_corruption_info(
                        path,
                        "unsupported_version",
                        backup_path,
                    )),
                )
            }
            Err(parse_err) => {
                log::warn!(
                    "session load: parse failed at {} ({}); falling back to empty session",
                    path.display(),
                    parse_err
                );

                let backup_path =
                    write_corruption_backup(path, data.as_bytes()).unwrap_or_else(|e| {
                        // AC6: backup write failure must not block startup.
                        // Log and proceed - telemetry still fires with a
                        // `backup_path: None`, so the operator can still see
                        // the corruption rate even if forensics are missing.
                        log::warn!(
                            "session load: backup write failed at {}: {e}",
                            path.display()
                        );
                        None
                    });

                (
                    None,
                    Some(session_corruption_info(
                        path,
                        serde_category_tag(&parse_err),
                        backup_path,
                    )),
                )
            }
        }
    }

    /// Rebuild workspaces from a saved session. Each workspace's layout tree
    /// is reconstructed via `LayoutTree::from_layout_node` with CWD-aware
    /// terminal spawning. Returns the workspace list and active index.
    pub(crate) fn restore_workspaces(
        session: &paneflow_config::schema::SessionState,
        cx: &mut Context<Self>,
    ) -> (Vec<Workspace>, usize) {
        let mut workspaces = Vec::new();

        // U-016: cap restored workspaces. Each layout's pane count is bounded by
        // `validate_layout` (US-011) below, so this is the only remaining
        // unbounded restore axis - a session.json with thousands of workspace
        // entries would otherwise each spawn ≥1 real PTY.
        if session.workspaces.len() > MAX_WORKSPACES {
            log::warn!(
                "session restore: {} workspaces exceeds MAX_WORKSPACES ({MAX_WORKSPACES}); restoring the first {MAX_WORKSPACES}",
                session.workspaces.len()
            );
        }
        for ws_session in session.workspaces.iter().take(MAX_WORKSPACES) {
            let mut cwd = restored_workspace_cwd(&ws_session.cwd);
            let mut title = ws_session.title.clone();
            if should_repair_restored_root_terminal(&title, &cwd) {
                let repaired_cwd = launch_cwd::implicit_launch_cwd();
                log::info!(
                    "session restore: repairing legacy default workspace at filesystem root"
                );
                title = launch_cwd::title_for_cwd_or(&repaired_cwd, title);
                cwd = repaired_cwd;
            }
            let ws_id = next_workspace_id();

            // US-009 AC2 / US-011: `validate_layout` best-effort-caps the leaf
            // budget, but its ">= 2 children" padding re-introduces a bounded
            // O(depth) overshoot of app-synthesized pad panes once that budget
            // is spent (a crafted deeply-nested session.json - local-only, but
            // still a self-DoS). Enforce the hard MAX_PANES ceiling HERE, the
            // location US-009 AC2 names, so no workspace can ever restore more
            // than MAX_PANES real PTYs: over the cap we drop the layout and fall
            // back to a single default terminal.
            let restored_layout = ws_session
                .layout
                .clone()
                .map(without_persisted_scrollback)
                .and_then(validated_layout_within_cap);

            let mut workspace = if let Some(layout) = restored_layout {
                let mut pane_deque: VecDeque<Entity<Pane>> = VecDeque::new();
                let ws_cwd = cwd.clone();
                let tree = LayoutTree::from_layout_node(&layout, &mut pane_deque, &mut |node| {
                    let surfaces = match node {
                        LayoutNode::Pane { surfaces } => surfaces.as_slice(),
                        _ => &[],
                    };
                    Self::spawn_pane_from_surfaces(ws_id, surfaces, &ws_cwd, cx)
                });
                Workspace::with_layout_and_id(ws_id, title.clone(), cwd, tree)
            } else {
                let terminal =
                    cx.new(|cx| TerminalView::with_cwd(ws_id, Some(cwd.clone()), None, cx));
                cx.subscribe(&terminal, Self::handle_terminal_event)
                    .detach();
                let pane = cx.new(|cx| Pane::new(terminal, ws_id, cx));
                cx.subscribe(&pane, Self::handle_pane_event).detach();
                Workspace::with_cwd_and_id(ws_id, title.clone(), cwd, pane)
            };

            workspace.custom_buttons = ws_session.custom_buttons.clone();
            // EP-002 (orchestration-v2): rehydrate worktree ownership so the
            // close-time teardown still applies after a restart.
            workspace.managed_worktrees = ws_session
                .managed_worktrees
                .iter()
                .filter_map(rehydrate_managed_worktree)
                .collect();
            // US-007: rehydrate expanded dirs as absolute paths under this
            // workspace's cwd. Paths that no longer resolve to a directory are
            // dropped lazily later (by the tree's `hydrated` filter on open),
            // so a deleted folder never resurrects a dead row.
            workspace.files_expanded = ws_session
                .expanded_paths
                .iter()
                .filter_map(|rel| rehydrate_expanded_path(&workspace.cwd, rel))
                .collect();
            workspace.propagate_custom_buttons(cx);
            // US-013: kick off the deferred git-stats probe (off render thread).
            Self::spawn_initial_git_stats(ws_id, workspace.cwd.clone(), cx);
            workspaces.push(workspace);
        }

        // US-009 (orchestration-v2): `git worktree prune` on every repo whose
        // restored workspaces own worktrees - drops references whose directory
        // vanished (manual rm -rf, crashed teardown). Git-native guarantee: a
        // worktree whose directory still exists is untouched (AC5). Best-effort,
        // off the render thread, deduplicated per repo.
        let mut prune_roots: Vec<std::path::PathBuf> = workspaces
            .iter()
            .flat_map(|ws| ws.managed_worktrees.iter().map(|wt| wt.repo_root.clone()))
            .collect();
        prune_roots.sort();
        prune_roots.dedup();
        if !prune_roots.is_empty() {
            cx.spawn(async move |_this, _cx: &mut gpui::AsyncApp| {
                smol::unblock(move || {
                    for root in prune_roots {
                        if let Err(e) = crate::workspace::worktree::prune(&root) {
                            log::debug!("worktree prune skipped for {}: {e}", root.display());
                        }
                    }
                })
                .await;
            })
            .detach();
        }

        let active_idx = session
            .active_workspace
            .min(workspaces.len().saturating_sub(1));
        (workspaces, active_idx)
    }

    /// Create a `Pane` (with one tab per surface) from serialized surface
    /// definitions. Falls back to a single terminal in `fallback_cwd` when
    /// the surface list is empty.
    pub(crate) fn spawn_pane_from_surfaces(
        workspace_id: u64,
        surfaces: &[paneflow_config::schema::SurfaceDefinition],
        fallback_cwd: &std::path::Path,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        use std::path::PathBuf;

        let mut focus_idx: usize = 0;
        let tabs: Vec<crate::pane::TabContent> = if surfaces.is_empty() {
            let t = cx.new(|cx| {
                TerminalView::with_cwd(workspace_id, Some(fallback_cwd.to_path_buf()), None, cx)
            });
            cx.subscribe(&t, Self::handle_terminal_event).detach();
            vec![crate::pane::TabContent::Terminal(t)]
        } else {
            surfaces
                .iter()
                .enumerate()
                .filter_map(|(i, surface)| {
                    if surface.surface_type.as_deref() == Some("markdown") {
                        let path = surface.path.as_ref().map(PathBuf::from)?;
                        let markdown = cx.new(|cx: &mut Context<crate::markdown::MarkdownView>| {
                            crate::markdown::MarkdownView::open(path, cx)
                        });
                        if surface.focus == Some(true) {
                            focus_idx = i;
                        }
                        return Some(crate::pane::TabContent::Markdown(markdown));
                    }
                    let cwd = resolved_surface_cwd(surface.cwd.as_deref(), fallback_cwd);

                    // US-014: forward the per-surface env override; the global
                    // `terminal.env` default is merged underneath in
                    // `TerminalState::new`.
                    let surface_env = surface.env.clone();
                    let t = cx.new(|cx| {
                        TerminalView::with_cwd_and_env(
                            workspace_id,
                            Some(cwd),
                            None,
                            surface_env,
                            cx,
                        )
                    });

                    // Explicit layout definitions may still seed scrollback.
                    // Session restore clears the legacy field before this path.
                    if let Some(ref scrollback) = surface.scrollback {
                        t.read(cx).restore_scrollback(scrollback);
                    }
                    // US-013: re-apply the persisted custom name.
                    if let Some(ref custom) = surface.custom_name {
                        t.update(cx, |view, _cx| {
                            view.terminal.custom_name = Some(custom.clone());
                        });
                    }
                    // EP-005 US-013: restore the identity pill as a dimmed
                    // "last known" value. Ingress whitelist: `from_tag` is an
                    // exact match against the known agent tags, so an
                    // unknown, oversized, or control-char value from a
                    // hand-edited session.json maps to `None` and no pill is
                    // rendered (parity with the US-057/EP-010 invariant -
                    // session.json is local-only but validated anyway). The
                    // first scan (0/2 s burst on restore activity) then
                    // confirms or clears it.
                    if let Some(agent) = surface
                        .agent
                        .as_deref()
                        .and_then(crate::agent_launcher::TerminalAgent::from_tag)
                    {
                        t.update(cx, |view, _cx| {
                            view.terminal.detected_agent = Some(agent);
                            view.terminal.agent_confirmed = false;
                        });
                    }
                    // EP-006 US-019: restore the per-pane font zoom through
                    // the ingress sanitizer - NaN/inf dropped, finite values
                    // clamped to [8.0, 32.0]; never fed raw to the cell
                    // geometry (US-057/EP-010 invariant).
                    if let Some(size) = surface
                        .font_size
                        .and_then(crate::terminal::element::sanitize_font_override)
                    {
                        t.update(cx, |view, _cx| {
                            view.terminal.font_size_override = Some(size);
                        });
                    }
                    cx.subscribe(&t, Self::handle_terminal_event).detach();
                    if surface.focus == Some(true) {
                        focus_idx = i;
                    }
                    Some(crate::pane::TabContent::Terminal(t))
                })
                .collect()
        };

        let Some(_) = tabs.first() else {
            log::error!("spawn_pane_from_surfaces: no restorable tabs built; using fallback");
            let t = cx.new(|cx| {
                TerminalView::with_cwd(workspace_id, Some(fallback_cwd.to_path_buf()), None, cx)
            });
            cx.subscribe(&t, Self::handle_terminal_event).detach();
            let pane = cx.new(|cx| Pane::new(t, workspace_id, cx));
            cx.subscribe(&pane, Self::handle_pane_event).detach();
            return pane;
        };
        let pane = cx.new(|cx| Pane::new_with_tabs(tabs, focus_idx, workspace_id, cx));
        cx.subscribe(&pane, Self::handle_pane_event).detach();
        pane
    }
}

// ---------------------------------------------------------------------------
// EP-003 ingress-bound helpers (free functions, free of `&self`)
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum SessionRead {
    Data(Vec<u8>),
    Missing,
    Rejected(&'static str),
}

/// Read `path`, bounded at [`MAX_SESSION_SIZE_BYTES`] and rejecting
/// non-regular files (U-008/U-016). Stats the OPEN fd, not the path, and caps
/// the read with `take`, so a swap/grow between stat and read cannot defeat the
/// bound (the FIFO/device + TOCTOU class, mirroring `read_config_string`).
fn read_session_capped(path: &Path) -> std::io::Result<SessionRead> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SessionRead::Missing),
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    if !meta.is_file() {
        log::warn!(
            "session load: {} is not a regular file; starting empty",
            path.display()
        );
        return Ok(SessionRead::Rejected("non_regular"));
    }
    if meta.len() > MAX_SESSION_SIZE_BYTES {
        log::warn!(
            "session load: {} is {} bytes (> {MAX_SESSION_SIZE_BYTES} cap); starting empty",
            path.display(),
            meta.len()
        );
        return Ok(SessionRead::Rejected("oversize"));
    }
    let mut data = Vec::new();
    // +1 so a file grown past the cap between stat and read is still caught.
    file.take(MAX_SESSION_SIZE_BYTES + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 > MAX_SESSION_SIZE_BYTES {
        log::warn!(
            "session load: {} exceeded the {MAX_SESSION_SIZE_BYTES} cap during read; starting empty",
            path.display()
        );
        return Ok(SessionRead::Rejected("oversize"));
    }
    Ok(SessionRead::Data(data))
}

fn session_corruption_info(
    path: &Path,
    error_category: &'static str,
    backup_path: Option<PathBuf>,
) -> SessionCorruptionInfo {
    let metadata = std::fs::metadata(path).ok();
    let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
    let file_age_seconds = metadata
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|mt| SystemTime::now().duration_since(mt).ok())
        .map(|d| d.as_secs());

    SessionCorruptionInfo {
        error_category,
        file_size,
        file_age_seconds,
        backup_path,
    }
}

fn restored_workspace_cwd(raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_dir() {
        return path;
    }
    let fallback = launch_cwd::implicit_launch_cwd();
    log::warn!(
        "session restore: workspace cwd {} is not a directory; falling back to {}",
        path.display(),
        fallback.display()
    );
    fallback
}

fn resolved_surface_cwd(raw: Option<&str>, fallback_cwd: &Path) -> PathBuf {
    let Some(raw) = raw else {
        return fallback_cwd.to_path_buf();
    };
    let path = PathBuf::from(raw);
    if path.is_dir() {
        return path;
    }
    log::warn!(
        "session restore: surface cwd {} is not a directory; falling back to {}",
        path.display(),
        fallback_cwd.display()
    );
    fallback_cwd.to_path_buf()
}

fn without_persisted_scrollback(mut layout: LayoutNode) -> LayoutNode {
    fn clear(node: &mut LayoutNode) {
        match node {
            LayoutNode::Pane { surfaces } => {
                for surface in surfaces {
                    surface.scrollback = None;
                }
            }
            LayoutNode::Split { children, .. } => {
                for child in children {
                    clear(child);
                }
            }
        }
    }

    clear(&mut layout);
    layout
}

/// Validate a persisted layout and enforce the hard `MAX_PANES` ceiling
/// (US-009 AC2 / US-011). `validate_layout` best-effort-caps the leaf budget,
/// but its ">= 2 children" padding re-introduces a bounded `O(depth)` overshoot
/// of app-synthesized pad panes once that budget is spent (a crafted
/// deeply-nested session.json). Returns `None` - restore a single default
/// terminal - when the post-validation leaf count still exceeds `MAX_PANES`, so
/// no workspace can ever restore more than `MAX_PANES` real PTYs. The location
/// US-009 AC2 names; defence-in-depth on top of `validate_layout`'s budget.
fn validated_layout_within_cap(mut layout: LayoutNode) -> Option<LayoutNode> {
    paneflow_config::loader::validate_layout(&mut layout);
    let leaves = layout.leaf_count();
    if leaves > MAX_PANES {
        log::warn!(
            "session restore: layout has {leaves} panes after validation \
             (> MAX_PANES {MAX_PANES}); discarding it and restoring a single terminal"
        );
        return None;
    }
    Some(layout)
}

fn should_repair_restored_root_terminal(title: &str, cwd: &Path) -> bool {
    is_numbered_terminal_title(title) && launch_cwd::is_filesystem_root(cwd)
}

fn is_numbered_terminal_title(title: &str) -> bool {
    let Some(number) = title.strip_prefix("Terminal ") else {
        return false;
    };
    !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit())
}

/// Rehydrate one persisted `expanded_paths` entry into an absolute path under
/// `cwd`, re-asserting containment (U-030). The save side strips to a relative
/// inside-root path, but `Path::join` does not normalize, so a hand-edited /
/// agent-written session.json could carry `../../etc` or an absolute `/etc`
/// that silently replaces the base. Reject any traversal/absolute component up
/// front, then re-check `starts_with(base)` after the join. Returns `None`
/// (drop the entry) on any escape.
fn rehydrate_expanded_path(cwd: &str, rel: &str) -> Option<PathBuf> {
    let rel_path = Path::new(rel);
    if rel_path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        log::warn!(
            "session restore: dropping expanded_path with traversal/absolute component: {rel:?}"
        );
        return None;
    }
    let base = PathBuf::from(cwd);
    let abs = base.join(rel_path);
    if !abs.starts_with(&base) {
        log::warn!("session restore: dropping expanded_path escaping workspace root: {rel:?}");
        return None;
    }
    Some(abs)
}

fn rehydrate_managed_worktree(
    def: &paneflow_config::schema::ManagedWorktreeDef,
) -> Option<crate::workspace::worktree::ManagedWorktree> {
    crate::workspace::worktree::managed_worktree_from_record(
        &def.path,
        &def.repo_root,
        &def.branch,
        &def.teardown,
    )
}

fn persisted_expanded_paths(cwd: &str, expanded: &[PathBuf]) -> Vec<String> {
    let mut paths: Vec<String> = expanded
        .iter()
        .filter_map(|p| p.strip_prefix(cwd).ok())
        .map(|rel| rel.to_string_lossy().into_owned())
        .collect();
    paths.sort();
    paths
}

// ---------------------------------------------------------------------------
// US-006: corruption-backup helpers (free functions, free of `&self`)
// ---------------------------------------------------------------------------

/// US-011: serialize a [`SessionState`] to `path` with an atomic
/// write-temp-then-rename, so a crash mid-write never truncates the live
/// `session.json`. Best-effort: any error is logged, never propagated. Runs off
/// the GPUI main thread in the deferred path (`save_session` wraps it in
/// `smol::unblock`); `save_session_blocking` calls it directly at quit.
fn write_session_json(path: &Path, state: &paneflow_config::schema::SessionState) {
    let _guard = session_write_guard();
    write_session_json_inner(path, state);
}

fn write_session_json_if_current(
    path: &Path,
    state: &paneflow_config::schema::SessionState,
    save_seq: &AtomicU64,
    seq: u64,
) {
    let _guard = session_write_guard();
    if save_seq.load(Ordering::SeqCst) != seq {
        return;
    }
    write_session_json_inner(path, state);
}

fn write_session_json_inner(path: &Path, state: &paneflow_config::schema::SessionState) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(state) {
        Ok(json) => {
            let tmp_path = session_tmp_path(path);
            match std::fs::write(&tmp_path, &json) {
                Ok(()) => {
                    if let Err(e) = std::fs::rename(&tmp_path, path) {
                        log::warn!("session save rename failed: {e}");
                        let _ = std::fs::remove_file(&tmp_path);
                    }
                }
                Err(e) => {
                    log::warn!("session save failed: {e}");
                    let _ = std::fs::remove_file(&tmp_path);
                }
            }
        }
        Err(e) => log::warn!("session serialize failed: {e}"),
    }
}

fn session_tmp_path(path: &Path) -> PathBuf {
    let seq = SESSION_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let Some(parent) = path.parent() else {
        return path.with_extension(format!("json.tmp.{}.{}", std::process::id(), seq));
    };
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("session.json");
    parent.join(format!(".{file_name}.tmp.{}.{}", std::process::id(), seq))
}

/// Convert `serde_json::Error::classify()` to a fixed string. Telemetry
/// schema commits to these four buckets so we can dashboard them
/// directly without remapping if serde widens its enum later.
fn serde_category_tag(err: &serde_json::Error) -> &'static str {
    match err.classify() {
        serde_json::error::Category::Io => "io",
        serde_json::error::Category::Syntax => "syntax",
        serde_json::error::Category::Data => "data",
        serde_json::error::Category::Eof => "eof",
    }
}

/// Persist the corrupted file's bytes to
/// `<session_path>.corrupted-<unix-timestamp>` and rotate the backup
/// directory down to [`MAX_CORRUPTION_BACKUPS`] entries.
///
/// Returns `Ok(Some(path))` on success, `Ok(None)` if the wall clock is
/// before `UNIX_EPOCH` (a degenerate state we do not want to crash on),
/// `Err` on actual filesystem failures so the caller can log without
/// blocking startup.
fn write_corruption_backup(
    session_path: &Path,
    contents: &[u8],
) -> std::io::Result<Option<PathBuf>> {
    let parent = match session_path.parent() {
        Some(p) => p,
        None => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "session path has no parent",
            ));
        }
    };
    std::fs::create_dir_all(parent)?;

    let ts = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos(),
        Err(_) => return Ok(None),
    };
    let seq = SESSION_CORRUPTION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = session_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("session.json");
    let backup = parent.join(format!(
        "{stem}.corrupted-{ts}-{}-{seq}",
        std::process::id()
    ));
    std::fs::write(&backup, contents)?;

    rotate_corruption_backups(parent, stem);
    Ok(Some(backup))
}

/// Cap the count of `<stem>.corrupted-*` files in `dir` to
/// [`MAX_CORRUPTION_BACKUPS`], deleting the oldest first. Best-effort -
/// any filesystem error during rotation is logged and swallowed because
/// failing rotation must never abort startup (R8 mitigation, AC6
/// spirit).
fn rotate_corruption_backups(dir: &Path, stem: &str) {
    let prefix = format!("{stem}.corrupted-");
    let mut backups: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(it) => it
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix))
            })
            .collect(),
        Err(_) => return,
    };

    if backups.len() <= MAX_CORRUPTION_BACKUPS {
        return;
    }

    // Sort by the timestamp prefix ascending (oldest first). Older builds used
    // `corrupted-<seconds>`; current builds append `-<pid>-<seq>` to avoid
    // same-second collisions.
    backups.sort_by_key(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix(&prefix))
            .and_then(corruption_backup_timestamp)
            .unwrap_or(u128::MAX)
    });

    let drop_count = backups.len() - MAX_CORRUPTION_BACKUPS;
    for old in backups.into_iter().take(drop_count) {
        if let Err(e) = std::fs::remove_file(&old) {
            log::warn!(
                "session backup rotation: could not remove {}: {e}",
                old.display()
            );
        }
    }
}

fn corruption_backup_timestamp(suffix: &str) -> Option<u128> {
    suffix.split('-').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform_root() -> PathBuf {
        std::env::current_dir()
            .ok()
            .and_then(|path| path.ancestors().last().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR.to_string()))
    }

    #[test]
    fn restored_root_terminal_repair_only_targets_numbered_default_titles() {
        let root = platform_root();
        assert!(should_repair_restored_root_terminal("Terminal 1", &root));
        assert!(should_repair_restored_root_terminal("Terminal 12", &root));
        assert!(!should_repair_restored_root_terminal("Terminal", &root));
        assert!(!should_repair_restored_root_terminal("Root shell", &root));
    }

    #[test]
    fn restored_root_terminal_repair_ignores_non_root_cwd() {
        let mut cwd = platform_root();
        cwd.push("project");

        assert!(!should_repair_restored_root_terminal("Terminal 1", &cwd));
    }

    #[test]
    fn rehydrate_expanded_path_keeps_inside_root_and_drops_escapes() {
        // U-030: a legitimate relative path joins under the cwd…
        assert_eq!(
            rehydrate_expanded_path("/home/u/proj", "src/app"),
            Some(PathBuf::from("/home/u/proj/src/app"))
        );
        // …while traversal and absolute entries from a tampered session.json
        // are dropped rather than silently escaping the workspace root.
        assert_eq!(rehydrate_expanded_path("/home/u/proj", "../../etc"), None);
        assert_eq!(rehydrate_expanded_path("/home/u/proj", "/etc/passwd"), None);
        assert_eq!(rehydrate_expanded_path("/home/u/proj", "a/../../b"), None);
    }

    #[test]
    fn persisted_expanded_paths_are_workspace_relative_and_sorted() {
        let root = PathBuf::from("project");
        let cwd = root.to_string_lossy().into_owned();
        let paths = vec![
            root.join("src").join("z"),
            PathBuf::from("outside"),
            root.join("src").join("a"),
        ];

        let expected_a = PathBuf::from("src")
            .join("a")
            .to_string_lossy()
            .into_owned();
        let expected_z = PathBuf::from("src")
            .join("z")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            persisted_expanded_paths(&cwd, &paths),
            vec![expected_a, expected_z]
        );
    }

    #[test]
    fn session_restore_discards_legacy_scrollback_recursively() {
        let surface = |scrollback: &str| paneflow_config::schema::SurfaceDefinition {
            scrollback: Some(scrollback.to_string()),
            ..Default::default()
        };
        let layout = LayoutNode::Split {
            direction: "horizontal".to_string(),
            ratio: None,
            ratios: None,
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![surface("old pane")],
                },
                LayoutNode::Split {
                    direction: "vertical".to_string(),
                    ratio: None,
                    ratios: None,
                    children: vec![LayoutNode::Pane {
                        surfaces: vec![surface("old nested pane")],
                    }],
                },
            ],
        };

        let cleaned = without_persisted_scrollback(layout);
        let mut pending = vec![&cleaned];
        while let Some(node) = pending.pop() {
            match node {
                LayoutNode::Pane { surfaces } => {
                    assert!(surfaces.iter().all(|surface| surface.scrollback.is_none()));
                }
                LayoutNode::Split { children, .. } => pending.extend(children),
            }
        }
    }

    #[test]
    fn validated_layout_within_cap_rejects_overshooting_deep_layout() {
        use paneflow_config::schema::LayoutNode;
        // A small, valid layout passes through unchanged.
        let small = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: None,
            ratios: None,
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        assert!(
            validated_layout_within_cap(small).is_some(),
            "a 2-pane layout is within MAX_PANES"
        );

        // US-009 AC2 / US-011: a deeply-nested left-leaning chain defeats
        // `validate_layout`'s leaf budget via its >=2-children padding (each
        // budget-0 ancestor smuggles in an uncounted pad pane), so the
        // post-validation leaf_count exceeds MAX_PANES. The hard cap must drop
        // it rather than spawn O(depth) PTYs.
        let mut deep = LayoutNode::Pane {
            surfaces: vec![Default::default()],
        };
        for _ in 0..60 {
            deep = LayoutNode::Split {
                direction: "vertical".to_string(),
                ratio: None,
                ratios: None,
                children: vec![
                    deep,
                    LayoutNode::Pane {
                        surfaces: vec![Default::default()],
                    },
                ],
            };
        }
        assert!(
            validated_layout_within_cap(deep).is_none(),
            "a layout exceeding MAX_PANES after validation must be discarded"
        );
    }

    #[test]
    fn read_session_capped_reads_small_file_and_rejects_non_regular() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Happy path: a normal small file round-trips.
        let path = tmp.path().join("session.json");
        std::fs::write(&path, "{\"ok\":true}").expect("seed");
        assert_eq!(
            read_session_capped(&path).expect("io ok"),
            SessionRead::Data(b"{\"ok\":true}".to_vec())
        );
        // Opening a directory is platform-dependent: Unix usually reaches the
        // metadata guard, Windows can fail at open. Either path must not be
        // mistaken for a missing session.
        assert!(matches!(
            read_session_capped(tmp.path()),
            Ok(SessionRead::Rejected("non_regular")) | Err(_)
        ));
    }

    #[test]
    fn oversized_session_returns_corruption_info_without_backup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");
        let file = std::fs::File::create(&session_path).expect("seed");
        file.set_len(MAX_SESSION_SIZE_BYTES + 1)
            .expect("sparse oversize file");

        let (state, info) = PaneFlowApp::load_session_at(&session_path);

        assert!(state.is_none());
        let info = info.expect("oversize rejection emits diagnostics");
        assert_eq!(info.error_category, "oversize");
        assert_eq!(info.file_size, MAX_SESSION_SIZE_BYTES + 1);
        assert!(
            info.backup_path.is_none(),
            "do not copy huge rejected files"
        );
    }

    #[test]
    fn unsupported_session_version_returns_corruption_info_and_backup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");
        let contents = r#"{
            "version": 999,
            "active_workspace": 0,
            "workspaces": []
        }"#;
        std::fs::write(&session_path, contents).expect("seed unsupported session");

        let (state, info) = PaneFlowApp::load_session_at(&session_path);

        assert!(state.is_none());
        let info = info.expect("unsupported version emits diagnostics");
        assert_eq!(info.error_category, "unsupported_version");
        let backup = info.backup_path.expect("backup path populated");
        assert_eq!(
            std::fs::read_to_string(backup).expect("backup readable"),
            contents
        );
    }

    #[test]
    fn corruption_backup_names_do_not_collide_within_same_second() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");

        let first = write_corruption_backup(&session_path, b"first")
            .expect("first backup")
            .expect("first path");
        let second = write_corruption_backup(&session_path, b"second")
            .expect("second backup")
            .expect("second path");

        assert_ne!(first, second, "backups must not overwrite each other");
        assert_eq!(std::fs::read(&first).expect("first readable"), b"first");
        assert_eq!(std::fs::read(&second).expect("second readable"), b"second");
    }

    #[test]
    fn restored_cwd_helpers_fall_back_for_missing_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let valid = tmp.path().to_path_buf();
        let missing = tmp.path().join("missing");
        let valid_str = valid.to_string_lossy().into_owned();
        let missing_str = missing.to_string_lossy().into_owned();

        assert_eq!(
            restored_workspace_cwd(&valid_str),
            valid,
            "existing workspace cwd is preserved"
        );

        let workspace_fallback = restored_workspace_cwd(&missing_str);
        assert!(
            workspace_fallback.is_dir(),
            "missing workspace cwd falls back to a live directory"
        );
        assert_ne!(workspace_fallback, missing);

        let surface_fallback = tmp.path().join("fallback");
        std::fs::create_dir_all(&surface_fallback).expect("fallback dir");
        assert_eq!(
            resolved_surface_cwd(Some(&missing_str), &surface_fallback),
            surface_fallback.clone(),
            "missing surface cwd falls back to workspace cwd"
        );
        assert_eq!(
            resolved_surface_cwd(None, &surface_fallback),
            surface_fallback,
            "absent surface cwd falls back to workspace cwd"
        );
    }

    /// Write a `session.json` with deliberately broken JSON, run the
    /// path-parametrised loader, assert the corruption-info shape and
    /// the on-disk backup file. Covers AC1 (None fallback + info
    /// emitted), AC2 (backup written before fallback), AC3 (load_session
    /// behaviour test).
    #[test]
    fn malformed_json_returns_corruption_info_and_writes_backup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");
        std::fs::write(&session_path, "{").expect("seed broken session");

        let (state, info) = PaneFlowApp::load_session_at(&session_path);
        assert!(state.is_none(), "fallback to empty session expected");

        let info = info.expect("corruption info expected");
        assert_eq!(info.error_category, "eof", "trailing brace = EOF bucket");
        assert_eq!(info.file_size, 1, "single byte file");
        let backup = info.backup_path.expect("backup path populated");
        assert!(backup.exists(), "backup file actually on disk");
        assert!(
            backup
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("session.json.corrupted-")),
            "backup name format honoured"
        );
        let backup_contents = std::fs::read_to_string(&backup).expect("backup is readable");
        assert_eq!(backup_contents, "{", "backup preserves original bytes");
    }

    #[test]
    fn invalid_utf8_returns_corruption_info_and_writes_backup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");
        std::fs::write(&session_path, [0xff, 0xfe, b'{']).expect("seed broken session");

        let (state, info) = PaneFlowApp::load_session_at(&session_path);
        assert!(state.is_none(), "fallback to empty session expected");

        let info = info.expect("corruption info expected");
        assert_eq!(info.error_category, "data");
        let backup = info.backup_path.expect("backup path populated");
        let backup_contents = std::fs::read(&backup).expect("backup is readable");
        assert_eq!(backup_contents, vec![0xff, 0xfe, b'{']);
    }

    /// AC1: missing file is *not* corruption - both halves of the
    /// return tuple must be `None` so `bootstrap.rs` doesn't emit a
    /// noisy `session_corrupted` event for every fresh install.
    #[test]
    fn missing_file_yields_no_state_no_corruption() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("nonexistent.json");

        let (state, info) = PaneFlowApp::load_session_at(&path);
        assert!(state.is_none());
        assert!(info.is_none(), "missing file is not corruption");
    }

    /// R8: backup directory must not grow unbounded. After 7 induced
    /// corruptions only the 5 newest survive - verifies the
    /// timestamp-sort + drop-oldest path in `rotate_corruption_backups`.
    #[test]
    fn corruption_backup_rotation_caps_at_five() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");

        // Pre-seed 7 backups with monotonic synthetic timestamps so
        // the test does not depend on the host's wall-clock resolution
        // (a real run produces one new backup per parse failure, but
        // bursts within the same second would otherwise collide).
        for ts in 1000..1007u64 {
            let p = tmp.path().join(format!("session.json.corrupted-{ts}"));
            std::fs::write(&p, format!("backup{ts}")).expect("seed backup");
        }

        rotate_corruption_backups(tmp.path(), "session.json");

        let mut surviving: Vec<u64> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|n| n.strip_prefix("session.json.corrupted-"))
                    .and_then(|s| s.parse::<u64>().ok())
            })
            .collect();
        surviving.sort_unstable();
        assert_eq!(
            surviving,
            vec![1002, 1003, 1004, 1005, 1006],
            "5 newest survive, 2 oldest deleted"
        );

        // Live session.json itself is unaffected by the rotation.
        std::fs::write(&session_path, "{").expect("seed");
        assert!(session_path.exists());
    }

    /// US-011 AC: a burst of `save_session` calls (e.g. closing 20 workspaces)
    /// must coalesce to a single disk write - only the most-recent snapshot
    /// wins. This guards the `save_seq` monotonic-token predicate that the
    /// deferred path uses (`save_session` checks `load() == seq` after its
    /// debounce). A full `save_session` needs a live GPUI `App`, so this tests
    /// the coalescing invariant directly on the same atomic logic.
    #[test]
    fn save_seq_burst_coalesces_to_a_single_write() {
        use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

        let save_seq = AtomicU64::new(0);
        // Simulate 20 saves fired in a burst; each captures its token the way
        // `save_session` does (`fetch_add(1) + 1`).
        let captured: Vec<u64> = (0..20).map(|_| save_seq.fetch_add(1, SeqCst) + 1).collect();

        // After the burst, exactly one captured token equals the latest value,
        // so exactly one deferred task survives its post-debounce check.
        let latest = save_seq.load(SeqCst);
        let survivors = captured.iter().filter(|&&s| s == latest).count();
        assert_eq!(survivors, 1, "a 20-save burst coalesces to one write");
        assert_eq!(
            captured.last().copied(),
            Some(latest),
            "the most-recent snapshot is the survivor"
        );
    }

    /// Regression guard for the US-011 quit-path race: a deferred save that
    /// passed its debounce check must still skip its write if a blocking or
    /// newer save bumped the token before it acquired the write lock.
    #[test]
    fn deferred_save_skips_write_when_superseded_before_write() {
        use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

        let save_seq = AtomicU64::new(0);
        // A deferred save is scheduled and passes its post-debounce check.
        let deferred = save_seq.fetch_add(1, SeqCst) + 1;
        assert_eq!(
            save_seq.load(SeqCst),
            deferred,
            "deferred is latest pre-drain"
        );

        // Before it reaches the write lock, a blocking save bumps the token.
        save_seq.fetch_add(1, SeqCst);

        // The pre-write re-check inside `smol::unblock` must now observe the
        // mismatch and skip - so the older deferred snapshot never renames over
        // the final quit write.
        assert_ne!(
            save_seq.load(SeqCst),
            deferred,
            "deferred write must be skipped after a quit-time bump"
        );
    }

    #[test]
    fn restored_managed_worktree_must_match_paneflow_worktree_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        let branch = "feat/session-hardening";
        let owned_path = crate::workspace::worktree::worktree_dir(&repo_root, branch);
        std::fs::create_dir_all(&owned_path).expect("owned worktree dir");
        std::fs::write(
            crate::workspace::worktree::owner_marker_path(&owned_path),
            "owner=paneflow\n",
        )
        .expect("owner marker");
        let valid = paneflow_config::schema::ManagedWorktreeDef {
            path: owned_path.to_string_lossy().into_owned(),
            repo_root: repo_root.to_string_lossy().into_owned(),
            branch: branch.to_string(),
            teardown: "auto".to_string(),
        };

        let restored = rehydrate_managed_worktree(&valid).expect("valid owned worktree restores");
        assert_eq!(restored.path, owned_path);
        assert_eq!(
            restored.teardown,
            crate::workspace::worktree::TeardownPolicy::Auto
        );

        let outside = paneflow_config::schema::ManagedWorktreeDef {
            path: tmp.path().join("external").to_string_lossy().into_owned(),
            ..valid.clone()
        };
        assert!(
            rehydrate_managed_worktree(&outside).is_none(),
            "a restored worktree path outside Paneflow's generated dir is dropped"
        );

        let unknown_policy = paneflow_config::schema::ManagedWorktreeDef {
            teardown: "delete".to_string(),
            ..valid
        };
        let restored =
            rehydrate_managed_worktree(&unknown_policy).expect("shape-valid worktree restores");
        assert_eq!(
            restored.teardown,
            crate::workspace::worktree::TeardownPolicy::Keep,
            "unknown restored policy must not become auto-remove"
        );
    }
}
