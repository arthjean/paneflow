use crate::hooks::{
    is_paneflow_hook_command, is_paneflow_matcher_group, merge_paneflow_hooks, HookConfigGuard,
    CLAUDE_HOOK_EVENTS,
};
use std::path::Path;

fn command_preserves_event_arg(command: &str, event: &str) -> bool {
    command.ends_with(&format!(" {event}"))
        || command.ends_with(&format!(" {event}\\\""))
        || command.contains(&format!(" {event} "))
}

// ---------- US-005: HookConfigGuard ----------
//
// All tests call `HookConfigGuard::install_at` with a tempdir-backed
// `.claude/` path rather than mutating `std::env::current_dir()` - the
// same env-free discipline used by US-002/003 tests.

use serde_json::json;

fn read_settings(claude_dir: &Path) -> serde_json::Value {
    let content = std::fs::read_to_string(claude_dir.join("settings.local.json")).unwrap();
    serde_json::from_str(&content).unwrap()
}

fn count_paneflow_entries(root: &serde_json::Value, event: &str) -> usize {
    root["hooks"][event]
        .as_array()
        .map(|a| a.iter().filter(|v| is_paneflow_matcher_group(v)).count())
        .unwrap_or(0)
}

#[test]
fn install_at_creates_file_with_all_five_events() {
    let td = tempfile::TempDir::new().unwrap();
    let claude_dir = td.path().join(".claude");

    let guard = HookConfigGuard::install_at(&claude_dir)
        .expect("install_at into an empty tempdir must succeed");

    let root = read_settings(&claude_dir);
    for event in CLAUDE_HOOK_EVENTS {
        let handlers = root["hooks"][*event].as_array().unwrap();
        assert_eq!(
            handlers.len(),
            1,
            "expected exactly one matcher-group for {event}"
        );

        // The exact command shape (bare name vs. absolute path) depends on
        // whether `current_exe()` finds a sibling `paneflow-ai-hook` -
        // which it does NOT in `cargo test` (test binary lives under
        // `target/debug/deps/`, hook binary lives under `target/debug/`).
        // Assert the contract instead of the format: it must be detectable
        // by `is_paneflow_hook_command`, and it must preserve the event
        // name so Claude Code dispatches to the correct handler.
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

        let timeout = handlers[0].pointer("/hooks/0/timeout").unwrap();
        assert_eq!(
            timeout,
            &json!(5),
            "timeout is in seconds per Claude Code docs"
        );

        // The marker sits on the OUTER matcher-group wrapper, not on
        // the inner Claude-Code-native handler (we don't pollute the
        // handler object with custom fields that Claude Code would
        // ignore anyway).
        assert_eq!(
            handlers[0].get("_paneflow_managed"),
            Some(&json!(true)),
            "outer matcher-group must carry the managed marker"
        );
        assert!(
            handlers[0].pointer("/hooks/0/_paneflow_managed").is_none(),
            "inner handler object must NOT carry the custom marker"
        );
    }

    drop(guard);
    // We created both the dir and the file - cleanup must remove both.
    assert!(!claude_dir.join("settings.local.json").exists());
    assert!(!claude_dir.exists());
}

#[test]
#[cfg(unix)]
fn install_at_refuses_symlinked_config_dir() {
    use std::os::unix::fs::symlink;

    // Attacker plants `.claude` as a DIRECTORY symlink (as git does on
    // checkout) pointing at a sibling dir OUTSIDE the project. `is_dir()`
    // follows it, so without the symlink_metadata guard `install_at`
    // would write `settings.local.json` into the target dir, crossing the
    // project boundary (CWE-59 / f004).
    let td = tempfile::TempDir::new().unwrap();
    let outside = td.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let claude_dir = td.path().join(".claude");
    symlink(&outside, &claude_dir).unwrap();

    let guard = HookConfigGuard::install_at(&claude_dir);
    assert!(
        guard.is_err(),
        "install_at must refuse a symlinked config dir"
    );
    assert!(
        !outside.join("settings.local.json").exists(),
        "no file may be planted through the symlink into the outside dir"
    );
}

