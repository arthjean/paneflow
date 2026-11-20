use std::path::PathBuf;

use gpui::{AppContext as _, Context, Entity};

use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;
use crate::layout::{LayoutTree, SplitDirection};
use crate::pane::Pane;
use crate::terminal::TerminalView;
use crate::workspace::{Tab, Workspace};

pub(crate) struct PaneSpec {
    pub agent: Option<&'static str>,
    pub title: &'static str,
    pub output: Vec<u8>,
}

pub(crate) struct WorkspaceSpec {
    pub title: &'static str,
    pub cwd: String,
    pub git: Option<(usize, usize)>,
    pub tab: Option<&'static str>,
    pub expanded: bool,
}

/// The pane tree of the active workspace's tab.
pub(crate) enum Layout {
    /// One pane.
    Single(PaneSpec),
    /// A pane on the left, and a column of two on the right.
    LeftAndColumn {
        left: PaneSpec,
        top: PaneSpec,
        bottom: PaneSpec,
    },
}

pub(crate) struct WindowFixture {
    pub workspaces: Vec<WorkspaceSpec>,
    pub active: usize,
    pub layout: Layout,
    /// Index, in layout order, of the pane that holds focus.
    pub focused: usize,
}

pub(crate) struct Mounted {
    pub terminals: Vec<(Entity<TerminalView>, Vec<u8>)>,
    pub panes: Vec<Entity<Pane>>,
    pub focused: usize,
}

fn terminal(
    workspace_id: u64,
    spec: &PaneSpec,
    cx: &mut Context<PaneFlowApp>,
) -> (Entity<Pane>, Entity<TerminalView>) {
    let agent = spec.agent.and_then(TerminalAgent::from_binary);
    let title = spec.title.to_string();
    let terminal = cx.new(|cx| {
        let mut view = TerminalView::display_only_for_test(workspace_id, cx);
        view.terminal.detected_agent = agent;
        view.terminal.title = title;
        view
    });
    let pane = cx.new(|cx| Pane::new(terminal.clone(), workspace_id, cx));
    (pane, terminal)
}

pub(crate) fn install(
    app: &mut PaneFlowApp,
    fixture: WindowFixture,
    cx: &mut Context<PaneFlowApp>,
) -> Mounted {
    let mut terminals = Vec::new();
    let mut mounted_panes = Vec::new();
    let mut workspaces = Vec::new();
    let mut layout = Some(fixture.layout);
    for (index, spec) in fixture.workspaces.iter().enumerate() {
        let id = 9_000 + index as u64;
        let cwd = PathBuf::from(&spec.cwd);
        let mut workspace = match spec.tab {
            None => Workspace::empty_with_cwd_and_id(id, spec.title, cwd),
            Some(tab_title) => {
                let root = if index == fixture.active
                    && let Some(layout) = layout.take()
                {
                    let mut mount = |spec: PaneSpec| {
                        let (pane, view) = terminal(id, &spec, cx);
                        terminals.push((view, spec.output));
                        mounted_panes.push(pane.clone());
                        pane
                    };
                    match layout {
                        Layout::Single(spec) => LayoutTree::Leaf(mount(spec)),
                        Layout::LeftAndColumn { left, top, bottom } => {
                            let left = mount(left);
                            let top = mount(top);
                            let bottom = mount(bottom);
                            LayoutTree::new_split(
                                SplitDirection::Vertical,
                                LayoutTree::Leaf(left),
                                LayoutTree::new_split(
                                    SplitDirection::Horizontal,
                                    LayoutTree::Leaf(top),
                                    LayoutTree::Leaf(bottom),
                                ),
                            )
                        }
                    }
                } else {
                    let idle = PaneSpec {
                        agent: None,
                        title: "",
                        output: Vec::new(),
                    };
                    LayoutTree::Leaf(terminal(id, &idle, cx).0)
                };
                Workspace::restored_with_id(
                    id,
                    spec.title,
                    cwd,
                    vec![Tab::new(tab_title, Some(root))],
                    0,
                )
            }
        };
        if let Some((insertions, deletions)) = spec.git {
            workspace.is_git_repo = true;
            workspace.git_branch = "main".into();
            workspace.git_stats.files_changed = 1;
            workspace.git_stats.insertions = insertions;
            workspace.git_stats.deletions = deletions;
        }
        workspace.sidebar_expanded = spec.expanded;
        workspaces.push(workspace);
    }
    app.workspaces = workspaces;
    app.active_idx = fixture.active;
    app.usage
        .set_snapshot_for_test(vec![usage_account()], vec![usage_plan()]);
    Mounted {
        terminals,
        panes: mounted_panes,
        focused: fixture.focused,
    }
}

fn usage_account() -> crate::app::usage::model::InventoryEntry {
    use paneflow_agent_config::usage::accounts::{OriginKind, OriginRef, UsageAccount, UsageAgent};
    crate::app::usage::model::InventoryEntry {
        account: UsageAccount {
            agent: UsageAgent::ClaudeCode,
            config_dir: PathBuf::from("/home/arthur/.claude"),
            origins: vec![OriginRef {
                kind: OriginKind::Default,
                name: String::new(),
            }],
            label: "/home/arthur/.claude".into(),
            api_billing: false,
        },
        status_line: paneflow_agent_config::usage::status_line::StatusLineState::Managed,
    }
}

fn usage_plan() -> paneflow_serve::usage::plans::AccountPlan {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    serde_json::from_value(serde_json::json!({
        "agent": "claude-code",
        "config_dir": "/home/arthur/.claude",
        "label": "/home/arthur/.claude",
        "state": "reported",
        "primary": {"used_percent": 78.0, "window_secs": 18000, "resets_at": now + 7200},
        "secondary": {"used_percent": 41.0, "window_secs": 604800, "resets_at": now + 300000},
        "updated_at_ms": now * 1000
    }))
    .unwrap_or_else(|error| panic!("usage plan fixture: {error}"))
}
