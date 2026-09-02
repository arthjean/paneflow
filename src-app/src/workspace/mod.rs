mod git;
pub mod pid_resolve;
mod ports;
pub mod surface_naming;
mod tab;
pub mod worktree;

pub use git::{
    GitDiffStats, detect_branch, find_git_dir, resolve_repo_root, resolve_worktree_root,
};
#[cfg(test)]
pub(crate) use ports::PortEntry;
pub use ports::{PaneScan, scan_panes};
pub use tab::Tab;

pub(crate) const MAX_WORKSPACES: usize = 20;

pub(crate) const MAX_TABS_PER_WORKSPACE: usize = 32;

use gpui::{App, Entity, Window};
use paneflow_config::schema::{ButtonCommand, LayoutNode, TabSession};

use crate::ai_types::AgentSession;
use crate::launch_cwd;
use crate::layout::LayoutTree;
use crate::pane::Pane;

use self::git::parse_head;

static NEXT_WORKSPACE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn next_workspace_id() -> u64 {
    NEXT_WORKSPACE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Default)]
pub(crate) struct AgentCompletionNotification {
    unread: std::collections::HashSet<Option<u64>>,
}

impl AgentCompletionNotification {
    pub(crate) fn record_finished(&mut self, seen: bool, surface_id: Option<u64>) {
        if seen {
            self.unread.remove(&surface_id);
        } else {
            self.unread.insert(surface_id);
        }
    }

    pub(crate) fn acknowledge(
        &mut self,
        seen: &std::collections::HashSet<u64>,
        live: &std::collections::HashSet<u64>,
    ) {
        self.unread
            .retain(|key| key.is_some_and(|id| live.contains(&id) && !seen.contains(&id)));
    }

    pub(crate) fn is_unread(&self) -> bool {
        !self.unread.is_empty()
    }

    pub(crate) fn has_unattributed_unread(&self) -> bool {
        self.unread.contains(&None)
    }

    pub(crate) fn is_unread_for(&self, surfaces: &std::collections::HashSet<u64>) -> bool {
        self.unread
            .iter()
            .any(|key| key.is_some_and(|id| surfaces.contains(&id)))
    }
}

pub struct Workspace {
    pub id: u64,
    pub title: String,
    pub cwd: String,
    tabs: Vec<Tab>,
    active_tab_idx: usize,
    pub git_stats: GitDiffStats,
    pub git_branch: String,
    pub is_git_repo: bool,
    pub git_dir: Option<std::path::PathBuf>,
    pub repo_root: Option<std::path::PathBuf>,
    #[allow(dead_code)]
    pub is_worktree: bool,
    pub worktree_root: std::path::PathBuf,
    pub active_ports: Vec<u16>,
    pub port_scan_generation: u64,
    pub port_scan_pending: bool,
    pub service_labels: std::collections::HashMap<u16, crate::terminal::ServiceInfo>,
    pub agent_sessions: std::collections::HashMap<u32, AgentSession>,
    pub(crate) agent_completion_notification: AgentCompletionNotification,
    pub detected_agents: std::collections::HashSet<String>,
    pub custom_buttons: Vec<ButtonCommand>,
    pub files_expanded: Vec<std::path::PathBuf>,
    pub managed_worktrees: Vec<worktree::ManagedWorktree>,
    pub sidebar_expanded: bool,
}

impl Workspace {
    fn build(id: u64, title: String, cwd: String, root: LayoutTree) -> Self {
        Self::build_with_tab(id, title, cwd, Tab::new(String::new(), Some(root)))
    }

