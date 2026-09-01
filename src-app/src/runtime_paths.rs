//! Resolve the PaneFlow runtime directory with a macOS-aware fallback chain,
//! and enforce the `sockaddr_un.sun_path` length limit (macOS: 104 bytes,
//! Linux: 108 - we use the smaller ceiling so a path built here works on both
//! platforms without a second guard at bind time).
//!
//! Public helpers:
//! - `ipc::start_server` consumes `socket_path()` for the main JSON-RPC socket,
//! - `terminal::paneflow_socket_path` propagates the same path as the
//!   `PANEFLOW_SOCKET_PATH` env var passed into each PTY child shell.
//!
//! Keeping the chain in one place prevents the two sites from drifting -
//! a difference in one branch would silently break IPC on macOS
//! without any visible error.
//!
//! US-013 removed the former third consumer (the AI-hook wrapper-scripts
//! bin-dir helper) along with its call sites - the extraction targets
//! never existed in the embed set, so the helper and its PATH-injection
//! caller were dead code.
//!
//! `PANEFLOW_SOCKET_PATH` overrides the computed path on every platform so
//! isolated debug/test instances and panes launched from a running instance
//! agree on the exact IPC endpoint. Without this, clients can point at one pipe
//! while the server keeps binding the default one.
//!
//! US-009 (prd-windows-port.md) - on Windows, `socket_path` falls back to the
//! named pipe path `\\.\pipe\paneflow` (or `paneflow-dev` in debug). The
//! XDG/TMPDIR chain and sun_path guard remain Unix-only.

use std::path::{Path, PathBuf};

/// macOS `sockaddr_un.sun_path` is `[c_char; 104]`. Linux allows 108, but
/// using the smaller ceiling keeps paths portable across both targets.
/// Unused on Windows (named pipes are limited to 256 chars, well above
/// anything we compose).
#[cfg(unix)]
pub(crate) const MAX_SOCKET_PATH_BYTES: usize = 104;

/// Application directory namespace. Switches to `paneflow-dev` in debug
/// builds so `cargo run`-launched instances stop colliding with the
/// release-installed `/usr/bin/paneflow` on the same machine: distinct
/// data dir (no shared `threads.db` / `session.json` lock), distinct
/// config dir, distinct cache dir, distinct shell helper dir, distinct
/// IPC socket dir. The user can run an installed Paneflow and a
/// from-source build side by side and each holds its own state. Same
/// rule applies cross-crate -- see `paneflow_config::APP_SUBDIR` and
/// `paneflow_threads::APP_SUBDIR` which mirror this const so per-build
/// isolation reaches every persistence surface.
pub const APP_SUBDIR: &str = if cfg!(debug_assertions) {
    "paneflow-dev"
} else {
    "paneflow"
};

#[cfg(unix)]
const PANEFLOW_SUBDIR: &str = APP_SUBDIR;
/// Socket filename, namespaced per build profile so a `cargo run` debug
/// instance and an installed release instance can coexist on the same host
/// without one silently stealing the other's socket. Each instance binds
/// its own listener and the AI-hook wrapper scripts (which read
/// `PANEFLOW_SOCKET_PATH` from the PTY env) route to the right one.
#[cfg(unix)]
const SOCKET_FILE: &str = if cfg!(debug_assertions) {
    "paneflow-dev.sock"
} else {
    "paneflow.sock"
};

/// IPC endpoint plus ownership metadata for the server-side binder.
///
/// `PANEFLOW_SOCKET_PATH` is useful for tests and intentionally isolated debug
/// instances, but the path belongs to the caller, not Paneflow. The IPC server
/// must therefore not create/chmod its parent directory or reclaim a non-socket
/// file there. The default path is Paneflow-owned and may be prepared by the
/// server before bind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IpcSocketPath {
    path: PathBuf,
    owned_parent: bool,
}

impl IpcSocketPath {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(unix)]
    pub(crate) fn owned_parent(&self) -> bool {
        self.owned_parent
    }
}

