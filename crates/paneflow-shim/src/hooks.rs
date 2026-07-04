//! Claude Code hook-config injection + Codex hook config + Windows JSONL tee
//! (US-052 split).

use crate::locate_sibling_hook_binary;
use std::env;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
// The cross-platform atomic writers call `tmp.flush()` (method form), which
// needs the `Write` trait in scope on both platforms.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
// US-052: the Windows JSONL-tee path (`run_codex_with_jsonl_tee`, cfg'd out on
// Unix) reuses the exec module's exit-status mapping and needs the process /
// OS-string types. All cfg-gated so they aren't flagged unused on the Unix
// build where the tee code is absent.
#[cfg(not(unix))]
use crate::exec::{exit_code_from_status, raw_exit_code_from_status};
#[cfg(not(unix))]
use std::ffi::OsString;
#[cfg(not(unix))]
use std::process::ExitCode;

// ---------------------------------------------------------------------------
// Hook config injection (US-005) - idempotent `.claude/settings.local.json`
// ---------------------------------------------------------------------------

/// Claude Code 2.x hook events the shim registers handlers for. `SubagentStop`
/// is intentionally omitted - the server maps it to `ai.stop` identically to
/// `Stop` (US-002), and registering both would produce duplicate IPC frames.
pub(crate) const CLAUDE_HOOK_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "Notification",
    "Stop",
    "PreToolUse",
    "PostToolUse",
];

/// Bare-name fallback used when `locate_sibling_hook_binary()` cannot resolve
/// the absolute path to the sibling hook binary (test harness, exotic FS,
/// `current_exe` failure). Detection (`is_paneflow_hook_command`) is
/// basename-based so this fallback is also recognized for cleanup, alongside
/// the absolute-path form normally written by `resolve_hook_command`.
///
/// Cleanup matches by basename as a belt-and-suspenders complement to the
/// `_paneflow_managed` marker: Claude Code does not guarantee unknown fields
/// survive re-serialization (anthropics/claude-code#5886), so the command
/// string is the only round-trip-stable identifier we can rely on.
///
/// Known namespace-collision limitation: any user-authored hook command whose
/// program basename is literally `paneflow-ai-hook` (or `.exe` on Windows)
/// will be treated as PaneFlow-managed and removed on cleanup. The basename
/// rule narrows this further than the previous bare-prefix rule did, but the
/// theoretical collision remains.
pub(crate) const HOOK_COMMAND_PREFIX: &str = "paneflow-ai-hook ";

const CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const CONFIG_LOCK_RETRY: Duration = Duration::from_millis(25);
const STALE_CONFIG_LOCK_AFTER: Duration = Duration::from_secs(60);
static NEXT_LEASE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InvalidJsonPolicy {
    Replace,
    Refuse,
}

struct ConfigFileLock {
    path: PathBuf,
    _file: File,
}

impl Drop for ConfigFileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_path_for(path: &Path) -> PathBuf {
    let mut lock = path.as_os_str().to_owned();
    lock.push(".paneflow.lock");
    PathBuf::from(lock)
}

fn remove_stale_lock(lock_path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(lock_path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let Ok(age) = modified.elapsed() else {
        return false;
    };
    age > STALE_CONFIG_LOCK_AFTER && std::fs::remove_file(lock_path).is_ok()
}

fn acquire_config_lock(path: &Path) -> std::io::Result<ConfigFileLock> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "settings path has no parent",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let lock_path = lock_path_for(path);
    let deadline = Instant::now() + CONFIG_LOCK_TIMEOUT;
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => {
                return Ok(ConfigFileLock {
                    path: lock_path,
                    _file: file,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if remove_stale_lock(&lock_path) {
                    continue;
                }
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("timed out waiting for {}", safe_path_display(&lock_path)),
                    ));
                }
                std::thread::sleep(CONFIG_LOCK_RETRY);
            }
            Err(e) => return Err(e),
        }
    }
}

fn with_config_lock<T>(path: &Path, f: impl FnOnce() -> std::io::Result<T>) -> std::io::Result<T> {
    let _lock = acquire_config_lock(path)?;
    f()
}

fn resource_hash(path: &Path) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn lease_dir_for(path: &Path) -> Option<PathBuf> {
    path.parent().map(|parent| {
        parent
            .join(".paneflow-hook-leases")
            .join(format!("{:016x}", resource_hash(path)))
    })
}

#[derive(Debug)]
pub(crate) struct HookLease {
    dir: PathBuf,
    marker: PathBuf,
}

impl HookLease {
    fn acquire(resource_path: &Path) -> Option<Self> {
        let dir = lease_dir_for(resource_path)?;
        std::fs::create_dir_all(&dir).ok()?;
        let lease_id = NEXT_LEASE_ID.fetch_add(1, Ordering::Relaxed);
        let marker = dir.join(format!(
            "{}-{:?}-{lease_id}.lease",
            std::process::id(),
            std::thread::current().id()
        ));
        File::create(&marker).ok()?;
        Some(Self { dir, marker })
    }

    fn release_and_is_last(&self) -> bool {
        let _ = std::fs::remove_file(&self.marker);
        let has_other = std::fs::read_dir(&self.dir)
            .ok()
            .into_iter()
            .flat_map(|entries| entries.filter_map(Result::ok))
            .any(|entry| entry.path().extension().and_then(OsStr::to_str) == Some("lease"));
        if !has_other {
            let _ = std::fs::remove_dir(&self.dir);
            if let Some(parent) = self.dir.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
        !has_other
    }
}

impl Drop for HookLease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.marker);
    }
}

/// Render `path` for inclusion in an `eprintln!` going to the user's
/// terminal. Replaces bytes outside the printable ASCII range (`0x20..=0x7E`)
/// with `?` to defuse ANSI-escape-sequence injection via a maliciously
/// named CWD or `.claude/` directory. Path content is never a secret - it
/// was always going to be visible on stderr - but without this scrub, a
/// crafted directory could clear the screen, set the terminal title, or
/// inject false log lines (Phase 7 security audit, MEDIUM finding).
pub(crate) fn safe_path_display(path: &Path) -> String {
    path.display()
        .to_string()
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .collect()
}

fn safe_log_text(text: &str) -> String {
    text.chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .collect()
}

/// Returns `true` iff PaneFlow's IPC channel looks usable for hook-config
/// install. We treat "no channel" as "PaneFlow not running": in that state,
/// installing hook config is pointless (the hooks would invoke
/// `paneflow-ai-hook`, which fails silently per PRD constraint C4) and we
/// instead sweep any orphan entries left by a previous SIGKILL'd session.
///
/// The probe is deliberately NOT a uniform `Path::exists()` - that is correct
/// on Unix but actively wrong on Windows:
///
/// - **Unix**: `$PANEFLOW_SOCKET_PATH` is a domain-socket *filesystem node*,
///   so `Path::exists()` is a passive, side-effect-free `stat(2)` - a correct
///   liveness probe that is `true` exactly while the listener is bound.
/// - **Windows**: the path is a named pipe (`\\.\pipe\paneflow`). There,
///   `Path::exists()` is NOT passive - Rust's `fs::metadata` calls
///   `CreateFileW`, which on a pipe path *opens a client connection*. That
///   consumes the server's single pending pipe instance and returns
///   `ERROR_PIPE_BUSY` (so `exists() == false`) whenever the next instance
///   has not been re-created yet. Against the app's non-blocking accept loop
///   (`src-app/src/ipc.rs`, which sleeps 10 ms between `accept()`s) that race
///   is lost ~87% of the time, so the probe spuriously reported the *live*
///   server as unreachable and the shim skipped hook install - leaving the
///   sidebar agent status permanently dead on Windows while it worked on
///   Unix. Every connect also polluted the server with a phantom connection.
///   `$PANEFLOW_SOCKET_PATH` is only ever set by PaneFlow's own PTY
///   (`pty_session::assemble_pty_env`), so its presence already proves we are
///   inside a live PaneFlow session; we trust that and let the
///   fire-and-forget hook delivery fail silently (C4) in the rare case the
///   pipe is actually gone (app exited but the shell is still open).
pub(crate) fn paneflow_ipc_reachable() -> bool {
    reachable_from_socket_env(env::var_os("PANEFLOW_SOCKET_PATH").as_deref())
}

/// Testable inner for [`paneflow_ipc_reachable`] - takes the raw env value so
/// the policy is unit-testable without mutating the process-global env (the
/// `detect_tool_from` / `read_ai_pid_from` convention). See the caller's doc
/// for why the Windows branch skips the destructive `Path::exists()` probe.
fn reachable_from_socket_env(raw: Option<&OsStr>) -> bool {
    let Some(raw) = raw else {
        return false;
    };
    if raw.is_empty() {
        return false;
    }
    // Unix: passive `stat(2)`. Windows: presence of the PaneFlow-set env var
    // is authoritative (a `Path::exists()` probe would connect-as-client and
    // race the accept loop - see the caller's doc comment).
    #[cfg(not(windows))]
    {
        Path::new(raw).exists()
    }
    #[cfg(windows)]
    {
        true
    }
}

/// Returns `true` iff `dir` exists and is a symlink (i.e. `dir` itself is a
/// symbolic link, not the entry it points at). Uses `fs::symlink_metadata`
/// which - unlike `is_dir()` / `metadata()` - does NOT follow the final
/// component, so a repo-committed `.claude`/`.codex` directory symlink
/// (git mode 120000, materialized on checkout) is detected before we treat
/// it as a usable config dir. A symlinked intermediate dir would let
/// `write_atomic`'s `NamedTempFile::new_in(parent).persist(...)` create and
/// rename a file through the link, planting Paneflow-owned JSON outside the
/// project boundary (CWE-59 TOCTOU file-plant). A `NotFound`/IO error means
/// "not a symlink we need to refuse" - the caller then creates the dir
/// itself. Cross-platform: `FileType::is_symlink()` is correct on Unix,
/// macOS, and Windows (where it reports directory/file symlinks and
/// mount-point reparse points).
pub(crate) fn config_dir_is_symlink(dir: &Path) -> bool {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) => meta.file_type().is_symlink(),
        Err(_) => false,
    }
}

/// Best-effort removal of PaneFlow-managed entries from an existing hook
/// config file when no active IPC channel is reachable. Reads, runs
/// `remove_fn`, writes back (or deletes if the file is now empty). All
/// failures swallow silently - a sweep that fails just retries on the
/// next shim invocation. Used by `install()` to clean up after a previous
/// SIGKILL'd session that never got to fire its `Drop` impl.
pub(crate) fn sweep_orphan_hook_config(
    settings_path: &Path,
    remove_fn: fn(&mut serde_json::Value),
) {
    // Refuse to sweep through a symlinked config dir: `settings_path`'s parent
    // is the untrusted `.claude`/`.codex` directory taken from the project
    // CWD, and `write_atomic` below would create + rename the temp file
    // through it, planting (or removing) a file outside the project boundary.
    // Same guard as `install_hook_config_file` (CWE-59).
    if settings_path.parent().is_some_and(config_dir_is_symlink) {
        return;
    }
    if !settings_path.exists() {
        return;
    }
    let _ = with_config_lock(settings_path, || {
        let Ok(content) = std::fs::read_to_string(settings_path) else {
            return Ok(());
        };
        let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&content) else {
            return Ok(());
        };
        let before = root.clone();
        remove_fn(&mut root);
        if root == before {
            // Nothing to sweep - file has no PaneFlow entries.
            return Ok(());
        }
        let is_empty = root
            .as_object()
            .map(serde_json::Map::is_empty)
            .unwrap_or(false);
        if is_empty {
            let _ = std::fs::remove_file(settings_path);
        } else {
            write_atomic(settings_path, &root)?;
        }
        Ok(())
    });
}

