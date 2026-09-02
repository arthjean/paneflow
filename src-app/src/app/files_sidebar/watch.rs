use std::path::{Path, PathBuf};

use gpui::Context;

use crate::PaneFlowApp;
use crate::app::files_tree;

impl PaneFlowApp {
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

    pub(crate) fn spawn_files_hydration(
        &mut self,
        root: PathBuf,
        persisted: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.files_watcher = None;
        self.files_event_rx = None;
        self.files_tree = files_tree::FilesTreeState::root_shell(root.clone());

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
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
