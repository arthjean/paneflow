use super::*;

impl PaneFlowApp {
    pub(super) fn render_workspace_template_editor(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(idx) = self.selected_workspace_template_index() else {
            return setting_card(ui)
                .child(
                    div()
                        .px(px(12.))
                        .py(px(14.))
                        .text_size(px(12.))
                        .text_color(ui.muted)
                        .child("Create a workspace to start."),
                )
                .into_any_element();
        };
        let Some(command) = self.cached_config.commands.get(idx) else {
            return div().into_any_element();
        };
        let Some(workspace) = command.workspace() else {
            return div().into_any_element();
        };

        let panes = template_surfaces(workspace);
        let selected_pane = self
            .workspace_template_selected_pane
            .min(panes.len().saturating_sub(1));

        let details_card = setting_card(ui)
            .child(self.workspace_text_row(
                "Workspace name",
                "Shown in the workspace list and saved with the template.",
                self.workspace_template_name_input.clone(),
                ui,
                cx,
            ))
            .child(hairline(ui))
            .child(self.workspace_project_path_row(ui, cx))
            .child(hairline(ui))
            .child(self.layout_row(workspace, ui, cx))
            .child(hairline(ui))
            .child(
                div().px(px(12.)).py(px(10.)).flex().justify_end().child(
                    save_icon_button(
                        "workspace-template-save-details",
                        "Save details",
                        "icons/check.svg",
                        ui,
                        true,
                    )
                    .on_click(cx.listener(
                        move |this, _: &ClickEvent, _window, cx| {
                            this.save_workspace_template_details(cx);
                        },
                    )),
                ),
            );

        let panes_card = self.render_workspace_panes_card(idx, &panes, selected_pane, ui, cx);
        let inspector = self.render_workspace_pane_inspector(idx, panes.get(selected_pane), ui, cx);
        let status = self.workspace_template_status.as_ref().map(|message| {
            let is_error = message.starts_with("Error:");
            let color = if is_error {
                apple_red()
            } else if message.starts_with("Saving") {
                ui.muted
            } else {
                switch_blue()
            };
            let bg = if is_error {
                with_alpha(apple_red(), 0.12)
            } else {
                with_alpha(color, 0.12)
            };
            div()
                .px(px(12.))
                .py(px(8.))
                .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                .bg(bg)
                .text_size(px(12.))
                .text_color(color)
                .child(message.clone())
        });

        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(details_card)
            .child(panes_card)
            .child(inspector)
            .when_some(status, |d, s| d.child(s))
            .into_any_element()
    }

