use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::{Context, Window};
use paneflow_agent_config::usage::accounts::{UsageAgent, account_dir_key, canonical_account_dir};
use paneflow_agent_integrations::{IntegrationState, IntegrationTarget};
use paneflow_config::schema::{AgentProfileConfig, SpaceId};

use super::{AgentAccountNamer, NewAccount, is_account_profile, profile_base, usage_agent};
use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;
use crate::settings::tabs::space_accounts::space_account_value;

const FIRST_EXTRA_ACCOUNT: u32 = 2;
const LAST_EXTRA_ACCOUNT: u32 = 99;
const MAX_ACCOUNT_SLUG_CHARS: usize = 32;

const fn account_dir_base(agent: UsageAgent) -> &'static str {
    agent.default_dir_name()
}

pub(crate) fn account_dir_value(agent: UsageAgent, slug: &str) -> String {
    format!("~/{}-{slug}", account_dir_base(agent))
}

fn account_slug_of(agent: UsageAgent, value: &str) -> Option<&str> {
    value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))?
        .strip_prefix(account_dir_base(agent))?
        .strip_prefix('-')
        .filter(|slug| !slug.is_empty())
}

pub(crate) fn account_dir_prefix(agent: UsageAgent) -> String {
    account_dir_value(agent, "")
}

pub(crate) fn chosen_account_slug(typed: &str) -> Result<String, &'static str> {
    let slug = typed.trim();
    if slug.is_empty() {
        return Err("Type a name for the folder.");
    }
    if slug.chars().count() > MAX_ACCOUNT_SLUG_CHARS {
        return Err("Use 32 characters at most.");
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_');
    if !slug.chars().all(allowed) {
        return Err("Use only letters, digits, hyphens and underscores.");
    }
    if !slug.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return Err("Start the name with a letter or a digit.");
    }
    Ok(slug.to_string())
}

fn chosen_account_slot(
    agent: UsageAgent,
    slug: String,
    home: &Path,
    taken: &HashSet<String>,
) -> Result<(String, PathBuf), String> {
    let dir = home.join(format!("{}-{slug}", account_dir_base(agent)));
    if dir.exists() || taken.contains(&account_dir_key(&canonical_account_dir(&dir))) {
        return Err(format!(
            "{} already exists",
            account_dir_value(agent, &slug)
        ));
    }
    Ok((slug, dir))
}

pub(crate) fn auto_account_name(agent: TerminalAgent, slug: &str) -> String {
    format!("{} {slug}", agent.display_name())
}

fn free_account_slot(
    agent: UsageAgent,
    home: &Path,
    taken: &HashSet<String>,
) -> Option<(String, PathBuf)> {
    (FIRST_EXTRA_ACCOUNT..=LAST_EXTRA_ACCOUNT).find_map(|number| {
        let slug = number.to_string();
        let dir = home.join(format!("{}-{slug}", account_dir_base(agent)));
        let free = !dir.exists() && !taken.contains(&account_dir_key(&canonical_account_dir(&dir)));
        free.then_some((slug, dir))
    })
}

pub(crate) fn adopt_account_emails(
    profiles: &[AgentProfileConfig],
    email_of: impl Fn(UsageAgent, &str) -> Option<String>,
) -> Option<Vec<AgentProfileConfig>> {
    let mut next = profiles.to_vec();
    let mut changed = false;
    for index in 0..next.len() {
        let entry = &next[index];
        let Some(agent) = profile_base(entry) else {
            continue;
        };
        let Some(usage) =
            usage_agent(agent).filter(|usage| is_account_profile(entry, Some(*usage)))
        else {
            continue;
        };
        let Some(value) = entry.env.values().next() else {
            continue;
        };
        let auto_named = account_slug_of(usage, value)
            .is_some_and(|slug| entry.name == auto_account_name(agent, slug));
        if !auto_named {
            continue;
        }
        let Some(email) = email_of(usage, value) else {
            continue;
        };
        let taken = next
            .iter()
            .any(|other| profile_base(other) == Some(agent) && other.name == email);
        if !taken {
            next[index].name = email;
            changed = true;
        }
    }
    changed.then_some(next)
}

