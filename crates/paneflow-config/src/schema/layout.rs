use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct CommandDefinition {
    pub name: String,
    pub description: Option<String>,
    pub keywords: Vec<String>,
    pub target: CommandTarget,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(untagged)]
pub enum CommandTarget {
    Workspace { workspace: WorkspaceDefinition },
    Shell { command: String },
}

impl CommandDefinition {
    pub fn workspace(&self) -> Option<&WorkspaceDefinition> {
        match &self.target {
            CommandTarget::Workspace { workspace } => Some(workspace),
            CommandTarget::Shell { .. } => None,
        }
    }

    pub fn workspace_mut(&mut self) -> Option<&mut WorkspaceDefinition> {
        match &mut self.target {
            CommandTarget::Workspace { workspace } => Some(workspace),
            CommandTarget::Shell { .. } => None,
        }
    }

    pub fn shell_command(&self) -> Option<&str> {
        match &self.target {
            CommandTarget::Shell { command } => Some(command),
            CommandTarget::Workspace { .. } => None,
        }
    }
}

impl Serialize for CommandDefinition {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("CommandDefinition", 5)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("description", &self.description)?;
        state.serialize_field("keywords", &self.keywords)?;
        match &self.target {
            CommandTarget::Workspace { workspace } => {
                state.serialize_field("workspace", &Some(workspace))?;
                state.serialize_field("command", &Option::<&str>::None)?;
            }
            CommandTarget::Shell { command } => {
                state.serialize_field("workspace", &Option::<&WorkspaceDefinition>::None)?;
                state.serialize_field("command", &Some(command.as_str()))?;
            }
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for CommandDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawCommandDefinition {
            name: String,
            description: Option<String>,
            #[serde(default)]
            keywords: Vec<String>,
            workspace: Option<WorkspaceDefinition>,
            command: Option<String>,
        }

        let raw = RawCommandDefinition::deserialize(deserializer)?;
        if raw.name.trim().is_empty() {
            return Err(serde::de::Error::custom("command name must not be blank"));
        }

        let target = match (raw.workspace, raw.command) {
            (Some(workspace), None) => CommandTarget::Workspace { workspace },
            (None, Some(command)) if !command.trim().is_empty() => CommandTarget::Shell { command },
            (Some(_), Some(_)) => {
                return Err(serde::de::Error::custom(
                    "command must contain exactly one of `workspace` or `command`",
                ));
            }
            (None, Some(_)) => {
                return Err(serde::de::Error::custom("shell command must not be blank"));
            }
            (None, None) => {
                return Err(serde::de::Error::custom(
                    "command must contain either `workspace` or `command`",
                ));
            }
        };

        Ok(Self {
            name: raw.name,
            description: raw.description,
            keywords: raw.keywords,
            target,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceDefinition {
    pub name: Option<String>,
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_preset: Option<String>,
    pub color: Option<String>,
    pub layout: Option<LayoutNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutNode {
    Pane {
        #[serde(default)]
        surfaces: Vec<SurfaceDefinition>,
    },
    Split {
        direction: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ratio: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ratios: Option<Vec<f64>>,
        #[serde(default)]
        children: Vec<LayoutNode>,
    },
}

impl LayoutNode {
    pub fn leaf_count(&self) -> usize {
        match self {
            LayoutNode::Pane { .. } => 1,
            LayoutNode::Split { children, .. } => children.iter().map(|c| c.leaf_count()).sum(),
        }
    }

    pub fn resolved_ratios(&self) -> Vec<f64> {
        match self {
            LayoutNode::Pane { .. } => vec![1.0],
            LayoutNode::Split {
                ratio,
                ratios,
                children,
                ..
            } => {
                let n = children.len().max(1);
                let raw = if let Some(rs) = ratios {
                    rs.clone()
                } else if let Some(r) = ratio {
                    if children.len() == 2 {
                        legacy_ratios(*r)
                    } else {
                        return vec![1.0 / n as f64; n];
                    }
                } else {
                    return vec![1.0 / n as f64; n];
                };
                sanitize_ratios(raw, n)
            }
        }
    }
}

pub const MAX_LAYOUT_LEAVES: usize = 32;

pub(crate) const MAX_SPLIT_CHILDREN: usize = 32;

pub(crate) const MAX_PANE_SURFACES: usize = 64;

const MIN_RATIO: f64 = 0.01;

const MIN_LEGACY_RATIO: f64 = 0.1;

fn legacy_ratios(ratio: f64) -> Vec<f64> {
    let ratio = if ratio.is_finite() {
        ratio.clamp(MIN_LEGACY_RATIO, 1.0 - MIN_LEGACY_RATIO)
    } else {
        0.5
    };
    vec![ratio, 1.0 - ratio]
}

fn sanitize_ratios(mut ratios: Vec<f64>, n: usize) -> Vec<f64> {
    if ratios.len() != n {
        return vec![1.0 / n as f64; n];
    }
    for r in ratios.iter_mut() {
        *r = if r.is_finite() {
            r.clamp(MIN_RATIO, 1.0)
        } else {
            MIN_RATIO
        };
    }
    let sum: f64 = ratios.iter().sum();
    if sum > 0.0 && (sum - 1.0).abs() > 1e-9 {
        for r in ratios.iter_mut() {
            *r /= sum;
        }
    }
    for r in ratios.iter_mut() {
        *r = r.clamp(MIN_RATIO, 1.0);
    }
    ratios
}

pub fn validate_layout(node: &mut LayoutNode) {
    let placeholder = default_layout_pane();
    let original = std::mem::replace(node, placeholder);
    let mut leaf_budget = MAX_LAYOUT_LEAVES;
    *node = sanitize_layout_node(original, &mut leaf_budget).unwrap_or_else(default_layout_pane);
}

fn sanitize_layout_node(node: LayoutNode, leaf_budget: &mut usize) -> Option<LayoutNode> {
    match node {
        LayoutNode::Pane { mut surfaces } => {
            if *leaf_budget == 0 {
                return None;
            }
            *leaf_budget -= 1;

            if surfaces.len() > MAX_PANE_SURFACES {
                tracing::warn!(
                    "pane has {} surfaces (cap {MAX_PANE_SURFACES}); truncating",
                    surfaces.len()
                );
                surfaces.truncate(MAX_PANE_SURFACES);
            }
            if surfaces.is_empty() {
                tracing::warn!("pane has no surfaces; adding a default surface");
                surfaces.push(SurfaceDefinition::default());
            }

            let mut focus_seen = false;
            for surface in &mut surfaces {
                if surface.focus == Some(true) {
                    if focus_seen {
                        tracing::warn!(
                            "pane has multiple focused surfaces; dropping extra focus flag"
                        );
                        surface.focus = None;
                    } else {
                        focus_seen = true;
                    }
                }
            }
            Some(LayoutNode::Pane { surfaces })
        }
        LayoutNode::Split {
            mut direction,
            ratio,
            ratios,
            children,
        } => {
            if direction != "horizontal" && direction != "vertical" {
                tracing::warn!("split direction `{direction}` is invalid; resetting to horizontal");
                direction = "horizontal".to_string();
            }

            if children.len() > MAX_SPLIT_CHILDREN {
                tracing::warn!(
                    "split has {} children (cap {MAX_SPLIT_CHILDREN}); truncating",
                    children.len()
                );
            }

            let mut kept = Vec::with_capacity(children.len().min(MAX_SPLIT_CHILDREN));
            for child in children.into_iter().take(MAX_SPLIT_CHILDREN) {
                if *leaf_budget == 0 {
                    break;
                }
                if let Some(child) = sanitize_layout_node(child, leaf_budget) {
                    kept.push(child);
                }
            }

            match kept.len() {
                0 => None,
                1 => {
                    tracing::warn!("split has one surviving child; collapsing split");
                    kept.pop()
                }
                n => {
                    let legacy = ratio.map(|value| legacy_ratios(value)[0]);
                    let normalized = match ratios {
                        Some(values) => Some(sanitize_ratios(values, n)),
                        None if n == 2 => legacy.map(legacy_ratios),
                        None => {
                            if legacy.is_some() {
                                tracing::warn!(
                                    "legacy ratio ignored on N-ary split ({n} children)"
                                );
                            }
                            None
                        }
                    };
                    Some(LayoutNode::Split {
                        direction,
                        ratio: if n == 2 { legacy } else { None },
                        ratios: normalized,
                        children: kept,
                    })
                }
            }
        }
    }
}

pub(crate) fn default_layout_pane() -> LayoutNode {
    LayoutNode::Pane {
        surfaces: vec![SurfaceDefinition::default()],
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SurfaceDefinition {
    pub surface_type: Option<String>,
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub focus: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrollback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
}

impl Default for SurfaceDefinition {
    fn default() -> Self {
        Self {
            surface_type: Some("terminal".to_string()),
            name: None,
            custom_name: None,
            command: None,
            prompt: None,
            cwd: None,
            path: None,
            env: None,
            focus: None,
            scrollback: None,
            agent: None,
            font_size: None,
        }
    }
}
