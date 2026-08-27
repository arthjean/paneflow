// US-017: Config schema types

mod agent_panel;
mod config;
mod layout;
mod session;
mod terminal;

pub use agent_panel::*;
pub use config::*;
pub use layout::*;
pub use session::*;
pub use terminal::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    fn object_keys(value: &serde_json::Value) -> BTreeSet<String> {
        value
            .as_object()
            .expect("expected JSON object")
            .keys()
            .cloned()
            .collect()
    }

    fn key_set(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    fn assert_doc_mentions_property_keys(doc: &str, value: &serde_json::Value, context: &str) {
        for key in value
            .as_object()
            .expect("expected schema properties")
            .keys()
        {
            let needle = format!("`{key}`");
            let dotted = format!("`{context}.{key}`");
            assert!(
                doc.contains(&needle) || doc.contains(&dotted),
                "configuration docs do not mention public schema key {context}.{key}"
            );
        }
    }

    #[test]
    fn public_json_schema_covers_every_config_field() {
        let mut permissions = HashMap::new();
        permissions.insert(
            "read".to_string(),
            ToolPermissionsEntry {
                always_allow: vec!["src/".to_string()],
                always_deny: vec!["secrets/".to_string()],
            },
        );
        let mut profiles = HashMap::new();
        profiles.insert(
            "Write".to_string(),
            ProfileConfig {
                agent: Some("codex".to_string()),
                model: Some("default".to_string()),
                mode: Some("default".to_string()),
                effort: Some("medium".to_string()),
                tools: vec!["read".to_string()],
            },
        );

        // Deliberately exhaustive struct literals: adding a Rust config field
        // fails this test at compile time until the public schema is updated.
        let config = PaneFlowConfig {
            shortcuts: HashMap::new(),
            default_shell: Some("sh".to_string()),
            theme: Some("One Dark".to_string()),
            theme_mode: Some("dark".to_string()),
            commands: Vec::new(),
            window_decorations: Some("client".to_string()),
            window_backdrop: Some("auto".to_string()),
            windows_terminal_material: Some(true),
            windows_chrome_material: Some(true),
            macos_chrome_material: Some(true),
            unfocused_pane_opacity: Some(0.7),
            reduce_motion: Some(false),
            line_height: Some(1.2),
            cell_width: Some(0.6),
            font_family: Some("Geist Mono".to_string()),
            font_fallbacks: Some(vec!["FiraCode Nerd Font Mono".to_string()]),
            font_size: Some(13.0),
            font_weight: Some("normal".to_string()),
            option_as_meta: Some(true),
            shell_integration: Some(true),
            agent_stall_detection: Some(true),
            agent_stall_threshold_secs: Some(300),
            review_prefill_delay_ms: Some(2000),
            submit_paste_delay_ms: Some(70),
            external_editor: Some("auto".to_string()),
            claude_code_bypass_permissions: Some(false),
            ai_unrestricted: Some(true),
            ai_injection_fence: Some(false),
            claude_code_button_visible: Some(true),
            codex_button_visible: Some(true),
            opencode_button_visible: Some(true),
            pi_button_visible: Some(true),
            hermes_agent_button_visible: Some(true),
            grok_button_visible: Some(true),
            amp_button_visible: Some(true),
            cursor_button_visible: Some(true),
            gemini_button_visible: Some(true),
            kiro_button_visible: Some(true),
            antigravity_button_visible: Some(true),
            copilot_button_visible: Some(true),
            codebuddy_button_visible: Some(true),
            factory_button_visible: Some(true),
            qoder_button_visible: Some(true),
            openclaw_button_visible: Some(true),
            telemetry: Some(TelemetryConfig {
                enabled: Some(false),
            }),
            terminal: Some(TerminalConfig {
                backend: TerminalBackendConfig::Auto,
                ligatures: Some(false),
                integrated_glyphs: Some(true),
                color_emoji: Some(true),
                cursor_color: Some(APPLE_SYSTEM_BLUE_HEX.to_string()),
                scrollback_lines: Some(10_000),
                cursor_shape: Some(CursorShapeConfig::Block),
                cursor_blink: Some(CursorBlinkConfig::TerminalControlled),
                env: Some(HashMap::new()),
                scroll_multiplier: Some(1.0),
            }),
            agent_panel: Some(AgentPanelConfig {
                max_content_width: Some(760),
                thinking_display: Some(ThinkingDisplayMode::Auto),
                profiles,
                default_profile: Some("Write".to_string()),
                notify_when_agent_waiting: Some(NotifyWhenAgentWaiting::PrimaryScreen),
            }),
            tool_permissions: permissions,
        };

        let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../schemas/paneflow.schema.json");
        let schema: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(schema_path).unwrap()).unwrap();
        let serialized = serde_json::to_value(&config).unwrap();
        let loaded = crate::loader::try_parse_and_validate(&serialized.to_string()).unwrap();
        assert_eq!(
            loaded, config,
            "every serialized PaneFlowConfig field must be owned by the canonical deserializer"
        );
        let mut schema_top_level = object_keys(&schema["properties"]);
        schema_top_level.remove("$schema");
        schema_top_level.remove("$schemaVersion");

        assert_eq!(
            object_keys(&serialized),
            schema_top_level,
            "top-level PaneFlowConfig and public JSON Schema drifted"
        );
        assert_eq!(
            object_keys(&serialized["terminal"]),
            object_keys(&schema["properties"]["terminal"]["properties"]),
            "TerminalConfig and public JSON Schema drifted"
        );
        assert_eq!(
            object_keys(&serialized["agent_panel"]),
            object_keys(&schema["properties"]["agent_panel"]["properties"]),
            "AgentPanelConfig and public JSON Schema drifted"
        );
        assert_eq!(
            object_keys(&serialized["agent_panel"]["profiles"]["Write"]),
            object_keys(&schema["definitions"]["profileConfig"]["properties"]),
            "ProfileConfig and public JSON Schema drifted"
        );
        assert_eq!(
            object_keys(&serialized["tool_permissions"]["read"]),
            object_keys(&schema["definitions"]["toolPermissionsEntry"]["properties"]),
            "ToolPermissionsEntry and public JSON Schema drifted"
        );

        let command = CommandDefinition {
            name: "Dev".to_string(),
            description: Some("Open dev workspace".to_string()),
            keywords: vec!["dev".to_string()],
            target: CommandTarget::Workspace {
                workspace: WorkspaceDefinition {
                    name: Some("Dev".to_string()),
                    cwd: Some("~/dev".to_string()),
                    layout_preset: Some("even_h".to_string()),
                    color: Some("007aff".to_string()),
                    layout: Some(LayoutNode::Pane {
                        surfaces: vec![SurfaceDefinition {
                            surface_type: Some("terminal".to_string()),
                            name: Some("Claude".to_string()),
                            custom_name: Some("Agent".to_string()),
                            command: Some("claude".to_string()),
                            prompt: Some("Review this".to_string()),
                            cwd: Some("~/dev/app".to_string()),
                            path: None,
                            env: Some(HashMap::new()),
                            focus: Some(true),
                            scrollback: Some("previous output".to_string()),
                            agent: Some("claude_code".to_string()),
                            font_size: Some(13.0),
                        }],
                    }),
                },
            },
        };
        let serialized_command = serde_json::to_value(command).unwrap();
        assert_eq!(
            object_keys(&serialized_command),
            object_keys(&schema["definitions"]["commandDefinition"]["properties"]),
            "CommandDefinition and public JSON Schema drifted"
        );
        let command_schema = &schema["definitions"]["commandDefinition"];
        assert_eq!(command_schema["properties"]["name"]["pattern"], "\\S");
        assert_eq!(command_schema["oneOf"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            command_schema["oneOf"][0]["required"],
            serde_json::json!(["workspace"])
        );
        assert_eq!(
            command_schema["oneOf"][0]["properties"]["command"]["type"],
            "null"
        );
        assert_eq!(
            command_schema["oneOf"][1]["required"],
            serde_json::json!(["command"])
        );
        assert_eq!(
            command_schema["oneOf"][1]["properties"]["workspace"]["type"],
            "null"
        );
        assert_eq!(
            command_schema["oneOf"][1]["properties"]["command"]["pattern"],
            "\\S"
        );
        assert_eq!(
            object_keys(&serialized_command["workspace"]),
            object_keys(&schema["definitions"]["workspaceDefinition"]["properties"]),
            "WorkspaceDefinition and public JSON Schema drifted"
        );
        assert_eq!(
            key_set(&["type", "surfaces"]),
            object_keys(&schema["definitions"]["layoutNode"]["oneOf"][0]["properties"]),
            "Pane layout node and public JSON Schema drifted"
        );
        assert_eq!(
            key_set(&["type", "direction", "ratio", "ratios", "children"]),
            object_keys(&schema["definitions"]["layoutNode"]["oneOf"][1]["properties"]),
            "Split layout node and public JSON Schema drifted"
        );
        assert_eq!(
            object_keys(&serialized_command["workspace"]["layout"]["surfaces"][0]),
            object_keys(&schema["definitions"]["surface"]["properties"]),
            "SurfaceDefinition and public JSON Schema drifted"
        );
    }

    #[test]
    fn public_configuration_schema_doc_mentions_schema_keys() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let schema_path = root.join("schemas/paneflow.schema.json");
        let doc_path = root.join("docs/user/configuration/schema.md");
        let schema: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(schema_path).unwrap()).unwrap();
        let doc = std::fs::read_to_string(doc_path).unwrap();

        assert_doc_mentions_property_keys(&doc, &schema["properties"], "top-level");
        assert_doc_mentions_property_keys(
            &doc,
            &schema["properties"]["terminal"]["properties"],
            "terminal",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["properties"]["agent_panel"]["properties"],
            "agent_panel",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["profileConfig"]["properties"],
            "profileConfig",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["toolPermissionsEntry"]["properties"],
            "toolPermissionsEntry",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["commandDefinition"]["properties"],
            "commandDefinition",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["workspaceDefinition"]["properties"],
            "workspaceDefinition",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["layoutNode"]["oneOf"][0]["properties"],
            "paneLayoutNode",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["layoutNode"]["oneOf"][1]["properties"],
            "splitLayoutNode",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["surface"]["properties"],
            "surface",
        );

        assert!(
            doc.contains("| `font_size` | number or null | `13.0` |"),
            "configuration docs must publish the runtime font_size default"
        );
        assert!(
            doc.contains("| `line_height` | number or null | `1.2` |"),
            "configuration docs must publish the runtime line_height default"
        );
        assert!(
            doc.contains("Windows: configured -> `pwsh.exe` -> `powershell.exe`"),
            "configuration docs must describe the Windows shell fallback chain"
        );
    }

    #[test]
    fn terminal_scrollback_profiles_resolve_defaults_and_caps() {
        let cfg = TerminalConfig::default();
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Normal),
            10_000
        );
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Agent),
            10_000
        );
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Review),
            2_000
        );
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Cached),
            1_000
        );

        let cfg = TerminalConfig {
            scrollback_lines: Some(50_000),
            ..Default::default()
        };
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Normal),
            50_000
        );
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Agent),
            10_000
        );
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Review),
            2_000
        );

        let cfg = TerminalConfig {
            scrollback_lines: Some(500),
            ..Default::default()
        };
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Agent),
            500
        );
    }

    #[test]
    fn agent_stall_settings_resolve_with_defaults_and_clamp() {
        // EP-004 US-011 + US-013: default ON, threshold 60 s (tightened from
        // 300 s so a lost ai.stop surfaces in seconds, not minutes).
        let cfg = PaneFlowConfig::default();
        assert!(cfg.agent_stall_detection_enabled());
        assert_eq!(cfg.resolved_agent_stall_threshold_secs(), 60);

        // Kill switch.
        let cfg = PaneFlowConfig {
            agent_stall_detection: Some(false),
            ..Default::default()
        };
        assert!(!cfg.agent_stall_detection_enabled());

        // Clamp both ends.
        let cfg = PaneFlowConfig {
            agent_stall_threshold_secs: Some(1),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_agent_stall_threshold_secs(), 30);
        let cfg = PaneFlowConfig {
            agent_stall_threshold_secs: Some(u64::MAX),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_agent_stall_threshold_secs(), 86_400);
        let cfg = PaneFlowConfig {
            agent_stall_threshold_secs: Some(600),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_agent_stall_threshold_secs(), 600);
    }

    #[test]
    fn cockpit_chrome_material_respects_current_platform_switch() {
        let cfg = PaneFlowConfig::default();
        assert_eq!(
            cfg.cockpit_chrome_material_enabled(),
            !cfg!(target_os = "windows")
        );

        let cfg = PaneFlowConfig {
            windows_chrome_material: Some(true),
            ..Default::default()
        };
        assert!(cfg.cockpit_chrome_material_enabled());

        let cfg = PaneFlowConfig {
            windows_chrome_material: Some(false),
            ..Default::default()
        };
        assert_eq!(
            cfg.cockpit_chrome_material_enabled(),
            !cfg!(target_os = "windows")
        );

        let cfg = PaneFlowConfig {
            window_backdrop: Some("opaque".to_string()),
            windows_chrome_material: Some(true),
            ..Default::default()
        };
        assert!(!cfg.cockpit_chrome_material_enabled());
    }

    #[test]
    fn macos_chrome_material_defaults_on_and_respects_switches() {
        assert!(PaneFlowConfig::default().macos_chrome_material_enabled());

        let disabled = PaneFlowConfig {
            macos_chrome_material: Some(false),
            ..Default::default()
        };
        assert!(!disabled.macos_chrome_material_enabled());

        let globally_opaque = PaneFlowConfig {
            window_backdrop: Some("opaque".to_string()),
            macos_chrome_material: Some(true),
            ..Default::default()
        };
        assert!(!globally_opaque.macos_chrome_material_enabled());

        let raw_transparent = PaneFlowConfig {
            window_backdrop: Some("transparent".to_string()),
            macos_chrome_material: Some(true),
            ..Default::default()
        };
        assert!(!raw_transparent.macos_chrome_material_enabled());
    }

    #[test]
    fn unfocused_pane_dim_alpha_inverts_clamps_and_disables() {
        // The accessor is the single point of inversion: opacity -> fill alpha.
        assert!(
            (PaneFlowConfig::default().resolved_unfocused_pane_dim_alpha() - 0.3).abs() < 1e-6,
            "default 0.7 opacity must paint a 0.3 overlay"
        );
        // 1.0 is the off switch: no layer at all.
        let cfg = PaneFlowConfig {
            unfocused_pane_opacity: Some(1.0),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_unfocused_pane_dim_alpha(), 0.0);
        // Below the floor clamps up to 0.15 opacity -> 0.85 overlay.
        let cfg = PaneFlowConfig {
            unfocused_pane_opacity: Some(-4.0),
            ..Default::default()
        };
        assert!((cfg.resolved_unfocused_pane_dim_alpha() - 0.85).abs() < 1e-6);
        // Above the ceiling clamps down to the off switch.
        let cfg = PaneFlowConfig {
            unfocused_pane_opacity: Some(12.0),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_unfocused_pane_dim_alpha(), 0.0);
        // NaN falls back to the default instead of poisoning the layer alpha.
        let cfg = PaneFlowConfig {
            unfocused_pane_opacity: Some(f32::NAN),
            ..Default::default()
        };
        assert!((cfg.resolved_unfocused_pane_dim_alpha() - 0.3).abs() < 1e-6);
    }

    #[test]
    fn submit_paste_delay_resolves_with_default_and_clamp() {
        // EP-001 US-001 (agent-control-plane-hardening): default 70 ms,
        // clamped to [10, 5000].
        assert_eq!(
            PaneFlowConfig::default().resolved_submit_paste_delay_ms(),
            70
        );
        // Below the floor clamps up.
        let cfg = PaneFlowConfig {
            submit_paste_delay_ms: Some(0),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_submit_paste_delay_ms(), 10);
        // Above the ceiling clamps down.
        let cfg = PaneFlowConfig {
            submit_paste_delay_ms: Some(u64::MAX),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_submit_paste_delay_ms(), 5_000);
        // In-range passes through untouched.
        let cfg = PaneFlowConfig {
            submit_paste_delay_ms: Some(120),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_submit_paste_delay_ms(), 120);
    }

    #[test]
    fn submit_paste_delay_serde_roundtrips() {
        // The knob travels through the public JSON shape unchanged
        // (`#[serde(default)]` on the struct fills every other field).
        let cfg: PaneFlowConfig =
            serde_json::from_str(r#"{"submit_paste_delay_ms": 90}"#).expect("valid config");
        assert_eq!(cfg.submit_paste_delay_ms, Some(90));
        assert_eq!(cfg.resolved_submit_paste_delay_ms(), 90);
        // Absent -> None -> default.
        let cfg: PaneFlowConfig = serde_json::from_str("{}").expect("empty config");
        assert!(cfg.submit_paste_delay_ms.is_none());
        assert_eq!(cfg.resolved_submit_paste_delay_ms(), 70);
    }

    #[test]
    fn ai_access_toggles_default_safe_and_tolerate_garbage() {
        // EP-003 US-008 AC #1/#5: a fresh config never opens free-access and
        // always fences.
        let cfg = PaneFlowConfig::default();
        assert!(!cfg.ai_unrestricted_enabled(), "unrestricted defaults OFF");
        assert!(cfg.ai_injection_fence_enabled(), "fence defaults ON");

        // Explicit booleans round-trip through the lenient deserializer.
        let cfg: PaneFlowConfig =
            serde_json::from_str(r#"{"ai_unrestricted": true, "ai_injection_fence": false}"#)
                .unwrap();
        assert!(cfg.ai_unrestricted_enabled());
        assert!(!cfg.ai_injection_fence_enabled());

        // AC #3: a non-boolean value fails CLOSED (unrestricted -> false, fence
        // -> true) instead of erroring the whole parse, and does NOT wipe the
        // sibling settings the all-or-nothing loader fallback would have lost.
        let cfg: PaneFlowConfig = serde_json::from_str(
            r#"{"theme": "One Dark", "ai_unrestricted": "yes", "ai_injection_fence": 0}"#,
        )
        .unwrap();
        assert!(
            !cfg.ai_unrestricted_enabled(),
            "a garbage value must never open the mode"
        );
        assert!(
            cfg.ai_injection_fence_enabled(),
            "a garbage value must never drop the fence"
        );
        assert_eq!(
            cfg.theme.as_deref(),
            Some("One Dark"),
            "siblings survive a malformed AI-access toggle"
        );
    }

    #[test]
    fn agent_panel_notifications_are_opt_in_by_default() {
        let cfg: AgentPanelConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert!(cfg.notify_when_agent_waiting.is_none());
        assert_eq!(
            cfg.resolved_notify_when_agent_waiting(),
            NotifyWhenAgentWaiting::Never
        );

        let cfg: AgentPanelConfig =
            serde_json::from_str(r#"{"notify_when_agent_waiting": "Bogus"}"#).unwrap();
        assert_eq!(
            cfg.resolved_notify_when_agent_waiting(),
            NotifyWhenAgentWaiting::Never
        );

        let cfg: AgentPanelConfig =
            serde_json::from_str(r#"{"notify_when_agent_waiting": "PrimaryScreen"}"#).unwrap();
        assert_eq!(
            cfg.resolved_notify_when_agent_waiting(),
            NotifyWhenAgentWaiting::PrimaryScreen
        );
    }

    #[test]
    fn agent_panel_thinking_display_pascal_case_roundtrip() {
        // US-109 AC #1: PascalCase tags as documented in the PRD.
        let raw = r#"{"thinking_display": "Preview"}"#;
        let cfg: AgentPanelConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.thinking_display, Some(ThinkingDisplayMode::Preview));

        let raw = r#"{"thinking_display": "AlwaysExpanded"}"#;
        let cfg: AgentPanelConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(
            cfg.thinking_display,
            Some(ThinkingDisplayMode::AlwaysExpanded)
        );

        let raw = r#"{"thinking_display": "AlwaysCollapsed"}"#;
        let cfg: AgentPanelConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(
            cfg.thinking_display,
            Some(ThinkingDisplayMode::AlwaysCollapsed)
        );

        let raw = r#"{"thinking_display": "Auto"}"#;
        let cfg: AgentPanelConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.thinking_display, Some(ThinkingDisplayMode::Auto));
    }

    #[test]
    fn agent_panel_thinking_display_unknown_falls_back_to_auto() {
        // US-109 AC #7: unknown string deserialises as Auto (the
        // custom deserialiser logs a warn! line; this test asserts
        // only the surface behavior since `warn!` is not captured).
        let raw = r#"{"thinking_display": "Bogus"}"#;
        let cfg: AgentPanelConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.thinking_display, Some(ThinkingDisplayMode::Auto));
    }

    #[test]
    fn agent_panel_thinking_display_missing_resolves_to_auto() {
        // US-109 AC #1: missing field resolves to Auto via the
        // resolver (the on-disk Option stays `None`).
        let raw = r#"{}"#;
        let cfg: AgentPanelConfig = serde_json::from_str(raw).unwrap();
        assert!(cfg.thinking_display.is_none());
        assert_eq!(cfg.resolved_thinking_display(), ThinkingDisplayMode::Auto);
    }

    #[test]
    fn cursor_shape_and_blink_config_serde() {
        // US-007 / US-008: snake_case config values + historical defaults.
        assert_eq!(CursorShapeConfig::default(), CursorShapeConfig::Block);
        assert_eq!(
            CursorBlinkConfig::default(),
            CursorBlinkConfig::TerminalControlled
        );

        let cfg: TerminalConfig =
            serde_json::from_str(r#"{"cursor_shape": "beam", "cursor_blink": "off"}"#).unwrap();
        assert_eq!(cfg.cursor_shape, Some(CursorShapeConfig::Beam));
        assert_eq!(cfg.cursor_blink, Some(CursorBlinkConfig::Off));

        let cfg: TerminalConfig = serde_json::from_str(r#"{"cursor_shape": "hollow"}"#).unwrap();
        assert_eq!(cfg.cursor_shape, Some(CursorShapeConfig::Hollow));

        let cfg: TerminalConfig = serde_json::from_str(r#"{"cursor_shape": "vintage"}"#).unwrap();
        assert_eq!(cfg.cursor_shape, Some(CursorShapeConfig::Vintage));

        let cfg: TerminalConfig =
            serde_json::from_str(r#"{"cursor_shape": "double_underline"}"#).unwrap();
        assert_eq!(cfg.cursor_shape, Some(CursorShapeConfig::DoubleUnderline));

        let cfg: TerminalConfig =
            serde_json::from_str(r#"{"cursor_shape": "filled_box"}"#).unwrap();
        assert_eq!(cfg.cursor_shape, Some(CursorShapeConfig::Block));

        // Missing → None → resolves to historical defaults.
        let cfg: TerminalConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert!(cfg.cursor_shape.is_none() && cfg.cursor_blink.is_none());
        assert_eq!(
            cfg.cursor_shape.unwrap_or_default(),
            CursorShapeConfig::Block
        );
        assert_eq!(
            cfg.cursor_blink.unwrap_or_default(),
            CursorBlinkConfig::TerminalControlled
        );
    }

    #[test]
    fn terminal_backend_serializes_and_fails_safe_on_unknown_values() {
        let automatic: TerminalConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(automatic.backend, TerminalBackendConfig::Auto);

        let ghostty: TerminalConfig = serde_json::from_str(r#"{"backend":"ghostty"}"#).unwrap();
        assert_eq!(ghostty.backend, TerminalBackendConfig::Ghostty);

        let alacritty: TerminalConfig = serde_json::from_str(r#"{"backend":"alacritty"}"#).unwrap();
        assert_eq!(alacritty.backend, TerminalBackendConfig::Alacritty);

        assert_eq!(
            serde_json::to_string(&TerminalBackendConfig::Auto).unwrap(),
            r#""auto""#
        );
        assert_eq!(
            serde_json::to_string(&TerminalBackendConfig::Ghostty).unwrap(),
            r#""ghostty""#
        );
        assert_eq!(
            serde_json::to_string(&TerminalBackendConfig::Alacritty).unwrap(),
            r#""alacritty""#
        );

        let unknown: TerminalConfig =
            serde_json::from_str(r#"{"backend":"future-engine"}"#).unwrap();
        assert_eq!(unknown.backend, TerminalBackendConfig::Alacritty);

        let legacy: PaneFlowConfig =
            serde_json::from_str(r#"{"theme":"One Dark","terminal":{"scrollback_lines":4321}}"#)
                .unwrap();
        let terminal = legacy.terminal.expect("legacy terminal block");
        assert_eq!(legacy.theme.as_deref(), Some("One Dark"));
        assert_eq!(terminal.backend, TerminalBackendConfig::Auto);
        assert_eq!(terminal.scrollback_lines, Some(4321));

        // US-017: a backend typo must cost the backend setting and nothing
        // else. A derived `Deserialize` would abort the whole document and
        // `parse_and_validate` would hand back defaults, silently wiping the
        // theme, shell, shortcuts, and agent settings.
        let typo: PaneFlowConfig = serde_json::from_str(
            r#"{
                "theme": "One Dark",
                "default_shell": "/bin/zsh",
                "shortcuts": {"split_right": "cmd-d"},
                "agent_panel": {"default_profile": "Write"},
                "terminal": {"backend": "gostty", "scrollback_lines": 4321}
            }"#,
        )
        .expect("US-017: an unknown backend value must not fail the document");
        let terminal = typo.terminal.expect("terminal block");
        assert_eq!(terminal.backend, TerminalBackendConfig::Alacritty);
        assert_eq!(terminal.scrollback_lines, Some(4321));
        assert_eq!(typo.theme.as_deref(), Some("One Dark"));
        assert_eq!(typo.default_shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(
            typo.shortcuts.get("split_right").map(String::as_str),
            Some("cmd-d")
        );
        assert_eq!(
            typo.agent_panel
                .expect("agent_panel block")
                .default_profile
                .as_deref(),
            Some("Write")
        );
    }

    #[test]
    fn cursor_color_hex_normalizes_and_defaults_to_theme_when_absent() {
        let cfg: TerminalConfig = serde_json::from_str(r##"{"cursor_color": "#0a84ff"}"##).unwrap();
        assert_eq!(cfg.normalized_cursor_color().as_deref(), Some("#0A84FF"));

        let cfg: TerminalConfig = serde_json::from_str(r#"{"cursor_color": "abc"}"#).unwrap();
        assert_eq!(cfg.normalized_cursor_color().as_deref(), Some("#AABBCC"));

        let cfg: TerminalConfig =
            serde_json::from_str(r#"{"cursor_color": "not-a-color"}"#).unwrap();
        assert!(cfg.normalized_cursor_color().is_none());

        let cfg: TerminalConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert!(cfg.cursor_color.is_none());
        assert!(cfg.normalized_cursor_color().is_none());
    }
}