/// Shared scaffold for both Claude and Codex hook config installation:
/// validate the config dir (creating it if absent), read+parse any existing
/// JSON tree, apply `merge_fn`, and write atomically. Project-local
/// ephemeral configs can repair corrupt JSON; primary user configs refuse it
/// so Paneflow never overwrites state it cannot parse. Returns
/// `Some((settings_path, created_dir, lease))` on success; `None` when the
/// filesystem refuses our writes or the config dir is occupied by a
/// non-directory.
///
/// Both `HookConfigGuard::install_at` (Claude) and
/// `CodexHookConfigGuard::install_at` (Codex) layer their type-construction
/// on top of this - the heavy filesystem + JSON work was previously
/// duplicated across ~80 lines apart.
pub(crate) fn install_hook_config_file(
    config_dir: &Path,
    config_filename: &str,
    tool_label: &str,
    merge_fn: fn(&mut serde_json::Value),
    invalid_json_policy: InvalidJsonPolicy,
) -> Option<(PathBuf, bool, HookLease)> {
    let settings_path = config_dir.join(config_filename);
    // Refuse a symlinked config dir BEFORE the `is_dir()` gate: `is_dir()`
    // follows symlinks, so a repo-committed `.claude -> D` directory symlink
    // would pass the gate and `write_atomic` (NamedTempFile::new_in(parent)
    // .persist) would create + rename the settings file inside `D`, crossing
    // the project boundary (CWE-59 TOCTOU file-plant). The rename-replaces
    // -symlink defense only covers a symlinked final-target file, not a
    // symlinked intermediate dir.
    if config_dir_is_symlink(config_dir) {
        eprintln!(
            "paneflow-shim: {} is a symlink; refusing to write {tool_label} \
             hooks through it (potential file-plant outside the project)",
            safe_path_display(config_dir)
        );
        return None;
    }
    // `exists()` returns true for both files and directories; we need to
    // distinguish so a stale `.claude`/`.codex` regular file (e.g. left
    // behind by an earlier tool) doesn't masquerade as a usable directory
    // and silently break hook injection further down. Surface the case
    // with an actionable message instead of letting `write_atomic` fail
    // with a cryptic ENOTDIR from inside `tempfile::NamedTempFile::new_in`.
    let existed_as_dir = config_dir.is_dir();
    let exists_as_other = config_dir.exists() && !existed_as_dir;

    if exists_as_other {
        eprintln!(
            "paneflow-shim: {} exists but is not a directory; remove or \
             rename it to enable {tool_label} hooks this session",
            safe_path_display(config_dir)
        );
        return None;
    }

    if !existed_as_dir {
        if let Err(e) = std::fs::create_dir_all(config_dir) {
            eprintln!(
                "paneflow-shim: cannot create {} ({e}); {tool_label} hooks \
                 disabled this session",
                safe_path_display(config_dir)
            );
            return None;
        }
    }

    let lease = HookLease::acquire(&settings_path)?;
    if let Err(e) = with_config_lock(&settings_path, || {
        let existing = std::fs::read_to_string(&settings_path).unwrap_or_default();
        let mut root: serde_json::Value = if existing.trim().is_empty() {
            serde_json::json!({})
        } else {
            match serde_json::from_str(&existing) {
                Ok(v) => v,
                Err(e) => match invalid_json_policy {
                    InvalidJsonPolicy::Replace => {
                        // Project-local ephemeral configs can be repaired in
                        // place: the user asked this shim invocation to own
                        // that managed file for the current session.
                        eprintln!(
                            "paneflow-shim: {} contained invalid JSON ({e}); \
                             overwriting with a fresh config",
                            safe_path_display(&settings_path)
                        );
                        serde_json::json!({})
                    }
                    InvalidJsonPolicy::Refuse => {
                        eprintln!(
                            "paneflow-shim: {} contained invalid JSON ({e}); \
                             refusing to overwrite primary {tool_label} config",
                            safe_path_display(&settings_path)
                        );
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "invalid JSON in primary user config",
                        ));
                    }
                },
            }
        };

        merge_fn(&mut root);
        write_atomic(&settings_path, &root)
    }) {
        eprintln!(
            "paneflow-shim: cannot write {} ({e}); {tool_label} hooks \
             disabled this session",
            safe_path_display(&settings_path)
        );
        if !existed_as_dir {
            let _ = std::fs::remove_dir(config_dir);
        }
        return None;
    }

    Some((settings_path, !existed_as_dir, lease))
}

/// Shared cleanup used by both guards' `Drop` impls: read the settings
/// file, run `remove_fn` to strip PaneFlow's entries, then either delete
/// the file (if now empty) and rmdir the config dir (if we created it),
/// or write the cleaned tree back atomically. All failures swallow
/// silently - Drop must never panic, and any error here means the next
/// shim invocation's merge-idempotency will converge the state.
pub(crate) fn cleanup_hook_config_file(
    settings_path: &Path,
    config_dir: &Path,
    created_dir: bool,
    remove_fn: fn(&mut serde_json::Value),
    lease: &HookLease,
) {
    let remove_created_dir = with_config_lock(settings_path, || {
        if !lease.release_and_is_last() {
            return Ok(false);
        }
        let Ok(content) = std::fs::read_to_string(settings_path) else {
            return Ok(false);
        };
        let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&content) else {
            return Ok(false);
        };

        remove_fn(&mut root);

        let is_empty = root
            .as_object()
            .map(serde_json::Map::is_empty)
            .unwrap_or(false);

        if is_empty {
            let _ = std::fs::remove_file(settings_path);
        } else {
            let _ = write_atomic(settings_path, &root);
        }
        Ok(is_empty && created_dir)
    })
    .unwrap_or(false);

    if remove_created_dir {
        // `remove_dir` only succeeds if the directory is empty - safe even if
        // the user dropped other files into the config dir. This must happen
        // after the lock guard drops, otherwise the lock file itself keeps the
        // directory non-empty.
        let _ = std::fs::remove_dir(config_dir);
    }
}

/// RAII guard: writes PaneFlow's hook config on construction, removes it on
/// drop. The guard must live for the duration of the child Claude Code
/// process, then drop normally when `main()` returns - this is why US-005
/// forces `run_real()` (vs `exec()`) so destructors actually fire.
/// Cross-platform home directory via std env only (the shim has no `dirs`
/// dep): `HOME` (Unix / most shells) then `USERPROFILE` (Windows).
fn home_dir_env() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| env::var_os("USERPROFILE").filter(|h| !h.is_empty()))
        .map(PathBuf::from)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PersistentHookState {
    Absent,
    Alive { command: String },
    Stale { command: Option<String> },
}

/// EP-004 US-018 + hardening US-005: state of `~/.claude/settings.json`.
/// Persistent hooks suppress the project-local injection only when at least
/// one managed command points at a binary we can prove exists. A stale global
/// hook is worse than no hook: it blocks the local fallback and leaves the
/// agent permanently `hooked:false`.
fn persistent_claude_hooks_state() -> PersistentHookState {
    let Some(home) = home_dir_env() else {
        return PersistentHookState::Absent;
    };
    let settings = home.join(".claude").join("settings.json");
    let Ok(bytes) = std::fs::read(&settings) else {
        return PersistentHookState::Absent;
    };
    let Ok(root) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return PersistentHookState::Absent;
    };
    settings_managed_hook_state(&root)
}

/// Pure check: does a parsed settings tree carry a Paneflow-managed hook for
/// any registered event? Split out so it is unit-testable without touching the
/// real home directory.
#[cfg(test)]
fn settings_has_managed_hook(root: &serde_json::Value) -> bool {
    !matches!(
        settings_managed_hook_state(root),
        PersistentHookState::Absent
    )
}

fn settings_managed_hook_state(root: &serde_json::Value) -> PersistentHookState {
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else {
        return PersistentHookState::Absent;
    };
    let mut stale_command: Option<String> = None;
    let mut saw_managed_without_command = false;

    for event in CLAUDE_HOOK_EVENTS {
        let Some(arr) = hooks.get(*event).and_then(|a| a.as_array()) else {
            continue;
        };
        for group in arr {
            if !is_paneflow_matcher_group(group) {
                continue;
            }
            let commands = paneflow_hook_commands_in_group(group);
            if commands.is_empty() {
                saw_managed_without_command = true;
                continue;
            }
            for command in commands {
                if paneflow_hook_command_program_exists(&command) {
                    return PersistentHookState::Alive { command };
                }
                stale_command.get_or_insert(command);
            }
        }
    }

    if stale_command.is_some() || saw_managed_without_command {
        PersistentHookState::Stale {
            command: stale_command,
        }
    } else {
        PersistentHookState::Absent
    }
}

pub(crate) struct HookConfigGuard {
    settings_path: PathBuf,
    claude_dir: PathBuf,
    // Whether the shim created `.claude/`. Only rmdir if we created it, so we
    // don't clobber a user-created directory that happened to be empty.
    created_dir: bool,
    lease: HookLease,
}

impl HookConfigGuard {
    /// Install in the project CWD. Returns `None` if the filesystem refuses
    /// our writes (read-only, permission denied, etc.) - the shim proceeds
    /// without hooks in that case, per PRD constraint C4. Also returns
    /// `None` (after sweeping any orphan entries left by a previous
    /// SIGKILL'd session) when no PaneFlow IPC socket is reachable: writing
    /// hook config that would invoke a dead handler would just create
    /// config noise per C4.
    pub(crate) fn install() -> Option<Self> {
        // EP-002 US-004 (agent-control-plane-hardening): every `None` branch
        // below now names itself in `PANEFLOW_HOOK_LOG`. The top-level
        // `install_hook_guard = None` line in `main` cannot distinguish a
        // persistent-hook skip from a filesystem refusal; these lines can, so
        // a `send "claude"` that lands `unknown_running` is self-diagnosing.
        let cwd = match env::current_dir() {
            Ok(c) => c,
            Err(e) => {
                crate::diagnose(&format!(
                    "claude: hook install skipped - current_dir() failed: {e}"
                ));
                return None;
            }
        };
        let claude_dir = cwd.join(".claude");
        if !paneflow_ipc_reachable() {
            crate::diagnose(
                "claude: hook install skipped - no Paneflow IPC socket reachable \
                 (PANEFLOW_SOCKET_PATH unset/stale); swept any orphan config",
            );
            sweep_orphan_hook_config(
                &claude_dir.join("settings.local.json"),
                remove_paneflow_hooks,
            );
            return None;
        }
        // EP-004 US-018: persistent user-scope hooks (`paneflow hooks setup`)
        // take precedence only when they are actually executable. A stale
        // global config used to suppress the local injection and strand the
        // pane in `unknown_running`; now it falls through to `install_at`.
        match persistent_claude_hooks_state() {
            PersistentHookState::Alive { command } => {
                crate::diagnose(&format!(
                    "claude: project-local hook install suppressed - verified \
                     Paneflow-managed persistent hook in ~/.claude/settings.json \
                     is executable ({}); swept any orphan config",
                    safe_log_text(&command)
                ));
                sweep_orphan_hook_config(
                    &claude_dir.join("settings.local.json"),
                    remove_paneflow_hooks,
                );
                return None;
            }
            PersistentHookState::Stale { command } => {
                let detail = command
                    .as_deref()
                    .map(safe_log_text)
                    .unwrap_or_else(|| "managed group carried no paneflow command".into());
                crate::diagnose(&format!(
                    "claude: ignoring stale Paneflow-managed persistent hook in \
                     ~/.claude/settings.json ({detail}); falling back to project-local \
                     hook install"
                ));
            }
            PersistentHookState::Absent => {}
        }
        match Self::install_at(&claude_dir) {
            Some(guard) => Some(guard),
            None => {
                // `install_at` logs the precise filesystem reason to stderr
                // (symlinked `.claude`, non-writable cwd, write failure). Mirror
                // a summary into PANEFLOW_HOOK_LOG so the whole chain lands in
                // one file (parity with the Windows named-pipe diagnostics).
                crate::diagnose(&format!(
                    "claude: hook install_at({}) returned None - filesystem refused \
                     (symlinked .claude, non-writable cwd, or write failure; see shim stderr)",
                    claude_dir.display()
                ));
                None
            }
        }
    }

    /// Testable inner. Takes the absolute path to the `.claude/` directory.
    /// Does NOT check `PANEFLOW_SOCKET_PATH` - the orphan-sweep gate lives
    /// in `install()` so unit tests can drive `install_at` without
    /// fabricating a live IPC socket.
    pub(crate) fn install_at(claude_dir: &Path) -> Option<Self> {
        let (settings_path, created_dir, lease) = install_hook_config_file(
            claude_dir,
            "settings.local.json",
            "Claude Code",
            merge_paneflow_hooks,
            InvalidJsonPolicy::Replace,
        )?;
        Some(Self {
            settings_path,
            claude_dir: claude_dir.to_path_buf(),
            created_dir,
            lease,
        })
    }
}

impl Drop for HookConfigGuard {
    fn drop(&mut self) {
        cleanup_hook_config_file(
            &self.settings_path,
            &self.claude_dir,
            self.created_dir,
            remove_paneflow_hooks,
            &self.lease,
        );
    }
}

/// Build the `"command"` string written into `settings.local.json` /
/// `hooks.json` for the given hook `event`. Prefers the absolute path to the
/// sibling `paneflow-ai-hook` binary (resolved via `current_exe().parent()`)
/// so hooks resolve even when Claude Code or Codex is launched outside the
/// PATH-injected shell paneflow normally provides - e.g., when a user runs
/// `claude` directly in a project where a previous shim invocation left a
/// managed `settings.local.json` in place. Falls back to the bare binary name
/// when `current_exe()` fails or the sibling isn't present (test harnesses
/// where the test binary lives in `target/debug/deps/`, exotic filesystems);
/// the bare form is still recognized by `is_paneflow_hook_command` for
/// detection and cleanup.
#[cfg(windows)]
pub(crate) fn resolve_hook_command(event: &str) -> String {
    resolve_windows_hook_command(event)
}

#[cfg(not(windows))]
pub(crate) fn resolve_hook_command(event: &str) -> String {
    match locate_sibling_hook_binary() {
        Some(path) => format!("{} {}", shell_program_path(&path), event),
        None => format!("{HOOK_COMMAND_PREFIX}{event}"),
    }
}

#[cfg(windows)]
pub(crate) fn resolve_hook_command_windows(event: &str) -> String {
    resolve_windows_hook_command(event)
}

#[cfg(windows)]
fn resolve_windows_hook_command(event: &str) -> String {
    match locate_sibling_hook_binary() {
        Some(path) => windows_powershell_hook_command(&path, event),
        None => format!(
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \"& '{}' {event}\"",
            HOOK_COMMAND_PREFIX.trim_end()
        ),
    }
}

#[cfg(windows)]
fn windows_powershell_hook_command(path: &Path, event: &str) -> String {
    format!(
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \"& {} {event}\"",
        powershell_single_quoted(&display_hook_program(path))
    )
}

