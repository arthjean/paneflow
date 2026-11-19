use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{AppContext, Context, Focusable, Window};

use super::code::view::{CodeView, CodeViewEvent, FilePosition, FileReveal};
use super::model::DiffDockTab;
use crate::PaneFlowApp;
use crate::app::close_policy::CloseTarget;
use crate::terminal::{TerminalEvent, TerminalView};

const CLOSE_ARM_DELAY: Duration = Duration::from_millis(400);
const CLOSE_ARM_EXPIRY: Duration = Duration::from_secs(4);
pub(crate) const CLOSED_FILE_TABS_KEPT: usize = 20;
pub(crate) const FILE_POSITIONS_KEPT: usize = 200;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FilePositions(VecDeque<(PathBuf, FilePosition)>);

impl FilePositions {
    pub(crate) fn record(&mut self, path: PathBuf, position: FilePosition) {
        self.0.retain(|(known, _)| *known != path);
        self.0.push_front((path, position));
        self.0.truncate(FILE_POSITIONS_KEPT);
    }

    pub(crate) fn get(&self, path: &Path) -> Option<FilePosition> {
        self.0
            .iter()
            .find(|(known, _)| known == path)
            .map(|(_, position)| *position)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClosedFileTab {
    pub(crate) path: PathBuf,
    pub(crate) scroll_line: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ClosedFileTabs(VecDeque<ClosedFileTab>);

impl ClosedFileTabs {
    pub(crate) fn record(&mut self, closed: ClosedFileTab) {
        self.0.retain(|known| known.path != closed.path);
        self.0.push_front(closed);
        self.0.truncate(CLOSED_FILE_TABS_KEPT);
    }

    pub(crate) fn take_latest(&mut self) -> Option<ClosedFileTab> {
        self.0.pop_front()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn paths(&self) -> Vec<PathBuf> {
        self.0.iter().map(|closed| closed.path.clone()).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabBatch {
    Others(usize),
    ToTheRight(usize),
    All,
}

impl PaneFlowApp {
    pub(crate) fn diff_file_tab_active(&self) -> bool {
        matches!(
            self.diff_dock.diff_tabs.get(self.diff_dock.diff_active_tab),
            Some(DiffDockTab::File(_) | DiffDockTab::PendingFile)
        )
    }

    pub(crate) fn diff_files_tab_active(&self) -> bool {
        matches!(
            self.diff_dock.diff_tabs.get(self.diff_dock.diff_active_tab),
            Some(DiffDockTab::PendingFile)
        )
    }

    pub(crate) fn open_diff_changes_tab(&mut self, cx: &mut Context<Self>) {
        let index = self
            .diff_dock
            .diff_tabs
            .iter()
            .position(|tab| matches!(tab, DiffDockTab::Changes));
        let index = index.unwrap_or_else(|| {
            self.diff_dock.diff_tabs.push(DiffDockTab::Changes);
            self.diff_dock.diff_tabs.len() - 1
        });
        self.select_diff_tab(index, cx);
    }

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

        let terminal = cx.new(|cx| {
            TerminalView::spawned(
                ws_id,
                cwd,
                None,
                paneflow_config::schema::TerminalSurfaceProfile::Normal,
                cx,
            )
        });
        cx.subscribe(&terminal, |_, _, event: &TerminalEvent, cx| {
            if matches!(event, TerminalEvent::ChildExited) {
                cx.notify();
            }
        })
        .detach();

        let focus = terminal.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.diff_dock
            .diff_tabs
            .push(DiffDockTab::Terminal(terminal));
        self.select_diff_tab(self.diff_dock.diff_tabs.len() - 1, cx);
    }

    pub(crate) fn open_diff_file_tab(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_diff_file_tab_at(path, None, None, window, cx);
    }

    pub(crate) fn open_diff_file_tab_at(
        &mut self,
        path: PathBuf,
        scroll_line: Option<f64>,
        reveal: Option<FileReveal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.record_dock_file_open(&path);
        let pending = self.pending_file_tab();
        if let Some(index) = pending {
            self.diff_dock.diff_tabs.remove(index);
        }
        let index = match file_open_plan(&self.diff_tab_facts(cx), &path, pending) {
            FileOpen::Activate(index) => {
                if let (Some(reveal), Some(DiffDockTab::File(view))) =
                    (reveal, self.diff_dock.diff_tabs.get(index))
                {
                    view.update(cx, |view, cx| view.reveal(reveal, cx));
                }
                index
            }
            FileOpen::Insert(index) => {
                let remembered = self.diff_dock.file_positions.get(&path);
                let view = self.new_code_view(path, false, cx);
                view.update(cx, |view, cx| {
                    if let Some(position) = remembered {
                        view.restore_position(position);
                    }
                    if let Some(line) = scroll_line {
                        view.restore_scroll_line(line);
                    }
                    if let Some(reveal) = reveal {
                        view.reveal(reveal, cx);
                    }
                });
                self.diff_dock
                    .diff_tabs
                    .insert(index, DiffDockTab::File(view));
                index
            }
        };
        self.select_diff_tab(index, cx);
        self.focus_diff_tab(index, window, cx);
    }

    pub(crate) fn new_code_view(
        &mut self,
        path: PathBuf,
        deferred: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<CodeView> {
        let view = cx.new(|cx| {
            if deferred {
                CodeView::deferred(path, cx)
            } else {
                CodeView::new(path, cx)
            }
        });
        let controls = view.read(cx).controls.clone();
        cx.subscribe(
            &controls,
            |this, _, display: &super::code::controls::EditorDisplay, cx| {
                this.persist_setting(false, "editor", display.to_config_value(), cx);
            },
        )
        .detach();
        cx.subscribe(&view, |this, _, event: &CodeViewEvent, cx| {
            this.handle_code_view_event(event, cx);
        })
        .detach();
        view
    }

    pub(crate) fn handle_code_view_event(&mut self, event: &CodeViewEvent, cx: &mut Context<Self>) {
        match event {
            CodeViewEvent::Toast(message) => self.show_toast(message.to_string(), cx),
            CodeViewEvent::DisplayChanged(display) => {
                self.persist_setting(false, "editor", display.to_config_value(), cx);
            }
            CodeViewEvent::OpenWithMenu { path, position } => {
                let root = self
                    .active_workspace()
                    .map(|ws| PathBuf::from(&ws.cwd))
                    .unwrap_or_default();
                self.dismiss_transient_surfaces();
                self.files_menu_open = Some(crate::FilesContextMenu {
                    root,
                    path: path.clone(),
                    position: *position,
                    scope: crate::app::window::FilesMenuScope::OpenWith,
                });
                cx.notify();
            }
        }
    }

    fn pending_file_tab(&self) -> Option<usize> {
        self.diff_dock
            .diff_tabs
            .iter()
            .position(|tab| matches!(tab, DiffDockTab::PendingFile))
    }

    pub(super) fn diff_tab_facts(&self, cx: &gpui::App) -> Vec<DiffTabFact> {
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
                DiffDockTab::PendingFile => DiffTabFact::Files,
                DiffDockTab::Changes | DiffDockTab::Terminal(_) => DiffTabFact::Fixed,
            })
            .collect()
    }

    pub(crate) fn focus_diff_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = match self.diff_dock.diff_tabs.get(index) {
            Some(DiffDockTab::File(view)) => Some(view.read(cx).focus_handle(cx)),
            Some(DiffDockTab::Terminal(terminal)) => Some(terminal.read(cx).focus_handle(cx)),
            Some(DiffDockTab::PendingFile) if self.files_sidebar_open => {
                Some(self.files_sidebar.read(cx).focus_handle(cx))
            }
            _ => None,
        };
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        } else {
            window.blur(cx);
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
                self.select_diff_tab(self.diff_dock.diff_tabs.len() - 1, cx);
            }
        }
        if self.files_sidebar_open {
            self.focus_files_sidebar(window, cx);
        }
    }

    pub(crate) fn select_diff_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.diff_dock.diff_tabs.len() {
            self.diff_dock.picker = false;
            self.diff_dock.picked = true;
            self.diff_dock.diff_active_tab = index;
            self.diff_dock.diff_tab_close_armed = None;
            self.diff_dock.vertical_scrollbar.cancel_drag();
            self.diff_dock.tab_scroll.scroll_to_item(index);
            self.diff_dock.crumb_menu = None;
            self.diff_dock.crumb_scroll_pending = true;
            if matches!(
                self.diff_dock.diff_tabs.get(index),
                Some(DiffDockTab::PendingFile)
            ) && !self.files_sidebar_open
            {
                self.toggle_files_sidebar(cx);
            }
            self.reveal_active_file_in_tree(cx);
            self.save_session(cx);
            cx.notify();
        }
    }

