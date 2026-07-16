// US-017: JSON config loader with validation

use crate::schema::{CommandDefinition, LayoutNode, PaneFlowConfig};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::warn;

/// Application directory namespace. Switches to `paneflow-dev` in debug
/// builds so a `cargo run` instance (typical dev workflow) never reads
/// or writes the same config / session file as the user's installed
/// `/usr/bin/paneflow`. Mirrors `paneflow_app::runtime_paths::APP_SUBDIR`
/// so per-build isolation is consistent across every persistence
/// surface (config, session, threads, sockets, caches).
pub const APP_SUBDIR: &str = if cfg!(debug_assertions) {
    "paneflow-dev"
} else {
    "paneflow"
};

/// Hard cap on the size of any config file we will read into memory.
/// Real configs are kilobytes; this guards against a runaway or hostile
/// file on disk causing the GPUI main thread to stall while
/// `read_to_string` allocates. 1 MiB is roughly two orders of magnitude
/// above any plausible config.
const MAX_CONFIG_SIZE_BYTES: u64 = 1 << 20;

/// Errors that can occur when loading configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    IoError(#[from] std::io::Error),
    #[error("invalid JSON in config file: {0}")]
    ParseError(#[from] serde_json::Error),
}

/// Returns the platform-appropriate config file path.
///
/// - Linux: `$XDG_CONFIG_HOME/paneflow/paneflow.json`
/// - macOS: `~/Library/Application Support/paneflow/paneflow.json`
/// - Windows: `%APPDATA%\paneflow\paneflow.json`
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join(APP_SUBDIR).join("paneflow.json"))
}

/// Returns the platform-appropriate session file path.
///
/// - Linux: `$XDG_CACHE_HOME/paneflow/session.json`
/// - macOS: `~/Library/Caches/paneflow/session.json`
///
/// The filename is namespaced per build profile (`session-dev.json` in
/// debug builds) so a `cargo run` instance and an installed release
/// instance never overwrite each other's persisted layout on quit.
pub fn session_path() -> Option<PathBuf> {
    let filename = if cfg!(debug_assertions) {
        "session-dev.json"
    } else {
        "session.json"
    };
    dirs::cache_dir().map(|dir| dir.join(APP_SUBDIR).join(filename))
}

/// Load the PaneFlow configuration from the default platform path.
///
/// - If the config file does not exist, returns `PaneFlowConfig::default()`.
/// - If the file contains invalid JSON, logs a warning and returns defaults.
/// - Individual command entries with validation errors are skipped with warnings.
pub fn load_config() -> PaneFlowConfig {
    let Some(path) = config_path() else {
        warn!("could not determine config directory; using defaults");
        return PaneFlowConfig::default();
    };

    load_config_from_path(&path)
}

/// US-029: outcome of reading the config file with the oversize guard applied.
pub enum ConfigRead {
    /// File read successfully.
    Contents(String),
    /// File does not exist (normal at cold start; a deletion at runtime).
    Absent,
    /// File exists but was rejected - over the size cap or unreadable. The
    /// reason was already logged.
    Rejected,
}

/// US-029: read the config file to a string with the oversize guard applied
/// BEFORE allocating (cheap `metadata` stat first). Shared by the cold loader
/// and the hot watcher reload so the DoS guard can never be missing on either
/// path - the watcher's `attempt_reload` previously read with no cap at all.
pub fn read_config_string(path: &Path) -> ConfigRead {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ConfigRead::Absent,
        Err(e) => {
            warn!("failed to open config file {}: {e}", path.display());
            return ConfigRead::Rejected;
        }
    };

    match file.metadata() {
        // U-028: a FIFO/character device reports len 0 (passing the size cap)
        // but `read_to_string` would then block indefinitely or stream unbounded
        // bytes. Metadata is taken from the already-open file, so a path swap
        // between stat and read cannot bypass the guard.
        Ok(meta) if !meta.file_type().is_file() => {
            warn!(
                "config file {} is not a regular file; using defaults",
                path.display()
            );
            ConfigRead::Rejected
        }
        Ok(meta) if meta.len() > MAX_CONFIG_SIZE_BYTES => {
            warn!(
                "config file {} is {} bytes (over {}-byte cap); using defaults",
                path.display(),
                meta.len(),
                MAX_CONFIG_SIZE_BYTES
            );
            ConfigRead::Rejected
        }
        Ok(_) => {
            let mut contents = String::new();
            match file
                .take(MAX_CONFIG_SIZE_BYTES + 1)
                .read_to_string(&mut contents)
            {
                Ok(_) if contents.len() as u64 <= MAX_CONFIG_SIZE_BYTES => {
                    ConfigRead::Contents(contents)
                }
                Ok(_) => {
                    warn!(
                        "config file {} exceeded the {}-byte cap during read; using defaults",
                        path.display(),
                        MAX_CONFIG_SIZE_BYTES
                    );
                    ConfigRead::Rejected
                }
                Err(e) => {
                    warn!("failed to read config file {}: {e}", path.display());
                    ConfigRead::Rejected
                }
            }
        }
        Err(e) => {
            warn!(
                "failed to inspect config file {} after open: {e}",
                path.display()
            );
            ConfigRead::Rejected
        }
    }
}

fn parse_config_field<T>(root: &Map<String, Value>, key: &str) -> Option<T>
where
    T: DeserializeOwned,
{
    root.get(key).and_then(|raw| {
        serde_json::from_value(raw.clone())
            .map_err(|e| warn!("ignoring invalid config field `{key}`: {e}"))
            .ok()
    })
}

fn parse_commands(root: &Map<String, Value>) -> Vec<CommandDefinition> {
    let Some(raw) = root.get("commands") else {
        return Vec::new();
    };
    let Some(items) = raw.as_array() else {
        warn!("ignoring config field `commands`: expected an array");
        return Vec::new();
    };

    items
        .iter()
        .enumerate()
        .filter_map(
            |(idx, raw)| match serde_json::from_value::<CommandDefinition>(raw.clone()) {
                Ok(cmd) if validate_command(&cmd) => Some(cmd),
                Ok(_) => None,
                Err(e) => {
                    warn!("skipping invalid command entry at index {idx}: {e}");
                    None
                }
            },
        )
        .collect()
}

/// Load and validate configuration from a specific file path.
///
/// This is the core loading function, also useful for testing.
pub fn load_config_from_path(path: &std::path::Path) -> PaneFlowConfig {
    match read_config_string(path) {
        ConfigRead::Contents(contents) => parse_and_validate_with_path(&contents, path),
        ConfigRead::Absent | ConfigRead::Rejected => PaneFlowConfig::default(),
    }
}

/// Parse a JSON string into a validated `PaneFlowConfig`.
///
/// Invalid JSON produces a warning and returns defaults.
/// Individual commands with validation errors are filtered out with warnings.
pub fn parse_and_validate(json: &str) -> PaneFlowConfig {
    parse_and_validate_with_path(json, Path::new("<config>"))
}

/// Parse + validate. `path` is threaded into the warning so a malformed save
/// names the offending file instead of an anonymous "config".
pub fn parse_and_validate_with_path(json: &str, path: &Path) -> PaneFlowConfig {
    try_parse_and_validate(json).unwrap_or_else(|e| {
        warn!(
            "invalid JSON in config {}: {e}; using defaults",
            path.display()
        );
        PaneFlowConfig::default()
    })
}

