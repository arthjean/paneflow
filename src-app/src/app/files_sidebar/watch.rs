//! Files sidebar live filesystem watch + per-workspace expansion persistence
//! (PRD `prd-files-tree-sidebar-2026-Q3`, EP-002).
//!
//! `spawn_files_hydration` reads the tree + registers non-recursive `notify`
//! watches for the root and expanded dirs off the render thread (US-018; US-005
//! wiring, degrading gracefully on failure per US-006); the background drain loop in `bootstrap` debounces +
//! coalesces events and calls `refresh_files_dirs` for the targeted
//! per-directory re-read. `sync_files_expansion` mirrors the live expansion into
//! the active `Workspace` so it persists to `session.json` (US-007). Split out
//! of `mod.rs` to keep each file under the 250-line budget.

use std::path::{Path, PathBuf};

use gpui::Context;

use crate::PaneFlowApp;
use crate::app::files_tree;

impl PaneFlowApp {
    /// Mirror the live tree's expansion into the active workspace (excluding
    /// the implicit root) so it survives close/reopen and persists to
    /// `session.json` (US-007).
    pub(super) fn sync_files_expansion(&mut self) {
        let root = self.files_tree.root.clone();
        let mut expanded: Vec<PathBuf> = self
            .files_tree
            .expanded
            .iter()
            .filter(|p| **p != root)
            .cloned()
            .collect();
        expanded.sort();
        if let Some(ws) = self.workspaces.get_mut(self.active_idx) {
            ws.files_expanded = expanded;
        }
    }

    /// US-018: hydrate the Files tree and install non-recursive watches off the
    /// GPUI main thread.
    ///
    /// A recursive `notify` watch walks the entire subtree at registration
    /// (inotify adds one watch per directory), so large repos can freeze the UI
    /// and exhaust OS watcher budgets. We instead watch only the root and
    /// currently-expanded directories. Collapsed subtrees are read and watched
    /// lazily on expand.
    pub(crate) fn spawn_files_hydration(
        &mut self,
        root: PathBuf,
        persisted: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        // Drop the previous watch + channel immediately (cheap), and show a
        // root shell so the panel paints this frame while the reads run.
        self.files_watcher = None;
        self.files_event_rx = None;
        self.files_tree = files_tree::FilesTreeState::root_shell(root.clone());

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                // Stage 1: directory reads. Inject the populated tree first so
                // content appears before watch registration completes.
                let tree = smol::unblock({
                    let root = root.clone();
                    let persisted = persisted.clone();
                    move || files_tree::FilesTreeState::hydrated(root, &persisted)
                })
                .await;
                let watch_dirs = tree.expanded.iter().cloned().collect::<Vec<_>>();
                let still_current = this
                    .update(cx, |app, cx| {
                        if app.files_sidebar_open && app.files_tree.root == root {
                            app.files_tree = tree;
                            app.sync_files_expansion();
                            app.clamp_files_selection();
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !still_current {
                    return;
                }

                // Stage 2: non-recursive watch registration for the visible
                // tree frontier. The watcher is still built off-thread because
                // some platforms do synchronous filesystem work at registration.
                let built = smol::unblock({
                    let root = root.clone();
                    move || build_files_watcher(&root, &watch_dirs)
                })
                .await;
                let _ = this.update(cx, |app, _cx| {
                    if app.files_sidebar_open
                        && app.files_tree.root == root
                        && let Some((watcher, rx)) = built
                    {
                        app.files_watcher = Some(watcher);
                        app.files_event_rx = Some(rx);
                    }
                });
            },
        )
        .detach();
    }

    pub(super) fn watch_files_dir(&mut self, dir: &Path) {
        let Some(watcher) = self.files_watcher.as_mut() else {
            return;
        };
        use notify::Watcher;
        if let Err(e) = watcher.watch(dir, notify::RecursiveMode::NonRecursive) {
            log::warn!(
                "files watcher: failed to watch expanded dir {} ({e}); falling back to on-expand reads for it",
                dir.display()
            );
        }
    }

    pub(super) fn unwatch_files_dir(&mut self, dir: &Path) {
        if dir == self.files_tree.root {
            return;
        }
        let Some(watcher) = self.files_watcher.as_mut() else {
            return;
        };
        use notify::Watcher;
        if let Err(e) = watcher.unwatch(dir) {
            tracing::debug!(
                target: "paneflow_app::files_sidebar",
                "files watcher: unwatch {} failed: {e}",
                dir.display()
            );
        }
    }

    /// Apply a debounced, prefix-coalesced batch of changed directories
    /// (US-005), called from the background drain loop in `bootstrap`. Re-reads
    /// only the cached (expanded) directories among the affected parents - a
    /// change under a collapsed/uncached dir is ignored until it's expanded
    /// (then read fresh by `toggle_dir`). `rescan` (a notify overflow/Rescan
    /// signal, US-006 AC3) forces a root re-read. Never walks the whole tree.
    pub(crate) fn refresh_files_dirs(
        &mut self,
        mut dirs: Vec<PathBuf>,
        rescan: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.files_sidebar_open {
            return;
        }
        let root = self.files_tree.root.clone();
        if rescan {
            dirs.push(root.clone());
        }
        let mut changed = false;
        for dir in files_tree::coalesce_by_prefix(dirs) {
            // AC4: only re-read directories we've already cached (expanded).
            if let std::collections::hash_map::Entry::Occupied(mut e) =
                self.files_tree.children.entry(dir.clone())
            {
                e.insert(files_tree::read_dir_sorted(&root, &dir));
                changed = true;
            }
        }
        if changed {
            self.clamp_files_selection();
            cx.notify();
        }
    }
}

/// US-018: build non-recursive `notify` watches for the root and currently
/// expanded directories, returning the watcher + its event channel, or `None`
/// on failure. The caller falls back to on-expand reads (US-006).
///
/// Runs on a background thread; the caller re-injects the returned handles.
#[allow(clippy::type_complexity)]
fn build_files_watcher(
    root: &Path,
    dirs: &[PathBuf],
) -> Option<(
    notify::RecommendedWatcher,
    std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
)> {
    use notify::Watcher;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("files watcher unavailable: {e}; falling back to on-expand reads");
            return None;
        }
    };
    let mut watched = std::collections::HashSet::new();
    for dir in std::iter::once(root.to_path_buf()).chain(dirs.iter().cloned()) {
        if !watched.insert(dir.clone()) {
            continue;
        }
        if let Err(e) = watcher.watch(&dir, notify::RecursiveMode::NonRecursive) {
            log::warn!(
                "files watcher: failed to watch {} ({e}); falling back to on-expand reads",
                dir.display()
            );
            return None;
        }
    }
    Some((watcher, rx))
}
