use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AppContext, Context};

use super::panel::{FilesEvent, FilesSidebar};
use super::projection::{FileRow, SearchRows};
use super::worker::{FilesWorker, TreeUpdate};

pub(crate) const EXPAND_ALL_CAPPED: &str = "Expanded the first 2,000 folders";

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
                if this
                    .update(cx, |this, cx| this.apply_update(epoch, update, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn apply_update(&mut self, epoch: u64, update: TreeUpdate, cx: &mut Context<Self>) {
        if !self.active || self.epoch != epoch {
            return;
        }
        let previous_index = self.selected_index();
        self.model.apply(update.changes);
        self.git = update.git;
        if self.refreshing && update.refreshed == self.refresh_ticket {
            self.refreshing = false;
        }
        if let Some(outcome) = update.expanded_all
            && outcome.ticket == self.expand_all_ticket
        {
            self.expanding_all = false;
            if outcome.capped {
                cx.emit(FilesEvent::Toast(EXPAND_ALL_CAPPED.into()));
            }
        }
        if !update.watcher_available {
            tracing::debug!(target: "paneflow_app::files_sidebar", "files watcher unavailable; background polling remains active");
        }
        if self.revision == update.revision
            && let Some(expanded) = update.expanded
        {
            self.model.replace_expanded(expanded);
            self.emit_expansion(cx);
        }
        self.schedule_projection_from(previous_index, false, cx);
    }

    pub(super) fn schedule_projection(&mut self, reveal: bool, cx: &mut Context<Self>) {
        self.schedule_projection_from(None, reveal, cx);
    }

    fn schedule_projection_from(
        &mut self,
        previous_index: Option<usize>,
        reveal: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.active {
            return;
        }
        self.pending_reveal |= reveal;
        self.projection_revision = self.projection_revision.wrapping_add(1);
        if self.query.is_empty() {
            self.projection_task = None;
            if let Some(previous) = self.search_rows.take() {
                cx.background_spawn(async move { drop(previous) }).detach();
            }
            let previous_index = previous_index.or_else(|| self.selected_index());
            self.projection_landed(previous_index.unwrap_or(0), cx);
            return;
        }
        let revision = self.projection_revision;
        let epoch = self.epoch;
        let root = self.model.root().to_path_buf();
        let git = self.git.clone();
        let hits = self.search.hits.clone().unwrap_or_default();
        self.projection_task = Some(cx.spawn(async move |this, cx| {
            let rows = cx
                .background_spawn(async move { SearchRows::build(&root, &hits, &git) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.active || this.epoch != epoch || this.projection_revision != revision {
                    return;
                }
                let previous_index = this.selected_index().unwrap_or(0);
                if let Some(previous) = this.search_rows.replace(Arc::new(rows)) {
                    cx.background_spawn(async move { drop(previous) }).detach();
                }
                this.projection_landed(previous_index, cx);
            });
        }));
    }

    fn projection_landed(&mut self, previous_index: usize, cx: &mut Context<Self>) {
        self.selected = match &self.search_rows {
            Some(rows) => rows.reconcile_selection(self.selected.as_deref(), previous_index),
            None => self
                .model
                .reconcile_selection(self.selected.as_deref(), previous_index),
        };
        if std::mem::take(&mut self.pending_reveal) {
            self.reveal_selection();
        }
        self.settle_reveal_target();
        #[cfg(test)]
        {
            self.projection_count += 1;
        }
        cx.notify();
    }

    pub(super) fn row_count(&self) -> usize {
        match &self.search_rows {
            Some(rows) => rows.rows.len(),
            None => self.model.row_count(),
        }
    }

    pub(super) fn rows_in(&self, range: Range<usize>) -> Vec<FileRow> {
        let rows = match &self.search_rows {
            Some(rows) => rows
                .rows
                .get(range.start.min(rows.rows.len())..range.end.min(rows.rows.len()))
                .map(<[FileRow]>::to_vec)
                .unwrap_or_default(),
            None => self
                .model
                .rows(range)
                .into_iter()
                .map(|row| FileRow::from_ref(row, &self.git))
                .collect(),
        };
        #[cfg(test)]
        self.rows_built.set(self.rows_built.get() + rows.len());
        rows
    }

    pub(super) fn row_at(&self, index: usize) -> Option<FileRow> {
        self.rows_in(index..index.saturating_add(1)).pop()
    }

    pub(super) fn row_index(&self, path: &Path) -> Option<usize> {
        match &self.search_rows {
            Some(rows) => rows.index(path),
            None => self.model.index(path),
        }
    }
}
