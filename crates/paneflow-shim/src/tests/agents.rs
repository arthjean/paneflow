#[cfg(unix)]
use crate::hooks::{
    enable_codex_feature_flag, CodexHookConfigGuard, CODEX_HOOK_EVENTS, CODEX_TOML_MARKER,
};
use crate::hooks::{
    hermes_managed_block, is_paneflow_hook_command, merge_codebuddy_hooks, merge_cursor_hooks,
    merge_gemini_hooks, merge_qoder_hooks, remove_cursor_hooks, remove_gemini_hooks,
    remove_paneflow_hooks, remove_qoder_hooks, resolve_hook_command, strip_hermes_managed_block,
    GrokHookFileGuard, HermesHookConfigGuard, InvalidJsonPolicy, ManagedHookConfigGuard,
    ManagedHookSpec, OpenCodePluginGuard, PiExtensionGuard, CLAUDE_HOOK_EVENTS, HERMES_BLOCK_BEGIN,
    PANEFLOW_TS_BASENAME,
};
use serde_json::json;

fn command_preserves_event_arg(command: &str, event: &str) -> bool {
    command
        .split_whitespace()
        .any(|token| token.trim_matches(['"', '\\', '\'']) == event)
}

#[test]
fn qoder_merge_skips_notification_event() {
    let mut root = json!({});
    merge_qoder_hooks(&mut root).unwrap();
    let hooks = root["hooks"].as_object().unwrap();
    assert!(hooks.contains_key("UserPromptSubmit"));
    assert!(hooks.contains_key("Stop"));
    assert!(
        !hooks.contains_key("Notification"),
        "Notification must not be registered for Qoder"
    );
    let group = &root["hooks"]["UserPromptSubmit"][0];
    assert!(
        group.get("_paneflow_managed").is_none(),
        "Qoder public schema does not document Paneflow-only markers"
    );
    assert!(
        group["hooks"][0].get("commandWindows").is_none(),
        "Qoder public schema does not document commandWindows"
    );
    remove_qoder_hooks(&mut root);
    assert_eq!(root, json!({}));
}

#[test]
fn gemini_nested_merge_writes_official_shape_and_roundtrips() {
    let mut root = json!({});
    merge_gemini_hooks(&mut root).unwrap();
    let before_agent = root["hooks"]["BeforeAgent"].as_array().unwrap();
    assert_eq!(before_agent.len(), 1);
    let group = &before_agent[0];
    assert_eq!(group["matcher"], json!("*"));
    let inner = group["hooks"].as_array().unwrap();
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0]["name"], json!("paneflow-status"));
    assert_eq!(inner[0]["type"], json!("command"));
    assert_eq!(
        inner[0]["timeout"],
        json!(5000),
        "Gemini hook timeout is milliseconds"
    );
    let cmd = inner[0]["command"].as_str().unwrap();
    assert!(
        command_preserves_event_arg(cmd, "UserPromptSubmit"),
        "BeforeAgent must invoke the canonical UserPromptSubmit: {cmd}"
    );
    assert!(group.get("_paneflow_managed").is_none());
    assert!(group.get("command").is_none());
    merge_gemini_hooks(&mut root).unwrap();
    assert_eq!(root["hooks"]["BeforeAgent"].as_array().unwrap().len(), 1);
    remove_gemini_hooks(&mut root);
    assert_eq!(root, json!({}));
}

#[test]
fn cursor_flat_merge_stamps_version_and_preserves_user_entries() {
    let mut root = json!({
        "hooks": {
            "preToolUse": [ { "command": "/usr/bin/audit-tool" } ]
        }
    });
    merge_cursor_hooks(&mut root).unwrap();
    assert_eq!(root["version"], json!(1), "Cursor requires version: 1");
    let arr = root["hooks"]["preToolUse"].as_array().unwrap();
    assert_eq!(arr.len(), 2, "user entry + paneflow entry");
    assert_eq!(arr[0]["command"], json!("/usr/bin/audit-tool"));

    remove_cursor_hooks(&mut root);
    let arr = root["hooks"]["preToolUse"].as_array().unwrap();
    assert_eq!(arr.len(), 1, "only the user's entry survives removal");
    assert_eq!(root["version"], json!(1));
}

