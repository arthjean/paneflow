use gpui::{
    Animation, AnimationExt, AnyElement, AppContext, FontWeight, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder,
    px, rgb, svg,
};

use crate::app::pull_request::{PrState, PullRequest};
use crate::ui_primitives::TooltipDelayExt;

use super::{SIDEBAR_ACTION_BUTTON_SIZE, SidebarAgentState, SidebarAgentSummary, SidebarTooltip};

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
                let color = crate::app::constants::sidebar_needs_input_color();
                (color, icon("icons/bell.svg", color))
            }
            SidebarAgentState::Errored => {
                (ui.agent_error, icon("icons/x_circle.svg", ui.agent_error))
            }
            SidebarAgentState::Thinking => {
                let color = summary.tint.map_or(ui.muted, |tint| rgb(tint).into());
                (color, render_comet_trail_loader(row_key, color))
            }
            SidebarAgentState::Finished => {
                let color = crate::app::constants::sidebar_finished_color();
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
        .text_size(crate::ui_primitives::LABEL_XS)
        .line_height(px(20.))
        .font_features(super::tabular_numerals())
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .text_color(color)
        .aria_label(tooltip.clone())
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

const COMET_TRAIL_DOT_SIZE: f32 = 3.0;
const COMET_TRAIL_DOT_GAP: f32 = 1.0;

pub(in crate::app) fn render_comet_trail_loader(row_key: &str, color: gpui::Hsla) -> AnyElement {
    static SYNC_EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

    const CYCLE_MS: u64 = 720;
    const PERIMETER: usize = 8;

    let loader = div()
        .size(px(11.))
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(COMET_TRAIL_DOT_GAP));
    if crate::ui_primitives::reduce_motion() {
        return comet_trail_matrix(loader, 0, color).into_any_element();
    }
    loader
        .with_animation(
            SharedString::from(format!("comet-trail-{row_key}")),
            Animation::new(std::time::Duration::from_millis(CYCLE_MS)).repeat(),
            move |loader, _delta| {
                let cycle_elapsed = SYNC_EPOCH
                    .get_or_init(std::time::Instant::now)
                    .elapsed()
                    .as_millis()
                    % u128::from(CYCLE_MS);
                let head = (cycle_elapsed * PERIMETER as u128 / u128::from(CYCLE_MS)) as usize;
                comet_trail_matrix(loader, head, color)
            },
        )
        .into_any_element()
}

fn comet_trail_matrix(loader: gpui::Div, head: usize, color: gpui::Hsla) -> gpui::Div {
    const MATRIX_SIZE: usize = 3;
    const PERIMETER: usize = 8;
    const BASE_OPACITY: f32 = 0.06;
    const TAIL_OPACITIES: [f32; 3] = [0.8144, 0.4864, 0.2568];

    loader.children((0..MATRIX_SIZE).map(|row| {
        div()
            .h(px(COMET_TRAIL_DOT_SIZE))
            .flex_none()
            .flex()
            .flex_row()
            .gap(px(COMET_TRAIL_DOT_GAP))
            .children((0..MATRIX_SIZE).map(move |col| {
                let order = match (row, col) {
                    (0, 0) => Some(0),
                    (0, 1) => Some(1),
                    (0, 2) => Some(2),
                    (1, 2) => Some(3),
                    (2, 2) => Some(4),
                    (2, 1) => Some(5),
                    (2, 0) => Some(6),
                    (1, 0) => Some(7),
                    _ => None,
                };
                let opacity = order.map_or_else(
                    || if head.is_multiple_of(2) { 0.1 } else { 0.18 },
                    |order| {
                        let trail = (head + PERIMETER - order) % PERIMETER;
                        TAIL_OPACITIES.get(trail).copied().unwrap_or(BASE_OPACITY)
                    },
                );

                div()
                    .size(px(COMET_TRAIL_DOT_SIZE))
                    .flex_none()
                    .rounded_full()
                    .bg(color.opacity(opacity))
            }))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::app::pull_request::{PrState, PullRequest};

    fn pr(state: PrState) -> PullRequest {
        PullRequest { number: 46, state }
    }

    #[test]
    fn an_agent_state_outranks_the_pull_request() {
        let working = SidebarAgentSummary {
            state: SidebarAgentState::Thinking,
            count: 1,
            tint: None,
        };
        assert_eq!(
            infer_lane(Some(working), Some(pr(PrState::Open))),
            Some(Lane::Agent(working))
        );
        assert_eq!(
            infer_lane(None, Some(pr(PrState::Open))),
            Some(Lane::PullRequest(pr(PrState::Open)))
        );
    }

    #[test]
    fn a_closed_pull_request_draws_no_lane() {
        assert_eq!(infer_lane(None, Some(pr(PrState::Closed))), None);
        assert_eq!(infer_lane(None, None), None);
    }

    #[test]
    fn every_lane_answers_with_a_word_and_only_agents_count() {
        let summary = |state, count| {
            Lane::Agent(SidebarAgentSummary {
                state,
                count,
                tint: None,
            })
        };
        assert_eq!(summary(SidebarAgentState::NeedsInput, 1).label(), "Input");
        assert_eq!(summary(SidebarAgentState::NeedsInput, 2).label(), "Input 2");
        assert_eq!(summary(SidebarAgentState::Errored, 1).label(), "Error");
        assert_eq!(summary(SidebarAgentState::Thinking, 1).label(), "");
        assert_eq!(summary(SidebarAgentState::Thinking, 2).label(), "2");
        assert_eq!(summary(SidebarAgentState::Finished, 3).label(), "Done 3");
        assert_eq!(Lane::PullRequest(pr(PrState::Open)).label(), "Review");
        assert_eq!(Lane::PullRequest(pr(PrState::Draft)).label(), "Draft");
        assert_eq!(Lane::PullRequest(pr(PrState::Merged)).label(), "Merged");
    }
}