    fn layout_row(
        &self,
        workspace: &WorkspaceDefinition,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preset = workspace_layout_preset(workspace);
        let current_label = layout_label(preset);
        let is_open = self.workspace_template_dropdown == Some(WorkspaceTemplateDropdown::Layout);
        let pane_count = template_surfaces(workspace).len().max(1);

        let mut trigger = select_trigger("workspace-layout-trigger", ui)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.workspace_template_dropdown = if is_open {
                        None
                    } else {
                        Some(WorkspaceTemplateDropdown::Layout)
                    };
                    this.settings_focus.focus(window, cx);
                    cx.notify();
                }),
            )
            .child(layout_preview(preset, pane_count, ui))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(ui.text)
                    .truncate()
                    .child(current_label),
            )
            .child(select_chevron(ui));

        if is_open {
            let mut menu = select_menu("workspace-layout-menu", ui).on_mouse_down_out(cx.listener(
                |this, _, _w, cx| {
                    if this.workspace_template_dropdown == Some(WorkspaceTemplateDropdown::Layout) {
                        this.workspace_template_dropdown = None;
                        cx.notify();
                    }
                },
            ));
            for (i, (value, label)) in LAYOUT_PRESETS.iter().enumerate() {
                let selected = preset == *value;
                let next = (*value).to_string();
                menu = menu.child(
                    menu_row(("workspace-layout", i), selected, ui)
                        .cursor(CursorStyle::Arrow)
                        .h(px(44.))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            this.workspace_template_dropdown = None;
                            this.set_workspace_template_layout(next.clone(), cx);
                        }))
                        .child(layout_preview(value, pane_count, ui))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(ui.text)
                                .child(*label),
                        ),
                );
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }

        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .child(settings_label(
                ui,
                "Layout",
                "Preset applied when the template launches.",
            ))
            .child(div().flex_shrink_0().child(trigger))
            .into_any_element()
    }

    fn render_workspace_panes_card(
        &self,
        idx: usize,
        panes: &[SurfaceDefinition],
        selected_pane: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut pane_list = div().flex().flex_col().gap(px(4.)).p(px(8.));

        if panes.is_empty() {
            pane_list = pane_list.child(
                div()
                    .px(px(12.))
                    .py(px(14.))
                    .text_size(px(12.))
                    .text_color(ui.muted)
                    .child("No panes yet."),
            );
        } else {
            for (pane_idx, pane) in panes.iter().enumerate() {
                let selected = pane_idx == selected_pane;
                let kind = pane_kind(pane);
                let resting_background = if selected {
                    with_alpha(ui.text, 0.07)
                } else {
                    with_alpha(ui.text, 0.0)
                };
                let hover_background = with_alpha(ui.text, 0.05);
                pane_list = pane_list.child(
                    div()
                        .id(("workspace-pane-row", pane_idx))
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(px(10.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.))
                        .bg(resting_background)
                        .animated_hover_bg(resting_background, hover_background)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.select_workspace_template_pane(pane_idx, cx);
                        }))
                        .child(pane_kind_icon(kind, ui))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(ui.text)
                                        .truncate()
                                        .child(surface_title(pane, pane_idx)),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(ui.muted)
                                        .truncate()
                                        .child(surface_detail(pane)),
                                ),
                        )
                        .child(kind_badge(kind, ui))
                        .child(
                            pane_delete_button(
                                SharedString::from(format!("workspace-pane-delete-{pane_idx}")),
                                ui,
                            )
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, _window, cx| {
                                    this.remove_workspace_template_pane_at(pane_idx, cx);
                                    cx.stop_propagation();
                                },
                            )),
                        ),
                );
            }
        }

        quiet_card()
            .child(pane_list)
            .child(
                div()
                    .px(px(8.))
                    .pb(px(8.))
                    .flex()
                    .flex_row()
                    .gap(px(8.))
                    .child(
                        icon_button(
                            "workspace-pane-add",
                            "Add pane",
                            "icons/plus.svg",
                            ui,
                            false,
                            true,
                        )
                        .on_click(cx.listener(
                            move |this, _: &ClickEvent, _window, cx| {
                                this.add_workspace_template_pane(idx, cx);
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    fn render_workspace_pane_inspector(
        &self,
        _idx: usize,
        pane: Option<&SurfaceDefinition>,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(pane) = pane else {
            return setting_card(ui)
                .child(
                    div()
                        .px(px(12.))
                        .py(px(14.))
                        .text_size(px(12.))
                        .text_color(ui.muted)
                        .child("Add a pane to configure it."),
                )
                .into_any_element();
        };

        let kind = pane_kind(pane);
        let visible_agents = TerminalAgent::visible(&self.cached_config);
        let mut agent_grid = div().flex().flex_col().gap(px(6.));
        for agent in visible_agents {
            let selected = pane.agent.as_deref() == Some(agent.tag());
            let resting_background = if selected {
                with_alpha(switch_blue(), 0.16)
            } else {
                ui.subtle
            };
            let hover_background = with_alpha(ui.text, 0.08);
            agent_grid = agent_grid.child(
                div()
                    .id(SharedString::from(format!(
                        "workspace-pane-agent-{}",
                        agent.tag()
                    )))
                    .px(px(9.))
                    .py(px(6.))
                    .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                    .bg(resting_background)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .animated_hover_bg(resting_background, hover_background)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.set_workspace_template_pane_agent(agent, cx);
                    }))
                    .child(agent_icon(agent, ui))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(ui.text)
                            .child(agent.display_name()),
                    ),
            );
        }

        let mut card = setting_card(ui)
            .child(
                div()
                    .px(px(12.))
                    .py(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui.text)
                            .child("Pane type"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(8.))
                            .child(pane_kind_chip(PaneKind::Agent, kind, ui, cx))
                            .child(pane_kind_chip(PaneKind::Command, kind, ui, cx))
                            .child(pane_kind_chip(PaneKind::Empty, kind, ui, cx)),
                    ),
            )
            .child(hairline(ui))
            .child(self.workspace_text_row(
                "Pane name",
                "Optional label shown on the launched pane.",
                self.workspace_pane_name_input.clone(),
                ui,
                cx,
            ))
            .child(hairline(ui))
            .child(self.workspace_text_row(
                "Pane cwd",
                "Leave empty to inherit the project path.",
                self.workspace_pane_cwd_input.clone(),
                ui,
                cx,
            ));

        if kind == PaneKind::Agent {
            card = card
                .child(hairline(ui))
                .child(
                    div()
                        .px(px(12.))
                        .py(px(10.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ui.text)
                                .child("Agent"),
                        )
                        .child(agent_grid),
                )
                .child(hairline(ui))
                .child(self.workspace_text_row(
                    "Prompt",
                    "Prefilled only. Paneflow does not submit it for you.",
                    self.workspace_pane_prompt_input.clone(),
                    ui,
                    cx,
                ));
        } else if kind == PaneKind::Command {
            card = card.child(hairline(ui)).child(self.workspace_text_row(
                "Command",
                "Shell command typed into the pane after launch.",
                self.workspace_pane_command_input.clone(),
                ui,
                cx,
            ));
        }

        card = card.child(hairline(ui)).child(
            div().px(px(12.)).py(px(10.)).flex().justify_end().child(
                save_icon_button(
                    "workspace-pane-save",
                    "Save pane",
                    "icons/check.svg",
                    ui,
                    true,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.save_workspace_template_pane(cx);
                })),
            ),
        );

        card.into_any_element()
    }

    fn workspace_text_row(
        &self,
        title: &'static str,
        description: &'static str,
        input: gpui::Entity<crate::widgets::text_input::TextInput>,
        ui: crate::theme::UiColors,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .child(settings_label(ui, title, description))
            .child(text_field(input, ui))
            .into_any_element()
    }

    fn workspace_project_path_row(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .child(settings_label(
                ui,
                "Project path",
                "Default cwd for every pane unless a pane overrides it.",
            ))
            .child(project_path_picker(
                self.workspace_template_project_input.clone(),
                ui,
                cx,
            ))
            .into_any_element()
    }

    pub(crate) fn sync_workspace_template_inputs(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected_workspace_template_index();
        self.workspace_template_selected = selected;
        let Some(idx) = selected else {
            set_input(&self.workspace_template_name_input, "", cx);
            set_input(&self.workspace_template_project_input, "", cx);
            set_input(&self.workspace_pane_name_input, "", cx);
            set_input(&self.workspace_pane_cwd_input, "", cx);
            set_input(&self.workspace_pane_command_input, "", cx);
            set_input(&self.workspace_pane_prompt_input, "", cx);
            return;
        };

        let Some(command) = self.cached_config.commands.get(idx) else {
            return;
        };
        let Some(workspace) = command.workspace() else {
            return;
        };
        let title = workspace
            .name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| command.name.clone());
        set_input(&self.workspace_template_name_input, &title, cx);
        set_input(
            &self.workspace_template_project_input,
            workspace.cwd.as_deref().unwrap_or(""),
            cx,
        );
        self.sync_workspace_pane_inputs(cx);
    }

    fn sync_workspace_pane_inputs(&mut self, cx: &mut Context<Self>) {
        let Some(idx) = self.selected_workspace_template_index() else {
            return;
        };
        let Some(workspace) = self
            .cached_config
            .commands
            .get(idx)
            .and_then(CommandDefinition::workspace)
        else {
            return;
        };
        let panes = template_surfaces(workspace);
        if panes.is_empty() {
            set_input(&self.workspace_pane_name_input, "", cx);
            set_input(&self.workspace_pane_cwd_input, "", cx);
            set_input(&self.workspace_pane_command_input, "", cx);
            set_input(&self.workspace_pane_prompt_input, "", cx);
            return;
        }
        self.workspace_template_selected_pane =
            self.workspace_template_selected_pane.min(panes.len() - 1);
        let pane = &panes[self.workspace_template_selected_pane];
        set_input(
            &self.workspace_pane_name_input,
            pane.name
                .as_deref()
                .or(pane.custom_name.as_deref())
                .unwrap_or(""),
            cx,
        );
        set_input(
            &self.workspace_pane_cwd_input,
            pane.cwd.as_deref().unwrap_or(""),
            cx,
        );
        set_input(
            &self.workspace_pane_command_input,
            pane.command.as_deref().unwrap_or(""),
            cx,
        );
        set_input(
            &self.workspace_pane_prompt_input,
            pane.prompt.as_deref().unwrap_or(""),
            cx,
        );
    }

    fn select_workspace_template_pane(&mut self, pane_idx: usize, cx: &mut Context<Self>) {
        self.workspace_template_selected_pane = pane_idx;
        self.workspace_template_status = None;
        self.sync_workspace_pane_inputs(cx);
        cx.notify();
    }

    fn save_workspace_template_details(&mut self, cx: &mut Context<Self>) {
        let mut commands = self.cached_config.commands.clone();
        match self.apply_workspace_inputs(&mut commands, cx) {
            Ok(_) => {
                self.workspace_template_status = Some("Workspace details saved.".to_string());
                self.persist_workspace_commands(commands, cx);
            }
            Err(message) => self.workspace_template_status = Some(format!("Error: {message}")),
        }
        cx.notify();
    }

    fn save_workspace_template_pane(&mut self, cx: &mut Context<Self>) {
        let mut commands = self.cached_config.commands.clone();
        let result = self
            .apply_workspace_inputs(&mut commands, cx)
            .and_then(|idx| self.apply_pane_inputs(&mut commands, idx, cx));
        match result {
            Ok(_) => {
                self.workspace_template_status = Some("Pane saved.".to_string());
                self.persist_workspace_commands(commands, cx);
            }
            Err(message) => self.workspace_template_status = Some(format!("Error: {message}")),
        }
        cx.notify();
    }

    fn pick_workspace_template_project_path(&mut self, cx: &mut Context<Self>) {
        if self.selected_workspace_template_index().is_none() {
            self.workspace_template_status = Some("Error: create a workspace first".to_string());
            cx.notify();
            return;
        }

        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                if let Ok(Ok(Some(paths))) = receiver.await {
                    let Some(path) = paths.into_iter().next() else {
                        return;
                    };
                    let path = path.to_string_lossy().into_owned();
                    cx.update(|cx| {
                        this.update(cx, |app, cx| {
                            set_input(&app.workspace_template_project_input, &path, cx);
                            app.save_workspace_template_details(cx);
                        })
                        .ok();
                    });
                }
            },
        )
        .detach();
    }

    fn set_workspace_template_layout(&mut self, preset: String, cx: &mut Context<Self>) {
        let Some(idx) = self.selected_workspace_template_index() else {
            return;
        };
        let mut commands = self.cached_config.commands.clone();
        let Some(workspace) = commands
            .get_mut(idx)
            .and_then(CommandDefinition::workspace_mut)
        else {
            return;
        };
        let panes = template_surfaces(workspace);
        workspace.layout_preset = Some(preset.clone());
        workspace.layout = build_layout_from_surfaces(&preset, panes);
        self.workspace_template_status = Some("Layout updated.".to_string());
        self.persist_workspace_commands(commands, cx);
        self.sync_workspace_template_inputs(cx);
    }

    fn add_workspace_template_pane(&mut self, idx: usize, cx: &mut Context<Self>) {
        let mut commands = self.cached_config.commands.clone();
        let Some(workspace) = commands
            .get_mut(idx)
            .and_then(CommandDefinition::workspace_mut)
        else {
            return;
        };
        let mut panes = template_surfaces(workspace);
        let mut pane = SurfaceDefinition {
            surface_type: Some("terminal".to_string()),
            name: Some(format!("Pane {}", panes.len() + 1)),
            focus: panes.is_empty().then_some(true),
            ..Default::default()
        };
        if let Some(agent) = TerminalAgent::visible(&self.cached_config).first().copied() {
            pane.agent = Some(agent.tag().to_string());
            pane.prompt = Some(String::new());
        }
        panes.push(pane);
        let preset = workspace_layout_preset(workspace).to_string();
        workspace.layout_preset = Some(preset.clone());
        workspace.layout = build_layout_from_surfaces(&preset, panes);
        self.workspace_template_selected = Some(idx);
        self.workspace_template_selected_pane = workspace
            .layout
            .as_ref()
            .map(|layout| {
                template_surfaces_from_layout(layout)
                    .len()
                    .saturating_sub(1)
            })
            .unwrap_or(0);
        self.workspace_template_status = Some("Pane added.".to_string());
        self.persist_workspace_commands(commands, cx);
        self.sync_workspace_pane_inputs(cx);
    }

    fn remove_workspace_template_pane_at(&mut self, remove_idx: usize, cx: &mut Context<Self>) {
        let Some(idx) = self.selected_workspace_template_index() else {
            return;
        };
        let mut commands = self.cached_config.commands.clone();
        let Some(workspace) = commands
            .get_mut(idx)
            .and_then(CommandDefinition::workspace_mut)
        else {
            return;
        };
        let mut panes = template_surfaces(workspace);
        if panes.is_empty() {
            return;
        }
        let remove_idx = remove_idx.min(panes.len() - 1);
        let selected_idx = self.workspace_template_selected_pane;
        panes.remove(remove_idx);
        let next_selected_pane = if panes.is_empty() {
            0
        } else if selected_idx == remove_idx {
            remove_idx.saturating_sub(1).min(panes.len() - 1)
        } else if selected_idx > remove_idx {
            selected_idx - 1
        } else {
            selected_idx.min(panes.len() - 1)
        };
        let preset = workspace_layout_preset(workspace).to_string();
        workspace.layout_preset = Some(preset.clone());
        workspace.layout = build_layout_from_surfaces(&preset, panes);
        self.workspace_template_selected_pane = next_selected_pane;
        self.workspace_template_status = Some("Pane removed.".to_string());
        self.persist_workspace_commands(commands, cx);
        self.sync_workspace_pane_inputs(cx);
    }

    fn set_workspace_template_pane_kind(&mut self, kind: PaneKind, cx: &mut Context<Self>) {
        let Some((idx, pane_idx)) = self.selected_template_and_pane() else {
            return;
        };
        let mut commands = self.cached_config.commands.clone();
        let Some(workspace) = commands
            .get_mut(idx)
            .and_then(CommandDefinition::workspace_mut)
        else {
            return;
        };
        let mut panes = template_surfaces(workspace);
        let Some(pane) = panes.get_mut(pane_idx) else {
            return;
        };
        match kind {
            PaneKind::Empty => {
                pane.agent = None;
                pane.command = None;
                pane.prompt = None;
            }
            PaneKind::Command => {
                pane.agent = None;
                pane.prompt = None;
                if pane.command.as_deref().unwrap_or("").trim().is_empty() {
                    pane.command = Some("clear && bun dev".to_string());
                }
            }
            PaneKind::Agent => {
                pane.command = None;
                pane.prompt.get_or_insert_with(String::new);
                if pane.agent.is_none()
                    && let Some(agent) = TerminalAgent::visible(&self.cached_config).first()
                {
                    pane.agent = Some(agent.tag().to_string());
                }
            }
        }
        let preset = workspace_layout_preset(workspace).to_string();
        workspace.layout = build_layout_from_surfaces(&preset, panes);
        self.workspace_template_status = Some("Pane type updated.".to_string());
        self.persist_workspace_commands(commands, cx);
        self.sync_workspace_pane_inputs(cx);
    }

    fn set_workspace_template_pane_agent(&mut self, agent: TerminalAgent, cx: &mut Context<Self>) {
        let Some((idx, pane_idx)) = self.selected_template_and_pane() else {
            return;
        };
        let mut commands = self.cached_config.commands.clone();
        let Some(workspace) = commands
            .get_mut(idx)
            .and_then(CommandDefinition::workspace_mut)
        else {
            return;
        };
        let mut panes = template_surfaces(workspace);
        let Some(pane) = panes.get_mut(pane_idx) else {
            return;
        };
        pane.agent = Some(agent.tag().to_string());
        pane.command = None;
        pane.prompt.get_or_insert_with(String::new);
        let preset = workspace_layout_preset(workspace).to_string();
        workspace.layout = build_layout_from_surfaces(&preset, panes);
        self.workspace_template_status = Some(format!("Agent set to {}.", agent.display_name()));
        self.persist_workspace_commands(commands, cx);
        self.sync_workspace_pane_inputs(cx);
    }

    fn selected_template_and_pane(&self) -> Option<(usize, usize)> {
        let idx = self.selected_workspace_template_index()?;
        let workspace = self.cached_config.commands.get(idx)?.workspace()?;
        let panes = template_surfaces(workspace);
        if panes.is_empty() {
            None
        } else {
            Some((
                idx,
                self.workspace_template_selected_pane.min(panes.len() - 1),
            ))
        }
    }

    pub(super) fn apply_workspace_inputs(
        &self,
        commands: &mut [CommandDefinition],
        cx: &mut Context<Self>,
    ) -> Result<usize, String> {
        let idx = self
            .selected_workspace_template_index()
            .ok_or_else(|| "create a workspace first".to_string())?;
        let name = input_value(&self.workspace_template_name_input, cx)
            .trim()
            .to_string();
        let project = input_value(&self.workspace_template_project_input, cx)
            .trim()
            .to_string();
        if name.is_empty() {
            return Err("workspace name is required".to_string());
        }
        if project.is_empty() {
            return Err("project path is required".to_string());
        }
        let Some(command) = commands.get_mut(idx) else {
            return Err("selected workspace no longer exists".to_string());
        };
        command.name = name.clone();
        command.description = Some(format!("Workspace template for {project}"));
        let workspace = command
            .workspace_mut()
            .ok_or_else(|| "selected command is not a workspace template".to_string())?;
        workspace.name = Some(name);
        workspace.cwd = Some(project);
        Ok(idx)
    }

    pub(super) fn apply_pane_inputs(
        &self,
        commands: &mut [CommandDefinition],
        idx: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(workspace) = commands
            .get_mut(idx)
            .and_then(CommandDefinition::workspace_mut)
        else {
            return Err("selected workspace is not a workspace template".to_string());
        };
        let mut panes = template_surfaces(workspace);
        if panes.is_empty() {
            return Ok(());
        }
        let pane_idx = self.workspace_template_selected_pane.min(panes.len() - 1);
        let name = input_value(&self.workspace_pane_name_input, cx)
            .trim()
            .to_string();
        let cwd = input_value(&self.workspace_pane_cwd_input, cx)
            .trim()
            .to_string();
        let command_input = input_value(&self.workspace_pane_command_input, cx)
            .trim()
            .to_string();
        let prompt = input_value(&self.workspace_pane_prompt_input, cx)
            .trim()
            .to_string();

        let pane = &mut panes[pane_idx];
        pane.name = (!name.is_empty()).then_some(name);
        pane.custom_name = None;
        pane.cwd = (!cwd.is_empty()).then_some(cwd);
        match pane_kind(pane) {
            PaneKind::Command => {
                if command_input.is_empty() {
                    return Err("command panes need a command".to_string());
                }
                pane.command = Some(command_input);
                pane.prompt = None;
                pane.agent = None;
            }
            PaneKind::Agent => {
                pane.command = None;
                pane.prompt = (!prompt.is_empty()).then_some(prompt);
                if pane.agent.is_none() {
                    let Some(agent) = TerminalAgent::visible(&self.cached_config).first().copied()
                    else {
                        return Err("enable at least one agent first".to_string());
                    };
                    pane.agent = Some(agent.tag().to_string());
                }
            }
            PaneKind::Empty => {
                pane.command = None;
                pane.prompt = None;
                pane.agent = None;
            }
        }

        let preset = workspace_layout_preset(workspace).to_string();
        workspace.layout = build_layout_from_surfaces(&preset, panes);
        Ok(())
    }
}

