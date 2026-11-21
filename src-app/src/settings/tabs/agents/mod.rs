mod accounts;
mod dialogs;
mod kit;
mod sign_in;

pub(crate) use sign_in::{AgentSignIn, NewAccount};

use std::collections::BTreeMap;

use gpui::{
    AnyElement, ClickEvent, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::*, px, svg,
};
use paneflow_config::schema::AgentProfileConfig;

use paneflow_agent_config::usage::accounts::UsageAgent;
use paneflow_agent_config::usage::model::UsageLevel;

use crate::PaneFlowApp;
use crate::agent_launcher::{AgentProfile, TerminalAgent};
use crate::agent_updates::{self, AgentUpdateState, UpdateMethod, UpdatePhase};
use crate::app::usage::model::AgentAccountRow;
use crate::settings::components::{deferred_select_menu, select_menu, select_option};
use crate::settings::search::{self, Block, SearchCard};
use crate::ui_primitives::{TooltipDelayExt, text_tooltip};
use crate::widgets::text_input::TextInput;

use kit::{Ink, MeterTone, Size, Text, Tone};

const INPUT_HEIGHT: f32 = 28.;
const MENU_WIDTH: f32 = 232.;
const MENU_GLYPH: f32 = 16.;
const PAGE_BOTTOM_SPACE: f32 = 180.;
const AGENTS_INTRO: &str = "The coding agents on your PATH, their accounts and launch presets.";
const MORE_AGENTS_HINT: &str = "Install one and it appears above, ready to launch.";
const OTHER_PROFILES_DESCRIPTION: &str =
    "Their agent is not installed here, or Paneflow cannot read them.";
const AGENTS_LIST_KEYWORDS: &[&str] = &[
    "Agents",
    "Profiles",
    "Launch presets",
    "Accounts",
    "Updates",
    "Update with",
    "Update command",
    "Location",
    "Hooks",
    "MCP",
];
const ADD_ICON: &str = "icons/settings/add-01.svg";
const SETTINGS_ICON: &str = "icons/settings/settings-02.svg";
const ACTIONS_ICON: &str = "icons/dock/more-horizontal.svg";
const EDIT_ICON: &str = "icons/settings/pencil-edit-02.svg";
const DELETE_ICON: &str = "icons/settings/delete-02.svg";

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

