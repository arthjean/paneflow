use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageServerEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initialization_options: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LanguageServerDefinition {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub extensions: Vec<String>,
    pub language_id: String,
    pub enabled: bool,
    pub initialization_options: Option<Value>,
    pub settings: Value,
}

impl LanguageServerDefinition {
    pub fn handles_extension(&self, extension: &str) -> bool {
        self.extensions.iter().any(|known| {
            known
                .trim_start_matches('.')
                .eq_ignore_ascii_case(extension)
        })
    }
}

pub fn builtin_language_servers() -> Vec<LanguageServerDefinition> {
    vec![
        LanguageServerDefinition {
            id: "rust-analyzer".to_string(),
            command: "rust-analyzer".to_string(),
            args: Vec::new(),
            extensions: vec!["rs".to_string()],
            language_id: "rust".to_string(),
            enabled: true,
            initialization_options: None,
            settings: Value::Null,
        },
        LanguageServerDefinition {
            id: "zls".to_string(),
            command: "zls".to_string(),
            args: Vec::new(),
            extensions: vec!["zig".to_string(), "zon".to_string()],
            language_id: "zig".to_string(),
            enabled: true,
            initialization_options: None,
            settings: Value::Null,
        },
    ]
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct LanguageServersConfig(pub BTreeMap<String, LanguageServerEntry>);

impl LanguageServersConfig {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn catalog(&self) -> Vec<LanguageServerDefinition> {
        let mut catalog = builtin_language_servers();
        for (id, entry) in &self.0 {
            match catalog.iter_mut().find(|known| &known.id == id) {
                Some(known) => apply_entry(known, entry),
                None => {
                    if let Some(definition) = definition_from_entry(id, entry) {
                        catalog.push(definition);
                    }
                }
            }
        }
        catalog
    }

    pub fn server_for_extension(&self, extension: &str) -> Option<LanguageServerDefinition> {
        self.catalog()
            .into_iter()
            .find(|definition| definition.enabled && definition.handles_extension(extension))
    }
}

fn apply_entry(definition: &mut LanguageServerDefinition, entry: &LanguageServerEntry) {
    if let Some(command) = &entry.command {
        definition.command.clone_from(command);
    }
    if let Some(args) = &entry.args {
        definition.args.clone_from(args);
    }
    if let Some(extensions) = &entry.extensions {
        definition.extensions.clone_from(extensions);
    }
    if let Some(language_id) = &entry.language_id {
        definition.language_id.clone_from(language_id);
    }
    if let Some(enabled) = entry.enabled {
        definition.enabled = enabled;
    }
    if let Some(options) = &entry.initialization_options {
        definition.initialization_options = Some(options.clone());
    }
    if let Some(settings) = &entry.settings {
        definition.settings = settings.clone();
    }
}

fn definition_from_entry(
    id: &str,
    entry: &LanguageServerEntry,
) -> Option<LanguageServerDefinition> {
    let mut definition = LanguageServerDefinition {
        id: id.to_string(),
        command: entry.command.clone()?,
        args: Vec::new(),
        extensions: entry.extensions.clone()?,
        language_id: entry.language_id.clone()?,
        enabled: true,
        initialization_options: None,
        settings: Value::Null,
    };
    apply_entry(&mut definition, entry);
    Some(definition)
}

fn is_builtin(id: &str) -> bool {
    builtin_language_servers()
        .iter()
        .any(|known| known.id == id)
}

impl<'de> Deserialize<'de> for LanguageServersConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let Value::Object(entries) = value else {
            tracing::warn!("ignoring config field `language_servers`: expected an object");
            return Ok(Self::default());
        };
        let mut servers = BTreeMap::new();
        for (id, raw) in entries {
            match serde_json::from_value::<LanguageServerEntry>(raw) {
                Ok(entry) if is_builtin(&id) || definition_from_entry(&id, &entry).is_some() => {
                    servers.insert(id, entry);
                }
                Ok(_) => tracing::warn!(
                    "skipping language server `{id}`: an entry that is not built in needs `command`, `extensions` and `language_id`"
                ),
                Err(error) => tracing::warn!("skipping invalid language server `{id}`: {error}"),
            }
        }
        Ok(Self(servers))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> LanguageServersConfig {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn with_no_user_entry_the_catalog_holds_rust_analyzer_and_zls() {
        let catalog = LanguageServersConfig::default().catalog();
        let rust = catalog
            .iter()
            .find(|server| server.id == "rust-analyzer")
            .unwrap();
        assert!(rust.handles_extension("rs"));
        assert_eq!(rust.language_id, "rust");
        assert_eq!(rust.command, "rust-analyzer");
        let zig = catalog.iter().find(|server| server.id == "zls").unwrap();
        assert!(zig.handles_extension("zig"));
        assert_eq!(zig.language_id, "zig");
    }

    #[test]
    fn a_user_entry_with_a_built_in_id_replaces_the_fields_it_sets() {
        let config = parse(json!({
            "rust-analyzer": {"command": "/opt/ra/bin/rust-analyzer", "settings": {"cargo": {"features": "all"}}}
        }));
        let rust = config.server_for_extension("rs").unwrap();
        assert_eq!(rust.id, "rust-analyzer");
        assert_eq!(rust.command, "/opt/ra/bin/rust-analyzer");
        assert_eq!(rust.settings, json!({"cargo": {"features": "all"}}));
        assert_eq!(rust.language_id, "rust");
    }

    #[test]
    fn every_entry_field_is_parsed_and_a_new_server_joins_the_catalog() {
        let config = parse(json!({
            "gopls": {
                "command": "gopls",
                "args": ["serve"],
                "extensions": ["go"],
                "language_id": "go",
                "enabled": true,
                "initialization_options": {"a": 1},
                "settings": {"semanticTokens": true}
            }
        }));
        let go = config.server_for_extension("go").unwrap();
        assert_eq!(go.args, vec!["serve"]);
        assert_eq!(go.initialization_options, Some(json!({"a": 1})));
        assert_eq!(go.settings, json!({"semanticTokens": true}));
    }

    #[test]
    fn a_disabled_server_matches_no_extension() {
        let config = parse(json!({"zls": {"enabled": false}}));
        assert!(config.server_for_extension("zig").is_none());
    }

    #[test]
    fn a_malformed_entry_is_dropped_and_the_others_still_load() {
        let config = parse(json!({
            "broken": {"command": 7},
            "unknown-field": {"command": "x", "extensions": ["x"], "language_id": "x", "colour": 1},
            "incomplete": {"command": "pyright"},
            "gopls": {"command": "gopls", "extensions": ["go"], "language_id": "go"}
        }));
        assert_eq!(config.0.keys().collect::<Vec<_>>(), vec!["gopls"]);
        assert!(config.server_for_extension("go").is_some());
        assert!(config.server_for_extension("rs").is_some());
    }
}