/// Resolve the PaneFlow runtime directory. Fallback chain:
/// 1. `$XDG_RUNTIME_DIR` - explicit Linux XDG (usually `/run/user/<uid>`).
/// 2. `dirs::runtime_dir()` - same on Linux, `None` on macOS.
/// 3. `$TMPDIR` - populated on macOS (usually `/var/folders/xx/.../T/`).
/// 4. `dirs::cache_dir().join("run")` - last-resort cross-platform fallback.
///
/// Returns `None` only if every layer fails, which in practice means the
/// caller runs on an environment with neither XDG nor TMPDIR nor a cache
/// dir (e.g. a broken container). Callers should `log::warn!` and disable
/// IPC rather than panic.
#[cfg(unix)]
fn runtime_dir() -> Option<PathBuf> {
    std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(dirs::runtime_dir)
        .or_else(|| {
            std::env::var("TMPDIR")
                .ok()
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty())
        })
        .or_else(|| dirs::cache_dir().map(|d| d.join("run")))
}

/// Full path to the IPC socket.
///
/// Unix: `<runtime_dir>/paneflow/paneflow.sock`, or `None` if the runtime
/// dir cannot be resolved or the composed path would exceed the `sun_path`
/// limit. A `log::warn!` is emitted in the over-length case so the user
/// can see why IPC is disabled.
///
/// Windows (US-009): the named-pipe path `\\.\pipe\paneflow`, unconditionally.
/// Named pipes live in a global kernel namespace - there is no runtime dir
/// to resolve, no sun_path limit to enforce, and no XDG fallback chain.
#[cfg(unix)]
pub(crate) fn socket_path_spec() -> Option<IpcSocketPath> {
    if let Some(path) = socket_path_from_env(std::env::var_os("PANEFLOW_SOCKET_PATH")) {
        return check_sun_path_fits(&path).then_some(IpcSocketPath {
            path,
            owned_parent: false,
        });
    }
    let path = runtime_dir()?.join(PANEFLOW_SUBDIR).join(SOCKET_FILE);
    check_sun_path_fits(&path).then_some(IpcSocketPath {
        path,
        owned_parent: true,
    })
}

#[cfg(windows)]
pub(crate) fn socket_path_spec() -> Option<IpcSocketPath> {
    if let Some(path) = socket_path_from_env(std::env::var_os("PANEFLOW_SOCKET_PATH")) {
        return Some(IpcSocketPath {
            path,
            owned_parent: false,
        });
    }
    Some(IpcSocketPath {
        path: PathBuf::from(if cfg!(debug_assertions) {
            r"\\.\pipe\paneflow-dev"
        } else {
            r"\\.\pipe\paneflow"
        }),
        owned_parent: false,
    })
}

pub(crate) fn socket_path() -> Option<PathBuf> {
    socket_path_spec().map(|spec| spec.path)
}

pub(crate) fn shell_integration_dir() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("shell"))
}

fn socket_path_from_env(raw: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(raw?);
    path.is_absolute().then_some(path)
}

