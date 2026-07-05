//! Keyboard handling for the docked Files sidebar.

use gpui::{Context, KeyDownEvent, Window};

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

    pub(super) fn select_files_row(&mut self, path: &std::path::Path) {
        if let Some(idx) = self
            .files_visible_rows()
            .iter()
            .position(|row| row.node.path == path)
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

    pub(super) fn handle_files_sidebar_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.files_visible_rows();
        let len = rows.len();
        match event.keystroke.key.as_str() {
            "escape" => self.close_files_sidebar(cx),
            "enter" | "space" if len > 0 => {
                let selected = self.files_selected.min(len - 1);
                let row = rows[selected];
                let path = row.node.path.clone();
                let is_dir = row.node.is_dir;
                drop(rows);
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
        path: std::path::PathBuf,
        is_dir: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.select_files_row(&path);
        if is_dir {
            self.toggle_dir(&path, cx);
        } else if files_tree::is_markdown(&path) {
            self.open_markdown_in_active_pane(path, window, cx);
        }
    }
}
