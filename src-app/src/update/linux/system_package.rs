//! System-package self-update via `pkexec dnf|apt-get install` (US-001
//! of the pkexec system-package PRD, `tasks/prd-pkexec-system-package-update.md`).
//!
//! This is the Linux-only counterpart to [`super::appimage`] /
//! [`super::targz`] for users on the signed rpm/deb repo
//! (`pkg.paneflow.dev`). It spawns a polkit-elevated
//! `dnf install paneflow-<ver>` (Fedora/RHEL/Rocky),
//! `apt-get install paneflow=<ver>-1` (Ubuntu/Debian), or
//! `zypper install paneflow=<ver>` (openSUSE) subprocess and
//! routes the outcome through the existing `UpdateError` taxonomy so
//! the existing toast/pill renderer needs no new variants.
//!
//! # Design notes
//!
//! - **Argv only, never shell.** Every invocation is
//!   `Command::new("pkexec").args(&[…])`. The version string is
//!   regex-validated before any subprocess is spawned so a compromised
//!   GitHub tag cannot inject extra argv tokens.
//! - **pkexec exit-code contract** (see openSUSE `pkexec(1)` manpage):
//!   - `0` - wrapped command exited 0.
//!   - `126` - user dismissed the polkit auth dialog.
//!   - `127` - no polkit agent available, or pkexec / target binary
//!     not found on PATH.
//!   - other - the wrapped command's own exit code.
//! - **Two-thread pipe drain.** A single `BufReader` draining stdout
//!   first deadlocks when stderr fills its ~64 KiB pipe - `dnf`/`apt`
//!   routinely produce more than that during a transaction. Each pipe
//!   gets its own drain thread; the parent joins both drains before
//!   calling `child.wait()`.
//! - **Log hygiene.** Stdout lines go to `log::info!` so an admin
//!   tailing the PaneFlow log can see transaction progress live. The
//!   stderr buffer is only emitted at `log::debug!` on a non-zero exit
//!   that is *not* user-cancel (126) - routine cancels never leak
//!   stderr.
//! - **No new dependency.** Uses only `std`, `anyhow`, `log`, and the
//!   already-present `which` crate.
//!
//! The public entry point is [`run_update`]; the dispatcher in
//! `app/self_update_flow.rs` (US-002) routes `InstallMethod::SystemPackage
//! { Dnf | Apt | Zypper }` here via `smol::unblock`. `PackageManager::Other`
//! continues to use the clipboard-copy fallback upstream.

use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, Result};

use super::super::error::UpdateError;
use super::super::install_method::PackageManager;

/// Hard cap on the stderr buffer retained for `log::debug!` on a failed
/// transaction. Matches the 1 MiB ceiling in the PRD's Non-Functional
/// Requirements table - dnf can be chatty when a mirror is broken, but
/// multi-megabyte buffers are never useful in a user-visible log.
const STDERR_BUFFER_CAP_BYTES: usize = 1024 * 1024;

/// Canonical install path for `pkexec`. Spawning from this absolute
/// path (rather than resolving "pkexec" via `$PATH` at exec time)
/// prevents an attacker who can write to an earlier PATH entry (e.g.
/// `~/bin/`) from shadowing the binary with a look-alike that silently
/// no-ops the update. `which::which` still runs up-front as the "is it
/// installed at all?" probe, but the actual `Command::new` call uses
/// this constant. All PRD-supported distros (Fedora 40+, Ubuntu 22.04+,
/// Debian, openSUSE, Arch) install `pkexec` here as part of the polkit
/// package; NixOS-style non-FHS layouts are routed to the
/// `PackageManager::Other` clipboard fallback upstream.
const PKEXEC_ABSOLUTE_PATH: &str = "/usr/bin/pkexec";

/// Upper bound on `mpsc` messages the drain threads can enqueue without
/// the main thread dequeuing. Prevents a pathological pkg-mgr producing
/// multi-GB stderr from blowing up the channel buffer - the producer
/// blocks on `send` once the queue is full, which throttles it to the
/// main thread's drain rate. 1024 lines × a few hundred bytes each is
/// ample for a normal transaction.
const DRAIN_CHANNEL_CAPACITY: usize = 1024;

/// One line pulled off a child pipe, tagged with its source so the
/// main-thread drain loop can dispatch it correctly.
enum DrainedLine {
    Stdout(String),
    Stderr(String),
}

/// User-visible backpressure message for the US-005 busy pre-flight.
/// A constant (rather than a string literal in two places) so the
/// dispatcher in `app/self_update_flow.rs` can match against exactly
/// the same sentinel the runner emits, without risking a typo.
pub(crate) const BUSY_MESSAGE: &str = "Package manager is busy - try again in a moment.";

/// Authoritative dnf5 transaction lock on Fedora 41+. Lives on tmpfs
/// (`/run/…`) so the file is always cleaned on reboot, which keeps
/// stale-file false-positives bounded. Shared by CLI `dnf5`,
/// `dnf-automatic`, and PackageKit (GNOME Software / Plasma Discover)
/// since F41 - probing it alone catches every "system is doing
/// package management right now" scenario we care about.
const DNF_LOCK_PATH: &str = "/run/dnf/rpmtransaction.lock";

/// Run the pkexec-elevated package-manager upgrade.
///
/// Returns `Ok(())` on a clean transaction. Errors are returned as
/// `anyhow::Error` wrapping an [`UpdateError`] variant so the caller's
/// `UpdateError::classify` downcast succeeds without substring-matching
/// the formatted error chain.
///
/// Error mapping, from the PRD acceptance criteria:
///
/// | pkexec exit | `UpdateError` variant |
/// |---|---|
/// | 126 (polkit cancelled) | `InstallDeclined { message: "Authentication cancelled" }` |
/// | 127 (no agent / missing) | `EnvironmentBroken { message: … }` |
/// | any other non-zero | `InstallFailed { log_path: "" }` |
/// | signal-killed | `Other("killed by signal N")` |
///
/// `PackageManager::Other` is treated as a programmer error - the
/// dispatcher is required to route it to the clipboard fallback. We
/// still return a structured error rather than panicking, to honour
/// the workspace "no `unwrap`/`expect` in production" rule.
pub fn run_update(manager: &PackageManager, version: &str) -> Result<()> {
    run_update_impl(
        manager,
        version,
        which::which("pkexec").is_ok(),
        Path::new(PKEXEC_ABSOLUTE_PATH),
    )
}