/// Prepend the common per-user `bin/` directories to the process `PATH`
/// so PATH-based lookups see binaries installed under the user's home - `~/.bun/bin`,
/// `~/.cargo/bin`, `~/.local/bin`, plus `/opt/homebrew/bin` on macOS.
///
/// Why: when Paneflow is launched from a `.desktop` file, Finder, or the
/// Windows Start Menu, it inherits the systemd-user / launchd / Explorer
/// PATH, which does NOT include `~/.bun/bin`. Agent launch and CLI helper
/// paths then fail to find user-installed tools even though they are available
/// in a normal terminal. Zed, VS Code, and most GUI dev tools all patch their
/// own PATH at startup for the same reason.
///
/// Dirs are prepended (not appended), so user installs always win over any
/// system-shadowed name. Existing entries in PATH are skipped - no
/// duplicates. Idempotent: safe to call multiple times.
///
/// Safety: mutates a process-global env var. Must be called from `main`
/// before any other thread is spawned (i.e. before GPUI initialises),
/// otherwise concurrent readers may observe a torn PATH. Rust 2024 marks
/// `set_var` as `unsafe` for this exact reason.
pub fn augment_path_for_gui_launch() {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".bun").join("bin"));
        candidates.push(home.join(".cargo").join("bin"));
        candidates.push(home.join(".local").join("bin"));
    }

    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from("/opt/homebrew/bin"));
        candidates.push(PathBuf::from("/usr/local/bin"));
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".bun").join("bin"));
        }
        // US-041: Git for Windows ships `git.exe` under `<install>\cmd`, but a
        // GUI launch (Start Menu / Explorer) inherits a PATH that frequently
        // omits it, so the diff viewer's `Command::new("git")` (`diff/git.rs`)
        // fails with NotFound and the whole diff mode is dead on Windows. Add
        // the standard system (`%ProgramFiles%`, `%ProgramFiles(x86)%`) and
        // per-user (`%LOCALAPPDATA%\Programs`) Git locations; the `is_dir()`
        // filter below drops whichever ones aren't present.
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            candidates.push(PathBuf::from(&program_files).join("Git").join("cmd"));
        }
        if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
            candidates.push(PathBuf::from(&program_files_x86).join("Git").join("cmd"));
        }
        if let Some(local) = dirs::data_local_dir() {
            candidates.push(local.join("Programs").join("Git").join("cmd"));
        }
    }

    let current = std::env::var_os("PATH").unwrap_or_default();
    let existing: Vec<PathBuf> = std::env::split_paths(&current).collect();

    let mut to_prepend: Vec<PathBuf> = Vec::new();
    for cand in candidates {
        if !cand.is_dir() {
            continue;
        }
        if existing.iter().any(|p| p == &cand) {
            continue;
        }
        if to_prepend.contains(&cand) {
            continue;
        }
        to_prepend.push(cand);
    }

    if to_prepend.is_empty() {
        return;
    }

    let mut merged: Vec<PathBuf> = to_prepend.clone();
    merged.extend(existing);

    match std::env::join_paths(&merged) {
        Ok(joined) => {
            log::info!(
                "paneflow: augmented PATH with user bin dirs: {}",
                to_prepend
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            // SAFETY: called from `main` before GPUI / IPC / PTY threads start,
            // so no other thread is reading PATH concurrently.
            unsafe { std::env::set_var("PATH", joined) };
        }
        Err(e) => {
            log::warn!("paneflow: failed to join augmented PATH ({e}); leaving PATH unchanged");
        }
    }
}

/// Resolve the PaneFlow per-user data directory (cross-platform).
///
/// - Linux: `$XDG_DATA_HOME/paneflow` (typically `~/.local/share/paneflow`)
/// - macOS: `~/Library/Application Support/paneflow`
/// - Windows: `%LOCALAPPDATA%\paneflow` - **non-roaming** on purpose, so a
///   roamed profile does not carry the per-install telemetry_id to another
///   machine.
///
/// The directory is created if it does not already exist. Returns `None` if
/// either the platform helper returns `None` (broken environment) or the
/// `create_dir_all` call fails (read-only FS, permission denied, etc.).
/// Callers should fall back to an ephemeral in-memory UUID in that case
/// (see `telemetry::id::telemetry_id`).
pub fn data_dir() -> Option<PathBuf> {
    let dir = dirs::data_local_dir()?.join(APP_SUBDIR);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::debug!(
            "paneflow: data_dir {} is unwritable ({e}); callers will use ephemeral state",
            dir.display()
        );
        return None;
    }
    Some(dir)
}

