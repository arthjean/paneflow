//! Rosetta projection, recent-history, and GPUI surface helpers.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, ClickEvent, Context, Focusable as _, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, SharedString, Styled, Transformation,
    Window, deferred, div, percentage, prelude::*, px, rgb, svg,
};
use paneflow_config::schema::AppMode;

use crate::agent_launcher::TerminalAgent;
use crate::ai_types::{AgentSession, AgentState};
use crate::app::ipc_handler::find_pane_by_surface_id;
use crate::app::workspace_ops::WorkspaceFocusTarget;
use crate::project::{AgentsTarget, Project, Thread, ThreadStatus};
use crate::settings::components::with_alpha;
use crate::workspace::Workspace;

pub(crate) const ROSETTA_AGENT_TEXT_CAP_CHARS: usize = 512;
pub(crate) const ROSETTA_RECENT_EVENT_CAP: usize = 25;
pub(crate) const ROSETTA_RECENT_EVENT_RETENTION: Duration = Duration::from_secs(5 * 60);
const ROSETTA_COMPACT_MAX_WIDTH: f32 = 460.;
const ROSETTA_COMPACT_PASSIVE_MAX_WIDTH: f32 = 360.;
const ROSETTA_EXPANDED_MAX_WIDTH: f32 = 640.;
const ROSETTA_CARD_SIDE_GUTTER: f32 = 16.;
const ROSETTA_CARD_TOP: f32 = 14.;
const ROSETTA_MAX_PANEL_HEIGHT_RATIO: f32 = 0.70;
const ROSETTA_MIN_PANEL_HEIGHT: f32 = 260.;
const ROSETTA_MAX_PANEL_HEIGHT: f32 = 520.;
const ROSETTA_PANEL_CHROME_HEIGHT: f32 = 92.;
const ROSETTA_CARD_RADIUS: f32 = 14.;
pub(crate) const ROSETTA_SNOOZE_DURATION: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RosettaRowState {
    Errored,
    WaitingForInput,
    Stalled,
    Thinking,
    Finished,
}

impl RosettaRowState {
    pub(crate) fn from_agent_state(state: &AgentState) -> Self {
        match state {
            AgentState::Errored => Self::Errored,
            AgentState::WaitingForInput => Self::WaitingForInput,
            AgentState::Stalled => Self::Stalled,
            AgentState::Thinking => Self::Thinking,
            AgentState::Finished => Self::Finished,
        }
    }

    pub(crate) fn from_thread_status(status: ThreadStatus) -> Option<Self> {
        match status {
            ThreadStatus::Idle => None,
            ThreadStatus::Thinking => Some(Self::Thinking),
            ThreadStatus::WaitingForInput => Some(Self::WaitingForInput),
            ThreadStatus::Failed => Some(Self::Errored),
        }
    }

