use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, CursorStyle, DispatchPhase, FocusHandle, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Orientation, Point, ScrollHandle, Task, canvas, rgb,
};

use super::agents::{AgentRank, agent_candidates, order_agents};
use super::lane::{mark_shimmer_drawn, render_lane};
use super::*;
use crate::agent_launcher::TerminalAgent;
use crate::app::workspace_ops::WorkspaceFocusTarget;
use crate::window_state::AgentsSectionState;

pub(super) const AGENTS_HEADER_HEIGHT: f32 = 36.;
pub(super) const AGENTS_TREE_MIN_HEIGHT: f32 = 120.;
const AGENTS_LIST_BOTTOM_PADDING: f32 = 4.;
const AGENTS_HEIGHT_CAP: f32 = 330.;
const AGENTS_RESIZE_FLOOR: f32 =
    AGENTS_HEADER_HEIGHT + AGENT_ROW_HEIGHT + AGENTS_LIST_BOTTOM_PADDING;
const AGENTS_RESIZE_MAX: f32 = 620.;
const AGENTS_RESIZE_HIT_HEIGHT: f32 = 9.;
const AGENTS_RESIZE_STEP: f32 = 16.;
const AGENTS_RESIZE_FOCUS_ALPHA: f32 = 0.35;
const AGENTS_RESIZE_GROUP: &str = "agents-resize-group";
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

fn agents_natural_height(rows: &[AgentRow]) -> f32 {
    AGENTS_HEADER_HEIGHT + agents_content_height(rows) + AGENTS_LIST_BOTTOM_PADDING
}