/// Stable, **non-versioned** absolute path of the embedded `paneflow-mcp`
/// bridge binary (EP-001 US-003).
///
/// Unlike the shim / ai-hook helpers - which live under
/// `cache_dir()/paneflow/bin/<VERSION>/` and are re-resolved by Paneflow on
/// every launch - the bridge path is written into **external, persistent
/// agent configs** (`~/.claude.json`, `~/.codex/config.toml`, ...) by
/// `paneflow mcp install`. A version-pinned path would go stale on the next
/// Paneflow update, and `cache_dir()` can be purged by the OS. So the bridge
/// lives under `data_dir()` (durable, non-versioned):
///
/// - Linux:   `~/.local/share/paneflow/bin/paneflow-mcp`
/// - macOS:   `~/Library/Application Support/paneflow/bin/paneflow-mcp`
/// - Windows: `%LOCALAPPDATA%\paneflow\bin\paneflow-mcp.exe`
///
/// Returns `None` when `data_dir()` is unresolvable or unwritable. Callers
/// (`ai_hooks::extract::ensure_bridge_extracted`, and later `paneflow mcp
/// install`) must treat `None` as "refuse to register a config pointing at a
/// path that does not exist" rather than fabricating a path.
///
/// This only **computes** the path; it does not extract. The byte
/// materialization + SHA-compared atomic rewrite is
/// `ai_hooks::extract::ensure_bridge_extracted`.
pub fn bridge_binary_path() -> Option<PathBuf> {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    Some(
        data_dir()?
            .join("bin")
            .join(format!("paneflow-mcp{suffix}")),
    )
}

/// Stable, non-versioned path of the `paneflow-ai-hook` callback binary
/// (EP-004 US-016, prd-cli-agent-orchestration). Same rationale as
/// [`bridge_binary_path`]: `paneflow hooks setup` writes this path into
/// **external, persistent agent configs** (`~/.claude/settings.json`, …), so it
/// must survive Paneflow updates - unlike the version-pinned shim/ai-hook copy
/// under `cache_dir()/paneflow/bin/<VERSION>/` that the shim itself resolves at
/// launch. Lives alongside the bridge under `data_dir()/paneflow/bin/`:
///
/// - Linux:   `~/.local/share/paneflow/bin/paneflow-ai-hook`
/// - macOS:   `~/Library/Application Support/paneflow/bin/paneflow-ai-hook`
/// - Windows: `%LOCALAPPDATA%\paneflow\bin\paneflow-ai-hook.exe`
///
/// Returns `None` when `data_dir()` is unresolvable. Computes the path only;
/// the byte materialization is `ai_hooks::extract::ensure_ai_hook_extracted`.
pub fn ai_hook_binary_path() -> Option<PathBuf> {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    Some(
        data_dir()?
            .join("bin")
            .join(format!("paneflow-ai-hook{suffix}")),
    )
}

#[cfg(unix)]
fn check_sun_path_fits(path: &std::path::Path) -> bool {
    let bytes = path.as_os_str().len();
    // `MAX_SOCKET_PATH_BYTES` is `sizeof(sun_path)`, and `bind()` needs room for
    // the trailing NUL inside that array - so a path of *exactly* the array size
    // does not fit. Reject `>=`, not `>` (the usable maximum is the array size
    // minus one).
    if bytes >= MAX_SOCKET_PATH_BYTES {
        log::warn!(
            "paneflow: computed IPC socket path does not fit sun_path ({} >= {} bytes, no room for the NUL terminator): {} - IPC will be disabled. Set $XDG_RUNTIME_DIR (Linux) or shorten $TMPDIR (macOS) to enable it.",
            bytes,
            MAX_SOCKET_PATH_BYTES,
            path.display()
        );
        false
    } else {
        true
    }
}

