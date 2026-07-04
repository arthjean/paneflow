#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]
//! PaneFlow AI-binary shim.
//!
//! Copied (by US-008 extraction) under every `TerminalAgent` binary name
//! (`claude`, `codex`, `gemini`, `cursor-agent`, …) into the PaneFlow bin
//! cache dir, which US-009 prepends to the PTY's `$PATH`. When the user
//! runs one of those tools, this shim:
//!
//! 1. Reads its own filename via `current_exe()` to decide which tool to
//!    front for (`detect_tool`).
//! 2. PATH-walks `$PATH`, **excluding its own directory**, to locate the
//!    real AI binary (`find_real_binary`). Self-exclusion prevents an
//!    infinite exec-loop when the shim dir is first on `$PATH`.
//! 3. Runs the real binary with argv and env passed through. Both
//!    platforms spawn + wait (`Command::status()`): US-005's drop-cleanup
//!    guards and EP-004's `ai.exit` exit-status report are incompatible
//!    with `exec()`-style process replacement. The exit code is
//!    propagated verbatim (shell `128+signum` convention for signals).
//!
//! US-004 scope: detect / find / exec only. Hook config injection
//! (`.claude/settings.local.json` via US-005; `.codex/hooks.json` via
//! US-006) and env-var injection (`$PANEFLOW_AI_TOOL` / `$PANEFLOW_AI_PID`
//! for US-003 consumption) are added in later stories by wrapping around
//! this skeleton.

use std::env;
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

mod detect;
mod exec;
mod hooks;

use detect::*;
use exec::*;
use hooks::*;

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

