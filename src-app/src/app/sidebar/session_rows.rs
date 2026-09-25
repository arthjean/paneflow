use super::*;

fn session_row_lane(row: &HostAgentRow) -> Option<SidebarAgentState> {
    match row.state? {
        AgentState::WaitingForInput => Some(SidebarAgentState::NeedsInput),
        AgentState::Errored => Some(SidebarAgentState::Errored),
        AgentState::Thinking => Some(SidebarAgentState::Thinking),
        AgentState::Finished => Some(SidebarAgentState::Finished),
    }
}

fn session_row_agent_word(state: Option<AgentState>) -> &'static str {
    match state {
        Some(AgentState::WaitingForInput) => "needs input",
        Some(AgentState::Thinking) => "thinking",
        Some(AgentState::Finished) => "finished",
        Some(AgentState::Errored) => "errored",
        None => "idle",
    }
}

fn ended_preview(ended_total: usize, cap: usize, expanded: bool) -> (usize, usize) {
    let shown = if expanded {
        ended_total
    } else {
        cap.min(ended_total)
    };
    (shown, ended_total - shown)
}

fn collapsed_sessions_label(collapsed: usize) -> String {
    if collapsed == 1 {
        "1 more ended session".to_string()
    } else {
        format!("{collapsed} more ended sessions")
    }
}

fn session_row_tooltip(
    session: &OwnedSession,
    row: Option<&HostAgentRow>,
    now_ms: u64,
    disconnected: Option<&str>,
    list_stale: bool,
    unknown: bool,
    restart_recommended: bool,
) -> String {
    let cwd = &session.cwd;
    let mut text = if session.live {
        let word = session_row_agent_word(row.and_then(|row| row.state));
        match row {
            Some(row) if row.stale => {
                format!("Session running in {cwd}. Agent last seen {word}, now stale.")
            }
            _ => format!("Session running in {cwd}. Agent {word}."),
        }
    } else {
        format!(
            "{} {} in {cwd}.",
            lifecycle_sentence(&session.lifecycle),
            relative_age(now_ms, session.updated_at_ms)
        )
    };
    if let Some(reason) = disconnected {
        text.push_str(&format!(
            " The agent stream is disconnected ({reason}), so this state may be stale."
        ));
    }
    if restart_recommended {
        text.push_str(
            " Paneflow was updated since it started; restart it when convenient to run the new version.",
        );
    }
    if list_stale {
        text.push_str(" The local host did not answer the last listing, so this list is stale.");
    }
    if unknown {
        text.push_str(" The outcome of the last action on it is unknown.");
    }
    text.push_str(if session.live {
        " Click to reopen it, right-click for more."
    } else {
        " Click to resume it, right-click for more."
    });
    text
}

