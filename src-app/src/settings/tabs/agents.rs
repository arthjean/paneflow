use std::collections::BTreeMap;

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Entity, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, img,
    prelude::*, px, rgb, svg,
};
use paneflow_config::schema::AgentProfileConfig;

use crate::PaneFlowApp;
use crate::SidebarWidthAnimation;
use crate::agent_launcher::{AgentProfile, TerminalAgent};
use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, deferred_select_menu, destructive_color, hairline,
    secondary_button, section_header, section_header_with_action, select_chevron, select_item,
    select_menu, select_trigger, setting_card, setting_text, toggle_pill, toggle_row, with_alpha,
};
use crate::ui_primitives::{AnimatedHoverExt, BODY, LABEL_SM, LABEL_XS, ROW_RADIUS, squircle_skin};
use crate::widgets::text_input::TextInput;

const ROW_ICON: f32 = 18.;
const CONTROL_WIDTH: f32 = 300.;
const INPUT_HEIGHT: f32 = 28.;
const ICON_BUTTON_SIZE: f32 = 26.;
const AGENT_ROW_HEIGHT: f32 = 44.;
const HAIRLINE_HEIGHT: f32 = 1.;

fn secondary_list_height() -> f32 {
    TerminalAgent::secondary().count() as f32 * (AGENT_ROW_HEIGHT + HAIRLINE_HEIGHT)
}

pub(crate) struct EnvRowInputs {
    pub(crate) key: Entity<TextInput>,
    pub(crate) value: Entity<TextInput>,
}

pub(crate) struct AgentProfileEditor {
    pub(crate) index: Option<usize>,
    pub(crate) agent: TerminalAgent,
    pub(crate) agent_menu_open: bool,
    pub(crate) error: Option<String>,
    pub(crate) env_rows: Vec<EnvRowInputs>,
}