#[cfg(windows)]
fn powershell_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Render a single `command` field for agents that do not document a
/// Windows-specific override. On Windows, make the command self-contained:
/// several hook runners execute the field through PowerShell, where a quoted
/// executable path requires the `&` call operator.
fn resolve_plain_hook_command(event: &str) -> String {
    #[cfg(windows)]
    {
        resolve_windows_hook_command(event)
    }
    #[cfg(not(windows))]
    {
        resolve_hook_command(event)
    }
}

/// Render the hook binary's absolute path for the `command` string the agent
/// writes into its hook config and later executes through a shell.
///
/// On Windows this MUST use forward slashes. Claude Code on Windows runs hook
/// commands via bash (`/usr/bin/bash -c …`), and bash treats `\` as an escape
/// character: a native `C:\Users\…\paneflow-ai-hook.exe` is de-escaped to
/// `C:Users…paneflow-ai-hook.exe` → "command not found", so the hook never
/// fires and the sidebar agent status stays dead on Windows (observed in the
/// field). `C:/Users/…/paneflow-ai-hook.exe` is accepted verbatim by bash,
/// cmd.exe, and PowerShell, and `Path::file_name` still extracts the basename
/// (Windows `Path` treats `/` as a separator), so [`is_paneflow_hook_command`]
/// keeps recognizing it for idempotent merge + cleanup - no detection change
/// needed, and the legacy backslash form is still matched for cleanup.
///
fn display_hook_program(path: &Path) -> String {
    let rendered = path.display().to_string();
    #[cfg(windows)]
    {
        rendered.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        rendered
    }
}

/// Render the hook program as a shell command token. This mirrors the durable
/// writer in `paneflow-mcp-install`: macOS stable paths can live under
/// `Application Support`, Windows users can have spaces in their profile path,
/// and stale-hook detection already parses the single-quote escape form.
#[cfg(not(windows))]
fn shell_program_path(path: &Path) -> String {
    let rendered = display_hook_program(path);
    if rendered
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\' | '$' | '`' | ';' | '&' | '|'))
    {
        format!("'{}'", rendered.replace('\'', "'\\''"))
    } else {
        rendered
    }
}

/// Returns `true` if `command` is a paneflow-managed hook command, regardless
/// of whether it uses the legacy bare-name format (`paneflow-ai-hook <Event>`)
/// or the absolute-path format produced by `resolve_hook_command`. Detection
/// is basename-based so it works across both shapes and across platforms
/// (`paneflow-ai-hook` on Unix, `paneflow-ai-hook.exe` on Windows).
///
/// Intentionally does NOT verify that the binary exists on disk: a stale
/// config pointing at a removed cache dir (e.g., user uninstalled paneflow,
/// `cargo clean` between sessions) must still be recognized so cleanup can
/// remove it on the next shim run.
pub(crate) fn is_paneflow_hook_command(command: &str) -> bool {
    paneflow_hook_program_token(command).is_some()
}

fn paneflow_hook_program_token(command: &str) -> Option<String> {
    if let Some(program) = command_program_token(command) {
        if is_paneflow_hook_program(&program) {
            return Some(program);
        }
    }
    embedded_paneflow_hook_program_token(command)
        .filter(|program| is_paneflow_hook_program(program))
}

fn is_paneflow_hook_program(program: &str) -> bool {
    let basename = Path::new(program)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(program);
    basename == "paneflow-ai-hook" || basename == "paneflow-ai-hook.exe"
}

fn paneflow_hook_commands_in_group(group: &serde_json::Value) -> Vec<String> {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .into_iter()
        .flatten()
        .filter_map(|hook| hook.get("command").and_then(|c| c.as_str()))
        .filter(|command| is_paneflow_hook_command(command))
        .map(ToOwned::to_owned)
        .collect()
}

fn paneflow_hook_command_program_exists(command: &str) -> bool {
    paneflow_hook_program_token(command)
        .as_deref()
        .is_some_and(program_exists)
}

fn embedded_paneflow_hook_program_token(command: &str) -> Option<String> {
    let needle = "paneflow-ai-hook";
    let idx = command.find(needle)?;
    let prefix = &command[..idx];
    let (start, quote) = if let Some((pos, ch)) = prefix
        .char_indices()
        .rev()
        .find(|(_, ch)| matches!(ch, '\'' | '"'))
    {
        (pos + ch.len_utf8(), Some(ch))
    } else {
        let start = prefix
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace() || *ch == '&')
            .map_or(0, |(pos, ch)| pos + ch.len_utf8());
        (start, None)
    };

    let suffix = &command[idx..];
    let end = if let Some(quote) = quote {
        suffix.find(quote).map_or(command.len(), |pos| idx + pos)
    } else {
        suffix
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace())
            .map_or(command.len(), |(pos, _)| idx + pos)
    };

    let mut program = command[start..end].to_owned();
    if quote == Some('\'') {
        program = program.replace("''", "'");
    }
    (!program.is_empty()).then_some(program)
}

/// Parse the shell command's first program token. This is intentionally small:
/// it supports unquoted paths, single/double quoted paths, and the shell
/// `'\''` sequence produced by standard single-quote escaping. It does not try
/// to evaluate a shell; if the first token cannot be parsed into a usable path,
/// the persistent hook is treated as stale and the shim falls back locally.
fn command_program_token(command: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = command.trim_start().chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(ch) = chars.next() {
        match quote {
            None => {
                if ch.is_whitespace() {
                    break;
                }
                match ch {
                    '\'' | '"' => quote = Some(ch),
                    '\\' if chars.peek() == Some(&'\'') => {
                        let _ = chars.next();
                        out.push('\'');
                    }
                    _ => out.push(ch),
                }
            }
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    out.push(ch);
                }
            }
            Some('"') => {
                if ch == '"' {
                    quote = None;
                } else if ch == '\\' {
                    match chars.peek().copied() {
                        Some(next @ ('"' | '\\' | '$' | '`')) => {
                            let _ = chars.next();
                            out.push(next);
                        }
                        _ => out.push(ch),
                    }
                } else {
                    out.push(ch);
                }
            }
            _ => {}
        }
    }

    (!out.is_empty()).then_some(out)
}

fn program_exists(program: &str) -> bool {
    let path = Path::new(program);
    if path.is_file() {
        return true;
    }
    if path.is_absolute() || path.parent().is_some_and(|p| !p.as_os_str().is_empty()) {
        return false;
    }
    path_program_exists(program)
}

fn path_program_exists(program: &str) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    for dir in env::split_paths(&path) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            if Path::new(program).extension().is_none() {
                let pathext = env::var_os("PATHEXT")
                    .and_then(|v| v.into_string().ok())
                    .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
                for ext in pathext.split(';').filter(|e| !e.is_empty()) {
                    if dir.join(format!("{program}{ext}")).is_file() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Merge PaneFlow's hook handlers into the parsed settings tree. Idempotent:
/// if a PaneFlow handler for an event is already present (identified by the
/// command basename OR the `_paneflow_managed` marker), we don't duplicate.
pub(crate) fn merge_paneflow_hooks(root: &mut serde_json::Value) {
    merge_matcher_hooks_for_events(root, CLAUDE_HOOK_EVENTS);
}

/// CodeBuddy uses the Claude matcher-group shape, but its public schema does
/// not document `commandWindows`. Keep the object strict and put the
/// cross-shell Windows quoting in the single `command` string.
pub(crate) fn merge_codebuddy_hooks(root: &mut serde_json::Value) {
    merge_strict_matcher_hooks_for_events_with(root, CLAUDE_HOOK_EVENTS, plain_hook_handler);
}

fn merge_matcher_hooks_for_events(root: &mut serde_json::Value, events: &[&str]) {
    merge_matcher_hooks_for_events_with(root, events, hook_handler);
}

fn merge_strict_matcher_hooks_for_events_with(
    root: &mut serde_json::Value,
    events: &[&str],
    handler_for_event: fn(&str) -> serde_json::Value,
) {
    merge_matcher_hooks_for_events_with_marker(root, events, handler_for_event, false);
}

fn merge_matcher_hooks_for_events_with(
    root: &mut serde_json::Value,
    events: &[&str],
    handler_for_event: fn(&str) -> serde_json::Value,
) {
    merge_matcher_hooks_for_events_with_marker(root, events, handler_for_event, true);
}

/// Matcher-group merge parameterized by event list. Claude/Codex get the
/// Paneflow marker because their settings writers tolerate it; stricter
/// clone schemas use the same shape without the marker and rely on command
/// basename detection for cleanup.
fn merge_matcher_hooks_for_events_with_marker(
    root: &mut serde_json::Value,
    events: &[&str],
    handler_for_event: fn(&str) -> serde_json::Value,
    include_marker: bool,
) {
    let root_obj = match root.as_object_mut() {
        Some(o) => o,
        None => {
            *root = serde_json::json!({});
            // re-borrow after replacement - `if let Some(...)` because we
            // just assigned an object, so this unwrap is infallible.
            let Some(o) = root.as_object_mut() else {
                return;
            };
            o
        }
    };

    let hooks_entry = root_obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks_entry.is_object() {
        // User's `hooks` key is not an object (e.g., user set it to a
        // string by mistake). Overwrite it - we own the managed entries.
        *hooks_entry = serde_json::json!({});
    }
    let Some(hooks_obj) = hooks_entry.as_object_mut() else {
        return;
    };

    for event in events {
        let entry = hooks_obj
            .entry(*event)
            .or_insert_with(|| serde_json::json!([]));
        let Some(array) = entry.as_array_mut() else {
            // User's event value is not an array; skip this event rather
            // than clobber what might be intentional config.
            continue;
        };

        // Self-healing instead of skip-if-present: drop any prior paneflow
        // entry (possibly STALE - an older shim version, or the pre-fix
        // Windows backslash command that bash de-escaped to "command not
        // found") before adding the freshly-resolved one. A plain skip would
        // otherwise pin a broken command across an upgrade until the next
        // clean Drop-cleanup (which never runs if the prior session was
        // hard-killed). Net effect stays idempotent: exactly one entry.
        array.retain(|g| !is_paneflow_matcher_group(g));

        let mut group = serde_json::Map::new();
        if include_marker {
            group.insert("_paneflow_managed".into(), serde_json::json!(true));
        }
        group.insert(
            "hooks".into(),
            serde_json::Value::Array(vec![handler_for_event(event)]),
        );
        array.push(serde_json::Value::Object(group));
    }
}

fn hook_handler(event: &str) -> serde_json::Value {
    let mut handler = serde_json::Map::new();
    handler.insert("type".into(), serde_json::Value::String("command".into()));
    handler.insert(
        "command".into(),
        serde_json::Value::String(resolve_hook_command(event)),
    );
    handler.insert("timeout".into(), serde_json::json!(5));
    #[cfg(windows)]
    {
        handler.insert(
            "commandWindows".into(),
            serde_json::Value::String(resolve_hook_command_windows(event)),
        );
    }
    serde_json::Value::Object(handler)
}

fn plain_hook_handler(event: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "command",
        "command": resolve_plain_hook_command(event),
        "timeout": 5,
    })
}

/// Remove PaneFlow's hook handlers from the parsed settings tree. Leaves
/// user entries untouched. Collapses empty event arrays and the empty
/// `hooks` key so cleanup produces a minimal file.
pub(crate) fn remove_paneflow_hooks(root: &mut serde_json::Value) {
    remove_matcher_hooks_for_events(root, CLAUDE_HOOK_EVENTS);
}

/// Claude-Code-FORMAT removal parameterized by event list (see
/// [`merge_matcher_hooks_for_events`]).
fn remove_matcher_hooks_for_events(root: &mut serde_json::Value, events: &[&str]) {
    let Some(root_obj) = root.as_object_mut() else {
        return;
    };
    let Some(hooks_obj) = root_obj.get_mut("hooks").and_then(|v| v.as_object_mut()) else {
        return;
    };

    for event in events {
        let Some(array) = hooks_obj.get_mut(*event).and_then(|v| v.as_array_mut()) else {
            continue;
        };
        array.retain(|matcher_group| !is_paneflow_matcher_group(matcher_group));
    }

    // Drop empty event arrays.
    hooks_obj.retain(|_k, v| v.as_array().is_none_or(|a| !a.is_empty()));

    // Drop the empty `hooks` object so the outer file can in turn become
    // empty and be deleted.
    if hooks_obj.is_empty() {
        root_obj.remove("hooks");
    }
}

// ---------------------------------------------------------------------------
// Claude-Code-compatible clones + flat-format CLIs (multi-agent hooks)
// ---------------------------------------------------------------------------

/// Qoder CLI hook events - the Claude Code set minus `Notification`
/// (unsupported there; `PostToolUseFailure` exists but has no `ai.*`
/// mapping, so it is deliberately not registered).
pub(crate) const QODER_HOOK_EVENTS: &[&str] =
    &["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"];

/// Qoder merge/remove - Claude Code format, reduced event list.
pub(crate) fn merge_qoder_hooks(root: &mut serde_json::Value) {
    merge_strict_matcher_hooks_for_events_with(root, QODER_HOOK_EVENTS, plain_hook_handler);
}
pub(crate) fn remove_qoder_hooks(root: &mut serde_json::Value) {
    remove_matcher_hooks_for_events(root, QODER_HOOK_EVENTS);
}

