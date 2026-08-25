use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single command definition, compatible with the cmux workspace format.
///
/// Each entry is either a workspace definition (with `workspace`) or a simple
/// shell command (with `command`).
#[derive(Debug, Clone, PartialEq)]
pub struct CommandDefinition {
    /// Display name (must not be blank).
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Search keywords for fuzzy matching.
    pub keywords: Vec<String>,
    /// Exactly one command payload. Its untagged representation preserves the
    /// existing top-level JSON key (`workspace` or `command`).
    pub target: CommandTarget,
}

/// The mutually exclusive payload of a [`CommandDefinition`].
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

/// Workspace definition containing layout, working directory, and visual config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceDefinition {
    /// Workspace display name.
    pub name: Option<String>,
    /// Default working directory for the workspace.
    pub cwd: Option<String>,
    /// Layout preset used by the visual workspace builder.
    ///
    /// Accepted values mirror `paneflow up`: `"even_h"`, `"even_v"`,
    /// `"main_vertical"`, and `"tiled"`. Older configs may omit this and rely
    /// on `layout` alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_preset: Option<String>,
    /// Color as a 6-digit hex string (e.g. "ff6600").
    pub color: Option<String>,
    /// Root layout node describing pane arrangement.
    pub layout: Option<LayoutNode>,
}

/// A node in the layout tree: either a leaf pane or a split container.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutNode {
    /// A leaf pane containing one or more surfaces.
    Pane {
        /// Surfaces within this pane (must have >= 1).
        #[serde(default)]
        surfaces: Vec<SurfaceDefinition>,
    },
    /// A split container dividing space between 2 or more children.
    Split {
        /// Split direction: "horizontal" or "vertical".
        direction: String,
        /// Legacy: single split ratio for binary (2-child) layouts.
        /// Ignored when `ratios` is present. Defaults to 0.5 if omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ratio: Option<f64>,
        /// Per-child ratios for N-ary layouts. When present, must have
        /// the same length as `children`. Values should sum to ~1.0.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ratios: Option<Vec<f64>>,
        /// 2 or more child layout nodes.
        #[serde(default)]
        children: Vec<LayoutNode>,
    },
}

impl LayoutNode {
    /// Count the number of leaf (Pane) nodes in the layout tree.
    pub fn leaf_count(&self) -> usize {
        match self {
            LayoutNode::Pane { .. } => 1,
            LayoutNode::Split { children, .. } => children.iter().map(|c| c.leaf_count()).sum(),
        }
    }

    /// Resolve per-child ratios for a Split node.
    ///
    /// Returns `ratios` if present, else converts legacy `ratio` to binary
    /// `[ratio, 1-ratio]`, else returns equal ratios for the child count.
    ///
    /// US-056: persisted ratios are untrusted input - a hand-edited or corrupt
    /// `session.json` can carry NaN, negative, zero, or wrong-length values. Any
    /// user-supplied set is run through `sanitize_ratios` (clamp into
    /// `[MIN_RATIO, 1.0]`, reject non-finite/negative, normalize to sum 1.0)
    /// before it reaches layout construction; the internally generated
    /// equal-share fallback is already valid and returned verbatim.
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

/// Hard ceiling for panes restored from one persisted layout. This is the
/// canonical source used by the config boundary and the live application.
pub const MAX_LAYOUT_LEAVES: usize = 32;

/// Maximum number of direct children retained on one split.
pub(crate) const MAX_SPLIT_CHILDREN: usize = 32;

/// Maximum number of terminal surfaces retained in one pane.
pub(crate) const MAX_PANE_SURFACES: usize = 64;

/// Floor for any single persisted split ratio. Clamping to this keeps every
/// pane visible and prevents a divide-by-zero when the set is normalized.
const MIN_RATIO: f64 = 0.01;

/// Legacy binary ratios historically used a stricter visibility floor.
const MIN_LEGACY_RATIO: f64 = 0.1;

fn legacy_ratios(ratio: f64) -> Vec<f64> {
    let ratio = if ratio.is_finite() {
        ratio.clamp(MIN_LEGACY_RATIO, 1.0 - MIN_LEGACY_RATIO)
    } else {
        0.5
    };
    vec![ratio, 1.0 - ratio]
}

/// Clamp every ratio into `[MIN_RATIO, 1.0]` (mapping NaN/inf/negative to the
/// floor), then normalize so the set sums to 1.0. A length mismatch with the
/// child count is unrecoverable - we cannot know which child a stale ratio was
/// meant for - so it degrades to equal shares.
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
    // US-056 (EP-010 review): re-clamp after normalize. Dividing by a sum > 1
    // can push a just-clamped ratio back below `MIN_RATIO` (e.g. raw
    // `[1.0, 0.005]` → clamp `[1.0, 0.01]` → normalize `[0.990, 0.0099]`),
    // silently violating the floor this fn promises. This helper is shared by
    // both validation and rendering, so both frontiers honour the same 0.01
    // floor. The renderer re-normalizes proportionally at paint time, so
    // the post-re-clamp sum need not be exactly 1.0 - the floor is the invariant.
    for r in ratios.iter_mut() {
        *r = r.clamp(MIN_RATIO, 1.0);
    }
    ratios
}

/// Validate and canonicalize an untrusted layout in place.
///
/// The resulting tree always contains between one and
/// [`MAX_LAYOUT_LEAVES`] panes. Invalid or over-budget split branches are
/// removed; a split with one surviving child collapses to that child. This
/// avoids the former padding path, which could reintroduce `O(depth)` panes
/// after the shared leaf budget had already been exhausted.
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
/// A surface within a pane (terminal, browser, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SurfaceDefinition {
    /// Surface type identifier: "terminal", "browser", etc.
    pub surface_type: Option<String>,
    /// Display name for this surface.
    pub name: Option<String>,
    /// User-assigned custom name (US-013). When set, it overrides the
    /// auto-derived surface name everywhere (sidebar/IPC `surface.list`/MCP),
    /// and survives restart via this field. Cleared by renaming to empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
    /// Shell command to run in this surface.
    pub command: Option<String>,
    /// Prompt text to prefill after launching an agent command.
    ///
    /// Kept optional so session persistence and plain command panes do not
    /// carry template-only state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Working directory override for this surface.
    pub cwd: Option<String>,
    /// File path for non-terminal surfaces such as markdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Extra environment variables merged over `terminal.env`. The same
    /// protected-key and loader-key filtering applies at PTY spawn.
    pub env: Option<HashMap<String, String>>,
    /// Whether this surface should receive initial focus.
    pub focus: Option<bool>,
    /// Saved scrollback text (plain, ANSI stripped). Up to 4000 lines / 400K chars.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrollback: Option<String>,
    /// EP-005 US-013: stable tag of the agent CLI last detected in this
    /// surface's PTY subtree (e.g. `"claude_code"`), so the identity pill
    /// survives restart as a dimmed "last known" until the first scan
    /// confirms it. Whitelisted at ingress against the known agent tags;
    /// unknown or malformed values are dropped silently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// EP-006 US-019: per-pane font-size override in points. `None` =
    /// follow the global config. Validated at restore ingress (NaN/inf
    /// dropped, finite values clamped to [8.0, 32.0]) - never fed raw to
    /// the cell geometry.
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
