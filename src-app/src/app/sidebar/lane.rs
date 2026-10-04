use std::cell::Cell;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Task, div, prelude::FluentBuilder, px, rgb, svg,
};

use crate::app::pull_request::{PrState, PullRequest};
use crate::ui_primitives::TooltipDelayExt;

use super::{
    SIDEBAR_ACTION_BUTTON_SIZE, SIDEBAR_LANE_GLYPH_SIZE, SidebarAgentState, SidebarAgentSummary,
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

fn lane_visual(lane: Lane, ui: crate::theme::UiColors) -> (gpui::Hsla, AnyElement) {
    let icon = |path: &'static str, color: gpui::Hsla| {
        svg()
            .size(px(SIDEBAR_LANE_GLYPH_SIZE))
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
                (color, render_comet_trail_loader(color))
            }
            SidebarAgentState::Finished => {
                let color = crate::app::constants::sidebar_finished_color();
                (
                    color,
                    div()
                        .size(px(SIDEBAR_LANE_GLYPH_SIZE))
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
    reserve_action_slot: bool,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let (color, glyph) = lane_visual(lane, ui);
    div()
        .id(SharedString::from(format!("lane-{row_key}")))
        .flex_none()
        .h(px(20.))
        .when(reserve_action_slot, |slot| {
            slot.min_w(px(SIDEBAR_ACTION_BUTTON_SIZE))
        })
        .flex()
        .flex_row()
        .items_start()
        .justify_end()
        .gap(px(3.))
        .text_size(crate::ui_primitives::LABEL_XS)
        .line_height(px(20.))
        .font_features(super::tabular_numerals())
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .text_color(color)
        .aria_label(tooltip.clone())
        .delayed_tooltip(crate::ui_primitives::text_tooltip(tooltip))
        .child(
            div()
                .flex_none()
                .mt(px(super::sidebar_lane_glyph_top()))
                .child(glyph),
        )
        .when(!lane.label().is_empty(), |slot| {
            let ink_height_em = if lane.word().is_empty() {
                super::GEIST_CAP_HEIGHT_EM
            } else {
                super::GEIST_X_HEIGHT_EM
            };
            let label_axis = super::sidebar_text_ink_axis(
                crate::ui_primitives::LABEL_XS.as_f32(),
                ink_height_em,
            );
            let drop = (super::sidebar_lane_glyph_center() - label_axis).round();
            slot.child(div().relative().top(px(drop)).child(lane.label()))
        })
        .into_any_element()
}

pub(super) enum LaneSlot {
    UnderHoverAction(SharedString),
    BesideHoverAction,
}

pub(super) fn render_lane_slot(
    lane: Option<Lane>,
    row_key: &str,
    tooltip: impl FnOnce(SidebarAgentSummary) -> SharedString,
    slot: LaneSlot,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let hidden_by_hover_action = match slot {
        LaneSlot::UnderHoverAction(group) => Some(group),
        LaneSlot::BesideHoverAction => None,
    };
    match lane {
        Some(lane) => {
            let tooltip = match lane {
                Lane::Agent(summary) => tooltip(summary),
                Lane::PullRequest(pr) => pull_request_tooltip(pr),
            };
            let reserve_action_slot = hidden_by_hover_action.is_some();
            div()
                .flex_none()
                .when_some(hidden_by_hover_action, |slot, group| {
                    slot.group_hover(group, |style| style.invisible())
                })
                .child(render_lane(lane, row_key, tooltip, reserve_action_slot, ui))
                .into_any_element()
        }
        None if hidden_by_hover_action.is_none() => div().flex_none().into_any_element(),
        None => div()
            .flex_none()
            .w(px(SIDEBAR_ACTION_BUTTON_SIZE))
            .into_any_element(),
    }
}

const COMET_TRAIL_DOT_SIZE: f32 = 3.0;
const COMET_TRAIL_DOT_GAP: f32 = 1.0;

const SPINNER_CYCLE_MS: u64 = 720;
const SPINNER_STEPS: u64 = 8;
const SPINNER_STEP_MS: u64 = SPINNER_CYCLE_MS / SPINNER_STEPS;

thread_local! {
    static SPINNER_DRAWN: Cell<bool> = const { Cell::new(false) };
}

fn spinner_epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn spinner_head(elapsed: Duration) -> usize {
    let cycle_elapsed = elapsed.as_millis() % u128::from(SPINNER_CYCLE_MS);
    (cycle_elapsed * u128::from(SPINNER_STEPS) / u128::from(SPINNER_CYCLE_MS)) as usize
}

fn spinner_next_step(elapsed: Duration) -> Duration {
    let into_step = (elapsed.as_millis() % u128::from(SPINNER_STEP_MS)) as u64;
    Duration::from_millis(SPINNER_STEP_MS - into_step + 1)
}

fn take_spinner_drawn() -> bool {
    SPINNER_DRAWN.with(|drawn| drawn.replace(false))
}

#[derive(Default)]
pub(crate) struct SpinnerClock {
    ticker: Option<Task<()>>,
}

impl SpinnerClock {
    pub(crate) fn sync<V: 'static>(&mut self, cx: &mut Context<V>) {
        if !take_spinner_drawn() {
            self.ticker = None;
            return;
        }
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |view, cx| {
            loop {
                let executor = cx.background_executor().clone();
                let elapsed = executor.now().saturating_duration_since(spinner_epoch());
                executor.timer(spinner_next_step(elapsed)).await;
                if view.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
    }

    #[cfg(test)]
    fn running(&self) -> bool {
        self.ticker.is_some()
    }
}

pub(in crate::app) fn render_comet_trail_loader(color: gpui::Hsla) -> AnyElement {
    comet_trail_loader(color, !crate::ui_primitives::reduce_motion())
}

fn comet_trail_loader(color: gpui::Hsla, animate: bool) -> AnyElement {
    let loader = div()
        .size(px(SIDEBAR_LANE_GLYPH_SIZE))
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(COMET_TRAIL_DOT_GAP));
    let head = if animate {
        SPINNER_DRAWN.with(|drawn| drawn.set(true));
        spinner_head(spinner_epoch().elapsed())
    } else {
        0
    };
    comet_trail_matrix(loader, head, color).into_any_element()
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

    #[test]
    fn the_stepped_spinner_keeps_eight_positions_over_a_720_ms_cycle() {
        let heads: Vec<usize> = (0..SPINNER_CYCLE_MS)
            .map(|ms| spinner_head(Duration::from_millis(ms)))
            .collect();
        for (step, window) in heads.chunks(SPINNER_STEP_MS as usize).enumerate() {
            assert!(
                window.iter().all(|head| *head == step),
                "step {step}: {window:?}"
            );
        }
        assert_eq!(spinner_head(Duration::from_millis(SPINNER_CYCLE_MS)), 0);
        assert_eq!(
            spinner_head(Duration::from_millis(SPINNER_CYCLE_MS + 91)),
            1
        );
        assert_eq!(spinner_next_step(Duration::ZERO), Duration::from_millis(91));
        assert_eq!(
            spinner_next_step(Duration::from_millis(89)),
            Duration::from_millis(2)
        );
        assert_eq!(
            spinner_next_step(Duration::from_millis(181)),
            Duration::from_millis(90)
        );
    }

    struct SpinnerHarness {
        spinners: usize,
        animate: bool,
        clock: SpinnerClock,
    }

    impl gpui::Render for SpinnerHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut Context<Self>,
        ) -> impl IntoElement {
            let color = gpui::white();
            let root =
                div().children((0..self.spinners).map(|_| comet_trail_loader(color, self.animate)));
            self.clock.sync(cx);
            root
        }
    }

    fn spinner_view(
        cx: &mut gpui::TestAppContext,
        spinners: usize,
        animate: bool,
    ) -> (
        gpui::Entity<SpinnerHarness>,
        &mut gpui::VisualTestContext,
        std::rc::Rc<std::cell::Cell<usize>>,
    ) {
        let (view, cx) = cx.add_window_view(move |_, _| SpinnerHarness {
            spinners,
            animate,
            clock: SpinnerClock::default(),
        });
        let notifies = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = notifies.clone();
        cx.update(|window, cx| {
            cx.observe(&view, move |_, _| counted.set(counted.get() + 1))
                .detach();
            window.draw(cx).clear(cx);
        });
        (view, cx, notifies)
    }

    fn run_for(cx: &mut gpui::VisualTestContext, span: Duration) {
        let step = Duration::from_millis(10);
        let mut elapsed = Duration::ZERO;
        while elapsed < span {
            cx.executor().advance_clock(step);
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            elapsed += step;
        }
    }

    #[gpui::test]
    fn one_thinking_agent_wakes_the_view_at_most_twelve_times_per_second(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, notifies) = spinner_view(cx, 1, true);
        assert!(view.read_with(cx, |view, _| view.clock.running()));
        run_for(cx, Duration::from_secs(1));
        let woken = notifies.get();
        assert!((10..=12).contains(&woken), "{woken} wakes in one second");
    }

    #[gpui::test]
    fn eight_thinking_agents_share_one_clock(cx: &mut gpui::TestAppContext) {
        let (_view, cx, notifies) = spinner_view(cx, 8, true);
        run_for(cx, Duration::from_secs(1));
        let woken = notifies.get();
        assert!((10..=12).contains(&woken), "{woken} wakes in one second");
    }

    #[gpui::test]
    fn reduce_motion_draws_a_static_spinner_without_a_clock(cx: &mut gpui::TestAppContext) {
        let (view, cx, notifies) = spinner_view(cx, 3, false);
        assert!(!view.read_with(cx, |view, _| view.clock.running()));
        run_for(cx, Duration::from_secs(1));
        assert_eq!(notifies.get(), 0);
    }

    #[gpui::test]
    fn the_clock_stops_when_the_last_spinner_leaves_the_tree(cx: &mut gpui::TestAppContext) {
        let (view, cx, notifies) = spinner_view(cx, 2, true);
        run_for(cx, Duration::from_millis(300));
        assert!(notifies.get() > 0);
        view.update(cx, |view, cx| {
            view.spinners = 0;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(!view.read_with(cx, |view, _| view.clock.running()));
        let after_close = notifies.get();
        run_for(cx, Duration::from_secs(1));
        assert_eq!(notifies.get(), after_close, "no periodic wake survives");
    }
}
