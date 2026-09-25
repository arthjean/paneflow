use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use paneflow_agent_config::claude_hooks::MANAGED_MARKER;
use paneflow_agent_config::{
    runtime_by_command_alias, runtime_by_slug, Runtime, RuntimeHookAdapter,
    RuntimeLifecycleAuthority, RUNTIMES,
};
use serde_json::{json, Value};

use crate::{io, merge};

const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "StopFailure",
    "PreToolUse",
    "PermissionRequest",
    "SubagentStart",
    "SubagentStop",
    "Notification",
];
const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "Stop",
    "Interrupt",
    "SubagentStart",
    "SubagentStop",
    "SessionEnd",
];
const CLAUDE_RETIRED_EVENTS: &[&str] = &["PostToolUse"];
const CODEX_RETIRED_EVENTS: &[&str] = &["PostToolUse"];
const UNMARKED_HOOKS_ADOPTION: &str = "Paneflow hook entries without an integration marker";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrationState {
    Installed,
    NotInstalled,
    UnsupportedPlatform,
    DetectionOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationStatus {
    pub id: &'static str,
    pub slug: &'static str,
    pub label: &'static str,
    pub summary: &'static str,
    pub post_install_step: Option<&'static str>,
    pub state: IntegrationState,
}

#[derive(Clone, Debug)]
pub struct IntegrationBinaries {
    pub hook_binary: PathBuf,
    pub bridge_binary: PathBuf,
}

#[derive(Clone, Debug)]
struct ConfigPaths {
    marker_home: PathBuf,
    claude_settings: PathBuf,
    claude_mcp: PathBuf,
    codex_hooks: PathBuf,
    codex_config: PathBuf,
}

impl ConfigPaths {
    fn resolve() -> Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("cannot resolve the user home"))?;
        let marker_home = paneflow_home::paneflow_home()
            .ok_or_else(|| anyhow!("cannot resolve PANEFLOW_HOME"))?;
        let claude_settings = paneflow_agent_config::claude_config_dir()
            .ok_or_else(|| anyhow!("cannot resolve the Claude config directory"))?
            .join("settings.json");
        let codex_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| home.join(".codex"));
        Ok(Self {
            marker_home,
            claude_settings,
            claude_mcp: home.join(".claude.json"),
            codex_hooks: codex_home.join("hooks.json"),
            codex_config: codex_home.join("config.toml"),
        })
    }

    fn marker(&self, slug: &str) -> PathBuf {
        self.marker_home
            .join("integrations")
            .join(format!("{slug}.json"))
    }
}

pub fn list_integrations() -> Vec<IntegrationStatus> {
    match ConfigPaths::resolve() {
        Ok(paths) => list_integrations_in(&paths),
        Err(_) => RUNTIMES
            .iter()
            .map(|runtime| status_for(runtime, None))
            .collect(),
    }
}

fn list_integrations_in(paths: &ConfigPaths) -> Vec<IntegrationStatus> {
    RUNTIMES
        .iter()
        .map(|runtime| status_for(runtime, Some(paths)))
        .collect()
}

fn status_for(runtime: &'static Runtime, paths: Option<&ConfigPaths>) -> IntegrationStatus {
    let state = if !runtime.supports_current_platform() {
        IntegrationState::UnsupportedPlatform
    } else if !has_installer(runtime) {
        IntegrationState::DetectionOnly
    } else if paths.is_some_and(|paths| {
        paths.marker(runtime.slug).is_file() && installed_evidence(paths, runtime)
    }) {
        IntegrationState::Installed
    } else {
        IntegrationState::NotInstalled
    };
    IntegrationStatus {
        id: runtime.id,
        slug: runtime.slug,
        label: runtime.label,
        summary: runtime.integration.summary,
        post_install_step: runtime.integration.post_install_step,
        state,
    }
}

fn installed_evidence(paths: &ConfigPaths, runtime: &Runtime) -> bool {
    if !owned_hooks_present(paths, runtime) {
        return false;
    }
    match runtime.integration.hook_adapter {
        RuntimeHookAdapter::Claude => merge::read_json_or_default(&paths.claude_mcp)
            .ok()
            .and_then(|root| root.pointer("/mcpServers/paneflow").cloned())
            .is_some(),
        RuntimeHookAdapter::Codex => merge::read_toml_or_default(&paths.codex_config)
            .ok()
            .and_then(|document| {
                document
                    .get("mcp_servers")?
                    .get("paneflow")?
                    .get("command")?
                    .as_str()
                    .map(str::to_owned)
            })
            .is_some(),
        _ => false,
    }
}

fn has_installer(runtime: &Runtime) -> bool {
    runtime.lifecycle.authority != RuntimeLifecycleAuthority::None
        && matches!(
            runtime.integration.hook_adapter,
            RuntimeHookAdapter::Claude | RuntimeHookAdapter::Codex
        )
}

pub fn install_integration(
    slug: &str,
    binaries: &IntegrationBinaries,
) -> Result<IntegrationStatus, String> {
    let paths = ConfigPaths::resolve().map_err(format_error)?;
    install_integration_at(&paths, slug, binaries, None).map_err(format_error)
}

