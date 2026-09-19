#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimePlatform {
    Linux,
    Macos,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeLifecycleSource {
    Hooks,
    Output,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeLifecycleAuthority {
    Complete,
    Partial,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeLifecycleFallback {
    Screen,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeHookAdapter {
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

#[derive(Debug, Clone, Copy)]
pub struct RuntimeDisplay {
    pub order: u16,
    pub tint: Option<u32>,
    pub icon_asset_path: &'static str,
    pub icon_multicolor: bool,
    pub visibility_config_key: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeDetection {
    pub command_aliases: &'static [&'static str],
    pub process_aliases: &'static [&'static str],
    pub script_path_signatures: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeEnvironment {
    pub strip_inherited: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeLifecycle {
    pub source: RuntimeLifecycleSource,
    pub authority: RuntimeLifecycleAuthority,
    pub fallback: RuntimeLifecycleFallback,
    pub escape_cancels_turn: bool,
    pub attention_clears_on_output: bool,
    pub anchor_start_event_to_output: bool,
    pub terminal_title_signal: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeScreen {
    pub working: &'static [&'static str],
    pub idle_prompt: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeIntegration {
    pub summary: &'static str,
    pub post_install_step: Option<&'static str>,
    pub hook_adapter: RuntimeHookAdapter,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeInstall {
    pub command: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeSuggestedPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub command: &'static str,
    pub quick_launch: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Runtime {
    pub id: &'static str,
    pub slug: &'static str,
    pub label: &'static str,
    pub platforms: &'static [RuntimePlatform],
    pub capabilities: &'static [&'static str],
    pub display: RuntimeDisplay,
    pub detection: RuntimeDetection,
    pub environment: RuntimeEnvironment,
    pub lifecycle: RuntimeLifecycle,
    pub screen: Option<RuntimeScreen>,
    pub integration: RuntimeIntegration,
    pub install: RuntimeInstall,
    pub suggested_presets: &'static [RuntimeSuggestedPreset],
}

include!(concat!(env!("OUT_DIR"), "/runtime_catalog.rs"));

impl Runtime {
    pub fn supports_current_platform(&self) -> bool {
        let platform = if cfg!(target_os = "windows") {
            RuntimePlatform::Windows
        } else if cfg!(target_os = "macos") {
            RuntimePlatform::Macos
        } else {
            RuntimePlatform::Linux
        };
        self.platforms.contains(&platform)
    }
}

pub fn runtime_by_id(id: &str) -> Option<&'static Runtime> {
    RUNTIMES.iter().find(|runtime| runtime.id == id)
}

pub fn runtime_by_slug(slug: &str) -> Option<&'static Runtime> {
    RUNTIMES.iter().find(|runtime| runtime.slug == slug)
}

pub fn runtime_by_preset_id(id: &str) -> Option<&'static Runtime> {
    RUNTIMES.iter().find(|runtime| {
        runtime
            .suggested_presets
            .first()
            .is_some_and(|preset| preset.id == id)
    })
}

pub fn runtime_by_command_alias(alias: &str) -> Option<&'static Runtime> {
    RUNTIMES
        .iter()
        .find(|runtime| runtime.detection.command_aliases.contains(&alias))
}

pub fn runtime_by_process_alias(alias: &str) -> Option<&'static Runtime> {
    RUNTIMES
        .iter()
        .find(|runtime| runtime.detection.process_aliases.contains(&alias))
}

pub fn runtime_by_script_path(path: &str) -> Option<&'static Runtime> {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    RUNTIMES.iter().find(|runtime| {
        runtime
            .detection
            .script_path_signatures
            .iter()
            .any(|signature| normalized.contains(&signature.to_ascii_lowercase()))
    })
}

