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

    pub(crate) fn reveal_dir(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        let root = self.tree.root.clone();
        if path == root {
            self.reveal_target = None;
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            cx.notify();
            return;
        }
        if !path.starts_with(&root) {
            return;
        }
        self.reveal_generation = self.reveal_generation.wrapping_add(1);
        self.reveal_task = None;
        self.reveal_entry(path, true, cx);
    }

    pub(crate) fn reveal_file(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.active || self.reveal_request.as_deref() == Some(path) {
            return;
        }
        self.reveal_request = Some(path.to_path_buf());
        let root = self.tree.root.clone();
        if path == root || !path.starts_with(&root) {
            return;
        }
        if self.selected.as_deref() == Some(path) && self.projection.index(path).is_some() {
            return;
        }
        self.reveal_generation = self.reveal_generation.wrapping_add(1);
        let generation = self.reveal_generation;
        let target = path.to_path_buf();
        self.reveal_task = Some(cx.spawn(async move |this, cx| {
            let probe = target.clone();
            let ignored = smol::unblock(move || {
                crate::app::files_tree::path_is_git_ignored(&root, &probe, false)
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if this.reveal_generation != generation {
                    return;
                }
                this.reveal_task = None;
                if this.active && !ignored {
                    this.reveal_entry(&target, false, cx);
                }
            });
        }));
    }

    fn reveal_entry(&mut self, path: &Path, expand_self: bool, cx: &mut Context<Self>) {
        let root = self.tree.root.clone();
        self.clear_search(cx);
        let first = if expand_self {
            Some(path)
        } else {
            path.parent()
        };
        for ancestor in first
            .into_iter()
            .flat_map(Path::ancestors)
            .take_while(|ancestor| *ancestor != root)
        {
            self.expanded.insert(ancestor.to_path_buf());
        }
        self.send_expansion();
        self.selected = Some(path.to_path_buf());
        self.reveal_target = Some(path.to_path_buf());
        self.emit_expansion(cx);
        self.schedule_projection(false, cx);
    }

    pub(super) fn clear_search(&mut self, cx: &mut Context<Self>) {
        if self.query.is_empty() && self.filter_input.read(cx).value().is_empty() {
            return;
        }
        self.query.clear();
        self.search.cancel();
        self.search.hits = None;
        self.search.truncated = false;
        self.filter_input.update(cx, |input, cx| input.clear(cx));
    }

    pub(super) fn settle_reveal_target(&mut self) {
        let Some(target) = self.reveal_target.as_ref() else {
            return;
        };
        if let Some(index) = self.projection.index(target) {
            self.selected = Some(target.clone());
            self.scroll.scroll_to_item(index, ScrollStrategy::Center);
            self.reveal_target = None;
        }
    }

    fn select_index(&mut self, index: usize, cx: &mut Context<Self>) {
        self.reveal_target = None;
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
        self.send_expansion();
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
        self.reveal_target = None;
        self.select_path(path, cx);
        if is_dir && !self.query.is_empty() {
            self.reveal_dir(path, cx);
        } else if is_dir {
            if self
                .projection
                .index(path)
                .and_then(|index| self.projection.rows.get(index))
                .is_some_and(|row| row.state == super::projection::DirState::Failed)
            {
                self.reload_dir(path.to_path_buf(), cx);
                return;
            }
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
