//! Lifecycle of the diff dock's tabs: opening a terminal tab from the `+`
//! menu, selecting a tab, and closing one.
//!
//! The `Changes` tab is permanent and always index 0, so a dock that has never
//! been given a second tab behaves exactly as before this strip existed.

use std::path::{Path, PathBuf};

use gpui::{AppContext, Context, Entity, Focusable, Window};

use super::code::view::CodeView;
use super::model::{DiffDockTab, MAX_DIFF_FILE_TABS};
use crate::PaneFlowApp;
use crate::terminal::{TerminalEvent, TerminalView};

impl PaneFlowApp {
    /// Open a terminal tab in the dock and focus it. The shell lands in the
    /// folder the dock is diffing, falling back to the active workspace root
    /// (the same chain `new_terminal_cwd` uses for a split).
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
        // Only the exit is wired: the dock terminal has no pane in the layout
        // tree, so the app-level CWD / port-scan / open-path handlers have
        // nothing to act on for it.
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

    /// Open `path` in a dock file tab and focus it (US-017).
    ///
    /// Re-opening a file that already has a tab activates that tab instead of
    /// stacking a second copy of the same document, which would give the file
    /// two independent undo stacks over one path.
    pub(crate) fn open_diff_file_tab(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The placeholder tab is the question this click answers: retire it
        // first, so the document lands in its slot instead of stacking beside a
        // tab still asking for one.
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
        // Eviction may have shortened the strip under the placeholder's index,
        // so the slot is only honored while it still exists; otherwise the tab
        // appends, exactly as it did before the placeholder existed.
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

    /// Where the dock's placeholder `File` tab sits, if it has one.
    fn pending_file_tab(&self) -> Option<usize> {
        self.diff_dock
            .diff_tabs
            .iter()
            .position(|tab| matches!(tab, DiffDockTab::PendingFile))
    }

    /// Project the strip into the facts the lifecycle rules read.
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

    /// Enforce the [`MAX_DIFF_FILE_TABS`] cap before a new file tab is pushed.
    fn evict_oldest_diff_file_tab(&mut self, cx: &mut Context<Self>) {
        let facts = self.diff_tab_facts(cx);
        if let Some(index) = file_tab_eviction(&facts, self.diff_dock.diff_active_tab) {
            self.close_diff_tab(index, cx);
        }
    }

    /// Move keyboard focus onto whatever the tab at `index` hosts. The
    /// `Changes` tab owns no focus handle of its own, so it is a no-op there.
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

    /// Ctrl+G, and the `+` menu's "File" row (US-018). Both open the Files
    /// sidebar so the user picks the file to edit; the picker's click is what
    /// calls [`Self::open_diff_file_tab`]. The chord is inert unless the diff
    /// dock is actually on screen ([`Self::diff_dock_visible`], not the
    /// `open` flag alone, which survives a trip through Settings or
    /// a mode switch), which is what keeps it from acting as a global "open the
    /// sidebar" shortcut.
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

    /// Ctrl+J: the chord the `+` menu advertises on its "Terminal" row
    /// (US-018). Same dock-open gate as the file chord.
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

    /// Open the dock's placeholder tab and bring up the Files sidebar as its
    /// picker (US-018). A sidebar that is already open is a no-op beyond
    /// re-focusing it: toggling would close the very picker the gesture asked
    /// for.
    pub(crate) fn open_diff_file_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The dock takes its waiting room straight away rather than staying on
        // `Changes`: "File" is an answer about what the dock is for, and the
        // tree that follows only supplies the path.
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
            self.files_focus.focus(window, cx);
        }
    }

    pub(crate) fn select_diff_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.diff_dock.diff_tabs.len() && self.diff_dock.diff_active_tab != index {
            self.diff_dock.diff_active_tab = index;
            // Moving off a tab drops any pending close confirmation: the arm is
            // a one-gesture state, not a mode the user has to escape.
            self.diff_dock.diff_tab_close_armed = None;
            cx.notify();
        }
    }

    /// The close affordance's entry point (US-017). A modified file tab arms
    /// instead of closing: the second press on the armed control is the
    /// confirmation. Every other tab closes on the first press.
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

    /// Close the tab at `index`. Index 0 (`Changes`) is permanent, so the call
    /// is a no-op there. The selection falls back to the previous tab.
    pub(crate) fn close_diff_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == 0 || index >= self.diff_dock.diff_tabs.len() {
            return;
        }
        self.diff_dock.diff_tabs.remove(index);
        self.diff_dock.diff_active_tab =
            active_tab_after_close(self.diff_dock.diff_active_tab, index);
        // The armed index refers to a strip that just shifted, so it is dropped
        // rather than re-mapped: a stale arm would put the confirmation on
        // whatever tab slid into the slot.
        self.diff_dock.diff_tab_close_armed = None;
        cx.notify();
    }

    /// Close whichever tab hosts `terminal` (the shell exited under it).
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

/// A dock tab reduced to what US-017's lifecycle rules actually read.
///
/// The rules below are index arithmetic over this projection, which is what
/// lets them be proven directly: a live `PaneFlowApp` cannot be built in a
/// test (its constructor starts the IPC server, PTYs and file watchers), so
/// the entry points delegate their whole decision here and keep only the
/// entity plumbing for themselves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DiffTabFact {
    /// `Changes` or a dock terminal: no path, never unsaved.
    Fixed,
    File {
        path: PathBuf,
        dirty: bool,
    },
}