pub(crate) fn default_accounts_with(
    current: &BTreeMap<String, String>,
    agent: UsageAgent,
    dir: Option<&Path>,
) -> BTreeMap<String, String> {
    let mut next = current.clone();
    match dir {
        Some(dir) => {
            next.insert(agent.runtime_slug().to_string(), dir.display().to_string());
        }
        None => {
            next.remove(agent.runtime_slug());
        }
    }
    next
}

fn resolved_dir_key(value: &str, home: Option<&Path>) -> Option<String> {
    crate::env_expand::expand_with_process_env(value, home)
        .ok()
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .map(|dir| account_dir_key(&canonical_account_dir(&dir)))
}

fn account_marker_dir(paneflow_home: &Path, agent: UsageAgent, slug: &str) -> PathBuf {
    paneflow_home
        .join("integrations")
        .join("accounts")
        .join(format!("{}-{slug}", agent.runtime_slug()))
}

fn without_account_profiles(
    profiles: &[AgentProfileConfig],
    agent: UsageAgent,
    key: &str,
    home: Option<&Path>,
) -> (Vec<AgentProfileConfig>, Vec<AgentProfileConfig>) {
    profiles.iter().cloned().partition(|entry| {
        !(is_account_profile(entry, Some(agent))
            && entry
                .env
                .values()
                .next()
                .and_then(|value| resolved_dir_key(value, home))
                .as_deref()
                == Some(key))
    })
}

fn account_target(
    paneflow_home: &Path,
    agent: UsageAgent,
    slug: &str,
    dir: &Path,
) -> IntegrationTarget {
    let claude = (agent == UsageAgent::ClaudeCode).then(|| paneflow_agent_config::ClaudePaths {
        config_dir: dir.to_path_buf(),
        global_config: dir.join(".claude.json"),
    });
    let codex_home = (agent == UsageAgent::Codex).then(|| dir.to_path_buf());
    IntegrationTarget {
        marker_dir: account_marker_dir(paneflow_home, agent, slug),
        config_file: paneflow_home::config_path_in(paneflow_home),
        claude,
        codex_home,
    }
}

fn prepare_account_dir(
    agent: UsageAgent,
    slug: &str,
    dir: &Path,
    integration: Option<&'static str>,
) -> Result<Option<String>, String> {
    crate::space_env::ensure_dir(dir)
        .map_err(|error| format!("Could not create {}: {error}", dir.display()))?;
    let Some(integration) = integration else {
        return Ok(None);
    };
    let installed = paneflow_home::paneflow_home()
        .ok_or_else(|| "the Paneflow home directory cannot be resolved".to_string())
        .and_then(|home| {
            let binaries = paneflow_agent_integrations::IntegrationBinaries {
                hook_binary: crate::ai_hooks::extract::ensure_ai_hook_extracted()
                    .map_err(|error| format!("hook extraction failed: {error:#}"))?,
            };
            paneflow_agent_integrations::install_target_integration(
                &account_target(&home, agent, slug, dir),
                integration,
                &binaries,
            )
        });
    Ok(installed
        .err()
        .map(|error| format!("Hooks and MCP were not added to this account: {error}")))
}

impl PaneFlowApp {
    fn agent_integration_installed(&self, agent: TerminalAgent) -> bool {
        let slug = agent.runtime().slug;
        self.integration_status.as_ref().is_some_and(|statuses| {
            statuses.iter().any(|status| {
                status.slug == slug && matches!(status.state, IntegrationState::Installed)
            })
        })
    }