    fn priority_rank(self) -> u8 {
        match self {
            Self::Errored => 5,
            Self::WaitingForInput => 4,
            Self::Stalled => 3,
            Self::Thinking => 2,
            Self::Finished => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RosettaFocusTarget {
    WorkspaceSurface { workspace_id: u64, surface_id: u64 },
    AgentsThread { thread_id: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RosettaRowSource {
    WorkspaceSession {
        workspace_id: u64,
        pid: u32,
    },
    AgentsThread {
        target: AgentsTarget,
        thread_id: u64,
    },
    RecentEvent {
        sequence: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RosettaRowKey {
    WorkspaceSession { workspace_id: u64, pid: u32 },
    AgentsThread { thread_id: u64 },
    RecentEvent { sequence: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RosettaRow {
    pub(crate) source: RosettaRowSource,
    pub(crate) state: RosettaRowState,
    pub(crate) tool: Option<TerminalAgent>,
    pub(crate) workspace_title: Option<String>,
    pub(crate) thread_title: Option<String>,
    pub(crate) context: Option<String>,
    pub(crate) surface_id: Option<u64>,
    pub(crate) focus_target: Option<RosettaFocusTarget>,
    pub(crate) message: Option<String>,
    pub(crate) waiting_secs: Option<u64>,
    pub(crate) last_activity_secs: Option<u64>,
    pub(crate) active_tool_name: Option<String>,
    pub(crate) last_result: Option<String>,
    sort_order: usize,
}

impl RosettaRow {
    pub(crate) fn key(&self) -> RosettaRowKey {
        match self.source {
            RosettaRowSource::WorkspaceSession { workspace_id, pid } => {
                RosettaRowKey::WorkspaceSession { workspace_id, pid }
            }
            RosettaRowSource::AgentsThread { thread_id, .. } => {
                RosettaRowKey::AgentsThread { thread_id }
            }
            RosettaRowSource::RecentEvent { sequence } => RosettaRowKey::RecentEvent { sequence },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RosettaProjection {
    pub(crate) rows: Vec<RosettaRow>,
}

impl RosettaProjection {
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RosettaSectionKind {
    NeedsInput,
    Failed,
    Stalled,
    Running,
    Recent,
}

impl RosettaSectionKind {
    fn title(self) -> &'static str {
        match self {
            Self::NeedsInput => "Needs input",
            Self::Failed => "Failed",
            Self::Stalled => "Stalled",
            Self::Running => "Running",
            Self::Recent => "Recent",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RosettaSection<'a> {
    pub(crate) kind: RosettaSectionKind,
    pub(crate) rows: Vec<&'a RosettaRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RosettaCompactSummary {
    pub(crate) label: &'static str,
    pub(crate) title: String,
    pub(crate) detail: Option<String>,
    pub(crate) state: RosettaRowState,
    pub(crate) count: usize,
    pub(crate) passive_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RosettaTargetStatus {
    Navigable,
    NoPane,
    Unavailable,
}

impl RosettaTargetStatus {
    fn is_navigable(self) -> bool {
        matches!(self, Self::Navigable)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RosettaSecondaryAction {
    Read,
    Snooze,
    Dismiss,
}

pub(crate) fn rosetta_sections(rows: &[RosettaRow]) -> Vec<RosettaSection<'_>> {
    let mut sections = Vec::new();
    for kind in [
        RosettaSectionKind::NeedsInput,
        RosettaSectionKind::Failed,
        RosettaSectionKind::Stalled,
        RosettaSectionKind::Running,
        RosettaSectionKind::Recent,
    ] {
        let section_rows: Vec<_> = rows
            .iter()
            .filter(|row| rosetta_section_kind(row.state) == kind)
            .collect();
        if !section_rows.is_empty() {
            sections.push(RosettaSection {
                kind,
                rows: section_rows,
            });
        }
    }
    sections
}

pub(crate) fn rosetta_compact_summary(rows: &[RosettaRow]) -> Option<RosettaCompactSummary> {
    let row_refs: Vec<_> = rows.iter().collect();
    rosetta_compact_summary_for_rows(&row_refs)
}

fn rosetta_compact_summary_for_rows(rows: &[&RosettaRow]) -> Option<RosettaCompactSummary> {
    let primary = *rows.first()?;
    let state = primary.state;
    let label = rosetta_state_label(state);
    let count = rows.iter().filter(|row| row.state == state).count();
    let title = if count > 1 {
        format!("{count} {}", rosetta_plural_subject(state))
    } else {
        rosetta_row_title(primary)
    };
    let passive_only = rows
        .iter()
        .all(|row| row.state == RosettaRowState::Thinking);
    let detail = (!passive_only)
        .then(|| rosetta_row_detail(primary))
        .flatten();

    Some(RosettaCompactSummary {
        label,
        title,
        detail,
        state,
        count,
        passive_only,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct RosettaRecentEvent {
    pub(crate) state: RosettaRowState,
    pub(crate) tool: Option<TerminalAgent>,
    pub(crate) workspace_title: Option<String>,
    pub(crate) thread_title: Option<String>,
    pub(crate) context: Option<String>,
    pub(crate) surface_id: Option<u64>,
    pub(crate) focus_target: Option<RosettaFocusTarget>,
    pub(crate) message: Option<String>,
    pub(crate) last_result: Option<String>,
    pub(crate) occurred_at: Instant,
    sequence: u64,
}

impl RosettaRecentEvent {
    pub(crate) fn new(state: RosettaRowState, occurred_at: Instant) -> Self {
        Self {
            state,
            tool: None,
            workspace_title: None,
            thread_title: None,
            context: None,
            surface_id: None,
            focus_target: None,
            message: None,
            last_result: None,
            occurred_at,
            sequence: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RosettaRecentHistory {
    events: VecDeque<RosettaRecentEvent>,
    next_sequence: u64,
}

impl RosettaRecentHistory {
    pub(crate) fn push(&mut self, mut event: RosettaRecentEvent) {
        if let Some(target) = event.focus_target {
            self.events.retain(|existing| {
                existing.focus_target != Some(target)
                    || existing.state != RosettaRowState::Finished
                    || event.state != RosettaRowState::Finished
            });
            if event.state != RosettaRowState::Finished {
                self.events.retain(|existing| {
                    !(existing.focus_target == Some(target)
                        && existing.state == RosettaRowState::Finished)
                });
            }
        }
        event.sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        event.message = cap_optional_agent_text(event.message.as_deref());
        event.last_result = cap_optional_agent_text(event.last_result.as_deref());
        self.events.push_back(event);

        while self.events.len() > ROSETTA_RECENT_EVENT_CAP {
            let _ = self.events.pop_front();
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }

    pub(crate) fn dismiss_sequence(&mut self, sequence: u64) -> bool {
        let before = self.events.len();
        self.events.retain(|event| event.sequence != sequence);
        before != self.events.len()
    }

    fn visible_events(&self, now: Instant) -> impl Iterator<Item = &RosettaRecentEvent> {
        self.events
            .iter()
            .filter(move |event| elapsed(now, event.occurred_at) <= ROSETTA_RECENT_EVENT_RETENTION)
    }
}

#[derive(Clone, Copy)]
struct WorkspaceSessionRef<'a> {
    workspace_id: u64,
    workspace_title: &'a str,
    pid: u32,
    session: &'a AgentSession,
}

impl crate::PaneFlowApp {
    pub(crate) fn rosetta_projection(&self, now: Instant) -> RosettaProjection {
        build_rosetta_projection(
            &self.workspaces,
            &self.projects,
            &self.chats,
            &self.rosetta_recent_history,
            now,
        )
    }

    pub(crate) fn record_workspace_rosetta_event(
        &mut self,
        workspace_id: u64,
        session_key: u32,
        state: RosettaRowState,
        occurred_at: Instant,
    ) {
        let event = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .and_then(|workspace| {
                let session = workspace.agent_sessions.get(&session_key)?;
                Some(rosetta_recent_event_from_workspace_session(
                    workspace.id,
                    &workspace.title,
                    session,
                    state,
                    occurred_at,
                ))
            });
        if let Some(event) = event {
            self.rosetta_recent_history.push(event);
        }
    }

    pub(crate) fn record_agents_thread_rosetta_event(
        &mut self,
        target: AgentsTarget,
        state: RosettaRowState,
        occurred_at: Instant,
    ) {
        let event = self.thread_for_target(target).map(|thread| {
            let context = match target {
                AgentsTarget::Thread { project_idx, .. } => self
                    .projects
                    .get(project_idx)
                    .map(|project| project.title.as_str()),
                AgentsTarget::Chat { .. } => Some("Chat"),
            };
            rosetta_recent_event_from_agents_thread(thread, context, state, occurred_at)
        });
        if let Some(event) = event {
            self.rosetta_recent_history.push(event);
        }
    }

    pub(crate) fn render_rosetta_surface(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.rosetta_surface_allowed() {
            self.reset_rosetta_surface_state();
            return None;
        }

        let now = Instant::now();
        let projection = self.rosetta_projection(now);
        self.prune_rosetta_action_state(&projection.rows, now);
        if projection.is_empty() && !self.rosetta_surface_expanded {
            self.reset_rosetta_surface_state();
            return None;
        }
        if !self.rosetta_passive_display_enabled
            && !self.rosetta_surface_expanded
            && projection
                .rows
                .iter()
                .all(|row| row.state == RosettaRowState::Thinking)
        {
            self.reset_rosetta_surface_state();
            return None;
        }

        let ui = crate::theme::ui_colors();
        let content = if self.rosetta_surface_expanded {
            let max_width = rosetta_preferred_card_max_width(window, ROSETTA_EXPANDED_MAX_WIDTH);
            let max_panel_height = rosetta_panel_max_height(window);
            self.render_rosetta_expanded(&projection, max_width, max_panel_height, ui, cx)
        } else {
            let compact_rows = self.rosetta_compact_rows(&projection.rows);
            let summary = rosetta_compact_summary_for_rows(&compact_rows)?;
            let max_width = rosetta_card_max_width(window, self.rosetta_surface_expanded, &summary);
            self.render_rosetta_compact(&summary, max_width, ui, cx)
        };

        Some(
            deferred(
                div()
                    .id("rosetta-surface-anchor")
                    .absolute()
                    .top(px(ROSETTA_CARD_TOP))
                    .left_0()
                    .right_0()
                    .px(px(ROSETTA_CARD_SIDE_GUTTER))
                    .flex()
                    .items_start()
                    .justify_center()
                    .child(content),
            )
            .with_priority(5)
            .into_any_element(),
        )
    }

    pub(crate) fn handle_toggle_rosetta_surface(
        &mut self,
        _: &crate::ToggleRosettaSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_transient_surfaces();

        if !self.rosetta_surface_allowed() {
            self.reset_rosetta_surface_state();
            cx.notify();
            return;
        }

        if self.rosetta_surface_expanded {
            self.close_rosetta_surface(window, cx);
        } else {
            self.open_rosetta_surface(window, cx);
        }
    }

    fn render_rosetta_compact(
        &self,
        summary: &RosettaCompactSummary,
        max_width: f32,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status =
            rosetta_status_indicator(summary.state, ui, SharedString::from("rosetta-compact"));
        let mut card = div()
            .id("rosetta-compact")
            .occlude()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .w_full()
            .max_w(px(max_width))
            .min_h(px(34.))
            .px(px(11.))
            .py(px(7.))
            .rounded(px(ROSETTA_CARD_RADIUS))
            .bg(ui.surface)
            .shadow_md()
            .text_size(px(12.))
            .text_color(ui.text)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.open_rosetta_surface(window, cx);
                cx.stop_propagation();
            }))
            .child(status)
            .child(rosetta_label_chip(summary.label, ui))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(summary.title.clone()),
            );

        if let Some(detail) = &summary.detail {
            card = card.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(ui.muted)
                    .child(detail.clone()),
            );
        }

        card.into_any_element()
    }

    fn render_rosetta_expanded(
        &mut self,
        projection: &RosettaProjection,
        max_width: f32,
        max_panel_height: f32,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sections = rosetta_sections(&projection.rows);
        let flat_rows = rosetta_flat_rows(&sections);
        let selected = self
            .rosetta_surface_selected
            .min(flat_rows.len().saturating_sub(1));
        self.rosetta_surface_selected = selected;
        self.rosetta_surface_selected_key = flat_rows.get(selected).map(|row| row.key());
        let list_max_height = (max_panel_height - ROSETTA_PANEL_CHROME_HEIGHT)
            .max(ROSETTA_MIN_PANEL_HEIGHT - ROSETTA_PANEL_CHROME_HEIGHT);

        let mut list = div()
            .id("rosetta-list")
            .flex()
            .flex_col()
            .max_h(px(list_max_height))
            .overflow_y_scroll()
            .track_scroll(&self.rosetta_surface_scroll);

        let mut flat_idx = 0;
        for section in &sections {
            list = list.child(
                div()
                    .px(px(12.))
                    .pt(px(10.))
                    .pb(px(4.))
                    .text_size(px(10.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(ui.muted)
                    .child(section.kind.title()),
            );
            for row in &section.rows {
                let row = *row;
                let idx = flat_idx;
                let status = self.rosetta_target_status(row, cx);
                list =
                    list.child(self.render_rosetta_row(row, idx, idx == selected, status, ui, cx));
                flat_idx += 1;
            }
        }
        if flat_rows.is_empty() {
            list = list.child(render_rosetta_empty_state(ui));
        }

        div()
            .id("rosetta-expanded")
            .occlude()
            .track_focus(&self.rosetta_surface_focus)
            .on_key_down(cx.listener(Self::handle_rosetta_surface_key_down))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.close_rosetta_surface(window, cx);
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .w_full()
            .max_w(px(max_width))
            .max_h(px(max_panel_height))
            .flex()
            .flex_col()
            .rounded(px(ROSETTA_CARD_RADIUS))
            .bg(ui.surface)
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(9.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.))
                            .text_color(ui.text)
                            .child("Rosetta"),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(px(7.))
                            .py(px(3.))
                            .rounded(px(6.))
                            .bg(ui.subtle)
                            .text_size(px(10.))
                            .text_color(ui.text)
                            .child("Esc"),
                    ),
            )
            .child(list)
            .child(self.render_rosetta_footer(&projection.rows, ui, cx))
            .into_any_element()
    }

    fn render_rosetta_footer(
        &self,
        rows: &[RosettaRow],
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let passive_only = !rows.is_empty()
            && rows
                .iter()
                .all(|row| row.state == RosettaRowState::Thinking);
        let mut footer = div()
            .px(px(12.))
            .py(px(7.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .text_size(px(10.))
            .text_color(ui.muted)
            .child(div().flex_1().min_w_0().child(if rows.is_empty() {
                "No active Rosetta items"
            } else {
                "Enter opens the selected target"
            }));

        if passive_only {
            footer = footer.child(
                rosetta_text_button("Hide running", ui)
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.rosetta_passive_display_enabled = false;
                        this.rosetta_surface_expanded = false;
                        this.rosetta_surface_selected = 0;
                        this.rosetta_surface_selected_key = None;
                        this.persist_setting(
                            false,
                            "rosetta_show_passive",
                            serde_json::Value::Bool(false),
                            cx,
                        );
                        cx.notify();
                        cx.stop_propagation();
                    }))
                    .into_any_element(),
            );
        }

        footer.into_any_element()
    }

    fn render_rosetta_row(
        &self,
        row: &RosettaRow,
        idx: usize,
        selected: bool,
        status: RosettaTargetStatus,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row_id = SharedString::from(format!("rosetta-row-{idx}"));
        let row_key = row.key();
        let detail = rosetta_row_detail(row);
        let is_navigable = status.is_navigable();
        let is_read = self.rosetta_read_rows.contains(&row_key);
        let mut title_el = div()
            .min_w_0()
            .overflow_x_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(if is_read { ui.muted } else { ui.text })
            .child(rosetta_row_title(row));
        if detail.is_some() {
            title_el = title_el.flex_none().max_w(px(180.));
        } else {
            title_el = title_el.flex_1();
        }

        let mut actions = div()
            .flex_none()
            .ml_auto()
            .flex()
            .flex_row()
            .justify_end()
            .items_center()
            .gap(px(4.));
        if let Some(time) = rosetta_row_time(row) {
            actions = actions.child(
                div()
                    .flex_none()
                    .w(px(36.))
                    .text_align(gpui::TextAlign::Right)
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .child(time),
            );
        }
        let secondary_actions = rosetta_secondary_actions(row, is_read);
        let has_dismiss = secondary_actions.contains(&RosettaSecondaryAction::Dismiss);
        for action in secondary_actions
            .into_iter()
            .filter(|action| *action != RosettaSecondaryAction::Dismiss)
        {
            actions = actions.child(self.render_rosetta_secondary_action(row_key, action, ui, cx));
        }
        actions = actions.child(self.render_rosetta_primary_action(
            row_key,
            idx,
            rosetta_primary_action_label(row, status),
            status,
            ui,
            cx,
        ));
        if has_dismiss {
            actions = actions.child(self.render_rosetta_dismiss_action(row_key, ui, cx));
        }

        let mut row_el = div()
            .id(row_id)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(7.))
            .text_size(px(12.))
            .when(selected, |d| d.bg(ui.subtle))
            .when(is_read, |d| d.opacity(0.68))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(rosetta_status_indicator(
                row.state,
                ui,
                SharedString::from(format!("rosetta-row-{idx}")),
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_none()
                            .text_color(if is_read { ui.muted } else { ui.text })
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(rosetta_agent_label(row)),
                    )
                    .when(row.state == RosettaRowState::WaitingForInput, |d| {
                        d.child(rosetta_attention_badge(ui))
                    })
                    .child(title_el)
                    .when_some(detail, |d, detail| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_x_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(11.))
                                .text_color(if is_navigable {
                                    ui.muted
                                } else {
                                    with_alpha(ui.muted, 0.78)
                                })
                                .child(detail),
                        )
                    })
                    .when_some(rosetta_row_context(row), |d, context| {
                        d.child(
                            div()
                                .flex_none()
                                .max_w(px(118.))
                                .overflow_x_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(10.))
                                .text_color(ui.muted)
                                .child(context),
                        )
                    }),
            )
            .child(actions);

        if is_navigable {
            row_el = row_el.cursor_pointer().on_click(cx.listener(
                move |this, _: &ClickEvent, window, cx| {
                    this.rosetta_surface_selected = idx;
                    this.rosetta_surface_selected_key = Some(row_key);
                    let _ = this.activate_rosetta_row_by_key(row_key, window, cx);
                    cx.stop_propagation();
                },
            ));
        } else {
            row_el = row_el.on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.rosetta_surface_selected = idx;
                this.rosetta_surface_selected_key = Some(row_key);
                this.explain_unavailable_rosetta_row(status, cx);
                cx.notify();
                cx.stop_propagation();
            }));
        }

        row_el.into_any_element()
    }

    fn render_rosetta_primary_action(
        &self,
        row_key: RosettaRowKey,
        idx: usize,
        label: &'static str,
        status: RosettaTargetStatus,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_navigable = status.is_navigable();
        let button = rosetta_text_button(label, ui).text_color(if is_navigable {
            ui.text
        } else {
            ui.muted
        });

        if is_navigable {
            button
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.rosetta_surface_selected = idx;
                    this.rosetta_surface_selected_key = Some(row_key);
                    let _ = this.activate_rosetta_row_by_key(row_key, window, cx);
                    cx.stop_propagation();
                }))
                .into_any_element()
        } else {
            button
                .opacity(0.62)
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.rosetta_surface_selected = idx;
                    this.rosetta_surface_selected_key = Some(row_key);
                    this.explain_unavailable_rosetta_row(status, cx);
                    cx.stop_propagation();
                }))
                .into_any_element()
        }
    }

    fn render_rosetta_secondary_action(
        &self,
        row_key: RosettaRowKey,
        action: RosettaSecondaryAction,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = match action {
            RosettaSecondaryAction::Read => "Read",
            RosettaSecondaryAction::Snooze => "Snooze",
            RosettaSecondaryAction::Dismiss => "Dismiss",
        };

        rosetta_text_button(label, ui)
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                match action {
                    RosettaSecondaryAction::Read => this.mark_rosetta_row_read(row_key, cx),
                    RosettaSecondaryAction::Snooze => this.snooze_rosetta_row(row_key, cx),
                    RosettaSecondaryAction::Dismiss => this.dismiss_rosetta_row(row_key, cx),
                }
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    fn render_rosetta_dismiss_action(
        &self,
        row_key: RosettaRowKey,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!(
                "rosetta-action-dismiss-{row_key:?}"
            )))
            .occlude()
            .flex_none()
            .w(px(20.))
            .h(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .text_color(ui.muted)
            .bg(with_alpha(ui.text, 0.06))
            .cursor_pointer()
            .hover(|s| s.bg(with_alpha(ui.text, 0.10)).text_color(ui.text))
            .tooltip(crate::ui_primitives::text_tooltip("Dismiss"))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.dismiss_rosetta_row(row_key, cx);
                cx.stop_propagation();
            }))
            .child(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path("icons/trash.svg")
                    .text_color(ui.muted),
            )
            .into_any_element()
    }

    fn open_rosetta_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.rosetta_surface_allowed() {
            self.reset_rosetta_surface_state();
            cx.notify();
            return;
        }

        let projection = self.rosetta_projection(Instant::now());
        self.rosetta_surface_expanded = true;
        self.rosetta_surface_selected = self
            .rosetta_surface_selected
            .min(projection.rows.len().saturating_sub(1));
        self.rosetta_surface_selected_key = projection
            .rows
            .get(self.rosetta_surface_selected)
            .map(RosettaRow::key);
        self.rosetta_surface_focus.focus(window, cx);
        self.rosetta_surface_pending_focus = false;
        cx.notify();
    }

    fn close_rosetta_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rosetta_surface_expanded = false;
        self.rosetta_surface_selected = 0;
        self.rosetta_surface_selected_key = None;
        self.rosetta_surface_pending_focus = false;
        self.restore_rosetta_focus(window, cx);
        cx.notify();
    }

    fn handle_rosetta_surface_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let projection = self.rosetta_projection(Instant::now());
        let sections = rosetta_sections(&projection.rows);
        let flat_rows = rosetta_flat_rows(&sections);
        let row_count = flat_rows.len();
        match event.keystroke.key.as_str() {
            "escape" => {
                self.close_rosetta_surface(window, cx);
                cx.stop_propagation();
            }
            "up" if row_count > 0 => {
                self.rosetta_surface_selected = self.rosetta_surface_selected.saturating_sub(1);
                self.rosetta_surface_selected_key = flat_rows
                    .get(self.rosetta_surface_selected)
                    .map(|row| row.key());
                cx.notify();
                cx.stop_propagation();
            }
            "down" if row_count > 0 => {
                self.rosetta_surface_selected =
                    (self.rosetta_surface_selected + 1).min(row_count - 1);
                self.rosetta_surface_selected_key = flat_rows
                    .get(self.rosetta_surface_selected)
                    .map(|row| row.key());
                cx.notify();
                cx.stop_propagation();
            }
            "enter" if row_count > 0 => {
                if let Some(key) = self.rosetta_surface_selected_key {
                    let _ = self.activate_rosetta_row_by_key(key, window, cx);
                } else {
                    let selected = self.rosetta_surface_selected.min(row_count - 1);
                    if let Some(row) = flat_rows.get(selected) {
                        let _ = self.activate_rosetta_row_by_key(row.key(), window, cx);
                    }
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn activate_rosetta_row_by_key(
        &mut self,
        key: RosettaRowKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let now = Instant::now();
        let projection = self.rosetta_projection(now);
        self.prune_rosetta_action_state(&projection.rows, now);
        let Some(row) = projection.rows.iter().find(|row| row.key() == key) else {
            self.explain_unavailable_rosetta_row(RosettaTargetStatus::Unavailable, cx);
            return false;
        };
        let status = self.rosetta_target_status(row, cx);
        let Some(target) = row.focus_target else {
            self.explain_unavailable_rosetta_row(status, cx);
            return false;
        };
        if !status.is_navigable() || !self.activate_rosetta_target(target, window, cx) {
            self.explain_unavailable_rosetta_row(status, cx);
            return false;
        }
        true
    }

    fn activate_rosetta_target(
        &mut self,
        target: RosettaFocusTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match target {
            RosettaFocusTarget::WorkspaceSurface { surface_id, .. } => {
                let Some((ws_idx, pane, tab_idx)) =
                    find_pane_by_surface_id(&self.workspaces, surface_id, cx)
                else {
                    cx.notify();
                    return false;
                };
                self.enter_cli_mode(window, cx);
                self.activate_workspace_at(
                    ws_idx,
                    WorkspaceFocusTarget::PaneTab {
                        pane: pane.clone(),
                        tab_idx,
                    },
                    window,
                    cx,
                );
                self.jump_cursor = Some(surface_id);
                self.rosetta_surface_expanded = false;
                self.rosetta_surface_selected = 0;
                self.rosetta_surface_selected_key = None;
                cx.notify();
                true
            }
            RosettaFocusTarget::AgentsThread { thread_id } => {
                let Some(target) = self.agents_thread_target_by_id(thread_id) else {
                    cx.notify();
                    return false;
                };
                self.enter_agents_mode(cx);
                self.select_agents_target(target, cx);
                if let Some(view) = self
                    .agents_view
                    .agents_terminal_view_cache
                    .get(&thread_id)
                    .cloned()
                {
                    view.read(cx).focus_handle(cx).focus(window, cx);
                }
                self.rosetta_surface_expanded = false;
                self.rosetta_surface_selected = 0;
                self.rosetta_surface_selected_key = None;
                cx.notify();
                true
            }
        }
    }

    fn snooze_rosetta_row(&mut self, key: RosettaRowKey, cx: &mut Context<Self>) {
        let until = Instant::now()
            .checked_add(ROSETTA_SNOOZE_DURATION)
            .unwrap_or_else(Instant::now);
        self.rosetta_snoozed_rows.insert(key, until);
        self.show_toast("Rosetta snoozed this waiting row for 10 minutes", cx);
        cx.notify();
    }

    fn mark_rosetta_row_read(&mut self, key: RosettaRowKey, cx: &mut Context<Self>) {
        if matches!(key, RosettaRowKey::RecentEvent { .. }) {
            self.rosetta_read_rows.insert(key);
        }
        cx.notify();
    }

    fn dismiss_rosetta_row(&mut self, key: RosettaRowKey, cx: &mut Context<Self>) {
        match key {
            RosettaRowKey::RecentEvent { sequence } => {
                if self.rosetta_recent_history.dismiss_sequence(sequence) {
                    self.rosetta_surface_selected = self.rosetta_surface_selected.saturating_sub(1);
                    self.rosetta_surface_selected_key = None;
                }
            }
            _ => {
                self.rosetta_dismissed_rows.insert(key);
            }
        }
        cx.notify();
    }

    fn explain_unavailable_rosetta_row(
        &mut self,
        status: RosettaTargetStatus,
        cx: &mut Context<Self>,
    ) {
        let message = match status {
            RosettaTargetStatus::Navigable => "Rosetta target refreshed",
            RosettaTargetStatus::NoPane => "Rosetta has no pane for this row",
            RosettaTargetStatus::Unavailable => "Rosetta target unavailable",
        };
        self.show_toast(message, cx);
    }

    fn restore_rosetta_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            AppMode::Cli | AppMode::Diff => {
                if let Some(ws) = self.workspaces.get_mut(self.active_idx) {
                    ws.focus_first(window, cx);
                }
            }
            AppMode::Agents => {
                let Some(target) = self.current_thread_view_target() else {
                    return;
                };
                let Some(thread_id) = self.thread_for_target(target).map(|thread| thread.id) else {
                    return;
                };
                if let Some(view) = self
                    .agents_view
                    .agents_terminal_view_cache
                    .get(&thread_id)
                    .cloned()
                {
                    view.read(cx).focus_handle(cx).focus(window, cx);
                }
            }
        }
    }

    fn rosetta_target_status(&self, row: &RosettaRow, cx: &Context<Self>) -> RosettaTargetStatus {
        match row.focus_target {
            None => RosettaTargetStatus::NoPane,
            Some(RosettaFocusTarget::WorkspaceSurface { surface_id, .. }) => {
                if find_pane_by_surface_id(&self.workspaces, surface_id, cx).is_some() {
                    RosettaTargetStatus::Navigable
                } else {
                    RosettaTargetStatus::Unavailable
                }
            }
            Some(RosettaFocusTarget::AgentsThread { thread_id }) => {
                if self.agents_thread_target_by_id(thread_id).is_some() {
                    RosettaTargetStatus::Navigable
                } else {
                    RosettaTargetStatus::Unavailable
                }
            }
        }
    }

    fn prune_rosetta_action_state(&mut self, rows: &[RosettaRow], now: Instant) {
        let live_states: HashMap<_, _> = rows.iter().map(|row| (row.key(), row.state)).collect();
        self.rosetta_snoozed_rows.retain(|key, until| {
            *until > now && live_states.get(key) == Some(&RosettaRowState::WaitingForInput)
        });
        self.rosetta_dismissed_rows
            .retain(|key| live_states.get(key) == Some(&RosettaRowState::Errored));
        self.rosetta_read_rows
            .retain(|key| live_states.contains_key(key));
    }

    fn rosetta_compact_rows<'a>(&self, rows: &'a [RosettaRow]) -> Vec<&'a RosettaRow> {
        let snoozed: HashSet<_> = self.rosetta_snoozed_rows.keys().copied().collect();
        rosetta_compact_rows(
            rows,
            &snoozed,
            &self.rosetta_dismissed_rows,
            &self.rosetta_read_rows,
        )
    }

    pub(crate) fn sync_rosetta_config_state(&mut self) {
        self.rosetta_passive_display_enabled = self.cached_config.rosetta_show_passive_enabled();
        if !self.cached_config.rosetta_enabled() || !self.rosetta_mode_enabled() {
            self.reset_rosetta_surface_state();
        }
    }

    pub(crate) fn rosetta_surface_allowed(&self) -> bool {
        self.cached_config.rosetta_enabled()
            && self.settings_section.is_none()
            && self.rosetta_mode_enabled()
    }

    fn rosetta_mode_enabled(&self) -> bool {
        matches!(self.mode, AppMode::Cli | AppMode::Agents)
    }

    pub(crate) fn reset_rosetta_surface_state(&mut self) {
        self.rosetta_surface_expanded = false;
        self.rosetta_surface_selected = 0;
        self.rosetta_surface_selected_key = None;
        self.rosetta_surface_pending_focus = false;
    }
}

