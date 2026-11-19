use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AppContext, Context, Task};

use super::panel::FilesSidebar;
use crate::terminal::project_files::{ProjectFiles, rank_project_files, walk_project_files};

pub(crate) const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);
pub(crate) const SEARCH_RESULT_LIMIT: usize = 200;
const INDEX_FRESHNESS: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchHit {
    pub rel: String,
    pub highlights: Vec<Range<usize>>,
}

#[derive(Default)]
pub(crate) struct TreeSearch {
    pub generation: u64,
    pub hits: Option<Arc<Vec<SearchHit>>>,
    pub truncated: bool,
    pub pending: bool,
    task: Option<Task<()>>,
    walk: Option<Task<()>>,
    files: Option<(Arc<ProjectFiles>, Instant)>,
}

impl TreeSearch {
    pub(super) fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.task = None;
        self.walk = None;
        self.pending = false;
    }

    pub(super) fn invalidate_files(&mut self) {
        self.files = None;
    }

    fn fresh_files(&self) -> Option<Arc<ProjectFiles>> {
        self.files
            .as_ref()
            .filter(|(_, built)| built.elapsed() < INDEX_FRESHNESS)
            .map(|(files, _)| files.clone())
    }

    #[cfg(test)]
    pub(super) fn install_files(&mut self, files: ProjectFiles) {
        self.files = Some((Arc::new(files), Instant::now()));
    }
}

pub(crate) fn rank_hits(files: &ProjectFiles, query: &str) -> (Vec<SearchHit>, bool) {
    let ranked = rank_project_files(&files.paths, query, SEARCH_RESULT_LIMIT + 1);
    let truncated = ranked.len() > SEARCH_RESULT_LIMIT;
    let hits = ranked
        .into_iter()
        .take(SEARCH_RESULT_LIMIT)
        .map(|found| SearchHit {
            rel: files.paths[found.index].clone(),
            highlights: found.highlights,
        })
        .collect();
    (hits, truncated)
}

#[cfg(test)]
pub(super) fn walk_for_test(root: &std::path::Path) -> Option<ProjectFiles> {
    walk_project_files(root, &std::sync::atomic::AtomicBool::new(false))
}

impl FilesSidebar {
    pub(super) fn query_changed(&mut self, cx: &mut Context<Self>) {
        self.search.generation = self.search.generation.wrapping_add(1);
        self.search.task = None;
        if self.query.is_empty() {
            self.search.hits = None;
            self.search.truncated = false;
            self.search.pending = false;
            self.schedule_projection(true, cx);
            return;
        }
        if !self.active {
            return;
        }
        self.search.pending = true;
        let generation = self.search.generation;
        self.search.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.run_search(generation, cx));
        }));
        cx.notify();
    }

    pub(super) fn run_search(&mut self, generation: u64, cx: &mut Context<Self>) {
        if !self.active || self.search.generation != generation || self.query.is_empty() {
            return;
        }
        let Some(files) = self.search.fresh_files() else {
            self.start_search_walk(cx);
            return;
        };
        let query = self.query.clone();
        self.search.task = Some(cx.spawn(async move |this, cx| {
            let (hits, truncated) = cx
                .background_spawn(async move { rank_hits(&files, &query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.active || this.search.generation != generation {
                    return;
                }
                this.search.hits = Some(Arc::new(hits));
                this.search.truncated = truncated;
                this.search.pending = false;
                this.schedule_projection(true, cx);
            });
        }));
    }

    fn start_search_walk(&mut self, cx: &mut Context<Self>) {
        if self.search.walk.is_some() {
            return;
        }
        let root = self.model.root().to_path_buf();
        let cancelled = self.walk_cancelled.clone();
        let epoch = self.epoch;
        self.search.walk = Some(cx.spawn(async move |this, cx| {
            let files = smol::unblock(move || walk_project_files(&root, &cancelled)).await;
            let _ = this.update(cx, |this, cx| {
                if !this.active || this.epoch != epoch {
                    return;
                }
                this.search.walk = None;
                let Some(files) = files else {
                    this.search.pending = false;
                    cx.notify();
                    return;
                };
                this.search.files = Some((Arc::new(files), Instant::now()));
                let generation = this.search.generation;
                this.run_search(generation, cx);
            });
        }));
    }
}
