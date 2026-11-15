use super::*;

use crate::ui_primitives::AnimatedHoverExt;

impl PaneFlowApp {
    pub(super) fn render_workspace_row(
        &self,
        i: usize,
        cursor: bool,
        folder: FolderGlyphs,
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
        let pick = SidebarPick::Workspace(ws_id);
        let shape = self.sidebar_pick_shape(pick);
        let picked = shape.picked;
        let row_name: SharedString = if picked >= 0.5 {
            format!("{}, selected", ws.title).into()
        } else {
            ws_title.clone()
        };

        let title_tooltip =
            (title.chars().count() > SIDEBAR_TITLE_TOOLTIP_MIN_CHARS).then(|| ws_title.clone());
        let drag_preview_width = self.sidebar_width() - 2. * SIDEBAR_ROW_MARGIN_X;
        let row_shell = sidebar_row_shell()
            .id(SharedString::from(format!("ws-{ws_id}")))
            .role(Role::TreeItem)
            .aria_level(1)
            .aria_label(row_name)
            .aria_expanded(is_expanded)
            .aria_selected(self.shown_workspace_idx() == Some(i))
            .when_some(title_tooltip, |shell, title| {
                shell.delayed_tooltip(crate::ui_primitives::text_tooltip(title))
            })
            .group(group_name.clone())
            .on_drag(
                WorkspaceDrag {
                    id: ws_id,
                    title: ws_title.clone(),
                },
                move |drag, _offset, _window, cx| {
                    cx.new(|_| WorkspaceDragPreview {
                        title: drag.title.clone(),
                        width: drag_preview_width,
                    })
                },
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                if this.sidebar_selection_click(SidebarRow::Folder(idx), e.modifiers(), window, cx)
                {
                    return;
                }
                this.dismiss_transient_surfaces();
                let was_renaming = this.renaming_tab.is_some();
                this.commit_rename(cx);
                this.select_workspace(idx, window, cx);
                if !was_renaming {
                    this.toggle_workspace_expanded(idx, cx);
                }
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                if e.is_right_click()
                    && let Some(position) = e.mouse_position()
                {
                    cx.stop_propagation();
                    if this.open_sidebar_selection_menu(
                        SidebarRow::Folder(idx),
                        position,
                        window,
                        cx,
                    ) {
                        return;
                    }
                    this.commit_rename(cx);
                    this.dismiss_transient_surfaces();
                    this.place_sidebar_cursor(SidebarRow::Folder(idx), window, cx);
                    this.workspace_menu_open = Some(WorkspaceContextMenu::at(idx, position));
                    cx.stop_propagation();
                    cx.notify();
                }
            }));

        let folder_sessions = || folder_row_sessions(ws.agent_sessions.values(), is_expanded);
        let agent_status = ai_types::workspace_agent_status(folder_sessions(), &ws.detected_agents);
        let row_agent_status = folder_row_agent_summary(self.tree_status(), ws, is_expanded);
        let lane = infer_lane(
            row_agent_status,
            (!is_expanded)
                .then(|| self.workspace_pull_request(ws))
                .flatten(),
        );
        let title_el = div()
            .flex_initial()
            .max_w_full()
            .min_w_0()
            .overflow_x_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(sidebar_row_title_color(ui, picked))
            .group_hover(group_name.clone(), move |style| style.text_color(ui.text))
            .text_size(px(14.))
            .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
            .font_weight(FontWeight::MEDIUM)
            .child(self.sidebar_filter_label(title, cx));

        let openness = self.sidebar_row_motion.borrow_mut().folder_openness(
            ws_id,
            is_expanded,
            std::time::Instant::now(),
        );
        let folder_glyph = |path: &'static str, visibility: f32| {
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .opacity(visibility)
                .child(folder.render(path, ui))
        };
        let disclosure = div()
            .relative()
            .flex_none()
            .size(px(SIDEBAR_FOLDER_SLOT_WIDTH))
            .when(openness < 1., |slot| {
                slot.child(folder_glyph(folder.closed, 1. - openness))
            })
            .when(openness > 0., |slot| {
                slot.child(folder_glyph(folder.open, openness))
            });

        let row_content_width = sidebar_workspace_row_content_width(self.sidebar_width());
        let title_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SIDEBAR_TITLE_ROW_GAP))
            .w(px(row_content_width))
            .max_w(px(row_content_width))
            .min_w_0()
            .overflow_x_hidden()
            .child(disclosure)
            .child(sidebar_title_with_trailing(
                title_el,
                self.render_workspace_branch_label(ws, cx),
                SIDEBAR_BRANCH_GAP,
            ))
            .child(div().flex_1())
            .children(
                self.render_workspace_diffstat(ws)
                    .map(|counts| div().flex_none().child(counts)),
            );
        let lane_slot = lane.is_some().then(|| {
            render_lane_slot(
                lane,
                &format!("ws-{ws_id}"),
                |summary| sidebar_agent_status_tooltip(summary, &agent_status),
                ui,
            )
        });
        let new_tab_trailing_space = if lane_slot.is_some() {
            SIDEBAR_TITLE_ROW_GAP
        } else {
            0.
        };
        let open_new_tab = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_pane_palette(idx, window, cx);
            cx.stop_propagation();
        });

        let row = sidebar_row_skin(row_shell, group_name, shape, None, Some(hover_bg))
            .animated_hover_element(move |row, reveal| {
                let new_tab_button = sidebar_hover_revealed_glyph_button(
                    SharedString::from(format!("ws-new-tab-{ws_id}")),
                    "icons/plus.svg",
                    SharedString::from(format!("New tab in {ws_title}")),
                    reveal,
                    new_tab_trailing_space,
                    ui,
                )
                .on_click(open_new_tab);
                row.extend([title_row
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_row()
                            .child(new_tab_button)
                            .children(lane_slot),
                    )
                    .into_any_element()]);
            })
            .when(picked > 0., |row| {
                row.child(sidebar_selection_fill(ui, shape))
            })
            .when(cursor, |row| {
                row.children(self.sidebar_cursor_ring_for(shape, ui))
            });

        div()
            .id(SharedString::from(format!("ws-drop-{ws_id}")))
            .mx(px(SIDEBAR_ROW_MARGIN_X))
            .flex_none()
            .flex()
            .flex_col()
            .rounded(rail_row_radius())
            .child(row)
    }

    fn render_workspace_branch_label(&self, ws: &Workspace, cx: &gpui::App) -> Option<AnyElement> {
        if !self.cached_config.sidebar_show.branch_enabled() {
            return None;
        }
        let (label, _) = workspace_checkout(ws)?;
        if label.is_empty() {
            return None;
        }
        Some(self.render_checkout_branch_label(
            SharedString::from(format!("ws-branch-{}", ws.id)),
            label,
            self.workspace_checkout_pull_request(ws),
            cx,
        ))
    }

    fn render_workspace_diffstat(&self, ws: &Workspace) -> Option<AnyElement> {
        let (_, stats) = workspace_checkout(ws)?;
        checkout_diffstat_visible(self.cached_config.sidebar_show, &stats)
            .then(|| render_diffstat_counts(&stats, crate::theme::ui_colors()).into_any_element())
    }
}
