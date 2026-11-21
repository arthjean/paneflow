use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Div, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::*, px,
};
use paneflow_mcp_install::IntegrationState;

use super::kit::{self, Ink, Size, Text, Tone};
use super::{AgentAccountNamer, AgentProfileEditor, input_box_sized};
use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;
use crate::agent_updates::{self, UpdateMethod};
use crate::settings::components::{
    ModalKey, deferred_select_menu_at, modal_key, select_chevron, select_menu, select_option,
    select_trigger,
};
use crate::ui_primitives::AccessibleControlExt;

const FIELD_HEIGHT: f32 = 32.;
const SELECT_HEIGHT: f32 = 28.;
const SELECT_WIDTH: f32 = 156.;
const ENV_NAME_WIDTH: f32 = 160.;
const FORM_GAP: f32 = 16.;
const ENV_GAP: f32 = 6.;
const HOOKS_DESCRIPTION: &str =
    "Lets Paneflow resume its conversations and registers the Paneflow MCP server.";
const LAUNCH_PRESETS_DESCRIPTION: &str =
    "A named way to start this agent with its own variables and arguments.";
const COPY_ICON: &str = "icons/settings/copy-01.svg";
const REMOVE_ICON: &str = "icons/settings/delete-02.svg";

impl PaneFlowApp {
    pub(crate) fn agent_dialog_is_open(&self) -> bool {
        self.agent_profile_editor.is_some()
            || self.agent_account_namer.is_some()
            || self.agent_settings_is_open()
    }

    pub(crate) fn render_agent_dialogs(&self, cx: &mut Context<Self>) -> AnyElement {
        let ink = Ink::of(crate::theme::ui_colors());
        if let Some(namer) = self.agent_account_namer.as_ref() {
            return self.render_agent_account_namer(namer, ink, cx);
        }
        if let Some(editor) = self.agent_profile_editor.as_ref() {
            return self.render_agent_profile_dialog(editor, ink, cx);
        }
        match self
            .agent_settings_dialog
            .filter(|agent| agent.is_installed())
        {
            Some(agent) => self.render_agent_settings_dialog(agent, ink, cx),
            None => div().into_any_element(),
        }
    }

