use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{App, MouseDownEvent, MouseMoveEvent, Point, ScrollHandle, Task, rgb};

use super::agents::{AgentRank, agent_candidates, order_agents};
use super::lane::{mark_shimmer_drawn, render_lane};
use super::*;
use crate::agent_launcher::TerminalAgent;
use crate::app::workspace_ops::WorkspaceFocusTarget;

pub(super) const AGENTS_HEADER_HEIGHT: f32 = 36.;
pub(super) const AGENTS_TREE_MIN_HEIGHT: f32 = 120.;
const AGENTS_LIST_BOTTOM_PADDING: f32 = 4.;
const AGENTS_HEIGHT_CAP: f32 = 330.;
const AGENTS_EMPTY_HEIGHT: f32 = 32.;
const AGENTS_EMPTY_INSET: f32 = 15.;
const AGENT_ROW_HEIGHT: f32 =
    SIDEBAR_ROW_PADDING_Y * 2. + SIDEBAR_ROW_LINE_HEIGHT * 2. + SIDEBAR_ROW_GAP;
const AGENT_RANK_SPACING: f32 = SIDEBAR_GROUP_SPACING - SIDEBAR_ROW_SPACING;
const AGENT_LOGO_SIZE: f32 = 15.;
const AGENT_META_ICON_SIZE: f32 = 12.;
const AGENTS_CHEVRON_SIZE: f32 = 12.;
const AGENTS_CHEVRON_TURN_SECS: f32 = 0.18;
const NO_STATUS_GRACE: Duration = Duration::from_secs(3);
const AGENTS_TOGGLE_GROUP: &str = "agents-section-toggle-group";

type PickAgent = Rc<dyn Fn(u64, &mut Window, &mut App)>;

#[derive(Default)]
pub(crate) struct AgentsSection {
    collapsed: bool,
    turn_from: f32,
    turn_started: Option<Instant>,
    grace_wake: Option<(Instant, Task<()>)>,
    scroll: ScrollHandle,
    scroll_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
    top_surface: Option<u64>,
}

impl AgentsSection {
    fn chevron_turn_at(&self, now: Instant) -> f32 {
        let to = if self.collapsed { 1. } else { 0. };
        let Some(started) = self.turn_started else {
            return to;
        };
        let progress =
            (now.duration_since(started).as_secs_f32() / AGENTS_CHEVRON_TURN_SECS).min(1.);
        let eased = 1. - (1. - progress).powi(3);
        self.turn_from + (to - self.turn_from) * eased
    }

    fn chevron_turning(&self, now: Instant) -> bool {
        self.turn_started.is_some_and(|started| {
            now.duration_since(started).as_secs_f32() < AGENTS_CHEVRON_TURN_SECS
        })
    }

    fn toggle(&mut self, now: Instant, animate: bool) {
        self.turn_from = self.chevron_turn_at(now);
        self.collapsed = !self.collapsed;
        self.turn_started = animate.then_some(now);
    }

    pub(crate) fn collapsed(&self) -> bool {
        self.collapsed
    }

    pub(crate) fn set_collapsed(&mut self, collapsed: bool, now: Instant, animate: bool) {
        if self.collapsed != collapsed {
            self.toggle(now, animate);
        }
    }

