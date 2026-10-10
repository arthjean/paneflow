#![cfg_attr(test, allow(dead_code))]

#[cfg(not(all(test, feature = "screen-rules")))]
#[path = "src/screen_rules.rs"]
#[allow(
    dead_code,
    reason = "the build script only parses and validates rules, the evaluator serves the host"
)]
mod screen_rules;
#[cfg(all(test, feature = "screen-rules"))]
use crate::screen_rules;

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub schema_version: u32,
    pub id: String,
    pub slug: String,
    pub label: String,
    pub platforms: Vec<Platform>,
    pub display: Display,
    pub detection: Detection,
    pub environment: Environment,
    pub lifecycle: Lifecycle,
    pub integration: Integration,
    pub resume: Option<Resume>,
    pub sessions: Option<Sessions>,
    pub suggested_presets: Vec<SuggestedPreset>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    Macos,
    Windows,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display {
    pub order: u16,
    pub tint: Option<String>,
    pub icon_asset_path: String,
    pub icon_multicolor: bool,
    pub visibility_config_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detection {
    pub command_aliases: Vec<String>,
    pub process_aliases: Vec<String>,
    pub script_path_signatures: Vec<String>,
    #[serde(default)]
    pub title_prefix: Option<String>,
}

pub const CONTESTED_ALIASES: &[&str] = &["fx"];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub strip_inherited: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lifecycle {
    pub source: LifecycleSource,
    pub authority: LifecycleAuthority,
    pub fallback: LifecycleFallback,
    pub escape_cancels_turn: bool,
    pub attention_clears_on_output: bool,
    pub anchor_start_event_to_output: bool,
    #[serde(default)]
    pub bell_attention: Option<bool>,
}

impl Lifecycle {
    fn resolved_bell_attention(&self) -> bool {
        self.bell_attention
            .unwrap_or(self.authority != LifecycleAuthority::Complete)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleSource {
    Hooks,
    Output,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleAuthority {
    Complete,
    Screen,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleFallback {
    Screen,
    None,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Integration {
    pub summary: String,
    pub post_install_step: Option<String>,
    pub hook_adapter: HookAdapter,
    #[serde(default)]
    pub mcp_config: Option<McpConfig>,
    #[serde(default)]
    pub skills_dir: Option<SkillsDir>,
}

pub const ACCEPTED_MCP_CONFIGS: &str = "claude, codex, gemini, opencode, fx";
pub const ACCEPTED_SKILLS_DIRS: &str = "claude";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum McpConfig {
    Claude,
    Codex,
    Gemini,
    OpenCode,
    Fx,
}

impl McpConfig {
    fn variant(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini",
            Self::OpenCode => "OpenCode",
            Self::Fx => "Fx",
        }
    }
}

impl TryFrom<String> for McpConfig {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "gemini" => Ok(Self::Gemini),
            "opencode" => Ok(Self::OpenCode),
            "fx" => Ok(Self::Fx),
            other => Err(format!(
                "integration.mcp_config '{other}' has no MCP config writer; accepted writers: {ACCEPTED_MCP_CONFIGS}"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum SkillsDir {
    Claude,
}

impl SkillsDir {
    fn variant(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
        }
    }
}

impl TryFrom<String> for SkillsDir {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "claude" => Ok(Self::Claude),
            other => Err(format!(
                "integration.skills_dir '{other}' is not a verified skills directory; accepted directories: {ACCEPTED_SKILLS_DIRS}"
            )),
        }
    }
}

pub const ACCEPTED_HOOK_ADAPTERS: &str = "none, claude, codex";
pub const INSTALLER_HOOK_ADAPTERS: &str = "claude, codex";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum HookAdapter {
    Claude,
    Codex,
    None,
}

impl HookAdapter {
    fn has_installer(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::None => "none",
        }
    }
}

impl TryFrom<String> for HookAdapter {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "none" => Ok(Self::None),
            other => Err(format!(
                "integration.hook_adapter '{other}' has no installer; accepted adapters: {ACCEPTED_HOOK_ADAPTERS}"
            )),
        }
    }
}

pub const ACCEPTED_SESSION_READERS: &str = "claude, codex, opencode, pi, gemini, grok";
pub const VISIBILITY_CONFIG_KEY_SUFFIX: &str = "_button_visible";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sessions {
    pub reader: SessionReader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum SessionReader {
    Claude,
    Codex,
    OpenCode,
    Pi,
    Gemini,
    Grok,
}

impl SessionReader {
    fn variant(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
            Self::Gemini => "Gemini",
            Self::Grok => "Grok",
        }
    }
}

impl TryFrom<String> for SessionReader {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::OpenCode),
            "pi" => Ok(Self::Pi),
            "gemini" => Ok(Self::Gemini),
            "grok" => Ok(Self::Grok),
            other => Err(format!(
                "sessions.reader '{other}' is not a session reader; accepted readers: {ACCEPTED_SESSION_READERS}"
            )),
        }
    }
}

