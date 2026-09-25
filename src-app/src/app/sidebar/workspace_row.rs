use super::*;

impl PaneFlowApp {
    pub(super) fn render_workspace_row(
        &self,
        i: usize,
        cursor: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ws = &self.workspaces[i];

        let title = ws.title.clone();

        let idx = i;
        let ws_id = ws.id;
        let ws_title: SharedString = ws.title.clone().into();
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        let group_name = SharedString::from(format!("ws-row-{ws_id}"));
        let is_expanded = ws.sidebar_expanded;

        let title_tooltip =
            (title.chars().count() > SIDEBAR_TITLE_TOOLTIP_MIN_CHARS).then(|| ws_title.clone());
        let row_shell = sidebar_row_shell()
            .id(SharedString::from(format!("ws-{ws_id}")))
            .role(Role::TreeItem)
            .aria_level(1)
            .aria_label(ws_title.clone())
            .aria_expanded(is_expanded)
            .aria_selected(i == self.active_idx)
            .when_some(title_tooltip, |shell, title| {
                shell.delayed_tooltip(crate::ui_primitives::text_tooltip(title))
            })
            .group(group_name.clone())
            .on_drag(
                WorkspaceDrag {
                    id: ws_id,
                    title: ws_title.clone(),
                },
                |drag, _offset, _window, cx| {
                    cx.new(|_| WorkspaceDragPreview {
                        title: drag.title.clone(),
                    })
                },
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                this.dismiss_transient_surfaces();
                let was_renaming = this.renaming_tab.is_some();
                this.commit_rename(cx);
                this.select_workspace(idx, window, cx);
                let is_double = matches!(e, ClickEvent::Mouse(m) if m.down.click_count >= 2);
                if !was_renaming && !is_double {
                    this.toggle_workspace_expanded(idx, cx);
                }
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                if e.is_right_click()
                    && let Some(position) = e.mouse_position()
                {
                    this.commit_rename(cx);
                    this.dismiss_transient_surfaces();
                    this.place_sidebar_cursor(SidebarRow::Folder(idx), window, cx);
                    this.workspace_menu_open = Some(WorkspaceContextMenu { idx, position });
                    cx.stop_propagation();
                    cx.notify();
                }
            }));

        let folder_sessions = || folder_row_sessions(ws.agent_sessions.values(), is_expanded);
        let agent_status = ai_types::workspace_agent_status(folder_sessions(), &ws.detected_agents);
        let completion_unread = if is_expanded {
            usize::from(ws.agent_completion_notification.has_unattributed_unread())
        } else {
            ws.agent_completion_notification.unread_count()
        };
        let row_agent_status = sidebar_agent_summary(folder_sessions(), completion_unread);
        let lane = infer_lane(
            row_agent_status,
            (!is_expanded)
                .then(|| self.workspace_pull_request(ws))
                .flatten(),
        );
        let title_el = div()
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(ui.text)
            .text_size(px(14.))
            .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
            .font_weight(FontWeight::MEDIUM)
            .child(self.sidebar_filter_label(title, cx));

        let folder_path = if is_expanded {
            "icons/folder-open.svg"
        } else {
            "icons/folder.svg"
        };
        let disclosure = div()
            .flex_none()
            .size(px(SIDEBAR_FOLDER_SLOT_WIDTH))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .size(px(SIDEBAR_WORKSPACE_FOLDER_ICON_WIDTH))
                    .flex_none()
                    .path(folder_path)
                    .text_color(ui.muted),
            );

        let title_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SIDEBAR_TITLE_ROW_GAP))
            .w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
            .max_w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
            .min_w_0()
            .overflow_x_hidden()
            .child(disclosure)
            .child(title_el)
            .child(render_lane_slot(
                lane,
                &format!("ws-{ws_id}"),
                |summary| sidebar_agent_status_tooltip(summary, &agent_status),
                group_name.clone(),
                ui,
            ));

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(SIDEBAR_ROW_GAP))
            .child(title_row);

        body = body.child(
            sidebar_hover_actions(group_name.clone()).child(
                sidebar_action_button(
                    SharedString::from(format!("ws-new-tab-{ws_id}")),
                    "icons/plus.svg",
                    12.,
                    SharedString::from(format!("New tab in {ws_title}")),
                    ui,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_pane_palette(idx, window, cx);
                    cx.stop_propagation();
                })),
            ),
        );

        let row = sidebar_row(row_shell, group_name.clone(), None, Some(hover_bg), body)
            .when(cursor, |row| row.child(sidebar_cursor_ring(ui)));

        div()
            .id(SharedString::from(format!("ws-drop-{ws_id}")))
            .mx(px(SIDEBAR_ROW_MARGIN_X))
            .flex_none()
            .flex()
            .flex_col()
            .rounded(ROW_RADIUS)
            .child(row)
    }
}