pub(crate) fn build_rosetta_projection(
    workspaces: &[Workspace],
    projects: &[Project],
    chats: &[Thread],
    recent_history: &RosettaRecentHistory,
    now: Instant,
) -> RosettaProjection {
    let workspace_sessions = workspace_session_refs(workspaces);
    build_rosetta_projection_from_parts(&workspace_sessions, projects, chats, recent_history, now)
}

fn build_rosetta_projection_from_parts(
    workspace_sessions: &[WorkspaceSessionRef<'_>],
    projects: &[Project],
    chats: &[Thread],
    recent_history: &RosettaRecentHistory,
    now: Instant,
) -> RosettaProjection {
    let mut rows = Vec::new();
    let mut sort_order = 0;

    for source in workspace_sessions {
        rows.push(row_from_workspace_session(*source, now, sort_order));
        sort_order += 1;
    }

    for (project_idx, project) in projects.iter().enumerate() {
        for (thread_idx, thread) in project.threads.iter().enumerate() {
            let Some(state) = RosettaRowState::from_thread_status(thread.status) else {
                continue;
            };
            let target = AgentsTarget::Thread {
                project_idx,
                thread_idx,
            };
            rows.push(row_from_agents_thread(
                thread,
                state,
                target,
                Some(project.title.as_str()),
                now,
                sort_order,
            ));
            sort_order += 1;
        }
    }

    for (chat_idx, chat) in chats.iter().enumerate() {
        let Some(state) = RosettaRowState::from_thread_status(chat.status) else {
            continue;
        };
        rows.push(row_from_agents_thread(
            chat,
            state,
            AgentsTarget::Chat { chat_idx },
            Some("Chat"),
            now,
            sort_order,
        ));
        sort_order += 1;
    }

    for event in recent_history.visible_events(now) {
        rows.push(row_from_recent_event(event, now, sort_order));
        sort_order += 1;
    }

    rank_rows(&mut rows);
    RosettaProjection { rows }
}

fn workspace_session_refs(workspaces: &[Workspace]) -> Vec<WorkspaceSessionRef<'_>> {
    let mut refs = Vec::new();

    for workspace in workspaces {
        let mut sessions: Vec<_> = workspace.agent_sessions.iter().collect();
        sessions.sort_by(|(pid_a, session_a), (pid_b, session_b)| {
            session_a
                .tool
                .display_rank()
                .cmp(&session_b.tool.display_rank())
                .then_with(|| pid_a.cmp(pid_b))
        });

        for (pid, session) in sessions {
            refs.push(WorkspaceSessionRef {
                workspace_id: workspace.id,
                workspace_title: &workspace.title,
                pid: *pid,
                session,
            });
        }
    }

    refs
}