#[test]
fn cursor_flat_remove_drops_version_when_nothing_else_remains() {
    let mut root = json!({});
    merge_cursor_hooks(&mut root).unwrap();
    remove_cursor_hooks(&mut root);
    assert_eq!(
        root,
        json!({}),
        "a fully-managed file must collapse to empty (then deleted)"
    );
}

#[test]
fn managed_guard_install_and_drop_roundtrip_in_clone_dir() {
    let td = tempfile::TempDir::new().unwrap();
    let dir = td.path().join(".codebuddy");

    let guard = ManagedHookConfigGuard::install_at(
        &dir,
        ManagedHookSpec::new(
            ".codebuddy",
            "settings.local.json",
            "CodeBuddy",
            merge_codebuddy_hooks,
            remove_paneflow_hooks,
        ),
        InvalidJsonPolicy::Replace,
    )
    .expect("install in fresh dir must succeed");

    let content = std::fs::read_to_string(dir.join("settings.local.json")).unwrap();
    let root: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(root["hooks"]["UserPromptSubmit"].is_array());
    let group = &root["hooks"]["UserPromptSubmit"][0];
    assert!(
        group.get("_paneflow_managed").is_none(),
        "CodeBuddy public schema does not document Paneflow-only markers"
    );
    let handler = &root["hooks"]["UserPromptSubmit"][0]["hooks"][0];
    assert_eq!(handler["type"], json!("command"));
    assert!(
        handler.get("commandWindows").is_none(),
        "CodeBuddy public schema does not document commandWindows"
    );

    drop(guard);
    assert!(
        !dir.exists(),
        "drop must delete the managed file and the created dir"
    );
}

#[test]
fn managed_guard_refuses_invalid_primary_user_config() {
    let td = tempfile::TempDir::new().unwrap();
    let dir = td.path().join(".gemini");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, "{ broken").unwrap();

    let guard = ManagedHookConfigGuard::install_at(
        &dir,
        ManagedHookSpec::new(
            ".gemini",
            "settings.json",
            "Gemini",
            merge_gemini_hooks,
            remove_gemini_hooks,
        ),
        InvalidJsonPolicy::Refuse,
    );

    assert!(guard.is_err());
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "{ broken",
        "primary user config must not be overwritten on parse failure"
    );
}

#[test]
fn pi_extension_guard_roundtrip() {
    let td = tempfile::TempDir::new().unwrap();
    let ext_dir = td.path().join(".pi/agent/extensions");
    let guard = PiExtensionGuard::install_at(&ext_dir).expect("install must succeed");
    let ext = ext_dir.join(PANEFLOW_TS_BASENAME);
    let content = std::fs::read_to_string(&ext).unwrap();
    assert!(
        content.contains("PANEFLOW_SOCKET_PATH"),
        "extension must be env-gated to stay inert outside Paneflow"
    );
    drop(guard);
    assert!(!ext.exists(), "drop must remove the extension file");
}

#[test]
fn opencode_guard_declares_plugin_and_cleans_up() {
    let td = tempfile::TempDir::new().unwrap();
    let dir = td.path().join("opencode");

    let guard = OpenCodePluginGuard::install_at(&dir).expect("fresh install must succeed");
    let plugin = dir.join("plugins").join(PANEFLOW_TS_BASENAME);
    assert!(plugin.is_file());
    let root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("opencode.json")).unwrap()).unwrap();
    let entries = root["plugin"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].as_str().unwrap().ends_with(PANEFLOW_TS_BASENAME));

    drop(guard);
    assert!(!plugin.exists(), "drop must remove the plugin file");
    assert!(
        !dir.join("opencode.json").exists(),
        "a config we created and fully own must be deleted on drop"
    );
}

#[test]
fn opencode_guard_preserves_user_config_and_refuses_unparseable() {
    let td = tempfile::TempDir::new().unwrap();
    let dir = td.path().join("opencode");
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(
        dir.join("opencode.json"),
        r#"{"model": "anthropic/claude-opus-4-8", "plugin": ["./mine.ts"]}"#,
    )
    .unwrap();
    let guard = OpenCodePluginGuard::install_at(&dir).unwrap();
    let root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("opencode.json")).unwrap()).unwrap();
    assert_eq!(root["plugin"].as_array().unwrap().len(), 2);
    drop(guard);
    let root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("opencode.json")).unwrap()).unwrap();
    assert_eq!(root["plugin"], json!(["./mine.ts"]));
    assert_eq!(root["model"], json!("anthropic/claude-opus-4-8"));

    std::fs::write(dir.join("opencode.json"), "{ definitely not json").unwrap();
    assert!(
        OpenCodePluginGuard::install_at(&dir).is_err(),
        "unparseable primary config must skip the install"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("opencode.json")).unwrap(),
        "{ definitely not json",
        "the user's file must be byte-identical after the refusal"
    );
}

