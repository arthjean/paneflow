use gpui::{App, KeyDownEvent, MouseDownEvent, PathPromptOptions};
use paneflow_config::schema::LayoutNode;

use super::canvas::{
    AGENT_GLYPH, COMMAND_GLYPH, SHELL_GLYPH, agent_logo, glyph, pane_agent, pane_icon,
    preset_glyph, splits_side_by_side, template_layout,
};
use super::*;
use crate::WorkspaceTemplateDropdown;
use crate::settings::components::{
    CARD_PADDING_X, SETTINGS_ROW_PADDING_Y, card_button, deferred_select_menu, destructive_color,
    ghost_skin, ghost_tint_skin, hairline, input_frame, raised_button, row_text, select_chevron,
    select_menu, select_option, select_trigger, settings_label, soft_icon_button, text_field,
};
use crate::ui_primitives::{TooltipDelayExt, squircle::squircle_border, text_tooltip};

const SECTION_GAP: f32 = 24.;
const IDENTITY_PADDING: f32 = 16.;
const NAME_FIELD_WIDTH: f32 = 320.;
const NAME_FIELD_HEIGHT: f32 = 32.;
const CANVAS_INSET: f32 = 6.;
const CANVAS_HEIGHT: f32 = 196.;
const CANVAS_HEIGHT_TALL: f32 = 264.;
const CANVAS_TALL_FROM: usize = 5;
const TILE_GAP: f32 = 6.;
const TILE_ICON: f32 = 20.;
const TILE_ROW_ICON: f32 = 18.;
const TILE_ROW_PADDING_X: f32 = 14.;
const TILE_SELECTED_RING: f32 = 1.5;
const PILL_HEIGHT: f32 = 28.;
const PRESET_GLYPH_WIDTH: f32 = 18.;
const PRESET_GLYPH_HEIGHT: f32 = 12.;
const FIELD_MIN_WIDTH: f32 = 180.;
const FIELD_MAX_WIDTH: f32 = 320.;
const AGENT_OPTION_LOGO: f32 = 16.;
const DETAIL_BOTTOM_SPACE: f32 = 136.;
const SELECTED_FILL: f32 = 0.10;
const TILE_SELECTED_FILL: f32 = 0.08;
const TILE_FILL: f32 = 0.045;
const TILE_HOVER: f32 = 0.075;
const PILL_HOVER: f32 = 0.06;
const TILE_ROW_MIN: f32 = 52.;
const TILE_COLUMN_MIN: f32 = 84.;
const SELECTED_RING_ALPHA: f32 = 0.4;

impl PaneFlowApp {
    pub(super) fn render_workspace_template_detail(
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
        let title = template_title(command, workspace);
        let panes = template_surfaces(workspace);
        let selected_pane = self
            .workspace_template_selected_pane
            .min(panes.len().saturating_sub(1));

        let mut block = Block::new(BLOCK).gap(SECTION_GAP);
        if let Some(message) = self.template_notice(TemplateNoticeAt::Launch) {
            block = block.child(render_hint(message, HintTone::Error, ui));
        }
        block = block
            .child(self.render_template_identity(workspace, ui, cx))
            .child(self.render_template_layout(workspace, &panes, selected_pane, ui, cx));
        if let Some(pane) = panes.get(selected_pane) {
            block = block.child(self.render_template_pane(pane, selected_pane, ui, cx));
        }
        block
            .child(self.render_template_manage(idx, title, ui, cx))
            .child(div().h(px(DETAIL_BOTTOM_SPACE)).flex_none())
            .finish()
    }