fn row_from_workspace_session(
    source: WorkspaceSessionRef<'_>,
    now: Instant,
    sort_order: usize,
) -> RosettaRow {
    let session = source.session;
    RosettaRow {
        source: RosettaRowSource::WorkspaceSession {
            workspace_id: source.workspace_id,
            pid: source.pid,
        },
        state: RosettaRowState::from_agent_state(&session.state),
        tool: Some(session.tool),
        workspace_title: Some(source.workspace_title.to_string()),
        thread_title: None,
        context: None,
        surface_id: session.surface_id,
        focus_target: session
            .surface_id
            .map(|surface_id| RosettaFocusTarget::WorkspaceSurface {
                workspace_id: source.workspace_id,
                surface_id,
            }),
        message: cap_optional_agent_text(session.message.as_deref()),
        waiting_secs: session
            .waiting_since
            .map(|since| elapsed(now, since).as_secs()),
        last_activity_secs: Some(elapsed(now, session.last_activity).as_secs()),
        active_tool_name: cap_optional_agent_text(session.active_tool_name.as_deref()),
        last_result: cap_optional_agent_text(session.last_result.as_deref()),
        sort_order,
    }
}

fn row_from_agents_thread(
    thread: &Thread,
    state: RosettaRowState,
    target: AgentsTarget,
    context: Option<&str>,
    now: Instant,
    sort_order: usize,
) -> RosettaRow {
    RosettaRow {
        source: RosettaRowSource::AgentsThread {
            target,
            thread_id: thread.id,
        },
        state,
        tool: Some(thread_tool(thread)),
        workspace_title: None,
        thread_title: Some(thread.title.clone()),
        context: context.map(ToOwned::to_owned),
        surface_id: None,
        focus_target: Some(RosettaFocusTarget::AgentsThread {
            thread_id: thread.id,
        }),
        message: cap_optional_agent_text(thread.message.as_deref()),
        waiting_secs: thread
            .waiting_since
            .map(|since| elapsed(now, since).as_secs()),
        last_activity_secs: Some(elapsed(now, thread.last_activity).as_secs()),
        active_tool_name: cap_optional_agent_text(thread.active_tool_name.as_deref()),
        last_result: cap_optional_agent_text(thread.last_result.as_deref()),
        sort_order,
    }
}

