//! EP-006 US-018 - fleet grep: one query across every pane of every
//! workspace.
//!
//! Triggered from a terminal's find bar (`ToggleFleetSearch`, alt-f, or the
//! "Fleet" toggle button). The fan-out runs SEQUENTIALLY on the background
//! executor - per-pane `FairMutex` locks are disjoint, `search_term` holds
//! each one only for the scan, and at MAX_PANES = 32 a sequential pass
//! stays far under the 500 ms budget without contending 32 locks against
//! the render thread at once (PRD "stratégie de lock").
//!
//! Memory contract (US-018 AC): the overlay keeps counts + display names
//! only - never the per-pane `Vec<SearchMatch>` (the cap is 10 000 per
//! pane; 32 panes of full vectors would be unbounded-ish). Activating a
//! row re-runs the LOCAL search on the target view (`arm_search`), which
//! recomputes matches fresh - also how the US-017 rail lights up.

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, SharedString, Styled, Window, deferred, div, prelude::*, px,
};

use crate::PaneFlowApp;
use crate::app::ipc_handler::find_pane_by_surface_id;
use crate::app::workspace_ops::WorkspaceFocusTarget;
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

/// How long the per-tab match-count badges linger after a fan-out
/// (US-018: "auto-dismiss 4 s ou à la fermeture de la recherche").
const BADGE_HOLD_SECS: u64 = 4;

/// Fan-out outcome: per-pane `(surface_id, name, ws title, count)` hits,
/// the fleet-wide total, and the single regex error if any.
type FleetScanOutcome = (Vec<(u64, String, String, usize)>, usize, Option<String>);

/// One matching pane (bounded: count + names, no match vectors).
pub(crate) struct FleetHit {
    pub(crate) surface_id: u64,
    /// Display name - custom name or OSC title, bidi-stripped + clamped at
    /// collection time (terminal titles are UNTRUSTED).
    pub(crate) surface_name: String,
    pub(crate) ws_title: String,
    pub(crate) count: usize,
}

/// Overlay state (`PaneFlowApp::fleet_search`, `None` = closed).
pub(crate) struct FleetSearchState {
    pub(crate) query: String,
    pub(crate) regex: bool,
    pub(crate) results: Vec<FleetHit>,
    pub(crate) total: usize,
    /// The single regex error (US-018 AC: the engine's one error, never N
    /// duplicated copies - the fan-out stops at the first).
    pub(crate) error: Option<String>,
    pub(crate) running: bool,
    pub(crate) selected: usize,
}

impl PaneFlowApp {
    /// Entry point - `TerminalEvent::FleetSearchRequested` lands here (no
    /// `Window` in scope: overlay focus is deferred to the next render via
    /// `fleet_search_pending_focus`).
    pub(crate) fn start_fleet_search(
        &mut self,
        query: String,
        regex: bool,
        cx: &mut Context<Self>,
    ) {
        // Snapshot the fleet on the main thread with an opaque backend handle.
        // The background scan never receives a concrete terminal-grid type.
        let mut targets: Vec<(u64, String, String, crate::terminal::TerminalSessionBackend)> =
            Vec::new();
        for ws in &self.workspaces {
            if let Some(root) = &ws.active_tab().root {
                for pane in root.collect_leaves() {
                    for t in pane.read(cx).terminals() {
                        let r = t.read(cx);
                        let raw_name = r
                            .terminal
                            .custom_name
                            .clone()
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| r.terminal.title.clone());
                        // OSC titles are untrusted - same scrub as the
                        // EP-005 conflict tooltip (bidi strip + clamp).
                        let name = crate::markdown::strip_bidi_zero_width(
                            raw_name.chars().take(64).collect(),
                        );
                        targets.push((
                            t.entity_id().as_u64(),
                            name,
                            ws.title.clone(),
                            r.terminal.session_backend(),
                        ));
                    }
                }
            }
        }