/// Gemini CLI hook registrations as `(foreign_event, canonical_event)`.
/// The CONFIG key uses Gemini's event vocabulary; the hook command's final
/// arg carries our canonical Claude-shaped event name so `paneflow-ai-hook`
/// needs zero per-CLI mapping. `Notification` is deliberately skipped: its
/// Gemini payload shape doesn't carry the `notification_type` whitelist the
/// hook gates `WaitingForInput` on, so registering it would only burn a
/// subprocess per notification for a frame the hook drops.
pub(crate) const GEMINI_HOOK_EVENTS: &[(&str, &str)] = &[
    ("BeforeAgent", "UserPromptSubmit"),
    ("AfterAgent", "Stop"),
    ("BeforeTool", "PreToolUse"),
    ("AfterTool", "PostToolUse"),
];

/// Cursor CLI (`cursor-agent`) hook registrations - camelCase vocabulary,
/// same `(foreign, canonical)` translation as Gemini, but Cursor's config
/// schema is flat rather than matcher-grouped.
pub(crate) const CURSOR_HOOK_EVENTS: &[(&str, &str)] = &[
    ("beforeSubmitPrompt", "UserPromptSubmit"),
    ("stop", "Stop"),
    ("preToolUse", "PreToolUse"),
    ("postToolUse", "PostToolUse"),
];

/// Merge hook entries in Cursor's FLAT format:
/// `hooks.<event>: [{command, timeout}]` - no matcher-group wrapper. No
/// `_paneflow_managed` marker either: Cursor's parser is stricter than
/// Claude Code's about unknown fields, so ownership detection rides
/// exclusively on the command basename ([`is_paneflow_hook_command`]).
/// `version_field`
/// stamps Cursor's required top-level `"version": 1` when absent.
fn merge_flat_hooks_for_events(
    root: &mut serde_json::Value,
    events: &[(&str, &str)],
    version_field: bool,
) {
    let root_obj = match root.as_object_mut() {
        Some(o) => o,
        None => {
            *root = serde_json::json!({});
            let Some(o) = root.as_object_mut() else {
                return;
            };
            o
        }
    };
    if version_field {
        root_obj
            .entry("version")
            .or_insert_with(|| serde_json::json!(1));
    }

    let hooks_entry = root_obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks_entry.is_object() {
        *hooks_entry = serde_json::json!({});
    }
    let Some(hooks_obj) = hooks_entry.as_object_mut() else {
        return;
    };

    for (foreign, canonical) in events {
        let entry = hooks_obj
            .entry(*foreign)
            .or_insert_with(|| serde_json::json!([]));
        let Some(array) = entry.as_array_mut() else {
            continue;
        };
        // Self-healing (see merge_matcher_hooks_for_events): replace any prior
        // paneflow entry rather than skip it, so a stale command (old format /
        // pre-fix Windows backslashes) is corrected on the next launch. Stays
        // idempotent - exactly one paneflow entry per event.
        array.retain(|e| !is_paneflow_flat_entry(e));
        array.push(serde_json::json!({
            "command": resolve_plain_hook_command(canonical),
            "timeout": 5,
        }));
    }
}

/// Removal counterpart of [`merge_flat_hooks_for_events`]. Also drops the
/// `"version"` field once the file holds nothing else of ours, so an
/// otherwise-empty managed file can be deleted by the shared cleanup.
fn remove_flat_hooks_for_events(root: &mut serde_json::Value, events: &[(&str, &str)]) {
    let Some(root_obj) = root.as_object_mut() else {
        return;
    };
    if let Some(hooks_obj) = root_obj.get_mut("hooks").and_then(|v| v.as_object_mut()) {
        for (foreign, _) in events {
            if let Some(array) = hooks_obj.get_mut(*foreign).and_then(|v| v.as_array_mut()) {
                array.retain(|entry| !is_paneflow_flat_entry(entry));
            }
        }
        hooks_obj.retain(|_k, v| v.as_array().is_none_or(|a| !a.is_empty()));
    }
    let hooks_empty = root_obj
        .get("hooks")
        .and_then(|v| v.as_object())
        .is_none_or(serde_json::Map::is_empty);
    if hooks_empty {
        root_obj.remove("hooks");
        // A bare `{"version": 1}` left behind would block file deletion in
        // `cleanup_hook_config_file` - drop it iff nothing else remains.
        if root_obj.len() == 1 && root_obj.contains_key("version") {
            root_obj.remove("version");
        }
    }
}

/// Flat-entry ownership test: `{command: "<…>paneflow-ai-hook <Event>"}`.
fn is_paneflow_flat_entry(value: &serde_json::Value) -> bool {
    value
        .get("command")
        .and_then(serde_json::Value::as_str)
        .is_some_and(is_paneflow_hook_command)
}

/// Per-tool wrapper fns - `install_hook_config_file` takes plain `fn`
/// pointers, so the event tables are baked in here.
pub(crate) fn merge_gemini_hooks(root: &mut serde_json::Value) {
    merge_gemini_hooks_for_events(root, GEMINI_HOOK_EVENTS);
}
pub(crate) fn remove_gemini_hooks(root: &mut serde_json::Value) {
    remove_gemini_hooks_for_events(root, GEMINI_HOOK_EVENTS);
}
pub(crate) fn merge_cursor_hooks(root: &mut serde_json::Value) {
    merge_flat_hooks_for_events(root, CURSOR_HOOK_EVENTS, true);
}
pub(crate) fn remove_cursor_hooks(root: &mut serde_json::Value) {
    remove_flat_hooks_for_events(root, CURSOR_HOOK_EVENTS);
}

/// Gemini's current official hook schema is matcher-grouped:
/// `hooks.<event>: [{ matcher, hooks: [{ name, type, command, timeout_ms }] }]`.
/// Timeout is milliseconds for Gemini, unlike Cursor/Hermes' seconds.
fn merge_gemini_hooks_for_events(root: &mut serde_json::Value, events: &[(&str, &str)]) {
    let root_obj = match root.as_object_mut() {
        Some(o) => o,
        None => {
            *root = serde_json::json!({});
            let Some(o) = root.as_object_mut() else {
                return;
            };
            o
        }
    };

    let hooks_entry = root_obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks_entry.is_object() {
        *hooks_entry = serde_json::json!({});
    }
    let Some(hooks_obj) = hooks_entry.as_object_mut() else {
        return;
    };

    for (foreign, canonical) in events {
        let entry = hooks_obj
            .entry(*foreign)
            .or_insert_with(|| serde_json::json!([]));
        let Some(array) = entry.as_array_mut() else {
            continue;
        };
        array.retain(|g| !is_paneflow_matcher_group(g));
        array.push(serde_json::json!({
            "matcher": "*",
            "hooks": [
                {
                    "name": "paneflow-status",
                    "type": "command",
                    "command": resolve_plain_hook_command(canonical),
                    "timeout": 5000,
                }
            ]
        }));
    }
}

fn remove_gemini_hooks_for_events(root: &mut serde_json::Value, events: &[(&str, &str)]) {
    let Some(root_obj) = root.as_object_mut() else {
        return;
    };
    if let Some(hooks_obj) = root_obj.get_mut("hooks").and_then(|v| v.as_object_mut()) {
        for (foreign, _) in events {
            if let Some(array) = hooks_obj.get_mut(*foreign).and_then(|v| v.as_array_mut()) {
                array.retain(|entry| !is_paneflow_matcher_group(entry));
            }
        }
        hooks_obj.retain(|_k, v| v.as_array().is_none_or(|a| !a.is_empty()));
    }
    let hooks_empty = root_obj
        .get("hooks")
        .and_then(|v| v.as_object())
        .is_none_or(serde_json::Map::is_empty);
    if hooks_empty {
        root_obj.remove("hooks");
    }
}

/// RAII guard for every JSON-config agent beyond Claude/Codex (CodeBuddy,
/// Qoder, Gemini, Cursor): same install/sweep/cleanup lifecycle as
/// [`HookConfigGuard`], parameterized by config location and merge/remove
/// pair. Two anchor modes:
/// - **cwd**: project-local config dir (`.codebuddy/`, `.qoder/`) - the
///   Claude `settings.local.json` precedent, ephemeral by design.
/// - **home**: user-scope config dir (`~/.gemini/`, `~/.cursor/`) - used
///   where the project file is the tool's PRIMARY config (often
///   git-tracked; mutating it would churn the user's diff all session).
///   Outside a Paneflow PTY the installed hooks are inert: the hook binary
///   exits silently when `PANEFLOW_SOCKET_PATH` is absent (C4).
pub(crate) struct ManagedHookConfigGuard {
    settings_path: PathBuf,
    config_dir: PathBuf,
    created_dir: bool,
    remove_fn: fn(&mut serde_json::Value),
    lease: HookLease,
}

impl ManagedHookConfigGuard {
    /// Project-CWD anchor (Claude-clone pattern).
    pub(crate) fn install_in_cwd(
        dir_name: &str,
        config_filename: &str,
        tool_label: &str,
        merge_fn: fn(&mut serde_json::Value),
        remove_fn: fn(&mut serde_json::Value),
    ) -> Option<Self> {
        let cwd = env::current_dir().ok()?;
        Self::install_anchored(
            &cwd.join(dir_name),
            config_filename,
            tool_label,
            merge_fn,
            remove_fn,
            InvalidJsonPolicy::Replace,
        )
    }

    /// Home-dir anchor (user-scope config tools).
    pub(crate) fn install_in_home(
        dir_name: &str,
        config_filename: &str,
        tool_label: &str,
        merge_fn: fn(&mut serde_json::Value),
        remove_fn: fn(&mut serde_json::Value),
    ) -> Option<Self> {
        let home = home_dir_env()?;
        Self::install_anchored(
            &home.join(dir_name),
            config_filename,
            tool_label,
            merge_fn,
            remove_fn,
            InvalidJsonPolicy::Refuse,
        )
    }

    fn install_anchored(
        config_dir: &Path,
        config_filename: &str,
        tool_label: &str,
        merge_fn: fn(&mut serde_json::Value),
        remove_fn: fn(&mut serde_json::Value),
        invalid_json_policy: InvalidJsonPolicy,
    ) -> Option<Self> {
        if !paneflow_ipc_reachable() {
            sweep_orphan_hook_config(&config_dir.join(config_filename), remove_fn);
            return None;
        }
        Self::install_at(
            config_dir,
            config_filename,
            tool_label,
            merge_fn,
            remove_fn,
            invalid_json_policy,
        )
    }

    /// Testable inner - no IPC gate, mirrors [`HookConfigGuard::install_at`].
    pub(crate) fn install_at(
        config_dir: &Path,
        config_filename: &str,
        tool_label: &str,
        merge_fn: fn(&mut serde_json::Value),
        remove_fn: fn(&mut serde_json::Value),
        invalid_json_policy: InvalidJsonPolicy,
    ) -> Option<Self> {
        let (settings_path, created_dir, lease) = install_hook_config_file(
            config_dir,
            config_filename,
            tool_label,
            merge_fn,
            invalid_json_policy,
        )?;
        Some(Self {
            settings_path,
            config_dir: config_dir.to_path_buf(),
            created_dir,
            remove_fn,
            lease,
        })
    }
}

impl Drop for ManagedHookConfigGuard {
    fn drop(&mut self) {
        cleanup_hook_config_file(
            &self.settings_path,
            &self.config_dir,
            self.created_dir,
            self.remove_fn,
            &self.lease,
        );
    }
}

// ---------------------------------------------------------------------------
// TypeScript-plugin agents: Pi (auto-loaded extension) + OpenCode (declared
// plugin)
// ---------------------------------------------------------------------------

/// Basename of the bridge file both TS-plugin guards materialize. Ownership
/// detection and cleanup key off this exact name.
pub(crate) const PANEFLOW_TS_BASENAME: &str = "paneflow-status.ts";

/// Embedded TS sources (staged next to the crate; `include_str!` keeps the
/// shim free of any asset-loading machinery).
const PI_EXTENSION_SOURCE: &str = include_str!("../assets/pi-paneflow-status.ts");
const OPENCODE_PLUGIN_SOURCE: &str = include_str!("../assets/opencode-paneflow-status.ts");

/// RAII guard for Pi: drops `paneflow-status.ts` into the AUTO-LOADED
/// global extension dir `~/.pi/agent/extensions/` on install, deletes it on
/// drop. No config file to edit - Pi discovers `*.ts` there by itself. The
/// extension is inert outside Paneflow PTYs (it early-returns without
/// `PANEFLOW_SOCKET_PATH`), so the install/remove window racing a non-
/// Paneflow `pi` session is harmless.
pub(crate) struct PiExtensionGuard {
    ext_path: PathBuf,
    lease: HookLease,
}

impl PiExtensionGuard {
    pub(crate) fn install() -> Option<Self> {
        let home = home_dir_env()?;
        let ext_dir = home.join(".pi").join("agent").join("extensions");
        if !paneflow_ipc_reachable() {
            // Orphan sweep: a previous SIGKILL'd session never dropped.
            let ext_path = ext_dir.join(PANEFLOW_TS_BASENAME);
            if ext_path.exists() {
                let _ = with_config_lock(&ext_path, || {
                    let _ = std::fs::remove_file(&ext_path);
                    Ok(())
                });
            }
            return None;
        }
        Self::install_at(&ext_dir)
    }

    /// Testable inner - no IPC gate.
    pub(crate) fn install_at(ext_dir: &Path) -> Option<Self> {
        if config_dir_is_symlink(ext_dir) {
            return None;
        }
        std::fs::create_dir_all(ext_dir).ok()?;
        let ext_path = ext_dir.join(PANEFLOW_TS_BASENAME);
        let lease = HookLease::acquire(&ext_path)?;
        with_config_lock(&ext_path, || {
            write_text_atomic(&ext_path, PI_EXTENSION_SOURCE)
        })
        .ok()?;
        Some(Self { ext_path, lease })
    }
}

