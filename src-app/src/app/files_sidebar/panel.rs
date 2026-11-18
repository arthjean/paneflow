use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    AnyWindowHandle, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Task,
    UniformListScrollHandle,
};

use super::projection::FilesProjection;
use super::search::TreeSearch;
use super::worker::FilesWorker;
use crate::app::files_git::GitStatuses;
use crate::app::files_tree::FilesTreeState;
use crate::widgets::text_input::TextInput;

pub(crate) const SEARCH_QUERY_MAX_CHARS: usize = 256;

pub(crate) enum FilesEvent {
    Close(AnyWindowHandle),
    OpenFile {
        path: PathBuf,
        root: PathBuf,
        window: AnyWindowHandle,
    },
    ContextMenu(crate::FilesContextMenu),
    Expanded {
        root: PathBuf,
        paths: Vec<PathBuf>,
    },
    Toast(String),
}

pub(crate) struct FilesSidebar {
    pub(super) tree: Arc<FilesTreeState>,
    pub(super) git: Arc<GitStatuses>,
    pub(super) expanded: HashSet<PathBuf>,
    pub(super) projection: Arc<FilesProjection>,
    pub(super) selected: Option<PathBuf>,
    pub(super) filter_input: Entity<TextInput>,
    pub(super) query: String,
    pub(super) search: TreeSearch,
    pub(super) focus: FocusHandle,
    pub(super) scroll: UniformListScrollHandle,
    pub(super) active: bool,
    pub(super) revision: u64,
    pub(super) epoch: u64,
    pub(super) projection_revision: u64,
    pub(super) projection_task: Option<Task<()>>,
    pub(super) updates_task: Option<Task<()>>,
    pub(super) worker: Option<FilesWorker>,
    pub(super) pending_reveal: bool,
    pub(super) reveal_target: Option<PathBuf>,
    pub(super) reveal_generation: u64,
    pub(super) reveal_task: Option<Task<()>>,
    pub(super) reveal_request: Option<PathBuf>,
    pub(super) refresh_ticket: u64,
    pub(super) refreshing: bool,
    pub(super) expand_all_ticket: u64,
    pub(super) expanding_all: bool,
    pub(super) all_expanded: bool,
    pub(super) walk_cancelled: Arc<AtomicBool>,
    #[cfg(test)]
    pub(super) render_count: usize,
    #[cfg(test)]
    pub(super) projection_count: usize,
}

impl EventEmitter<FilesEvent> for FilesSidebar {}

impl Focusable for FilesSidebar {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FilesSidebar {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let filter_input = cx.new(|cx| TextInput::new("", "Search files", cx));
        cx.observe(&filter_input, |this, input, cx| {
            let value = input.read(cx).value();
            let query: String = value.chars().take(SEARCH_QUERY_MAX_CHARS).collect();
            if query.len() < value.len() {
                let kept = query.clone();
                cx.defer(move |cx| input.update(cx, |input, cx| input.set_value(kept, cx)));
            }
            if this.query != query {
                this.query = query;
                this.selected = None;
                this.query_changed(cx);
            }
        })
        .detach();
        if let Some(signal) = crate::theme::theme_signal(cx) {
            cx.observe(&signal, |_, _, cx| cx.notify()).detach();
        }
        Self {
            tree: Arc::new(FilesTreeState::default()),
            git: Arc::default(),
            expanded: HashSet::new(),
            projection: Arc::default(),
            selected: None,
            filter_input,
            query: String::new(),
            search: TreeSearch::default(),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            active: false,
            revision: 0,
            epoch: 0,
            projection_revision: 0,
            projection_task: None,
            updates_task: None,
            worker: None,
            pending_reveal: false,
            reveal_target: None,
            reveal_generation: 0,
            reveal_task: None,
            reveal_request: None,
            refresh_ticket: 0,
            refreshing: false,
            expand_all_ticket: 0,
            expanding_all: false,
            all_expanded: false,
            walk_cancelled: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            render_count: 0,
            #[cfg(test)]
            projection_count: 0,
        }
    }

