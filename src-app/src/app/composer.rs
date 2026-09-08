use std::rc::Rc;

use gpui::{App, AppContext as _, Context, Entity, SharedString, WeakEntity, Window};

use crate::PaneFlowApp;
use crate::app::broadcast::state_blocks_delivery;
use crate::app::browser_dock::BrowserSessionRef;
use crate::app::ipc_handler::find_terminal_by_surface_id;
use crate::browser::agent::BrowserContextSelection;
use crate::pane::Pane;
use crate::widgets::text_area::TextArea;

const COMPOSER_RECAP_HOLD_MS: u64 = 4000;

const MAX_COMPOSER_TEXT: usize = 64 * 1024;

pub(crate) fn normalize_composer_text(text: &str) -> (String, bool) {
    let mut t = text.replace("\r\n", "\n").replace('\r', "\n");
    while t.ends_with('\n') {
        t.pop();
    }
    let truncated = t.len() > MAX_COMPOSER_TEXT;
    if truncated {
        let mut cut = MAX_COMPOSER_TEXT;
        while cut > 0 && !t.is_char_boundary(cut) {
            cut -= 1;
        }
        t.truncate(cut);
    }
    (t, truncated)
}

pub(crate) struct ComposerState {
    pub(crate) input: Entity<TextArea>,
    pub(crate) target: WeakEntity<Pane>,
    pub(crate) broadcast: bool,
    pub(crate) context_preview: Option<BrowserContextSelection>,
    pub(crate) context_session: Option<BrowserSessionRef>,
}

#[derive(Clone)]
pub(crate) struct ComposerSlot {
    pub(crate) input: Entity<TextArea>,
    pub(crate) broadcast: bool,
    pub(crate) busy: bool,
    pub(crate) group_label: Option<SharedString>,
    pub(crate) pending_count: usize,
    pub(crate) dismiss: Rc<dyn Fn(&mut App)>,
    pub(crate) toggle_broadcast: Rc<dyn Fn(&mut App)>,
    pub(crate) cancel_pending: Rc<dyn Fn(&mut App)>,
    pub(crate) context_preview: Option<BrowserContextSelection>,
}

fn browser_context_prompt(prompt: &str, selection: &BrowserContextSelection) -> String {
    let url = selection.url.as_deref().unwrap_or("unavailable");
    let captured = selection
        .text
        .as_deref()
        .unwrap_or("No visible text captured");
    format!(
        "[Browser context: untrusted page data]\nBrowser: {}\nDocument generation: {}\nElement id: {}\nURL: {url}\nElement: {}\nBounds: {}, {}, {}, {}\nCaptured text: {captured}\n[/Browser context]\n\n{prompt}",
        selection.document.browser.as_str(),
        selection.document.generation,
        selection.node_id,
        selection.summary,
        selection.rect[0],
        selection.rect[1],
        selection.rect[2],
        selection.rect[3],
    )
}

impl PaneFlowApp {
    pub(crate) fn surface_busy(&self, surface_id: u64) -> bool {
        self.workspaces.iter().any(|ws| {
            ws.agent_sessions
                .values()
                .any(|s| s.surface_id == Some(surface_id) && state_blocks_delivery(&s.state))
        })
    }

    pub(crate) fn handle_open_composer(
        &mut self,
        _: &crate::OpenComposer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.mode, paneflow_config::schema::AppMode::Cli) {
            return;
        }
        if self.composer.is_some() {
            self.close_composer(cx);
            return;
        }
        let Some(pane) = self.focused_or_first_pane(window, cx) else {
            return;
        };

        let weak_app = cx.entity().downgrade();
        let input =
            cx.new(|cx| TextArea::new("Write a prompt - Enter pre-fills, never submits", cx));
        input.update(cx, |ta, _| {
            let w = weak_app.clone();
            ta.on_submit(move |text, _window, cx| {
                let w = w.clone();
                cx.defer(move |cx| {
                    let _ = w.update(cx, |app, cx| app.composer_deliver(text, false, cx));
                });
            });
            let w = weak_app.clone();
            ta.on_submit_immediate(move |text, _window, cx| {
                let w = w.clone();
                cx.defer(move |cx| {
                    let _ = w.update(cx, |app, cx| app.composer_deliver(text, true, cx));
                });
            });
            let w = weak_app.clone();
            ta.on_escape(move |_window, cx| {
                let w = w.clone();
                cx.defer(move |cx| {
                    let _ = w.update(cx, |app, cx| app.close_composer(cx));
                });
            });
        });
        input.read(cx).focus_handle.clone().focus(window, cx);