impl Drop for PiExtensionGuard {
    fn drop(&mut self) {
        if self.lease.release_and_is_last() {
            let _ = std::fs::remove_file(&self.ext_path);
        }
    }
}

/// OpenCode's config dir, honoring `$XDG_CONFIG_HOME` the way OpenCode
/// itself does, with the `~/.config` fallback.
fn opencode_config_dir() -> Option<PathBuf> {
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(xdg).join("opencode"));
    }
    home_dir_env().map(|h| h.join(".config").join("opencode"))
}

/// RAII guard for OpenCode: materializes the bridge plugin at
/// `<config>/plugins/paneflow-status.ts` AND declares it in the global
/// `opencode.json` `plugin` array (OpenCode has no auto-discovery dir - an
/// undeclared file is dead weight). Drop removes both.
///
/// The config file is OpenCode's PRIMARY user config, so unlike the
/// settings.local.json scaffold this guard never clobbers on parse failure:
/// unreadable JSON (or a `.jsonc`-only setup, which serde_json cannot
/// round-trip without destroying comments) skips the install - the user
/// keeps a working OpenCode, just without sidebar status (C4).
pub(crate) struct OpenCodePluginGuard {
    plugin_path: PathBuf,
    config_path: PathBuf,
    created_config: bool,
    lease: HookLease,
}

impl OpenCodePluginGuard {
    pub(crate) fn install() -> Option<Self> {
        let dir = opencode_config_dir()?;
        if !paneflow_ipc_reachable() {
            Self::sweep_orphan(&dir);
            return None;
        }
        Self::install_at(&dir)
    }

    /// Testable inner - no IPC gate. `dir` is the opencode config dir.
    pub(crate) fn install_at(dir: &Path) -> Option<Self> {
        let config_path = dir.join("opencode.json");
        // A `.jsonc`-only setup wins: opencode prefers it, and editing it
        // would strip the user's comments. Skip rather than degrade.
        if !config_path.exists() && dir.join("opencode.jsonc").exists() {
            return None;
        }

        let plugins_dir = dir.join("plugins");
        if config_dir_is_symlink(&plugins_dir) {
            return None;
        }
        std::fs::create_dir_all(&plugins_dir).ok()?;
        let plugin_path = plugins_dir.join(PANEFLOW_TS_BASENAME);
        let lease = HookLease::acquire(&plugin_path)?;
        with_config_lock(&plugin_path, || {
            write_text_atomic(&plugin_path, OPENCODE_PLUGIN_SOURCE)
        })
        .ok()?;
        let mut created_config = false;

        if with_config_lock(&config_path, || {
            let existing = std::fs::read_to_string(&config_path).ok();
            created_config = existing.is_none();
            let mut root: serde_json::Value = match existing {
                None => serde_json::json!({}),
                Some(content) => match serde_json::from_str(&content) {
                    Ok(v) => v,
                    Err(_) => {
                        // PRIMARY config - never overwrite what we can't parse.
                        eprintln!(
                            "paneflow-shim: {} is not parseable JSON; skipping \
                             OpenCode status plugin this session",
                            safe_path_display(&config_path)
                        );
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "invalid OpenCode JSON",
                        ));
                    }
                },
            };
            merge_opencode_plugin_entry(&mut root, &plugin_path.to_string_lossy());
            write_atomic(&config_path, &root)
        })
        .is_err()
        {
            let _ = std::fs::remove_file(&plugin_path);
            return None;
        }

        Some(Self {
            plugin_path,
            config_path,
            created_config,
            lease,
        })
    }

    /// Best-effort removal of a previous session's leftovers (no live IPC).
    fn sweep_orphan(dir: &Path) {
        let plugin_path = dir.join("plugins").join(PANEFLOW_TS_BASENAME);
        if plugin_path.exists() {
            let _ = with_config_lock(&plugin_path, || {
                let _ = std::fs::remove_file(&plugin_path);
                Ok(())
            });
        }
        let config_path = dir.join("opencode.json");
        if !config_path.exists() {
            return;
        }
        let _ = with_config_lock(&config_path, || {
            let Ok(content) = std::fs::read_to_string(&config_path) else {
                return Ok(());
            };
            let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&content) else {
                return Ok(());
            };
            let before = root.clone();
            remove_opencode_plugin_entry(&mut root);
            if root != before {
                let is_empty = root
                    .as_object()
                    .map(serde_json::Map::is_empty)
                    .unwrap_or(false);
                if is_empty {
                    let _ = std::fs::remove_file(&config_path);
                } else {
                    write_atomic(&config_path, &root)?;
                }
            }
            Ok(())
        });
    }
}

impl Drop for OpenCodePluginGuard {
    fn drop(&mut self) {
        let _ = with_config_lock(&self.config_path, || {
            if !self.lease.release_and_is_last() {
                return Ok(());
            }
            let _ = std::fs::remove_file(&self.plugin_path);
            let Ok(content) = std::fs::read_to_string(&self.config_path) else {
                return Ok(());
            };
            let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&content) else {
                return Ok(());
            };
            remove_opencode_plugin_entry(&mut root);
            let is_empty = root
                .as_object()
                .map(serde_json::Map::is_empty)
                .unwrap_or(false);
            if is_empty && self.created_config {
                let _ = std::fs::remove_file(&self.config_path);
            } else {
                write_atomic(&self.config_path, &root)?;
            }
            Ok(())
        });
    }
}

/// Idempotent insert of the plugin path into `opencode.json`'s `plugin`
/// array. Ownership detection is by file basename so an older absolute path
/// (different cache dir, moved home) still counts as ours and gets replaced
/// rather than duplicated.
pub(crate) fn merge_opencode_plugin_entry(root: &mut serde_json::Value, plugin_path: &str) {
    let root_obj = match root.as_object_mut() {
        Some(o) => o,
        None => {
            *root = serde_json::json!({});
            let Some(o) = root.as_object_mut() else {
                return;
            };
            o
        }
    };
    let entry = root_obj
        .entry("plugin")
        .or_insert_with(|| serde_json::json!([]));
    let Some(array) = entry.as_array_mut() else {
        return;
    };
    array.retain(|v| !is_paneflow_plugin_entry(v));
    array.push(serde_json::Value::String(plugin_path.to_owned()));
}

/// Removal counterpart - drops our entries, collapses an empty `plugin`
/// array so a fully-managed file can become empty and be deleted.
pub(crate) fn remove_opencode_plugin_entry(root: &mut serde_json::Value) {
    let Some(root_obj) = root.as_object_mut() else {
        return;
    };
    if let Some(array) = root_obj.get_mut("plugin").and_then(|v| v.as_array_mut()) {
        array.retain(|v| !is_paneflow_plugin_entry(v));
    }
    let empty = root_obj
        .get("plugin")
        .and_then(|v| v.as_array())
        .is_some_and(|a| a.is_empty());
    if empty {
        root_obj.remove("plugin");
    }
}

/// A `plugin` array entry is ours iff it is a string whose final path
/// component is [`PANEFLOW_TS_BASENAME`]. Tuple-form entries
/// (`["pkg", {...}]`) are always user-owned.
fn is_paneflow_plugin_entry(value: &serde_json::Value) -> bool {
    value.as_str().is_some_and(|s| {
        Path::new(s)
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == PANEFLOW_TS_BASENAME)
    })
}

// ---------------------------------------------------------------------------
// Grok (`~/.grok/hooks/paneflow.json` - dedicated merged hook file)
// ---------------------------------------------------------------------------

/// Grok Build hook events - PascalCase, Claude-compatible names.
/// `Notification` is skipped (Grok's stdin payload has no
/// `notification_type`, so the hook's whitelist would drop every frame);
/// approval prompts use `PermissionRequest`, which `paneflow-ai-hook`
/// normalizes to `notification_type: "permission_prompt"`.
/// `SessionStart`/`SessionEnd` are covered by the shim's universal lifecycle.
pub(crate) const GROK_HOOK_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Stop",
];

/// RAII guard for Grok: writes a DEDICATED `~/.grok/hooks/paneflow.json`
/// (Grok merges every `*.json` in that dir at discovery - global hooks are
/// always trusted) and deletes it on drop. The file is wholly ours, so
/// there is no read-modify-write of any user config at all; Grok's
/// documented behavior on a malformed hook file is skip-with-warning
/// (fail-open), so the downside risk is bounded to a log line.
pub(crate) struct GrokHookFileGuard {
    hook_path: PathBuf,
    lease: HookLease,
}

impl GrokHookFileGuard {
    pub(crate) fn install() -> Option<Self> {
        let home = home_dir_env()?;
        let hooks_dir = home.join(".grok").join("hooks");
        if !paneflow_ipc_reachable() {
            let hook_path = hooks_dir.join("paneflow.json");
            if hook_path.exists() {
                let _ = with_config_lock(&hook_path, || {
                    let _ = std::fs::remove_file(&hook_path);
                    Ok(())
                });
            }
            return None;
        }
        Self::install_at(&hooks_dir)
    }

    /// Testable inner - no IPC gate.
    pub(crate) fn install_at(hooks_dir: &Path) -> Option<Self> {
        if config_dir_is_symlink(hooks_dir) {
            return None;
        }
        std::fs::create_dir_all(hooks_dir).ok()?;
        let hook_path = hooks_dir.join("paneflow.json");
        let lease = HookLease::acquire(&hook_path)?;
        // Grok's hook schema is the Claude matcher-group shape verbatim
        // (timeout in seconds, `type: command`) but its public docs do not
        // document `commandWindows`, so use the strict one-command handler.
        let mut root = serde_json::json!({});
        merge_strict_matcher_hooks_for_events_with(&mut root, GROK_HOOK_EVENTS, plain_hook_handler);
        with_config_lock(&hook_path, || write_atomic(&hook_path, &root)).ok()?;
        Some(Self { hook_path, lease })
    }
}

impl Drop for GrokHookFileGuard {
    fn drop(&mut self) {
        if self.lease.release_and_is_last() {
            let _ = std::fs::remove_file(&self.hook_path);
        }
    }
}

// ---------------------------------------------------------------------------
// Hermes (`~/.hermes/config.yaml`, YAML - string-level marked block)
// ---------------------------------------------------------------------------

/// Markers bounding Paneflow's managed block in `~/.hermes/config.yaml`.
/// String-level append/strip (the Codex TOML-marker precedent): a serde
/// round-trip of the user's PRIMARY YAML config would destroy their
/// comments and formatting, which is never acceptable.
pub(crate) const HERMES_BLOCK_BEGIN: &str =
    "# >>> paneflow managed hooks (auto-installed; removed on session end) >>>";
pub(crate) const HERMES_BLOCK_END: &str = "# <<< paneflow managed hooks <<<";

/// Quote a string for a YAML double-quoted scalar (backslashes + quotes -
/// enough for command paths; the rest of the command is our own ASCII).
fn yaml_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The managed `hooks:` block for Hermes. Event mapping notes:
/// - Hermes has NO turn-end event, so `post_llm_call` maps to `Stop`: the
///   final LLM response of a turn correctly lands `Finished`, and a
///   mid-turn `Stop` is immediately re-promoted to `Thinking` by the next
///   `pre_tool_call`/`pre_llm_call` (the server's tool_use arm revives
///   Finished sessions by design). Worst case is a brief "done" flicker
///   between tool calls - self-healing, never stuck.
/// - `pre_approval_request` maps to the `PermissionRequest` argv, which the
///   hook translates to `ai.notification` with `notification_type:
///   permission_prompt` itself - Hermes' stdin payload doesn't carry the
///   field the plain `Notification` whitelist gates on.
/// - `on_session_start`/`on_session_end` are NOT registered: the shim's
///   universal `ai.exit`/`ai.session_end` already covers the lifecycle and
///   registering them would double-fire.
pub(crate) fn hermes_managed_block() -> String {
    let q = |event: &str| yaml_quote(&resolve_plain_hook_command(event));
    format!(
        "{HERMES_BLOCK_BEGIN}\n\
         hooks:\n\
         \x20 pre_llm_call:\n\
         \x20   - command: {}\n\
         \x20     timeout: 5\n\
         \x20 post_llm_call:\n\
         \x20   - command: {}\n\
         \x20     timeout: 5\n\
         \x20 pre_tool_call:\n\
         \x20   - command: {}\n\
         \x20     timeout: 5\n\
         \x20 post_tool_call:\n\
         \x20   - command: {}\n\
         \x20     timeout: 5\n\
         \x20 pre_approval_request:\n\
         \x20   - command: {}\n\
         \x20     timeout: 5\n\
         {HERMES_BLOCK_END}\n",
        q("UserPromptSubmit"),
        q("Stop"),
        q("PreToolUse"),
        q("PostToolUse"),
        q("PermissionRequest"),
    )
}

/// Strip the managed block (markers inclusive, plus the end marker's
/// trailing newline). `None` when no complete block is present.
pub(crate) fn strip_hermes_managed_block(content: &str) -> Option<String> {
    let begin = content.find(HERMES_BLOCK_BEGIN)?;
    let end_rel = content[begin..].find(HERMES_BLOCK_END)?;
    let mut end = begin + end_rel + HERMES_BLOCK_END.len();
    if content[end..].starts_with('\n') {
        end += 1;
    }
    let mut out = String::with_capacity(content.len() - (end - begin));
    out.push_str(&content[..begin]);
    out.push_str(&content[end..]);
    Some(out)
}

