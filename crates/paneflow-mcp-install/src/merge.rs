//! No-clobber config-merge primitives (EP-002 US-006).
//!
//! Two formats, two rules:
//! - **JSON** (Claude Code, Gemini) is merged via `serde_json::Value`, so
//!   unknown keys and sibling MCP servers are preserved semantically.
//! - **JSONC** (opencode) is parsed here and surgically edited through
//!   `paneflow-agent-config`, preserving comments and surrounding formatting.
//! - **TOML** (Codex) is edited via `toml_edit::DocumentMut`, which
//!   preserves comments and key order. Only `[<table>.paneflow]` is
//!   upserted.
//!
//! Both `read_*_or_default` helpers treat a **missing** file as an empty
//! skeleton (so a fresh install creates it) but a **present-but-invalid**
//! file as an error (so we never overwrite a config we could not parse -
//! the user repairs it by hand). This is the no-clobber guarantee.

use std::path::Path;

use anyhow::{Context, Result};
use paneflow_agent_config::jsonc;

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// Read + parse a JSON config. Missing file → empty object skeleton.
/// Present but unparseable → `Err` (caller must abort, never clobber).
pub fn read_json_or_default(path: &Path) -> Result<serde_json::Value> {
    match std::fs::read(path) {
        Ok(bytes) => parse_json_or_jsonc(path, &bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(serde_json::Value::Object(serde_json::Map::new()))
        }
        Err(e) => Err(e).with_context(|| format!("read {} failed", path.display())),
    }
}