/// Internal form of [`run_update`] that takes the pkexec environment as
/// explicit parameters so the US-006 test harness can drive the full
/// flow against a stub `pkexec` binary without having to mutate
/// `$PATH` (racy across parallel tests) and without undoing US-001's
/// hardcoded-`/usr/bin/pkexec` security hardening.
///
/// - `pkexec_installed` - result of the "is pkexec on PATH?" probe
///   (`which::which("pkexec").is_ok()` in production; forced in tests).
/// - `pkexec_spawn_path` - absolute path to the binary we actually
///   exec (`/usr/bin/pkexec` in production; a temp-dir stub script in
///   tests).
fn run_update_impl(
    manager: &PackageManager,
    version: &str,
    pkexec_installed: bool,
    pkexec_spawn_path: &Path,
) -> Result<()> {
    if matches!(manager, PackageManager::Other | PackageManager::RpmOstree) {
        return Err(anyhow::Error::new(UpdateError::Other(
            "pkexec branch reached with non-dnf/apt/zypper PackageManager".into(),
        )));
    }

    let normalized_version = validate_version(version)?;

    if !pkexec_installed {
        return Err(anyhow::Error::new(UpdateError::EnvironmentBroken {
            message: "pkexec not found - update via your system package manager".into(),
        }));
    }

    // US-005: backpressure pre-flight. Block here instead of spawning
    // pkexec + dnf/apt which would otherwise hang for minutes behind
    // `dnf-automatic.timer` or an interactive `sudo apt upgrade` in
    // another terminal. Emits the sentinel `BUSY_MESSAGE` that the
    // dispatcher matches to route to a neutral "try again" toast
    // (NOT counting against the 3-strikes retry counter).
    // For apt the probe surfaces the lock-owner PID; for dnf the
    // tmpfs lock file carries no PID so only the fact-of-lock is
    // attached. The PID (when available) goes into the anyhow
    // context chain - useful for bug reports via `{err:#}` - and is
    // NEVER included in the user-visible `BUSY_MESSAGE` (PRD AC).
    let busy_context: Option<String> = match manager {
        PackageManager::Dnf => dnf_lock_held()
            .then(|| format!("dnf lock held during pre-flight ({DNF_LOCK_PATH} present)")),
        PackageManager::Apt => apt_lock_owner_from_proc(Path::new("/proc"))
            .map(|pid| format!("apt/dpkg transaction in flight during pre-flight (pid {pid})")),
        // zypper exposes locks through libzypp rather than a stable,
        // distro-wide tmpfs lock file. Let zypper perform its own
        // non-interactive lock handling instead of guessing.
        PackageManager::Zypper => None,
        // Other / RpmOstree already rejected at the top of the fn.
        PackageManager::Other | PackageManager::RpmOstree => None,
    };
    if let Some(diag) = busy_context {
        return Err(anyhow::Error::new(UpdateError::Other(BUSY_MESSAGE.into())).context(diag));
    }

    let argv = build_argv(manager, normalized_version);
    let manager_label = manager_label(manager).to_string();

    // argv[0] is the literal "pkexec" token kept for display / test
    // consistency with the PRD argv spec. The actual spawn uses the
    // injected `pkexec_spawn_path` (production = `PKEXEC_ABSOLUTE_PATH`
    // = `/usr/bin/pkexec`; US-006 tests = a temp-dir stub script).
    // The `pkexec_installed` probe above has already confirmed the
    // binary exists; we refuse to let an earlier PATH entry hand us
    // a different binary at exec time.
    let (_display_program, args) = argv
        .split_first()
        .context("build_argv returned an empty command vector")?;

    let mut child = Command::new(pkexec_spawn_path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn `{}`", argv.join(" ")))?;

    let stdout = child
        .stdout
        .take()
        .context("pkexec child did not expose stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("pkexec child did not expose stderr")?;

    // Two-thread + mpsc drain, per the PRD research section. A single
    // BufReader would deadlock when the unread pipe fills (~64 KiB) -
    // `dnf` and `apt` routinely produce more than that during a
    // transaction. Each pipe gets a dedicated producer thread; the
    // main thread drains the receiver and dispatches per-stream. The
    // drain loop exits when both senders are dropped (thread exit),
    // which is our signal that both pipes have EOF'd.
    //
    // `sync_channel(N)` is used instead of the unbounded `channel()`
    // so a pathological pkg-mgr that emits multi-GB stderr cannot
    // exhaust heap before the main thread throttles it - once the
    // bounded queue fills, producer `send` blocks, which back-pressures
    // the `BufReader::lines` iterator, which back-pressures the pipe.
    let (tx, rx) = mpsc::sync_channel::<DrainedLine>(DRAIN_CHANNEL_CAPACITY);
    let tx_stderr = tx.clone();
    let stdout_handle = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(DrainedLine::Stdout(line)).is_err() {
                break;
            }
        }
    });
    let stderr_handle = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            if tx_stderr.send(DrainedLine::Stderr(line)).is_err() {
                break;
            }
        }
    });

    let mut stderr_buf: Vec<String> = Vec::new();
    let mut stderr_bytes: usize = 0;
    let mut stderr_truncated = false;
    for event in rx {
        match event {
            DrainedLine::Stdout(line) => {
                log::info!("self-update/{manager_label}: {line}");
            }
            DrainedLine::Stderr(line) => {
                // +1 for the newline we logically retain in the joined output.
                stderr_bytes = stderr_bytes.saturating_add(line.len().saturating_add(1));
                if stderr_bytes > STDERR_BUFFER_CAP_BYTES {
                    stderr_truncated = true;
                } else {
                    stderr_buf.push(line);
                }
            }
        }
    }
    if stderr_truncated {
        log::warn!("self-update: stderr buffer truncated at 1 MiB cap");
    }

    // The drain loop above already ran to completion (both Senders
    // dropped), so these joins are cleanup - but calling them makes
    // thread-panic propagation explicit rather than relying on an
    // abandoned handle.
    let _ = stdout_handle.join();
    let _ = stderr_handle.join();

    let status = child.wait().context("failed to wait on pkexec child")?;

    if status.success() {
        log::info!("self-update/{manager_label}: upgrade succeeded");
        return Ok(());
    }

    let code = status.code();
    let signal = status.signal();
    let update_err = classify_exit(code, signal, &stderr_buf, &manager_label);

    // Per the US-001 AC: attach the raw exit code / signal to the
    // anyhow context chain so a caller doing `format!("{err:#}")` or
    // walking `err.chain()` can surface the numeric outcome for
    // diagnostics. `UpdateError::classify` downcasts via the chain, so
    // this extra wrapper does not interfere with taxonomy routing.
    let wrapped = anyhow::Error::new(update_err);
    let wrapped = match (code, signal) {
        (Some(n), _) => wrapped.context(format!("pkexec exited with code {n}")),
        (None, Some(sig)) => wrapped.context(format!("pkexec killed by signal {sig}")),
        (None, None) => wrapped,
    };
    Err(wrapped)
}

