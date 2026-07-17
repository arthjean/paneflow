//! Attention Queue (EP-002 US-004, CLI Cockpit).
//!
//! A cross-workspace overlay (theme-picker modal scaffold) listing every
//! agent session in `WaitingForInput`: tool + workspace + the sanitized
//! question (`AgentSession::message`, ≤512 chars, bidi-stripped at ingress -
//! rendered as inert text, never interpreted) + relative wait time, sorted
//! longest-waiting first. Enter / click teleports to the pane through the
//! same mechanics as `handle_jump_next_waiting` (workspace switch + hidden
//! tab activation + focus). The rows are derived from live session state on
//! every render - never a snapshot - so a session that unblocks while the
//! queue is open disappears at the next repaint, and a row whose pane died
//! is dropped rather than left navigable.

use std::collections::HashSet;

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, SharedString, Styled, Window, deferred, div, prelude::*, px,
};

use crate::PaneFlowApp;
use crate::ai_types::AgentState;
use crate::app::ipc_handler::find_pane_by_surface_id;
use crate::app::workspace_ops::WorkspaceFocusTarget;
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

/// One row of the queue, derived live from `agent_sessions`.
pub(crate) struct QueueRow {
    /// `Some` = navigable (resolved surface alive in the layout);
    /// `None` = the session never resolved a surface - listed last,
    /// non-navigable (US-019 orchestration-v2: navigation requires the
    /// mapping, never a guessed pane).
    pub(crate) surface_id: Option<u64>,
    pub(crate) ws_title: String,
    pub(crate) tool_label: &'static str,
    /// The agent's question - UNTRUSTED display-only text (already
    /// sanitized at the IPC ingress). Rendered verbatim, single line,
    /// ellipsized.
    pub(crate) message: Option<String>,
    pub(crate) waiting_secs: u64,
}

/// Longest wait first; sessions without a resolved surface sink to the end
/// regardless of their wait. Pure - unit-tested.
pub(crate) fn sort_rows(rows: &mut [QueueRow]) {
    rows.sort_by(|a, b| {
        b.surface_id
            .is_some()
            .cmp(&a.surface_id.is_some())
            .then(b.waiting_secs.cmp(&a.waiting_secs))
    });
}

/// Compact wait label: `42s`, `7m`, `1h 12m`. Pure - unit-tested.
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
    /// Derive the queue rows from live session state. Sessions whose
    /// resolved surface no longer exists in any layout (pane closed after
    /// resolution, sweep not yet run) are dropped entirely - the queue must
    /// never offer a navigable row to a dead pane.
    pub(crate) fn attention_queue_rows(&self, cx: &Context<Self>) -> Vec<QueueRow> {
        let mut live_surfaces: HashSet<u64> = HashSet::new();
        for ws in &self.workspaces {
            if let Some(root) = &ws.root {
                for pane in root.collect_leaves() {
                    for t in pane.read(cx).terminals() {
                        live_surfaces.insert(t.entity_id().as_u64());
                    }
                }
            }
        }
        let mut rows = Vec::new();
        for ws in &self.workspaces {
            for session in ws.agent_sessions.values() {
                if session.state != AgentState::WaitingForInput {
                    continue;
                }
                let surface_id = match session.surface_id {
                    Some(sid) if live_surfaces.contains(&sid) => Some(sid),
                    // Resolved but dead: drop the row (US-004 AC7).
                    Some(_) => continue,
                    // Never resolved: listed last, non-navigable (AC4).
                    None => None,
                };
                rows.push(QueueRow {
                    surface_id,
                    ws_title: ws.title.clone(),
                    tool_label: session.tool.display_name(),
                    message: session.message.clone(),
                    waiting_secs: session
                        .waiting_since
                        .map(|t| t.elapsed().as_secs())
                        .unwrap_or(0),
                });
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
        if !matches!(self.mode, paneflow_config::schema::AppMode::Cli) {
            return;
        }
        if self.attention_queue_open {
            self.close_attention_queue_and_restore_focus(window, cx);
            return;
        }
        self.attention_queue_open = true;
        self.attention_queue_selected = 0;
        self.attention_queue_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn close_attention_queue(&mut self, cx: &mut Context<Self>) {
        self.attention_queue_open = false;
        self.attention_queue_selected = 0;
        cx.notify();
    }

    pub(crate) fn close_attention_queue_and_restore_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_attention_queue(cx);
        if let Some(ws) = self.workspaces.get_mut(self.active_idx) {
            ws.focus_first(window, cx);
        }
    }

    /// Enter / click on a row: teleport to the waiting pane (workspace
    /// switch + tab activation + focus - `handle_jump_next_waiting`
    /// mechanics) and close the queue. The surface is re-resolved at
    /// activation time: a pane closed between render and Enter is a clean
    /// no-op (the row is gone at the next repaint anyway).
    pub(crate) fn attention_queue_activate(
        &mut self,
        surface_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((ws_idx, pane, tab_idx)) =
            find_pane_by_surface_id(&self.workspaces, surface_id, cx)
        else {
            cx.notify();
            return;
        };
        self.activate_workspace_at(
            ws_idx,
            WorkspaceFocusTarget::PaneTab {
                pane: pane.clone(),
                tab_idx,
            },
            window,
            cx,
        );
        // Keep the jump cycle coherent: a queue teleport counts as visiting
        // that surface, so the next Ctrl+Shift+J continues from here.
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
            // US-004 AC5: explicit empty state - never a silent no-op.
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
                // The question is inert untrusted text: one ellipsized line,
                // no links, no ANSI (US-004 AC8).
                let question: SharedString = row
                    .message
                    .clone()
                    .unwrap_or_else(|| "Needs input".to_string())
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
                    .child(
                        div()
                            .flex_none()
                            .w(px(6.))
                            .h(px(6.))
                            .rounded_full()
                            .bg(ui.vc_conflict),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(ui.text)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(row.tool_label),
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
                    // AC4: unresolved sessions are visible but inert.
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
                    .child("Enter focuses the pane · Esc closes"),
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
            tool_label: "Claude",
            message: None,
            waiting_secs,
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
        // Navigable rows by wait desc, the unmapped session last despite its
        // huge wait (US-004 AC2 + AC4).
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