/// US-029: parse + validate, parsing the JSON exactly once and surfacing a
/// syntax error as `Err` instead of silently returning defaults. The hot
/// reload path uses this so it can keep the previous config on a malformed
/// save (never broadcasting defaults) AND avoid the old double-parse (a
/// syntax-guard `from_str` followed by a second parse inside
/// `parse_and_validate_with_path`). Command filtering + layout fixups are
/// applied on the success path, unchanged.
pub fn try_parse_and_validate(json: &str) -> Result<PaneFlowConfig, serde_json::Error> {
    let raw: Value = serde_json::from_str(json)?;
    let Some(root) = raw.as_object() else {
        warn!("config root is not a JSON object; using defaults");
        return Ok(PaneFlowConfig::default());
    };

    let mut config = PaneFlowConfig::default();
    match root.get("$schemaVersion") {
        Some(Value::String(version)) if version != "1.0.0" => {
            warn!("config schema version `{version}` is not recognized; loading leniently");
        }
        Some(Value::String(_)) | None => {}
        Some(_) => warn!("config schema version is not a string; ignoring it"),
    }

    macro_rules! set_field {
        ($field:ident) => {
            if let Some(value) = parse_config_field(root, stringify!($field)) {
                config.$field = value;
            }
        };
    }

    set_field!(shortcuts);
    set_field!(default_shell);
    set_field!(theme);
    set_field!(theme_mode);
    set_field!(window_decorations);
    set_field!(window_backdrop);
    set_field!(windows_terminal_material);
    set_field!(windows_chrome_material);
    set_field!(line_height);
    set_field!(cell_width);
    set_field!(font_family);
    set_field!(font_fallbacks);
    set_field!(font_size);
    set_field!(font_weight);
    set_field!(option_as_meta);
    set_field!(shell_integration);
    set_field!(agent_stall_detection);
    set_field!(agent_stall_threshold_secs);
    set_field!(review_prefill_delay_ms);
    set_field!(submit_paste_delay_ms);
    set_field!(external_editor);
    set_field!(claude_code_bypass_permissions);
    set_field!(ai_unrestricted);
    set_field!(ai_injection_fence);
    set_field!(claude_code_button_visible);
    set_field!(codex_button_visible);
    set_field!(opencode_button_visible);
    set_field!(pi_button_visible);
    set_field!(hermes_agent_button_visible);
    set_field!(grok_button_visible);
    set_field!(amp_button_visible);
    set_field!(cursor_button_visible);
    set_field!(gemini_button_visible);
    set_field!(kiro_button_visible);
    set_field!(antigravity_button_visible);
    set_field!(copilot_button_visible);
    set_field!(codebuddy_button_visible);
    set_field!(factory_button_visible);
    set_field!(qoder_button_visible);
    set_field!(openclaw_button_visible);
    set_field!(telemetry);
    set_field!(terminal);
    set_field!(agent_panel);
    set_field!(tool_permissions);

    // Validate and filter commands.
    config.commands = parse_commands(root);

    // Validate and fix layout nodes in-place.
    for cmd in &mut config.commands {
        if let Some(ref mut ws) = cmd.workspace {
            if let Some(ref mut layout) = ws.layout {
                validate_layout(layout);
            }
        }
    }

    Ok(config)
}

/// Validate a single command definition. Returns `false` if it should be skipped.
fn validate_command(cmd: &CommandDefinition) -> bool {
    if cmd.name.trim().is_empty() {
        warn!("skipping command with blank name");
        return false;
    }
    if cmd.workspace.is_some() && cmd.command.as_ref().is_some_and(|s| !s.trim().is_empty()) {
        warn!(
            "skipping command `{}` with both workspace and command",
            cmd.name
        );
        return false;
    }
    if cmd.workspace.is_none() && cmd.command.as_ref().is_some_and(|s| s.trim().is_empty()) {
        warn!("skipping command `{}` with blank shell command", cmd.name);
        return false;
    }
    true
}

/// US-011: total pane (leaf) budget for a single restored layout. Mirrors
/// src-app's `MAX_PANES` - defined locally because `paneflow-config` is a leaf
/// crate that cannot import the src-app constant (US-013 documents the pairing).
const MAX_LAYOUT_LEAVES: usize = 32;

/// US-011: max direct children of one `Split` node at the schema boundary.
const MAX_SPLIT_CHILDREN: usize = 32;

/// US-011: max surfaces (tabs) in one `Pane` at the schema boundary.
const MAX_PANE_SURFACES: usize = 64;

/// Recursively validate and fix a layout node, bounding its breadth and total
/// leaf count at the schema boundary (U-008/U-016).
///
/// - Attacker-driven panes (leaves) are capped to [`MAX_LAYOUT_LEAVES`]: a wide
///   OR deep tree can never restore more terminals than that from session.json
///   content, so a hand-edited / agent-written file can't spawn unbounded PTYs.
///   (A pruned split may gain ≤1 app-synthesized pad pane to stay structurally
///   valid; that is bounded and not attacker-amplified - see the pad note.)
/// - Split nodes: direct children bounded to [`MAX_SPLIT_CHILDREN`]; must have
///   at least 2 children; legacy `ratio` clamped to [0.1, 0.9] and (for a
///   2-child split) converted to an explicit `ratios` pair (U-007); per-child
///   `ratios` clamped to [0.01, 1.0].
/// - Pane nodes: surfaces bounded to [`MAX_PANE_SURFACES`]; must have at least 1.
pub fn validate_layout(node: &mut LayoutNode) {
    let mut leaf_budget = MAX_LAYOUT_LEAVES;
    validate_node(node, &mut leaf_budget);
}

fn validate_node(node: &mut LayoutNode, leaf_budget: &mut usize) {
    match node {
        LayoutNode::Split {
            ref mut direction,
            ref mut ratio,
            ref mut ratios,
            ref mut children,
            ..
        } => {
            if direction != "horizontal" && direction != "vertical" {
                warn!("split direction `{direction}` is invalid; resetting to horizontal");
                *direction = "horizontal".to_string();
            }

            // U-008: bound a single Split's direct breadth before anything else
            // touches the (possibly huge) children vec.
            if children.len() > MAX_SPLIT_CHILDREN {
                warn!(
                    "split has {} children (cap {MAX_SPLIT_CHILDREN}); truncating",
                    children.len()
                );
                children.truncate(MAX_SPLIT_CHILDREN);
            }

            // U-008/U-016: recurse under a shared leaf budget and drop whole
            // subtrees once it is spent, so the total pane count across the tree
            // can never exceed MAX_LAYOUT_LEAVES.
            let mut kept = 0usize;
            for child in children.iter_mut() {
                if *leaf_budget == 0 {
                    break;
                }
                validate_node(child, leaf_budget);
                kept += 1;
            }
            if kept < children.len() {
                warn!(
                    "layout exceeds {MAX_LAYOUT_LEAVES} panes; dropping {} subtree(s)",
                    children.len() - kept
                );
                children.truncate(kept);
            }

            // Must have at least 2 children; pad if fewer (malformed input, or
            // an over-pruned split when earlier siblings spent the budget).
            // The DoS guarantee is about ATTACKER-DRIVEN leaves: those are hard
            // capped at MAX_LAYOUT_LEAVES above. These pad panes are
            // app-synthesized to keep a split structurally valid (>= 2
            // children) and add at most one per pruned split - a bounded
            // structural overshoot, never attacker-amplified PTY spawning.
            while children.len() < 2 {
                warn!(
                    "split node has {} children (need >= 2); padding",
                    children.len()
                );
                children.push(LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                });
                *leaf_budget = leaf_budget.saturating_sub(1);
            }

            // Clamp legacy ratio to [0.1, 0.9]; reject non-finite values.
            if let Some(r) = ratio {
                if !r.is_finite() {
                    warn!("split ratio is NaN/Infinity; resetting to 0.5");
                    *r = 0.5;
                } else if *r < 0.1 {
                    warn!("split ratio {r} is below minimum; clamping to 0.1");
                    *r = 0.1;
                } else if *r > 0.9 {
                    warn!("split ratio {r} is above maximum; clamping to 0.9");
                    *r = 0.9;
                }
            }

            // U-007: a legacy single `ratio` is only meaningful for a 2-child
            // split - convert it to an explicit `ratios` pair so it survives
            // restore (resolved_ratios only honors it transiently). For an
            // N-ary split it is ambiguous, so warn that it is ignored rather
            // than silently returning equal shares.
            if ratios.is_none() {
                if let Some(r) = ratio {
                    if children.len() == 2 {
                        *ratios = Some(vec![*r, 1.0 - *r]);
                    } else {
                        warn!(
                            "legacy ratio ignored on N-ary split ({} children)",
                            children.len()
                        );
                    }
                }
            }

            // Validate per-child ratios: reject non-finite, fix length mismatch, normalize.
            if let Some(ref mut rs) = ratios {
                // Reject NaN/Infinity values.
                for r in rs.iter_mut() {
                    if !r.is_finite() {
                        warn!("per-child ratio is NaN/Infinity; resetting");
                        *r = 1.0 / children.len() as f64;
                    }
                }
                // Fix length mismatch: trim or extend to match children count.
                let n = children.len();
                if rs.len() != n {
                    warn!(
                        "ratios length ({}) != children count ({}); fixing",
                        rs.len(),
                        n
                    );
                    rs.resize(n, 1.0 / n as f64);
                }
                // Clamp individual values to [0.01, 1.0].
                for r in rs.iter_mut() {
                    *r = r.clamp(0.01, 1.0);
                }
                // Normalize to sum ~1.0. `1e-6` (not `f64::EPSILON`, ~2.2e-16)
                // so trivial float drift does not trigger a needless rescale.
                let sum: f64 = rs.iter().sum();
                if sum > 0.0 && (sum - 1.0).abs() > 1e-6 {
                    for r in rs.iter_mut() {
                        *r /= sum;
                    }
                }
                // Re-clamp: normalization can push a value back below the 0.01
                // floor (e.g. one near-1.0 ratio among many children). The floor
                // is the invariant we guarantee; the renderer re-normalizes
                // proportionally at paint time.
                for r in rs.iter_mut() {
                    *r = r.clamp(0.01, 1.0);
                }
            }
            // Note: children were already validated in the budget-bounded
            // recursion above, so there is no separate recurse pass here.
        }
        LayoutNode::Pane {
            ref mut surfaces, ..
        } => {
            // U-008: bound tabs per pane - a pane is one leaf in the tree (so
            // the leaf budget does not catch it), but each surface still spawns
            // a real terminal on restore.
            if surfaces.len() > MAX_PANE_SURFACES {
                warn!(
                    "pane has {} surfaces (cap {MAX_PANE_SURFACES}); truncating",
                    surfaces.len()
                );
                surfaces.truncate(MAX_PANE_SURFACES);
            }
            if surfaces.is_empty() {
                warn!("pane has no surfaces; adding a default surface");
                surfaces.push(Default::default());
            }
            let mut focus_seen = false;
            for surface in surfaces.iter_mut() {
                if surface.focus == Some(true) {
                    if focus_seen {
                        warn!("pane has multiple focused surfaces; dropping extra focus flag");
                        surface.focus = None;
                    } else {
                        focus_seen = true;
                    }
                }
            }
            *leaf_budget = leaf_budget.saturating_sub(1);
        }
    }
}

