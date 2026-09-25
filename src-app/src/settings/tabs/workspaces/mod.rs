mod editor;

use crate::ui_primitives::TooltipDelayExt;
use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, ElementId, FontWeight, InteractiveElement,
    IntoElement, MouseButton, ParentElement, PathPromptOptions, SharedString, Styled, div,
    prelude::*, px, rgb, svg,
};
use paneflow_config::schema::{
    CommandDefinition, CommandTarget, SurfaceDefinition, WorkspaceDefinition,
};

use crate::agent_launcher::TerminalAgent;
use crate::app::workspace_ops::templates::*;
use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, apple_red, card_tint, deferred_select_menu,
    destructive_icon_button, hairline, icon_button, menu_row, quiet_card, save_icon_button,
    section_header_with_action, select_chevron, select_menu, select_trigger, setting_card,
    settings_label, switch_blue, text_field, with_alpha,
};
use crate::settings::search::Block;
use crate::ui_primitives::{AnimatedHover, AnimatedHoverExt};
use crate::{PaneFlowApp, WorkspaceTemplateDropdown};

impl PaneFlowApp {
    pub(crate) fn render_workspaces_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        if self.workspace_template_detail_open
            && let Some(idx) = self.selected_workspace_template_index()
        {
            return self.render_workspace_template_detail(idx, ui, cx);
        }

