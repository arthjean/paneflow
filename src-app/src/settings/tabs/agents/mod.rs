use std::collections::BTreeMap;

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Div, Entity, Hsla, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::*, px, rgb, svg,
};
use paneflow_config::schema::AgentProfileConfig;
use paneflow_mcp_install::IntegrationState;

use crate::PaneFlowApp;
use crate::agent_launcher::{AgentProfile, TerminalAgent};
use crate::agent_updates::{self, AgentUpdateState, UpdateMethod, UpdatePhase};
use crate::settings::components::{
    CARD_PADDING_X, CARD_PADDING_Y, SETTINGS_ROW_PADDING_Y, card_button, deferred_select_menu,
    destructive_color, ghost_icon_button, ghost_skin, ghost_tint_skin, hairline, inverse_fill,
    primary_button, raised_button, raised_icon_button, render_logo, row_text, section_header,
    section_header_with_action, section_title, select_chevron, select_menu, select_option,
    select_trigger, setting_card, setting_text, settings_breadcrumb, soft_button, toggle_row,
    toggle_switch, with_alpha,
};
use crate::settings::search::{self, Block, SearchCard};
use crate::settings::tabs::spaces::{HintTone, render_hint};
use crate::ui_primitives::{
    AccessibleControlExt, BODY, BODY_EMPHASIS, LABEL_SM, LABEL_XS, ROW_RADIUS, TITLE,
    TooltipDelayExt, squircle_skin, text_tooltip,
};
use crate::widgets::text_input::TextInput;

const CONTROL_WIDTH: f32 = 300.;
const ENV_NAME_WIDTH: f32 = 200.;
const INPUT_HEIGHT: f32 = 28.;
const ICON_BUTTON_SIZE: f32 = 26.;
const UPDATE_METHOD_WIDTH: f32 = 220.;
const SECTION_GAP: f32 = 24.;
const SECTION_ACTION_HEIGHT: f32 = 26.;
const INTRO_MAX_WIDTH: f32 = 520.;
const LIST_INSET: f32 = 6.;
const LIST_ROW_PADDING_X: f32 = 10.;
const LIST_ROW_PADDING_Y: f32 = 9.;
const LIST_ROW_GAP: f32 = 12.;
const LIST_ROW_HOVER: f32 = 0.05;
const LIST_LOGO: f32 = 24.;
const PROFILE_LOGO: f32 = 18.;
const HIDDEN_ALPHA: f32 = 0.45;
const IDENTITY_PADDING: f32 = 16.;
const IDENTITY_GAP: f32 = 16.;
const IDENTITY_PLATE: f32 = 48.;
const IDENTITY_PLATE_RADIUS: f32 = 12.;
const IDENTITY_PLATE_FILL: f32 = 0.06;
const IDENTITY_LOGO: f32 = 28.;
const MISSING_LOGO: f32 = 16.;
const MISSING_ALPHA: f32 = 0.55;
const MISSING_GAP: f32 = 14.;
const WELCOME_TEXT_WIDTH: f32 = 380.;
const EDITOR_FILL: f32 = 0.04;
const EDITOR_PADDING: f32 = 12.;
const EDITOR_ROW_GAP: f32 = 14.;
const PAGE_BOTTOM_SPACE: f32 = 180.;
const AGENTS_INTRO: &str = "The coding agents found on your PATH. Open one to update it, connect \
     it to Paneflow, or give it profiles.";
const AGENTS_EMPTY_TITLE: &str = "No coding agent on your PATH";
const AGENTS_EMPTY_BODY: &str =
    "Install one of these agents and it shows up here, ready to launch from any pane.";
const HOOKS_DESCRIPTION: &str =
    "Lets Paneflow resume its conversations and registers the Paneflow MCP server.";
const OTHER_PROFILES_DESCRIPTION: &str =
    "Their agent is not installed here, or Paneflow cannot read them.";
const AGENTS_LIST_KEYWORDS: &[&str] = &["Agents", "Profiles", "Updates", "Hooks"];

fn installed_agents() -> impl Iterator<Item = TerminalAgent> {
    TerminalAgent::for_this_platform().filter(|agent| agent.is_installed())
}

fn missing_agents() -> impl Iterator<Item = TerminalAgent> {
    TerminalAgent::for_this_platform().filter(|agent| !agent.is_installed())
}

fn profile_base(entry: &AgentProfileConfig) -> Option<TerminalAgent> {
    TerminalAgent::from_tag(entry.agent.trim())
}

pub(crate) struct EnvRowInputs {
    pub(crate) key: Entity<TextInput>,
    pub(crate) value: Entity<TextInput>,
}

pub(crate) struct AgentProfileEditor {
    pub(crate) index: Option<usize>,
    pub(crate) agent: TerminalAgent,
    pub(crate) pick_agent: bool,
    pub(crate) agent_menu_open: bool,
    pub(crate) delete_armed: bool,
    pub(crate) error: Option<String>,
    pub(crate) env_rows: Vec<EnvRowInputs>,
}