pub(super) fn agents_section_height(
    rows: &[AgentRow],
    collapsed: bool,
    chosen: Option<f32>,
) -> f32 {
    if collapsed {
        return AGENTS_HEADER_HEIGHT;
    }
    let natural = agents_natural_height(rows);
    match chosen.filter(|_| !rows.is_empty()) {
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

pub(super) fn agent_row_label(row: &AgentRow) -> String {
    format!(
        "{}, {}, {}, {}",
        row.tool.display_name(),
        row.title,
        row.workspace,
        agent_row_spoken_state(row)
    )
}

pub(super) fn agent_row_child_index(position: usize) -> usize {
    position * 2
}

fn agents_nav_targets(enabled: bool, collapsed: bool, rows: &[AgentRow]) -> Vec<u64> {
    if !enabled || collapsed {
        return Vec::new();
    }
    rows.iter().map(|row| row.surface_id).collect()
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
        .child(title_row)
        .child(meta)
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
    pub(super) rows: &'a [AgentRow],
    pub(super) cursor: Option<u64>,
    pub(super) collapsed: bool,
    pub(super) chosen_height: Option<f32>,
    pub(super) chevron_turn: f32,
    pub(super) scroll: &'a ScrollHandle,
    pub(super) scrollbar: Option<AnyElement>,
    pub(super) resize_handle: Option<AnyElement>,
}

fn render_agents_list(
    rows: &[AgentRow],
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
        list = list.child(render_agent_row(
            row,
            cursor == Some(row.surface_id),
            ui,
            on_pick.clone(),
        ));
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
        rows,
        cursor,
        collapsed,
        chosen_height,
        chevron_turn,
        scroll,
        scrollbar,
        resize_handle,
    } = frame;
    let section = div()
        .relative()
        .h(px(agents_section_height(rows, collapsed, chosen_height)))
        .min_h(px(AGENTS_HEADER_HEIGHT))
        .flex_shrink_1()
        .flex()
        .flex_col()
        .debug_selector(|| "agents-section".into())
        .child(render_agents_header(collapsed, chevron_turn, ui, on_toggle));
    if collapsed {
        return section;
    }
    let resize_handle = resize_handle.filter(|_| !rows.is_empty());
    let list = render_agents_list(rows, cursor, scroll, ui, on_pick);
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
            self.active_space.as_ref(),
            self.shown_workspace_idx(),
            now,
            cx,
        );
        self.schedule_no_status_wake(grace_deadline, cx);
        let (collapsed, chosen_height, dragging, chevron_turn, turning, scroll) = {
            let mut section = self.agents_section.borrow_mut();
            if section.follow_top_row(rows.first().map(|row| row.surface_id)) {
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
        let natural = agents_natural_height(&rows);
        let resize_app = cx.weak_entity();
        let resize_handle = render_agents_resize_handle(
            AgentsResizeFrame {
                focus: &self.agents_resize_focus,
                focused: self.agents_resize_focus.is_focused(window),
                dragging,
                height: agents_section_height(&rows, collapsed, chosen_height),
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
                rows: &rows,
                cursor,
                collapsed,
                chosen_height,
                chevron_turn,
                scroll: &scroll,
                scrollbar,
                resize_handle: Some(resize_handle),
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

    pub(super) fn agents_nav_surfaces(&self, cx: &App) -> Vec<u64> {
        let shown = agent_rows(
            &self.workspaces,
            self.active_space.as_ref(),
            self.shown_workspace_idx(),
            Instant::now(),
            cx,
        );
        agents_nav_targets(
            self.cached_config.sidebar_show.agents_enabled(),
            self.agents_section.borrow().collapsed,
            &shown.rows,
        )
    }

    pub(super) fn scroll_agents_to(&self, position: usize) {
        self.agents_section
            .borrow()
            .scroll
            .scroll_to_item(agent_row_child_index(position));
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
        assert_eq!(agents_section_height(&rows, false, None), 36. + 177. + 4.);
        assert_eq!(agents_section_height(&rows[..1], false, None), 95.);
        assert_eq!(agents_section_height(&[], false, None), 36. + 32. + 4.);
    }

    #[test]
    fn the_section_height_is_capped_and_a_collapsed_section_keeps_only_its_header() {
        let rows = ranked_rows(&[AgentRank::Working; 10]);
        assert!(36. + agents_content_height(&rows) + 4. > 330.);
        assert_eq!(agents_section_height(&rows, false, None), 330.);
        assert_eq!(agents_section_height(&rows, true, None), 36.);
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
        let natural = agents_natural_height(&ten);
        assert_eq!(natural, 36. + 10. * 55. + 9. * 2. + 4.);
        assert_eq!(agents_section_height(&ten, false, Some(450.)), 450.);
        assert_eq!(agents_section_height(&ten, false, Some(10.)), 95.);
        assert_eq!(agents_section_height(&ten, false, Some(900.)), natural);
        let twelve = ranked_rows(&[AgentRank::Working; 12]);
        assert_eq!(agents_section_height(&twelve, false, Some(900.)), 620.);
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
        assert_eq!(section.height, Some(95.));
        assert!(section.resize(AgentsResizeInput::Release, 95., 589.));
        assert!(!section.resize(AgentsResizeInput::Release, 95., 589.));
        assert!(!section.resize(AgentsResizeInput::Drag(0.), 95., 589.));
        assert_eq!(section.height, Some(95.), "a move after release is ignored");
    }

    #[test]
    fn keys_step_by_16_px_jump_to_the_bounds_and_enter_fits_the_content() {
        let mut section = AgentsSection::default();
        assert!(section.resize(AgentsResizeInput::Step(16.), 330., 589.));
        assert_eq!(section.height, Some(346.));
        assert!(section.resize(AgentsResizeInput::Step(-16.), 346., 589.));
        assert_eq!(section.height, Some(330.));
        assert!(section.resize(AgentsResizeInput::Floor, 330., 589.));
        assert_eq!(section.height, Some(95.));
        assert!(section.resize(AgentsResizeInput::Step(-16.), 95., 589.));
        assert_eq!(section.height, Some(95.));
        assert!(section.resize(AgentsResizeInput::Ceiling, 95., 589.));
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
            let natural = agents_natural_height(&self.rows);
            let dragging = self.section.resize_drag.is_some();
            let chosen = self.section.height;
            let this = cx.weak_entity();
            let handle = render_agents_resize_handle(
                AgentsResizeFrame {
                    focus: &self.focus,
                    focused: self.focus.is_focused(window),
                    dragging,
                    height: agents_section_height(&self.rows, self.section.collapsed, chosen),
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
                        rows: &self.rows,
                        cursor: None,
                        collapsed: self.section.collapsed,
                        chosen_height: chosen,
                        chevron_turn: if self.section.collapsed { 1. } else { 0. },
                        scroll: &self.section.scroll,
                        scrollbar: None,
                        resize_handle: Some(handle),
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
            ranked_rows(&[AgentRank::Working; 10]),
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
        assert_eq!(section_height(cx), px(95.));
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
            ranked_rows(&[AgentRank::Working; 10]),
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
        assert_eq!(press(cx, "home"), px(95.));
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
        let (view, cx) = resize_harness(cx, ranked_rows(&[AgentRank::Working; 10]), chosen, short);
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
                        cursor: self.cursor,
                        collapsed: self.collapsed,
                        chosen_height: None,
                        chevron_turn: if self.collapsed { 1. } else { 0. },
                        scroll: &self.scroll,
                        scrollbar: None,
                        resize_handle: None,
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
    }

    #[test]
    fn the_keyboard_cursor_reaches_the_rows_only_of_a_shown_expanded_section() {
        let rows = ranked_rows(&[AgentRank::Blocked, AgentRank::Working]);
        assert_eq!(agents_nav_targets(true, false, &rows), [1, 2]);
        assert!(agents_nav_targets(true, true, &rows).is_empty());
        assert!(agents_nav_targets(false, false, &rows).is_empty());
        assert!(agents_nav_targets(true, false, &[]).is_empty());
    }

    #[test]
    fn the_section_list_is_a_list_named_agents() {
        use gpui::Element as _;
        let list = render_agents_list(
            &ranked_rows(&[AgentRank::Blocked]),
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
        let scroll = ScrollHandle::new();
        let held = scroll.clone();
        let (view, cx) = cx.add_window_view(move |_, _| SectionHarness {
            rows: ranked_rows(&[AgentRank::Working; 10]),
            cursor: Some(10),
            scroll: held,
            collapsed: false,
            height: 900.,
            picks: Rc::new(RefCell::new(Vec::new())),
            toggles: Rc::new(Cell::new(0)),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let section = cx.debug_bounds("agents-section").expect("section");
        assert!(
            cx.debug_bounds("agent-row-10")
                .is_none_or(|row| row.top() >= section.bottom()),
            "the last row starts below the 330 px section"
        );

        scroll.scroll_to_item(agent_row_child_index(9));
        view.update(cx, |_, cx| cx.notify());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let row = cx.debug_bounds("agent-row-10").expect("last row");
        assert!(
            row.bottom() <= section.bottom(),
            "the cursor row is visible"
        );
        let ring = cx.debug_bounds("agent-cursor-10").expect("cursor ring");
        assert_eq!(ring, row);
        assert!(cx.debug_bounds("agent-cursor-9").is_none());
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
                cursor: None,
                collapsed: false,
                chosen_height: None,
                chevron_turn: 0.,
                scroll: &ScrollHandle::new(),
                scrollbar: None,
                resize_handle: None,
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
