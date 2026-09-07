use std::time::Instant;

use gpui::Context;
use paneflow_config::schema::TabTitleSource;

use crate::PaneFlowApp;
use crate::app::ipc_handler::tab_for_surface;
use crate::auto_naming::{Decision, Role, build_prompt, pick_summarizer, summarize};

impl PaneFlowApp {
    pub(crate) fn record_auto_naming_message(
        &mut self,
        ws_id: u64,
        session_key: u32,
        role: Role,
        text: &str,
    ) {
        if let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == ws_id)
            && let Some(session) = ws.agent_sessions.get_mut(&session_key)
        {
            session.auto_naming.record(role, text);
        }
    }

    pub(crate) fn schedule_auto_naming(
        &mut self,
        ws_id: u64,
        session_key: u32,
        cx: &mut Context<Self>,
    ) {
        if !self.cached_config.automation.tab_auto_naming_enabled() {
            return;
        }
        let Some(ws_idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) else {
            return;
        };
        let Some((tool, surface_id)) = self.workspaces[ws_idx]
            .agent_sessions
            .get(&session_key)
            .and_then(|session| Some((session.tool, session.surface_id?)))
        else {
            return;
        };
        let Some((tab_idx, 1)) = tab_for_surface(&self.workspaces[ws_idx], surface_id, cx) else {
            return;
        };
        let Some(current_title) = self.workspaces[ws_idx]
            .tabs()
            .get(tab_idx)
            .filter(|tab| !tab.title_is_user_owned())
            .map(|tab| tab.title().to_string())
        else {
            return;
        };
        let now = Instant::now();
        let Some(session) = self.workspaces[ws_idx].agent_sessions.get_mut(&session_key) else {
            return;
        };
        if session.auto_naming.decide(now) != Decision::Run {
            return;
        }
        let context = session.auto_naming.begin(now);
        let prompt = build_prompt(Some(&current_title), &context);
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let title = smol::unblock(move || {
                    pick_summarizer(tool)
                        .and_then(|agent| summarize(agent, &prompt, Some(&current_title)))
                })
                .await;
                cx.update(|cx| {
                    let _ = this.update(cx, |app, cx| {
                        app.finish_auto_naming(ws_id, session_key, title, cx);
                    });
                });
            },
        )
        .detach();
    }

    fn finish_auto_naming(
        &mut self,
        ws_id: u64,
        session_key: u32,
        title: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(ws_idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) else {
            return;
        };
        let Some(surface_id) = self.workspaces[ws_idx]
            .agent_sessions
            .get_mut(&session_key)
            .and_then(|session| {
                session.auto_naming.finish();
                session.surface_id
            })
        else {
            return;
        };
        let Some(title) = title else {
            return;
        };
        let Some((tab_idx, 1)) = tab_for_surface(&self.workspaces[ws_idx], surface_id, cx) else {
            return;
        };
        if self.workspaces[ws_idx]
            .tab_mut(tab_idx)
            .is_some_and(|tab| tab.set_title(&title, TabTitleSource::Summarized))
        {
            self.save_session(cx);
            cx.notify();
        }
    }
}
