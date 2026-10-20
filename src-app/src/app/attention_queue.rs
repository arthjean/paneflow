use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, SharedString, Styled, Window, deferred, div, prelude::*, px,
};

use paneflow_config::schema::SpaceId;

use crate::PaneFlowApp;
use crate::ai_types::AgentState;
use crate::app::declared_status::surface_has_agent;
use crate::app::ipc_handler::find_pane_by_surface_id;
use crate::app::workspace_ops::WorkspaceFocusTarget;
use crate::terminal::view::conversation::{ALLOW_WRITE_LABEL, DENY_WRITE_LABEL};
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};
use crate::workspace::Workspace;

pub(crate) struct QueueRow {
    pub(crate) surface_id: Option<u64>,
    pub(crate) workspace_idx: Option<usize>,
    pub(crate) ws_title: String,
    pub(crate) tool_label: SharedString,
    pub(crate) message: Option<String>,
    pub(crate) waiting_secs: u64,
    pub(crate) write_request: Option<u64>,
    pub(crate) errored: bool,
}

pub(crate) const WRITE_REQUEST_LABEL: &str = "Write request";

pub(crate) fn sort_rows(rows: &mut [QueueRow]) {
    rows.sort_by(|a, b| {
        b.surface_id
            .is_some()
            .cmp(&a.surface_id.is_some())
            .then(b.waiting_secs.cmp(&a.waiting_secs))
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttentionLevel {
    Input,
    Error,
    Finished,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SpaceAttention {
    pub(crate) input: usize,
    pub(crate) errors: usize,
    pub(crate) finished: usize,
}

impl SpaceAttention {
    pub(crate) fn total(&self) -> usize {
        self.input + self.errors + self.finished
    }

    pub(crate) fn most_urgent(&self) -> Option<AttentionLevel> {
        [
            (AttentionLevel::Input, self.input),
            (AttentionLevel::Error, self.errors),
            (AttentionLevel::Finished, self.finished),
        ]
        .into_iter()
        .find(|(_, count)| *count > 0)
        .map(|(level, _)| level)
    }
}

pub(crate) fn space_attention(
    workspaces: &[Workspace],
    rows: &[QueueRow],
) -> HashMap<Option<SpaceId>, SpaceAttention> {
    let mut spaces: HashMap<Option<SpaceId>, SpaceAttention> = HashMap::new();
    for row in rows {
        let Some(workspace) = row.workspace_idx.and_then(|idx| workspaces.get(idx)) else {
            continue;
        };
        let counts = spaces.entry(workspace.space.clone()).or_default();
        if row.errored {
            counts.errors += 1;
        } else {
            counts.input += 1;
        }
    }
    for workspace in workspaces.iter().filter(|workspace| !workspace.muted) {
        let finished = workspace.agent_completion_notification.unread_count();
        if finished > 0 {
            spaces.entry(workspace.space.clone()).or_default().finished += finished;
        }
    }
    spaces
}

pub(crate) fn queue_row_origin(space: Option<&str>, workspace_title: &str) -> String {
    match space {
        Some(space) if workspace_title.is_empty() => space.to_string(),
        Some(space) => format!("{space} · {workspace_title}"),
        None => workspace_title.to_string(),
    }
}

pub(crate) fn wait_label(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h {}m", secs / 3_600, (secs % 3_600) / 60)
    }
}

impl PaneFlowApp {
    pub(crate) fn attention_queue_rows(&self, cx: &Context<Self>) -> Vec<QueueRow> {
        let mut rows = self.attention_items(cx);
        sort_rows(&mut rows);
        rows
    }

    pub(crate) fn space_attention(
        &self,
        cx: &Context<Self>,
    ) -> HashMap<Option<SpaceId>, SpaceAttention> {
        space_attention(&self.workspaces, &self.attention_items(cx))
    }

    fn attention_items(&self, cx: &Context<Self>) -> Vec<QueueRow> {
        let mut live_surfaces: HashSet<u64> = HashSet::new();
        let mut session_surfaces = HashMap::new();
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            for pane in ws.collect_panes() {
                for t in pane.read(cx).terminals() {
                    let surface_id = t.entity_id().as_u64();
                    live_surfaces.insert(surface_id);
                    session_surfaces
                        .insert(t.read(cx).terminal.session_id.clone(), (surface_id, ws_idx));
                }
            }
        }
        let mut rows = Vec::new();
        for pending in &self.write_approvals.pending {
            let place = session_surfaces.get(&pending.target);
            let workspace_idx = place.map(|(_, ws_idx)| *ws_idx);
            rows.push(QueueRow {
                surface_id: place.map(|(surface_id, _)| *surface_id),
                workspace_idx,
                ws_title: workspace_idx
                    .map(|idx| self.workspaces[idx].title.clone())
                    .unwrap_or_default(),
                tool_label: SharedString::new_static(WRITE_REQUEST_LABEL),
                message: Some(pending.request().message()),
                waiting_secs: pending.asked_at.elapsed().as_secs(),
                write_request: Some(pending.id),
                errored: false,
            });
        }
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            for session in ws.agent_sessions.values() {
                if session.state != AgentState::WaitingForInput {
                    continue;
                }
                let surface_id = match session.surface_id {
                    Some(sid) if live_surfaces.contains(&sid) => Some(sid),
                    Some(_) => continue,
                    None => None,
                };
                rows.push(QueueRow {
                    surface_id,
                    workspace_idx: Some(ws_idx),
                    ws_title: ws.title.clone(),
                    tool_label: SharedString::new_static(session.tool.display_name()),
                    message: session.message.clone(),
                    waiting_secs: session
                        .waiting_since
                        .map(|t| t.elapsed().as_secs())
                        .unwrap_or(0),
                    write_request: None,
                    errored: false,
                });
            }
        }
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            for pane in ws.collect_panes() {
                for terminal in pane.read(cx).terminals() {
                    let surface_id = terminal.entity_id().as_u64();
                    let view = terminal.read(cx);
                    let row = self.host_agent_row(&view.terminal.session_id);
                    if surface_has_agent(ws, surface_id, row) {
                        continue;
                    }
                    let Some(entry) = self.host_agents.declared_watch.queue_entry(
                        surface_id,
                        view.terminal.declared_status.as_ref(),
                        &crate::pane::Pane::terminal_surface_title(terminal, cx),
                    ) else {
                        continue;
                    };
                    rows.push(QueueRow {
                        surface_id: Some(surface_id),
                        workspace_idx: Some(ws_idx),
                        ws_title: ws.title.clone(),
                        tool_label: entry.label.into(),
                        message: entry.message,
                        waiting_secs: self
                            .declared_since(surface_id)
                            .map(|since| since.elapsed().as_secs())
                            .unwrap_or(0),
                        write_request: None,
                        errored: entry.errored,
                    });
                }
            }
        }
        rows
    }

    pub(crate) fn handle_open_attention_queue(
        &mut self,
        _: &crate::OpenAttentionQueue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.attention_queue_open {
            self.close_attention_queue_and_restore_focus(window, cx);
            return;
        }
        self.attention_queue_return = crate::FocusReturn::capture(window, cx);
        self.attention_queue_open = true;
        self.attention_queue_selected = 0;
        self.attention_queue_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn close_attention_queue(&mut self, cx: &mut Context<Self>) {
        self.attention_queue_open = false;
        self.attention_queue_selected = 0;
        self.attention_queue_return = crate::FocusReturn::default();
        cx.notify();
    }

    pub(crate) fn close_attention_queue_and_restore_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let origin = std::mem::take(&mut self.attention_queue_return);
        self.close_attention_queue(cx);
        self.return_focus(&origin, window, cx);
    }

    pub(crate) fn attention_queue_activate(
        &mut self,
        surface_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(loc) = find_pane_by_surface_id(&self.workspaces, surface_id, cx) else {
            cx.notify();
            return;
        };
        let (ws_idx, pane) = (loc.workspace_idx, loc.pane);
        self.activate_workspace_at(ws_idx, WorkspaceFocusTarget::Pane { pane }, window, cx);
        self.jump_cursor = Some(surface_id);
        self.close_attention_queue(cx);
    }

    pub(crate) fn handle_attention_queue_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let rows = self.attention_queue_rows(cx);
        let len = rows.len();
        match key {
            "escape" => self.close_attention_queue_and_restore_focus(window, cx),
            "enter" if len > 0 => {
                let idx = self.attention_queue_selected.min(len - 1);
                if let Some(sid) = rows[idx].surface_id {
                    self.attention_queue_activate(sid, window, cx);
                }
            }
            "a" | "d" if len > 0 => {
                let idx = self.attention_queue_selected.min(len - 1);
                if let Some(id) = rows[idx].write_request {
                    self.decide_write_request(id, key == "a", cx);
                }
            }
            "up" if len > 0 && self.attention_queue_selected > 0 => {
                self.attention_queue_selected -= 1;
                cx.notify();
            }
            "down" if len > 0 && self.attention_queue_selected + 1 < len => {
                self.attention_queue_selected += 1;
                cx.notify();
            }
            _ => {}
        }
    }

    pub(crate) fn render_attention_queue(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let rows = self.attention_queue_rows(cx);
        let selected = self
            .attention_queue_selected
            .min(rows.len().saturating_sub(1));

        let mut card = div()
            .id("attention-queue")
            .occlude()
            .track_focus(&self.attention_queue_focus)
            .on_key_down(cx.listener(Self::handle_attention_queue_key_down))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.close_attention_queue_and_restore_focus(window, cx);
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .w(px(560.))
            .flex()
            .flex_col()
            .bg(ui.overlay)
            .border_1()
            .border_color(ui.border)
            .rounded(px(8.))
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .px(px(14.))
                    .py(px(10.))
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(ui.text)
                    .border_b_1()
                    .border_color(ui.border)
                    .child("Waiting for input"),
            );

        if rows.is_empty() {
            card = card.child(
                div()
                    .px(px(14.))
                    .py(px(14.))
                    .text_size(px(12.))
                    .text_color(ui.muted)
                    .child("No agent is waiting for input"),
            );
        } else {
            for (idx, row) in rows.iter().enumerate() {
                let is_selected = idx == selected;
                let navigable = row.surface_id.is_some();
                let sid = row.surface_id;
                let row_id: SharedString = sid
                    .map(|surface_id| format!("attention-row-{surface_id}"))
                    .unwrap_or_else(|| format!("attention-row-unmapped-{idx}"))
                    .into();
                let origin = queue_row_origin(
                    row.workspace_idx
                        .and_then(|idx| self.workspaces.get(idx))
                        .and_then(|ws| {
                            crate::app::spaces::declared_space_label(&self.cached_config, ws)
                        }),
                    &row.ws_title,
                );
                let resting_background = if is_selected {
                    ui.subtle
                } else {
                    ui.subtle.opacity(0.0)
                };
                let question: SharedString = row
                    .message
                    .clone()
                    .unwrap_or_else(|| {
                        if row.errored { "Failed" } else { "Needs input" }.to_string()
                    })
                    .into();
                let mut r = div()
                    .id(row_id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(7.))
                    .text_size(px(12.))
                    .bg(resting_background)
                    .child(div().flex_none().w(px(6.)).h(px(6.)).rounded_full().bg(
                        if row.errored {
                            ui.agent_error
                        } else {
                            ui.vc_conflict
                        },
                    ))
                    .child(
                        div()
                            .flex_none()
                            .text_color(ui.text)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(row.tool_label.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(160.))
                            .truncate()
                            .text_color(ui.muted)
                            .child(origin),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(if navigable { ui.text } else { ui.muted })
                            .child(question),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(ui.muted)
                            .child(wait_label(row.waiting_secs)),
                    );
                if let Some(id) = row.write_request {
                    r = r
                        .child(crate::settings::components::secondary_button(
                            SharedString::from(format!("attention-write-allow-{id}")),
                            ALLOW_WRITE_LABEL,
                            ui,
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.decide_write_request(id, true, cx);
                                cx.stop_propagation();
                            }),
                        ))
                        .child(crate::settings::components::secondary_button(
                            SharedString::from(format!("attention-write-deny-{id}")),
                            DENY_WRITE_LABEL,
                            ui,
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.decide_write_request(id, false, cx);
                                cx.stop_propagation();
                            }),
                        ));
                }
                if navigable {
                    let r = r
                        .cursor_pointer()
                        .animated_hover(move |style, delta| {
                            style.bg(lerp_color(resting_background, ui.subtle, delta));
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            if let Some(sid) = sid {
                                this.attention_queue_activate(sid, window, cx);
                            }
                            cx.stop_propagation();
                        }));
                    card = card.child(r);
                } else {
                    r = r.child(
                        div()
                            .flex_none()
                            .text_size(px(10.))
                            .text_color(ui.muted)
                            .child("no pane"),
                    );
                    card = card.child(r);
                }
            }
            card = card.child(
                div()
                    .px(px(14.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(ui.border)
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .child(if rows.iter().any(|row| row.write_request.is_some()) {
                        "Enter focuses the pane · A allows · D denies a write request · Esc closes"
                    } else {
                        "Enter focuses the pane · Esc closes"
                    }),
            );
        }

        deferred(
            div()
                .id("attention-queue-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(96.))
                .bg(gpui::hsla(0., 0., 0., 0.4))
                .child(card),
        )
        .with_priority(6)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(surface_id: Option<u64>, waiting_secs: u64) -> QueueRow {
        QueueRow {
            surface_id,
            workspace_idx: None,
            ws_title: String::new(),
            tool_label: SharedString::new_static("Claude"),
            message: None,
            waiting_secs,
            write_request: None,
            errored: false,
        }
    }

    #[test]
    fn sorts_longest_wait_first_unmapped_last() {
        let mut rows = vec![
            row(Some(1), 10),
            row(None, 9_999),
            row(Some(2), 300),
            row(Some(3), 60),
        ];
        sort_rows(&mut rows);
        let order: Vec<Option<u64>> = rows.iter().map(|r| r.surface_id).collect();
        assert_eq!(order, vec![Some(2), Some(3), Some(1), None]);
    }

    fn space(n: u8) -> SpaceId {
        SpaceId::parse(&format!("550e8400-e29b-41d4-a716-4466554400{n:02}")).unwrap()
    }

    fn workspace(id: u64, space: Option<SpaceId>) -> Workspace {
        let mut workspace =
            Workspace::empty_with_cwd_and_id(id, format!("ws{id}"), std::path::PathBuf::new());
        workspace.space = space;
        workspace
    }

    fn located(workspace_idx: Option<usize>, errored: bool) -> QueueRow {
        QueueRow {
            workspace_idx,
            errored,
            ..row(Some(1), 0)
        }
    }

    #[test]
    fn each_space_counts_its_waiting_failed_and_unseen_finished_agents() {
        let mut workspaces = vec![
            workspace(1, None),
            workspace(2, Some(space(1))),
            workspace(3, Some(space(1))),
            workspace(4, Some(space(2))),
        ];
        workspaces[2]
            .agent_completion_notification
            .record_finished(false, Some(30));
        workspaces[3]
            .agent_completion_notification
            .record_finished(false, Some(40));
        let rows = vec![
            located(Some(1), false),
            located(Some(2), true),
            located(Some(0), true),
            located(None, false),
        ];

        let counts = space_attention(&workspaces, &rows);

        let work = counts[&Some(space(1))];
        assert_eq!(
            work,
            SpaceAttention {
                input: 1,
                errors: 1,
                finished: 1
            }
        );
        assert_eq!(work.total(), 3);
        assert_eq!(work.most_urgent(), Some(AttentionLevel::Input));
        assert_eq!(counts[&None].most_urgent(), Some(AttentionLevel::Error));
        assert_eq!(
            counts[&Some(space(2))].most_urgent(),
            Some(AttentionLevel::Finished)
        );
        assert_eq!(SpaceAttention::default().most_urgent(), None);
    }

    #[test]
    fn a_finish_in_a_muted_workspace_does_not_count() {
        let mut workspaces = vec![workspace(1, None), workspace(2, Some(space(1)))];
        workspaces[1].muted = true;
        workspaces[1]
            .agent_completion_notification
            .record_finished(false, Some(20));

        let counts = space_attention(&workspaces, &[]);

        assert_eq!(counts.get(&Some(space(1))), None);
    }

    #[test]
    fn a_queue_row_names_its_space_before_its_workspace() {
        assert_eq!(queue_row_origin(Some("Work"), "api"), "Work · api");
        assert_eq!(queue_row_origin(Some("Work"), ""), "Work");
        assert_eq!(queue_row_origin(None, "api"), "api");
    }

    #[gpui::test]
    fn activating_a_hidden_agent_from_the_queue_switches_to_its_space(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::pane::Pane;
        use crate::terminal::TerminalView;
        use gpui::AppContext as _;

        let cx = cx.add_empty_window();
        let mut terminals = Vec::new();
        let workspaces: Vec<Workspace> = [(1, None), (2, Some(space(1)))]
            .into_iter()
            .map(|(id, space)| {
                let terminal = cx.new(|cx| TerminalView::display_only_for_test(id, cx));
                terminals.push(terminal.entity_id().as_u64());
                let pane = cx.new(|cx| Pane::new(terminal, id, cx));
                let mut workspace = Workspace::with_id(id, format!("ws{id}"), pane);
                workspace.space = space;
                workspace
            })
            .collect();
        let mut active_idx = 0;
        let mut active_space = None;
        let mut memory = crate::app::spaces::SpaceMemory::new();
        let closed = cx.new(|cx| TerminalView::display_only_for_test(3, cx));
        let closed = closed.entity_id().as_u64();

        let gone = cx.update(|_, cx| find_pane_by_surface_id(&workspaces, closed, cx));
        assert!(gone.is_none());
        assert_eq!((active_idx, active_space.clone()), (0, None));

        let destination = cx
            .update(|_, cx| find_pane_by_surface_id(&workspaces, terminals[1], cx))
            .expect("the waiting agent's pane is found");
        assert!(crate::app::spaces::point_at(
            &workspaces,
            &mut active_idx,
            &mut active_space,
            &mut memory,
            destination.workspace_idx,
        ));

        assert_eq!(active_space, Some(space(1)));
        assert_eq!(active_idx, 1);
        let focused = cx.update(|_, cx| {
            destination
                .pane
                .read(cx)
                .terminals()
                .next()
                .map(|terminal| terminal.entity_id().as_u64())
        });
        assert_eq!(focused, Some(terminals[1]));
    }

    #[test]
    fn wait_labels_are_compact() {
        assert_eq!(wait_label(0), "0s");
        assert_eq!(wait_label(59), "59s");
        assert_eq!(wait_label(60), "1m");
        assert_eq!(wait_label(3_599), "59m");
        assert_eq!(wait_label(4_380), "1h 13m");
    }
}
