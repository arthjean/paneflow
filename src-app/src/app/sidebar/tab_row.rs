use super::*;

fn tab_diffstat_visible(
    show: paneflow_config::schema::SidebarShow,
    stats: &crate::workspace::GitDiffStats,
) -> bool {
    show.diffstat_enabled() && (stats.insertions > 0 || stats.deletions > 0)
}

pub(crate) fn tab_display_title(tab: &Tab, tab_idx: usize) -> String {
    if tab.title().trim().is_empty() {
        format!("Tab {}", tab_idx + 1)
    } else {
        tab.title().to_string()
    }
}

impl PaneFlowApp {
    pub(super) fn render_tab_row(
        &self,
        ws_idx: usize,
        tab_idx: usize,
        cursor: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ws = &self.workspaces[ws_idx];
        let ws_id = ws.id;
        let tab = &ws.tabs()[tab_idx];
        let tab_id = tab.id;
        let title = tab_display_title(tab, tab_idx);
        let is_active_tab = tab_idx == ws.active_tab_idx();
        let is_active_workspace = ws_idx == self.active_idx;
        let is_renaming = self.renaming_tab == Some((ws_idx, tab_idx));

        let panes = tab.collect_panes();
        let detached_panes: Vec<_> = panes
            .iter()
            .filter(|pane| pane.read(cx).is_detached())
            .cloned()
            .collect();
        let mut surfaces: std::collections::HashSet<u64> =
            std::collections::HashSet::with_capacity(panes.len());
        let mut tab_agents: std::collections::HashSet<String> = std::collections::HashSet::new();
        for pane in &panes {
            let pane = pane.read(cx);
            for terminal in pane.terminals() {
                surfaces.insert(terminal.entity_id().as_u64());
                if let Some(agent) = terminal.read(cx).terminal.detected_agent {
                    tab_agents.insert(agent.binary().to_string());
                }
            }
        }
        let tab_sessions = || tab_row_sessions(ws.agent_sessions.values(), &surfaces);
        let row_agent_status = sidebar_agent_summary(
            tab_sessions(),
            ws.agent_completion_notification.unread_count_for(&surfaces),
        );
        let agent_status = ai_types::workspace_agent_status(tab_sessions(), &tab_agents);
        let lane = infer_lane(row_agent_status, self.tab_pull_request(ws, tab));
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        let (resting_bg, hovered_bg) = if is_active_tab && is_active_workspace {
            (
                Some(crate::app::constants::sidebar_tab_active_background()),
                None,
            )
        } else {
            (None, Some(hover_bg))
        };
        let text_color = ui.text;

        let title_el = if is_renaming {
            self.inline_rename_field(ui)
        } else {
            div()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(text_color)
                .text_size(px(14.))
                .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                .font_weight(FontWeight::MEDIUM)
                .child(self.sidebar_filter_label(title.clone(), cx))
        };

        let tab_group = SharedString::from(format!("tab-row-group-{tab_id}"));
        let indent_guide = self.cached_config.sidebar_show.indent_guide_enabled();
        let title_indent = SIDEBAR_FOLDER_SLOT_WIDTH + SIDEBAR_TITLE_ROW_GAP;
        let row_inset = if indent_guide { title_indent } else { 0. };
        let content_width = SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH - row_inset;
        let diffstat = self.render_tab_diffstat_chip(ws, tab);
        let diffstat_reserves_slot = diffstat.is_some();
        let mut title_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SIDEBAR_TITLE_ROW_GAP))
            .w(px(content_width))
            .max_w(px(content_width))
            .min_w_0()
            .when(!indent_guide, |row| {
                row.child(div().flex_none().w(px(SIDEBAR_FOLDER_SLOT_WIDTH)))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(SIDEBAR_TITLE_ROW_GAP))
                    .flex_1()
                    .min_w_0()
                    .child(title_el)
                    .when(!detached_panes.is_empty(), |titled| {
                        titled.child(
                            sidebar_action_button(
                                SharedString::from(format!("tab-detached-{tab_id}")),
                                "icons/detach-pane.svg",
                                12.,
                                "Show detached pane".into(),
                                ui,
                            )
                            .my(px(
                                (SIDEBAR_ROW_LINE_HEIGHT - SIDEBAR_ACTION_BUTTON_SIZE) / 2.
                            ))
                            .on_click(move |_, _, cx| {
                                let current = detached_panes.iter().position(|pane| {
                                    pane.read(cx).detached.is_some_and(|placement| {
                                        crate::agents::notifications::is_window_active(
                                            placement.window.window_id(),
                                        )
                                    })
                                });
                                let index =
                                    current.map_or(0, |index| (index + 1) % detached_panes.len());
                                PaneFlowApp::focus_pane_window(detached_panes[index].clone(), cx);
                                cx.stop_propagation();
                            }),
                        )
                    }),
            )
            .children(diffstat.map(|chip| {
                div()
                    .flex_none()
                    .group_hover(tab_group.clone(), |style| style.invisible())
                    .child(chip)
            }))
            .when(lane.is_some() || !diffstat_reserves_slot, |row| {
                row.child(render_lane_slot(
                    lane,
                    &format!("tab-{tab_id}"),
                    |summary| sidebar_agent_status_tooltip(summary, &agent_status),
                    tab_group.clone(),
                    ui,
                ))
            });
        title_row = title_row.child(
            sidebar_hover_actions(tab_group.clone()).child(
                sidebar_action_button(
                    SharedString::from(format!("tab-close-{tab_id}")),
                    "icons/close.svg",
                    12.,
                    "Close tab".into(),
                    ui,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if let Some((at_ws, at_tab)) = this
                        .workspaces
                        .iter()
                        .position(|ws| ws.id == ws_id)
                        .and_then(|at_ws| {
                            this.workspaces[at_ws]
                                .tabs()
                                .iter()
                                .position(|tab| tab.id == tab_id)
                                .map(|at_tab| (at_ws, at_tab))
                        })
                    {
                        this.commit_rename(cx);
                        this.close_workspace_tab(at_ws, at_tab, window, cx);
                    }
                    cx.stop_propagation();
                })),
            ),
        );

        let title_tooltip = (!is_renaming
            && title.chars().count() > SIDEBAR_TITLE_TOOLTIP_MIN_CHARS)
            .then(|| SharedString::from(title.clone()));
        let row_shell = sidebar_row_shell()
            .ml(px(row_inset))
            .id(SharedString::from(format!("tab-row-{tab_id}")))
            .role(Role::TreeItem)
            .aria_level(2)
            .aria_label(SharedString::from(title.clone()))
            .aria_selected(is_active_tab && is_active_workspace)
            .when_some(title_tooltip, |shell, title| {
                shell.delayed_tooltip(crate::ui_primitives::text_tooltip(title))
            })
            .group(tab_group.clone())
            .on_drag(
                TabDrag {
                    workspace_id: ws_id,
                    tab_id,
                    title: SharedString::from(title),
                },
                |drag, _offset, _window, cx| {
                    cx.new(|_| WorkspaceDragPreview {
                        title: drag.title.clone(),
                    })
                },
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                let is_double = matches!(e, ClickEvent::Mouse(m) if m.down.click_count == 2);
                if is_double {
                    this.begin_tab_rename(ws_idx, tab_idx, cx);
                } else {
                    this.select_workspace_tab(ws_idx, tab_idx, window, cx);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                if e.is_right_click()
                    && let Some(position) = e.mouse_position()
                {
                    this.commit_rename(cx);
                    this.dismiss_transient_surfaces();
                    this.place_sidebar_cursor(SidebarRow::Tab(ws_idx, tab_idx), window, cx);
                    this.tab_menu_open = Some(TabContextMenu {
                        ws_idx,
                        tab_idx,
                        position,
                    });
                    this.spawn_worktree_listing(ws_idx, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _window, cx| {
                if this.renaming_tab != Some((ws_idx, tab_idx)) {
                    return;
                }
                match e.keystroke.key.as_str() {
                    "enter" => {
                        this.commit_rename(cx);
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "escape" => {
                        this.renaming_tab = None;
                        cx.stop_propagation();
                        cx.notify();
                    }
                    _ => {}
                }
            }));

        let body = match self.render_tab_checkout_meta(
            ws,
            tab,
            title_indent - row_inset,
            content_width,
            cx,
        ) {
            Some(meta) => div()
                .flex()
                .flex_col()
                .gap(px(SIDEBAR_ROW_GAP))
                .child(title_row)
                .child(meta)
                .into_any_element(),
            None => title_row.into_any_element(),
        };

        let row = sidebar_row(row_shell, tab_group, resting_bg, hovered_bg, body)
            .when(cursor, |row| row.child(sidebar_cursor_ring(ui)));
        let hidden_rows = if tab_idx + 1 == ws.tab_count() {
            self.render_session_rows(
                SessionRowScope::Workspace(ws_idx),
                title_indent,
                row_inset,
                content_width,
                ui,
                cx,
            )
        } else {
            Vec::new()
        };

        div()
            .id(SharedString::from(format!("tab-drop-{tab_id}")))
            .mx(px(SIDEBAR_ROW_MARGIN_X))
            .flex_none()
            .flex()
            .flex_col()
            .relative()
            .rounded(ROW_RADIUS)
            .gap(px(SIDEBAR_ROW_SPACING))
            .when(indent_guide, |el| el.child(render_sidebar_indent_guide(ui)))
            .child(row)
            .children(hidden_rows)
    }

    fn tab_row_checkout(
        &self,
        ws: &Workspace,
        tab: &Tab,
    ) -> Option<(String, crate::workspace::GitDiffStats)> {
        let label = |branch: &str, path: &std::path::Path| {
            crate::workspace::worktree::checkout_label(Some(branch), path, &ws.worktree_root)
        };
        match tab.worktree.as_ref() {
            Some(path) => {
                let git = self.tab_checkout_git(tab)?;
                git.is_repo
                    .then(|| (label(&git.branch, path), git.stats.clone()))
            }
            None => ws.is_git_repo.then(|| {
                (
                    label(&ws.git_branch, &ws.worktree_root),
                    ws.git_stats.clone(),
                )
            }),
        }
    }

    fn render_tab_diffstat_chip(&self, ws: &Workspace, tab: &Tab) -> Option<AnyElement> {
        let show = self.cached_config.sidebar_show;
        if !show.any_enabled() {
            return None;
        }
        let (label, stats) = self.tab_row_checkout(ws, tab)?;
        if show.branch_enabled() && !label.is_empty() {
            return None;
        }
        if !tab_diffstat_visible(show, &stats) {
            return None;
        }
        Some(render_diffstat_counts(&stats, crate::theme::ui_colors()).into_any_element())
    }

    pub(super) fn render_tab_checkout_meta(
        &self,
        ws: &Workspace,
        tab: &Tab,
        indent: f32,
        width: f32,
        cx: &gpui::App,
    ) -> Option<AnyElement> {
        let ui = crate::theme::ui_colors();
        let show = self.cached_config.sidebar_show;
        if !show.any_enabled() {
            return None;
        }
        let (label, stats) = self.tab_row_checkout(ws, tab)?;
        let draw_branch = show.branch_enabled() && !label.is_empty();
        if !draw_branch {
            return None;
        }
        let draw_counts = tab_diffstat_visible(show, &stats);

        let pr = self.tab_pull_request(ws, tab);
        let branch_tooltip = match pr {
            Some(pr) => Some(format!("{label}\n{}", lane::pull_request_tooltip(pr))),
            None => {
                (label.chars().count() > SIDEBAR_TITLE_TOOLTIP_MIN_CHARS).then(|| label.clone())
            }
        };

        let branch = draw_branch.then(|| {
            div()
                .id(SharedString::from(format!("tab-branch-{}", tab.id)))
                .when_some(branch_tooltip, |branch, text| {
                    branch.delayed_tooltip(crate::ui_primitives::text_tooltip(text))
                })
                .flex()
                .flex_row()
                .items_center()
                .gap(px(3.))
                .flex_1()
                .min_w_0()
                .child(
                    svg()
                        .size(px(12.))
                        .flex_none()
                        .path(match pr {
                            Some(_) => "icons/git-pull-request.svg",
                            None => "icons/git-branch-sidebar.svg",
                        })
                        .text_color(match pr {
                            Some(pr) => pr.state.color(ui),
                            None => ui.muted,
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(14.))
                        .text_color(ui.muted)
                        .child(self.sidebar_filter_label(label, cx)),
                )
        });

        let counts = draw_counts.then(|| render_diffstat_counts(&stats, ui));

        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(6.))
                .h(px(SIDEBAR_ROW_LINE_HEIGHT))
                .pl(px(indent))
                .w(px(width))
                .max_w(px(width))
                .overflow_x_hidden()
                .when_some(branch, |row, branch| row.child(branch))
                .when_some(counts, |row, counts| row.child(counts))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::workspace::Tab;

    fn show(diffstat: bool) -> paneflow_config::schema::SidebarShow {
        paneflow_config::schema::SidebarShow {
            branch: Some(false),
            diffstat: Some(diffstat),
            pr: Some(false),
            indent_guide: Some(false),
        }
    }

    fn dirty() -> crate::workspace::GitDiffStats {
        crate::workspace::GitDiffStats {
            files_changed: 3,
            insertions: 142,
            deletions: 38,
        }
    }

    #[test]
    fn the_diffstat_needs_both_a_switch_and_something_to_report() {
        let clean = crate::workspace::GitDiffStats::default();
        assert!(
            !tab_diffstat_visible(show(false), &dirty()),
            "the counts are opt-in even on a dirty checkout"
        );
        assert!(
            !tab_diffstat_visible(show(true), &clean),
            "a clean checkout prints its branch and stops"
        );
        assert!(tab_diffstat_visible(show(true), &dirty()));
    }

    #[test]
    fn tab_display_title_falls_back_to_position() {
        let unnamed = Tab::new(String::new(), None);
        assert_eq!(tab_display_title(&unnamed, 0), "Tab 1");
        assert_eq!(tab_display_title(&unnamed, 4), "Tab 5");

        let blank = Tab::new("   ".to_string(), None);
        assert_eq!(tab_display_title(&blank, 1), "Tab 2");

        let named = Tab::new("build".to_string(), None);
        assert_eq!(tab_display_title(&named, 3), "build");
    }
}
