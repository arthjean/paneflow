use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, CursorStyle, DispatchPhase, FocusHandle, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Orientation, Point, ScrollHandle, Task, canvas, rgb,
};

use super::agents::{AgentRank, agent_candidates, order_agents};
use super::customize_menu::{SHIMMER_ICON, SidebarShowLine, menu_divider, render_show_option};
use super::lane::{mark_shimmer_drawn, render_lane, render_status_badge};
use super::*;
use crate::agent_launcher::TerminalAgent;
use crate::app::workspace_ops::WorkspaceFocusTarget;
use crate::settings::components::{MENU_PADDING, MENU_ROW_GAP, menu_surface_filled};
use crate::window_state::AgentsSectionState;

pub(super) const AGENTS_HEADER_HEIGHT: f32 = 36.;
pub(super) const AGENTS_TREE_MIN_HEIGHT: f32 = 120.;
const AGENTS_LIST_BOTTOM_PADDING: f32 = 4.;
const AGENTS_HEIGHT_CAP: f32 = 330.;
const AGENTS_RESIZE_FLOOR: f32 = AGENTS_HEADER_HEIGHT
    + AGENT_GROUP_HEADER_HEIGHT
    + AGENT_LINE_HEIGHT
    + AGENTS_LIST_BOTTOM_PADDING;
const AGENTS_RESIZE_MAX: f32 = 620.;
const AGENTS_RESIZE_HIT_HEIGHT: f32 = 9.;
const AGENTS_RESIZE_STEP: f32 = 16.;
const AGENTS_RESIZE_FOCUS_ALPHA: f32 = 0.35;
const AGENTS_RESIZE_GROUP: &str = "agents-resize-group";
const AGENTS_EMPTY_HEIGHT: f32 = 32.;
const AGENTS_EMPTY_INSET: f32 = 15.;
const AGENT_LINE_HEIGHT: f32 = SIDEBAR_ROW_PADDING_Y * 2. + SIDEBAR_ROW_LINE_HEIGHT;
const AGENT_GROUP_HEADER_HEIGHT: f32 = 24.;
const AGENT_GROUP_SPACING: f32 = SIDEBAR_TITLE_ROW_GAP;
const AGENT_GROUP_HEADER_INSET: f32 = SIDEBAR_ROW_MARGIN_X + SIDEBAR_ROW_PADDING_X;
const AGENT_GROUP_COUNT_GAP: f32 = 6.;
const AGENT_GROUP_HEADING_LEVEL: usize = 2;
const AGENT_LOGO_SIZE: f32 = 15.;
const AGENTS_CHEVRON_SIZE: f32 = 12.;
const AGENTS_TOGGLE_HEIGHT: f32 = 20.;
const AGENTS_TOGGLE_GAP: f32 = 4.;
const AGENTS_CHEVRON_TURN_SECS: f32 = 0.18;
const NO_STATUS_GRACE: Duration = Duration::from_secs(3);
const AGENTS_TOGGLE_GROUP: &str = "agents-section-toggle-group";
const AGENTS_DISPLAY_GROUP: &str = "agents-display-group";
const AGENTS_DISPLAY_MENU_WIDTH: f32 = 172.;
const AGENTS_DISPLAY_MENU_GAP: f32 = 10.;
const AGENT_LOGO_ICON: &str = "icons/chat-bot.svg";
const AGENT_WORKSPACE_ICON: &str = "icons/folder.svg";
const AGENT_ALL_SPACES_ICON: &str = "icons/palette/layers-01.svg";