impl PaneFlowApp {
    pub(crate) fn render_agents_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        div()
            .flex()
            .flex_col()
            .child(self.render_agent_list_section(ui, cx))
            .child(self.render_agent_profiles_section(ui, cx))
            .child(self.render_agent_permissions_section(ui, cx))
            .child(div().h(px(180.)).flex_none())
    }

    pub(crate) fn tick_agents_list_animation(&mut self, window: &mut gpui::Window) {
        let Some(animation) = self.agents_list_animation else {
            return;
        };
        if animation.is_finished(std::time::Instant::now()) {
            self.agents_list_animation = None;
        } else {
            window.request_animation_frame();
        }
    }

    fn agents_list_height_now(&self) -> f32 {
        match self.agents_list_animation {
            Some(animation) => animation.width_at(std::time::Instant::now()),
            None if self.agents_list_expanded => secondary_list_height(),
            None => 0.,
        }
    }

    fn toggle_agents_list(&mut self, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let from_width = self.agents_list_height_now();
        self.agents_list_expanded = !self.agents_list_expanded;
        let to_width = if self.agents_list_expanded {
            secondary_list_height()
        } else {
            0.
        };
        self.agents_list_animation =
            if !crate::ui_primitives::reduce_motion() && (from_width - to_width).abs() > 0.5 {
                Some(SidebarWidthAnimation {
                    from_width,
                    to_width,
                    started_at: now,
                })
            } else {
                None
            };
        cx.notify();
    }

    pub(crate) fn probe_agent_versions(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            smol::unblock(TerminalAgent::probe_missing_versions).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn render_agent_list_section(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.agents_list_expanded;
        let hidden = TerminalAgent::secondary().count();
        let shown = if expanded {
            TerminalAgent::ALL.len()
        } else {
            TerminalAgent::PRIMARY.len()
        };
        let counter: SharedString = format!("{shown} of {} shown", TerminalAgent::ALL.len()).into();

        let mut card = setting_card(ui);
        for (idx, agent) in TerminalAgent::PRIMARY.into_iter().enumerate() {
            if idx > 0 {
                card = card.child(hairline(ui));
            }
            card = card.child(self.render_agent_row(agent, ui, cx));
        }
        let secondary_height = self.agents_list_height_now();
        if secondary_height > 0. {
            let mut secondary = div()
                .flex()
                .flex_col()
                .flex_none()
                .h(px(secondary_height))
                .overflow_hidden();
            for agent in TerminalAgent::secondary() {
                secondary = secondary
                    .child(hairline(ui))
                    .child(self.render_agent_row(agent, ui, cx));
            }
            card = card.child(secondary);
        }
        let more_label: SharedString = if expanded {
            "Show less".into()
        } else {
            format!("Show {hidden} more").into()
        };
        let more_row = squircle_skin(
            div().id("agents-show-more"),
            "agents-show-more",
            ROW_RADIUS,
            None,
            Some(with_alpha(ui.text, 0.05)),
        )
        .mx(px(6.))
        .my(px(6.))
        .h(px(30.))
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .cursor(CursorStyle::PointingHand)
        .text_size(LABEL_SM)
        .text_color(ui.muted)
        .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
            this.toggle_agents_list(cx);
        }))
        .child(more_label)
        .child(
            svg()
                .size(px(10.))
                .flex_none()
                .path(if expanded {
                    "icons/chevron_up.svg"
                } else {
                    "icons/chevron-down.svg"
                })
                .text_color(ui.muted),
        );
        card = card.child(hairline(ui)).child(more_row);

        div()
            .flex()
            .flex_col()
            .child(section_header_with_action(
                ui,
                "Agents",
                div()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child(counter),
            ))
            .child(card)
            .into_any_element()
    }

    fn render_agent_row(
        &self,
        agent: TerminalAgent,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let installed = agent.is_installed();
        let visible = agent.is_visible(&self.cached_config);
        let binary: SharedString = agent.binary().into();
        let mut status = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .text_size(LABEL_SM)
            .text_color(ui.muted)
            .whitespace_nowrap()
            .overflow_hidden();
        status = if installed {
            status
                .child("Installed ·")
                .child(mono_text(binary, ui.muted))
                .when_some(agent.cached_version(), |s, version| {
                    s.child(mono_text(SharedString::from(version), ui.muted))
                })
        } else {
            status
                .child("Not installed ·")
                .child(mono_text(binary, ui.muted))
                .child("not on PATH")
        };

        let control = if installed {
            let key = agent.visibility_config_key();
            let target = !visible;
            div()
                .id(SharedString::from(format!("agent-visible-{}", agent.tag())))
                .flex_shrink_0()
                .cursor(CursorStyle::PointingHand)
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.persist_setting(false, key, serde_json::Value::Bool(target), cx);
                }))
                .child(toggle_pill(visible, ui))
                .into_any_element()
        } else {
            div()
                .flex_shrink_0()
                .opacity(0.35)
                .child(toggle_pill(false, ui))
                .into_any_element()
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .h(px(AGENT_ROW_HEIGHT))
            .gap(px(12.))
            .px(px(12.))
            .child(
                div()
                    .when(!installed, |d| d.opacity(0.45))
                    .child(agent_icon_el(agent, ui)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .text_size(BODY)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(if installed { ui.text } else { ui.muted })
                            .child(agent.display_name()),
                    )
                    .child(status),
            )
            .child(control)
            .into_any_element()
    }

    fn render_agent_profiles_section(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entries = &self.cached_config.agent_profiles;
        let editor = self.agent_profile_editor.as_ref();
        let creating = editor.is_some_and(|editor| editor.index.is_none());

        let mut card = setting_card(ui);
        if entries.is_empty() && !creating {
            card = card.child(
                div()
                    .px(px(12.))
                    .py(px(14.))
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child(
                        "A profile launches one of the agents above with its own environment \
                         variables and arguments, for example a second Claude Code account \
                         through CLAUDE_CONFIG_DIR.",
                    ),
            );
        }
        for (idx, entry) in entries.iter().enumerate() {
            if idx > 0 {
                card = card.child(hairline(ui));
            }
            let editing = editor.is_some_and(|editor| editor.index == Some(idx));
            card = card.child(self.render_agent_profile_row(idx, entry, editing, ui, cx));
            if editing && let Some(editor) = editor {
                card = card.child(self.render_agent_profile_editor(editor, ui, cx));
            }
        }
        if creating && let Some(editor) = editor {
            if !entries.is_empty() {
                card = card.child(hairline(ui));
            }
            card = card
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(12.))
                        .px(px(12.))
                        .py(px(10.))
                        .child(agent_icon_el(editor.agent, ui))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .text_size(BODY)
                                        .text_color(ui.muted)
                                        .child("New profile"),
                                )
                                .child(
                                    div()
                                        .text_size(LABEL_SM)
                                        .text_color(ui.muted)
                                        .child("Pick a base agent, then name it."),
                                ),
                        ),
                )
                .child(self.render_agent_profile_editor(editor, ui, cx));
        }

        let new_button = div()
            .id("agent-profile-new")
            .px(px(6.))
            .py(px(2.))
            .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
            .cursor(CursorStyle::PointingHand)
            .text_size(LABEL_SM)
            .text_color(ui.muted)
            .animated_hover_bg(with_alpha(ui.text, 0.0), with_alpha(ui.text, 0.05))
            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.open_agent_profile_editor(None, cx);
            }))
            .child("+ New profile");

        div()
            .mt(px(24.))
            .flex()
            .flex_col()
            .child(section_header_with_action(ui, "Profiles", new_button))
            .child(card)
            .into_any_element()
    }

    fn render_agent_profile_row(
        &self,
        idx: usize,
        entry: &AgentProfileConfig,
        editing: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group = SharedString::from(format!("agent-profile-row-{idx}"));
        let mut meta = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .overflow_hidden()
            .text_size(LABEL_SM)
            .text_color(ui.muted)
            .whitespace_nowrap();
        let icon = match AgentProfile::from_config(entry) {
            Ok(profile) => {
                meta = meta.child(profile.agent.display_name());
                if !profile.env.is_empty() || !profile.args.is_empty() {
                    meta = meta.child("·");
                }
                for (key, value) in &profile.env {
                    meta = meta.child(chip(format!("{key}={value}"), ui));
                }
                if !profile.args.is_empty() {
                    meta = meta.child(chip(profile.args.join(" "), ui));
                }
                agent_icon_el(profile.agent, ui)
            }
            Err(reason) => {
                meta = meta
                    .text_color(destructive_color())
                    .child(format!("Invalid profile: {reason}"));
                div().size(px(ROW_ICON)).flex_none().into_any_element()
            }
        };

        let actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .flex_shrink_0()
            .when(!editing, |d| d.invisible())
            .group_hover(group.clone(), |style| style.visible())
            .child(
                icon_button(
                    SharedString::from(format!("agent-profile-edit-{idx}")),
                    "icons/edit.svg",
                    ui.muted,
                    ui.text,
                    ui,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.open_agent_profile_editor(Some(idx), cx);
                })),
            )
            .child(
                icon_button(
                    SharedString::from(format!("agent-profile-delete-{idx}")),
                    "icons/trash.svg",
                    ui.muted,
                    destructive_color(),
                    ui,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.delete_agent_profile(idx, cx);
                })),
            );

        div()
            .group(group)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .py(px(10.))
            .child(icon)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .text_size(BODY)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(ui.text)
                            .truncate()
                            .child(entry.name.clone()),
                    )
                    .child(meta),
            )
            .child(actions)
            .into_any_element()
    }

    fn render_agent_profile_editor(
        &self,
        editor: &AgentProfileEditor,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let base_select = self.render_agent_profile_base_select(editor, ui, cx);

        let mut env_column = div()
            .flex()
            .flex_col()
            .items_end()
            .gap(px(6.))
            .w(px(CONTROL_WIDTH));
        for (row_idx, row) in editor.env_rows.iter().enumerate() {
            let removable = editor.env_rows.len() > 1;
            env_column = env_column.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .w_full()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(input_box(row.key.clone(), true, ui)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .relative()
                            .child(input_box(row.value.clone(), true, ui))
                            .when(removable, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px((INPUT_HEIGHT - ICON_BUTTON_SIZE) / 2.))
                                        .right(px(1.))
                                        .child(
                                            icon_button(
                                                SharedString::from(format!(
                                                    "agent-profile-env-remove-{row_idx}"
                                                )),
                                                "icons/trash.svg",
                                                ui.muted,
                                                destructive_color(),
                                                ui,
                                            )
                                            .on_click(
                                                cx.listener(move |this, _: &ClickEvent, _w, cx| {
                                                    if let Some(editor) =
                                                        this.agent_profile_editor.as_mut()
                                                        && row_idx < editor.env_rows.len()
                                                    {
                                                        editor.env_rows.remove(row_idx);
                                                        cx.notify();
                                                    }
                                                }),
                                            ),
                                        ),
                                )
                            }),
                    ),
            );
        }
        env_column = env_column.child(
            div()
                .id("agent-profile-env-add")
                .flex_none()
                .px(px(6.))
                .py(px(2.))
                .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                .cursor(CursorStyle::PointingHand)
                .text_size(LABEL_SM)
                .text_color(ui.muted)
                .animated_hover_bg(with_alpha(ui.text, 0.0), with_alpha(ui.text, 0.05))
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.add_agent_profile_env_row(cx);
                }))
                .child("+ Add variable"),
        );

        let footer_text: AnyElement = match editor.error.as_ref() {
            Some(error) => div()
                .flex_1()
                .min_w_0()
                .text_size(LABEL_SM)
                .text_color(destructive_color())
                .child(error.clone())
                .into_any_element(),
            None => {
                let preview = self.agent_profile_command_preview(editor, cx);
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child("Launches")
                    .child(mono_text(SharedString::from(preview), ui.text))
                    .into_any_element()
            }
        };

        let save = div()
            .id("agent-profile-save")
            .px(px(10.))
            .py(px(4.))
            .rounded(ROW_RADIUS)
            .cursor(CursorStyle::PointingHand)
            .text_size(BODY)
            .font_weight(gpui::FontWeight::MEDIUM)
            .bg(ui.text)
            .text_color(ui.base)
            .animated_hover(|style, delta| {
                style.opacity(1.0 - 0.15 * delta);
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.save_agent_profile(cx);
            }))
            .child("Save");

        div()
            .flex()
            .flex_col()
            .child(hairline(ui))
            .child(editor_row(
                "Base agent",
                "Status, hooks, and sessions follow it.",
                div().w(px(CONTROL_WIDTH)).child(base_select),
                ui,
            ))
            .child(hairline(ui))
            .child(editor_row(
                "Name",
                "Shown in the launcher.",
                div().w(px(CONTROL_WIDTH)).child(input_box(
                    self.agent_profile_name_input.clone(),
                    false,
                    ui,
                )),
                ui,
            ))
            .child(hairline(ui))
            .child(editor_row(
                "Environment variables",
                "Set on the agent process. A leading ~ expands to your home.",
                env_column,
                ui,
            ))
            .child(hairline(ui))
            .child(editor_row(
                "Extra arguments",
                "Appended after the agent's own flags.",
                div().w(px(CONTROL_WIDTH)).child(input_box(
                    self.agent_profile_args_input.clone(),
                    true,
                    ui,
                )),
                ui,
            ))
            .child(hairline(ui))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .px(px(12.))
                    .py(px(10.))
                    .child(footer_text)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(6.))
                            .flex_shrink_0()
                            .child(secondary_button(
                                "agent-profile-cancel",
                                "Cancel",
                                ui,
                                cx.listener(|this, _: &ClickEvent, _w, cx| {
                                    this.close_agent_profile_editor(cx);
                                }),
                            ))
                            .child(save),
                    ),
            )
            .into_any_element()
    }

    fn render_agent_profile_base_select(
        &self,
        editor: &AgentProfileEditor,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_open = editor.agent_menu_open;
        let current = editor.agent;
        let mut trigger = select_trigger(SharedString::from("agent-profile-agent"), ui)
            .w_full()
            .max_w(px(CONTROL_WIDTH))
            .h(px(INPUT_HEIGHT))
            .py(px(0.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    if let Some(editor) = this.agent_profile_editor.as_mut() {
                        editor.agent_menu_open = !is_open;
                    }
                    this.settings_focus.focus(window, cx);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .flex_1()
                    .min_w_0()
                    .child(agent_icon_sized(current, 14., ui))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(BODY)
                            .text_color(ui.text)
                            .truncate()
                            .child(current.display_name()),
                    ),
            )
            .child(select_chevron(ui));
        if is_open {
            let mut menu = select_menu(SharedString::from("agent-profile-agent-list"), ui)
                .on_mouse_down_out(cx.listener(|this, _, _w, cx| {
                    if let Some(editor) = this.agent_profile_editor.as_mut() {
                        editor.agent_menu_open = false;
                        cx.notify();
                    }
                }));
            for agent in TerminalAgent::ALL {
                let item = select_item(
                    SharedString::from(format!("agent-profile-agent-item-{}", agent.tag())),
                    agent == current,
                    ui,
                )
                .cursor(CursorStyle::Arrow)
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    if let Some(editor) = this.agent_profile_editor.as_mut() {
                        editor.agent = agent;
                        editor.agent_menu_open = false;
                    }
                    cx.notify();
                }))
                .child(agent_icon_sized(agent, 14., ui))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ui.text)
                        .child(agent.display_name()),
                );
                menu = menu.child(item);
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }
        div().relative().w_full().child(trigger).into_any_element()
    }

    fn render_agent_permissions_section(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bypass = self
            .cached_config
            .claude_code_bypass_permissions
            .unwrap_or(false);
        let unrestricted = self.cached_config.ai_unrestricted_enabled();
        let fence = self.cached_config.ai_injection_fence_enabled();

        let mut card = setting_card(ui)
            .child(toggle_row(
                "row-claude-bypass",
                "Full access for Claude Code",
                "Edits any file and runs networked commands without asking. No protection \
                 against prompt injection.",
                None,
                bypass,
                "claude_code_bypass_permissions",
                ui,
                cx,
            ))
            .child(hairline(ui))
            .child(toggle_row(
                "row-ai-unrestricted",
                "AI free access",
                "Lets an agent auto-submit prompts to your other panes, without the \
                 PANEFLOW_IPC_SCRIPTING gate. Every write is logged.",
                None,
                unrestricted,
                "ai_unrestricted",
                ui,
                cx,
            ));
        if unrestricted {
            card = card.child(hairline(ui)).child(toggle_row(
                "row-ai-injection-fence",
                "Injection fence",
                "Marks peer-pane output as untrusted when an agent reads it, so a malicious \
                 repo cannot hijack it.",
                None,
                fence,
                "ai_injection_fence",
                ui,
                cx,
            ));
            if !fence {
                card = card.child(hairline(ui)).child(
                    div()
                        .px(px(12.))
                        .py(px(8.))
                        .text_size(BODY)
                        .text_color(destructive_color())
                        .child("Fence off: a malicious pane can silently redirect your agent."),
                );
            }
        }

        div()
            .mt(px(24.))
            .flex()
            .flex_col()
            .child(section_header(ui, "Permissions"))
            .child(card)
            .into_any_element()
    }

    fn agent_profile_command_preview(
        &self,
        editor: &AgentProfileEditor,
        cx: &mut Context<Self>,
    ) -> String {
        let mut parts = vec![editor.agent.binary().to_string()];
        parts.extend(
            self.agent_profile_args_input
                .read(cx)
                .value()
                .split_whitespace()
                .map(str::to_string),
        );
        parts.join(" ")
    }

    pub(crate) fn open_agent_profile_editor(
        &mut self,
        index: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let entry = index.and_then(|idx| self.cached_config.agent_profiles.get(idx).cloned());
        let agent = entry
            .as_ref()
            .and_then(|entry| TerminalAgent::from_tag(entry.agent.trim()))
            .unwrap_or(TerminalAgent::ClaudeCode);
        let (name, env, args) = entry
            .map(|entry| (entry.name, entry.env, entry.args.join(" ")))
            .unwrap_or_default();
        self.agent_profile_name_input.update(cx, |input, cx| {
            input.set_value(SharedString::from(name), cx)
        });
        self.agent_profile_args_input.update(cx, |input, cx| {
            input.set_value(SharedString::from(args), cx)
        });
        let mut env_rows: Vec<EnvRowInputs> = env
            .iter()
            .map(|(key, value)| new_env_row(key, value, cx))
            .collect();
        if env_rows.is_empty() {
            env_rows.push(new_env_row("", "", cx));
        }
        self.agent_profile_editor = Some(AgentProfileEditor {
            index,
            agent,
            agent_menu_open: false,
            error: None,
            env_rows,
        });
        cx.notify();
    }

    pub(crate) fn close_agent_profile_editor(&mut self, cx: &mut Context<Self>) {
        self.agent_profile_editor = None;
        cx.notify();
    }

    fn add_agent_profile_env_row(&mut self, cx: &mut Context<Self>) {
        let row = new_env_row("", "", cx);
        if let Some(editor) = self.agent_profile_editor.as_mut() {
            editor.env_rows.push(row);
        }
        cx.notify();
    }

    fn save_agent_profile(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.agent_profile_editor.as_ref() else {
            return;
        };
        let index = editor.index;
        let agent = editor.agent;
        let name = self.agent_profile_name_input.read(cx).value();
        let args_text = self.agent_profile_args_input.read(cx).value();
        let raw_env: Vec<(String, String)> = editor
            .env_rows
            .iter()
            .map(|row| (row.key.read(cx).value(), row.value.read(cx).value()))
            .collect();

        let draft = collect_env(&raw_env).and_then(|env| {
            let entry = AgentProfileConfig {
                name: name.trim().to_string(),
                agent: agent.tag().to_string(),
                env,
                args: args_text.split_whitespace().map(str::to_string).collect(),
            };
            AgentProfile::from_config(&entry).map(|_| entry)
        });
        let entry = match draft {
            Ok(entry) => entry,
            Err(message) => {
                if let Some(editor) = self.agent_profile_editor.as_mut() {
                    editor.error = Some(message);
                }
                cx.notify();
                return;
            }
        };

        let mut profiles = self.cached_config.agent_profiles.clone();
        match index {
            Some(idx) if idx < profiles.len() => profiles[idx] = entry,
            _ => profiles.push(entry),
        }
        self.agent_profile_editor = None;
        self.persist_agent_profiles(profiles, cx);
    }

    fn delete_agent_profile(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.cached_config.agent_profiles.len() {
            return;
        }
        let mut profiles = self.cached_config.agent_profiles.clone();
        profiles.remove(idx);
        if self
            .agent_profile_editor
            .as_ref()
            .is_some_and(|editor| editor.index == Some(idx))
        {
            self.agent_profile_editor = None;
        }
        self.persist_agent_profiles(profiles, cx);
    }

    fn persist_agent_profiles(
        &mut self,
        profiles: Vec<AgentProfileConfig>,
        cx: &mut Context<Self>,
    ) {
        let value = if profiles.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::to_value(profiles).unwrap_or(serde_json::Value::Null)
        };
        self.persist_setting(false, "agent_profiles", value, cx);
    }
}