impl PaneFlowApp {
    pub(crate) fn render_agents_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        if let Some(agent) = self.open_agent_page() {
            return self.render_agent_page(agent, ui, cx);
        }
        let installed: Vec<TerminalAgent> = installed_agents().collect();
        let missing: Vec<TerminalAgent> = missing_agents().collect();
        let mut page = div().flex().flex_col();
        if installed.is_empty() {
            page = page.child(
                Block::new("Agents")
                    .child(render_agents_welcome(&missing, ui))
                    .finish(),
            );
        } else {
            page = page
                .child(self.render_installed_agents_section(&installed, ui, cx))
                .child(render_missing_agents_section(&missing, ui));
        }
        page.children(self.render_other_profiles_section(ui, cx))
            .child(self.render_agent_permissions_section(ui, cx))
            .child(div().h(px(PAGE_BOTTOM_SPACE)).flex_none())
            .into_any_element()
    }

    pub(crate) fn render_agents_heading(&self, title: Div, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        if let Some(agent) = self.open_agent_page() {
            return settings_breadcrumb(
                "agents-breadcrumb",
                "Agents",
                "Back to all agents",
                agent.display_name(),
                ui,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_agent_detail(window, cx);
                }),
            )
            .pb(px(20.))
            .into_any_element();
        }
        let below = if installed_agents().next().is_some() {
            6.
        } else {
            20.
        };
        title.pb(px(below)).into_any_element()
    }

    fn open_agent_page(&self) -> Option<TerminalAgent> {
        self.agent_detail.filter(|agent| agent.is_installed())
    }

    pub(crate) fn agent_detail_is_open(&self) -> bool {
        self.open_agent_page().is_some()
    }

    fn open_agent_detail(
        &mut self,
        agent: TerminalAgent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agent_detail = Some(agent);
        self.agent_profile_editor = None;
        self.agent_update_menu_open = None;
        self.settings_focus.focus(window, cx);
        self.reset_settings_scroll();
        cx.notify();
    }

    pub(crate) fn close_agent_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_detail = None;
        self.agent_profile_editor = None;
        self.agent_update_menu_open = None;
        self.settings_focus.focus(window, cx);
        self.reset_settings_scroll();
        cx.notify();
    }

    pub(crate) fn step_back_from_agent_detail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agent_update_menu_open.take().is_some() {
            cx.notify();
            return;
        }
        self.close_agent_detail(window, cx);
    }

    fn agent_profile_count(&self, agent: TerminalAgent) -> usize {
        self.cached_config
            .agent_profiles
            .iter()
            .filter(|entry| profile_base(entry) == Some(agent))
            .count()
    }

    pub(crate) fn probe_agent_versions(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            smol::unblock(TerminalAgent::probe_missing_versions).await;
            let _ = this.update(cx, |this, cx| {
                cx.notify();
                this.inspect_agent_updates(cx);
            });
        })
        .detach();
    }

    pub(crate) fn refresh_integration_status(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let statuses = smol::unblock(paneflow_mcp_install::list_integrations).await;
            let _ = this.update(cx, |this, cx| {
                this.integration_status = Some(statuses);
                cx.notify();
            });
        })
        .detach();
    }

    fn change_integration(&mut self, slug: String, install: bool, cx: &mut Context<Self>) {
        if self.integration_busy.is_some() {
            return;
        }
        self.integration_busy = Some(slug.clone());
        self.integration_errors.remove(&slug);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let action_slug = slug.clone();
            let result = smol::unblock(move || {
                if install {
                    let hook_binary = crate::ai_hooks::extract::ensure_ai_hook_extracted()
                        .map_err(|error| format!("hook extraction failed: {error:#}"))?;
                    let bridge_binary = crate::ai_hooks::extract::ensure_bridge_extracted()
                        .map_err(|error| format!("bridge extraction failed: {error:#}"))?;
                    paneflow_mcp_install::install_integration(
                        &action_slug,
                        &paneflow_mcp_install::IntegrationBinaries {
                            hook_binary,
                            bridge_binary,
                        },
                        false,
                    )
                } else {
                    paneflow_mcp_install::remove_integration(&action_slug)
                }
            })
            .await;
            let statuses = smol::unblock(paneflow_mcp_install::list_integrations).await;
            let _ = this.update(cx, |this, cx| {
                this.integration_busy = None;
                this.integration_status = Some(statuses);
                match result {
                    Err(error) => {
                        this.integration_errors.insert(slug, error);
                    }
                    Ok(status) if !status.notes.is_empty() => {
                        this.integration_errors
                            .insert(slug, status.notes.join("\n"));
                    }
                    Ok(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_installed_agents_section(
        &self,
        installed: &[TerminalAgent],
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut list = setting_card(ui).p(px(LIST_INSET)).gap(px(2.));
        for agent in installed {
            list = list.child(self.render_agent_list_row(*agent, ui, cx));
        }
        Block::new("Agents")
            .child(
                div()
                    .pb(px(16.))
                    .px(px(2.))
                    .max_w(px(INTRO_MAX_WIDTH))
                    .text_size(BODY)
                    .text_color(ui.muted)
                    .child(AGENTS_INTRO),
            )
            .region("agents-list", AGENTS_LIST_KEYWORDS, list)
            .finish()
    }

    fn render_agent_list_row(
        &self,
        agent: TerminalAgent,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tag = agent.tag();
        let visible = agent.is_visible(&self.cached_config);
        let installed = agent.cached_version();
        let update = self.agent_updates.get(&agent);
        let running = update.is_some_and(AgentUpdateState::is_running);
        let newer = update
            .and_then(|update| update.newer_version(installed.as_deref()))
            .map(str::to_string);
        let meta = agent_meta_label(
            agent,
            installed.as_deref(),
            self.agent_profile_count(agent),
            visible,
        );

        let status: Option<AnyElement> = if running {
            Some(crate::ui_primitives::comet_spinner(px(14.), ui.muted))
        } else {
            newer.map(|newer| {
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.))
                    .text_size(LABEL_SM)
                    .text_color(ui.text)
                    .child(
                        svg()
                            .size(px(12.))
                            .flex_none()
                            .path("icons/settings/arrow-up-01.svg")
                            .text_color(ui.text),
                    )
                    .child(format!("{newer} available"))
                    .into_any_element()
            })
        };

        let key = agent.visibility_config_key();
        let target = !visible;
        let toggle = div()
            .flex_none()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                agent_visibility_switch(agent, visible, ui).on_click(cx.listener(
                    move |this, _: &ClickEvent, _w, cx| {
                        cx.stop_propagation();
                        this.persist_setting(false, key, serde_json::Value::Bool(target), cx);
                    },
                )),
            );

        let group = format!("agent-row-{tag}");
        squircle_skin(
            div()
                .id(SharedString::from(group.clone()))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(LIST_ROW_GAP))
                .px(px(LIST_ROW_PADDING_X))
                .py(px(LIST_ROW_PADDING_Y))
                .cursor(CursorStyle::PointingHand),
            group,
            ROW_RADIUS,
            None,
            Some(with_alpha(ui.text, LIST_ROW_HOVER)),
        )
        .role(gpui::accesskit::Role::Button)
        .aria_label(format!("Open {}", agent.display_name()))
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_agent_detail(agent, window, cx);
        }))
        .child(
            div()
                .flex_none()
                .size(px(LIST_LOGO))
                .flex()
                .items_center()
                .justify_center()
                .when(!visible, |slot| slot.opacity(HIDDEN_ALPHA))
                .child(agent_logo(agent, LIST_LOGO, ui)),
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
                        .truncate()
                        .text_size(BODY_EMPHASIS)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(if visible { ui.text } else { ui.muted })
                        .child(agent.display_name()),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(LABEL_SM)
                        .text_color(ui.muted)
                        .child(meta),
                ),
        )
        .children(status)
        .child(toggle)
        .child(
            svg()
                .size(px(12.))
                .flex_none()
                .path("icons/settings/arrow-right-01.svg")
                .text_color(ui.muted),
        )
        .into_any_element()
    }

    fn render_agent_page(
        &self,
        agent: TerminalAgent,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut page = Block::new("Agents")
            .gap(SECTION_GAP)
            .child(self.render_agent_identity(agent, ui, cx));
        if let Some(installation) = self.render_agent_installation(agent, ui, cx) {
            page = page.child(installation);
        }
        if let Some(integration) = self.render_agent_integration(agent, ui, cx) {
            page = page.child(integration);
        }
        div()
            .flex()
            .flex_col()
            .child(
                page.child(self.render_agent_profiles_section(agent, ui, cx))
                    .finish(),
            )
            .child(div().h(px(PAGE_BOTTOM_SPACE)).flex_none())
            .into_any_element()
    }

    fn render_agent_identity(
        &self,
        agent: TerminalAgent,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tag = agent.tag();
        let installed = agent.cached_version();
        let method = self.selected_update_method(agent);
        let state = self.agent_updates.get(&agent);
        let inspection = state.and_then(|state| state.inspection.as_ref());
        let plan = inspection.and_then(|inspection| inspection.plan(method));
        let latest = inspection.and_then(|inspection| inspection.latest.as_deref());
        let newer = state.and_then(|state| state.newer_version(installed.as_deref()));
        let up_to_date = state.is_some_and(|state| state.is_up_to_date(installed.as_deref()));
        let running = state.is_some_and(AgentUpdateState::is_running);
        let phase = state.and_then(|state| state.phase.as_ref());

        let (status, status_color) = update_status_line(phase, plan, newer, latest, ui);
        let offers_update = plan.is_some_and(Result::is_ok) && !up_to_date;
        let sessions = self.open_agent_sessions(agent);
        let sessions_note = (sessions > 0 && offers_update && !running).then(|| {
            if sessions == 1 {
                "1 open session keeps the current version until it restarts.".to_string()
            } else {
                format!("{sessions} open sessions keep the current version until they restart.")
            }
        });

        let version: SharedString = match installed.as_deref() {
            Some(version) => format!("Version {version}").into(),
            None => "Version unknown".into(),
        };
        let action: Option<AnyElement> = if running {
            Some(crate::ui_primitives::comet_spinner(px(16.), ui.muted))
        } else if offers_update {
            let (fill, ink) = inverse_fill(ui);
            Some(
                raised_icon_button(
                    SharedString::from(format!("agent-update-{tag}")),
                    match newer {
                        Some(newer) => format!("Update to {newer}"),
                        None => "Update".to_string(),
                    },
                    "icons/settings/arrow-up-01.svg",
                    fill,
                    ink,
                    true,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.start_agent_update(agent, cx);
                }))
                .into_any_element(),
            )
        } else {
            None
        };

        let plate = div()
            .flex_none()
            .size(px(IDENTITY_PLATE))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(IDENTITY_PLATE_RADIUS))
            .bg(with_alpha(ui.text, IDENTITY_PLATE_FILL))
            .child(agent_logo(agent, IDENTITY_LOGO, ui));

        setting_card(ui)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(IDENTITY_GAP))
                    .p(px(IDENTITY_PADDING))
                    .child(plate)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(TITLE)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(ui.text)
                                    .child(version),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_size(BODY)
                                    .text_color(status_color)
                                    .when(running, |line| line.truncate())
                                    .child(status),
                            )
                            .when_some(sessions_note, |column, note| {
                                column.child(
                                    div().text_size(LABEL_SM).text_color(ui.muted).child(note),
                                )
                            }),
                    )
                    .children(action),
            )
            .into_any_element()
    }

    fn render_agent_installation(
        &self,
        agent: TerminalAgent,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let inspection = self.agent_updates.get(&agent)?.inspection.as_ref()?;
        let tag = agent.tag();
        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(method) = self.render_agent_update_method_row(agent, ui, cx) {
            rows.push(method);
        }
        let command = inspection
            .plan(self.selected_update_method(agent))
            .and_then(|plan| plan.as_ref().ok())
            .map(agent_updates::UpdatePlan::display_command);
        if let Some(command) = command {
            let copied = command.clone();
            rows.push(card_row(
                "Update command",
                div()
                    .min_w_0()
                    .truncate()
                    .child(mono_text(command.into(), ui.muted))
                    .into_any_element(),
                Some(
                    copy_button(
                        SharedString::from(format!("agent-update-copy-{tag}")),
                        "Copy command",
                        ui,
                    )
                    .on_click(cx.listener(move |_, _: &ClickEvent, _w, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.clone()));
                    }))
                    .into_any_element(),
                ),
                ui,
            ));
        }
        if let Some(executable) = inspection.executable.as_ref() {
            let path = executable.to_string_lossy().into_owned();
            let copied = path.clone();
            rows.push(card_row(
                "Location",
                div()
                    .min_w_0()
                    .truncate()
                    .child(mono_text(path.into(), ui.muted))
                    .into_any_element(),
                Some(
                    copy_button(
                        SharedString::from(format!("agent-path-copy-{tag}")),
                        "Copy path",
                        ui,
                    )
                    .on_click(cx.listener(move |_, _: &ClickEvent, _w, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.clone()));
                    }))
                    .into_any_element(),
                ),
                ui,
            ));
        }
        if rows.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .child(section_header(ui, "Installation"))
                .child(separated_card(rows, ui))
                .into_any_element(),
        )
    }

    fn render_agent_update_method_row(
        &self,
        agent: TerminalAgent,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
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
        .h(px(INPUT_HEIGHT))
        .py(px(0.))
        .on_press(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.agent_update_menu_open = if is_open { None } else { Some(agent) };
            this.settings_focus.focus(window, cx);
            cx.notify();
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(BODY)
                .text_color(ui.text)
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
                            .text_color(ui.text)
                            .child(option_label(method)),
                    ),
                    cx.listener(move |this, _: &(), _w, cx| {
                        this.set_agent_update_method(agent, method, cx);
                    }),
                );
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }

        let mut description = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .min_w_0()
            .child(detail_text(
                "The tool Paneflow runs to update it.".to_string(),
                ui.muted,
            ));
        if let Some(Err(reason)) = inspection.plan(selected) {
            description = description.child(render_hint(reason.clone(), HintTone::Error, ui));
        }
        if let Some(via) = auto_via.filter(|_| inspection.installs_second_copy(selected)) {
            description = description.child(render_hint(
                format!(
                    "{} was installed through {via}: this installs a second copy, and the one \
                     first on PATH runs.",
                    agent.display_name()
                ),
                HintTone::Warning,
                ui,
            ));
        }

        Some(card_row(
            "Update with",
            description.into_any_element(),
            Some(
                div()
                    .relative()
                    .flex_none()
                    .w(px(UPDATE_METHOD_WIDTH))
                    .child(trigger)
                    .into_any_element(),
            ),
            ui,
        ))
    }

    fn render_agent_integration(
        &self,
        agent: TerminalAgent,
        ui: crate::theme::UiColors,
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
        let error = self.integration_errors.get(slug).cloned();
        let description = match status.post_install_step.filter(|_| installed) {
            Some(step) => step.to_string(),
            None => HOOKS_DESCRIPTION.to_string(),
        };
        let control = if self.integration_busy.as_deref() == Some(slug) {
            crate::ui_primitives::comet_spinner(px(14.), ui.muted)
        } else {
            let owned = slug.to_string();
            toggle_switch(
                SharedString::from(format!("integration-toggle-{slug}")),
                format!("Paneflow hooks for {}", agent.display_name()),
                installed,
                ui,
            )
            .cursor(CursorStyle::PointingHand)
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.change_integration(owned.clone(), !installed, cx);
            }))
            .into_any_element()
        };
        let mut text = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(detail_text(description, ui.muted));
        if let Some(error) = error {
            text = text.child(render_hint(error, HintTone::Error, ui));
        }
        Some(
            div()
                .flex()
                .flex_col()
                .child(section_header(ui, "Integration"))
                .child(separated_card(
                    vec![card_row(
                        "Hooks and MCP",
                        text.into_any_element(),
                        Some(control),
                        ui,
                    )],
                    ui,
                ))
                .into_any_element(),
        )
    }

    fn render_agent_profiles_section(
        &self,
        agent: TerminalAgent,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editor = self.agent_profile_editor.as_ref();
        let creating = editor.is_some_and(|editor| editor.index.is_none());
        let attached: Vec<(usize, &AgentProfileConfig)> = self
            .cached_config
            .agent_profiles
            .iter()
            .enumerate()
            .filter(|(_, entry)| profile_base(entry) == Some(agent))
            .collect();

        let action = if attached.is_empty() || creating {
            div().h(px(SECTION_ACTION_HEIGHT)).into_any_element()
        } else {
            soft_button(
                "agent-profile-new",
                "New profile",
                ui,
                cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.open_agent_profile_editor(None, agent, false, cx);
                }),
            )
            .into_any_element()
        };

        let mut card = setting_card(ui).p(px(LIST_INSET)).gap(px(2.));
        for (idx, entry) in &attached {
            card = card.child(self.render_profile_slot(*idx, entry, false, ui, cx));
        }
        if creating && let Some(editor) = editor {
            card = card.child(self.render_agent_profile_editor(editor, ui, cx));
        } else if attached.is_empty() {
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(16.))
                    .px(px(LIST_ROW_PADDING_X))
                    .py(px(LIST_ROW_PADDING_Y))
                    .child(row_text(
                        "No profiles",
                        format!(
                            "Launch {} with its own variables and arguments, such as a second \
                             account.",
                            agent.display_name()
                        ),
                        ui,
                    ))
                    .child(
                        ghost_icon_button(
                            "agent-profile-add",
                            "New profile",
                            "icons/settings/add-01.svg",
                            ui,
                        )
                        .on_click(cx.listener(
                            move |this, _: &ClickEvent, _w, cx| {
                                this.open_agent_profile_editor(None, agent, false, cx);
                            },
                        )),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .child(section_header_with_action(ui, "Profiles", action))
            .child(card)
            .into_any_element()
    }

    fn render_other_profiles_section(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let orphans: Vec<(usize, &AgentProfileConfig)> = self
            .cached_config
            .agent_profiles
            .iter()
            .enumerate()
            .filter(|(_, entry)| !profile_base(entry).is_some_and(TerminalAgent::is_installed))
            .collect();
        if orphans.is_empty() {
            return None;
        }
        let mut card = setting_card(ui).p(px(LIST_INSET)).gap(px(2.));
        for (idx, entry) in orphans {
            card = card.child(self.render_profile_slot(idx, entry, true, ui, cx));
        }
        Some(
            Block::new("Other profiles")
                .top_gap(SECTION_GAP)
                .child(section_title(
                    ui,
                    "Other profiles",
                    Some(OTHER_PROFILES_DESCRIPTION),
                ))
                .child(card)
                .finish(),
        )
    }

    fn render_profile_slot(
        &self,
        idx: usize,
        entry: &AgentProfileConfig,
        show_agent: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self
            .agent_profile_editor
            .as_ref()
            .filter(|editor| editor.index == Some(idx))
        {
            Some(editor) => self.render_agent_profile_editor(editor, ui, cx),
            None => self.render_agent_profile_row(idx, entry, show_agent, ui, cx),
        }
    }

    fn render_agent_profile_row(
        &self,
        idx: usize,
        entry: &AgentProfileConfig,
        show_agent: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let base = profile_base(entry);
        match AgentProfile::from_config(entry) {
            Ok(profile) => {
                let detailed = !profile.env.is_empty() || !profile.args.is_empty();
                if show_agent {
                    meta = meta.child(if profile.agent.is_installed() {
                        profile.agent.display_name().to_string()
                    } else {
                        format!("{} is not installed", profile.agent.display_name())
                    });
                    if detailed {
                        meta = meta.child("·");
                    }
                }
                for (key, value) in &profile.env {
                    meta = meta.child(chip(format!("{key}={value}"), ui));
                }
                if !profile.args.is_empty() {
                    meta = meta.child(chip(profile.args.join(" "), ui));
                }
                if !detailed && !show_agent {
                    meta = meta.child(format!("Launches {} as is", profile.agent.binary()));
                }
            }
            Err(reason) => {
                meta = meta
                    .text_color(ui.vc_deleted)
                    .child(format!("Cannot be read: {reason}"));
            }
        }

        let fallback = base.unwrap_or(TerminalAgent::ClaudeCode);
        let group = format!("agent-profile-row-{idx}");
        squircle_skin(
            div()
                .id(SharedString::from(group.clone()))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(LIST_ROW_GAP))
                .px(px(LIST_ROW_PADDING_X))
                .py(px(LIST_ROW_PADDING_Y))
                .cursor(CursorStyle::PointingHand),
            group.clone(),
            ROW_RADIUS,
            None,
            Some(with_alpha(ui.text, LIST_ROW_HOVER)),
        )
        .role(gpui::accesskit::Role::Button)
        .aria_label(format!("Edit profile {}", entry.name))
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.open_agent_profile_editor(Some(idx), fallback, show_agent, cx);
        }))
        .when(show_agent, |row| {
            row.child(
                div()
                    .flex_none()
                    .size(px(PROFILE_LOGO))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when_some(base, |slot, agent| {
                        slot.child(agent_logo(agent, PROFILE_LOGO, ui))
                    }),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .truncate()
                        .text_size(BODY_EMPHASIS)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(ui.text)
                        .child(entry.name.clone()),
                )
                .child(meta),
        )
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path("icons/settings/pencil-edit-02.svg")
                .text_color(ui.muted)
                .opacity(0.)
                .group_hover(SharedString::from(group), |style| style.opacity(1.)),
        )
        .into_any_element()
    }

    fn render_agent_profile_editor(
        &self,
        editor: &AgentProfileEditor,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let home = dirs::home_dir();
        let row_values: Vec<String> = editor
            .env_rows
            .iter()
            .map(|row| row.value.read(cx).value())
            .collect();

        let removable = editor.env_rows.len() > 1;
        let mut env_column = div().flex().flex_col().gap(px(6.)).w_full();
        for (row_idx, row) in editor.env_rows.iter().enumerate() {
            let hint = row_values
                .get(row_idx)
                .and_then(|value| env_value_hint(value, home.as_deref(), ui));
            env_column = env_column.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .w_full()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .w_full()
                            .child(div().w(px(ENV_NAME_WIDTH)).flex_none().child(input_box(
                                row.key.clone(),
                                true,
                                ui,
                            )))
                            .child(div().flex_1().min_w_0().child(input_box(
                                row.value.clone(),
                                true,
                                ui,
                            )))
                            .when(removable, |d| {
                                d.child(
                                    icon_button(
                                        SharedString::from(format!(
                                            "agent-profile-env-remove-{row_idx}"
                                        )),
                                        "icons/settings/delete-02.svg",
                                        with_alpha(ui.text, 0.8),
                                        destructive_color(),
                                        ui,
                                    )
                                    .accessible_control(
                                        gpui::accesskit::Role::Button,
                                        "Remove variable",
                                    )
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, _w, cx| {
                                            if let Some(editor) = this.agent_profile_editor.as_mut()
                                                && row_idx < editor.env_rows.len()
                                            {
                                                editor.env_rows.remove(row_idx);
                                                cx.notify();
                                            }
                                        },
                                    )),
                                )
                            }),
                    )
                    .when_some(hint, |d, hint| d.child(hint)),
            );
        }
        env_column = env_column.child(
            div().flex().flex_row().child(
                ghost_icon_button(
                    "agent-profile-env-add",
                    "Add variable",
                    "icons/settings/add-01.svg",
                    ui,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.add_agent_profile_env_row(cx);
                })),
            ),
        );

        let summary: AnyElement = match editor.error.as_ref() {
            Some(error) => render_hint(error.clone(), HintTone::Error, ui),
            None => div()
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
                .child(mono_text(
                    SharedString::from(self.agent_profile_command_preview(editor, cx)),
                    ui.text,
                ))
                .into_any_element(),
        };

        let delete: Option<AnyElement> = editor.index.map(|idx| {
            if editor.delete_armed {
                raised_button(
                    "agent-profile-delete-confirm",
                    "Delete profile",
                    destructive_color(),
                    gpui::white(),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.delete_agent_profile(idx, cx);
                }))
                .into_any_element()
            } else {
                ghost_tint_skin(
                    div()
                        .id("agent-profile-delete")
                        .flex_none()
                        .px(px(10.))
                        .py(px(4.))
                        .text_size(BODY)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(ui.vc_deleted),
                    "agent-profile-delete-squircle",
                    ROW_RADIUS,
                    ui.vc_deleted,
                )
                .accessible_control(gpui::accesskit::Role::Button, "Delete profile")
                .child("Delete")
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    if let Some(editor) = this.agent_profile_editor.as_mut() {
                        editor.delete_armed = true;
                    }
                    cx.notify();
                }))
                .into_any_element()
            }
        });

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .children(delete)
            .child(div().flex_1())
            .child(
                card_button("agent-profile-cancel", "Cancel", ui).on_click(cx.listener(
                    |this, _: &ClickEvent, _w, cx| {
                        this.close_agent_profile_editor(cx);
                    },
                )),
            )
            .child(primary_button(
                "agent-profile-save",
                "Save",
                cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.save_agent_profile(cx);
                }),
            ));

        let mut form = div()
            .flex()
            .flex_col()
            .gap(px(EDITOR_ROW_GAP))
            .child(editor_row(
                "Name",
                "Shown in the agent picker.",
                div().w(px(CONTROL_WIDTH)).child(input_box(
                    self.agent_profile_name_input.clone(),
                    false,
                    ui,
                )),
                ui,
            ));
        if editor.pick_agent {
            form = form.child(editor_row(
                "Base agent",
                "Status, hooks, and sessions follow it.",
                div()
                    .w(px(CONTROL_WIDTH))
                    .child(self.render_agent_profile_base_select(editor, ui, cx)),
                ui,
            ));
        }
        form = form
            .child(editor_stacked_row(
                "Variables",
                "Set on the agent process. A leading ~ is your home, and $NAME, ${NAME} or \
                 %NAME% takes the value from your environment.",
                env_column,
                ui,
            ))
            .child(editor_row(
                "Arguments",
                "Added after the agent's own flags.",
                div().w(px(CONTROL_WIDTH)).child(input_box(
                    self.agent_profile_args_input.clone(),
                    true,
                    ui,
                )),
                ui,
            ))
            .child(summary)
            .child(footer);

        squircle_skin(
            div()
                .id("agent-profile-editor")
                .flex()
                .flex_col()
                .p(px(EDITOR_PADDING)),
            "agent-profile-editor-squircle",
            ROW_RADIUS,
            Some(with_alpha(ui.text, EDITOR_FILL)),
            Some(with_alpha(ui.text, EDITOR_FILL)),
        )
        .child(form)
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
        let mut trigger = select_trigger(
            SharedString::from("agent-profile-agent"),
            "Agent",
            is_open,
            ui,
        )
        .w_full()
        .max_w(px(CONTROL_WIDTH))
        .h(px(INPUT_HEIGHT))
        .py(px(0.))
        .on_press(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            if let Some(editor) = this.agent_profile_editor.as_mut() {
                editor.agent_menu_open = !is_open;
            }
            this.settings_focus.focus(window, cx);
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
                .child(agent_logo(current, 14., ui))
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
            for agent in TerminalAgent::for_this_platform() {
                let item = select_option(
                    SharedString::from(format!("agent-profile-agent-item-{}", agent.tag())),
                    agent == current,
                    ui,
                )
                .cursor(CursorStyle::Arrow)
                .child(agent_logo(agent, 14., ui))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ui.text)
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
            trigger = trigger.child(deferred_select_menu(menu));
        }
        div().relative().w_full().child(trigger).into_any_element()
    }

    fn inspect_agent_updates(&mut self, cx: &mut Context<Self>) {
        for agent in installed_agents() {
            if self
                .agent_updates
                .get(&agent)
                .is_some_and(AgentUpdateState::is_running)
            {
                continue;
            }
            cx.spawn(async move |this, cx| {
                let inspection = smol::unblock(move || agent_updates::inspect(agent)).await;
                let _ = this.update(cx, |this, cx| {
                    let state = this.agent_updates.entry(agent).or_default();
                    if !state.is_running() {
                        state.inspection = Some(inspection);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    fn selected_update_method(&self, agent: TerminalAgent) -> UpdateMethod {
        UpdateMethod::from_id(
            self.cached_config
                .agents
                .as_ref()
                .and_then(|agents| agents.update_methods.get(agent.runtime().slug))
                .map(String::as_str),
        )
    }

    fn set_agent_update_method(
        &mut self,
        agent: TerminalAgent,
        method: UpdateMethod,
        cx: &mut Context<Self>,
    ) {
        self.agent_update_menu_open = None;
        let mut agents = self.cached_config.agents.clone().unwrap_or_default();
        let slug = agent.runtime().slug.to_string();
        if method == UpdateMethod::Auto {
            agents.update_methods.remove(&slug);
        } else {
            agents.update_methods.insert(slug, method.id().to_string());
        }
        if let Some(state) = self.agent_updates.get_mut(&agent)
            && !state.is_running()
        {
            state.phase = None;
        }
        match serde_json::to_value(agents) {
            Ok(value) => self.persist_setting(false, "agents", value, cx),
            Err(_) => cx.notify(),
        }
    }

    fn start_agent_update(&mut self, agent: TerminalAgent, cx: &mut Context<Self>) {
        let method = self.selected_update_method(agent);
        let Some(state) = self.agent_updates.get_mut(&agent) else {
            return;
        };
        if state.is_running() {
            return;
        }
        let Some(plan) = state.runnable_plan(method).cloned() else {
            return;
        };
        state.phase = Some(UpdatePhase::Running { progress: None });
        cx.notify();
        let before = agent.cached_version();
        cx.spawn(async move |this, cx| {
            let (sender, receiver) = smol::channel::unbounded::<String>();
            let job = smol::unblock(move || {
                agent_updates::run_update(&plan, move |line| {
                    let _ = sender.try_send(line);
                })
            });
            while let Ok(line) = receiver.recv().await {
                let _ = this.update(cx, |this, cx| {
                    if let Some(state) = this.agent_updates.get_mut(&agent)
                        && state.is_running()
                    {
                        state.phase = Some(UpdatePhase::Running {
                            progress: Some(line),
                        });
                        cx.notify();
                    }
                });
            }
            let result = job.await;
            let after = smol::unblock(move || agent.refresh_version()).await;
            let phase = match result {
                Err(message) => UpdatePhase::Failed { message },
                Ok(()) if after.is_some() && after != before => {
                    UpdatePhase::Updated { version: after }
                }
                Ok(()) => UpdatePhase::Unchanged,
            };
            let _ = this.update(cx, |this, cx| {
                this.agent_updates.entry(agent).or_default().phase = Some(phase);
                cx.notify();
            });
        })
        .detach();
    }

    fn open_agent_sessions(&self, agent: TerminalAgent) -> usize {
        let mut surfaces = std::collections::HashSet::new();
        let mut unbound = 0;
        for session in self
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.agent_sessions.values())
            .filter(|session| session.tool == agent)
        {
            match session.surface_id {
                Some(surface) => {
                    surfaces.insert(surface);
                }
                None => unbound += 1,
            }
        }
        surfaces.len() + unbound
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

        let mut card = SearchCard::new(ui)
            .row(
                &search::CLAUDE_FULL_ACCESS,
                toggle_row(
                    "row-claude-bypass",
                    search::CLAUDE_FULL_ACCESS.title,
                    search::CLAUDE_FULL_ACCESS.description,
                    None,
                    bypass,
                    "claude_code_bypass_permissions",
                    ui,
                    cx,
                ),
            )
            .row(
                &search::AI_FREE_ACCESS,
                toggle_row(
                    "row-ai-unrestricted",
                    search::AI_FREE_ACCESS.title,
                    search::AI_FREE_ACCESS.description,
                    None,
                    unrestricted,
                    "ai_unrestricted",
                    ui,
                    cx,
                ),
            );
        if unrestricted {
            card = card.row(
                &search::INJECTION_FENCE,
                toggle_row(
                    "row-ai-injection-fence",
                    search::INJECTION_FENCE.title,
                    search::INJECTION_FENCE.description,
                    None,
                    fence,
                    "ai_injection_fence",
                    ui,
                    cx,
                ),
            );
            if !fence {
                card = card.fixed(
                    div()
                        .px(px(CARD_PADDING_X))
                        .py(px(8.))
                        .text_size(BODY)
                        .text_color(destructive_color())
                        .child("Fence off: a malicious pane can silently redirect your agent."),
                );
            }
        }

        Block::new("Permissions")
            .top_gap(SECTION_GAP)
            .child(section_title(ui, "Permissions", None))
            .card(card)
            .finish()
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
        agent: TerminalAgent,
        pick_agent: bool,
        cx: &mut Context<Self>,
    ) {
        let entry = index.and_then(|idx| self.cached_config.agent_profiles.get(idx).cloned());
        let agent = entry.as_ref().and_then(profile_base).unwrap_or(agent);
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
            pick_agent,
            agent_menu_open: false,
            delete_armed: false,
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

        let home = dirs::home_dir();
        let draft = collect_env(&raw_env).and_then(|env| {
            validate_env_values(&env, home.as_deref())?;
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

pub(crate) fn new_env_row(key: &str, value: &str, cx: &mut Context<PaneFlowApp>) -> EnvRowInputs {
    let key = cx
        .new(|cx| TextInput::new(key.to_string(), "KEY", cx).with_accessible_name("Variable name"));
    cx.observe(&key, |_, _, cx| cx.notify()).detach();
    let value = cx.new(|cx| {
        TextInput::new(value.to_string(), "value", cx).with_accessible_name("Variable value")
    });
    cx.observe(&value, |_, _, cx| cx.notify()).detach();
    EnvRowInputs { key, value }
}

pub(crate) fn validate_env_values(
    env: &BTreeMap<String, String>,
    home: Option<&std::path::Path>,
) -> Result<(), String> {
    for (key, value) in env {
        if let Err(name) = crate::env_expand::expand_with_process_env(value, home) {
            return Err(format!(
                "'{key}' references {name}, which is not set in your environment"
            ));
        }
    }
    Ok(())
}

pub(crate) fn collect_env(rows: &[(String, String)]) -> Result<BTreeMap<String, String>, String> {
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

fn render_missing_agents_section(
    missing: &[TerminalAgent],
    ui: crate::theme::UiColors,
) -> AnyElement {
    if missing.is_empty() {
        return div().into_any_element();
    }
    Block::new("Not installed")
        .top_gap(SECTION_GAP)
        .child(section_header_with_action(
            ui,
            "Not installed",
            div()
                .text_size(LABEL_SM)
                .text_color(ui.muted)
                .child(missing.len().to_string()),
        ))
        .child(
            setting_card(ui).child(
                div()
                    .px(px(CARD_PADDING_X + 2.))
                    .py(px(14.))
                    .child(missing_agents_strip(missing, false, ui)),
            ),
        )
        .finish()
}

fn render_agents_welcome(missing: &[TerminalAgent], ui: crate::theme::UiColors) -> AnyElement {
    setting_card(ui)
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .px(px(32.))
                .pt(px(32.))
                .pb(px(28.))
                .child(
                    div()
                        .text_size(TITLE)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(ui.text)
                        .child(AGENTS_EMPTY_TITLE),
                )
                .child(
                    div()
                        .pt(px(6.))
                        .max_w(px(WELCOME_TEXT_WIDTH))
                        .text_center()
                        .text_size(BODY)
                        .text_color(ui.muted)
                        .child(AGENTS_EMPTY_BODY),
                )
                .child(
                    div()
                        .pt(px(22.))
                        .max_w(px(INTRO_MAX_WIDTH))
                        .child(missing_agents_strip(missing, true, ui)),
                ),
        )
        .into_any_element()
}

fn missing_agents_strip(
    agents: &[TerminalAgent],
    centered: bool,
    ui: crate::theme::UiColors,
) -> Div {
    let mut strip = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(MISSING_GAP))
        .when(centered, |strip| strip.justify_center());
    for agent in agents {
        strip = strip.child(
            div()
                .id(SharedString::from(format!("agent-missing-{}", agent.tag())))
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .delayed_tooltip(text_tooltip(format!(
                    "Paneflow looks for {} on your PATH",
                    agent.binary()
                )))
                .child(div().flex_none().opacity(MISSING_ALPHA).child(agent_logo(
                    *agent,
                    MISSING_LOGO,
                    ui,
                )))
                .child(
                    div()
                        .text_size(BODY)
                        .text_color(ui.muted)
                        .child(agent.display_name()),
                ),
        );
    }
    strip
}

fn agent_visibility_switch(
    agent: TerminalAgent,
    visible: bool,
    ui: crate::theme::UiColors,
) -> gpui::Stateful<Div> {
    toggle_switch(
        SharedString::from(format!("agent-visible-{}", agent.tag())),
        format!("Show {} in the agent picker", agent.display_name()),
        visible,
        ui,
    )
    .cursor(CursorStyle::PointingHand)
}

fn agent_meta_label(
    agent: TerminalAgent,
    version: Option<&str>,
    profiles: usize,
    visible: bool,
) -> String {
    let mut parts = vec![match version {
        Some(version) => format!("{} {version}", agent.binary()),
        None => agent.binary().to_string(),
    }];
    match profiles {
        0 => {}
        1 => parts.push("1 profile".to_string()),
        n => parts.push(format!("{n} profiles")),
    }
    if !visible {
        parts.push("Hidden from the picker".to_string());
    }
    parts.join(" · ")
}

fn update_status_line(
    phase: Option<&UpdatePhase>,
    plan: Option<&Result<agent_updates::UpdatePlan, String>>,
    newer: Option<&str>,
    latest: Option<&str>,
    ui: crate::theme::UiColors,
) -> (String, Hsla) {
    match (phase, plan) {
        (Some(UpdatePhase::Running { progress }), _) => (
            progress.clone().unwrap_or_else(|| "Updating…".to_string()),
            ui.muted,
        ),
        (Some(UpdatePhase::Failed { message }), _) => {
            (format!("Update failed: {message}"), ui.vc_deleted)
        }
        (Some(UpdatePhase::Updated { version }), _) => (
            match version {
                Some(version) => format!("Updated to {version}."),
                None => "Updated.".to_string(),
            },
            ui.muted,
        ),
        (Some(UpdatePhase::Unchanged), _) => ("Already up to date.".to_string(), ui.muted),
        (None, None) => ("Checking for updates…".to_string(), ui.muted),
        (None, Some(Err(reason))) => (reason.clone(), ui.muted),
        (None, Some(Ok(plan))) => (
            match (newer, latest) {
                (Some(newer), _) => format!("Version {newer} is available through {}.", plan.via),
                (None, Some(_)) => "Up to date.".to_string(),
                (None, None) => format!("Updates through {}.", plan.via),
            },
            ui.muted,
        ),
    }
}

fn separated_card(rows: Vec<AnyElement>, ui: crate::theme::UiColors) -> Div {
    let mut card = setting_card(ui).py(px(CARD_PADDING_Y));
    for (idx, row) in rows.into_iter().enumerate() {
        if idx > 0 {
            card = card.child(div().px(px(CARD_PADDING_X)).child(hairline(ui)));
        }
        card = card.child(row);
    }
    card
}

fn card_row(
    title: &'static str,
    description: AnyElement,
    control: Option<AnyElement>,
    ui: crate::theme::UiColors,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(16.))
        .px(px(CARD_PADDING_X))
        .py(px(SETTINGS_ROW_PADDING_Y))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(BODY)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(ui.text)
                        .child(title),
                )
                .child(description),
        )
        .children(control)
        .into_any_element()
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
        .child(setting_text(ui, title, description))
        .child(div().flex_shrink_0().child(control))
}

fn editor_stacked_row(
    title: &'static str,
    description: &'static str,
    control: impl IntoElement,
    ui: crate::theme::UiColors,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .flex_row()
                .child(setting_text(ui, title, description)),
        )
        .child(control)
}

fn env_value_hint(
    value: &str,
    home: Option<&std::path::Path>,
    ui: crate::theme::UiColors,
) -> Option<AnyElement> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let base = div()
        .pl(px(ENV_NAME_WIDTH + 6.))
        .text_size(LABEL_XS)
        .truncate();
    match crate::env_expand::expand_with_process_env(value, home) {
        Ok(expanded) if expanded == value => None,
        Ok(expanded) => Some(
            base.font_family(mono_family())
                .text_color(ui.muted)
                .child(expanded)
                .into_any_element(),
        ),
        Err(name) => Some(
            base.text_color(ui.vc_deleted)
                .child(format!("{name} is not set in your environment"))
                .into_any_element(),
        ),
    }
}

fn detail_text(text: String, color: Hsla) -> AnyElement {
    div()
        .text_size(LABEL_SM)
        .text_color(color)
        .child(text)
        .into_any_element()
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

pub(crate) fn input_box(
    input: Entity<TextInput>,
    mono: bool,
    ui: crate::theme::UiColors,
) -> impl IntoElement {
    crate::settings::components::input_frame(
        div()
            .w_full()
            .h(px(INPUT_HEIGHT))
            .flex()
            .items_center()
            .px(px(10.)),
        &input,
        false,
        ui,
    )
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

pub(crate) fn icon_button(
    id: SharedString,
    icon: &'static str,
    resting: Hsla,
    hovered: Hsla,
    ui: crate::theme::UiColors,
) -> crate::ui_primitives::AnimatedHover {
    let group = SharedString::from(format!("{id}-squircle"));
    ghost_skin(
        div()
            .id(id)
            .flex_none()
            .w(px(ICON_BUTTON_SIZE))
            .h(px(ICON_BUTTON_SIZE))
            .flex()
            .items_center()
            .justify_center(),
        group.clone(),
        px(7.),
        ui,
    )
    .child(
        svg()
            .size(px(13.))
            .flex_none()
            .path(icon)
            .text_color(resting)
            .group_hover(group, |style| style.text_color(hovered)),
    )
}

fn copy_button(
    id: SharedString,
    label: &'static str,
    ui: crate::theme::UiColors,
) -> crate::ui_primitives::AnimatedHover {
    icon_button(
        id,
        "icons/settings/copy-01.svg",
        with_alpha(ui.text, 0.8),
        ui.text,
        ui,
    )
    .accessible_control(gpui::accesskit::Role::Button, label)
}

fn agent_logo(agent: TerminalAgent, size: f32, ui: crate::theme::UiColors) -> AnyElement {
    render_logo(
        agent.icon_path(),
        agent.icon_multicolor(),
        px(size),
        agent.accent().map(|c| rgb(c).into()).unwrap_or(ui.text),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_agent_lists_hold_exactly_this_platforms_runtimes() {
        let listed: std::collections::HashSet<TerminalAgent> =
            installed_agents().chain(missing_agents()).collect();
        let expected: std::collections::HashSet<TerminalAgent> =
            TerminalAgent::for_this_platform().collect();
        assert_eq!(listed, expected);
        assert_eq!(listed.contains(&TerminalAgent::Fx), !cfg!(windows));
    }

    struct AgentControlsHarness;

    impl gpui::Render for AgentControlsHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let ui = crate::theme::ui_colors_with(&(crate::theme::THEMES[0].1)());
            let agents: Vec<TerminalAgent> = TerminalAgent::for_this_platform().collect();
            let mut column = div()
                .flex()
                .flex_col()
                .child(copy_button("copy-command".into(), "Copy command", ui))
                .child(missing_agents_strip(&agents, true, ui));
            for agent in &agents {
                column = column
                    .child(agent_visibility_switch(*agent, true, ui))
                    .child(agent_visibility_switch(*agent, false, ui));
            }
            column
        }
    }

    #[gpui::test]
    fn the_agents_page_controls_draw_without_a_second_tooltip(cx: &mut gpui::TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, _| AgentControlsHarness);
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    #[test]
    fn an_agent_row_names_its_binary_version_profiles_and_hidden_state() {
        let agent = TerminalAgent::ClaudeCode;
        let binary = agent.binary();
        assert_eq!(agent_meta_label(agent, None, 0, true), binary);
        assert_eq!(
            agent_meta_label(agent, Some("2.0.14"), 1, true),
            format!("{binary} 2.0.14 · 1 profile")
        );
        assert_eq!(
            agent_meta_label(agent, Some("2.0.14"), 3, false),
            format!("{binary} 2.0.14 · 3 profiles · Hidden from the picker")
        );
    }

    #[test]
    fn a_profile_belongs_to_the_agent_its_tag_names() {
        let entry = |agent: &str| AgentProfileConfig {
            name: "Work".to_string(),
            agent: agent.to_string(),
            env: BTreeMap::new(),
            args: Vec::new(),
        };
        let claude = TerminalAgent::ClaudeCode;
        assert_eq!(profile_base(&entry(claude.tag())), Some(claude));
        assert_eq!(
            profile_base(&entry(&format!(" {} ", claude.tag()))),
            Some(claude)
        );
        assert_eq!(profile_base(&entry("not-an-agent")), None);
    }

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

    #[test]
    fn validate_env_values_rejects_a_name_the_environment_does_not_define() {
        let known = BTreeMap::from([("A".to_string(), "$PATH".to_string())]);
        assert!(validate_env_values(&known, None).is_ok());
        let literal = BTreeMap::from([("A".to_string(), "~/.claude-perso".to_string())]);
        assert!(validate_env_values(&literal, None).is_ok());
        let unknown = BTreeMap::from([(
            "CLAUDE_CONFIG_DIR".to_string(),
            "$PANEFLOW_NOT_A_REAL_VARIABLE/x".to_string(),
        )]);
        let error = validate_env_values(&unknown, None).unwrap_err();
        assert!(error.contains("CLAUDE_CONFIG_DIR"), "{error}");
        assert!(error.contains("PANEFLOW_NOT_A_REAL_VARIABLE"), "{error}");
    }
}