fn manager_label(manager: &PackageManager) -> &'static str {
    match manager {
        PackageManager::Dnf => "dnf",
        PackageManager::Apt => "apt",
        PackageManager::Zypper => "zypper",
        PackageManager::Other => "other",
        // US-004: guarded-out in `run_update` before reaching this
        // helper; label is only kept for exhaustiveness.
        PackageManager::RpmOstree => "rpm-ostree",
    }
}

/// Validate `^v?\d+\.\d+\.\d+$` and return the `v`-stripped slice.
///
/// Rejects pre-release suffixes (`-rc1`, `-beta`), whitespace, shell
/// metacharacters, and the empty string. Implemented by hand rather
/// than via the `regex` crate to avoid a new runtime dependency on the
/// hot path - the grammar is trivially small.
fn validate_version(raw: &str) -> Result<&str> {
    let rest = raw.strip_prefix('v').unwrap_or(raw);

    let mut completed_parts: usize = 0;
    let mut segment_len: usize = 0;
    for ch in rest.chars() {
        match ch {
            '0'..='9' => {
                segment_len = segment_len.saturating_add(1);
            }
            '.' => {
                if segment_len == 0 {
                    return Err(invalid_version(raw));
                }
                completed_parts = completed_parts.saturating_add(1);
                segment_len = 0;
            }
            _ => return Err(invalid_version(raw)),
        }
    }
    if segment_len == 0 {
        return Err(invalid_version(raw));
    }
    completed_parts = completed_parts.saturating_add(1);
    if completed_parts != 3 {
        return Err(invalid_version(raw));
    }

    Ok(rest)
}

fn invalid_version(raw: &str) -> anyhow::Error {
    anyhow::Error::new(UpdateError::Other(format!(
        "Invalid version string: {raw:?}"
    )))
}

/// Assemble the static argv for the given package manager.
///
/// Pure function - the US-006 test suite drives it without a
/// subprocess. Returning an empty `Vec` for `PackageManager::Other` is
/// a defensive sentinel: `run_update` already rejects `Other` up-front,
/// but if a future caller ever skips that guard, `split_first()` on the
/// empty vec will surface a loud error instead of spawning a bare
/// `pkexec` call.
fn build_argv(manager: &PackageManager, version_stripped: &str) -> Vec<String> {
    match manager {
        // `--refresh` is a global flag - it MUST prefix the `install`
        // subcommand (dnf's global args are positional). Without it,
        // dnf loads its local metadata cache (default TTL 48h) and
        // fails with "No match for argument: paneflow-<ver>" when the
        // cache predates the release we're installing. Observed in
        // the v0.2.3 release manual acceptance. See PRD v1.2.
        PackageManager::Dnf => vec![
            "pkexec".into(),
            "dnf".into(),
            "--refresh".into(),
            "install".into(),
            "-y".into(),
            "--best".into(),
            "--setopt=install_weak_deps=False".into(),
            format!("paneflow-{version_stripped}"),
        ],
        // apt has no single-command equivalent of `dnf --refresh
        // install` - `apt-get update` is a mandatory separate step
        // to refresh the package lists before `install pkg=version`
        // can resolve a freshly-published version. We wrap both
        // commands inside ONE `pkexec sh -c` so the user sees a
        // single polkit prompt (matching the Dnf UX) instead of two.
        //
        // cargo-deb emits Debian package versions as `<upstream>-1`
        // (`revision = "1"` in src-app/Cargo.toml), so the apt pin must
        // include that Debian revision. The GitHub tag is only the upstream
        // semver, validated by `validate_version`; the deterministic `-1`
        // is appended here after validation.
        //
        // The shell body is a STATIC constant string: the Debian package version
        // flows in as the POSITIONAL parameter `$1`, double-quoted.
        // bash / dash POSIX shells do NOT re-interpret metacharacters
        // inside a double-quoted positional expansion - so even a
        // regex-bypassing version string would be treated as literal
        // data in the `paneflow=$1` arg to apt-get. Defense-in-depth
        // on top of `validate_version`'s allow-list regex.
        //
        // `_` is the conventional `$0` placeholder (shell convention
        // for script name in `sh -c '...' _ <args>`).
        //
        // Refactor guard: this argv is locked at its exact shape by
        // `build_apt_argv_wraps_in_sh_c_with_positional_version` and
        // `build_apt_argv_passes_version_as_positional_not_interpolated`
        // in the test suite. A refactor that inlines the version into
        // the script body (or reorders) will break those tests.
        PackageManager::Apt => vec![
            "pkexec".into(),
            "sh".into(),
            "-c".into(),
            "apt-get update -q && apt-get install -y --no-install-recommends \"paneflow=$1\""
                .into(),
            "_".into(),
            format!("{version_stripped}-1"),
        ],
        // zypper supports exact-version selection through `name=version`.
        // As with apt, metadata refresh and install are separate commands,
        // so wrap both in one `pkexec sh -c` and pass the version as `$1`
        // instead of interpolating it into the shell body.
        PackageManager::Zypper => vec![
            "pkexec".into(),
            "sh".into(),
            "-c".into(),
            "zypper --non-interactive --gpg-auto-import-keys refresh && zypper --non-interactive install --no-recommends --force \"paneflow=$1\""
                .into(),
            "_".into(),
            version_stripped.into(),
        ],
        // Defensive sentinels - `run_update` guards Other / RpmOstree
        // up-front, so an empty argv is never actually spawned; if a
        // future caller skips that guard, `split_first()` on the empty
        // vec surfaces a loud error instead of spawning a bare `pkexec`.
        PackageManager::Other | PackageManager::RpmOstree => Vec::new(),
    }
}