    pub(crate) fn open(&mut self, root: PathBuf, expanded: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.deactivate();
        self.release_snapshot(cx);
        self.active = true;
        self.revision = 0;
        self.tree = Arc::new(FilesTreeState::root_shell(root.clone()));
        self.git = Arc::default();
        self.expanded = expanded
            .iter()
            .cloned()
            .chain(std::iter::once(root.clone()))
            .collect();
        self.projection = Arc::default();
        self.selected = None;
        self.query.clear();
        self.search = TreeSearch::default();
        self.all_expanded = false;
        self.walk_cancelled = Arc::new(AtomicBool::new(false));
        self.scroll = UniformListScrollHandle::new();
        self.filter_input.update(cx, |input, cx| input.clear(cx));
        self.start_worker(root, expanded, cx);
        cx.notify();
    }

    pub(crate) fn retry(&mut self, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        let root = self.tree.root.clone();
        let expanded = self.expanded_paths();
        self.open(root, expanded, cx);
    }

    pub(crate) fn deactivate(&mut self) {
        self.active = false;
        self.epoch = self.epoch.wrapping_add(1);
        self.projection_revision = self.projection_revision.wrapping_add(1);
        self.worker = None;
        self.updates_task = None;
        self.projection_task = None;
        self.pending_reveal = false;
        self.reveal_target = None;
        self.reveal_generation = self.reveal_generation.wrapping_add(1);
        self.reveal_task = None;
        self.reveal_request = None;
        self.refreshing = false;
        self.expanding_all = false;
        self.walk_cancelled.store(true, Ordering::Relaxed);
        self.search.cancel();
    }

    pub(crate) fn release_snapshot(&mut self, cx: &mut Context<Self>) {
        if self.active {
            return;
        }
        let tree = std::mem::take(&mut self.tree);
        let git = std::mem::take(&mut self.git);
        let projection = std::mem::take(&mut self.projection);
        let expanded = std::mem::take(&mut self.expanded);
        let search = std::mem::take(&mut self.search);
        self.selected = None;
        cx.background_spawn(async move {
            drop((tree, git, projection, expanded, search));
        })
        .detach();
    }

    pub(super) fn expanded_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<_> = self
            .expanded
            .iter()
            .filter(|path| **path != self.tree.root)
            .cloned()
            .collect();
        paths.sort();
        paths
    }

    pub(super) fn emit_expansion(&self, cx: &mut Context<Self>) {
        cx.emit(FilesEvent::Expanded {
            root: self.tree.root.clone(),
            paths: self.expanded_paths(),
        });
    }

    pub(super) fn send_expansion(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        if let Some(worker) = &self.worker {
            worker.set_expanded(self.revision, self.expanded_paths());
        }
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        self.refresh_ticket = self.refresh_ticket.wrapping_add(1);
        self.refreshing = true;
        self.search.invalidate_files();
        if let Some(worker) = &self.worker {
            worker.refresh(self.refresh_ticket);
        }
        cx.notify();
    }

    pub(super) fn reload_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if let Some(worker) = &self.worker {
            worker.reload(dir);
        }
        cx.notify();
    }

    pub(crate) fn toggle_expand_all(&mut self, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        if self.all_expanded {
            self.all_expanded = false;
            self.expanding_all = false;
            self.expanded = HashSet::from([self.tree.root.clone()]);
            self.send_expansion();
            self.emit_expansion(cx);
            self.schedule_projection(false, cx);
            return;
        }
        self.all_expanded = true;
        self.expanding_all = true;
        self.revision = self.revision.wrapping_add(1);
        self.expand_all_ticket = self.expand_all_ticket.wrapping_add(1);
        if let Some(worker) = &self.worker {
            worker.expand_all(self.revision, self.expand_all_ticket);
        }
        cx.notify();
    }
}