/// True iff the (block-stripped) YAML content already has a top-level
/// `hooks:` key. Appending a second `hooks:` mapping would be a duplicate
/// key - PyYAML-family loaders silently keep the LAST one, which would
/// CLOBBER the user's own hooks for the session. Refusing is the only safe
/// answer without a comment-preserving YAML rewriter.
fn yaml_has_top_level_hooks_key(content: &str) -> bool {
    content
        .lines()
        .any(|l| l.starts_with("hooks:") || l == "hooks")
}

/// RAII guard for Hermes: appends the marked `hooks:` block to
/// `~/.hermes/config.yaml` on install, strips it on drop. Also sets
/// `HERMES_ACCEPT_HOOKS=1` in the shim's own env (inherited by the child)
/// so Hermes' first-use consent prompt doesn't block a fresh session
/// mid-turn - scoped to sessions where Paneflow actually manages the
/// hooks, and `~/.hermes` is user-owned (same-UID writers could edit the
/// consent allowlist directly anyway).
pub(crate) struct HermesHookConfigGuard {
    config_path: PathBuf,
    created_file: bool,
    lease: HookLease,
}

impl HermesHookConfigGuard {
    pub(crate) fn install() -> Option<Self> {
        let home = home_dir_env()?;
        let hermes_dir = home.join(".hermes");
        let config_path = hermes_dir.join("config.yaml");
        if !paneflow_ipc_reachable() {
            Self::sweep_orphan(&config_path);
            return None;
        }
        let guard = Self::install_at(&hermes_dir)?;
        // Safe (not the Rust-2024 unsafe form) on this crate's 2021
        // edition; single-threaded here, before the child spawn.
        env::set_var("HERMES_ACCEPT_HOOKS", "1");
        Some(guard)
    }

    /// Testable inner - no IPC gate, no env mutation.
    pub(crate) fn install_at(hermes_dir: &Path) -> Option<Self> {
        if config_dir_is_symlink(hermes_dir) {
            return None;
        }
        std::fs::create_dir_all(hermes_dir).ok()?;
        let config_path = hermes_dir.join("config.yaml");
        let lease = HookLease::acquire(&config_path)?;

        let mut created_file = false;
        if with_config_lock(&config_path, || {
            let existing = std::fs::read_to_string(&config_path).ok();
            created_file = existing.is_none();
            let content = existing.unwrap_or_default();
            // Idempotent re-install: strip any block a previous session left.
            let base = strip_hermes_managed_block(&content).unwrap_or(content);

            if yaml_has_top_level_hooks_key(&base) {
                eprintln!(
                    "paneflow-shim: {} already defines a hooks: section; \
                     skipping Hermes status hooks this session (a duplicate \
                     YAML key would override yours)",
                    safe_path_display(&config_path)
                );
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "user Hermes config already has hooks",
                ));
            }

            let mut next = base;
            if !next.is_empty() && !next.ends_with('\n') {
                next.push('\n');
            }
            next.push_str(&hermes_managed_block());
            write_text_atomic(&config_path, &next)
        })
        .is_err()
        {
            return None;
        }

        Some(Self {
            config_path,
            created_file,
            lease,
        })
    }

    fn sweep_orphan(config_path: &Path) {
        if !config_path.exists() {
            return;
        }
        let _ = with_config_lock(config_path, || {
            let Ok(content) = std::fs::read_to_string(config_path) else {
                return Ok(());
            };
            if let Some(stripped) = strip_hermes_managed_block(&content) {
                if stripped.trim().is_empty() {
                    let _ = std::fs::remove_file(config_path);
                } else {
                    write_text_atomic(config_path, &stripped)?;
                }
            }
            Ok(())
        });
    }
}

impl Drop for HermesHookConfigGuard {
    fn drop(&mut self) {
        let _ = with_config_lock(&self.config_path, || {
            if !self.lease.release_and_is_last() {
                return Ok(());
            }
            let Ok(content) = std::fs::read_to_string(&self.config_path) else {
                return Ok(());
            };
            let Some(stripped) = strip_hermes_managed_block(&content) else {
                return Ok(());
            };
            if stripped.trim().is_empty() && self.created_file {
                let _ = std::fs::remove_file(&self.config_path);
            } else {
                write_text_atomic(&self.config_path, &stripped)?;
            }
            Ok(())
        });
    }
}

/// Returns `true` if `value` is a matcher-group object that PaneFlow owns.
/// Belt-and-suspenders: first checks the `_paneflow_managed` marker on the
/// outer wrapper, then falls back to scanning the inner `hooks` array for a
/// command whose basename matches `paneflow-ai-hook[.exe]` (legacy bare-name
/// or absolute-path form). The second pass catches the case where Claude
/// Code's own settings writer strips unknown fields, AND the case where a
/// previous shim version wrote a bare-name command (forward compatibility
/// with the migration to absolute paths).
pub(crate) fn is_paneflow_matcher_group(value: &serde_json::Value) -> bool {
    if value
        .get("_paneflow_managed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    value
        .get("hooks")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|inner| {
            inner.iter().any(|handler| {
                handler
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(is_paneflow_hook_command)
            })
        })
}

/// Atomic write via `NamedTempFile::persist`: create a tempfile in the same
/// directory (so `persist` can `rename` without crossing filesystems), write
/// the JSON, then rename over the target. This avoids torn reads if Claude
/// Code concurrently reads the file at prompt time.
pub(crate) fn write_atomic(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "settings path has no parent",
        )
    })?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut tmp, value).map_err(std::io::Error::other)?;
    // Trailing newline matches what most human editors leave behind and
    // keeps diffs clean when users inspect the merged file.
    std::io::Write::write_all(&mut tmp, b"\n")?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Codex hook config injection (US-006, Unix) -
//   `.codex/hooks.json` + `~/.codex/config.toml`
// ---------------------------------------------------------------------------

/// Codex hook events (per Codex docs as of April 2026). Notably `Notification`
/// is NOT a Codex hook event - Claude Code has one but Codex does not. The
/// `paneflow-ai-hook` binary (US-003) accepts `Notification` as a valid event
/// name, but it's only fired from Windows JSONL `error` events (see
/// `parse_codex_event`), never from this hooks.json registration.
///
/// Cross-platform as of June 2026: Codex now supports hooks on **Windows**
/// too (a `commandWindows` override field, and no `[features] hooks = true`
/// flag is required - hooks are on by default). Codex's `hooks.json` uses the
/// SAME matcher-group shape and the SAME event names as Claude Code, so the
/// Windows build registers these via [`merge_codex_hooks_win`] (a plain
/// `ManagedHookConfigGuard` over `.codex/hooks.json`), while Unix keeps
/// `CodexHookConfigGuard` (which additionally toggles the `config.toml`
/// feature flag still required there).
pub(crate) const CODEX_HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Stop",
];

/// Windows Codex hook merge/remove. Reuses the shared Claude-format
/// matcher-group helpers (Codex's `hooks.json` is byte-compatible) with the
/// Codex event set. Unlike `CodexHookConfigGuard`, no `config.toml` feature
/// flag is toggled - Codex enables hooks by default on Windows.
#[cfg(not(unix))]
pub(crate) fn merge_codex_hooks_win(root: &mut serde_json::Value) {
    merge_matcher_hooks_for_events_with(root, CODEX_HOOK_EVENTS, codex_windows_hook_handler);
}
#[cfg(not(unix))]
pub(crate) fn remove_codex_hooks_win(root: &mut serde_json::Value) {
    remove_matcher_hooks_for_events(root, CODEX_HOOK_EVENTS);
}

#[cfg(windows)]
fn codex_windows_hook_handler(event: &str) -> serde_json::Value {
    hook_handler(event)
}

/// Marker for the TOML comment placed above `hooks = true` in
/// `~/.codex/config.toml`. Cleanup scans for this literal line.
#[cfg(unix)]
pub(crate) const CODEX_TOML_MARKER: &str = "# _paneflow_managed: true";

/// Resolve `~/.codex/config.toml` using only std. The shim has no `dirs` dep
/// and `HOME` is universally set on Unix. Returns `None` on Windows builds
/// (unreachable at runtime - this function is `#[cfg(unix)]` - but kept for
/// documentation).
#[cfg(unix)]
pub(crate) fn codex_global_config_toml() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".codex").join("config.toml"))
}

/// RAII guard for Codex's project-level hooks.json + the global config.toml
/// feature flag. Both are populated on construction and reverted on drop.
#[cfg(unix)]
pub(crate) struct CodexHookConfigGuard {
    hooks_json_path: PathBuf,
    codex_dir: PathBuf,
    /// Whether the shim created `./.codex/`. Only rmdir on drop if we did.
    created_dir: bool,
    /// Absolute path to `~/.codex/config.toml` (if resolvable); `None` means
    /// no `HOME` - skip global-config work entirely.
    config_toml_path: Option<PathBuf>,
    /// Whether we appended the feature-flag block on install. Drop undoes it
    /// only if we did; never touches a pre-existing user-authored flag.
    added_feature_flag: bool,
    lease: HookLease,
}

#[cfg(unix)]
impl CodexHookConfigGuard {
    /// Install in the project CWD. Mirrors `HookConfigGuard::install`: same
    /// orphan-sweep gate when `PANEFLOW_SOCKET_PATH` is unreachable, then
    /// delegate to `install_at`.
    pub(crate) fn install() -> Option<Self> {
        let cwd = env::current_dir().ok()?;
        let codex_dir = cwd.join(".codex");
        if !paneflow_ipc_reachable() {
            sweep_orphan_hook_config(&codex_dir.join("hooks.json"), remove_codex_hooks);
            return None;
        }
        Self::install_at(&codex_dir, codex_global_config_toml().as_deref())
    }

    /// Testable inner. `config_toml_path` is the absolute path to the global
    /// `config.toml` (usually `~/.codex/config.toml`); pass `None` to skip
    /// the feature-flag step entirely (used by tests that don't want to
    /// pollute the test runner's home dir).
    pub(crate) fn install_at(codex_dir: &Path, config_toml_path: Option<&Path>) -> Option<Self> {
        let (hooks_json_path, created_dir, lease) = install_hook_config_file(
            codex_dir,
            "hooks.json",
            "Codex",
            merge_codex_hooks,
            InvalidJsonPolicy::Replace,
        )?;

        // Codex-specific extra: enable the `hooks = true` feature flag
        // in `~/.codex/config.toml`. Failure here is non-fatal - the user
        // can enable it manually and the rest of the hook config is in
        // place. Runs AFTER the shared install so the per-project hooks.json
        // is on disk before we touch the global config.
        let added_feature_flag = config_toml_path
            .and_then(enable_codex_feature_flag)
            .unwrap_or(false);

        Some(Self {
            hooks_json_path,
            codex_dir: codex_dir.to_path_buf(),
            created_dir,
            config_toml_path: config_toml_path.map(Path::to_path_buf),
            added_feature_flag,
            lease,
        })
    }
}

#[cfg(unix)]
impl Drop for CodexHookConfigGuard {
    fn drop(&mut self) {
        // Roll back the feature flag first so if something below fails, the
        // global config is left in a consistent state.
        if self.added_feature_flag {
            if let Some(p) = self.config_toml_path.as_deref() {
                disable_codex_feature_flag(p);
            }
        }
        cleanup_hook_config_file(
            &self.hooks_json_path,
            &self.codex_dir,
            self.created_dir,
            remove_codex_hooks,
            &self.lease,
        );
    }
}

#[cfg(unix)]
pub(crate) fn merge_codex_hooks(root: &mut serde_json::Value) {
    if !root.is_object() {
        *root = serde_json::json!({});
    }
    let Some(root_obj) = root.as_object_mut() else {
        return;
    };
    let hooks_entry = root_obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks_entry.is_object() {
        *hooks_entry = serde_json::json!({});
    }
    let Some(hooks_obj) = hooks_entry.as_object_mut() else {
        return;
    };

    for event in CODEX_HOOK_EVENTS {
        let entry = hooks_obj
            .entry(*event)
            .or_insert_with(|| serde_json::json!([]));
        let Some(array) = entry.as_array_mut() else {
            continue;
        };
        if array.iter().any(is_paneflow_matcher_group) {
            continue;
        }
        array.push(serde_json::json!({
            "_paneflow_managed": true,
            "hooks": [
                {
                    "type": "command",
                    "command": resolve_hook_command(event),
                    "timeout": 5,
                }
            ]
        }));
    }
}

#[cfg(unix)]
pub(crate) fn remove_codex_hooks(root: &mut serde_json::Value) {
    let Some(root_obj) = root.as_object_mut() else {
        return;
    };
    let Some(hooks_obj) = root_obj.get_mut("hooks").and_then(|v| v.as_object_mut()) else {
        return;
    };
    for event in CODEX_HOOK_EVENTS {
        let Some(array) = hooks_obj.get_mut(*event).and_then(|v| v.as_array_mut()) else {
            continue;
        };
        array.retain(|m| !is_paneflow_matcher_group(m));
    }
    hooks_obj.retain(|_k, v| v.as_array().is_none_or(|a| !a.is_empty()));
    if hooks_obj.is_empty() {
        root_obj.remove("hooks");
    }
}

