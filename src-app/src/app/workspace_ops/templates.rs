use paneflow_config::schema::{LayoutNode, SurfaceDefinition, WorkspaceDefinition};

use crate::agent_launcher::TerminalAgent;

pub(crate) const LAYOUT_PRESETS: &[(&str, &str)] = &[
    ("even_h", "Side by side"),
    ("even_v", "Stacked"),
    ("main_vertical", "Main left"),
    ("tiled", "Tiled"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneKind {
    Empty,
    Agent,
    Command,
}

pub(crate) fn workspace_layout_preset(workspace: &WorkspaceDefinition) -> &str {
    workspace
        .layout_preset
        .as_deref()
        .filter(|preset| LAYOUT_PRESETS.iter().any(|(value, _)| value == preset))
        .unwrap_or("even_h")
}

pub(crate) fn layout_label(preset: &str) -> &'static str {
    LAYOUT_PRESETS
        .iter()
        .find_map(|(value, label)| (*value == preset).then_some(*label))
        .unwrap_or("Side by side")
}

pub(crate) fn template_surfaces(workspace: &WorkspaceDefinition) -> Vec<SurfaceDefinition> {
    workspace
        .layout
        .as_ref()
        .map(template_surfaces_from_layout)
        .unwrap_or_default()
}

pub(crate) fn template_surfaces_from_layout(layout: &LayoutNode) -> Vec<SurfaceDefinition> {
    let mut out = Vec::new();
    collect_surfaces(layout, &mut out);
    out
}

fn collect_surfaces(node: &LayoutNode, out: &mut Vec<SurfaceDefinition>) {
    match node {
        LayoutNode::Pane { surfaces } => {
            if surfaces.is_empty() {
                out.push(Default::default());
            } else {
                out.extend(surfaces.iter().cloned());
            }
        }
        LayoutNode::Split { children, .. } => {
            for child in children {
                collect_surfaces(child, out);
            }
        }
    }
}

pub(crate) fn build_layout_from_surfaces(
    preset: &str,
    surfaces: Vec<SurfaceDefinition>,
) -> Option<LayoutNode> {
    let leaves: Vec<LayoutNode> = surfaces
        .into_iter()
        .map(|surface| LayoutNode::Pane {
            surfaces: vec![surface],
        })
        .collect();
    if leaves.is_empty() {
        return None;
    }
    if leaves.len() == 1 {
        return leaves.into_iter().next();
    }
    match preset {
        "even_v" => Some(split("horizontal", leaves, None)),
        "main_vertical" => Some(main_vertical_layout(leaves)),
        "tiled" => Some(tiled_layout(leaves)),
        _ => Some(split("vertical", leaves, None)),
    }
}

fn split(direction: &str, children: Vec<LayoutNode>, ratios: Option<Vec<f64>>) -> LayoutNode {
    LayoutNode::Split {
        direction: direction.to_string(),
        ratio: None,
        ratios,
        children,
    }
}

fn main_vertical_layout(mut leaves: Vec<LayoutNode>) -> LayoutNode {
    let main = leaves.remove(0);
    let side = if leaves.len() == 1 {
        leaves.remove(0)
    } else {
        split("horizontal", leaves, None)
    };
    split("vertical", vec![main, side], Some(vec![0.5, 0.5]))
}

fn tiled_layout(leaves: Vec<LayoutNode>) -> LayoutNode {
    if leaves.len() <= 2 {
        return split("vertical", leaves, None);
    }
    let n = leaves.len();
    let mut rows = 1usize;
    let mut cols = 1usize;
    while rows * cols < n {
        if cols <= rows {
            cols += 1;
        } else {
            rows += 1;
        }
    }

    let row_ratio = 1.0 / rows as f64;
    let mut leaf_iter = leaves.into_iter();
    let mut row_nodes = Vec::with_capacity(rows);
    for row_idx in 0..rows {
        let panes_in_row = if row_idx < rows - 1 {
            cols
        } else {
            n - cols * (rows - 1)
        };
        let mut row_leaves: Vec<_> = leaf_iter.by_ref().take(panes_in_row).collect();
        let row_node = if row_leaves.len() == 1 {
            row_leaves.remove(0)
        } else {
            split("vertical", row_leaves, None)
        };
        row_nodes.push(row_node);
    }
    split("horizontal", row_nodes, Some(vec![row_ratio; rows]))
}

pub(crate) fn pane_kind(surface: &SurfaceDefinition) -> PaneKind {
    if surface
        .agent
        .as_deref()
        .is_some_and(|agent| !agent.trim().is_empty())
    {
        PaneKind::Agent
    } else if surface
        .command
        .as_deref()
        .is_some_and(|command| !command.trim().is_empty())
    {
        PaneKind::Command
    } else {
        PaneKind::Empty
    }
}

pub(crate) fn surface_title(surface: &SurfaceDefinition, idx: usize) -> String {
    surface
        .name
        .as_deref()
        .or(surface.custom_name.as_deref())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Pane {}", idx + 1))
}

pub(crate) fn surface_detail(surface: &SurfaceDefinition) -> String {
    if let Some(agent) = surface.agent.as_deref().and_then(TerminalAgent::from_tag) {
        agent.display_name().to_string()
    } else if let Some(command) = surface.command.as_deref().filter(|c| !c.trim().is_empty()) {
        command.to_string()
    } else {
        "Empty shell".to_string()
    }
}

pub(crate) fn template_summary(workspace: &WorkspaceDefinition) -> String {
    let panes = template_surfaces(workspace);
    if panes.is_empty() {
        return "Draft, no panes yet".to_string();
    }
    let agents = panes
        .iter()
        .filter(|pane| pane_kind(pane) == PaneKind::Agent)
        .count();
    let commands = panes
        .iter()
        .filter(|pane| pane_kind(pane) == PaneKind::Command)
        .count();
    let shells = panes.len().saturating_sub(agents + commands);
    format!("{agents} agents · {commands} commands · {shells} shells")
}

#[cfg(test)]
#[allow(
    clippy::items_after_test_module,
    reason = "layout fixtures remain beside the workspace presentation helpers"
)]
mod layout_tests {
    use paneflow_config::schema::LayoutNode;

    use super::tiled_layout;

    fn leaf() -> LayoutNode {
        LayoutNode::Pane {
            surfaces: Vec::new(),
        }
    }

    fn leaf_count(node: &LayoutNode) -> usize {
        match node {
            LayoutNode::Pane { .. } => 1,
            LayoutNode::Split { children, .. } => children.iter().map(leaf_count).sum(),
        }
    }

    #[test]
    fn tiled_layout_matches_runtime_grid_for_seven_panes() {
        let leaves = (0..7).map(|_| leaf()).collect();
        let layout = tiled_layout(leaves);

        let LayoutNode::Split {
            direction,
            ratios,
            children,
            ..
        } = layout
        else {
            panic!("tiled layout should split rows");
        };

        assert_eq!(direction, "horizontal");
        assert_eq!(ratios, Some(vec![1.0 / 3.0; 3]));
        assert_eq!(children.len(), 3);
        assert_eq!(
            children.iter().map(leaf_count).collect::<Vec<_>>(),
            vec![3, 3, 1]
        );
    }
}