#[test]
fn opencode_guard_skips_jsonc_only_setup() {
    let td = tempfile::TempDir::new().unwrap();
    let dir = td.path().join("opencode");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("opencode.jsonc"), "{ /* user comment */ }").unwrap();
    assert!(OpenCodePluginGuard::install_at(&dir).is_err());
    assert!(!dir.join("opencode.json").exists());
}

#[test]
fn grok_guard_writes_dedicated_file_and_removes_on_drop() {
    let td = tempfile::TempDir::new().unwrap();
    let hooks_dir = td.path().join(".grok/hooks");
    let guard = GrokHookFileGuard::install_at(&hooks_dir).expect("install must succeed");
    let path = hooks_dir.join("paneflow.json");
    let root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(root["hooks"]["UserPromptSubmit"].is_array());
    assert!(root["hooks"]["PermissionRequest"].is_array());
    assert!(root["hooks"]["Stop"].is_array());
    assert!(
        root["hooks"].get("Notification").is_none(),
        "Notification must not be registered for Grok; PermissionRequest handles approvals"
    );
    assert!(
        root["hooks"]["PreToolUse"][0]
            .get("_paneflow_managed")
            .is_none(),
        "Grok public docs do not document Paneflow-only markers"
    );
    let cmd = root["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(command_preserves_event_arg(cmd, "PreToolUse"));
    drop(guard);
    assert!(!path.exists(), "drop must delete the dedicated hook file");
}

#[test]
fn hermes_guard_appends_block_and_strips_on_drop() {
    let td = tempfile::TempDir::new().unwrap();
    let dir = td.path().join(".hermes");
    std::fs::create_dir_all(&dir).unwrap();
    let user_yaml = "model: hermes-4\n# my comment\nverbose: true\n";
    std::fs::write(dir.join("config.yaml"), user_yaml).unwrap();

    let guard = HermesHookConfigGuard::install_at(&dir).expect("install must succeed");
    let content = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
    assert!(content.starts_with(user_yaml), "user content untouched");
    assert!(content.contains(HERMES_BLOCK_BEGIN));
    assert!(content.contains("pre_llm_call:"));
    assert!(content.contains(" UserPromptSubmit"));
    assert!(content.contains(" PermissionRequest"));

    drop(guard);
    let content = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
    assert_eq!(
        content, user_yaml,
        "drop must restore the file byte-identical (comments included)"
    );
}

#[test]
fn hermes_guard_refuses_when_user_has_hooks_key() {
    let td = tempfile::TempDir::new().unwrap();
    let dir = td.path().join(".hermes");
    std::fs::create_dir_all(&dir).unwrap();
    let user_yaml = "hooks:\n  pre_tool_call:\n    - command: \"~/mine.sh\"\n";
    std::fs::write(dir.join("config.yaml"), user_yaml).unwrap();

    assert!(HermesHookConfigGuard::install_at(&dir).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.join("config.yaml")).unwrap(),
        user_yaml,
        "refusal must leave the file untouched"
    );
}

#[test]
fn hermes_guard_reinstall_is_idempotent_and_fresh_file_deleted() {
    let td = tempfile::TempDir::new().unwrap();
    let dir = td.path().join(".hermes");
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(dir.join("config.yaml"), hermes_managed_block()).unwrap();
    let g2 = HermesHookConfigGuard::install_at(&dir).unwrap();
    let content = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
    assert_eq!(
        content.matches(HERMES_BLOCK_BEGIN).count(),
        1,
        "re-install must replace, not stack, the managed block"
    );
    drop(g2);
    let content = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
    assert!(strip_hermes_managed_block(&content).is_none());
    assert!(content.trim().is_empty());
}