fn install_integration_at(
    paths: &ConfigPaths,
    slug: &str,
    binaries: &IntegrationBinaries,
    adopted_from: Option<&str>,
) -> Result<IntegrationStatus> {
    let runtime = integration_runtime(slug)?;
    if !runtime.supports_current_platform() {
        bail!(
            "{} integration is not supported on this platform",
            runtime.label
        );
    }
    if !has_installer(runtime) {
        bail!("{} offers detection only", runtime.label);
    }
    if !binaries.hook_binary.is_file() {
        bail!(
            "hook reporter is missing at {}",
            binaries.hook_binary.display()
        );
    }
    if !binaries.bridge_binary.is_file() {
        bail!(
            "MCP bridge is missing at {}",
            binaries.bridge_binary.display()
        );
    }
    preflight_install(paths, runtime, binaries)?;
    let unix_reporter = write_unix_reporter(&binaries.hook_binary)?;
    match runtime.integration.hook_adapter {
        RuntimeHookAdapter::Claude => {
            install_claude(paths, &unix_reporter, binaries)?;
        }
        RuntimeHookAdapter::Codex => {
            install_codex(paths, &unix_reporter, binaries)?;
        }
        _ => bail!("{} offers detection only", runtime.label),
    }
    write_marker(paths, runtime.slug, adopted_from)?;
    Ok(status_for(runtime, Some(paths)))
}

fn preflight_install(
    paths: &ConfigPaths,
    runtime: &Runtime,
    binaries: &IntegrationBinaries,
) -> Result<()> {
    let reporter = binaries
        .hook_binary
        .parent()
        .ok_or_else(|| anyhow!("hook reporter path has no parent"))?
        .join("paneflow-ai-hook.sh");
    refuse_symlink(&reporter)?;
    refuse_symlink(&paths.marker(runtime.slug))?;
    match runtime.integration.hook_adapter {
        RuntimeHookAdapter::Claude => {
            refuse_symlink(&paths.claude_settings)?;
            refuse_symlink(&paths.claude_mcp)?;
            for path in [&paths.claude_settings, &paths.claude_mcp] {
                if !merge::read_json_or_default(path)?.is_object() {
                    bail!("{}: config root is not a JSON object", path.display());
                }
            }
        }
        RuntimeHookAdapter::Codex => {
            refuse_symlink(&paths.codex_hooks)?;
            refuse_symlink(&paths.codex_config)?;
            if !merge::read_json_or_default(&paths.codex_hooks)?.is_object() {
                bail!(
                    "{}: config root is not a JSON object",
                    paths.codex_hooks.display()
                );
            }
            let _ = merge::read_toml_or_default(&paths.codex_config)?;
        }
        _ => bail!("{} offers detection only", runtime.label),
    }
    Ok(())
}

pub fn remove_integration(slug: &str) -> Result<IntegrationStatus, String> {
    let paths = ConfigPaths::resolve().map_err(format_error)?;
    remove_integration_at(&paths, slug).map_err(format_error)
}

pub fn run_integrations_cli(args: &[String], binaries: Option<IntegrationBinaries>) -> i32 {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => {
            for status in list_integrations() {
                println!("{}\t{}", status.slug, state_label(status.state));
            }
            0
        }
        Some("install") if args.len() == 2 => {
            let Some(binaries) = binaries else {
                eprintln!("paneflow integrations: embedded integration binaries are unavailable");
                return 1;
            };
            match install_integration(&args[1], &binaries) {
                Ok(status) => {
                    println!("{}: installed", status.label);
                    if let Some(step) = status.post_install_step {
                        println!("{step}");
                    }
                    0
                }
                Err(error) => {
                    eprintln!("paneflow integrations: {error}");
                    1
                }
            }
        }
        Some("remove") if args.len() == 2 => match remove_integration(&args[1]) {
            Ok(status) => {
                println!("{}: not installed", status.label);
                0
            }
            Err(error) => {
                eprintln!("paneflow integrations: {error}");
                1
            }
        },
        Some("--help" | "-h") if args.len() == 1 => {
            print_integrations_help();
            0
        }
        _ => {
            print_integrations_help();
            2
        }
    }
}

pub(crate) fn state_label(state: IntegrationState) -> &'static str {
    match state {
        IntegrationState::Installed => "installed",
        IntegrationState::NotInstalled => "not installed",
        IntegrationState::UnsupportedPlatform => "not supported on this platform",
        IntegrationState::DetectionOnly => "no integration (detection only)",
    }
}

fn print_integrations_help() {
    println!(
        "Usage: paneflow integrations <COMMAND>\n\nCommands:\n  list\n  install <runtime>\n  remove <runtime>"
    );
}

fn remove_integration_at(paths: &ConfigPaths, slug: &str) -> Result<IntegrationStatus> {
    let runtime = integration_runtime(slug)?;
    if !has_installer(runtime) {
        bail!("{} offers detection only", runtime.label);
    }
    match runtime.integration.hook_adapter {
        RuntimeHookAdapter::Claude => remove_claude(paths)?,
        RuntimeHookAdapter::Codex => remove_codex(paths)?,
        _ => bail!("{} offers detection only", runtime.label),
    }
    let marker = paths.marker(runtime.slug);
    if let Err(error) = std::fs::remove_file(&marker) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error).with_context(|| format!("remove {}", marker.display()));
        }
    }
    Ok(status_for(runtime, Some(paths)))
}

