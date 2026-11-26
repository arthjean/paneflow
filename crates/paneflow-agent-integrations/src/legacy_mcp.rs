use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use paneflow_agent_config::jsonc;
use serde_json::Value;

use crate::{io, merge};

pub const LEGACY_ENTRY_KEYS: [&str; 3] = ["paneflow", "paneflow-dev", "paneflow-mcp"];
pub const LEGACY_BRIDGE_STEM: &str = "paneflow-mcp";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegacyAgent {
    ClaudeCode,
    Codex,
    Gemini,
    OpenCode,
    Fx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Json,
    Toml,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandShape {
    String,
    FirstOfArray,
}

impl LegacyAgent {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini CLI",
            Self::OpenCode => "OpenCode",
            Self::Fx => "fx",
        }
    }

    fn container(self) -> &'static str {
        match self {
            Self::ClaudeCode | Self::Gemini => "mcpServers",
            Self::Codex => "mcp_servers",
            Self::OpenCode | Self::Fx => "mcp",
        }
    }

    fn format(self) -> Format {
        match self {
            Self::Codex => Format::Toml,
            _ => Format::Json,
        }
    }

    fn command_shape(self) -> CommandShape {
        match self {
            Self::OpenCode | Self::Fx => CommandShape::FirstOfArray,
            _ => CommandShape::String,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyMcpFile {
    pub agent: LegacyAgent,
    pub path: PathBuf,
}

impl LegacyMcpFile {
    #[must_use]
    pub fn new(agent: LegacyAgent, path: PathBuf) -> Self {
        Self { agent, path }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovedEntry {
    pub agent: LegacyAgent,
    pub file: PathBuf,
    pub key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PurgeReport {
    pub removed: Vec<RemovedEntry>,
    pub warnings: Vec<String>,
}

impl PurgeReport {
    pub fn extend(&mut self, other: PurgeReport) {
        self.removed.extend(other.removed);
        self.warnings.extend(other.warnings);
    }

    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.removed
            .iter()
            .map(|entry| {
                format!(
                    "removed the {} `{}` MCP entry from {}",
                    entry.agent.label(),
                    entry.key,
                    entry.file.display()
                )
            })
            .collect()
    }
}

#[must_use]
pub fn default_home_files() -> Vec<LegacyMcpFile> {
    let home = dirs::home_dir();
    let mut files = Vec::new();
    if let Some(path) = claude_config() {
        files.push(LegacyMcpFile::new(LegacyAgent::ClaudeCode, path));
    }
    if let Some(path) = codex_config() {
        files.push(LegacyMcpFile::new(LegacyAgent::Codex, path));
    }
    if let Some(path) = gemini_config_in(home.as_deref()) {
        files.push(LegacyMcpFile::new(LegacyAgent::Gemini, path));
    }
    files.extend(
        opencode_configs()
            .into_iter()
            .map(|path| LegacyMcpFile::new(LegacyAgent::OpenCode, path)),
    );
    if let Some(path) = fx_config_in(home.as_deref()) {
        files.push(LegacyMcpFile::new(LegacyAgent::Fx, path));
    }
    files
}

#[must_use]
pub fn account_files(
    claude: Option<&paneflow_agent_config::ClaudePaths>,
    codex_home: Option<&Path>,
) -> Vec<LegacyMcpFile> {
    let mut files = Vec::new();
    if let Some(claude) = claude {
        files.push(LegacyMcpFile::new(
            LegacyAgent::ClaudeCode,
            claude.global_config.clone(),
        ));
    }
    if let Some(codex_home) = codex_home {
        files.push(LegacyMcpFile::new(
            LegacyAgent::Codex,
            codex_home.join("config.toml"),
        ));
    }
    files
}

#[must_use]
pub fn purge(files: &[LegacyMcpFile]) -> PurgeReport {
    let mut report = PurgeReport::default();
    let mut seen: Vec<&Path> = Vec::new();
    for file in files {
        if seen.contains(&file.path.as_path()) {
            continue;
        }
        seen.push(&file.path);
        match purge_file(file) {
            Ok(removed) => report
                .removed
                .extend(removed.into_iter().map(|key| RemovedEntry {
                    agent: file.agent,
                    file: file.path.clone(),
                    key,
                })),
            Err(error) => report.warnings.push(format!(
                "{}: the Paneflow MCP entry was left in place: {error:#}",
                file.path.display()
            )),
        }
    }
    report
}

fn purge_file(file: &LegacyMcpFile) -> Result<Vec<String>> {
    let Some(source) = read_source(&file.path)? else {
        return Ok(Vec::new());
    };
    if stale_keys(file.agent, &source)?.is_empty() {
        return Ok(Vec::new());
    }
    if std::fs::symlink_metadata(&file.path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(anyhow!(
            "it is a symbolic link, so Paneflow does not rewrite it; remove the entry by hand"
        ));
    }
    io::with_config_lock(&file.path, || {
        let Some(source) = read_source(&file.path)? else {
            return Ok(Vec::new());
        };
        let keys = stale_keys(file.agent, &source)?;
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let updated = match file.agent.format() {
            Format::Json => remove_json_keys(file.agent, &source, &keys)?,
            Format::Toml => remove_toml_keys(file.agent, &source, &keys)?,
        };
        io::write_if_changed_unlocked(&file.path, updated.as_bytes())?;
        Ok(keys)
    })
}

fn read_source(path: &Path) -> Result<Option<String>> {
    match merge::read_agent_config_string(path) {
        Ok(source) => Ok(Some(source)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {} failed", path.display())),
    }
}

fn stale_keys(agent: LegacyAgent, source: &str) -> Result<Vec<String>> {
    let entries: Vec<(String, Option<String>)> = match agent.format() {
        Format::Json => json_entries(agent, source)?,
        Format::Toml => toml_entries(agent, source)?,
    };
    Ok(entries
        .into_iter()
        .filter(|(key, command)| {
            LEGACY_ENTRY_KEYS.contains(&key.as_str())
                && command.as_deref().is_some_and(is_bridge_command)
        })
        .map(|(key, _)| key)
        .collect())
}

fn json_entries(agent: LegacyAgent, source: &str) -> Result<Vec<(String, Option<String>)>> {
    let root = jsonc::parse(source).context("the file is not valid JSON")?;
    let Some(container) = root.get(agent.container()) else {
        return Ok(Vec::new());
    };
    let Some(container) = container.as_object() else {
        return Ok(Vec::new());
    };
    Ok(container
        .iter()
        .map(|(key, entry)| (key.clone(), json_command(agent.command_shape(), entry)))
        .collect())
}

fn json_command(shape: CommandShape, entry: &Value) -> Option<String> {
    let command = entry.get("command")?;
    match shape {
        CommandShape::String => command.as_str(),
        CommandShape::FirstOfArray => command.as_array()?.first()?.as_str(),
    }
    .map(str::to_owned)
}

fn toml_entries(agent: LegacyAgent, source: &str) -> Result<Vec<(String, Option<String>)>> {
    let doc = source
        .parse::<toml_edit::DocumentMut>()
        .context("the file is not valid TOML")?;
    let Some(container) = doc
        .get(agent.container())
        .and_then(toml_edit::Item::as_table_like)
    else {
        return Ok(Vec::new());
    };
    Ok(container
        .iter()
        .map(|(key, entry)| {
            let command = entry
                .as_table_like()
                .and_then(|table| table.get("command"))
                .and_then(toml_edit::Item::as_str)
                .map(str::to_owned);
            (key.to_string(), command)
        })
        .collect())
}

fn remove_json_keys(agent: LegacyAgent, source: &str, keys: &[String]) -> Result<String> {
    let mut updated = source.to_string();
    for key in keys {
        if let Some(next) = jsonc::remove_entry(&updated, agent.container(), key)
            .with_context(|| format!("remove `{key}`"))?
        {
            updated = next;
        }
    }
    Ok(updated)
}

fn remove_toml_keys(agent: LegacyAgent, source: &str, keys: &[String]) -> Result<String> {
    let mut doc = source
        .parse::<toml_edit::DocumentMut>()
        .context("the file is not valid TOML")?;
    let container = doc
        .get_mut(agent.container())
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or_else(|| anyhow!("could not locate `{}`", agent.container()))?;
    for key in keys {
        if container.remove(key).is_none() {
            bail!("could not locate the `{key}` entry");
        }
    }
    Ok(doc.to_string())
}

#[must_use]
pub fn is_bridge_command(command: &str) -> bool {
    let name = command.rsplit(['/', '\\']).next().unwrap_or(command);
    let stem = name
        .len()
        .checked_sub(4)
        .filter(|at| name.is_char_boundary(*at) && name[*at..].eq_ignore_ascii_case(".exe"))
        .map_or(name, |at| &name[..at]);
    stem.eq_ignore_ascii_case(LEGACY_BRIDGE_STEM)
}

pub(crate) fn claude_config() -> Option<PathBuf> {
    paneflow_agent_config::ClaudePaths::current().map(|paths| paths.global_config)
}

pub(crate) fn codex_config() -> Option<PathBuf> {
    codex_config_from(dirs::home_dir(), std::env::var_os("CODEX_HOME"))
}

pub(crate) fn codex_config_from(
    home: Option<PathBuf>,
    codex_home: Option<OsString>,
) -> Option<PathBuf> {
    paneflow_agent_config::codex_home_from(codex_home, home).map(|h| h.join("config.toml"))
}

pub(crate) fn gemini_config_in(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|h| h.join(".gemini").join("settings.json"))
}

pub(crate) fn fx_config_in(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|h| h.join(".fx").join("mcp.json"))
}

pub(crate) fn opencode_configs() -> Vec<PathBuf> {
    opencode_configs_from(
        dirs::home_dir(),
        dirs::config_dir(),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("OPENCODE_CONFIG"),
        std::env::var_os("OPENCODE_CONFIG_DIR"),
    )
}

pub(crate) fn opencode_configs_from(
    home: Option<PathBuf>,
    _platform_config_dir: Option<PathBuf>,
    _xdg_config_home: Option<OsString>,
    opencode_config: Option<OsString>,
    opencode_config_dir: Option<OsString>,
) -> Vec<PathBuf> {
    if let Some(config) = opencode_config
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return vec![config];
    }

    let mut out = Vec::new();
    if let Some(dir) =
        paneflow_agent_config::absolute_env_dir("OPENCODE_CONFIG_DIR", opencode_config_dir)
    {
        push_opencode_files(&mut out, &dir);
        return out;
    }

    #[cfg(windows)]
    {
        if let Some(home) = home.clone() {
            push_opencode_files(&mut out, &home.join(".config").join("opencode"));
        }
        if let Some(dir) = _platform_config_dir {
            push_opencode_files(&mut out, &dir.join("opencode"));
        }
    }

    #[cfg(not(windows))]
    {
        if let Some(dir) = _xdg_config_home
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| home.map(|h| h.join(".config")))
        {
            push_opencode_files(&mut out, &dir.join("opencode"));
        }
    }

    out
}

fn push_opencode_files(out: &mut Vec<PathBuf>, dir: &Path) {
    out.push(dir.join("opencode.jsonc"));
    out.push(dir.join("opencode.json"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BRIDGE: &str = "/home/alice/.paneflow/bin/paneflow-mcp";

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        std::fs::write(path, text).expect("config");
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("config")
    }

    fn keys_of(text: &str, container: &str) -> Vec<String> {
        let root: Value = serde_json::from_str(text).expect("JSON");
        root[container]
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn top_keys(text: &str) -> Vec<String> {
        let root: Value = jsonc::parse(text).expect("JSON");
        root.as_object().expect("object").keys().cloned().collect()
    }

    #[test]
    fn the_three_claude_keys_go_and_every_other_key_keeps_its_value_and_order() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".claude.json");
        let original = json!({
            "numStartups": 12,
            "mcpServers": {
                "zeta": {"type": "stdio", "command": "zeta-server", "args": []},
                "paneflow": {"type": "stdio", "command": BRIDGE, "args": []},
                "paneflow-dev": {"type": "stdio", "command": "/home/alice/.paneflow-dev/bin/paneflow-mcp"},
                "alpha": {"type": "stdio", "command": "alpha-server"},
                "paneflow-mcp": {"type": "stdio", "command": BRIDGE}
            },
            "projects": {"/work": {"allowedTools": []}},
            "autoUpdates": false
        });
        write(&path, &serde_json::to_string_pretty(&original).unwrap());

        let report = purge(&[LegacyMcpFile::new(LegacyAgent::ClaudeCode, path.clone())]);

        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let keys: Vec<&str> = report.removed.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["paneflow", "paneflow-dev", "paneflow-mcp"]);
        let text = read(&path);
        assert_eq!(
            top_keys(&text),
            ["numStartups", "mcpServers", "projects", "autoUpdates"]
        );
        assert_eq!(keys_of(&text, "mcpServers"), ["zeta", "alpha"]);
        let root: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(root["mcpServers"]["zeta"], original["mcpServers"]["zeta"]);
        assert_eq!(root["projects"], original["projects"]);
        assert_eq!(root["numStartups"], json!(12));
    }

    #[test]
    fn the_codex_table_goes_and_a_comment_above_another_table_stays() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let original = format!(
            "model = \"gpt-6\"\n\n[mcp_servers.paneflow]\ncommand = \"{BRIDGE}\"\nargs = []\nenv_vars = [\"PANEFLOW_HOME\"]\n\n[mcp_servers.paneflow.env]\nRUST_LOG = \"debug\"\n\n# my own server, keep it\n[mcp_servers.other]\ncommand = \"other\"\n"
        );
        write(&path, &original);

        let report = purge(&[LegacyMcpFile::new(LegacyAgent::Codex, path.clone())]);

        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.removed.len(), 1);
        let text = read(&path);
        assert!(
            text.contains("# my own server, keep it\n[mcp_servers.other]"),
            "{text}"
        );
        assert!(text.starts_with("model = \"gpt-6\"\n"), "{text}");
        assert!(!text.contains("paneflow"), "{text}");
        let doc = text.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(
            doc["mcp_servers"]["other"]["command"].as_str(),
            Some("other")
        );
    }

    #[test]
    fn a_codex_entry_written_as_an_inline_table_goes_and_is_reported_only_once_removed() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        write(
            &path,
            &format!(
                "model = \"gpt-6\"\nmcp_servers = {{ paneflow = {{ command = \"{BRIDGE}\" }}, other = {{ command = \"other\" }} }}\n"
            ),
        );
        let files = [LegacyMcpFile::new(LegacyAgent::Codex, path.clone())];

        let report = purge(&files);

        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.removed.len(), 1);
        let text = read(&path);
        assert!(!text.contains("paneflow"), "{text}");
        let doc = text.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(
            doc["mcp_servers"]["other"]["command"].as_str(),
            Some("other")
        );
        assert_eq!(purge(&files), PurgeReport::default());
    }

    #[test]
    fn the_gemini_entry_goes_from_jsonc_and_its_comments_stay() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let original = format!(
            "{{\n  // the theme I like\n  \"theme\": \"Dracula\",\n  \"mcpServers\": {{\n    /* mine */\n    \"mine\": {{ \"command\": \"mine\" }},\n    \"paneflow\": {{ \"command\": \"{BRIDGE}\", \"args\": [], \"trust\": true }},\n  }},\n}}\n"
        );
        write(&path, &original);

        let report = purge(&[LegacyMcpFile::new(LegacyAgent::Gemini, path.clone())]);

        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.removed.len(), 1);
        let text = read(&path);
        assert!(text.contains("// the theme I like"), "{text}");
        assert!(text.contains("/* mine */"), "{text}");
        assert!(!text.contains("paneflow"), "{text}");
        let root = jsonc::parse(&text).unwrap();
        assert_eq!(root["mcpServers"]["mine"]["command"], json!("mine"));
    }

    #[test]
    fn the_opencode_and_fx_entries_go_by_the_first_command_element() {
        let dir = tempfile::TempDir::new().unwrap();
        let opencode = dir.path().join("opencode").join("opencode.jsonc");
        write(
            &opencode,
            &format!(
                "{{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"mcp\": {{\n    \"paneflow\": {{ \"type\": \"local\", \"command\": [\"{BRIDGE}\"], \"enabled\": true }}\n  }}\n}}\n"
            ),
        );
        let fx = dir.path().join(".fx").join("mcp.json");
        write(
            &fx,
            r#"{"mcp":{"paneflow":{"type":"local","command":["C:\\Users\\alice\\.paneflow\\bin\\paneflow-mcp.exe"],"enabled":true},"keep":{"type":"local","command":["keep"]}}}"#,
        );

        let report = purge(&[
            LegacyMcpFile::new(LegacyAgent::OpenCode, opencode.clone()),
            LegacyMcpFile::new(LegacyAgent::Fx, fx.clone()),
        ]);

        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.removed.len(), 2);
        assert!(!read(&opencode).contains("paneflow"));
        assert!(read(&opencode).contains("$schema"));
        assert_eq!(keys_of(&read(&fx), "mcp"), ["keep"]);
    }

    #[test]
    fn a_user_entry_named_paneflow_that_runs_another_server_is_kept() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".claude.json");
        let original =
            r#"{"mcpServers":{"paneflow":{"type":"stdio","command":"/usr/bin/other-server"}}}"#;
        write(&path, original);

        let report = purge(&[LegacyMcpFile::new(LegacyAgent::ClaudeCode, path.clone())]);

        assert_eq!(report, PurgeReport::default());
        assert_eq!(read(&path), original);
    }

    #[test]
    fn a_malformed_file_is_left_unchanged_named_in_a_warning_and_the_others_are_purged() {
        let dir = tempfile::TempDir::new().unwrap();
        let broken = dir.path().join("broken.json");
        write(&broken, "{ \"mcpServers\": { \"paneflow\": ");
        let toml = dir.path().join("config.toml");
        write(&toml, "[mcp_servers.paneflow\ncommand = 1\n");
        let good = dir.path().join("good.json");
        write(
            &good,
            &format!(r#"{{"mcpServers":{{"paneflow":{{"command":"{BRIDGE}"}}}}}}"#),
        );

        let report = purge(&[
            LegacyMcpFile::new(LegacyAgent::ClaudeCode, broken.clone()),
            LegacyMcpFile::new(LegacyAgent::Codex, toml.clone()),
            LegacyMcpFile::new(LegacyAgent::ClaudeCode, good.clone()),
        ]);

        assert_eq!(read(&broken), "{ \"mcpServers\": { \"paneflow\": ");
        assert_eq!(read(&toml), "[mcp_servers.paneflow\ncommand = 1\n");
        assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
        assert!(report.warnings[0].contains(&broken.display().to_string()));
        assert!(report.warnings[1].contains(&toml.display().to_string()));
        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.removed[0].file, good);
        assert_eq!(keys_of(&read(&good), "mcpServers"), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_is_never_rewritten_and_a_warning_names_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("dotfiles").join("claude.json");
        let original = format!(r#"{{"mcpServers":{{"paneflow":{{"command":"{BRIDGE}"}}}}}}"#);
        write(&target, &original);
        let link = dir.path().join(".claude.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let report = purge(&[LegacyMcpFile::new(LegacyAgent::ClaudeCode, link.clone())]);

        assert!(report.removed.is_empty());
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains(&link.display().to_string()));
        assert!(report.warnings[0].contains("symbolic link"));
        assert_eq!(read(&target), original);
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn a_purged_config_is_not_rewritten_by_a_second_purge() {
        let dir = tempfile::TempDir::new().unwrap();
        let claude = dir.path().join(".claude.json");
        write(
            &claude,
            &format!(r#"{{"mcpServers":{{"paneflow":{{"command":"{BRIDGE}"}}}}}}"#),
        );
        let codex = dir.path().join("config.toml");
        write(
            &codex,
            &format!("[mcp_servers.paneflow]\ncommand = \"{BRIDGE}\"\n"),
        );
        let files = [
            LegacyMcpFile::new(LegacyAgent::ClaudeCode, claude.clone()),
            LegacyMcpFile::new(LegacyAgent::Codex, codex.clone()),
            LegacyMcpFile::new(LegacyAgent::Gemini, dir.path().join("absent.json")),
        ];
        assert_eq!(purge(&files).removed.len(), 2);
        for path in [&claude, &codex] {
            std::fs::remove_file(PathBuf::from(format!("{}.bak", path.display()))).unwrap();
        }
        let modified: Vec<_> = [&claude, &codex]
            .iter()
            .map(|path| std::fs::metadata(path).unwrap().modified().unwrap())
            .collect();

        let again = purge(&files);

        assert_eq!(again, PurgeReport::default());
        for (path, before) in [&claude, &codex].iter().zip(modified) {
            assert_eq!(std::fs::metadata(path).unwrap().modified().unwrap(), before);
            assert!(!PathBuf::from(format!("{}.bak", path.display())).exists());
        }
        assert!(!dir.path().join("absent.json").exists());
    }

    #[test]
    fn the_report_lists_each_removed_entry_with_its_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let claude = dir.path().join(".claude.json");
        write(
            &claude,
            &format!(
                r#"{{"mcpServers":{{"paneflow":{{"command":"{BRIDGE}"}},"paneflow-dev":{{"command":"{BRIDGE}"}}}}}}"#
            ),
        );
        let fx = dir.path().join("mcp.json");
        write(
            &fx,
            &format!(r#"{{"mcp":{{"paneflow":{{"command":["{BRIDGE}"]}}}}}}"#),
        );

        let report = purge(&[
            LegacyMcpFile::new(LegacyAgent::ClaudeCode, claude.clone()),
            LegacyMcpFile::new(LegacyAgent::Fx, fx.clone()),
        ]);

        assert_eq!(
            report.removed,
            [
                RemovedEntry {
                    agent: LegacyAgent::ClaudeCode,
                    file: claude.clone(),
                    key: "paneflow".into()
                },
                RemovedEntry {
                    agent: LegacyAgent::ClaudeCode,
                    file: claude.clone(),
                    key: "paneflow-dev".into()
                },
                RemovedEntry {
                    agent: LegacyAgent::Fx,
                    file: fx.clone(),
                    key: "paneflow".into()
                },
            ]
        );
        let lines = report.lines();
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0].contains("`paneflow`") && lines[0].contains(&claude.display().to_string())
        );
        assert!(lines[2].contains("fx") && lines[2].contains(&fx.display().to_string()));
    }

    #[test]
    fn the_bridge_is_recognized_by_its_file_stem_on_every_platform() {
        for command in [
            "paneflow-mcp",
            "/home/alice/.paneflow/bin/paneflow-mcp",
            "C:\\Users\\alice\\.paneflow\\bin\\paneflow-mcp.exe",
            "C:\\Users\\alice\\.paneflow\\bin\\PANEFLOW-MCP.EXE",
        ] {
            assert!(is_bridge_command(command), "{command}");
        }
        for command in [
            "/usr/bin/other-server",
            "paneflow",
            "/opt/paneflow-mcp-proxy",
            "/opt/paneflow-mcp/run",
            "",
        ] {
            assert!(!is_bridge_command(command), "{command}");
        }
    }

    #[test]
    fn account_files_name_only_the_directories_the_account_moves() {
        let claude = paneflow_agent_config::ClaudePaths {
            config_dir: PathBuf::from("/accounts/work/claude"),
            global_config: PathBuf::from("/accounts/work/claude/.claude.json"),
        };
        assert_eq!(
            account_files(Some(&claude), Some(Path::new("/accounts/work/codex"))),
            [
                LegacyMcpFile::new(
                    LegacyAgent::ClaudeCode,
                    PathBuf::from("/accounts/work/claude/.claude.json")
                ),
                LegacyMcpFile::new(
                    LegacyAgent::Codex,
                    PathBuf::from("/accounts/work/codex/config.toml")
                ),
            ]
        );
        assert!(account_files(None, None).is_empty());
    }

    #[test]
    fn codex_config_honors_codex_home() {
        let codex_home = std::env::temp_dir().join("codex-home");
        assert_eq!(
            codex_config_from(
                Some(PathBuf::from("/home/alice")),
                Some(codex_home.clone().into_os_string())
            )
            .unwrap(),
            codex_home.join("config.toml")
        );
        assert_eq!(
            codex_config_from(
                Some(PathBuf::from("/home/alice")),
                Some(OsString::from("relative"))
            )
            .unwrap(),
            PathBuf::from("/home/alice")
                .join(".codex")
                .join("config.toml")
        );
    }

    #[test]
    fn opencode_config_candidates_prefer_custom_path() {
        assert_eq!(
            opencode_configs_from(
                Some(PathBuf::from("/home/alice")),
                None,
                None,
                Some(OsString::from("/tmp/opencode.jsonc")),
                None,
            ),
            vec![PathBuf::from("/tmp/opencode.jsonc")]
        );
    }

    #[test]
    fn opencode_config_dir_holds_the_config_files_directly() {
        let dir = std::env::temp_dir().join("opencode-config");
        assert_eq!(
            opencode_configs_from(
                Some(PathBuf::from("/home/alice")),
                None,
                None,
                None,
                Some(dir.clone().into_os_string()),
            ),
            vec![dir.join("opencode.jsonc"), dir.join("opencode.json")]
        );
    }

    #[test]
    fn a_relative_opencode_config_dir_is_ignored() {
        let home = std::env::temp_dir().join("alice");
        let defaults = opencode_configs_from(Some(home.clone()), None, None, None, None);
        assert_eq!(
            opencode_configs_from(
                Some(home),
                None,
                None,
                None,
                Some(OsString::from("relative/opencode")),
            ),
            defaults
        );
    }
}
