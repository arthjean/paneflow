use gpui::{
    AnyElement, AppContext, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, div, prelude::FluentBuilder, px, rgb, svg,
};

use crate::app::pull_request::{PrState, PullRequest};
use crate::ui_primitives::TooltipDelayExt;

use super::{
    SIDEBAR_ACTION_BUTTON_SIZE, SidebarAgentState, SidebarAgentSummary, SidebarTooltip,
    render_comet_trail_loader,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lane {
    Agent(SidebarAgentSummary),
    PullRequest(PullRequest),
}

pub(super) fn infer_lane(
    agent: Option<SidebarAgentSummary>,
    pull_request: Option<PullRequest>,
) -> Option<Lane> {
    if let Some(summary) = agent {
        return Some(Lane::Agent(summary));
    }
    pull_request
        .filter(|pr| pr.state != PrState::Closed)
        .map(Lane::PullRequest)
}

impl Lane {
    pub(super) fn word(self) -> &'static str {
        match self {
            Lane::Agent(summary) => match summary.state {
                SidebarAgentState::NeedsInput => "Input",
                SidebarAgentState::Errored => "Error",
                SidebarAgentState::Thinking => "",
                SidebarAgentState::Finished => "Done",
            },
            Lane::PullRequest(pr) => match pr.state {
                PrState::Open => "Review",
                PrState::Draft => "Draft",
                PrState::Merged => "Merged",
                PrState::Closed => "Closed",
            },
        }
    }

    pub(super) fn label(self) -> String {
        match self {
            Lane::Agent(summary) if summary.count > 1 => {
                format!("{} {}", self.word(), summary.count)
                    .trim_start()
                    .to_string()
            }
            _ => self.word().to_string(),
        }
    }
}

pub(super) fn pull_request_tooltip(pr: PullRequest) -> SharedString {
    let state = match pr.state {
        PrState::Open => "open",
        PrState::Draft => "draft",
        PrState::Merged => "merged",
        PrState::Closed => "closed",
    };
    format!("Pull request #{} {state}", pr.number).into()
}

fn lane_visual(lane: Lane, row_key: &str, ui: crate::theme::UiColors) -> (gpui::Hsla, AnyElement) {
    let icon = |path: &'static str, color: gpui::Hsla| {
        svg()
            .size(px(11.))
            .flex_none()
            .path(path)
            .text_color(color)
            .into_any_element()
    };
    match lane {
        Lane::Agent(summary) => match summary.state {
            SidebarAgentState::NeedsInput => {
                let color: gpui::Hsla = rgb(0xFBBF24).into();
                (color, icon("icons/bell.svg", color))
            }
            SidebarAgentState::Errored => {
                (ui.agent_error, icon("icons/x_circle.svg", ui.agent_error))
            }
            SidebarAgentState::Thinking => (ui.muted, render_comet_trail_loader(row_key, ui.muted)),
            SidebarAgentState::Finished => {
                let color: gpui::Hsla = rgb(0x83C3FF).into();
                (
                    color,
                    div()
                        .size(px(11.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().size(px(7.)).rounded_full().bg(color))
                        .into_any_element(),
                )
            }
        },
        Lane::PullRequest(pr) => {
            let color = pr.state.color(ui);
            let path = match pr.state {
                PrState::Merged => "icons/git-merge.svg",
                _ => "icons/git-pull-request.svg",
            };
            (color, icon(path, color))
        }
    }
}

pub(super) fn render_lane(
    lane: Lane,
    row_key: &str,
    tooltip: SharedString,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let (color, glyph) = lane_visual(lane, row_key, ui);
    div()
        .id(SharedString::from(format!("lane-{row_key}")))
        .flex_none()
        .h(px(20.))
        .min_w(px(SIDEBAR_ACTION_BUTTON_SIZE))
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap(px(3.))
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .text_color(color)
        .delayed_tooltip(move |_w, cx| {
            cx.new(|_| SidebarTooltip {
                label: tooltip.clone(),
            })
            .into()
        })
        .child(glyph)
        .when(!lane.label().is_empty(), |slot| slot.child(lane.label()))
        .into_any_element()
}

pub(super) fn render_lane_slot(
    lane: Option<Lane>,
    row_key: &str,
    tooltip: impl FnOnce(SidebarAgentSummary) -> SharedString,
    group: SharedString,
    ui: crate::theme::UiColors,
) -> AnyElement {
    match lane {
        Some(lane) => {
            let tooltip = match lane {
                Lane::Agent(summary) => tooltip(summary),
                Lane::PullRequest(pr) => pull_request_tooltip(pr),
            };
            div()
                .flex_none()
                .group_hover(group, |style| style.invisible())
                .child(render_lane(lane, row_key, tooltip, ui))
                .into_any_element()
        }
        None => div()
            .flex_none()
            .w(px(SIDEBAR_ACTION_BUTTON_SIZE))
            .into_any_element(),
    }
}
