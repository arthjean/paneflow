use std::path::{Path, PathBuf};

use gpui::{App, Context, KeyDownEvent, Window};

use super::filter;
use crate::PaneFlowApp;
use crate::app::files_tree;

impl PaneFlowApp {
    pub(super) fn files_visible_rows(&self) -> Vec<files_tree::VisibleRowRef<'_>> {
        files_tree::flatten_visible_refs(
            &self.files_tree.root,
            &self.files_tree.expanded,
            &self.files_tree.children,
        )
    }

    pub(super) fn files_filter_lowered(&self, cx: &App) -> String {
        self.files_filter_input.read(cx).value().to_lowercase()
    }

    fn files_rendered_rows(&self, cx: &App) -> Vec<(PathBuf, bool)> {
        let lowered = self.files_filter_lowered(cx);
        if lowered.is_empty() {
            self.files_visible_rows()
                .iter()
                .map(|row| (row.node.path.clone(), row.node.is_dir))
                .collect()
        } else {
            filter::filter_rows(&self.files_tree.root, &self.files_tree.children, &lowered)
                .into_iter()
                .map(|row| (row.node.path.clone(), row.node.is_dir))
                .collect()
        }
    }

    pub(super) fn select_files_row(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Some(idx) = self
            .files_rendered_rows(cx)
            .iter()
            .position(|(row_path, _)| row_path == path)
        {
            self.files_selected = idx;
        }
    }

    pub(super) fn clamp_files_selection(&mut self) {
        let len = files_tree::visible_len(
            &self.files_tree.root,
            &self.files_tree.expanded,
            &self.files_tree.children,
        );
        if len == 0 {
            self.files_selected = 0;
        } else if self.files_selected >= len {
            self.files_selected = len - 1;
        }
    }

    pub(super) fn clear_files_filter(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.files_filter_input.read(cx).value().is_empty() {
            return false;
        }
        self.files_filter_input
            .update(cx, |input, cx| input.clear(cx));
        self.files_selected = 0;
        self.files_focus.focus(window, cx);
        cx.notify();
        true
    }

    pub(super) fn handle_files_sidebar_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.files_rendered_rows(cx);
        let len = rows.len();
        match event.keystroke.key.as_str() {
            "escape" => {
                if !self.clear_files_filter(window, cx) {
                    self.close_files_sidebar(cx);
                }
            }
            "enter" | "space" if len > 0 => {
                let selected = self.files_selected.min(len - 1);
                let (path, is_dir) = rows[selected].clone();
                self.activate_files_path(path, is_dir, window, cx);
            }
            "up" if len > 0 => {
                self.files_selected = self.files_selected.saturating_sub(1);
                cx.notify();
            }
            "down" if len > 0 => {
                self.files_selected = (self.files_selected + 1).min(len - 1);
                cx.notify();
            }
            "home" if len > 0 => {
                self.files_selected = 0;
                cx.notify();
            }
            "end" if len > 0 => {
                self.files_selected = len - 1;
                cx.notify();
            }
            _ => {}
        }
    }

    fn activate_files_path(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_files_row(&path, cx);
        if is_dir {
            self.toggle_dir(&path, cx);
        } else {
            self.open_file_in_diff_dock(path, window, cx);
        }
    }
}