/// Strip Windows verbatim prefixes from a canonical path.
///
/// `std::fs::canonicalize` returns `\\?\C:\...` on Windows. That spelling is
/// fine for the Rust filesystem APIs that produced it, and even for `git -C`,
/// but it breaks two things that matter here:
///
/// - `cmd.exe` treats it as an unsupported UNC cwd and silently falls back to
///   `C:\Windows`;
/// - git rewrites it to `//?/C:/...` and cannot create directories under it, so
///   `git worktree add` fails with `could not create leading directories`.
///
/// It also does not compare equal to the forward-slash paths git prints
/// (`git worktree list --porcelain`), while the stripped form does: Windows
/// path comparison treats `/` and `\` as the same separator.
///
/// A third caller depends on the same normalization for a different reason:
/// `update::install_method` compares the canonical `current_exe()` against the
/// non-verbatim `%ProgramFiles%` env value, and `Path::starts_with`'s leading
/// component (`Prefix(VerbatimDisk)` vs `Prefix(Disk)`) never matches.
///
/// So a path that leaves Paneflow for a subprocess, or that gets compared
/// against one's output, carries the normal DOS/UNC spelling. No-op on
/// non-verbatim and Unix paths, so it is safe to call on all targets. Pure
/// string logic, so the regression tests run on Linux CI.
pub fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    // Decide on a borrowed view, then move `path` only in the fall-through:
    // returning `path` from inside a `match path.to_str() { … }` arm would
    // conflict with the `to_str()` borrow.
    let stripped = path.to_str().and_then(|s| {
        s.strip_prefix(r"\\?\UNC\")
            // `\\?\UNC\server\share\…` → `\\server\share\…`
            .map(|rest| PathBuf::from(format!(r"\\{rest}")))
            // `\\?\C:\…` → `C:\…`
            .or_else(|| s.strip_prefix(r"\\?\").map(PathBuf::from))
    });
    stripped.unwrap_or(path)
}

#[cfg(test)]
mod verbatim_prefix_tests {
    use super::strip_verbatim_prefix;
    use std::path::PathBuf;

    #[test]
    fn disk_unc_and_passthrough() {
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:\dev\paneflow")),
            PathBuf::from(r"C:\dev\paneflow")
        );
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\paneflow")),
            PathBuf::from(r"\\server\share\paneflow")
        );
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"C:\dev\paneflow")),
            PathBuf::from(r"C:\dev\paneflow")
        );
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from("/home/arthur/paneflow")),
            PathBuf::from("/home/arthur/paneflow")
        );
    }

    /// Only the `\\?\` prefix is matched, so a tail spelled with forward
    /// slashes survives intact. `update::install_method`'s Windows tests rely
    /// on it: they feed a mixed-separator path so `Path::starts_with` stays
    /// component-based on Linux CI.
    #[test]
    fn a_forward_slash_tail_is_left_alone() {
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:/Program Files/PaneFlow/paneflow.exe")),
            PathBuf::from("C:/Program Files/PaneFlow/paneflow.exe")
        );
    }

    /// The reason `canonicalize_or` strips: a verbatim `repo_root` never
    /// compares equal to the forward-slash path `git worktree list` prints, so
    /// "is this checkout the repository's own?" answered `no` for every branch.
    #[cfg(windows)]
    #[test]
    fn stripped_form_matches_what_git_prints() {
        let from_git = PathBuf::from("C:/dev/paneflow");
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:\dev\paneflow")),
            from_git
        );
        assert_ne!(PathBuf::from(r"\\?\C:\dev\paneflow"), from_git);
    }
}

#[cfg(test)]
mod socket_env_tests {
    use super::*;

    #[test]
    fn socket_path_env_helper_requires_absolute_path() {
        let absolute = if cfg!(windows) {
            r"\\.\pipe\paneflow-test"
        } else {
            "/tmp/paneflow-test.sock"
        };
        assert_eq!(
            socket_path_from_env(Some(std::ffi::OsString::from(absolute))),
            Some(PathBuf::from(absolute))
        );
        assert_eq!(
            socket_path_from_env(Some(std::ffi::OsString::from("relative-paneflow.sock"))),
            None
        );
        assert_eq!(socket_path_from_env(None), None);
    }
}

