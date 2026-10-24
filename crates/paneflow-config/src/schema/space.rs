use super::identity::SpaceId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_SPACES: usize = 9;

pub const MAX_SPACE_NAME_CHARS: usize = 32;

pub const DEFAULT_SPACE_NAME: &str = "Default";

pub const MAX_SPACE_AVATAR_CHARS: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpaceColor {
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpaceConfig {
    pub id: SpaceId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<SpaceColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceNameError {
    Required,
    TooLong,
    Reserved,
    AlreadyUsed,
}

impl SpaceNameError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Required => "Name required",
            Self::TooLong => "32 characters maximum",
            Self::Reserved => "Default is reserved",
            Self::AlreadyUsed => "Name already used",
        }
    }
}

pub fn validate_space_name<'a>(
    name: &str,
    taken: impl IntoIterator<Item = &'a str>,
) -> Result<String, SpaceNameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(SpaceNameError::Required);
    }
    if name.chars().count() > MAX_SPACE_NAME_CHARS {
        return Err(SpaceNameError::TooLong);
    }
    let folded = name.to_lowercase();
    if folded == DEFAULT_SPACE_NAME.to_lowercase() {
        return Err(SpaceNameError::Reserved);
    }
    if taken
        .into_iter()
        .any(|existing| existing.trim().to_lowercase() == folded)
    {
        return Err(SpaceNameError::AlreadyUsed);
    }
    Ok(name.to_string())
}

pub fn is_valid_space_avatar(file_name: &str) -> bool {
    let Some(stem) = file_name.strip_suffix(".png") else {
        return false;
    };
    file_name.len() <= MAX_SPACE_AVATAR_CHARS
        && stem
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
}

pub(super) fn lenient_spaces<'de, D>(d: D) -> Result<Vec<SpaceConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(d)?;
    let Some(items) = value.as_array() else {
        tracing::warn!("ignoring config field `spaces`: expected an array");
        return Ok(Vec::new());
    };

    let mut spaces: Vec<SpaceConfig> = Vec::new();
    for (index, raw) in items.iter().enumerate() {
        match parse_space(raw, &spaces) {
            Ok(space) => spaces.push(space),
            Err(reason) => {
                tracing::warn!("skipping space entry at index {index}: {reason}");
            }
        }
    }
    Ok(spaces)
}

fn parse_space(raw: &serde_json::Value, accepted: &[SpaceConfig]) -> Result<SpaceConfig, String> {
    if accepted.len() >= MAX_SPACES {
        return Err(format!("at most {MAX_SPACES} spaces are allowed"));
    }
    let id = raw
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing `id`".to_string())?;
    let id = SpaceId::parse(id).map_err(|error| error.to_string())?;
    if accepted.iter().any(|space| space.id == id) {
        return Err(format!("duplicate id {id}"));
    }
    let name = raw
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let name = validate_space_name(name, accepted.iter().map(|space| space.name.as_str()))
        .map_err(|error| format!("invalid name {name:?}: {}", error.message()))?;
    let color = raw
        .get("color")
        .and_then(|color| serde_json::from_value::<SpaceColor>(color.clone()).ok());
    let avatar = raw
        .get("avatar")
        .and_then(serde_json::Value::as_str)
        .and_then(|avatar| {
            if is_valid_space_avatar(avatar) {
                Some(avatar.to_string())
            } else {
                tracing::warn!("space {id}: ignoring avatar {avatar:?}: expected a PNG file name");
                None
            }
        });
    let env = raw
        .get("env")
        .and_then(serde_json::Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(key, value)| match value.as_str() {
                    Some(value) => Some((key.clone(), value.to_string())),
                    None => {
                        tracing::warn!("space {id}: ignoring env {key:?}: expected a string");
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(SpaceConfig {
        id,
        name,
        color,
        avatar,
        env,
    })
}