pub const SESSION_ID_PLACEHOLDER: &str = "{session_id}";
pub const MAX_SESSION_ID_PATTERN_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resume {
    pub session_argv: Vec<String>,
    pub continue_argv: Option<Vec<String>>,
    pub fork_argv: Option<Vec<String>>,
    pub session_id_pattern: String,
    pub failure_markers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestedPreset {
    pub id: String,
    pub command: String,
    pub name: Option<String>,
    pub platforms: Option<Vec<Platform>>,
    #[serde(default)]
    pub tmux_compat: bool,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct LocatedDescriptor {
    pub path: PathBuf,
    pub descriptor: Descriptor,
    pub screen: Option<ScreenRules>,
}

#[derive(Debug, Clone)]
pub struct ScreenRules {
    pub path: PathBuf,
    pub states: BTreeSet<&'static str>,
}

pub const SCREEN_RULES_FILE: &str = "screen.toml";

fn read_screen_rules(path: &Path) -> Result<Option<ScreenRules>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let rules = screen_rules::parse_base_rules(&text, screen_rules::RuleOrigin::Builtin)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(Some(ScreenRules {
        path: path.to_path_buf(),
        states: rules.iter().map(|rule| rule.state.as_str()).collect(),
    }))
}

pub fn discover_and_validate(root: &Path) -> Result<Vec<LocatedDescriptor>, String> {
    let entries = fs::read_dir(root)
        .map_err(|error| format!("{}: unable to discover runtimes: {error}", root.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", root.display()))?;
        let path = entry.path().join("runtime.toml");
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
            && path.is_file()
        {
            paths.push(path);
        }
    }
    paths.sort();
    let mut descriptors = Vec::with_capacity(paths.len());
    for path in paths {
        let text =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let descriptor = toml::from_str::<Descriptor>(&text)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let screen = read_screen_rules(&path.with_file_name(SCREEN_RULES_FILE))?;
        descriptors.push(LocatedDescriptor {
            path,
            descriptor,
            screen,
        });
    }
    descriptors.sort_by_key(|located| located.descriptor.display.order);
    validate(&descriptors)?;
    Ok(descriptors)
}

fn validate(descriptors: &[LocatedDescriptor]) -> Result<(), String> {
    let mut errors = Vec::new();
    let mut ids = BTreeMap::<&str, &Path>::new();
    let mut aliases = BTreeMap::<String, (&str, &Path)>::new();
    let mut display_orders = BTreeMap::<u16, &Path>::new();
    let mut visibility_keys = BTreeMap::<&str, &Path>::new();
    for located in descriptors {
        let path = located.path.as_path();
        let runtime = &located.descriptor;
        let directory_slug = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if runtime.schema_version != 1 {
            errors.push(format!("{}: schema_version must be 1", path.display()));
        }
        if runtime.slug != directory_slug {
            errors.push(format!(
                "{}: slug '{}' differs from directory name '{directory_slug}'",
                path.display(),
                runtime.slug
            ));
        }
        if !is_reverse_dns(&runtime.id) {
            errors.push(format!(
                "{}: id '{}' is not reverse-DNS",
                path.display(),
                runtime.id
            ));
        }
        if let Some(first) = ids.insert(&runtime.id, path) {
            errors.push(format!(
                "{}: duplicate id '{}' first declared in {}",
                path.display(),
                runtime.id,
                first.display()
            ));
        }
        if runtime.label.trim().is_empty() {
            errors.push(format!("{}: label must not be empty", path.display()));
        }
        if let Some(first) = display_orders.insert(runtime.display.order, path) {
            errors.push(format!(
                "{}: display.order {} first declared in {}",
                path.display(),
                runtime.display.order,
                first.display()
            ));
        }
        let visibility_key = runtime.display.visibility_config_key.as_str();
        if !is_visibility_config_key(visibility_key) {
            errors.push(format!(
                "{}: display.visibility_config_key '{visibility_key}' must be a lowercase snake_case key ending in '{VISIBILITY_CONFIG_KEY_SUFFIX}'",
                path.display()
            ));
        }
        if let Some(first) = visibility_keys.insert(visibility_key, path) {
            errors.push(format!(
                "{}: display.visibility_config_key '{visibility_key}' first declared in {}",
                path.display(),
                first.display()
            ));
        }
        if runtime.platforms.is_empty() {
            errors.push(format!("{}: platforms must not be empty", path.display()));
        }
        let platform_count = runtime
            .platforms
            .iter()
            .map(|value| *value as u8)
            .collect::<BTreeSet<_>>()
            .len();
        if platform_count != runtime.platforms.len() {
            errors.push(format!(
                "{}: platforms contains duplicate values",
                path.display()
            ));
        }
        if runtime.detection.command_aliases.is_empty() {
            errors.push(format!(
                "{}: detection.command_aliases must not be empty",
                path.display()
            ));
        }
        for (field, values) in [
            (
                "detection.command_aliases",
                &runtime.detection.command_aliases,
            ),
            (
                "detection.process_aliases",
                &runtime.detection.process_aliases,
            ),
        ] {
            let mut local = BTreeSet::new();
            for alias in values {
                if !is_alias(alias) {
                    errors.push(format!(
                        "{}: {field} contains invalid alias '{alias}'",
                        path.display()
                    ));
                }
                if !local.insert(alias) {
                    errors.push(format!(
                        "{}: {field} contains duplicate alias '{alias}'",
                        path.display()
                    ));
                }
                if CONTESTED_ALIASES.contains(&alias.as_str())
                    && runtime.detection.title_prefix.is_none()
                {
                    errors.push(format!(
                        "{}: {field} alias '{alias}' is also the name of another program and requires detection.title_prefix to confirm the runtime from its terminal title",
                        path.display()
                    ));
                }
                let key = alias.to_ascii_lowercase();
                if let Some((owner, owner_path)) = aliases.insert(key, (&runtime.id, path)) {
                    if owner_path != path {
                        errors.push(format!(
                            "{}: {field} alias '{alias}' conflicts with runtime '{owner}' in {}",
                            path.display(),
                            owner_path.display()
                        ));
                    }
                }
            }
        }
        if runtime
            .detection
            .title_prefix
            .as_deref()
            .is_some_and(|prefix| prefix.trim().is_empty())
        {
            errors.push(format!(
                "{}: detection.title_prefix must not be blank",
                path.display()
            ));
        }
        if runtime.lifecycle.bell_attention == Some(true)
            && runtime.lifecycle.authority == LifecycleAuthority::Complete
        {
            errors.push(format!(
                "{}: lifecycle.bell_attention = true conflicts with authority = 'complete', whose hooks own attention",
                path.display()
            ));
        }
        let screen_states = located.screen.as_ref().map(|screen| &screen.states);
        if runtime.lifecycle.fallback == LifecycleFallback::Screen
            && !screen_states
                .is_some_and(|states| states.contains("working") && states.contains("idle"))
        {
            errors.push(format!(
                "{}: lifecycle.fallback = 'screen' requires a {SCREEN_RULES_FILE} beside it with at least one working and one idle rule",
                path.display()
            ));
        }
        if runtime.lifecycle.authority == LifecycleAuthority::None
            && runtime.lifecycle.fallback != LifecycleFallback::None
        {
            errors.push(format!(
                "{}: lifecycle.authority = 'none' requires fallback = 'none'",
                path.display()
            ));
        }
        if runtime.lifecycle.authority == LifecycleAuthority::Screen
            && runtime.lifecycle.fallback != LifecycleFallback::Screen
        {
            errors.push(format!(
                "{}: lifecycle.authority = 'screen' requires fallback = 'screen'",
                path.display()
            ));
        }
        let adapter = runtime.integration.hook_adapter;
        if runtime.lifecycle.authority == LifecycleAuthority::Complete && !adapter.has_installer() {
            errors.push(format!(
                "{}: lifecycle.authority = 'complete' requires an integration.hook_adapter with an installer, got '{}'; accepted adapters: {INSTALLER_HOOK_ADAPTERS}",
                path.display(),
                adapter.name()
            ));
        }
        if adapter.has_installer() && runtime.integration.mcp_config.is_none() {
            errors.push(format!(
                "{}: integration.hook_adapter = '{}' installs the MCP bridge with the hooks and requires an integration.mcp_config; accepted writers: {ACCEPTED_MCP_CONFIGS}",
                path.display(),
                adapter.name()
            ));
        }
        if adapter.has_installer() && runtime.lifecycle.authority != LifecycleAuthority::Complete {
            errors.push(format!(
                "{}: integration.hook_adapter = '{}' installs lifecycle hooks and requires lifecycle.authority = 'complete'",
                path.display(),
                adapter.name()
            ));
        }
        if parse_tint(runtime.display.tint.as_deref()).is_err() {
            errors.push(format!(
                "{}: display.tint must be #RRGGBB or omitted",
                path.display()
            ));
        }
        if !runtime.display.icon_asset_path.starts_with("icons/")
            && !runtime.display.icon_asset_path.starts_with("agents/")
        {
            errors.push(format!(
                "{}: display.icon_asset_path must use an embedded asset root",
                path.display()
            ));
        }
        if let Some(resume) = &runtime.resume {
            validate_resume(
                path,
                &runtime.detection.command_aliases,
                resume,
                &mut errors,
            );
        }
        if runtime.suggested_presets.is_empty() {
            errors.push(format!(
                "{}: suggested_presets must not be empty",
                path.display()
            ));
        }
        for preset in &runtime.suggested_presets {
            if preset.id.is_empty() || preset.command.is_empty() {
                errors.push(format!(
                    "{}: suggested_presets fields must not be empty",
                    path.display()
                ));
            }
            validate_preset(path, &runtime.platforms, preset, &mut errors);
        }
        let first_has_extras = runtime.suggested_presets.first().is_some_and(|first| {
            first.name.is_some()
                || first.platforms.is_some()
                || first.tmux_compat
                || !first.env.is_empty()
        });
        if first_has_extras {
            errors.push(format!(
                "{}: the first suggested preset is the runtime's own launch and takes only id and command",
                path.display()
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn validate_resume(
    path: &Path,
    command_aliases: &[String],
    resume: &Resume,
    errors: &mut Vec<String>,
) {
    let templates = [
        ("resume.session_argv", Some(&resume.session_argv), true),
        ("resume.continue_argv", resume.continue_argv.as_ref(), false),
        ("resume.fork_argv", resume.fork_argv.as_ref(), true),
    ];
    for (field, argv, needs_session_id) in templates {
        let Some(argv) = argv else {
            continue;
        };
        let Some(program) = argv.first() else {
            errors.push(format!("{}: {field} must not be empty", path.display()));
            continue;
        };
        if program == SESSION_ID_PLACEHOLDER {
            errors.push(format!(
                "{}: {field} must not put {SESSION_ID_PLACEHOLDER} in program position",
                path.display()
            ));
        } else if !command_aliases.contains(program) {
            errors.push(format!(
                "{}: {field} program '{program}' is not one of the runtime's detection.command_aliases",
                path.display()
            ));
        }
        let placeholders = argv
            .iter()
            .filter(|arg| arg.as_str() == SESSION_ID_PLACEHOLDER)
            .count();
        if needs_session_id && placeholders != 1 {
            errors.push(format!(
                "{}: {field} must contain {SESSION_ID_PLACEHOLDER} exactly once as a whole argument",
                path.display()
            ));
        }
        if !needs_session_id && placeholders != 0 {
            errors.push(format!(
                "{}: {field} must not contain {SESSION_ID_PLACEHOLDER}",
                path.display()
            ));
        }
        for arg in argv.iter().skip(1) {
            if arg != SESSION_ID_PLACEHOLDER && !is_plain_argument(arg) {
                errors.push(format!(
                    "{}: {field} argument '{arg}' must use only ASCII letters, digits, '-', '_', '.' or '='",
                    path.display()
                ));
            }
        }
    }
    if let Err(error) = regex::RegexBuilder::new(&resume.session_id_pattern)
        .size_limit(MAX_SESSION_ID_PATTERN_BYTES)
        .build()
    {
        errors.push(format!(
            "{}: resume.session_id_pattern is not a valid regex: {error}",
            path.display()
        ));
    }
    if resume
        .failure_markers
        .iter()
        .any(|marker| marker.trim().is_empty())
    {
        errors.push(format!(
            "{}: resume.failure_markers must not contain empty markers",
            path.display()
        ));
    }
}

fn is_plain_argument(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'='))
}

fn is_visibility_config_key(value: &str) -> bool {
    value
        .strip_suffix(VISIBILITY_CONFIG_KEY_SUFFIX)
        .is_some_and(|stem| {
            stem.starts_with(|c: char| c.is_ascii_lowercase())
                && stem
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
}

fn is_reverse_dns(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() >= 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && part
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && part
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
}

fn is_alias(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

fn parse_tint(value: Option<&str>) -> Result<Option<u32>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(hex) = value.strip_prefix('#') else {
        return Err(());
    };
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(());
    }
    u32::from_str_radix(hex, 16).map(Some).map_err(|_| ())
}

pub fn generate_catalog(descriptors: &[LocatedDescriptor]) -> Result<String, String> {
    let mut output = String::from("pub static RUNTIMES: &[Runtime] = &[\n");
    for located in descriptors {
        let runtime = &located.descriptor;
        let tint = match parse_tint(runtime.display.tint.as_deref()) {
            Ok(Some(value)) => format!("Some(0x{value:06X})"),
            Ok(None) => "None".to_string(),
            Err(()) => return Err(format!("{}: invalid tint", located.path.display())),
        };
        output.push_str("Runtime {\n");
        output.push_str(&format!(
            "id: {:?}, slug: {:?}, label: {:?},\n",
            runtime.id, runtime.slug, runtime.label
        ));
        output.push_str(&format!("platforms: &{},\n", platforms(&runtime.platforms)));
        output.push_str(&format!(
            "display: RuntimeDisplay {{ order: {}, tint: {tint}, icon_asset_path: {:?}, icon_multicolor: {}, visibility_config_key: {:?} }},\n",
            runtime.display.order,
            runtime.display.icon_asset_path,
            runtime.display.icon_multicolor,
            runtime.display.visibility_config_key
        ));
        output.push_str(&format!(
            "detection: RuntimeDetection {{ command_aliases: &{}, process_aliases: &{}, script_path_signatures: &{}, title_prefix: {} }},\n",
            strings(&runtime.detection.command_aliases),
            strings(&runtime.detection.process_aliases),
            strings(&runtime.detection.script_path_signatures),
            option_string(runtime.detection.title_prefix.as_deref())
        ));
        output.push_str(&format!(
            "environment: RuntimeEnvironment {{ strip_inherited: &{} }},\n",
            strings(&runtime.environment.strip_inherited)
        ));
        output.push_str(&format!(
            "lifecycle: RuntimeLifecycle {{ source: RuntimeLifecycleSource::{}, authority: RuntimeLifecycleAuthority::{}, fallback: RuntimeLifecycleFallback::{}, escape_cancels_turn: {}, attention_clears_on_output: {}, anchor_start_event_to_output: {}, bell_attention: {} }},\n",
            source(runtime.lifecycle.source),
            authority(runtime.lifecycle.authority),
            fallback(runtime.lifecycle.fallback),
            runtime.lifecycle.escape_cancels_turn,
            runtime.lifecycle.attention_clears_on_output,
            runtime.lifecycle.anchor_start_event_to_output,
            runtime.lifecycle.resolved_bell_attention()
        ));
        output.push_str(&format!(
            "integration: RuntimeIntegration {{ summary: {:?}, post_install_step: {}, hook_adapter: RuntimeHookAdapter::{}, mcp_config: {}, skills_dir: {} }},\n",
            runtime.integration.summary,
            option_string(runtime.integration.post_install_step.as_deref()),
            hook_adapter(runtime.integration.hook_adapter),
            runtime
                .integration
                .mcp_config
                .map_or("None".to_string(), |config| format!(
                    "Some(RuntimeMcpConfig::{})",
                    config.variant()
                )),
            runtime
                .integration
                .skills_dir
                .map_or("None".to_string(), |dir| format!(
                    "Some(RuntimeSkillsDir::{})",
                    dir.variant()
                ))
        ));
        match &runtime.sessions {
            Some(sessions) => output.push_str(&format!(
                "sessions: Some(RuntimeSessions {{ reader: RuntimeSessionReader::{} }}),\n",
                sessions.reader.variant()
            )),
            None => output.push_str("sessions: None,\n"),
        }
        output.push_str("suggested_presets: &[\n");
        for preset in &runtime.suggested_presets {
            let env = preset
                .env
                .iter()
                .map(|(key, value)| format!("({key:?}, {value:?})"))
                .collect::<Vec<_>>()
                .join(", ");
            output.push_str(&format!(
                "RuntimeSuggestedPreset {{ id: {:?}, command: {:?}, name: {}, platforms: {}, tmux_compat: {}, env: &[{env}] }},\n",
                preset.id,
                preset.command,
                option_string(preset.name.as_deref()),
                preset
                    .platforms
                    .as_deref()
                    .map_or_else(|| "None".to_string(), |values| format!("Some(&{})", platforms(values))),
                preset.tmux_compat
            ));
        }
        output.push_str("] },\n");
    }
    output.push_str("];\n");
    output.push_str(&format!(
        "pub const RUNTIME_COUNT: usize = {};\npub const SESSION_RUNTIME_COUNT: usize = {};\n",
        descriptors.len(),
        descriptors
            .iter()
            .filter(|located| located.descriptor.sessions.is_some())
            .count()
    ));
    output.push_str("#[macro_export]\nmacro_rules! runtime_identity_constants {\n($ty:ident) => {\n#[allow(non_upper_case_globals)]\nimpl $ty {\n");
    for located in descriptors {
        output.push_str(&format!(
            "pub const {}: $ty = $ty({:?});\n",
            constant_name(&located.descriptor.slug),
            located.descriptor.id
        ));
    }
    output.push_str("}\n};\n}\n");
    output.push_str("pub fn canonical_command_for_alias(alias: &str) -> Option<&'static str> {\nmatch alias {\n");
    for located in descriptors {
        let runtime = &located.descriptor;
        let aliases = runtime
            .detection
            .command_aliases
            .iter()
            .chain(runtime.detection.process_aliases.iter())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|alias| format!("{alias:?}"))
            .collect::<Vec<_>>()
            .join(" | ");
        output.push_str(&format!(
            "{aliases} => Some({:?}),\n",
            runtime.detection.command_aliases[0]
        ));
    }
    output.push_str("_ => None,\n}\n}\n");
    output.push_str(
        "pub fn canonical_command_for_script_path(path: &str) -> Option<&'static str> {\n",
    );
    for located in descriptors {
        let runtime = &located.descriptor;
        for signature in &runtime.detection.script_path_signatures {
            let windows = signature.replace('/', "\\");
            output.push_str(&format!(
                "if path.as_bytes().windows({}).any(|window| window == {signature:?}.as_bytes() || window == {windows:?}.as_bytes()) {{ return Some({:?}); }}\n",
                signature.len(),
                runtime.detection.command_aliases[0]
            ));
        }
    }
    output.push_str("None\n}\n");
    output.push_str(
        "pub fn command_alias_literal(alias: &str) -> Option<&'static str> {\nmatch alias {\n",
    );
    for located in descriptors {
        for alias in &located.descriptor.detection.command_aliases {
            output.push_str(&format!("{alias:?} => Some({alias:?}),\n"));
        }
    }
    output.push_str("_ => None,\n}\n}\n");
    output.push_str(
        "pub fn runtime_resume(runtime_id: &str) -> Option<&'static RuntimeResume> {\nmatch runtime_id {\n",
    );
    for located in descriptors {
        let runtime = &located.descriptor;
        let Some(resume) = &runtime.resume else {
            continue;
        };
        output.push_str(&format!(
            "{:?} => Some(&RuntimeResume {{ session_argv: &{}, continue_argv: {}, fork_argv: {}, session_id_pattern: {:?}, failure_markers: &{} }}),\n",
            runtime.id,
            strings(&resume.session_argv),
            option_strings(resume.continue_argv.as_deref()),
            option_strings(resume.fork_argv.as_deref()),
            resume.session_id_pattern,
            strings(&resume.failure_markers)
        ));
    }
    output.push_str("_ => None,\n}\n}\n");
    output.push_str("#[cfg(feature = \"screen-rules\")]\npub static SCREEN_RULE_SOURCES: &[(&str, &str)] = &[\n");
    for located in descriptors {
        let Some(screen) = &located.screen else {
            continue;
        };
        let path = fs::canonicalize(&screen.path)
            .map_err(|error| format!("{}: {error}", screen.path.display()))?;
        let path = path
            .to_str()
            .ok_or_else(|| format!("{}: the path is not valid UTF-8", screen.path.display()))?;
        output.push_str(&format!(
            "({:?}, include_str!({:?})),\n",
            located.descriptor.slug,
            path.strip_prefix(r"\\?\").unwrap_or(path)
        ));
    }
    output.push_str("];\n");
    Ok(output)
}

fn constant_name(slug: &str) -> String {
    slug.split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}

fn option_strings(values: Option<&[String]>) -> String {
    values.map_or_else(
        || "None".to_string(),
        |values| format!("Some(&{})", strings(values)),
    )
}

fn strings(values: &[String]) -> String {
    let values = values
        .iter()
        .map(|value| format!("{value:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{values}]")
}

fn validate_preset(
    path: &Path,
    runtime_platforms: &[Platform],
    preset: &SuggestedPreset,
    errors: &mut Vec<String>,
) {
    if preset.name.as_deref().is_some_and(str::is_empty) {
        errors.push(format!(
            "{}: suggested preset '{}' has an empty name",
            path.display(),
            preset.id
        ));
    }
    if let Some(platforms) = &preset.platforms {
        let outside = platforms.iter().any(|platform| {
            !runtime_platforms
                .iter()
                .any(|supported| *supported as u8 == *platform as u8)
        });
        if platforms.is_empty() || outside {
            errors.push(format!(
                "{}: suggested preset '{}' platforms must be a non-empty subset of the runtime platforms",
                path.display(),
                preset.id
            ));
        }
    }
    if preset.tmux_compat
        && preset
            .platforms
            .as_deref()
            .unwrap_or(runtime_platforms)
            .iter()
            .any(|platform| matches!(platform, Platform::Windows))
    {
        errors.push(format!(
            "{}: suggested preset '{}' uses tmux_compat, which has no Windows path",
            path.display(),
            preset.id
        ));
    }
    for key in preset.env.keys() {
        let valid = !key.is_empty()
            && !key.starts_with(|c: char| c.is_ascii_digit())
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            errors.push(format!(
                "{}: suggested preset '{}' has an invalid env key '{key}'",
                path.display(),
                preset.id
            ));
        }
    }
}

fn platforms(values: &[Platform]) -> String {
    let values = values
        .iter()
        .map(|value| match value {
            Platform::Linux => "RuntimePlatform::Linux",
            Platform::Macos => "RuntimePlatform::Macos",
            Platform::Windows => "RuntimePlatform::Windows",
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{values}]")
}

fn option_string(value: Option<&str>) -> String {
    value.map_or_else(|| "None".to_string(), |value| format!("Some({value:?})"))
}

fn source(value: LifecycleSource) -> &'static str {
    match value {
        LifecycleSource::Hooks => "Hooks",
        LifecycleSource::Output => "Output",
        LifecycleSource::None => "None",
    }
}

fn authority(value: LifecycleAuthority) -> &'static str {
    match value {
        LifecycleAuthority::Complete => "Complete",
        LifecycleAuthority::Screen => "Screen",
        LifecycleAuthority::None => "None",
    }
}

fn fallback(value: LifecycleFallback) -> &'static str {
    match value {
        LifecycleFallback::Screen => "Screen",
        LifecycleFallback::None => "None",
    }
}

fn hook_adapter(value: HookAdapter) -> &'static str {
    match value {
        HookAdapter::Claude => "Claude",
        HookAdapter::Codex => "Codex",
        HookAdapter::None => "None",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(slug: &str, id: &str, alias: &str) -> String {
        format!(
            r##"schema_version = 1
id = "{id}"
slug = "{slug}"
label = "Alpha"
platforms = ["linux"]

[display]
order = 0
tint = "#112233"
icon_asset_path = "agents/alpha.svg"
icon_multicolor = false
visibility_config_key = "{slug}_button_visible"

[detection]
command_aliases = ["{alias}"]
process_aliases = ["{alias}"]
script_path_signatures = []

[environment]
strip_inherited = []

[lifecycle]
source = "output"
authority = "none"
fallback = "none"
escape_cancels_turn = false
attention_clears_on_output = true
anchor_start_event_to_output = true

[integration]
summary = "None"
hook_adapter = "none"

[[suggested_presets]]
id = "{slug}"
command = "{alias}"
"##
        )
    }

    fn write_runtime(root: &Path, slug: &str, text: &str) {
        let directory = root.join(slug);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("runtime.toml"), text).unwrap();
    }

    #[test]
    fn rejects_unknown_fields_with_the_descriptor_path() {
        let temp = tempfile::TempDir::new().unwrap();
        let text = descriptor("alpha", "com.example.alpha", "alpha") + "\nunknown = true\n";
        write_runtime(temp.path(), "alpha", &text);
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(error.contains("runtime.toml"));
        assert!(error.contains("unknown field"));
    }

    #[test]
    fn rejects_every_retired_descriptor_key_by_name() {
        let base = descriptor("alpha", "com.example.alpha", "alpha");
        let cases = [
            (
                "capabilities",
                base.replace(
                    "platforms = [\"linux\"]\n",
                    "platforms = [\"linux\"]\ncapabilities = []\n",
                ),
            ),
            (
                "terminal_title_signal",
                base.replace(
                    "anchor_start_event_to_output = true\n",
                    "anchor_start_event_to_output = true\nterminal_title_signal = false\n",
                ),
            ),
            (
                "install",
                base.replace(
                    "[[suggested_presets]]",
                    "[install]\ncommand = \"\"\n\n[[suggested_presets]]",
                ),
            ),
            (
                "label",
                base.replace("id = \"alpha\"\n", "id = \"alpha\"\nlabel = \"Alpha\"\n"),
            ),
            (
                "quick_launch",
                base.replace(
                    "command = \"alpha\"\n",
                    "command = \"alpha\"\nquick_launch = false\n",
                ),
            ),
        ];
        for (key, text) in cases {
            assert_ne!(text, base, "{key} fixture did not change");
            let temp = tempfile::TempDir::new().unwrap();
            write_runtime(temp.path(), "alpha", &text);
            let error = discover_and_validate(temp.path()).unwrap_err();
            assert!(error.contains("runtime.toml"), "{key}: {error}");
            assert!(
                error.contains(&format!("unknown field `{key}`")),
                "{key}: {error}"
            );
        }
    }

    #[test]
    fn rejects_slug_policy_and_duplicate_identity_failures() {
        let temp = tempfile::TempDir::new().unwrap();
        let one = descriptor("wrong", "com.example.same", "same");
        let two = descriptor("beta", "com.example.same", "same");
        write_runtime(temp.path(), "alpha", &one);
        write_runtime(temp.path(), "beta", &two);
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(error.contains("differs from directory name"));
        assert!(error.contains("duplicate id"));
        assert!(error.contains("conflicts with runtime"));
    }

    #[test]
    fn a_complete_authority_without_an_installer_fails_naming_the_file_field_and_adapters() {
        let temp = tempfile::TempDir::new().unwrap();
        let text = descriptor("alpha", "com.example.alpha", "alpha")
            .replace("source = \"output\"", "source = \"hooks\"")
            .replace("authority = \"none\"", "authority = \"complete\"");
        write_runtime(temp.path(), "alpha", &text);
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(error.contains("alpha"), "{error}");
        assert!(error.contains("runtime.toml"), "{error}");
        assert!(error.contains("lifecycle.authority"), "{error}");
        assert!(error.contains("integration.hook_adapter"), "{error}");
        assert!(
            error.contains("accepted adapters: claude, codex"),
            "{error}"
        );
    }

    #[test]
    fn a_hook_adapter_without_an_installer_fails_listing_the_accepted_adapters() {
        let temp = tempfile::TempDir::new().unwrap();
        let text = descriptor("alpha", "com.example.alpha", "alpha")
            .replace("authority = \"none\"", "authority = \"complete\"")
            .replace("hook_adapter = \"none\"", "hook_adapter = \"gemini\"");
        write_runtime(temp.path(), "alpha", &text);
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(error.contains("runtime.toml"), "{error}");
        assert!(
            error.contains("integration.hook_adapter 'gemini'"),
            "{error}"
        );
        assert!(
            error.contains("accepted adapters: none, claude, codex"),
            "{error}"
        );
    }

    #[test]
    fn an_installer_adapter_requires_an_mcp_writer_and_unknown_writers_are_named() {
        let temp = tempfile::TempDir::new().unwrap();
        let text = descriptor("alpha", "com.example.alpha", "alpha")
            .replace("hook_adapter = \"none\"", "hook_adapter = \"claude\"");
        write_runtime(temp.path(), "alpha", &text);
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(
            error.contains("requires an integration.mcp_config"),
            "{error}"
        );

        let temp = tempfile::TempDir::new().unwrap();
        let text = descriptor("alpha", "com.example.alpha", "alpha").replace(
            "hook_adapter = \"none\"",
            "hook_adapter = \"none\"\nmcp_config = \"cursor\"\nskills_dir = \"gemini\"",
        );
        write_runtime(temp.path(), "alpha", &text);
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(
            error.contains("integration.mcp_config 'cursor'")
                && error.contains(ACCEPTED_MCP_CONFIGS),
            "{error}"
        );
    }

    #[test]
    fn an_installer_adapter_requires_the_complete_authority() {
        let temp = tempfile::TempDir::new().unwrap();
        let text = descriptor("alpha", "com.example.alpha", "alpha")
            .replace("hook_adapter = \"none\"", "hook_adapter = \"claude\"");
        write_runtime(temp.path(), "alpha", &text);
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(
            error.contains("requires lifecycle.authority = 'complete'"),
            "{error}"
        );
    }

    #[test]
    fn a_screen_authority_requires_the_screen_fallback() {
        let temp = tempfile::TempDir::new().unwrap();
        let text = descriptor("alpha", "com.example.alpha", "alpha")
            .replace("authority = \"none\"", "authority = \"screen\"");
        write_runtime(temp.path(), "alpha", &text);
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(
            error.contains("lifecycle.authority = 'screen' requires fallback = 'screen'"),
            "{error}"
        );
    }

    #[test]
    fn an_invalid_screen_rule_file_fails_the_build_with_its_path_and_line() {
        let temp = tempfile::TempDir::new().unwrap();
        let text = descriptor("alpha", "com.example.alpha", "alpha")
            .replace("authority = \"none\"", "authority = \"screen\"")
            .replace("fallback = \"none\"", "fallback = \"screen\"");
        write_runtime(temp.path(), "alpha", &text);
        fs::write(
            temp.path().join("alpha").join(SCREEN_RULES_FILE),
            "engine = 2\n\n[[rules]]\nid = \"busy\"\nstate = \"working\"\nany = ['(open']\n",
        )
        .unwrap();
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(error.contains("screen.toml: line 6"), "{error}");
        assert!(error.contains("invalid regex"), "{error}");

        fs::write(
            temp.path().join("alpha").join(SCREEN_RULES_FILE),
            "engine = 2\n\n[[rules]]\nid = \"busy\"\nstate = \"working\"\nany = ['busy']\n",
        )
        .unwrap();
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(
            error.contains("at least one working and one idle rule"),
            "{error}"
        );

        fs::write(
            temp.path().join("alpha").join(SCREEN_RULES_FILE),
            "engine = 2\n\n[[rules]]\nid = \"busy\"\nstate = \"working\"\nany = ['busy']\n\n[[rules]]\nid = \"prompt\"\nstate = \"idle\"\nany = ['>']\n",
        )
        .unwrap();
        let descriptors = discover_and_validate(temp.path()).unwrap();
        let generated = generate_catalog(&descriptors).unwrap();
        assert!(generated.contains("SCREEN_RULE_SOURCES"), "{generated}");
        assert!(
            generated.contains("(\"alpha\", include_str!("),
            "{generated}"
        );
    }

    #[test]
    fn rejects_screen_and_authority_inconsistencies() {
        let temp = tempfile::TempDir::new().unwrap();
        let text = descriptor("alpha", "com.example.alpha", "alpha")
            .replace("fallback = \"none\"", "fallback = \"screen\"");
        write_runtime(temp.path(), "alpha", &text);
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(error.contains("requires a screen.toml"));
        assert!(error.contains("authority = 'none'"));
    }

    fn with_resume(resume: &str) -> String {
        descriptor("alpha", "com.example.alpha", "alpha").replace(
            "[[suggested_presets]]",
            &format!("[resume]\n{resume}\n[[suggested_presets]]"),
        )
    }

    fn resume_error(resume: &str) -> String {
        let temp = tempfile::TempDir::new().unwrap();
        write_runtime(temp.path(), "alpha", &with_resume(resume));
        discover_and_validate(temp.path()).unwrap_err()
    }

    #[test]
    fn a_resume_section_with_alias_programs_and_a_valid_pattern_is_generated() {
        let temp = tempfile::TempDir::new().unwrap();
        let text = with_resume(
            "session_argv = [\"alpha\", \"--resume\", \"{session_id}\"]\ncontinue_argv = [\"alpha\", \"--continue\"]\nfork_argv = [\"alpha\", \"fork\", \"{session_id}\"]\nsession_id_pattern = \"^[0-9a-f-]{36}$\"\nfailure_markers = [\"No session\"]\n",
        );
        write_runtime(temp.path(), "alpha", &text);
        let descriptors = discover_and_validate(temp.path()).unwrap();
        let generated = generate_catalog(&descriptors).unwrap();
        assert!(generated.contains("pub fn runtime_resume"), "{generated}");
        assert!(
            generated.contains("session_argv: &[\"alpha\", \"--resume\", \"{session_id}\"]"),
            "{generated}"
        );
        assert!(
            generated.contains("continue_argv: Some(&[\"alpha\", \"--continue\"])"),
            "{generated}"
        );
    }

    #[test]
    fn a_resume_program_outside_the_command_aliases_is_refused() {
        let error = resume_error(
            "session_argv = [\"sh\", \"-c\", \"{session_id}\"]\nsession_id_pattern = \".*\"\nfailure_markers = []\n",
        );
        assert!(
            error.contains("resume.session_argv program 'sh'"),
            "{error}"
        );
        assert!(error.contains("detection.command_aliases"), "{error}");
    }

    #[test]
    fn a_session_id_in_program_position_is_refused() {
        let error = resume_error(
            "session_argv = [\"alpha\", \"{session_id}\"]\nfork_argv = [\"{session_id}\", \"alpha\"]\nsession_id_pattern = \".*\"\nfailure_markers = []\n",
        );
        assert!(
            error.contains("resume.fork_argv must not put {session_id} in program position"),
            "{error}"
        );
    }

    #[test]
    fn an_invalid_session_id_pattern_is_refused() {
        let error = resume_error(
            "session_argv = [\"alpha\", \"{session_id}\"]\nsession_id_pattern = \"(unclosed\"\nfailure_markers = []\n",
        );
        assert!(
            error.contains("resume.session_id_pattern is not a valid regex"),
            "{error}"
        );
    }

    #[test]
    fn a_template_without_exactly_one_session_id_or_with_shell_syntax_is_refused() {
        let error = resume_error(
            "session_argv = [\"alpha\", \"--resume\"]\ncontinue_argv = [\"alpha\", \"{session_id}\", \"$(id)\"]\nsession_id_pattern = \".*\"\nfailure_markers = []\n",
        );
        assert!(
            error.contains("resume.session_argv must contain {session_id} exactly once"),
            "{error}"
        );
        assert!(
            error.contains("resume.continue_argv must not contain {session_id}"),
            "{error}"
        );
        assert!(error.contains("argument '$(id)'"), "{error}");
    }

    #[test]
    fn an_unknown_session_reader_fails_listing_the_accepted_readers() {
        let temp = tempfile::TempDir::new().unwrap();
        let text = descriptor("alpha", "com.example.alpha", "alpha").replace(
            "[[suggested_presets]]",
            "[sessions]\nreader = \"inconnu\"\n\n[[suggested_presets]]",
        );
        write_runtime(temp.path(), "alpha", &text);
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(error.contains("runtime.toml"), "{error}");
        assert!(error.contains("sessions.reader 'inconnu'"), "{error}");
        assert!(
            error.contains("accepted readers: claude, codex, opencode, pi, gemini, grok"),
            "{error}"
        );
    }

    #[test]
    fn a_visibility_key_outside_the_button_visible_convention_or_duplicated_is_refused() {
        let temp = tempfile::TempDir::new().unwrap();
        let text = descriptor("alpha", "com.example.alpha", "alpha")
            .replace("alpha_button_visible", "AlphaVisible");
        write_runtime(temp.path(), "alpha", &text);
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(
            error.contains("display.visibility_config_key 'AlphaVisible'"),
            "{error}"
        );

        let temp = tempfile::TempDir::new().unwrap();
        write_runtime(
            temp.path(),
            "alpha",
            &descriptor("alpha", "com.example.alpha", "alpha"),
        );
        write_runtime(
            temp.path(),
            "beta",
            &descriptor("beta", "com.example.beta", "beta")
                .replace("order = 0", "order = 1")
                .replace("beta_button_visible", "alpha_button_visible"),
        );
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(
            error.contains("display.visibility_config_key 'alpha_button_visible' first declared"),
            "{error}"
        );
    }

    #[test]
    fn a_contested_alias_is_refused_unless_a_title_prefix_confirms_the_runtime() {
        let temp = tempfile::TempDir::new().unwrap();
        write_runtime(
            temp.path(),
            "alpha",
            &descriptor("alpha", "com.example.alpha", "fx"),
        );
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(
            error.contains("alias 'fx' is also the name of another program and requires detection.title_prefix"),
            "{error}"
        );

        let confirmed = descriptor("alpha", "com.example.alpha", "fx").replace(
            "script_path_signatures = []",
            "script_path_signatures = []\ntitle_prefix = \"fx v\"",
        );
        write_runtime(temp.path(), "alpha", &confirmed);
        let descriptors = discover_and_validate(temp.path()).unwrap();
        let generated = generate_catalog(&descriptors).unwrap();
        assert!(
            generated.contains("title_prefix: Some(\"fx v\")"),
            "{generated}"
        );
        assert!(generated.contains("bell_attention: true"), "{generated}");

        write_runtime(
            temp.path(),
            "alpha",
            &confirmed.replace("title_prefix = \"fx v\"", "title_prefix = \" \""),
        );
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(
            error.contains("detection.title_prefix must not be blank"),
            "{error}"
        );
    }

    #[test]
    fn bell_attention_defaults_from_the_authority_and_is_refused_beside_complete_hooks() {
        let temp = tempfile::TempDir::new().unwrap();
        let hooked = descriptor("alpha", "com.example.alpha", "alpha")
            .replace("authority = \"none\"", "authority = \"complete\"")
            .replace("source = \"output\"", "source = \"hooks\"")
            .replace(
                "hook_adapter = \"none\"",
                "hook_adapter = \"claude\"\nmcp_config = \"claude\"",
            );
        write_runtime(temp.path(), "alpha", &hooked);
        let descriptors = discover_and_validate(temp.path()).unwrap();
        let generated = generate_catalog(&descriptors).unwrap();
        assert!(generated.contains("bell_attention: false"), "{generated}");

        write_runtime(
            temp.path(),
            "alpha",
            &hooked.replace(
                "anchor_start_event_to_output = true",
                "anchor_start_event_to_output = true\nbell_attention = true",
            ),
        );
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(
            error.contains("lifecycle.bell_attention = true conflicts with authority = 'complete'"),
            "{error}"
        );
    }

    fn fixture_catalog_with_an_added_runtime() -> tempfile::TempDir {
        let catalog = tempfile::TempDir::new().unwrap();
        let runtimes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtimes");
        for entry in fs::read_dir(&runtimes).unwrap() {
            let source = entry.unwrap().path().join("runtime.toml");
            if source.is_file() {
                let slug = source.parent().unwrap().file_name().unwrap();
                fs::create_dir_all(catalog.path().join(slug)).unwrap();
                fs::copy(&source, catalog.path().join(slug).join("runtime.toml")).unwrap();
                let rules = source.with_file_name("screen.toml");
                if rules.is_file() {
                    fs::copy(&rules, catalog.path().join(slug).join("screen.toml")).unwrap();
                }
            }
        }
        write_runtime(
            catalog.path(),
            "alpha",
            &descriptor("alpha", "dev.example.alpha", "alpha-cli")
                .replace("order = 0", "order = 99"),
        );
        catalog
    }

    #[test]
    fn a_runtime_added_without_rust_edits_compiles_into_the_launcher_and_visibility_table() {
        let catalog = fixture_catalog_with_an_added_runtime();
        let descriptors = discover_and_validate(catalog.path()).unwrap();
        let generated = generate_catalog(&descriptors).unwrap();
        let build = tempfile::TempDir::new().unwrap();
        fs::write(build.path().join("runtime_catalog.rs"), generated).unwrap();
        let catalog_module = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("runtime_catalog.rs");
        let program = format!(
            r#"#![allow(dead_code)]
#[macro_use]
#[path = {catalog_module:?}]
mod runtime_catalog;
use runtime_catalog::*;
struct Agent(&'static str);
runtime_identity_constants!(Agent);
fn main() {{
    let explicit = launcher_runtimes(|key| (key == "alpha_button_visible").then_some(true), |_| false);
    assert_eq!(explicit.iter().map(|runtime| runtime.slug).collect::<Vec<_>>(), vec!["alpha"]);
    let installed = launcher_runtimes(|_| None, |runtime| runtime.slug == "alpha" || runtime.slug == "codex");
    assert_eq!(installed.iter().map(|runtime| runtime.slug).collect::<Vec<_>>(), vec!["codex", "alpha"]);
    let hidden = launcher_runtimes(|key| (key == "alpha_button_visible").then_some(false), |_| true);
    assert!(hidden.iter().all(|runtime| runtime.slug != "alpha"));
    assert!(visibility_config_keys().any(|key| key == "alpha_button_visible"));
    assert_eq!(Agent::Alpha.0, "dev.example.alpha");
    assert_eq!(Agent::ClaudeCode.0, "com.anthropic.claude-code");
    assert_eq!(RUNTIME_COUNT, RUNTIMES.len());
    assert!(runtime_by_id("dev.example.alpha").is_some_and(|runtime| runtime.sessions.is_none()));
}}
"#
        );
        let source = build.path().join("main.rs");
        fs::write(&source, program).unwrap();
        let binary = build
            .path()
            .join(format!("catalog_probe{}", std::env::consts::EXE_SUFFIX));
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let compiled = std::process::Command::new(rustc)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("OUT_DIR", build.path())
            .args(["--edition", "2021", "--crate-name", "catalog_probe", "-o"])
            .arg(&binary)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let run = std::process::Command::new(&binary).output().unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
}