        let templates = self.workspace_template_indices();
        let create = icon_button(
            "workspace-template-create",
            "Create workspace",
            "icons/plus.svg",
            ui,
            true,
            true,
        )
        .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
            this.create_workspace_template(cx);
        }));

        let mut list = div().flex().flex_col().gap(px(10.));
        if templates.is_empty() {
            list = list.child(empty_templates_card(ui, cx));
        } else {
            for idx in templates {
                list = list.child(self.render_workspace_template_card(idx, ui, cx));
            }
        }

        div()
            .flex()
            .flex_col()
            .child(
                Block::new("Workspace templates")
                    .gap(20.)
                    .child(section_header_with_action(
                        ui,
                        "Workspace templates",
                        create,
                    ))
                    .child(list)
                    .finish(),
            )
            .child(div().h(px(160.)).flex_none())
            .into_any_element()
    }

    fn render_workspace_template_detail(
        &self,
        idx: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(command) = self.cached_config.commands.get(idx) else {
            return div().into_any_element();
        };
        let Some(workspace) = command.workspace() else {
            return div().into_any_element();
        };
        let title = workspace
            .name
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(command.name.as_str());
        let cwd = workspace.cwd.as_deref().unwrap_or("No project path");
        let pane_count = template_surfaces(workspace).len();

        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div().flex().flex_row().items_center().child(
                    icon_button(
                        "workspace-template-back",
                        "Back",
                        "icons/arrow_left.svg",
                        ui,
                        false,
                        true,
                    )
                    .on_click(cx.listener(
                        |this, _: &ClickEvent, _window, cx| {
                            this.close_workspace_template_detail(cx);
                        },
                    )),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.))
                    .child(layout_preview(
                        workspace_layout_preset(workspace),
                        pane_count.max(1),
                        ui,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ui.text)
                                    .truncate()
                                    .child(title.to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(ui.muted)
                                    .truncate()
                                    .child(cwd.to_string()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(8.))
                            .child(
                                icon_button(
                                    ("workspace-template-run", idx),
                                    "Run",
                                    "icons/player-play.svg",
                                    ui,
                                    true,
                                    pane_count > 0,
                                )
                                .when(pane_count > 0, |b| {
                                    b.on_click(cx.listener(
                                        move |this, _: &ClickEvent, _window, cx| {
                                            this.run_workspace_template_in_open_project(idx, cx);
                                        },
                                    ))
                                }),
                            )
                            .child(
                                icon_button(
                                    ("workspace-template-duplicate", idx),
                                    "Duplicate",
                                    "icons/file-text.svg",
                                    ui,
                                    false,
                                    true,
                                )
                                .on_click(cx.listener(
                                    move |this, _: &ClickEvent, _window, cx| {
                                        this.duplicate_workspace_template(idx, cx);
                                    },
                                )),
                            )
                            .child(
                                destructive_icon_button(
                                    ("workspace-template-delete", idx),
                                    "Delete",
                                    "icons/trash.svg",
                                    ui,
                                    true,
                                )
                                .on_click(cx.listener(
                                    move |this, _: &ClickEvent, _window, cx| {
                                        this.delete_workspace_template(idx, cx);
                                    },
                                )),
                            ),
                    ),
            )
            .child(self.render_workspace_template_editor(ui, cx))
            .child(div().h(px(160.)).flex_none())
            .into_any_element()
    }

    fn render_workspace_template_card(
        &self,
        idx: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(command) = self.cached_config.commands.get(idx) else {
            return div().into_any_element();
        };
        let Some(workspace) = command.workspace() else {
            return div().into_any_element();
        };
        let selected = self.workspace_template_detail_open
            && self.selected_workspace_template_index() == Some(idx);
        let title = workspace
            .name
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(command.name.as_str());
        let cwd = workspace.cwd.as_deref().unwrap_or("No project path");
        let pane_count = template_surfaces(workspace).len();
        let layout = workspace_layout_preset(workspace);
        let summary = template_summary(workspace);

        setting_card(ui)
            .when(selected, |d| {
                d.child(card_tint(with_alpha(switch_blue(), 0.08)))
            })
            .id(("workspace-template-card", idx))
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.select_workspace_template(idx, cx);
            }))
            .child(
                div()
                    .px(px(12.))
                    .py(px(10.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.))
                    .child(layout_preview(layout, pane_count, ui))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ui.text)
                                    .truncate()
                                    .child(title.to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(ui.muted)
                                    .truncate()
                                    .child(cwd.to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(ui.muted)
                                    .truncate()
                                    .child(summary),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(px(999.))
                            .bg(with_alpha(ui.text, 0.08))
                            .text_size(px(11.))
                            .text_color(ui.text)
                            .child(format!("{pane_count} panes")),
                    )
                    .child(
                        svg()
                            .size(px(14.))
                            .flex_none()
                            .path("icons/chevron-right.svg")
                            .text_color(ui.muted),
                    ),
            )
            .into_any_element()
    }

    fn create_workspace_template(&mut self, cx: &mut Context<Self>) {
        let cwd = self
            .active_workspace()
            .map(|ws| ws.cwd.clone())
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|p| p.display().to_string())
            })
            .unwrap_or_default();
        let name = self
            .active_workspace()
            .map(|ws| format!("{} setup", ws.title))
            .unwrap_or_else(|| "New workspace".to_string());
        let command = CommandDefinition {
            name: name.clone(),
            description: Some(format!("Workspace template for {cwd}")),
            keywords: vec!["workspace".to_string()],
            target: CommandTarget::Workspace {
                workspace: WorkspaceDefinition {
                    name: Some(name),
                    cwd: Some(cwd),
                    layout_preset: Some("even_h".to_string()),
                    color: None,
                    layout: None,
                },
            },
        };
        let mut commands = self.cached_config.commands.clone();
        commands.push(command);
        self.workspace_template_selected = Some(commands.len() - 1);
        self.workspace_template_detail_open = true;
        self.workspace_template_selected_pane = 0;
        self.workspace_template_status =
            Some("Draft created. Add panes to enable Run.".to_string());
        self.persist_workspace_commands(commands, cx);
        self.sync_workspace_template_inputs(cx);
    }

    fn select_workspace_template(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.workspace_template_selected = Some(idx);
        self.workspace_template_detail_open = true;
        self.workspace_template_selected_pane = 0;
        self.workspace_template_status = None;
        self.workspace_template_dropdown = None;
        self.sync_workspace_template_inputs(cx);
    }

    fn close_workspace_template_detail(&mut self, cx: &mut Context<Self>) {
        self.workspace_template_detail_open = false;
        self.workspace_template_dropdown = None;
        self.workspace_template_status = None;
        cx.notify();
    }

    fn duplicate_workspace_template(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(template) = self.cached_config.commands.get(idx).cloned() else {
            return;
        };
        let mut copy = template;
        copy.name = format!("{} copy", copy.name);
        let copy_name = copy.name.clone();
        if let Some(workspace) = copy.workspace_mut() {
            workspace.name = Some(copy_name);
        }
        let mut commands = self.cached_config.commands.clone();
        commands.push(copy);
        self.workspace_template_selected = Some(commands.len() - 1);
        self.workspace_template_detail_open = true;
        self.workspace_template_selected_pane = 0;
        self.workspace_template_status = Some("Workspace duplicated.".to_string());
        self.persist_workspace_commands(commands, cx);
        self.sync_workspace_template_inputs(cx);
    }

    fn delete_workspace_template(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.cached_config.commands.len() {
            return;
        }
        let mut commands = self.cached_config.commands.clone();
        commands.remove(idx);
        self.workspace_template_selected = commands
            .iter()
            .enumerate()
            .find_map(|(i, command)| command.workspace().is_some().then_some(i));
        self.workspace_template_detail_open = false;
        self.workspace_template_selected_pane = 0;
        self.workspace_template_status = Some("Workspace deleted.".to_string());
        self.persist_workspace_commands(commands, cx);
        self.sync_workspace_template_inputs(cx);
    }

    fn run_workspace_template_in_open_project(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.workspace_template_selected = Some(idx);
        let mut commands = self.cached_config.commands.clone();
        let result = self
            .apply_workspace_inputs(&mut commands, cx)
            .and_then(|idx| self.apply_pane_inputs(&mut commands, idx, cx).map(|_| idx))
            .and_then(|idx| {
                let params = self.workspace_up_params(&commands, idx)?;
                let project = commands
                    .get(idx)
                    .and_then(CommandDefinition::workspace)
                    .and_then(|workspace| workspace.cwd.as_deref())
                    .ok_or_else(|| "project path is required".to_string())?;
                let target_idx = self
                    .open_workspace_index_for_project(project)
                    .ok_or_else(|| "open this project first".to_string())?;
                self.launch_workspace_params_in_open_workspace(&params, target_idx, cx)
            });

        match result {
            Ok(_) => {
                self.persist_workspace_commands(commands, cx);
                self.close_settings(cx);
            }
            Err(message) => {
                self.workspace_template_status = Some(format!("Error: {message}"));
                cx.notify();
            }
        }
    }

    fn persist_workspace_commands(
        &mut self,
        commands: Vec<CommandDefinition>,
        cx: &mut Context<Self>,
    ) {
        self.cached_config =
            crate::config_writer::with_commands(&self.cached_config, commands.clone());
        let saved_message = self
            .workspace_template_status
            .as_deref()
            .filter(|message| !message.starts_with("Error:"))
            .map(|message| format!("Saved: {}", message.trim_end_matches('.')))
            .unwrap_or_else(|| "Saved workspace templates".to_string());
        self.workspace_template_status = Some("Saving workspace templates...".to_string());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let ok =
                smol::unblock(move || crate::config_writer::save_commands_checked(commands)).await;
            let _ = this.update(cx, |this, cx| {
                if ok {
                    this.workspace_template_status = Some(saved_message);
                } else {
                    log::warn!("settings: failed to persist workspace templates");
                    this.workspace_template_status = Some(
                        "Error: workspace templates changed in memory but could not be saved"
                            .to_string(),
                    );
                    this.show_toast("Workspace templates could not be saved", cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn workspace_template_indices(&self) -> Vec<usize> {
        self.cached_config
            .commands
            .iter()
            .enumerate()
            .filter_map(|(idx, command)| command.workspace().is_some().then_some(idx))
            .collect()
    }

    fn selected_workspace_template_index(&self) -> Option<usize> {
        if let Some(idx) = self.workspace_template_selected
            && self
                .cached_config
                .commands
                .get(idx)
                .and_then(CommandDefinition::workspace)
                .is_some()
        {
            return Some(idx);
        }
        self.cached_config
            .commands
            .iter()
            .position(|command| command.workspace().is_some())
    }
}

fn empty_templates_card(ui: crate::theme::UiColors, _cx: &mut Context<PaneFlowApp>) -> AnyElement {
    setting_card(ui)
        .child(
            div()
                .px(px(12.))
                .py(px(14.))
                .text_size(px(12.))
                .text_color(ui.muted)
                .child("No workspace templates yet."),
        )
        .into_any_element()
}

fn layout_preview(preset: &str, count: usize, ui: crate::theme::UiColors) -> AnyElement {
    let n = count.clamp(1, 4);
    let cell = |active: bool| {
        div().flex_1().rounded(px(3.)).bg(if active {
            with_alpha(switch_blue(), 0.72)
        } else {
            with_alpha(ui.text, 0.12)
        })
    };
    let mut preview = div()
        .w(px(54.))
        .h(px(38.))
        .p(px(4.))
        .rounded(px(7.))
        .bg(ui.subtle)
        .gap(px(3.));

    preview = match preset {
        "even_v" => {
            let mut col = preview.flex().flex_col();
            for i in 0..n {
                col = col.child(cell(i == 0));
            }
            col
        }
        "main_vertical" => preview.flex().flex_row().child(cell(true)).child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(cell(false))
                .child(cell(false)),
        ),
        "tiled" if n > 2 => preview
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .gap(px(3.))
                    .child(cell(true))
                    .child(cell(false)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .gap(px(3.))
                    .child(cell(false))
                    .child(cell(false)),
            ),
        _ => {
            let mut row = preview.flex().flex_row();
            for i in 0..n {
                row = row.child(cell(i == 0));
            }
            row
        }
    };
    preview.into_any_element()
}
