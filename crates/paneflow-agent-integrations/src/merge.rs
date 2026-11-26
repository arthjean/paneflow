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

fn has_jsonc_extension(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("jsonc")
}

fn read_config_or_default(path: &Path, jsonc: bool) -> Result<serde_json::Value> {
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

pub fn json_to_bytes(root: &serde_json::Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut s = serde_json::to_string_pretty(root)?;
    s.push('\n');
    Ok(s.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
