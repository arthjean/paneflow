use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, SharedString, Styled, div, prelude::*, px, svg,
};
use paneflow_agent_config::usage::accounts::{UsageAgent, account_dir_key, canonical_account_dir};
use paneflow_config::schema::{SpaceConfig, SpaceId};
use paneflow_mcp_install::IntegrationState;

use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;
use crate::app::usage::model::AgentAccountRow;
use crate::settings::components::{
    ModalKey, deferred_select_menu, modal_key, select_action, select_chevron, select_menu,
    select_option, select_trigger, with_alpha,
};
use crate::settings::kit::{self, Ink, Size, Text, Tone};
use crate::settings::tabs::agents::input_box;
use crate::settings::tabs::spaces::{HintTone, hint_line};
use crate::ui_primitives::{AccessibleControlExt, LABEL_SM};

const SELECT_WIDTH: f32 = 200.;
const CODE_ROW_PADDING_BOTTOM: f32 = 12.;
const ACCOUNTS_HINT: &str = "The account each agent signs in with in this space.";
const ADD_ICON: &str = "icons/settings/add-01.svg";
const ADD_ACCOUNT_LABEL: &str = "Add account…";

pub(crate) const ACCOUNT_AGENTS: [(UsageAgent, TerminalAgent); 3] = [
    (UsageAgent::ClaudeCode, TerminalAgent::ClaudeCode),
    (UsageAgent::Codex, TerminalAgent::Codex),
    (UsageAgent::Grok, TerminalAgent::Grok),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SpaceAccount {
    Default,
    Folder { key: String, dir: PathBuf },
    Unusable { value: String },
}

fn usage_of(agent: TerminalAgent) -> Option<UsageAgent> {
    ACCOUNT_AGENTS
        .iter()
        .find(|(_, candidate)| *candidate == agent)
        .map(|(usage, _)| *usage)
}

pub(crate) fn same_key(defined: &str, wanted: &str) -> bool {
    crate::space_env::platform_key(defined) == crate::space_env::platform_key(wanted)
}

pub(crate) fn is_account_key(key: &str) -> bool {
    UsageAgent::ALL
        .into_iter()
        .any(|agent| same_key(key, agent.config_dir_key()))
}

pub(crate) fn space_account(
    env: &BTreeMap<String, String>,
    agent: UsageAgent,
    home: Option<&Path>,
) -> SpaceAccount {
    let Some(value) = env
        .iter()
        .find(|(key, _)| same_key(key, agent.config_dir_key()))
        .map(|(_, value)| value)
    else {
        return SpaceAccount::Default;
    };
    match crate::env_expand::expand_with_process_env(value, home)
        .ok()
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
    {
        Some(dir) => {
            let dir = canonical_account_dir(&dir);
            SpaceAccount::Folder {
                key: account_dir_key(&dir),
                dir,
            }
        }
        None => SpaceAccount::Unusable {
            value: value.clone(),
        },
    }
}

pub(crate) fn space_account_value(
    env: &BTreeMap<String, String>,
    agent: UsageAgent,
) -> Option<String> {
    env.iter()
        .find(|(key, _)| same_key(key, agent.config_dir_key()))
        .map(|(_, value)| value.clone())
}

pub(crate) fn env_with_account(
    env: &BTreeMap<String, String>,
    agent: UsageAgent,
    value: Option<String>,
) -> BTreeMap<String, String> {
    let mut next: BTreeMap<String, String> = env
        .iter()
        .filter(|(key, _)| !same_key(key, agent.config_dir_key()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if let Some(value) = value {
        next.insert(agent.config_dir_key().to_string(), value);
    }
    next
}

pub(crate) fn home_relative(dir: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| dir.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => {
            let parts: Vec<String> = rest
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            format!("~/{}", parts.join("/"))
        }
        None => dir.display().to_string(),
    }
}

pub(crate) fn account_label(row: &AgentAccountRow, home: Option<&Path>) -> String {
    if row.title.contains('@') {
        row.title.clone()
    } else {
        home_relative(&row.dir, home)
    }
}

pub(crate) fn space_view_summary(row: &AgentAccountRow, api_billing: bool) -> String {
    if api_billing {
        API_BILLING.to_string()
    } else if row.api_billing {
        if row.title.contains('@') {
            "Signed in".to_string()
        } else {
            "Not signed in".to_string()
        }
    } else {
        account_summary(row)
    }
}

pub(crate) fn account_summary(row: &AgentAccountRow) -> String {
    let mut parts = vec![row.detail.clone()];
    parts.extend(
        row.meters
            .iter()
            .map(|meter| format!("{} {}", meter.label, meter.percent)),
    );
    parts.join(" · ")
}

pub(crate) fn pickable_accounts(
    rows: &[AgentAccountRow],
) -> impl Iterator<Item = &AgentAccountRow> {
    rows.iter().filter(|row| !row.is_default)
}

pub(crate) fn default_account<'a>(
    rows: &'a [AgentAccountRow],
    agent: UsageAgent,
    defaults: &[(UsageAgent, String)],
) -> Option<&'a AgentAccountRow> {
    match defaults.iter().find(|(candidate, _)| *candidate == agent) {
        Some((_, key)) => rows.iter().find(|row| &row.key == key),
        None => rows.iter().find(|row| row.is_default),
    }
}