    pub(crate) fn request_close_diff_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.diff_dock.diff_tabs.len() {
            return;
        }
        let now = Instant::now();
        match close_click(
            &self.diff_tab_facts(cx),
            index,
            self.diff_dock.diff_tab_close_armed,
            now,
        ) {
            CloseClick::Close => {}
            CloseClick::Ignore => return,
            CloseClick::Arm => {
                self.diff_dock.diff_tab_close_armed = Some((index, now));
                cx.spawn(
                    async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                        cx.background_executor().timer(CLOSE_ARM_EXPIRY).await;
                        let _ = this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                            if app.diff_dock.diff_tab_close_armed == Some((index, now)) {
                                app.diff_dock.diff_tab_close_armed = None;
                                cx.notify();
                            }
                        });
                    },
                )
                .detach();
                cx.notify();
                return;
            }
        }
        self.request_close(CloseTarget::DiffTerminal(index), Some(window), cx);
    }

    pub(crate) fn close_diff_tab_batch(
        &mut self,
        batch: TabBatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let plan = plan_batch_close(
            &self.diff_tab_facts(cx),
            batch,
            self.diff_dock.diff_active_tab,
        );
        for index in &plan.close {
            self.remove_diff_tab(*index, cx);
        }
        if let Some(active) = plan.active_after
            && active < self.diff_dock.diff_tabs.len()
        {
            self.select_diff_tab(active, cx);
            self.focus_diff_tab(active, window, cx);
        }
        if plan.kept_modified > 0 {
            self.show_toast(
                format!(
                    "{} kept",
                    crate::app::plural(plan.kept_modified, "modified tab", "modified tabs")
                ),
                cx,
            );
        }
        cx.notify();
    }

    pub(crate) fn reopen_closed_diff_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(closed) = self.diff_dock.closed_files.take_latest() else {
            return;
        };
        cx.notify();
        let window = window.window_handle();
        let probe = closed.path.clone();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let exists = smol::unblock(move || probe.is_file()).await;
                cx.update(|cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        let _ = this.update(cx, |app, cx| {
                            if exists {
                                app.diff_dock.picker = false;
                                app.diff_dock.picked = true;
                                app.open_diff_file_tab_at(
                                    closed.path,
                                    Some(closed.scroll_line),
                                    None,
                                    window,
                                    cx,
                                );
                            } else {
                                app.show_toast("File no longer exists", cx);
                            }
                        });
                    });
                });
            },
        )
        .detach();
    }

    pub(crate) fn remove_diff_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.diff_dock.diff_tabs.len() {
            return;
        }
        let closed = self.diff_dock.diff_tabs.remove(index);
        if let DiffDockTab::File(view) = &closed {
            let view = view.read(cx);
            self.diff_dock.closed_files.record(ClosedFileTab {
                path: view.path().to_path_buf(),
                scroll_line: view.current_scroll_line(),
            });
            if let Some(position) = view.position() {
                self.diff_dock
                    .file_positions
                    .record(view.path().to_path_buf(), position);
            }
        }
        if matches!(closed, DiffDockTab::File(_) | DiffDockTab::PendingFile)
            && !self
                .diff_dock
                .diff_tabs
                .iter()
                .any(|tab| matches!(tab, DiffDockTab::File(_) | DiffDockTab::PendingFile))
        {
            self.close_files_sidebar(cx);
        }
        self.diff_dock.diff_active_tab =
            active_tab_after_close(self.diff_dock.diff_active_tab, index);
        self.diff_dock.diff_tab_close_armed = None;
        self.diff_dock.vertical_scrollbar.cancel_drag();
        self.diff_dock.tab_menu = None;
        self.diff_dock.crumb_menu = None;
        if self.diff_dock.diff_tabs.is_empty() {
            self.diff_dock.picker = true;
            self.diff_dock.picked = false;
            self.diff_dock.picker_focus_pending = true;
        }
        self.close_diff_options_menu(cx);
        self.close_diff_new_tab_menu(cx);
        self.diff_dock.diff_branch_menu = None;
        self.save_session(cx);
        cx.notify();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DiffTabFact {
    Fixed,
    Files,
    File { path: PathBuf, dirty: bool },
}

