//! EP-001 (prd-git-diff-mode-2026-Q3.md): lifecycle + main-area entry
//! point for the dedicated Git Diff mode ([`AppMode::Diff`]).
//!
//! The mode owns the full main area plus its own left sidebar, entered
//! via the CLI / Review tabs in the shared sidebar footer. EP-001 stands up the shell
//! only - `enter_diff_mode` just flips the mode and `render_diff_main`
//! renders a placeholder. EP-002 (US-004/US-005) mounts the reused
//! `diff::DiffView` engine here; EP-005 adds the scope selector.

use crate::diff::{DiffScope, DiffView, DiffViewEvent, DiffWorktree, RepoGroup};
use crate::{OpenDiffView, PaneFlowApp};
use gpui::{
    AnyElement, AppContext, Context, Entity, IntoElement, ParentElement, Styled, Window, div, px,
};
use paneflow_config::schema::AppMode;
use std::path::{Path, PathBuf};

/// US-016: max retained single-repo diff hosts. Each holds only suspended rows
/// (bounded by the per-diff `MAX_FILE_*` caps), no watchers, so the ceiling is a
/// few tens of MB worst case; a session usually visits 1-3 repos so the cap is
/// rarely hit.
const DIFF_VIEW_CACHE_CAP: usize = 6;

/// US-016 warm-resume cache key for a single-repo [`crate::diff::DiffView`]
/// (Project / Worktree scope). A diff entity is reused across a CLI↔Diff toggle
/// (or a workspace switch back to a visited repo) only when all three match: the
/// repo root, the scope, and the exact worktree set (column layout). A scope
/// toggle or a changed worktree set therefore correctly misses the cache and
/// mounts a structurally-correct entity - a stale layout can never render.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DiffViewKey {
    repo_root: PathBuf,
    scope: DiffScope,
    worktrees_hash: u64,
}

impl DiffViewKey {
    fn new(repo_root: &Path, scope: DiffScope, worktrees: &[DiffWorktree]) -> Self {
        use std::hash::{Hash as _, Hasher as _};
        // Hash the RAW path strings - NOT `norm_path` (which `fs::canonicalize`s,
        // a blocking syscall that would stall the GPUI thread on a slow/NFS mount
        // for every worktree on each rebuild). The key only needs to be stable per
        // (repo, scope, worktree set), and the seed paths already are; symlink
        // dedup is the column builder's job (`spawn_worktree_discovery`), not the
        // cache key's. Worst case a symlinked duplicate misses the warm cache.
        let mut paths: Vec<String> = worktrees
            .iter()
            .map(|w| w.path.to_string_lossy().into_owned())
            .collect();
        paths.sort();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        paths.hash(&mut h);
        Self {
            repo_root: repo_root.to_path_buf(),
            scope,
            worktrees_hash: h.finish(),
        }
    }
}

/// Worktree-scope curation filter: keep only the worktrees whose raw path is in
/// the chosen set. `None` (or an empty set) ⇒ keep ALL (the default). Raw path
/// strings (not `norm_path`) so there is no `canonicalize` syscall on the GPUI
/// thread and the match is stable with the picker + `diff_chosen_worktrees` keys.
fn filter_chosen(
    worktrees: Vec<DiffWorktree>,
    chosen: Option<&std::collections::HashSet<String>>,
) -> Vec<DiffWorktree> {
    match chosen {
        Some(set) if !set.is_empty() => worktrees
            .into_iter()
            .filter(|w| set.contains(&w.path.to_string_lossy().into_owned()))
            .collect(),
        _ => worktrees,
    }
}