pub(crate) fn space_bills_through_api_key(
    env: &BTreeMap<String, String>,
    agent: UsageAgent,
    inherited: impl Fn(&str) -> bool,
) -> bool {
    agent.billing_keys().iter().any(|wanted| {
        match env.iter().find(|(key, _)| same_key(key, wanted)) {
            Some((_, value)) => !value.trim().is_empty(),
            None => inherited(wanted),
        }
    })
}

pub(crate) fn other_variable_count(env: &BTreeMap<String, String>) -> usize {
    env.keys().filter(|key| !is_account_key(key)).count()
}

struct AccountState {
    text: String,
    tone: HintTone,
    sign_in: Option<PathBuf>,
}

impl PaneFlowApp {
    pub(crate) fn space_account_labels(&self, space: &SpaceConfig) -> Vec<String> {
        let home = dirs::home_dir();
        ACCOUNT_AGENTS
            .iter()
            .filter_map(
                |(usage, _)| match space_account(&space.env, *usage, home.as_deref()) {
                    SpaceAccount::Default => None,
                    SpaceAccount::Folder { key, dir } => Some(
                        self.agent_accounts(*usage)
                            .iter()
                            .find(|row| row.key == key)
                            .map(|row| account_label(row, home.as_deref()))
                            .unwrap_or_else(|| home_relative(&dir, home.as_deref())),
                    ),
                    SpaceAccount::Unusable { value } => Some(value),
                },
            )
            .collect()
    }

    pub(crate) fn set_space_account(
        &mut self,
        space: &SpaceId,
        agent: TerminalAgent,
        dir: Option<&Path>,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(usage) = usage_of(agent) else {
            return;
        };
        let home = dirs::home_dir();
        let mut spaces = self.cached_config.spaces.clone();
        let Some(entry) = spaces.iter_mut().find(|entry| &entry.id == space) else {
            return;
        };
        let value = dir.map(|dir| home_relative(dir, home.as_deref()));
        let next = env_with_account(&entry.env, usage, value);
        self.space_account_menu = None;
        if next == entry.env {
            cx.notify();
            return;
        }
        entry.env = next;
        let space_name = entry.name.clone();
        self.persist_spaces(spaces, cx);
        self.mirror_integrations_into_space(space, cx);
        self.refresh_usage_inventory(true, cx);
        self.check_agent_accounts(cx);
        let agent_name = agent.display_name();
        let message = match dir {
            Some(_) => format!(
                "New {agent_name} panes in {space_name} use {label}. Open panes keep their account."
            ),
            None => format!(
                "New {agent_name} panes in {space_name} use your default account. Open panes keep their account."
            ),
        };
        self.show_toast(message, cx);
    }