impl DiffTabFact {
    fn batch_closable(&self) -> bool {
        matches!(self, Self::Files | Self::File { dirty: false, .. })
    }

    fn modified(&self) -> bool {
        matches!(self, Self::File { dirty: true, .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileOpen {
    Activate(usize),
    Insert(usize),
}

pub(super) fn file_open_plan(
    facts: &[DiffTabFact],
    path: &Path,
    pending: Option<usize>,
) -> FileOpen {
    if let Some(index) = file_tab_index(facts, path) {
        return FileOpen::Activate(index);
    }
    FileOpen::Insert(
        pending
            .filter(|index| *index <= facts.len())
            .unwrap_or(facts.len()),
    )
}

pub(super) fn file_tab_index(facts: &[DiffTabFact], path: &Path) -> Option<usize> {
    facts
        .iter()
        .position(|fact| matches!(fact, DiffTabFact::File { path: open, .. } if open == path))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BatchPlan {
    pub(super) close: Vec<usize>,
    pub(super) kept_modified: usize,
    pub(super) active_after: Option<usize>,
}

pub(super) fn plan_batch_close(facts: &[DiffTabFact], batch: TabBatch, active: usize) -> BatchPlan {
    let targeted = |index: usize| match batch {
        TabBatch::Others(keep) => index != keep,
        TabBatch::ToTheRight(from) => index > from,
        TabBatch::All => true,
    };
    let mut close = Vec::new();
    let mut kept_modified = 0;
    for (index, fact) in facts.iter().enumerate() {
        if !targeted(index) {
            continue;
        }
        if fact.batch_closable() {
            close.push(index);
        } else if fact.modified() {
            kept_modified += 1;
        }
    }
    let shifted = |index: usize| index - close.iter().filter(|closed| **closed < index).count();
    let remaining = facts.len() - close.len();
    let active_after = if remaining == 0 {
        None
    } else if !close.contains(&active) {
        Some(shifted(active))
    } else {
        match batch {
            TabBatch::Others(anchor) | TabBatch::ToTheRight(anchor) if anchor < facts.len() => {
                Some(shifted(anchor))
            }
            _ => Some(
                (0..active)
                    .rev()
                    .find(|index| !close.contains(index))
                    .map_or(0, shifted),
            ),
        }
    };
    close.reverse();
    BatchPlan {
        close,
        kept_modified,
        active_after,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CloseClick {
    Arm,
    Ignore,
    Close,
}

pub(super) fn close_click(
    facts: &[DiffTabFact],
    index: usize,
    armed: Option<(usize, Instant)>,
    now: Instant,
) -> CloseClick {
    let dirty = matches!(
        facts.get(index),
        Some(DiffTabFact::File { dirty: true, .. })
    );
    if !dirty {
        return CloseClick::Close;
    }
    match armed {
        Some((armed, at)) if armed == index => {
            let elapsed = now.saturating_duration_since(at);
            if elapsed < CLOSE_ARM_DELAY {
                CloseClick::Ignore
            } else if elapsed <= CLOSE_ARM_EXPIRY {
                CloseClick::Close
            } else {
                CloseClick::Arm
            }
        }
        _ => CloseClick::Arm,
    }
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

    fn files(count: usize) -> Vec<DiffTabFact> {
        (0..count)
            .map(|i| file(&format!("/repo/{i}.rs"), false))
            .collect()
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
        assert_eq!(
            file_open_plan(&facts, Path::new("/repo/README.md"), None),
            FileOpen::Activate(3),
            "a file already open is activated, never duplicated"
        );
    }

    #[test]
    fn a_ninth_and_a_fiftieth_file_open_beside_the_others() {
        let mut facts = files(8);
        assert_eq!(
            file_open_plan(&facts, Path::new("/repo/ninth.rs"), None),
            FileOpen::Insert(8)
        );
        facts.push(file("/repo/ninth.rs", false));
        assert_eq!(facts.len(), 9);

        let mut facts = files(49);
        assert_eq!(
            file_open_plan(&facts, Path::new("/repo/last.rs"), None),
            FileOpen::Insert(49)
        );
        facts.push(file("/repo/last.rs", false));
        assert_eq!(
            file_open_plan(&facts, Path::new("/repo/0.rs"), None),
            FileOpen::Activate(0),
            "fifty tabs keep the oldest one open"
        );
    }

    #[test]
    fn a_file_opened_from_the_files_tab_takes_its_place() {
        let facts = [DiffTabFact::Fixed, file("/repo/a.rs", false)];
        assert_eq!(
            file_open_plan(&facts, Path::new("/repo/b.rs"), Some(1)),
            FileOpen::Insert(1)
        );
        assert_eq!(
            file_open_plan(&facts, Path::new("/repo/b.rs"), Some(9)),
            FileOpen::Insert(2)
        );
    }

    #[test]
    fn a_batch_close_keeps_modified_tabs_and_the_fixed_surfaces() {
        let facts = [
            DiffTabFact::Fixed,
            file("/repo/a.rs", false),
            file("/repo/b.rs", true),
            DiffTabFact::Files,
            DiffTabFact::Fixed,
            file("/repo/c.rs", true),
            file("/repo/d.rs", false),
        ];

        let all = plan_batch_close(&facts, TabBatch::All, 6);
        assert_eq!(all.close, vec![6, 3, 1], "highest index first");
        assert_eq!(all.kept_modified, 2);
        assert_eq!(all.active_after, Some(3), "the nearest tab to the left");

        let others = plan_batch_close(&facts, TabBatch::Others(2), 2);
        assert_eq!(others.close, vec![6, 3, 1]);
        assert_eq!(others.kept_modified, 1);
        assert_eq!(others.active_after, Some(1), "the active tab survives");

        let others = plan_batch_close(&facts, TabBatch::Others(6), 3);
        assert_eq!(others.close, vec![3, 1]);
        assert_eq!(others.active_after, Some(4), "the anchor takes over");

        let right = plan_batch_close(&facts, TabBatch::ToTheRight(3), 6);
        assert_eq!(right.close, vec![6]);
        assert_eq!(right.kept_modified, 1);
        assert_eq!(right.active_after, Some(3));
    }

    #[test]
    fn closing_every_tab_leaves_no_active_tab() {
        let plan = plan_batch_close(&files(3), TabBatch::All, 1);
        assert_eq!(plan.close, vec![2, 1, 0]);
        assert_eq!(plan.active_after, None);
        assert_eq!(plan.kept_modified, 0);
    }

    #[test]
    fn file_positions_keep_the_two_hundred_latest_files_per_window() {
        let mut positions = FilePositions::default();
        let at = |line: usize| FilePosition {
            scroll_line: line as f64,
            line,
            column: 0,
        };
        for index in 0..FILE_POSITIONS_KEPT + 25 {
            positions.record(PathBuf::from(format!("/repo/{index}.rs")), at(index));
        }
        assert_eq!(positions.len(), FILE_POSITIONS_KEPT);
        assert_eq!(
            positions.get(Path::new("/repo/0.rs")),
            None,
            "the oldest is forgotten"
        );
        assert_eq!(
            positions.get(Path::new("/repo/224.rs")),
            Some(at(224)),
            "the newest is kept"
        );
        positions.record(PathBuf::from("/repo/30.rs"), at(7));
        assert_eq!(positions.get(Path::new("/repo/30.rs")), Some(at(7)));
        assert_eq!(
            positions.len(),
            FILE_POSITIONS_KEPT,
            "a re-record replaces its entry"
        );
    }

    #[test]
    fn closed_tabs_reopen_newest_first_and_keep_twenty() {
        let mut closed = ClosedFileTabs::default();
        assert!(closed.is_empty());
        for index in 0..CLOSED_FILE_TABS_KEPT + 5 {
            closed.record(ClosedFileTab {
                path: PathBuf::from(format!("/repo/{index}.rs")),
                scroll_line: index as f64,
            });
        }
        assert_eq!(closed.paths().len(), CLOSED_FILE_TABS_KEPT);
        let latest = closed.take_latest().expect("latest");
        assert_eq!(
            latest.path,
            PathBuf::from(format!("/repo/{}.rs", CLOSED_FILE_TABS_KEPT + 4))
        );
        assert_eq!(latest.scroll_line, (CLOSED_FILE_TABS_KEPT + 4) as f64);

        closed.record(ClosedFileTab {
            path: PathBuf::from("/repo/10.rs"),
            scroll_line: 3.0,
        });
        let paths = closed.paths();
        assert_eq!(paths[0], PathBuf::from("/repo/10.rs"));
        assert_eq!(
            paths
                .iter()
                .filter(|p| p.as_path() == Path::new("/repo/10.rs"))
                .count(),
            1,
            "a path closed twice is listed once"
        );
    }

    #[test]
    fn closing_a_modified_tab_arms_before_it_closes() {
        let facts = [
            DiffTabFact::Fixed,
            file("/repo/dirty.rs", true),
            file("/repo/saved.rs", false),
            DiffTabFact::Fixed,
        ];

        let at = Instant::now();
        let later = |ms: u64| at + Duration::from_millis(ms);

        assert_eq!(close_click(&facts, 1, None, at), CloseClick::Arm);
        assert_eq!(
            close_click(&facts, 1, Some((1, at)), later(500)),
            CloseClick::Close
        );
        assert_eq!(
            close_click(&facts, 1, Some((2, at)), later(500)),
            CloseClick::Arm
        );
        assert_eq!(close_click(&facts, 2, None, at), CloseClick::Close);
        assert_eq!(close_click(&facts, 3, None, at), CloseClick::Close);
        assert_eq!(close_click(&facts, 9, None, at), CloseClick::Close);
    }

    #[test]
    fn a_fast_double_click_never_closes_a_modified_tab() {
        let facts = [DiffTabFact::Fixed, file("/repo/dirty.rs", true)];
        let at = Instant::now();
        let later = |ms: u64| at + Duration::from_millis(ms);

        assert_eq!(
            close_click(&facts, 1, Some((1, at)), later(120)),
            CloseClick::Ignore
        );
        assert_eq!(
            close_click(&facts, 1, Some((1, at)), later(399)),
            CloseClick::Ignore
        );
        assert_eq!(
            close_click(&facts, 1, Some((1, at)), later(400)),
            CloseClick::Close
        );
        assert_eq!(
            close_click(&facts, 1, Some((1, at)), later(4000)),
            CloseClick::Close
        );
        assert_eq!(
            close_click(&facts, 1, Some((1, at)), later(4001)),
            CloseClick::Arm,
            "an arming older than four seconds has expired"
        );
    }

    #[test]
    fn the_active_index_stays_in_bounds_after_a_close() {
        assert_eq!(active_tab_after_close(3, 3), 2);
        assert_eq!(active_tab_after_close(3, 1), 2);
        assert_eq!(active_tab_after_close(1, 2), 1);
        assert_eq!(active_tab_after_close(1, 1), 0);
        assert_eq!(active_tab_after_close(0, 0), 0);
        assert_eq!(active_tab_after_close(1, 0), 0);

        for len in 2..=12usize {
            for closed in 0..len {
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
