#![cfg_attr(test, allow(dead_code))]

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
    pub screen: Option<Screen>,
    pub integration: Integration,
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
}

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
pub struct Screen {
    pub working: Vec<String>,
    pub idle_prompt: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Integration {
    pub summary: String,
    pub post_install_step: Option<String>,
    pub hook_adapter: HookAdapter,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookAdapter {
    Claude,
    Codex,
    Codebuddy,
    Qoder,
    Gemini,
    Cursor,
    Opencode,
    Hermes,
    Grok,
    Muse,
    Pi,
    Dsh,
    None,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestedPreset {
    pub id: String,
    pub command: String,
}

#[derive(Debug, Clone)]
pub struct LocatedDescriptor {
    pub path: PathBuf,
    pub descriptor: Descriptor,
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
        descriptors.push(LocatedDescriptor { path, descriptor });
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
        if runtime.lifecycle.fallback == LifecycleFallback::Screen && runtime.screen.is_none() {
            errors.push(format!(
                "{}: lifecycle.fallback = 'screen' requires [screen]",
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
        if runtime.lifecycle.source == LifecycleSource::Hooks
            && matches!(runtime.integration.hook_adapter, HookAdapter::None)
        {
            errors.push(format!(
                "{}: lifecycle.source = 'hooks' requires integration.hook_adapter",
                path.display()
            ));
        }
        if let Some(screen) = &runtime.screen {
            if screen.working.is_empty() || screen.idle_prompt.is_empty() {
                errors.push(format!(
                    "{}: [screen] requires working and idle_prompt patterns",
                    path.display()
                ));
            }
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
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
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
            "detection: RuntimeDetection {{ command_aliases: &{}, process_aliases: &{}, script_path_signatures: &{} }},\n",
            strings(&runtime.detection.command_aliases),
            strings(&runtime.detection.process_aliases),
            strings(&runtime.detection.script_path_signatures)
        ));
        output.push_str(&format!(
            "environment: RuntimeEnvironment {{ strip_inherited: &{} }},\n",
            strings(&runtime.environment.strip_inherited)
        ));
        output.push_str(&format!(
            "lifecycle: RuntimeLifecycle {{ source: RuntimeLifecycleSource::{}, authority: RuntimeLifecycleAuthority::{}, fallback: RuntimeLifecycleFallback::{}, escape_cancels_turn: {}, attention_clears_on_output: {}, anchor_start_event_to_output: {} }},\n",
            source(runtime.lifecycle.source),
            authority(runtime.lifecycle.authority),
            fallback(runtime.lifecycle.fallback),
            runtime.lifecycle.escape_cancels_turn,
            runtime.lifecycle.attention_clears_on_output,
            runtime.lifecycle.anchor_start_event_to_output
        ));
        match &runtime.screen {
            Some(screen) => output.push_str(&format!(
                "screen: Some(RuntimeScreen {{ working: &{}, idle_prompt: &{} }}),\n",
                strings(&screen.working),
                strings(&screen.idle_prompt)
            )),
            None => output.push_str("screen: None,\n"),
        }
        output.push_str(&format!(
            "integration: RuntimeIntegration {{ summary: {:?}, post_install_step: {}, hook_adapter: RuntimeHookAdapter::{} }},\n",
            runtime.integration.summary,
            option_string(runtime.integration.post_install_step.as_deref()),
            hook_adapter(runtime.integration.hook_adapter)
        ));
        output.push_str("suggested_presets: &[\n");
        for preset in &runtime.suggested_presets {
            output.push_str(&format!(
                "RuntimeSuggestedPreset {{ id: {:?}, command: {:?} }},\n",
                preset.id, preset.command
            ));
        }
        output.push_str("] },\n");
    }
    output.push_str("];\n");
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
                "if path.contains({signature:?}) || path.contains({windows:?}) {{ return Some({:?}); }}\n",
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
        "pub fn hook_adapter_for_command_alias(alias: &str) -> Option<RuntimeHookAdapter> {\nmatch alias {\n",
    );
    for located in descriptors {
        let runtime = &located.descriptor;
        output.push_str(&format!(
            "{} => Some(RuntimeHookAdapter::{}),\n",
            alias_pattern(&runtime.detection.command_aliases),
            hook_adapter(runtime.integration.hook_adapter)
        ));
    }
    output.push_str("_ => None,\n}\n}\n");
    output.push_str(
        "pub fn command_alias_supports_current_platform(alias: &str) -> bool {\nmatch alias {\n",
    );
    for located in descriptors {
        let runtime = &located.descriptor;
        output.push_str(&format!(
            "{} => {},\n",
            alias_pattern(&runtime.detection.command_aliases),
            platform_guard(&runtime.platforms)
        ));
    }
    output.push_str("_ => false,\n}\n}\n");
    Ok(output)
}

fn strings(values: &[String]) -> String {
    let values = values
        .iter()
        .map(|value| format!("{value:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{values}]")
}

fn alias_pattern(aliases: &[String]) -> String {
    aliases
        .iter()
        .map(|alias| format!("{alias:?}"))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn platform_guard(values: &[Platform]) -> String {
    values
        .iter()
        .map(|value| match value {
            Platform::Linux => "cfg!(target_os = \"linux\")",
            Platform::Macos => "cfg!(target_os = \"macos\")",
            Platform::Windows => "cfg!(target_os = \"windows\")",
        })
        .collect::<Vec<_>>()
        .join(" || ")
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
        HookAdapter::Codebuddy => "Codebuddy",
        HookAdapter::Qoder => "Qoder",
        HookAdapter::Gemini => "Gemini",
        HookAdapter::Cursor => "Cursor",
        HookAdapter::Opencode => "Opencode",
        HookAdapter::Hermes => "Hermes",
        HookAdapter::Grok => "Grok",
        HookAdapter::Muse => "Muse",
        HookAdapter::Pi => "Pi",
        HookAdapter::Dsh => "Dsh",
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
visibility_config_key = "alpha_visible"

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
    fn rejects_screen_and_authority_inconsistencies() {
        let temp = tempfile::TempDir::new().unwrap();
        let text = descriptor("alpha", "com.example.alpha", "alpha")
            .replace("fallback = \"none\"", "fallback = \"screen\"");
        write_runtime(temp.path(), "alpha", &text);
        let error = discover_and_validate(temp.path()).unwrap_err();
        assert!(error.contains("requires [screen]"));
        assert!(error.contains("authority = 'none'"));
    }
}
