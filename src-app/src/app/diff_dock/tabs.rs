use std::path::{Path, PathBuf};

use gpui::{AppContext, Context, Entity, Focusable, Window};

use super::code::view::CodeView;
use super::model::{DiffDockTab, MAX_DIFF_FILE_TABS};
use crate::PaneFlowApp;
use crate::terminal::{TerminalEvent, TerminalView};

impl PaneFlowApp {
    pub(crate) fn open_diff_terminal_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.active_workspace() else {
            return;
        };
        let ws_id = ws.id;
        let cwd = self
            .diff_dock
            .data
            .as_ref()
            .map(|data| data.cwd.clone())
            .filter(|cwd| !cwd.is_empty())
            .map(std::path::PathBuf::from);
        let cwd = self.new_terminal_cwd(cwd);

        let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, cwd, None, cx));
        cx.subscribe(
            &terminal,
            |this, terminal: Entity<TerminalView>, event: &TerminalEvent, cx| {
                if matches!(event, TerminalEvent::ChildExited) {
                    this.close_diff_terminal_tab(&terminal, cx);
                }
            },
        )
        .detach();

        let focus = terminal.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.diff_dock
            .diff_tabs
            .push(DiffDockTab::Terminal(terminal));
        self.diff_dock.diff_active_tab = self.diff_dock.diff_tabs.len() - 1;
        self.diff_dock.diff_tab_close_armed = None;
        cx.notify();
    }

    pub(crate) fn open_diff_file_tab(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = self.pending_file_tab();
        if let Some(index) = pending {
            self.diff_dock.diff_tabs.remove(index);
        }

        if let Some(index) = file_tab_index(&self.diff_tab_facts(cx), &path) {
            self.diff_dock.diff_active_tab = index;
            self.diff_dock.diff_tab_close_armed = None;
            self.focus_diff_tab(index, window, cx);
            cx.notify();
            return;
        }

        self.evict_oldest_diff_file_tab(cx);

        let view = cx.new(|cx| CodeView::new(path, cx));
        let index = pending
            .filter(|index| *index <= self.diff_dock.diff_tabs.len())
            .unwrap_or(self.diff_dock.diff_tabs.len());
        self.diff_dock
            .diff_tabs
            .insert(index, DiffDockTab::File(view));
        self.diff_dock.diff_active_tab = index;
        self.diff_dock.diff_tab_close_armed = None;
        self.focus_diff_tab(index, window, cx);
        cx.notify();
    }

    fn pending_file_tab(&self) -> Option<usize> {
        self.diff_dock
            .diff_tabs
            .iter()
            .position(|tab| matches!(tab, DiffDockTab::PendingFile))
    }

    fn diff_tab_facts(&self, cx: &Context<Self>) -> Vec<DiffTabFact> {
        self.diff_dock
            .diff_tabs
            .iter()
            .map(|tab| match tab {
                DiffDockTab::File(view) => {
                    let view = view.read(cx);
                    DiffTabFact::File {
                        path: view.path().to_path_buf(),
                        dirty: view.is_dirty(),
                    }
                }
                _ => DiffTabFact::Fixed,
            })
            .collect()
    }

    fn evict_oldest_diff_file_tab(&mut self, cx: &mut Context<Self>) {
        let facts = self.diff_tab_facts(cx);
        if let Some(index) = file_tab_eviction(&facts, self.diff_dock.diff_active_tab) {
            self.close_diff_tab(index, cx);
        }
    }

    fn focus_diff_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let focus = match self.diff_dock.diff_tabs.get(index) {
            Some(DiffDockTab::File(view)) => Some(view.read(cx).focus_handle(cx)),
            Some(DiffDockTab::Terminal(terminal)) => Some(terminal.read(cx).focus_handle(cx)),
            _ => None,
        };
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
    }

    pub(crate) fn handle_diff_new_file_tab(
        &mut self,
        _: &crate::DiffNewFileTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.diff_dock_visible() {
            return;
        }
        self.open_diff_file_picker(window, cx);
    }

    pub(crate) fn handle_diff_new_terminal_tab(
        &mut self,
        _: &crate::DiffNewTerminalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.diff_dock_visible() {
            return;
        }
        self.open_diff_terminal_tab(window, cx);
    }

    pub(crate) fn open_diff_file_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.pending_file_tab() {
            Some(index) => self.select_diff_tab(index, cx),
            None => {
                self.diff_dock.diff_tabs.push(DiffDockTab::PendingFile);
                self.diff_dock.diff_active_tab = self.diff_dock.diff_tabs.len() - 1;
                self.diff_dock.diff_tab_close_armed = None;
                cx.notify();
            }
        }
        if !self.files_sidebar_open {
            self.toggle_files_sidebar(cx);
        }
        if self.files_sidebar_open {
            self.focus_files_sidebar(window, cx);
        }
    }

    pub(crate) fn select_diff_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.diff_dock.diff_tabs.len() && self.diff_dock.diff_active_tab != index {
            self.diff_dock.diff_active_tab = index;
            self.diff_dock.diff_tab_close_armed = None;
            cx.notify();
        }
    }

    pub(crate) fn request_close_diff_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == 0 || index >= self.diff_dock.diff_tabs.len() {
            return;
        }
        if close_arms_first(
            &self.diff_tab_facts(cx),
            index,
            self.diff_dock.diff_tab_close_armed,
        ) {
            self.diff_dock.diff_tab_close_armed = Some(index);
            cx.notify();
            return;
        }
        self.close_diff_tab(index, cx);
    }

    pub(crate) fn close_diff_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == 0 || index >= self.diff_dock.diff_tabs.len() {
            return;
        }
        self.diff_dock.diff_tabs.remove(index);
        self.diff_dock.diff_active_tab =
            active_tab_after_close(self.diff_dock.diff_active_tab, index);
        self.diff_dock.diff_tab_close_armed = None;
        cx.notify();
    }

    fn close_diff_terminal_tab(&mut self, terminal: &Entity<TerminalView>, cx: &mut Context<Self>) {
        let found = self
            .diff_dock
            .diff_tabs
            .iter()
            .position(|tab| matches!(tab, DiffDockTab::Terminal(t) if t == terminal));
        if let Some(index) = found {
            self.close_diff_tab(index, cx);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DiffTabFact {
    Fixed,
    File { path: PathBuf, dirty: bool },
}

pub(super) fn file_tab_index(facts: &[DiffTabFact], path: &Path) -> Option<usize> {
    facts
        .iter()
        .position(|fact| matches!(fact, DiffTabFact::File { path: open, .. } if open == path))
}

pub(super) fn file_tab_eviction(facts: &[DiffTabFact], active: usize) -> Option<usize> {
    let open = facts
        .iter()
        .filter(|fact| matches!(fact, DiffTabFact::File { .. }))
        .count();
    if open < MAX_DIFF_FILE_TABS {
        return None;
    }
    facts.iter().enumerate().position(|(index, fact)| {
        index != active && matches!(fact, DiffTabFact::File { dirty: false, .. })
    })
}

pub(super) fn close_arms_first(facts: &[DiffTabFact], index: usize, armed: Option<usize>) -> bool {
    let dirty = matches!(
        facts.get(index),
        Some(DiffTabFact::File { dirty: true, .. })
    );
    dirty && armed != Some(index)
}

pub(super) fn active_tab_after_close(active: usize, closed: usize) -> usize {
    if active >= closed {
        active.saturating_sub(1)
    } else {
        active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, dirty: bool) -> DiffTabFact {
        DiffTabFact::File {
            path: PathBuf::from(path),
            dirty,
        }
    }

    #[test]
    fn reopening_a_file_finds_its_existing_tab() {
        let facts = [
            DiffTabFact::Fixed,
            file("/repo/src/main.rs", false),
            DiffTabFact::Fixed,
            file("/repo/README.md", true),
        ];

        assert_eq!(
            file_tab_index(&facts, Path::new("/repo/src/main.rs")),
            Some(1)
        );
        assert_eq!(
            file_tab_index(&facts, Path::new("/repo/README.md")),
            Some(3)
        );
        assert_eq!(file_tab_index(&facts, Path::new("/repo/Cargo.toml")), None);
        assert_eq!(
            file_tab_index(&[DiffTabFact::Fixed], Path::new("/repo")),
            None
        );
    }

    #[test]
    fn the_cap_evicts_the_oldest_saved_inactive_tab() {
        let mut facts = vec![DiffTabFact::Fixed];
        facts.extend((0..MAX_DIFF_FILE_TABS - 1).map(|i| file(&format!("/repo/{i}.rs"), false)));
        assert_eq!(file_tab_eviction(&facts, 0), None);

        facts.push(file("/repo/last.rs", false));
        assert_eq!(file_tab_eviction(&facts, 0), Some(1));
        assert_eq!(file_tab_eviction(&facts, 1), Some(2));
    }

    #[test]
    fn the_cap_never_evicts_a_modified_tab() {
        let mut facts = vec![DiffTabFact::Fixed];
        facts.extend((0..MAX_DIFF_FILE_TABS).map(|i| file(&format!("/repo/{i}.rs"), true)));
        assert_eq!(
            file_tab_eviction(&facts, 0),
            None,
            "every file tab is modified: the cap gives way rather than losing edits"
        );

        facts[3] = file("/repo/saved.rs", false);
        assert_eq!(file_tab_eviction(&facts, 0), Some(3));
    }

    #[test]
    fn closing_a_modified_tab_arms_before_it_closes() {
        let facts = [
            DiffTabFact::Fixed,
            file("/repo/dirty.rs", true),
            file("/repo/saved.rs", false),
            DiffTabFact::Fixed,
        ];

        assert!(close_arms_first(&facts, 1, None));
        assert!(!close_arms_first(&facts, 1, Some(1)));
        assert!(close_arms_first(&facts, 1, Some(2)));
        assert!(!close_arms_first(&facts, 2, None));
        assert!(!close_arms_first(&facts, 3, None));
        assert!(!close_arms_first(&facts, 9, None));
    }

    #[test]
    fn the_active_index_stays_in_bounds_after_a_close() {
        assert_eq!(active_tab_after_close(3, 3), 2);
        assert_eq!(active_tab_after_close(3, 1), 2);
        assert_eq!(active_tab_after_close(1, 2), 1);
        assert_eq!(active_tab_after_close(1, 1), 0);

        for len in 2..=12usize {
            for closed in 1..len {
                for active in 0..len {
                    let next = active_tab_after_close(active, closed);
                    assert!(
                        next < len - 1,
                        "len={len} closed={closed} active={active} -> {next} is out of bounds"
                    );
                }
            }
        }
    }
}