    fn handle_agent_dialog_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match modal_key(event) {
            Some(ModalKey::Dismiss) => self.dismiss_agent_dialog_layer(window, cx),
            Some(ModalKey::Confirm) => {
                let menu_open = self.agent_update_menu_open.is_some()
                    || self
                        .agent_profile_editor
                        .as_ref()
                        .is_some_and(|editor| editor.agent_menu_open);
                if menu_open {
                    return;
                }
                if self.agent_account_namer.is_some() {
                    self.confirm_agent_account_name(window, cx);
                } else if self.agent_profile_editor.is_some() {
                    self.save_agent_profile(window, cx);
                } else {
                    self.close_agent_settings(window, cx);
                }
            }
            None => return,
        }
        cx.stop_propagation();
    }

    pub(crate) fn dismiss_agent_dialog_layer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agent_account_namer.is_some() {
            self.close_agent_account_namer(window, cx);
            return;
        }
        if let Some(editor) = self.agent_profile_editor.as_mut() {
            if editor.agent_menu_open {
                editor.agent_menu_open = false;
                cx.notify();
            } else {
                self.close_agent_profile_editor(window, cx);
            }
            return;
        }
        if self.agent_update_menu_open.take().is_some() {
            cx.notify();
            return;
        }
        self.close_agent_settings(window, cx);
    }

    fn render_agent_settings_dialog(
        &self,
        agent: TerminalAgent,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut rows: Vec<AnyElement> = Vec::new();
        let inspection = self
            .agent_updates
            .get(&agent)
            .and_then(|state| state.inspection.as_ref());
        match inspection {
            None => rows.push(
                kit::row(
                    ink,
                    None,
                    Text::new("Updates").line(kit::line(ink, "Checking for updates…")),
                    Some(kit::spinner(ink)),
                )
                .into_any_element(),
            ),
            Some(inspection) => {
                rows.extend(self.render_agent_update_method_row(agent, ink, cx));
                let command = inspection
                    .plan(self.selected_update_method(agent))
                    .map(|plan| {
                        plan.as_ref()
                            .map(agent_updates::UpdatePlan::display_command)
                    });
                match command {
                    Some(Ok(command)) => rows.push(copy_row(
                        SharedString::from(format!("agent-update-copy-{}", agent.tag())),
                        "Update command",
                        "Copy command",
                        command,
                        ink,
                        cx,
                    )),
                    Some(Err(reason)) => rows.push(
                        kit::row(
                            ink,
                            None,
                            Text::new("Updates").line(kit::line(ink, reason.clone())),
                            None,
                        )
                        .into_any_element(),
                    ),
                    None => {}
                }
                if let Some(executable) = inspection.executable.as_ref() {
                    rows.push(copy_row(
                        SharedString::from(format!("agent-path-copy-{}", agent.tag())),
                        "Location",
                        "Copy path",
                        executable.to_string_lossy().into_owned(),
                        ink,
                        cx,
                    ));
                }
            }
        }
        rows.extend(self.render_agent_integration_row(agent, ink, cx));
        rows.push(
            kit::row(
                ink,
                None,
                Text::new("Launch presets").line(kit::line(ink, LAUNCH_PRESETS_DESCRIPTION)),
                Some(
                    kit::button(
                        SharedString::from(format!("agent-profile-add-{}", agent.tag())),
                        "New preset",
                        Tone::Secondary,
                        Size::Small,
                        ink,
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.open_agent_profile_editor(None, agent, false, window, cx);
                    }))
                    .into_any_element(),
                ),
            )
            .into_any_element(),
        );

        let card = kit::dialog(
            "agent-settings-dialog",
            ink,
            format!("{} settings", agent.display_name()),
            "Changes apply as you make them.",
        )
        .track_focus(&self.agent_dialog_focus)
        .on_key_down(cx.listener(Self::handle_agent_dialog_key_down))
        .child(kit::separated(ink, rows))
        .child(kit::dialog_footer(
            None,
            vec![
                kit::button(
                    "agent-settings-done",
                    "Done",
                    Tone::Primary,
                    Size::Medium,
                    ink,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_agent_settings(window, cx);
                }))
                .into_any_element(),
            ],
        ));
        kit::backdrop(
            "agent-settings-backdrop",
            ink,
            card,
            cx.listener(|this, _, window, cx| this.close_agent_settings(window, cx)),
        )
    }

    fn render_agent_update_method_row(
        &self,
        agent: TerminalAgent,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ui = crate::theme::ui_colors();
        let inspection = self.agent_updates.get(&agent)?.inspection.as_ref()?;
        let selected = self.selected_update_method(agent);
        let offered = inspection.offered_methods(selected);
        if offered.len() <= 1 {
            return None;
        }
        let tag = agent.tag();
        let auto_via = inspection
            .plan(UpdateMethod::Auto)
            .and_then(|plan| plan.as_ref().ok())
            .map(|plan| plan.via);
        let option_label = |method: UpdateMethod| -> String {
            match (method, auto_via) {
                (UpdateMethod::Auto, Some(via)) => format!("Automatic ({via})"),
                _ => method.label().to_string(),
            }
        };

        let is_open = self.agent_update_menu_open == Some(agent);
        let mut trigger = select_trigger(
            SharedString::from(format!("agent-update-method-{tag}")),
            "Update method",
            is_open,
            ui,
        )
        .w_full()
        .min_w(px(0.))
        .h(px(SELECT_HEIGHT))
        .py(px(0.))
        .on_press(cx.listener(move |this, _, _window, cx| {
            cx.stop_propagation();
            this.agent_update_menu_open = if is_open { None } else { Some(agent) };
            cx.notify();
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(kit::SIZE_SM))
                .text_color(ink.primary)
                .truncate()
                .child(option_label(selected)),
        )
        .child(select_chevron(ui));
        if is_open {
            let mut menu = select_menu(
                SharedString::from(format!("agent-update-method-list-{tag}")),
                ui,
            )
            .on_mouse_down_out(cx.listener(|this, _, _w, cx| {
                this.agent_update_menu_open = None;
                cx.notify();
            }));
            for method in offered {
                menu = menu.option(
                    select_option(
                        SharedString::from(format!("agent-update-method-{tag}-{}", method.id())),
                        method == selected,
                        ui,
                    )
                    .cursor(CursorStyle::Arrow)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(ink.primary)
                            .child(option_label(method)),
                    ),
                    cx.listener(move |this, _: &(), _w, cx| {
                        this.set_agent_update_method(agent, method, cx);
                    }),
                );
            }
            trigger = trigger.child(deferred_select_menu_at(menu, kit::DIALOG_MENU_PRIORITY));
        }

        let mut text =
            Text::new("Update with").line(kit::line(ink, "The tool Paneflow runs to update it."));
        if let Some(Err(reason)) = inspection.plan(selected) {
            text = text.line(kit::line_in(ink.danger, reason.clone()));
        }
        if let Some(via) = auto_via.filter(|_| inspection.installs_second_copy(selected)) {
            text = text.line(kit::line_in(
                ink.warning,
                format!(
                    "{} was installed through {via}: this installs a second copy, and the one \
                     first on PATH runs.",
                    agent.display_name()
                ),
            ));
        }
        Some(
            kit::row(
                ink,
                None,
                text,
                Some(
                    div()
                        .relative()
                        .flex_none()
                        .w(px(SELECT_WIDTH))
                        .child(trigger)
                        .into_any_element(),
                ),
            )
            .into_any_element(),
        )
    }

    fn render_agent_integration_row(
        &self,
        agent: TerminalAgent,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let slug = agent.runtime().slug;
        let status = self
            .integration_status
            .as_ref()?
            .iter()
            .find(|status| status.slug == slug)?;
        let installed = match status.state {
            IntegrationState::Installed => true,
            IntegrationState::NotInstalled => false,
            IntegrationState::UnsupportedPlatform | IntegrationState::DetectionOnly => {
                return None;
            }
        };
        let description = match status.post_install_step.filter(|_| installed) {
            Some(step) => step.to_string(),
            None => HOOKS_DESCRIPTION.to_string(),
        };
        let control = if self.integration_busy.as_deref() == Some(slug) {
            kit::spinner(ink)
        } else {
            let owned = slug.to_string();
            kit::switch(
                SharedString::from(format!("integration-toggle-{slug}")),
                format!("Paneflow hooks for {}", agent.display_name()),
                installed,
                ink,
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.change_integration(owned.clone(), !installed, cx);
            }))
            .into_any_element()
        };
        let error = self
            .integration_errors
            .get(slug)
            .map(|error| kit::line_in(ink.danger, error.clone()).into_any_element());
        Some(
            kit::row(
                ink,
                None,
                Text::new("Hooks and MCP")
                    .line(kit::line(ink, description))
                    .maybe_line(error),
                Some(control),
            )
            .into_any_element(),
        )
    }

    fn render_agent_account_namer(
        &self,
        namer: &AgentAccountNamer,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let name = namer.agent.display_name();
        let prefix = super::usage_agent(namer.agent)
            .map(super::accounts::account_dir_prefix)
            .unwrap_or_default();
        let folder = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(ENV_GAP))
            .w_full()
            .child(
                kit::mono_line(ink, prefix)
                    .flex_none()
                    .text_color(ink.secondary),
            )
            .child(div().flex_1().min_w_0().child(input_box_sized(
                self.agent_account_name_input.clone(),
                true,
                FIELD_HEIGHT,
                ui,
            )));
        let hint = match namer.error.as_ref() {
            Some(error) => kit::line_in(ink.danger, error.clone()),
            None => kit::line(ink, "Letters, digits, hyphens and underscores."),
        };
        let card = kit::dialog(
            "agent-account-namer-dialog",
            ink,
            format!("New {name} account"),
            "Name the folder that holds this account, or keep the suggested number. The name \
             cannot change once the account exists.",
        )
        .track_focus(&self.agent_dialog_focus)
        .on_key_down(cx.listener(Self::handle_agent_dialog_key_down))
        .child(kit::field(
            ink,
            "Folder",
            folder,
            Some(hint.into_any_element()),
        ))
        .child(kit::dialog_footer(
            None,
            vec![
                kit::button(
                    "agent-account-namer-cancel",
                    "Cancel",
                    Tone::Secondary,
                    Size::Medium,
                    ink,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_agent_account_namer(window, cx);
                }))
                .into_any_element(),
                kit::button(
                    "agent-account-namer-add",
                    "Add account",
                    Tone::Primary,
                    Size::Medium,
                    ink,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.confirm_agent_account_name(window, cx);
                }))
                .into_any_element(),
            ],
        ));
        kit::backdrop(
            "agent-account-namer-backdrop",
            ink,
            card,
            cx.listener(|this, _, window, cx| this.close_agent_account_namer(window, cx)),
        )
    }

    fn render_agent_profile_dialog(
        &self,
        editor: &AgentProfileEditor,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let name = editor.agent.display_name();
        let title = match editor.index {
            Some(_) => "Edit launch preset".to_string(),
            None => format!("New {name} launch preset"),
        };
        let body = format!(
            "Start {name} with its own variables and arguments, such as another model or a \
             proxy. A second account is added with Add account instead."
        );

        let mut form = div().flex().flex_col().gap(px(FORM_GAP)).child(kit::field(
            ink,
            "Name",
            input_box_sized(
                self.agent_profile_name_input.clone(),
                false,
                FIELD_HEIGHT,
                ui,
            ),
            Some(kit::line(ink, "Shown in the agent picker.").into_any_element()),
        ));
        if editor.pick_agent {
            form = form.child(kit::field(
                ink,
                "Base agent",
                self.render_agent_profile_base_select(editor, ink, cx),
                Some(kit::line(ink, "Status, hooks, and sessions follow it.").into_any_element()),
            ));
        }
        form = form
            .child(kit::field(
                ink,
                "Variables",
                self.render_agent_profile_env(editor, ink, cx),
                Some(
                    kit::line(
                        ink,
                        "A leading ~ is your home, and $NAME, ${NAME} or %NAME% takes the \
                         value from your environment.",
                    )
                    .into_any_element(),
                ),
            ))
            .child(kit::field(
                ink,
                "Arguments",
                input_box_sized(
                    self.agent_profile_args_input.clone(),
                    true,
                    FIELD_HEIGHT,
                    ui,
                ),
                Some(kit::line(ink, "Added after the agent's own flags.").into_any_element()),
            ))
            .child(match editor.error.as_ref() {
                Some(error) => kit::line_in(ink.danger, error.clone()).into_any_element(),
                None => div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(kit::line(ink, "Launches"))
                    .child(
                        kit::mono_line(ink, self.agent_profile_command_preview(editor, cx))
                            .text_color(ink.primary),
                    )
                    .into_any_element(),
            });

        let aside = editor.index.map(|idx| {
            let armed = editor.delete_armed;
            kit::button(
                "agent-profile-delete",
                if armed { "Confirm delete" } else { "Delete" },
                Tone::Danger,
                Size::Medium,
                ink,
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                let armed = this
                    .agent_profile_editor
                    .as_ref()
                    .is_some_and(|editor| editor.delete_armed);
                if armed {
                    this.delete_agent_profile(idx, window, cx);
                } else if let Some(editor) = this.agent_profile_editor.as_mut() {
                    editor.delete_armed = true;
                    cx.notify();
                }
            }))
            .into_any_element()
        });

        let card = kit::dialog("agent-profile-dialog", ink, title, body)
            .track_focus(&self.agent_dialog_focus)
            .on_key_down(cx.listener(Self::handle_agent_dialog_key_down))
            .child(form)
            .child(kit::dialog_footer(
                aside,
                vec![
                    kit::button(
                        "agent-profile-cancel",
                        "Cancel",
                        Tone::Secondary,
                        Size::Medium,
                        ink,
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.close_agent_profile_editor(window, cx);
                    }))
                    .into_any_element(),
                    kit::button(
                        "agent-profile-save",
                        "Save",
                        Tone::Primary,
                        Size::Medium,
                        ink,
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.save_agent_profile(window, cx);
                    }))
                    .into_any_element(),
                ],
            ));
        kit::backdrop(
            "agent-profile-backdrop",
            ink,
            card,
            cx.listener(|this, _, window, cx| this.close_agent_profile_editor(window, cx)),
        )
    }

    fn render_agent_profile_env(
        &self,
        editor: &AgentProfileEditor,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> Div {
        let ui = crate::theme::ui_colors();
        let home = dirs::home_dir();
        let removable = editor.env_rows.len() > 1;
        let mut column = div().flex().flex_col().gap(px(ENV_GAP)).w_full();
        for (row_idx, row) in editor.env_rows.iter().enumerate() {
            let value = row.value.read(cx).value();
            let hint = env_value_hint(&value, home.as_deref(), ink);
            column =
                column.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .w_full()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(ENV_GAP))
                                .w_full()
                                .child(div().w(px(ENV_NAME_WIDTH)).flex_none().child(
                                    input_box_sized(row.key.clone(), true, FIELD_HEIGHT, ui),
                                ))
                                .child(div().flex_1().min_w_0().child(input_box_sized(
                                    row.value.clone(),
                                    true,
                                    FIELD_HEIGHT,
                                    ui,
                                )))
                                .when(removable, |line| {
                                    line.child(
                                        kit::icon_button(
                                            SharedString::from(format!(
                                                "agent-profile-env-remove-{row_idx}"
                                            )),
                                            "Remove variable",
                                            REMOVE_ICON,
                                            ink,
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
                                    )
                                }),
                        )
                        .children(hint),
                );
        }
        column.child(
            div().flex().flex_row().child(
                kit::button(
                    "agent-profile-env-add",
                    "Add variable",
                    Tone::Ghost,
                    Size::Small,
                    ink,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.add_agent_profile_env_row(cx);
                })),
            ),
        )
    }

    fn render_agent_profile_base_select(
        &self,
        editor: &AgentProfileEditor,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let is_open = editor.agent_menu_open;
        let current = editor.agent;
        let mut trigger = select_trigger(
            SharedString::from("agent-profile-agent"),
            "Agent",
            is_open,
            ui,
        )
        .w_full()
        .max_w_full()
        .h(px(FIELD_HEIGHT))
        .py(px(0.))
        .on_press(cx.listener(move |this, _, _window, cx| {
            cx.stop_propagation();
            if let Some(editor) = this.agent_profile_editor.as_mut() {
                editor.agent_menu_open = !is_open;
            }
            cx.notify();
        }))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .flex_1()
                .min_w_0()
                .child(kit::mark_tile(current, 18., ink))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(kit::SIZE_MD))
                        .text_color(ink.primary)
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
            for agent in TerminalAgent::for_this_platform() {
                let item = select_option(
                    SharedString::from(format!("agent-profile-agent-item-{}", agent.tag())),
                    agent == current,
                    ui,
                )
                .cursor(CursorStyle::Arrow)
                .child(kit::mark_tile(agent, 18., ink))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ink.primary)
                        .child(agent.display_name()),
                );
                menu = menu.option(
                    item,
                    cx.listener(move |this, _: &(), _w, cx| {
                        if let Some(editor) = this.agent_profile_editor.as_mut() {
                            editor.agent = agent;
                            editor.agent_menu_open = false;
                        }
                        cx.notify();
                    }),
                );
            }
            trigger = trigger.child(deferred_select_menu_at(menu, kit::DIALOG_MENU_PRIORITY));
        }
        div().relative().w_full().child(trigger).into_any_element()
    }
}