impl Default for crate::schema::SurfaceDefinition {
    fn default() -> Self {
        Self {
            surface_type: Some("terminal".to_string()),
            name: None,
            custom_name: None,
            command: None,
            prompt: None,
            cwd: None,
            path: None,
            env: None,
            focus: None,
            scrollback: None,
            agent: None,
            font_size: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::*;
    use std::collections::HashMap;
    use tempfile::NamedTempFile;

    #[test]
    fn test_default_config() {
        let config = PaneFlowConfig::default();
        assert!(config.shortcuts.is_empty());
        assert!(config.default_shell.is_none());
        assert!(config.commands.is_empty());
    }

    #[test]
    fn test_config_path_is_some() {
        // On most systems dirs::config_dir() succeeds. The subdir varies
        // by build profile (`paneflow` in release, `paneflow-dev` in
        // debug -- see `APP_SUBDIR`) so tests assert against the const,
        // not a hardcoded `paneflow` literal.
        let path = config_path();
        assert!(path.is_some());
        let p = path.unwrap();
        let suffix_unix = format!("{APP_SUBDIR}/paneflow.json");
        let suffix_win = format!("{APP_SUBDIR}\\paneflow.json");
        assert!(
            p.ends_with(&suffix_unix) || p.ends_with(&suffix_win),
            "config path {p:?} does not end with {suffix_unix}"
        );
    }

    #[test]
    fn test_missing_file_returns_defaults() {
        let config = load_config_from_path(std::path::Path::new("/nonexistent/path/config.json"));
        assert_eq!(config, PaneFlowConfig::default());
    }

    #[test]
    fn test_invalid_json_returns_defaults() {
        let config = parse_and_validate("this is not json {{{");
        assert_eq!(config, PaneFlowConfig::default());
    }

    #[test]
    fn test_unknown_terminal_enum_falls_back_not_wipes_config() {
        // Regression guard: a typo in a terminal enum (`"squiggle"`,
        // `"blinky"`) must fall back to that enum's default WITHOUT discarding
        // the rest of the config. Before the custom `Deserialize`, serde hard-
        // errored here and `parse_and_validate` returned `default()` for the
        // whole file -- theme, shell, and shortcuts all silently lost.
        let json = r#"{
            "theme": "One Dark",
            "default_shell": "/bin/zsh",
            "terminal": { "cursor_shape": "squiggle", "cursor_blink": "blinky" }
        }"#;
        let config = parse_and_validate(json);

        // The surrounding config survives the bad enum values.
        assert_eq!(config.theme.as_deref(), Some("One Dark"));
        assert_eq!(config.default_shell.as_deref(), Some("/bin/zsh"));

        // Each unrecognised enum value resolves to its documented default.
        let term = config
            .terminal
            .expect("terminal block must survive unknown enum values");
        assert_eq!(term.cursor_shape, Some(CursorShapeConfig::Block));
        assert_eq!(
            term.cursor_blink,
            Some(CursorBlinkConfig::TerminalControlled)
        );
    }

    #[test]
    fn test_empty_json_object_returns_defaults() {
        let config = parse_and_validate("{}");
        assert_eq!(config, PaneFlowConfig::default());
    }

    #[test]
    fn test_valid_minimal_config() {
        let json = r#"{
            "default_shell": "/bin/zsh",
            "shortcuts": {"ctrl+t": "new_tab"},
            "commands": []
        }"#;
        let config = parse_and_validate(json);
        assert_eq!(config.default_shell, Some("/bin/zsh".to_string()));
        assert_eq!(config.shortcuts.get("ctrl+t"), Some(&"new_tab".to_string()));
        assert!(config.commands.is_empty());
    }

