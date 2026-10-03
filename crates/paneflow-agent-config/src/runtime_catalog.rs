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
    Screen,
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
    pub title_prefix: Option<&'static str>,
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
    pub bell_attention: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMcpConfig {
    Claude,
    Codex,
    Gemini,
    OpenCode,
    Fx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSkillsDir {
    Claude,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeIntegration {
    pub summary: &'static str,
    pub post_install_step: Option<&'static str>,
    pub hook_adapter: RuntimeHookAdapter,
    pub mcp_config: Option<RuntimeMcpConfig>,
    pub skills_dir: Option<RuntimeSkillsDir>,
}

pub const SESSION_ID_PLACEHOLDER: &str = "{session_id}";

#[derive(Debug, Clone, Copy)]
pub struct RuntimeResume {
    pub session_argv: &'static [&'static str],
    pub continue_argv: Option<&'static [&'static str]>,
    pub fork_argv: Option<&'static [&'static str]>,
    pub session_id_pattern: &'static str,
    pub failure_markers: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeSessionReader {
    Claude,
    Codex,
    OpenCode,
    Pi,
    Gemini,
    Kiro,
    Grok,
}

impl RuntimeSessionReader {
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
            Self::Gemini => "gemini",
            Self::Kiro => "kiro",
            Self::Grok => "grok",
        }
    }

    pub fn summarizes(self) -> bool {
        matches!(self, Self::Claude | Self::Codex | Self::OpenCode | Self::Pi)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeSessions {
    pub reader: RuntimeSessionReader,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeSuggestedPreset {
    pub id: &'static str,
    pub command: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct Runtime {
    pub id: &'static str,
    pub slug: &'static str,
    pub label: &'static str,
    pub platforms: &'static [RuntimePlatform],
    pub display: RuntimeDisplay,
    pub detection: RuntimeDetection,
    pub environment: RuntimeEnvironment,
    pub lifecycle: RuntimeLifecycle,
    pub integration: RuntimeIntegration,
    pub sessions: Option<RuntimeSessions>,
    pub suggested_presets: &'static [RuntimeSuggestedPreset],
}

include!(concat!(env!("OUT_DIR"), "/runtime_catalog.rs"));

pub fn current_platform() -> RuntimePlatform {
    if cfg!(target_os = "windows") {
        RuntimePlatform::Windows
    } else if cfg!(target_os = "macos") {
        RuntimePlatform::Macos
    } else {
        RuntimePlatform::Linux
    }
}

impl Runtime {
    pub fn supports_current_platform(&self) -> bool {
        self.supports(current_platform())
    }

    pub fn supports(&self, platform: RuntimePlatform) -> bool {
        self.platforms.contains(&platform)
    }

    pub fn title_confirms_identity(&self, title: Option<&str>) -> bool {
        self.detection
            .title_prefix
            .is_none_or(|prefix| title.is_some_and(|title| title.starts_with(prefix)))
    }

    pub fn has_launch_shim(&self) -> bool {
        self.detection.title_prefix.is_none()
    }
}

pub fn launcher_runtimes(
    explicit_visibility: impl Fn(&str) -> Option<bool>,
    installed: impl Fn(&Runtime) -> bool,
) -> Vec<&'static Runtime> {
    let mut runtimes = RUNTIMES
        .iter()
        .filter(|runtime| {
            explicit_visibility(runtime.display.visibility_config_key)
                .unwrap_or_else(|| installed(runtime))
        })
        .collect::<Vec<_>>();
    runtimes.sort_by_key(|runtime| runtime.display.order);
    runtimes
}

pub fn visibility_config_keys() -> impl Iterator<Item = &'static str> {
    RUNTIMES
        .iter()
        .map(|runtime| runtime.display.visibility_config_key)
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

pub fn runtime_for_tool(tool: &str) -> Option<&'static Runtime> {
    runtime_by_command_alias(tool)
        .or_else(|| runtime_by_slug(tool))
        .or_else(|| runtime_by_process_alias(tool))
        .or_else(|| runtime_by_id(tool))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_nineteen_complete_runtime_packages() {
        assert_eq!(RUNTIMES.len(), 19);
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
    fn generic_interpreters_are_not_runtimes() {
        for false_positive in ["node", "sh", "python"] {
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
    fn the_fx_alias_names_the_agent_only_when_its_title_confirms_it() {
        let fx = runtime_by_process_alias("fx").expect("fx runtime");
        assert_eq!(fx.slug, "fx");
        assert!(!fx.has_launch_shim());
        assert!(fx.title_confirms_identity(Some("fx v0.0.12 | paneflow")));
        assert!(!fx.title_confirms_identity(Some("fx data.json")));
        assert!(!fx.title_confirms_identity(Some("~/projects")));
        assert!(!fx.title_confirms_identity(None));
        for runtime in RUNTIMES {
            for alias in runtime
                .detection
                .command_aliases
                .iter()
                .chain(runtime.detection.process_aliases)
            {
                if *alias == "fx" {
                    assert!(runtime.detection.title_prefix.is_some(), "{}", runtime.slug);
                }
            }
        }
        let codex = runtime_by_slug("codex").expect("codex");
        assert!(codex.title_confirms_identity(None));
        assert!(codex.has_launch_shim());
    }

    #[test]
    fn fx_rings_for_attention_and_complete_hook_runtimes_do_not() {
        let fx = runtime_by_slug("fx").expect("fx");
        assert!(fx.lifecycle.bell_attention);
        assert!(!fx.supports(RuntimePlatform::Windows));
        assert!(fx.supports(RuntimePlatform::Linux) && fx.supports(RuntimePlatform::Macos));
        for runtime in RUNTIMES {
            assert_eq!(
                runtime.lifecycle.bell_attention,
                runtime.lifecycle.authority != RuntimeLifecycleAuthority::Complete,
                "{}",
                runtime.slug
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
        assert_eq!(codex.lifecycle.fallback, RuntimeLifecycleFallback::Screen);

        let gemini = runtime_by_slug("gemini").expect("gemini");
        assert!(gemini.lifecycle.escape_cancels_turn);
        assert_eq!(gemini.lifecycle.fallback, RuntimeLifecycleFallback::Screen);

        let grok = runtime_by_slug("grok").expect("grok");
        assert!(!grok.lifecycle.attention_clears_on_output);
        assert!(!grok.lifecycle.anchor_start_event_to_output);

        for slug in ["opencode", "pi", "hermes"] {
            let runtime = runtime_by_slug(slug).expect("runtime");
            assert_eq!(
                runtime.lifecycle.authority,
                RuntimeLifecycleAuthority::Screen
            );
            assert_eq!(runtime.lifecycle.fallback, RuntimeLifecycleFallback::Screen);
        }

        for slug in ["antigravity", "deepseek-harness"] {
            let runtime = runtime_by_slug(slug).expect("runtime");
            assert_eq!(runtime.lifecycle.authority, RuntimeLifecycleAuthority::None);
            assert_eq!(runtime.lifecycle.fallback, RuntimeLifecycleFallback::None);
        }
    }

    #[test]
    fn flat_command_alias_lookups_agree_with_the_runtime_table() {
        for runtime in RUNTIMES {
            for alias in runtime.detection.command_aliases {
                assert_eq!(command_alias_literal(alias), Some(*alias), "{alias}");
            }
        }
        for unknown in ["node", "sh", "python", "claude-code"] {
            assert_eq!(command_alias_literal(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn only_verified_clis_declare_resume_or_fork() {
        let resumable = RUNTIMES
            .iter()
            .filter(|runtime| runtime_resume(runtime.id).is_some())
            .map(|runtime| runtime.slug)
            .collect::<Vec<_>>();
        assert_eq!(
            resumable,
            vec![
                "claude-code",
                "codex",
                "opencode",
                "pi",
                "grok",
                "gemini",
                "kiro",
                "fx"
            ]
        );
        let fx = runtime_resume("sh.fx.cli").expect("fx resume");
        assert_eq!(fx.continue_argv, Some(&["fx", "--continue"][..]));
        assert!(runtime_by_slug("fx").is_some_and(|runtime| runtime.sessions.is_none()));
        let forkable = RUNTIMES
            .iter()
            .filter(|runtime| {
                runtime_resume(runtime.id).is_some_and(|resume| resume.fork_argv.is_some())
            })
            .map(|runtime| runtime.slug)
            .collect::<Vec<_>>();
        assert_eq!(forkable, vec!["claude-code", "codex"]);
        let claude = runtime_resume("com.anthropic.claude-code").expect("claude resume");
        assert_eq!(
            claude.fork_argv,
            Some(
                &[
                    "claude",
                    "--resume",
                    SESSION_ID_PLACEHOLDER,
                    "--fork-session"
                ][..]
            )
        );
        let codex = runtime_resume("com.openai.codex").expect("codex resume");
        assert_eq!(
            codex.fork_argv,
            Some(&["codex", "fork", SESSION_ID_PLACEHOLDER][..])
        );
    }

    #[test]
    fn captured_resume_failures_contain_a_declared_marker_and_normal_screens_do_not() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("runtimes");
        for slug in ["claude-code", "codex"] {
            let runtime = runtime_by_slug(slug).expect("runtime");
            let resume = runtime_resume(runtime.id).expect("resume");
            assert!(!resume.failure_markers.is_empty(), "{slug}");
            let fixtures = root.join(slug).join("fixtures");
            let failure = std::fs::read_to_string(fixtures.join("resume-failure.txt"))
                .expect("resume failure fixture");
            assert!(
                resume
                    .failure_markers
                    .iter()
                    .any(|marker| failure.contains(marker)),
                "{slug} resume failure fixture"
            );
            let mut normal_screens = 0;
            for entry in std::fs::read_dir(fixtures.join("screens")).expect("screen corpus") {
                let path = entry.expect("corpus entry").path();
                let screen = std::fs::read_to_string(&path).expect("capture");
                normal_screens += 1;
                assert!(
                    !resume
                        .failure_markers
                        .iter()
                        .any(|marker| screen.contains(marker)),
                    "{slug} {} false positive",
                    path.display()
                );
            }
            assert!(normal_screens >= 3, "{slug} corpus");
        }
    }
}