fn copy_row(
    id: SharedString,
    title: &'static str,
    copy_label: &'static str,
    value: String,
    ink: Ink,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let copied = value.clone();
    kit::row(
        ink,
        None,
        Text::new(title).line(kit::mono_line(ink, value)),
        Some(
            kit::icon_button(id, copy_label, COPY_ICON, ink)
                .on_click(cx.listener(move |_, _: &ClickEvent, _w, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.clone()));
                }))
                .into_any_element(),
        ),
    )
    .into_any_element()
}

fn env_value_hint(value: &str, home: Option<&std::path::Path>, ink: Ink) -> Option<AnyElement> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let base = |line: Div| {
        line.pl(px(ENV_NAME_WIDTH + ENV_GAP))
            .text_size(px(kit::SIZE_XS))
            .line_height(px(kit::LINE_XS))
            .truncate()
            .into_any_element()
    };
    match crate::env_expand::expand_with_process_env(value, home) {
        Ok(expanded) if expanded == value => None,
        Ok(expanded) => Some(base(
            div()
                .font_family(kit::mono_family())
                .text_color(ink.tertiary)
                .child(expanded),
        )),
        Err(name) => Some(base(
            div()
                .text_color(ink.danger)
                .child(format!("{name} is not set in your environment")),
        )),
    }
}