    fn follow_top_row(&mut self, top: Option<u64>) -> bool {
        if self.top_surface == top {
            return false;
        }
        self.top_surface = top;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentRowStatus {
    Lane(SidebarAgentState),
    NoStatus,
    Silent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AgentRow {
    pub(super) surface_id: u64,
    pub(super) rank: AgentRank,
    pub(super) tool: TerminalAgent,
    pub(super) title: String,
    pub(super) workspace: String,
    pub(super) active: bool,
    pub(super) starts_rank: bool,
    pub(super) status: AgentRowStatus,
}

pub(super) struct AgentRows {
    pub(super) rows: Vec<AgentRow>,
    pub(super) grace_deadline: Option<Instant>,
}

pub(super) fn agent_rows(
    workspaces: &[Workspace],
    active_space: Option<&paneflow_config::schema::SpaceId>,
    shown_workspace: Option<usize>,
    now: Instant,
    cx: &App,
) -> AgentRows {
    let mut previous_rank = None;
    let mut grace_deadline: Option<Instant> = None;
    let rows = order_agents(agent_candidates(workspaces, active_space, cx), true)
        .into_iter()
        .filter_map(|candidate| {
            let workspace = workspaces.get(candidate.workspace_idx)?;
            let session = workspace.agent_sessions.get(&candidate.session_key)?;
            let tab = workspace.tabs().get(candidate.tab_idx)?;
            let status = match candidate.rank {
                AgentRank::Blocked => AgentRowStatus::Lane(SidebarAgentState::NeedsInput),
                AgentRank::Errored => AgentRowStatus::Lane(SidebarAgentState::Errored),
                AgentRank::Done => AgentRowStatus::Lane(SidebarAgentState::Finished),
                AgentRank::Working => AgentRowStatus::Lane(SidebarAgentState::Thinking),
                AgentRank::Idle => AgentRowStatus::Silent,
                AgentRank::NoStatus => {
                    let due = session.started_at + NO_STATUS_GRACE;
                    if now >= due {
                        AgentRowStatus::NoStatus
                    } else {
                        grace_deadline = Some(grace_deadline.map_or(due, |held| held.min(due)));
                        AgentRowStatus::Silent
                    }
                }
            };
            let title = if tab.title().trim().is_empty() {
                session.tool.display_name().to_string()
            } else {
                tab.title().to_string()
            };
            let starts_rank = previous_rank.is_some_and(|previous| previous != candidate.rank);
            previous_rank = Some(candidate.rank);
            Some(AgentRow {
                surface_id: candidate.surface_id,
                rank: candidate.rank,
                tool: session.tool,
                title,
                workspace: workspace.title.clone(),
                active: shown_workspace == Some(candidate.workspace_idx)
                    && workspace.active_tab_idx() == candidate.tab_idx,
                starts_rank,
                status,
            })
        })
        .collect();
    AgentRows {
        rows,
        grace_deadline,
    }
}

pub(super) fn agents_content_height(rows: &[AgentRow]) -> f32 {
    if rows.is_empty() {
        return AGENTS_EMPTY_HEIGHT;
    }
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let spacing = if index > 0 { SIDEBAR_ROW_SPACING } else { 0. };
            let rank_spacing = if row.starts_rank {
                AGENT_RANK_SPACING
            } else {
                0.
            };
            AGENT_ROW_HEIGHT + spacing + rank_spacing
        })
        .sum()
}

pub(super) fn agents_section_height(rows: &[AgentRow], collapsed: bool) -> f32 {
    if collapsed {
        return AGENTS_HEADER_HEIGHT;
    }
    (AGENTS_HEADER_HEIGHT + agents_content_height(rows) + AGENTS_LIST_BOTTOM_PADDING)
        .min(AGENTS_HEIGHT_CAP)
}

pub(super) fn no_status_tooltip(tool: TerminalAgent) -> String {
    match tool.runtime().display.status_since {
        Some(version) => format!(
            "No OSC 7501 status received. {} reports it from version {version}.",
            tool.display_name()
        ),
        None => format!("No OSC 7501 status received from {}.", tool.display_name()),
    }
}

fn agent_state_words(state: SidebarAgentState) -> &'static str {
    match state {
        SidebarAgentState::NeedsInput => "Needs input",
        SidebarAgentState::Errored => "Errored",
        SidebarAgentState::Finished => "Finished, not yet seen",
        SidebarAgentState::Thinking => "Thinking",
    }
}

pub(super) fn agent_surface_location(
    workspaces: &[Workspace],
    surface_id: u64,
    cx: &App,
) -> Option<(usize, gpui::Entity<crate::pane::Pane>, usize)> {
    workspaces
        .iter()
        .enumerate()
        .find_map(|(workspace_idx, workspace)| {
            workspace
                .tabs()
                .iter()
                .flat_map(|tab| tab.collect_panes())
                .find_map(|pane| {
                    let surface_idx = pane.read(cx).surfaces().iter().position(|surface| {
                        surface
                            .as_terminal()
                            .is_some_and(|terminal| terminal.entity_id().as_u64() == surface_id)
                    })?;
                    Some((workspace_idx, pane, surface_idx))
                })
        })
}

fn render_agents_header(
    collapsed: bool,
    chevron_turn: f32,
    ui: crate::theme::UiColors,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
) -> gpui::Div {
    let hover_bg = crate::app::constants::sidebar_tab_hover_background();
    let chevron = svg()
        .size(px(AGENTS_CHEVRON_SIZE))
        .flex_none()
        .path("icons/chevron-down.svg")
        .text_color(ui.muted)
        .group_hover(AGENTS_TOGGLE_GROUP, move |style| style.text_color(ui.text))
        .with_transformation(gpui::Transformation::rotate(gpui::radians(
            -std::f32::consts::FRAC_PI_2 * chevron_turn,
        )));
    div()
        .h(px(AGENTS_HEADER_HEIGHT))
        .flex_none()
        .px(px(SIDEBAR_ROW_MARGIN_X))
        .flex()
        .flex_row()
        .items_center()
        .debug_selector(|| "agents-header".into())
        .child(
            squircle_skin(
                div()
                    .id("agents-section-toggle")
                    .h(px(SIDEBAR_ACTION_BUTTON_SIZE))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(SIDEBAR_TITLE_ROW_GAP))
                    .pl(px(SIDEBAR_ROW_PADDING_X))
                    .pr(px(6.))
                    .text_size(crate::ui_primitives::BODY_EMPHASIS)
                    .text_color(ui.muted)
                    .group_hover(AGENTS_TOGGLE_GROUP, move |style| style.text_color(ui.text))
                    .debug_selector(|| "agents-toggle".into()),
                AGENTS_TOGGLE_GROUP,
                px(7.),
                None,
                Some(hover_bg),
            )
            .role(Role::Button)
            .aria_label("Agents")
            .aria_expanded(!collapsed)
            .on_click(move |_, window, cx| on_toggle(window, cx))
            .child("Agents")
            .child(chevron),
        )
}

