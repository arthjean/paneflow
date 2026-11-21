use gpui::{
    AnyElement, Div, Hsla, IntoElement, ParentElement, Styled, div, prelude::*, px, rgb, svg,
};
use paneflow_config::schema::{LayoutNode, SurfaceDefinition, WorkspaceDefinition};

use crate::agent_launcher::TerminalAgent;
use crate::app::workspace_ops::templates::{
    PaneKind, build_layout_from_surfaces, pane_kind, template_surfaces, workspace_layout_preset,
};
use crate::settings::components::{render_logo, with_alpha};

pub(super) const AGENT_GLYPH: &str = "icons/settings/ai-magic.svg";
pub(super) const COMMAND_GLYPH: &str = "icons/settings/command-line.svg";
pub(super) const SHELL_GLYPH: &str = "icons/settings/terminal.svg";

const MARK_WIDTH: f32 = crate::settings::kit::LEAD;
const MARK_HEIGHT: f32 = 24.;
const MARK_INSET: f32 = 3.;
const MARK_CELL_RADIUS: f32 = 3.;
const MARK_GAP: f32 = 2.;
const GLYPH_GAP: f32 = 1.5;
const PREVIEW_WIDTH: f32 = 184.;
const PREVIEW_HEIGHT: f32 = 112.;
const PREVIEW_INSET: f32 = 5.;
const PREVIEW_TILE_RADIUS: f32 = 8.;
const PREVIEW_GAP: f32 = 4.;
const PREVIEW_AGENT: f32 = 24.;
const PREVIEW_GLYPH: f32 = 16.;

pub(super) fn splits_side_by_side(direction: &str) -> bool {
    direction != "horizontal"
}

pub(super) fn template_layout(workspace: &WorkspaceDefinition) -> Option<LayoutNode> {
    workspace.layout.clone().or_else(|| {
        build_layout_from_surfaces(
            workspace_layout_preset(workspace),
            template_surfaces(workspace),
        )
    })
}

fn placeholder_layout(preset: &str, panes: usize) -> Option<LayoutNode> {
    build_layout_from_surfaces(preset, vec![SurfaceDefinition::default(); panes.max(1)])
}

fn miniature(node: &LayoutNode, gap: f32, radius: f32, fill: Hsla) -> Div {
    match node {
        LayoutNode::Pane { .. } => div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .rounded(px(radius))
            .bg(fill),
        LayoutNode::Split {
            direction,
            children,
            ..
        } => {
            let mut split = div().flex_1().min_w_0().min_h_0().flex().gap(px(gap));
            split = if splits_side_by_side(direction) {
                split.flex_row()
            } else {
                split.flex_col()
            };
            for child in children {
                split = split.child(miniature(child, gap, radius, fill));
            }
            split
        }
    }
}

pub(super) fn preset_glyph(
    preset: &str,
    panes: usize,
    width: f32,
    height: f32,
    ink: Hsla,
) -> AnyElement {
    div()
        .flex_none()
        .w(px(width))
        .h(px(height))
        .flex()
        .when_some(placeholder_layout(preset, panes), |frame, node| {
            frame.child(miniature(&node, GLYPH_GAP, GLYPH_GAP, ink))
        })
        .into_any_element()
}

pub(super) fn layout_mark(
    workspace: &WorkspaceDefinition,
    ink: crate::settings::kit::Ink,
) -> AnyElement {
    let panes = template_surfaces(workspace).len();
    let node = if panes == 0 {
        None
    } else {
        template_layout(workspace)
    };
    div()
        .flex_none()
        .w(px(MARK_WIDTH))
        .h(px(MARK_HEIGHT))
        .p(px(MARK_INSET))
        .flex()
        .rounded(px(MARK_CELL_RADIUS + MARK_INSET))
        .bg(ink.well)
        .when_some(node, |mark, node| {
            mark.child(miniature(
                &node,
                MARK_GAP,
                MARK_CELL_RADIUS,
                with_alpha(ink.primary, 0.24),
            ))
        })
        .into_any_element()
}

pub(super) fn agent_logo(
    agent: TerminalAgent,
    size: f32,
    ui: crate::theme::UiColors,
) -> AnyElement {
    render_logo(
        agent.icon_path(),
        agent.icon_multicolor(),
        px(size),
        agent.accent().map(|c| rgb(c).into()).unwrap_or(ui.text),
    )
}

pub(super) fn glyph(path: &'static str, size: f32, color: Hsla) -> AnyElement {
    svg()
        .size(px(size))
        .flex_none()
        .path(path)
        .text_color(color)
        .into_any_element()
}

pub(super) fn pane_agent(surface: &SurfaceDefinition) -> Option<TerminalAgent> {
    surface.agent.as_deref().and_then(TerminalAgent::from_tag)
}

pub(super) fn pane_icon(
    surface: &SurfaceDefinition,
    size: f32,
    ui: crate::theme::UiColors,
) -> AnyElement {
    match pane_kind(surface) {
        PaneKind::Agent => match pane_agent(surface) {
            Some(agent) => agent_logo(agent, size, ui),
            None => glyph(AGENT_GLYPH, size, ui.muted),
        },
        PaneKind::Command => glyph(COMMAND_GLYPH, size, ui.muted),
        PaneKind::Empty => glyph(SHELL_GLYPH, size, ui.muted),
    }
}

pub(super) fn template_agents(workspace: &WorkspaceDefinition) -> Vec<TerminalAgent> {
    let mut agents: Vec<TerminalAgent> = Vec::new();
    for pane in template_surfaces(workspace) {
        if let Some(agent) = pane_agent(&pane)
            && !agents.contains(&agent)
        {
            agents.push(agent);
        }
    }
    agents
}

pub(super) fn welcome_preview(
    agent: Option<TerminalAgent>,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let tile = |content: AnyElement| {
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(PREVIEW_TILE_RADIUS))
            .bg(with_alpha(ui.text, 0.07))
            .child(content)
    };
    let main = match agent {
        Some(agent) => agent_logo(agent, PREVIEW_AGENT, ui),
        None => glyph(AGENT_GLYPH, PREVIEW_AGENT, ui.muted),
    };
    div()
        .flex_none()
        .w(px(PREVIEW_WIDTH))
        .h(px(PREVIEW_HEIGHT))
        .p(px(PREVIEW_INSET))
        .flex()
        .flex_row()
        .gap(px(PREVIEW_GAP))
        .rounded(px(PREVIEW_TILE_RADIUS + PREVIEW_INSET))
        .bg(with_alpha(ui.text, 0.04))
        .child(tile(main))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(PREVIEW_GAP))
                .child(tile(glyph(COMMAND_GLYPH, PREVIEW_GLYPH, ui.muted)))
                .child(tile(glyph(SHELL_GLYPH, PREVIEW_GLYPH, ui.muted))),
        )
        .into_any_element()
}