    pub(crate) fn restore_space_account(
        &mut self,
        space: &SpaceId,
        agent: UsageAgent,
        key: &str,
        previous: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let home = dirs::home_dir();
        let mut spaces = self.cached_config.spaces.clone();
        let Some(entry) = spaces.iter_mut().find(|entry| &entry.id == space) else {
            return;
        };
        let still_new = matches!(
            space_account(&entry.env, agent, home.as_deref()),
            SpaceAccount::Folder { key: held, .. } if held == key
        );
        if !still_new {
            return;
        }
        entry.env = env_with_account(&entry.env, agent, previous);
        self.persist_spaces(spaces, cx);
        self.mirror_integrations_into_space(space, cx);
        self.refresh_usage_inventory(true, cx);
    }

    fn global_integration_installed(&self, agent: TerminalAgent) -> bool {
        let slug = agent.runtime().slug;
        self.integration_status.as_ref().is_some_and(|statuses| {
            statuses.iter().any(|status| {
                status.slug == slug && matches!(status.state, IntegrationState::Installed)
            })
        })
    }

    pub(crate) fn render_space_accounts(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ink = Ink::of(ui);
        let home = dirs::home_dir();
        let shown: Vec<(UsageAgent, TerminalAgent)> = ACCOUNT_AGENTS
            .into_iter()
            .filter(|(usage, agent)| {
                agent.is_installed()
                    || space_account(&space.env, *usage, home.as_deref()) != SpaceAccount::Default
            })
            .collect();
        if shown.is_empty() {
            return None;
        }
        let rows: Vec<AnyElement> = shown
            .into_iter()
            .map(|(usage, agent)| self.render_space_account_row(space, usage, agent, ui, ink, cx))
            .collect();
        Some(
            kit::section(
                ink,
                "Accounts",
                Some(ACCOUNTS_HINT),
                kit::separated(ink, rows),
            )
            .into_any_element(),
        )
    }