fn row_from_recent_event(
    event: &RosettaRecentEvent,
    now: Instant,
    sort_order: usize,
) -> RosettaRow {
    RosettaRow {
        source: RosettaRowSource::RecentEvent {
            sequence: event.sequence,
        },
        state: event.state,
        tool: event.tool,
        workspace_title: event.workspace_title.clone(),
        thread_title: event.thread_title.clone(),
        context: event.context.clone(),
        surface_id: event.surface_id,
        focus_target: event.focus_target,
        message: event.message.clone(),
        waiting_secs: None,
        last_activity_secs: Some(elapsed(now, event.occurred_at).as_secs()),
        active_tool_name: None,
        last_result: event.last_result.clone(),
        sort_order,
    }
}

pub(crate) fn rosetta_recent_event_from_workspace_session(
    workspace_id: u64,
    workspace_title: &str,
    session: &AgentSession,
    state: RosettaRowState,
    occurred_at: Instant,
) -> RosettaRecentEvent {
    let mut event = RosettaRecentEvent::new(state, occurred_at);
    event.tool = Some(session.tool);
    event.workspace_title = Some(workspace_title.to_string());
    event.surface_id = session.surface_id;
    event.focus_target =
        session
            .surface_id
            .map(|surface_id| RosettaFocusTarget::WorkspaceSurface {
                workspace_id,
                surface_id,
            });
    event.message = session.message.clone();
    event.last_result = session.last_result.clone();
    event
}

pub(crate) fn rosetta_recent_event_from_agents_thread(
    thread: &Thread,
    context: Option<&str>,
    state: RosettaRowState,
    occurred_at: Instant,
) -> RosettaRecentEvent {
    let mut event = RosettaRecentEvent::new(state, occurred_at);
    event.tool = Some(thread_tool(thread));
    event.thread_title = Some(thread.title.clone());
    event.context = context.map(ToOwned::to_owned);
    event.focus_target = Some(RosettaFocusTarget::AgentsThread {
        thread_id: thread.id,
    });
    event.message = thread.message.clone();
    event.last_result = thread.last_result.clone();
    event
}

fn rank_rows(rows: &mut [RosettaRow]) {
    rows.sort_by(|a, b| {
        b.state
            .priority_rank()
            .cmp(&a.state.priority_rank())
            .then_with(|| compare_waiting_age(a, b))
            .then_with(|| a.sort_order.cmp(&b.sort_order))
    });
}

fn compare_waiting_age(a: &RosettaRow, b: &RosettaRow) -> std::cmp::Ordering {
    match (a.state, b.state, a.waiting_secs, b.waiting_secs) {
        (
            RosettaRowState::WaitingForInput,
            RosettaRowState::WaitingForInput,
            Some(a_secs),
            Some(b_secs),
        ) => b_secs.cmp(&a_secs),
        _ => std::cmp::Ordering::Equal,
    }
}

fn thread_tool(thread: &Thread) -> TerminalAgent {
    match thread.terminal_agent {
        Some(agent) => agent,
        None => TerminalAgent::from_agent_kind(thread.agent),
    }
}

fn elapsed(now: Instant, then: Instant) -> Duration {
    now.checked_duration_since(then).unwrap_or(Duration::ZERO)
}

fn cap_optional_agent_text(text: Option<&str>) -> Option<String> {
    text.map(cap_agent_text)
}

fn cap_agent_text(text: &str) -> String {
    let without_ansi = strip_ansi_sequences(text);
    let without_bidi = crate::markdown::strip_bidi_zero_width(without_ansi);
    let mut chars = without_bidi.chars().filter_map(|c| match c {
        '\n' | '\r' | '\t' => Some(' '),
        '\u{0000}'..='\u{001f}' | '\u{007f}' | '\u{0080}'..='\u{009f}' => None,
        _ => Some(c),
    });
    let mut capped: String = chars.by_ref().take(ROSETTA_AGENT_TEXT_CAP_CHARS).collect();
    if chars.next().is_some() {
        while capped.chars().count() > ROSETTA_AGENT_TEXT_CAP_CHARS.saturating_sub(3) {
            let _ = capped.pop();
        }
        capped.push_str("...");
    }
    capped
}

fn rosetta_preferred_card_max_width(window: &Window, preferred: f32) -> f32 {
    let viewport_w = f32::from(window.viewport_size().width);
    let available = (viewport_w - (ROSETTA_CARD_SIDE_GUTTER * 2.)).max(240.);
    preferred.min(available)
}

fn strip_ansi_sequences(text: &str) -> String {
    let mut sanitized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\x1b' {
            sanitized.push(c);
            continue;
        }

        match chars.peek().copied() {
            Some('[') => {
                let _ = chars.next();
                for seq in chars.by_ref() {
                    if ('\u{0040}'..='\u{007e}').contains(&seq) {
                        break;
                    }
                }
            }
            Some(']') => {
                let _ = chars.next();
                let mut saw_esc = false;
                for seq in chars.by_ref() {
                    if seq == '\u{0007}' || (saw_esc && seq == '\\') {
                        break;
                    }
                    saw_esc = seq == '\x1b';
                }
            }
            Some(_) => {
                let _ = chars.next();
            }
            None => {}
        }
    }

    sanitized
}

fn rosetta_card_max_width(window: &Window, expanded: bool, summary: &RosettaCompactSummary) -> f32 {
    let preferred = if expanded {
        ROSETTA_EXPANDED_MAX_WIDTH
    } else if summary.passive_only {
        ROSETTA_COMPACT_PASSIVE_MAX_WIDTH
    } else {
        ROSETTA_COMPACT_MAX_WIDTH
    };
    rosetta_preferred_card_max_width(window, preferred)
}

fn rosetta_panel_max_height(window: &Window) -> f32 {
    let viewport_h = f32::from(window.viewport_size().height);
    (viewport_h * ROSETTA_MAX_PANEL_HEIGHT_RATIO)
        .clamp(ROSETTA_MIN_PANEL_HEIGHT, ROSETTA_MAX_PANEL_HEIGHT)
}

fn rosetta_flat_rows<'a>(sections: &'a [RosettaSection<'a>]) -> Vec<&'a RosettaRow> {
    sections
        .iter()
        .flat_map(|section| section.rows.iter().copied())
        .collect()
}

fn rosetta_compact_rows<'a>(
    rows: &'a [RosettaRow],
    snoozed: &HashSet<RosettaRowKey>,
    dismissed: &HashSet<RosettaRowKey>,
    read: &HashSet<RosettaRowKey>,
) -> Vec<&'a RosettaRow> {
    let mut compact_rows: Vec<_> = rows.iter().collect();
    compact_rows.sort_by(|a, b| {
        rosetta_compact_priority(b, snoozed, dismissed, read)
            .cmp(&rosetta_compact_priority(a, snoozed, dismissed, read))
            .then_with(|| a.sort_order.cmp(&b.sort_order))
    });
    compact_rows
        .into_iter()
        .filter(|row| !read.contains(&row.key()))
        .filter(|row| !(dismissed.contains(&row.key()) && row.state == RosettaRowState::Errored))
        .collect()
}

fn rosetta_compact_priority(
    row: &RosettaRow,
    snoozed: &HashSet<RosettaRowKey>,
    dismissed: &HashSet<RosettaRowKey>,
    read: &HashSet<RosettaRowKey>,
) -> u8 {
    if read.contains(&row.key()) {
        return 0;
    }
    if dismissed.contains(&row.key()) && row.state == RosettaRowState::Errored {
        return 0;
    }
    if snoozed.contains(&row.key()) && row.state == RosettaRowState::WaitingForInput {
        return 1;
    }
    row.state.priority_rank()
}

fn rosetta_section_kind(state: RosettaRowState) -> RosettaSectionKind {
    match state {
        RosettaRowState::WaitingForInput => RosettaSectionKind::NeedsInput,
        RosettaRowState::Errored => RosettaSectionKind::Failed,
        RosettaRowState::Stalled => RosettaSectionKind::Stalled,
        RosettaRowState::Thinking => RosettaSectionKind::Running,
        RosettaRowState::Finished => RosettaSectionKind::Recent,
    }
}

fn rosetta_state_label(state: RosettaRowState) -> &'static str {
    match state {
        RosettaRowState::WaitingForInput => "needs response",
        RosettaRowState::Errored => "failed",
        RosettaRowState::Stalled => "stalled",
        RosettaRowState::Thinking => "running",
        RosettaRowState::Finished => "done",
    }
}

fn rosetta_plural_subject(state: RosettaRowState) -> &'static str {
    match state {
        RosettaRowState::WaitingForInput => "need responses",
        RosettaRowState::Errored => "failed",
        RosettaRowState::Stalled => "stalled",
        RosettaRowState::Thinking => "running",
        RosettaRowState::Finished => "done",
    }
}

fn rosetta_primary_action_label(row: &RosettaRow, status: RosettaTargetStatus) -> &'static str {
    match status {
        RosettaTargetStatus::NoPane => "No pane",
        RosettaTargetStatus::Unavailable => "Unavailable",
        RosettaTargetStatus::Navigable => {
            if row.state == RosettaRowState::WaitingForInput {
                "Reply"
            } else {
                "Open"
            }
        }
    }
}