fn integration_runtime(slug_or_command: &str) -> Result<&'static Runtime> {
    runtime_by_slug(slug_or_command)
        .or_else(|| runtime_by_command_alias(slug_or_command))
        .ok_or_else(|| anyhow!("unknown runtime '{slug_or_command}'"))
}

pub fn adopt_and_refresh_installed(
    binaries: &IntegrationBinaries,
) -> Vec<(String, Result<(), String>)> {
    let Ok(paths) = ConfigPaths::resolve() else {
        return Vec::new();
    };
    let mut results = Vec::new();
    for runtime in RUNTIMES.iter().filter(|runtime| has_installer(runtime)) {
        let marker = paths.marker(runtime.slug);
        let adoption = (!marker.exists() && owned_hooks_present(&paths, runtime))
            .then_some(UNMARKED_HOOKS_ADOPTION);
        if marker.exists() || adoption.is_some() {
            let result = install_integration_at(&paths, runtime.slug, binaries, adoption)
                .map(|_| ())
                .map_err(|error| format!("{error:#}"));
            results.push((runtime.slug.to_string(), result));
        }
    }
    results
}

pub(crate) fn has_owned_hooks(slug: &str) -> bool {
    let Ok(paths) = ConfigPaths::resolve() else {
        return false;
    };
    integration_runtime(slug).is_ok_and(|runtime| owned_hooks_present(&paths, runtime))
}

pub(crate) fn runtime_detected(slug: &str) -> bool {
    let Ok(runtime) = integration_runtime(slug) else {
        return false;
    };
    if runtime
        .detection
        .command_aliases
        .iter()
        .any(|alias| which::which(alias).is_ok())
    {
        return true;
    }
    let Ok(paths) = ConfigPaths::resolve() else {
        return false;
    };
    let config_dir = match runtime.integration.hook_adapter {
        RuntimeHookAdapter::Claude => paths.claude_settings.parent(),
        RuntimeHookAdapter::Codex => paths.codex_hooks.parent(),
        _ => None,
    };
    config_dir.is_some_and(Path::exists)
}

fn owned_hooks_present(paths: &ConfigPaths, runtime: &Runtime) -> bool {
    let path = match runtime.integration.hook_adapter {
        RuntimeHookAdapter::Claude => &paths.claude_settings,
        RuntimeHookAdapter::Codex => &paths.codex_hooks,
        _ => return false,
    };
    merge::read_json_or_default(path)
        .ok()
        .and_then(|root| root.get("hooks").and_then(Value::as_object).cloned())
        .is_some_and(|hooks| {
            hooks.values().any(|entries| {
                entries
                    .as_array()
                    .is_some_and(|entries| entries.iter().any(is_owned_group))
            })
        })
}

fn write_marker(paths: &ConfigPaths, slug: &str, adopted_from: Option<&str>) -> Result<()> {
    let marker = paths.marker(slug);
    refuse_symlink(&marker)?;
    let value = json!({
        "schema": 1,
        "runtime": slug,
        "installed_at_ms": epoch_millis(),
        "adopted_from": adopted_from,
    });
    io::write_if_changed(&marker, &merge::json_to_bytes(&value)?)?;
    Ok(())
}