fn project_path_picker(
    input: gpui::Entity<crate::widgets::text_input::TextInput>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> impl IntoElement {
    let value = input.read(cx).value();
    let label = if value.trim().is_empty() {
        "Choose folder".to_string()
    } else {
        value
    };
    let label_color = if label == "Choose folder" {
        ui.muted
    } else {
        ui.text
    };
    let hover_background = with_alpha(ui.text, 0.08);

    div()
        .id("workspace-project-path-picker")
        .flex_1()
        .min_w(px(180.))
        .max_w(px(320.))
        .px(px(10.))
        .py(px(6.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(ui.subtle)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .animated_hover_bg(ui.subtle, hover_background)
        .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
            this.pick_workspace_template_project_path(cx);
        }))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path("icons/folder-open.svg")
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.))
                .text_color(label_color)
                .child(label),
        )
}

fn pane_delete_button(id: impl Into<ElementId>, ui: crate::theme::UiColors) -> AnimatedHover {
    let icon_color = ui.muted;
    let resting_background = with_alpha(ui.text, 0.0);
    let hover_bg = with_alpha(ui.text, 0.06);

    div()
        .id(id)
        .flex_none()
        .w(px(26.))
        .h(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .text_color(icon_color)
        .animated_hover_bg(resting_background, hover_bg)
        .delayed_tooltip(crate::ui_primitives::text_tooltip("Delete pane"))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path("icons/trash.svg")
                .text_color(icon_color),
        )
}