#[test]
fn strip_hermes_block_handles_absent_and_partial_markers() {
    assert!(strip_hermes_managed_block("model: x\n").is_none());
    let partial = format!("a: 1\n{HERMES_BLOCK_BEGIN}\nhooks:\n");
    assert!(strip_hermes_managed_block(&partial).is_none());
}

#[cfg(unix)]
#[test]
fn codex_install_at_creates_hooks_json_with_all_six_events() {
    let td = tempfile::TempDir::new().unwrap();
    let codex_dir = td.path().join(".codex");
    let guard = CodexHookConfigGuard::install_at(&codex_dir, None)
        .expect("install_at on empty tempdir must succeed");

    let content = std::fs::read_to_string(codex_dir.join("hooks.json")).unwrap();
    let root: serde_json::Value = serde_json::from_str(&content).unwrap();

    for event in CODEX_HOOK_EVENTS {
        let handlers = root["hooks"][*event].as_array().unwrap();
        assert_eq!(
            handlers.len(),
            1,
            "expected exactly one matcher-group for Codex {event}"
        );
        assert_eq!(
            handlers[0].get("_paneflow_managed"),
            Some(&json!(true)),
            "outer wrapper must carry the managed marker"
        );
        let cmd = handlers[0]
            .pointer("/hooks/0/command")
            .and_then(|v| v.as_str())
            .expect("command must be a string");
        assert!(
            is_paneflow_hook_command(cmd),
            "{event}: command {cmd:?} must be recognized as paneflow-managed"
        );
        assert!(
            command_preserves_event_arg(cmd, event),
            "{event}: command {cmd:?} must preserve the event name"
        );
    }

    assert!(
        root["hooks"].get("Notification").is_none(),
        "Codex hooks.json must not register a Notification event - it is not a Codex hook"
    );

    drop(guard);
    assert!(!codex_dir.join("hooks.json").exists());
    assert!(!codex_dir.exists());
}

#[cfg(unix)]
#[test]
fn codex_install_at_preserves_user_hooks_and_cleanup() {
    let td = tempfile::TempDir::new().unwrap();
    let codex_dir = td.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let initial = json!({
        "hooks": {
            "PreToolUse": [
                { "hooks": [{ "type": "command", "command": "echo codex-user-hook" }] }
            ]
        }
    });
    std::fs::write(
        codex_dir.join("hooks.json"),
        serde_json::to_string_pretty(&initial).unwrap(),
    )
    .unwrap();

    let guard = CodexHookConfigGuard::install_at(&codex_dir, None).unwrap();
    let content = std::fs::read_to_string(codex_dir.join("hooks.json")).unwrap();
    let root: serde_json::Value = serde_json::from_str(&content).unwrap();
    let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(arr.len(), 2, "user + paneflow entries coexist");

    drop(guard);
    let content = std::fs::read_to_string(codex_dir.join("hooks.json")).unwrap();
    let root: serde_json::Value = serde_json::from_str(&content).unwrap();
    let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(
        arr[0].pointer("/hooks/0/command"),
        Some(&json!("echo codex-user-hook"))
    );
}

#[cfg(unix)]
#[test]
fn enable_codex_feature_flag_creates_block_on_empty_file() {
    let td = tempfile::TempDir::new().unwrap();
    let path = td.path().join("config.toml");

    let result = enable_codex_feature_flag(&path);
    assert!(result.unwrap(), "empty file should trigger an append");

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains(CODEX_TOML_MARKER));
    assert!(content.contains("[features]"));
    assert!(content.contains("hooks = true"));
}

#[cfg(unix)]
#[test]
fn enable_codex_feature_flag_noop_when_already_enabled() {
    let td = tempfile::TempDir::new().unwrap();
    let path = td.path().join("config.toml");
    std::fs::write(&path, "[features]\nhooks = true\nother = false\n").unwrap();

    let result = enable_codex_feature_flag(&path);
    assert!(!result.unwrap(), "already-enabled must be a no-op");

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(!content.contains(CODEX_TOML_MARKER));
}

