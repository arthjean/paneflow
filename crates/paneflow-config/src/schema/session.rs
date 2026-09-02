use super::layout::{default_layout_pane, LayoutNode, SurfaceDefinition};
use serde::{Deserialize, Serialize};

pub const SESSION_SCHEMA_VERSION: u32 = 2;

pub const SESSION_SCHEMA_VERSION_V1: u32 = 1;

pub const MAX_SESSION_TABS: usize = 32;

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppMode {
    #[default]
    Cli,
    Diff,
}

impl<'de> Deserialize<'de> for AppMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ModeVisitor;

        impl serde::de::Visitor<'_> for ModeVisitor {
            type Value = AppMode;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a UI mode string")
            }

            fn visit_str<E>(self, value: &str) -> Result<AppMode, E>
            where
                E: serde::de::Error,
            {
                Ok(match value {
                    "diff" => AppMode::Diff,
                    _ => AppMode::Cli,
                })
            }
        }

        deserializer.deserialize_str(ModeVisitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionState {
    pub version: u32,
    pub active_workspace: usize,
    pub workspaces: Vec<WorkspaceSession>,
    #[serde(default)]
    pub mode: AppMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_scope: Option<String>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if hands the field by reference"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TabTitleSource {
    #[default]
    Preset,
    Prompt,
    Generated,
    User,
}

impl TabTitleSource {
    fn rank(self) -> u8 {
        match self {
            Self::Preset => 0,
            Self::Prompt => 1,
            Self::Generated => 2,
            Self::User => 3,
        }
    }

    fn replaces_itself(self) -> bool {
        matches!(self, Self::Generated | Self::User)
    }

    pub fn yields_to(self, incoming: Self) -> bool {
        incoming.rank() > self.rank() || (incoming == self && incoming.replaces_itself())
    }

    pub fn is_settled(self) -> bool {
        matches!(self, Self::Generated | Self::User)
    }
}

impl<'de> Deserialize<'de> for TabTitleSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SourceVisitor;

        impl serde::de::Visitor<'_> for SourceVisitor {
            type Value = TabTitleSource;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a tab title provenance string")
            }

            fn visit_str<E>(self, value: &str) -> Result<TabTitleSource, E>
            where
                E: serde::de::Error,
            {
                Ok(match value {
                    "preset" | "auto" => TabTitleSource::Preset,
                    "prompt" => TabTitleSource::Prompt,
                    "generated" => TabTitleSource::Generated,
                    _ => TabTitleSource::User,
                })
            }
        }

        deserializer.deserialize_str(SourceVisitor)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TabSession {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub title_source: Option<TabTitleSource>,
    #[serde(default)]
    pub layout: Option<LayoutNode>,
    #[serde(default)]
    pub worktree: Option<String>,
}

impl TabSession {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_layout(layout: LayoutNode) -> Self {
        Self {
            title: String::new(),
            title_source: Some(TabTitleSource::Preset),
            layout: Some(layout),
            worktree: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceSession {
    pub title: String,
    pub cwd: String,
    #[serde(default)]
    pub tabs: Vec<TabSession>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub active_tab: usize,
    #[serde(rename = "layout", default, skip_serializing_if = "Option::is_none")]
    pub legacy_layout: Option<LayoutNode>,
    #[serde(rename = "empty", default, skip_serializing_if = "is_false")]
    pub legacy_empty: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_buttons: Vec<ButtonCommand>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub managed_worktrees: Vec<ManagedWorktreeDef>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub sidebar_collapsed: bool,
}

pub fn migrate_session_v1(state: &mut SessionState) {
    for ws in &mut state.workspaces {
        migrate_workspace_v1(ws);
    }
    state.version = SESSION_SCHEMA_VERSION;
}

fn migrate_workspace_v1(ws: &mut WorkspaceSession) {
    let legacy_empty = std::mem::take(&mut ws.legacy_empty);
    let legacy_layout = ws.legacy_layout.take();
    if !ws.tabs.is_empty() {
        return;
    }
    let Some(mut root) = legacy_layout else {
        ws.tabs.push(if legacy_empty {
            TabSession::empty()
        } else {
            TabSession::with_layout(default_layout_pane())
        });
        return;
    };
    let mut promoted = Vec::new();
    demote_panes_to_focused_surface(&mut root, &mut promoted);
    ws.tabs.push(TabSession::with_layout(root));
    ws.tabs.append(&mut promoted);
    if ws.tabs.len() > MAX_SESSION_TABS {
        let dropped = ws.tabs.len() - MAX_SESSION_TABS;
        tracing::warn!(
            workspace = %ws.title,
            dropped,
            cap = MAX_SESSION_TABS,
            "session v1 migration: workspace exceeds the tab cap, surplus tabs dropped"
        );
        ws.tabs.truncate(MAX_SESSION_TABS);
    }
    ws.active_tab = 0;
}

fn demote_panes_to_focused_surface(node: &mut LayoutNode, promoted: &mut Vec<TabSession>) {
    match node {
        LayoutNode::Pane { surfaces } => {
            if surfaces.is_empty() {
                surfaces.push(SurfaceDefinition::default());
                return;
            }
            let focused = surfaces
                .iter()
                .position(|s| s.focus == Some(true))
                .unwrap_or(0);
            let mut drained: Vec<SurfaceDefinition> = std::mem::take(surfaces);
            surfaces.push(drained.remove(focused));
            for surface in drained {
                let title = surface_title(&surface);
                promoted.push(TabSession {
                    title,
                    title_source: None,
                    layout: Some(LayoutNode::Pane {
                        surfaces: vec![surface],
                    }),
                    worktree: None,
                });
            }
        }
        LayoutNode::Split { children, .. } => {
            for child in children.iter_mut() {
                demote_panes_to_focused_surface(child, promoted);
            }
        }
    }
}

fn surface_title(surface: &SurfaceDefinition) -> String {
    surface
        .custom_name
        .as_deref()
        .or(surface.name.as_deref())
        .unwrap_or_default()
        .to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ManagedWorktreeDef {
    pub path: String,
    pub repo_root: String,
    pub branch: String,
    pub teardown: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ButtonCommand {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub command: String,
}