/// Index of the tab already showing `path`, if any. Backs the dedupe in
/// [`PaneFlowApp::open_diff_file_tab`]: re-opening a file activates its tab
/// rather than stacking a second document over one path.
pub(super) fn file_tab_index(facts: &[DiffTabFact], path: &Path) -> Option<usize> {
    facts
        .iter()
        .position(|fact| matches!(fact, DiffTabFact::File { path: open, .. } if open == path))
}

/// Which tab the [`MAX_DIFF_FILE_TABS`] cap closes before a new file tab is
/// pushed: the leftmost (oldest) file tab that is neither modified nor active.
///
/// A modified tab is never a victim, so the cap can only ever cost the user a
/// saved document. When every file tab is modified or active the cap gives way
/// and returns `None` - dropping unsaved work to honor a display limit would be
/// the worse failure.
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

/// True when the close gesture on `index` must arm instead of closing, i.e.
/// the tab holds unsaved edits and is not already awaiting its confirmation.
pub(super) fn close_arms_first(facts: &[DiffTabFact], index: usize, armed: Option<usize>) -> bool {
    let dirty = matches!(
        facts.get(index),
        Some(DiffTabFact::File { dirty: true, .. })
    );
    dirty && armed != Some(index)
}

/// The active tab index after the tab at `closed` is removed. Selection falls
/// back to the previous tab, and can never point past the shortened strip.
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

    /// US-017: opening a file that already has a tab activates it instead of
    /// creating a second one.
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
        // A file with no tab yet is a miss, and `Changes` / terminals never
        // answer a path lookup.
        assert_eq!(file_tab_index(&facts, Path::new("/repo/Cargo.toml")), None);
        assert_eq!(
            file_tab_index(&[DiffTabFact::Fixed], Path::new("/repo")),
            None
        );
    }

    /// US-017: under the cap nothing is evicted; at the cap the oldest tab
    /// that is neither modified nor active goes.
    #[test]
    fn the_cap_evicts_the_oldest_saved_inactive_tab() {
        let mut facts = vec![DiffTabFact::Fixed];
        facts.extend((0..MAX_DIFF_FILE_TABS - 1).map(|i| file(&format!("/repo/{i}.rs"), false)));
        // One short of the cap: the new tab fits, nothing closes.
        assert_eq!(file_tab_eviction(&facts, 0), None);

        facts.push(file("/repo/last.rs", false));
        // At the cap: the leftmost file tab (index 1) is the victim.
        assert_eq!(file_tab_eviction(&facts, 0), Some(1));
        // ... unless it is the active one, which is skipped for the next.
        assert_eq!(file_tab_eviction(&facts, 1), Some(2));
    }

    /// US-017: the cap never drops unsaved work - a modified tab is skipped,
    /// and an all-modified strip evicts nothing at all.
    #[test]
    fn the_cap_never_evicts_a_modified_tab() {
        let mut facts = vec![DiffTabFact::Fixed];
        facts.extend((0..MAX_DIFF_FILE_TABS).map(|i| file(&format!("/repo/{i}.rs"), true)));
        assert_eq!(
            file_tab_eviction(&facts, 0),
            None,
            "every file tab is modified: the cap gives way rather than losing edits"
        );

        // A single saved tab in the middle is the one that goes.
        facts[3] = file("/repo/saved.rs", false);
        assert_eq!(file_tab_eviction(&facts, 0), Some(3));
    }

    /// US-017: closing a modified tab asks for confirmation; the second press
    /// on the armed control goes through. Saved tabs and terminals close on
    /// the first press.
    #[test]
    fn closing_a_modified_tab_arms_before_it_closes() {
        let facts = [
            DiffTabFact::Fixed,
            file("/repo/dirty.rs", true),
            file("/repo/saved.rs", false),
            DiffTabFact::Fixed,
        ];

        assert!(close_arms_first(&facts, 1, None));
        // Armed on that very tab: the confirming press closes.
        assert!(!close_arms_first(&facts, 1, Some(1)));
        // An arm on another tab does not confirm this one.
        assert!(close_arms_first(&facts, 1, Some(2)));
        // Nothing else ever asks.
        assert!(!close_arms_first(&facts, 2, None));
        assert!(!close_arms_first(&facts, 3, None));
        // Out of bounds is inert rather than a panic.
        assert!(!close_arms_first(&facts, 9, None));
    }

    /// US-017: closing a tab leaves `diff_active_tab` inside the strip, and
    /// the `Changes` tab is always a valid landing spot.
    #[test]
    fn the_active_index_stays_in_bounds_after_a_close() {
        // Closing the active tab falls back to the previous one.
        assert_eq!(active_tab_after_close(3, 3), 2);
        // Closing before the active tab shifts it up.
        assert_eq!(active_tab_after_close(3, 1), 2);
        // Closing after it leaves it alone.
        assert_eq!(active_tab_after_close(1, 2), 1);
        // The last remaining non-permanent tab lands back on `Changes`.
        assert_eq!(active_tab_after_close(1, 1), 0);

        // Exhaustive: for any strip up to 12 tabs, closing any closable index
        // leaves an index inside the shortened strip.
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