#[cfg(unix)]
#[test]
fn enable_codex_feature_flag_concurrent_no_duplicate_features() {
    let td = tempfile::TempDir::new().unwrap();
    let path = td.path().join("config.toml");
    std::fs::write(&path, "model = \"gpt-5\"\n").unwrap();

    let p1 = path.clone();
    let p2 = path.clone();
    let t1 = std::thread::spawn(move || enable_codex_feature_flag(&p1));
    let t2 = std::thread::spawn(move || enable_codex_feature_flag(&p2));
    let _ = t1.join();
    let _ = t2.join();

    let content = std::fs::read_to_string(&path).unwrap();
    let features = content.lines().filter(|l| l.trim() == "[features]").count();
    assert_eq!(
        features, 1,
        "exactly one [features] section after a concurrent enable, got:\n{content}"
    );
    assert!(content.contains("hooks = true"));
}

#[cfg(unix)]
#[test]
fn enable_codex_feature_flag_abstains_on_existing_features_section() {
    let td = tempfile::TempDir::new().unwrap();
    let path = td.path().join("config.toml");
    std::fs::write(&path, "[features]\nother_flag = false\n").unwrap();

    let result = enable_codex_feature_flag(&path);
    assert!(result.is_err());

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(!content.contains(CODEX_TOML_MARKER));
    assert!(!content.contains("hooks = true"));
}

#[cfg(unix)]
#[test]
fn codex_guard_wires_feature_flag_through_config_toml() {
    let td = tempfile::TempDir::new().unwrap();
    let codex_dir = td.path().join(".codex");
    let config_toml = td.path().join("config.toml");
    let user_config = "[user_stuff]\nkey = 1\n";
    std::fs::write(&config_toml, user_config).unwrap();

    let guard = CodexHookConfigGuard::install_at(&codex_dir, Some(&config_toml)).unwrap();

    let toml_content = std::fs::read_to_string(&config_toml).unwrap();
    assert!(toml_content.contains("hooks = true"));
    assert!(toml_content.contains(CODEX_TOML_MARKER));

    drop(guard);
    assert_eq!(
        std::fs::read_to_string(config_toml).unwrap(),
        user_config,
        "feature cleanup must restore the user's TOML byte-for-byte"
    );
}

#[test]
fn is_paneflow_hook_command_accepts_legacy_bare_name() {
    for event in CLAUDE_HOOK_EVENTS {
        let cmd = format!("paneflow-ai-hook {event}");
        assert!(
            is_paneflow_hook_command(&cmd),
            "legacy bare-name format must be recognized: {cmd:?}"
        );
    }
}

#[test]
fn is_paneflow_hook_command_accepts_unix_absolute_path() {
    let cmd = "/home/user/.cache/paneflow/bin/0.1.0/paneflow-ai-hook Stop";
    assert!(is_paneflow_hook_command(cmd));

    let cmd = "/usr/local/bin/paneflow-ai-hook PreToolUse";
    assert!(is_paneflow_hook_command(cmd));
}

#[test]
fn is_paneflow_hook_command_accepts_exe_basename() {
    let cmd = "/some/path/paneflow-ai-hook.exe Stop";
    assert!(is_paneflow_hook_command(cmd));
}

#[test]
fn is_paneflow_hook_command_recognizes_orphans_without_filesystem_check() {
    let cmd = "/nonexistent/old/cache/paneflow-ai-hook UserPromptSubmit";
    assert!(
        is_paneflow_hook_command(cmd),
        "orphaned absolute paths must be detectable for cleanup"
    );
}

#[test]
fn is_paneflow_hook_command_rejects_user_hooks() {
    let user_hooks = [
        "echo hello",
        "/usr/bin/git status",
        "node my-hook.js",
        "paneflow-shim Stop",
        "my-paneflow-ai-hook Stop",
        "/path/to/paneflow-ai-hook-2 Stop",
        "",
        "   ",
        "notarealcommand",
    ];
    for cmd in user_hooks {
        assert!(
            !is_paneflow_hook_command(cmd),
            "user hook {cmd:?} must NOT be classified as paneflow-managed"
        );
    }
}

#[test]
fn resolve_hook_command_output_is_recognized_by_detector() {
    for event in CLAUDE_HOOK_EVENTS {
        let cmd = resolve_hook_command(event);
        assert!(
            is_paneflow_hook_command(&cmd),
            "resolve_hook_command output must be detectable: {cmd:?}"
        );
        assert!(
            command_preserves_event_arg(&cmd, event),
            "resolve_hook_command output must preserve the event name: {cmd:?}"
        );
    }
}
