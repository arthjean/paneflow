//! "Workspaces" settings page - visual builder for reusable workspace
//! templates backed by the existing `commands[].workspace` config surface.
//!
//! This is intentionally a `WorkspaceSpec`-level builder, not a `flow.toml`
//! DAG editor: users can compose panes, agents, shell commands, cwd and prompt
//! prefill, then launch through the same `workspace.up` path the CLI uses.

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, ElementId, FontWeight, Hsla, InteractiveElement,
    IntoElement, MouseButton, ParentElement, PathPromptOptions, SharedString, Stateful, Styled,
    div, img, prelude::*, px, rgb, svg,
};
use paneflow_config::schema::{
    CommandDefinition, LayoutNode, SurfaceDefinition, TerminalSurfaceProfile, WorkspaceDefinition,
};
use serde_json::{Value, json};

use crate::agent_launcher::TerminalAgent;
use crate::app::ipc_handler::{build_up_layout, canonicalize_workspace_cwd, sanitize_pane_name};
use crate::layout::MAX_PANES;
use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, card_colors, deferred_select_menu, hairline,
    section_header_with_action, select_chevron, select_item, select_menu, select_trigger,
    setting_card, with_alpha,
};
use crate::terminal::TerminalView;
use crate::{PaneFlowApp, WorkspaceTemplateDropdown};