    fn render_template_identity(
        &self,
        workspace: &WorkspaceDefinition,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name_field = input_frame(
            div()
                .id("workspace-template-name")
                .w_full()
                .max_w(px(NAME_FIELD_WIDTH))
                .h(px(NAME_FIELD_HEIGHT))
                .flex()
                .items_center()
                .px(px(10.)),
            &self.workspace_template_name_input,
            self.template_notice(TemplateNoticeAt::Identity).is_some(),
            ui,
        )
        .text_size(TITLE)
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(ui.text)
        .cursor(CursorStyle::IBeam)
        .aria_label("Template name")
        .child(self.workspace_template_name_input.clone());

        let project = workspace
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|cwd| !cwd.is_empty());
        let (project_label, project_color) = match project {
            Some(cwd) => (
                crate::settings::tabs::worktrees::display_path(std::path::Path::new(cwd)),
                ui.text,
            ),
            None => ("Choose a project folder".to_string(), ui.muted),
        };
        let picker = ghost_skin(
            div()
                .id("workspace-template-project")
                .flex_none()
                .max_w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(5.)),
            "workspace-template-project-squircle",
            ROW_RADIUS,
            ui,
        )
        .accessible_control(gpui::accesskit::Role::Button, "Choose the project folder")
        .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
            this.pick_workspace_template_project_path(cx);
        }))
        .child(glyph(
            "icons/settings/folder-open.svg",
            14.,
            with_alpha(ui.text, 0.8),
        ))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(BODY)
                .text_color(project_color)
                .child(project_label),
        );

        setting_card(ui)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .p(px(IDENTITY_PADDING))
                    .child(self.commit_on_leave(name_field, cx))
                    .child(div().ml(px(-2.)).flex().child(picker)),
            )
            .when_some(
                self.template_notice(TemplateNoticeAt::Identity),
                |card, message| {
                    card.child(
                        div()
                            .px(px(IDENTITY_PADDING))
                            .pb(px(14.))
                            .child(render_hint(message, HintTone::Error, ui)),
                    )
                },
            )
            .into_any_element()
    }

    fn render_template_layout(
        &self,
        workspace: &WorkspaceDefinition,
        panes: &[SurfaceDefinition],
        selected_pane: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preset = workspace_layout_preset(workspace);
        let add = soft_icon_button(
            "workspace-pane-add",
            "Add pane",
            "icons/settings/add-01.svg",
            ui,
        )
        .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
            this.add_workspace_template_pane(cx);
        }));
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .pb(px(8.))
            .child(
                div()
                    .flex_1()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child("Layout"),
            )
            .child(self.render_preset_control(preset, panes.len(), ui, cx))
            .child(add);

        let node = template_layout(workspace);
        let height = canvas_height(node.as_ref(), panes.len());
        let canvas = if panes.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .rounded(ROW_RADIUS)
                .bg(with_alpha(ui.text, TILE_FILL))
                .text_size(BODY)
                .text_color(ui.muted)
                .child("Add a pane to start the layout.")
                .into_any_element()
        } else {
            match node {
                Some(node) => {
                    let mut cursor = CanvasCursor {
                        panes,
                        selected: selected_pane,
                        next: 0,
                    };
                    self.render_canvas_node(&node, height, &mut cursor, ui, cx)
                }
                None => div().into_any_element(),
            }
        };

        div()
            .flex()
            .flex_col()
            .child(header)
            .child(
                setting_card(ui)
                    .p(px(CANVAS_INSET))
                    .child(div().h(px(height)).flex().child(canvas)),
            )
            .into_any_element()
    }

    fn render_canvas_node(
        &self,
        node: &LayoutNode,
        height: f32,
        cursor: &mut CanvasCursor<'_>,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            LayoutNode::Pane { .. } => {
                let pane_idx = cursor.next;
                cursor.next += 1;
                self.render_pane_tile(
                    pane_idx,
                    cursor.panes.get(pane_idx),
                    pane_idx == cursor.selected,
                    height >= TILE_COLUMN_MIN,
                    ui,
                    cx,
                )
            }
            LayoutNode::Split {
                direction,
                children,
                ..
            } => {
                let side_by_side = splits_side_by_side(direction);
                let child_height = if side_by_side {
                    height
                } else {
                    stacked_share(height, children.len())
                };
                let mut split = div().flex_1().min_w_0().min_h_0().flex().gap(px(TILE_GAP));
                split = if side_by_side {
                    split.flex_row()
                } else {
                    split.flex_col()
                };
                for child in children {
                    split =
                        split.child(self.render_canvas_node(child, child_height, cursor, ui, cx));
                }
                split.into_any_element()
            }
        }
    }

    fn render_pane_tile(
        &self,
        pane_idx: usize,
        pane: Option<&SurfaceDefinition>,
        selected: bool,
        upright: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let fallback = SurfaceDefinition::default();
        let pane = pane.unwrap_or(&fallback);
        let title = surface_title(pane, pane_idx);
        let detail = pane_detail(pane);
        let detail = (!detail.eq_ignore_ascii_case(&title)).then_some(detail);
        let (rest, hover) = if selected {
            (TILE_SELECTED_FILL, None)
        } else {
            (TILE_FILL, Some(with_alpha(ui.text, TILE_HOVER)))
        };
        let text_align = |line: gpui::Div| {
            if upright { line.text_center() } else { line }
        };
        let label = div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .map(|text| {
                if upright {
                    text.w_full().items_center()
                } else {
                    text.flex_1()
                }
            })
            .child(
                text_align(div().w_full())
                    .truncate()
                    .text_size(BODY)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(ui.text)
                    .child(title.clone()),
            )
            .when_some(detail.clone(), |text, detail| {
                text.child(
                    text_align(div().w_full())
                        .truncate()
                        .text_size(LABEL_SM)
                        .text_color(ui.muted)
                        .child(detail),
                )
            });
        let frame = div()
            .id(("workspace-pane-tile", pane_idx))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .cursor(CursorStyle::PointingHand);
        let frame = if upright {
            frame
                .flex_col()
                .justify_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(8.))
        } else {
            frame.flex_row().gap(px(10.)).px(px(TILE_ROW_PADDING_X))
        };
        squircle_skin(
            frame,
            format!("workspace-pane-tile-{pane_idx}"),
            ROW_RADIUS,
            Some(with_alpha(ui.text, rest)),
            hover,
        )
        .role(gpui::accesskit::Role::RadioButton)
        .aria_label(match &detail {
            Some(detail) => format!("{title}, {detail}"),
            None => title.clone(),
        })
        .aria_toggled(if selected {
            gpui::accesskit::Toggled::True
        } else {
            gpui::accesskit::Toggled::False
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.select_workspace_template_pane(pane_idx, cx);
        }))
        .when(selected, |tile| {
            tile.child(squircle_border(
                ROW_RADIUS,
                px(TILE_SELECTED_RING),
                with_alpha(ui.text, SELECTED_RING_ALPHA),
            ))
        })
        .child(pane_icon(
            pane,
            if upright { TILE_ICON } else { TILE_ROW_ICON },
            ui,
        ))
        .child(label)
        .into_any_element()
    }

    fn render_preset_control(
        &self,
        current: &str,
        panes: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut group = div()
            .id("workspace-layout-presets")
            .flex()
            .flex_row()
            .gap(px(2.))
            .role(gpui::accesskit::Role::RadioGroup)
            .aria_label("Layout");
        for (i, (value, label)) in LAYOUT_PRESETS.iter().enumerate() {
            let selected = *value == current;
            let next = (*value).to_string();
            let ink = if selected {
                with_alpha(ui.text, 0.85)
            } else {
                with_alpha(ui.text, 0.45)
            };
            let hover = with_alpha(ui.text, if selected { SELECTED_FILL } else { PILL_HOVER });
            group = group.child(
                div()
                    .id(("workspace-layout-preset", i))
                    .h(px(PILL_HEIGHT))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .when(selected, |pill| pill.bg(with_alpha(ui.text, SELECTED_FILL)))
                    .hover(move |style| style.bg(hover))
                    .cursor_pointer()
                    .role(gpui::accesskit::Role::RadioButton)
                    .aria_label(*label)
                    .aria_toggled(if selected {
                        gpui::accesskit::Toggled::True
                    } else {
                        gpui::accesskit::Toggled::False
                    })
                    .delayed_tooltip(text_tooltip(*label))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.set_workspace_template_layout(next.clone(), cx);
                    }))
                    .child(preset_glyph(
                        value,
                        panes.clamp(3, 4),
                        PRESET_GLYPH_WIDTH,
                        PRESET_GLYPH_HEIGHT,
                        ink,
                    )),
            );
        }
        group.into_any_element()
    }

    fn render_template_pane(
        &self,
        pane: &SurfaceDefinition,
        pane_idx: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = pane_kind(pane);
        let remove = ghost_tint_skin(
            div()
                .id("workspace-pane-remove")
                .flex_none()
                .px(px(10.))
                .py(px(4.))
                .text_size(BODY)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ui.vc_deleted),
            "workspace-pane-remove-squircle",
            ROW_RADIUS,
            ui.vc_deleted,
        )
        .accessible_control(gpui::accesskit::Role::Button, "Remove pane")
        .child("Remove pane")
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.remove_workspace_template_pane_at(pane_idx, cx);
        }));
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .pb(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child(format!("Pane · {}", surface_title(pane, pane_idx))),
            )
            .child(remove);

        let mut card = setting_card(ui).child(template_row(
            "Runs",
            "What opens in this pane.",
            self.render_kind_control(kind, ui, cx),
            ui,
        ));
        match kind {
            PaneKind::Agent => {
                card = card
                    .child(hairline(ui))
                    .child(template_row(
                        "Agent",
                        "Starts in this pane when the template launches.",
                        self.render_agent_select(pane, ui, cx),
                        ui,
                    ))
                    .child(hairline(ui))
                    .child(self.template_text_row(
                        "Prompt",
                        "Typed into the agent. You press Enter to send it.",
                        self.workspace_pane_prompt_input.clone(),
                        ui,
                        cx,
                    ));
            }
            PaneKind::Command => {
                card = card.child(hairline(ui)).child(self.template_text_row(
                    "Command",
                    "Runs as soon as the pane opens.",
                    self.workspace_pane_command_input.clone(),
                    ui,
                    cx,
                ));
            }
            PaneKind::Empty => {}
        }
        card = card
            .child(hairline(ui))
            .child(self.template_text_row(
                "Name",
                "Shown on the pane tab.",
                self.workspace_pane_name_input.clone(),
                ui,
                cx,
            ))
            .child(hairline(ui))
            .child(self.template_text_row(
                "Folder",
                "Leave empty to open the project folder.",
                self.workspace_pane_cwd_input.clone(),
                ui,
                cx,
            ));

        div()
            .flex()
            .flex_col()
            .child(header)
            .child(card)
            .when_some(
                self.template_notice(TemplateNoticeAt::Pane),
                |column, message| {
                    column.child(div().pt(px(8.)).px(px(2.)).child(render_hint(
                        message,
                        HintTone::Error,
                        ui,
                    )))
                },
            )
            .into_any_element()
    }

    fn render_kind_control(
        &self,
        current: PaneKind,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut group = div()
            .id("workspace-pane-kinds")
            .flex()
            .flex_row()
            .gap(px(2.))
            .role(gpui::accesskit::Role::RadioGroup)
            .aria_label("Pane type");
        for (kind, label, icon) in [
            (PaneKind::Agent, "Agent", AGENT_GLYPH),
            (PaneKind::Command, "Command", COMMAND_GLYPH),
            (PaneKind::Empty, "Shell", SHELL_GLYPH),
        ] {
            let selected = kind == current;
            let ink = if selected { ui.text } else { ui.muted };
            let hover = with_alpha(ui.text, if selected { SELECTED_FILL } else { PILL_HOVER });
            group = group.child(
                div()
                    .id(SharedString::from(format!("workspace-pane-kind-{label}")))
                    .h(px(PILL_HEIGHT))
                    .pl(px(9.))
                    .pr(px(11.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .rounded_full()
                    .when(selected, |pill| pill.bg(with_alpha(ui.text, SELECTED_FILL)))
                    .hover(move |style| style.bg(hover))
                    .cursor_pointer()
                    .text_size(BODY)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(ink)
                    .role(gpui::accesskit::Role::RadioButton)
                    .aria_label(label)
                    .aria_toggled(if selected {
                        gpui::accesskit::Toggled::True
                    } else {
                        gpui::accesskit::Toggled::False
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.set_workspace_template_pane_kind(kind, cx);
                    }))
                    .child(glyph(icon, 13., ink))
                    .child(label),
            );
        }
        group.into_any_element()
    }

    fn render_agent_select(
        &self,
        pane: &SurfaceDefinition,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = pane_agent(pane);
        let is_open = self.workspace_template_dropdown == Some(WorkspaceTemplateDropdown::Agent);
        let mut trigger = select_trigger("workspace-pane-agent-trigger", "Agent", is_open, ui)
            .on_press(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.workspace_template_dropdown = if is_open {
                    None
                } else {
                    Some(WorkspaceTemplateDropdown::Agent)
                };
                this.settings_focus.focus(window, cx);
                cx.notify();
            }))
            .child(match current {
                Some(agent) => agent_logo(agent, AGENT_OPTION_LOGO, ui),
                None => glyph(AGENT_GLYPH, AGENT_OPTION_LOGO, ui.muted),
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(BODY)
                    .text_color(ui.text)
                    .truncate()
                    .child(current.map_or("Choose an agent", |agent| agent.display_name())),
            )
            .child(select_chevron(ui));

        if is_open {
            let mut menu = select_menu("workspace-pane-agent-menu", ui).on_mouse_down_out(
                cx.listener(|this, _, _w, cx| {
                    if this.workspace_template_dropdown == Some(WorkspaceTemplateDropdown::Agent) {
                        this.workspace_template_dropdown = None;
                        cx.notify();
                    }
                }),
            );
            for (i, agent) in TerminalAgent::visible(&self.cached_config)
                .into_iter()
                .enumerate()
            {
                menu = menu.option(
                    select_option(("workspace-pane-agent", i), current == Some(agent), ui)
                        .child(agent_logo(agent, AGENT_OPTION_LOGO, ui))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(ui.text)
                                .child(agent.display_name()),
                        ),
                    cx.listener(move |this, _: &(), _w, cx| {
                        this.workspace_template_dropdown = None;
                        this.set_workspace_template_pane_agent(agent, cx);
                    }),
                );
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }
        trigger.into_any_element()
    }

    fn render_template_manage(
        &self,
        idx: usize,
        title: String,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let armed = self.workspace_template_delete_armed;
        let duplicate = card_button("workspace-template-duplicate", "Duplicate", ui).on_click(
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.duplicate_workspace_template(idx, cx);
            }),
        );
        let delete_actions = if armed {
            div()
                .flex_none()
                .flex()
                .flex_row()
                .gap(px(6.))
                .child(
                    card_button("workspace-template-delete-cancel", "Cancel", ui).on_click(
                        cx.listener(|this, _: &ClickEvent, _w, cx| {
                            this.workspace_template_delete_armed = false;
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    raised_button(
                        "workspace-template-delete-confirm",
                        "Delete template",
                        destructive_color(),
                        gpui::white(),
                    )
                    .on_click(cx.listener(
                        move |this, _: &ClickEvent, window, cx| {
                            this.delete_workspace_template(idx, window, cx);
                        },
                    )),
                )
                .into_any_element()
        } else {
            ghost_tint_skin(
                div()
                    .id("workspace-template-delete")
                    .flex_none()
                    .px(px(10.))
                    .py(px(4.))
                    .text_size(BODY)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(ui.vc_deleted),
                "workspace-template-delete-squircle",
                ROW_RADIUS,
                ui.vc_deleted,
            )
            .accessible_control(gpui::accesskit::Role::Button, "Delete template")
            .child("Delete")
            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.workspace_template_delete_armed = true;
                cx.notify();
            }))
            .into_any_element()
        };
        let delete_title = if armed {
            format!("Delete {title}?")
        } else {
            "Delete template".to_string()
        };
        setting_card(ui)
            .child(manage_row(
                row_text("Duplicate template", "Start a variation from this one.", ui),
                duplicate,
            ))
            .child(hairline(ui))
            .child(manage_row(
                row_text(
                    delete_title,
                    "Workspaces it already opened stay as they are.",
                    ui,
                ),
                delete_actions,
            ))
            .into_any_element()
    }

    fn template_text_row(
        &self,
        title: &'static str,
        description: &'static str,
        input: gpui::Entity<crate::widgets::text_input::TextInput>,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(16.))
            .px(px(CARD_PADDING_X))
            .py(px(SETTINGS_ROW_PADDING_Y))
            .child(settings_label(ui, title, description))
            .child(
                self.commit_on_leave(text_field(input, ui), cx)
                    .flex_1()
                    .min_w(px(FIELD_MIN_WIDTH))
                    .max_w(px(FIELD_MAX_WIDTH))
                    .flex(),
            )
            .into_any_element()
    }

    fn commit_on_leave(&self, field: impl IntoElement, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .child(field)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "enter" {
                    this.commit_workspace_template_fields(cx);
                    this.settings_focus.focus(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.commit_workspace_template_if_editing(window, cx);
            }))
    }

    fn template_input_focused(&self, window: &Window, cx: &App) -> bool {
        [
            &self.workspace_template_name_input,
            &self.workspace_pane_name_input,
            &self.workspace_pane_cwd_input,
            &self.workspace_pane_command_input,
            &self.workspace_pane_prompt_input,
        ]
        .into_iter()
        .any(|input| input.read(cx).focus_handle.is_focused(window))
    }

    pub(super) fn commit_workspace_template_if_editing(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.template_input_focused(window, cx) {
            self.commit_workspace_template_fields(cx);
        }
    }

    pub(super) fn commit_workspace_template_fields(&mut self, cx: &mut Context<Self>) {
        let Some(idx) = self.selected_workspace_template_index() else {
            return;
        };
        let mut commands = self.cached_config.commands.clone();
        match self.apply_workspace_inputs(&mut commands, idx, cx) {
            Ok(()) => self.clear_template_notice(TemplateNoticeAt::Identity),
            Err(message) => {
                self.workspace_template_notice = Some(TemplateNotice {
                    at: TemplateNoticeAt::Identity,
                    message,
                });
            }
        }
        match self.apply_pane_inputs(&mut commands, idx, cx) {
            Ok(()) => self.clear_template_notice(TemplateNoticeAt::Pane),
            Err(message) => {
                self.workspace_template_notice = Some(TemplateNotice {
                    at: TemplateNoticeAt::Pane,
                    message,
                });
            }
        }
        if commands != self.cached_config.commands {
            self.persist_workspace_commands(commands, cx);
        }
        cx.notify();
    }

    pub(crate) fn sync_workspace_template_inputs(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected_workspace_template_index();
        self.workspace_template_selected = selected;
        let Some(idx) = selected else {
            for input in [
                &self.workspace_template_name_input,
                &self.workspace_template_project_input,
            ] {
                set_input(input, "", cx);
            }
            self.clear_pane_inputs(cx);
            return;
        };
        let Some(command) = self.cached_config.commands.get(idx) else {
            return;
        };
        let Some(workspace) = command.workspace() else {
            return;
        };
        let title = template_title(command, workspace);
        let cwd = workspace.cwd.clone().unwrap_or_default();
        set_input(&self.workspace_template_name_input, &title, cx);
        set_input(&self.workspace_template_project_input, &cwd, cx);
        self.sync_workspace_pane_inputs(cx);
    }

    fn clear_pane_inputs(&mut self, cx: &mut Context<Self>) {
        for input in [
            &self.workspace_pane_name_input,
            &self.workspace_pane_cwd_input,
            &self.workspace_pane_command_input,
            &self.workspace_pane_prompt_input,
        ] {
            set_input(input, "", cx);
        }
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
            self.clear_pane_inputs(cx);
            return;
        }
        self.workspace_template_selected_pane =
            self.workspace_template_selected_pane.min(panes.len() - 1);
        let pane = &panes[self.workspace_template_selected_pane];
        let name = pane
            .name
            .as_deref()
            .or(pane.custom_name.as_deref())
            .unwrap_or("");
        set_input(&self.workspace_pane_name_input, name, cx);
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
        self.workspace_template_dropdown = None;
        self.clear_template_notice(TemplateNoticeAt::Pane);
        self.sync_workspace_pane_inputs(cx);
        cx.notify();
    }

    fn pick_workspace_template_project_path(&mut self, cx: &mut Context<Self>) {
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
                            app.commit_workspace_template_fields(cx);
                        })
                        .ok();
                    });
                }
            },
        )
        .detach();
    }

    fn update_selected_template(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut WorkspaceDefinition, &mut Vec<SurfaceDefinition>, usize) -> bool,
    ) {
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
        let selected = self
            .workspace_template_selected_pane
            .min(panes.len().saturating_sub(1));
        if !change(workspace, &mut panes, selected) {
            return;
        }
        let preset = workspace_layout_preset(workspace).to_string();
        workspace.layout_preset = Some(preset.clone());
        workspace.layout = build_layout_from_surfaces(&preset, panes);
        self.persist_workspace_commands(commands, cx);
        self.sync_workspace_pane_inputs(cx);
        cx.notify();
    }

    fn set_workspace_template_layout(&mut self, preset: String, cx: &mut Context<Self>) {
        self.update_selected_template(cx, |workspace, _, _| {
            workspace.layout_preset = Some(preset);
            true
        });
    }

    fn add_workspace_template_pane(&mut self, cx: &mut Context<Self>) {
        let agent = TerminalAgent::visible(&self.cached_config).first().copied();
        let mut added = None;
        self.update_selected_template(cx, |_, panes, _| {
            let mut pane = SurfaceDefinition {
                surface_type: Some("terminal".to_string()),
                name: Some(format!("Pane {}", panes.len() + 1)),
                focus: panes.is_empty().then_some(true),
                ..Default::default()
            };
            if let Some(agent) = agent {
                pane.agent = Some(agent.tag().to_string());
                pane.name = Some(agent.display_name().to_string());
            }
            panes.push(pane);
            added = Some(panes.len() - 1);
            true
        });
        if let Some(added) = added {
            self.select_workspace_template_pane(added, cx);
        }
    }

    fn remove_workspace_template_pane_at(&mut self, remove_idx: usize, cx: &mut Context<Self>) {
        let mut next_selected = None;
        self.update_selected_template(cx, |_, panes, selected| {
            if remove_idx >= panes.len() {
                return false;
            }
            panes.remove(remove_idx);
            next_selected = Some(if selected >= remove_idx {
                selected.saturating_sub(1)
            } else {
                selected
            });
            true
        });
        if let Some(next) = next_selected {
            self.select_workspace_template_pane(next, cx);
        }
    }

    fn set_workspace_template_pane_kind(&mut self, kind: PaneKind, cx: &mut Context<Self>) {
        let agent = TerminalAgent::visible(&self.cached_config).first().copied();
        self.clear_template_notice(TemplateNoticeAt::Pane);
        self.update_selected_template(cx, |_, panes, selected| {
            let Some(pane) = panes.get_mut(selected) else {
                return false;
            };
            if pane_kind(pane) == kind {
                return false;
            }
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
                        pane.command = Some(DEFAULT_SERVER_COMMAND.to_string());
                    }
                }
                PaneKind::Agent => {
                    pane.command = None;
                    if pane.agent.is_none() {
                        pane.agent = agent.map(|agent| agent.tag().to_string());
                    }
                }
            }
            true
        });
    }

    fn set_workspace_template_pane_agent(&mut self, agent: TerminalAgent, cx: &mut Context<Self>) {
        self.update_selected_template(cx, |_, panes, selected| {
            let Some(pane) = panes.get_mut(selected) else {
                return false;
            };
            pane.agent = Some(agent.tag().to_string());
            pane.command = None;
            true
        });
    }

    fn apply_workspace_inputs(
        &self,
        commands: &mut [CommandDefinition],
        idx: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let name = self
            .workspace_template_name_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        let project = self
            .workspace_template_project_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        if name.is_empty() {
            return Err("Give the template a name.".to_string());
        }
        if project.is_empty() {
            return Err("Choose a project folder.".to_string());
        }
        let Some(command) = commands.get_mut(idx) else {
            return Err("This template no longer exists.".to_string());
        };
        let Some(workspace) = command.workspace_mut() else {
            return Err("This template no longer exists.".to_string());
        };
        workspace.name = Some(name.clone());
        workspace.cwd = Some(project.clone());
        command.name = name;
        command.description = Some(format!("Workspace template for {project}"));
        Ok(())
    }

    fn apply_pane_inputs(
        &self,
        commands: &mut [CommandDefinition],
        idx: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(workspace) = commands
            .get_mut(idx)
            .and_then(CommandDefinition::workspace_mut)
        else {
            return Err("This template no longer exists.".to_string());
        };
        let mut panes = template_surfaces(workspace);
        if panes.is_empty() {
            return Ok(());
        }
        let pane_idx = self.workspace_template_selected_pane.min(panes.len() - 1);
        let value = |input: &gpui::Entity<crate::widgets::text_input::TextInput>| {
            input.read(cx).value().trim().to_string()
        };
        let name = value(&self.workspace_pane_name_input);
        let cwd = value(&self.workspace_pane_cwd_input);
        let command = value(&self.workspace_pane_command_input);
        let prompt = value(&self.workspace_pane_prompt_input);

        let pane = &mut panes[pane_idx];
        let kind = pane_kind(pane);
        if kind == PaneKind::Command && command.is_empty() {
            return Err(
                "A command pane needs a command. Pick Shell for a plain terminal.".to_string(),
            );
        }
        pane.name = (!name.is_empty()).then_some(name);
        pane.custom_name = None;
        pane.cwd = (!cwd.is_empty()).then_some(cwd);
        match kind {
            PaneKind::Command => pane.command = Some(command),
            PaneKind::Agent => pane.prompt = (!prompt.is_empty()).then_some(prompt),
            PaneKind::Empty => {}
        }
        let unchanged = workspace
            .layout
            .as_ref()
            .map(template_surfaces_from_layout)
            .unwrap_or_default()
            == panes;
        if unchanged {
            return Ok(());
        }
        let preset = workspace_layout_preset(workspace).to_string();
        workspace.layout = build_layout_from_surfaces(&preset, panes);
        Ok(())
    }
}