fn render_agent_status(row: &AgentRow, ui: crate::theme::UiColors) -> Option<AnyElement> {
    match row.status {
        AgentRowStatus::Lane(state) => Some(render_lane(
            Lane::Agent(SidebarAgentSummary {
                state,
                count: 1,
                tint: row.tool.accent(),
            }),
            &format!("agent-{}", row.surface_id),
            agent_state_words(state).into(),
            false,
            ui,
        )),
        AgentRowStatus::NoStatus => {
            let tooltip = SharedString::from(no_status_tooltip(row.tool));
            Some(
                div()
                    .id(SharedString::from(format!(
                        "agent-no-status-{}",
                        row.surface_id
                    )))
                    .flex_none()
                    .h(px(SIDEBAR_ROW_LINE_HEIGHT))
                    .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                    .text_size(crate::ui_primitives::LABEL_XS)
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .text_color(ui.muted)
                    .aria_label(tooltip.clone())
                    .delayed_tooltip(crate::ui_primitives::text_tooltip(tooltip))
                    .debug_selector(|| "agent-no-status".into())
                    .child("No status")
                    .into_any_element(),
            )
        }
        AgentRowStatus::Silent => None,
    }
}

fn render_agent_row(row: &AgentRow, ui: crate::theme::UiColors, on_pick: PickAgent) -> AnyElement {
    let surface_id = row.surface_id;
    let group = SharedString::from(format!("agent-row-group-{surface_id}"));
    let (resting, hovered) = if row.active {
        (
            Some(crate::app::constants::sidebar_tab_active_background()),
            None,
        )
    } else {
        (
            None,
            Some(crate::app::constants::sidebar_tab_hover_background()),
        )
    };
    let logo_tint = row
        .tool
        .accent()
        .map_or(ui.text, |tint| gpui::Hsla::from(rgb(tint)));
    let logo = div()
        .flex_none()
        .size(px(SIDEBAR_FOLDER_SLOT_WIDTH))
        .flex()
        .items_center()
        .justify_center()
        .child(crate::settings::components::render_logo(
            row.tool.icon_path(),
            row.tool.icon_multicolor(),
            px(AGENT_LOGO_SIZE),
            logo_tint,
        ));
    let title_text = div()
        .flex_1()
        .min_w_0()
        .overflow_x_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_size(crate::ui_primitives::TITLE)
        .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
        .font_weight(FontWeight::MEDIUM)
        .text_color(ui.text);
    let shimmer = row.status == AgentRowStatus::Lane(SidebarAgentState::Thinking)
        && !crate::ui_primitives::reduce_motion();
    let title = if shimmer {
        mark_shimmer_drawn();
        title_text.child(crate::ui_primitives::shimmer::shimmer_text(
            SharedString::from(row.title.clone()),
            ui.text,
            crate::ui_primitives::shimmer::shimmer_glint(ui.text),
            true,
            SharedString::from(row.title.clone()),
        ))
    } else {
        title_text.child(row.title.clone())
    };
    let title_row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(SIDEBAR_TITLE_ROW_GAP))
        .w_full()
        .min_w_0()
        .child(logo)
        .child(title)
        .children(render_agent_status(row, ui));
    let meta = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(SIDEBAR_ROW_GAP))
        .h(px(SIDEBAR_ROW_LINE_HEIGHT))
        .pl(px(SIDEBAR_FOLDER_SLOT_WIDTH + SIDEBAR_TITLE_ROW_GAP))
        .min_w_0()
        .child(
            svg()
                .size(px(AGENT_META_ICON_SIZE))
                .flex_none()
                .path("icons/folder.svg")
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(crate::ui_primitives::TITLE)
                .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                .text_color(ui.muted)
                .child(row.workspace.clone()),
        );
    let title_tooltip = [&row.title, &row.workspace]
        .iter()
        .any(|text| text.chars().count() > SIDEBAR_TITLE_TOOLTIP_MIN_CHARS)
        .then(|| SharedString::from(format!("{} \u{b7} {}", row.title, row.workspace)));
    let shell = sidebar_row_shell()
        .mx(px(SIDEBAR_ROW_MARGIN_X))
        .id(SharedString::from(format!("agent-row-{surface_id}")))
        .debug_selector(move || format!("agent-row-{surface_id}"));
    squircle_skin(shell, group, ROW_RADIUS, resting, hovered)
        .aria_label(SharedString::from(row.title.clone()))
        .when_some(title_tooltip, |shell, title| {
            shell.delayed_tooltip(crate::ui_primitives::text_tooltip(title))
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_pick(surface_id, window, cx);
        })
        .child(title_row)
        .child(meta)
        .into_any_element()
}

pub(super) struct AgentsSectionFrame<'a> {
    pub(super) rows: &'a [AgentRow],
    pub(super) collapsed: bool,
    pub(super) chevron_turn: f32,
    pub(super) scroll: &'a ScrollHandle,
    pub(super) scrollbar: Option<AnyElement>,
}

pub(super) fn render_agents_section(
    frame: AgentsSectionFrame,
    ui: crate::theme::UiColors,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    on_pick: PickAgent,
) -> gpui::Div {
    let AgentsSectionFrame {
        rows,
        collapsed,
        chevron_turn,
        scroll,
        scrollbar,
    } = frame;
    let section = div()
        .h(px(agents_section_height(rows, collapsed)))
        .min_h(px(AGENTS_HEADER_HEIGHT))
        .flex_shrink_1()
        .flex()
        .flex_col()
        .debug_selector(|| "agents-section".into())
        .child(render_agents_header(collapsed, chevron_turn, ui, on_toggle));
    if collapsed {
        return section;
    }
    let mut list = div()
        .id("agents-section-list")
        .flex_1()
        .min_h_0()
        .overflow_x_hidden()
        .overflow_y_scroll()
        .track_scroll(scroll)
        .flex()
        .flex_col()
        .pb(px(AGENTS_LIST_BOTTOM_PADDING));
    if rows.is_empty() {
        list = list.child(
            div()
                .flex_none()
                .h(px(AGENTS_EMPTY_HEIGHT))
                .flex()
                .items_center()
                .px(px(AGENTS_EMPTY_INSET))
                .text_size(crate::ui_primitives::BODY_EMPHASIS)
                .text_color(ui.muted)
                .debug_selector(|| "agents-empty".into())
                .child("No agents running"),
        );
    }
    for (index, row) in rows.iter().enumerate() {
        let spacing = if index > 0 { SIDEBAR_ROW_SPACING } else { 0. }
            + if row.starts_rank {
                AGENT_RANK_SPACING
            } else {
                0.
            };
        if spacing > 0. {
            list = list.child(div().flex_none().h(px(spacing)));
        }
        list = list.child(render_agent_row(row, ui, on_pick.clone()));
    }
    section.child(
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(list)
            .children(scrollbar),
    )
}