/// Opt-in diagnostic logging for the sidebar-status hook chain. Mirrors
/// `paneflow-ai-hook`'s `diagnose()`: appends one line to `$PANEFLOW_HOOK_LOG`
/// when set and non-empty, a silent no-op otherwise. Deliberately NOT stderr -
/// the shim sits in front of the agent's TUI and stderr noise would corrupt
/// it (and Claude Code surfaces hook stderr in its UI). The app, shim, agent,
/// and ai-hook all honour the same env var, so one file captures the whole
/// pipeline and shows exactly where the chain stops on Windows.
///
/// EP-002 US-004 (agent-control-plane-hardening): `pub(crate)` so the hook
/// installer (`hooks.rs`) can pinpoint WHICH `None` branch it took - the
/// top-level `install_hook_guard = None` line alone cannot tell a persistent-
/// hook skip from a filesystem refusal.
pub(crate) fn diagnose(msg: &str) {
    let Some(path) = env::var_os("PANEFLOW_HOOK_LOG") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    // One atomic append (whole line incl. newline) so concurrent writers
    // (app, shim, ai-hook) don't interleave or drop lines.
    let line = format!("paneflow-shim[{}]: {msg}\n", std::process::id());
    let _ = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

fn main() -> ExitCode {
    let Some(tool) = detect_tool() else {
        // Direct invocation (`./paneflow-shim`) or unexpected rename. Exit 2
        // matches `getopts` convention for "usage error" - the one case
        // where stderr output is acceptable because the user's command
        // cannot proceed regardless of PaneFlow state.
        eprintln!(
            "paneflow-shim: invoked under an unexpected name; copy or \
             hardlink this binary under one of the Paneflow-wrapped agent \
             CLI names ('claude', 'codex', 'gemini', …) and put that \
             directory first on $PATH."
        );
        return ExitCode::from(2);
    };

    let Some(real) = find_real_binary(tool) else {
        // Same rationale: stderr output is the bash convention for
        // "command not found" (exit 127). The user's invocation cannot
        // succeed, so silent fail would be worse than a clear message.
        eprintln!("paneflow-shim: could not find real '{tool}' on PATH after self-exclusion");
        return ExitCode::from(127);
    };

    // Install hook config guards before spawning the child, remove on drop.
    // The binding is held to end of `main` so destructors fire after
    // `run_real` returns; `None` is the graceful-degradation path for a
    // read-only FS / missing permissions (PRD C4) - and for every wrapped
    // tool with no hook integration yet (the shim still provides the
    // universal `ai.exit`/`ai.session_end` lifecycle below).
    let _hook_guard = install_hook_guard(tool);
    // The single most important diagnostic: did hook config actually land?
    // `None` here (with `ipc_reachable=true`) means a real install failure;
    // `None` with `ipc_reachable=false` means the reachability gate rejected
    // us (the Windows named-pipe regression). `installed` means the agent
    // will fire `paneflow-ai-hook` and the sidebar should update.
    diagnose(&format!(
        "install_hook_guard({tool}) = {}; ipc_reachable = {}",
        if _hook_guard.is_some() {
            "installed"
        } else {
            "None (skipped)"
        },
        paneflow_ipc_reachable(),
    ));

    let args: Vec<OsString> = env::args_os().skip(1).collect();

    // Windows + codex: the JSONL tee path substitutes for config-file hooks.
    // Gated on detecting the `exec` subcommand; interactive codex falls
    // through to the plain `run_real` path without tee.
    #[cfg(not(unix))]
    let (code, agent_exit) = if tool == "codex" {
        let (final_args, should_tee) = rewrite_codex_args(&args);
        if should_tee {
            run_codex_with_jsonl_tee(&real, &final_args)
        } else {
            run_real(tool, &real, &final_args)
        }
    } else {
        run_real(tool, &real, &args)
    };
    #[cfg(unix)]
    let (code, agent_exit) = run_real(tool, &real, &args);

    // EP-004 US-010: report the agent binary's REAL exit status. The shell's
    // ChildExit only carries the shell's exit; this is the one place that
    // knows the agent's. Emitted BEFORE `notify_session_end` - both block on
    // the hook subprocess (`.status()`), so the server is guaranteed to see
    // `ai.exit` (which may set `Errored`) before `ai.session_end` (which
    // spares an `Errored` session instead of removing it). `None` (spawn or
    // wait failure) emits nothing - the server keeps today's behavior.
    if let Some(exit_code) = agent_exit {
        notify_exit(tool, exit_code);
    }

    // The real AI binary has exited. Neither claude nor codex fires a
    // session-end hook event of their own, so the sidebar loader would
    // stick indefinitely if the user quit during a `Thinking` turn (no
    // `Stop` hook fired ⇒ no 5s auto-reset armed). Best-effort poke at
    // `paneflow-ai-hook SessionEnd` to send a single `ai.session_end`
    // IPC frame; the server clears `ai_state` to `Inactive`. Any failure
    // here is silent - the worst case is a stale loader, not a broken
    // shell.
    notify_session_end(tool);

    code
}

/// Per-tool hook-config installation. One guard variant per config FORMAT:
/// Claude Code keeps its dedicated guard (persistent-hooks precedence logic);
/// Codex keeps its TOML+JSON pair (Unix only - Windows uses the JSONL tee);
/// everything else rides [`ManagedHookConfigGuard`] parameterized by
/// location + merge/remove pair. Tools without a hook integration return
/// `None` - they still get the shim's universal exit/session-end lifecycle.
// Fields are never READ - they exist solely so the wrapped guard's `Drop`
// (hook-config cleanup) fires when `main` returns.
#[allow(dead_code)]
enum ToolHookGuard {
    Claude(HookConfigGuard),
    #[cfg(unix)]
    Codex(CodexHookConfigGuard),
    Managed(ManagedHookConfigGuard),
    Pi(PiExtensionGuard),
    OpenCode(OpenCodePluginGuard),
    Hermes(HermesHookConfigGuard),
    Grok(GrokHookFileGuard),
}

fn install_hook_guard(tool: &str) -> Option<ToolHookGuard> {
    match tool {
        "claude" => HookConfigGuard::install().map(ToolHookGuard::Claude),
        #[cfg(unix)]
        "codex" => CodexHookConfigGuard::install().map(ToolHookGuard::Codex),
        // Windows: Codex now supports hooks (June 2026) using the SAME
        // matcher-group `hooks.json` format and event names as Claude - and
        // with NO `config.toml` feature flag - so ride the generic managed
        // guard over `.codex/hooks.json`. This gives INTERACTIVE Codex sidebar
        // status on Windows (the `codex exec` JSONL tee below only covered the
        // non-interactive case). See `merge_codex_hooks_win`.
        #[cfg(not(unix))]
        "codex" => ManagedHookConfigGuard::install_in_cwd(
            ".codex",
            "hooks.json",
            "Codex",
            merge_codex_hooks_win,
            remove_codex_hooks_win,
        )
        .map(ToolHookGuard::Managed),
        // Claude-Code-compatible clones: same settings.local.json format,
        // project-local dir, different event coverage.
        "codebuddy" => ManagedHookConfigGuard::install_in_cwd(
            ".codebuddy",
            "settings.local.json",
            "CodeBuddy",
            merge_codebuddy_hooks,
            remove_paneflow_hooks,
        )
        .map(ToolHookGuard::Managed),
        "qodercli" => ManagedHookConfigGuard::install_in_cwd(
            ".qoder",
            "settings.local.json",
            "Qoder",
            merge_qoder_hooks,
            remove_qoder_hooks,
        )
        .map(ToolHookGuard::Managed),
        // User-scope JSON agents (their project files are primary configs,
        // often git-tracked - mutating those would churn the user's diff for
        // the whole session). Gemini is matcher-grouped; Cursor is flat.
        "gemini" => ManagedHookConfigGuard::install_in_home(
            ".gemini",
            "settings.json",
            "Gemini CLI",
            merge_gemini_hooks,
            remove_gemini_hooks,
        )
        .map(ToolHookGuard::Managed),
        "cursor-agent" => ManagedHookConfigGuard::install_in_home(
            ".cursor",
            "hooks.json",
            "Cursor",
            merge_cursor_hooks,
            remove_cursor_hooks,
        )
        .map(ToolHookGuard::Managed),
        // TypeScript-plugin agents: an embedded bridge file is materialized
        // (and, for OpenCode, declared in opencode.json) for the session.
        "pi" => PiExtensionGuard::install().map(ToolHookGuard::Pi),
        "opencode" => OpenCodePluginGuard::install().map(ToolHookGuard::OpenCode),
        // YAML config, string-level marked block (comment-preserving).
        "hermes" => HermesHookConfigGuard::install().map(ToolHookGuard::Hermes),
        // Dedicated merged hook file - wholly Paneflow-owned, zero RMW.
        "grok" => GrokHookFileGuard::install().map(ToolHookGuard::Grok),
        // Deliberately ABSENT (documented, not forgotten):
        // - "copilot": no hook/JSON-stream surface exists at all.
        // - "kiro-cli": hooks live inside PER-AGENT definition files
        //   (`~/.kiro/agents/<name>.json`) - injecting would mean rewriting
        //   every agent the user defined, and the default agent has no
        //   file to extend. No per-session surface exists.
        // - "droid": hooks are dashboard-managed (closed-source).
        // - "agy" / "openclaw" / the rest: no stable public hook surface.
        // They all still get the universal `ai.exit`/`ai.session_end`
        // lifecycle plus the sidebar's process-scan "running" row.
        _ => None,
    }
}

/// EP-004 US-010: best-effort notify of `ai.exit { exit_code }` after the
/// real AI binary exits. Same contract as [`notify_session_end`] (sibling
/// hook binary, blocking `.status()` wait, silent failure); the raw code
/// rides in `PANEFLOW_AI_EXIT_CODE` since the hook's stdin is null on
/// shim-synthesized events.
fn notify_exit(tool: &str, exit_code: i32) {
    let Some(hook_path) = locate_sibling_hook_binary() else {
        return;
    };
    let _ = std::process::Command::new(&hook_path)
        .arg("Exit")
        .env("PANEFLOW_AI_TOOL", tool)
        .env("PANEFLOW_AI_PID", std::process::id().to_string())
        .env("PANEFLOW_AI_EXIT_CODE", exit_code.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Best-effort notify of `ai.session_end` after the real AI binary exits.
///
/// Locates `paneflow-ai-hook` next to this shim binary (US-008 extracts
/// both into the same cache dir, and `current_exe()` handles symlink/
/// hardlink resolution per `find_real_binary` precedent), then spawns it
/// with `SessionEnd` and the `PANEFLOW_AI_TOOL` env so the hook tags the
/// frame with the right tool identity. Inherits `PANEFLOW_SOCKET_PATH`
/// and `PANEFLOW_WORKSPACE_ID` from the shim's own env (they were set
/// by `pty_session::inject_ai_hook_env`).
///
/// Blocking wait with no explicit timeout: the hook's only work is a
/// single Unix-socket write of a tiny JSON frame, typically <5 ms. The
/// PRD's 15 ms latency budget for shim overhead (US-004 AC) is preserved
/// even adding this - a Unix-socket connect+write is well under that
/// alone, and we're outside the spawn-to-exec critical path here (the
/// user's command has already returned its exit code).
fn notify_session_end(tool: &str) {
    let Some(hook_path) = locate_sibling_hook_binary() else {
        return;
    };
    let _ = std::process::Command::new(&hook_path)
        .arg("SessionEnd")
        .env("PANEFLOW_AI_TOOL", tool)
        .env("PANEFLOW_AI_PID", std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Resolve `paneflow-ai-hook` (or `.exe` on Windows) sitting in the same
/// directory as this shim binary. Returns `None` if `current_exe()`
/// fails or the sibling isn't a regular file - in either case, the
/// caller silently skips notification.
pub(crate) fn locate_sibling_hook_binary() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let dir = exe.parent()?;
    #[cfg(unix)]
    let name = "paneflow-ai-hook";
    #[cfg(windows)]
    let name = "paneflow-ai-hook.exe";
    let candidate = dir.join(name);
    candidate.is_file().then_some(candidate)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn command_preserves_event_arg(command: &str, event: &str) -> bool {
        command.ends_with(&format!(" {event}"))
            || command.ends_with(&format!(" {event}\""))
            || command.contains(&format!(" {event} "))
    }

    #[test]
    fn detect_tool_from_stem_maps_known_stems() {
        // Every wrapped tool maps to itself - the stem IS the wire id.
        for tool in detect::WRAPPED_TOOLS {
            assert_eq!(detect_tool_from_stem(tool), Some(*tool));
        }
        assert_eq!(detect_tool_from_stem("claude"), Some("claude"));
        assert_eq!(detect_tool_from_stem("cursor-agent"), Some("cursor-agent"));
        assert_eq!(detect_tool_from_stem("qodercli"), Some("qodercli"));
    }

    #[test]
    fn detect_tool_from_stem_rejects_everything_else() {
        assert_eq!(detect_tool_from_stem("paneflow-shim"), None);
        assert_eq!(detect_tool_from_stem("Claude"), None, "case-sensitive");
        assert_eq!(detect_tool_from_stem("claude-code"), None);
        assert_eq!(detect_tool_from_stem("OpenCode"), None);
        assert_eq!(detect_tool_from_stem(""), None);
        assert_eq!(detect_tool_from_stem(" "), None);
    }

    #[cfg(unix)]
    #[test]
    fn candidate_names_unix_returns_bare_tool() {
        assert_eq!(candidate_names("claude"), vec!["claude".to_owned()]);
        assert_eq!(candidate_names("codex"), vec!["codex".to_owned()]);
    }

    #[cfg(windows)]
    #[test]
    fn candidate_names_windows_tries_exe_then_cmd() {
        assert_eq!(
            candidate_names("claude"),
            vec!["claude.exe".to_owned(), "claude.cmd".to_owned()],
            ".exe must be tried before .cmd so native builds win over wrappers"
        );
    }

    /// US-037: a real binary on `$PATH` carries the executable bit; the walk
    /// now requires it (a non-executable homonym must be skipped). Test fakes
    /// must therefore be made executable to stand in for real binaries.
    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn find_real_binary_in_locates_tempdir_binary() {
        let dir = tempfile::TempDir::new().unwrap();
        let fake = dir.path().join("claude");
        std::fs::File::create(&fake).unwrap();
        make_executable(&fake);

        let found = find_real_binary_in("claude", vec![dir.path().to_owned()], None, None);
        assert_eq!(found.as_deref(), Some(fake.as_path()));
    }

    #[cfg(unix)]
    #[test]
    fn find_real_binary_in_skips_non_executable_homonym() {
        // US-037 negative test: a non-executable file named like the tool
        // earlier in $PATH must be skipped so the real (executable) binary
        // later in $PATH is returned, mirroring execvp.
        let early = tempfile::TempDir::new().unwrap();
        let late = tempfile::TempDir::new().unwrap();
        std::fs::File::create(early.path().join("claude")).unwrap(); // 0644, no x
        let real = late.path().join("claude");
        std::fs::File::create(&real).unwrap();
        make_executable(&real);

        let found = find_real_binary_in(
            "claude",
            vec![early.path().to_owned(), late.path().to_owned()],
            None,
            None,
        );
        assert_eq!(
            found.as_deref(),
            Some(real.as_path()),
            "non-executable homonym must be skipped for the executable one"
        );
    }

    /// Windows counterpart: Claude Code ships as `claude.cmd` (a Node.js
    /// wrapper) on Windows today. The walk must find that file when no
    /// `.exe` exists alongside. When a `.exe` exists, it must win.
    #[cfg(windows)]
    #[test]
    fn find_real_binary_in_locates_cmd_then_exe_on_windows() {
        let dir = tempfile::TempDir::new().unwrap();
        let cmd_path = dir.path().join("claude.cmd");
        std::fs::File::create(&cmd_path).unwrap();

        // With only .cmd present, the walk falls through to it.
        let found = find_real_binary_in("claude", vec![dir.path().to_owned()], None, None);
        assert_eq!(found.as_deref(), Some(cmd_path.as_path()));

        // With both .exe and .cmd present, .exe wins per candidate ordering.
        let exe_path = dir.path().join("claude.exe");
        std::fs::File::create(&exe_path).unwrap();
        let found = find_real_binary_in("claude", vec![dir.path().to_owned()], None, None);
        assert_eq!(
            found.as_deref(),
            Some(exe_path.as_path()),
            "native .exe must take precedence over the .cmd wrapper"
        );
    }

    /// US-017 (cli-hardening-followup-2026-Q3): a hardlink of the
    /// shim binary planted in a DIFFERENT `$PATH` directory must be
    /// detected by file identity and skipped. The previous dir-only check
    /// let this through, recursively re-invoking the shim every
    /// time the user typed `claude` -- a single-user fork-bomb.
    #[test]
    fn shim_refuses_hardlink_loop() {
        let shim_dir = tempfile::TempDir::new().unwrap();
        let attacker_dir = tempfile::TempDir::new().unwrap();
        // Stand-in for the shim binary itself.
        let real_shim = shim_dir.path().join("paneflow-shim");
        std::fs::File::create(&real_shim).unwrap();
        // The hardlink shares the inode, so this also makes `attack_link`
        // executable - required now that the walk filters on the exec bit.
        #[cfg(unix)]
        make_executable(&real_shim);
        // Hardlink it into the attacker-controlled `$PATH` dir as
        // `claude` -- the dir-canonicalize check at the head of
        // `find_real_binary_in` would NOT catch this, but the
        // file-identity comparison must.
        let attack_link = attacker_dir.path().join(&candidate_names("claude")[0]);
        std::fs::hard_link(&real_shim, &attack_link).expect("hard_link");

        // `current_exe` analog: pretend the shim binary is at `real_shim`.
        let found = find_real_binary_in(
            "claude",
            vec![attacker_dir.path().to_owned()],
            Some(shim_dir.path()),
            Some(real_shim.as_path()),
        );
        assert!(
            found.is_none(),
            "hardlinked shim must be skipped; got {found:?}"
        );

        // Sanity: with NO self_exe (i.e. degraded mode where we can't
        // compute identity), the walk falls back to dir-only semantics
        // and DOES find the attacker file. The fix is dependent on
        // current_exe() resolving correctly -- documented degradation.
        let found = find_real_binary_in(
            "claude",
            vec![attacker_dir.path().to_owned()],
            Some(shim_dir.path()),
            None,
        );
        assert!(found.is_some(), "no-identity fallback finds candidate");
    }

    #[cfg(unix)]
    #[test]
    fn find_real_binary_in_excludes_self_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let fake = dir.path().join("claude");
        std::fs::File::create(&fake).unwrap();

        // The tempdir appears as both the only PATH entry AND as the self
        // dir. The self-exclusion must skip it and yield `None` - otherwise
        // the shim would exec itself and recurse.
        let found = find_real_binary_in(
            "claude",
            vec![dir.path().to_owned()],
            Some(dir.path()),
            None,
        );
        assert!(found.is_none(), "self_dir must be excluded from PATH walk");
    }

    #[cfg(unix)]
    #[test]
    fn find_real_binary_in_walks_past_self_dir_to_find_real_binary() {
        // Simulates the production layout: PATH = [shim_dir, real_dir].
        // The shim entry is self_dir and must be skipped; the second entry
        // yields the real binary.
        let shim_dir = tempfile::TempDir::new().unwrap();
        let real_dir = tempfile::TempDir::new().unwrap();

        // Create a fake `claude` in the shim dir too - this would cause
        // infinite recursion in production if self-exclusion didn't work.
        std::fs::File::create(shim_dir.path().join("claude")).unwrap();
        let real_fake = real_dir.path().join("claude");
        std::fs::File::create(&real_fake).unwrap();
        make_executable(&real_fake);

        let found = find_real_binary_in(
            "claude",
            vec![shim_dir.path().to_owned(), real_dir.path().to_owned()],
            Some(shim_dir.path()),
            None,
        );
        assert_eq!(found.as_deref(), Some(real_fake.as_path()));
    }

    #[test]
    fn find_real_binary_in_returns_none_when_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        // Empty dir, no matching binary anywhere on the passed "PATH".
        let found = find_real_binary_in("claude", vec![dir.path().to_owned()], None, None);
        assert!(found.is_none());
    }

    #[test]
    fn find_real_binary_in_tolerates_nonexistent_path_entries() {
        // PATH in the wild routinely contains stale directories (old
        // Python virtualenvs, uninstalled packages, typo'd PATH edits).
        // The walker must skip them silently rather than erroring.
        let dirs = vec![
            PathBuf::from("/definitely/does/not/exist/foo"),
            PathBuf::from("/also/not/real/bar"),
        ];
        let found = find_real_binary_in("claude", dirs, None, None);
        assert!(found.is_none());
    }

    /// Linux-gated timing guard. Replaces the PRD's "criterion benchmark"
    /// (PRD US-004 AC bullet 7) with a lightweight check that stays within
    /// the 15 ms budget even with a realistic number of stale `$PATH`
    /// entries. Criterion would pull ~30 dev-deps for one number; this
    /// guards the same invariant at ~zero cost.
    #[cfg(target_os = "linux")]
    #[test]
    fn find_real_binary_in_completes_under_15ms_budget() {
        let dirs: Vec<PathBuf> = (0..20)
            .map(|i| PathBuf::from(format!("/tmp/paneflow-nonexistent-{i}")))
            .collect();

        let start = std::time::Instant::now();
        let _ = find_real_binary_in("claude", dirs, None, None);
        let elapsed = start.elapsed();

        assert!(
            elapsed < std::time::Duration::from_millis(15),
            "PATH walk must complete under 15 ms; got {elapsed:?}"
        );
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
            guard.is_none(),
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
        merge_paneflow_hooks(&mut root);
        for event in CLAUDE_HOOK_EVENTS {
            assert_eq!(
                count_paneflow_entries(&root, event),
                1,
                "{event} must be populated in the same merge pass"
            );
        }
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

        // File must be fully cleaned - only our entry existed, so after
        // cleanup the file is gone. The directory was created by the test
        // (simulating a user-owned `.claude/`), so the guard correctly
        // leaves it in place - the `cleanup_handles_preexisting_claude_dir`
        // test separately validates the "we created it, we rmdir it"
        // inverse case.
        assert!(!claude_dir.join("settings.local.json").exists());
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
    fn install_at_tolerates_corrupt_existing_json() {
        // A corrupt settings file (mid-edit save, interrupted write)
        // shouldn't abort the shim - we overwrite and proceed.
        let td = tempfile::TempDir::new().unwrap();
        let claude_dir = td.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("settings.local.json"), "{not json}").unwrap();

        let guard = HookConfigGuard::install_at(&claude_dir)
            .expect("corrupt JSON must not prevent install");
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

    // ---------- Multi-agent: clones + JSON/TS/YAML guards ----------

    #[test]
    fn qoder_merge_skips_notification_event() {
        // Qoder has no `Notification` hook event - registering it could
        // make its config validator reject the whole file.
        let mut root = json!({});
        merge_qoder_hooks(&mut root);
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
        // Round-trip: removal leaves an empty tree (deletable file).
        remove_qoder_hooks(&mut root);
        assert_eq!(root, json!({}));
    }

    #[test]
    fn gemini_nested_merge_writes_official_shape_and_roundtrips() {
        let mut root = json!({});
        merge_gemini_hooks(&mut root);
        // Foreign key on the config side…
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
        // …canonical Claude-shaped event in the command arg.
        let cmd = inner[0]["command"].as_str().unwrap();
        assert!(
            command_preserves_event_arg(cmd, "UserPromptSubmit"),
            "BeforeAgent must invoke the canonical UserPromptSubmit: {cmd}"
        );
        // No Paneflow-only marker field (stricter parsers).
        assert!(group.get("_paneflow_managed").is_none());
        assert!(group.get("command").is_none());
        // Idempotent merge.
        merge_gemini_hooks(&mut root);
        assert_eq!(root["hooks"]["BeforeAgent"].as_array().unwrap().len(), 1);
        // Removal restores an empty tree.
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
        merge_cursor_hooks(&mut root);
        assert_eq!(root["version"], json!(1), "Cursor requires version: 1");
        let arr = root["hooks"]["preToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2, "user entry + paneflow entry");
        assert_eq!(arr[0]["command"], json!("/usr/bin/audit-tool"));

        remove_cursor_hooks(&mut root);
        let arr = root["hooks"]["preToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "only the user's entry survives removal");
        // `version` is kept while user content remains.
        assert_eq!(root["version"], json!(1));
    }

    #[test]
    fn cursor_flat_remove_drops_version_when_nothing_else_remains() {
        let mut root = json!({});
        merge_cursor_hooks(&mut root);
        remove_cursor_hooks(&mut root);
        assert_eq!(
            root,
            json!({}),
            "a fully-managed file must collapse to empty (then deleted)"
        );
    }

    #[test]
    fn managed_guard_install_and_drop_roundtrip_in_clone_dir() {
        // End-to-end for the clone path: .codebuddy/settings.local.json is
        // created with Claude-format hooks, then fully cleaned up on drop
        // (file deleted, created dir removed).
        let td = tempfile::TempDir::new().unwrap();
        let dir = td.path().join(".codebuddy");

        let guard = ManagedHookConfigGuard::install_at(
            &dir,
            "settings.local.json",
            "CodeBuddy",
            merge_codebuddy_hooks,
            remove_paneflow_hooks,
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
            "settings.json",
            "Gemini",
            merge_gemini_hooks,
            remove_gemini_hooks,
            InvalidJsonPolicy::Refuse,
        );

        assert!(guard.is_none());
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
            serde_json::from_str(&std::fs::read_to_string(dir.join("opencode.json")).unwrap())
                .unwrap();
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

        // User config with their own plugin entry survives the roundtrip.
        std::fs::write(
            dir.join("opencode.json"),
            r#"{"model": "anthropic/claude-opus-4-8", "plugin": ["./mine.ts"]}"#,
        )
        .unwrap();
        let guard = OpenCodePluginGuard::install_at(&dir).unwrap();
        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("opencode.json")).unwrap())
                .unwrap();
        assert_eq!(root["plugin"].as_array().unwrap().len(), 2);
        drop(guard);
        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("opencode.json")).unwrap())
                .unwrap();
        assert_eq!(root["plugin"], json!(["./mine.ts"]));
        assert_eq!(root["model"], json!("anthropic/claude-opus-4-8"));

        // PRIMARY config that doesn't parse must never be clobbered.
        std::fs::write(dir.join("opencode.json"), "{ definitely not json").unwrap();
        assert!(
            OpenCodePluginGuard::install_at(&dir).is_none(),
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
        // serde_json can't round-trip comments - a .jsonc-only setup must
        // be left alone entirely.
        std::fs::write(dir.join("opencode.jsonc"), "{ /* user comment */ }").unwrap();
        assert!(OpenCodePluginGuard::install_at(&dir).is_none());
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
        // Claude matcher-group shape, reduced event set with explicit
        // permission requests.
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
        // A duplicate top-level `hooks:` key would silently override the
        // user's own hooks under PyYAML-family last-wins semantics.
        let td = tempfile::TempDir::new().unwrap();
        let dir = td.path().join(".hermes");
        std::fs::create_dir_all(&dir).unwrap();
        let user_yaml = "hooks:\n  pre_tool_call:\n    - command: \"~/mine.sh\"\n";
        std::fs::write(dir.join("config.yaml"), user_yaml).unwrap();

        assert!(HermesHookConfigGuard::install_at(&dir).is_none());
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

        // Simulate a previous process that died after writing the managed
        // block but before Drop. A real crash kills that process too, so no
        // live lease remains in this process.
        std::fs::write(dir.join("config.yaml"), hermes_managed_block()).unwrap();
        let g2 = HermesHookConfigGuard::install_at(&dir).unwrap();
        let content = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert_eq!(
            content.matches(HERMES_BLOCK_BEGIN).count(),
            1,
            "re-install must replace, not stack, the managed block"
        );
        drop(g2);
        // g2 was created over a file g1 made - created_file=false for g2, so
        // the file survives but holds no managed block.
        let content = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(strip_hermes_managed_block(&content).is_none());
        assert!(content.trim().is_empty());
    }

    #[test]
    fn strip_hermes_block_handles_absent_and_partial_markers() {
        assert!(strip_hermes_managed_block("model: x\n").is_none());
        // Begin without end (truncated write) → refuse to strip.
        let partial = format!("a: 1\n{HERMES_BLOCK_BEGIN}\nhooks:\n");
        assert!(strip_hermes_managed_block(&partial).is_none());
    }

    // ---------- US-006: CodexHookConfigGuard (Unix) ----------

    #[cfg(unix)]
    #[test]
    fn codex_install_at_creates_hooks_json_with_all_six_events() {
        let td = tempfile::TempDir::new().unwrap();
        let codex_dir = td.path().join(".codex");
        // Pass None for config.toml path so tests don't touch real `~/.codex`.
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

        // `Notification` is NOT a Codex hook - confirm the registration
        // respects the platform's actual event surface even though the
        // `paneflow-ai-hook` binary happens to accept that event name.
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

    // ---------- US-006: TOML feature-flag mutation (Unix) ----------

    #[cfg(unix)]
    #[test]
    fn enable_codex_feature_flag_creates_block_on_empty_file() {
        let td = tempfile::TempDir::new().unwrap();
        let path = td.path().join("config.toml");

        let result = enable_codex_feature_flag(&path);
        assert_eq!(result, Some(true), "empty file should trigger an append");

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
        assert_eq!(result, Some(false), "already-enabled must be a no-op");

        // File unchanged.
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains(CODEX_TOML_MARKER));
    }

    #[cfg(unix)]
    #[test]
    fn enable_codex_feature_flag_concurrent_no_duplicate_features() {
        // US-027: two concurrent shims racing to enable the flag must not
        // produce a duplicate `[features]` section (invalid TOML). The flock
        // serializes the read-modify-write, so the second caller re-reads the
        // now-updated config and no-ops.
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
        // User already has `[features]` without `hooks` - appending
        // another `[features]` would trigger a duplicate-section TOML
        // parse error on Codex's side, so the shim must abstain.
        std::fs::write(&path, "[features]\nother_flag = false\n").unwrap();

        let result = enable_codex_feature_flag(&path);
        assert_eq!(result, None);

        // File untouched.
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains(CODEX_TOML_MARKER));
        assert!(!content.contains("hooks = true"));
    }

    #[cfg(unix)]
    #[test]
    fn disable_codex_feature_flag_removes_managed_block() {
        let td = tempfile::TempDir::new().unwrap();
        let path = td.path().join("config.toml");
        std::fs::write(&path, "[user_stuff]\nkey = 1\n").unwrap();

        // Install adds our block.
        let added = enable_codex_feature_flag(&path).unwrap();
        assert!(added);
        let with_block = std::fs::read_to_string(&path).unwrap();
        assert!(with_block.contains(CODEX_TOML_MARKER));

        // Cleanup strips exactly our 3-line block, preserving the user's
        // original content byte-for-byte.
        disable_codex_feature_flag(&path);
        let cleaned = std::fs::read_to_string(&path).unwrap();
        assert_eq!(cleaned, "[user_stuff]\nkey = 1\n");
    }

    #[cfg(unix)]
    #[test]
    fn disable_codex_feature_flag_deletes_empty_file() {
        let td = tempfile::TempDir::new().unwrap();
        let path = td.path().join("config.toml");

        // Install from nothing - file becomes just our 3-line block.
        assert_eq!(enable_codex_feature_flag(&path), Some(true));
        disable_codex_feature_flag(&path);
        assert!(!path.exists(), "config.toml we created must be removed");
    }

    #[cfg(unix)]
    #[test]
    fn codex_guard_wires_feature_flag_through_config_toml() {
        let td = tempfile::TempDir::new().unwrap();
        let codex_dir = td.path().join(".codex");
        let config_toml = td.path().join("config.toml");

        let guard = CodexHookConfigGuard::install_at(&codex_dir, Some(&config_toml)).unwrap();

        let toml_content = std::fs::read_to_string(&config_toml).unwrap();
        assert!(toml_content.contains("hooks = true"));
        assert!(toml_content.contains(CODEX_TOML_MARKER));

        drop(guard);
        // Config.toml cleanup: since this test created config.toml from
        // nothing, the file should be removed.
        assert!(!config_toml.exists());
    }

    // ---------- US-006: Windows JSONL parser + argv rewrite ----------
    //
    // These tests are cross-platform (no `#[cfg]`) because `parse_codex_event`
    // and `rewrite_codex_args` are compile-gated `#[cfg(not(unix))]`. We
    // re-gate the tests to keep them compiling on Windows CI. On Unix the
    // functions don't exist, so the tests go away cleanly.

    #[cfg(not(unix))]
    #[test]
    fn parse_codex_event_maps_known_types_to_paneflow_events() {
        assert_eq!(
            parse_codex_event(r#"{"type":"turn.started"}"#),
            Some("UserPromptSubmit")
        );
        assert_eq!(
            parse_codex_event(r#"{"type":"turn.completed","usage":{"input_tokens":1}}"#),
            Some("Stop")
        );
        assert_eq!(
            parse_codex_event(r#"{"type":"error","message":"oops"}"#),
            Some("Notification")
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn parse_codex_event_returns_none_for_unmapped_known_types() {
        // These types ARE emitted by Codex but we intentionally don't
        // translate them into IPC frames - `thread.started` isn't a
        // meaningful sidebar state, `turn.failed` is already covered by
        // `error`-style notifications, and `item.*` are sub-events that
        // would over-fire the loader.
        for t in &[
            "thread.started",
            "turn.failed",
            "item.started",
            "item.completed",
        ] {
            let line = format!(r#"{{"type":"{t}"}}"#);
            assert_eq!(
                parse_codex_event(&line),
                None,
                "{t} must be silently skipped"
            );
        }
    }

    #[cfg(not(unix))]
    #[test]
    fn parse_codex_event_returns_none_for_invalid_or_unknown_input() {
        assert_eq!(parse_codex_event(""), None);
        assert_eq!(parse_codex_event("   "), None);
        assert_eq!(parse_codex_event("not json"), None);
        assert_eq!(parse_codex_event(r#"{"no_type": true}"#), None);
        assert_eq!(
            parse_codex_event(r#"{"type":"unknown.future.event"}"#),
            None
        );
    }

    /// Schema-pin test (PRD US-006 AC bullet 3): assert that every entry in
    /// `KNOWN_CODEX_EVENT_TYPES` has an explicit mapping in `parse_codex_event`
    /// AND the fixture below covers every known event 1:1. When Codex adds a
    /// new event type:
    ///   1. Add the new type string to `KNOWN_CODEX_EVENT_TYPES`.
    ///   2. Add a match arm for it in `parse_codex_event` (even if it's
    ///      `None` - the arm must be explicit, not the catch-all).
    ///   3. Add a `(type, expected)` entry to the fixture below.
    ///
    /// Failing to do (3) trips the `fixture.len() == KNOWN.len()` check
    /// with a clear message. Failing to do (2) trips the membership assert.
    /// Failing to do (1) still compiles but leaves the catch-all `_ => None`
    /// arm of `parse_codex_event` handling the new type silently - which is
    /// acceptable (loader doesn't update) but is the one drift mode this
    /// test cannot catch without actually running Codex.
    #[cfg(not(unix))]
    #[test]
    fn parse_codex_event_schema_pin() {
        let fixture: &[(&str, Option<&str>)] = &[
            ("thread.started", None),
            ("turn.started", Some("UserPromptSubmit")),
            ("turn.completed", Some("Stop")),
            ("turn.failed", None),
            ("item.started", None),
            ("item.completed", None),
            ("error", Some("Notification")),
        ];

        assert_eq!(
            fixture.len(),
            KNOWN_CODEX_EVENT_TYPES.len(),
            "KNOWN_CODEX_EVENT_TYPES has {} entries but fixture has {}; \
             update both together when Codex adds a new event type",
            KNOWN_CODEX_EVENT_TYPES.len(),
            fixture.len()
        );

        for (codex_type, expected) in fixture {
            assert!(
                KNOWN_CODEX_EVENT_TYPES.contains(codex_type),
                "fixture contains {codex_type} but KNOWN_CODEX_EVENT_TYPES \
                 does not - add it there and to parse_codex_event's match"
            );
            let line = format!(r#"{{"type":"{codex_type}"}}"#);
            let actual = parse_codex_event(&line);
            assert_eq!(
                actual, *expected,
                "schema drift: Codex event {codex_type} mapped to {actual:?}; \
                 expected {expected:?}. If Codex has added / renamed this \
                 event, update KNOWN_CODEX_EVENT_TYPES, parse_codex_event, \
                 and this fixture together."
            );
        }
    }

    #[cfg(not(unix))]
    #[test]
    fn rewrite_codex_args_injects_json_after_exec_at_any_position() {
        // `exec` at argv[0] - classic case.
        let exec_first = vec![
            OsString::from("exec"),
            OsString::from("--model"),
            OsString::from("o4"),
        ];
        let (rewritten, should_tee) = rewrite_codex_args(&exec_first);
        assert!(should_tee);
        assert_eq!(
            rewritten,
            vec![
                OsString::from("exec"),
                OsString::from("--json"),
                OsString::from("--model"),
                OsString::from("o4"),
            ]
        );

        // Global flag before subcommand: `codex --config cfg.toml exec prompt`
        // - the Phase 6 reviewer's SHOULD_FIX #8 scenario. The scan-anywhere
        // fix ensures we still detect `exec` and inject `--json` after it.
        let global_then_exec = vec![
            OsString::from("--config"),
            OsString::from("cfg.toml"),
            OsString::from("exec"),
            OsString::from("prompt"),
        ];
        let (rewritten, should_tee) = rewrite_codex_args(&global_then_exec);
        assert!(
            should_tee,
            "global flag before `exec` must still trigger tee"
        );
        assert_eq!(
            rewritten,
            vec![
                OsString::from("--config"),
                OsString::from("cfg.toml"),
                OsString::from("exec"),
                OsString::from("--json"),
                OsString::from("prompt"),
            ]
        );

        // Interactive invocation - no `exec` token, no tee.
        let interactive: Vec<OsString> = vec![];
        let (rewritten, should_tee) = rewrite_codex_args(&interactive);
        assert!(!should_tee);
        assert_eq!(rewritten, interactive);

        // Other subcommand - still no tee.
        let resume = vec![OsString::from("resume")];
        let (rewritten, should_tee) = rewrite_codex_args(&resume);
        assert!(!should_tee);
        assert_eq!(rewritten, resume);
    }

    // ---------- Hook-command detection (basename rule) ----------

    /// The legacy bare-name format MUST stay recognized so a shim upgrade
    /// can clean up `settings.local.json` files written by the previous
    /// version (which used `format!("paneflow-ai-hook {event}")` directly).
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

    /// New absolute-path format produced by `resolve_hook_command` when a
    /// sibling binary is present. This is the production case for end users.
    #[test]
    fn is_paneflow_hook_command_accepts_unix_absolute_path() {
        let cmd = "/home/user/.cache/paneflow/bin/0.1.0/paneflow-ai-hook Stop";
        assert!(is_paneflow_hook_command(cmd));

        let cmd = "/usr/local/bin/paneflow-ai-hook PreToolUse";
        assert!(is_paneflow_hook_command(cmd));
    }

    /// Windows variant: the binary basename is `paneflow-ai-hook.exe` and
    /// `Path::file_name` on Unix still extracts the trailing component
    /// correctly when the input uses forward slashes (Path semantics differ
    /// on Windows for `\`, but the basename rule covers both names).
    #[test]
    fn is_paneflow_hook_command_accepts_exe_basename() {
        let cmd = "/some/path/paneflow-ai-hook.exe Stop";
        assert!(is_paneflow_hook_command(cmd));
    }

    /// Fix B (orphan cleanup): even if the binary at the absolute path no
    /// longer exists on disk, the command must still be recognized so
    /// `remove_paneflow_hooks` can purge stale entries written by an
    /// earlier paneflow install that has since been removed.
    #[test]
    fn is_paneflow_hook_command_recognizes_orphans_without_filesystem_check() {
        // Path that almost certainly does not exist - the function must NOT
        // touch the filesystem.
        let cmd = "/nonexistent/old/cache/paneflow-ai-hook UserPromptSubmit";
        assert!(
            is_paneflow_hook_command(cmd),
            "orphaned absolute paths must be detectable for cleanup"
        );
    }

    /// User hooks must NOT be misclassified as paneflow-managed. The
    /// basename rule narrows the namespace collision risk vs. the previous
    /// bare-prefix rule, but rejection of common user patterns is the
    /// primary safety property.
    #[test]
    fn is_paneflow_hook_command_rejects_user_hooks() {
        let user_hooks = [
            "echo hello",
            "/usr/bin/git status",
            "node my-hook.js",
            "paneflow-shim Stop",               // sibling binary, different name
            "my-paneflow-ai-hook Stop",         // similar but distinct basename
            "/path/to/paneflow-ai-hook-2 Stop", // suffixed name
            "",                                 // empty
            "   ",                              // whitespace only
            "notarealcommand",                  // no event
        ];
        for cmd in user_hooks {
            assert!(
                !is_paneflow_hook_command(cmd),
                "user hook {cmd:?} must NOT be classified as paneflow-managed"
            );
        }
    }

    /// Round-trip property: `resolve_hook_command` must produce a string
    /// that `is_paneflow_hook_command` recognizes, regardless of which
    /// branch (sibling-found or bare-name fallback) was taken. Without
    /// this, a user could end up with hooks they cannot clean up.
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
}
