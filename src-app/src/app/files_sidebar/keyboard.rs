use std::path::Path;

use gpui::{Context, KeyDownEvent, ScrollStrategy, Window};

use super::panel::{FilesEvent, FilesSidebar};

impl FilesSidebar {
    pub(super) fn selected_index(&self) -> Option<usize> {
        self.selected
            .as_deref()
            .and_then(|path| self.projection.index(path))
    }

    pub(super) fn reveal_selection(&self) {
        if let Some(index) = self.selected_index() {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
    }

    pub(super) fn select_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.projection.index(path).is_some() {
            self.selected = Some(path.to_path_buf());
            self.reveal_selection();
            cx.notify();
        }
    }

    fn select_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.projection.rows.get(index) {
            self.selected = Some(row.node.path.clone());
            self.reveal_selection();
            cx.notify();
        }
    }

    pub(super) fn toggle_dir(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_path_buf());
        }
        self.revision = self.revision.wrapping_add(1);
        if let Some(worker) = &self.worker {
            worker.set_expanded(self.revision, self.expanded_paths());
        }
        self.emit_expansion(cx);
        self.schedule_projection(true, cx);
    }

    pub(super) fn activate_path(
        &mut self,
        path: &Path,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.active {
            return;
        }
        self.select_path(path, cx);
        if is_dir {
            self.toggle_dir(path, cx);
        } else {
            cx.emit(FilesEvent::OpenFile {
                path: path.to_path_buf(),
                root: self.tree.root.clone(),
                window: window.window_handle(),
            });
        }
    }

    pub(super) fn clear_files_filter(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.filter_input.read(cx).value().is_empty() {
            return false;
        }
        self.filter_input.update(cx, |input, cx| input.clear(cx));
        self.focus.focus(window, cx);
        true
    }

    pub(super) fn handle_files_sidebar_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.active {
            return;
        }
        if self.filter_input.read(cx).focus_handle.is_focused(window)
            && !matches!(
                event.keystroke.key.as_str(),
                "up" | "down" | "enter" | "escape"
            )
        {
            return;
        }
        let count = self.projection.rows.len();
        let index = self.selected_index().unwrap_or(0);
        let row = self.projection.rows.get(index).cloned();
        match event.keystroke.key.as_str() {
            "escape" => {
                if !self.clear_files_filter(window, cx) {
                    cx.emit(FilesEvent::Close(window.window_handle()));
                }
            }
            "up" if count > 0 => self.select_index(index.saturating_sub(1), cx),
            "down" if count > 0 => self.select_index((index + 1).min(count - 1), cx),
            "home" if count > 0 => self.select_index(0, cx),
            "end" if count > 0 => self.select_index(count - 1, cx),
            "enter" | "space" if let Some(row) = row => {
                self.activate_path(&row.node.path, row.node.is_dir, window, cx)
            }
            "right" if let Some(row) = row => {
                if row.node.is_dir && self.query.is_empty() {
                    if !self.expanded.contains(&row.node.path) {
                        self.toggle_dir(&row.node.path, cx);
                    } else if self
                        .projection
                        .rows
                        .get(index + 1)
                        .is_some_and(|next| next.depth > row.depth)
                    {
                        self.select_index(index + 1, cx);
                    }
                }
            }
            "left" if let Some(row) = row => {
                if self.query.is_empty() {
                    if row.node.is_dir && self.expanded.contains(&row.node.path) {
                        self.toggle_dir(&row.node.path, cx);
                    } else if let Some(parent) = row.node.path.parent() {
                        self.select_path(parent, cx);
                    }
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}