fn rosetta_secondary_actions(row: &RosettaRow, is_read: bool) -> Vec<RosettaSecondaryAction> {
    let mut actions = Vec::new();
    if matches!(row.source, RosettaRowSource::RecentEvent { .. }) && !is_read {
        actions.push(RosettaSecondaryAction::Read);
    }
    match row.state {
        RosettaRowState::WaitingForInput => actions.push(RosettaSecondaryAction::Snooze),
        RosettaRowState::Errored => actions.push(RosettaSecondaryAction::Dismiss),
        RosettaRowState::Finished if matches!(row.source, RosettaRowSource::RecentEvent { .. }) => {
            actions.push(RosettaSecondaryAction::Dismiss);
        }
        RosettaRowState::Stalled | RosettaRowState::Thinking | RosettaRowState::Finished => {}
    }
    actions
}

fn rosetta_agent_label(row: &RosettaRow) -> &'static str {
    row.tool.map(|tool| tool.display_name()).unwrap_or("Agent")
}

fn rosetta_row_title(row: &RosettaRow) -> String {
    row.thread_title
        .as_ref()
        .or(row.workspace_title.as_ref())
        .or(row.context.as_ref())
        .cloned()
        .unwrap_or_else(|| rosetta_agent_label(row).to_string())
}

fn rosetta_row_context(row: &RosettaRow) -> Option<String> {
    let title = rosetta_row_title(row);
    row.context
        .as_ref()
        .filter(|context| context.as_str() != title)
        .cloned()
        .or_else(|| {
            row.workspace_title
                .as_ref()
                .filter(|workspace| workspace.as_str() != title)
                .cloned()
        })
}

fn rosetta_row_detail(row: &RosettaRow) -> Option<String> {
    row.message
        .as_ref()
        .filter(|message| !message.trim().is_empty())
        .cloned()
        .or_else(|| {
            row.last_result
                .as_ref()
                .filter(|result| !result.trim().is_empty())
                .cloned()
        })
        .or_else(|| {
            row.active_tool_name
                .as_ref()
                .filter(|tool| !tool.trim().is_empty())
                .map(|tool| format!("Using {tool}"))
        })
        .or_else(|| {
            (row.state == RosettaRowState::WaitingForInput)
                .then(|| "Needs your response".to_string())
        })
}

fn rosetta_row_time(row: &RosettaRow) -> Option<String> {
    if let Some(secs) = row.waiting_secs {
        return Some(rosetta_duration_label(secs));
    }
    row.last_activity_secs.map(rosetta_duration_label)
}

pub(crate) fn rosetta_duration_label(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h {}m", secs / 3_600, (secs % 3_600) / 60)
    }
}

fn rosetta_status_indicator(
    row_state: RosettaRowState,
    ui: crate::theme::UiColors,
    animation_id: SharedString,
) -> AnyElement {
    let spinner_color = rgb(0xD29922);
    let slot = div()
        .flex_none()
        .w(px(14.))
        .h(px(14.))
        .flex()
        .items_center()
        .justify_center();

    match row_state {
        RosettaRowState::Thinking => slot
            .child(
                svg()
                    .size(px(13.))
                    .flex_none()
                    .path("icons/loader-circle.svg")
                    .text_color(spinner_color)
                    .with_animation(
                        SharedString::from(format!("{animation_id}-spinner")),
                        Animation::new(Duration::from_secs(1)).repeat(),
                        |svg, delta| {
                            svg.with_transformation(Transformation::rotate(percentage(delta)))
                        },
                    ),
            )
            .into_any_element(),
        RosettaRowState::Finished => slot
            .child(
                div()
                    .w(px(14.))
                    .h(px(14.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(0x3FB950))
                    .child(
                        svg()
                            .size(px(9.))
                            .flex_none()
                            .path("icons/check.svg")
                            .text_color(ui.base),
                    ),
            )
            .into_any_element(),
        _ => slot
            .child(
                div()
                    .w(px(7.))
                    .h(px(7.))
                    .rounded_full()
                    .bg(rosetta_status_color(row_state, ui)),
            )
            .into_any_element(),
    }
}

fn render_rosetta_empty_state(ui: crate::theme::UiColors) -> AnyElement {
    div()
        .min_h(px(168.))
        .flex()
        .items_center()
        .justify_center()
        .px(px(24.))
        .py(px(28.))
        .text_center()
        .text_size(px(12.))
        .text_color(ui.muted)
        .child("No active Rosetta items")
        .into_any_element()
}

fn rosetta_label_chip(label: &'static str, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_none()
        .px(px(7.))
        .py(px(2.))
        .rounded(px(6.))
        .bg(with_alpha(ui.text, 0.08))
        .text_size(px(10.))
        .text_color(ui.text)
        .child(label)
        .into_any_element()
}

fn rosetta_attention_badge(ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(5.))
        .bg(rgb(0xFBBF24))
        .text_size(px(10.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(ui.base)
        .child("Needs response")
        .into_any_element()
}

fn rosetta_text_button(
    label: &'static str,
    ui: crate::theme::UiColors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("rosetta-action-{label}")))
        .occlude()
        .px(px(6.))
        .py(px(2.))
        .rounded(px(6.))
        .text_size(px(10.))
        .text_color(ui.muted)
        .bg(with_alpha(ui.text, 0.06))
        .cursor_pointer()
        .hover(|s| s.bg(with_alpha(ui.text, 0.10)).text_color(ui.text))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(label)
}

