use std::path::Path;

use anyhow::{Context, Result};
use paneflow_agent_config::jsonc;

pub(crate) const MAX_AGENT_CONFIG_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) fn read_agent_config(path: &Path) -> std::io::Result<Vec<u8>> {
    paneflow_home::read_regular_capped(path, MAX_AGENT_CONFIG_BYTES)
}

pub(crate) fn read_agent_config_string(path: &Path) -> std::io::Result<String> {
    paneflow_home::read_regular_string_capped(path, MAX_AGENT_CONFIG_BYTES)
}

pub fn read_json_or_default(path: &Path) -> Result<serde_json::Value> {
    read_config_or_default(path, has_jsonc_extension(path))
}

#[must_use]
pub fn has_jsonc_extension(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("jsonc")
}

pub fn read_config_or_default(path: &Path, jsonc: bool) -> Result<serde_json::Value> {
    match read_agent_config(path) {
        Ok(bytes) => parse_json_or_jsonc(path, &bytes, jsonc),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(serde_json::Value::Object(serde_json::Map::new()))
        }
        Err(e) => Err(e).with_context(|| format!("read {} failed", path.display())),
    }
}

fn parse_json_or_jsonc(path: &Path, bytes: &[u8], jsonc: bool) -> Result<serde_json::Value> {
    match serde_json::from_slice(bytes) {
        Ok(value) => Ok(value),
        Err(_json_error) if jsonc => {
            let text = std::str::from_utf8(bytes).with_context(|| {
                format!(
                    "{} is not valid UTF-8 JSONC - refusing to overwrite it; fix or remove it, then re-run",
                    path.display()
                )
            })?;
            jsonc::parse(text).with_context(|| {
                format!(
                    "{} is not valid JSONC - refusing to overwrite it; fix or remove it, then re-run",
                    path.display()
                )
            })
        }
        Err(json_error) => Err(json_error).with_context(|| {
            format!(
                "{} is not valid JSON - refusing to overwrite it; \
                 fix or remove it, then re-run",
                path.display()
            )
        }),
    }
}

#[must_use]
pub fn merged_entry(
    existing: Option<&serde_json::Value>,
    managed: &serde_json::Value,
    defaults: &serde_json::Value,
) -> serde_json::Value {
    let (Some(existing), Some(managed)) = (
        existing.and_then(serde_json::Value::as_object),
        managed.as_object(),
    ) else {
        let mut fresh = defaults.as_object().cloned().unwrap_or_default();
        if let Some(managed) = managed.as_object() {
            fresh.extend(managed.clone());
            return serde_json::Value::Object(fresh);
        }
        return managed.clone();
    };
    let mut merged = existing.clone();
    for (key, value) in defaults.as_object().into_iter().flatten() {
        merged.entry(key.clone()).or_insert_with(|| value.clone());
    }
    for (key, value) in managed {
        merged.insert(key.clone(), value.clone());
    }
    serde_json::Value::Object(merged)
}

pub fn merge_json_entry(
    root: &mut serde_json::Value,
    container_key: &str,
    entry_name: &str,
    managed: &serde_json::Value,
    defaults: &serde_json::Value,
) -> Result<bool> {
    let obj = root
        .as_object_mut()
        .context("config root is not a JSON object - refusing to overwrite")?;

    let container = obj
        .entry(container_key)
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let container = container.as_object_mut().with_context(|| {
        format!("config key `{container_key}` is not an object - refusing to overwrite")
    })?;

    let entry_value = merged_entry(container.get(entry_name), managed, defaults);
    if container.get(entry_name) == Some(&entry_value) {
        return Ok(false);
    }
    container.insert(entry_name.to_string(), entry_value);
    Ok(true)
}

pub fn remove_json_entry(
    root: &mut serde_json::Value,
    container_key: &str,
    entry_name: &str,
) -> bool {
    root.as_object_mut()
        .and_then(|obj| obj.get_mut(container_key))
        .and_then(serde_json::Value::as_object_mut)
        .is_some_and(|container| container.remove(entry_name).is_some())
}

pub fn json_to_bytes(root: &serde_json::Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut s = serde_json::to_string_pretty(root)?;
    s.push('\n');
    Ok(s.into_bytes())
}

pub fn read_toml_or_default(path: &Path) -> Result<toml_edit::DocumentMut> {
    match read_agent_config_string(path) {
        Ok(text) => text.parse::<toml_edit::DocumentMut>().with_context(|| {
            format!(
                "{} is not valid TOML - refusing to overwrite it; \
                 fix or remove it, then re-run",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml_edit::DocumentMut::new()),
        Err(e) => Err(e).with_context(|| format!("read {} failed", path.display())),
    }
}

pub struct TomlEntry<'a> {
    pub command: &'a str,
    pub args: &'a [&'a str],
    pub env_vars: &'a [&'a str],
}