fn parse_json_or_jsonc(path: &Path, bytes: &[u8]) -> Result<serde_json::Value> {
    match serde_json::from_slice(bytes) {
        Ok(value) => Ok(value),
        Err(_json_error) if path.extension().and_then(|e| e.to_str()) == Some("jsonc") => {
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

/// Upsert `root[container_key][entry_name] = entry_value`, creating the
/// container object if needed. Returns `true` iff the document changed
/// (the entry was absent or differed); `false` is a no-op (idempotent).
///
/// Sibling entries under `container_key`, and every other top-level key,
/// are left untouched. Errors only if `root` (or an existing
/// `container_key`) is present but not a JSON object - overwriting a
/// non-object there would be a clobber.
pub fn merge_json_entry(
    root: &mut serde_json::Value,
    container_key: &str,
    entry_name: &str,
    entry_value: serde_json::Value,
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

    if container.get(entry_name) == Some(&entry_value) {
        return Ok(false);
    }
    container.insert(entry_name.to_string(), entry_value);
    Ok(true)
}

/// Remove `root[container_key][entry_name]`. Returns `true` iff something
/// was removed. Leaves siblings and the container itself in place.
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

/// Serialize a JSON config back to bytes: pretty-printed, trailing newline
/// (matches what editors and `claude mcp add` leave behind).
///
/// US-038: returns `Result` and propagates a serialization error instead of
/// the old `unwrap_or_else(|_| "{}")` fallback, which would have silently
/// written an empty object over the user's real MCP servers (a no-clobber
/// violation) if a parsed `Value` ever failed to re-serialize.
pub fn json_to_bytes(root: &serde_json::Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut s = serde_json::to_string_pretty(root)?;
    s.push('\n');
    Ok(s.into_bytes())
}

// ---------------------------------------------------------------------------
// TOML
// ---------------------------------------------------------------------------

/// Read + parse a TOML config. Missing file → empty document. Present but
/// unparseable → `Err` (no-clobber).
pub fn read_toml_or_default(path: &Path) -> Result<toml_edit::DocumentMut> {
    match std::fs::read_to_string(path) {
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

/// Upsert `[<table_path>.<name>]` with `command = <command>` and
/// `args = [...]`, preserving the rest of the document (comments, key
/// order, sibling tables). Returns `true` iff the serialized document
/// changed.
pub fn upsert_toml_entry(
    doc: &mut toml_edit::DocumentMut,
    table_path: &str,
    name: &str,
    command: &str,
    args: &[&str],
) -> Result<bool> {
    use toml_edit::{value, Array, Item, Table, Value};

    let before = doc.to_string();

    // Auto-vivify the parent table only when absent; an existing parent is
    // reused untouched so we never strip a user's `[mcp_servers]` header or
    // its other entries.
    let parent = match doc.entry(table_path) {
        toml_edit::Entry::Vacant(v) => {
            let mut t = Table::new();
            // Render as `[mcp_servers.paneflow]` rather than emitting a
            // bare `[mcp_servers]` header for a freshly created parent.
            t.set_implicit(true);
            v.insert(Item::Table(t))
        }
        toml_edit::Entry::Occupied(o) => o.into_mut(),
    };
    let parent = parent
        .as_table_mut()
        .with_context(|| format!("`{table_path}` is not a TOML table - refusing to overwrite"))?;

    let mut entry = Table::new();
    entry["command"] = value(command);
    let mut arr = Array::new();
    for a in args {
        arr.push(Value::from(*a));
    }
    entry["args"] = value(arr);
    parent[name] = Item::Table(entry);

    Ok(doc.to_string() != before)
}

/// Remove `[<table_path>.<name>]`. Returns `true` iff the document changed.
pub fn remove_toml_entry(doc: &mut toml_edit::DocumentMut, table_path: &str, name: &str) -> bool {
    let Some(parent) = doc
        .get_mut(table_path)
        .and_then(toml_edit::Item::as_table_mut)
    else {
        return false;
    };
    parent.remove(name).is_some()
}

/// Serialize a TOML document back to bytes.
#[must_use]
pub fn toml_to_bytes(doc: &toml_edit::DocumentMut) -> Vec<u8> {
    doc.to_string().into_bytes()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn paneflow_entry() -> serde_json::Value {
        json!({ "command": "/data/bin/paneflow-mcp", "args": [] })
    }

    #[test]
    fn merge_json_inserts_without_touching_siblings() {
        let mut root = json!({
            "mcpServers": { "other": { "command": "x" } },
            "theme": "dark"
        });
        let changed =
            merge_json_entry(&mut root, "mcpServers", "paneflow", paneflow_entry()).unwrap();
        assert!(changed);
        // Sibling server and unrelated top-level key preserved.
        assert_eq!(root["mcpServers"]["other"]["command"], json!("x"));
        assert_eq!(root["theme"], json!("dark"));
        assert_eq!(root["mcpServers"]["paneflow"], paneflow_entry());
    }

    #[test]
    fn merge_json_is_noop_when_identical() {
        let mut root = json!({ "mcpServers": { "paneflow": paneflow_entry() } });
        let changed =
            merge_json_entry(&mut root, "mcpServers", "paneflow", paneflow_entry()).unwrap();
        assert!(!changed, "identical entry must be a no-op");
    }

    #[test]
    fn merge_json_creates_container_when_absent() {
        let mut root = json!({});
        let changed = merge_json_entry(&mut root, "mcp", "paneflow", paneflow_entry()).unwrap();
        assert!(changed);
        assert_eq!(root["mcp"]["paneflow"], paneflow_entry());
    }

    #[test]
    fn merge_json_errors_on_non_object_root() {
        let mut root = json!([1, 2, 3]);
        assert!(merge_json_entry(&mut root, "mcpServers", "paneflow", paneflow_entry()).is_err());
    }

    #[test]
    fn remove_json_only_removes_target() {
        let mut root = json!({
            "mcpServers": { "paneflow": paneflow_entry(), "other": { "command": "x" } }
        });
        assert!(remove_json_entry(&mut root, "mcpServers", "paneflow"));
        assert!(root["mcpServers"].get("paneflow").is_none());
        assert_eq!(root["mcpServers"]["other"]["command"], json!("x"));
        // Removing again is a no-op.
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
            "/data/bin/paneflow-mcp",
            &[],
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
        upsert_toml_entry(&mut doc, "mcp_servers", "paneflow", "/p", &[]).unwrap();
        let changed = upsert_toml_entry(&mut doc, "mcp_servers", "paneflow", "/p", &[]).unwrap();
        assert!(!changed, "re-upsert of identical entry must be a no-op");
    }

    #[test]
    fn upsert_toml_updates_changed_path() {
        let mut doc = toml_edit::DocumentMut::new();
        upsert_toml_entry(&mut doc, "mcp_servers", "paneflow", "/old", &[]).unwrap();
        let changed = upsert_toml_entry(&mut doc, "mcp_servers", "paneflow", "/new", &[]).unwrap();
        assert!(changed);
        assert!(doc.to_string().contains("/new"));
        assert!(!doc.to_string().contains("/old"));
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