impl PaneFlowApp {
    pub(super) fn render_fallback_session_group(
        &self,
        list: gpui::Stateful<gpui::Div>,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let rows = self.render_session_rows(
            SessionRowScope::Fallback,
            SIDEBAR_FOLDER_SLOT_WIDTH + SIDEBAR_TITLE_ROW_GAP,
            0.,
            SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH,
            ui,
            cx,
        );
        if rows.is_empty() {
            return list;
        }
        let header = div()
            .id("sidebar-other-sessions")
            .flex_none()
            .mx(px(SIDEBAR_ROW_MARGIN_X))
            .mt(px(8.))
            .px(px(SIDEBAR_ROW_PADDING_X))
            .py(px(SIDEBAR_ROW_PADDING_Y))
            .text_size(px(13.))
            .text_color(ui.muted)
            .child("Other sessions");
        list.child(header).child(
            div()
                .id("sidebar-other-sessions-rows")
                .mx(px(SIDEBAR_ROW_MARGIN_X))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(SIDEBAR_ROW_SPACING))
                .children(rows),
        )
    }

    pub(super) fn render_session_rows(
        &self,
        scope: SessionRowScope,
        title_indent: f32,
        row_inset: f32,
        content_width: f32,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let (listed, expanded, expand_target, scope_key) = match scope {
            SessionRowScope::Workspace(ws_idx) => {
                let ws = &self.workspaces[ws_idx];
                (
                    self.owned_sessions_for_workspace(ws, cx),
                    self.ended_sessions_expanded(ws),
                    Some(ws.durable_id.clone()),
                    ws_idx.to_string(),
                )
            }
            SessionRowScope::Fallback => (
                self.fallback_owned_sessions(cx),
                self.fallback_sessions_expanded(),
                None,
                "fallback".to_string(),
            ),
        };
        if listed.is_empty() {
            return Vec::new();
        }
        let cap = usize::from(self.cached_config.resolved_sidebar_ended_sessions());
        let ended_total = listed.iter().filter(|session| !session.live).count();
        let (shown_ended, collapsed) = ended_preview(ended_total, cap, expanded);
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        let icon_indent = title_indent - SIDEBAR_TITLE_ROW_GAP - row_inset;
        let disconnected = self
            .host_agents_are_stale()
            .then(|| self.host_agents_disconnect_reason().unwrap_or("no stream"));
        let reconnecting = self.worker_is_reconnecting();
        let restart_capability = self.worker_advertises("restart.recommendation");
        let list_stale = self.owned_sessions_are_stale();
        let now_ms = crate::ipc_events::now_ms();

        let mut rendered = 0usize;
        let mut rows: Vec<AnyElement> = Vec::new();
        for session in listed {
            if !session.live {
                rendered += 1;
                if rendered > shown_ended {
                    continue;
                }
            }
            let label = session.label();
            let key = session.session.to_string();
            let group = SharedString::from(format!("session-row-group-{key}"));
            let agent = self.host_agent_row(&session.session);
            let lane = session
                .live
                .then(|| agent.and_then(session_row_lane))
                .flatten()
                .map(|state| {
                    Lane::Agent(SidebarAgentSummary {
                        state,
                        count: 1,
                        tint: agent
                            .and_then(|row| row.tool)
                            .and_then(crate::agent_launcher::TerminalAgent::accent),
                    })
                });
            let unknown = self.session_outcome_unknown(&session.session);
            let restart_recommended =
                restart_capability && agent.is_some_and(|row| row.restart_recommended);
            let tooltip = session_row_tooltip(
                &session,
                agent,
                now_ms,
                disconnected,
                list_stale,
                unknown,
                restart_recommended,
            );
            let dimmed = !session.live
                || list_stale
                || unknown
                || reconnecting
                || disconnected.is_some()
                || agent.is_some_and(|row| row.stale);
            let resting_color = if dimmed { ui.muted } else { ui.text };
            let row_tooltip = format!("{label}\n{tooltip}");
            let lane_tooltip = SharedString::from(tooltip.clone());
            let body = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(SIDEBAR_TITLE_ROW_GAP))
                .w(px(content_width))
                .max_w(px(content_width))
                .min_w_0()
                .when(icon_indent > 0., |row| {
                    row.child(div().flex_none().w(px(icon_indent)))
                })
                .child(
                    svg()
                        .size(px(SESSION_ROW_ICON_SIZE))
                        .flex_none()
                        .path("icons/terminal.svg")
                        .text_color(resting_color)
                        .group_hover(group.clone(), move |style| style.text_color(ui.text)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(resting_color)
                        .group_hover(group.clone(), move |style| style.text_color(ui.text))
                        .text_size(crate::ui_primitives::TITLE)
                        .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                        .child(label),
                )
                .child(render_lane_slot(
                    lane,
                    &format!("session-row-{key}"),
                    move |_| lane_tooltip,
                    group.clone(),
                    ui,
                ));
            let click_session = session.clone();
            let menu_session = session.clone();
            let shell = sidebar_row_shell()
                .ml(px(row_inset))
                .id(SharedString::from(format!("session-row-{key}")))
                .cursor_pointer()
                .delayed_tooltip(crate::ui_primitives::text_tooltip(row_tooltip))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_listed_session(scope, click_session.clone(), window, cx);
                    cx.stop_propagation();
                }))
                .on_aux_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                    if event.is_right_click()
                        && let Some(position) = event.mouse_position()
                    {
                        this.dismiss_transient_surfaces();
                        this.session_menu_open = Some(crate::SessionContextMenu {
                            scope,
                            session: Box::new(menu_session.clone()),
                            position,
                        });
                        cx.stop_propagation();
                        cx.notify();
                    }
                }));
            rows.push(sidebar_row(shell, group, None, Some(hover_bg), body).into_any_element());
        }

        if collapsed > 0 {
            let group = SharedString::from(format!("session-more-group-{scope_key}"));
            let body = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(SIDEBAR_TITLE_ROW_GAP))
                .w(px(content_width))
                .max_w(px(content_width))
                .min_w_0()
                .child(div().flex_none().w(px(icon_indent
                    + SIDEBAR_TITLE_ROW_GAP
                    + SESSION_ROW_ICON_SIZE)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(ui.muted)
                        .group_hover(group.clone(), move |style| style.text_color(ui.text))
                        .text_size(crate::ui_primitives::TITLE)
                        .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                        .child(collapsed_sessions_label(collapsed)),
                );
            let shell = sidebar_row_shell()
                .ml(px(row_inset))
                .id(SharedString::from(format!("session-more-{scope_key}")))
                .cursor_pointer()
                .delayed_tooltip(crate::ui_primitives::text_tooltip(
                    "Show all ended sessions".to_string(),
                ))
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    match expand_target.clone() {
                        Some(workspace_id) => this.expand_ended_sessions(workspace_id, cx),
                        None => this.expand_fallback_sessions(cx),
                    }
                    cx.stop_propagation();
                }));
            rows.push(sidebar_row(shell, group, None, Some(hover_bg), body).into_any_element());
        }

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ai_types::AgentState;
    use crate::app::host_agents::HostAgentRow;

    fn host_row(state: Option<AgentState>, stale: bool) -> HostAgentRow {
        HostAgentRow {
            session: paneflow_config::schema::SessionId::new(),
            tool: None,
            state,
            message: None,
            last_result: None,
            active_tool_name: None,
            pid: None,
            waiting_since_ms: None,
            last_event_at_ms: None,
            stale,
            live: !stale,
            activity_source: crate::app::host_agents::ActivitySource::Hooks,
            restart_recommended: false,
            unread: false,
        }
    }

    #[test]
    fn a_session_row_shows_the_host_agent_state() {
        let cases = [
            (AgentState::WaitingForInput, SidebarAgentState::NeedsInput),
            (AgentState::Errored, SidebarAgentState::Errored),
            (AgentState::Thinking, SidebarAgentState::Thinking),
            (AgentState::Finished, SidebarAgentState::Finished),
        ];
        for (host, sidebar) in cases {
            assert_eq!(
                session_row_lane(&host_row(Some(host), false)),
                Some(sidebar)
            );
        }
        assert_eq!(session_row_lane(&host_row(None, false)), None);
    }

    fn listed(live: bool) -> OwnedSession {
        OwnedSession {
            session: paneflow_config::schema::SessionId::new(),
            generation: paneflow_config::schema::SessionGeneration::FIRST,
            workspace: None,
            title: Some("api".to_string()),
            cwd: "/src/api".to_string(),
            live,
            lifecycle: if live {
                paneflow_host::SessionLifecycle::Running
            } else {
                paneflow_host::SessionLifecycle::Exited {
                    code: 130,
                    signal: None,
                }
            },
            updated_at_ms: 0,
        }
    }

    #[test]
    fn a_disconnected_host_reads_as_stale_never_as_idle_or_finished() {
        let working = host_row(Some(AgentState::Thinking), false);

        let live = session_row_tooltip(&listed(true), Some(&working), 0, None, false, false, false);
        assert!(live.contains("Agent thinking"), "{live}");
        assert!(!live.contains("stale"), "{live}");
        assert!(live.contains("Click to reopen"), "{live}");

        let dropped = session_row_tooltip(
            &listed(true),
            Some(&working),
            0,
            Some("stream closed"),
            false,
            false,
            false,
        );
        assert!(dropped.contains("stale"), "{dropped}");
        assert!(dropped.contains("stream closed"), "{dropped}");
        assert!(!dropped.contains("finished"), "{dropped}");

        let lost = host_row(Some(AgentState::Thinking), true);
        let lost = session_row_tooltip(&listed(true), Some(&lost), 0, None, false, false, false);
        assert!(lost.contains("last seen thinking"), "{lost}");
        assert!(lost.contains("stale"), "{lost}");
    }

    #[test]
    fn an_ended_row_states_its_lifecycle_its_age_and_its_verb() {
        let tooltip = session_row_tooltip(&listed(false), None, 120_000, None, false, false, false);
        assert!(tooltip.contains("Exited with code 130"), "{tooltip}");
        assert!(tooltip.contains("2 min ago"), "{tooltip}");
        assert!(tooltip.contains("Click to resume"), "{tooltip}");

        let stale = session_row_tooltip(&listed(false), None, 120_000, None, true, true, false);
        assert!(stale.contains("this list is stale"), "{stale}");
        assert!(stale.contains("outcome of the last action"), "{stale}");

        let older_core =
            session_row_tooltip(&listed(true), None, 120_000, None, false, false, true);
        assert!(
            older_core.contains("Paneflow was updated since it started"),
            "a restart recommendation is surfaced, never acted on for the user: {older_core}"
        );
    }

    #[test]
    fn the_overflow_row_counts_what_it_hides() {
        assert_eq!(collapsed_sessions_label(1), "1 more ended session");
        assert_eq!(collapsed_sessions_label(4), "4 more ended sessions");
    }

    #[test]
    fn the_preview_cap_collapses_the_rest_and_never_drops_a_record() {
        assert_eq!(ended_preview(9, 5, false), (5, 4));
        assert_eq!(ended_preview(9, 5, true), (9, 0));
        assert_eq!(ended_preview(3, 5, false), (3, 0));
        assert_eq!(ended_preview(3, 0, false), (0, 3));
        assert_eq!(ended_preview(0, 5, false), (0, 0));
        for cap in paneflow_config::schema::ENDED_SESSION_CAPS {
            let (shown, collapsed) = ended_preview(12, usize::from(*cap), false);
            assert_eq!(shown + collapsed, 12, "every ended record keeps a place");
        }
    }
}