pub fn runtime_for_integration_install(slug: &str) -> Result<&'static Runtime, String> {
    let runtime = runtime_by_slug(slug).ok_or_else(|| format!("unknown runtime '{slug}'"))?;
    if !runtime.capabilities.contains(&"integration_install") {
        return Err(format!(
            "{} has no lifecycle integration capability",
            runtime.label
        ));
    }
    if !runtime.supports_current_platform() {
        return Err(format!(
            "{} integration is not supported on this platform",
            runtime.label
        ));
    }
    Ok(runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_eighteen_complete_runtime_packages() {
        assert_eq!(RUNTIMES.len(), 18);
        for runtime in RUNTIMES {
            assert!(
                !runtime.detection.command_aliases.is_empty(),
                "{}",
                runtime.slug
            );
            assert!(!runtime.suggested_presets.is_empty(), "{}", runtime.slug);
        }
    }

    #[test]
    fn generic_interpreters_and_the_fx_json_viewer_are_not_runtimes() {
        for false_positive in ["node", "sh", "python", "fx"] {
            assert!(
                runtime_by_command_alias(false_positive).is_none(),
                "{false_positive}"
            );
            assert!(
                runtime_by_process_alias(false_positive).is_none(),
                "{false_positive}"
            );
        }
    }

    #[test]
    fn script_signatures_identify_npm_claude_and_codex() {
        assert_eq!(
            runtime_by_script_path(r"C:\Users\a\node_modules\@anthropic-ai\claude-code\cli.js")
                .map(|runtime| runtime.slug),
            Some("claude-code")
        );
        assert_eq!(
            runtime_by_script_path("/usr/lib/node_modules/@openai/codex/bin/codex.js")
                .map(|runtime| runtime.slug),
            Some("codex")
        );
    }

    #[test]
    fn screen_fixtures_follow_the_catalog_rules() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("runtimes");
        for slug in ["claude-code", "codex", "gemini"] {
            let runtime = runtime_by_slug(slug).expect("fixture runtime");
            let screen = runtime.screen.expect("screen rules");
            let working = std::fs::read_to_string(root.join(slug).join("fixtures/working.txt"))
                .expect("working fixture");
            let idle = std::fs::read_to_string(root.join(slug).join("fixtures/idle.txt"))
                .expect("idle fixture");
            assert!(
                matches_any(&working, screen.working),
                "{slug} working fixture"
            );
            assert!(
                matches_any(&idle, screen.idle_prompt),
                "{slug} idle fixture"
            );
            assert!(
                !matches_any(&idle, screen.working),
                "{slug} idle false positive"
            );
        }
    }

    #[test]
    fn descriptor_policies_match_the_epic_contract() {
        let windows = RUNTIMES
            .iter()
            .filter(|runtime| runtime.platforms.contains(&RuntimePlatform::Windows))
            .map(|runtime| runtime.slug)
            .collect::<Vec<_>>();
        assert_eq!(
            windows,
            vec!["claude-code", "codex", "amp", "gemini", "github-copilot"]
        );

        let claude = runtime_by_slug("claude-code").expect("claude");
        assert_eq!(
            claude.environment.strip_inherited,
            &[
                "CLAUDECODE",
                "CLAUDE_CODE_CHILD_SESSION",
                "CLAUDE_CODE_ENTRYPOINT",
                "CLAUDE_CODE_SESSION_ID",
                "CLAUDE_PID",
            ]
        );
        assert!(claude.lifecycle.terminal_title_signal);
        assert_eq!(claude.lifecycle.fallback, RuntimeLifecycleFallback::Screen);

        let codex = runtime_by_slug("codex").expect("codex");
        assert_eq!(
            codex.environment.strip_inherited,
            &[
                "CODEX_CI",
                "CODEX_SHELL",
                "CODEX_THREAD_ID",
                "CODEX_TUI_RECORD_SESSION",
                "CODEX_TUI_SESSION_LOG_PATH",
            ]
        );
        assert!(codex.lifecycle.terminal_title_signal);
        assert_eq!(codex.lifecycle.fallback, RuntimeLifecycleFallback::Screen);

        let gemini = runtime_by_slug("gemini").expect("gemini");
        assert!(gemini.lifecycle.escape_cancels_turn);
        assert_eq!(gemini.lifecycle.fallback, RuntimeLifecycleFallback::Screen);

        let grok = runtime_by_slug("grok").expect("grok");
        assert!(!grok.lifecycle.attention_clears_on_output);
        assert!(!grok.lifecycle.anchor_start_event_to_output);

        for slug in ["pi", "antigravity", "deepseek-harness"] {
            let runtime = runtime_by_slug(slug).expect("runtime");
            assert_eq!(runtime.lifecycle.authority, RuntimeLifecycleAuthority::None);
            assert_eq!(runtime.lifecycle.fallback, RuntimeLifecycleFallback::None);
            let error = runtime_for_integration_install(slug).expect_err("missing capability");
            assert!(error.contains(runtime.label));
            assert!(error.contains("no lifecycle integration capability"));
        }
    }

    #[test]
    fn flat_command_alias_lookups_agree_with_the_runtime_table() {
        for runtime in RUNTIMES {
            for alias in runtime.detection.command_aliases {
                assert_eq!(command_alias_literal(alias), Some(*alias), "{alias}");
                assert_eq!(
                    hook_adapter_for_command_alias(alias),
                    Some(runtime.integration.hook_adapter),
                    "{alias}"
                );
                assert_eq!(
                    command_alias_supports_current_platform(alias),
                    runtime.supports_current_platform(),
                    "{alias}"
                );
            }
        }
        for unknown in ["node", "sh", "python", "fx", "claude-code"] {
            assert_eq!(command_alias_literal(unknown), None, "{unknown}");
            assert_eq!(hook_adapter_for_command_alias(unknown), None, "{unknown}");
            assert!(
                !command_alias_supports_current_platform(unknown),
                "{unknown}"
            );
        }
    }

    fn matches_any(viewport: &str, patterns: &[&str]) -> bool {
        let viewport = viewport.to_lowercase();
        patterns
            .iter()
            .any(|pattern| viewport.contains(&pattern.to_lowercase()))
    }
}