    #[test]
    fn test_blank_name_skipped() {
        let json = r#"{
            "commands": [
                {"name": "", "keywords": []},
                {"name": "  ", "keywords": []},
                {"name": "valid", "keywords": ["test"]}
            ]
        }"#;
        let config = parse_and_validate(json);
        assert_eq!(config.commands.len(), 1);
        assert_eq!(config.commands[0].name, "valid");
    }

    #[test]
    fn test_malformed_command_entry_does_not_drop_valid_siblings_or_config() {
        let json = r#"{
            "theme": "One Dark",
            "commands": [
                {"description": "missing name", "command": "bad"},
                {"name": "valid", "keywords": ["test"], "command": "echo ok"}
            ]
        }"#;

        let config = parse_and_validate(json);

        assert_eq!(config.theme.as_deref(), Some("One Dark"));
        assert_eq!(config.commands.len(), 1);
        assert_eq!(config.commands[0].name, "valid");
    }

    #[test]
    fn test_command_with_workspace() {
        let json = r#"{
            "commands": [{
                "name": "dev",
                "description": "Development workspace",
                "keywords": ["dev", "work"],
                "workspace": {
                    "name": "Dev Workspace",
                    "cwd": "/home/user/projects",
                    "color": "ff6600",
                    "layout": {
                        "type": "split",
                        "direction": "horizontal",
                        "ratio": 0.5,
                        "children": [
                            {
                                "type": "pane",
                                "surfaces": [{"surface_type": "terminal", "command": "vim"}]
                            },
                            {
                                "type": "pane",
                                "surfaces": [{"surface_type": "terminal", "command": "cargo watch"}]
                            }
                        ]
                    }
                }
            }]
        }"#;
        let config = parse_and_validate(json);
        assert_eq!(config.commands.len(), 1);
        let cmd = &config.commands[0];
        assert_eq!(cmd.name, "dev");
        assert_eq!(cmd.description.as_deref(), Some("Development workspace"));

        let ws = cmd.workspace.as_ref().unwrap();
        assert_eq!(ws.name.as_deref(), Some("Dev Workspace"));
        assert_eq!(ws.color.as_deref(), Some("ff6600"));

        match ws.layout.as_ref().unwrap() {
            LayoutNode::Split {
                direction,
                ratio,
                children,
                ..
            } => {
                assert_eq!(direction, "horizontal");
                assert_eq!(*ratio, Some(0.5));
                assert_eq!(children.len(), 2);
            }
            _ => panic!("expected split layout"),
        }
    }

    #[test]
    fn test_command_with_shell_command() {
        let json = r#"{
            "commands": [{
                "name": "htop",
                "keywords": ["monitor"],
                "command": "htop"
            }]
        }"#;
        let config = parse_and_validate(json);
        assert_eq!(config.commands.len(), 1);
        assert_eq!(config.commands[0].command.as_deref(), Some("htop"));
        assert!(config.commands[0].workspace.is_none());
    }

    #[test]
    fn test_split_ratio_clamped_low() {
        let json = r#"{
            "commands": [{
                "name": "test",
                "keywords": [],
                "workspace": {
                    "layout": {
                        "type": "split",
                        "direction": "vertical",
                        "ratio": 0.01,
                        "children": [
                            {"type": "pane", "surfaces": [{"surface_type": "terminal"}]},
                            {"type": "pane", "surfaces": [{"surface_type": "terminal"}]}
                        ]
                    }
                }
            }]
        }"#;
        let config = parse_and_validate(json);
        let ws = config.commands[0].workspace.as_ref().unwrap();
        match ws.layout.as_ref().unwrap() {
            LayoutNode::Split { ratio, .. } => {
                assert!((ratio.unwrap() - 0.1).abs() < f64::EPSILON);
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn test_split_ratio_clamped_high() {
        let json = r#"{
            "commands": [{
                "name": "test",
                "keywords": [],
                "workspace": {
                    "layout": {
                        "type": "split",
                        "direction": "vertical",
                        "ratio": 0.99,
                        "children": [
                            {"type": "pane", "surfaces": [{"surface_type": "terminal"}]},
                            {"type": "pane", "surfaces": [{"surface_type": "terminal"}]}
                        ]
                    }
                }
            }]
        }"#;
        let config = parse_and_validate(json);
        let ws = config.commands[0].workspace.as_ref().unwrap();
        match ws.layout.as_ref().unwrap() {
            LayoutNode::Split { ratio, .. } => {
                assert!((ratio.unwrap() - 0.9).abs() < f64::EPSILON);
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn test_per_child_ratios_floor_respected_after_normalize() {
        // US-057: clamp -> normalize can push a value back below the 0.01 floor.
        // The re-clamp must restore it. ratios [100.0, 0.001] -> clamp
        // [1.0, 0.01] -> normalize ~[0.990, 0.0099] (2nd below floor) -> re-clamp.
        let mut node = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: None,
            ratios: Some(vec![100.0, 0.001]),
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        validate_layout(&mut node);
        match node {
            LayoutNode::Split { ratios, .. } => {
                let rs = ratios.unwrap();
                assert!(
                    rs.iter().all(|r| *r >= 0.01),
                    "every ratio must respect the 0.01 floor after normalize: {rs:?}"
                );
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn test_split_nary_children_accepted() {
        // 3+ children are valid in N-ary layout.
        let json = r#"{
            "commands": [{
                "name": "test",
                "keywords": [],
                "workspace": {
                    "layout": {
                        "type": "split",
                        "direction": "horizontal",
                        "ratios": [0.33, 0.33, 0.34],
                        "children": [
                            {"type": "pane", "surfaces": [{"surface_type": "terminal"}]},
                            {"type": "pane", "surfaces": [{"surface_type": "terminal"}]},
                            {"type": "pane", "surfaces": [{"surface_type": "terminal"}]}
                        ]
                    }
                }
            }]
        }"#;
        let config = parse_and_validate(json);
        let ws = config.commands[0].workspace.as_ref().unwrap();
        match ws.layout.as_ref().unwrap() {
            LayoutNode::Split {
                children, ratios, ..
            } => {
                assert_eq!(children.len(), 3);
                let rs = ratios.as_ref().unwrap();
                assert_eq!(rs.len(), 3);
                assert!((rs[0] - 0.33).abs() < f64::EPSILON);
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn test_split_zero_children_padded() {
        let json = r#"{
            "commands": [{
                "name": "test",
                "keywords": [],
                "workspace": {
                    "layout": {
                        "type": "split",
                        "direction": "horizontal",
                        "children": []
                    }
                }
            }]
        }"#;
        let config = parse_and_validate(json);
        let ws = config.commands[0].workspace.as_ref().unwrap();
        match ws.layout.as_ref().unwrap() {
            LayoutNode::Split { children, .. } => {
                assert_eq!(children.len(), 2);
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn test_pane_no_surfaces_gets_default() {
        let json = r#"{
            "commands": [{
                "name": "test",
                "keywords": [],
                "workspace": {
                    "layout": {
                        "type": "pane",
                        "surfaces": []
                    }
                }
            }]
        }"#;
        let config = parse_and_validate(json);
        let ws = config.commands[0].workspace.as_ref().unwrap();
        match ws.layout.as_ref().unwrap() {
            LayoutNode::Pane { surfaces } => {
                assert_eq!(surfaces.len(), 1);
                assert_eq!(surfaces[0].surface_type.as_deref(), Some("terminal"));
            }
            _ => panic!("expected pane"),
        }
    }

    #[test]
    fn test_load_from_file() {
        use std::io::Write;
        let mut tmp = NamedTempFile::new().unwrap();
        write!(
            tmp,
            r#"{{"default_shell": "/bin/bash", "commands": [{{"name": "ls", "keywords": [], "command": "ls -la"}}]}}"#
        )
        .unwrap();

        let config = load_config_from_path(tmp.path());
        assert_eq!(config.default_shell, Some("/bin/bash".to_string()));
        assert_eq!(config.commands.len(), 1);
        assert_eq!(config.commands[0].name, "ls");
    }

    #[test]
    fn test_load_from_file_invalid_json() {
        use std::io::Write;
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "not valid json!!").unwrap();

        let config = load_config_from_path(tmp.path());
        assert_eq!(config, PaneFlowConfig::default());
    }

    #[test]
    fn test_nested_split_validation() {
        let json = r#"{
            "commands": [{
                "name": "nested",
                "keywords": [],
                "workspace": {
                    "layout": {
                        "type": "split",
                        "direction": "horizontal",
                        "ratio": 0.5,
                        "children": [
                            {
                                "type": "split",
                                "direction": "vertical",
                                "ratio": 0.05,
                                "children": [
                                    {"type": "pane", "surfaces": [{"surface_type": "terminal"}]},
                                    {"type": "pane", "surfaces": [{"surface_type": "terminal"}]}
                                ]
                            },
                            {"type": "pane", "surfaces": [{"surface_type": "terminal"}]}
                        ]
                    }
                }
            }]
        }"#;
        let config = parse_and_validate(json);
        let ws = config.commands[0].workspace.as_ref().unwrap();
        match ws.layout.as_ref().unwrap() {
            LayoutNode::Split { children, .. } => {
                // Inner split should have ratio clamped to 0.1.
                match &children[0] {
                    LayoutNode::Split { ratio, .. } => {
                        assert!((ratio.unwrap() - 0.1).abs() < f64::EPSILON);
                    }
                    _ => panic!("expected nested split"),
                }
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn test_surface_with_env_and_focus() {
        let json = r#"{
            "commands": [{
                "name": "envtest",
                "keywords": [],
                "workspace": {
                    "layout": {
                        "type": "pane",
                        "surfaces": [{
                            "surface_type": "terminal",
                            "name": "main",
                            "command": "cargo run",
                            "cwd": "/tmp",
                            "env": {"RUST_LOG": "debug"},
                            "focus": true
                        }]
                    }
                }
            }]
        }"#;
        let config = parse_and_validate(json);
        let ws = config.commands[0].workspace.as_ref().unwrap();
        match ws.layout.as_ref().unwrap() {
            LayoutNode::Pane { surfaces } => {
                assert_eq!(surfaces.len(), 1);
                let s = &surfaces[0];
                assert_eq!(s.name.as_deref(), Some("main"));
                assert_eq!(s.command.as_deref(), Some("cargo run"));
                assert_eq!(s.cwd.as_deref(), Some("/tmp"));
                assert_eq!(s.focus, Some(true));
                let env = s.env.as_ref().unwrap();
                assert_eq!(env.get("RUST_LOG"), Some(&"debug".to_string()));
            }
            _ => panic!("expected pane"),
        }
    }

    #[test]
    fn test_serialization_roundtrip() {
        let config = PaneFlowConfig {
            shortcuts: {
                let mut m = HashMap::new();
                m.insert("ctrl+n".to_string(), "new_window".to_string());
                m
            },
            default_shell: Some("/bin/fish".to_string()),
            theme: Some("One Dark".to_string()),
            theme_mode: Some("dark".to_string()),
            commands: vec![CommandDefinition {
                name: "test".to_string(),
                description: Some("A test command".to_string()),
                keywords: vec!["test".to_string()],
                workspace: None,
                command: Some("echo hello".to_string()),
            }],
            window_decorations: None,
            window_backdrop: None,
            windows_terminal_material: None,
            windows_chrome_material: None,
            line_height: None,
            cell_width: None,
            font_family: None,
            font_fallbacks: None,
            font_size: None,
            font_weight: None,
            option_as_meta: None,
            shell_integration: None,
            agent_stall_detection: None,
            agent_stall_threshold_secs: None,
            review_prefill_delay_ms: None,
            submit_paste_delay_ms: None,
            claude_code_bypass_permissions: None,
            ai_unrestricted: None,
            ai_injection_fence: None,
            claude_code_button_visible: None,
            codex_button_visible: None,
            opencode_button_visible: None,
            pi_button_visible: None,
            hermes_agent_button_visible: None,
            grok_button_visible: None,
            amp_button_visible: None,
            cursor_button_visible: None,
            gemini_button_visible: None,
            kiro_button_visible: None,
            antigravity_button_visible: None,
            copilot_button_visible: None,
            codebuddy_button_visible: None,
            factory_button_visible: None,
            qoder_button_visible: None,
            openclaw_button_visible: None,
            telemetry: None,
            terminal: None,
            agent_panel: None,
            external_editor: None,
            tool_permissions: HashMap::new(),
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        let reparsed = parse_and_validate(&json);
        assert_eq!(config, reparsed);
    }

    #[test]
    fn test_nary_layout_roundtrip() {
        // Build a 6-pane N-ary layout: 3 panes horizontal on top, 3 on bottom.
        let make_pane = |name: &str| LayoutNode::Pane {
            surfaces: vec![SurfaceDefinition {
                surface_type: Some("terminal".to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            }],
        };

        let top_row = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: None,
            ratios: Some(vec![0.33, 0.33, 0.34]),
            children: vec![make_pane("A"), make_pane("B"), make_pane("C")],
        };
        let bottom_row = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: None,
            ratios: Some(vec![0.33, 0.33, 0.34]),
            children: vec![make_pane("D"), make_pane("E"), make_pane("F")],
        };
        let root = LayoutNode::Split {
            direction: "horizontal".to_string(),
            ratio: None,
            ratios: Some(vec![0.5, 0.5]),
            children: vec![top_row, bottom_row],
        };

        // Serialize to JSON and back.
        let json = serde_json::to_string_pretty(&root).unwrap();
        let deserialized: LayoutNode = serde_json::from_str(&json).unwrap();
        assert_eq!(root, deserialized);
    }

    #[test]
    fn test_legacy_binary_still_works() {
        // Legacy format with single `ratio` field (no `ratios`).
        let json = r#"{
            "type": "split",
            "direction": "horizontal",
            "ratio": 0.6,
            "children": [
                {"type": "pane", "surfaces": [{"surface_type": "terminal"}]},
                {"type": "pane", "surfaces": [{"surface_type": "terminal"}]}
            ]
        }"#;
        let node: LayoutNode = serde_json::from_str(json).unwrap();
        match &node {
            LayoutNode::Split {
                ratio,
                ratios,
                children,
                ..
            } => {
                assert_eq!(*ratio, Some(0.6));
                assert!(ratios.is_none());
                assert_eq!(children.len(), 2);
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn test_layout_node_leaf_count() {
        let single = LayoutNode::Pane {
            surfaces: vec![Default::default()],
        };
        assert_eq!(single.leaf_count(), 1);

        // 3-child flat split = 3 leaves
        let flat = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: None,
            ratios: Some(vec![0.33, 0.33, 0.34]),
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        assert_eq!(flat.leaf_count(), 3);

        // Nested: 2 rows of 3 = 6 leaves
        let nested = LayoutNode::Split {
            direction: "horizontal".to_string(),
            ratio: None,
            ratios: Some(vec![0.5, 0.5]),
            children: vec![flat.clone(), flat],
        };
        assert_eq!(nested.leaf_count(), 6);
    }

    #[test]
    fn test_resolved_ratios_nary() {
        let node = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: None,
            ratios: Some(vec![0.25, 0.25, 0.5]),
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        assert_eq!(node.resolved_ratios(), vec![0.25, 0.25, 0.5]);
    }

    #[test]
    fn test_resolved_ratios_legacy_binary() {
        let node = LayoutNode::Split {
            direction: "horizontal".to_string(),
            ratio: Some(0.6),
            ratios: None,
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        let rs = node.resolved_ratios();
        assert_eq!(rs.len(), 2);
        assert!((rs[0] - 0.6).abs() < f64::EPSILON);
        assert!((rs[1] - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn test_resolved_ratios_rejects_nan_and_negative() {
        // US-056: a corrupt session.json can carry NaN/negative/out-of-range
        // ratios. They must be clamped, normalized, and never propagate.
        let node = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: None,
            ratios: Some(vec![f64::NAN, -0.5, 2.0]),
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        let rs = node.resolved_ratios();
        assert_eq!(rs.len(), 3);
        // EP-010 review: post-US-057-parity invariant. NaN/negative are floored,
        // 2.0 is clamped to 1.0; every ratio is finite and in `[0.01, 1.0]`. The
        // post-normalize re-clamp (matching `validate_layout`) keeps the floor,
        // so the sum is ~1.0 but not exactly - the renderer re-normalizes at
        // paint. Assert the floor + a sane sum band, not `== 1.0`.
        assert!(rs
            .iter()
            .all(|&r| r.is_finite() && (0.01 - 1e-9..=1.0).contains(&r)));
        let sum: f64 = rs.iter().sum();
        assert!(
            (sum - 1.0).abs() < 0.05,
            "ratios must stay near 1.0, got {sum}"
        );
    }

    #[test]
    fn test_resolved_ratios_floor_respected_after_normalize() {
        // EP-010 review: the SESSION path (`resolved_ratios` -> `sanitize_ratios`)
        // must honour the 0.01 floor AFTER normalize, matching the config path
        // (`validate_layout`, see `test_per_child_ratios_floor_respected_after_normalize`).
        // `[1.0, 0.005]` clamps to `[1.0, 0.01]` (sum 1.01); normalizing alone
        // would push the second child to ~0.0099 - below the floor. The
        // post-normalize re-clamp must pull it back to 0.01.
        let node = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: None,
            ratios: Some(vec![1.0, 0.005]),
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        let rs = node.resolved_ratios();
        assert_eq!(rs.len(), 2);
        assert!(
            rs.iter().all(|&r| r >= 0.01 - 1e-9),
            "every ratio must stay at/above the 0.01 floor after normalize, got {rs:?}"
        );
    }

    #[test]
    fn test_resolved_ratios_length_mismatch_falls_back() {
        // US-056: a ratios array whose length disagrees with the child count
        // is unrecoverable -> equal shares, never a panic or stale mapping.
        let node = LayoutNode::Split {
            direction: "horizontal".to_string(),
            ratio: None,
            ratios: Some(vec![0.9]), // 1 ratio, 2 children
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        let rs = node.resolved_ratios();
        assert_eq!(rs, vec![0.5, 0.5]);
    }

    #[test]
    fn test_resolved_ratios_fallback_equal() {
        let node = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: None,
            ratios: None,
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        let rs = node.resolved_ratios();
        assert_eq!(rs.len(), 3);
        for r in &rs {
            assert!((r - 1.0 / 3.0).abs() < f64::EPSILON);
        }
    }

    // --- Session persistence round-trip tests (US-017) ---

    fn make_surface(cwd: &str) -> SurfaceDefinition {
        SurfaceDefinition {
            surface_type: Some("terminal".to_string()),
            cwd: Some(cwd.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_session_roundtrip_single_workspace() {
        let state = SessionState {
            version: 1,
            active_workspace: 0,
            workspaces: vec![WorkspaceSession {
                title: "main".to_string(),
                cwd: "/home/user/project".to_string(),
                layout: Some(LayoutNode::Pane {
                    surfaces: vec![make_surface("/home/user/project")],
                }),
                custom_buttons: vec![],
                expanded_paths: vec![],
                managed_worktrees: vec![],
            }],
            projects: Vec::new(),
            active_project: 0,
            chats: Vec::new(),
            agents_target: None,
            mode: AppMode::default(),
            diff_scope: None,
        };
        let json = serde_json::to_string_pretty(&state).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
    }

    #[test]
    fn test_session_roundtrip_multiple_workspaces() {
        let state = SessionState {
            version: 1,
            active_workspace: 1,
            workspaces: vec![
                WorkspaceSession {
                    title: "frontend".to_string(),
                    cwd: "/home/user/web".to_string(),
                    layout: Some(LayoutNode::Pane {
                        surfaces: vec![make_surface("/home/user/web")],
                    }),
                    custom_buttons: vec![],
                    expanded_paths: vec![],
                    managed_worktrees: vec![],
                },
                WorkspaceSession {
                    title: "backend".to_string(),
                    cwd: "/home/user/api".to_string(),
                    layout: Some(LayoutNode::Pane {
                        surfaces: vec![make_surface("/home/user/api")],
                    }),
                    custom_buttons: vec![],
                    expanded_paths: vec![],
                    managed_worktrees: vec![],
                },
                WorkspaceSession {
                    title: "devops".to_string(),
                    cwd: "/home/user/infra".to_string(),
                    layout: None,
                    custom_buttons: vec![],
                    expanded_paths: vec![],
                    managed_worktrees: vec![],
                },
            ],
            projects: Vec::new(),
            active_project: 0,
            chats: Vec::new(),
            agents_target: None,
            mode: AppMode::default(),
            diff_scope: None,
        };
        let json = serde_json::to_string_pretty(&state).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
        assert_eq!(restored.active_workspace, 1);
        assert_eq!(restored.workspaces.len(), 3);
    }

    #[test]
    fn test_session_roundtrip_nested_splits() {
        let state = SessionState {
            version: 1,
            active_workspace: 0,
            workspaces: vec![WorkspaceSession {
                title: "dev".to_string(),
                cwd: "/home/user".to_string(),
                custom_buttons: vec![],
                expanded_paths: vec![],
                managed_worktrees: vec![],
                layout: Some(LayoutNode::Split {
                    direction: "horizontal".to_string(),
                    ratio: None,
                    ratios: Some(vec![0.6, 0.4]),
                    children: vec![
                        LayoutNode::Pane {
                            surfaces: vec![make_surface("/home/user/code")],
                        },
                        LayoutNode::Split {
                            direction: "vertical".to_string(),
                            ratio: None,
                            ratios: Some(vec![0.5, 0.5]),
                            children: vec![
                                LayoutNode::Pane {
                                    surfaces: vec![make_surface("/home/user/tests")],
                                },
                                LayoutNode::Pane {
                                    surfaces: vec![make_surface("/home/user/logs")],
                                },
                            ],
                        },
                    ],
                }),
            }],
            projects: Vec::new(),
            active_project: 0,
            chats: Vec::new(),
            agents_target: None,
            mode: AppMode::default(),
            diff_scope: None,
        };
        let json = serde_json::to_string_pretty(&state).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
        // Verify structure: root is split with 2 children, second child is also a split
        let layout = restored.workspaces[0].layout.as_ref().unwrap();
        assert_eq!(layout.leaf_count(), 3);
    }

    #[test]
    fn test_session_roundtrip_with_scrollback() {
        let state = SessionState {
            version: 1,
            active_workspace: 0,
            workspaces: vec![WorkspaceSession {
                title: "main".to_string(),
                cwd: "/tmp".to_string(),
                custom_buttons: vec![],
                expanded_paths: vec![],
                managed_worktrees: vec![],
                layout: Some(LayoutNode::Pane {
                    surfaces: vec![SurfaceDefinition {
                        surface_type: Some("terminal".to_string()),
                        cwd: Some("/tmp".to_string()),
                        scrollback: Some(
                            "$ ls\nfile1.txt\nfile2.txt\n$ echo hello\nhello".to_string(),
                        ),
                        ..Default::default()
                    }],
                }),
            }],
            projects: Vec::new(),
            active_project: 0,
            chats: Vec::new(),
            agents_target: None,
            mode: AppMode::default(),
            diff_scope: None,
        };
        let json = serde_json::to_string_pretty(&state).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
        let surface = match &restored.workspaces[0].layout {
            Some(LayoutNode::Pane { surfaces }) => &surfaces[0],
            _ => panic!("expected pane"),
        };
        assert!(surface.scrollback.as_ref().unwrap().contains("hello"));
    }

    #[test]
    fn test_session_corrupted_json_returns_none() {
        // Truncated JSON - simulates crash during write
        let corrupted = r#"{"version":1,"active_workspace":0,"workspaces":[{"title":"ma"#;
        let result: Result<SessionState, _> = serde_json::from_str(corrupted);
        assert!(result.is_err(), "Corrupted JSON should fail to parse");
    }

    #[test]
    fn test_session_scrollback_none_omitted_from_json() {
        let surface = SurfaceDefinition {
            scrollback: None,
            ..Default::default()
        };
        let json = serde_json::to_string(&surface).unwrap();
        assert!(
            !json.contains("scrollback"),
            "None scrollback should be omitted from JSON"
        );
    }

    // ─── Telemetry config (US-009) ────────────────────────────────────────
    //
    // Tri-state encoding:
    //   - outer None          → user never prompted (block missing from JSON)
    //   - Some { enabled: None } → block present, question unanswered
    //   - Some { enabled: Some(true|false) } → explicit consent answer
    //
    // No event is ever emitted unless `enabled == Some(true)` at the
    // capture layer (US-012).

    #[test]
    fn test_telemetry_missing_block() {
        // No `telemetry` key at all → outer None (never asked).
        let config = parse_and_validate(r#"{"default_shell": "/bin/sh"}"#);
        assert!(config.telemetry.is_none());
    }

    #[test]
    fn test_telemetry_enabled_null_and_empty() {
        // Both `{"enabled": null}` and `{}` must parse to the same state:
        // block present, enabled unresolved. Both forms are expected in
        // the wild (users editing by hand vs. the modal writing `{}` before
        // the user clicks).
        let via_null = parse_and_validate(r#"{"telemetry": {"enabled": null}}"#);
        let via_empty = parse_and_validate(r#"{"telemetry": {}}"#);

        assert_eq!(via_null.telemetry, Some(TelemetryConfig { enabled: None }));
        assert_eq!(via_empty.telemetry, Some(TelemetryConfig { enabled: None }));
        assert_eq!(via_null.telemetry, via_empty.telemetry);
    }

    #[test]
    fn test_telemetry_enabled_true() {
        let config = parse_and_validate(r#"{"telemetry": {"enabled": true}}"#);
        assert_eq!(
            config.telemetry,
            Some(TelemetryConfig {
                enabled: Some(true)
            })
        );

        // Round-trip: re-serialize then re-parse - the consent answer
        // must survive without loss so the modal never re-prompts.
        let json = serde_json::to_string(&config).unwrap();
        let reparsed = parse_and_validate(&json);
        assert_eq!(reparsed.telemetry, config.telemetry);
    }

    #[test]
    fn test_telemetry_enabled_false() {
        let config = parse_and_validate(r#"{"telemetry": {"enabled": false}}"#);
        assert_eq!(
            config.telemetry,
            Some(TelemetryConfig {
                enabled: Some(false)
            })
        );

        let json = serde_json::to_string(&config).unwrap();
        let reparsed = parse_and_validate(&json);
        assert_eq!(reparsed.telemetry, config.telemetry);
    }

    // ─── Terminal config - ligatures (US-008) ─────────────────────────────
    //
    // Behavior contract:
    //   - block missing                   → terminal = None    (default off)
    //   - {"terminal": {}}                → terminal = Some(TerminalConfig { ligatures: None })
    //   - {"terminal": {"ligatures": null}} → terminal = Some(TerminalConfig { ligatures: None })
    //   - {"terminal": {"ligatures": true}}  → ligatures opt-in
    //   - {"terminal": {"ligatures": false}} → explicit opt-out (same as default)

    #[test]
    fn test_terminal_block_missing_defaults_off() {
        let config = parse_and_validate(r#"{"default_shell": "/bin/sh"}"#);
        assert!(config.terminal.is_none());
    }

    #[test]
    fn test_terminal_ligatures_default_when_block_empty() {
        let from_empty = parse_and_validate(r#"{"terminal": {}}"#);
        let from_null = parse_and_validate(r#"{"terminal": {"ligatures": null}}"#);
        assert_eq!(
            from_empty.terminal,
            Some(TerminalConfig {
                backend: Default::default(),
                ligatures: None,
                integrated_glyphs: None,
                color_emoji: None,
                cursor_color: None,
                scrollback_lines: None,
                cursor_shape: None,
                cursor_blink: None,
                env: None,
                scroll_multiplier: None,
            })
        );
        assert_eq!(
            from_null.terminal,
            Some(TerminalConfig {
                backend: Default::default(),
                ligatures: None,
                integrated_glyphs: None,
                color_emoji: None,
                cursor_color: None,
                scrollback_lines: None,
                cursor_shape: None,
                cursor_blink: None,
                env: None,
                scroll_multiplier: None,
            })
        );
    }

    #[test]
    fn test_terminal_ligatures_true() {
        let config = parse_and_validate(r#"{"terminal": {"ligatures": true}}"#);
        assert_eq!(
            config.terminal,
            Some(TerminalConfig {
                backend: Default::default(),
                ligatures: Some(true),
                integrated_glyphs: None,
                color_emoji: None,
                cursor_color: None,
                scrollback_lines: None,
                cursor_shape: None,
                cursor_blink: None,
                env: None,
                scroll_multiplier: None,
            })
        );

        // Survive a serialize → parse round-trip so the user's opt-in
        // isn't dropped if Paneflow rewrites the config file.
        let json = serde_json::to_string(&config).unwrap();
        let reparsed = parse_and_validate(&json);
        assert_eq!(reparsed.terminal, config.terminal);
    }

    #[test]
    fn test_terminal_ligatures_false() {
        let config = parse_and_validate(r#"{"terminal": {"ligatures": false}}"#);
        assert_eq!(
            config.terminal,
            Some(TerminalConfig {
                backend: Default::default(),
                ligatures: Some(false),
                integrated_glyphs: None,
                color_emoji: None,
                cursor_color: None,
                scrollback_lines: None,
                cursor_shape: None,
                cursor_blink: None,
                env: None,
                scroll_multiplier: None,
            })
        );
    }

    #[test]
    fn test_terminal_integrated_glyphs_default_on_and_false_opt_out() {
        let absent = parse_and_validate(r#"{"terminal": {}}"#);
        assert!(
            absent
                .terminal
                .as_ref()
                .expect("terminal block present")
                .resolved_integrated_glyphs(),
            "absent terminal.integrated_glyphs resolves to enabled"
        );

        let disabled = parse_and_validate(r#"{"terminal": {"integrated_glyphs": false}}"#);
        assert_eq!(
            disabled.terminal,
            Some(TerminalConfig {
                backend: Default::default(),
                ligatures: None,
                integrated_glyphs: Some(false),
                color_emoji: None,
                cursor_color: None,
                scrollback_lines: None,
                cursor_shape: None,
                cursor_blink: None,
                env: None,
                scroll_multiplier: None,
            })
        );
        assert!(
            !disabled
                .terminal
                .as_ref()
                .expect("terminal block present")
                .resolved_integrated_glyphs(),
            "explicit false disables integrated glyphs"
        );
    }

    #[test]
    fn test_terminal_color_emoji_default_on_and_false_opt_out() {
        let absent = parse_and_validate(r#"{"terminal": {}}"#);
        assert!(
            absent
                .terminal
                .as_ref()
                .expect("terminal block present")
                .resolved_color_emoji(),
            "absent terminal.color_emoji resolves to enabled"
        );

        let disabled = parse_and_validate(r#"{"terminal": {"color_emoji": false}}"#);
        assert_eq!(
            disabled.terminal,
            Some(TerminalConfig {
                backend: Default::default(),
                ligatures: None,
                integrated_glyphs: None,
                color_emoji: Some(false),
                cursor_color: None,
                scrollback_lines: None,
                cursor_shape: None,
                cursor_blink: None,
                env: None,
                scroll_multiplier: None,
            })
        );
        assert!(
            !disabled
                .terminal
                .as_ref()
                .expect("terminal block present")
                .resolved_color_emoji(),
            "explicit false disables color emoji"
        );
    }

    #[test]
    fn test_terminal_scrollback_lines_resolves_to_default_when_absent() {
        let config = parse_and_validate(r#"{"terminal": {}}"#);
        let tc = config.terminal.expect("terminal block present");
        assert_eq!(
            tc.resolved_scrollback_lines(),
            TerminalConfig::DEFAULT_SCROLLBACK_LINES
        );
    }

    #[test]
    fn test_terminal_scrollback_lines_clamps_out_of_range() {
        let tc = TerminalConfig {
            backend: Default::default(),
            ligatures: None,
            integrated_glyphs: None,
            color_emoji: None,
            cursor_color: None,
            scrollback_lines: Some(50), // below MIN_SCROLLBACK_LINES
            cursor_shape: None,
            cursor_blink: None,
            env: None,
            scroll_multiplier: None,
        };
        assert_eq!(
            tc.resolved_scrollback_lines(),
            TerminalConfig::MIN_SCROLLBACK_LINES
        );
        let tc = TerminalConfig {
            backend: Default::default(),
            ligatures: None,
            integrated_glyphs: None,
            color_emoji: None,
            cursor_color: None,
            scrollback_lines: Some(20_000_000), // way above MAX
            cursor_shape: None,
            cursor_blink: None,
            env: None,
            scroll_multiplier: None,
        };
        assert_eq!(
            tc.resolved_scrollback_lines(),
            TerminalConfig::MAX_SCROLLBACK_LINES
        );
    }

    // US-014: global terminal.env round-trips through parse + serialize.
    #[test]
    fn test_terminal_env_round_trip() {
        let config = parse_and_validate(
            r#"{"terminal": {"env": {"RUST_LOG": "debug", "ANTHROPIC_API_KEY": "sk-x"}}}"#,
        );
        let env = config
            .terminal
            .as_ref()
            .and_then(|t| t.env.as_ref())
            .expect("terminal.env must parse");
        assert_eq!(env.get("RUST_LOG").map(String::as_str), Some("debug"));
        assert_eq!(
            env.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-x")
        );

        // Survive a serialize → parse round-trip.
        let json = serde_json::to_string(&config).unwrap();
        let reparsed = parse_and_validate(&json);
        assert_eq!(reparsed.terminal, config.terminal);
    }

    // US-014: an absent env block resolves to None (no injection).
    #[test]
    fn test_terminal_env_absent_is_none() {
        let config = parse_and_validate(r#"{"terminal": {}}"#);
        assert!(
            config
                .terminal
                .expect("terminal block present")
                .env
                .is_none(),
            "US-014: absent terminal.env must be None"
        );
    }

    // US-022: scroll_multiplier resolver - default, clamp, in-range, round-trip.
    #[test]
    fn test_scroll_multiplier_resolver_default_and_clamp() {
        assert_eq!(
            TerminalConfig::default().resolved_scroll_multiplier(),
            1.0,
            "absent → default 1.0"
        );
        assert_eq!(
            TerminalConfig {
                scroll_multiplier: Some(0.01),
                ..Default::default()
            }
            .resolved_scroll_multiplier(),
            TerminalConfig::MIN_SCROLL_MULTIPLIER,
            "below min → clamped"
        );
        assert_eq!(
            TerminalConfig {
                scroll_multiplier: Some(99.0),
                ..Default::default()
            }
            .resolved_scroll_multiplier(),
            TerminalConfig::MAX_SCROLL_MULTIPLIER,
            "above max → clamped"
        );
        assert_eq!(
            TerminalConfig {
                scroll_multiplier: Some(2.5),
                ..Default::default()
            }
            .resolved_scroll_multiplier(),
            2.5,
            "in range → unchanged"
        );
    }

    #[test]
    fn test_scroll_multiplier_serde_roundtrip() {
        let config = parse_and_validate(r#"{"terminal": {"scroll_multiplier": 3.0}}"#);
        let tc = config.terminal.expect("terminal block present");
        assert_eq!(tc.scroll_multiplier, Some(3.0));
        assert_eq!(tc.resolved_scroll_multiplier(), 3.0);

        let absent = parse_and_validate(r#"{"terminal": {}}"#);
        let tc = absent.terminal.expect("terminal block present");
        assert!(tc.scroll_multiplier.is_none());
        assert_eq!(tc.resolved_scroll_multiplier(), 1.0);
    }

    #[test]
    fn test_terminal_ligatures_wrong_type_falls_back_to_defaults() {
        // A typo in one terminal field must not discard siblings or the whole
        // config. The bad bool resolves as absent, while valid neighbours stay.
        let config = parse_and_validate(
            r#"{"theme": "One Dark", "terminal": {"ligatures": "yes", "color_emoji": false}}"#,
        );
        assert_eq!(config.theme.as_deref(), Some("One Dark"));
        let terminal = config.terminal.expect("terminal block survives");
        assert_eq!(terminal.ligatures, None);
        assert_eq!(terminal.color_emoji, Some(false));
    }

    // US-007 (prd-agents-view.md): SessionState gained `projects`,
    // `active_project`, `mode`. The three tests below cover the AC
    // explicitly: round-trip with mixed state, backward-compat with a
    // pre-US-007 session.json, and AppMode enum serialisation.

    #[test]
    fn test_session_roundtrip_mixed_workspaces_and_projects() {
        let state = SessionState {
            version: 1,
            active_workspace: 0,
            workspaces: vec![WorkspaceSession {
                title: "main".to_string(),
                cwd: "/home/user".to_string(),
                layout: Some(LayoutNode::Pane {
                    surfaces: vec![make_surface("/home/user")],
                }),
                custom_buttons: vec![],
                expanded_paths: vec![],
                managed_worktrees: vec![],
            }],
            projects: vec![ProjectSession {
                id: 42,
                title: "Paneflow".to_string(),
                cwd: "/home/user/dev/paneflow".to_string(),
                is_expanded: true,
                threads: vec![ThreadSession {
                    id: 100,
                    title: "Wire up the agents view".to_string(),
                    agent: "claude_code".to_string(),
                    cwd: "/home/user/dev/paneflow".to_string(),
                    created_at: 1_716_336_000_000,
                    model: Some("sonnet".to_string()),
                    mode: Some("default".to_string()),
                    store_id: Some("uuid-abc-123".to_string()),
                    kind: None,
                    terminal_agent: None,
                    pinned: false,
                    session_id: None,
                    title_user_set: false,
                }],
            }],
            active_project: 0,
            chats: Vec::new(),
            agents_target: None,
            mode: AppMode::Agents,
            diff_scope: None,
        };
        let json = serde_json::to_string_pretty(&state).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
        assert_eq!(restored.projects[0].threads[0].agent, "claude_code");
        assert_eq!(restored.mode, AppMode::Agents);
    }

    // US-001/US-002 (prd-agents-ui-codex-redesign-2026-Q3.md): the
    // SessionState gained `chats` and ThreadSession gained `pinned`. These
    // cover the round-trip with both fields populated and the backward-compat
    // default when a pre-refonte session.json lacks the keys.
    #[test]
    fn test_session_roundtrip_with_chats_and_pinned() {
        let state = SessionState {
            version: 1,
            active_workspace: 0,
            workspaces: vec![],
            projects: vec![ProjectSession {
                id: 1,
                title: "Paneflow".to_string(),
                cwd: "/home/user/dev/paneflow".to_string(),
                is_expanded: true,
                threads: vec![ThreadSession {
                    id: 10,
                    title: "Pinned project thread".to_string(),
                    agent: "claude_code".to_string(),
                    cwd: "/home/user/dev/paneflow".to_string(),
                    created_at: 1_716_336_000_000,
                    model: None,
                    mode: None,
                    store_id: None,
                    kind: Some("terminal".to_string()),
                    terminal_agent: Some("claude_code".to_string()),
                    pinned: true,
                    session_id: None,
                    title_user_set: false,
                }],
            }],
            active_project: 0,
            chats: vec![ThreadSession {
                id: 20,
                title: "Quick scratch chat".to_string(),
                agent: "codex".to_string(),
                cwd: "/home/user".to_string(),
                created_at: 1_716_337_000_000,
                model: None,
                mode: None,
                store_id: None,
                kind: Some("terminal".to_string()),
                terminal_agent: Some("codex".to_string()),
                pinned: false,
                session_id: None,
                title_user_set: false,
            }],
            agents_target: None,
            mode: AppMode::Agents,
            diff_scope: None,
        };
        let json = serde_json::to_string_pretty(&state).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
        assert_eq!(restored.chats.len(), 1, "the free chat round-trips");
        assert_eq!(restored.chats[0].cwd, "/home/user", "chat anchored on home");
        assert!(
            restored.projects[0].threads[0].pinned,
            "the pinned flag round-trips on a project thread"
        );
        assert!(!restored.chats[0].pinned, "an unpinned chat stays unpinned");
    }

    #[test]
    fn test_session_pre_refonte_defaults_chats_empty_and_unpinned() {
        // A pre-refonte session.json: a project thread with no `pinned`
        // key, and no top-level `chats` key. Must restore as `chats = []`
        // and `pinned = false` everywhere - no migration, no error.
        let legacy = r#"{
            "version": 1,
            "active_workspace": 0,
            "workspaces": [],
            "projects": [
                {
                    "id": 1,
                    "title": "Paneflow",
                    "cwd": "/home/user/dev/paneflow",
                    "is_expanded": true,
                    "threads": [
                        {
                            "id": 10,
                            "title": "Old thread",
                            "agent": "claude_code",
                            "cwd": "/home/user/dev/paneflow",
                            "created_at": 0
                        }
                    ]
                }
            ],
            "active_project": 0,
            "mode": "agents"
        }"#;
        let restored: SessionState = serde_json::from_str(legacy).unwrap();
        assert!(restored.chats.is_empty(), "chats must default to []");
        assert!(
            !restored.projects[0].threads[0].pinned,
            "a thread with no `pinned` key restores as unpinned"
        );
    }

    #[test]
    fn test_session_backward_compat_pre_us007() {
        // A literal pre-US-007 session.json: no `projects`, no
        // `active_project`, no `mode` keys. Must deserialise to an
        // empty project list and the default `AppMode::Cli`.
        let legacy = r#"{
            "version": 1,
            "active_workspace": 0,
            "workspaces": [
                { "title": "main", "cwd": "/tmp", "layout": null }
            ]
        }"#;
        let restored: SessionState = serde_json::from_str(legacy).unwrap();
        assert_eq!(restored.workspaces.len(), 1);
        assert!(restored.projects.is_empty(), "projects must default to []");
        assert_eq!(restored.active_project, 0);
        assert_eq!(
            restored.mode,
            AppMode::Cli,
            "legacy session.json must restore in CLI mode"
        );
    }

    #[test]
    fn test_app_mode_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&AppMode::Cli).unwrap(), "\"cli\"");
        assert_eq!(
            serde_json::to_string(&AppMode::Agents).unwrap(),
            "\"agents\""
        );
        // US-001 (prd-git-diff-mode-2026-Q3.md): the third mode.
        assert_eq!(serde_json::to_string(&AppMode::Diff).unwrap(), "\"diff\"");
    }

    #[test]
    fn test_app_mode_diff_round_trips() {
        // US-001 (prd-git-diff-mode-2026-Q3.md): `Diff` survives a
        // serialize -> deserialize cycle and a session.json carrying it
        // restores into `AppMode::Diff` (not the `Cli` default).
        let json = serde_json::to_string(&AppMode::Diff).unwrap();
        let back: AppMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, AppMode::Diff);

        let session = r#"{
            "version": 1,
            "active_workspace": 0,
            "workspaces": [],
            "mode": "diff"
        }"#;
        let restored: SessionState = serde_json::from_str(session).unwrap();
        assert_eq!(restored.mode, AppMode::Diff);
    }

    #[test]
    fn test_session_diff_scope_round_trips_and_defaults() {
        // US-015 (prd-git-diff-mode-2026-Q3.md): diff_scope persists, and a
        // session.json written before this field restores it as `None`.
        let legacy = r#"{ "version": 1, "active_workspace": 0, "workspaces": [] }"#;
        let restored: SessionState = serde_json::from_str(legacy).unwrap();
        assert_eq!(restored.diff_scope, None);

        let with_scope = r#"{
            "version": 1,
            "active_workspace": 0,
            "workspaces": [],
            "diff_scope": "worktree"
        }"#;
        let restored2: SessionState = serde_json::from_str(with_scope).unwrap();
        assert_eq!(restored2.diff_scope.as_deref(), Some("worktree"));
    }

    #[test]
    fn test_project_session_is_expanded_defaults_true_when_absent() {
        // A ProjectSession written before `is_expanded` existed (or with
        // the key stripped) must restore expanded -- otherwise the
        // sidebar would silently hide threads on first relaunch.
        let json = r#"{
            "id": 7,
            "title": "Proj",
            "cwd": "/tmp",
            "threads": []
        }"#;
        let restored: ProjectSession = serde_json::from_str(json).unwrap();
        assert!(restored.is_expanded);
    }

    #[test]
    fn validate_layout_converts_legacy_2child_ratio_to_explicit_pair() {
        // U-007: a 2-child split's legacy `ratio` is promoted to an explicit
        // `ratios` pair so it survives restore instead of being dropped.
        let mut node = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: Some(0.3),
            ratios: None,
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        validate_layout(&mut node);
        match node {
            LayoutNode::Split { ratios, .. } => {
                let rs = ratios.expect("legacy ratio should be promoted to ratios");
                assert_eq!(rs.len(), 2);
                assert!((rs[0] - 0.3).abs() < 1e-6, "first ratio preserved: {rs:?}");
                assert!((rs[1] - 0.7).abs() < 1e-6, "second ratio = 1 - r: {rs:?}");
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn validate_layout_drops_legacy_ratio_on_nary_split() {
        // U-007: an N-ary split's legacy `ratio` is ambiguous; it stays unset
        // (a warn is logged) rather than being silently honored.
        let mut node = LayoutNode::Split {
            direction: "horizontal".to_string(),
            ratio: Some(0.3),
            ratios: None,
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        validate_layout(&mut node);
        match node {
            LayoutNode::Split { ratios, .. } => {
                assert!(ratios.is_none(), "N-ary legacy ratio must not be converted")
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn validate_layout_normalizes_unknown_split_direction() {
        let mut node = LayoutNode::Split {
            direction: "diagonal".to_string(),
            ratio: None,
            ratios: None,
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        validate_layout(&mut node);
        match node {
            LayoutNode::Split { direction, .. } => assert_eq!(direction, "horizontal"),
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn validate_layout_caps_total_leaves() {
        // U-008/U-016: an over-broad layout is trimmed to MAX_LAYOUT_LEAVES.
        let mut node = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: None,
            ratios: None,
            children: (0..100)
                .map(|_| LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                })
                .collect(),
        };
        validate_layout(&mut node);
        assert!(
            node.leaf_count() <= MAX_LAYOUT_LEAVES,
            "got {} leaves",
            node.leaf_count()
        );
    }

    #[test]
    fn validate_layout_caps_surfaces_per_pane() {
        // U-008: a pane is one leaf, but each surface spawns a terminal - cap it.
        let mut node = LayoutNode::Pane {
            surfaces: (0..200).map(|_| Default::default()).collect(),
        };
        validate_layout(&mut node);
        match node {
            LayoutNode::Pane { surfaces, .. } => assert!(surfaces.len() <= MAX_PANE_SURFACES),
            _ => panic!("expected pane"),
        }
    }

    #[test]
    fn validate_layout_keeps_only_first_surface_focus() {
        let focused_surface = SurfaceDefinition {
            focus: Some(true),
            ..Default::default()
        };
        let mut node = LayoutNode::Pane {
            surfaces: vec![
                Default::default(),
                focused_surface.clone(),
                focused_surface.clone(),
                Default::default(),
            ],
        };

        validate_layout(&mut node);

        match node {
            LayoutNode::Pane { surfaces, .. } => {
                assert_eq!(surfaces[0].focus, None);
                assert_eq!(surfaces[1].focus, Some(true));
                assert_eq!(surfaces[2].focus, None);
                assert_eq!(surfaces[3].focus, None);
            }
            _ => panic!("expected pane"),
        }
    }
}