        self.fleet_search_generation += 1;
        let generation = self.fleet_search_generation;
        self.fleet_search = Some(FleetSearchState {
            query: query.clone(),
            regex,
            results: Vec::new(),
            total: 0,
            error: None,
            running: true,
            selected: 0,
        });
        self.fleet_search_pending_focus = true;
        cx.notify();

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                // Sequential fan-out off the render thread; `search_term`
                // locks each grid internally for the scan only. Counts +
                // first-error only come back (memory contract above).
                let scan = smol::unblock(move || {
                    let mut hits: Vec<(u64, String, String, usize)> = Vec::new();
                    let mut total = 0usize;
                    let mut error: Option<String> = None;
                    for (sid, name, ws_title, backend) in targets {
                        let result = backend.search(&query, regex);
                        if let Some(e) = result.regex_error {
                            // Same query, same engine → the error is
                            // identical for every pane: keep ONE, stop.
                            error = Some(e);
                            break;
                        }
                        if !result.matches.is_empty() {
                            total += result.matches.len();
                            hits.push((sid, name, ws_title, result.matches.len()));
                        }
                    }
                    (hits, total, error)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app, cx| {
                        app.apply_fleet_search(generation, scan, cx);
                    })
                });
            },
        )
        .detach();
    }

    /// Deposit the fan-out result (main thread). A pane closed during the
    /// scan simply no longer resolves at badge-push/activation time - its
    /// row is dropped on activation (US-018 AC: jeté silencieusement).
    fn apply_fleet_search(
        &mut self,
        generation: u64,
        (hits, total, error): FleetScanOutcome,
        cx: &mut Context<Self>,
    ) {
        if self.fleet_search_generation != generation {
            return;
        }
        let Some(state) = &mut self.fleet_search else {
            return;
        };
        state.running = false;
        state.total = total;
        state.error = error;
        state.results = hits
            .into_iter()
            .map(|(surface_id, surface_name, ws_title, count)| FleetHit {
                surface_id,
                surface_name,
                ws_title,
                count,
            })
            .collect();

        // Transient per-tab badges (FR-11 lowest-priority slot). Pushed to
        // the LIVE tree - a pane closed mid-scan never receives one.
        let counts: std::collections::HashMap<u64, usize> = self
            .fleet_search
            .as_ref()
            .map(|s| s.results.iter().map(|h| (h.surface_id, h.count)).collect())
            .unwrap_or_default();
        self.push_fleet_badges(&counts, cx);

        // Auto-dismiss after 4 s unless a newer fan-out replaced this one
        // (closing the overlay also clears, in `close_fleet_search`).
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                smol::Timer::after(std::time::Duration::from_secs(BADGE_HOLD_SECS)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app, cx| {
                        if app.fleet_search_generation == generation {
                            app.push_fleet_badges(&std::collections::HashMap::new(), cx);
                        }
                    })
                });
            },
        )
        .detach();
        cx.notify();
    }

    /// Idempotent badge push (empty map = clear), mirroring the
    /// `sync_attention` contract.
    fn push_fleet_badges(
        &self,
        counts: &std::collections::HashMap<u64, usize>,
        cx: &mut Context<Self>,
    ) {
        for ws in &self.workspaces {
            if let Some(root) = &ws.active_tab().root {
                for pane in root.collect_leaves() {
                    let hits = pane
                        .read(cx)
                        .active_terminal_opt()
                        .and_then(|t| counts.get(&t.entity_id().as_u64()).copied());
                    pane.update(cx, |p, cx| p.set_search_hits(hits, cx));
                }
            }
        }
    }

    pub(crate) fn close_fleet_search(&mut self, cx: &mut Context<Self>) {
        self.fleet_search = None;
        // Closing the search dismisses the badges (US-018 AC) - and bumping
        // the generation cancels any in-flight deposit/timer.
        self.fleet_search_generation += 1;
        self.push_fleet_badges(&std::collections::HashMap::new(), cx);
        cx.notify();
    }

    /// Enter / click on a row: teleport to the pane (Attention Queue
    /// mechanics) and arm its LOCAL search with the fleet query - matches
    /// recompute fresh, the viewport lands on the first hit, and the
    /// US-017 rail renders from the same state.
    pub(crate) fn fleet_search_activate(
        &mut self,
        surface_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((query, regex)) = self
            .fleet_search
            .as_ref()
            .map(|s| (s.query.clone(), s.regex))
        else {
            return;
        };
        let Some(loc) = find_pane_by_surface_id(&self.workspaces, surface_id, cx) else {
            // Pane closed between render and Enter: drop the row, no panic.
            if let Some(state) = &mut self.fleet_search {
                state.results.retain(|h| h.surface_id != surface_id);
            }
            cx.notify();
            return;
        };
        // US-003 (cli-tab-hierarchy): the hit may live in a background
        // workspace tab, so make that tab visible before focusing its pane.
        let (ws_idx, pane) = (loc.workspace_idx, loc.pane);
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            ws.set_active_tab(loc.tab_idx);
        }
        self.activate_workspace_at(
            ws_idx,
            WorkspaceFocusTarget::Pane { pane: pane.clone() },
            window,
            cx,
        );
        if let Some(t) = pane.read(cx).active_terminal_opt().cloned() {
            t.update(cx, |view, cx| view.arm_search(&query, regex, cx));
        }
        self.close_fleet_search(cx);
    }

    pub(crate) fn handle_fleet_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let (len, selected) = match &self.fleet_search {
            Some(s) => (s.results.len(), s.selected),
            None => return,
        };
        match key {
            "escape" => self.close_fleet_search(cx),
            "enter" if len > 0 => {
                let idx = selected.min(len - 1);
                let sid = self
                    .fleet_search
                    .as_ref()
                    .map(|s| s.results[idx].surface_id);
                if let Some(sid) = sid {
                    self.fleet_search_activate(sid, window, cx);
                }
            }
            "up" if len > 0 && selected > 0 => {
                if let Some(s) = &mut self.fleet_search {
                    s.selected -= 1;
                }
                cx.notify();
            }
            "down" if len > 0 && selected + 1 < len => {
                if let Some(s) = &mut self.fleet_search {
                    s.selected += 1;
                }
                cx.notify();
            }
            _ => {}
        }
    }

    pub(crate) fn render_fleet_search(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let Some(state) = &self.fleet_search else {
            return div().into_any_element();
        };
        let selected = state.selected.min(state.results.len().saturating_sub(1));

        let mut card = div()
            .id("fleet-search")
            .occlude()
            .track_focus(&self.fleet_search_focus)
            .on_key_down(cx.listener(Self::handle_fleet_search_key_down))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_fleet_search(cx);
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
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(ui.border)
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(ui.text)
                            .child("Fleet search"),
                    )
                    .child(
                        // The query is user input - inert text, ellipsized.
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.))
                            .text_color(ui.muted)
                            .child(SharedString::from(state.query.clone())),
                    )
                    .when(state.regex, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .px(px(4.))
                                .rounded(px(3.))
                                .bg(ui.subtle)
                                .text_size(px(10.))
                                .text_color(ui.muted)
                                .child("regex"),
                        )
                    }),
            );

        if state.running {
            card = card.child(
                div()
                    .px(px(14.))
                    .py(px(14.))
                    .text_size(px(12.))
                    .text_color(ui.muted)
                    .child("Searching the fleet…"),
            );
        } else if let Some(err) = &state.error {
            // US-018 AC: the engine's single error - one surface, verbatim.
            card = card.child(
                div()
                    .px(px(14.))
                    .py(px(14.))
                    .text_size(px(12.))
                    .text_color(ui.agent_error)
                    .child(SharedString::from(err.clone())),
            );
        } else if state.results.is_empty() {
            card = card.child(
                div()
                    .px(px(14.))
                    .py(px(14.))
                    .text_size(px(12.))
                    .text_color(ui.muted)
                    .child("0 results across the fleet"),
            );
        } else {
            for (idx, hit) in state.results.iter().enumerate() {
                let is_selected = idx == selected;
                let sid = hit.surface_id;
                let resting_background = if is_selected {
                    ui.subtle
                } else {
                    ui.subtle.opacity(0.0)
                };
                let row = div()
                    .id(SharedString::from(format!("fleet-row-{sid}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(7.))
                    .text_size(px(12.))
                    .bg(resting_background)
                    .cursor_pointer()
                    .animated_hover(move |style, delta| {
                        style.bg(lerp_color(resting_background, ui.subtle, delta));
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.fleet_search_activate(sid, window, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(ui.text)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(SharedString::from(hit.surface_name.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(140.))
                            .truncate()
                            .text_color(ui.muted)
                            .child(SharedString::from(hit.ws_title.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(px(5.))
                            .rounded(px(4.))
                            .bg(ui.subtle)
                            .text_size(px(11.))
                            .text_color(ui.accent)
                            .child(format!("{}", hit.count)),
                    );
                card = card.child(row);
            }
            card = card.child(
                div()
                    .px(px(14.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(ui.border)
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .child(format!(
                        "{} match(es) in {} pane(s) · Enter focuses with the search armed · Esc closes",
                        state.total,
                        state.results.len()
                    )),
            );
        }

        deferred(
            div()
                .id("fleet-search-backdrop")
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