fn rosetta_status_color(state: RosettaRowState, ui: crate::theme::UiColors) -> gpui::Hsla {
    match state {
        RosettaRowState::WaitingForInput => ui.vc_conflict,
        RosettaRowState::Errored => ui.agent_error,
        RosettaRowState::Stalled => ui.agent_stalled,
        RosettaRowState::Thinking => ui.accent,
        RosettaRowState::Finished => ui.vc_added,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_acp::AgentKind;

    fn session(
        state: AgentState,
        surface_id: Option<u64>,
        now: Instant,
        waiting_secs: Option<u64>,
        last_activity_secs: u64,
    ) -> AgentSession {
        let mut session = AgentSession::new(TerminalAgent::ClaudeCode, state);
        session.surface_id = surface_id;
        session.waiting_since = waiting_secs.map(|secs| now - Duration::from_secs(secs));
        session.last_activity = now - Duration::from_secs(last_activity_secs);
        session
    }

    fn source<'a>(
        workspace_id: u64,
        workspace_title: &'a str,
        pid: u32,
        session: &'a AgentSession,
    ) -> WorkspaceSessionRef<'a> {
        WorkspaceSessionRef {
            workspace_id,
            workspace_title,
            pid,
            session,
        }
    }

    fn projection_from_sessions(
        sessions: &[WorkspaceSessionRef<'_>],
        now: Instant,
    ) -> RosettaProjection {
        build_rosetta_projection_from_parts(
            sessions,
            &[],
            &[],
            &RosettaRecentHistory::default(),
            now,
        )
    }

    fn sample_row(state: RosettaRowState, title: &str, waiting_secs: Option<u64>) -> RosettaRow {
        RosettaRow {
            source: RosettaRowSource::RecentEvent { sequence: 1 },
            state,
            tool: Some(TerminalAgent::Codex),
            workspace_title: Some(title.to_string()),
            thread_title: None,
            context: None,
            surface_id: None,
            focus_target: None,
            message: Some("status detail".to_string()),
            waiting_secs,
            last_activity_secs: Some(3),
            active_tool_name: None,
            last_result: None,
            sort_order: 0,
        }
    }

    #[test]
    fn empty_projection_returns_no_rows() {
        let projection = build_rosetta_projection_from_parts(
            &[],
            &[],
            &[],
            &RosettaRecentHistory::default(),
            Instant::now(),
        );

        assert!(projection.is_empty());
    }

    #[test]
    fn workspace_session_rows_include_available_fields() {
        let now = Instant::now();
        let mut session = session(AgentState::WaitingForInput, Some(77), now, Some(42), 9);
        session.message = Some("Allow cargo test?".to_string());
        session.active_tool_name = Some("Bash".to_string());
        session.last_result = Some("Tests are green".to_string());

        let sources = [source(3, "backend", 1234, &session)];
        let projection = projection_from_sessions(&sources, now);
        let row = &projection.rows[0];

        assert_eq!(row.tool, Some(TerminalAgent::ClaudeCode));
        assert_eq!(row.state, RosettaRowState::WaitingForInput);
        assert_eq!(row.workspace_title.as_deref(), Some("backend"));
        assert_eq!(row.surface_id, Some(77));
        assert_eq!(row.message.as_deref(), Some("Allow cargo test?"));
        assert_eq!(row.waiting_secs, Some(42));
        assert_eq!(row.last_activity_secs, Some(9));
        assert_eq!(row.active_tool_name.as_deref(), Some("Bash"));
        assert_eq!(row.last_result.as_deref(), Some("Tests are green"));
        assert_eq!(
            row.focus_target,
            Some(RosettaFocusTarget::WorkspaceSurface {
                workspace_id: 3,
                surface_id: 77
            })
        );
    }

    #[test]
    fn unresolved_workspace_session_stays_visible_without_focus_target() {
        let now = Instant::now();
        let session = session(AgentState::Thinking, None, now, None, 1);
        let sources = [source(3, "unresolved", 1, &session)];
        let projection = projection_from_sessions(&sources, now);

        assert_eq!(projection.rows.len(), 1);
        assert_eq!(projection.rows[0].surface_id, None);
        assert_eq!(projection.rows[0].focus_target, None);
    }

    #[test]
    fn rows_rank_by_rosetta_salience() {
        let now = Instant::now();
        let finished = session(AgentState::Finished, Some(1), now, None, 1);
        let thinking = session(AgentState::Thinking, Some(2), now, None, 1);
        let stalled = session(AgentState::Stalled, Some(3), now, None, 1);
        let waiting = session(AgentState::WaitingForInput, Some(4), now, Some(5), 1);
        let errored = session(AgentState::Errored, Some(5), now, None, 1);
        let sources = [
            source(1, "w", 1, &finished),
            source(1, "w", 2, &thinking),
            source(1, "w", 3, &stalled),
            source(1, "w", 4, &waiting),
            source(1, "w", 5, &errored),
        ];

        let states: Vec<_> = projection_from_sessions(&sources, now)
            .rows
            .iter()
            .map(|row| row.state)
            .collect();

        assert_eq!(
            states,
            vec![
                RosettaRowState::Errored,
                RosettaRowState::WaitingForInput,
                RosettaRowState::Stalled,
                RosettaRowState::Thinking,
                RosettaRowState::Finished,
            ]
        );
    }

    #[test]
    fn waiting_rows_tie_break_by_longest_wait() {
        let now = Instant::now();
        let younger = session(AgentState::WaitingForInput, Some(1), now, Some(10), 1);
        let older = session(AgentState::WaitingForInput, Some(2), now, Some(90), 1);
        let sources = [source(1, "w", 1, &younger), source(1, "w", 2, &older)];

        let projection = projection_from_sessions(&sources, now);

        assert_eq!(projection.rows[0].surface_id, Some(2));
        assert_eq!(projection.rows[0].waiting_secs, Some(90));
    }

    #[test]
    fn rosetta_sections_use_prd_order() {
        let rows = vec![
            sample_row(RosettaRowState::Thinking, "running", None),
            sample_row(RosettaRowState::Finished, "done", None),
            sample_row(RosettaRowState::WaitingForInput, "waiting", Some(120)),
            sample_row(RosettaRowState::Errored, "failed", None),
            sample_row(RosettaRowState::Stalled, "stalled", None),
        ];

        let kinds: Vec<_> = rosetta_sections(&rows)
            .into_iter()
            .map(|section| section.kind)
            .collect();

        assert_eq!(
            kinds,
            vec![
                RosettaSectionKind::NeedsInput,
                RosettaSectionKind::Failed,
                RosettaSectionKind::Stalled,
                RosettaSectionKind::Running,
                RosettaSectionKind::Recent,
            ]
        );
    }

    #[test]
    fn compact_summary_for_passive_running_is_single_line() {
        let rows = vec![
            sample_row(RosettaRowState::Thinking, "one", None),
            sample_row(RosettaRowState::Thinking, "two", None),
        ];

        let summary = rosetta_compact_summary(&rows).expect("summary");

        assert_eq!(summary.label, "running");
        assert_eq!(summary.title, "2 running");
        assert_eq!(summary.detail, None);
        assert!(summary.passive_only);
    }

    #[test]
    fn rosetta_duration_labels_are_compact() {
        assert_eq!(rosetta_duration_label(9), "9s");
        assert_eq!(rosetta_duration_label(60), "1m");
        assert_eq!(rosetta_duration_label(3_660), "1h 1m");
    }

    #[test]
    fn waiting_rows_render_reply_not_direct_approval_from_freeform_text() {
        let row = sample_row(RosettaRowState::WaitingForInput, "waiting", Some(20));

        assert_eq!(
            rosetta_primary_action_label(&row, RosettaTargetStatus::Navigable),
            "Reply"
        );
    }

    #[test]
    fn waiting_rows_fallback_to_needs_response_copy() {
        let mut row = sample_row(RosettaRowState::WaitingForInput, "waiting", Some(20));
        row.message = None;

        assert_eq!(rosetta_state_label(row.state), "needs response");
        assert_eq!(rosetta_plural_subject(row.state), "need responses");
        assert_eq!(
            rosetta_row_detail(&row).as_deref(),
            Some("Needs your response")
        );
    }

    #[test]
    fn agent_text_is_plain_sanitized_and_capped() {
        let raw = format!(
            "**Approve** [link](https://example.com) \x1b[31mred\x1b[0m \
             \x1b]8;;https://example.com\u{0007}linked\x1b]8;;\u{0007} \
             \u{202E}\u{200B}\u{0008}\u{0085}{}",
            "x".repeat(600)
        );

        let text = cap_agent_text(&raw);

        assert!(text.contains("**Approve**"));
        assert!(text.contains("[link](https://example.com)"));
        assert!(text.contains("red"));
        assert!(!text.contains("\x1b"));
        assert!(!text.contains("[31m"));
        assert!(!text.contains("\u{202E}"));
        assert!(!text.contains("\u{200B}"));
        assert!(!text.chars().any(|c| {
            matches!(
                c,
                '\u{0000}'..='\u{001f}' | '\u{007f}' | '\u{0080}'..='\u{009f}'
            )
        }));
        assert_eq!(text.chars().count(), ROSETTA_AGENT_TEXT_CAP_CHARS);
        assert!(text.ends_with("..."));
    }

    #[test]
    fn workspace_rows_sanitize_message_and_last_result() {
        let now = Instant::now();
        let mut session = session(AgentState::Errored, Some(77), now, None, 1);
        session.message = Some("\x1b[32mClick Approve\u{202E}".to_string());
        session.last_result = Some(format!("{}\x1b[0m", "r".repeat(600)));
        session.active_tool_name = Some("Bash\x1b[31m\u{202E}".to_string());

        let sources = [source(3, "backend", 1234, &session)];
        let projection = projection_from_sessions(&sources, now);
        let row = &projection.rows[0];

        assert_eq!(row.message.as_deref(), Some("Click Approve"));
        assert_eq!(row.active_tool_name.as_deref(), Some("Bash"));
        assert_eq!(
            row.last_result
                .as_deref()
                .map(str::chars)
                .map(Iterator::count),
            Some(ROSETTA_AGENT_TEXT_CAP_CHARS)
        );
        assert!(!row.last_result.as_deref().unwrap_or("").contains("\x1b"));
    }

    #[test]
    fn compact_rows_demote_snoozed_waiting_but_keep_expanded_row_available() {
        let mut waiting = sample_row(RosettaRowState::WaitingForInput, "waiting", Some(300));
        waiting.source = RosettaRowSource::RecentEvent { sequence: 1 };
        let mut running = sample_row(RosettaRowState::Thinking, "running", None);
        running.source = RosettaRowSource::RecentEvent { sequence: 2 };
        let waiting_key = waiting.key();
        let rows = vec![waiting, running];
        let snoozed = HashSet::from([waiting_key]);
        let compact = rosetta_compact_rows(&rows, &snoozed, &HashSet::new(), &HashSet::new());

        assert_eq!(compact[0].state, RosettaRowState::Thinking);
        assert!(rows.iter().any(|row| row.key() == waiting_key));
    }

    #[test]
    fn dismissed_error_sinks_from_compact_without_removing_expanded_row() {
        let mut error = sample_row(RosettaRowState::Errored, "failed", None);
        error.source = RosettaRowSource::RecentEvent { sequence: 1 };
        let mut running = sample_row(RosettaRowState::Thinking, "running", None);
        running.source = RosettaRowSource::RecentEvent { sequence: 2 };
        let error_key = error.key();
        let rows = vec![error, running];
        let dismissed = HashSet::from([error_key]);
        let compact = rosetta_compact_rows(&rows, &HashSet::new(), &dismissed, &HashSet::new());

        assert_eq!(compact[0].state, RosettaRowState::Thinking);
        assert!(rows.iter().any(|row| row.key() == error_key));
    }

    #[test]
    fn read_recent_rows_leave_expanded_projection_but_leave_compact() {
        let mut finished = sample_row(RosettaRowState::Finished, "done", None);
        finished.source = RosettaRowSource::RecentEvent { sequence: 1 };
        let finished_key = finished.key();
        let mut running = sample_row(RosettaRowState::Thinking, "running", None);
        running.source = RosettaRowSource::RecentEvent { sequence: 2 };
        let rows = vec![finished, running];
        let read = HashSet::from([finished_key]);
        let compact = rosetta_compact_rows(&rows, &HashSet::new(), &HashSet::new(), &read);

        assert_eq!(compact.len(), 1);
        assert_eq!(compact[0].state, RosettaRowState::Thinking);
        assert!(rows.iter().any(|row| row.key() == finished_key));
    }

    #[test]
    fn recent_history_can_dismiss_finished_event_by_sequence() {
        let now = Instant::now();
        let mut history = RosettaRecentHistory::default();
        history.push(RosettaRecentEvent::new(RosettaRowState::Finished, now));
        let projection = build_rosetta_projection_from_parts(&[], &[], &[], &history, now);
        let sequence = match projection.rows[0].source {
            RosettaRowSource::RecentEvent { sequence } => sequence,
            _ => unreachable!("projection row should be recent"),
        };

        assert!(history.dismiss_sequence(sequence));
        assert!(history.visible_events(now).next().is_none());
    }

    #[test]
    fn recent_history_replaces_finished_events_for_same_target_only() {
        let now = Instant::now();
        let target = RosettaFocusTarget::WorkspaceSurface {
            workspace_id: 3,
            surface_id: 77,
        };
        let other_target = RosettaFocusTarget::WorkspaceSurface {
            workspace_id: 3,
            surface_id: 88,
        };
        let mut history = RosettaRecentHistory::default();
        let mut old_finished = RosettaRecentEvent::new(RosettaRowState::Finished, now);
        old_finished.focus_target = Some(target);
        old_finished.last_result = Some("old".to_string());
        let mut other_finished = RosettaRecentEvent::new(RosettaRowState::Finished, now);
        other_finished.focus_target = Some(other_target);
        let mut new_finished = RosettaRecentEvent::new(RosettaRowState::Finished, now);
        new_finished.focus_target = Some(target);
        new_finished.last_result = Some("new".to_string());
        history.push(old_finished);
        history.push(other_finished);
        history.push(new_finished);

        let events: Vec<_> = history.visible_events(now).collect();
        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|event| {
            event.state == RosettaRowState::Finished
                && event.focus_target == Some(target)
                && event.last_result.as_deref() == Some("new")
        }));
        assert!(events.iter().any(|event| {
            event.state == RosettaRowState::Finished && event.focus_target == Some(other_target)
        }));
    }

    #[test]
    fn recent_history_clears_finished_when_same_target_changes_state() {
        let now = Instant::now();
        let target = RosettaFocusTarget::WorkspaceSurface {
            workspace_id: 3,
            surface_id: 77,
        };
        let mut history = RosettaRecentHistory::default();
        let mut finished = RosettaRecentEvent::new(RosettaRowState::Finished, now);
        finished.focus_target = Some(target);
        let mut errored = RosettaRecentEvent::new(RosettaRowState::Errored, now);
        errored.focus_target = Some(target);

        history.push(finished);
        history.push(errored);

        let events: Vec<_> = history.visible_events(now).collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, RosettaRowState::Errored);
        assert_eq!(events[0].focus_target, Some(target));
    }

    #[test]
    fn agents_threads_project_waiting_rows_include_context_and_target() {
        let now = Instant::now();
        let mut project = Project::new("paneflow-web", "C:/dev/paneflow-web");
        let mut thread =
            Thread::new_terminal("Codex polish", &project.cwd, Some(TerminalAgent::Codex));
        thread.status = ThreadStatus::WaitingForInput;
        thread.message = Some("Approve cargo test?".to_string());
        thread.waiting_since = Some(now - Duration::from_secs(33));
        thread.last_activity = now - Duration::from_secs(7);
        thread.active_tool_name = Some("Bash".to_string());
        thread.last_result = Some("Previous turn".to_string());
        let thread_id = thread.id;
        project.threads.push(thread);

        let projection = build_rosetta_projection_from_parts(
            &[],
            &[project],
            &[],
            &RosettaRecentHistory::default(),
            now,
        );
        let row = &projection.rows[0];

        assert_eq!(row.state, RosettaRowState::WaitingForInput);
        assert_eq!(row.tool, Some(TerminalAgent::Codex));
        assert_eq!(row.thread_title.as_deref(), Some("Codex polish"));
        assert_eq!(row.context.as_deref(), Some("paneflow-web"));
        assert_eq!(
            row.focus_target,
            Some(RosettaFocusTarget::AgentsThread { thread_id })
        );
        assert_eq!(row.message.as_deref(), Some("Approve cargo test?"));
        assert_eq!(row.waiting_secs, Some(33));
        assert_eq!(row.last_activity_secs, Some(7));
        assert_eq!(row.active_tool_name.as_deref(), Some("Bash"));
        assert_eq!(row.last_result.as_deref(), Some("Previous turn"));
        assert_eq!(
            row.source,
            RosettaRowSource::AgentsThread {
                target: AgentsTarget::Thread {
                    project_idx: 0,
                    thread_idx: 0,
                },
                thread_id,
            }
        );
    }

    #[test]
    fn agents_thread_failed_ranks_above_waiting_and_thinking() {
        let mut project = Project::new("p", "/tmp");
        let mut thinking = Thread::new_terminal("thinking", &project.cwd, None);
        thinking.status = ThreadStatus::Thinking;
        let mut waiting = Thread::new_terminal("waiting", &project.cwd, None);
        waiting.status = ThreadStatus::WaitingForInput;
        let mut failed = Thread::new_terminal("failed", &project.cwd, None);
        failed.status = ThreadStatus::Failed;
        project.threads = vec![thinking, waiting, failed];

        let states: Vec<_> = build_rosetta_projection_from_parts(
            &[],
            &[project],
            &[],
            &RosettaRecentHistory::default(),
            Instant::now(),
        )
        .rows
        .iter()
        .map(|row| row.state)
        .collect();

        assert_eq!(
            states,
            vec![
                RosettaRowState::Errored,
                RosettaRowState::WaitingForInput,
                RosettaRowState::Thinking,
            ]
        );
    }

    #[test]
    fn idle_agents_threads_are_omitted() {
        let mut project = Project::new("p", "/tmp");
        project
            .threads
            .push(Thread::new_terminal("idle", &project.cwd, None));

        let projection = build_rosetta_projection_from_parts(
            &[],
            &[project],
            &[],
            &RosettaRecentHistory::default(),
            Instant::now(),
        );

        assert!(projection.rows.is_empty());
    }

    #[test]
    fn stalled_is_not_fabricated_from_agents_thread_status() {
        assert_eq!(
            RosettaRowState::from_thread_status(ThreadStatus::Thinking),
            Some(RosettaRowState::Thinking)
        );
        assert_eq!(
            ThreadStatus::from_agent_state(AgentState::Stalled),
            ThreadStatus::Thinking
        );
    }

    #[test]
    fn chats_are_projected_with_chat_context() {
        let mut chat =
            Thread::new_terminal("Ask Codex", "C:/Users/Arthur", Some(TerminalAgent::Codex));
        chat.status = ThreadStatus::Thinking;
        let thread_id = chat.id;

        let projection = build_rosetta_projection_from_parts(
            &[],
            &[],
            &[chat],
            &RosettaRecentHistory::default(),
            Instant::now(),
        );

        assert_eq!(projection.rows[0].context.as_deref(), Some("Chat"));
        assert_eq!(
            projection.rows[0].focus_target,
            Some(RosettaFocusTarget::AgentsThread { thread_id })
        );
    }

    #[test]
    fn recent_history_starts_empty() {
        let history = RosettaRecentHistory::default();

        assert_eq!(history.len(), 0);
        assert!(history.visible_events(Instant::now()).next().is_none());
    }

    #[test]
    fn recent_history_caps_message_and_rows() {
        let now = Instant::now();
        let mut history = RosettaRecentHistory::default();

        for idx in 0..30 {
            let mut event =
                RosettaRecentEvent::new(RosettaRowState::Finished, now - Duration::from_secs(idx));
            event.message = Some("x".repeat(600));
            history.push(event);
        }

        assert_eq!(history.len(), ROSETTA_RECENT_EVENT_CAP);
        let projection = build_rosetta_projection_from_parts(&[], &[], &[], &history, now);

        assert_eq!(projection.rows.len(), ROSETTA_RECENT_EVENT_CAP);
        assert_eq!(
            projection.rows[0]
                .message
                .as_deref()
                .map(str::chars)
                .map(Iterator::count),
            Some(ROSETTA_AGENT_TEXT_CAP_CHARS)
        );
        assert_eq!(
            projection
                .rows
                .iter()
                .filter_map(|row| match row.source {
                    RosettaRowSource::RecentEvent { sequence } => Some(sequence),
                    _ => None,
                })
                .min(),
            Some(5)
        );
    }

    #[test]
    fn recent_history_omits_events_after_retention_window() {
        let now = Instant::now();
        let mut history = RosettaRecentHistory::default();
        history.push(RosettaRecentEvent::new(
            RosettaRowState::Finished,
            now - ROSETTA_RECENT_EVENT_RETENTION - Duration::from_secs(1),
        ));
        history.push(RosettaRecentEvent::new(
            RosettaRowState::Finished,
            now - Duration::from_secs(10),
        ));

        let projection = build_rosetta_projection_from_parts(&[], &[], &[], &history, now);

        assert_eq!(projection.rows.len(), 1);
    }

    #[test]
    fn workspace_recent_event_snapshots_session_fields_and_caps_on_push() {
        let now = Instant::now();
        let mut session = session(AgentState::Finished, Some(77), now, None, 3);
        session.last_result = Some("x".repeat(600));

        let mut history = RosettaRecentHistory::default();
        history.push(rosetta_recent_event_from_workspace_session(
            3,
            "backend",
            &session,
            RosettaRowState::Finished,
            now,
        ));
        let projection = build_rosetta_projection_from_parts(&[], &[], &[], &history, now);
        let row = &projection.rows[0];

        assert_eq!(row.state, RosettaRowState::Finished);
        assert_eq!(row.workspace_title.as_deref(), Some("backend"));
        assert_eq!(row.surface_id, Some(77));
        assert_eq!(
            row.focus_target,
            Some(RosettaFocusTarget::WorkspaceSurface {
                workspace_id: 3,
                surface_id: 77
            })
        );
        assert_eq!(
            row.last_result
                .as_deref()
                .map(str::chars)
                .map(Iterator::count),
            Some(ROSETTA_AGENT_TEXT_CAP_CHARS)
        );
    }

    #[test]
    fn agents_recent_event_snapshots_thread_context_and_target() {
        let now = Instant::now();
        let thread = Thread::new_terminal(
            "Codex polish",
            "C:/dev/paneflow",
            Some(TerminalAgent::Codex),
        );
        let event = rosetta_recent_event_from_agents_thread(
            &thread,
            Some("paneflow"),
            RosettaRowState::Finished,
            now,
        );

        assert_eq!(event.state, RosettaRowState::Finished);
        assert_eq!(event.tool, Some(TerminalAgent::Codex));
        assert_eq!(event.thread_title.as_deref(), Some("Codex polish"));
        assert_eq!(event.context.as_deref(), Some("paneflow"));
        assert_eq!(
            event.focus_target,
            Some(RosettaFocusTarget::AgentsThread {
                thread_id: thread.id
            })
        );
    }

    #[test]
    fn thread_tool_falls_back_to_legacy_agent_kind() {
        let thread = Thread::new("legacy", AgentKind::Codex, "/tmp");

        assert_eq!(thread_tool(&thread), TerminalAgent::Codex);
    }
}