/// Map a finished child's exit info into an [`UpdateError`] variant.
///
/// Emits the buffered stderr at `log::debug!` only for the generic
/// non-zero-exit branch - the 126 (polkit cancel) branch is *not* a
/// failure and must not leak stderr.
fn classify_exit(
    code: Option<i32>,
    signal: Option<i32>,
    stderr_buf: &[String],
    manager_label: &str,
) -> UpdateError {
    if let Some(sig) = signal {
        return UpdateError::Other(format!("package manager killed by signal {sig}"));
    }

    match code {
        Some(0) => {
            UpdateError::Other("classify_exit called on a successful status - caller bug".into())
        }
        Some(126) => UpdateError::InstallDeclined {
            message: "Authentication cancelled".into(),
        },
        Some(127) => UpdateError::EnvironmentBroken {
            message: "pkexec returned 127 (no polkit agent or command missing)".into(),
        },
        // Shell / POSIX convention: a process whose child was killed by
        // signal N often reports exit code 128+N. The kernel's own
        // WIFSIGNALED status would surface through `status.signal()`
        // and hit the branch above, but pkexec (and some intermediaries
        // on older polkit/bash chains) may propagate the child signal
        // as a plain 128+N exit code instead. Classify those as
        // `Other("killed by signal")` so the toast copy matches the
        // direct-signal path rather than a generic "install failed".
        // The 128+0..=31 window is what POSIX realtime-signal ranges
        // recommend checking; anything above 159 we treat as a genuine
        // install failure.
        Some(n) if (129..=159).contains(&n) => {
            let sig = n - 128;
            UpdateError::Other(format!("package manager killed by signal {sig}"))
        }
        Some(n) => {
            if !stderr_buf.is_empty() {
                log::debug!(
                    "self-update/{manager_label}: stderr (exit {n}):\n{}",
                    stderr_buf.join("\n")
                );
            }
            UpdateError::InstallFailed {
                log_path: PathBuf::new(),
            }
        }
        None => UpdateError::Other("package manager exited without an exit code or signal".into()),
    }
}

// ─── US-005: lock pre-flight helpers ─────────────────────────────────
//
// dnf5 and apt-get both acquire an exclusive `flock(2)` on a root-owned
// lock file (`0640`). An unprivileged PaneFlow process cannot open
// those files for writing and therefore cannot run the canonical
// "try to take the lock" probe. We fall back to two heuristics that
// are reachable without elevation:
//
// - **dnf5**: `/run/dnf/rpmtransaction.lock` existence check. The file
//   lives on tmpfs (cleared on reboot) and is created by libdnf5 at
//   transaction start. Existence is a heuristic - a crashed dnf5 could
//   leave the file on disk - but it fails safe (user sees "try again",
//   no actual harm; tmpfs reset clears false positives).
//
// - **apt**: scan `/proc/*/comm` (world-readable) for processes named
//   `apt`, `apt-get`, `dpkg`, or `unattended-upgr`. The `lock-frontend`
//   file is persistent on disk and cannot be flock-probed unprivileged,
//   so process inspection is the only reliable pre-flight signal.
//
// Both checks err on the side of false-positives (treat "unsure" as
// "busy") - a retry toast is a trivial UX friction whereas a minutes-
// long freeze behind an in-flight transaction is not.

/// Public wrapper for the dnf5 lock probe. Checks the canonical
/// tmpfs path; see [`dnf_lock_held_at`] for the injectable variant
/// used by tests.
fn dnf_lock_held() -> bool {
    dnf_lock_held_at(Path::new(DNF_LOCK_PATH))
}

/// Parameter-driven dnf lock probe. `true` iff `path` exists as a
/// regular file or symlink; `false` for missing, unreadable-parent,
/// or broken-symlink cases.
///
/// Per the PRD (Unhappy paths section, "Permissions error reading
/// the lock file"): treat the ambiguous `Err` case as "unknown,
/// proceed" rather than "busy, block". Blocking on an unresolvable
/// probe error would strand users on hardened / sandboxed systems
/// (SELinux profiles, filtered /run mounts, flatpak sandboxes)
/// permanently without ever surfacing the real issue, and without
/// ever feeding the 3-strikes "open releases page" escape hatch.
/// Instead we log a warning and let dnf5's own `flock(2)` acquisition
/// serve as the authoritative gate.
fn dnf_lock_held_at(path: &Path) -> bool {
    match path.try_exists() {
        Ok(exists) => exists,
        Err(err) => {
            log::warn!(
                "self-update/dnf: lock probe at {} failed ({err}); proceeding without pre-flight",
                path.display()
            );
            false
        }
    }
}

/// Names we consider "an apt/dpkg transaction is in flight". Derived
/// by reading `/proc/{pid}/comm`, which on Linux is capped at 15
/// characters + newline - `unattended-upgrade` shows as
/// `unattended-upgr`. All entries must be ≤15 chars.
const APT_PROCESS_COMMS: &[&str] = &[
    "apt",
    "apt-get",
    "apt.systemd.da",
    "dpkg",
    "unattended-upgr",
];