#[test]
fn install_at_preserves_existing_user_hooks_and_permissions() {
    let td = tempfile::TempDir::new().unwrap();
    let claude_dir = td.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();

    // Pre-existing settings: user config + one of their own hooks on
    // UserPromptSubmit that must survive both install and cleanup.
    let initial = json!({
        "permissions": { "allow": ["Bash(ls:*)"] },
        "hooks": {
            "UserPromptSubmit": [
                { "hooks": [{ "type": "command", "command": "echo user-hook" }] }
            ]
        }
    });
    std::fs::write(
        claude_dir.join("settings.local.json"),
        serde_json::to_string_pretty(&initial).unwrap(),
    )
    .unwrap();

    let guard = HookConfigGuard::install_at(&claude_dir).unwrap();

    // After install: user entry + PaneFlow entry side-by-side.
    let root = read_settings(&claude_dir);
    let arr = root["hooks"]["UserPromptSubmit"].as_array().unwrap();
    assert_eq!(arr.len(), 2, "user + paneflow entries coexist");
    assert_eq!(
        arr.iter().filter(|v| is_paneflow_matcher_group(v)).count(),
        1
    );
    // Unrelated sections untouched.
    assert_eq!(root["permissions"]["allow"][0], json!("Bash(ls:*)"));

    drop(guard);

    // After drop: only the user's hook remains; the file persists
    // because the user's content is non-empty.
    let root = read_settings(&claude_dir);
    let arr = root["hooks"]["UserPromptSubmit"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let surviving_cmd = arr[0].pointer("/hooks/0/command").unwrap();
    assert_eq!(surviving_cmd, &json!("echo user-hook"));
    assert_eq!(root["permissions"]["allow"][0], json!("Bash(ls:*)"));
    // We did NOT create `.claude/`, so cleanup must leave it in place.
    assert!(claude_dir.exists());
}

#[test]
fn install_at_is_idempotent_on_reinstall() {
    let td = tempfile::TempDir::new().unwrap();
    let claude_dir = td.path().join(".claude");

    let first = HookConfigGuard::install_at(&claude_dir).unwrap();
    // Second install on top of the first must NOT duplicate entries.
    let second = HookConfigGuard::install_at(&claude_dir).unwrap();

    let root = read_settings(&claude_dir);
    for event in CLAUDE_HOOK_EVENTS {
        assert_eq!(
            count_paneflow_entries(&root, event),
            1,
            "{event} must carry exactly one PaneFlow entry after re-install"
        );
    }

    drop(second);
    drop(first); // idempotent drop: second pass reads the already-cleaned file
}

#[test]
fn first_guard_drop_preserves_hooks_for_sibling_session() {
    let td = tempfile::TempDir::new().unwrap();
    let claude_dir = td.path().join(".claude");

    let first = HookConfigGuard::install_at(&claude_dir).unwrap();
    let second = HookConfigGuard::install_at(&claude_dir).unwrap();

    drop(first);
    let root = read_settings(&claude_dir);
    for event in CLAUDE_HOOK_EVENTS {
        assert_eq!(
            count_paneflow_entries(&root, event),
            1,
            "{event} must remain installed while a sibling guard is alive"
        );
    }

    drop(second);
    assert!(
        !claude_dir.join("settings.local.json").exists(),
        "last guard drop owns the final cleanup"
    );
}

#[test]
fn merge_replaces_non_object_hooks_and_populates_events() {
    let mut root = json!({ "hooks": "broken" });
    merge_paneflow_hooks(&mut root).unwrap();
    for event in CLAUDE_HOOK_EVENTS {
        assert_eq!(
            count_paneflow_entries(&root, event),
            1,
            "{event} must be populated in the same merge pass"
        );
    }
}

#[test]
fn install_refuses_non_array_event_without_rewriting_the_file() {
    let td = tempfile::TempDir::new().unwrap();
    let claude_dir = td.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let settings_path = claude_dir.join("settings.local.json");
    let original = r#"{"hooks":{"Stop":"broken"},"theme":"dark"}"#;
    std::fs::write(&settings_path, original).unwrap();

    assert!(HookConfigGuard::install_at(&claude_dir).is_err());
    assert_eq!(std::fs::read_to_string(settings_path).unwrap(), original);
}

#[test]
fn cleanup_removes_managed_entries_even_when_marker_was_stripped() {
    // Simulate Claude Code re-serializing and stripping the
    // `_paneflow_managed` marker from the inner hook object. The
    // belt-and-suspenders prefix check on `command` must still detect
    // and clean up the handler. (anthropics/claude-code#5886)
    let td = tempfile::TempDir::new().unwrap();
    let claude_dir = td.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();

    let stripped = json!({
        "hooks": {
            "Stop": [
                {
                    // `_paneflow_managed` on the outer wrapper is gone.
                    "hooks": [
                        {
                            "type": "command",
                            "command": "paneflow-ai-hook Stop",
                            "timeout": 5
                        }
                    ]
                }
            ]
        }
    });
    std::fs::write(
        claude_dir.join("settings.local.json"),
        serde_json::to_string(&stripped).unwrap(),
    )
    .unwrap();

    // Install (no-op - detects our entry via command prefix) then drop.
    let guard = HookConfigGuard::install_at(&claude_dir).unwrap();
    drop(guard);

    // The managed handler is gone, but this fixture predates the guard and
    // carries no durable PaneFlow ownership evidence. Cleanup must preserve
    // the user's file rather than infer ownership from its now-empty shape.
    let settings = claude_dir.join("settings.local.json");
    assert!(settings.exists());
    let root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(settings).unwrap()).unwrap();
    assert_eq!(root, json!({}));
}