// US-009 - these tests assert Unix socket path composition and sun_path
// length limits, so they are structurally Unix-only.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env vars are process-global; tests that mutate them must be serialised.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        socket: Option<String>,
        xdg: Option<String>,
        tmp: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn take() -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            Self {
                socket: std::env::var("PANEFLOW_SOCKET_PATH").ok(),
                xdg: std::env::var("XDG_RUNTIME_DIR").ok(),
                tmp: std::env::var("TMPDIR").ok(),
                _guard: guard,
            }
        }

        fn clear(&self) {
            // SAFETY: serialised by ENV_LOCK; no other test or production
            // thread mutates these vars during the test window.
            unsafe {
                std::env::remove_var("PANEFLOW_SOCKET_PATH");
                std::env::remove_var("XDG_RUNTIME_DIR");
                std::env::remove_var("TMPDIR");
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: serialised by ENV_LOCK (still held via _guard).
            unsafe {
                match &self.socket {
                    Some(v) => std::env::set_var("PANEFLOW_SOCKET_PATH", v),
                    None => std::env::remove_var("PANEFLOW_SOCKET_PATH"),
                }
                match &self.xdg {
                    Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                    None => std::env::remove_var("XDG_RUNTIME_DIR"),
                }
                match &self.tmp {
                    Some(v) => std::env::set_var("TMPDIR", v),
                    None => std::env::remove_var("TMPDIR"),
                }
            }
        }
    }

    #[test]
    fn paneflow_socket_path_env_wins_when_absolute() {
        let g = EnvGuard::take();
        g.clear();
        // SAFETY: ENV_LOCK held.
        unsafe {
            std::env::set_var("PANEFLOW_SOCKET_PATH", "/tmp/paneflow-isolated.sock");
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        }
        assert_eq!(
            socket_path(),
            Some(PathBuf::from("/tmp/paneflow-isolated.sock"))
        );
        let spec = socket_path_spec().expect("env socket path resolves");
        assert_eq!(spec.path(), Path::new("/tmp/paneflow-isolated.sock"));
        assert!(
            !spec.owned_parent(),
            "env override parent must not be treated as Paneflow-owned"
        );
    }

    #[test]
    fn xdg_runtime_dir_wins_when_set() {
        let g = EnvGuard::take();
        g.clear();
        // SAFETY: ENV_LOCK held.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000") };
        let p = socket_path().expect("runtime dir must resolve");
        assert_eq!(
            p,
            PathBuf::from(format!("/run/user/1000/{APP_SUBDIR}/{SOCKET_FILE}")),
            "AC5: Linux with XDG_RUNTIME_DIR must resolve to the XDG path \
             (subdir + filename vary by build profile via APP_SUBDIR / SOCKET_FILE)"
        );
        assert!(
            socket_path_spec().expect("socket spec").owned_parent(),
            "default runtime-dir socket is Paneflow-owned"
        );
    }

    #[test]
    fn tmpdir_fallback_when_xdg_and_runtime_dir_missing() {
        let g = EnvGuard::take();
        g.clear();
        // SAFETY: ENV_LOCK held.
        unsafe { std::env::set_var("TMPDIR", "/tmp/macos-stub") };
        let p = socket_path();
        if let Some(p) = p {
            // On Linux, dirs::runtime_dir() may still return Some before we
            // reach the TMPDIR branch - accept either but prove the path is
            // well-formed.
            assert!(p.ends_with(format!("{APP_SUBDIR}/{SOCKET_FILE}")));
        }
    }

    #[test]
    fn overlong_path_returns_none() {
        let g = EnvGuard::take();
        g.clear();
        // 120-byte XDG_RUNTIME_DIR → joined path blows past 104.
        let long = "/".to_string() + &"x".repeat(119);
        // SAFETY: ENV_LOCK held.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &long) };
        assert!(
            socket_path().is_none(),
            "AC6: over-long sun_path must return None rather than a bind-time error"
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        socket: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn take() -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            Self {
                socket: std::env::var("PANEFLOW_SOCKET_PATH").ok(),
                _guard: guard,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: serialised by ENV_LOCK (still held via _guard).
            unsafe {
                match &self.socket {
                    Some(v) => std::env::set_var("PANEFLOW_SOCKET_PATH", v),
                    None => std::env::remove_var("PANEFLOW_SOCKET_PATH"),
                }
            }
        }
    }

    #[test]
    fn paneflow_socket_path_env_wins_for_named_pipe() {
        let _guard = EnvGuard::take();
        // SAFETY: ENV_LOCK held.
        unsafe {
            std::env::set_var("PANEFLOW_SOCKET_PATH", r"\\.\pipe\paneflow-isolated-test");
        }
        assert_eq!(
            socket_path(),
            Some(PathBuf::from(r"\\.\pipe\paneflow-isolated-test"))
        );
    }
}