    fn account_dir_keys(&self, agent: UsageAgent, home: Option<&Path>) -> HashSet<String> {
        let profiles = self
            .cached_config
            .agent_profiles
            .iter()
            .map(|entry| &entry.env);
        let spaces = self.cached_config.spaces.iter().map(|space| &space.env);
        profiles
            .chain(spaces)
            .flat_map(|vars| vars.iter())
            .filter(|(key, _)| key.eq_ignore_ascii_case(agent.config_dir_key()))
            .filter_map(|(_, value)| resolved_dir_key(value, home))
            .chain(
                self.usage
                    .inventory_dirs(agent)
                    .map(|dir| account_dir_key(&canonical_account_dir(dir))),
            )
            .collect()
    }

    pub(crate) fn open_agent_account_namer(
        &mut self,
        agent: TerminalAgent,
        space: Option<SpaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(usage) = usage_agent(agent) else {
            return;
        };
        if self.agent_sign_in.is_some() {
            self.show_toast("Finish or cancel the sign-in in progress first", cx);
            return;
        }
        self.agent_settings_dialog = None;
        self.agent_update_menu_open = None;
        self.agent_profile_menu = None;
        self.agent_profile_editor = None;
        self.agent_account_name_input
            .update(cx, |input, cx| input.clear(cx));
        self.agent_account_namer = Some(AgentAccountNamer {
            agent,
            space,
            error: None,
        });
        let focus = self.agent_account_name_input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let taken = self.account_dir_keys(usage, Some(&home));
        cx.spawn(async move |this, cx| {
            let suggested = smol::unblock(move || {
                free_account_slot(usage, &home, &taken).map(|(slug, _)| slug)
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                let still_open = this
                    .agent_account_namer
                    .as_ref()
                    .is_some_and(|namer| namer.agent == agent);
                let untouched = this.agent_account_name_input.read(cx).value().is_empty();
                if let Some(slug) = suggested.filter(|_| still_open && untouched) {
                    this.agent_account_name_input.update(cx, |input, cx| {
                        input.set_value(gpui::SharedString::from(slug), cx);
                    });
                }
            });
        })
        .detach();
    }

    pub(crate) fn close_agent_account_namer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agent_account_namer = None;
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn confirm_agent_account_name(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((agent, space)) = self
            .agent_account_namer
            .as_ref()
            .map(|namer| (namer.agent, namer.space.clone()))
        else {
            return;
        };
        let Some(usage) = usage_agent(agent) else {
            return;
        };
        let typed = self.agent_account_name_input.read(cx).value();
        let home = dirs::home_dir();
        let checked = chosen_account_slug(&typed)
            .map_err(str::to_string)
            .and_then(|slug| {
                let value = account_dir_value(usage, &slug);
                let used = resolved_dir_key(&value, home.as_deref()).is_some_and(|key| {
                    self.account_dir_keys(usage, home.as_deref()).contains(&key)
                });
                if used {
                    Err(format!("An account already uses {value}."))
                } else {
                    Ok(slug)
                }
            });
        match checked {
            Ok(slug) => {
                self.close_agent_account_namer(window, cx);
                self.add_agent_account_in(agent, space, slug, window, cx);
            }
            Err(error) => {
                if let Some(namer) = self.agent_account_namer.as_mut() {
                    namer.error = Some(error);
                }
                cx.notify();
            }
        }
    }

