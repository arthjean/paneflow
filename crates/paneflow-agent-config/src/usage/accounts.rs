use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::io::absolute_env_dir;

pub const CLAUDE_BILLING_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
];
pub const CODEX_BILLING_KEYS: &[&str] = &["OPENAI_API_KEY"];
pub const ACCOUNT_LABEL_FILE_CAP: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UsageAgent {
    ClaudeCode,
    Codex,
}

impl UsageAgent {
    pub const ALL: [Self; 2] = [Self::ClaudeCode, Self::Codex];

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    pub const fn config_dir_key(self) -> &'static str {
        match self {
            Self::ClaudeCode => "CLAUDE_CONFIG_DIR",
            Self::Codex => "CODEX_HOME",
        }
    }

    pub const fn billing_keys(self) -> &'static [&'static str] {
        match self {
            Self::ClaudeCode => CLAUDE_BILLING_KEYS,
            Self::Codex => CODEX_BILLING_KEYS,
        }
    }

    pub const fn runtime_slug(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }

    pub fn for_runtime_slug(slug: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|agent| agent.runtime_slug() == slug)
    }

    pub fn for_profile_agent(agent: &str) -> Option<Self> {
        match agent {
            "claude_code" | "claude" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginKind {
    Default,
    Space,
    Profile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountOrigin {
    pub kind: OriginKind,
    pub name: String,
    pub agent: Option<UsageAgent>,
    pub env: BTreeMap<String, String>,
}

impl AccountOrigin {
    pub fn process_default(name: impl Into<String>) -> Self {
        let keys = UsageAgent::ALL.iter().flat_map(|agent| {
            std::iter::once(agent.config_dir_key()).chain(agent.billing_keys().iter().copied())
        });
        Self {
            kind: OriginKind::Default,
            name: name.into(),
            agent: None,
            env: keys
                .filter_map(|key| {
                    std::env::var(key)
                        .ok()
                        .map(|value| (key.to_string(), value))
                })
                .collect(),
        }
    }

    fn covers(&self, agent: UsageAgent) -> bool {
        self.agent.is_none_or(|only| only == agent)
    }

    fn bills_through_api_key(
        &self,
        agent: UsageAgent,
        inherited: Option<&BTreeMap<String, String>>,
    ) -> bool {
        agent.billing_keys().iter().any(|key| {
            self.env
                .get(*key)
                .or_else(|| inherited.and_then(|env| env.get(*key)))
                .is_some_and(|value| !value.is_empty())
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginRef {
    pub kind: OriginKind,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageAccount {
    pub agent: UsageAgent,
    pub config_dir: PathBuf,
    pub origins: Vec<OriginRef>,
    pub label: String,
    pub api_billing: bool,
}

struct Resolved {
    dir: PathBuf,
    account_file: Option<PathBuf>,
}

pub fn inventory(origins: &[AccountOrigin], home: Option<&Path>) -> Vec<UsageAccount> {
    let mut accounts: Vec<(String, Option<PathBuf>, UsageAccount)> = Vec::new();
    let inherited = origins
        .iter()
        .find(|origin| origin.kind == OriginKind::Default)
        .map(|origin| &origin.env);
    for agent in UsageAgent::ALL {
        for origin in origins.iter().filter(|origin| origin.covers(agent)) {
            let Some(resolved) = resolve(origin, agent, home) else {
                continue;
            };
            let key = dir_key(&resolved.dir);
            let reference = OriginRef {
                kind: origin.kind,
                name: origin.name.clone(),
            };
            let api_billing = origin.bills_through_api_key(agent, inherited);
            match accounts
                .iter_mut()
                .find(|(held, _, account)| account.agent == agent && *held == key)
            {
                Some((_, _, account)) => {
                    account.origins.push(reference);
                    account.api_billing |= api_billing;
                }
                None => accounts.push((
                    key,
                    resolved.account_file,
                    UsageAccount {
                        agent,
                        config_dir: resolved.dir,
                        label: origin.name.clone(),
                        origins: vec![reference],
                        api_billing,
                    },
                )),
            }
        }
    }
    accounts
        .into_iter()
        .map(|(_, account_file, mut account)| {
            if let Some(email) = account_file.as_deref().and_then(claude_account_email) {
                account.label = email;
            }
            account
        })
        .collect()
}

fn resolve(origin: &AccountOrigin, agent: UsageAgent, home: Option<&Path>) -> Option<Resolved> {
    let key = agent.config_dir_key();
    let declared = origin
        .env
        .get(key)
        .map(|value| expand_home(value, home))
        .and_then(|value| absolute_env_dir(key, Some(value)));
    let (dir, account_file) = match (declared, origin.kind) {
        (Some(dir), _) => {
            let account_file = dir.join(".claude.json");
            (dir, account_file)
        }
        (None, OriginKind::Default) => match agent {
            UsageAgent::ClaudeCode => (home?.join(".claude"), home?.join(".claude.json")),
            UsageAgent::Codex => (home?.join(".codex"), PathBuf::new()),
        },
        (None, _) => return None,
    };
    let account_file = (agent == UsageAgent::ClaudeCode).then_some(account_file);
    Some(Resolved {
        dir: canonical_dir(&dir),
        account_file,
    })
}

fn expand_home(value: &str, home: Option<&Path>) -> OsString {
    let rest = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
        .or_else(|| (value == "~").then_some(""));
    match (rest, home) {
        (Some(""), Some(home)) => home.as_os_str().to_owned(),
        (Some(rest), Some(home)) => home.join(rest).into_os_string(),
        _ => OsString::from(value),
    }
}

fn canonical_dir(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(canonical) => strip_verbatim_prefix(canonical),
        Err(_) => lexically_normal(path),
    }
}

fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}

fn lexically_normal(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other.as_os_str()),
        }
    }
    normal
}

fn dir_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    } else {
        text.trim_end_matches('/').to_string()
    }
}

pub fn claude_account_email(account_file: &Path) -> Option<String> {
    let bytes = paneflow_home::read_regular_capped(account_file, ACCOUNT_LABEL_FILE_CAP).ok()?;
    let root: Value = serde_json::from_slice(&bytes).ok()?;
    root.get("oauthAccount")?
        .get("emailAddress")?
        .as_str()
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .map(str::to_string)
}

pub fn session_account_dir(
    agent: UsageAgent,
    env: &BTreeMap<String, String>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    let origin = AccountOrigin {
        kind: OriginKind::Default,
        name: String::new(),
        agent: Some(agent),
        env: env.clone(),
    };
    resolve(&origin, agent, home).map(|resolved| resolved.dir)
}

pub fn canonical_account_dir(path: &Path) -> PathBuf {
    canonical_dir(path)
}

pub fn account_dir_key(path: &Path) -> String {
    dir_key(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(
        kind: OriginKind,
        name: &str,
        agent: Option<UsageAgent>,
        env: &[(&str, &str)],
    ) -> AccountOrigin {
        AccountOrigin {
            kind,
            name: name.to_string(),
            agent,
            env: env
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    fn default_origin(env: &[(&str, &str)]) -> AccountOrigin {
        origin(OriginKind::Default, "Default", None, env)
    }

    fn summary(accounts: &[UsageAccount]) -> Vec<(UsageAgent, PathBuf, Vec<String>)> {
        accounts
            .iter()
            .map(|account| {
                (
                    account.agent,
                    account.config_dir.clone(),
                    account
                        .origins
                        .iter()
                        .map(|origin| origin.name.clone())
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn the_default_origin_yields_one_claude_and_one_codex_account() {
        let home = tempfile::tempdir().unwrap();
        let home = canonical_dir(home.path());
        let accounts = inventory(&[default_origin(&[])], Some(&home));
        assert_eq!(
            summary(&accounts),
            vec![
                (
                    UsageAgent::ClaudeCode,
                    home.join(".claude"),
                    vec!["Default".to_string()]
                ),
                (
                    UsageAgent::Codex,
                    home.join(".codex"),
                    vec!["Default".to_string()]
                ),
            ]
        );
        assert!(accounts.iter().all(|account| account.label == "Default"));
        assert!(accounts.iter().all(|account| !account.api_billing));
    }

    #[test]
    fn the_process_environment_moves_the_default_directories() {
        let home = tempfile::tempdir().unwrap();
        let home = canonical_dir(home.path());
        let claude = home.join("claude-env");
        let codex = home.join("codex-env");
        let accounts = inventory(
            &[default_origin(&[
                ("CLAUDE_CONFIG_DIR", claude.to_str().unwrap()),
                ("CODEX_HOME", codex.to_str().unwrap()),
            ])],
            Some(&home),
        );
        assert_eq!(accounts[0].config_dir, claude);
        assert_eq!(accounts[1].config_dir, codex);
    }

    #[test]
    fn a_space_and_a_profile_add_their_own_claude_accounts() {
        let home = tempfile::tempdir().unwrap();
        let home = canonical_dir(home.path());
        let accounts = inventory(
            &[
                default_origin(&[]),
                origin(
                    OriginKind::Space,
                    "Work",
                    None,
                    &[("CLAUDE_CONFIG_DIR", "~/.claude-work")],
                ),
                origin(
                    OriginKind::Profile,
                    "Claude perso",
                    Some(UsageAgent::ClaudeCode),
                    &[("CLAUDE_CONFIG_DIR", "~/.claude-perso")],
                ),
                origin(OriginKind::Space, "Plain", None, &[]),
            ],
            Some(&home),
        );
        let claude: Vec<_> = accounts
            .iter()
            .filter(|account| account.agent == UsageAgent::ClaudeCode)
            .map(|account| {
                (
                    account.config_dir.clone(),
                    account.origins[0].kind,
                    account.label.clone(),
                )
            })
            .collect();
        assert_eq!(
            claude,
            vec![
                (
                    home.join(".claude"),
                    OriginKind::Default,
                    "Default".to_string()
                ),
                (
                    home.join(".claude-work"),
                    OriginKind::Space,
                    "Work".to_string()
                ),
                (
                    home.join(".claude-perso"),
                    OriginKind::Profile,
                    "Claude perso".to_string()
                ),
            ]
        );
        assert_eq!(
            accounts
                .iter()
                .filter(|account| account.agent == UsageAgent::Codex)
                .count(),
            1,
            "a Claude-only profile and a space without CODEX_HOME add no Codex account"
        );
    }

    #[test]
    fn two_origins_naming_one_directory_share_one_account() {
        let home = tempfile::tempdir().unwrap();
        let home = canonical_dir(home.path());
        let work = home.join(".claude-work");
        std::fs::create_dir_all(&work).unwrap();
        let spelled = work.join("..").join(".claude-work");
        let accounts = inventory(
            &[
                origin(
                    OriginKind::Space,
                    "Work",
                    None,
                    &[("CLAUDE_CONFIG_DIR", "~/.claude-work")],
                ),
                origin(
                    OriginKind::Profile,
                    "Work profile",
                    Some(UsageAgent::ClaudeCode),
                    &[("CLAUDE_CONFIG_DIR", spelled.to_str().unwrap())],
                ),
            ],
            Some(&home),
        );
        assert_eq!(
            summary(&accounts),
            vec![(
                UsageAgent::ClaudeCode,
                work,
                vec!["Work".to_string(), "Work profile".to_string()]
            )]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_ignores_directory_case() {
        let home = tempfile::tempdir().unwrap();
        let home = canonical_dir(home.path());
        let accounts = inventory(
            &[
                origin(
                    OriginKind::Space,
                    "A",
                    None,
                    &[("CODEX_HOME", "~/Codex-Work")],
                ),
                origin(
                    OriginKind::Space,
                    "B",
                    None,
                    &[("CODEX_HOME", "~/codex-work")],
                ),
            ],
            Some(&home),
        );
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].origins.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_resolves_to_its_target() {
        let home = tempfile::tempdir().unwrap();
        let home = canonical_dir(home.path());
        let target = home.join("real-claude");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, home.join("link-claude")).unwrap();
        let accounts = inventory(
            &[
                origin(
                    OriginKind::Space,
                    "A",
                    None,
                    &[("CLAUDE_CONFIG_DIR", "~/real-claude")],
                ),
                origin(
                    OriginKind::Space,
                    "B",
                    None,
                    &[("CLAUDE_CONFIG_DIR", "~/link-claude")],
                ),
            ],
            Some(&home),
        );
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].config_dir, target);
    }

    #[test]
    fn the_claude_email_labels_the_account_from_the_right_file() {
        let home = tempfile::tempdir().unwrap();
        let home = canonical_dir(home.path());
        std::fs::write(
            home.join(".claude.json"),
            r#"{"oauthAccount": {"emailAddress": "me@perso.dev"}}"#,
        )
        .unwrap();
        let work = home.join(".claude-work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(
            work.join(".claude.json"),
            r#"{"oauthAccount": {"emailAddress": "me@company.com", "organizationName": "Co"}}"#,
        )
        .unwrap();
        let accounts = inventory(
            &[
                default_origin(&[]),
                origin(
                    OriginKind::Space,
                    "Work",
                    None,
                    &[("CLAUDE_CONFIG_DIR", "~/.claude-work")],
                ),
            ],
            Some(&home),
        );
        let labels: Vec<_> = accounts
            .iter()
            .map(|account| account.label.as_str())
            .collect();
        assert_eq!(labels, vec!["me@perso.dev", "me@company.com", "Default"]);
    }

    #[test]
    fn an_unreadable_account_file_falls_back_to_the_origin_name() {
        let home = tempfile::tempdir().unwrap();
        let home = canonical_dir(home.path());
        let cases: [&[u8]; 4] = [
            b"not json",
            br#"{"oauthAccount": {"emailAddress": "  "}}"#,
            br#"{"oauthAccount": "me@x.dev"}"#,
            br#"[1, 2, 3]"#,
        ];
        for bytes in cases {
            std::fs::write(home.join(".claude.json"), bytes).unwrap();
            let accounts = inventory(&[default_origin(&[])], Some(&home));
            assert_eq!(accounts[0].label, "Default");
        }
        let oversized = format!(
            r#"{{"oauthAccount": {{"emailAddress": "me@x.dev"}}, "pad": "{}"}}"#,
            "x".repeat(ACCOUNT_LABEL_FILE_CAP as usize)
        );
        std::fs::write(home.join(".claude.json"), oversized).unwrap();
        assert_eq!(
            inventory(&[default_origin(&[])], Some(&home))[0].label,
            "Default"
        );
        std::fs::remove_file(home.join(".claude.json")).unwrap();
        assert_eq!(
            inventory(&[default_origin(&[])], Some(&home))[0].label,
            "Default"
        );
    }

    #[test]
    fn an_api_key_marks_only_the_matching_agent() {
        let home = tempfile::tempdir().unwrap();
        let home = canonical_dir(home.path());
        for key in CLAUDE_BILLING_KEYS {
            let accounts = inventory(&[default_origin(&[(key, "sk-x")])], Some(&home));
            assert!(accounts[0].api_billing, "{key}");
            assert!(!accounts[1].api_billing, "{key}");
        }
        let accounts = inventory(
            &[default_origin(&[("OPENAI_API_KEY", "sk-x")])],
            Some(&home),
        );
        assert!(!accounts[0].api_billing);
        assert!(accounts[1].api_billing);
        let accounts = inventory(&[default_origin(&[("ANTHROPIC_API_KEY", "")])], Some(&home));
        assert!(!accounts[0].api_billing, "an empty value is not a key");
    }

    #[test]
    fn a_key_in_the_paneflow_environment_bills_every_origin_that_does_not_redefine_it() {
        let home = tempfile::tempdir().unwrap();
        let home = canonical_dir(home.path());
        let accounts = inventory(
            &[
                default_origin(&[("ANTHROPIC_API_KEY", "sk-x")]),
                origin(
                    OriginKind::Space,
                    "Work",
                    None,
                    &[("CLAUDE_CONFIG_DIR", "~/.claude-work")],
                ),
                origin(
                    OriginKind::Space,
                    "Cleared",
                    None,
                    &[
                        ("CLAUDE_CONFIG_DIR", "~/.claude-cleared"),
                        ("ANTHROPIC_API_KEY", ""),
                    ],
                ),
            ],
            Some(&home),
        );
        let billing: Vec<_> = accounts
            .iter()
            .filter(|account| account.agent == UsageAgent::ClaudeCode)
            .map(|account| (account.label.as_str(), account.api_billing))
            .collect();
        assert_eq!(
            billing,
            vec![("Default", true), ("Work", true), ("Cleared", false)]
        );
    }

    #[test]
    fn a_relative_config_dir_is_ignored() {
        let home = tempfile::tempdir().unwrap();
        let home = canonical_dir(home.path());
        let accounts = inventory(
            &[
                default_origin(&[("CLAUDE_CONFIG_DIR", "relative/claude")]),
                origin(
                    OriginKind::Space,
                    "Rel",
                    None,
                    &[("CLAUDE_CONFIG_DIR", "rel")],
                ),
            ],
            Some(&home),
        );
        let claude: Vec<_> = accounts
            .iter()
            .filter(|account| account.agent == UsageAgent::ClaudeCode)
            .map(|account| account.config_dir.clone())
            .collect();
        assert_eq!(claude, vec![home.join(".claude")]);
    }

    #[test]
    fn profile_agents_map_to_usage_agents() {
        assert_eq!(
            UsageAgent::for_profile_agent("claude_code"),
            Some(UsageAgent::ClaudeCode)
        );
        assert_eq!(
            UsageAgent::for_profile_agent("codex"),
            Some(UsageAgent::Codex)
        );
        assert_eq!(UsageAgent::for_profile_agent("gemini"), None);
    }

    #[test]
    fn a_session_resolves_its_account_from_its_launch_env_or_the_default_dir() {
        let home = tempfile::tempdir().unwrap();
        let work = BTreeMap::from([(
            "CLAUDE_CONFIG_DIR".to_string(),
            "~/.claude-work".to_string(),
        )]);
        assert_eq!(
            session_account_dir(UsageAgent::ClaudeCode, &work, Some(home.path())),
            Some(canonical_account_dir(&home.path().join(".claude-work")))
        );
        assert_eq!(
            session_account_dir(UsageAgent::ClaudeCode, &BTreeMap::new(), Some(home.path())),
            Some(canonical_account_dir(&home.path().join(".claude")))
        );
        assert_eq!(
            session_account_dir(UsageAgent::Codex, &work, Some(home.path())),
            Some(canonical_account_dir(&home.path().join(".codex"))),
            "a Claude variable never moves the Codex account"
        );
        assert_eq!(
            UsageAgent::for_runtime_slug("claude-code"),
            Some(UsageAgent::ClaudeCode)
        );
        assert_eq!(UsageAgent::for_runtime_slug("gemini"), None);
    }
}