pub fn upsert_toml_entry(
    doc: &mut toml_edit::DocumentMut,
    table_path: &str,
    name: &str,
    managed: &TomlEntry<'_>,
) -> Result<bool> {
    use toml_edit::{value, Array, Item, Table, Value};

    let before = doc.to_string();

    let parent = match doc.entry(table_path) {
        toml_edit::Entry::Vacant(v) => {
            let mut t = Table::new();
            t.set_implicit(true);
            v.insert(Item::Table(t))
        }
        toml_edit::Entry::Occupied(o) => o.into_mut(),
    };
    let parent = parent
        .as_table_mut()
        .with_context(|| format!("`{table_path}` is not a TOML table - refusing to overwrite"))?;

    let entry = parent
        .entry(name)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .with_context(|| {
            format!("`{table_path}.{name}` is not a TOML table - refusing to overwrite")
        })?;

    if entry.get("command").and_then(Item::as_str) != Some(managed.command) {
        entry.insert("command", value(managed.command));
    }
    let current_args: Option<Vec<&str>> = entry
        .get("args")
        .and_then(Item::as_array)
        .and_then(|array| array.iter().map(Value::as_str).collect());
    if current_args.as_deref() != Some(managed.args) {
        let mut arr = Array::new();
        for a in managed.args {
            arr.push(Value::from(*a));
        }
        entry.insert("args", value(arr));
    }
    if !managed.env_vars.is_empty() {
        match entry.get_mut("env_vars").and_then(Item::as_array_mut) {
            Some(existing) => {
                let missing: Vec<&str> = managed
                    .env_vars
                    .iter()
                    .copied()
                    .filter(|name| !existing.iter().any(|v| v.as_str() == Some(name)))
                    .collect();
                for name in missing {
                    existing.push(name);
                }
            }
            None => {
                let mut arr = Array::new();
                for name in managed.env_vars {
                    arr.push(*name);
                }
                entry.insert("env_vars", value(arr));
            }
        }
    }

    Ok(doc.to_string() != before)
}

pub fn remove_toml_entry(doc: &mut toml_edit::DocumentMut, table_path: &str, name: &str) -> bool {
    let Some(parent) = doc
        .get_mut(table_path)
        .and_then(toml_edit::Item::as_table_mut)
    else {
        return false;
    };
    parent.remove(name).is_some()
}