pub(crate) struct AgentAccountNamer {
    pub(crate) agent: TerminalAgent,
    pub(crate) space: Option<paneflow_config::schema::SpaceId>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentMenuTarget {
    Profile(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AgentProfileMenu {
    pub(crate) target: AgentMenuTarget,
    pub(crate) delete_armed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusTone {
    Quiet,
    Danger,
}

impl PaneFlowApp {
    pub(crate) fn render_agents_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let ink = Ink::of(ui);
        let installed: Vec<TerminalAgent> = installed_agents().collect();
        let missing: Vec<TerminalAgent> = missing_agents().collect();
        let mut page = div().flex().flex_col();
        let mut first = true;
        if !installed.is_empty() {
            let mut cards = div().flex().flex_col().w_full().gap(px(kit::CARD_GAP));
            let mut keywords: Vec<&str> = AGENTS_LIST_KEYWORDS.to_vec();
            for agent in &installed {
                keywords.push(agent.display_name());
                cards = cards.child(self.render_agent_card(*agent, ink, cx));
            }
            page = page.child(
                Block::new("Agents")
                    .top_gap(section_gap(&mut first))
                    .region("agents-list", &keywords, cards)
                    .finish(),
            );
        }
        if !missing.is_empty() {
            page = page.child(
                Block::new("More agents")
                    .top_gap(section_gap(&mut first))
                    .child(kit::section(
                        ink,
                        "More agents",
                        Some(MORE_AGENTS_HINT),
                        kit::separated(ink, missing.iter().map(|agent| install_row(*agent, ink))),
                    ))
                    .finish(),
            );
        }
        if let Some(orphans) = self.render_other_profiles_section(ink, cx) {
            page = page.child(
                Block::new("Other launch presets")
                    .top_gap(section_gap(&mut first))
                    .child(orphans)
                    .finish(),
            );
        }
        page.child(self.render_agent_permissions_section(section_gap(&mut first), ink, cx))
            .child(div().h(px(PAGE_BOTTOM_SPACE)).flex_none())
            .into_any_element()
    }

    pub(crate) fn render_agents_heading(&self, title: Div, _cx: &mut Context<Self>) -> AnyElement {
        let ink = Ink::of(crate::theme::ui_colors());
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(kit::TEXT_INSET))
            .pb(px(kit::SECTION_GAP))
            .child(title)
            .child(kit::page_description(ink, AGENTS_INTRO))
            .into_any_element()
    }

    pub(crate) fn agent_settings_is_open(&self) -> bool {
        self.agent_settings_dialog
            .is_some_and(|agent| agent.is_installed())
    }

    fn open_agent_settings(
        &mut self,
        agent: TerminalAgent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agent_settings_dialog = Some(agent);
        self.agent_profile_editor = None;
        self.agent_profile_menu = None;
        self.agent_update_menu_open = None;
        self.agent_dialog_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn close_agent_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_settings_dialog = None;
        self.agent_update_menu_open = None;
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn close_agent_profile_menu(&mut self, cx: &mut Context<Self>) {
        if self.agent_profile_menu.take().is_some() {
            cx.notify();
        }
    }

    fn render_agent_card(
        &self,
        agent: TerminalAgent,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tag = agent.tag();
        let mut card = kit::group(ink).child(self.render_agent_head(agent, ink, cx));
        let tracked = usage_agent(agent);
        let accounts = tracked
            .map(|usage| self.agent_accounts(usage))
            .unwrap_or_default();
        for account in &accounts {
            let row = if self.signing_in(agent, &account.key) {
                self.render_sign_in_row(agent, Some(&account.title), ink, cx)
            } else {
                self.render_account_row(agent, account, ink, cx)
            };
            card = card.child(kit::rule(ink)).child(row);
        }
        let keys: Vec<&str> = accounts
            .iter()
            .map(|account| account.key.as_str())
            .collect();
        if self.pending_sign_in_without_row(agent, &keys) {
            card = card
                .child(kit::rule(ink))
                .child(self.render_sign_in_row(agent, None, ink, cx));
        }
        for (idx, entry) in self
            .cached_config
            .agent_profiles
            .iter()
            .enumerate()
            .filter(|(_, entry)| profile_base(entry) == Some(agent))
        {
            let shown_as_account = is_account_profile(entry, tracked)
                && accounts
                    .iter()
                    .any(|account| account.profile.as_deref() == Some(entry.name.as_str()));
            if shown_as_account {
                continue;
            }
            card = card
                .child(kit::rule(ink))
                .child(self.render_agent_profile_row(idx, entry, false, ink, cx));
        }
        if tracked.is_some() {
            card = card.child(kit::rule(ink)).child(
                kit::link_row(
                    SharedString::from(format!("agent-account-add-{tag}")),
                    ink,
                    Some(kit::disc(ink, ADD_ICON)),
                    Text::new("Add account"),
                    true,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_agent_account_namer(agent, None, window, cx);
                })),
            );
        }
        card.into_any_element()
    }

    fn render_account_row(
        &self,
        agent: TerminalAgent,
        account: &AgentAccountRow,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tag = agent.tag();
        let mut controls = div().flex().flex_row().items_center().gap(px(12.));
        if account.needs_sign_in {
            let dir = account.dir.clone();
            controls = controls.child(
                kit::button(
                    SharedString::from(format!("agent-account-sign-in-{tag}-{}", account.key)),
                    "Sign in",
                    Tone::Secondary,
                    Size::Small,
                    ink,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.start_agent_sign_in(agent, dir.clone(), cx);
                })),
            );
        }
        controls = controls.children(account_meters(tag, account, ink));
        kit::row(
            ink,
            Some(kit::avatar(&account.title)),
            account_text(account, ink),
            Some(controls.into_any_element()),
        )
        .into_any_element()
    }

    fn render_agent_head(
        &self,
        agent: TerminalAgent,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tag = agent.tag();
        let name = agent.display_name();
        let visible = agent.is_visible(&self.cached_config);
        let installed = agent.cached_version();
        let method = self.selected_update_method(agent);
        let state = self.agent_updates.get(&agent);
        let plan = state
            .and_then(|state| state.inspection.as_ref())
            .and_then(|inspection| inspection.plan(method));
        let newer = state.and_then(|state| state.newer_version(installed.as_deref()));
        let up_to_date = state.is_some_and(|state| state.is_up_to_date(installed.as_deref()));
        let running = state.is_some_and(AgentUpdateState::is_running);
        let phase = state.and_then(|state| state.phase.as_ref());
        let offers_update = plan.is_some_and(Result::is_ok) && !up_to_date && newer.is_some();

        let status = agent_status_line(installed.as_deref(), phase, newer).map(|(text, tone)| {
            let color = match tone {
                StatusTone::Quiet => ink.tertiary,
                StatusTone::Danger => ink.danger,
            };
            kit::line_in(color, text).into_any_element()
        });
        let sessions = self.open_agent_sessions(agent);
        let sessions_note = (sessions > 0 && offers_update && !running)
            .then(|| kit::line(ink, sessions_note(sessions)).into_any_element());

        let mut controls = div().flex().flex_row().items_center().gap(px(12.));
        if offers_update || running {
            controls = controls.child(
                kit::button_with(
                    SharedString::from(format!("agent-update-{tag}")),
                    "Update",
                    Tone::Secondary,
                    Size::Small,
                    running,
                    true,
                    ink,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.start_agent_update(agent, cx);
                })),
            );
        }
        let key = agent.visibility_config_key();
        controls = controls
            .child(
                kit::icon_button(
                    SharedString::from(format!("agent-settings-{tag}")),
                    format!("{name} settings"),
                    SETTINGS_ICON,
                    ink,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_agent_settings(agent, window, cx);
                })),
            )
            .child(
                kit::switch(
                    SharedString::from(format!("agent-visible-{tag}")),
                    format!("Show {name} in the agent picker"),
                    visible,
                    ink,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.persist_setting(false, key, serde_json::Value::Bool(!visible), cx);
                })),
            );

        kit::row(
            ink,
            Some(kit::mark_tile(agent, kit::LEAD, ink)),
            Text::new(name).maybe_line(status).maybe_line(sessions_note),
            Some(controls.into_any_element()),
        )
        .into_any_element()
    }

    fn render_other_profiles_section(&self, ink: Ink, cx: &mut Context<Self>) -> Option<Div> {
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
        let rows: Vec<AnyElement> = orphans
            .into_iter()
            .map(|(idx, entry)| self.render_agent_profile_row(idx, entry, true, ink, cx))
            .collect();
        Some(kit::section(
            ink,
            "Other launch presets",
            Some(OTHER_PROFILES_DESCRIPTION),
            kit::separated(ink, rows),
        ))
    }

    fn render_agent_profile_row(
        &self,
        idx: usize,
        entry: &AgentProfileConfig,
        show_agent: bool,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut text = Text::new(entry.name.clone());
        match AgentProfile::from_config(entry) {
            Ok(profile) => {
                if show_agent {
                    text = text.line(kit::line(
                        ink,
                        if profile.agent.is_installed() {
                            profile.agent.display_name().to_string()
                        } else {
                            format!("{} is not installed", profile.agent.display_name())
                        },
                    ));
                }
                text = text.line(kit::mono_line(
                    ink,
                    profile_summary(&profile.env, &profile.args, profile.agent.binary()),
                ));
            }
            Err(reason) => {
                text = text.line(kit::line_in(
                    ink.danger,
                    format!("Cannot be read: {reason}"),
                ));
            }
        }
        let fallback = profile_base(entry).unwrap_or(TerminalAgent::ClaudeCode);
        kit::row(
            ink,
            Some(kit::avatar(&entry.name)),
            text,
            Some(self.render_profile_actions(idx, fallback, show_agent, ink, cx)),
        )
        .into_any_element()
    }

    fn render_profile_actions(
        &self,
        idx: usize,
        agent: TerminalAgent,
        pick_agent: bool,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let menu_state = self
            .agent_profile_menu
            .filter(|menu| menu.target == AgentMenuTarget::Profile(idx));
        let trigger = kit::icon_button(
            SharedString::from(format!("agent-profile-actions-{idx}")),
            "Launch preset actions",
            ACTIONS_ICON,
            ink,
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.agent_profile_menu = match this.agent_profile_menu {
                Some(menu) if menu.target == AgentMenuTarget::Profile(idx) => None,
                _ => Some(AgentProfileMenu {
                    target: AgentMenuTarget::Profile(idx),
                    delete_armed: false,
                }),
            };
            cx.notify();
        }));
        let mut anchor = div().relative().flex_none().child(trigger);
        if let Some(state) = menu_state {
            let menu_row = |id: String, icon: &'static str, label: &'static str, color| {
                select_option(SharedString::from(id), false, ui)
                    .child(
                        svg()
                            .size(px(MENU_GLYPH))
                            .flex_none()
                            .path(icon)
                            .text_color(color),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(color)
                            .child(label),
                    )
            };
            let delete_label = if state.delete_armed {
                "Confirm delete"
            } else {
                "Delete launch preset"
            };
            let menu = select_menu(SharedString::from(format!("agent-profile-menu-{idx}")), ui)
                .min_w(px(MENU_WIDTH))
                .on_mouse_down_out(cx.listener(|this, _, _w, cx| {
                    this.close_agent_profile_menu(cx);
                }))
                .option(
                    menu_row(
                        format!("agent-profile-edit-{idx}"),
                        EDIT_ICON,
                        "Edit launch preset",
                        ink.primary,
                    ),
                    cx.listener(move |this, _: &(), window, cx| {
                        this.agent_profile_menu = None;
                        this.open_agent_profile_editor(Some(idx), agent, pick_agent, window, cx);
                    }),
                )
                .option(
                    menu_row(
                        format!("agent-profile-delete-{idx}"),
                        DELETE_ICON,
                        delete_label,
                        ink.danger,
                    ),
                    cx.listener(
                        move |this, _: &(), window, cx| match this.agent_profile_menu {
                            Some(menu)
                                if menu.target == AgentMenuTarget::Profile(idx)
                                    && menu.delete_armed =>
                            {
                                this.agent_profile_menu = None;
                                this.delete_agent_profile(idx, window, cx);
                            }
                            _ => {
                                this.agent_profile_menu = Some(AgentProfileMenu {
                                    target: AgentMenuTarget::Profile(idx),
                                    delete_armed: true,
                                });
                                cx.notify();
                            }
                        },
                    ),
                );
            anchor = anchor.child(deferred_select_menu(menu));
        }
        anchor.into_any_element()
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
        top_gap: f32,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let bypass = self
            .cached_config
            .claude_code_bypass_permissions
            .unwrap_or(false);
        let unrestricted = self.cached_config.ai_unrestricted_enabled();
        let fence = self.cached_config.ai_injection_fence_enabled();

        let mut card = SearchCard::new(ui)
            .row(
                &search::CLAUDE_FULL_ACCESS,
                self.permission_row(
                    "row-claude-bypass",
                    &search::CLAUDE_FULL_ACCESS,
                    bypass,
                    "claude_code_bypass_permissions",
                    ink,
                    cx,
                ),
            )
            .row(
                &search::AI_FREE_ACCESS,
                self.permission_row(
                    "row-ai-unrestricted",
                    &search::AI_FREE_ACCESS,
                    unrestricted,
                    "ai_unrestricted",
                    ink,
                    cx,
                ),
            );
        if unrestricted {
            card = card.row(
                &search::INJECTION_FENCE,
                self.permission_row(
                    "row-ai-injection-fence",
                    &search::INJECTION_FENCE,
                    fence,
                    "ai_injection_fence",
                    ink,
                    cx,
                ),
            );
            if !fence {
                card = card.fixed(kit::row(
                    ink,
                    None,
                    Text::new("Fence off").danger(ink).line(kit::line(
                        ink,
                        "A malicious pane can silently redirect your agent.",
                    )),
                    None,
                ));
            }
        }
        let (matched, rows) = card.into_rows();

        Block::new("Permissions")
            .top_gap(top_gap)
            .found(matched)
            .child(kit::section(
                ink,
                "Permissions",
                None,
                search_group(ink, rows),
            ))
            .finish()
    }

    fn permission_row(
        &self,
        id: &'static str,
        copy: &search::SettingCopy,
        on: bool,
        config_key: &'static str,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> Div {
        kit::row(
            ink,
            None,
            Text::new(copy.title).line(kit::line(ink, copy.description)),
            Some(
                kit::switch(id, copy.title, on, ink)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.persist_setting(false, config_key, serde_json::Value::Bool(!on), cx);
                    }))
                    .into_any_element(),
            ),
        )
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agent_settings_dialog = None;
        self.agent_update_menu_open = None;
        self.agent_profile_menu = None;
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
        let focus = self.agent_profile_name_input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn close_agent_profile_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agent_profile_editor = None;
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    fn add_agent_profile_env_row(&mut self, cx: &mut Context<Self>) {
        let row = new_env_row("", "", cx);
        if let Some(editor) = self.agent_profile_editor.as_mut() {
            editor.env_rows.push(row);
        }
        cx.notify();
    }

    fn save_agent_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.settings_focus.focus(window, cx);
        self.persist_agent_profiles(profiles, cx);
    }

    fn delete_agent_profile(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        if idx >= self.cached_config.agent_profiles.len() {
            return;
        }
        let mut profiles = self.cached_config.agent_profiles.clone();
        let removed = profiles.remove(idx);
        self.forget_default_account_of(&removed, cx);
        if self
            .agent_profile_editor
            .as_ref()
            .is_some_and(|editor| editor.index == Some(idx))
        {
            self.agent_profile_editor = None;
        }
        self.settings_focus.focus(window, cx);
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

fn usage_agent(agent: TerminalAgent) -> Option<UsageAgent> {
    UsageAgent::ALL
        .into_iter()
        .find(|candidate| paneflow_serve::usage::probe::binary_name(*candidate) == agent.binary())
}

fn meter_tone(level: UsageLevel) -> MeterTone {
    match level {
        UsageLevel::Normal => MeterTone::Normal,
        UsageLevel::Warn => MeterTone::Warning,
        UsageLevel::Critical => MeterTone::Danger,
    }
}

fn account_detail(account: &AgentAccountRow) -> String {
    match &account.source {
        Some(source) => format!("{} · {source}", account.detail),
        None => account.detail.clone(),
    }
}

fn account_text(account: &AgentAccountRow, ink: Ink) -> Text {
    let text = Text::new(account.title.clone()).line(kit::line(ink, account_detail(account)));
    if account.in_use {
        text.badge(kit::badge(ink, "In use"))
    } else {
        text
    }
}

fn account_meters(tag: &str, account: &AgentAccountRow, ink: Ink) -> Option<AnyElement> {
    if account.meters.is_empty() {
        return None;
    }
    let meters = kit::meters(account.meters.iter().map(|meter| {
        kit::meter(
            ink,
            meter.label,
            meter.fraction,
            meter.percent.clone(),
            meter_tone(meter.level),
        )
    }));
    Some(match &account.resets {
        Some(resets) => meters
            .id(SharedString::from(format!(
                "agent-account-{tag}-{}",
                account.key
            )))
            .delayed_tooltip(text_tooltip(resets.clone()))
            .into_any_element(),
        None => meters.into_any_element(),
    })
}

fn is_account_profile(entry: &AgentProfileConfig, agent: Option<UsageAgent>) -> bool {
    agent.is_some_and(|agent| {
        entry.args.is_empty()
            && entry.env.len() == 1
            && entry
                .env
                .keys()
                .all(|key| key.eq_ignore_ascii_case(agent.config_dir_key()))
    })
}

fn section_gap(first: &mut bool) -> f32 {
    if std::mem::take(first) {
        0.
    } else {
        kit::SECTION_GAP
    }
}

fn install_row(agent: TerminalAgent, ink: Ink) -> AnyElement {
    kit::row(
        ink,
        Some(kit::mark_tile(agent, kit::LEAD, ink)),
        Text::new(agent.display_name()).line(kit::line(
            ink,
            format!(
                "Install {} yourself to use it. Paneflow looks for {} on your PATH.",
                agent.display_name(),
                agent.binary()
            ),
        )),
        None,
    )
    .into_any_element()
}

fn search_group(ink: Ink, rows: Vec<(f32, AnyElement)>) -> Div {
    let mut card = kit::group(ink);
    let mut previous: Option<f32> = None;
    for (amount, row) in rows {
        if let Some(above) = previous {
            let visible = above.min(amount);
            card = card.child(
                div()
                    .w_full()
                    .h(px(visible))
                    .overflow_hidden()
                    .opacity(visible)
                    .child(kit::rule(ink)),
            );
        }
        card = card.child(row);
        previous = Some(amount);
    }
    card
}

fn version_label(version: &str) -> String {
    format!("v{}", version.trim_start_matches(['v', 'V']))
}

fn agent_status_line(
    installed: Option<&str>,
    phase: Option<&UpdatePhase>,
    newer: Option<&str>,
) -> Option<(String, StatusTone)> {
    let quiet = |text: String| Some((text, StatusTone::Quiet));
    match phase {
        Some(UpdatePhase::Running { progress }) => {
            quiet(progress.clone().unwrap_or_else(|| "Updating…".to_string()))
        }
        Some(UpdatePhase::Failed { message }) => {
            Some((format!("Update failed: {message}"), StatusTone::Danger))
        }
        Some(UpdatePhase::Updated { version }) => quiet(match version.as_deref().or(installed) {
            Some(version) => format!("{} · Updated", version_label(version)),
            None => "Updated".to_string(),
        }),
        Some(UpdatePhase::Unchanged) => quiet(match installed {
            Some(version) => format!("{} · Up to date", version_label(version)),
            None => "Up to date".to_string(),
        }),
        None => match (installed, newer) {
            (Some(version), Some(newer)) => quiet(format!(
                "{} · {} available",
                version_label(version),
                version_label(newer)
            )),
            (Some(version), None) => quiet(version_label(version)),
            (None, Some(newer)) => quiet(format!("{} available", version_label(newer))),
            (None, None) => None,
        },
    }
}

fn sessions_note(sessions: usize) -> String {
    if sessions == 1 {
        "1 open session keeps the current version until it restarts.".to_string()
    } else {
        format!("{sessions} open sessions keep the current version until they restart.")
    }
}

fn profile_summary(env: &BTreeMap<String, String>, args: &[String], binary: &str) -> String {
    let mut parts: Vec<String> = env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    if !args.is_empty() {
        parts.push(args.join(" "));
    }
    if parts.is_empty() {
        format!("Launches {binary} as is")
    } else {
        parts.join(" · ")
    }
}

pub(crate) fn input_box(
    input: Entity<TextInput>,
    mono: bool,
    ui: crate::theme::UiColors,
) -> impl IntoElement {
    input_box_sized(input, mono, INPUT_HEIGHT, ui)
}

pub(crate) fn input_box_sized(
    input: Entity<TextInput>,
    mono: bool,
    height: f32,
    ui: crate::theme::UiColors,
) -> Div {
    crate::settings::components::input_frame(
        div()
            .w_full()
            .h(px(height))
            .flex()
            .items_center()
            .px(px(10.)),
        &input,
        false,
        ui,
    )
    .when(mono, |d| d.font_family(kit::mono_family()))
    .text_size(px(if mono { kit::SIZE_SM } else { kit::SIZE_MD }))
    .text_color(ui.text)
    .child(input)
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
            let ink = Ink::of(ui);
            let mut column = div()
                .flex()
                .flex_col()
                .child(kit::icon_button(
                    "copy-command",
                    "Copy command",
                    ADD_ICON,
                    ink,
                ))
                .child(kit::button(
                    "update",
                    "Update",
                    Tone::Secondary,
                    Size::Small,
                    ink,
                ))
                .child(kit::link_row(
                    "add-profile",
                    ink,
                    Some(kit::disc(ink, ADD_ICON)),
                    Text::new("Add profile"),
                    true,
                ));
            let account = AgentAccountRow {
                key: "/home/a/.claude".to_string(),
                dir: std::path::PathBuf::from("/home/a/.claude"),
                profile: None,
                is_default: true,
                needs_sign_in: false,
                api_billing: false,
                title: "me@example.com".to_string(),
                detail: "Claude Max".to_string(),
                source: Some("Default".to_string()),
                in_use: true,
                meters: vec![crate::app::usage::model::AccountMeter {
                    label: "Week",
                    fraction: 0.97,
                    level: UsageLevel::Critical,
                    percent: "97%".to_string(),
                }],
                resets: Some("Week resets in 2 days".to_string()),
            };
            column = column
                .child(kit::row(
                    ink,
                    Some(kit::avatar(&account.title)),
                    account_text(&account, ink),
                    account_meters("claude", &account, ink),
                ))
                .child(kit::button(
                    "agent-account-sign-in",
                    "Sign in",
                    Tone::Secondary,
                    Size::Small,
                    ink,
                ));
            for agent in TerminalAgent::for_this_platform() {
                column = column
                    .child(install_row(agent, ink))
                    .child(kit::switch(
                        SharedString::from(format!("on-{}", agent.tag())),
                        "Show",
                        true,
                        ink,
                    ))
                    .child(kit::switch(
                        SharedString::from(format!("off-{}", agent.tag())),
                        "Show",
                        false,
                        ink,
                    ));
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
    fn an_agent_card_says_its_version_and_the_one_it_can_update_to() {
        assert_eq!(agent_status_line(None, None, None), None);
        assert_eq!(
            agent_status_line(Some("2.0.14"), None, None),
            Some(("v2.0.14".to_string(), StatusTone::Quiet))
        );
        assert_eq!(
            agent_status_line(Some("v2.0.14"), None, Some("2.1.0")),
            Some(("v2.0.14 · v2.1.0 available".to_string(), StatusTone::Quiet))
        );
        let failed = UpdatePhase::Failed {
            message: "npm exited 1".to_string(),
        };
        assert_eq!(
            agent_status_line(Some("2.0.14"), Some(&failed), Some("2.1.0")),
            Some((
                "Update failed: npm exited 1".to_string(),
                StatusTone::Danger
            ))
        );
        let updated = UpdatePhase::Updated {
            version: Some("2.1.0".to_string()),
        };
        assert_eq!(
            agent_status_line(Some("2.0.14"), Some(&updated), None),
            Some(("v2.1.0 · Updated".to_string(), StatusTone::Quiet))
        );
    }

    #[test]
    fn a_profile_row_summarizes_its_variables_and_arguments() {
        let env = BTreeMap::from([("CLAUDE_CONFIG_DIR".to_string(), "~/.work".to_string())]);
        let args = vec!["--model".to_string(), "opus".to_string()];
        assert_eq!(
            profile_summary(&env, &args, "claude"),
            "CLAUDE_CONFIG_DIR=~/.work · --model opus"
        );
        assert_eq!(
            profile_summary(&BTreeMap::new(), &[], "claude"),
            "Launches claude as is"
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