    fn build_with_tab(id: u64, title: String, cwd: String, tab: Tab) -> Self {
        let git_dir = find_git_dir(&cwd);
        let (git_branch, is_git_repo) = match &git_dir {
            Some(dir) => parse_head(dir),
            None => (String::new(), false),
        };
        let (repo_root, is_worktree) = match &git_dir {
            Some(dir) => resolve_repo_root(dir),
            None => (None, false),
        };
        let worktree_root =
            git::resolve_worktree_root(&cwd, git_dir.as_deref(), repo_root.as_deref(), is_worktree);
        Self {
            id,
            title,
            cwd,
            tabs: vec![tab],
            active_tab_idx: 0,
            git_stats: GitDiffStats::default(),
            git_branch,
            is_git_repo,
            git_dir,
            repo_root,
            is_worktree,
            worktree_root,
            active_ports: vec![],
            port_scan_generation: 0,
            port_scan_pending: false,
            service_labels: std::collections::HashMap::new(),
            agent_sessions: std::collections::HashMap::new(),
            agent_completion_notification: AgentCompletionNotification::default(),
            detected_agents: std::collections::HashSet::new(),
            custom_buttons: Vec::new(),
            files_expanded: Vec::new(),
            managed_worktrees: Vec::new(),
            sidebar_expanded: true,
        }
    }

    pub fn with_id(id: u64, title: impl Into<String>, pane: Entity<Pane>) -> Self {
        let cwd = launch_cwd::implicit_launch_cwd().display().to_string();
        Self::build(id, title.into(), cwd, LayoutTree::Leaf(pane))
    }

    pub fn with_cwd_and_id(
        id: u64,
        title: impl Into<String>,
        cwd: std::path::PathBuf,
        pane: Entity<Pane>,
    ) -> Self {
        Self::build(
            id,
            title.into(),
            cwd.display().to_string(),
            LayoutTree::Leaf(pane),
        )
    }

    pub fn empty_with_cwd_and_id(
        id: u64,
        title: impl Into<String>,
        cwd: std::path::PathBuf,
    ) -> Self {
        Self::build_with_tab(id, title.into(), cwd.display().to_string(), Tab::empty())
    }

    #[cfg(test)]
    pub fn with_layout_and_id(
        id: u64,
        title: impl Into<String>,
        cwd: std::path::PathBuf,
        root: LayoutTree,
    ) -> Self {
        Self::build(id, title.into(), cwd.display().to_string(), root)
    }

    pub fn restored_with_id(
        id: u64,
        title: impl Into<String>,
        cwd: std::path::PathBuf,
        mut tabs: Vec<Tab>,
        active_tab: usize,
    ) -> Self {
        if tabs.len() > MAX_TABS_PER_WORKSPACE {
            log::warn!(
                "session restore: workspace holds {} tabs, keeping the first {}",
                tabs.len(),
                MAX_TABS_PER_WORKSPACE
            );
            tabs.truncate(MAX_TABS_PER_WORKSPACE);
        }
        let first = if tabs.is_empty() {
            Tab::empty()
        } else {
            tabs.remove(0)
        };
        let mut ws = Self::build_with_tab(id, title.into(), cwd.display().to_string(), first);
        ws.tabs.append(&mut tabs);
        ws.active_tab_idx = active_tab.min(ws.tabs.len().saturating_sub(1));
        ws
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab_idx(&self) -> usize {
        self.active_tab_idx.min(self.tabs.len().saturating_sub(1))
    }

    pub fn active_tab(&self) -> &Tab {
        let idx = self.active_tab_idx();
        debug_assert!(!self.tabs.is_empty(), "workspace must keep one tab");
        &self.tabs[idx]
    }

    pub fn active_tab_mut(&mut self) -> &mut Tab {
        let idx = self.active_tab_idx();
        debug_assert!(!self.tabs.is_empty(), "workspace must keep one tab");
        &mut self.tabs[idx]
    }

    pub fn tab_mut(&mut self, idx: usize) -> Option<&mut Tab> {
        self.tabs.get_mut(idx)
    }

    pub fn set_active_tab(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.active_tab_idx = idx;
        }
    }