    fn render_space_account_row(
        &self,
        space: &SpaceConfig,
        usage: UsageAgent,
        agent: TerminalAgent,
        ui: crate::theme::UiColors,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let home = dirs::home_dir();
        let rows = self.agent_accounts(usage);
        let chosen = space_account(&space.env, usage, home.as_deref());
        let defaults = self.default_account_keys();
        let fallback = default_account(&rows, usage, &defaults);
        let progress = match &chosen {
            SpaceAccount::Folder { key, .. } => self.sign_in_progress(agent, key),
            _ => None,
        };
        let api_billing = space_bills_through_api_key(
            &space.env,
            usage,
            crate::space_env::inherited_from_paneflow,
        );
        let state = account_state(&chosen, &rows, fallback, api_billing, home.as_deref());
        let tag = agent.tag();
        let space_key = space.id.to_string();
        let mut controls = div().flex().flex_row().items_center().gap(px(8.));
        let mut text = Text::new(agent.display_name());
        match progress {
            Some(progress) => {
                text = text.line(hint_line(
                    ink,
                    if progress.linked {
                        "Finish signing in in your browser"
                    } else {
                        "Starting the sign-in…"
                    },
                    HintTone::Preview,
                ));
                if progress.linked {
                    controls = controls.child(
                        kit::button(
                            SharedString::from(format!("space-account-open-{space_key}-{tag}")),
                            "Open the page",
                            Tone::Secondary,
                            Size::Small,
                            ink,
                        )
                        .on_activate(cx.listener(
                            |this, _: &ClickEvent, _w, cx| {
                                this.reopen_sign_in_page(cx);
                            },
                        )),
                    );
                }
                controls = controls.child(
                    kit::button(
                        SharedString::from(format!("space-account-cancel-{space_key}-{tag}")),
                        "Cancel",
                        Tone::Ghost,
                        Size::Small,
                        ink,
                    )
                    .on_activate(cx.listener(
                        |this, _: &ClickEvent, _w, cx| {
                            this.cancel_agent_sign_in(cx);
                        },
                    )),
                );
            }
            None => {
                text = text.line(hint_line(ink, state.text.clone(), state.tone));
                if let Some(dir) = state.sign_in.clone() {
                    controls = controls.child(
                        kit::button(
                            SharedString::from(format!("space-account-sign-in-{space_key}-{tag}")),
                            "Sign in",
                            Tone::Secondary,
                            Size::Small,
                            ink,
                        )
                        .on_activate(cx.listener(
                            move |this, _: &ClickEvent, _w, cx| {
                                this.start_agent_sign_in(agent, dir.clone(), cx);
                            },
                        )),
                    );
                }
            }
        }
        if let Some((line, action)) = self.space_hooks_state(space, agent, ink, cx) {
            text = text.line(line);
            if let Some(action) = action {
                controls = controls.child(action);
            }
        }
        controls = controls.child(self.render_space_account_select(
            space,
            usage,
            agent,
            &rows,
            &chosen,
            fallback,
            api_billing,
            ui,
            cx,
        ));
        let row = kit::wide_row(
            ink,
            Some(kit::mark_tile(agent, kit::LEAD, ink)),
            text,
            Some(controls.into_any_element()),
        );
        let code_open = progress.is_some_and(|progress| progress.code_open);
        if !code_open {
            return row.into_any_element();
        }
        let typed = !self
            .agent_sign_in_code_input
            .read(cx)
            .value()
            .trim()
            .is_empty();
        div()
            .flex()
            .flex_col()
            .child(row)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(kit::COLUMN_INSET))
                    .pr(px(kit::ROW_INSET))
                    .pb(px(CODE_ROW_PADDING_BOTTOM))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "space-account-code-{space_key}-{tag}"
                            )))
                            .flex_1()
                            .min_w_0()
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _w, cx| {
                                if matches!(modal_key(event), Some(ModalKey::Confirm)) {
                                    this.submit_sign_in_code(cx);
                                    cx.stop_propagation();
                                }
                            }))
                            .child(input_box(self.agent_sign_in_code_input.clone(), true, ui)),
                    )
                    .when(typed, |line| {
                        line.child(
                            kit::button(
                                "space-account-code-send",
                                "Continue",
                                Tone::Primary,
                                Size::Small,
                                ink,
                            )
                            .on_activate(cx.listener(
                                |this, _: &ClickEvent, _w, cx| {
                                    this.submit_sign_in_code(cx);
                                },
                            )),
                        )
                    }),
            )
            .into_any_element()
    }

    fn space_hooks_state(
        &self,
        space: &SpaceConfig,
        agent: TerminalAgent,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> Option<(AnyElement, Option<AnyElement>)> {
        let slug = agent.runtime().slug;
        let status = self
            .space_integrations
            .status
            .get(&space.id)?
            .iter()
            .find(|status| status.slug == slug)?;
        let key = (space.id.clone(), slug.to_string());
        if self.space_integrations.busy.as_ref() == Some(&key) {
            return Some((
                hint_line(ink, "Adding Paneflow's hooks…", HintTone::Preview),
                None,
            ));
        }
        let error = self.space_integrations.errors.get(&key);
        let missing = status.state == IntegrationState::NotInstalled
            && self.global_integration_installed(agent);
        let text = match (error, missing) {
            (Some(error), _) => {
                format!("Paneflow could not add its hooks to this account: {error}")
            }
            (None, true) => "Paneflow's hooks are not in this account yet".to_string(),
            (None, false) => return None,
        };
        let id = space.id.clone();
        let action = kit::button(
            SharedString::from(format!("space-account-hooks-{}-{slug}", space.id)),
            if error.is_some() {
                "Retry"
            } else {
                "Add hooks"
            },
            Tone::Secondary,
            Size::Small,
            ink,
        )
        .on_activate(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.retry_space_integration(&id, slug, cx);
        }))
        .into_any_element();
        let tone = if error.is_some() {
            HintTone::Error
        } else {
            HintTone::Warning
        };
        Some((hint_line(ink, text, tone), Some(action)))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_space_account_select(
        &self,
        space: &SpaceConfig,
        usage: UsageAgent,
        agent: TerminalAgent,
        rows: &[AgentAccountRow],
        chosen: &SpaceAccount,
        fallback: Option<&AgentAccountRow>,
        api_billing: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let home = dirs::home_dir();
        let tag = agent.tag();
        let space_key = space.id.to_string();
        let open = self.space_account_menu.as_ref() == Some(&(space.id.clone(), usage));
        let chosen_key = match chosen {
            SpaceAccount::Folder { key, .. } => Some(key.as_str()),
            _ => None,
        };
        let current = match chosen {
            SpaceAccount::Default => "Default".to_string(),
            SpaceAccount::Folder { key, dir } => rows
                .iter()
                .find(|row| &row.key == key)
                .map(|row| account_label(row, home.as_deref()))
                .unwrap_or_else(|| home_relative(dir, home.as_deref())),
            SpaceAccount::Unusable { value } => value.clone(),
        };
        let menu_key = (space.id.clone(), usage);
        let mut trigger = select_trigger(
            SharedString::from(format!("space-account-{space_key}-{tag}")),
            format!("{} account", agent.display_name()),
            open,
            ui,
        )
        .w(px(SELECT_WIDTH))
        .min_w(px(0.))
        .on_press(cx.listener(move |this, _, _w, cx| {
            cx.stop_propagation();
            this.space_account_menu = if open { None } else { Some(menu_key.clone()) };
            cx.notify();
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(LABEL_SM)
                .text_color(ui.text)
                .child(current),
        )
        .child(select_chevron(ui));
        if !open {
            return trigger.into_any_element();
        }
        let option_row = |id: String, selected: bool, label: String, note: Option<String>| {
            select_option(SharedString::from(id), label.clone(), selected, ui)
                .cursor(CursorStyle::Arrow)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(170.))
                                .truncate()
                                .text_color(ui.text)
                                .child(label),
                        )
                        .when_some(note, |line, note| {
                            line.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(ui.muted)
                                    .child(note),
                            )
                        }),
                )
        };
        let space_id = space.id.clone();
        let mut menu = select_menu(
            SharedString::from(format!("space-account-menu-{space_key}-{tag}")),
            ui,
        )
        .min_w(px(SELECT_WIDTH + 60.))
        .on_mouse_down_out(cx.listener(|this, _, _w, cx| {
            this.space_account_menu = None;
            cx.notify();
        }));
        let id = space_id.clone();
        menu = menu.option(
            option_row(
                format!("space-account-default-{space_key}-{tag}"),
                *chosen == SpaceAccount::Default,
                "Default".to_string(),
                fallback.map(|row| account_label(row, home.as_deref())),
            ),
            cx.listener(move |this, _: &(), _w, cx| {
                this.set_space_account(&id, agent, None, "", cx);
            }),
        );
        let mut listed = false;
        for row in pickable_accounts(rows) {
            listed |= chosen_key == Some(row.key.as_str());
            let (id, dir, label) = (
                space_id.clone(),
                row.dir.clone(),
                account_label(row, home.as_deref()),
            );
            let shown = label.clone();
            menu = menu.option(
                option_row(
                    format!("space-account-{space_key}-{tag}-{}", row.key),
                    chosen_key == Some(row.key.as_str()),
                    shown,
                    Some(space_view_summary(row, api_billing)),
                ),
                cx.listener(move |this, _: &(), _w, cx| {
                    this.set_space_account(&id, agent, Some(&dir), &label, cx);
                }),
            );
        }
        if let SpaceAccount::Folder { dir, .. } = chosen
            && !listed
        {
            menu = menu.option(
                option_row(
                    format!("space-account-custom-{space_key}-{tag}"),
                    true,
                    home_relative(dir, home.as_deref()),
                    Some("Folder set for this space".to_string()),
                ),
                cx.listener(|this, _: &(), _w, cx| {
                    this.space_account_menu = None;
                    cx.notify();
                }),
            );
        }
        let id = space_id;
        menu = menu.option(
            select_action(
                SharedString::from(format!("space-account-add-{space_key}-{tag}")),
                ADD_ACCOUNT_LABEL,
                ui,
            )
            .cursor(CursorStyle::Arrow)
            .child(
                svg()
                    .size(px(14.))
                    .flex_none()
                    .path(ADD_ICON)
                    .text_color(with_alpha(ui.text, 0.8)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(ui.text)
                    .child(ADD_ACCOUNT_LABEL),
            ),
            cx.listener(move |this, _: &(), window, cx| {
                this.space_account_menu = None;
                this.open_agent_account_namer(agent, Some(id.clone()), window, cx);
            }),
        );
        trigger = trigger.child(deferred_select_menu(menu));
        trigger.into_any_element()
    }
}