#[test]
fn cleanup_handles_preexisting_claude_dir_without_deleting_it() {
    // The user created `.claude/` themselves (for other Claude Code
    // files). Cleanup must NOT rmdir it, even when our settings file
    // was the only item inside.
    let td = tempfile::TempDir::new().unwrap();
    let claude_dir = td.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();

    let guard = HookConfigGuard::install_at(&claude_dir).unwrap();
    assert!(claude_dir.join("settings.local.json").exists());
    drop(guard);

    // Settings file gone (was only managed entries), but the directory
    // that the user already owned must remain.
    assert!(!claude_dir.join("settings.local.json").exists());
    assert!(
        claude_dir.exists(),
        "cleanup must not rmdir a user-owned .claude/"
    );
}

#[test]
fn cleanup_preserves_preexisting_empty_config_file() {
    let td = tempfile::TempDir::new().unwrap();
    let claude_dir = td.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let settings = claude_dir.join("settings.local.json");
    std::fs::write(&settings, "{}").unwrap();

    let guard = HookConfigGuard::install_at(&claude_dir).unwrap();
    drop(guard);

    assert!(settings.exists());
    let root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(settings).unwrap()).unwrap();
    assert_eq!(root, json!({}));
}

#[test]
fn install_at_tolerates_corrupt_existing_json() {
    // A corrupt settings file (mid-edit save, interrupted write)
    // shouldn't abort the shim - we overwrite and proceed.
    let td = tempfile::TempDir::new().unwrap();
    let claude_dir = td.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("settings.local.json"), "{not json}").unwrap();

    let guard =
        HookConfigGuard::install_at(&claude_dir).expect("corrupt JSON must not prevent install");
    let root = read_settings(&claude_dir);
    assert_eq!(count_paneflow_entries(&root, "UserPromptSubmit"), 1);

    drop(guard);
}

#[test]
fn merge_does_not_clobber_user_hooks_in_other_events() {
    let td = tempfile::TempDir::new().unwrap();
    let claude_dir = td.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let initial = json!({
        "hooks": {
            "PreToolUse": [
                { "matcher": "Bash", "hooks": [{ "type": "command", "command": "echo user" }] }
            ]
        }
    });
    std::fs::write(
        claude_dir.join("settings.local.json"),
        serde_json::to_string(&initial).unwrap(),
    )
    .unwrap();

    let guard = HookConfigGuard::install_at(&claude_dir).unwrap();

    let root = read_settings(&claude_dir);
    let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(arr.len(), 2, "user's Bash matcher + PaneFlow entry");
    // User's matcher preserved byte-for-byte.
    assert_eq!(arr[0]["matcher"], json!("Bash"));
    assert_eq!(
        arr[0].pointer("/hooks/0/command"),
        Some(&json!("echo user"))
    );

    drop(guard);

    let root = read_settings(&claude_dir);
    let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["matcher"], json!("Bash"));
}
