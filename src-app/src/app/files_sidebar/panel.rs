use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AnyWindowHandle, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Task,
    UniformListScrollHandle,
};

use super::projection::FilesProjection;
use super::worker::FilesWorker;
use crate::app::files_tree::FilesTreeState;
use crate::widgets::text_input::TextInput;

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
}

pub(crate) struct FilesSidebar {
    pub(super) tree: Arc<FilesTreeState>,
    pub(super) expanded: HashSet<PathBuf>,
    pub(super) projection: Arc<FilesProjection>,
    pub(super) selected: Option<PathBuf>,
    pub(super) filter_input: Entity<TextInput>,
    pub(super) query: String,
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
        let filter_input = cx.new(|cx| TextInput::new("", "Filter files...", cx));
        cx.observe(&filter_input, |this, input, cx| {
            let query = input.read(cx).value().to_lowercase();
            if this.query != query {
                this.query = query;
                this.selected = None;
                this.schedule_projection(true, cx);
            }
        })
        .detach();
        if let Some(signal) = crate::theme::theme_signal(cx) {
            cx.observe(&signal, |_, _, cx| cx.notify()).detach();
        }
        Self {
            tree: Arc::new(FilesTreeState::default()),
            expanded: HashSet::new(),
            projection: Arc::default(),
            selected: None,
            filter_input,
            query: String::new(),
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
        self.expanded = expanded
            .iter()
            .cloned()
            .chain(std::iter::once(root.clone()))
            .collect();
        self.projection = Arc::default();
        self.selected = None;
        self.query.clear();
        self.scroll = UniformListScrollHandle::new();
        self.filter_input.update(cx, |input, cx| input.clear(cx));
        self.start_worker(root, expanded, cx);
        cx.notify();
    }

    pub(crate) fn deactivate(&mut self) {
        self.active = false;
        self.epoch = self.epoch.wrapping_add(1);
        self.projection_revision = self.projection_revision.wrapping_add(1);
        self.worker = None;
        self.updates_task = None;
        self.projection_task = None;
        self.pending_reveal = false;
    }

    pub(crate) fn release_snapshot(&mut self, cx: &mut Context<Self>) {
        if self.active {
            return;
        }
        let tree = std::mem::take(&mut self.tree);
        let projection = std::mem::take(&mut self.projection);
        let expanded = std::mem::take(&mut self.expanded);
        self.selected = None;
        cx.background_spawn(async move {
            drop((tree, projection, expanded));
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
}