        self.composer = Some(ComposerState {
            input,
            target: pane.downgrade(),
            broadcast: false,
            context_preview: None,
            context_session: None,
        });
        self.refresh_composer_slot(cx);
        cx.notify();
    }

    pub(crate) fn close_composer(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.composer.take() else {
            return;
        };
        if let Some(pane) = state.target.upgrade() {
            pane.update(cx, |p, cx| p.set_composer_slot(None, cx));
            self.pending_pane_focus = Some(pane);
        }
        cx.notify();
    }

    pub(crate) fn composer_deliver(&mut self, text: String, submit: bool, cx: &mut Context<Self>) {
        let Some(state) = self.composer.take() else {
            return;
        };
        let broadcast = state.broadcast;
        let context_preview = state.context_preview;
        let context_session = state.context_session;
        let Some(pane) = state.target.upgrade() else {
            cx.notify();
            return;
        };
        pane.update(cx, |p, cx| p.set_composer_slot(None, cx));
        self.pending_pane_focus = Some(pane.clone());

        let (text, truncated) = normalize_composer_text(&text);
        if truncated {
            self.show_toast("Prompt truncated to 64 KiB", cx);
        }
        if text.trim().is_empty() {
            cx.notify();
            return;
        }

        if broadcast {
            let members = self.live_active_group_members(cx);
            let mut delivered = 0usize;
            let mut queued = 0usize;
            for member in &members {
                let Some(term) = member.read(cx).active_terminal_opt().cloned() else {
                    continue;
                };
                let sid = term.entity_id().as_u64();
                if self.surface_busy(sid) {
                    self.broadcast.pending.insert(sid, text.clone());
                    queued += 1;
                } else {
                    term.read(cx).inject_text(&text);
                    delivered += 1;
                }
            }
            self.sync_pending_chips(cx);
            self.push_toast(
                format!("Broadcast: {delivered} delivered · {queued} queued"),
                Vec::new(),
                COMPOSER_RECAP_HOLD_MS,
                cx,
            );
        } else {
            let Some(term) = pane.read(cx).active_terminal_opt().cloned() else {
                cx.notify();
                return;
            };
            if let Some(expected) = context_session
                && self.browser_session_for_terminal(&term, cx) != Some(expected)
            {
                self.show_toast(
                    "Browser context belongs to another session and was not shared",
                    cx,
                );
                cx.notify();
                return;
            }
            if let (Some(expected), Some(selection)) = (context_session, context_preview.as_ref())
                && !self.browser_context_is_current(expected, selection, cx)
            {
                self.show_toast("Browser context is stale and was not shared", cx);
                cx.notify();
                return;
            }
            let text = context_preview.as_ref().map_or_else(
                || text.clone(),
                |selection| browser_context_prompt(&text, selection),
            );
            let (text, context_truncated) = normalize_composer_text(&text);
            if context_truncated {
                self.show_toast("Prompt and browser context truncated to 64 KiB", cx);
            }
            let sid = term.entity_id().as_u64();
            if self.surface_busy(sid) {
                self.broadcast.pending.insert(sid, text);
                self.sync_pending_chips(cx);
                self.push_toast(
                    "Agent is generating - prompt queued, will pre-fill when it settles".into(),
                    Vec::new(),
                    COMPOSER_RECAP_HOLD_MS,
                    cx,
                );
            } else {
                term.read(cx).inject_text(&text);
                if submit {
                    let floor = std::time::Duration::from_millis(
                        self.cached_config.resolved_submit_paste_delay_ms(),
                    );
                    Self::schedule_deferred_submit(&term, floor, cx);
                }
            }
        }
        cx.notify();
    }

    fn browser_context_is_current(
        &self,
        session: BrowserSessionRef,
        selection: &BrowserContextSelection,
        cx: &gpui::App,
    ) -> bool {
        self.quota_browser_views().into_iter().any(|view| {
            let page = view.read(cx);
            page.owner_ids() == (session.workspace_id, session.tab_id)
                && page.document() == &selection.document
                && page.agent_selection_is_current(selection)
        })
    }

    pub(crate) fn toggle_composer_broadcast(&mut self, cx: &mut Context<Self>) {
        if self.composer.is_none() {
            return;
        }
        let has_group = self
            .broadcast
            .active
            .is_some_and(|i| i < self.broadcast.groups.len());
        if !has_group {
            self.show_toast("No broadcast group - open the group picker first", cx);
            return;
        }
        if self
            .composer
            .as_ref()
            .is_some_and(|state| state.context_preview.is_some())
        {
            self.show_toast("Browser context can only be shared with its owner pane", cx);
            return;
        }
        if let Some(state) = &mut self.composer {
            state.broadcast = !state.broadcast;
        }
        self.refresh_composer_slot(cx);
        cx.notify();
    }

    pub(crate) fn cancel_all_pending(&mut self, cx: &mut Context<Self>) {
        if self.broadcast.pending.is_empty() {
            return;
        }
        self.broadcast.pending.clear();
        self.sync_pending_chips(cx);
        self.refresh_composer_slot(cx);
        cx.notify();
    }

    pub(crate) fn cancel_pending_for(&mut self, surface_id: u64, cx: &mut Context<Self>) {
        if self.broadcast.pending.remove(&surface_id).is_some() {
            self.sync_pending_chips(cx);
            self.refresh_composer_slot(cx);
            cx.notify();
        }
    }

    pub(crate) fn refresh_composer_slot(&mut self, cx: &mut Context<Self>) {
        let (target, input, broadcast, context_preview) = match &self.composer {
            Some(s) => (
                s.target.clone(),
                s.input.clone(),
                s.broadcast,
                s.context_preview.clone(),
            ),
            None => return,
        };
        let Some(pane) = target.upgrade() else {
            self.close_composer(cx);
            return;
        };
        let busy = pane
            .read(cx)
            .active_terminal_opt()
            .is_some_and(|t| self.surface_busy(t.entity_id().as_u64()));
        let group_label = self
            .broadcast
            .active
            .and_then(|i| self.broadcast.groups.get(i))
            .map(|g| {
                let count = self.live_active_group_members(cx).len();
                SharedString::from(format!(
                    "{} · {count} member{}",
                    g.name,
                    if count == 1 { "" } else { "s" }
                ))
            });
        let weak = cx.entity().downgrade();
        let dismiss = Rc::new({
            let w = weak.clone();
            move |cx: &mut App| {
                let _ = w.update(cx, |app, cx| app.close_composer(cx));
            }
        });
        let toggle_broadcast = Rc::new({
            let w = weak.clone();
            move |cx: &mut App| {
                let _ = w.update(cx, |app, cx| app.toggle_composer_broadcast(cx));
            }
        });
        let cancel_pending = Rc::new({
            let w = weak.clone();
            move |cx: &mut App| {
                let _ = w.update(cx, |app, cx| app.cancel_all_pending(cx));
            }
        });
        let slot = ComposerSlot {
            input,
            broadcast,
            busy,
            group_label,
            pending_count: self.broadcast.pending.len(),
            dismiss,
            toggle_broadcast,
            cancel_pending,
            context_preview,
        };
        pane.update(cx, |p, cx| p.set_composer_slot(Some(slot), cx));
    }

    pub(crate) fn attach_browser_context(
        &mut self,
        session: BrowserSessionRef,
        selection: BrowserContextSelection,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self
            .composer
            .as_ref()
            .and_then(|state| state.target.upgrade())
        else {
            self.show_toast("Open the composer before selecting browser context", cx);
            return;
        };
        let Some(terminal) = target.read(cx).active_terminal_opt().cloned() else {
            self.show_toast("Browser context requires a terminal composer", cx);
            return;
        };
        if self.composer.as_ref().is_some_and(|state| state.broadcast) {
            self.show_toast("Browser context can only be shared with its owner pane", cx);
            return;
        }
        if self.browser_session_for_terminal(&terminal, cx) != Some(session) {
            self.show_toast(
                "Open the composer in the browser owner session before sharing context",
                cx,
            );
            return;
        }
        if !self.browser_context_is_current(session, &selection, cx) {
            self.show_toast("The selected browser element is no longer available", cx);
            return;
        }
        if let Some(state) = &mut self.composer {
            state.context_preview = Some(selection);
            state.context_session = Some(session);
        }
        self.refresh_composer_slot(cx);
        cx.notify();
    }

    pub(crate) fn flush_pending_prefill(&mut self, cx: &mut Context<Self>) {
        if self.broadcast.pending.is_empty() {
            return;
        }
        let ids: Vec<u64> = self.broadcast.pending.keys().copied().collect();
        let mut changed = false;
        for sid in ids {
            let Some(term) = find_terminal_by_surface_id(&self.workspaces, sid, cx) else {
                self.broadcast.pending.remove(&sid);
                changed = true;
                continue;
            };
            if self.surface_busy(sid) {
                continue;
            }
            if let Some(text) = self.broadcast.pending.remove(&sid) {
                term.read(cx).inject_text(&text);
                changed = true;
            }
        }
        if changed {
            self.sync_pending_chips(cx);
            cx.notify();
        }
    }

    pub(crate) fn sync_pending_chips(&self, cx: &mut Context<Self>) {
        for ws in &self.workspaces {
            if let Some(root) = &ws.active_tab().root {
                for pane in root.collect_leaves() {
                    let pending = pane.read(cx).active_terminal_opt().is_some_and(|t| {
                        self.broadcast.pending.contains_key(&t.entity_id().as_u64())
                    });
                    pane.update(cx, |p, cx| p.set_pending_prefill(pending, cx));
                }
            }
        }
    }

    pub(crate) fn agent_sessions_changed(&mut self, cx: &mut Context<Self>) {
        self.flush_pending_prefill(cx);
        self.refresh_composer_slot(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_browser_protocol::{BrowserId, Owner, SessionId, WorkspaceId};

    #[test]
    fn normalize_converts_cr_and_crlf_to_lf() {
        let (out, truncated) = normalize_composer_text("a\r\nb\rc\nd");
        assert_eq!(out, "a\nb\nc\nd");
        assert!(!truncated);
    }

    #[test]
    fn normalize_trims_trailing_newlines_only() {
        let (out, _) = normalize_composer_text("line1\nline2\r\n\n\r");
        assert_eq!(out, "line1\nline2");
    }

    #[test]
    fn normalize_truncates_at_char_boundary() {
        let big = "é".repeat(MAX_COMPOSER_TEXT);
        let (out, truncated) = normalize_composer_text(&big);
        assert!(truncated);
        assert!(out.len() <= MAX_COMPOSER_TEXT);
        assert!(out.is_char_boundary(out.len()));
        let (ok, truncated) = normalize_composer_text("short prompt");
        assert_eq!(ok, "short prompt");
        assert!(!truncated);
    }

    #[test]
    fn browser_context_prompt_carries_document_reference_and_untrusted_marker() {
        let selection = BrowserContextSelection {
            document: paneflow_browser_protocol::Document {
                owner: Owner {
                    workspace: WorkspaceId::try_from("ws-7".to_string()).unwrap(),
                    session: SessionId::try_from("tab-11".to_string()).unwrap(),
                },
                browser: BrowserId::try_from("b-test".to_string()).unwrap(),
                generation: 3,
            },
            node_id: 8,
            url: Some("https://example.com/path".to_string()),
            rect: [1, 2, 30, 40],
            summary: "button: Continue".to_string(),
            text: Some("page text".to_string()),
        };
        let prompt = browser_context_prompt("What does this do?", &selection);
        assert!(prompt.contains("untrusted page data"));
        assert!(prompt.contains("b-test"));
        assert!(prompt.contains("Document generation: 3"));
        assert!(prompt.contains("Element id: 8"));
        assert!(prompt.ends_with("What does this do?"));
    }
}