fn template_row(
    title: &'static str,
    description: &'static str,
    control: impl IntoElement,
    ui: crate::theme::UiColors,
) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(16.))
        .px(px(CARD_PADDING_X))
        .py(px(SETTINGS_ROW_PADDING_Y))
        .child(settings_label(ui, title, description))
        .child(div().flex_shrink_0().child(control))
}

struct CanvasCursor<'a> {
    panes: &'a [SurfaceDefinition],
    selected: usize,
    next: usize,
}

fn stacked_share(height: f32, count: usize) -> f32 {
    let count = count.max(1) as f32;
    (height - TILE_GAP * (count - 1.)) / count
}

fn stacked_depth(node: &LayoutNode) -> usize {
    match node {
        LayoutNode::Pane { .. } => 1,
        LayoutNode::Split {
            direction,
            children,
            ..
        } => {
            let depths = children.iter().map(stacked_depth);
            if splits_side_by_side(direction) {
                depths.max().unwrap_or(1)
            } else {
                depths.sum::<usize>().max(1)
            }
        }
    }
}

fn canvas_height(layout: Option<&LayoutNode>, panes: usize) -> f32 {
    let base = if panes >= CANVAS_TALL_FROM {
        CANVAS_HEIGHT_TALL
    } else {
        CANVAS_HEIGHT
    };
    let depth = layout.map_or(1, stacked_depth) as f32;
    base.max(depth * TILE_ROW_MIN + (depth - 1.) * TILE_GAP)
}