    fn add_agent_account_in(
        &mut self,
        agent: TerminalAgent,
        space: Option<SpaceId>,
        chosen: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(usage) = usage_agent(agent) else {
            return;
        };
        if self.agent_account_adding.is_some() {
            return;
        }
        if self.agent_sign_in.is_some() {
            self.show_toast("Finish or cancel the sign-in in progress first", cx);
            return;
        }
        let Some(home) = dirs::home_dir() else {
            self.show_toast("Your home folder cannot be found", cx);
            return;
        };
        let taken = self.account_dir_keys(usage, Some(&home));
        let integration = self
            .agent_integration_installed(agent)
            .then_some(agent.runtime().slug);
        self.agent_profile_menu = None;
        self.agent_account_adding = Some(agent);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let prepared = smol::unblock(move || {
                let (slug, dir) = chosen_account_slot(usage, chosen, &home, &taken)?;
                let note = prepare_account_dir(usage, &slug, &dir, integration)?;
                let marker_dir = integration
                    .and(paneflow_home::paneflow_home())
                    .map(|paneflow_home| account_marker_dir(&paneflow_home, usage, &slug));
                Ok::<_, String>((slug, dir, note, marker_dir))
            })
            .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.agent_account_adding = None;
                let (slug, dir, note, marker_dir) = match prepared {
                    Ok(prepared) => prepared,
                    Err(message) => {
                        this.agent_account_namer = Some(AgentAccountNamer {
                            agent,
                            space: space.clone(),
                            error: Some(format!("{message}.")),
                        });
                        let focus = this.agent_account_name_input.read(cx).focus_handle.clone();
                        window.focus(&focus, cx);
                        cx.notify();
                        return;
                    }
                };
                let mut profiles = this.cached_config.agent_profiles.clone();
                profiles.push(AgentProfileConfig {
                    name: auto_account_name(agent, &slug),
                    agent: agent.tag().to_string(),
                    env: BTreeMap::from([(
                        usage.config_dir_key().to_string(),
                        account_dir_value(usage, &slug),
                    )]),
                    args: Vec::new(),
                });
                this.persist_agent_profiles(profiles, cx);
                let previous = space.as_ref().map(|id| {
                    let value = this
                        .cached_config
                        .space(id)
                        .and_then(|entry| space_account_value(&entry.env, usage));
                    (id.clone(), value)
                });
                if let Some(space) = space.as_ref() {
                    this.set_space_account(
                        space,
                        agent,
                        Some(&dir),
                        &auto_account_name(agent, &slug),
                        cx,
                    );
                }
                if let Some(note) = note {
                    this.show_toast(note, cx);
                }
                let account = NewAccount {
                    dir: dir.clone(),
                    marker_dir,
                    space: previous,
                };
                this.begin_agent_sign_in(agent, dir, Some(account), cx);
            });
        })
        .detach();
    }

    pub(super) fn remove_new_account(
        &mut self,
        agent: TerminalAgent,
        key: &str,
        account: NewAccount,
        cx: &mut Context<Self>,
    ) {
        let Some(usage) = usage_agent(agent) else {
            return;
        };
        let home = dirs::home_dir();
        let (kept, removed) = without_account_profiles(
            &self.cached_config.agent_profiles,
            usage,
            key,
            home.as_deref(),
        );
        for entry in &removed {
            self.forget_default_account_of(entry, cx);
        }
        if !removed.is_empty() {
            self.persist_agent_profiles(kept, cx);
        }
        if let Some((space, previous)) = account.space {
            self.restore_space_account(&space, usage, key, previous, cx);
        }
        self.show_toast("Sign-in canceled. The new account was removed.", cx);
    }

    pub(crate) fn adopt_known_account_emails(&mut self, cx: &mut Context<Self>) {
        let home = dirs::home_dir();
        let email_of = |agent: UsageAgent, value: &str| {
            let key = resolved_dir_key(value, home.as_deref())?;
            self.usage.known_email(agent, &key)
        };
        if let Some(profiles) = adopt_account_emails(&self.cached_config.agent_profiles, email_of) {
            self.persist_agent_profiles(profiles, cx);
        }
    }

    pub(crate) fn forget_default_account_of(
        &mut self,
        entry: &AgentProfileConfig,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = profile_base(entry).and_then(usage_agent) else {
            return;
        };
        if !is_account_profile(entry, Some(agent)) {
            return;
        }
        let Some(agents) = self.cached_config.agents.clone() else {
            return;
        };
        let home = dirs::home_dir();
        let removed = entry
            .env
            .values()
            .next()
            .and_then(|value| resolved_dir_key(value, home.as_deref()));
        let current = agents
            .default_accounts
            .get(agent.runtime_slug())
            .and_then(|value| resolved_dir_key(value, home.as_deref()));
        if removed.is_none() || removed != current {
            return;
        }
        let mut agents = agents;
        agents.default_accounts = default_accounts_with(&agents.default_accounts, agent, None);
        if let Ok(value) = serde_json::to_value(agents) {
            self.persist_setting(false, "agents", value, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account_profile(
        name: &str,
        agent: TerminalAgent,
        key: &str,
        dir: &str,
    ) -> AgentProfileConfig {
        AgentProfileConfig {
            name: name.to_string(),
            agent: agent.tag().to_string(),
            env: BTreeMap::from([(key.to_string(), dir.to_string())]),
            args: Vec::new(),
        }
    }

    #[test]
    fn a_canceled_new_account_takes_only_its_own_profiles_with_it() {
        let home = tempfile::tempdir().unwrap();
        let claude = TerminalAgent::ClaudeCode;
        let profiles = vec![
            account_profile("Claude Code 2", claude, "CLAUDE_CONFIG_DIR", "~/.claude-2"),
            account_profile("Claude Code 3", claude, "CLAUDE_CONFIG_DIR", "~/.claude-3"),
            account_profile("Codex 3", TerminalAgent::Codex, "CODEX_HOME", "~/.codex-3"),
        ];
        let key = account_dir_key(&canonical_account_dir(&home.path().join(".claude-3")));
        let (kept, removed) =
            without_account_profiles(&profiles, UsageAgent::ClaudeCode, &key, Some(home.path()));
        let names = |entries: &[AgentProfileConfig]| {
            entries
                .iter()
                .map(|entry| entry.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&removed), ["Claude Code 3"]);
        assert_eq!(names(&kept), ["Claude Code 2", "Codex 3"]);
    }

    #[test]
    fn a_chosen_folder_name_is_one_plain_word_that_fits_every_platform() {
        assert_eq!(chosen_account_slug("  work ").as_deref(), Ok("work"));
        assert_eq!(
            chosen_account_slug("Client_2-b").as_deref(),
            Ok("Client_2-b")
        );
        for refused in [
            "",
            "  ",
            "-work",
            "_x",
            "a b",
            "a/b",
            "..",
            "caf\u{e9}",
            "a.b",
        ] {
            assert!(chosen_account_slug(refused).is_err(), "{refused:?}");
        }
        assert!(chosen_account_slug(&"a".repeat(MAX_ACCOUNT_SLUG_CHARS)).is_ok());
        assert!(chosen_account_slug(&"a".repeat(MAX_ACCOUNT_SLUG_CHARS + 1)).is_err());
        assert_eq!(account_dir_prefix(UsageAgent::Grok), "~/.grok-");
    }

    #[test]
    fn a_chosen_folder_is_refused_when_it_exists_or_an_account_uses_it() {
        let home = tempfile::tempdir().unwrap();
        let taken = HashSet::new();
        let (slug, dir) =
            chosen_account_slot(UsageAgent::ClaudeCode, "work".into(), home.path(), &taken)
                .expect("a free name is taken as typed");
        assert_eq!(slug, "work");
        assert_eq!(dir, home.path().join(".claude-work"));
        std::fs::create_dir(&dir).unwrap();
        assert_eq!(
            chosen_account_slot(UsageAgent::ClaudeCode, "work".into(), home.path(), &taken),
            Err("~/.claude-work already exists".to_string())
        );
        let used = HashSet::from([account_dir_key(&canonical_account_dir(
            &home.path().join(".grok-perso"),
        ))]);
        assert!(
            chosen_account_slot(UsageAgent::Grok, "perso".into(), home.path(), &used).is_err(),
            "a folder an account already names is refused before it exists"
        );
    }

    #[test]
    fn a_new_account_takes_the_first_numbered_folder_nobody_uses() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".claude-2")).unwrap();
        let taken = HashSet::from([account_dir_key(&canonical_account_dir(
            &home.path().join(".claude-3"),
        ))]);
        let (slug, dir) = free_account_slot(UsageAgent::ClaudeCode, home.path(), &taken).unwrap();
        assert_eq!(slug, "4");
        assert_eq!(dir, home.path().join(".claude-4"));
        let (slug, _) = free_account_slot(UsageAgent::Codex, home.path(), &taken).unwrap();
        assert_eq!(slug, "2", "each agent counts its own folders");
        assert_eq!(account_dir_value(UsageAgent::Codex, "2"), "~/.codex-2");
        assert_eq!(account_slug_of(UsageAgent::Codex, "~/.codex-2"), Some("2"));
        assert_eq!(account_slug_of(UsageAgent::Codex, "~/.claude-2"), None);
        assert_eq!(account_slug_of(UsageAgent::Codex, "/srv/.codex-2"), None);
    }

    #[test]
    fn an_account_named_by_paneflow_takes_its_email_once_known() {
        let claude = TerminalAgent::ClaudeCode;
        let profiles = vec![
            account_profile("Claude Code 2", claude, "CLAUDE_CONFIG_DIR", "~/.claude-2"),
            account_profile("Pro", claude, "CLAUDE_CONFIG_DIR", "~/.claude-3"),
            account_profile("Claude Code 4", claude, "CLAUDE_CONFIG_DIR", "~/.claude-4"),
        ];
        let renamed = adopt_account_emails(&profiles, |_, value| match value {
            "~/.claude-2" | "~/.claude-3" => Some(format!("{value}@example.com")),
            _ => None,
        })
        .expect("the auto-named account with a known email is renamed");
        assert_eq!(renamed[0].name, "~/.claude-2@example.com");
        assert_eq!(renamed[1].name, "Pro", "a name the user chose is kept");
        assert_eq!(renamed[2].name, "Claude Code 4", "no email yet, no rename");
        assert!(
            adopt_account_emails(&renamed, |_, value| {
                (value == "~/.claude-2").then(|| "x@example.com".to_string())
            })
            .is_none(),
            "a renamed account is left alone"
        );
        let clash = vec![
            account_profile("me@example.com", claude, "CLAUDE_CONFIG_DIR", "~/.claude-5"),
            account_profile("Claude Code 2", claude, "CLAUDE_CONFIG_DIR", "~/.claude-2"),
        ];
        assert!(
            adopt_account_emails(&clash, |_, _| Some("me@example.com".into())).is_none(),
            "two profiles never share a name"
        );
    }

    #[test]
    fn choosing_the_agent_default_folder_clears_the_override() {
        let current = BTreeMap::from([("codex".to_string(), "/home/a/.codex-old".to_string())]);
        let chosen = default_accounts_with(
            &current,
            UsageAgent::ClaudeCode,
            Some(Path::new("/home/a/.claude-work")),
        );
        assert_eq!(chosen["claude-code"], "/home/a/.claude-work");
        assert_eq!(chosen["codex"], "/home/a/.codex-old");
        let cleared = default_accounts_with(&chosen, UsageAgent::ClaudeCode, None);
        assert!(!cleared.contains_key("claude-code"));
        assert_eq!(cleared.len(), 1);
    }

    #[test]
    fn an_account_target_installs_into_the_account_folder_only() {
        let dir = Path::new("/home/a/.claude-work");
        let target = account_target(
            Path::new("/home/a/.paneflow"),
            UsageAgent::ClaudeCode,
            "work",
            dir,
        );
        assert_eq!(
            target
                .claude
                .as_ref()
                .map(|paths| paths.config_dir.as_path()),
            Some(dir)
        );
        assert_eq!(target.codex_home, None);
        assert!(target.marker_dir.ends_with("accounts/claude-code-work"));
    }
}