/// Parameter-driven apt lock probe. Scans `proc_root/*/comm` for a
/// process whose basename matches [`APT_PROCESS_COMMS`] and returns
/// its PID so the caller can attach it to the anyhow context chain
/// (PRD US-005 AC: "raw lock-owner PID is included in the anyhow
/// context for diagnostics but not surfaced to the user").
///
/// Uses only world-readable filesystem primitives - no elevation
/// required. `/proc` unreadable at probe time is treated as "unknown,
/// proceed" (PRD Unhappy paths) rather than "busy, block", so hardened
/// sandboxes don't permanently strand the updater.
fn apt_lock_owner_from_proc(proc_root: &Path) -> Option<u32> {
    let entries = match std::fs::read_dir(proc_root) {
        Ok(e) => e,
        Err(err) => {
            log::warn!(
                "self-update/apt: /proc scan at {} failed ({err}); proceeding without pre-flight",
                proc_root.display()
            );
            return None;
        }
    };
    for entry in entries.flatten() {
        // Numeric-only directory names are PIDs. Skip `self`, `net`,
        // `sys`, etc. cheaply without a full stat().
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        let comm_path = entry.path().join("comm");
        let Ok(bytes) = std::fs::read(&comm_path) else {
            // Process may have exited between read_dir and read;
            // that's normal on a busy system, not an error.
            continue;
        };
        let comm = match std::str::from_utf8(&bytes) {
            // Strip both `\n` (normal kernel format) and `\0`
            // (possible mid-read race when the process name is
            // being rewritten via `prctl(PR_SET_NAME)`).
            Ok(s) => s.trim_end_matches(['\n', '\0']),
            Err(_) => continue,
        };
        if APT_PROCESS_COMMS.contains(&comm) {
            return Some(pid);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // ─── dnf lock probe ────────────────────────────────────────

    #[test]
    fn dnf_lock_held_returns_true_when_lock_file_present() {
        let dir = tempdir().unwrap();
        let lock = dir.path().join("rpmtransaction.lock");
        fs::write(&lock, b"").unwrap();
        assert!(dnf_lock_held_at(&lock));
    }

    #[test]
    fn dnf_lock_held_returns_false_when_lock_file_absent() {
        let dir = tempdir().unwrap();
        let lock = dir.path().join("rpmtransaction.lock");
        // Never create the file.
        assert!(!dnf_lock_held_at(&lock));
    }

    // ─── apt lock probe ────────────────────────────────────────

    fn fake_proc_entry(proc_root: &Path, pid: &str, comm: &str) {
        let dir = proc_root.join(pid);
        fs::create_dir_all(&dir).unwrap();
        // Linux appends a trailing newline to /proc/{pid}/comm. Match
        // the real kernel format so the probe's `trim_end_matches('\n')`
        // path is exercised.
        fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
    }

    #[test]
    fn apt_lock_held_detects_running_dpkg_in_proc_scan() {
        let dir = tempdir().unwrap();
        // A few non-apt processes - these must not false-positive.
        fake_proc_entry(dir.path(), "1", "systemd");
        fake_proc_entry(dir.path(), "123", "bash");
        // The real signal.
        fake_proc_entry(dir.path(), "456", "dpkg");
        assert_eq!(apt_lock_owner_from_proc(dir.path()), Some(456));
    }

    #[test]
    fn apt_lock_held_detects_unattended_upgr_truncated_comm() {
        // `/proc/{pid}/comm` is capped at 15 chars on Linux, so
        // `unattended-upgrade` shows as `unattended-upgr`. Regression
        // guard for the truncation.
        let dir = tempdir().unwrap();
        fake_proc_entry(dir.path(), "999", "unattended-upgr");
        assert_eq!(apt_lock_owner_from_proc(dir.path()), Some(999));
    }

    #[test]
    fn apt_lock_held_returns_false_when_no_pkg_mgr_process() {
        let dir = tempdir().unwrap();
        fake_proc_entry(dir.path(), "1", "systemd");
        fake_proc_entry(dir.path(), "2", "kthreadd");
        fake_proc_entry(dir.path(), "123", "bash");
        assert_eq!(apt_lock_owner_from_proc(dir.path()), None);
    }

    #[test]
    fn apt_lock_held_ignores_non_pid_entries() {
        // `/proc` also contains non-numeric names like `self`, `net`,
        // `sys`, `version`. Those must not be scanned.
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("self")).unwrap();
        fs::write(dir.path().join("version"), b"Linux").unwrap();
        // A real PID entry confirms the scan still works:
        fake_proc_entry(dir.path(), "123", "bash");
        assert_eq!(apt_lock_owner_from_proc(dir.path()), None);
    }

    #[test]
    fn apt_lock_held_returns_none_when_proc_root_missing() {
        // PRD Unhappy paths: "treat as 'unknown', proceed". If we
        // cannot read /proc at all (hardened sandbox, filtered mount,
        // catastrophic FS error), we must NOT block the update
        // permanently - `apt_lock_owner_from_proc` returns `None` so
        // the gate falls through and dnf/apt's own `flock(2)` serves
        // as the authoritative lock.
        let dir = tempdir().unwrap();
        let missing = dir.path().join("not-a-proc");
        // Do not create `missing` - `read_dir` will fail.
        assert_eq!(apt_lock_owner_from_proc(&missing), None);
    }

    #[test]
    fn apt_lock_owner_returns_pid_for_matching_process() {
        // Regression guard for the US-005 AC: "raw lock-owner PID is
        // included in the anyhow context for diagnostics". The
        // probe's caller uses the returned PID in the context string.
        let dir = tempdir().unwrap();
        fake_proc_entry(dir.path(), "4242", "apt-get");
        assert_eq!(apt_lock_owner_from_proc(dir.path()), Some(4242));
    }

    #[test]
    fn dnf_lock_held_at_treats_err_as_not_held() {
        // PRD Unhappy paths: "Permissions error reading the lock
        // file: treat as 'unknown', proceed with the update". We
        // cannot easily trigger a real EACCES from userspace without
        // root, but we CAN exercise the Err branch by pointing
        // `try_exists` at a path whose parent component is a
        // regular file (which makes the entire lookup fail with
        // NotADirectory). The branch returns `false` so the gate
        // doesn't block the update spuriously.
        let dir = tempdir().unwrap();
        let not_a_dir = dir.path().join("some_file");
        fs::write(&not_a_dir, b"").unwrap();
        let impossible = not_a_dir.join("rpmtransaction.lock");
        // `try_exists` treats "parent is not a directory" as Err on
        // most Linux kernels; if the host returns Ok(false) instead,
        // the assertion still holds (never held).
        assert!(!dnf_lock_held_at(&impossible));
    }

    // ──────────────────────────────────────────────────────────
    // US-006: version validator, argv builder, exit classifier,
    // and stub-pkexec integration coverage. Each section targets
    // one acceptance criterion from the US-006 checklist so a
    // regression lands as a single failing test with a clear
    // name rather than a vague panic.
    // ──────────────────────────────────────────────────────────

    // ─── validate_version ────────────────────────────────────

    #[test]
    fn validate_version_accepts_semver() {
        // PRD AC: "validate_version_accepts_semver" - both the
        // leading-`v` form (rpm NEVRA convention) and the raw
        // form must pass and return the v-stripped slice.
        assert_eq!(validate_version("v0.2.3").unwrap(), "0.2.3");
        assert_eq!(validate_version("0.2.3").unwrap(), "0.2.3");
        // Larger version numbers must pass (no upper bound).
        assert_eq!(validate_version("v99.100.42").unwrap(), "99.100.42");
    }

    #[test]
    fn validate_version_rejects_malformed() {
        // PRD AC: table of adversarial inputs that must ALL fail
        // before any subprocess is spawned. This is the primary
        // defense against R1 (arg injection via compromised tag).
        for bad in [
            "",                 // empty
            "0.2",              // only two parts
            "0.2.3.4",          // four parts
            "0.2.3-rc1",        // pre-release suffix
            "v0.2.3 && reboot", // whitespace + shell metachars
            "latest",           // non-numeric
            " 0.2.3",           // leading whitespace
            "0.2.3 ",           // trailing whitespace
            "v.2.3",            // empty segment after strip
            ".0.2.3",           // leading dot
            "0..2.3",           // double dot
            "v0.2.3;rm -rf /",  // semicolon injection attempt
        ] {
            assert!(
                validate_version(bad).is_err(),
                "should have rejected {bad:?}"
            );
        }
    }

    #[test]
    fn version_validators_agree() {
        // US-054: `validate_version` (here, Linux-only) and
        // `app::self_update_flow::is_strict_semver` (cross-platform) hand-roll
        // the SAME `^v?\d+\.\d+\.\d+$` grammar in two places. They cannot share
        // an impl without coupling the Linux module to the app module, so this
        // parity test guards them against drifting apart instead.
        use crate::app::self_update_flow::is_strict_semver;
        for case in [
            "v0.2.3",
            "0.2.3",
            "v99.100.42",
            "",
            "0.2",
            "0.2.3.4",
            "0.2.3-rc1",
            "v0.2.3 && reboot",
            "latest",
            " 0.2.3",
            "0.2.3 ",
            "v.2.3",
            ".0.2.3",
            "0..2.3",
            "v0.2.3;rm -rf /",
        ] {
            assert_eq!(
                is_strict_semver(case),
                validate_version(case).is_ok(),
                "validators disagree on {case:?}"
            );
        }
    }

    // ─── build_argv: dnf ──────────────────────────────────────

    #[test]
    fn build_dnf_argv_strips_v_prefix() {
        // Validator normalises the leading `v`; the argv must
        // contain `paneflow-0.2.3`, not `paneflow-v0.2.3`.
        let normalised = validate_version("v0.2.3").unwrap();
        let argv = build_argv(&PackageManager::Dnf, normalised);
        assert!(
            argv.iter().any(|t| t == "paneflow-0.2.3"),
            "argv missing NEVRA form: {argv:?}"
        );
        assert!(
            !argv.iter().any(|t| t.contains("paneflow-v")),
            "argv still has leading v: {argv:?}"
        );
    }

    #[test]
    fn build_dnf_argv_rejects_shell_metachars_via_regex() {
        // PRD R1 mitigation: a compromised GitHub tag like
        // "v0.2.3; rm -rf /" must FAIL validation before
        // build_argv is ever reached. The test confirms the
        // regex-style validator catches it - so there is no path
        // for shell metacharacters to end up in argv.
        assert!(validate_version("v0.2.3; rm -rf /").is_err());
        assert!(validate_version("0.2.3|cat /etc/shadow").is_err());
        assert!(validate_version("0.2.3\n/bin/sh").is_err());
    }

    #[test]
    fn build_dnf_argv_includes_best_and_weak_deps_setopt() {
        // --best fails cleanly when the exact NEVRA is absent
        // instead of silently downgrading; weak-deps=False pulls
        // only the paneflow package, not its suggested deps.
        let argv = build_argv(&PackageManager::Dnf, "0.2.3");
        assert!(argv.iter().any(|t| t == "--best"), "argv: {argv:?}");
        assert!(
            argv.iter().any(|t| t == "--setopt=install_weak_deps=False"),
            "argv: {argv:?}"
        );
    }

    #[test]
    fn build_dnf_argv_puts_refresh_before_install_subcommand() {
        // PRD v1.2: the `--refresh` global flag MUST appear before
        // the `install` subcommand (dnf's global args are positional
        // - a trailing `--refresh` after `install` is rejected as an
        // unknown install-subcommand flag). This test locks the
        // order in so a reformat or autofix never transposes them.
        let argv = build_argv(&PackageManager::Dnf, "0.2.3");
        let refresh_idx = argv
            .iter()
            .position(|t| t == "--refresh")
            .unwrap_or_else(|| panic!("argv missing --refresh: {argv:?}"));
        let install_idx = argv
            .iter()
            .position(|t| t == "install")
            .unwrap_or_else(|| panic!("argv missing install: {argv:?}"));
        assert!(
            refresh_idx < install_idx,
            "--refresh ({refresh_idx}) must come before install ({install_idx}): {argv:?}"
        );
        // Also pin the exact positions we expect - any drift means
        // the canonical argv layout changed and callers of the
        // classifier need re-verification.
        assert_eq!(argv[0], "pkexec");
        assert_eq!(argv[1], "dnf");
        assert_eq!(argv[2], "--refresh");
        assert_eq!(argv[3], "install");
    }

    // ─── build_argv: apt ──────────────────────────────────────

    #[test]
    fn build_apt_argv_uses_equals_version_form() {
        // apt pinning uses `name=version`, NOT the rpm-flavoured
        // `name-version` form. Easy regression to introduce by
        // copy-paste from the Dnf arm. Since PRD v1.2 the apt path
        // is wrapped in `sh -c`, so the pin string lives INSIDE the
        // script body (arg 3) as the literal `"paneflow=$1"` - the
        // Debian package version itself is the positional argv[5].
        let argv = build_argv(&PackageManager::Apt, "0.2.3");
        let script_body = argv
            .get(3)
            .cloned()
            .unwrap_or_else(|| panic!("argv too short: {argv:?}"));
        assert!(
            script_body.contains("\"paneflow=$1\""),
            "script body missing quoted positional pin: {script_body:?}"
        );
        assert!(
            !script_body.contains("paneflow-"),
            "script body used rpm `-` pin form for apt: {script_body:?}"
        );
        // And the runtime Debian package version must be the positional at argv[5].
        assert_eq!(argv.get(5).map(String::as_str), Some("0.2.3-1"));
    }

    #[test]
    fn build_apt_argv_includes_no_install_recommends() {
        // In the sh -c shape, `--no-install-recommends` lives inside
        // the script body (arg 3), not as a standalone argv token.
        // Check via substring of arg 3 rather than `iter().any()`.
        let argv = build_argv(&PackageManager::Apt, "0.2.3");
        let script_body = argv
            .get(3)
            .cloned()
            .unwrap_or_else(|| panic!("argv too short: {argv:?}"));
        assert!(
            script_body.contains("--no-install-recommends"),
            "script body missing --no-install-recommends: {script_body:?}"
        );
    }

    #[test]
    fn build_apt_argv_wraps_in_sh_c_with_positional_version() {
        // PRD v1.2 exact-match lock: the apt argv is a FIXED shape.
        // - argv[0] = "pkexec"          (elevation wrapper)
        // - argv[1] = "sh"               (POSIX shell for && chaining)
        // - argv[2] = "-c"               (read script from next arg)
        // - argv[3] = script body        (constant - NO interpolation)
        // - argv[4] = "_"                (conventional $0 placeholder)
        // - argv[5] = Debian version    (the variable - positional $1)
        //
        // If a refactor moves the version into the script body (via
        // `format!`) or reorders any of these, this test fails.
        let argv = build_argv(&PackageManager::Apt, "0.2.3");
        let expected = vec![
            "pkexec".to_string(),
            "sh".to_string(),
            "-c".to_string(),
            "apt-get update -q && apt-get install -y --no-install-recommends \"paneflow=$1\""
                .to_string(),
            "_".to_string(),
            "0.2.3-1".to_string(),
        ];
        assert_eq!(argv, expected, "apt argv shape drifted from PRD v1.2 spec");
    }

    #[test]
    fn build_apt_argv_passes_version_as_positional_not_interpolated() {
        // PRD v1.2 defense-in-depth invariant: even if a future
        // caller bypasses `validate_version` and feeds `build_argv`
        // a string containing shell metacharacters, the builder
        // MUST treat the version purely as argv data and NEVER
        // interpolate it into the script body.
        //
        // We simulate the bypass by calling `build_argv` directly
        // with a string that would never clear the regex. The sh -c
        // execution path with `"$1"` double-quoted expansion means
        // the shell treats the value as literal data, not as a
        // command stream - this test pins that invariant at the
        // unit-test level, independent of the regex validator.
        let malicious = "0.2.3\"; echo pwned; #";
        let argv = build_argv(&PackageManager::Apt, malicious);

        // 1. Version lands AT argv[5] as a whole element - never
        //    split, never merged into another argv slot.
        assert_eq!(
            argv.get(5).map(String::as_str),
            Some("0.2.3\"; echo pwned; #-1"),
            "version plus Debian revision must be argv[5] verbatim: {argv:?}"
        );

        // 2. The script body (argv[3]) MUST NOT contain any part of
        //    the malicious version string. If it did, it would mean
        //    the builder is interpolating the version into the shell
        //    body - which defeats the `"$1"` positional safety.
        let script_body = argv
            .get(3)
            .cloned()
            .unwrap_or_else(|| panic!("argv too short: {argv:?}"));
        assert!(
            !script_body.contains("echo"),
            "script body was poisoned with version content: {script_body:?}"
        );
        assert!(
            !script_body.contains("pwned"),
            "script body was poisoned with version content: {script_body:?}"
        );
        assert!(
            !script_body.contains("0.2.3"),
            "script body must not embed ANY version substring: {script_body:?}"
        );

        // 3. The script body remains the expected constant literal.
        assert_eq!(
            script_body,
            "apt-get update -q && apt-get install -y --no-install-recommends \"paneflow=$1\"",
            "script body must be the canonical constant string"
        );
    }

    // ─── build_argv: zypper ──────────────────────────────────────

    #[test]
    fn build_zypper_argv_wraps_refresh_and_install_in_one_pkexec_prompt() {
        let argv = build_argv(&PackageManager::Zypper, "0.2.3");
        let expected = vec![
            "pkexec".to_string(),
            "sh".to_string(),
            "-c".to_string(),
            "zypper --non-interactive --gpg-auto-import-keys refresh && zypper --non-interactive install --no-recommends --force \"paneflow=$1\""
                .to_string(),
            "_".to_string(),
            "0.2.3".to_string(),
        ];
        assert_eq!(
            argv, expected,
            "zypper argv shape drifted from the single-prompt install spec"
        );
    }

    #[test]
    fn build_zypper_argv_passes_version_as_positional_not_interpolated() {
        let malicious = "0.2.3\"; echo pwned; #";
        let argv = build_argv(&PackageManager::Zypper, malicious);

        assert_eq!(
            argv.get(5).map(String::as_str),
            Some("0.2.3\"; echo pwned; #"),
            "version must be argv[5] verbatim: {argv:?}"
        );
        let script_body = argv
            .get(3)
            .cloned()
            .unwrap_or_else(|| panic!("argv too short: {argv:?}"));
        assert!(
            !script_body.contains("echo") && !script_body.contains("pwned"),
            "script body was poisoned with version content: {script_body:?}"
        );
        assert!(
            script_body.contains("\"paneflow=$1\""),
            "script body missing quoted positional pin: {script_body:?}"
        );
    }

    // ─── classify_exit ────────────────────────────────────────

    #[test]
    fn classify_exit_status_maps_126_to_install_declined() {
        let err = classify_exit(Some(126), None, &[], "dnf");
        assert!(
            matches!(err, UpdateError::InstallDeclined { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn classify_exit_status_maps_127_to_environment_broken() {
        let err = classify_exit(Some(127), None, &[], "dnf");
        assert!(
            matches!(err, UpdateError::EnvironmentBroken { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn classify_exit_status_maps_128_plus_n_to_other_signal() {
        // Shell-style exit-code-from-signal propagation (pkexec / bash
        // chains): 137 = SIGKILL (9), 143 = SIGTERM (15), 130 =
        // Ctrl-C (SIGINT, 2). Must classify as `Other("killed by
        // signal N")` to match the direct-signal UX, not the generic
        // InstallFailed toast.
        for (code, sig) in [(137, 9), (143, 15), (130, 2)] {
            let err = classify_exit(Some(code), None, &[], "dnf");
            match err {
                UpdateError::Other(msg) => {
                    assert!(
                        msg.contains(&format!("signal {sig}")),
                        "code {code}: got {msg}"
                    );
                }
                other => panic!("code {code}: got {other:?}"),
            }
        }
    }

    #[test]
    fn classify_exit_status_above_159_still_install_failed() {
        // Regression guard: codes at the top of the u8 range (255,
        // 200) must NOT be misread as "signal 72" / "signal 127". Only
        // the 129..=159 window is treated as a signal-propagation
        // hint.
        for code in [160_i32, 200, 255] {
            let err = classify_exit(Some(code), None, &[], "dnf");
            assert!(
                matches!(err, UpdateError::InstallFailed { .. }),
                "code {code}: got {err:?}"
            );
        }
    }

    #[test]
    fn classify_exit_status_maps_nonzero_to_install_failed() {
        // Any exit code that isn't 0, 126, 127, or signal-kill
        // routes to InstallFailed with an empty log_path (the
        // CLI package managers don't produce a standalone log
        // file; stderr goes to log::debug inside the runner).
        for code in [1_i32, 2, 99, 255] {
            let err = classify_exit(Some(code), None, &[], "dnf");
            match err {
                UpdateError::InstallFailed { log_path } => {
                    assert!(log_path.as_os_str().is_empty(), "code {code}: path set");
                }
                other => panic!("code {code}: got {other:?}"),
            }
        }
    }

    // ─── stub pkexec integration ─────────────────────────────

    /// Create a minimal bash script that exits with `exit_code` and
    /// ignores all argv. Returns `(tempdir, script_path)` - the
    /// tempdir must stay alive for the duration of the test so
    /// the script file is not deleted out from under `Command`.
    ///
    /// The write is an explicit `File::create + write_all +
    /// sync_all + drop` rather than the terser `fs::write`, and
    /// permissions are set AFTER the parent handle is closed. On Linux,
    /// `exec(2)` returns `ETXTBSY` ("Text file busy", OS error 26) if
    /// any process has a write handle open to the target file. A spawn
    /// concurrent with this write window can fork and inherit that
    /// handle until its own exec, even after the parent drops its copy.
    /// The harness therefore retries this transient at the spawn seam.
    fn make_stub_pkexec(exit_code: i32) -> (tempfile::TempDir, PathBuf) {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let script_path = dir.path().join("pkexec");
        // `exec 0<&- 1>/dev/null 2>/dev/null` closes the pipes
        // quickly so the parent's mpsc drain completes without
        // waiting for the shell to flush buffered output.
        let script =
            format!("#!/usr/bin/env bash\nexec 0<&- 1>/dev/null 2>/dev/null\nexit {exit_code}\n");
        {
            let mut file = fs::File::create(&script_path).unwrap();
            file.write_all(script.as_bytes()).unwrap();
            file.sync_all().unwrap();
        } // file dropped and fully closed here; releases write handle.
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
        (dir, script_path)
    }

    /// Retry a test-harness operation only when its anyhow chain contains
    /// `std::io::Error` 26 (`ETXTBSY`). A fixed deadline bounds contention
    /// from the parallel test runner without hiding unrelated failures.
    fn retry_etxtbsy<T>(mut operation: impl FnMut() -> Result<T>) -> Result<T> {
        const ETXTBSY: i32 = 26;
        const RETRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
        const RETRY_POLL: std::time::Duration = std::time::Duration::from_millis(5);

        let deadline = std::time::Instant::now() + RETRY_TIMEOUT;
        loop {
            let result = operation();
            let should_retry = result.as_ref().err().is_some_and(|err| {
                err.chain().any(|cause| {
                    cause
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|io| io.raw_os_error() == Some(ETXTBSY))
                })
            });
            if !should_retry {
                return result;
            }

            let now = std::time::Instant::now();
            if now >= deadline {
                return result;
            }
            std::thread::sleep(RETRY_POLL.min(deadline.saturating_duration_since(now)));
            if std::time::Instant::now() >= deadline {
                return result;
            }
        }
    }

    /// Keep the pkexec integration cases focused on exit classification while
    /// [`retry_etxtbsy`] owns the transient executable-stub race.
    fn run_update_impl_retry(
        manager: &PackageManager,
        version: &str,
        pkexec_installed: bool,
        pkexec_spawn_path: &Path,
    ) -> Result<()> {
        retry_etxtbsy(|| run_update_impl(manager, version, pkexec_installed, pkexec_spawn_path))
    }

    #[test]
    fn run_update_short_circuits_when_pkexec_missing() {
        // US-006 AC: "stub dir contains no pkexec; `which` returns
        // Err; `run_update` returns EnvironmentBroken without
        // spawning anything." We exercise this directly via
        // `run_update_impl` rather than mutating $PATH (racy under
        // `cargo test`'s parallel harness; `std::env::set_var` is
        // `unsafe` in the 2024 edition) - the path is set to a
        // non-existent binary that would panic if anything tried
        // to exec it.
        let result = run_update_impl(
            &PackageManager::Dnf,
            "0.2.3",
            false, // pkexec NOT installed
            Path::new("/nonexistent/pkexec-never-spawned"),
        );
        let err = result.unwrap_err();
        match UpdateError::classify(&err) {
            UpdateError::EnvironmentBroken { .. } => {}
            other => panic!("expected EnvironmentBroken, got {other:?}"),
        }
    }

    #[test]
    fn stub_pkexec_exit_0_returns_ok() {
        let (_dir, stub) = make_stub_pkexec(0);
        let result = run_update_impl_retry(&PackageManager::Dnf, "0.2.3", true, &stub);
        assert!(result.is_ok(), "got {result:?}");
    }

    #[test]
    fn stub_pkexec_exit_126_maps_to_install_declined() {
        let (_dir, stub) = make_stub_pkexec(126);
        let err = run_update_impl_retry(&PackageManager::Dnf, "0.2.3", true, &stub).unwrap_err();
        match UpdateError::classify(&err) {
            UpdateError::InstallDeclined { .. } => {}
            other => panic!("expected InstallDeclined, got {other:?}"),
        }
    }

    #[test]
    fn stub_pkexec_exit_127_maps_to_environment_broken() {
        let (_dir, stub) = make_stub_pkexec(127);
        let err = run_update_impl_retry(&PackageManager::Dnf, "0.2.3", true, &stub).unwrap_err();
        match UpdateError::classify(&err) {
            UpdateError::EnvironmentBroken { .. } => {}
            other => panic!("expected EnvironmentBroken, got {other:?}"),
        }
    }

    #[test]
    fn stub_pkexec_exit_1_maps_to_install_failed() {
        let (_dir, stub) = make_stub_pkexec(1);
        let err = run_update_impl_retry(&PackageManager::Dnf, "0.2.3", true, &stub).unwrap_err();
        match UpdateError::classify(&err) {
            UpdateError::InstallFailed { log_path } => {
                assert!(log_path.as_os_str().is_empty());
            }
            other => panic!("expected InstallFailed, got {other:?}"),
        }
    }

    #[test]
    fn stub_pkexec_exit_42_also_maps_to_install_failed() {
        // Regression guard: any "not 0/126/127" non-zero code
        // must land in InstallFailed, not get silently reclassified.
        let (_dir, stub) = make_stub_pkexec(42);
        let err = run_update_impl_retry(&PackageManager::Dnf, "0.2.3", true, &stub).unwrap_err();
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::InstallFailed { .. }
        ));
    }

    #[test]
    fn stub_pkexec_rejects_malformed_version_before_spawn() {
        // Sanity check: even with a "happy" stub script that would
        // exit 0, a bad version string must cause rejection BEFORE
        // the spawn ever happens. No way for a crafted tag to
        // reach argv.
        let (_dir, stub) = make_stub_pkexec(0);
        let err = run_update_impl_retry(&PackageManager::Dnf, "v0.2.3; rm -rf $HOME", true, &stub)
            .unwrap_err();
        match UpdateError::classify(&err) {
            UpdateError::Other(msg) => {
                assert!(
                    msg.contains("Invalid version string"),
                    "message missing expected prefix: {msg}"
                );
            }
            other => panic!("expected Other(Invalid version string …), got {other:?}"),
        }
    }

    #[test]
    fn public_run_update_wrapper_delegates_validation() {
        // Smoke test for the public `run_update` wrapper itself
        // (the 5-line delegator that US-006 introduced). The 17
        // tests above all exercise `run_update_impl` directly,
        // which means the wrapper could silently regress (e.g., a
        // `.is_ok()` → `.is_err()` flip) without any test failing.
        //
        // The test drives the public entry point with a malformed
        // version string; `validate_version` fires before the
        // `pkexec_installed` check, so the expected error is
        // `UpdateError::Other("Invalid version string: …")`
        // regardless of whether real pkexec is installed on the
        // host. This proves the public wrapper is at least alive
        // and its delegation compiles.
        let err = run_update(&PackageManager::Dnf, "not-a-version").unwrap_err();
        match UpdateError::classify(&err) {
            UpdateError::Other(msg) => {
                assert!(
                    msg.contains("Invalid version string"),
                    "public wrapper did not reject malformed version: {msg}"
                );
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }
}
