use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, SharedString, Styled, Window, deferred, div, prelude::*, px,
};

use crate::PaneFlowApp;
use crate::ai_types::AgentState;
use crate::app::declared_status::surface_has_agent;
use crate::app::ipc_handler::find_pane_by_surface_id;
use crate::app::workspace_ops::WorkspaceFocusTarget;
use crate::terminal::view::conversation::{ALLOW_WRITE_LABEL, DENY_WRITE_LABEL};
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

pub(crate) struct QueueRow {
    pub(crate) surface_id: Option<u64>,
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
        let mut live_surfaces: HashSet<u64> = HashSet::new();
        let mut session_surfaces = HashMap::new();
        for ws in &self.workspaces {
            for pane in ws.collect_panes() {
                for t in pane.read(cx).terminals() {
                    let surface_id = t.entity_id().as_u64();
                    live_surfaces.insert(surface_id);
                    session_surfaces.insert(
                        t.read(cx).terminal.session_id.clone(),
                        (surface_id, ws.title.clone()),
                    );
                }
            }
        }
        let mut rows = Vec::new();
        for pending in &self.write_approvals.pending {
            let place = session_surfaces.get(&pending.target);
            rows.push(QueueRow {
                surface_id: place.map(|(surface_id, _)| *surface_id),
                ws_title: place.map(|(_, title)| title.clone()).unwrap_or_default(),
                tool_label: SharedString::new_static(WRITE_REQUEST_LABEL),
                message: Some(pending.request().message()),
                waiting_secs: pending.asked_at.elapsed().as_secs(),
                write_request: Some(pending.id),
                errored: false,
            });
        }
        for ws in &self.workspaces {
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
        for ws in &self.workspaces {
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
        sort_rows(&mut rows);
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
                            .max_w(px(120.))
                            .truncate()
                            .text_color(ui.muted)
                            .child(row.ws_title.clone()),
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

    #[test]
    fn wait_labels_are_compact() {
        assert_eq!(wait_label(0), "0s");
        assert_eq!(wait_label(59), "59s");
        assert_eq!(wait_label(60), "1m");
        assert_eq!(wait_label(3_599), "59m");
        assert_eq!(wait_label(4_380), "1h 13m");
    }
}