/// Append `[features]\nhooks = true` (with a marker comment) to the
/// file at `path` iff: (a) the file doesn't already have `hooks = true` (or
/// the pre-Codex-0.130 alias `codex_hooks = true`) anywhere, AND (b) there's
/// no existing `[features]` section - appending would create a duplicate-
/// section TOML error, so we abstain in that case and warn.
///
/// Returns `Some(true)` if we modified the file, `Some(false)` if the flag
/// was already present (no-op), `None` if we aborted due to a conflict or
/// I/O error.
/// US-027: RAII advisory `flock` guard. Serializes the codex `[features]`
/// read-modify-write across concurrent shims so two near-simultaneous `codex`
/// launches can't both append `[features]` (→ duplicate-section invalid TOML).
#[cfg(unix)]
pub(crate) struct FlockGuard {
    file: std::fs::File,
}

#[cfg(unix)]
impl FlockGuard {
    /// Acquire an exclusive advisory lock on `lock_path` (creating it),
    /// blocking until granted. Returns `None` if the lock file can't be opened
    /// or locked - callers then proceed best-effort (no worse than no lock).
    fn acquire(lock_path: &Path) -> Option<Self> {
        use std::os::unix::io::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .ok()?;
        // SAFETY: `flock` on a valid owned fd; `LOCK_EX` blocks until the lock
        // is exclusively held. The lock is released in `Drop`.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        (rc == 0).then_some(Self { file })
    }
}

#[cfg(unix)]
impl Drop for FlockGuard {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // SAFETY: releasing our own advisory lock; ignore teardown errors.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(unix)]
pub(crate) fn enable_codex_feature_flag(path: &Path) -> Option<bool> {
    // US-027: hold an exclusive flock across the whole read-modify-write.
    // `write_text_atomic` guarantees byte-atomicity but NOT serialization, so
    // without this two concurrent shims both read a config lacking
    // `[features]`, both pass the guard below, and both append the section.
    // A failed lock acquisition degrades to the prior (unserialized) behavior.
    let _lock = path.parent().and_then(|dir| {
        let _ = std::fs::create_dir_all(dir);
        FlockGuard::acquire(&dir.join(".paneflow-codex.lock"))
    });

    let existing = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            eprintln!(
                "paneflow-shim: cannot read {} ({e}); leaving Codex `hooks` feature flag alone",
                safe_path_display(path)
            );
            return None;
        }
    };

    if has_hooks_flag(&existing) {
        return Some(false);
    }
    if has_features_section(&existing) {
        eprintln!(
            "paneflow-shim: {} already has a [features] section without `hooks`; skipping auto-enable (add `hooks = true` there manually to enable Codex hooks)",
            safe_path_display(path)
        );
        return None;
    }

    // Safe append: ensure a trailing newline, then add our block.
    let mut next = existing.clone();
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    if !next.is_empty() {
        next.push('\n');
    }
    next.push_str(CODEX_TOML_MARKER);
    next.push('\n');
    next.push_str("[features]\nhooks = true\n");

    // Ensure the parent dir exists - the Codex config dir may be absent
    // if the user has never run Codex before, but the shim's own invocation
    // implies the binary is installed somewhere.
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if let Err(e) = write_text_atomic(path, &next) {
        eprintln!(
            "paneflow-shim: cannot write {} ({e}); Codex `hooks` feature flag not set",
            safe_path_display(path)
        );
        return None;
    }
    Some(true)
}

#[cfg(unix)]
pub(crate) fn disable_codex_feature_flag(path: &Path) {
    let Ok(existing) = std::fs::read_to_string(path) else {
        return;
    };
    let Some(cleaned) = strip_codex_feature_block(&existing) else {
        // Marker not found, or the managed block was edited by the user.
        // Both cases: don't touch the file. No stderr noise - next shim
        // invocation's idempotent install will converge state eventually.
        return;
    };
    // If the file is now empty or only whitespace, delete it - we either
    // created it from nothing or the flag was the only content.
    let result = if cleaned.trim().is_empty() {
        std::fs::remove_file(path)
    } else {
        write_text_atomic(path, &cleaned)
    };
    if let Err(e) = result {
        // Phase 7 security audit MEDIUM #12: if cleanup write fails, the
        // managed block stays in `~/.codex/config.toml` and subsequent
        // shim runs see `hooks = true` already set, so they never
        // retry the cleanup. Surface the failure so the user can remove
        // the block manually. Known silent-failure mode.
        eprintln!(
            "paneflow-shim: could not revert {} ({e}); please remove the `{}` block manually",
            safe_path_display(path),
            CODEX_TOML_MARKER
        );
    }
}

/// Atomic text write via `tempfile::NamedTempFile::persist`. Mirrors the
/// existing `write_atomic` but for raw strings (not serde_json::Value).
/// Phase 7 security audit MEDIUM #3: concurrent shims both calling
/// `fs::write` on `~/.codex/config.toml` can produce torn bytes; rename
/// swap is the fix.
///
/// Cross-platform: also called by the Pi / OpenCode / Hermes guards above,
/// which compile on Windows too - keep this ungated (`tempfile` +
/// `std::io::Write` are cross-platform).
pub(crate) fn write_text_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "config path has no parent",
        )
    })?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut tmp, content.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn has_hooks_flag(content: &str) -> bool {
    // Codex 0.130 renamed `codex_hooks` -> `hooks`; accept both so the shim
    // stays silent for configs from either era.
    content.lines().any(|line| {
        let l = line.trim_start();
        if l.starts_with('#') {
            return false;
        }
        let stripped = l.split_once('#').map(|(k, _)| k).unwrap_or(l);
        let stripped = stripped.trim_end();
        match stripped.split_once('=') {
            Some((key, value)) => {
                let k = key.trim();
                (k == "hooks" || k == "codex_hooks") && value.trim() == "true"
            }
            None => false,
        }
    })
}

#[cfg(unix)]
pub(crate) fn has_features_section(content: &str) -> bool {
    content.lines().any(|line| line.trim() == "[features]")
}

/// Remove the exact 3-line PaneFlow block (marker comment + `[features]` +
/// `hooks = true`, or the legacy `codex_hooks = true` for installs predating
/// Codex 0.130) from the file content. Returns the cleaned content if the
/// block was found and removed; `None` if the marker wasn't present.
#[cfg(unix)]
pub(crate) fn strip_codex_feature_block(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let marker_idx = lines.iter().position(|l| l.trim() == CODEX_TOML_MARKER)?;
    // We expect the marker to be followed by exactly:
    //   `[features]`
    //   `hooks = true` (or the legacy `codex_hooks = true`)
    // Remove those three lines; anything else means the file was edited
    // and we should bail to avoid clobbering user content.
    let tail = lines.get(marker_idx + 1..marker_idx + 3)?;
    // Exact-match both lines: `starts_with("hooks")` would also strip a
    // hypothetical future `hooks_experimental = ...` line that happened
    // to sit in the managed block. Exact match fails closed - safer to
    // leave the block untouched than over-delete. Accept either key name
    // because installs predating the Codex 0.130 rename wrote `codex_hooks`.
    let second = tail[1].trim();
    if tail[0].trim() != "[features]"
        || (second != "hooks = true" && second != "codex_hooks = true")
    {
        return None;
    }

    // Reconstruct: before marker + (possibly one blank line preceding the
    // marker that we added) + after the 3-line block.
    let mut head_end = marker_idx;
    // Strip a single trailing blank line we may have added before the marker
    // so we don't accumulate blanks across install/uninstall cycles.
    if head_end > 0 && lines[head_end - 1].is_empty() {
        head_end -= 1;
    }

    let mut out = String::new();
    for line in &lines[..head_end] {
        out.push_str(line);
        out.push('\n');
    }
    for line in &lines[marker_idx + 3..] {
        out.push_str(line);
        out.push('\n');
    }
    // Preserve original EOF-newline behavior: if the input had no trailing
    // newline and we removed content from the tail, trim one.
    if !content.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Codex Windows JSONL fallback (US-006, non-Unix)
// ---------------------------------------------------------------------------
//
// Codex hook machinery is disabled on Windows as of April 2026. Instead, the
// shim spawns `codex exec --json`, tees the child's stdout (so the user sees
// every byte unchanged), and dispatches `paneflow-ai-hook <Event>` subprocesses
// on recognized NDJSON events. Only the `exec` subcommand supports `--json`;
// interactive `codex` invocations on Windows pass through without tee - they
// simply won't produce sidebar state updates (documented regression).

#[cfg(not(unix))]
/// Decide whether to tee + inject `--json`. The `exec` subcommand is the only
/// Codex mode that emits NDJSON; other invocations (`codex`, `codex resume`,
/// `codex --help`) must pass through unmodified.
///
/// Scans for `"exec"` anywhere in argv rather than only at position 0, so
/// that global flags preceding the subcommand (e.g.,
/// `codex --config cfg.toml exec prompt`) are still detected. Known false-
/// positive edge: a user who passes `exec` as the VALUE of a preceding
/// value-taking flag (e.g., `codex --profile exec`) would get `--json`
/// injected; Codex then errors clearly and the user can bypass the shim
/// by invoking the real binary directly. Accepting this trade because
/// missing the tee on `codex --config X exec` is a silent regression,
/// while the false-positive is loud and bypassable.
pub(crate) fn rewrite_codex_args(args: &[OsString]) -> (Vec<OsString>, bool) {
    let Some(exec_idx) = args.iter().position(|a| a == "exec") else {
        return (args.to_vec(), false);
    };
    let mut rewritten = Vec::with_capacity(args.len() + 1);
    rewritten.extend_from_slice(&args[..=exec_idx]);
    rewritten.push(OsString::from("--json"));
    rewritten.extend(args[exec_idx + 1..].iter().cloned());
    (rewritten, true)
}

/// The full catalog of Codex NDJSON `"type"` discriminators documented at
/// <https://developers.openai.com/codex/noninteractive> as of April 2026.
/// `parse_codex_event`'s match arms MUST exhaustively cover every string in
/// this list so the schema-pin test can detect drift. When Codex adds a new
/// event type, add it to BOTH this const AND to a `match` arm in
/// `parse_codex_event` (even if the arm is just `None`).
///
/// Test-only: the runtime `parse_codex_event` enumerates the strings inline
/// rather than referencing this const, so without `cfg(test)` the const is
/// dead code under `-D warnings` on Windows non-test builds.
#[cfg(all(test, not(unix)))]
pub(crate) const KNOWN_CODEX_EVENT_TYPES: &[&str] = &[
    "thread.started",
    "turn.started",
    "turn.completed",
    "turn.failed",
    "item.started",
    "item.completed",
    "error",
];

/// Map a Codex NDJSON line to a `paneflow-ai-hook` event name. Returns
/// `None` for unrecognized / sub-event / malformed lines so the caller can
/// log-and-skip without breaking the tee loop.
#[cfg(not(unix))]
pub(crate) fn parse_codex_event(line: &str) -> Option<&'static str> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let event_type = value.get("type")?.as_str()?;
    match event_type {
        "turn.started" => Some("UserPromptSubmit"),
        "turn.completed" => Some("Stop"),
        "error" => Some("Notification"),
        // Known-but-unmapped events: explicit arms (not a catch-all) so
        // adding a new Codex event type without touching this match arm
        // requires a code change, which the schema-pin test will then
        // either accept (if the fixture is updated) or fail loudly.
        "thread.started" | "turn.failed" | "item.started" | "item.completed" => None,
        // Truly unknown (new Codex event type we don't recognize yet):
        // silently skip. The schema-pin test cross-checks this against
        // KNOWN_CODEX_EVENT_TYPES so drift is surfaced at dev time.
        _ => None,
    }
}

// EP-004 US-010: returns the raw agent exit code alongside, mirroring
// `run_real` - `main` emits `ai.exit` from it. `None` on spawn/wait failure.
#[cfg(not(unix))]
pub(crate) fn run_codex_with_jsonl_tee(path: &Path, args: &[OsString]) -> (ExitCode, Option<i32>) {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let mut child = match std::process::Command::new(path)
        .args(args)
        .envs(env::vars_os())
        .env("PANEFLOW_AI_TOOL", "codex")
        .env("PANEFLOW_AI_PID", std::process::id().to_string())
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "paneflow-shim: spawn '{}' failed: {e}",
                safe_path_display(path)
            );
            return (ExitCode::from(127), None);
        }
    };

    // Resolve the sibling hook binary ONCE before the tee loop. Bare-name
    // `Command::new("paneflow-ai-hook")` works on Windows only when the
    // shim cache dir is already on `%PATH%`, which is a per-PTY contract
    // not guaranteed at the moment this function spawns its first hook.
    // Resolving by absolute path matches every other call site in this
    // crate (`send_interrupt_stop`, `notify_session_end`).
    let hook_path = locate_sibling_hook_binary();

    // Take stdout so we can tee it. If the child was spawned without piped
    // stdout somehow, skip the tee entirely and fall through to wait().
    if let Some(stdout) = child.stdout.take() {
        // Reader thread: keeps the child's stdout drained (prevents pipe
        // fills) and dispatches hook events as they arrive.
        let tee_handle = std::thread::spawn(move || {
            let mut out = std::io::stdout().lock();
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                // Echo the line verbatim, preserving Codex's NDJSON so any
                // downstream tooling (jq, log-tailers) still works.
                let _ = std::io::Write::write_all(&mut out, line.as_bytes());
                let _ = std::io::Write::write_all(&mut out, b"\n");
                let _ = std::io::Write::flush(&mut out);

                if let Some(event) = parse_codex_event(&line) {
                    // Fire-and-forget: we don't wait on the hook binary so
                    // the tee loop keeps pace with Codex's output. This
                    // function is compiled only on non-Unix (`#[cfg(not
                    // (unix))]`) - on Windows, dropping the `Child` handle
                    // releases the OS handle and the subprocess runs to
                    // completion independently, with the OS reaping it
                    // when it exits. No zombies, no cleanup required.
                    //
                    // Known limitation (Phase 7 audit, MEDIUM #8): there is
                    // no rate limit. If Codex ever emits thousands of events
                    // per second, the shim would spawn thousands of
                    // subprocesses per second. Typical Codex output is a
                    // few events/sec, so this is tolerated without state.
                    if let Some(ref hp) = hook_path {
                        let _ = std::process::Command::new(hp)
                            .arg(event)
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .spawn();
                    }
                }
            }
        });
        // Join the reader thread AFTER wait() so all buffered output makes
        // it to the user before we exit.
        let status = child.wait();
        let _ = tee_handle.join();
        match status {
            Ok(s) => (
                exit_code_from_status(&s),
                Some(raw_exit_code_from_status(&s)),
            ),
            Err(e) => {
                eprintln!(
                    "paneflow-shim: wait on '{}' failed: {e}",
                    safe_path_display(path)
                );
                (ExitCode::from(127), None)
            }
        }
    } else {
        match child.wait() {
            Ok(s) => (
                exit_code_from_status(&s),
                Some(raw_exit_code_from_status(&s)),
            ),
            Err(e) => {
                eprintln!(
                    "paneflow-shim: wait on '{}' failed: {e}",
                    safe_path_display(path)
                );
                (ExitCode::from(127), None)
            }
        }
    }
}