    pub fn tab_index_containing_pane(&self, pane: &Entity<Pane>) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.contains_pane(pane))
    }

    pub fn is_empty_shell(&self) -> bool {
        match self.tabs.as_slice() {
            [tab] => tab.title().is_empty() && tab.root.is_none() && tab.saved_layout.is_none(),
            _ => false,
        }
    }

    pub fn open_tab(&mut self, tab: Tab) -> bool {
        if self.is_empty_shell() {
            self.tabs[0] = tab;
            self.active_tab_idx = 0;
            return true;
        }
        if self.tabs.len() >= MAX_TABS_PER_WORKSPACE {
            log::warn!(
                "workspace {}: tab limit reached ({MAX_TABS_PER_WORKSPACE}), refusing to open a new tab",
                self.id
            );
            return false;
        }
        self.tabs.push(tab);
        self.active_tab_idx = self.tabs.len() - 1;
        true
    }

    pub fn can_open_tab(&self) -> bool {
        self.tabs.len() < MAX_TABS_PER_WORKSPACE
    }

    pub fn reorder_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to > self.tabs.len() || from == to {
            return;
        }
        let active_id = self.tabs[self.active_tab_idx()].id;
        let tab = self.tabs.remove(from);
        let insert_at = to.min(self.tabs.len());
        self.tabs.insert(insert_at, tab);
        if let Some(idx) = self.tabs.iter().position(|tab| tab.id == active_id) {
            self.active_tab_idx = idx;
        }
    }

    pub fn close_tab(&mut self, idx: usize) -> Option<Tab> {
        if idx >= self.tabs.len() {
            return None;
        }
        let removed = self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.tabs.push(Tab::empty());
            self.active_tab_idx = 0;
        } else if self.active_tab_idx >= self.tabs.len() {
            self.active_tab_idx = self.tabs.len() - 1;
        } else if self.active_tab_idx > idx {
            self.active_tab_idx -= 1;
        }
        Some(removed)
    }

    pub fn is_zoomed(&self) -> bool {
        self.active_tab().is_zoomed()
    }

    pub fn exit_zoom(&mut self, cx: &mut App) -> Option<Entity<Pane>> {
        self.active_tab_mut().exit_zoom(cx)
    }

    pub fn pane_count(&self) -> usize {
        self.tabs.iter().map(Tab::pane_count).sum()
    }

    pub fn any_pane(&self, mut f: impl FnMut(&Entity<Pane>) -> bool) -> bool {
        self.tabs.iter().any(|tab| tab.any_pane(&mut f))
    }

    pub fn collect_panes(&self) -> Vec<Entity<Pane>> {
        let mut panes = Vec::new();
        for tab in &self.tabs {
            for pane in tab.collect_panes() {
                if !panes.contains(&pane) {
                    panes.push(pane);
                }
            }
        }
        panes
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        self.active_tab().focus_first(window, cx);
    }

    pub fn serialize_layout(&self, cx: &App) -> Option<LayoutNode> {
        self.active_tab().serialize(cx)
    }

    pub fn tab_for_pane(&self, pane: &gpui::Entity<Pane>) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.contains_pane(pane))
    }

    pub fn bound_tab_worktrees(&self) -> Vec<String> {
        self.tabs
            .iter()
            .filter_map(|tab| tab.worktree.as_ref())
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }

    pub fn serialize_tabs_without_scrollback(&self, cx: &App) -> Vec<TabSession> {
        self.tabs
            .iter()
            .map(|tab| TabSession {
                title: tab.title().to_string(),
                title_source: Some(tab.title_source()),
                layout: tab.serialize_without_scrollback(cx),
                worktree: tab
                    .worktree
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
            })
            .collect()
    }
}

impl Workspace {
    pub fn propagate_config(&self, config: &paneflow_config::schema::PaneFlowConfig, cx: &mut App) {
        for tab in &self.tabs {
            if let Some(root) = &tab.root {
                walk_and_push_config(root, config, cx);
            }
            if let Some(saved) = &tab.saved_layout {
                walk_and_push_config(saved, config, cx);
            }
        }
    }
}

