use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AppContext, Context};

use super::panel::FilesSidebar;
use super::projection::FilesProjection;
use super::worker::FilesWorker;

impl FilesSidebar {
    pub(super) fn start_worker(
        &mut self,
        root: PathBuf,
        expanded: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let (worker, updates) = FilesWorker::start(root, expanded);
        self.worker = Some(worker);
        let epoch = self.epoch;
        self.updates_task = Some(cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                if this.update(cx, |this, cx| {
                    if !this.active || this.epoch != epoch || this.revision != update.revision {
                        return;
                    }
                    this.expanded = update.tree.expanded.clone();
                    if !update.watcher_available {
                        tracing::debug!(target: "paneflow_app::files_sidebar", "files watcher unavailable; background polling remains active");
                    }
                    let previous = std::mem::replace(&mut this.tree, Arc::new(update.tree));
                    cx.background_spawn(async move { drop(previous); }).detach();
                    this.emit_expansion(cx);
                    this.schedule_projection(false, cx);
                }).is_err() {
                    break;
                }
            }
        }));
    }

    pub(super) fn schedule_projection(&mut self, reveal: bool, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        self.pending_reveal |= reveal;
        self.projection_revision = self.projection_revision.wrapping_add(1);
        let revision = self.projection_revision;
        let epoch = self.epoch;
        let tree = self.tree.clone();
        let expanded = self.expanded.clone();
        let query = self.query.clone();
        self.projection_task = Some(cx.spawn(async move |this, cx| {
            let projection = cx
                .background_spawn(async move { FilesProjection::build(&tree, &expanded, &query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.active || this.epoch != epoch || this.projection_revision != revision {
                    return;
                }
                let previous_index = this.selected_index().unwrap_or(0);
                this.selected =
                    projection.reconcile_selection(this.selected.as_deref(), previous_index);
                let previous = std::mem::replace(&mut this.projection, Arc::new(projection));
                cx.background_spawn(async move {
                    drop(previous);
                })
                .detach();
                if std::mem::take(&mut this.pending_reveal) {
                    this.reveal_selection();
                }
                #[cfg(test)]
                {
                    this.projection_count += 1;
                }
                cx.notify();
            });
        }));
    }
}
