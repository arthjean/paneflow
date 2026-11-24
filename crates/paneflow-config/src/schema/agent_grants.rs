use std::collections::BTreeMap;

use serde_json::Value;

use super::PaneFlowConfig;

pub const AGENT_GRANT_RIGHTS: [&str; 3] = ["write", "spawn", "layout"];
pub const AGENT_GRANT_SCOPES: [&str; 2] = ["workspace", "instance"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileGrant {
    pub rights: Vec<String>,
    pub scope: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentGrantProfiles {
    profiles: BTreeMap<String, Result<Vec<ProfileGrant>, String>>,
}

fn profile_error(profile: &str, detail: &str) -> String {
    format!(
        "invalid config key agents.{profile}.grants: {detail}; no grant of the agent profile {profile} is stored"
    )
}

fn parse_grant(profile: &str, value: &Value) -> Result<ProfileGrant, String> {
    let Some(fields) = value.as_object() else {
        return Err(profile_error(
            profile,
            "each grant must be an object with rights and scope",
        ));
    };
    let Some(rights) = fields.get("rights").and_then(Value::as_array) else {
        return Err(profile_error(profile, "rights must be a list"));
    };
    let mut parsed = Vec::new();
    for right in rights {
        match right.as_str() {
            Some(name) if AGENT_GRANT_RIGHTS.contains(&name) => parsed.push(name.to_string()),
            Some(name) => {
                return Err(profile_error(
                    profile,
                    &format!(
                        "unknown right {name:?}, expected one of {}",
                        AGENT_GRANT_RIGHTS.join(", ")
                    ),
                ));
            }
            None => return Err(profile_error(profile, "a right must be a string")),
        }
    }
    let scope = match fields.get("scope") {
        None | Some(Value::Null) => "workspace".to_string(),
        Some(Value::String(scope)) if AGENT_GRANT_SCOPES.contains(&scope.as_str()) => scope.clone(),
        Some(other) => {
            return Err(profile_error(
                profile,
                &format!("unknown scope {other}, expected workspace or instance"),
            ));
        }
    };
    Ok(ProfileGrant {
        rights: parsed,
        scope,
    })
}

fn parse_profile(profile: &str, value: &Value) -> Option<Result<Vec<ProfileGrant>, String>> {
    let grants = value.as_object()?.get("grants")?;
    let Some(entries) = grants.as_array() else {
        return Some(Err(profile_error(profile, "grants must be a list")));
    };
    Some(
        entries
            .iter()
            .map(|entry| parse_grant(profile, entry))
            .collect(),
    )
}

impl AgentGrantProfiles {
    pub fn from_profiles(profiles: &BTreeMap<String, Value>) -> Self {
        Self {
            profiles: profiles
                .iter()
                .filter_map(|(name, value)| {
                    parse_profile(name, value).map(|parsed| (name.clone(), parsed))
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

    pub fn grants_for(&self, profile: &str) -> Option<(&str, &[ProfileGrant])> {
        self.profiles
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(profile))
            .and_then(|(name, parsed)| {
                parsed
                    .as_ref()
                    .ok()
                    .map(|grants| (name.as_str(), grants.as_slice()))
            })
    }

    pub fn usable(&self) -> impl Iterator<Item = (&str, &[ProfileGrant])> {
        self.profiles.iter().filter_map(|(name, parsed)| {
            parsed
                .as_ref()
                .ok()
                .map(|grants| (name.as_str(), grants.as_slice()))
        })
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

impl PaneFlowConfig {
    pub fn agent_grant_profiles(&self) -> AgentGrantProfiles {
        self.agents
            .as_ref()
            .map(|agents| AgentGrantProfiles::from_profiles(&agents.profiles))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles(json: &str) -> AgentGrantProfiles {
        let config: PaneFlowConfig = serde_json::from_str(json).expect("a config");
        config.agent_grant_profiles()
    }

    #[test]
    fn a_profile_lists_its_grants_beside_the_agents_settings() {
        let parsed = profiles(
            r#"{"agents": {"restore_conversations": false, "claude": {"grants": [{"rights": ["write", "spawn"], "scope": "instance"}, {"rights": ["layout"]}]}}}"#,
        );
        let (name, grants) = parsed.grants_for("Claude").expect("claude grants");
        assert_eq!(name, "claude");
        assert_eq!(
            grants,
            [
                ProfileGrant {
                    rights: vec!["write".into(), "spawn".into()],
                    scope: "instance".into()
                },
                ProfileGrant {
                    rights: vec!["layout".into()],
                    scope: "workspace".into()
                }
            ]
        );
        assert!(parsed.errors().is_empty());
        let config: PaneFlowConfig = serde_json::from_str(
            r#"{"agents": {"restore_conversations": false, "claude": {"grants": []}}}"#,
        )
        .expect("a config");
        assert_eq!(
            config.agents.as_ref().and_then(|a| a.restore_conversations),
            Some(false)
        );
    }

    #[test]
    fn an_unknown_right_names_the_profile_and_the_right_and_drops_the_profile() {
        let parsed = profiles(
            r#"{"agents": {"codex": {"grants": [{"rights": ["write"]}, {"rights": ["admin"]}]}}}"#,
        );
        assert!(parsed.grants_for("codex").is_none());
        let errors = parsed.errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("agents.codex.grants"), "{}", errors[0]);
        assert!(errors[0].contains("\"admin\""), "{}", errors[0]);
        assert!(errors[0].contains("codex"), "{}", errors[0]);
    }

    #[test]
    fn a_profile_without_grants_is_not_a_grant_profile() {
        assert!(profiles(r#"{"agents": {"claude": {"model": "x"}}}"#).is_empty());
        assert!(profiles(r#"{"agents": {"update_methods": {"codex": "bun"}}}"#).is_empty());
    }

    #[tracing_test::traced_test]
    #[test]
    fn loading_a_config_with_an_unknown_right_warns_naming_the_profile_and_the_right() {
        let config = crate::loader::try_parse_and_validate(
            r#"{"theme": "One Dark", "agents": {"claude": {"grants": [{"rights": ["root"]}]}}}"#,
        )
        .expect("the rest of the config still loads");
        assert_eq!(config.theme.as_deref(), Some("One Dark"));
        assert!(logs_contain(
            "invalid config key agents.claude.grants: unknown right \"root\""
        ));
    }
}