fn pane_kind_chip(
    kind: PaneKind,
    current: PaneKind,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let (label, icon) = match kind {
        PaneKind::Agent => ("Agent", "icons/sparkles.svg"),
        PaneKind::Command => ("Command", "icons/terminal.svg"),
        PaneKind::Empty => ("Shell", "icons/plus.svg"),
    };
    let selected = kind == current;
    let resting_background = if selected {
        with_alpha(switch_blue(), 0.16)
    } else {
        ui.subtle
    };
    let hover_background = with_alpha(ui.text, 0.08);
    div()
        .id(SharedString::from(format!("workspace-pane-kind-{label}")))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(9.))
        .py(px(5.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(resting_background)
        .text_size(px(12.))
        .text_color(ui.text)
        .animated_hover_bg(resting_background, hover_background)
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.set_workspace_template_pane_kind(kind, cx);
        }))
        .child(svg().size(px(13.)).path(icon).text_color(ui.text))
        .child(label)
        .into_any_element()
}

fn agent_icon(agent: TerminalAgent, ui: crate::theme::UiColors) -> AnyElement {
    crate::settings::components::render_logo(
        agent.icon_path(),
        agent.icon_multicolor(),
        px(16.),
        agent.accent().map(|c| rgb(c).into()).unwrap_or(ui.text),
    )
}