fn walk_and_push_config(
    node: &LayoutTree,
    config: &paneflow_config::schema::PaneFlowConfig,
    cx: &mut App,
) {
    match node {
        LayoutTree::Leaf(pane) => {
            pane.update(cx, |p, cx| {
                p.apply_config(config, cx);
            });
        }
        LayoutTree::Container { children, .. } => {
            for child in children {
                walk_and_push_config(&child.node, config, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, TestAppContext};

    use std::collections::HashSet;

    use super::{AgentCompletionNotification, MAX_TABS_PER_WORKSPACE, Tab, Workspace};
    use crate::layout::LayoutTree;
    use crate::terminal::TerminalView;

    fn test_workspace(cx: &mut impl AppContext) -> Workspace {
        let terminal = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
        let pane = cx.new(|cx| crate::pane::Pane::new(terminal, 1, cx));
        Workspace::build(1, "ws".to_string(), String::new(), LayoutTree::Leaf(pane))
    }

    #[gpui::test]
    fn workspace_keeps_one_tab_when_the_last_one_is_closed(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let mut ws = test_workspace(cx);
        assert_eq!(ws.tab_count(), 1);

        let removed = ws.close_tab(0);

        assert!(removed.is_some(), "closing the only tab must yield it");
        assert_eq!(ws.tab_count(), 1, "workspace must never hold zero tabs");
        assert_eq!(ws.active_tab_idx(), 0);
        assert!(
            ws.active_tab().root.is_none(),
            "the substitute tab is empty"
        );
        assert_eq!(ws.pane_count(), 0);
    }

    #[gpui::test]
    fn closing_a_tab_left_of_the_active_one_keeps_the_same_tab_visible(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let mut ws = test_workspace(cx);
        assert!(ws.open_tab(Tab::new("second", None)));
        let active_id = ws.active_tab().id;
        assert_eq!(ws.active_tab_idx(), 1);

        ws.close_tab(0);

        assert_eq!(ws.tab_count(), 1);
        assert_eq!(ws.active_tab().id, active_id);
        assert_eq!(ws.active_tab_idx(), 0);
    }

    #[gpui::test]
    fn opening_beyond_the_tab_cap_is_refused_without_mutation(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let mut ws = test_workspace(cx);
        for i in 1..MAX_TABS_PER_WORKSPACE {
            assert!(ws.open_tab(Tab::new(format!("t{i}"), None)), "tab {i}");
        }
        assert_eq!(ws.tab_count(), MAX_TABS_PER_WORKSPACE);
        let active_before = ws.active_tab().id;

        let accepted = ws.open_tab(Tab::new("overflow", None));

        assert!(!accepted, "the cap must refuse the extra tab");
        assert_eq!(ws.tab_count(), MAX_TABS_PER_WORKSPACE);
        assert_eq!(
            ws.active_tab().id,
            active_before,
            "a refused open must not move the active tab"
        );
    }

    #[gpui::test]
    fn zoom_is_per_tab(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let mut ws = test_workspace(cx);
        let pane = ws.active_tab().root.as_ref().unwrap().first_leaf().unwrap();
        let first_tab = ws.active_tab_idx();

        let full = ws.active_tab_mut().root.take().unwrap();
        ws.active_tab_mut().saved_layout = Some(full);
        ws.active_tab_mut().root = Some(LayoutTree::Leaf(pane));
        assert!(ws.is_zoomed());

        assert!(ws.open_tab(Tab::new("second", None)));
        assert!(!ws.is_zoomed(), "a fresh tab is not zoomed");

        ws.set_active_tab(first_tab);
        assert!(ws.is_zoomed(), "returning to the tab restores its zoom");
    }

    #[gpui::test]
    fn reorder_tab_keeps_the_same_tab_visible(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let mut ws = test_workspace(cx);
        assert!(ws.open_tab(Tab::new("second", None)));
        assert!(ws.open_tab(Tab::new("third", None)));
        let ids: Vec<u64> = ws.tabs().iter().map(|tab| tab.id).collect();
        ws.set_active_tab(0);

        ws.reorder_tab(0, 2);

        assert_eq!(
            ws.tabs().iter().map(|tab| tab.id).collect::<Vec<_>>(),
            vec![ids[1], ids[2], ids[0]]
        );
        assert_eq!(ws.active_tab().id, ids[0]);
        assert_eq!(ws.active_tab_idx(), 2);

        ws.reorder_tab(2, 2);
        ws.reorder_tab(9, 0);
        assert_eq!(
            ws.tabs().iter().map(|tab| tab.id).collect::<Vec<_>>(),
            vec![ids[1], ids[2], ids[0]]
        );
        assert_eq!(ws.active_tab().id, ids[0]);
    }

    #[gpui::test]
    fn can_open_tab_reports_the_cap_before_a_move_detaches_anything(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let mut ws = test_workspace(cx);
        assert!(ws.can_open_tab());
        for i in 1..MAX_TABS_PER_WORKSPACE {
            assert!(ws.open_tab(Tab::new(format!("t{i}"), None)), "tab {i}");
        }
        assert!(!ws.can_open_tab());
        assert!(!ws.open_tab(Tab::new("overflow", None)));
    }

    #[gpui::test]
    fn a_new_workspace_is_an_empty_folder(cx: &mut TestAppContext) {
        let _ = cx.add_empty_window();
        let ws = Workspace::empty_with_cwd_and_id(7, "project", std::path::PathBuf::from("/tmp"));

        assert!(ws.is_empty_shell(), "an opened folder starts empty");
        assert_eq!(ws.tab_count(), 1, "FR-01: one tab always exists");
        assert_eq!(ws.pane_count(), 0, "no pane means no PTY was spawned");
        assert!(ws.active_tab().root.is_none());
    }

    #[gpui::test]
    fn the_first_tab_of_an_empty_workspace_replaces_the_placeholder(cx: &mut TestAppContext) {
        let _ = cx.add_empty_window();
        let mut ws =
            Workspace::empty_with_cwd_and_id(7, "project", std::path::PathBuf::from("/tmp"));

        assert!(ws.open_tab(Tab::new("first", None)));

        assert_eq!(
            ws.tab_count(),
            1,
            "the placeholder is filled, not pushed past"
        );
        assert_eq!(ws.active_tab_idx(), 0);
        assert_eq!(ws.active_tab().title(), "first");
        assert!(!ws.is_empty_shell());

        assert!(ws.open_tab(Tab::new("second", None)));
        assert_eq!(ws.tab_count(), 2, "later tabs append as usual");
        assert_eq!(ws.active_tab_idx(), 1);
    }

    #[gpui::test]
    fn a_workspace_holding_a_pane_is_not_an_empty_shell(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let ws = test_workspace(cx);

        assert!(!ws.is_empty_shell());
    }

    #[test]
    fn agent_completion_is_unread_only_while_the_surface_is_not_seen() {
        let mut notification = AgentCompletionNotification::default();
        assert!(!notification.is_unread());

        notification.record_finished(false, Some(7));
        assert!(notification.is_unread());

        notification.record_finished(true, Some(7));
        assert!(!notification.is_unread());

        notification.record_finished(false, Some(7));
        notification.acknowledge(&HashSet::from([7]), &HashSet::from([7]));
        assert!(!notification.is_unread());
    }

    #[test]
    fn a_completion_is_claimed_by_the_tab_that_owns_its_surface() {
        let mut notification = AgentCompletionNotification::default();
        notification.record_finished(false, Some(7));

        let owning_tab = HashSet::from([7u64]);
        let sibling_tab = HashSet::from([8u64]);
        let both = HashSet::from([7u64, 8]);
        assert!(notification.is_unread_for(&owning_tab));
        assert!(!notification.is_unread_for(&sibling_tab));
        assert!(!notification.has_unattributed_unread());

        notification.acknowledge(&sibling_tab, &both);
        assert!(notification.is_unread_for(&owning_tab));

        notification.acknowledge(&owning_tab, &both);
        assert!(!notification.is_unread());
    }

    #[test]
    fn an_unattributed_completion_stays_on_the_folder_row() {
        let mut notification = AgentCompletionNotification::default();
        notification.record_finished(false, None);

        assert!(notification.has_unattributed_unread());
        assert!(notification.is_unread());
        assert!(!notification.is_unread_for(&HashSet::from([7u64])));

        notification.acknowledge(&HashSet::from([7u64]), &HashSet::from([7u64]));
        assert!(!notification.is_unread());
    }

    #[test]
    fn a_mark_left_by_a_closed_tab_is_retired_rather_than_stranded() {
        let mut notification = AgentCompletionNotification::default();
        notification.record_finished(false, Some(7));

        let live = HashSet::from([8u64]);
        notification.acknowledge(&live, &live);
        assert!(!notification.is_unread());
    }
}