fn epoch_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn write_unix_reporter(hook_binary: &Path) -> Result<PathBuf> {
    let parent = hook_binary
        .parent()
        .ok_or_else(|| anyhow!("hook reporter path has no parent"))?;
    let path = parent.join("paneflow-ai-hook.sh");
    refuse_symlink(&path)?;
    io::write_if_changed(
        &path,
        include_bytes!("../../../runtimes/claude-code/assets/hooks/lifecycle.sh"),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}

fn reporter_for_current_platform<'a>(unix_reporter: &'a Path, hook_binary: &'a Path) -> &'a Path {
    if cfg!(windows) {
        hook_binary
    } else {
        unix_reporter
    }
}

fn install_claude(
    paths: &ConfigPaths,
    unix_reporter: &Path,
    binaries: &IntegrationBinaries,
) -> Result<()> {
    refuse_symlink(&paths.claude_settings)?;
    io::with_config_lock(&paths.claude_settings, || {
        let mut root = merge::read_json_or_default(&paths.claude_settings)?;
        reconcile_claude_hooks(
            &mut root,
            reporter_for_current_platform(unix_reporter, &binaries.hook_binary),
        )?;
        io::write_if_changed_unlocked(&paths.claude_settings, &merge::json_to_bytes(&root)?)?;
        Ok(())
    })?;
    refuse_symlink(&paths.claude_mcp)?;
    io::with_config_lock(&paths.claude_mcp, || {
        let mut root = merge::read_json_or_default(&paths.claude_mcp)?;
        let servers = root
            .as_object_mut()
            .context("Claude config root is not an object")?
            .entry("mcpServers")
            .or_insert_with(|| json!({}));
        let servers = servers
            .as_object_mut()
            .context("Claude mcpServers is not an object")?;
        for legacy in ["paneflow-dev", "paneflow-mcp"] {
            servers.remove(legacy);
        }
        servers.insert(
            "paneflow".to_string(),
            json!({
                "type": "stdio",
                "command": binaries.bridge_binary.display().to_string(),
                "args": [],
            }),
        );
        io::write_if_changed_unlocked(&paths.claude_mcp, &merge::json_to_bytes(&root)?)?;
        Ok(())
    })
}

fn install_codex(
    paths: &ConfigPaths,
    unix_reporter: &Path,
    binaries: &IntegrationBinaries,
) -> Result<()> {
    refuse_symlink(&paths.codex_hooks)?;
    io::with_config_lock(&paths.codex_hooks, || {
        let mut root = merge::read_json_or_default(&paths.codex_hooks)?;
        reconcile_codex_hooks(&mut root, unix_reporter, &binaries.hook_binary)?;
        io::write_if_changed_unlocked(&paths.codex_hooks, &merge::json_to_bytes(&root)?)?;
        Ok(())
    })?;
    refuse_symlink(&paths.codex_config)?;
    io::with_config_lock(&paths.codex_config, || {
        let mut document = merge::read_toml_or_default(&paths.codex_config)?;
        merge::upsert_toml_entry(
            &mut document,
            "mcp_servers",
            "paneflow",
            &binaries.bridge_binary.display().to_string(),
            &[],
        )?;
        io::write_if_changed_unlocked(&paths.codex_config, &merge::toml_to_bytes(&document))?;
        Ok(())
    })
}

fn remove_claude(paths: &ConfigPaths) -> Result<()> {
    remove_json_hooks(
        &paths.claude_settings,
        &owned_events(CLAUDE_EVENTS, CLAUDE_RETIRED_EVENTS),
    )?;
    refuse_symlink(&paths.claude_mcp)?;
    if paths.claude_mcp.exists() {
        io::with_config_lock(&paths.claude_mcp, || {
            let mut root = merge::read_json_or_default(&paths.claude_mcp)?;
            merge::remove_json_entry(&mut root, "mcpServers", "paneflow");
            io::write_if_changed_unlocked(&paths.claude_mcp, &merge::json_to_bytes(&root)?)?;
            Ok(())
        })?;
    }
    Ok(())
}

fn remove_codex(paths: &ConfigPaths) -> Result<()> {
    remove_json_hooks(
        &paths.codex_hooks,
        &owned_events(CODEX_EVENTS, CODEX_RETIRED_EVENTS),
    )?;
    refuse_symlink(&paths.codex_config)?;
    if paths.codex_config.exists() {
        io::with_config_lock(&paths.codex_config, || {
            let mut document = merge::read_toml_or_default(&paths.codex_config)?;
            merge::remove_toml_entry(&mut document, "mcp_servers", "paneflow");
            io::write_if_changed_unlocked(&paths.codex_config, &merge::toml_to_bytes(&document))?;
            Ok(())
        })?;
    }
    Ok(())
}

fn owned_events(current: &[&'static str], retired: &[&'static str]) -> Vec<&'static str> {
    current
        .iter()
        .copied()
        .chain(retired.iter().copied())
        .collect()
}

fn remove_json_hooks(path: &Path, events: &[&str]) -> Result<()> {
    refuse_symlink(path)?;
    if !path.exists() {
        return Ok(());
    }
    io::with_config_lock(path, || {
        let mut root = merge::read_json_or_default(path)?;
        remove_owned_hooks(&mut root, events)?;
        io::write_if_changed_unlocked(path, &merge::json_to_bytes(&root)?)?;
        Ok(())
    })
}

fn reconcile_claude_hooks(root: &mut Value, reporter: &Path) -> Result<()> {
    let hooks = hooks_object(root)?;
    for event in CLAUDE_RETIRED_EVENTS {
        sweep_owned_hooks(hooks, event)?;
    }
    for event in CLAUDE_EVENTS {
        let entries = hook_entries(hooks, event)?;
        strip_owned_handlers(entries);
        let mut group = json!({
            MANAGED_MARKER: true,
            "hooks": [{
                "type": "command",
                "command": reporter.display().to_string(),
                "args": [event],
                "timeout": 5,
                "async": false,
            }],
        });
        if *event == "PermissionRequest" {
            group["matcher"] = Value::String("*".to_string());
        } else if *event == "PreToolUse" {
            group["matcher"] = Value::String("AskUserQuestion".to_string());
        }
        entries.push(group);
    }
    hooks.retain(|_, entries| !entries.as_array().is_some_and(Vec::is_empty));
    Ok(())
}

fn sweep_owned_hooks(hooks: &mut serde_json::Map<String, Value>, event: &str) -> Result<()> {
    let Some(entries) = hooks.get_mut(event) else {
        return Ok(());
    };
    strip_owned_handlers(
        entries
            .as_array_mut()
            .with_context(|| format!("hook event `{event}` is not an array"))?,
    );
    Ok(())
}