fn new_env_row(key: &str, value: &str, cx: &mut Context<PaneFlowApp>) -> EnvRowInputs {
    let key = cx.new(|cx| TextInput::new(key.to_string(), "KEY", cx));
    cx.observe(&key, |_, _, cx| cx.notify()).detach();
    let value = cx.new(|cx| TextInput::new(value.to_string(), "value", cx));
    cx.observe(&value, |_, _, cx| cx.notify()).detach();
    EnvRowInputs { key, value }
}

fn collect_env(rows: &[(String, String)]) -> Result<BTreeMap<String, String>, String> {
    let mut env = BTreeMap::new();
    for (key, value) in rows {
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() && value.is_empty() {
            continue;
        }
        if key.is_empty() {
            return Err(format!("the value '{value}' needs a variable name"));
        }
        if env.insert(key.to_string(), value.to_string()).is_some() {
            return Err(format!("'{key}' is listed twice"));
        }
    }
    Ok(env)
}

fn editor_row(
    title: &'static str,
    description: &'static str,
    control: impl IntoElement,
    ui: crate::theme::UiColors,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(16.))
        .px(px(12.))
        .py(px(10.))
        .child(setting_text(ui, title, description))
        .child(div().flex_shrink_0().child(control))
}

fn mono_family() -> SharedString {
    crate::terminal::element::resolve_font_family(None).into()
}