fn pane_kind_icon(kind: PaneKind, ui: crate::theme::UiColors) -> AnyElement {
    let icon = match kind {
        PaneKind::Agent => "icons/sparkles.svg",
        PaneKind::Command => "icons/terminal.svg",
        PaneKind::Empty => "icons/plus.svg",
    };
    svg()
        .size(px(15.))
        .flex_none()
        .path(icon)
        .text_color(ui.muted)
        .into_any_element()
}

fn kind_badge(kind: PaneKind, ui: crate::theme::UiColors) -> impl IntoElement {
    let label = match kind {
        PaneKind::Agent => "Agent",
        PaneKind::Command => "Command",
        PaneKind::Empty => "Shell",
    };
    div()
        .px(px(7.))
        .py(px(2.))
        .rounded(px(999.))
        .bg(with_alpha(ui.text, 0.08))
        .text_size(px(10.))
        .text_color(ui.muted)
        .child(label)
}

fn set_input(
    input: &gpui::Entity<crate::widgets::text_input::TextInput>,
    value: &str,
    cx: &mut Context<PaneFlowApp>,
) {
    input.update(cx, |input, cx| {
        input.set_value(SharedString::from(value.to_string()), cx);
    });
}

fn input_value(
    input: &gpui::Entity<crate::widgets::text_input::TextInput>,
    cx: &mut Context<PaneFlowApp>,
) -> String {
    input.read(cx).value()
}