fn manage_row(text: gpui::Div, action: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(16.))
        .px(px(CARD_PADDING_X))
        .py(px(SETTINGS_ROW_PADDING_Y))
        .child(text)
        .child(action)
}

fn pane_detail(pane: &SurfaceDefinition) -> String {
    match pane_kind(pane) {
        PaneKind::Agent => pane_agent(pane)
            .map_or("Agent", |agent| agent.display_name())
            .to_string(),
        PaneKind::Command => pane.command.clone().unwrap_or_default(),
        PaneKind::Empty => "Shell".to_string(),
    }
}

fn set_input(
    input: &gpui::Entity<crate::widgets::text_input::TextInput>,
    value: &str,
    cx: &mut Context<PaneFlowApp>,
) {
    if input.read(cx).value() == value {
        return;
    }
    input.update(cx, |input, cx| {
        input.set_value(SharedString::from(value.to_string()), cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pane_tiles_sit_concentric_in_the_canvas_card() {
        assert_eq!(
            crate::app::constants::SETTINGS_CARD_RADIUS,
            ROW_RADIUS + px(CANVAS_INSET)
        );
    }

    #[test]
    fn a_pane_tile_names_what_runs_in_it() {
        let shell = SurfaceDefinition::default();
        assert_eq!(pane_detail(&shell), "Shell");
        let server = SurfaceDefinition {
            command: Some("bun dev".to_string()),
            ..Default::default()
        };
        assert_eq!(pane_detail(&server), "bun dev");
    }

    fn layout(preset: &str, panes: usize) -> LayoutNode {
        crate::app::workspace_ops::templates::build_layout_from_surfaces(
            preset,
            vec![SurfaceDefinition::default(); panes],
        )
        .expect("a preset with panes builds a layout")
    }

    #[test]
    fn stacked_panes_lie_flat_and_never_drop_below_a_row() {
        let stacked = layout("even_v", 3);
        let height = canvas_height(Some(&stacked), 3);
        let row = stacked_share(height, 3);
        assert!((TILE_ROW_MIN..TILE_COLUMN_MIN).contains(&row));

        let tall = layout("even_v", 6);
        let height = canvas_height(Some(&tall), 6);
        assert!(stacked_share(height, 6) >= TILE_ROW_MIN);
    }

    #[test]
    fn panes_with_room_stand_upright() {
        let tiled = layout("tiled", 4);
        let height = canvas_height(Some(&tiled), 4);
        assert_eq!(stacked_depth(&tiled), 2);
        assert!(stacked_share(height, 2) >= TILE_COLUMN_MIN);
        assert_eq!(stacked_depth(&layout("even_h", 3)), 1);
    }
}