const LAYOUT_PRESETS: &[(&str, &str)] = &[
    ("even_h", "Side by side"),
    ("even_v", "Stacked"),
    ("main_vertical", "Main left"),
    ("tiled", "Tiled"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaneKind {
    Empty,
    Agent,
    Command,
}

struct ExistingWorkspacePanePlan {
    cwd: Option<std::path::PathBuf>,
    command: Option<String>,
    prompt: Option<String>,
    env: Option<std::collections::HashMap<String, String>>,
    profile: TerminalSurfaceProfile,
    focus: bool,
    label: Option<String>,
}

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
            .gap(px(20.))
            .child(section_header_with_action(
                ui,
                "Workspace templates",
                create,
            ))
            .child(list)
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
        let Some(workspace) = command.workspace.as_ref() else {
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
        let Some(workspace) = command.workspace.as_ref() else {
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
            .when(selected, |d| d.bg(with_alpha(switch_blue(), 0.08)))
            .id(("workspace-template-card", idx))
            .cursor(CursorStyle::PointingHand)
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

    fn render_workspace_template_editor(
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
        let Some(workspace) = command.workspace.as_ref() else {
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
        let status = self
            .workspace_template_status
            .as_ref()
            .filter(|message| message.starts_with("Error:"))
            .map(|message| {
                div()
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                    .bg(with_alpha(apple_red(), 0.12))
                    .text_size(px(12.))
                    .text_color(apple_red())
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
                    select_item(("workspace-layout", i), selected, ui)
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
                        .cursor(CursorStyle::PointingHand)
                        .when(selected, |d| d.bg(with_alpha(ui.text, 0.07)))
                        .hover(|d| d.bg(with_alpha(ui.text, 0.05)))
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
            agent_grid = agent_grid.child(
                div()
                    .id(SharedString::from(format!(
                        "workspace-pane-agent-{}",
                        agent.tag()
                    )))
                    .px(px(9.))
                    .py(px(6.))
                    .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                    .bg(if selected {
                        with_alpha(switch_blue(), 0.16)
                    } else {
                        ui.subtle
                    })
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|d| d.bg(with_alpha(ui.text, 0.08)))
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
        let Some(workspace) = command.workspace.as_ref() else {
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
            .and_then(|cmd| cmd.workspace.as_ref())
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
            workspace: Some(WorkspaceDefinition {
                name: Some(name),
                cwd: Some(cwd),
                layout_preset: Some("even_h".to_string()),
                color: None,
                layout: None,
            }),
            command: None,
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
        let Some(workspace) = commands.get_mut(idx).and_then(|cmd| cmd.workspace.as_mut()) else {
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
        let Some(workspace) = commands.get_mut(idx).and_then(|cmd| cmd.workspace.as_mut()) else {
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
        let Some(workspace) = commands.get_mut(idx).and_then(|cmd| cmd.workspace.as_mut()) else {
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
        let Some(workspace) = commands.get_mut(idx).and_then(|cmd| cmd.workspace.as_mut()) else {
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
        let Some(workspace) = commands.get_mut(idx).and_then(|cmd| cmd.workspace.as_mut()) else {
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

    fn duplicate_workspace_template(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(template) = self.cached_config.commands.get(idx).cloned() else {
            return;
        };
        let mut copy = template;
        copy.name = format!("{} copy", copy.name);
        if let Some(workspace) = copy.workspace.as_mut() {
            workspace.name = Some(copy.name.clone());
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
            .find_map(|(i, command)| command.workspace.is_some().then_some(i));
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
                    .and_then(|command| command.workspace.as_ref())
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

    pub(crate) fn workspace_template_for_workspace(&self, workspace_idx: usize) -> Option<usize> {
        let workspace = self.workspaces.get(workspace_idx)?;
        let project = canonicalize_workspace_cwd(&workspace.cwd).ok()?;
        self.cached_config
            .commands
            .iter()
            .enumerate()
            .find_map(|(idx, command)| {
                let workflow = command.workspace.as_ref()?;
                if template_surfaces(workflow).is_empty() {
                    return None;
                }
                let workflow_cwd = workflow.cwd.as_deref()?.trim();
                if workflow_cwd.is_empty() {
                    return None;
                }
                let workflow_project = canonicalize_workspace_cwd(workflow_cwd).ok()?;
                paths_equal(&project, &workflow_project).then_some(idx)
            })
    }

    pub(crate) fn run_saved_workspace_template_for_workspace(
        &mut self,
        workspace_idx: usize,
        template_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let commands = self.cached_config.commands.clone();
        let name = commands
            .get(template_idx)
            .map(|command| command.name.clone())
            .unwrap_or_else(|| "Workflow".to_string());
        let result = self
            .workspace_up_params(&commands, template_idx)
            .and_then(|params| {
                self.launch_workspace_params_in_open_workspace(&params, workspace_idx, cx)
            });

        match result {
            Ok(_) => self.show_toast(format!("{name} started"), cx),
            Err(message) => self.show_toast(format!("Workflow failed: {message}"), cx),
        }
    }

    fn open_workspace_index_for_project(&self, project: &str) -> Option<usize> {
        let project = canonicalize_workspace_cwd(project).ok()?;
        if let Some(active) = self.workspaces.get(self.active_idx)
            && workspace_cwd_matches(&active.cwd, &project)
        {
            return Some(self.active_idx);
        }
        self.workspaces
            .iter()
            .enumerate()
            .find_map(|(idx, workspace)| {
                workspace_cwd_matches(&workspace.cwd, &project).then_some(idx)
            })
    }

    fn launch_workspace_params_in_open_workspace(
        &mut self,
        params: &Value,
        target_idx: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let preset = params
            .get("layout")
            .and_then(Value::as_str)
            .unwrap_or("even_h");
        let pane_specs = params
            .get("panes")
            .and_then(Value::as_array)
            .filter(|panes| !panes.is_empty())
            .ok_or_else(|| "add at least one pane before running".to_string())?;
        let Some(workspace) = self.workspaces.get(target_idx) else {
            return Err("open project workspace no longer exists".to_string());
        };
        if workspace.is_zoomed() {
            return Err("unzoom the project before running the template here".to_string());
        }
        if pane_specs.len() > MAX_PANES {
            return Err(format!("maximum pane count reached ({MAX_PANES})"));
        }

        let mut planned = Vec::with_capacity(pane_specs.len());
        for spec in pane_specs {
            let cwd = match spec.get("cwd").and_then(Value::as_str) {
                Some(raw) => Some(canonicalize_workspace_cwd(raw).map_err(|err| err.message)?),
                None => None,
            };
            planned.push(ExistingWorkspacePanePlan {
                cwd,
                command: spec
                    .get("command")
                    .and_then(Value::as_str)
                    .filter(|command| !command.is_empty())
                    .map(str::to_string),
                prompt: spec
                    .get("prompt")
                    .and_then(Value::as_str)
                    .filter(|prompt| !prompt.is_empty())
                    .map(str::to_string),
                env: launch_env_from_value(spec.get("env")),
                profile: launch_profile_from_value(spec.get("profile")),
                focus: spec.get("focus").and_then(Value::as_bool).unwrap_or(false),
                label: spec
                    .get("label")
                    .or_else(|| spec.get("name"))
                    .and_then(Value::as_str)
                    .and_then(sanitize_pane_name),
            });
        }

        let ws_id = self.workspaces[target_idx].id;
        let focus_idx = planned.iter().position(|plan| plan.focus).unwrap_or(0);
        let mut launches = Vec::with_capacity(planned.len());
        let mut panes = Vec::with_capacity(planned.len());
        for plan in planned {
            let terminal = cx.new(|cx| {
                TerminalView::with_cwd_env_and_profile(
                    ws_id,
                    plan.cwd.clone(),
                    None,
                    plan.env.clone(),
                    plan.profile,
                    cx,
                )
            });
            if let Some(label) = plan.label {
                terminal.update(cx, |view, _cx| {
                    view.terminal.custom_name = Some(label);
                });
            }
            let new_pane = self.create_pane(terminal.clone(), ws_id, cx);
            launches.push((terminal, plan.command, plan.prompt));
            panes.push(new_pane);
        }
        let focus_pane = panes.get(focus_idx).cloned();
        let tree = build_up_layout(preset, panes, focus_idx)
            .ok_or_else(|| "could not build layout from panes".to_string())?;
        if let Some(workspace) = self.workspaces.get_mut(target_idx) {
            workspace.root = Some(tree);
            workspace.saved_layout = None;
            if let Some(first_cwd) = launches
                .iter()
                .find_map(|(terminal, _, _)| terminal.read(cx).terminal.cwd_now())
            {
                workspace.cwd = first_cwd.display().to_string();
            }
        }

        if self.active_idx != target_idx {
            self.active_idx = target_idx;
            self.reroot_files_tree(cx);
        }
        if let Some(pane) = focus_pane {
            self.pending_pane_focus = Some(pane);
        }
        for (pane_idx, (terminal, command, prompt)) in launches.into_iter().enumerate() {
            if let Some(command) = command {
                Self::schedule_launch_command(&terminal, command, prompt, pane_idx, cx);
            } else if let Some(prompt) = prompt {
                Self::schedule_prompt_prefill(&terminal, prompt, pane_idx, cx);
            }
        }
        self.save_session(cx);
        cx.notify();
        Ok(())
    }

    fn persist_workspace_commands(
        &mut self,
        commands: Vec<CommandDefinition>,
        cx: &mut Context<Self>,
    ) {
        self.cached_config =
            crate::config_writer::with_commands(&self.cached_config, commands.clone());
        cx.notify();
        cx.background_spawn(async move {
            smol::unblock(move || {
                if !crate::config_writer::save_commands_checked(commands) {
                    log::warn!("settings: failed to persist workspace templates");
                }
            })
            .await;
        })
        .detach();
    }

    fn workspace_template_indices(&self) -> Vec<usize> {
        self.cached_config
            .commands
            .iter()
            .enumerate()
            .filter_map(|(idx, command)| command.workspace.is_some().then_some(idx))
            .collect()
    }

    fn selected_workspace_template_index(&self) -> Option<usize> {
        if let Some(idx) = self.workspace_template_selected
            && self
                .cached_config
                .commands
                .get(idx)
                .and_then(|command| command.workspace.as_ref())
                .is_some()
        {
            return Some(idx);
        }
        self.cached_config
            .commands
            .iter()
            .position(|command| command.workspace.is_some())
    }

    fn selected_template_and_pane(&self) -> Option<(usize, usize)> {
        let idx = self.selected_workspace_template_index()?;
        let workspace = self.cached_config.commands.get(idx)?.workspace.as_ref()?;
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

    fn apply_workspace_inputs(
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
            .workspace
            .get_or_insert_with(|| WorkspaceDefinition {
                name: Some(name.clone()),
                cwd: Some(project.clone()),
                layout_preset: Some("even_h".to_string()),
                color: None,
                layout: None,
            });
        workspace.name = Some(name);
        workspace.cwd = Some(project);
        Ok(idx)
    }

    fn apply_pane_inputs(
        &self,
        commands: &mut [CommandDefinition],
        idx: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(workspace) = commands.get_mut(idx).and_then(|cmd| cmd.workspace.as_mut()) else {
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
                        return Err("enable at least one AI Agent first".to_string());
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

    fn workspace_up_params(
        &self,
        commands: &[CommandDefinition],
        idx: usize,
    ) -> Result<Value, String> {
        let command = commands
            .get(idx)
            .ok_or_else(|| "selected workspace no longer exists".to_string())?;
        let workspace = command
            .workspace
            .as_ref()
            .ok_or_else(|| "selected command is not a workspace".to_string())?;
        let project = workspace
            .cwd
            .as_deref()
            .filter(|cwd| !cwd.trim().is_empty())
            .ok_or_else(|| "project path is required".to_string())?;
        let panes = template_surfaces(workspace);
        if panes.is_empty() {
            return Err("add at least one pane before running".to_string());
        }
        let visible_agents = TerminalAgent::visible(&self.cached_config);
        let mut pane_values = Vec::with_capacity(panes.len());
        for (pane_idx, pane) in panes.iter().enumerate() {
            let has_command = pane
                .command
                .as_deref()
                .is_some_and(|command| !command.trim().is_empty());
            let has_agent = pane
                .agent
                .as_deref()
                .is_some_and(|agent| !agent.trim().is_empty());
            if has_command && has_agent {
                return Err(format!("pane {} has both agent and command", pane_idx + 1));
            }

            let mut spec = serde_json::Map::new();
            spec.insert(
                "cwd".to_string(),
                Value::String(pane.cwd.clone().unwrap_or_else(|| project.to_string())),
            );
            if let Some(label) = pane
                .name
                .as_deref()
                .or(pane.custom_name.as_deref())
                .filter(|label| !label.trim().is_empty())
            {
                spec.insert("name".to_string(), Value::String(label.trim().to_string()));
            }
            if let Some(true) = pane.focus {
                spec.insert("focus".to_string(), Value::Bool(true));
            }
            if let Some(env) = pane.env.as_ref().filter(|env| !env.is_empty()) {
                spec.insert("env".to_string(), json!(env));
            }
            if let Some(agent_tag) = pane.agent.as_deref().filter(|tag| !tag.trim().is_empty()) {
                let Some(agent) = TerminalAgent::from_tag(agent_tag) else {
                    return Err(format!("pane {} uses an unknown agent", pane_idx + 1));
                };
                if !visible_agents.contains(&agent) {
                    return Err(format!(
                        "enable {} in AI Agent settings before running",
                        agent.display_name()
                    ));
                }
                spec.insert(
                    "command".to_string(),
                    Value::String(agent.launch_command(&self.cached_config)),
                );
                spec.insert("profile".to_string(), Value::String("agent".to_string()));
            } else if let Some(command) = pane.command.as_deref().filter(|c| !c.trim().is_empty()) {
                spec.insert(
                    "command".to_string(),
                    Value::String(command.trim().to_string()),
                );
            }
            if let Some(prompt) = pane
                .prompt
                .as_deref()
                .filter(|prompt| !prompt.trim().is_empty())
            {
                spec.insert("prompt".to_string(), Value::String(prompt.to_string()));
            }
            pane_values.push(Value::Object(spec));
        }

        Ok(json!({
            "name": workspace
                .name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(command.name.as_str()),
            "layout": workspace_layout_preset(workspace),
            "panes": pane_values,
        }))
    }
}

fn settings_label(
    ui: crate::theme::UiColors,
    title: &'static str,
    description: &'static str,
) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .text_size(crate::ui_primitives::BODY_EMPHASIS)
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui.text)
                .child(title),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(ui.muted)
                .child(description),
        )
}

fn workspace_cwd_matches(cwd: &str, project: &std::path::Path) -> bool {
    canonicalize_workspace_cwd(cwd)
        .ok()
        .is_some_and(|cwd| paths_equal(&cwd, project))
}

fn paths_equal(left: &std::path::Path, right: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn launch_env_from_value(
    value: Option<&Value>,
) -> Option<std::collections::HashMap<String, String>> {
    let object = value?.as_object()?;
    let env = object
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
        .collect::<std::collections::HashMap<_, _>>();
    (!env.is_empty()).then_some(env)
}

fn launch_profile_from_value(value: Option<&Value>) -> TerminalSurfaceProfile {
    match value.and_then(Value::as_str) {
        Some("agent") => TerminalSurfaceProfile::Agent,
        Some("review") => TerminalSurfaceProfile::Review,
        Some("cached") => TerminalSurfaceProfile::Cached,
        _ => TerminalSurfaceProfile::Normal,
    }
}

fn switch_blue() -> Hsla {
    Hsla::from(rgb(0x339cff))
}

fn apple_red() -> Hsla {
    Hsla::from(rgb(0xff453a))
}

fn quiet_card() -> gpui::Div {
    let (bg, _) = card_colors();
    div()
        .flex()
        .flex_col()
        .bg(bg)
        .rounded(px(16.))
        .overflow_hidden()
}

fn text_field(
    input: gpui::Entity<crate::widgets::text_input::TextInput>,
    ui: crate::theme::UiColors,
) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .w(px(260.))
        .px(px(10.))
        .py(px(6.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(ui.subtle)
        .text_size(px(12.))
        .text_color(ui.text)
        .child(input)
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

    div()
        .id("workspace-project-path-picker")
        .flex_shrink_0()
        .w(px(260.))
        .px(px(10.))
        .py(px(6.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(ui.subtle)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .cursor(CursorStyle::PointingHand)
        .hover(|d| d.bg(with_alpha(ui.text, 0.08)))
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

fn icon_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: &'static str,
    ui: crate::theme::UiColors,
    primary: bool,
    enabled: bool,
) -> Stateful<gpui::Div> {
    let bg = if primary { switch_blue() } else { ui.subtle };
    let fg = if primary { gpui::white() } else { ui.text };
    let disabled_bg = ui.subtle;
    let disabled_fg = ui.muted;
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(5.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(if enabled { bg } else { disabled_bg })
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if enabled { fg } else { disabled_fg })
        .when(enabled, |d| {
            d.cursor(CursorStyle::PointingHand).hover(move |s| {
                s.bg(if primary {
                    with_alpha(bg, 0.86)
                } else {
                    with_alpha(ui.text, 0.06)
                })
            })
        })
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(if enabled { fg } else { disabled_fg }),
        )
        .child(label.into())
}

fn pane_delete_button(id: impl Into<ElementId>, ui: crate::theme::UiColors) -> Stateful<gpui::Div> {
    let icon_color = ui.muted;
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
        .cursor(CursorStyle::PointingHand)
        .hover(move |s| s.bg(hover_bg))
        .tooltip(crate::ui_primitives::text_tooltip("Delete pane"))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path("icons/trash.svg")
                .text_color(icon_color),
        )
}

fn destructive_icon_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: &'static str,
    ui: crate::theme::UiColors,
    enabled: bool,
) -> Stateful<gpui::Div> {
    let bg = apple_red();
    let hover_bg = Hsla {
        l: (bg.l - 0.05).max(0.0),
        ..bg
    };
    let fg = gpui::white();
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(5.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(if enabled { bg } else { ui.subtle })
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if enabled { fg } else { ui.muted })
        .when(enabled, |d| {
            d.cursor(CursorStyle::PointingHand)
                .hover(move |s| s.bg(hover_bg))
        })
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(if enabled { fg } else { ui.muted }),
        )
        .child(label.into())
}

fn save_icon_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: &'static str,
    ui: crate::theme::UiColors,
    enabled: bool,
) -> Stateful<gpui::Div> {
    let light_theme = ui.surface.l > 0.5;
    let bg: Hsla = if light_theme {
        rgb(0x000000).into()
    } else {
        gpui::white()
    };
    let fg: Hsla = if light_theme {
        gpui::white()
    } else {
        rgb(0x000000).into()
    };
    let hover_bg = Hsla { a: 0.86, ..bg };

    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(5.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(if enabled { bg } else { ui.subtle })
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if enabled { fg } else { ui.muted })
        .when(enabled, |d| {
            d.cursor(CursorStyle::PointingHand)
                .hover(move |s| s.bg(hover_bg))
        })
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(if enabled { fg } else { ui.muted }),
        )
        .child(label.into())
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
    div()
        .id(SharedString::from(format!("workspace-pane-kind-{label}")))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(9.))
        .py(px(5.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(if selected {
            with_alpha(switch_blue(), 0.16)
        } else {
            ui.subtle
        })
        .text_size(px(12.))
        .text_color(ui.text)
        .cursor(CursorStyle::PointingHand)
        .hover(|d| d.bg(with_alpha(ui.text, 0.08)))
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.set_workspace_template_pane_kind(kind, cx);
        }))
        .child(svg().size(px(13.)).path(icon).text_color(ui.text))
        .child(label)
        .into_any_element()
}

fn agent_icon(agent: TerminalAgent, ui: crate::theme::UiColors) -> AnyElement {
    let path = SharedString::from(agent.icon_path());
    if agent.icon_multicolor() {
        img(path).size(px(16.)).flex_none().into_any_element()
    } else {
        let tint: Hsla = agent.accent().map(|c| rgb(c).into()).unwrap_or(ui.text);
        svg()
            .size(px(16.))
            .flex_none()
            .path(path)
            .text_color(tint)
            .into_any_element()
    }
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
        input.content = SharedString::from(value.to_string());
        let len = input.content.len();
        input.selected_range = len..len;
        cx.notify();
    });
}

fn input_value(
    input: &gpui::Entity<crate::widgets::text_input::TextInput>,
    cx: &mut Context<PaneFlowApp>,
) -> String {
    input.read(cx).value()
}

fn workspace_layout_preset(workspace: &WorkspaceDefinition) -> &str {
    workspace
        .layout_preset
        .as_deref()
        .filter(|preset| LAYOUT_PRESETS.iter().any(|(value, _)| value == preset))
        .unwrap_or("even_h")
}

fn layout_label(preset: &str) -> &'static str {
    LAYOUT_PRESETS
        .iter()
        .find_map(|(value, label)| (*value == preset).then_some(*label))
        .unwrap_or("Side by side")
}

fn template_surfaces(workspace: &WorkspaceDefinition) -> Vec<SurfaceDefinition> {
    workspace
        .layout
        .as_ref()
        .map(template_surfaces_from_layout)
        .unwrap_or_default()
}

fn template_surfaces_from_layout(layout: &LayoutNode) -> Vec<SurfaceDefinition> {
    let mut out = Vec::new();
    collect_surfaces(layout, &mut out);
    out
}

fn collect_surfaces(node: &LayoutNode, out: &mut Vec<SurfaceDefinition>) {
    match node {
        LayoutNode::Pane { surfaces } => {
            if surfaces.is_empty() {
                out.push(Default::default());
            } else {
                out.extend(surfaces.iter().cloned());
            }
        }
        LayoutNode::Split { children, .. } => {
            for child in children {
                collect_surfaces(child, out);
            }
        }
    }
}

fn build_layout_from_surfaces(
    preset: &str,
    surfaces: Vec<SurfaceDefinition>,
) -> Option<LayoutNode> {
    let leaves: Vec<LayoutNode> = surfaces
        .into_iter()
        .map(|surface| LayoutNode::Pane {
            surfaces: vec![surface],
        })
        .collect();
    if leaves.is_empty() {
        return None;
    }
    if leaves.len() == 1 {
        return leaves.into_iter().next();
    }
    match preset {
        "even_v" => Some(split("horizontal", leaves, None)),
        "main_vertical" => Some(main_vertical_layout(leaves)),
        "tiled" => Some(tiled_layout(leaves)),
        _ => Some(split("vertical", leaves, None)),
    }
}

fn split(direction: &str, children: Vec<LayoutNode>, ratios: Option<Vec<f64>>) -> LayoutNode {
    LayoutNode::Split {
        direction: direction.to_string(),
        ratio: None,
        ratios,
        children,
    }
}

fn main_vertical_layout(mut leaves: Vec<LayoutNode>) -> LayoutNode {
    let main = leaves.remove(0);
    let side = if leaves.len() == 1 {
        leaves.remove(0)
    } else {
        split("horizontal", leaves, None)
    };
    split("vertical", vec![main, side], Some(vec![0.5, 0.5]))
}

fn tiled_layout(leaves: Vec<LayoutNode>) -> LayoutNode {
    if leaves.len() <= 2 {
        return split("vertical", leaves, None);
    }
    let n = leaves.len();
    let mut rows = 1usize;
    let mut cols = 1usize;
    while rows * cols < n {
        if cols <= rows {
            cols += 1;
        } else {
            rows += 1;
        }
    }

    let row_ratio = 1.0 / rows as f64;
    let mut leaf_iter = leaves.into_iter();
    let mut row_nodes = Vec::with_capacity(rows);
    for row_idx in 0..rows {
        let panes_in_row = if row_idx < rows - 1 {
            cols
        } else {
            n - cols * (rows - 1)
        };
        let mut row_leaves: Vec<_> = leaf_iter.by_ref().take(panes_in_row).collect();
        let row_node = if row_leaves.len() == 1 {
            row_leaves.remove(0)
        } else {
            split("vertical", row_leaves, None)
        };
        row_nodes.push(row_node);
    }
    split("horizontal", row_nodes, Some(vec![row_ratio; rows]))
}

fn pane_kind(surface: &SurfaceDefinition) -> PaneKind {
    if surface
        .agent
        .as_deref()
        .is_some_and(|agent| !agent.trim().is_empty())
    {
        PaneKind::Agent
    } else if surface
        .command
        .as_deref()
        .is_some_and(|command| !command.trim().is_empty())
    {
        PaneKind::Command
    } else {
        PaneKind::Empty
    }
}

#[cfg(test)]
mod layout_tests {
    use paneflow_config::schema::LayoutNode;

    use super::tiled_layout;

    fn leaf() -> LayoutNode {
        LayoutNode::Pane {
            surfaces: Vec::new(),
        }
    }

    fn leaf_count(node: &LayoutNode) -> usize {
        match node {
            LayoutNode::Pane { .. } => 1,
            LayoutNode::Split { children, .. } => children.iter().map(leaf_count).sum(),
        }
    }

    #[test]
    fn tiled_layout_matches_runtime_grid_for_seven_panes() {
        let leaves = (0..7).map(|_| leaf()).collect();
        let layout = tiled_layout(leaves);

        let LayoutNode::Split {
            direction,
            ratios,
            children,
            ..
        } = layout
        else {
            panic!("tiled layout should split rows");
        };

        assert_eq!(direction, "horizontal");
        assert_eq!(ratios, Some(vec![1.0 / 3.0; 3]));
        assert_eq!(children.len(), 3);
        assert_eq!(
            children.iter().map(leaf_count).collect::<Vec<_>>(),
            vec![3, 3, 1]
        );
    }
}

fn surface_title(surface: &SurfaceDefinition, idx: usize) -> String {
    surface
        .name
        .as_deref()
        .or(surface.custom_name.as_deref())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Pane {}", idx + 1))
}

fn surface_detail(surface: &SurfaceDefinition) -> String {
    if let Some(agent) = surface.agent.as_deref().and_then(TerminalAgent::from_tag) {
        agent.display_name().to_string()
    } else if let Some(command) = surface.command.as_deref().filter(|c| !c.trim().is_empty()) {
        command.to_string()
    } else {
        "Empty shell".to_string()
    }
}

fn template_summary(workspace: &WorkspaceDefinition) -> String {
    let panes = template_surfaces(workspace);
    if panes.is_empty() {
        return "Draft, no panes yet".to_string();
    }
    let agents = panes
        .iter()
        .filter(|pane| pane_kind(pane) == PaneKind::Agent)
        .count();
    let commands = panes
        .iter()
        .filter(|pane| pane_kind(pane) == PaneKind::Command)
        .count();
    let shells = panes.len().saturating_sub(agents + commands);
    format!("{agents} agents · {commands} commands · {shells} shells")
}