type PickAgent = Rc<dyn Fn(u64, &mut Window, &mut App)>;
type ResizeAgents = Rc<dyn Fn(AgentsResizeInput, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum AgentsResizeInput {
    Press(f32),
    Drag(f32),
    Release,
    Fit,
    Step(f32),
    Floor,
    Ceiling,
    Leave,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AgentsResizeDrag {
    anchor_y: f32,
    anchor_height: f32,
    ceiling: f32,
}

#[derive(Default)]
pub(crate) struct AgentsSection {
    collapsed: bool,
    height: Option<f32>,
    resize_drag: Option<AgentsResizeDrag>,
    turn_from: f32,
    turn_started: Option<Instant>,
    grace_wake: Option<(Instant, Task<()>)>,
    scroll: ScrollHandle,
    scroll_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
    top_surface: Option<u64>,
}

impl AgentsSection {
    pub(crate) fn restored(state: AgentsSectionState) -> Self {
        Self {
            collapsed: state.collapsed,
            height: state.height,
            ..Self::default()
        }
    }

    pub(crate) fn persisted(&self) -> AgentsSectionState {
        AgentsSectionState {
            height: self.height,
            collapsed: self.collapsed,
        }
    }

    fn shown_height(&self) -> f32 {
        AGENTS_HEADER_HEIGHT + f32::from(self.scroll.bounds().size.height)
    }

    fn available_height(&self, tree: &ScrollHandle) -> f32 {
        self.shown_height() + f32::from(tree.bounds().size.height)
    }

    fn resize_within(
        &mut self,
        input: AgentsResizeInput,
        natural: f32,
        tree: &ScrollHandle,
    ) -> bool {
        let ceiling = agents_resize_ceiling(natural, self.available_height(tree));
        self.resize(input, self.shown_height(), ceiling)
    }

    fn resize(&mut self, input: AgentsResizeInput, shown: f32, ceiling: f32) -> bool {
        let bounded =
            |height: f32| height.clamp(AGENTS_RESIZE_FLOOR, ceiling.max(AGENTS_RESIZE_FLOOR));
        match input {
            AgentsResizeInput::Press(y) => {
                self.resize_drag = Some(AgentsResizeDrag {
                    anchor_y: y,
                    anchor_height: shown,
                    ceiling,
                });
                false
            }
            AgentsResizeInput::Drag(y) => {
                let Some(drag) = self.resize_drag else {
                    return false;
                };
                self.height = Some(
                    (drag.anchor_height + drag.anchor_y - y)
                        .clamp(AGENTS_RESIZE_FLOOR, drag.ceiling.max(AGENTS_RESIZE_FLOOR)),
                );
                false
            }
            AgentsResizeInput::Release => self.resize_drag.take().is_some(),
            AgentsResizeInput::Fit => {
                self.resize_drag = None;
                self.height = None;
                true
            }
            AgentsResizeInput::Step(delta) => {
                self.height = Some(bounded(shown + delta));
                true
            }
            AgentsResizeInput::Floor => {
                self.height = Some(AGENTS_RESIZE_FLOOR);
                true
            }
            AgentsResizeInput::Ceiling => {
                self.height = Some(bounded(ceiling));
                true
            }
            AgentsResizeInput::Leave => false,
        }
    }

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AgentRowDetails {
    pub(super) logo: bool,
    pub(super) workspace: bool,
    pub(super) shimmer: bool,
}

impl AgentRowDetails {
    pub(super) const ALL: Self = Self {
        logo: true,
        workspace: true,
        shimmer: true,
    };

    pub(super) fn from_show(show: paneflow_config::schema::SidebarShow) -> Self {
        Self {
            logo: show.agent_logo_enabled(),
            workspace: show.agent_workspace_enabled(),
            shimmer: show.agent_shimmer_enabled(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct AgentScope<'a> {
    active_space: Option<&'a paneflow_config::schema::SpaceId>,
    every_space: Option<&'a paneflow_config::schema::PaneFlowConfig>,
}

impl<'a> AgentScope<'a> {
    pub(super) fn active_space_only(
        active_space: Option<&'a paneflow_config::schema::SpaceId>,
    ) -> Self {
        Self {
            active_space,
            every_space: None,
        }
    }

    pub(super) fn every_space(
        active_space: Option<&'a paneflow_config::schema::SpaceId>,
        config: &'a paneflow_config::schema::PaneFlowConfig,
    ) -> Self {
        Self {
            active_space,
            every_space: Some(config),
        }
    }

    pub(super) fn from_config(
        active_space: Option<&'a paneflow_config::schema::SpaceId>,
        config: &'a paneflow_config::schema::PaneFlowConfig,
    ) -> Self {
        if config.sidebar_show.agent_all_spaces_enabled() {
            Self::every_space(active_space, config)
        } else {
            Self::active_space_only(active_space)
        }
    }

    fn other_space_name(self, workspace: &Workspace, in_active_space: bool) -> Option<String> {
        let config = self.every_space?;
        (!in_active_space)
            .then(|| crate::app::spaces::space_label(config, workspace.space.as_ref()).to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AgentRow {
    pub(super) surface_id: u64,
    pub(super) rank: AgentRank,
    pub(super) tool: TerminalAgent,
    pub(super) title: String,
    pub(super) workspace: String,
    pub(super) space: Option<String>,
    pub(super) active: bool,
    pub(super) status: AgentRowStatus,
    pub(super) details: AgentRowDetails,
}

pub(super) struct AgentRows {
    pub(super) rows: Vec<AgentRow>,
    pub(super) grace_deadline: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentGroupStatus {
    NeedYou,
    Error,
    Done,
    Working,
    Idle,
    NoStatus,
}

impl AgentGroupStatus {
    const ORDER: [Self; 6] = [
        Self::NeedYou,
        Self::Error,
        Self::Done,
        Self::Working,
        Self::Idle,
        Self::NoStatus,
    ];

    fn of(status: AgentRowStatus) -> Self {
        match status {
            AgentRowStatus::Lane(SidebarAgentState::NeedsInput) => Self::NeedYou,
            AgentRowStatus::Lane(SidebarAgentState::Errored) => Self::Error,
            AgentRowStatus::Lane(SidebarAgentState::Finished) => Self::Done,
            AgentRowStatus::Lane(SidebarAgentState::Thinking) => Self::Working,
            AgentRowStatus::Silent => Self::Idle,
            AgentRowStatus::NoStatus => Self::NoStatus,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::NeedYou => "Need you",
            Self::Error => "Error",
            Self::Done => "Done",
            Self::Working => "Working",
            Self::Idle => "Idle",
            Self::NoStatus => "No status",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::NeedYou => "need-you",
            Self::Error => "error",
            Self::Done => "done",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::NoStatus => "no-status",
        }
    }

    fn color(self, ui: crate::theme::UiColors) -> gpui::Hsla {
        match self {
            Self::NeedYou => crate::app::constants::sidebar_needs_input_color(),
            Self::Error => ui.agent_error,
            Self::Done => crate::app::constants::sidebar_finished_color(),
            Self::Working | Self::Idle | Self::NoStatus => ui.muted,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AgentGroup<'a> {
    pub(super) status: AgentGroupStatus,
    pub(super) rows: Vec<&'a AgentRow>,
}

pub(super) fn agent_groups(rows: &[AgentRow]) -> Vec<AgentGroup<'_>> {
    AgentGroupStatus::ORDER
        .into_iter()
        .filter_map(|status| {
            let members: Vec<&AgentRow> = rows
                .iter()
                .filter(|row| AgentGroupStatus::of(row.status) == status)
                .collect();
            (!members.is_empty()).then_some(AgentGroup {
                status,
                rows: members,
            })
        })
        .collect()
}

pub(super) fn agent_group_label(status: AgentGroupStatus, count: usize) -> String {
    let noun = if count == 1 { "agent" } else { "agents" };
    format!("{}, {count} {noun}", status.label())
}

pub(super) fn agent_rows(
    workspaces: &[Workspace],
    scope: AgentScope<'_>,
    shown_workspace: Option<usize>,
    details: AgentRowDetails,
    now: Instant,
    cx: &App,
) -> AgentRows {
    let mut grace_deadline: Option<Instant> = None;
    let rows = order_agents(
        agent_candidates(workspaces, scope.active_space, cx),
        scope.every_space.is_none(),
    )
    .into_iter()
    .filter_map(|candidate| {
        let workspace = workspaces.get(candidate.workspace_idx)?;
        let session = workspace.agent_sessions.get(&candidate.session_key)?;
        let tab = workspace.tabs().get(candidate.tab_idx)?;
        let status = match (lane_state(candidate.rank), candidate.rank) {
            (Some(state), _) => AgentRowStatus::Lane(state),
            (None, AgentRank::NoStatus) => {
                let due = session.started_at + NO_STATUS_GRACE;
                if now >= due {
                    AgentRowStatus::NoStatus
                } else {
                    grace_deadline = Some(grace_deadline.map_or(due, |held| held.min(due)));
                    AgentRowStatus::Silent
                }
            }
            (None, _) => AgentRowStatus::Silent,
        };
        let title = if tab.title().trim().is_empty() {
            session.tool.display_name().to_string()
        } else {
            tab.title().to_string()
        };
        Some(AgentRow {
            surface_id: candidate.surface_id,
            rank: candidate.rank,
            tool: session.tool,
            title,
            workspace: workspace.title.clone(),
            space: scope.other_space_name(workspace, candidate.in_active_space),
            active: shown_workspace == Some(candidate.workspace_idx)
                && workspace.active_tab_idx() == candidate.tab_idx,
            status,
            details,
        })
    })
    .collect();
    AgentRows {
        rows,
        grace_deadline,
    }
}

pub(super) fn agents_content_height(groups: &[AgentGroup<'_>]) -> f32 {
    if groups.is_empty() {
        return AGENTS_EMPTY_HEIGHT;
    }
    groups
        .iter()
        .enumerate()
        .map(|(index, group)| {
            let spacing = if index > 0 { AGENT_GROUP_SPACING } else { 0. };
            let lines = group.rows.len() as f32;
            spacing
                + AGENT_GROUP_HEADER_HEIGHT
                + lines * AGENT_LINE_HEIGHT
                + (lines - 1.).max(0.) * SIDEBAR_ROW_SPACING
        })
        .sum()
}

fn agents_natural_height(groups: &[AgentGroup<'_>]) -> f32 {
    AGENTS_HEADER_HEIGHT + agents_content_height(groups) + AGENTS_LIST_BOTTOM_PADDING
}

pub(super) fn agents_section_height(
    groups: &[AgentGroup<'_>],
    collapsed: bool,
    chosen: Option<f32>,
) -> f32 {
    if collapsed {
        return AGENTS_HEADER_HEIGHT;
    }
    let natural = agents_natural_height(groups);
    match chosen.filter(|_| !groups.is_empty()) {
        Some(height) => height.clamp(
            AGENTS_RESIZE_FLOOR,
            natural.clamp(AGENTS_RESIZE_FLOOR, AGENTS_RESIZE_MAX),
        ),
        None => natural.min(AGENTS_HEIGHT_CAP),
    }
}

fn agents_resize_ceiling(natural: f32, available: f32) -> f32 {
    AGENTS_RESIZE_MAX
        .min(natural)
        .min(available - AGENTS_TREE_MIN_HEIGHT)
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

fn agent_row_spoken_state(row: &AgentRow) -> &'static str {
    match (row.status, row.rank) {
        (AgentRowStatus::Lane(SidebarAgentState::NeedsInput), _) => "waiting for input",
        (AgentRowStatus::Lane(SidebarAgentState::Errored), _) => "error",
        (AgentRowStatus::Lane(SidebarAgentState::Finished), _) => "done, not yet seen",
        (AgentRowStatus::Lane(SidebarAgentState::Thinking), _) => "working",
        (AgentRowStatus::NoStatus, _) => "no status",
        (AgentRowStatus::Silent, AgentRank::NoStatus) => "starting",
        (AgentRowStatus::Silent, _) => "idle",
    }
}

fn agent_title_shimmers(
    status: AgentRowStatus,
    details: AgentRowDetails,
    reduce_motion: bool,
) -> bool {
    details.shimmer && status == AgentRowStatus::Lane(SidebarAgentState::Thinking) && !reduce_motion
}

fn agent_workspace_suffix(row: &AgentRow) -> String {
    match &row.space {
        Some(space) => format!(" \u{b7} {} \u{b7} {space}", row.workspace),
        None => format!(" \u{b7} {}", row.workspace),
    }
}

fn agent_row_full_name(row: &AgentRow) -> String {
    format!("{}{}", row.title, agent_workspace_suffix(row))
}

fn agent_row_shown_name(row: &AgentRow) -> String {
    if row.details.workspace {
        agent_row_full_name(row)
    } else {
        row.title.clone()
    }
}

pub(super) fn agent_row_label(row: &AgentRow) -> String {
    let place = match &row.space {
        Some(space) => format!("{}, {space} space", row.workspace),
        None => row.workspace.clone(),
    };
    format!(
        "{}, {}, {place}, {}",
        row.tool.display_name(),
        row.title,
        agent_row_spoken_state(row)
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AgentListPosition {
    pub(super) row: usize,
    pub(super) group_header: Option<usize>,
}

pub(super) fn agent_list_position(
    groups: &[AgentGroup<'_>],
    surface_id: u64,
) -> Option<AgentListPosition> {
    let mut child = 0;
    for group in groups {
        let header = child;
        child += 1;
        for (index, row) in group.rows.iter().enumerate() {
            if row.surface_id == surface_id {
                return Some(AgentListPosition {
                    row: child,
                    group_header: (index == 0).then_some(header),
                });
            }
            child += 1;
        }
    }
    None
}

pub(super) fn scroll_agents_list_to(
    scroll: &ScrollHandle,
    groups: &[AgentGroup<'_>],
    surface_id: u64,
) {
    let Some(position) = agent_list_position(groups, surface_id) else {
        return;
    };
    let hidden_header = position.group_header.filter(|&header| {
        scroll
            .bounds_for_item(header)
            .is_some_and(|bounds| bounds.top() + scroll.offset().y < scroll.bounds().top())
    });
    scroll.scroll_to_item(hidden_header.unwrap_or(position.row));
}

fn agents_nav_targets(enabled: bool, collapsed: bool, groups: &[AgentGroup<'_>]) -> Vec<u64> {
    if !enabled || collapsed {
        return Vec::new();
    }
    groups
        .iter()
        .flat_map(|group| group.rows.iter().map(|row| row.surface_id))
        .collect()
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
    display_button: Option<AnyElement>,
    ui: crate::theme::UiColors,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
) -> gpui::Div {
    let chevron = svg()
        .size(px(AGENTS_CHEVRON_SIZE))
        .flex_none()
        .path("icons/palette/arrow-down-01.svg")
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
        .justify_between()
        .debug_selector(|| "agents-header".into())
        .child(
            div()
                .id("agents-section-toggle")
                .group(AGENTS_TOGGLE_GROUP)
                .h(px(AGENTS_TOGGLE_HEIGHT))
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(AGENTS_TOGGLE_GAP))
                .text_size(crate::ui_primitives::BODY)
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui.muted)
                .cursor_pointer()
                .hover(move |style| style.text_color(ui.text))
                .debug_selector(|| "agents-toggle".into())
                .role(Role::Button)
                .aria_label("Agents")
                .aria_expanded(!collapsed)
                .on_click(move |_, window, cx| on_toggle(window, cx))
                .child(chevron)
                .child("Agents"),
        )
        .children(display_button)
}

fn render_agent_group_header(
    group: &AgentGroup<'_>,
    leads_the_list: bool,
    ui: crate::theme::UiColors,
) -> gpui::Stateful<gpui::Div> {
    let status = group.status;
    let key = status.key();
    div()
        .id(SharedString::from(format!("agent-group-{key}")))
        .role(Role::Heading)
        .aria_level(AGENT_GROUP_HEADING_LEVEL)
        .aria_label(SharedString::from(agent_group_label(
            status,
            group.rows.len(),
        )))
        .flex_none()
        .h(px(AGENT_GROUP_HEADER_HEIGHT))
        .when(!leads_the_list, |header| header.mt(px(AGENT_GROUP_SPACING)))
        .px(px(AGENT_GROUP_HEADER_INSET))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(AGENT_GROUP_COUNT_GAP))
        .overflow_x_hidden()
        .whitespace_nowrap()
        .text_size(crate::ui_primitives::BODY)
        .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
        .font_weight(FontWeight::MEDIUM)
        .debug_selector(move || format!("agent-group-{key}"))
        .child(
            div()
                .flex_none()
                .text_color(status.color(ui))
                .debug_selector(move || format!("agent-group-label-{key}"))
                .child(status.label()),
        )
        .child(
            div()
                .flex_none()
                .text_color(ui.muted)
                .font_features(tabular_numerals())
                .debug_selector(move || format!("agent-group-count-{key}"))
                .child(group.rows.len().to_string()),
        )
}

fn render_agent_group_member(
    row: gpui::Stateful<gpui::Div>,
    index: usize,
    group_size: usize,
) -> gpui::Stateful<gpui::Div> {
    row.aria_position_in_set(index + 1)
        .aria_size_of_set(group_size)
        .when(index > 0, |row| row.mt(px(SIDEBAR_ROW_SPACING)))
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
                    .flex()
                    .items_start()
                    .aria_label(tooltip.clone())
                    .delayed_tooltip(crate::ui_primitives::text_tooltip(tooltip))
                    .debug_selector(|| "agent-no-status".into())
                    .child(render_status_badge(
                        "No status".to_string(),
                        &format!("agent-no-status-{}", row.surface_id),
                        ui.muted,
                    ))
                    .into_any_element(),
            )
        }
        AgentRowStatus::Silent => None,
    }
}

fn render_agent_row(
    row: &AgentRow,
    cursor: bool,
    ui: crate::theme::UiColors,
    on_pick: PickAgent,
) -> gpui::Stateful<gpui::Div> {
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
        .w(px(SIDEBAR_FOLDER_SLOT_WIDTH))
        .h(px(SIDEBAR_ROW_LINE_HEIGHT))
        .pt(px(sidebar_glyph_top(AGENT_LOGO_SIZE)))
        .flex()
        .justify_center()
        .child(
            div()
                .flex_none()
                .size(px(AGENT_LOGO_SIZE))
                .debug_selector(move || format!("agent-logo-{surface_id}"))
                .child(crate::settings::components::render_logo(
                    row.tool.icon_path(),
                    row.tool.icon_multicolor(),
                    px(AGENT_LOGO_SIZE),
                    logo_tint,
                )),
        );
    let title_text = div()
        .flex_none()
        .max_w_full()
        .min_w_0()
        .overflow_x_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .font_weight(FontWeight::MEDIUM)
        .text_color(ui.text)
        .debug_selector(move || format!("agent-title-{surface_id}"));
    let shimmer = agent_title_shimmers(
        row.status,
        row.details,
        crate::ui_primitives::reduce_motion(),
    );
    let title = if shimmer {
        mark_shimmer_drawn();
        let (resting, glint) = crate::ui_primitives::shimmer::working_title_shimmer(ui);
        title_text
            .text_color(resting)
            .child(crate::ui_primitives::shimmer::shimmer_text(
                SharedString::from(row.title.clone()),
                resting,
                glint,
                SharedString::from(row.title.clone()),
            ))
    } else {
        title_text.child(row.title.clone())
    };
    let workspace_suffix = row.details.workspace.then(|| {
        div()
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(ui.muted)
            .debug_selector(move || format!("agent-workspace-{surface_id}"))
            .child(agent_workspace_suffix(row))
    });
    let name = div()
        .flex_1()
        .min_w_0()
        .overflow_x_hidden()
        .flex()
        .flex_row()
        .text_size(crate::ui_primitives::TITLE)
        .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
        .child(title)
        .children(workspace_suffix);
    let line = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(SIDEBAR_TITLE_ROW_GAP))
        .w_full()
        .min_w_0()
        .when(row.details.logo, |line| line.child(logo))
        .child(name)
        .children(render_agent_status(row, ui).map(|status| {
            div()
                .flex_none()
                .debug_selector(move || format!("agent-status-{surface_id}"))
                .child(status)
        }));
    let title_tooltip = (agent_row_shown_name(row).chars().count()
        > SIDEBAR_TITLE_TOOLTIP_MIN_CHARS)
        .then(|| SharedString::from(agent_row_full_name(row)));
    let shell = sidebar_row_shell()
        .mx(px(SIDEBAR_ROW_MARGIN_X))
        .id(SharedString::from(format!("agent-row-{surface_id}")))
        .debug_selector(move || format!("agent-row-{surface_id}"));
    squircle_skin(shell, group, ROW_RADIUS, resting, hovered)
        .role(Role::ListItem)
        .aria_label(SharedString::from(agent_row_label(row)))
        .aria_selected(row.active)
        .when(row.status == AgentRowStatus::NoStatus, |shell| {
            shell.aria_description(no_status_tooltip(row.tool))
        })
        .when_some(title_tooltip, |shell, title| {
            shell.delayed_tooltip(crate::ui_primitives::text_tooltip(title))
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_pick(surface_id, window, cx);
        })
        .child(line)
        .when(cursor, |shell| {
            shell.child(
                div()
                    .absolute()
                    .inset_0()
                    .debug_selector(move || format!("agent-cursor-{surface_id}"))
                    .child(sidebar_cursor_ring(ui)),
            )
        })
}

fn agents_resize_key(event: &KeyDownEvent) -> Option<AgentsResizeInput> {
    let keystroke = &event.keystroke;
    let key = keystroke.key.as_str();
    if key == "tab" && keystroke.modifiers == gpui::Modifiers::shift() {
        return Some(AgentsResizeInput::Leave);
    }
    if keystroke.modifiers.modified() {
        return None;
    }
    match key {
        "up" => Some(AgentsResizeInput::Step(AGENTS_RESIZE_STEP)),
        "down" => Some(AgentsResizeInput::Step(-AGENTS_RESIZE_STEP)),
        "home" => Some(AgentsResizeInput::Floor),
        "end" => Some(AgentsResizeInput::Ceiling),
        "enter" => Some(AgentsResizeInput::Fit),
        "escape" => Some(AgentsResizeInput::Leave),
        _ => None,
    }
}

pub(super) struct AgentsResizeFrame<'a> {
    pub(super) focus: &'a FocusHandle,
    pub(super) focused: bool,
    pub(super) dragging: bool,
    pub(super) height: f32,
    pub(super) ceiling: f32,
}

pub(super) fn render_agents_resize_handle(
    frame: AgentsResizeFrame,
    ui: crate::theme::UiColors,
    on_resize: ResizeAgents,
) -> AnyElement {
    let AgentsResizeFrame {
        focus,
        focused,
        dragging,
        height,
        ceiling,
    } = frame;
    let rule = with_alpha(ui.text, 0.16);
    let line = div()
        .absolute()
        .left_0()
        .right_0()
        .top(px((AGENTS_RESIZE_HIT_HEIGHT - 1.) / 2.))
        .h(px(1.))
        .debug_selector(|| "agents-resize-line".into());
    let line = if focused {
        line.bg(with_alpha(ui.text, AGENTS_RESIZE_FOCUS_ALPHA))
    } else {
        line.when(dragging, |line| line.bg(rule))
            .group_hover(AGENTS_RESIZE_GROUP, move |style| style.bg(rule))
    };
    let press = on_resize.clone();
    let press_focus = focus.clone();
    let lift = on_resize.clone();
    let key = on_resize.clone();
    div()
        .id("agents-resize")
        .group(AGENTS_RESIZE_GROUP)
        .absolute()
        .top(px(-((AGENTS_RESIZE_HIT_HEIGHT - 1.) / 2.)))
        .left_0()
        .right_0()
        .h(px(AGENTS_RESIZE_HIT_HEIGHT))
        .cursor(CursorStyle::ResizeUpDown)
        .track_focus(focus)
        .role(Role::Splitter)
        .aria_label("Resize the Agents section")
        .aria_orientation(Orientation::Horizontal)
        .aria_numeric_value(f64::from(height))
        .aria_min_numeric_value(f64::from(AGENTS_RESIZE_FLOOR))
        .aria_max_numeric_value(f64::from(ceiling.max(AGENTS_RESIZE_FLOOR)))
        .debug_selector(|| "agents-resize".into())
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            cx.stop_propagation();
            press_focus.focus(window, cx);
            let input = if event.click_count >= 2 {
                AgentsResizeInput::Fit
            } else {
                AgentsResizeInput::Press(f32::from(event.position.y))
            };
            press(input, window, cx);
        })
        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
            lift(AgentsResizeInput::Release, window, cx);
        })
        .on_key_down(move |event, window, cx| {
            if let Some(input) = agents_resize_key(event) {
                cx.stop_propagation();
                key(input, window, cx);
            }
        })
        .child(line)
        .when(dragging, |handle| {
            handle.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let moved = on_resize.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            let input = if event.pressed_button == Some(MouseButton::Left) {
                                AgentsResizeInput::Drag(f32::from(event.position.y))
                            } else {
                                AgentsResizeInput::Release
                            };
                            moved(input, window, cx);
                        });
                        let released = on_resize.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture && event.button == MouseButton::Left
                            {
                                released(AgentsResizeInput::Release, window, cx);
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
        })
        .into_any_element()
}

pub(super) struct AgentsSectionFrame<'a> {
    pub(super) groups: &'a [AgentGroup<'a>],
    pub(super) cursor: Option<u64>,
    pub(super) collapsed: bool,
    pub(super) chosen_height: Option<f32>,
    pub(super) chevron_turn: f32,
    pub(super) scroll: &'a ScrollHandle,
    pub(super) scrollbar: Option<AnyElement>,
    pub(super) resize_handle: Option<AnyElement>,
    pub(super) display_button: Option<AnyElement>,
}

fn render_agents_list(
    groups: &[AgentGroup<'_>],
    cursor: Option<u64>,
    scroll: &ScrollHandle,
    ui: crate::theme::UiColors,
    on_pick: PickAgent,
) -> gpui::Stateful<gpui::Div> {
    let mut list = div()
        .id("agents-section-list")
        .role(Role::List)
        .aria_label("Agents")
        .flex_1()
        .min_h_0()
        .overflow_x_hidden()
        .overflow_y_scroll()
        .track_scroll(scroll)
        .flex()
        .flex_col()
        .pb(px(AGENTS_LIST_BOTTOM_PADDING));
    if groups.is_empty() {
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
    for (group_index, group) in groups.iter().enumerate() {
        list = list.child(render_agent_group_header(group, group_index == 0, ui));
        for (index, row) in group.rows.iter().enumerate() {
            list = list.child(render_agent_group_member(
                render_agent_row(row, cursor == Some(row.surface_id), ui, on_pick.clone()),
                index,
                group.rows.len(),
            ));
        }
    }
    list
}

pub(super) fn render_agents_section(
    frame: AgentsSectionFrame,
    ui: crate::theme::UiColors,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    on_pick: PickAgent,
) -> gpui::Div {
    let AgentsSectionFrame {
        groups,
        cursor,
        collapsed,
        chosen_height,
        chevron_turn,
        scroll,
        scrollbar,
        resize_handle,
        display_button,
    } = frame;
    let section = div()
        .relative()
        .h(px(agents_section_height(groups, collapsed, chosen_height)))
        .min_h(px(AGENTS_HEADER_HEIGHT))
        .flex_shrink_1()
        .flex()
        .flex_col()
        .debug_selector(|| "agents-section".into())
        .child(render_agents_header(
            collapsed,
            chevron_turn,
            display_button,
            ui,
            on_toggle,
        ));
    if collapsed {
        return section;
    }
    let resize_handle = resize_handle.filter(|_| !groups.is_empty());
    let list = render_agents_list(groups, cursor, scroll, ui, on_pick);
    section
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(list)
                .children(scrollbar),
        )
        .children(resize_handle)
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
            self.agent_scope(),
            self.shown_workspace_idx(),
            AgentRowDetails::from_show(self.cached_config.sidebar_show),
            now,
            cx,
        );
        self.schedule_no_status_wake(grace_deadline, cx);
        let groups = agent_groups(&rows);
        let top_row = groups
            .first()
            .and_then(|group| group.rows.first())
            .map(|row| row.surface_id);
        let (collapsed, chosen_height, dragging, chevron_turn, turning, scroll) = {
            let mut section = self.agents_section.borrow_mut();
            if section.follow_top_row(top_row) {
                section.scroll.set_offset(Point::default());
            }
            (
                section.collapsed,
                section.height,
                section.resize_drag.is_some(),
                section.chevron_turn_at(now),
                section.chevron_turning(now),
                section.scroll.clone(),
            )
        };
        if turning {
            window.request_animation_frame();
        }
        let ui = crate::theme::ui_colors();
        let natural = agents_natural_height(&groups);
        let resize_app = cx.weak_entity();
        let resize_handle = render_agents_resize_handle(
            AgentsResizeFrame {
                focus: &self.agents_resize_focus,
                focused: self.agents_resize_focus.is_focused(window),
                dragging,
                height: agents_section_height(&groups, collapsed, chosen_height),
                ceiling: agents_resize_ceiling(natural, self.agents_available_height()),
            },
            ui,
            Rc::new(move |input, window, cx| {
                let _ = resize_app.update(cx, |this, cx| {
                    this.resize_agents_section(input, natural, window, cx);
                });
            }),
        );
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
        let cursor = self
            .sidebar_agent_cursor()
            .filter(|_| self.sidebar_focus.is_focused(window));
        render_agents_section(
            AgentsSectionFrame {
                groups: &groups,
                cursor,
                collapsed,
                chosen_height,
                chevron_turn,
                scroll: &scroll,
                scrollbar,
                resize_handle: Some(resize_handle),
                display_button: Some(self.render_agents_display_button(ui, cx)),
            },
            ui,
            move |_, cx| {
                let _ = toggle_app.update(cx, |this, cx| {
                    let collapsed = this.agents_section.borrow().collapsed;
                    this.set_agents_section_collapsed(!collapsed, cx);
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

    fn agents_available_height(&self) -> f32 {
        self.agents_section
            .borrow()
            .available_height(&self.sidebar_scroll)
    }

    fn resize_agents_section(
        &mut self,
        input: AgentsResizeInput,
        natural: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if input == AgentsResizeInput::Leave {
            self.sidebar_focus.focus(window, cx);
            cx.notify();
            return;
        }
        let persist =
            self.agents_section
                .borrow_mut()
                .resize_within(input, natural, &self.sidebar_scroll);
        if persist {
            self.persist_agents_section(cx);
        }
        cx.notify();
    }

    pub(super) fn focus_agents_resize_handle(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.agents_nav_surfaces(cx).is_empty() {
            return false;
        }
        self.agents_resize_focus.focus(window, cx);
        cx.notify();
        true
    }

    fn agent_scope(&self) -> AgentScope<'_> {
        AgentScope::from_config(self.active_space.as_ref(), &self.cached_config)
    }

    fn shown_agent_rows(&self, cx: &App) -> Vec<AgentRow> {
        agent_rows(
            &self.workspaces,
            self.agent_scope(),
            self.shown_workspace_idx(),
            AgentRowDetails::ALL,
            Instant::now(),
            cx,
        )
        .rows
    }

    pub(super) fn agents_nav_surfaces(&self, cx: &App) -> Vec<u64> {
        agents_nav_targets(
            self.cached_config.sidebar_show.agents_enabled(),
            self.agents_section.borrow().collapsed,
            &agent_groups(&self.shown_agent_rows(cx)),
        )
    }

    pub(super) fn scroll_agents_to(&self, surface_id: u64, cx: &App) {
        scroll_agents_list_to(
            &self.agents_section.borrow().scroll,
            &agent_groups(&self.shown_agent_rows(cx)),
            surface_id,
        );
    }

    pub(crate) fn set_agents_section_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.agents_section.borrow_mut().set_collapsed(
            collapsed,
            Instant::now(),
            !crate::ui_primitives::reduce_motion(),
        );
        self.persist_agents_section(cx);
        cx.notify();
    }

    fn persist_agents_section(&self, cx: &mut Context<Self>) {
        crate::window_state::record_agents_section(self.agents_section.borrow().persisted());
        cx.background_spawn(async { crate::window_state::save() })
            .detach();
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

    fn close_agents_display_menu(&mut self, cx: &mut Context<Self>) {
        if self.agents_display_menu_open {
            self.agents_display_menu_open = false;
            cx.notify();
        }
    }

    fn render_agents_display_button(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.agents_display_menu_open;
        let hover = crate::app::constants::sidebar_tab_hover_background();
        let show = self.cached_config.sidebar_show;
        let spaces_declared = !self.cached_config.spaces.is_empty();
        let fill = crate::app::constants::sidebar_menu_background(
            self.cached_config.interface_style(),
            ui,
        );
        squircle_skin(
            div()
                .id("agents-display")
                .flex_none()
                .size(px(SIDEBAR_ACTION_BUTTON_SIZE))
                .flex()
                .items_center()
                .justify_center()
                .debug_selector(|| "agents-display".into()),
            AGENTS_DISPLAY_GROUP,
            px(7.),
            open.then_some(hover),
            (!open).then_some(hover),
        )
        .role(Role::Button)
        .aria_label("Customize agent rows")
        .aria_expanded(open)
        .delayed_tooltip(crate::ui_primitives::text_tooltip("Customize agent rows"))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
            this.agents_display_menu_open = !open;
            cx.notify();
        }))
        .child(
            svg()
                .size(px(SIDEBAR_HEADER_ICON_WIDTH))
                .flex_none()
                .path("icons/preference-horizontal.svg")
                .text_color(ui.muted),
        )
        .when(open, |trigger| {
            trigger.child(render_agents_display_menu(
                show,
                spaces_declared,
                fill,
                ui,
                cx,
            ))
        })
        .into_any_element()
    }
}

fn render_agents_display_menu(
    show: paneflow_config::schema::SidebarShow,
    spaces_declared: bool,
    fill: gpui::Hsla,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let menu = menu_surface_filled(div().id("agents-display-menu"), fill, ui)
        .flex()
        .flex_col()
        .gap(MENU_ROW_GAP)
        .p(MENU_PADDING)
        .font_family("Geist")
        .w(px(AGENTS_DISPLAY_MENU_WIDTH))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _, cx| this.close_agents_display_menu(cx)),
        )
        .child(render_show_option(
            "Agent logo",
            AGENT_LOGO_ICON,
            SidebarShowLine::AgentLogo,
            show.agent_logo_enabled(),
            ui,
            cx,
        ))
        .child(render_show_option(
            "Workspace",
            AGENT_WORKSPACE_ICON,
            SidebarShowLine::AgentWorkspace,
            show.agent_workspace_enabled(),
            ui,
            cx,
        ))
        .child(render_show_option(
            "Shimmer",
            SHIMMER_ICON,
            SidebarShowLine::AgentShimmer,
            show.agent_shimmer_enabled(),
            ui,
            cx,
        ))
        .when(spaces_declared, |menu| {
            menu.child(menu_divider(ui)).child(render_show_option(
                "All spaces",
                AGENT_ALL_SPACES_ICON,
                SidebarShowLine::AgentAllSpaces,
                show.agent_all_spaces_enabled(),
                ui,
                cx,
            ))
        });
    gpui::deferred(crate::ui_primitives::menu_reveal(
        "agents-display-menu-reveal",
        div()
            .absolute()
            .bottom(px(SIDEBAR_ACTION_BUTTON_SIZE + AGENTS_DISPLAY_MENU_GAP))
            .right(px(0.))
            .occlude()
            .child(menu),
    ))
    .with_priority(3)
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_customize_agent_rows_line_points_at_an_embedded_icon() {
        for icon in [
            AGENT_LOGO_ICON,
            AGENT_WORKSPACE_ICON,
            SHIMMER_ICON,
            AGENT_ALL_SPACES_ICON,
        ] {
            assert!(
                crate::assets::Assets::get(icon).is_some(),
                "the Customize agent rows menu points at the missing icon {icon}"
            );
        }
    }
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
            space: None,
            active: false,
            status,
            details: AgentRowDetails::ALL,
        }
    }

    fn rank_status(rank: AgentRank) -> AgentRowStatus {
        match lane_state(rank) {
            Some(state) => AgentRowStatus::Lane(state),
            None if rank == AgentRank::NoStatus => AgentRowStatus::NoStatus,
            None => AgentRowStatus::Silent,
        }
    }

    fn ranked_rows(ranks: &[AgentRank]) -> Vec<AgentRow> {
        ranks
            .iter()
            .enumerate()
            .map(|(index, &rank)| row(index as u64 + 1, rank, rank_status(rank)))
            .collect()
    }

    fn group_shape(groups: &[AgentGroup<'_>]) -> Vec<(AgentGroupStatus, Vec<u64>)> {
        groups
            .iter()
            .map(|group| {
                (
                    group.status,
                    group.rows.iter().map(|row| row.surface_id).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn groups_gather_rows_by_shown_status_in_rank_order_and_keep_their_relative_order() {
        let rows = vec![
            row(1, AgentRank::Blocked, rank_status(AgentRank::Blocked)),
            row(2, AgentRank::Blocked, rank_status(AgentRank::Blocked)),
            row(3, AgentRank::Errored, rank_status(AgentRank::Errored)),
            row(4, AgentRank::Done, rank_status(AgentRank::Done)),
            row(5, AgentRank::Working, rank_status(AgentRank::Working)),
            row(6, AgentRank::Working, rank_status(AgentRank::Working)),
            row(7, AgentRank::Idle, AgentRowStatus::Silent),
            row(8, AgentRank::NoStatus, AgentRowStatus::NoStatus),
            row(9, AgentRank::NoStatus, AgentRowStatus::Silent),
            row(10, AgentRank::NoStatus, AgentRowStatus::NoStatus),
        ];
        let groups = agent_groups(&rows);
        assert_eq!(
            group_shape(&groups),
            [
                (AgentGroupStatus::NeedYou, vec![1, 2]),
                (AgentGroupStatus::Error, vec![3]),
                (AgentGroupStatus::Done, vec![4]),
                (AgentGroupStatus::Working, vec![5, 6]),
                (AgentGroupStatus::Idle, vec![7, 9]),
                (AgentGroupStatus::NoStatus, vec![8, 10]),
            ],
            "a session still in its grace delay sits in Idle"
        );
        assert_eq!(
            groups
                .iter()
                .map(|group| group.status.label())
                .collect::<Vec<_>>(),
            ["Need you", "Error", "Done", "Working", "Idle", "No status"]
        );
    }

    #[test]
    fn a_status_no_agent_holds_produces_no_group_and_no_agent_produces_none() {
        let rows = ranked_rows(&[AgentRank::Errored, AgentRank::Idle, AgentRank::Idle]);
        assert_eq!(
            group_shape(&agent_groups(&rows)),
            [
                (AgentGroupStatus::Error, vec![1]),
                (AgentGroupStatus::Idle, vec![2, 3]),
            ]
        );
        assert!(agent_groups(&[]).is_empty());
    }

    #[test]
    fn the_section_takes_its_content_height_plus_the_header_and_a_bottom_margin() {
        let rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Blocked, AgentRank::Done]);
        let groups = agent_groups(&rows);
        assert_eq!(AGENT_GROUP_HEADER_HEIGHT, 24.);
        assert_eq!(AGENT_LINE_HEIGHT, 32.);
        assert_eq!(
            agents_content_height(&groups),
            (24. + 32. + 2. + 32.) + (8. + 24. + 32.)
        );
        assert_eq!(agents_section_height(&groups, false, None), 36. + 154. + 4.);
        assert_eq!(
            agents_section_height(&agent_groups(&rows[..1]), false, None),
            36. + 24. + 32. + 4.
        );
        assert_eq!(agents_section_height(&[], false, None), 36. + 32. + 4.);
    }

    #[test]
    fn six_agents_in_two_groups_fit_under_the_cap_without_scrolling() {
        let rows = ranked_rows(&[
            AgentRank::Working,
            AgentRank::Working,
            AgentRank::Idle,
            AgentRank::Idle,
            AgentRank::Idle,
            AgentRank::Idle,
        ]);
        let groups = agent_groups(&rows);
        assert_eq!(groups.len(), 2);
        let natural = agents_natural_height(&groups);
        assert_eq!(natural, 36. + 2. * 24. + 6. * 32. + 4. * 2. + 8. + 4.);
        assert!(natural < AGENTS_HEIGHT_CAP);
        assert_eq!(agents_section_height(&groups, false, None), natural);
    }

    #[test]
    fn the_row_details_follow_the_sidebar_show_config() {
        let show = paneflow_config::schema::SidebarShow {
            agent_workspace: Some(false),
            ..Default::default()
        };
        assert_eq!(
            AgentRowDetails::from_show(show),
            AgentRowDetails {
                logo: true,
                workspace: false,
                shimmer: true,
            }
        );
        let still = paneflow_config::schema::SidebarShow {
            agent_shimmer: Some(false),
            ..Default::default()
        };
        assert_eq!(
            AgentRowDetails::from_show(still),
            AgentRowDetails {
                shimmer: false,
                ..AgentRowDetails::ALL
            }
        );
        let tree_still = paneflow_config::schema::SidebarShow {
            shimmer: Some(false),
            ..Default::default()
        };
        assert_eq!(
            AgentRowDetails::from_show(tree_still),
            AgentRowDetails::ALL,
            "the tree switch leaves the Agents section alone"
        );
        assert_eq!(
            AgentRowDetails::from_show(Default::default()),
            AgentRowDetails::ALL
        );
    }

    #[test]
    fn a_retired_agent_status_setting_leaves_the_rows_with_their_status() {
        let config: paneflow_config::schema::PaneFlowConfig =
            serde_json::from_str(r#"{"sidebar_show": {"agent_status": false}}"#)
                .expect("a config that still sets the retired key loads");
        assert_eq!(
            AgentRowDetails::from_show(config.sidebar_show),
            AgentRowDetails::ALL
        );
        let ui = crate::theme::ui_colors();
        for rank in [
            AgentRank::Blocked,
            AgentRank::Errored,
            AgentRank::Done,
            AgentRank::Working,
            AgentRank::NoStatus,
        ] {
            let mut shown = row(1, rank, rank_status(rank));
            shown.details = AgentRowDetails::from_show(config.sidebar_show);
            assert!(render_agent_status(&shown, ui).is_some(), "{rank:?}");
        }
        assert!(
            render_agent_status(&row(1, AgentRank::Idle, AgentRowStatus::Silent), ui).is_none(),
            "an idle row carries no status"
        );
    }

    #[test]
    fn the_section_height_is_capped_and_a_collapsed_section_keeps_only_its_header() {
        let rows = ranked_rows(&[AgentRank::Working; 10]);
        let groups = agent_groups(&rows);
        assert_eq!(
            agents_content_height(&groups),
            24. + 10. * 32. + 9. * 2.,
            "one group header, ten rows, nine row gaps"
        );
        assert!(36. + agents_content_height(&groups) + 4. > 330.);
        assert_eq!(agents_section_height(&groups, false, None), 330.);
        assert_eq!(agents_section_height(&groups, true, None), 36.);
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
    fn a_chosen_height_is_bounded_by_one_row_620_px_and_the_content() {
        let ten = ranked_rows(&[AgentRank::Working; 10]);
        let ten = agent_groups(&ten);
        let natural = agents_natural_height(&ten);
        assert_eq!(natural, 36. + 24. + 10. * 32. + 9. * 2. + 4.);
        assert_eq!(agents_section_height(&ten, false, Some(350.)), 350.);
        assert_eq!(
            agents_section_height(&ten, false, Some(10.)),
            36. + 24. + 32. + 4.,
            "the floor is the section header, one group header and one row"
        );
        assert_eq!(AGENTS_RESIZE_FLOOR, 96.);
        assert_eq!(agents_section_height(&ten, false, Some(900.)), natural);
        let eighteen = ranked_rows(&[AgentRank::Working; 18]);
        let eighteen = agent_groups(&eighteen);
        assert!(agents_natural_height(&eighteen) > 620.);
        assert_eq!(agents_section_height(&eighteen, false, Some(900.)), 620.);
        assert_eq!(agents_section_height(&ten, true, Some(450.)), 36.);
        assert_eq!(
            agents_section_height(&[], false, Some(450.)),
            36. + 32. + 4.,
            "an empty section ignores the chosen height"
        );
    }

    #[test]
    fn the_resize_ceiling_leaves_120_px_to_the_tree() {
        assert_eq!(agents_resize_ceiling(608., 900.), 608.);
        assert_eq!(agents_resize_ceiling(700., 1000.), 620.);
        assert_eq!(agents_resize_ceiling(608., 500.), 380.);
    }

    #[test]
    fn a_drag_follows_the_pointer_within_its_bounds_and_persists_only_on_release() {
        let mut section = AgentsSection::default();
        assert!(!section.resize(AgentsResizeInput::Press(500.), 330., 589.));
        assert!(!section.resize(AgentsResizeInput::Drag(400.), 330., 589.));
        assert_eq!(section.height, Some(430.));
        assert!(!section.resize(AgentsResizeInput::Drag(-500.), 330., 589.));
        assert_eq!(section.height, Some(589.));
        assert!(!section.resize(AgentsResizeInput::Drag(2000.), 330., 589.));
        assert_eq!(section.height, Some(96.));
        assert!(section.resize(AgentsResizeInput::Release, 96., 589.));
        assert!(!section.resize(AgentsResizeInput::Release, 96., 589.));
        assert!(!section.resize(AgentsResizeInput::Drag(0.), 96., 589.));
        assert_eq!(section.height, Some(96.), "a move after release is ignored");
    }

    #[test]
    fn keys_step_by_16_px_jump_to_the_bounds_and_enter_fits_the_content() {
        let mut section = AgentsSection::default();
        assert!(section.resize(AgentsResizeInput::Step(16.), 330., 589.));
        assert_eq!(section.height, Some(346.));
        assert!(section.resize(AgentsResizeInput::Step(-16.), 346., 589.));
        assert_eq!(section.height, Some(330.));
        assert!(section.resize(AgentsResizeInput::Floor, 330., 589.));
        assert_eq!(section.height, Some(96.));
        assert!(section.resize(AgentsResizeInput::Step(-16.), 96., 589.));
        assert_eq!(section.height, Some(96.));
        assert!(section.resize(AgentsResizeInput::Ceiling, 96., 589.));
        assert_eq!(section.height, Some(589.));
        assert!(section.resize(AgentsResizeInput::Fit, 589., 589.));
        assert_eq!(section.height, None);
    }

    #[test]
    fn the_key_map_ignores_modified_keys() {
        fn key(text: &'static str) -> Option<AgentsResizeInput> {
            agents_resize_key(&KeyDownEvent {
                keystroke: gpui::Keystroke::parse(text).expect("keystroke"),
                is_held: false,
                prefer_character_input: false,
            })
        }
        assert_eq!(key("up"), Some(AgentsResizeInput::Step(16.)));
        assert_eq!(key("down"), Some(AgentsResizeInput::Step(-16.)));
        assert_eq!(key("home"), Some(AgentsResizeInput::Floor));
        assert_eq!(key("end"), Some(AgentsResizeInput::Ceiling));
        assert_eq!(key("enter"), Some(AgentsResizeInput::Fit));
        assert_eq!(key("escape"), Some(AgentsResizeInput::Leave));
        assert_eq!(key("shift-tab"), Some(AgentsResizeInput::Leave));
        assert_eq!(key("tab"), None);
        assert_eq!(key("shift-up"), None);
        assert_eq!(key("a"), None);
    }

    #[test]
    fn a_restored_section_reports_the_state_it_was_given() {
        let state = AgentsSectionState {
            height: Some(240.),
            collapsed: true,
        };
        let section = AgentsSection::restored(state);
        assert!(section.collapsed());
        assert_eq!(section.persisted(), state);
        assert_eq!(
            AgentsSection::default().persisted(),
            AgentsSectionState::default()
        );
    }

    struct ResizeHarness {
        rows: Vec<AgentRow>,
        section: AgentsSection,
        tree: ScrollHandle,
        focus: FocusHandle,
        height: f32,
        persists: usize,
        leaves: usize,
    }

    impl gpui::Render for ResizeHarness {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let groups = agent_groups(&self.rows);
            let natural = agents_natural_height(&groups);
            let dragging = self.section.resize_drag.is_some();
            let chosen = self.section.height;
            let this = cx.weak_entity();
            let handle = render_agents_resize_handle(
                AgentsResizeFrame {
                    focus: &self.focus,
                    focused: self.focus.is_focused(window),
                    dragging,
                    height: agents_section_height(&groups, self.section.collapsed, chosen),
                    ceiling: agents_resize_ceiling(
                        natural,
                        self.section.available_height(&self.tree),
                    ),
                },
                crate::theme::ui_colors(),
                Rc::new(move |input, _, cx| {
                    let _ = this.update(cx, |harness, cx| {
                        if input == AgentsResizeInput::Leave {
                            harness.leaves += 1;
                        }
                        if harness.section.resize_within(input, natural, &harness.tree) {
                            harness.persists += 1;
                        }
                        cx.notify();
                    });
                }),
            );
            div()
                .w(px(SIDEBAR_WIDTH))
                .h(px(self.height))
                .flex()
                .flex_col()
                .child(div().h(px(AGENTS_HEADER_HEIGHT)).flex_none())
                .child(
                    div()
                        .id("tree")
                        .flex_1()
                        .min_h(px(AGENTS_TREE_MIN_HEIGHT))
                        .overflow_y_scroll()
                        .track_scroll(&self.tree)
                        .debug_selector(|| "tree".into()),
                )
                .child(render_agents_section(
                    AgentsSectionFrame {
                        groups: &groups,
                        cursor: None,
                        collapsed: self.section.collapsed,
                        chosen_height: chosen,
                        chevron_turn: if self.section.collapsed { 1. } else { 0. },
                        scroll: &self.section.scroll,
                        scrollbar: None,
                        resize_handle: Some(handle),
                        display_button: None,
                    },
                    crate::theme::ui_colors(),
                    |_, _| {},
                    Rc::new(|_, _, _| {}),
                ))
                .child(div().h(px(55.)).flex_none())
        }
    }

    fn resize_harness(
        cx: &mut TestAppContext,
        rows: Vec<AgentRow>,
        section: AgentsSection,
        height: f32,
    ) -> (gpui::Entity<ResizeHarness>, &mut gpui::VisualTestContext) {
        let (view, cx) = cx.add_window_view(move |_, cx| ResizeHarness {
            rows,
            section,
            tree: ScrollHandle::new(),
            focus: cx.focus_handle(),
            height,
            persists: 0,
            leaves: 0,
        });
        redraw(cx);
        redraw(cx);
        (view, cx)
    }

    fn redraw(cx: &mut gpui::VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn section_height(cx: &mut gpui::VisualTestContext) -> gpui::Pixels {
        cx.debug_bounds("agents-section")
            .expect("section")
            .size
            .height
    }

    #[gpui::test]
    fn the_handle_is_a_9_px_zone_on_the_section_top_edge_with_a_1_px_rule(cx: &mut TestAppContext) {
        let (_, cx) = resize_harness(
            cx,
            ranked_rows(&[AgentRank::Working; 3]),
            AgentsSection::default(),
            800.,
        );
        let section = cx.debug_bounds("agents-section").expect("section");
        let handle = cx.debug_bounds("agents-resize").expect("handle");
        let rule = cx.debug_bounds("agents-resize-line").expect("rule");
        assert_eq!(handle.size.height, px(9.));
        assert_eq!(handle.top(), section.top() - px(4.));
        assert_eq!(handle.size.width, section.size.width);
        assert_eq!(rule.size.height, px(1.));
        assert_eq!(rule.top(), section.top());
    }

    #[gpui::test]
    fn a_collapsed_section_has_no_handle(cx: &mut TestAppContext) {
        let collapsed = AgentsSection::restored(AgentsSectionState {
            height: Some(300.),
            collapsed: true,
        });
        let (_, cx) = resize_harness(cx, ranked_rows(&[AgentRank::Working; 3]), collapsed, 800.);
        assert!(cx.debug_bounds("agents-resize").is_none());
        assert_eq!(section_height(cx), px(36.));
    }

    #[gpui::test]
    fn an_empty_section_has_no_handle(cx: &mut TestAppContext) {
        let (_, cx) = resize_harness(cx, Vec::new(), AgentsSection::default(), 800.);
        assert!(cx.debug_bounds("agents-resize").is_none());
        assert!(cx.debug_bounds("agents-empty").is_some());
    }

    #[gpui::test]
    fn dragging_the_handle_resizes_within_bounds_and_double_click_fits_the_content(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = resize_harness(
            cx,
            ranked_rows(&[AgentRank::Working; 18]),
            AgentsSection::default(),
            800.,
        );
        assert_eq!(section_height(cx), px(330.));
        let grip = cx.debug_bounds("agents-resize").expect("handle").center();

        cx.simulate_mouse_down(grip, MouseButton::Left, Modifiers::default());
        redraw(cx);
        cx.simulate_mouse_move(
            grip - Point::new(px(0.), px(100.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        redraw(cx);
        assert_eq!(section_height(cx), px(430.));
        assert_eq!(view.read_with(cx, |harness, _| harness.persists), 0);

        cx.simulate_mouse_move(
            Point::new(grip.x + px(400.), px(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        redraw(cx);
        let available = 800. - 36. - 55.;
        assert_eq!(
            section_height(cx),
            px(available - 120.),
            "the tree keeps 120 px even when the pointer leaves the sidebar"
        );
        cx.simulate_mouse_up(
            Point::new(grip.x + px(400.), px(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        redraw(cx);
        assert_eq!(view.read_with(cx, |harness, _| harness.persists), 1);
        assert_eq!(
            view.read_with(cx, |harness, _| harness.section.height),
            Some(available - 120.)
        );

        let grip = cx.debug_bounds("agents-resize").expect("handle").center();
        cx.simulate_mouse_down(grip, MouseButton::Left, Modifiers::default());
        redraw(cx);
        cx.simulate_mouse_move(
            grip + Point::new(px(0.), px(2000.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        redraw(cx);
        assert_eq!(section_height(cx), px(96.));
        cx.simulate_mouse_up(grip, MouseButton::Left, Modifiers::default());
        redraw(cx);

        let grip = cx.debug_bounds("agents-resize").expect("handle").center();
        cx.simulate_event(MouseDownEvent {
            position: grip,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        redraw(cx);
        cx.simulate_mouse_up(grip, MouseButton::Left, Modifiers::default());
        redraw(cx);
        assert_eq!(
            view.read_with(cx, |harness, _| harness.section.height),
            None
        );
        assert_eq!(section_height(cx), px(330.));
    }

    #[gpui::test]
    fn the_focused_handle_answers_arrows_home_end_and_enter(cx: &mut TestAppContext) {
        let (view, cx) = resize_harness(
            cx,
            ranked_rows(&[AgentRank::Working; 18]),
            AgentsSection::default(),
            800.,
        );
        let grip = cx.debug_bounds("agents-resize").expect("handle").center();
        cx.simulate_click(grip, Modifiers::default());
        redraw(cx);
        assert!(cx.update(|window, cx| view.read(cx).focus.is_focused(window)));
        let persists_after_click = view.read_with(cx, |harness, _| harness.persists);

        fn press(cx: &mut gpui::VisualTestContext, key: &str) -> gpui::Pixels {
            cx.simulate_keystrokes(key);
            redraw(cx);
            section_height(cx)
        }
        assert_eq!(press(cx, "up"), px(346.));
        assert_eq!(press(cx, "down"), px(330.));
        assert_eq!(press(cx, "home"), px(96.));
        assert_eq!(press(cx, "end"), px(800. - 36. - 55. - 120.));
        assert_eq!(press(cx, "enter"), px(330.));
        assert_eq!(
            view.read_with(cx, |harness, _| harness.persists),
            persists_after_click + 5
        );
        press(cx, "escape");
        assert_eq!(view.read_with(cx, |harness, _| harness.leaves), 1);
        assert_eq!(
            view.read_with(cx, |harness, _| harness.persists),
            persists_after_click + 5
        );
    }

    #[gpui::test]
    fn a_persisted_height_taller_than_the_window_is_bounded_but_kept(cx: &mut TestAppContext) {
        let chosen = AgentsSection::restored(AgentsSectionState {
            height: Some(500.),
            collapsed: false,
        });
        let short = 36. + 120. + 55. + 200.;
        let (view, cx) = resize_harness(cx, ranked_rows(&[AgentRank::Working; 18]), chosen, short);
        assert_eq!(section_height(cx), px(200.));
        assert_eq!(cx.debug_bounds("tree").expect("tree").size.height, px(120.));
        assert_eq!(
            view.read_with(cx, |harness, _| harness.section.height),
            Some(500.)
        );
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

        let shown = cx.update(|_, cx| {
            agent_rows(
                &workspaces,
                AgentScope::active_space_only(None),
                Some(0),
                AgentRowDetails::ALL,
                Instant::now(),
                cx,
            )
        });

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
            agent_groups(&shown.rows)
                .iter()
                .map(|group| (group.status, group.rows.len()))
                .collect::<Vec<_>>(),
            [
                (AgentGroupStatus::NeedYou, 1),
                (AgentGroupStatus::Working, 1),
                (AgentGroupStatus::NoStatus, 1),
            ]
        );
        assert_eq!(
            shown.rows.iter().map(|row| row.active).collect::<Vec<_>>(),
            [false, true, false],
            "only the active tab of the shown workspace is active"
        );
        assert_eq!(shown.grace_deadline, None);
    }

    #[gpui::test]
    fn every_space_scope_adds_the_other_spaces_after_the_active_one_and_names_them(
        cx: &mut TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let client_id = "550e8400-e29b-41d4-a716-446655440001";
        let client = paneflow_config::schema::SpaceId::parse(client_id).expect("space id");
        let declared = |all_spaces: bool| -> paneflow_config::schema::PaneFlowConfig {
            serde_json::from_value(serde_json::json!({
                "spaces": [{"id": client_id, "name": "Client"}],
                "sidebar_show": {"agent_all_spaces": all_spaces},
            }))
            .expect("loads")
        };
        let (mut here, here_surfaces) = workspace_with_agents(cx, 1, "paneflow", &["Sidebar"]);
        let (mut blocked, blocked_surfaces) = workspace_with_agents(cx, 2, "site", &["Docs"]);
        let (mut working, working_surfaces) = workspace_with_agents(cx, 3, "api", &["Routes"]);
        blocked.space = Some(client.clone());
        working.space = Some(client.clone());
        for (workspace, surfaces, state) in [
            (&mut here, &here_surfaces, AgentState::Thinking),
            (&mut blocked, &blocked_surfaces, AgentState::WaitingForInput),
            (&mut working, &working_surfaces, AgentState::Thinking),
        ] {
            workspace.agent_sessions.insert(
                1,
                agent(
                    TerminalAgent::ClaudeCode,
                    state,
                    true,
                    surfaces[0],
                    Duration::from_secs(10),
                ),
            );
        }
        let workspaces = vec![here, blocked, working];
        let (here, blocked, working) = (here_surfaces[0], blocked_surfaces[0], working_surfaces[0]);
        let shown = |scope: AgentScope<'_>, cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| {
                agent_rows(
                    &workspaces,
                    scope,
                    Some(0),
                    AgentRowDetails::ALL,
                    Instant::now(),
                    cx,
                )
                .rows
            })
        };

        let active_only = declared(false);
        let alone = shown(AgentScope::from_config(None, &active_only), cx);
        assert_eq!(
            alone
                .iter()
                .map(|row| (row.surface_id, row.space.clone()))
                .collect::<Vec<_>>(),
            [(here, None)],
            "the default scope keeps to the active space"
        );

        let everywhere = declared(true);
        let all = shown(AgentScope::from_config(None, &everywhere), cx);
        assert_eq!(
            all.iter()
                .map(|row| (row.surface_id, row.space.as_deref()))
                .collect::<Vec<_>>(),
            [
                (here, None),
                (blocked, Some("Client")),
                (working, Some("Client"))
            ],
            "the active space leads, and only the other spaces are named"
        );
        assert_eq!(
            group_shape(&agent_groups(&all)),
            [
                (AgentGroupStatus::NeedYou, vec![blocked]),
                (AgentGroupStatus::Working, vec![here, working]),
            ],
            "a status group lists the active space's agents first"
        );
        assert_eq!(
            all.iter().map(|row| row.active).collect::<Vec<_>>(),
            [true, false, false]
        );
        assert_eq!(
            agents_nav_targets(true, false, &agent_groups(&all)),
            [blocked, here, working],
            "the keyboard cursor reaches the other spaces' rows"
        );

        let from_client = shown(AgentScope::from_config(Some(&client), &everywhere), cx);
        assert_eq!(
            from_client
                .iter()
                .map(|row| (row.surface_id, row.space.as_deref()))
                .collect::<Vec<_>>(),
            [
                (blocked, None),
                (working, None),
                (here, Some(paneflow_config::schema::DEFAULT_SPACE_NAME))
            ],
            "seen from another space, the default space is the one named"
        );
    }

    #[test]
    fn a_row_from_another_space_names_it_after_its_workspace() {
        let mut elsewhere = row(1, AgentRank::Idle, AgentRowStatus::Silent);
        elsewhere.space = Some("Client".to_string());
        assert_eq!(
            agent_row_full_name(&elsewhere),
            "Attention sidebar \u{b7} paneflow \u{b7} Client"
        );
        assert_eq!(
            agent_row_label(&elsewhere),
            "Claude Code, Attention sidebar, paneflow, Client space, idle"
        );
        elsewhere.details.workspace = false;
        assert_eq!(agent_row_shown_name(&elsewhere), "Attention sidebar");
        assert_eq!(
            agent_row_label(&row(1, AgentRank::Idle, AgentRowStatus::Silent)),
            "Claude Code, Attention sidebar, paneflow, idle"
        );
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

        let young = cx.update(|_, cx| {
            agent_rows(
                &workspaces,
                AgentScope::active_space_only(None),
                None,
                AgentRowDetails::ALL,
                started,
                cx,
            )
        });
        assert_eq!(young.rows[0].rank, AgentRank::NoStatus);
        assert_eq!(young.rows[0].status, AgentRowStatus::Silent);
        assert_eq!(young.grace_deadline, Some(started + NO_STATUS_GRACE));
        assert_eq!(
            group_shape(&agent_groups(&young.rows)),
            [(AgentGroupStatus::Idle, vec![surfaces[0]])],
            "within its grace delay the session sits in Idle"
        );

        let due = started + NO_STATUS_GRACE;
        let grown = cx.update(|_, cx| {
            agent_rows(
                &workspaces,
                AgentScope::active_space_only(None),
                None,
                AgentRowDetails::ALL,
                due,
                cx,
            )
        });
        assert_eq!(grown.rows[0].status, AgentRowStatus::NoStatus);
        assert_eq!(grown.grace_deadline, None);
        assert_eq!(
            group_shape(&agent_groups(&grown.rows)),
            [(AgentGroupStatus::NoStatus, vec![surfaces[0]])],
            "the wake at the grace deadline moves it to No status"
        );

        let session = workspaces[0].agent_sessions.get_mut(&1).expect("session");
        session.program_status_capable = true;
        session.state = AgentState::Thinking;
        let capable = cx.update(|_, cx| {
            agent_rows(
                &workspaces,
                AgentScope::active_space_only(None),
                None,
                AgentRowDetails::ALL,
                due,
                cx,
            )
        });
        assert_eq!(capable.rows[0].rank, AgentRank::Working);
        assert_eq!(
            capable.rows[0].status,
            AgentRowStatus::Lane(SidebarAgentState::Thinking)
        );
    }

    #[gpui::test]
    fn the_tree_and_the_section_read_the_same_state_for_every_session(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let states = [
            AgentState::Thinking,
            AgentState::WaitingForInput,
            AgentState::Finished,
            AgentState::Errored,
        ];
        let mut shown_by_both = 0;
        for state in states {
            for unread in [false, true] {
                for capable in [false, true] {
                    let (mut workspace, surfaces) =
                        workspace_with_agents(cx, 1, "paneflow", &["Agent"]);
                    workspace.agent_sessions.insert(
                        1,
                        agent(
                            TerminalAgent::ClaudeCode,
                            state,
                            capable,
                            surfaces[0],
                            Duration::from_secs(10),
                        ),
                    );
                    if unread {
                        workspace
                            .agent_completion_notification
                            .record_finished(false, Some(surfaces[0]));
                    }
                    let workspaces = vec![workspace];
                    let tree_status_hidden = paneflow_config::schema::SidebarShow {
                        status: Some(false),
                        ..Default::default()
                    };
                    let (section, tab, folder, hidden_section, hidden_tab, hidden_folder) = cx
                        .update(|_, cx| {
                            let now = Instant::now();
                            let shown = agent_rows(
                                &workspaces,
                                AgentScope::active_space_only(None),
                                Some(0),
                                AgentRowDetails::ALL,
                                now,
                                cx,
                            );
                            let hidden = agent_rows(
                                &workspaces,
                                AgentScope::active_space_only(None),
                                Some(0),
                                AgentRowDetails::from_show(tree_status_hidden),
                                now,
                                cx,
                            );
                            let workspace = &workspaces[0];
                            let tab_surfaces = workspace.tabs()[0].surface_ids(cx);
                            (
                                shown.rows,
                                tab_row_agent_summary(TreeStatus::Shown, workspace, &tab_surfaces),
                                folder_row_agent_summary(TreeStatus::Shown, workspace, false),
                                hidden.rows,
                                tab_row_agent_summary(
                                    TreeStatus::resolve(tree_status_hidden, false),
                                    workspace,
                                    &tab_surfaces,
                                ),
                                folder_row_agent_summary(
                                    TreeStatus::resolve(tree_status_hidden, false),
                                    workspace,
                                    false,
                                ),
                            )
                        });
                    let case = format!("{state:?} unread={unread} capable={capable}");
                    assert_eq!(section.len(), 1, "{case}");
                    assert_eq!(
                        (hidden_tab, hidden_folder),
                        (None, None),
                        "a hidden tree status empties the tree rows, {case}"
                    );
                    assert_eq!(
                        hidden_section, section,
                        "the section keeps its statuses when the tree hides its own, {case}"
                    );
                    let section_state = match section[0].status {
                        AgentRowStatus::Lane(shown) => Some(shown),
                        AgentRowStatus::NoStatus | AgentRowStatus::Silent => None,
                    };
                    let tab_state = tab.map(|summary| summary.state);
                    let folder_state = folder.map(|summary| summary.state);
                    assert_eq!(tab_state, section_state, "tab row, {case}");
                    assert_eq!(
                        folder_state, section_state,
                        "collapsed workspace row, {case}"
                    );
                    if state == AgentState::Thinking {
                        assert_eq!(
                            section_state,
                            Some(SidebarAgentState::Thinking),
                            "an agent back at work reads as working everywhere, {case}"
                        );
                    }
                    shown_by_both += usize::from(section_state.is_some());
                }
            }
        }
        assert_eq!(
            shown_by_both, 14,
            "only finished and seen sessions show no state"
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
        cursor: Option<u64>,
        scroll: ScrollHandle,
        collapsed: bool,
        height: f32,
        picks: Rc<RefCell<Vec<u64>>>,
        toggles: Rc<Cell<usize>>,
    }

    impl gpui::Render for SectionHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let picks = self.picks.clone();
            let toggles = self.toggles.clone();
            let groups = agent_groups(&self.rows);
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
                        groups: &groups,
                        cursor: self.cursor,
                        collapsed: self.collapsed,
                        chosen_height: None,
                        chevron_turn: if self.collapsed { 1. } else { 0. },
                        scroll: &self.scroll,
                        scrollbar: None,
                        resize_handle: None,
                        display_button: None,
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
            cursor: None,
            scroll: ScrollHandle::new(),
            collapsed,
            height,
            picks,
            toggles,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (cx, held_picks, held_toggles)
    }

    const GROUP_KEYS: [&str; 6] = ["need-you", "error", "done", "working", "idle", "no-status"];
    const GROUP_SELECTORS: [&str; 6] = [
        "agent-group-need-you",
        "agent-group-error",
        "agent-group-done",
        "agent-group-working",
        "agent-group-idle",
        "agent-group-no-status",
    ];

    fn close(left: gpui::Pixels, right: gpui::Pixels) -> bool {
        (left - right).abs() < px(0.01)
    }

    #[gpui::test]
    fn the_rendered_section_sits_under_the_tree_with_its_header_group_headers_and_rows(
        cx: &mut TestAppContext,
    ) {
        let rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Errored, AgentRank::Errored]);
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
            px(36. + (24. + 32.) + (8. + 24. + 32. + 2. + 32.) + 4.)
        );
        let header = cx.debug_bounds("agents-header").expect("header");
        assert_eq!(header.size.height, px(36.));
        let toggle = cx.debug_bounds("agents-toggle").expect("toggle");
        assert_eq!(toggle.size.height, px(20.));
        assert_eq!(toggle.left(), section.left() + px(8.));
        assert_eq!(toggle.center().y, header.center().y);

        let need_you = cx.debug_bounds("agent-group-need-you").expect("Need you");
        let error = cx.debug_bounds("agent-group-error").expect("Error");
        let first = cx.debug_bounds("agent-row-1").expect("row 1");
        let second = cx.debug_bounds("agent-row-2").expect("row 2");
        let third = cx.debug_bounds("agent-row-3").expect("row 3");
        assert_eq!(
            need_you.top(),
            header.bottom(),
            "no gap above the first group"
        );
        assert_eq!(need_you.size.height, px(24.));
        assert_eq!(first.top(), need_you.bottom());
        assert_eq!(first.size.height, px(32.));
        assert_eq!(first.left(), section.left() + px(8.));
        assert_eq!(
            error.top() - first.bottom(),
            px(8.),
            "8 px before every group header but the first"
        );
        assert_eq!(error.size.height, px(24.));
        assert_eq!(second.top(), error.bottom());
        assert_eq!(third.top() - second.bottom(), px(2.));
        assert_eq!(third.size.height, px(32.));
        assert!(cx.debug_bounds("agents-empty").is_none());
        for absent in &GROUP_SELECTORS[2..] {
            assert!(
                cx.debug_bounds(absent).is_none(),
                "{absent} has no agent and no header"
            );
        }
    }

    #[gpui::test]
    fn a_group_header_shows_its_label_at_the_row_content_edge_then_its_count_six_px_after(
        cx: &mut TestAppContext,
    ) {
        let mut rows = ranked_rows(&[AgentRank::Working, AgentRank::Working, AgentRank::Idle]);
        rows[2].details.logo = false;
        let (cx, _, _) = harness(cx, rows, false, 800.);
        let section = cx.debug_bounds("agents-section").expect("section");
        let logo_slot_edge =
            cx.debug_bounds("agent-row-1").expect("row 1").left() + px(SIDEBAR_ROW_PADDING_X);
        let bare_title = cx.debug_bounds("agent-title-3").expect("title 3");
        assert_eq!(AGENT_GROUP_HEADER_INSET, 15.);
        assert_eq!(logo_slot_edge, section.left() + px(15.));
        assert_eq!(bare_title.left(), section.left() + px(15.));
        let shown = [
            (
                "agent-group-working",
                "agent-group-label-working",
                "agent-group-count-working",
            ),
            (
                "agent-group-idle",
                "agent-group-label-idle",
                "agent-group-count-idle",
            ),
        ];
        for (key, label, count) in shown {
            let header = cx.debug_bounds(key).expect("group header");
            let label = cx.debug_bounds(label).expect("group label");
            let count = cx.debug_bounds(count).expect("group count");
            assert_eq!(header.left(), section.left());
            assert_eq!(header.size.width, section.size.width);
            assert_eq!(label.left(), logo_slot_edge, "{key}");
            assert!(
                close(count.left() - label.right(), px(6.)),
                "{key}: {label:?} {count:?}"
            );
            assert_eq!(label.center().y, header.center().y, "{key}");
            assert_eq!(count.center().y, header.center().y, "{key}");
        }
    }

    #[test]
    fn a_group_header_takes_its_status_color_only_when_the_group_awaits_the_user() {
        let ui = crate::theme::ui_colors();
        assert_eq!(
            AgentGroupStatus::NeedYou.color(ui),
            crate::app::constants::sidebar_needs_input_color()
        );
        assert_eq!(AgentGroupStatus::Error.color(ui), ui.agent_error);
        assert_eq!(
            AgentGroupStatus::Done.color(ui),
            crate::app::constants::sidebar_finished_color()
        );
        for quiet in [
            AgentGroupStatus::Working,
            AgentGroupStatus::Idle,
            AgentGroupStatus::NoStatus,
        ] {
            assert_eq!(quiet.color(ui), ui.muted, "{quiet:?}");
        }
        assert_eq!(
            AgentGroupStatus::ORDER.map(AgentGroupStatus::key),
            GROUP_KEYS
        );
        assert_eq!(
            GROUP_KEYS.map(|key| format!("agent-group-{key}")),
            GROUP_SELECTORS
        );
    }

    #[gpui::test]
    fn a_row_is_one_32_px_line_with_logo_title_workspace_suffix_then_status(
        cx: &mut TestAppContext,
    ) {
        let rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Idle]);
        let (cx, _, _) = harness(cx, rows, false, 800.);
        let row = cx.debug_bounds("agent-row-1").expect("row 1");
        let logo = cx.debug_bounds("agent-logo-1").expect("logo");
        let title = cx.debug_bounds("agent-title-1").expect("title");
        let suffix = cx.debug_bounds("agent-workspace-1").expect("workspace");
        let badge = cx.debug_bounds("status-badge-agent-1").expect("badge");
        assert_eq!(row.size.height, px(32.));
        assert_eq!(
            title.left() - row.left(),
            px(SIDEBAR_ROW_PADDING_X + SIDEBAR_FOLDER_SLOT_WIDTH + SIDEBAR_TITLE_ROW_GAP)
        );
        let status = cx.debug_bounds("agent-status-1").expect("status");
        assert!(logo.right() < title.left());
        assert!(
            close(suffix.left(), title.right()),
            "the suffix follows the title: {title:?} {suffix:?}"
        );
        assert_eq!(title.center().y, row.center().y);
        assert_eq!(suffix.center().y, row.center().y);
        assert!(close(
            suffix.right(),
            status.left() - px(SIDEBAR_TITLE_ROW_GAP)
        ));
        assert!(status.contains(&badge.center()));
        assert!(
            close(status.right(), row.right() - px(SIDEBAR_ROW_PADDING_X)),
            "the status closes the line"
        );

        let idle = cx.debug_bounds("agent-row-2").expect("row 2");
        assert_eq!(idle.size.height, px(32.));
        assert!(
            cx.debug_bounds("agent-status-2").is_none(),
            "an idle row carries no status"
        );
    }

    #[gpui::test]
    fn a_long_name_ellipses_on_one_line_and_cuts_the_workspace_suffix_first(
        cx: &mut TestAppContext,
    ) {
        let mut rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Blocked]);
        rows[0].workspace = "a-workspace-name-far-longer-than-the-sidebar-row-can-show".to_string();
        rows[1].title =
            "A tab title far longer than the sidebar row can show on one line".to_string();
        let (cx, _, _) = harness(cx, rows, false, 800.);

        let row = cx.debug_bounds("agent-row-1").expect("row 1");
        let title = cx.debug_bounds("agent-title-1").expect("title");
        let suffix = cx.debug_bounds("agent-workspace-1").expect("workspace");
        let badge = cx.debug_bounds("status-badge-agent-1").expect("badge");
        assert_eq!(row.size.height, px(32.), "the text does not wrap");
        assert!(
            close(suffix.left(), title.right()),
            "a short title keeps its width"
        );
        assert!(
            close(suffix.right(), badge.left() - px(SIDEBAR_TITLE_ROW_GAP)),
            "the suffix is cut where the status begins"
        );
        assert!(suffix.size.width > px(0.));
        assert!(title.size.width > px(100.), "{title:?}");

        let row = cx.debug_bounds("agent-row-2").expect("row 2");
        let title = cx.debug_bounds("agent-title-2").expect("title");
        let badge = cx.debug_bounds("status-badge-agent-2").expect("badge");
        assert_eq!(row.size.height, px(32.), "the text does not wrap");
        assert!(
            close(title.right(), badge.left() - px(SIDEBAR_TITLE_ROW_GAP)),
            "the title is cut only once the suffix has no room left: {title:?} {badge:?}"
        );
        assert!(
            cx.debug_bounds("agent-workspace-2")
                .is_none_or(|suffix| suffix.size.width == px(0.)),
            "the suffix gave up all of its width first"
        );
    }

    #[test]
    fn a_name_longer_than_the_line_can_hold_gets_the_title_and_workspace_tooltip() {
        let mut long = row(1, AgentRank::Idle, AgentRowStatus::Silent);
        assert_eq!(
            agent_row_full_name(&long),
            "Attention sidebar \u{b7} paneflow"
        );
        assert_eq!(agent_row_shown_name(&long), agent_row_full_name(&long));
        long.details.workspace = false;
        assert_eq!(agent_row_shown_name(&long), "Attention sidebar");

        let mut short = row(2, AgentRank::Idle, AgentRowStatus::Silent);
        short.title = "Docs".to_string();
        short.workspace = "web".to_string();
        assert!(agent_row_shown_name(&short).chars().count() <= SIDEBAR_TITLE_TOOLTIP_MIN_CHARS);
        assert!(agent_row_shown_name(&long).chars().count() > SIDEBAR_TITLE_TOOLTIP_MIN_CHARS);
    }

    #[gpui::test]
    fn hiding_the_logo_or_the_workspace_suffix_keeps_each_row_at_32_px(cx: &mut TestAppContext) {
        let mut rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Done, AgentRank::Working]);
        rows[0].details.logo = false;
        rows[1].details.workspace = false;
        let groups = agent_groups(&rows);
        assert_eq!(
            agents_content_height(&groups),
            3. * (24. + 32.) + 2. * 8.,
            "the row details never change the height"
        );
        let (cx, _, _) = harness(cx, rows, false, 800.);

        let bare = cx.debug_bounds("agent-row-1").expect("row 1");
        let title = cx.debug_bounds("agent-title-1").expect("title");
        assert_eq!(bare.size.height, px(32.));
        assert!(cx.debug_bounds("agent-logo-1").is_none());
        assert!(cx.debug_bounds("agent-workspace-1").is_some());
        assert_eq!(title.left() - bare.left(), px(SIDEBAR_ROW_PADDING_X));

        let unnamed = cx.debug_bounds("agent-row-2").expect("row 2");
        assert_eq!(unnamed.size.height, px(32.));
        assert!(cx.debug_bounds("agent-logo-2").is_some());
        assert!(cx.debug_bounds("agent-workspace-2").is_none());
        assert!(cx.debug_bounds("status-badge-agent-2").is_some());

        assert!(cx.debug_bounds("agent-logo-3").is_some());
        assert!(cx.debug_bounds("agent-workspace-3").is_some());
    }

    #[test]
    fn only_a_working_row_shimmers_and_it_holds_still_under_reduce_motion() {
        let working = AgentRowStatus::Lane(SidebarAgentState::Thinking);
        let all = AgentRowDetails::ALL;
        let switched_off = AgentRowDetails {
            shimmer: false,
            ..all
        };
        assert!(agent_title_shimmers(working, all, false));
        assert!(!agent_title_shimmers(working, all, true));
        assert!(!agent_title_shimmers(working, switched_off, false));
        for still in [
            AgentRowStatus::Lane(SidebarAgentState::NeedsInput),
            AgentRowStatus::Lane(SidebarAgentState::Errored),
            AgentRowStatus::Lane(SidebarAgentState::Finished),
            AgentRowStatus::Silent,
            AgentRowStatus::NoStatus,
        ] {
            assert!(!agent_title_shimmers(still, all, false), "{still:?}");
        }
    }

    #[gpui::test]
    fn a_working_row_shows_the_matrix_in_place_of_a_badge(cx: &mut TestAppContext) {
        let (cx, _, _) = harness(cx, ranked_rows(&[AgentRank::Working]), false, 800.);
        let row = cx.debug_bounds("agent-row-1").expect("row 1");
        let status = cx.debug_bounds("agent-status-1").expect("status");
        assert_eq!(row.size.height, px(32.));
        assert!(cx.debug_bounds("status-badge-agent-1").is_none());
        assert_eq!(status.size.width, px(SIDEBAR_LANE_GLYPH_SIZE));
        assert!(close(
            status.right(),
            row.right() - px(SIDEBAR_ROW_PADDING_X)
        ));
    }

    #[gpui::test]
    fn no_field_declared_over_osc_7501_reaches_a_header_or_a_row(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let (mut workspace, surfaces) = workspace_with_agents(cx, 1, "paneflow", &["Docs nav"]);
        let mut session = agent(
            TerminalAgent::ClaudeCode,
            AgentState::WaitingForInput,
            true,
            surfaces[0],
            Duration::from_secs(10),
        );
        session.message = Some("declared-message".to_string());
        session.active_tool_name = Some("declared-app".to_string());
        session.last_result = Some("declared-result".to_string());
        session.pending_tab_title = Some("declared-title".to_string());
        workspace.agent_sessions.insert(1, session);
        let workspaces = vec![workspace];
        let shown = cx.update(|_, cx| {
            agent_rows(
                &workspaces,
                AgentScope::active_space_only(None),
                Some(0),
                AgentRowDetails::ALL,
                Instant::now(),
                cx,
            )
        });
        let groups = agent_groups(&shown.rows);
        let texts = [
            shown.rows[0].title.clone(),
            shown.rows[0].workspace.clone(),
            agent_row_full_name(&shown.rows[0]),
            agent_row_label(&shown.rows[0]),
            agent_group_label(groups[0].status, groups[0].rows.len()),
            groups[0].status.label().to_string(),
        ];
        assert_eq!(texts[0], "Docs nav");
        assert_eq!(texts[1], "paneflow");
        for text in &texts {
            assert!(!text.contains("declared"), "{text}");
        }
    }

    #[gpui::test]
    fn the_agent_logo_sits_on_the_whole_pixel_below_the_title_line_center(cx: &mut TestAppContext) {
        let (cx, _, _) = harness(cx, ranked_rows(&[AgentRank::Blocked]), false, 800.);
        let row = cx.debug_bounds("agent-row-1").expect("row 1");
        let logo = cx.debug_bounds("agent-logo-1").expect("logo");
        assert_eq!(logo.size.height, px(AGENT_LOGO_SIZE));
        assert_eq!(logo.top() - row.top(), px(SIDEBAR_ROW_PADDING_Y + 3.));
        assert_eq!(
            logo.left() - row.left(),
            px(SIDEBAR_ROW_PADDING_X + (SIDEBAR_FOLDER_SLOT_WIDTH - AGENT_LOGO_SIZE) / 2.)
        );
    }

    #[gpui::test]
    fn an_empty_section_says_no_agents_running(cx: &mut TestAppContext) {
        let (cx, _, _) = harness(cx, Vec::new(), false, 800.);
        let empty = cx.debug_bounds("agents-empty").expect("empty state");
        assert_eq!(empty.size.height, px(32.));
        for header in GROUP_SELECTORS {
            assert!(
                cx.debug_bounds(header).is_none(),
                "an empty section has no {header}"
            );
        }
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
        assert_eq!(first.size.height, px(32.));
        let idle = cx.debug_bounds("agent-group-idle").expect("Idle");
        let no_status = cx.debug_bounds("agent-group-no-status").expect("No status");
        let starting = cx.debug_bounds("agent-row-2").expect("row 2");
        assert_eq!(
            starting.top(),
            idle.bottom(),
            "a session in its grace delay sits under Idle with no status"
        );
        assert!(cx.debug_bounds("agent-status-2").is_none());
        assert_eq!(first.top(), no_status.bottom());
    }

    fn a11y_node(element: &gpui::Stateful<gpui::Div>) -> gpui::accesskit::Node {
        use gpui::Element as _;
        let mut node =
            gpui::accesskit::Node::new(element.a11y_role().expect("the element exposes a role"));
        element.write_a11y_info(&mut node);
        node
    }

    #[test]
    fn a_row_is_a_list_item_named_by_agent_title_workspace_and_state() {
        use gpui::Element as _;
        let ui = crate::theme::ui_colors();
        let waiting = row(
            1,
            AgentRank::Blocked,
            AgentRowStatus::Lane(SidebarAgentState::NeedsInput),
        );
        let element = render_agent_row(&waiting, false, ui, Rc::new(|_, _, _| {}));
        assert_eq!(element.a11y_role(), Some(gpui::accesskit::Role::ListItem));
        assert_eq!(
            a11y_node(&element).label(),
            Some("Claude Code, Attention sidebar, paneflow, waiting for input")
        );

        let spoken = [
            (
                AgentRank::Errored,
                AgentRowStatus::Lane(SidebarAgentState::Errored),
                "error",
            ),
            (
                AgentRank::Done,
                AgentRowStatus::Lane(SidebarAgentState::Finished),
                "done, not yet seen",
            ),
            (
                AgentRank::Working,
                AgentRowStatus::Lane(SidebarAgentState::Thinking),
                "working",
            ),
            (AgentRank::Idle, AgentRowStatus::Silent, "idle"),
            (AgentRank::NoStatus, AgentRowStatus::NoStatus, "no status"),
            (AgentRank::NoStatus, AgentRowStatus::Silent, "starting"),
        ];
        for (rank, status, words) in spoken {
            assert_eq!(
                agent_row_label(&row(1, rank, status)),
                format!("Claude Code, Attention sidebar, paneflow, {words}")
            );
        }
        assert_eq!(a11y_node(&element).description(), None);
        assert_eq!(a11y_node(&element).is_selected(), Some(false));

        let mut silent = row(2, AgentRank::NoStatus, AgentRowStatus::NoStatus);
        silent.active = true;
        let silent = a11y_node(&render_agent_row(&silent, false, ui, Rc::new(|_, _, _| {})));
        assert_eq!(
            silent.description(),
            Some(no_status_tooltip(TerminalAgent::ClaudeCode).as_str()),
            "the reason behind No status reaches assistive technology without a hover"
        );
        assert_eq!(silent.is_selected(), Some(true));

        let listed = ranked_rows(&[AgentRank::Blocked, AgentRank::Working, AgentRank::Working]);
        let groups = agent_groups(&listed);
        assert_eq!(groups.len(), 2);
        let spoken_states = ["waiting for input", "working", "working"];
        for (row, state) in groups
            .iter()
            .flat_map(|group| group.rows.iter())
            .zip(spoken_states)
        {
            let item = render_agent_row(row, false, ui, Rc::new(|_, _, _| {}));
            assert_eq!(item.a11y_role(), Some(gpui::accesskit::Role::ListItem));
            assert_eq!(
                a11y_node(&item).label(),
                Some(format!("Claude Code, Attention sidebar, paneflow, {state}").as_str())
            );
        }
        let working = &groups[1];
        let second = render_agent_group_member(
            render_agent_row(working.rows[1], false, ui, Rc::new(|_, _, _| {})),
            1,
            working.rows.len(),
        );
        let second = a11y_node(&second);
        assert_eq!(
            (second.position_in_set(), second.size_of_set()),
            (Some(2), Some(2)),
            "a row is announced by its place within its own group"
        );
    }

    #[test]
    fn a_group_header_is_a_heading_named_by_its_label_and_agent_count() {
        use gpui::Element as _;
        let ui = crate::theme::ui_colors();
        let rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Working, AgentRank::Working]);
        let groups = agent_groups(&rows);
        let names = ["Need you, 1 agent", "Working, 2 agents"];
        assert_eq!(groups.len(), names.len());
        for (index, (group, name)) in groups.iter().zip(names).enumerate() {
            let header = render_agent_group_header(group, index == 0, ui);
            assert_eq!(header.a11y_role(), Some(gpui::accesskit::Role::Heading));
            let node = a11y_node(&header);
            assert_eq!(node.label(), Some(name));
            assert_eq!(node.level(), Some(AGENT_GROUP_HEADING_LEVEL));
        }
        assert_eq!(
            agent_group_label(AgentGroupStatus::NoStatus, 12),
            "No status, 12 agents"
        );
    }

    #[test]
    fn the_keyboard_cursor_reaches_the_rows_only_of_a_shown_expanded_section() {
        let rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Working]);
        let groups = agent_groups(&rows);
        assert_eq!(agents_nav_targets(true, false, &groups), [1, 2]);
        assert!(agents_nav_targets(true, true, &groups).is_empty());
        assert!(agents_nav_targets(false, false, &groups).is_empty());
        assert!(agents_nav_targets(true, false, &[]).is_empty());
    }

    #[test]
    fn the_keyboard_cursor_walks_the_agents_in_shown_order_and_never_a_group_header() {
        let rows = vec![
            row(1, AgentRank::Blocked, rank_status(AgentRank::Blocked)),
            row(2, AgentRank::Working, rank_status(AgentRank::Working)),
            row(3, AgentRank::NoStatus, AgentRowStatus::NoStatus),
            row(4, AgentRank::NoStatus, AgentRowStatus::Silent),
        ];
        let groups = agent_groups(&rows);
        assert_eq!(groups.len(), 4);
        let targets = agents_nav_targets(true, false, &groups);
        assert_eq!(
            targets,
            [1, 2, 4, 3],
            "one stop per agent, in the order the groups show them"
        );
        assert_eq!(
            targets
                .iter()
                .map(|&surface| agent_list_position(&groups, surface).map(|at| at.row))
                .collect::<Vec<_>>(),
            [Some(1), Some(3), Some(5), Some(7)],
            "every stop is a row child, every header child is skipped"
        );
    }

    #[test]
    fn a_row_is_located_among_the_list_children_by_counting_the_group_headers() {
        let rows = ranked_rows(&[
            AgentRank::Blocked,
            AgentRank::Blocked,
            AgentRank::Working,
            AgentRank::Working,
        ]);
        let groups = agent_groups(&rows);
        assert_eq!(
            agent_list_position(&groups, 1),
            Some(AgentListPosition {
                row: 1,
                group_header: Some(0),
            })
        );
        assert_eq!(
            agent_list_position(&groups, 2),
            Some(AgentListPosition {
                row: 2,
                group_header: None,
            })
        );
        assert_eq!(
            agent_list_position(&groups, 3),
            Some(AgentListPosition {
                row: 4,
                group_header: Some(3),
            })
        );
        assert_eq!(
            agent_list_position(&groups, 4),
            Some(AgentListPosition {
                row: 5,
                group_header: None,
            })
        );
        assert_eq!(agent_list_position(&groups, 99), None);
    }

    #[test]
    fn the_section_list_is_a_list_named_agents() {
        use gpui::Element as _;
        let rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Working]);
        let list = render_agents_list(
            &agent_groups(&rows),
            None,
            &ScrollHandle::new(),
            crate::theme::ui_colors(),
            Rc::new(|_, _, _| {}),
        );
        assert_eq!(list.a11y_role(), Some(gpui::accesskit::Role::List));
        assert_eq!(a11y_node(&list).label(), Some("Agents"));
    }

    #[gpui::test]
    fn the_cursor_row_wears_the_sidebar_cursor_ring_and_is_scrolled_into_view(
        cx: &mut TestAppContext,
    ) {
        let mut ranks = vec![AgentRank::Blocked; 2];
        ranks.extend([AgentRank::Working; 10]);
        let rows = ranked_rows(&ranks);
        let shown = rows.clone();
        let groups = agent_groups(&shown);
        assert_eq!(groups.len(), 2);
        let scroll = ScrollHandle::new();
        let held = scroll.clone();
        let (view, cx) = cx.add_window_view(move |_, _| SectionHarness {
            rows,
            cursor: Some(12),
            scroll: held,
            collapsed: false,
            height: 900.,
            picks: Rc::new(RefCell::new(Vec::new())),
            toggles: Rc::new(Cell::new(0)),
        });
        let redraw = |cx: &mut gpui::VisualTestContext| {
            view.update(cx, |_, cx| cx.notify());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        };
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let section = cx.debug_bounds("agents-section").expect("section");
        let list_top = cx.debug_bounds("agents-header").expect("header").bottom();
        assert!(
            cx.debug_bounds("agent-row-12")
                .is_none_or(|row| row.top() >= section.bottom()),
            "the last row starts below the 330 px section"
        );

        scroll_agents_list_to(&scroll, &groups, 12);
        redraw(cx);
        let row = cx.debug_bounds("agent-row-12").expect("last row");
        assert!(
            row.bottom() <= section.bottom() && row.top() >= list_top,
            "the cursor row is visible past both group headers"
        );
        let ring = cx.debug_bounds("agent-cursor-12").expect("cursor ring");
        assert_eq!(ring, row);
        assert!(cx.debug_bounds("agent-cursor-11").is_none());

        view.update(cx, |harness, _| harness.cursor = Some(3));
        scroll_agents_list_to(&scroll, &groups, 3);
        redraw(cx);
        let working = cx.debug_bounds("agent-group-working").expect("Working");
        let row = cx.debug_bounds("agent-row-3").expect("row 3");
        assert_eq!(
            working.top(),
            list_top,
            "the first row of a group brings its header back into view"
        );
        assert_eq!(row.top(), working.bottom());
        assert_eq!(cx.debug_bounds("agent-cursor-3"), Some(row));

        view.update(cx, |harness, _| harness.cursor = Some(1));
        scroll_agents_list_to(&scroll, &groups, 1);
        redraw(cx);
        let need_you = cx.debug_bounds("agent-group-need-you").expect("Need you");
        let row = cx.debug_bounds("agent-row-1").expect("row 1");
        assert_eq!(need_you.top(), list_top, "Home shows the first header");
        assert_eq!(row.top(), need_you.bottom());

        view.update(cx, |harness, _| harness.cursor = Some(7));
        scroll_agents_list_to(&scroll, &groups, 12);
        redraw(cx);
        scroll_agents_list_to(&scroll, &groups, 7);
        redraw(cx);
        let row = cx.debug_bounds("agent-row-7").expect("row 7");
        assert!(
            row.top() >= list_top && row.bottom() <= section.bottom(),
            "a row in the middle of a group is scrolled to by its own child index"
        );
        assert_eq!(cx.debug_bounds("agent-cursor-7"), Some(row));
    }

    #[gpui::test]
    fn the_cursor_stays_on_its_agent_when_that_agent_changes_group(cx: &mut TestAppContext) {
        let rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Working, AgentRank::Working]);
        let (view, cx) = cx.add_window_view(move |_, _| SectionHarness {
            rows,
            cursor: Some(3),
            scroll: ScrollHandle::new(),
            collapsed: false,
            height: 900.,
            picks: Rc::new(RefCell::new(Vec::new())),
            toggles: Rc::new(Cell::new(0)),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let working = cx.debug_bounds("agent-group-working").expect("Working");
        let row = cx.debug_bounds("agent-row-3").expect("row 3");
        assert!(row.top() > working.bottom());
        assert_eq!(cx.debug_bounds("agent-cursor-3"), Some(row));

        view.update(cx, |harness, cx| {
            harness.rows[2].rank = AgentRank::Errored;
            harness.rows[2].status = rank_status(AgentRank::Errored);
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let error = cx.debug_bounds("agent-group-error").expect("Error");
        let working = cx.debug_bounds("agent-group-working").expect("Working");
        let moved = cx.debug_bounds("agent-row-3").expect("row 3");
        assert_eq!(moved.top(), error.bottom(), "the row moved under Error");
        assert!(moved.bottom() < working.top());
        assert_eq!(
            cx.debug_bounds("agent-cursor-3"),
            Some(moved),
            "the cursor followed the same agent"
        );
        for other in ["agent-cursor-1", "agent-cursor-2"] {
            assert!(
                cx.debug_bounds(other).is_none(),
                "no other row took the cursor"
            );
        }
        assert_eq!(
            agents_nav_targets(
                true,
                false,
                &agent_groups(&view.read_with(cx, |harness, _| harness.rows.clone()))
            ),
            [1, 3, 2]
        );
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
        let AgentRows { rows, .. } = agent_rows(
            workspaces,
            AgentScope::active_space_only(None),
            Some(0),
            AgentRowDetails::ALL,
            Instant::now(),
            cx,
        );
        render_agents_section(
            AgentsSectionFrame {
                groups: &agent_groups(&rows),
                cursor: None,
                collapsed: false,
                chosen_height: None,
                chevron_turn: 0.,
                scroll: &ScrollHandle::new(),
                scrollbar: None,
                resize_handle: None,
                display_button: None,
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
                AgentScope::active_space_only(None),
                Some(0),
                AgentRowDetails::ALL,
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
