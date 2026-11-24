use std::collections::BTreeMap;

use serde_json::Value;

use super::PaneFlowConfig;

pub const SANDBOX_PANEFLOW: &str = "paneflow";

pub const SANDBOX_REQUEST_ENV: &str = "PANEFLOW_SANDBOX";

pub const SANDBOX_REQUEST_PANEFLOW: &str = "paneflow";

pub const SANDBOX_REQUEST_PANEFLOW_UNIX_SCOPE: &str = "paneflow+unix-scope";

pub const SANDBOX_SUPPORTED_PLATFORMS: [&str; 1] = ["linux"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileSandbox {
    pub unix_scope: bool,
}

impl ProfileSandbox {
    pub const fn request_value(self) -> &'static str {
        if self.unix_scope {
            SANDBOX_REQUEST_PANEFLOW_UNIX_SCOPE
        } else {
            SANDBOX_REQUEST_PANEFLOW
        }
    }

    pub fn from_request(value: &str) -> Option<Self> {
        match value {
            SANDBOX_REQUEST_PANEFLOW => Some(Self { unix_scope: false }),
            SANDBOX_REQUEST_PANEFLOW_UNIX_SCOPE => Some(Self { unix_scope: true }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentSandboxProfiles {
    profiles: BTreeMap<String, Result<ProfileSandbox, String>>,
}

fn parse_profile(
    platform: &str,
    profile: &str,
    value: &Value,
) -> Option<Result<ProfileSandbox, String>> {
    let fields = value.as_object()?;
    let sandbox = fields.get("sandbox")?;
    let key = format!("agents.{profile}.sandbox");
    let unix_scope = match fields.get("sandbox_unix_scope") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(scope)) => *scope,
        Some(other) => {
            return Some(Err(format!(
                "invalid config key agents.{profile}.sandbox_unix_scope: expected true or false, got {other}; the agent profile {profile} does not start"
            )));
        }
    };
    match sandbox {
        Value::String(name) if name == SANDBOX_PANEFLOW => {
            if SANDBOX_SUPPORTED_PLATFORMS.contains(&platform) {
                Some(Ok(ProfileSandbox { unix_scope }))
            } else {
                Some(Err(format!(
                    "invalid config key {key}: Sandbox '{SANDBOX_PANEFLOW}' is not available on {platform}; the agent profile {profile} does not start"
                )))
            }
        }
        Value::Null => None,
        other => Some(Err(format!(
            "invalid config key {key}: unknown sandbox {other}, expected \"{SANDBOX_PANEFLOW}\"; the agent profile {profile} does not start"
        ))),
    }
}

impl AgentSandboxProfiles {
    pub fn from_profiles_on(platform: &str, profiles: &BTreeMap<String, Value>) -> Self {
        Self {
            profiles: profiles
                .iter()
                .filter_map(|(name, value)| {
                    parse_profile(platform, name, value).map(|parsed| (name.clone(), parsed))
                })
                .collect(),
        }
    }

    pub fn errors(&self) -> Vec<String> {
        self.profiles
            .values()
            .filter_map(|parsed| parsed.as_ref().err().cloned())
            .collect()
    }

    pub fn sandbox_for<'a>(
        &self,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Option<Result<ProfileSandbox, String>> {
        names.into_iter().find_map(|wanted| {
            self.profiles
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
                .map(|(_, parsed)| parsed.clone())
        })
    }

    pub fn sandbox_matching(
        &self,
        mut answers_to: impl FnMut(&str) -> bool,
    ) -> Option<Result<ProfileSandbox, String>> {
        self.profiles
            .iter()
            .find(|(name, _)| answers_to(name))
            .map(|(_, parsed)| parsed.clone())
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

impl PaneFlowConfig {
    pub fn agent_sandbox_profiles(&self) -> AgentSandboxProfiles {
        self.agent_sandbox_profiles_on(std::env::consts::OS)
    }

    pub fn agent_sandbox_profiles_on(&self, platform: &str) -> AgentSandboxProfiles {
        self.agents
            .as_ref()
            .map(|agents| AgentSandboxProfiles::from_profiles_on(platform, &agents.profiles))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(json: &str) -> PaneFlowConfig {
        serde_json::from_str(json).expect("a config")
    }

    #[test]
    fn a_profile_opts_into_the_paneflow_sandbox_with_or_without_the_unix_scope() {
        let parsed = config(
            r#"{"agents": {"claude": {"sandbox": "paneflow"}, "codex": {"sandbox": "paneflow", "sandbox_unix_scope": true}, "pi": {"grants": []}}}"#,
        )
        .agent_sandbox_profiles_on("linux");
        assert_eq!(
            parsed.sandbox_for(["Claude"]),
            Some(Ok(ProfileSandbox { unix_scope: false }))
        );
        assert_eq!(
            parsed.sandbox_for(["missing", "codex"]),
            Some(Ok(ProfileSandbox { unix_scope: true }))
        );
        assert_eq!(parsed.sandbox_for(["pi"]), None);
        assert!(parsed.errors().is_empty());
    }

    #[test]
    fn an_unsupported_platform_names_the_platform_and_the_profile() {
        let parsed = config(r#"{"agents": {"claude": {"sandbox": "paneflow"}}}"#)
            .agent_sandbox_profiles_on("freebsd");
        let errors = parsed.errors();
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("Sandbox 'paneflow' is not available on freebsd"),
            "{}",
            errors[0]
        );
        assert!(errors[0].contains("agents.claude.sandbox"), "{}", errors[0]);
        assert!(matches!(parsed.sandbox_for(["claude"]), Some(Err(_))));
    }

    #[test]
    fn an_unknown_sandbox_is_an_error_never_an_unsandboxed_profile() {
        let parsed = config(r#"{"agents": {"claude": {"sandbox": "docker"}}}"#)
            .agent_sandbox_profiles_on("linux");
        assert!(
            matches!(parsed.sandbox_for(["claude"]), Some(Err(message)) if message.contains("unknown sandbox"))
        );
    }

    #[test]
    fn a_request_value_round_trips() {
        for sandbox in [
            ProfileSandbox { unix_scope: false },
            ProfileSandbox { unix_scope: true },
        ] {
            assert_eq!(
                ProfileSandbox::from_request(sandbox.request_value()),
                Some(sandbox)
            );
        }
        assert_eq!(ProfileSandbox::from_request("off"), None);
    }

    #[test]
    fn only_a_platform_whose_sandbox_was_validated_offers_it() {
        let asked = r#"{"agents": {"claude": {"sandbox": "paneflow"}}}"#;
        assert!(config(asked)
            .agent_sandbox_profiles_on("linux")
            .errors()
            .is_empty());
        for platform in ["macos", "windows"] {
            let errors = config(asked).agent_sandbox_profiles_on(platform).errors();
            assert_eq!(errors.len(), 1, "{platform}");
            assert!(
                errors[0].contains(&format!(
                    "Sandbox 'paneflow' is not available on {platform}"
                )),
                "{}",
                errors[0]
            );
        }
    }
}
