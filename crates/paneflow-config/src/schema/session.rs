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
    pub review_layout: Option<LayoutNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_collapsed: Vec<String>,
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

pub const BROWSER_DESCRIPTOR_VERSION: u32 = 1;

pub const DEFAULT_BROWSER_ZOOM_PERCENT: u32 = 100;

fn default_browser_zoom() -> u32 {
    DEFAULT_BROWSER_ZOOM_PERCENT
}

fn default_browser_descriptor_version() -> u32 {
    BROWSER_DESCRIPTOR_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserDescriptor {
    #[serde(default = "default_browser_descriptor_version")]
    pub version: u32,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default = "default_browser_zoom")]
    pub zoom: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub active: bool,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub browsers: Vec<BrowserDescriptor>,
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
            browsers: Vec::new(),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_profile: Option<String>,
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
                    browsers: Vec::new(),
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

#[cfg(test)]
mod browser_descriptor_tests {
    use super::*;

    #[test]
    fn a_pre_feature_session_reads_back_without_browser_fields() {
        let json = r#"{"version":2,"active_workspace":0,"workspaces":[{"title":"ws","cwd":"/tmp","tabs":[{"title":"t"}]}]}"#;
        let state: SessionState = serde_json::from_str(json).unwrap();
        assert!(state.workspaces[0].browser_profile.is_none());
        assert!(state.workspaces[0].tabs[0].browsers.is_empty());
        let written = serde_json::to_string(&state).unwrap();
        assert!(!written.contains("browsers"), "{written}");
        assert!(!written.contains("browser_profile"), "{written}");
    }

    #[test]
    fn descriptors_round_trip_with_only_durable_fields() {
        let descriptor = BrowserDescriptor {
            version: BROWSER_DESCRIPTOR_VERSION,
            id: "b-1".into(),
            url: Some("http://localhost:5173/".into()),
            title: "Vite".into(),
            zoom: 125,
            muted: true,
            active: true,
        };
        let tab = TabSession {
            browsers: vec![descriptor.clone()],
            ..TabSession::default()
        };
        let json = serde_json::to_value(&tab).unwrap();
        let mut keys: Vec<_> = json["browsers"][0]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            ["active", "id", "muted", "title", "url", "version", "zoom"]
        );
        let back: TabSession = serde_json::from_value(json).unwrap();
        assert_eq!(back.browsers, vec![descriptor]);
    }

    #[test]
    fn a_minimal_descriptor_defaults_to_an_empty_dormant_page() {
        let back: BrowserDescriptor = serde_json::from_str(r#"{"id":"b-2"}"#).unwrap();
        assert_eq!(back.version, BROWSER_DESCRIPTOR_VERSION);
        assert_eq!(back.url, None);
        assert_eq!(back.zoom, DEFAULT_BROWSER_ZOOM_PERCENT);
        assert!(!back.muted && !back.active);
    }
}