#[must_use]
pub fn toml_to_bytes(doc: &toml_edit::DocumentMut) -> Vec<u8> {
    doc.to_string().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(command: &str) -> TomlEntry<'_> {
        TomlEntry {
            command,
            args: &[],
            env_vars: &[],
        }
    }

    fn paneflow_entry() -> serde_json::Value {
        json!({ "command": "/data/bin/paneflow-mcp", "args": [] })
    }

    #[test]
    fn merge_json_inserts_without_touching_siblings() {
        let mut root = json!({
            "mcpServers": { "other": { "command": "x" } },
            "theme": "dark"
        });
        let changed = merge_json_entry(
            &mut root,
            "mcpServers",
            "paneflow",
            &paneflow_entry(),
            &json!({}),
        )
        .unwrap();
        assert!(changed);
        assert_eq!(root["mcpServers"]["other"]["command"], json!("x"));
        assert_eq!(root["theme"], json!("dark"));
        assert_eq!(root["mcpServers"]["paneflow"], paneflow_entry());
    }

    #[test]
    fn merge_json_is_noop_when_identical() {
        let mut root = json!({ "mcpServers": { "paneflow": paneflow_entry() } });
        let changed = merge_json_entry(
            &mut root,
            "mcpServers",
            "paneflow",
            &paneflow_entry(),
            &json!({}),
        )
        .unwrap();
        assert!(!changed, "identical entry must be a no-op");
    }

    #[test]
    fn merge_json_creates_container_when_absent() {
        let mut root = json!({});
        let changed =
            merge_json_entry(&mut root, "mcp", "paneflow", &paneflow_entry(), &json!({})).unwrap();
        assert!(changed);
        assert_eq!(root["mcp"]["paneflow"], paneflow_entry());
    }

    #[test]
    fn merge_json_errors_on_non_object_root() {
        let mut root = json!([1, 2, 3]);
        assert!(merge_json_entry(
            &mut root,
            "mcpServers",
            "paneflow",
            &paneflow_entry(),
            &json!({})
        )
        .is_err());
    }

    #[test]
    fn remove_json_only_removes_target() {
        let mut root = json!({
            "mcpServers": { "paneflow": paneflow_entry(), "other": { "command": "x" } }
        });
        assert!(remove_json_entry(&mut root, "mcpServers", "paneflow"));
        assert!(root["mcpServers"].get("paneflow").is_none());
        assert_eq!(root["mcpServers"]["other"]["command"], json!("x"));
        assert!(!remove_json_entry(&mut root, "mcpServers", "paneflow"));
    }

    #[test]
    fn read_json_missing_is_empty_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let v = read_json_or_default(&dir.path().join("nope.json")).unwrap();
        assert!(v.is_object() && v.as_object().unwrap().is_empty());
    }

    #[test]
    fn read_json_invalid_is_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("broken.json");
        std::fs::write(&p, b"{ not json").unwrap();
        let err = read_json_or_default(&p).unwrap_err();
        assert!(err.to_string().contains("not valid JSON"));
    }

    #[test]
    fn read_jsonc_allows_comments_and_trailing_commas() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("opencode.jsonc");
        std::fs::write(
            &p,
            br#"
{
  // user comment
  "mcp": {
    "paneflow": {
      "command": ["/p"], // trailing comment
    },
  },
  "url": "https://example.com/path//kept"
}
"#,
        )
        .unwrap();

        let v = read_json_or_default(&p).unwrap();
        assert_eq!(v["mcp"]["paneflow"]["command"], json!(["/p"]));
        assert_eq!(v["url"], json!("https://example.com/path//kept"));
    }

    #[test]
    fn upsert_toml_preserves_comments_and_siblings() {
        let input = "\
# top comment
[mcp_servers.existing]
command = \"keepme\"
args = []
";
        let mut doc = input.parse::<toml_edit::DocumentMut>().unwrap();
        let changed = upsert_toml_entry(
            &mut doc,
            "mcp_servers",
            "paneflow",
            &entry("/data/bin/paneflow-mcp"),
        )
        .unwrap();
        assert!(changed);
        let out = doc.to_string();
        assert!(out.contains("# top comment"), "comment preserved");
        assert!(out.contains("keepme"), "sibling entry preserved");
        assert!(out.contains("paneflow"), "new entry written");
        assert!(out.contains("/data/bin/paneflow-mcp"));
    }

    #[test]
    fn upsert_toml_is_noop_when_identical() {
        let mut doc = toml_edit::DocumentMut::new();
        upsert_toml_entry(&mut doc, "mcp_servers", "paneflow", &entry("/p")).unwrap();
        let changed = upsert_toml_entry(&mut doc, "mcp_servers", "paneflow", &entry("/p")).unwrap();
        assert!(!changed, "re-upsert of identical entry must be a no-op");
    }

    #[test]
    fn upsert_toml_updates_changed_path() {
        let mut doc = toml_edit::DocumentMut::new();
        upsert_toml_entry(&mut doc, "mcp_servers", "paneflow", &entry("/old")).unwrap();
        let changed =
            upsert_toml_entry(&mut doc, "mcp_servers", "paneflow", &entry("/new")).unwrap();
        assert!(changed);
        assert!(doc.to_string().contains("/new"));
        assert!(!doc.to_string().contains("/old"));
    }

    #[test]
    fn upsert_toml_repairs_args_that_hold_non_string_values() {
        let mut doc: toml_edit::DocumentMut =
            "[mcp_servers.paneflow]\ncommand = \"/p\"\nargs = [1]\n"
                .parse()
                .unwrap();
        let changed = upsert_toml_entry(&mut doc, "mcp_servers", "paneflow", &entry("/p")).unwrap();
        assert!(changed, "a repair must converge on the managed empty args");
        assert_eq!(
            doc["mcp_servers"]["paneflow"]["args"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn remove_toml_only_removes_target() {
        let input = "\
[mcp_servers.existing]
command = \"keepme\"

[mcp_servers.paneflow]
command = \"/p\"
args = []
";
        let mut doc = input.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(remove_toml_entry(&mut doc, "mcp_servers", "paneflow"));
        let out = doc.to_string();
        assert!(out.contains("keepme"), "sibling preserved");
        assert!(!out.contains("[mcp_servers.paneflow]"));
        assert!(!remove_toml_entry(&mut doc, "mcp_servers", "paneflow"));
    }

    #[test]
    fn read_toml_invalid_is_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("broken.toml");
        std::fs::write(&p, b"this = = invalid").unwrap();
        let err = read_toml_or_default(&p).unwrap_err();
        assert!(err.to_string().contains("not valid TOML"));
    }
}