#[cfg(test)]
mod hooks_tests {
    #[cfg(not(windows))]
    use super::shell_program_path;
    use super::{
        command_program_token, display_hook_program, settings_managed_hook_state,
        PersistentHookState,
    };
    // Only the `#[cfg(windows)]` round-trip test below references this; gating
    // the import keeps the non-Windows test build warning-free under `-D warnings`.
    #[cfg(windows)]
    use super::is_paneflow_hook_command;
    use super::reachable_from_socket_env;
    use super::settings_has_managed_hook;
    use serde_json::json;
    use std::ffi::OsStr;
    use std::path::Path;

    /// Regression (prd-windows-port): the hook `command` string is executed by
    /// the agent through a shell. On Windows Claude Code uses bash, which
    /// de-escapes `\` and mangled `C:\Users\…\paneflow-ai-hook.exe` into
    /// `C:Users…` → "command not found", so the hook never fired and the
    /// sidebar stayed dead. Forward slashes survive bash AND stay detectable.
    #[cfg(windows)]
    #[test]
    fn windows_hook_program_uses_forward_slashes_and_stays_detectable() {
        let p =
            Path::new(r"C:\Users\Arthur\AppData\Local\paneflow-dev\bin\0.4.4\paneflow-ai-hook.exe");
        let rendered = display_hook_program(p);
        assert!(
            !rendered.contains('\\'),
            "no backslashes (bash mangles them): {rendered}"
        );
        assert!(
            rendered.contains('/'),
            "forward slashes expected: {rendered}"
        );
        // The full command must remain recognized for idempotent merge + cleanup.
        let cmd = format!("{rendered} Stop");
        assert!(
            is_paneflow_hook_command(&cmd),
            "forward-slash command must stay detectable: {cmd}"
        );
        // The legacy backslash form (left by an older shim) must ALSO still be
        // detected so cleanup removes it.
        assert!(is_paneflow_hook_command(
            r"C:\Users\Arthur\AppData\Local\paneflow-dev\bin\0.4.4\paneflow-ai-hook.exe Stop"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_powershell_hook_command_is_shell_safe_for_program_files() {
        let p = Path::new(r"C:\Program Files\PaneFlow\bin\paneflow-ai-hook.exe");
        let cmd = super::windows_powershell_hook_command(p, "PreToolUse");
        assert_eq!(
            cmd,
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \"& 'C:/Program Files/PaneFlow/bin/paneflow-ai-hook.exe' PreToolUse\""
        );
        assert!(
            !cmd.contains('\\'),
            "forward slashes keep the hook path stable across Windows shells: {cmd}"
        );
        assert!(
            is_paneflow_hook_command(&cmd),
            "PowerShell-wrapped hook command must stay detectable: {cmd}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_strict_hook_configs_use_shell_safe_command() {
        use super::{is_paneflow_matcher_group, merge_codebuddy_hooks, CLAUDE_HOOK_EVENTS};

        let mut root = json!({});
        merge_codebuddy_hooks(&mut root);
        for event in CLAUDE_HOOK_EVENTS {
            let arr = root["hooks"][*event]
                .as_array()
                .unwrap_or_else(|| panic!("missing event {event}"));
            assert_eq!(arr.len(), 1, "{event}: exactly one paneflow entry");
            assert!(is_paneflow_matcher_group(&arr[0]), "{event}: detectable");
            let handler = &arr[0]["hooks"][0];
            let cmd = handler["command"].as_str().unwrap();
            assert!(
                cmd.starts_with("powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \"& '"),
                "{event}: strict command must run through PowerShell explicitly, got {cmd}"
            );
            assert!(
                cmd.ends_with(&format!(" {event}\"")),
                "{event}: command must preserve the event arg, got {cmd}"
            );
            assert!(
                handler.get("commandWindows").is_none(),
                "{event}: strict clone schema must not grow commandWindows"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn powershell_single_quote_escaping_doubles_apostrophes() {
        assert_eq!(
            super::powershell_single_quoted("C:/Users/O'Neil/paneflow-ai-hook.exe"),
            "'C:/Users/O''Neil/paneflow-ai-hook.exe'"
        );
    }

    /// Unix path rendering is unchanged (no separator rewrite).
    #[cfg(unix)]
    #[test]
    fn unix_hook_program_is_unchanged() {
        let p = Path::new("/home/u/.cache/paneflow/bin/0.4.4/paneflow-ai-hook");
        assert_eq!(
            display_hook_program(p),
            "/home/u/.cache/paneflow/bin/0.4.4/paneflow-ai-hook"
        );
    }

    /// Windows Codex hooks (June 2026): `.codex/hooks.json` must carry one
    /// paneflow matcher-group per Codex event, with a PowerShell-safe generic
    /// command and Windows override - and must NOT register `Notification`
    /// (not a Codex event). Round-trips to empty on removal.
    #[cfg(not(unix))]
    #[test]
    fn codex_win_merge_writes_shell_safe_matcher_groups() {
        use super::{
            is_paneflow_matcher_group, merge_codex_hooks_win, remove_codex_hooks_win,
            CODEX_HOOK_EVENTS,
        };
        let mut root = json!({});
        merge_codex_hooks_win(&mut root);
        for event in CODEX_HOOK_EVENTS {
            let arr = root["hooks"][*event]
                .as_array()
                .unwrap_or_else(|| panic!("missing event {event}"));
            assert_eq!(arr.len(), 1, "{event}: exactly one paneflow entry");
            assert!(is_paneflow_matcher_group(&arr[0]), "{event}: detectable");
            let handler = &arr[0]["hooks"][0];
            let cmd = handler["command"].as_str().unwrap();
            assert!(
                cmd.starts_with("powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \"& '"),
                "{event}: command must run through PowerShell explicitly, got {cmd}"
            );
            assert!(
                cmd.ends_with(&format!(" {event}\"")),
                "{event}: command must preserve the event arg, got {cmd}"
            );
            assert!(
                cmd.contains("paneflow-ai-hook"),
                "{event}: command must invoke the ai-hook binary, got {cmd}"
            );
            let mut marker_stripped = arr[0].clone();
            marker_stripped
                .as_object_mut()
                .unwrap()
                .remove("_paneflow_managed");
            assert!(
                is_paneflow_matcher_group(&marker_stripped),
                "{event}: command fallback must remain detectable if the marker is stripped"
            );
            let win_cmd = handler["commandWindows"].as_str().unwrap();
            assert_eq!(
                win_cmd, cmd,
                "{event}: commandWindows must stay as shell-safe as command"
            );
        }
        assert!(
            root["hooks"].get("Notification").is_none(),
            "Notification is not a Codex hook event"
        );
        // Idempotent + clean removal.
        merge_codex_hooks_win(&mut root);
        for event in CODEX_HOOK_EVENTS {
            assert_eq!(root["hooks"][*event].as_array().unwrap().len(), 1);
        }
        remove_codex_hooks_win(&mut root);
        assert_eq!(root, json!({}));
    }

    #[test]
    fn unset_or_empty_socket_is_unreachable() {
        // No PaneFlow PTY → no env var → not reachable (and the callers then
        // sweep any orphan hook config).
        assert!(!reachable_from_socket_env(None));
        assert!(!reachable_from_socket_env(Some(OsStr::new(""))));
    }

    /// Regression (prd-windows-port): on Windows `$PANEFLOW_SOCKET_PATH` is a
    /// named pipe. The former `Path::exists()` probe opened a client
    /// connection that consumed the server's pending pipe instance and lost
    /// the `ERROR_PIPE_BUSY` race ~87% of the time against the live accept
    /// loop, so the shim skipped hook-config install and the sidebar agent
    /// status never updated. Presence of the PaneFlow-set env var is now
    /// authoritative on Windows - no destructive probe.
    #[cfg(windows)]
    #[test]
    fn windows_named_pipe_env_is_reachable_without_probing() {
        assert!(reachable_from_socket_env(Some(OsStr::new(
            r"\\.\pipe\paneflow"
        ))));
        assert!(reachable_from_socket_env(Some(OsStr::new(
            r"\\.\pipe\paneflow-dev"
        ))));
    }

    /// On Unix the probe stays a passive `stat(2)`: a non-existent path is
    /// unreachable; a real filesystem node is reachable.
    #[cfg(unix)]
    #[test]
    fn unix_uses_passive_filesystem_probe() {
        assert!(!reachable_from_socket_env(Some(OsStr::new(
            "/nonexistent/paneflow/paneflow.sock"
        ))));
        let dir = tempfile::TempDir::new().unwrap();
        let f = dir.path().join("paneflow.sock");
        std::fs::File::create(&f).unwrap();
        assert!(reachable_from_socket_env(Some(f.as_os_str())));
    }

    // US-018: the shim defers to a persistent hook installed by
    // `paneflow hooks setup`. Detection must recognize the byte-identical
    // managed shape (marker or ai-hook command) and must NOT trip on a foreign
    // user hook - otherwise the shim would wrongly skip its ephemeral injection
    // (false positive) or double-fire (false negative).
    #[test]
    fn settings_has_managed_hook_detects_managed_and_ignores_foreign() {
        // Marker-tagged managed group (what the engine writes).
        let managed = json!({
            "hooks": { "Stop": [ {
                "_paneflow_managed": true,
                "hooks": [ { "type": "command", "command": "/bin/paneflow-ai-hook Stop", "timeout": 5 } ]
            } ] }
        });
        assert!(settings_has_managed_hook(&managed));

        // Marker stripped by Claude's writer → fallback to the ai-hook basename.
        let by_command = json!({
            "hooks": { "Notification": [ {
                "hooks": [ { "type": "command", "command": "/x/paneflow-ai-hook Notification" } ]
            } ] }
        });
        assert!(settings_has_managed_hook(&by_command));

        // A foreign user hook must not be mistaken for a Paneflow-managed one.
        let foreign = json!({
            "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "my-own-hook" } ] } ] }
        });
        assert!(!settings_has_managed_hook(&foreign));

        // No hooks at all.
        assert!(!settings_has_managed_hook(&json!({})));
    }

    #[test]
    fn persistent_hook_state_requires_an_existing_program() {
        let stale = json!({
            "hooks": { "Stop": [ {
                "_paneflow_managed": true,
                "hooks": [ { "type": "command", "command": "/definitely/missing/paneflow-ai-hook Stop" } ]
            } ] }
        });
        assert!(matches!(
            settings_managed_hook_state(&stale),
            PersistentHookState::Stale { command: Some(_) }
        ));

        let dir = tempfile::TempDir::new().unwrap();
        let hook_name = if cfg!(windows) {
            "paneflow-ai-hook.exe"
        } else {
            "paneflow-ai-hook"
        };
        let hook_path = dir.path().join(hook_name);
        std::fs::File::create(&hook_path).unwrap();
        let alive = json!({
            "hooks": { "Stop": [ {
                "_paneflow_managed": true,
                "hooks": [ { "type": "command", "command": format!("{} Stop", hook_path.display()) } ]
            } ] }
        });
        assert!(matches!(
            settings_managed_hook_state(&alive),
            PersistentHookState::Alive { .. }
        ));
    }

    #[test]
    fn command_program_token_handles_quoted_hook_paths() {
        assert_eq!(
            command_program_token("  '/tmp/with space/paneflow-ai-hook' Stop").as_deref(),
            Some("/tmp/with space/paneflow-ai-hook")
        );
        assert_eq!(
            command_program_token("'a'\\''b/paneflow-ai-hook' Stop").as_deref(),
            Some("a'b/paneflow-ai-hook")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn shell_program_path_quotes_paths_that_shell_would_split() {
        let path = Path::new("/tmp/Application Support/paneflow-ai-hook");
        let command = format!("{} Stop", shell_program_path(path));

        assert_eq!(
            command_program_token(&command).as_deref(),
            Some("/tmp/Application Support/paneflow-ai-hook")
        );
        assert!(
            super::is_paneflow_hook_command(&command),
            "quoted hook command must remain detectable: {command}"
        );
    }
}