impl PaneFlowApp {
    pub(super) fn render_sidebar_agents(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let now = Instant::now();
        let AgentRows {
            rows,
            grace_deadline,
        } = agent_rows(
            &self.workspaces,
            self.active_space.as_ref(),
            self.shown_workspace_idx(),
            now,
            cx,
        );
        self.schedule_no_status_wake(grace_deadline, cx);
        let (collapsed, chevron_turn, turning, scroll) = {
            let mut section = self.agents_section.borrow_mut();
            if section.follow_top_row(rows.first().map(|row| row.surface_id)) {
                section.scroll.set_offset(Point::default());
            }
            (
                section.collapsed,
                section.chevron_turn_at(now),
                section.chevron_turning(now),
                section.scroll.clone(),
            )
        };
        if turning {
            window.request_animation_frame();
        }
        let ui = crate::theme::ui_colors();
        let scrollbar = crate::widgets::scrollbar::render(
            &scroll,
            ui,
            None,
            "agents-scrollbar-track",
            "agents-scrollbar-thumb",
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                let section = this.agents_section.borrow();
                if let Some(offset) =
                    crate::widgets::scrollbar::track_click_offset(&section.scroll, event.position.y)
                {
                    section.scroll.set_offset(Point::new(px(0.), px(offset)));
                    cx.notify();
                }
            }),
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                let mut section = this.agents_section.borrow_mut();
                let drag = crate::widgets::scrollbar::begin_drag(&section.scroll, event.position.y);
                section.scroll_drag = Some(drag);
                cx.stop_propagation();
            }),
        );
        let toggle_app = cx.weak_entity();
        let pick_app = cx.weak_entity();
        render_agents_section(
            AgentsSectionFrame {
                rows: &rows,
                collapsed,
                chevron_turn,
                scroll: &scroll,
                scrollbar,
            },
            ui,
            move |_, cx| {
                let _ = toggle_app.update(cx, |this, cx| {
                    this.agents_section
                        .borrow_mut()
                        .toggle(Instant::now(), !crate::ui_primitives::reduce_motion());
                    cx.notify();
                });
            },
            Rc::new(move |surface_id, window, cx| {
                let _ = pick_app.update(cx, |this, cx| {
                    this.focus_agent_surface(surface_id, window, cx);
                });
            }),
        )
        .on_scroll(cx.listener(|_, _, _, cx| cx.notify()))
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
            let section = this.agents_section.borrow();
            if let Some(drag) = section.scroll_drag
                && let Some(offset) =
                    crate::widgets::scrollbar::drag_offset(&section.scroll, &drag, event.position.y)
            {
                section.scroll.set_offset(Point::new(px(0.), px(offset)));
                cx.notify();
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                if this
                    .agents_section
                    .borrow_mut()
                    .scroll_drag
                    .take()
                    .is_some()
                {
                    cx.notify();
                }
            }),
        )
    }

    fn schedule_no_status_wake(&self, deadline: Option<Instant>, cx: &mut Context<Self>) {
        let mut section = self.agents_section.borrow_mut();
        let Some(deadline) = deadline else {
            section.grace_wake = None;
            return;
        };
        if section
            .grace_wake
            .as_ref()
            .is_some_and(|(held, _)| *held == deadline)
        {
            return;
        }
        let delay = deadline.saturating_duration_since(Instant::now());
        let wake = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        });
        section.grace_wake = Some((deadline, wake));
    }

    pub(crate) fn focus_agent_surface(
        &mut self,
        surface_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((workspace_idx, pane, surface_idx)) =
            agent_surface_location(&self.workspaces, surface_id, cx)
        else {
            cx.notify();
            return;
        };
        pane.update(cx, |pane, cx| pane.activate_surface(surface_idx, cx));
        self.activate_workspace_at(
            workspace_idx,
            WorkspaceFocusTarget::Pane { pane },
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_types::{AgentSession, AgentState};
    use crate::pane::Pane;
    use crate::terminal::TerminalView;
    use gpui::{AppContext as _, Modifiers, TestAppContext};
    use std::cell::{Cell, RefCell};

    fn codex() -> TerminalAgent {
        TerminalAgent::from_binary("codex").expect("codex runtime")
    }

    fn row(surface_id: u64, rank: AgentRank, status: AgentRowStatus) -> AgentRow {
        AgentRow {
            surface_id,
            rank,
            tool: TerminalAgent::ClaudeCode,
            title: "Attention sidebar".to_string(),
            workspace: "paneflow".to_string(),
            active: false,
            starts_rank: false,
            status,
        }
    }

    fn ranked_rows(ranks: &[AgentRank]) -> Vec<AgentRow> {
        let mut previous = None;
        ranks
            .iter()
            .enumerate()
            .map(|(index, &rank)| {
                let mut row = row(index as u64 + 1, rank, AgentRowStatus::Silent);
                row.starts_rank = previous.is_some_and(|previous| previous != rank);
                previous = Some(rank);
                row
            })
            .collect()
    }

    #[test]
    fn the_section_takes_its_content_height_plus_the_header_and_a_bottom_margin() {
        let rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Blocked, AgentRank::Done]);
        assert_eq!(AGENT_ROW_HEIGHT, 55.);
        assert_eq!(agents_content_height(&rows), 55. + (2. + 55.) + (10. + 55.));
        assert_eq!(agents_section_height(&rows, false), 36. + 177. + 4.);
        assert_eq!(agents_section_height(&rows[..1], false), 95.);
        assert_eq!(agents_section_height(&[], false), 36. + 32. + 4.);
    }

    #[test]
    fn the_section_height_is_capped_and_a_collapsed_section_keeps_only_its_header() {
        let rows = ranked_rows(&[AgentRank::Working; 10]);
        assert!(36. + agents_content_height(&rows) + 4. > 330.);
        assert_eq!(agents_section_height(&rows, false), 330.);
        assert_eq!(agents_section_height(&rows, true), 36.);
    }

    #[test]
    fn the_no_status_tooltip_names_the_version_only_when_the_catalog_knows_it() {
        assert_eq!(
            no_status_tooltip(TerminalAgent::ClaudeCode),
            "No OSC 7501 status received. Claude Code reports it from version 2.1.295."
        );
        assert_eq!(
            no_status_tooltip(codex()),
            format!(
                "No OSC 7501 status received from {}.",
                codex().display_name()
            )
        );
    }

    #[test]
    fn the_chevron_turns_over_180_ms_and_reverses_from_where_it_stands() {
        let now = Instant::now();
        let mut section = AgentsSection::default();
        assert_eq!(section.chevron_turn_at(now), 0.);

        section.toggle(now, true);
        assert!(section.collapsed);
        assert!(section.chevron_turning(now));
        let midway = section.chevron_turn_at(now + Duration::from_millis(90));
        assert!(midway > 0. && midway < 1., "{midway}");
        let settled = now + Duration::from_millis(180);
        assert_eq!(section.chevron_turn_at(settled), 1.);
        assert!(!section.chevron_turning(settled));

        let reversed_at = now + Duration::from_millis(90);
        section.toggle(reversed_at, true);
        assert_eq!(section.chevron_turn_at(reversed_at), midway);
        assert_eq!(
            section.chevron_turn_at(reversed_at + Duration::from_millis(180)),
            0.
        );
    }

    #[test]
    fn setting_the_collapsed_state_only_turns_the_chevron_on_a_change() {
        let now = Instant::now();
        let mut section = AgentsSection::default();
        section.set_collapsed(false, now, true);
        assert!(!section.collapsed() && !section.chevron_turning(now));
        section.set_collapsed(true, now, true);
        assert!(section.collapsed() && section.chevron_turning(now));
    }

    #[test]
    fn the_list_returns_to_the_top_only_when_a_new_agent_leads_it() {
        let mut section = AgentsSection::default();
        assert!(section.follow_top_row(Some(7)));
        assert!(!section.follow_top_row(Some(7)));
        assert!(section.follow_top_row(Some(3)));
        assert!(section.follow_top_row(None));
    }

    #[test]
    fn under_reduce_motion_the_chevron_snaps_without_an_animation() {
        let now = Instant::now();
        let mut section = AgentsSection::default();
        section.toggle(now, false);
        assert!(!section.chevron_turning(now));
        assert_eq!(section.chevron_turn_at(now), 1.);
        section.toggle(now, false);
        assert_eq!(section.chevron_turn_at(now), 0.);
    }

    fn workspace_with_agents(
        cx: &mut gpui::VisualTestContext,
        id: u64,
        name: &str,
        tabs: &[&str],
    ) -> (Workspace, Vec<u64>) {
        let mut surfaces = Vec::new();
        let mut workspace = None::<Workspace>;
        for (index, title) in tabs.iter().enumerate() {
            let terminal =
                cx.new(|cx| TerminalView::display_only_for_test(id * 10 + index as u64, cx));
            surfaces.push(terminal.entity_id().as_u64());
            let pane = cx.new(|cx| Pane::new(terminal, id, cx));
            match workspace.as_mut() {
                None => workspace = Some(Workspace::with_id(id, name, pane)),
                Some(workspace) => {
                    workspace.open_tab(crate::workspace::Tab::new(
                        String::new(),
                        Some(crate::layout::LayoutTree::Leaf(pane)),
                    ));
                }
            }
            let workspace = workspace.as_mut().expect("workspace");
            workspace.set_active_tab(index);
            workspace
                .active_tab_mut()
                .set_title(title, paneflow_config::schema::TabTitleSource::User);
        }
        let mut workspace = workspace.expect("workspace");
        workspace.set_active_tab(0);
        (workspace, surfaces)
    }

    fn agent(
        tool: TerminalAgent,
        state: AgentState,
        capable: bool,
        surface_id: u64,
        age: Duration,
    ) -> AgentSession {
        let mut session = AgentSession::new(tool, state);
        session.program_status_capable = capable;
        session.surface_id = Some(surface_id);
        session.started_at = Instant::now() - age;
        session
    }

    #[gpui::test]
    fn rows_carry_the_tab_title_workspace_runtime_and_their_rank_status(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let (mut first, first_surfaces) = workspace_with_agents(
            cx,
            1,
            "paneflow",
            &["Attention sidebar", "Refactor lane.rs"],
        );
        let (mut second, second_surfaces) = workspace_with_agents(cx, 2, "paneflow-web", &[""]);
        first.agent_sessions.insert(
            1,
            agent(
                TerminalAgent::ClaudeCode,
                AgentState::Thinking,
                true,
                first_surfaces[0],
                Duration::from_secs(10),
            ),
        );
        first.agent_sessions.insert(
            2,
            agent(
                codex(),
                AgentState::WaitingForInput,
                true,
                first_surfaces[1],
                Duration::from_secs(10),
            ),
        );
        second.agent_sessions.insert(
            3,
            agent(
                codex(),
                AgentState::Finished,
                false,
                second_surfaces[0],
                Duration::from_secs(10),
            ),
        );
        let workspaces = vec![first, second];

        let shown = cx.update(|_, cx| agent_rows(&workspaces, None, Some(0), Instant::now(), cx));

        assert_eq!(
            shown
                .rows
                .iter()
                .map(|row| (row.title.as_str(), row.workspace.as_str(), row.status))
                .collect::<Vec<_>>(),
            [
                (
                    "Refactor lane.rs",
                    "paneflow",
                    AgentRowStatus::Lane(SidebarAgentState::NeedsInput)
                ),
                (
                    "Attention sidebar",
                    "paneflow",
                    AgentRowStatus::Lane(SidebarAgentState::Thinking)
                ),
                (
                    codex().display_name(),
                    "paneflow-web",
                    AgentRowStatus::NoStatus
                ),
            ],
            "an untitled tab shows the runtime's name"
        );
        assert_eq!(shown.rows[1].tool, TerminalAgent::ClaudeCode);
        assert_eq!(
            shown
                .rows
                .iter()
                .map(|row| row.starts_rank)
                .collect::<Vec<_>>(),
            [false, true, true]
        );
        assert_eq!(
            shown.rows.iter().map(|row| row.active).collect::<Vec<_>>(),
            [false, true, false],
            "only the active tab of the shown workspace is active"
        );
        assert_eq!(shown.grace_deadline, None);
    }

    #[gpui::test]
    fn a_silent_session_shows_no_status_only_after_three_seconds_and_leaves_it_once_capable(
        cx: &mut TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let (mut workspace, surfaces) = workspace_with_agents(cx, 1, "paneflow", &["Docs nav"]);
        let started = Instant::now();
        let mut session = agent(
            codex(),
            AgentState::Finished,
            false,
            surfaces[0],
            Duration::ZERO,
        );
        session.started_at = started;
        workspace.agent_sessions.insert(1, session);
        let mut workspaces = vec![workspace];

        let young = cx.update(|_, cx| agent_rows(&workspaces, None, None, started, cx));
        assert_eq!(young.rows[0].rank, AgentRank::NoStatus);
        assert_eq!(young.rows[0].status, AgentRowStatus::Silent);
        assert_eq!(young.grace_deadline, Some(started + NO_STATUS_GRACE));

        let due = started + NO_STATUS_GRACE;
        let grown = cx.update(|_, cx| agent_rows(&workspaces, None, None, due, cx));
        assert_eq!(grown.rows[0].status, AgentRowStatus::NoStatus);
        assert_eq!(grown.grace_deadline, None);

        let session = workspaces[0].agent_sessions.get_mut(&1).expect("session");
        session.program_status_capable = true;
        session.state = AgentState::Thinking;
        let capable = cx.update(|_, cx| agent_rows(&workspaces, None, None, due, cx));
        assert_eq!(capable.rows[0].rank, AgentRank::Working);
        assert_eq!(
            capable.rows[0].status,
            AgentRowStatus::Lane(SidebarAgentState::Thinking)
        );
    }

    #[gpui::test]
    fn the_click_target_is_the_pane_and_surface_holding_the_session(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let (workspace, surfaces) = workspace_with_agents(cx, 1, "paneflow", &["a", "b"]);
        let second_terminal = cx.new(|cx| TerminalView::display_only_for_test(99, cx));
        let second_surface = second_terminal.entity_id().as_u64();
        let pane = workspace.tabs()[1].collect_panes()[0].clone();
        cx.update(|_, cx| {
            pane.update(cx, |pane, cx| {
                pane.push_surface(crate::pane::PaneSurface::Terminal(second_terminal), cx);
                pane.activate_surface(0, cx);
            });
        });
        let workspaces = vec![workspace];

        let located = cx.update(|_, cx| agent_surface_location(&workspaces, second_surface, cx));
        let (workspace_idx, located_pane, surface_idx) = located.expect("located");
        assert_eq!((workspace_idx, surface_idx), (0, 1));
        assert_eq!(located_pane.entity_id(), pane.entity_id());
        assert!(
            cx.update(|_, cx| agent_surface_location(&workspaces, surfaces[0], cx))
                .is_some_and(|(_, _, index)| index == 0)
        );
        assert!(
            cx.update(|_, cx| agent_surface_location(&workspaces, 9_999, cx))
                .is_none(),
            "a surface that no longer exists is ignored"
        );
    }

    struct SectionHarness {
        rows: Vec<AgentRow>,
        collapsed: bool,
        height: f32,
        picks: Rc<RefCell<Vec<u64>>>,
        toggles: Rc<Cell<usize>>,
    }

    impl gpui::Render for SectionHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let picks = self.picks.clone();
            let toggles = self.toggles.clone();
            div()
                .w(px(SIDEBAR_WIDTH))
                .h(px(self.height))
                .flex()
                .flex_col()
                .child(div().h(px(AGENTS_HEADER_HEIGHT)).flex_none())
                .child(
                    div()
                        .flex_1()
                        .min_h(px(AGENTS_TREE_MIN_HEIGHT))
                        .debug_selector(|| "tree".into()),
                )
                .child(render_agents_section(
                    AgentsSectionFrame {
                        rows: &self.rows,
                        collapsed: self.collapsed,
                        chevron_turn: if self.collapsed { 1. } else { 0. },
                        scroll: &ScrollHandle::new(),
                        scrollbar: None,
                    },
                    crate::theme::ui_colors(),
                    move |_, _| toggles.set(toggles.get() + 1),
                    Rc::new(move |surface_id, _, _| picks.borrow_mut().push(surface_id)),
                ))
                .child(div().h(px(55.)).flex_none())
        }
    }

    type Harness<'a> = (
        &'a mut gpui::VisualTestContext,
        Rc<RefCell<Vec<u64>>>,
        Rc<Cell<usize>>,
    );

    fn harness(
        cx: &mut TestAppContext,
        rows: Vec<AgentRow>,
        collapsed: bool,
        height: f32,
    ) -> Harness<'_> {
        let picks = Rc::new(RefCell::new(Vec::new()));
        let toggles = Rc::new(Cell::new(0));
        let (held_picks, held_toggles) = (picks.clone(), toggles.clone());
        let (_view, cx) = cx.add_window_view(move |_, _| SectionHarness {
            rows,
            collapsed,
            height,
            picks,
            toggles,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (cx, held_picks, held_toggles)
    }

    #[gpui::test]
    fn the_rendered_section_sits_under_the_tree_with_its_header_rows_and_rank_gaps(
        cx: &mut TestAppContext,
    ) {
        let mut rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Errored, AgentRank::Errored]);
        rows[0].status = AgentRowStatus::Lane(SidebarAgentState::NeedsInput);
        let (cx, _, _) = harness(cx, rows, false, 800.);

        let section = cx.debug_bounds("agents-section").expect("section");
        let tree = cx.debug_bounds("tree").expect("tree");
        assert_eq!(
            section.top(),
            tree.bottom(),
            "no separator between tree and section"
        );
        assert_eq!(
            section.size.height,
            px(36. + 55. + (10. + 55.) + (2. + 55.) + 4.)
        );
        let header = cx.debug_bounds("agents-header").expect("header");
        assert_eq!(header.size.height, px(36.));
        let toggle = cx.debug_bounds("agents-toggle").expect("toggle");
        assert_eq!(toggle.size.height, px(22.));
        assert_eq!(toggle.left(), section.left() + px(8.));

        let first = cx.debug_bounds("agent-row-1").expect("row 1");
        let second = cx.debug_bounds("agent-row-2").expect("row 2");
        let third = cx.debug_bounds("agent-row-3").expect("row 3");
        assert_eq!(first.top(), header.bottom());
        assert_eq!(first.size.height, px(55.));
        assert_eq!(first.left(), section.left() + px(8.));
        assert_eq!(
            second.top() - first.bottom(),
            px(10.),
            "a new rank adds 8 px"
        );
        assert_eq!(third.top() - second.bottom(), px(2.));
        assert!(cx.debug_bounds("agents-empty").is_none());
    }

    #[gpui::test]
    fn an_empty_section_says_no_agents_running(cx: &mut TestAppContext) {
        let (cx, _, _) = harness(cx, Vec::new(), false, 800.);
        let empty = cx.debug_bounds("agents-empty").expect("empty state");
        assert_eq!(empty.size.height, px(32.));
        assert_eq!(
            cx.debug_bounds("agents-section")
                .expect("section")
                .size
                .height,
            px(36. + 32. + 4.)
        );
    }

    #[gpui::test]
    fn a_long_list_caps_at_330_px(cx: &mut TestAppContext) {
        let (cx, _, _) = harness(cx, ranked_rows(&[AgentRank::Working; 10]), false, 900.);
        assert_eq!(
            cx.debug_bounds("agents-section")
                .expect("section")
                .size
                .height,
            px(330.)
        );
    }

    #[gpui::test]
    fn a_short_window_keeps_120_px_of_tree_and_the_header(cx: &mut TestAppContext) {
        let short = 36. + 120. + 55. + 100.;
        let (cx, _, _) = harness(cx, ranked_rows(&[AgentRank::Working; 10]), false, short);
        let tree = cx.debug_bounds("tree").expect("tree");
        let section = cx.debug_bounds("agents-section").expect("section");
        assert_eq!(tree.size.height, px(120.));
        assert_eq!(section.size.height, px(100.));
        assert_eq!(
            cx.debug_bounds("agents-header")
                .expect("header")
                .size
                .height,
            px(36.)
        );
    }

    #[gpui::test]
    fn a_collapsed_section_keeps_only_its_header(cx: &mut TestAppContext) {
        let rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Working]);
        let (cx, _, _) = harness(cx, rows, true, 800.);
        assert_eq!(
            cx.debug_bounds("agents-section")
                .expect("section")
                .size
                .height,
            px(36.)
        );
        assert!(cx.debug_bounds("agent-row-1").is_none());
    }

    #[gpui::test]
    fn clicking_a_row_picks_its_surface_and_the_header_toggles(cx: &mut TestAppContext) {
        let rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Working]);
        let (cx, picks, toggles) = harness(cx, rows, false, 800.);

        let second = cx.debug_bounds("agent-row-2").expect("row 2");
        cx.simulate_click(second.center(), Modifiers::default());
        assert_eq!(*picks.borrow(), [2]);

        let toggle = cx.debug_bounds("agents-toggle").expect("toggle");
        cx.simulate_click(toggle.center(), Modifiers::default());
        assert_eq!(toggles.get(), 1);
        assert_eq!(*picks.borrow(), [2]);
    }

    #[gpui::test]
    fn a_silent_session_renders_the_no_status_label(cx: &mut TestAppContext) {
        let rows = vec![
            row(1, AgentRank::NoStatus, AgentRowStatus::NoStatus),
            row(2, AgentRank::NoStatus, AgentRowStatus::Silent),
        ];
        let (cx, _, _) = harness(cx, rows, false, 800.);
        let label = cx.debug_bounds("agent-no-status").expect("no status label");
        let first = cx.debug_bounds("agent-row-1").expect("row 1");
        assert!(first.contains(&label.center()));
        assert_eq!(label.size.height, px(20.));
    }

    struct LoadedSidebar {
        workspaces: Vec<Workspace>,
        build_times: Rc<RefCell<Vec<Duration>>>,
    }

    impl gpui::Render for LoadedSidebar {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let started = Instant::now();
            let section = loaded_section(&self.workspaces, cx);
            self.build_times.borrow_mut().push(started.elapsed());
            div()
                .w(px(SIDEBAR_WIDTH))
                .h(px(900.))
                .flex()
                .flex_col()
                .child(div().flex_1().min_h(px(AGENTS_TREE_MIN_HEIGHT)))
                .child(section)
        }
    }

    fn loaded_section(workspaces: &[Workspace], cx: &App) -> gpui::Div {
        let AgentRows { rows, .. } = agent_rows(workspaces, None, Some(0), Instant::now(), cx);
        render_agents_section(
            AgentsSectionFrame {
                rows: &rows,
                collapsed: false,
                chevron_turn: 0.,
                scroll: &ScrollHandle::new(),
                scrollbar: None,
            },
            crate::theme::ui_colors(),
            |_, _| {},
            Rc::new(|_, _, _| {}),
        )
    }

    #[gpui::test]
    #[cfg_attr(
        debug_assertions,
        ignore = "frame budget holds for release builds only"
    )]
    fn fifty_agents_across_thirty_workspaces_fit_the_sidebar_frame_budget(cx: &mut TestAppContext) {
        let states = [
            (AgentState::WaitingForInput, true),
            (AgentState::Errored, true),
            (AgentState::Finished, true),
            (AgentState::Thinking, true),
            (AgentState::Finished, false),
        ];
        let workspaces = {
            let window = cx.add_empty_window();
            let mut workspaces = Vec::new();
            let mut placed = 0;
            for id in 0..30u64 {
                let tabs: &[&str] = if id < 20 {
                    &["Attention sidebar refactor", ""]
                } else {
                    &["Docs navigation"]
                };
                let (mut workspace, surfaces) =
                    workspace_with_agents(window, id + 1, &format!("workspace-{id}"), tabs);
                for surface in surfaces {
                    let (state, capable) = states[placed % states.len()];
                    let tool = if placed % 2 == 0 {
                        TerminalAgent::ClaudeCode
                    } else {
                        codex()
                    };
                    workspace.agent_sessions.insert(
                        placed as u32,
                        agent(tool, state, capable, surface, Duration::from_secs(10)),
                    );
                    placed += 1;
                }
                workspaces.push(workspace);
            }
            assert_eq!(placed, 50);
            workspaces
        };
        let build_times = Rc::new(RefCell::new(Vec::new()));
        let held_times = build_times.clone();
        let (view, cx) = cx.add_window_view(move |_, _| LoadedSidebar {
            workspaces,
            build_times,
        });

        let mut frames = Vec::new();
        for _ in 0..30 {
            view.update(cx, |_, cx| cx.notify());
            let started = Instant::now();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            frames.push(started.elapsed());
        }
        assert_eq!(
            cx.update(|_, cx| agent_rows(
                &view.read(cx).workspaces,
                None,
                Some(0),
                Instant::now(),
                cx
            ))
            .rows
            .len(),
            50
        );

        let builds = held_times.borrow();
        let worst_build = builds.iter().skip(5).max().copied().unwrap_or_default();
        let worst_frame = frames.iter().skip(5).max().copied().unwrap_or_default();
        println!("agents section: worst build {worst_build:?}, worst full frame {worst_frame:?}");
        assert!(
            worst_build < Duration::from_millis(16),
            "building the section took {worst_build:?}"
        );
        assert!(
            worst_frame < Duration::from_millis(16),
            "a full frame took {worst_frame:?}"
        );
    }
}
