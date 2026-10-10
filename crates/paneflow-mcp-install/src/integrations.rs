use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use paneflow_agent_config::claude_hooks::{
    cmd_command_word, sh_command_word, CLAUDE_HOOK_EVENTS as CLAUDE_EVENTS,
    CLAUDE_RETIRED_HOOK_EVENTS as CLAUDE_RETIRED_EVENTS, MANAGED_MARKER,
};
use paneflow_agent_config::{
    runtime_by_command_alias, runtime_by_slug, Runtime, RuntimeHookAdapter, RuntimeMcpConfig,
    RuntimeSkillsDir, RUNTIMES,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::agents::{self, AgentConfigWriter, InstallOutcome, StatusOutcome};
use crate::{io, merge};

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
const CODEX_RETIRED_EVENTS: &[&str] = &["PostToolUse"];
const UNMARKED_HOOKS_ADOPTION: &str = "Paneflow hook entries without an integration marker";
const LAUNCH_TIME_HOOKS_ADOPTION: &str = "pre-0.17 launch-time hook install";
const LAUNCH_TIME_HOOKS_EVIDENCE: &str = "agent-config-leases";
pub(crate) const FORCE_FLAG: &str = "--force";
const CONDUCTOR_SKILL: &str = "paneflow-conductor";
const CONDUCTOR_SKILL_SOURCE: &str = include_str!("../../../skills/paneflow-conductor/SKILL.md");
const HOOK_HELPER: &str = "paneflow-ai-hook";
const MCP_HELPER: &str = "paneflow-mcp";
const EXE_SUFFIX: &str = if cfg!(windows) { ".exe" } else { "" };

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstallMode {
    pub force: bool,
    pub debug_build: bool,
}

impl InstallMode {
    #[must_use]
    pub fn for_this_build(force: bool) -> Self {
        Self {
            force,
            debug_build: cfg!(debug_assertions),
        }
    }
}

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
    pub notes: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct IntegrationBinaries {
    pub hook_binary: PathBuf,
    pub bridge_binary: PathBuf,
}

#[derive(Clone, Debug)]
struct ConfigPaths {
    marker_home: PathBuf,
    claude_skills: PathBuf,
    claude_settings: PathBuf,
    claude_mcp: PathBuf,
    codex_hooks: PathBuf,
    codex_config: PathBuf,
}

impl ConfigPaths {
    fn current() -> Result<Self> {
        Self::resolve(
            paneflow_home::paneflow_home()
                .ok_or_else(|| anyhow!("cannot resolve PANEFLOW_HOME"))?,
        )
    }

    fn resolve(marker_home: PathBuf) -> Result<Self> {
        let claude = paneflow_agent_config::ClaudePaths::current()
            .ok_or_else(|| anyhow!("cannot resolve the Claude config directory"))?;
        let codex_home = paneflow_agent_config::codex_home()
            .ok_or_else(|| anyhow!("cannot resolve the Codex home"))?;
        Ok(Self {
            marker_home,
            claude_skills: claude.config_dir.join("skills"),
            claude_settings: claude.settings(),
            claude_mcp: claude.global_config,
            codex_hooks: codex_home.join("hooks.json"),
            codex_config: codex_home.join("config.toml"),
        })
    }

    fn mcp_writer(&self, runtime: &Runtime) -> Result<Box<dyn AgentConfigWriter>> {
        let config = runtime
            .integration
            .mcp_config
            .ok_or_else(|| anyhow!("{} declares no MCP config", runtime.label))?;
        Ok(match config {
            RuntimeMcpConfig::Claude => {
                Box::new(agents::claude_code::ClaudeCode::at(self.claude_mcp.clone()))
            }
            RuntimeMcpConfig::Codex => {
                Box::new(agents::codex::Codex::at(self.codex_config.clone()))
            }
            other => agents::writer_for(other),
        })
    }

    fn conductor_skill(&self, runtime: &Runtime) -> Option<PathBuf> {
        let skills = match runtime.integration.skills_dir? {
            RuntimeSkillsDir::Claude => &self.claude_skills,
        };
        Some(skills.join(CONDUCTOR_SKILL).join("SKILL.md"))
    }

    fn skill_record(&self, slug: &str) -> PathBuf {
        self.marker_home
            .join("integrations")
            .join(format!("{slug}.{CONDUCTOR_SKILL}.json"))
    }

    fn marker(&self, slug: &str) -> PathBuf {
        self.marker_home
            .join("integrations")
            .join(format!("{slug}.json"))
    }

    fn launch_time_hooks_evidence(&self) -> PathBuf {
        self.marker_home.join(LAUNCH_TIME_HOOKS_EVIDENCE)
    }

    fn launch_time_adoption_record(&self) -> PathBuf {
        self.marker_home
            .join("integrations")
            .join("launch-time-adoption.json")
    }
}

pub fn list_integrations() -> Vec<IntegrationStatus> {
    match ConfigPaths::current() {
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
        notes: Vec::new(),
    }
}

fn installed_evidence(paths: &ConfigPaths, runtime: &Runtime) -> bool {
    if !owned_hooks_present(paths, runtime) {
        return false;
    }
    paths
        .mcp_writer(runtime)
        .and_then(|writer| writer.status(None))
        .is_ok_and(|status| status != StatusOutcome::NotInstalled)
}

fn has_installer(runtime: &Runtime) -> bool {
    matches!(
        runtime.integration.hook_adapter,
        RuntimeHookAdapter::Claude | RuntimeHookAdapter::Codex
    )
}

pub fn install_integration(
    slug: &str,
    binaries: &IntegrationBinaries,
    force: bool,
) -> Result<IntegrationStatus, String> {
    let paths = ConfigPaths::current().map_err(format_error)?;
    install_integration_at(
        &paths,
        slug,
        binaries,
        None,
        InstallMode::for_this_build(force),
    )
    .map_err(format_error)
}

pub fn install_mcp_entry(
    writer: &dyn AgentConfigWriter,
    bridge: &Path,
    mode: InstallMode,
) -> Result<InstallOutcome> {
    guard_mcp_entry(writer, bridge, mode)?;
    writer.install(bridge)
}

fn guard_mcp_entry(writer: &dyn AgentConfigWriter, bridge: &Path, mode: InstallMode) -> Result<()> {
    let found = match writer.status(Some(bridge))? {
        StatusOutcome::StalePath { found, .. } => found,
        StatusOutcome::NeedsRepair {
            path: Some(found), ..
        } => found,
        _ => return Ok(()),
    };
    refuse_foreign_owner(
        &format!("the {} `paneflow` MCP entry", writer.label()),
        Path::new(&found),
        bridge,
        MCP_HELPER,
        mode,
    )
}

fn refuse_foreign_owner(
    entry: &str,
    found: &Path,
    ours: &Path,
    helper: &str,
    mode: InstallMode,
) -> Result<()> {
    if mode.force || found == ours {
        return Ok(());
    }
    let our_home = owner_home(ours).unwrap_or(ours);
    let owner = owner_home(found);
    if owner.is_some_and(|owner| same_path(owner, our_home)) {
        return Ok(());
    }
    if !mode.debug_build && !owner.is_some_and(|owner| is_live_home(owner, helper)) {
        return Ok(());
    }
    bail!(
        "{entry} belongs to the Paneflow home {}, not to this one ({}); re-run with --force to point it at {}",
        owner.unwrap_or(found).display(),
        our_home.display(),
        our_home.display()
    )
}

fn owner_home(command: &Path) -> Option<&Path> {
    let bin = command.parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    bin.parent()
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

fn is_live_home(home: &Path, helper: &str) -> bool {
    home.join("bin")
        .join(format!("{helper}{EXE_SUFFIX}"))
        .is_file()
        && paneflow_home::legacy_data_dir().is_none_or(|legacy| !same_path(&legacy, home))
}

fn hook_reporters(root: &Value) -> Vec<PathBuf> {
    let mut reporters: Vec<PathBuf> = root
        .get("hooks")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|hooks| hooks.values())
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .flat_map(|hook| {
            ["command", "command_windows", "commandWindows"]
                .into_iter()
                .filter_map(|key| hook.get(key).and_then(Value::as_str))
        })
        .filter_map(reporter_path)
        .collect();
    reporters.sort();
    reporters.dedup();
    reporters
}

fn reporter_path(command: &str) -> Option<PathBuf> {
    let at = command.find(HOOK_HELPER)?;
    let prefix = &command[..at];
    let directory = match prefix.rfind(['\'', '"']) {
        Some(quote) => &prefix[quote + 1..],
        None => prefix.trim_start(),
    };
    Some(PathBuf::from(format!("{directory}{HOOK_HELPER}")))
}

fn install_integration_at(
    paths: &ConfigPaths,
    slug: &str,
    binaries: &IntegrationBinaries,
    adopted_from: Option<&str>,
    mode: InstallMode,
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
    let writer = paths.mcp_writer(runtime)?;
    preflight_install(paths, runtime, binaries, writer.as_ref(), mode)?;
    let unix_reporter = write_unix_reporter(&binaries.hook_binary)?;
    match runtime.integration.hook_adapter {
        RuntimeHookAdapter::Claude => {
            install_claude_hooks(paths, &unix_reporter, binaries)?;
        }
        RuntimeHookAdapter::Codex => {
            install_codex_hooks(paths, &unix_reporter, binaries)?;
        }
        _ => bail!("{} offers detection only", runtime.label),
    }
    install_mcp_entry(writer.as_ref(), &binaries.bridge_binary, mode)?;
    if runtime.integration.mcp_config == Some(RuntimeMcpConfig::Claude) {
        remove_legacy_claude_mcp_entries(&paths.claude_mcp)?;
    }
    write_marker(paths, runtime.slug, adopted_from)?;
    let mut status = status_for(runtime, Some(paths));
    status.notes.extend(install_conductor_skill(paths, runtime));
    Ok(status)
}

fn versioned_conductor_skill() -> String {
    let body = CONDUCTOR_SKILL_SOURCE.replace("\r\n", "\n");
    let version = format!(
        "metadata:\n  paneflow-version: \"{}\"\n---\n",
        env!("CARGO_PKG_VERSION")
    );
    match body
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
    {
        Some((frontmatter, text)) => format!("---\n{frontmatter}\n{version}{text}"),
        None => format!("---\n{version}{body}"),
    }
}

fn fingerprint(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn recorded_skill_fingerprint(paths: &ConfigPaths, slug: &str) -> Option<String> {
    let record = std::fs::read(paths.skill_record(slug)).ok()?;
    serde_json::from_slice::<Value>(&record)
        .ok()?
        .get("sha256")?
        .as_str()
        .map(str::to_owned)
}

fn install_conductor_skill(paths: &ConfigPaths, runtime: &Runtime) -> Option<String> {
    let target = paths.conductor_skill(runtime)?;
    let skill = versioned_conductor_skill();
    let result = match std::fs::read(&target) {
        Ok(existing) if existing == skill.as_bytes() => Ok(None),
        Ok(existing)
            if recorded_skill_fingerprint(paths, runtime.slug)
                .is_some_and(|recorded| recorded == fingerprint(&existing)) =>
        {
            io::write_atomic(&target, skill.as_bytes()).map(|()| None)
        }
        Ok(_) => Ok(Some(format!(
            "{}: kept your modified {CONDUCTOR_SKILL} skill; delete it and install again for the {} copy",
            target.display(),
            env!("CARGO_PKG_VERSION")
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            io::write_atomic(&target, skill.as_bytes()).map(|()| None)
        }
        Err(error) => Err(anyhow::Error::from(error).context(format!("read {}", target.display()))),
    };
    let result = result.and_then(|note| {
        if note.is_none() {
            write_skill_record(paths, runtime.slug, &target, skill.as_bytes())?;
        }
        Ok(note)
    });
    result.unwrap_or_else(|error| {
        Some(format!(
            "{}: the {CONDUCTOR_SKILL} skill was not installed: {error:#}",
            target.display()
        ))
    })
}

fn write_skill_record(paths: &ConfigPaths, slug: &str, target: &Path, skill: &[u8]) -> Result<()> {
    let record = paths.skill_record(slug);
    refuse_symlink(&record)?;
    let value = json!({
        "schema": 1,
        "path": target.display().to_string(),
        "version": env!("CARGO_PKG_VERSION"),
        "sha256": fingerprint(skill),
    });
    io::write_if_changed(&record, &merge::json_to_bytes(&value)?)?;
    Ok(())
}

fn remove_conductor_skill(paths: &ConfigPaths, runtime: &Runtime) -> Result<Option<String>> {
    let Some(target) = paths.conductor_skill(runtime) else {
        return Ok(None);
    };
    let record = paths.skill_record(runtime.slug);
    let note = match std::fs::read(&target) {
        Ok(existing)
            if existing == versioned_conductor_skill().as_bytes()
                || recorded_skill_fingerprint(paths, runtime.slug)
                    .is_some_and(|recorded| recorded == fingerprint(&existing)) =>
        {
            std::fs::remove_file(&target)
                .with_context(|| format!("remove {}", target.display()))?;
            if let Some(directory) = target.parent() {
                let _ = std::fs::remove_dir(directory);
            }
            None
        }
        Ok(_) => Some(format!(
            "{}: kept your modified {CONDUCTOR_SKILL} skill",
            target.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", target.display()));
        }
    };
    if let Err(error) = std::fs::remove_file(&record) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error).with_context(|| format!("remove {}", record.display()));
        }
    }
    Ok(note)
}

fn preflight_install(
    paths: &ConfigPaths,
    runtime: &Runtime,
    binaries: &IntegrationBinaries,
    writer: &dyn AgentConfigWriter,
    mode: InstallMode,
) -> Result<()> {
    let reporter = binaries
        .hook_binary
        .parent()
        .ok_or_else(|| anyhow!("hook reporter path has no parent"))?
        .join("paneflow-ai-hook.sh");
    refuse_symlink(&reporter)?;
    refuse_symlink(&paths.marker(runtime.slug))?;
    let hooks_file = match runtime.integration.hook_adapter {
        RuntimeHookAdapter::Claude => &paths.claude_settings,
        RuntimeHookAdapter::Codex => &paths.codex_hooks,
        _ => bail!("{} offers detection only", runtime.label),
    };
    let hooks = merge::read_json_or_default(hooks_file)?;
    if !hooks.is_object() {
        bail!("{}: config root is not a JSON object", hooks_file.display());
    }
    if runtime.integration.mcp_config == Some(RuntimeMcpConfig::Claude)
        && !merge::read_json_or_default(&paths.claude_mcp)?.is_object()
    {
        bail!(
            "{}: config root is not a JSON object",
            paths.claude_mcp.display()
        );
    }
    for reporter in hook_reporters(&hooks) {
        refuse_foreign_owner(
            &format!("the Paneflow hook set in {}", hooks_file.display()),
            &reporter,
            &binaries.hook_binary,
            HOOK_HELPER,
            mode,
        )?;
    }
    guard_mcp_entry(writer, &binaries.bridge_binary, mode)
}

pub fn remove_integration(slug: &str) -> Result<IntegrationStatus, String> {
    let paths = ConfigPaths::current().map_err(format_error)?;
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
        Some("install") if args.len() == 2 || (args.len() == 3 && args[2] == FORCE_FLAG) => {
            let Some(binaries) = binaries else {
                eprintln!("paneflow integrations: embedded integration binaries are unavailable");
                return 1;
            };
            match install_integration(&args[1], &binaries, args.len() == 3) {
                Ok(status) => {
                    println!("{}: installed", status.label);
                    if let Some(step) = status.post_install_step {
                        println!("{step}");
                    }
                    for note in &status.notes {
                        eprintln!("{note}");
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
                for note in &status.notes {
                    eprintln!("{note}");
                }
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
        "Usage: paneflow integrations <COMMAND>\n\nCommands:\n  list\n  install <runtime> [--force]   --force takes over entries another Paneflow home owns\n  remove <runtime>"
    );
}

fn remove_integration_at(paths: &ConfigPaths, slug: &str) -> Result<IntegrationStatus> {
    let runtime = integration_runtime(slug)?;
    if !has_installer(runtime) {
        bail!("{} offers detection only", runtime.label);
    }
    match runtime.integration.hook_adapter {
        RuntimeHookAdapter::Claude => remove_json_hooks(
            &paths.claude_settings,
            &owned_events(CLAUDE_EVENTS, CLAUDE_RETIRED_EVENTS),
        )?,
        RuntimeHookAdapter::Codex => remove_json_hooks(
            &paths.codex_hooks,
            &owned_events(CODEX_EVENTS, CODEX_RETIRED_EVENTS),
        )?,
        _ => bail!("{} offers detection only", runtime.label),
    }
    paths.mcp_writer(runtime)?.uninstall()?;
    let skill_note = remove_conductor_skill(paths, runtime)?;
    let marker = paths.marker(runtime.slug);
    if let Err(error) = std::fs::remove_file(&marker) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error).with_context(|| format!("remove {}", marker.display()));
        }
    }
    let mut status = status_for(runtime, Some(paths));
    status.notes.extend(skill_note);
    Ok(status)
}

fn integration_runtime(slug_or_command: &str) -> Result<&'static Runtime> {
    runtime_by_slug(slug_or_command)
        .or_else(|| runtime_by_command_alias(slug_or_command))
        .ok_or_else(|| anyhow!("unknown runtime '{slug_or_command}'"))
}

pub fn adopt_and_refresh_installed(
    home: &Path,
    binaries: &IntegrationBinaries,
) -> Vec<(String, Result<(), String>)> {
    let Ok(paths) = ConfigPaths::resolve(home.to_path_buf()) else {
        return Vec::new();
    };
    adopt_and_refresh_installed_at(&paths, binaries, InstallMode::for_this_build(false))
}

fn adopt_and_refresh_installed_at(
    paths: &ConfigPaths,
    binaries: &IntegrationBinaries,
    mode: InstallMode,
) -> Vec<(String, Result<(), String>)> {
    let launch_time_install = paths.launch_time_hooks_evidence().is_dir()
        && !paths.launch_time_adoption_record().exists();
    let mut results = Vec::new();
    for runtime in RUNTIMES.iter().filter(|runtime| has_installer(runtime)) {
        let marker = paths.marker(runtime.slug);
        let adoption = if marker.exists() {
            None
        } else if owned_hooks_present(paths, runtime) {
            Some(UNMARKED_HOOKS_ADOPTION)
        } else if launch_time_install
            && runtime.supports_current_platform()
            && runtime_detected_in(paths, runtime)
        {
            Some(LAUNCH_TIME_HOOKS_ADOPTION)
        } else {
            None
        };
        if marker.exists() || adoption.is_some() {
            let result = install_integration_at(paths, runtime.slug, binaries, adoption, mode)
                .map(|_| ())
                .map_err(|error| format!("{error:#}"));
            results.push((runtime.slug.to_string(), result));
        }
    }
    if launch_time_install && results.iter().all(|(_, result)| result.is_ok()) {
        if let Err(error) = write_launch_time_adoption_record(paths) {
            results.push((
                LAUNCH_TIME_HOOKS_EVIDENCE.to_string(),
                Err(format!("{error:#}")),
            ));
        }
    }
    results
}

fn write_launch_time_adoption_record(paths: &ConfigPaths) -> Result<()> {
    let record = paths.launch_time_adoption_record();
    refuse_symlink(&record)?;
    let value = json!({
        "schema": 1,
        "adopted_at_ms": epoch_millis(),
    });
    io::write_if_changed(&record, &merge::json_to_bytes(&value)?)?;
    Ok(())
}

pub(crate) fn has_owned_hooks(slug: &str) -> bool {
    let Ok(paths) = ConfigPaths::current() else {
        return false;
    };
    integration_runtime(slug).is_ok_and(|runtime| owned_hooks_present(&paths, runtime))
}

pub(crate) fn runtime_detected(slug: &str) -> bool {
    let (Ok(runtime), Ok(paths)) = (integration_runtime(slug), ConfigPaths::current()) else {
        return false;
    };
    runtime_detected_in(&paths, runtime)
}

fn runtime_detected_in(paths: &ConfigPaths, runtime: &Runtime) -> bool {
    if runtime
        .detection
        .command_aliases
        .iter()
        .any(|alias| which::which(alias).is_ok())
    {
        return true;
    }
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

fn install_claude_hooks(
    paths: &ConfigPaths,
    unix_reporter: &Path,
    binaries: &IntegrationBinaries,
) -> Result<()> {
    io::with_config_lock(&paths.claude_settings, || {
        let mut root = merge::read_json_or_default(&paths.claude_settings)?;
        reconcile_claude_hooks(
            &mut root,
            reporter_for_current_platform(unix_reporter, &binaries.hook_binary),
        )?;
        io::write_if_changed_unlocked(&paths.claude_settings, &merge::json_to_bytes(&root)?)?;
        Ok(())
    })
}

fn remove_legacy_claude_mcp_entries(path: &Path) -> Result<()> {
    io::with_config_lock(path, || {
        let mut root = merge::read_json_or_default(path)?;
        let removed = ["paneflow-dev", "paneflow-mcp"]
            .into_iter()
            .filter(|legacy| merge::remove_json_entry(&mut root, "mcpServers", legacy))
            .count();
        if removed > 0 {
            io::write_if_changed_unlocked(path, &merge::json_to_bytes(&root)?)?;
        }
        Ok(())
    })
}

fn install_codex_hooks(
    paths: &ConfigPaths,
    unix_reporter: &Path,
    binaries: &IntegrationBinaries,
) -> Result<()> {
    io::with_config_lock(&paths.codex_hooks, || {
        let mut root = merge::read_json_or_default(&paths.codex_hooks)?;
        reconcile_codex_hooks(&mut root, unix_reporter, &binaries.hook_binary)?;
        io::write_if_changed_unlocked(&paths.codex_hooks, &merge::json_to_bytes(&root)?)?;
        Ok(())
    })
}

fn owned_events(current: &[&'static str], retired: &[&'static str]) -> Vec<&'static str> {
    current
        .iter()
        .copied()
        .chain(retired.iter().copied())
        .collect()
}

fn remove_json_hooks(path: &Path, events: &[&str]) -> Result<()> {
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
                "command": format!(
                    "{} {event}",
                    sh_command_word(&unix_reporter.display().to_string())
                ),
                "command_windows": format!(
                    "{} {event}",
                    cmd_command_word(&windows_reporter.display().to_string())
                ),
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

pub fn remove_legacy_project_hooks(
    project_dirs: &[PathBuf],
) -> Vec<(PathBuf, std::result::Result<(), String>)> {
    let mut results = Vec::new();
    for dir in project_dirs {
        let path = dir.join(".claude").join("settings.local.json");
        if !path.is_file() {
            continue;
        }
        match remove_legacy_project_hooks_at(&path) {
            Ok(false) => {}
            Ok(true) => results.push((path, Ok(()))),
            Err(error) => results.push((path, Err(format_error(error)))),
        }
    }
    results
}

fn remove_legacy_project_hooks_at(path: &Path) -> Result<bool> {
    io::with_config_lock(path, || {
        let mut root = merge::read_json_or_default(path)?;
        if !paneflow_agent_config::claude_hooks::remove_hooks_lenient(&mut root) {
            return Ok(false);
        }
        io::write_if_changed_unlocked(path, &merge::json_to_bytes(&root)?)?;
        Ok(true)
    })
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
            claude_skills: root.join("claude").join("skills"),
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
    fn only_claude_code_and_codex_have_an_installer() {
        let installers: Vec<_> = RUNTIMES
            .iter()
            .filter(|runtime| has_installer(runtime))
            .map(|runtime| runtime.slug)
            .collect();
        assert_eq!(installers, ["claude-code", "codex"]);
        let (_directory, paths, binaries) = fixture();
        for slug in ["gemini", "muse-code", "opencode", "hermes", "grok"] {
            let runtime = runtime_by_slug(slug).expect("runtime");
            let summary = runtime.integration.summary.to_ascii_lowercase();
            assert!(
                !summary.contains("hook") && !summary.contains("install"),
                "{slug}: {summary}"
            );
            assert!(
                install_integration_at(&paths, slug, &binaries, None, InstallMode::default())
                    .is_err(),
                "{slug}"
            );
        }
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
        install_integration_at(
            &paths,
            "claude-code",
            &binaries,
            None,
            InstallMode::default(),
        )
        .expect("install");
        let first = std::fs::read(&paths.claude_settings).expect("first config");
        install_integration_at(
            &paths,
            "claude-code",
            &binaries,
            None,
            InstallMode::default(),
        )
        .expect("reinstall");
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

    fn with_launch_time_install_evidence(paths: &ConfigPaths) {
        std::fs::create_dir_all(paths.launch_time_hooks_evidence()).expect("lease directory");
        std::fs::create_dir_all(paths.claude_settings.parent().expect("parent"))
            .expect("claude config directory");
    }

    #[test]
    fn an_upgrade_from_launch_time_hooks_adopts_the_detected_runtime_without_user_action() {
        let (_directory, paths, binaries) = fixture();
        with_launch_time_install_evidence(&paths);

        let results = adopt_and_refresh_installed_at(&paths, &binaries, InstallMode::default());

        assert!(
            results
                .iter()
                .any(|(slug, result)| slug == "claude-code" && result.is_ok()),
            "{results:?}"
        );
        let marker: Value =
            serde_json::from_slice(&std::fs::read(paths.marker("claude-code")).expect("marker"))
                .expect("marker JSON");
        assert_eq!(marker["adopted_from"], LAUNCH_TIME_HOOKS_ADOPTION);
        let settings = std::fs::read_to_string(&paths.claude_settings).expect("settings");
        assert!(settings.contains("paneflow-ai-hook"), "{settings}");
        assert!(paths.launch_time_adoption_record().is_file());
    }

    #[test]
    fn a_removed_integration_stays_removed_after_the_launch_time_adoption() {
        let (_directory, paths, binaries) = fixture();
        with_launch_time_install_evidence(&paths);
        adopt_and_refresh_installed_at(&paths, &binaries, InstallMode::default());
        remove_integration_at(&paths, "claude-code").expect("remove");

        adopt_and_refresh_installed_at(&paths, &binaries, InstallMode::default());

        assert!(!paths.marker("claude-code").exists());
    }

    #[test]
    fn a_machine_that_never_ran_launch_time_hooks_adopts_nothing() {
        let (_directory, paths, binaries) = fixture();
        std::fs::create_dir_all(paths.claude_settings.parent().expect("parent"))
            .expect("claude config directory");

        let results = adopt_and_refresh_installed_at(&paths, &binaries, InstallMode::default());

        assert!(results.is_empty(), "{results:?}");
        assert!(!paths.marker("claude-code").exists());
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
            InstallMode::default(),
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
        install_integration_at(&paths, "codex", &binaries, None, InstallMode::default())
            .expect("install");
        let hooks = std::fs::read(&paths.codex_hooks).expect("hooks");
        let config = std::fs::read(&paths.codex_config).expect("config");
        install_integration_at(&paths, "codex", &binaries, None, InstallMode::default())
            .expect("reinstall");
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
    fn the_worker_refresh_updates_the_codex_entry_in_place_and_keeps_every_user_key() {
        let (_directory, paths, binaries) = fixture();
        install_integration_at(&paths, "codex", &binaries, None, InstallMode::default())
            .expect("install");
        std::fs::write(&paths.codex_config, "# user\n[mcp_servers.paneflow]\ncommand = \"/old/paneflow-mcp\"\nargs = [\"--stale\"]\nenv_vars = [\"MY_TOKEN\", \"PANEFLOW_HOME\"]\nenabled = false\nrequired = true\nstartup_timeout_sec = 20\nstartup_timeout_ms = 20000\ntool_timeout_sec = 90\ncwd = \"/work\"\nenabled_tools = [\"list_panes\"]\ndisabled_tools = [\"search_pane\"]\n\n[mcp_servers.paneflow.env]\nRUST_LOG = \"debug\"\n\n[mcp_servers.paneflow.tools.read_pane]\napproval_mode = \"approve\"\n").expect("user entry");

        let results = adopt_and_refresh_installed_at(&paths, &binaries, InstallMode::default());

        assert!(
            results
                .iter()
                .any(|(slug, result)| slug == "codex" && result.is_ok()),
            "{results:?}"
        );
        let text = std::fs::read_to_string(&paths.codex_config).expect("config");
        let doc = text.parse::<toml_edit::DocumentMut>().expect("TOML");
        let entry = &doc["mcp_servers"]["paneflow"];
        assert_eq!(
            entry["command"].as_str(),
            Some(binaries.bridge_binary.display().to_string().as_str())
        );
        assert_eq!(entry["args"].as_array().map(|args| args.len()), Some(0));
        let env_vars: Vec<&str> = entry["env_vars"]
            .as_array()
            .expect("env_vars")
            .iter()
            .filter_map(|name| name.as_str())
            .collect();
        assert_eq!(&env_vars[..2], ["MY_TOKEN", "PANEFLOW_HOME"]);
        for name in crate::agents::CODEX_BRIDGE_ENV_VARS {
            assert_eq!(
                env_vars.iter().filter(|seen| *seen == name).count(),
                1,
                "{name}"
            );
        }
        assert_eq!(entry["enabled"].as_bool(), Some(false));
        assert_eq!(entry["required"].as_bool(), Some(true));
        assert_eq!(entry["startup_timeout_sec"].as_integer(), Some(20));
        assert_eq!(entry["startup_timeout_ms"].as_integer(), Some(20000));
        assert_eq!(entry["tool_timeout_sec"].as_integer(), Some(90));
        assert_eq!(entry["cwd"].as_str(), Some("/work"));
        assert_eq!(entry["enabled_tools"][0].as_str(), Some("list_panes"));
        assert_eq!(entry["disabled_tools"][0].as_str(), Some("search_pane"));
        assert_eq!(entry["env"]["RUST_LOG"].as_str(), Some("debug"));
        assert_eq!(
            entry["tools"]["read_pane"]["approval_mode"].as_str(),
            Some("approve")
        );
        assert!(text.starts_with("# user\n"));
    }

    #[test]
    fn codex_hook_commands_quote_only_a_reporter_path_that_needs_it() {
        let (_directory, paths, binaries) = fixture();
        install_integration_at(&paths, "codex", &binaries, None, InstallMode::default())
            .expect("install");
        let plain: Value =
            serde_json::from_slice(&std::fs::read(&paths.codex_hooks).expect("hooks")).unwrap();
        let hook = &plain["hooks"]["SessionStart"][0]["hooks"][0];
        assert_eq!(
            hook["command_windows"],
            format!("{} SessionStart", binaries.hook_binary.display()),
            "a path without special characters keeps the command Codex already approved"
        );
        assert!(!hook["command"].as_str().unwrap().starts_with('\''));

        let spaced = tempfile::tempdir().expect("temp directory");
        let bin = spaced.path().join("Jean Dupont").join("bin");
        std::fs::create_dir_all(&bin).expect("bin directory");
        let hook_binary = bin.join(if cfg!(windows) {
            "paneflow-ai-hook.exe"
        } else {
            "paneflow-ai-hook"
        });
        std::fs::write(&hook_binary, b"hook").expect("hook binary");
        let spaced_binaries = IntegrationBinaries {
            hook_binary: hook_binary.clone(),
            bridge_binary: binaries.bridge_binary.clone(),
        };
        install_integration_at(
            &paths,
            "codex",
            &spaced_binaries,
            None,
            InstallMode {
                force: true,
                debug_build: false,
            },
        )
        .expect("reinstall");
        let quoted: Value =
            serde_json::from_slice(&std::fs::read(&paths.codex_hooks).expect("hooks")).unwrap();
        let groups = quoted["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(
            groups.len(),
            1,
            "the old handler was replaced, not duplicated"
        );
        let hook = &groups[0]["hooks"][0];
        assert_eq!(
            hook["command_windows"],
            format!("\"{}\" SessionStart", hook_binary.display())
        );
        let command = hook["command"].as_str().unwrap();
        assert!(
            command.starts_with('\'') && command.ends_with("' SessionStart"),
            "{command}"
        );
        let program = paneflow_agent_config::claude_hooks::command_program_token(command)
            .expect("program token");
        assert!(
            program.contains("Jean Dupont") && program.ends_with("paneflow-ai-hook.sh"),
            "{program}"
        );
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
        install_integration_at(
            &paths,
            "codex",
            &binaries,
            Some(UNMARKED_HOOKS_ADOPTION),
            InstallMode::default(),
        )
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
        install_integration_at(
            &paths,
            "claude-code",
            &binaries,
            None,
            InstallMode::default(),
        )
        .expect("install");
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
        install_integration_at(
            &paths,
            "claude",
            &binaries,
            Some(UNMARKED_HOOKS_ADOPTION),
            InstallMode::default(),
        )
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
            install_integration_at(
                &first_paths,
                "codex",
                &first_binaries,
                None,
                InstallMode::default(),
            )
        });
        let second_paths = paths.clone();
        let second_binaries = binaries.clone();
        let second = std::thread::spawn(move || {
            install_integration_at(
                &second_paths,
                "codex",
                &second_binaries,
                None,
                InstallMode::default(),
            )
        });
        first.join().expect("first thread").expect("first install");
        second
            .join()
            .expect("second thread")
            .expect("second install");
        let hooks = std::fs::read(&paths.codex_hooks).expect("hooks");
        let config = std::fs::read(&paths.codex_config).expect("config");
        install_integration_at(&paths, "codex", &binaries, None, InstallMode::default())
            .expect("single install");
        assert_eq!(
            hooks,
            std::fs::read(&paths.codex_hooks).expect("hooks again")
        );
        assert_eq!(
            config,
            std::fs::read(&paths.codex_config).expect("config again")
        );
    }

    fn link_to(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).expect("symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, link).expect("symlink");
    }

    fn is_link(path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
    }

    #[test]
    fn symlinked_configs_are_written_through_and_stay_symlinks() {
        let (_directory, paths, binaries) = fixture();
        std::fs::create_dir_all(paths.claude_settings.parent().expect("parent"))
            .expect("config directory");
        let dotfiles = paths.marker_home.join("dotfiles");
        std::fs::create_dir_all(&dotfiles).expect("dotfiles");
        let settings_target = dotfiles.join("settings.json");
        let mcp_target = dotfiles.join("claude.json");
        std::fs::write(&settings_target, br#"{"keep":true}"#).expect("settings target");
        std::fs::write(&mcp_target, br#"{"keep":true}"#).expect("MCP target");
        link_to(&settings_target, &paths.claude_settings);
        link_to(&mcp_target, &paths.claude_mcp);

        install_integration_at(
            &paths,
            "claude-code",
            &binaries,
            None,
            InstallMode::default(),
        )
        .expect("install through the symlinks");

        assert!(is_link(&paths.claude_settings));
        assert!(is_link(&paths.claude_mcp));
        let settings: Value =
            serde_json::from_slice(&std::fs::read(&settings_target).expect("settings"))
                .expect("settings JSON");
        assert_eq!(settings["keep"], json!(true));
        assert!(settings["hooks"]["Stop"].is_array());
        let mcp: Value =
            serde_json::from_slice(&std::fs::read(&mcp_target).expect("MCP")).expect("MCP JSON");
        assert_eq!(mcp["keep"], json!(true));
        assert_eq!(
            mcp["mcpServers"]["paneflow"]["command"],
            json!(binaries.bridge_binary.display().to_string())
        );
    }

    fn other_home(paths: &ConfigPaths, name: &str) -> IntegrationBinaries {
        let bin = paths
            .marker_home
            .parent()
            .expect("root")
            .join(name)
            .join("bin");
        std::fs::create_dir_all(&bin).expect("other bin");
        let binaries = IntegrationBinaries {
            hook_binary: bin.join(format!("{HOOK_HELPER}{EXE_SUFFIX}")),
            bridge_binary: bin.join(format!("{MCP_HELPER}{EXE_SUFFIX}")),
        };
        std::fs::write(&binaries.hook_binary, b"hook").expect("other hook");
        std::fs::write(&binaries.bridge_binary, b"bridge").expect("other bridge");
        binaries
    }

    #[test]
    fn a_second_home_refuses_to_take_over_the_first_homes_entries_without_force() {
        let (_directory, paths, binaries) = fixture();
        let release_home = paths
            .marker_home
            .parent()
            .expect("root")
            .join("release-home");
        let release = ConfigPaths {
            marker_home: release_home.clone(),
            ..paths.clone()
        };
        let release_binaries = other_home(&paths, "release-home");
        for slug in ["claude-code", "codex"] {
            install_integration_at(
                &release,
                slug,
                &release_binaries,
                None,
                InstallMode::default(),
            )
            .expect("release install");
        }
        let settings = std::fs::read(&paths.claude_settings).expect("settings");
        let claude_mcp = std::fs::read(&paths.claude_mcp).expect("claude MCP");
        let codex_config = std::fs::read(&paths.codex_config).expect("codex config");

        for (slug, debug_build) in [
            ("claude-code", false),
            ("codex", false),
            ("claude-code", true),
        ] {
            let error = install_integration_at(
                &paths,
                slug,
                &binaries,
                None,
                InstallMode {
                    force: false,
                    debug_build,
                },
            )
            .expect_err("a foreign live home is not overwritten");
            let message = format!("{error:#}");
            assert!(
                message.contains(&release_home.display().to_string()),
                "{message}"
            );
            assert!(
                message.contains(&paths.marker_home.display().to_string()),
                "{message}"
            );
            assert!(message.contains("--force"), "{message}");
        }
        assert_eq!(
            std::fs::read(&paths.claude_settings).expect("settings"),
            settings
        );
        assert_eq!(
            std::fs::read(&paths.claude_mcp).expect("claude MCP"),
            claude_mcp
        );
        assert_eq!(
            std::fs::read(&paths.codex_config).expect("codex config"),
            codex_config
        );
        assert!(!paths.marker("claude-code").exists());

        install_integration_at(
            &paths,
            "claude-code",
            &binaries,
            None,
            InstallMode {
                force: true,
                debug_build: true,
            },
        )
        .expect("--force takes the entries over");
        let mcp: Value =
            serde_json::from_slice(&std::fs::read(&paths.claude_mcp).expect("claude MCP"))
                .expect("MCP JSON");
        assert_eq!(
            mcp["mcpServers"]["paneflow"]["command"],
            json!(binaries.bridge_binary.display().to_string())
        );
        install_integration_at(
            &release,
            "claude-code",
            &release_binaries,
            None,
            InstallMode::default(),
        )
        .expect_err("the release home now leaves the second home's entries alone");
    }

    #[test]
    fn an_invalid_settings_file_writes_nothing_and_names_the_file() {
        let (_directory, paths, binaries) = fixture();
        std::fs::create_dir_all(paths.claude_settings.parent().expect("parent"))
            .expect("config directory");
        std::fs::write(&paths.claude_settings, b"{ not json").expect("invalid settings");
        let error = install_integration_at(
            &paths,
            "claude-code",
            &binaries,
            None,
            InstallMode::default(),
        )
        .expect_err("invalid settings refused");
        assert!(
            format!("{error:#}").contains(&paths.claude_settings.display().to_string()),
            "{error:#}"
        );
        assert_eq!(
            std::fs::read(&paths.claude_settings).expect("settings after"),
            b"{ not json"
        );
        assert!(!paths.claude_mcp.exists());
        assert!(!paths.marker("claude-code").exists());
        assert!(!binaries
            .hook_binary
            .parent()
            .expect("bin")
            .join("paneflow-ai-hook.sh")
            .exists());
    }

    #[test]
    fn an_invalid_later_config_refuses_before_any_file_changes() {
        let (_directory, paths, binaries) = fixture();
        std::fs::create_dir_all(paths.claude_settings.parent().expect("parent"))
            .expect("config directory");
        let settings = br#"{"theme":"dark"}"#;
        std::fs::write(&paths.claude_settings, settings).expect("settings");
        std::fs::write(&paths.claude_mcp, b"{ broken").expect("invalid MCP config");
        let error =
            install_integration_at(&paths, "claude", &binaries, None, InstallMode::default())
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

    fn conductor_skill_path(paths: &ConfigPaths) -> PathBuf {
        paths.claude_skills.join(CONDUCTOR_SKILL).join("SKILL.md")
    }

    fn install_claude(paths: &ConfigPaths, binaries: &IntegrationBinaries) -> IntegrationStatus {
        install_integration_at(paths, "claude-code", binaries, None, InstallMode::default())
            .expect("install")
    }

    #[test]
    fn the_conductor_skill_is_installed_versioned_only_where_the_runtime_declares_a_skills_dir() {
        let (_directory, paths, binaries) = fixture();
        let status = install_claude(&paths, &binaries);
        assert!(status.notes.is_empty(), "{:?}", status.notes);
        let skill = std::fs::read_to_string(conductor_skill_path(&paths)).expect("skill");
        let frontmatter = skill
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .expect("frontmatter")
            .0;
        assert!(
            frontmatter.starts_with("name: paneflow-conductor\n"),
            "{frontmatter}"
        );
        assert!(
            frontmatter.contains(&format!(
                "  paneflow-version: \"{}\"",
                env!("CARGO_PKG_VERSION")
            )),
            "{frontmatter}"
        );
        assert!(skill.contains("# Paneflow conductor"));

        install_integration_at(&paths, "codex", &binaries, None, InstallMode::default())
            .expect("codex install");
        let codex = runtime_by_slug("codex").expect("codex");
        assert_eq!(codex.integration.skills_dir, None);
        assert_eq!(paths.conductor_skill(codex), None);
    }

    #[test]
    fn an_update_replaces_only_an_unmodified_skill_and_remove_keeps_a_modified_one() {
        let (_directory, paths, binaries) = fixture();
        install_claude(&paths, &binaries);
        let skill = conductor_skill_path(&paths);
        std::fs::write(
            &skill,
            b"---\nname: paneflow-conductor\n---\nolder release copy\n",
        )
        .expect("older copy");
        write_skill_record(
            &paths,
            "claude-code",
            &skill,
            b"---\nname: paneflow-conductor\n---\nolder release copy\n",
        )
        .expect("older record");
        let status = install_claude(&paths, &binaries);
        assert!(status.notes.is_empty(), "{:?}", status.notes);
        assert_eq!(
            std::fs::read_to_string(&skill).expect("updated"),
            versioned_conductor_skill()
        );

        std::fs::write(&skill, b"my own conductor notes\n").expect("user edit");
        let status = install_claude(&paths, &binaries);
        assert_eq!(status.notes.len(), 1, "{:?}", status.notes);
        assert!(
            status.notes[0].contains("kept your modified"),
            "{:?}",
            status.notes
        );
        assert_eq!(
            std::fs::read(&skill).expect("kept"),
            b"my own conductor notes\n"
        );

        let removed = remove_integration_at(&paths, "claude-code").expect("remove");
        assert_eq!(removed.notes.len(), 1, "{:?}", removed.notes);
        assert_eq!(
            std::fs::read(&skill).expect("still kept"),
            b"my own conductor notes\n"
        );

        std::fs::remove_file(&skill).expect("user deletes the copy");
        install_claude(&paths, &binaries);
        assert!(skill.is_file());
        let removed = remove_integration_at(&paths, "claude-code").expect("remove");
        assert!(removed.notes.is_empty(), "{:?}", removed.notes);
        assert!(!skill.exists());
        assert!(!paths.skill_record("claude-code").exists());
    }

    #[test]
    fn an_unwritable_skills_dir_fails_only_the_skill() {
        let (_directory, paths, binaries) = fixture();
        std::fs::create_dir_all(paths.claude_skills.parent().expect("claude dir"))
            .expect("claude dir");
        std::fs::write(
            &paths.claude_skills,
            b"a file where the skills directory belongs",
        )
        .expect("blocker");
        let status = install_claude(&paths, &binaries);
        assert_eq!(status.state, IntegrationState::Installed);
        assert_eq!(status.notes.len(), 1, "{:?}", status.notes);
        assert!(
            status.notes[0].contains("skill was not installed"),
            "{:?}",
            status.notes
        );
        let settings: Value =
            serde_json::from_slice(&std::fs::read(&paths.claude_settings).expect("settings"))
                .expect("settings JSON");
        assert!(settings["hooks"]["Stop"].is_array());
        let mcp: Value = serde_json::from_slice(&std::fs::read(&paths.claude_mcp).expect("MCP"))
            .expect("MCP JSON");
        assert!(mcp["mcpServers"]["paneflow"].is_object());
        assert!(!paths.skill_record("claude-code").exists());
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
        let hooks = root["hooks"].as_object_mut().expect("hooks");
        for event in CLAUDE_EVENTS.iter().chain(CLAUDE_RETIRED_EVENTS) {
            hooks
                .entry(event.to_string())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .expect("event")
                .push(json!({
                    MANAGED_MARKER: true,
                    "hooks": [{
                        "type": "command",
                        "command": paneflow_agent_config::claude_hooks::render_hook_command(
                            &binaries.hook_binary,
                            event,
                        ),
                        "timeout": 5,
                    }],
                }));
        }
        root
    }

    fn settings_from_the_integrations_command(binaries: &IntegrationBinaries) -> Value {
        let (_directory, paths, _) = fixture();
        write_settings(&paths, &user_stop_hook());
        install_integration_at(
            &paths,
            "claude-code",
            binaries,
            None,
            InstallMode::default(),
        )
        .expect("install");
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
            install_integration_at(
                &paths,
                "claude-code",
                &binaries,
                None,
                InstallMode::default(),
            )
            .expect("install");
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

        install_integration_at(
            &paths,
            "claude-code",
            &binaries,
            None,
            InstallMode::default(),
        )
        .expect("install");
        let installed = read_settings(&paths);
        install_integration_at(
            &paths,
            "claude-code",
            &binaries,
            None,
            InstallMode::default(),
        )
        .expect("reinstall");
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