const API_BILLING: &str = "Billed through an API key";

fn account_state(
    chosen: &SpaceAccount,
    rows: &[AgentAccountRow],
    fallback: Option<&AgentAccountRow>,
    api_billing: bool,
    home: Option<&Path>,
) -> AccountState {
    match chosen {
        SpaceAccount::Default => AccountState {
            text: match fallback {
                Some(row) => format!("Your default account, {}", account_label(row, home)),
                None => "Your default account".to_string(),
            },
            tone: HintTone::Preview,
            sign_in: None,
        },
        SpaceAccount::Unusable { value } => AccountState {
            text: format!("{value} is not a folder Paneflow can use. Pick an account."),
            tone: HintTone::Error,
            sign_in: None,
        },
        SpaceAccount::Folder { .. } if api_billing => AccountState {
            text: API_BILLING.to_string(),
            tone: HintTone::Preview,
            sign_in: None,
        },
        SpaceAccount::Folder { key, dir } => match rows.iter().find(|row| &row.key == key) {
            Some(row) if row.needs_sign_in || (row.api_billing && !row.title.contains('@')) => {
                AccountState {
                    text: format!(
                        "No account is signed in to {}",
                        home_relative(&row.dir, home)
                    ),
                    tone: HintTone::Error,
                    sign_in: Some(row.dir.clone()),
                }
            }
            Some(row) => AccountState {
                text: space_view_summary(row, false),
                tone: HintTone::Preview,
                sign_in: None,
            },
            None => AccountState {
                text: format!("Checking {}…", home_relative(dir, home)),
                tone: HintTone::Preview,
                sign_in: None,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str, dir: &str, title: &str, is_default: bool) -> AgentAccountRow {
        AgentAccountRow {
            key: key.to_string(),
            dir: PathBuf::from(dir),
            profile: None,
            is_default,
            title: title.to_string(),
            detail: "Claude Max".to_string(),
            source: None,
            in_use: false,
            needs_sign_in: false,
            api_billing: false,
            meters: Vec::new(),
            resets: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_space_reads_its_account_from_the_agent_config_folder_variable() {
        let home = Path::new("/home/a");
        let env = BTreeMap::from([
            ("CLAUDE_CONFIG_DIR".to_string(), "~/.claude-2".to_string()),
            ("CODEX_HOME".to_string(), "relative".to_string()),
            ("AWS_PROFILE".to_string(), "work".to_string()),
        ]);
        assert_eq!(
            space_account(&env, UsageAgent::ClaudeCode, Some(home)),
            SpaceAccount::Folder {
                key: account_dir_key(Path::new("/home/a/.claude-2")),
                dir: PathBuf::from("/home/a/.claude-2"),
            }
        );
        assert_eq!(
            space_account(&env, UsageAgent::Codex, Some(home)),
            SpaceAccount::Unusable {
                value: "relative".to_string()
            }
        );
        assert_eq!(
            space_account(&BTreeMap::new(), UsageAgent::Codex, Some(home)),
            SpaceAccount::Default
        );
        assert_eq!(other_variable_count(&env), 1);
    }

    #[test]
    fn choosing_an_account_rewrites_only_that_agent_variable() {
        let env = BTreeMap::from([
            ("CLAUDE_CONFIG_DIR".to_string(), "~/.claude-2".to_string()),
            ("CODEX_HOME".to_string(), "~/.codex-2".to_string()),
            ("AWS_PROFILE".to_string(), "work".to_string()),
        ]);
        let chosen = env_with_account(
            &env,
            UsageAgent::ClaudeCode,
            Some("~/.claude-3".to_string()),
        );
        assert_eq!(chosen["CLAUDE_CONFIG_DIR"], "~/.claude-3");
        assert_eq!(chosen["CODEX_HOME"], "~/.codex-2");
        assert_eq!(chosen["AWS_PROFILE"], "work");
        let cleared = env_with_account(&chosen, UsageAgent::ClaudeCode, None);
        assert!(!cleared.contains_key("CLAUDE_CONFIG_DIR"));
        assert_eq!(cleared.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn folders_under_home_are_stored_and_shown_with_a_tilde() {
        let home = Some(Path::new("/home/a"));
        assert_eq!(
            home_relative(Path::new("/home/a/.claude-2"), home),
            "~/.claude-2"
        );
        assert_eq!(home_relative(Path::new("/home/a/x/y"), home), "~/x/y");
        assert_eq!(home_relative(Path::new("/srv/claude"), home), "/srv/claude");
        let titled = row("k", "/home/a/.claude-2", "me@example.com", false);
        assert_eq!(account_label(&titled, home), "me@example.com");
        let untitled = row("k", "/home/a/.claude-2", "Work", false);
        assert_eq!(account_label(&untitled, home), "~/.claude-2");
    }

    #[test]
    fn the_agent_default_folder_is_never_offered_as_a_space_account() {
        let rows = vec![
            row("a", "/h/.claude", "me@example.com", true),
            row("b", "/h/.claude-2", "pro@example.com", false),
        ];
        let offered: Vec<&str> = pickable_accounts(&rows)
            .map(|row| row.key.as_str())
            .collect();
        assert_eq!(offered, ["b"]);
        assert_eq!(
            default_account(&rows, UsageAgent::ClaudeCode, &[]).map(|row| row.key.as_str()),
            Some("a")
        );
        let defaults = [(UsageAgent::ClaudeCode, "b".to_string())];
        assert_eq!(
            default_account(&rows, UsageAgent::ClaudeCode, &defaults).map(|row| row.key.as_str()),
            Some("b")
        );
    }

    #[test]
    fn a_signed_out_folder_names_itself_and_offers_sign_in() {
        let home = Some(Path::new("/h"));
        let mut signed_out = row("k", "/h/.claude-2", "Work", false);
        signed_out.needs_sign_in = true;
        let chosen = SpaceAccount::Folder {
            key: "k".to_string(),
            dir: PathBuf::from("/h/.claude-2"),
        };
        let state = account_state(&chosen, &[signed_out], None, false, home);
        assert_eq!(state.tone, HintTone::Error);
        assert_eq!(state.sign_in, Some(PathBuf::from("/h/.claude-2")));
        assert!(state.text.starts_with("No account is signed in to "));
        let fallback = row("d", "/h/.claude", "me@example.com", true);
        let state = account_state(&SpaceAccount::Default, &[], Some(&fallback), false, home);
        assert_eq!(state.text, "Your default account, me@example.com");
        let mut keyed = row("k", "/h/.claude-2", "pro@example.com", false);
        keyed.api_billing = true;
        keyed.detail = API_BILLING.to_string();
        let keyed = [keyed];
        assert_eq!(
            account_state(&chosen, &keyed, None, true, home).text,
            API_BILLING
        );
        assert_eq!(
            account_state(&chosen, &keyed, None, false, home).text,
            "Signed in",
            "a space that turns the inherited key off signs in with the account itself"
        );
    }

    #[test]
    fn a_space_bills_through_a_key_it_sets_or_inherits_without_blanking() {
        let inherited = |key: &str| key == "ANTHROPIC_API_KEY";
        assert!(space_bills_through_api_key(
            &BTreeMap::new(),
            UsageAgent::ClaudeCode,
            inherited
        ));
        let blanked = BTreeMap::from([("ANTHROPIC_API_KEY".to_string(), String::new())]);
        assert!(!space_bills_through_api_key(
            &blanked,
            UsageAgent::ClaudeCode,
            inherited
        ));
        let own = BTreeMap::from([("OPENAI_API_KEY".to_string(), "sk".to_string())]);
        assert!(space_bills_through_api_key(&own, UsageAgent::Codex, |_| {
            false
        }));
    }
}
