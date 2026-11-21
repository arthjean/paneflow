use std::path::Path;

use serde_json::Value;

use crate::usage::accounts::ACCOUNT_LABEL_FILE_CAP;

pub const GROK_IDENTITY_FILE: &str = "auth.json";
const GROK_FIRST_PARTY_ISSUER: &str = "https://auth.x.ai";
const MAX_GROK_IDENTITY_CHARS: usize = 120;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrokIdentity {
    pub email: Option<String>,
    pub plan: Option<String>,
}

pub fn grok_account_identity(config_dir: &Path) -> GrokIdentity {
    paneflow_home::read_regular_capped(&config_dir.join(GROK_IDENTITY_FILE), ACCOUNT_LABEL_FILE_CAP)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .map(|root| grok_identity_of(&root))
        .unwrap_or_default()
}

pub fn grok_identity_of(root: &Value) -> GrokIdentity {
    let text = |entry: &Value, name: &str| {
        entry
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| {
                !text.is_empty()
                    && text.chars().count() <= MAX_GROK_IDENTITY_CHARS
                    && !text.chars().any(char::is_control)
            })
            .map(str::to_string)
    };
    let Some((_, entry)) = root.as_object().and_then(|logins| {
        logins
            .iter()
            .filter(|(_, entry)| entry.is_object())
            .max_by_key(|(name, entry)| {
                let issuer = text(entry, "oidc_issuer").unwrap_or_else(|| (*name).clone());
                (
                    issuer.starts_with(GROK_FIRST_PARTY_ISSUER),
                    text(entry, "email").is_some(),
                )
            })
    }) else {
        return GrokIdentity::default();
    };
    GrokIdentity {
        email: text(entry, "email").filter(|email| email.contains('@')),
        plan: text(entry, "subscription_tier")
            .as_deref()
            .and_then(grok_plan_label),
    }
}

pub fn grok_plan_label(tier: &str) -> Option<String> {
    let stem = tier
        .strip_prefix("SUBSCRIPTION_TIER_")
        .or_else(|| tier.strip_prefix("TIER_"))
        .unwrap_or(tier);
    if matches!(
        stem.to_ascii_lowercase().as_str(),
        "" | "none" | "free" | "unspecified"
    ) {
        return None;
    }
    let words = stem
        .split(['_', '-', ' '])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ");
    Some(words.replace("Super Grok", "SuperGrok"))
}