fn reconcile_codex_hooks(
    root: &mut Value,
    unix_reporter: &Path,
    windows_reporter: &Path,
) -> Result<()> {
    let hooks = hooks_object(root)?;
    for event in CODEX_RETIRED_EVENTS {
        sweep_owned_hooks(hooks, event)?;
    }
    for event in CODEX_EVENTS {
        let entries = hook_entries(hooks, event)?;
        strip_owned_handlers(entries);
        let timeout = if matches!(*event, "Interrupt" | "SessionEnd") {
            1
        } else {
            5
        };
        let mut group = json!({
            MANAGED_MARKER: true,
            "hooks": [{
                "type": "command",
                "command": format!("{} {}", unix_reporter.display(), event),
                "command_windows": format!("{} {}", windows_reporter.display(), event),
                "timeout": timeout,
            }],
        });
        if *event == "PermissionRequest" {
            group["matcher"] = Value::String("*".to_string());
        }
        entries.push(group);
    }
    hooks.retain(|_, entries| !entries.as_array().is_some_and(Vec::is_empty));
    Ok(())
}

fn hooks_object(root: &mut Value) -> Result<&mut serde_json::Map<String, Value>> {
    let root = root
        .as_object_mut()
        .context("config root is not an object")?;
    root.entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("config key `hooks` is not an object")
}

fn hook_entries<'a>(
    hooks: &'a mut serde_json::Map<String, Value>,
    event: &str,
) -> Result<&'a mut Vec<Value>> {
    hooks
        .entry(event.to_string())
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .with_context(|| format!("hook event `{event}` is not an array"))
}

fn remove_owned_hooks(root: &mut Value, events: &[&str]) -> Result<()> {
    let root_object = root
        .as_object_mut()
        .context("config root is not an object")?;
    let Some(hooks) = root_object.get_mut("hooks") else {
        return Ok(());
    };
    let hooks = hooks
        .as_object_mut()
        .context("config key `hooks` is not an object")?;
    for event in events {
        let Some(entries) = hooks.get_mut(*event) else {
            continue;
        };
        let entries = entries
            .as_array_mut()
            .with_context(|| format!("hook event `{event}` is not an array"))?;
        strip_owned_handlers(entries);
    }
    hooks.retain(|_, entries| !entries.as_array().is_some_and(Vec::is_empty));
    if hooks.is_empty() {
        root_object.remove("hooks");
    }
    Ok(())
}

fn is_owned_group(group: &Value) -> bool {
    group.get(MANAGED_MARKER).and_then(Value::as_bool) == Some(true)
        || group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|hooks| hooks.iter().any(is_owned_hook))
}

fn is_owned_hook(hook: &Value) -> bool {
    ["command", "command_windows", "commandWindows"]
        .iter()
        .filter_map(|key| hook.get(*key).and_then(Value::as_str))
        .any(is_paneflow_reporter_command)
}

fn strip_owned_handlers(entries: &mut Vec<Value>) {
    entries.retain_mut(|entry| {
        let Some(group) = entry.as_object_mut() else {
            return true;
        };
        let marked = group.get(MANAGED_MARKER).and_then(Value::as_bool) == Some(true);
        if marked {
            group.remove(MANAGED_MARKER);
        }
        let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            return !marked;
        };
        let before = hooks.len();
        hooks.retain(|hook| !is_owned_hook(hook));
        !(hooks.is_empty() && (marked || hooks.len() < before))
    });
}

fn is_paneflow_reporter_command(command: &str) -> bool {
    command.contains("paneflow-ai-hook")
}