fn mono_text(text: SharedString, color: Hsla) -> impl IntoElement {
    div()
        .font_family(mono_family())
        .text_size(LABEL_SM)
        .text_color(color)
        .child(text)
}

fn input_box(input: Entity<TextInput>, mono: bool, ui: crate::theme::UiColors) -> impl IntoElement {
    div()
        .w_full()
        .h(px(INPUT_HEIGHT))
        .flex()
        .items_center()
        .px(px(10.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(ui.subtle)
        .when(mono, |d| d.font_family(mono_family()))
        .text_size(if mono { LABEL_SM } else { BODY })
        .text_color(ui.text)
        .child(input)
}

fn chip(text: String, ui: crate::theme::UiColors) -> impl IntoElement {
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(6.))
        .bg(with_alpha(ui.text, 0.06))
        .font_family(mono_family())
        .text_size(LABEL_XS)
        .text_color(ui.text)
        .child(text)
}

fn icon_button(
    id: SharedString,
    icon: &'static str,
    resting: Hsla,
    hovered: Hsla,
    ui: crate::theme::UiColors,
) -> crate::ui_primitives::AnimatedHover {
    div()
        .id(id)
        .flex_none()
        .w(px(ICON_BUTTON_SIZE))
        .h(px(ICON_BUTTON_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .cursor(CursorStyle::PointingHand)
        .text_color(resting)
        .animated_hover(move |style, delta| {
            style
                .bg(with_alpha(ui.text, 0.06 * delta))
                .text_color(crate::ui_primitives::lerp_color(resting, hovered, delta));
        })
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(resting),
        )
}

fn agent_icon_el(agent: TerminalAgent, ui: crate::theme::UiColors) -> AnyElement {
    agent_icon_sized(agent, ROW_ICON, ui)
}

fn agent_icon_sized(agent: TerminalAgent, size: f32, ui: crate::theme::UiColors) -> AnyElement {
    let path = SharedString::from(agent.icon_path());
    if agent.icon_multicolor() {
        img(path).size(px(size)).flex_none().into_any_element()
    } else {
        let tint: Hsla = agent.accent().map(|c| rgb(c).into()).unwrap_or(ui.text);
        svg()
            .size(px(size))
            .flex_none()
            .path(path)
            .text_color(tint)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_env_skips_blank_rows_and_rejects_nameless_or_duplicate_keys() {
        let rows = vec![
            (
                "CLAUDE_CONFIG_DIR".to_string(),
                "~/.claude-perso".to_string(),
            ),
            (String::new(), String::new()),
            (" A ".to_string(), " 1 ".to_string()),
        ];
        let env = collect_env(&rows).unwrap();
        assert_eq!(env.len(), 2);
        assert_eq!(env["A"], "1");
        assert!(collect_env(&[(String::new(), "value".to_string())]).is_err());
        assert!(
            collect_env(&[
                ("A".to_string(), "1".to_string()),
                ("A".to_string(), "2".to_string())
            ])
            .is_err()
        );
    }
}