/// US-016: stable signature of the Multi-project repo-group set, so the retained
/// host is reused only while the same projects are open (order-insensitive).
fn multiproject_signature(groups: &[RepoGroup]) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut entries: Vec<(String, Vec<String>)> = groups
        .iter()
        .map(|g| {
            let mut worktrees: Vec<String> = g
                .worktrees
                .iter()
                .map(|w| w.path.to_string_lossy().into_owned())
                .collect();
            worktrees.sort();
            (norm_path(&g.repo_root), worktrees)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = std::collections::hash_map::DefaultHasher::new();
    entries.hash(&mut h);
    h.finish()
}

/// Sidebar width when in [`AppMode::Diff`]. Pinned to [`SIDEBAR_WIDTH`]:
/// the mode tabs sit at the bottom of the rail itself, so a per-mode width
/// would make the rail jump under the pointer that just switched modes.
/// This deliberately drops the earlier 360 px Zed git-panel parity.
pub(crate) const DIFF_SIDEBAR_WIDTH: f32 = crate::SIDEBAR_WIDTH;

impl PaneFlowApp {
    /// Toggle the Git Diff mode: pressing the binding (or the action)
    /// from CLI enters diff mode; pressing it again from within diff
    /// mode returns to CLI.
    pub(crate) fn handle_open_diff_view(
        &mut self,
        _: &OpenDiffView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.mode {
            AppMode::Diff => self.enter_cli_mode(window, cx),
            AppMode::Cli => self.enter_diff_mode(cx),
        }
    }

    /// Switch into [`AppMode::Diff`] and mount the `DiffView` for the
    /// active workspace's repo. Keeps the two non-CLI surfaces mutually
    /// exclusive.
    pub(crate) fn enter_diff_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = AppMode::Diff;
        // `rebuild_diff_view` mounts the entity and calls `cx.notify()`.
        self.rebuild_diff_view(cx);
        self.save_session(cx);
    }

    /// (Re)point the mounted diff host to the active workspace's `repo_root` and
    /// the active scope. US-016 warm-resume: rather than destroy + cold-rebuild
    /// every time, this parks the currently-displayed host (suspending its
    /// watchers while the cache retains its computed rows), prunes hosts of
    /// closed repos, then either RESUMES a cached entity whose key matches (the
    /// instant warm path - a CLI↔Diff toggle or a workspace switch back to a
    /// visited repo) or builds a fresh one on a cache miss. Clears to the
    /// empty-state when the active workspace has no resolved repo. Called on
    /// diff-mode entry (US-004), workspace switch (US-005), and scope change
    /// (US-011). The seed (`collect_*`) is a pure in-memory read; git
    /// subprocesses run off the main thread inside the entity.
    pub(crate) fn rebuild_diff_view(&mut self, cx: &mut Context<Self>) {
        // Park (suspend + unpoint) the displayed host into the cache before
        // re-pointing, then drop cached hosts whose repo closed.
        self.park_displayed_diff(cx);
        self.prune_diff_cache();

        let repo_root = self
            .workspaces
            .get(self.active_idx)
            .and_then(|ws| ws.repo_root.clone());

        match self.diff_mode.diff_scope {
            // US-014: one host with a tab per repo, lazy-mounting each repo's
            // DiffView. Retained across toggles while the project set is stable.
            DiffScope::MultiProject => {
                self.diff_mode.diff_view_key = None;
                let groups = self.collect_multiproject_groups();
                if groups.is_empty() {
                    self.diff_mode.multi_diff_view_retained = None;
                    cx.notify();
                    return;
                }
                let sig = multiproject_signature(&groups);
                if let Some((retained_sig, view)) = self.diff_mode.multi_diff_view_retained.clone()
                    && retained_sig == sig
                {
                    view.update(cx, |v, cx| v.resume(cx));
                    self.diff_mode.multi_diff_view = Some(view);
                    cx.notify();
                    return;
                }
                let view = cx.new(|cx| crate::diff::MultiRepoDiffView::new(groups, cx));
                self.diff_mode.multi_diff_view_retained = Some((sig, view.clone()));
                self.diff_mode.multi_diff_view = Some(view);
            }
            // US-011: the active workspace only (one column).
            DiffScope::Project => {
                let Some(root) = repo_root else {
                    self.diff_mode.diff_view_key = None;
                    cx.notify();
                    return;
                };
                let worktrees = self.collect_project_worktrees();
                let key = DiffViewKey::new(&root, DiffScope::Project, &worktrees);
                let (view, _miss) = self.mount_or_resume_diff(key, root, worktrees, cx);
                self.diff_mode.diff_view = Some(view);
            }
            // US-013: open worktrees of the active repo; on a COLD mount they are
            // augmented off-thread with on-disk worktrees not open as workspaces.
            // A cache hit already carries those discovered columns, so discovery
            // re-runs only on a miss.
            DiffScope::Worktree => {
                let Some(root) = repo_root else {
                    self.diff_mode.diff_view_key = None;
                    cx.notify();
                    return;
                };
                // Curation: if the user chose a subset of branches for this repo,
                // build columns for exactly those (unchosen branches are never
                // diffed); no choice ⇒ all worktrees (the default).
                let chosen = self.diff_mode.diff_chosen_worktrees.get(&root).cloned();
                let open = filter_chosen(self.collect_diff_worktrees(&root), chosen.as_ref());
                let key = DiffViewKey::new(&root, DiffScope::Worktree, &open);
                let (view, miss) = self.mount_or_resume_diff(key, root.clone(), open.clone(), cx);
                self.diff_mode.diff_view = Some(view);
                // EP-003 US-012: eagerly fetch this repo's worktree list (off the
                // main thread) so the scope header's branch badge can read
                // "chosen/total" without the picker ever being opened. Guarded so
                // it fetches at most once per repo (the picker-open path refreshes
                // it too); the data is a best-effort hint, not load-bearing.
                if self.diff_mode.diff_available_repo.as_deref() != Some(root.as_path()) {
                    self.refresh_diff_available_worktrees(root.clone(), cx);
                }
                if miss {
                    self.spawn_worktree_discovery(root, open, chosen, cx);
                }
            }
        }
        cx.notify();
    }

    /// US-016: park the displayed diff host before re-pointing (mode exit,
    /// workspace switch, scope change). Suspends the entity - releasing its OS
    /// watchers + ending its debounce loop - while the cache (single-repo) or
    /// the retained slot (Multi-project) keeps it alive with its computed rows.
    /// Clears only the display pointer (pointer-vs-owner split).
    /// Also closes the prior `multi_diff_view` watcher leak (it was never
    /// cleared on CLI/Agents entry).
    pub(crate) fn park_displayed_diff(&mut self, cx: &mut Context<Self>) {
        if let Some(dv) = self.diff_mode.diff_view.take() {
            dv.update(cx, |v, cx| v.suspend(cx));
        }
        if let Some(mv) = self.diff_mode.multi_diff_view.take() {
            mv.update(cx, |v, cx| v.suspend(cx));
        }
    }

    /// US-016: resume + return the cached single-repo `DiffView` on a key hit
    /// (instant warm rows + a cheap fingerprint revalidation), or build, insert,
    /// and return a fresh one on a miss. Returns `(entity, was_miss)` so the
    /// Worktree branch knows whether to (re)run on-disk discovery. Sets
    /// `diff_view_key` to the mounted key.
    fn mount_or_resume_diff(
        &mut self,
        key: DiffViewKey,
        root: PathBuf,
        worktrees: Vec<DiffWorktree>,
        cx: &mut Context<Self>,
    ) -> (Entity<crate::diff::DiffView>, bool) {
        if let Some(view) = self.diff_mode.diff_view_cache.get(&key).cloned() {
            view.update(cx, |v, cx| v.resume(cx));
            self.diff_mode.diff_view_key = Some(key);
            return (view, false);
        }
        let view = cx.new(|cx| crate::diff::DiffView::new(root, worktrees, cx));
        // Worktree scope: the column-header `×` deselects the branch (rebuild
        // without it) instead of hiding it in place - shown-or-not, no "N hidden"
        // limbo. Wire it once on the fresh entity; the subscription + flag persist
        // across cache resume (the key is scope-stamped, so a worktree view is
        // never reused for another scope).
        if self.diff_mode.diff_scope == DiffScope::Worktree {
            view.update(cx, |v, _| v.set_close_removes(true));
            cx.subscribe(&view, Self::handle_diff_view_event).detach();
        }
        self.diff_mode
            .diff_view_cache
            .insert(key.clone(), view.clone());
        self.diff_mode.diff_view_key = Some(key.clone());
        self.evict_diff_cache_if_needed(&key);
        (view, true)
    }

    /// US-016: drop cached diff hosts whose repo is no longer backed by any open
    /// workspace (closed project). Dropping the (already-suspended) entity frees
    /// its retained rows; its watchers were released on suspend. Keeps the cache
    /// bounded to live repos.
    fn prune_diff_cache(&mut self) {
        let open: std::collections::HashSet<PathBuf> = self
            .workspaces
            .iter()
            .filter_map(|ws| ws.repo_root.clone())
            .collect();
        self.diff_mode
            .diff_view_cache
            .retain(|k, _| open.contains(&k.repo_root));
        if open.is_empty() {
            self.diff_mode.multi_diff_view_retained = None;
        }
    }

    /// US-016: bound the cache to `DIFF_VIEW_CACHE_CAP`. Every non-displayed
    /// entry is suspended (no watchers, just rows), so order-insensitive
    /// eviction is safe; `keep` (the just-mounted key) is never evicted.
    /// Dropping an entity frees its rows.
    fn evict_diff_cache_if_needed(&mut self, keep: &DiffViewKey) {
        if self.diff_mode.diff_view_cache.len() <= DIFF_VIEW_CACHE_CAP {
            return;
        }
        let victims: Vec<DiffViewKey> = self
            .diff_mode
            .diff_view_cache
            .keys()
            .filter(|k| *k != keep)
            .cloned()
            .collect();
        for k in victims {
            if self.diff_mode.diff_view_cache.len() <= DIFF_VIEW_CACHE_CAP {
                break;
            }
            self.diff_mode.diff_view_cache.remove(&k);
        }
    }

    /// Drop this repo's cached Worktree-scope hosts so the next rebuild re-runs
    /// on-disk discovery.
    ///
    /// [`DiffViewKey`] hashes the worktrees the *seed* carried, and the seed is
    /// the open workspaces only ([`Self::collect_diff_worktrees`]). Creating or
    /// removing a checkout behind a tab therefore leaves the key identical: the
    /// warm resume would keep rendering the old column set, and the change would
    /// only surface after a scope toggle or a restart. Discovery runs on a miss
    /// ([`Self::spawn_worktree_discovery`]), so forcing the miss is what makes a
    /// new column appear and a removed one leave.
    pub(crate) fn invalidate_worktree_diff_cache(
        &mut self,
        repo_root: &Path,
        cx: &mut Context<Self>,
    ) {
        let stale: Vec<DiffViewKey> = self
            .diff_mode
            .diff_view_cache
            .keys()
            .filter(|key| key.scope == DiffScope::Worktree && key.repo_root == repo_root)
            .cloned()
            .collect();
        if stale.is_empty() {
            return;
        }
        let was_displayed = self
            .diff_mode
            .diff_view_key
            .as_ref()
            .is_some_and(|key| stale.contains(key));
        for key in stale {
            self.diff_mode.diff_view_cache.remove(&key);
        }
        // Only a host on screen has to be re-mounted now. The others simply miss
        // the cache the next time their scope is selected, and outside Diff mode
        // the displayed host is already parked - `rebuild_diff_view` runs on
        // mode entry and overwrites the dangling key then.
        if was_displayed && self.mode == AppMode::Diff {
            self.rebuild_diff_view(cx);
        }
    }

    /// US-013: off the main thread, enumerate the repo's on-disk worktrees and
    /// append any not already open as workspaces (dedup by a case-safe
    /// normalized path) to the live `DiffView` *in place* via `add_columns` -
    /// no re-mount, so existing columns keep their loaded content (no
    /// Loading→Loading flash mid-session). Discovered-only worktrees carry
    /// `workspace_id: None`. No-op when nothing new is found.
    fn spawn_worktree_discovery(
        &mut self,
        root: std::path::PathBuf,
        open: Vec<crate::diff::DiffWorktree>,
        chosen: Option<std::collections::HashSet<String>>,
        cx: &mut Context<Self>,
    ) {
        // US-016++ (#8): flag the in-flight discovery so the sidebar shows a
        // "Discovering worktrees…" note instead of looking like columns are
        // missing during the brief cold-mount window.
        self.diff_mode.diff_discovering = true;
        self.diff_mode.diff_discovering_root = Some(root.clone());
        let requested_root = root.clone();
        cx.spawn(async move |this, cx| {
            let discovered =
                smol::unblock(move || crate::diff::list_repo_worktrees(&requested_root)).await;
            let mut seen: std::collections::HashSet<String> =
                open.iter().map(|w| norm_path(&w.path)).collect();
            let mut new_cols = Vec::new();
            for (path, branch) in discovered {
                // Curation: when the user chose a subset, only append discovered
                // worktrees that ARE in the chosen set (raw path key, matching the
                // picker + `filter_chosen`).
                if let Some(set) = &chosen
                    && !set.contains(&path.to_string_lossy().into_owned())
                {
                    continue;
                }
                if seen.insert(norm_path(&path)) {
                    new_cols.push(crate::diff::DiffWorktree {
                        path,
                        branch,
                        workspace_id: None,
                    });
                }
            }
            let _ = cx.update(|cx| {
                this.update(cx, |app, cx| {
                    let owns_discovery =
                        app.diff_mode.diff_discovering_root.as_deref() == Some(root.as_path());
                    let still_current_worktree_view = app.mode == AppMode::Diff
                        && app.diff_mode.diff_scope == crate::diff::DiffScope::Worktree
                        && app.diff_mode.diff_view_key.as_ref().is_some_and(|key| {
                            key.repo_root == root && key.scope == crate::diff::DiffScope::Worktree
                        });
                    if owns_discovery {
                        app.diff_mode.diff_discovering = false;
                        app.diff_mode.diff_discovering_root = None;
                    }
                    // Apply only if still showing this repo's worktree scope.
                    if !new_cols.is_empty()
                        && still_current_worktree_view
                        && owns_discovery
                        && let Some(dv) = app.diff_mode.diff_view.clone()
                    {
                        dv.update(cx, |v, cx| v.add_columns(new_cols, cx));
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    /// Worktree-scope branches picker: (re)fetch every worktree of `root` off the
    /// main thread so the picker can offer branches not currently shown (it lists
    /// the full set; columns show only the chosen subset). Lazy - called when the
    /// picker opens.
    pub(crate) fn refresh_diff_available_worktrees(
        &mut self,
        root: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.diff_mode.diff_available_repo = Some(root.clone());
        let requested_root = root.clone();
        cx.spawn(async move |this, cx| {
            let wts =
                smol::unblock(move || crate::diff::list_repo_worktrees(&requested_root)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |app, cx| {
                    if app.diff_mode.diff_available_repo.as_deref() != Some(root.as_path()) {
                        return;
                    }
                    app.diff_mode.diff_available_worktrees = wts
                        .into_iter()
                        .map(|(path, branch)| crate::diff::DiffWorktree {
                            path,
                            branch,
                            workspace_id: None,
                        })
                        .collect();
                    cx.notify();
                })
            });
        })
        .detach();
    }

    /// Worktree-scope branches picker: toggle `path` in/out of `root`'s chosen
    /// worktree set, then rebuild so the column set follows. The first toggle
    /// materializes the implicit "all" into an explicit set (so unchecking one
    /// from "all" works); emptying the set reverts to the all-default (a zero-
    /// column view is meaningless).
    pub(crate) fn toggle_chosen_worktree(
        &mut self,
        root: std::path::PathBuf,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let all: std::collections::HashSet<String> = self
            .diff_mode
            .diff_available_worktrees
            .iter()
            .map(|w| w.path.to_string_lossy().into_owned())
            .collect();
        let set = self
            .diff_mode
            .diff_chosen_worktrees
            .entry(root.clone())
            .or_insert(all);
        if !set.remove(&path) {
            set.insert(path);
        }
        let now_empty = set.is_empty();
        if now_empty {
            self.diff_mode.diff_chosen_worktrees.remove(&root);
        }
        self.rebuild_diff_view(cx);
    }

    /// Whether `path` is currently shown as a column (in the chosen set, or no
    /// chosen set ⇒ all shown). Drives the branches-picker checkmarks.
    pub(crate) fn diff_worktree_is_chosen(&self, root: &std::path::Path, path: &str) -> bool {
        match self.diff_mode.diff_chosen_worktrees.get(root) {
            Some(set) => set.contains(path),
            None => true,
        }
    }

    /// Worktree scope: a branch column asked to close (its header `×`). Drop it
    /// from the chosen set and rebuild - same selection model as the branches
    /// picker, so re-checking it there brings it back. No "hidden" limbo.
    pub(crate) fn handle_diff_view_event(
        &mut self,
        _view: Entity<DiffView>,
        event: &DiffViewEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            DiffViewEvent::CloseColumn { path } => {
                self.deselect_diff_worktree(path.to_string_lossy().into_owned(), cx);
            }
        }
    }

    /// Remove `path` from the active repo's chosen-worktree set, then rebuild so
    /// the column disappears. The "all-shown" default is materialized from the
    /// columns currently on screen (so on-disk-discovered branches survive), then
    /// `path` is dropped. No-op when only one column remains - a zero-column diff
    /// is meaningless (mirrors [`Self::toggle_chosen_worktree`]'s empty guard).
    fn deselect_diff_worktree(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(root) = self
            .workspaces
            .get(self.active_idx)
            .and_then(|ws| ws.repo_root.clone())
        else {
            return;
        };
        let shown: std::collections::HashSet<String> = self
            .diff_mode
            .diff_view
            .as_ref()
            .map(|v| {
                v.read(cx)
                    .column_paths()
                    .into_iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        if shown.len() <= 1 {
            return;
        }
        let set = self
            .diff_mode
            .diff_chosen_worktrees
            .entry(root)
            .or_insert(shown);
        set.remove(&path);
        self.rebuild_diff_view(cx);
    }

    /// Return to [`AppMode::Cli`] from any non-CLI mode. Idempotent
    /// when already in CLI. Tears down the mounted non-CLI surface and
    /// restores keyboard focus to the active workspace's first pane.
    pub(crate) fn enter_cli_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == AppMode::Cli {
            return;
        }
        // US-016 warm-resume: suspend (don't destroy) the mounted diff host.
        // Suspending releases its filesystem watchers + ends its debounce loop
        // (the watcher-release contract) while the cache retains its computed
        // rows, so a return to Diff mode shows the diff in one frame instead of
        // recomputing it. Also closes the prior `multi_diff_view` watcher leak.
        self.park_displayed_diff(cx);
        self.mode = AppMode::Cli;
        // Focus contract: hand the keyboard back to the terminal the
        // user left. PTYs are detached, so the process is still alive.
        if let Some(ws) = self.workspaces.get_mut(self.active_idx) {
            ws.focus_first(window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Main-content render branch for [`AppMode::Diff`].
    ///
    /// Renders the mounted `diff::DiffView` (the reused multi-worktree
    /// engine) when the active workspace backs a git repo, else the
    /// empty-state. The entity is (re)built off-render by
    /// `rebuild_diff_view`; this branch never mutates the view's diff state
    /// (re-entrancy) - it only PUSHES the scope breadcrumb fragment into the
    /// mounted view every frame (`scope_slot`, push-only contract like
    /// `TitleBar`), which the view mounts as the left side of its single
    /// toolbar row (Codex redesign: one row of chrome).
    pub(crate) fn render_diff_main(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use crate::diff::DiffScope;
        let ui = crate::theme::ui_colors();
        let breadcrumb = self.render_scope_header(cx);
        // No view mounted: a local bare bar hosts the breadcrumb - it carries
        // the scope/project pickers, the only way OUT of an empty scope.
        let empty = |msg: &'static str, breadcrumb: AnyElement| {
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(36.))
                        .px(px(10.))
                        .child(breadcrumb),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().text_color(ui.muted).text_size(px(13.)).child(msg)),
                )
                .into_any_element()
        };
        let body = match self.diff_mode.diff_scope {
            DiffScope::MultiProject => match self.diff_mode.multi_diff_view.clone() {
                Some(v) => {
                    v.update(cx, |mv, _| mv.scope_slot = Some(breadcrumb));
                    v.into_any_element()
                }
                None => empty("No open projects with a git repository", breadcrumb),
            },
            _ => match self.diff_mode.diff_view.clone() {
                Some(v) => {
                    v.update(cx, |dv, _| dv.scope_slot = Some(breadcrumb));
                    v.into_any_element()
                }
                None => empty("No git repository in the active workspace", breadcrumb),
            },
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

/// US-013: normalize a worktree path for dedup so the same worktree reported by
/// `git worktree list` and by an open workspace collapses to one entry. Resolves
/// symlinks via `canonicalize` (falling back to the raw path) and lowercases on
/// case-insensitive filesystems (macOS / Windows) so a case-different path does
/// not produce a duplicate column.
fn norm_path(p: &std::path::Path) -> String {
    let resolved = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = resolved.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        s.to_lowercase()
    } else {
        s
    }
}