fn refuse_symlink(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refused: {} is a symlink", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn format_error(error: anyhow::Error) -> String {
    format!("{error:#}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, ConfigPaths, IntegrationBinaries) {
        let directory = tempfile::tempdir().expect("temp directory");
        let root = directory.path();
        let bin = root.join("paneflow-home").join("bin");
        std::fs::create_dir_all(&bin).expect("bin directory");
        let hook_binary = bin.join(if cfg!(windows) {
            "paneflow-ai-hook.exe"
        } else {
            "paneflow-ai-hook"
        });
        let bridge_binary = bin.join(if cfg!(windows) {
            "paneflow-mcp.exe"
        } else {
            "paneflow-mcp"
        });
        std::fs::write(&hook_binary, b"hook").expect("hook binary");
        std::fs::write(&bridge_binary, b"bridge").expect("bridge binary");
        let paths = ConfigPaths {
            marker_home: root.join("paneflow-home"),
            claude_settings: root.join("claude").join("settings.json"),
            claude_mcp: root.join(".claude.json"),
            codex_hooks: root.join("codex").join("hooks.json"),
            codex_config: root.join("codex").join("config.toml"),
        };
        (
            directory,
            paths,
            IntegrationBinaries {
                hook_binary,
                bridge_binary,
            },
        )
    }

    #[test]
    fn claude_install_is_idempotent_and_preserves_foreign_entries() {
        let (_directory, paths, binaries) = fixture();
        std::fs::create_dir_all(paths.claude_settings.parent().expect("parent"))
            .expect("config directory");
        std::fs::write(
            &paths.claude_settings,
            br#"{"theme":"dark","hooks":{"Stop":[{"hooks":[{"type":"command","command":"my-hook"}]}]}}"#,
        )
        .expect("foreign config");
        install_integration_at(&paths, "claude-code", &binaries, None).expect("install");
        let first = std::fs::read(&paths.claude_settings).expect("first config");
        install_integration_at(&paths, "claude-code", &binaries, None).expect("reinstall");
        let second = std::fs::read(&paths.claude_settings).expect("second config");
        assert_eq!(first, second);
        let root: Value = serde_json::from_slice(&first).expect("valid JSON");
        assert_eq!(root["theme"], "dark");
        assert!(root["hooks"]["Stop"].as_array().is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry["hooks"][0]["command"] == "my-hook")
        }));
        let permission = root["hooks"]["PermissionRequest"]
            .as_array()
            .and_then(|entries| entries.last())
            .expect("permission group");
        assert_eq!(permission["matcher"], "*");
        assert_eq!(permission["hooks"][0]["args"], json!(["PermissionRequest"]));
        assert_eq!(permission["hooks"][0]["async"], false);
        let question = root["hooks"]["PreToolUse"]
            .as_array()
            .and_then(|entries| entries.last())
            .expect("AskUserQuestion group");
        assert_eq!(question["matcher"], "AskUserQuestion");
        assert_eq!(question["hooks"][0]["args"], json!(["PreToolUse"]));
    }

    #[test]
    fn adoption_rewrites_every_legacy_event_including_ones_it_no_longer_manages() {
        let (_directory, paths, binaries) = fixture();
        std::fs::create_dir_all(paths.claude_settings.parent().expect("parent"))
            .expect("config directory");
        std::fs::write(
            &paths.claude_settings,
            br#"{"hooks":{"PostToolUse":[{"_paneflow_managed":true,"hooks":[{"type":"command","command":"powershell.exe -NoProfile -Command C:/x/paneflow-ai-hook PostToolUse","timeout":5}]},{"hooks":[{"type":"command","command":"keep-me"}]}]}}"#,
        )
        .expect("legacy config");
        install_integration_at(
            &paths,
            "claude-code",
            &binaries,
            Some(UNMARKED_HOOKS_ADOPTION),
        )
        .expect("adopt");
        let root: Value =
            serde_json::from_slice(&std::fs::read(&paths.claude_settings).expect("settings"))
                .expect("settings JSON");
        let post_tool_use = root["hooks"]["PostToolUse"]
            .as_array()
            .expect("the foreign PostToolUse entry survives");
        assert_eq!(post_tool_use.len(), 1);
        assert_eq!(post_tool_use[0]["hooks"][0]["command"], "keep-me");
        assert!(
            !std::fs::read_to_string(&paths.claude_settings)
                .expect("settings text")
                .contains("powershell.exe"),
            "no shell-form Paneflow reporter survives adoption"
        );
        remove_integration_at(&paths, "claude-code").expect("remove");
        let root: Value =
            serde_json::from_slice(&std::fs::read(&paths.claude_settings).expect("settings"))
                .expect("settings JSON");
        assert_eq!(
            root["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
            "keep-me"
        );
    }

    #[test]
    fn codex_install_preserves_toml_comments_and_is_byte_stable() {
        let (_directory, paths, binaries) = fixture();
        std::fs::create_dir_all(paths.codex_config.parent().expect("parent"))
            .expect("config directory");
        std::fs::write(
            &paths.codex_config,
            "# keep\nmodel = \"gpt-6\"\n\n[mcp_servers.other]\ncommand = \"other\"\n",
        )
        .expect("foreign config");
        install_integration_at(&paths, "codex", &binaries, None).expect("install");
        let hooks = std::fs::read(&paths.codex_hooks).expect("hooks");
        let config = std::fs::read(&paths.codex_config).expect("config");
        install_integration_at(&paths, "codex", &binaries, None).expect("reinstall");
        assert_eq!(
            hooks,
            std::fs::read(&paths.codex_hooks).expect("hooks again")
        );
        assert_eq!(
            config,
            std::fs::read(&paths.codex_config).expect("config again")
        );
        let text = String::from_utf8(config).expect("UTF-8");
        assert!(text.contains("# keep"));
        assert!(text.contains("mcp_servers.other"));
        assert!(!text.contains("env ="));
        let root: Value = serde_json::from_slice(&hooks).expect("hooks JSON");
        assert_eq!(root["hooks"]["Interrupt"][0]["hooks"][0]["timeout"], 1);
        assert_eq!(root["hooks"]["SessionEnd"][0]["hooks"][0]["timeout"], 1);
        assert!(root.get("notify").is_none());
    }

    #[test]
    fn codex_adoption_drops_the_retired_legacy_event_and_keeps_foreign_entries() {
        let (_directory, paths, binaries) = fixture();
        std::fs::create_dir_all(paths.codex_hooks.parent().expect("parent"))
            .expect("config directory");
        std::fs::write(
            &paths.codex_hooks,
            br#"{"hooks":{"PostToolUse":[{"_paneflow_managed":true,"hooks":[{"type":"command","command":"/old/bin/paneflow-ai-hook PostToolUse"}]},{"hooks":[{"type":"command","command":"keep-me"}]}]}}"#,
        )
        .expect("legacy config");
        assert!(owned_hooks_present(
            &paths,
            runtime_by_slug("codex").expect("runtime")
        ));
        install_integration_at(&paths, "codex", &binaries, Some(UNMARKED_HOOKS_ADOPTION))
            .expect("adopt");
        let root: Value =
            serde_json::from_slice(&std::fs::read(&paths.codex_hooks).expect("hooks"))
                .expect("hooks JSON");
        let post_tool_use = root["hooks"]["PostToolUse"]
            .as_array()
            .expect("the foreign PostToolUse entry survives");
        assert_eq!(post_tool_use.len(), 1);
        assert_eq!(post_tool_use[0]["hooks"][0]["command"], "keep-me");
        remove_integration_at(&paths, "codex").expect("remove");
        let root: Value =
            serde_json::from_slice(&std::fs::read(&paths.codex_hooks).expect("hooks"))
                .expect("hooks JSON");
        assert_eq!(
            root["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
            "keep-me"
        );
    }

    #[test]
    fn remove_keeps_foreign_hooks_and_reporter_assets() {
        let (_directory, paths, binaries) = fixture();
        install_integration_at(&paths, "claude-code", &binaries, None).expect("install");
        let reporter = binaries
            .hook_binary
            .parent()
            .expect("bin")
            .join("paneflow-ai-hook.sh");
        remove_integration_at(&paths, "claude-code").expect("remove");
        assert!(reporter.exists());
        assert_eq!(
            status_for(
                runtime_by_slug("claude-code").expect("runtime"),
                Some(&paths)
            )
            .state,
            IntegrationState::NotInstalled
        );
    }

    #[test]
    fn a_legacy_hook_install_is_recorded_as_adopted() {
        let (_directory, paths, binaries) = fixture();
        std::fs::create_dir_all(paths.claude_settings.parent().expect("parent"))
            .expect("config directory");
        std::fs::write(
            &paths.claude_settings,
            br#"{"hooks":{"Stop":[{"_paneflow_managed":true,"hooks":[{"command":"paneflow-ai-hook Stop"}]}]}}"#,
        )
        .expect("legacy config");
        assert!(owned_hooks_present(
            &paths,
            runtime_by_slug("claude-code").expect("runtime")
        ));
        install_integration_at(&paths, "claude", &binaries, Some(UNMARKED_HOOKS_ADOPTION))
            .expect("adopt");
        let marker: Value =
            serde_json::from_slice(&std::fs::read(paths.marker("claude-code")).expect("marker"))
                .expect("marker JSON");
        assert_eq!(marker["adopted_from"], UNMARKED_HOOKS_ADOPTION);
    }

    #[test]
    fn concurrent_installs_serialize_to_the_same_files_as_one_install() {
        let (_directory, paths, binaries) = fixture();
        let first_paths = paths.clone();
        let first_binaries = binaries.clone();
        let first = std::thread::spawn(move || {
            install_integration_at(&first_paths, "codex", &first_binaries, None)
        });
        let second_paths = paths.clone();
        let second_binaries = binaries.clone();
        let second = std::thread::spawn(move || {
            install_integration_at(&second_paths, "codex", &second_binaries, None)
        });
        first.join().expect("first thread").expect("first install");
        second
            .join()
            .expect("second thread")
            .expect("second install");
        let hooks = std::fs::read(&paths.codex_hooks).expect("hooks");
        let config = std::fs::read(&paths.codex_config).expect("config");
        install_integration_at(&paths, "codex", &binaries, None).expect("single install");
        assert_eq!(
            hooks,
            std::fs::read(&paths.codex_hooks).expect("hooks again")
        );
        assert_eq!(
            config,
            std::fs::read(&paths.codex_config).expect("config again")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_config_is_refused_without_touching_its_target() {
        let (_directory, paths, binaries) = fixture();
        std::fs::create_dir_all(paths.claude_settings.parent().expect("parent"))
            .expect("config directory");
        let target = paths.marker_home.join("victim.json");
        std::fs::write(&target, b"{\"keep\":true}").expect("victim");
        std::os::unix::fs::symlink(&target, &paths.claude_settings).expect("symlink");
        let error = install_integration_at(&paths, "claude-code", &binaries, None)
            .expect_err("symlink refused");
        assert!(error.to_string().contains("symlink"));
        assert_eq!(std::fs::read(&target).expect("victim"), b"{\"keep\":true}");
    }

    #[test]
    fn an_invalid_later_config_refuses_before_any_file_changes() {
        let (_directory, paths, binaries) = fixture();
        std::fs::create_dir_all(paths.claude_settings.parent().expect("parent"))
            .expect("config directory");
        let settings = br#"{"theme":"dark"}"#;
        std::fs::write(&paths.claude_settings, settings).expect("settings");
        std::fs::write(&paths.claude_mcp, b"{ broken").expect("invalid MCP config");
        let error = install_integration_at(&paths, "claude", &binaries, None)
            .expect_err("invalid config refused");
        assert!(error
            .to_string()
            .contains(&paths.claude_mcp.display().to_string()));
        assert_eq!(
            std::fs::read(&paths.claude_settings).expect("settings after"),
            settings
        );
        assert_eq!(
            std::fs::read(&paths.claude_mcp).expect("MCP config after"),
            b"{ broken"
        );
        assert!(!binaries
            .hook_binary
            .parent()
            .expect("bin")
            .join("paneflow-ai-hook.sh")
            .exists());
        assert!(!paths.marker("claude-code").exists());
    }

    fn user_stop_hook() -> Value {
        json!({"theme": "dark", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "my-hook"}]}]}})
    }

    fn write_settings(paths: &ConfigPaths, root: &Value) {
        std::fs::create_dir_all(paths.claude_settings.parent().expect("parent"))
            .expect("config directory");
        std::fs::write(
            &paths.claude_settings,
            merge::json_to_bytes(root).expect("bytes"),
        )
        .expect("settings");
    }

    fn read_settings(paths: &ConfigPaths) -> Value {
        serde_json::from_slice(&std::fs::read(&paths.claude_settings).expect("settings"))
            .expect("settings JSON")
    }

    fn settings_from_the_old_hooks_command(binaries: &IntegrationBinaries) -> Value {
        let mut root = user_stop_hook();
        let reconciled = paneflow_agent_config::claude_hooks::reconcile_hooks(&mut root, |event| {
            paneflow_agent_config::claude_hooks::render_hook_command(&binaries.hook_binary, event)
        })
        .expect("old hooks reconcile");
        assert!(reconciled.changed);
        assert_eq!(
            root["hooks"].as_object().expect("hooks").len(),
            paneflow_agent_config::claude_hooks::CLAUDE_HOOK_EVENTS.len()
        );
        root
    }

    fn settings_from_the_integrations_command(binaries: &IntegrationBinaries) -> Value {
        let (_directory, paths, _) = fixture();
        write_settings(&paths, &user_stop_hook());
        install_integration_at(&paths, "claude-code", binaries, None).expect("install");
        let root = read_settings(&paths);
        assert_eq!(
            root["hooks"]
                .as_object()
                .expect("hooks")
                .values()
                .filter(|entries| entries
                    .as_array()
                    .is_some_and(|entries| entries.iter().any(is_owned_group)))
                .count(),
            CLAUDE_EVENTS.len()
        );
        root
    }

    #[test]
    fn hooks_from_either_command_converge_to_one_managed_set_and_keep_user_hooks() {
        let (_directory, paths, binaries) = fixture();
        let mut converged = Vec::new();
        for starting in [
            settings_from_the_old_hooks_command(&binaries),
            settings_from_the_integrations_command(&binaries),
        ] {
            write_settings(&paths, &starting);
            std::fs::remove_file(paths.marker("claude-code")).ok();
            install_integration_at(&paths, "claude-code", &binaries, None).expect("install");
            let root = read_settings(&paths);
            assert_eq!(root["theme"], "dark");
            assert!(root["hooks"]["Stop"]
                .as_array()
                .is_some_and(|entries| entries
                    .iter()
                    .any(|entry| entry["hooks"][0]["command"] == "my-hook")));
            assert!(root["hooks"].get("PostToolUse").is_none());
            converged.push(root);
        }
        assert_eq!(converged[0], converged[1]);
    }

    #[test]
    fn user_handlers_sharing_a_group_with_paneflow_survive_install_and_removal() {
        let (_directory, paths, binaries) = fixture();
        let mut marked = json!({
            MANAGED_MARKER: true,
            "matcher": "Write",
            "hooks": [
                {"type": "command", "command": "/old/bin/paneflow-ai-hook.sh", "args": ["Stop"]},
                {"type": "command", "command": "my-hook"},
            ],
        });
        let unmarked = json!({
            "hooks": [
                {"type": "command", "command": "/old/bin/paneflow-ai-hook Notification"},
                {"type": "command", "command": "their-hook"},
            ],
        });
        write_settings(
            &paths,
            &json!({"hooks": {"Stop": [marked.clone()], "Notification": [unmarked]}}),
        );

        install_integration_at(&paths, "claude-code", &binaries, None).expect("install");
        let installed = read_settings(&paths);
        install_integration_at(&paths, "claude-code", &binaries, None).expect("reinstall");
        assert_eq!(read_settings(&paths), installed);
        let stop = installed["hooks"]["Stop"].as_array().expect("Stop");
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["matcher"], "Write");
        assert_eq!(
            stop[0]["hooks"],
            json!([{"type": "command", "command": "my-hook"}])
        );
        assert!(stop[0].get(MANAGED_MARKER).is_none());
        assert_eq!(stop[1][MANAGED_MARKER], true);

        remove_integration_at(&paths, "claude-code").expect("remove");
        marked
            .as_object_mut()
            .expect("group")
            .remove(MANAGED_MARKER);
        marked["hooks"].as_array_mut().expect("hooks").remove(0);
        assert_eq!(
            read_settings(&paths),
            json!({"hooks": {
                "Stop": [marked],
                "Notification": [{"hooks": [{"type": "command", "command": "their-hook"}]}],
            }})
        );
    }

    #[test]
    fn removal_from_either_starting_state_leaves_only_user_hooks() {
        let (_directory, paths, binaries) = fixture();
        for starting in [
            settings_from_the_old_hooks_command(&binaries),
            settings_from_the_integrations_command(&binaries),
        ] {
            write_settings(&paths, &starting);
            remove_integration_at(&paths, "claude-code").expect("remove");
            assert_eq!(read_settings(&paths), user_stop_hook());
        }
    }
}
