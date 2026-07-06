//! Run the real AI binary + Unix signal handling (US-052 split).

use std::env;
use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;
// The interrupt path (`install_sigint_watcher` on Unix, `install_ctrl_c_handler`
// on Windows, and the shared `send_interrupt_stop`) consumes these. Gate them to
// unix+windows so a hypothetical bare target without either doesn't flag them
// unused; the shim only ships on unix + windows.
#[cfg(any(unix, windows))]
use crate::{
    locate_sibling_hook_binary, PANEFLOW_AI_EVENT_SOURCE_ENV, PANEFLOW_AI_EVENT_SOURCE_INTERRUPT,
};
#[cfg(any(unix, windows))]
use std::io::Write;

// ---------------------------------------------------------------------------
// Run chain - spawn the real AI binary and wait for it
// ---------------------------------------------------------------------------
//

// US-004 originally used `CommandExt::exec()` on Unix for zero-fork process
// replacement. US-005 introduced the `HookConfigGuard` drop-cleanup contract,
// which is incompatible with `exec()` - process replacement skips every Rust
// destructor, so the guard would never fire. Both platforms now use
// `Command::status()`; the shim pays one fork (~1-3 ms, well under the 15 ms
// budget) in exchange for reliable cleanup.
//
// `Command` inherits the parent env by default, so `.envs(env::vars_os())`
// is redundant - but the PRD AC bullet 5 lists it explicitly to make the
// env-pass-through contract discoverable in the source. The `.env(...)`
// calls afterward shadow per-key (Command::env is last-write-wins).
//
// PANEFLOW_AI_TOOL - set so `paneflow-ai-hook` (US-003) can tag every
// outbound IPC frame with the right tool identity (`claude` vs `codex`).
// Without this, `paneflow-ai-hook::detect_tool_from(None)` defaults to
// `TOOL_CLAUDE`, which makes the sidebar render "Claude thinking…" for
// every Codex turn - visible regression observed in the field.

// EP-004 US-010 (cli-cockpit): `run_real` also returns the agent binary's RAW
// exit code (`Some(i32)`, shell `128+signum` convention for signal deaths) so
// `main` can emit the `ai.exit` IPC frame. `None` means the agent never ran
// (spawn failure) or its status is unknown (wait failure) - no frame is
// emitted and the server keeps today's `ai.stop`-driven behavior.

pub(crate) fn run_real(tool: &str, path: &Path, args: &[OsString]) -> (ExitCode, Option<i32>) {
    let mut cmd = std::process::Command::new(path);
    cmd.args(args)
        .envs(env::vars_os())
        .env("PANEFLOW_AI_TOOL", tool)
        // PANEFLOW_AI_PID - stable session identity propagated to every
        // `paneflow-ai-hook` invocation fired by claude/codex during this
        // session. Without it, the server's `Workspace::agent_sessions`
        // (keyed by PID) collapses every Claude Code into one entry, so
        // the sidebar shows `Claude thinking` for two concurrent sessions
        // instead of `Claude thinking +1`. We use the shim's own PID
        // (process::id()) rather than the child's because (a) the child
        // PID isn't known until after spawn - too late for an env var on
        // Command - and (b) the shim outlives the child via `waitpid`,
        // so the PID stays reachable for the stale-PID sweep.
        .env("PANEFLOW_AI_PID", std::process::id().to_string());

    // Unix only: reset signal disposition + unblock SIGINT in the child.
    //
    // Required because Rust's `Command` inherits the parent's signal mask
    // and dispositions across `execve`. The parent installs:
    //   - `SIG_IGN` for SIGHUP/SIGTERM (shim survives PTY close / kill)
    //   - `SIG_BLOCK` mask for SIGINT (consumed synchronously by the
    //     `sigwait` thread in `install_sigint_watcher`, so the shim can
    //     emit an `ai.stop` IPC frame on every Ctrl+C - including
    //     mid-response interrupts where claude/codex intentionally fire
    //     no `Stop` hook of their own).
    //
    // Without this `pre_exec` reset+unblock, the child would inherit both
    // and Ctrl+C would do absolutely nothing (the AI would never see it,
    // since `SIG_BLOCK`'d signals on a Linux process stay blocked across
    // `execve`).
    //
    // `pre_exec` runs in the forked child between fork() and execve(). All
    // calls below are async-signal-safe.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        // US-037: capture the shim's own PID in the parent, before fork. Inside
        // `pre_exec` (post-fork, in the child) `std::process::id()` would return
        // the child's PID, so the parent PID must be captured here and moved into
        // the closure to detect reparenting below.
        #[cfg(target_os = "linux")]
        let shim_pid = std::process::id();
        cmd.pre_exec(move || {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGINT);
            libc::sigaddset(&mut set, libc::SIGHUP);
            libc::sigaddset(&mut set, libc::SIGTERM);
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());

            // US-016 (cli-hardening-followup-2026-Q3): on Linux,
            // install PR_SET_PDEATHSIG so the spawned agent CLI
            // (claude/codex/opencode) is killed by the kernel as
            // soon as the shim's parent process (Paneflow) dies --
            // even on a hard `kill -9` of Paneflow that bypasses
            // any graceful Drop discipline. Without this, the agent
            // is reparented to PID 1 and keeps streaming, burning
            // the user's API tokens until its natural timeout.
            //
            // `parent_guard.rs` documents this gap on Linux/macOS
            // because the GPUI app spawns through
            // `portable-pty::CommandBuilder` which does not expose
            // `pre_exec`. The shim wraps ~80% of those spawns
            // (claude/codex/opencode all go through it) so this
            // covers the realistic Linux path. macOS uses a stub
            // pending kqueue NOTE_EXIT (out of US-016 scope).
            //
            // SAFETY: prctl is async-signal-safe; the call happens
            // in the forked child between fork() and execve().
            // A non-zero return is rare (only on a stripped kernel
            // without prctl PR_SET_PDEATHSIG support) and is best-
            // effort: we emit nothing on stderr because writing
            // from pre_exec is not async-signal-safe in general,
            // and we explicitly do NOT abort the exec -- letting
            // the child run without PDEATHSIG is strictly better
            // than failing the spawn entirely.
            #[cfg(target_os = "linux")]
            {
                let _ = libc::prctl(
                    libc::PR_SET_PDEATHSIG,
                    libc::SIGKILL as libc::c_ulong,
                    0,
                    0,
                    0,
                );
                // US-037: close the fork↔prctl race. PDEATHSIG only fires on
                // a parent death that happens AFTER it's armed; if the shim
                // (our parent) already died in the window between fork() and
                // this prctl, the kernel never delivers the signal. A getppid()
                // that no longer matches the shim's captured PID means we were
                // already reparented - to init (PID 1) OR, on a host where
                // Paneflow runs under `systemd --user` (a PR_SET_CHILD_SUBREAPER
                // reaper), to the user manager whose PID is not 1. Comparing to
                // the captured `shim_pid` rather than the literal 1 catches the
                // subreaper case that `== 1` silently misses on modern Linux
                // desktops, so an orphaned agent self-terminates instead of
                // streaming on and burning the user's API tokens. Both calls are
                // async-signal-safe.
                if libc::getppid() as u32 != shim_pid {
                    libc::raise(libc::SIGKILL);
                }
            }
            Ok(())
        });
    }

    // Install signal isolation BEFORE spawn so the child inherits the
    // mask/dispositions at fork (then `pre_exec` flips them back for the
    // child only). Doing this BEFORE `cmd.spawn()` closes the race window
    // where a Ctrl+C could land between spawn and signal-install.
    #[cfg(unix)]
    ignore_terminal_signals();
    #[cfg(unix)]
    install_sigint_watcher(tool);
    // EP-005 US-017: Windows Ctrl+C -> `ai.stop` (claude/codex interrupt their
    // turn without exiting or firing a Stop hook). Installed before spawn so an
    // immediate Ctrl+C is already covered.
    #[cfg(windows)]
    install_ctrl_c_handler(tool);

    // EP-005 US-017: capture Paneflow's PID before spawn for the macOS orphan
    // guard's reparent detection. SAFETY: `getppid` is a trivial syscall.
    #[cfg(target_os = "macos")]
    let parent_pid = unsafe { libc::getppid() } as u32;
    // EP-005 US-017: flipped the instant `child.wait()` reaps the agent, so the
    // guard thread stops probing/signalling before the OS can recycle the child
    // PID - otherwise a late `kill(child_pid, …)` could hit an unrelated same-UID
    // process (mis-kill) or never return (the probe sees a recycled-live PID).
    #[cfg(target_os = "macos")]
    let child_reaped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("paneflow-shim: spawn '{}' failed: {e}", path.display());
            return (ExitCode::from(127), None);
        }
    };

    // EP-005 US-017: macOS parity for the Linux `PR_SET_PDEATHSIG` guard - kill
    // the agent if Paneflow dies, so a `kill -9` of Paneflow leaves no orphan.
    // Spawned after we hold the child PID.
    #[cfg(target_os = "macos")]
    spawn_parent_death_guard(child.id(), parent_pid, std::sync::Arc::clone(&child_reaped));

    let wait_result = child.wait();
    // EP-005 US-017: tell the guard the agent is reaped BEFORE its PID can be
    // recycled, so its next tick returns without touching a possibly-reused PID.
    #[cfg(target_os = "macos")]
    child_reaped.store(true, std::sync::atomic::Ordering::Release);
    match wait_result {
        Ok(status) => (
            exit_code_from_status(&status),
            Some(raw_exit_code_from_status(&status)),
        ),
        Err(e) => {
            eprintln!("paneflow-shim: wait on '{}' failed: {e}", path.display());
            (ExitCode::from(1), None)
        }
    }
}

/// US-037: map a child `ExitStatus` to this process's `ExitCode`.
///
/// `status.code()` is `None` only when the child was terminated by a signal
/// (Unix). The shell convention `128 + signum` (used by bash, `time(1)`, etc.)
/// lets the parent terminal see the real cause (e.g. 130 for SIGINT, 139 for
/// SIGSEGV) instead of an opaque `1`. Extracted to one place so the three
/// `wait()` call sites stay consistent. `u8::try_from` clamps out-of-range
/// codes to `1`.
pub(crate) fn exit_code_from_status(status: &std::process::ExitStatus) -> ExitCode {
    if let Some(code) = status.code() {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            let code = 128i32.saturating_add(sig);
            return ExitCode::from(u8::try_from(code).unwrap_or(1));
        }
    }
    ExitCode::from(1)
}

/// EP-004 US-010: the same mapping, kept as a full-width `i32` for the
/// `ai.exit` IPC frame. Unlike [`exit_code_from_status`] there is no `u8`
/// clamp: the server's classifier needs the verbatim value (e.g. Windows
/// `STATUS_CONTROL_C_EXIT` = `-1073741510`, which a `u8` clamp would fold
/// into an indistinguishable `1`). Unix signal deaths use the same shell
/// `128 + signum` convention (130 = SIGINT, 139 = SIGSEGV, …).
pub(crate) fn raw_exit_code_from_status(status: &std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128i32.saturating_add(sig);
        }
    }
    1
}

/// Make the shim survive PTY-close + kill signals so the child
/// (claude/codex) can handle them without taking us down with it.
///
/// SIGINT is intentionally NOT in this list - it's handled by
/// `install_sigint_watcher` via `sigwait` so we can emit a per-interrupt
/// `ai.stop` IPC frame (mid-response Ctrl+C interrupts fire no hook from
/// claude/codex, so this is the only signal we have).
#[cfg(unix)]
pub(crate) fn ignore_terminal_signals() {
    // SAFETY: `libc::signal` with `SIG_IGN` is async-signal-safe and only
    // mutates the kernel signal disposition table for the current process.
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
}

/// Block SIGINT in the shim, then spawn a dedicated thread that
/// `sigwait`s on it. On every Ctrl+C, send `ai.stop` to PaneFlow so the
/// sidebar loader transitions to `Finished` (then auto-resets to
/// `Inactive` after 5s server-side). This is the ONLY way to detect a
/// mid-stream interrupt because:
///   - Claude Code does not fire its `Stop` hook when a turn is
///     interrupted (only on natural completion).
///   - Codex does not fire any hook on `esc`/Ctrl+C either.
///
/// `sigwait` is the POSIX-correct synchronous-from-thread receive: no
/// async-signal-safety constraints, no self-pipe trick. Standard pattern
/// (see Stevens APUE §12.8 "pthread_sigmask").
#[cfg(unix)]
pub(crate) fn install_sigint_watcher(tool: &str) {
    // SAFETY: `pthread_sigmask` is thread-safe and only mutates the
    // calling thread's signal mask. Blocking SIGINT here propagates to
    // every thread spawned afterward (POSIX inheritance rule). The
    // `pre_exec` hook in `run_real` re-unblocks SIGINT in the child.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }

    let tool = tool.to_owned();
    let hook_path = locate_sibling_hook_binary();
    std::thread::spawn(move || {
        let Some(hook_path) = hook_path else {
            return;
        };
        loop {
            // SAFETY: `sigwait` blocks the calling thread until one of the
            // signals in `set` is delivered to the process. Returns 0 on
            // success and writes the received signal into `sig`. Spurious
            // wakeups are not part of the POSIX contract; if it ever does
            // return non-zero, exit the loop (the shim continues running,
            // we just lose interrupt-driven notifications for this
            // session - graceful degradation per PRD C4).
            let sig = unsafe {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGINT);
                let mut sig: libc::c_int = 0;
                if libc::sigwait(&set, &mut sig) != 0 {
                    return;
                }
                sig
            };
            if sig == libc::SIGINT {
                send_interrupt_stop(&hook_path, &tool);
            }
        }
    });
}

/// Ceiling on concurrent detached reaper threads (U-025). A user mashing
/// Ctrl+C while the IPC peer is wedged would otherwise leak one stuck thread +
/// child per keypress with no bound. Past this many in-flight reapers we drop
/// the stop (this one Ctrl+C just doesn't clear the loader) rather than grow
/// threads unboundedly. 8 covers any realistic burst of legitimate, fast hooks.
#[cfg(any(unix, windows))]
const MAX_INFLIGHT_REAPERS: usize = 8;

/// Live count of detached reaper threads spawned by [`send_interrupt_stop`].
#[cfg(any(unix, windows))]
static INFLIGHT_REAPERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Spawn `paneflow-ai-hook Stop` with `{}` piped to stdin. Best-effort;
/// any failure is silent (worst case: this Ctrl+C doesn't clear the
/// loader, but the shim and the child remain unaffected).
///
/// Reaping policy: the wait happens on a detached helper thread, NOT on
/// the calling sigwait thread. If the hook hangs (socket back-pressure,
/// filesystem stall) the reaper thread hangs with it - but the sigwait
/// thread stays responsive, so the next Ctrl+C lands as a fresh `ai.stop`
/// rather than queuing behind the previous one. Without the helper, a
/// dropped `Child` would leak a zombie until shim exit.
///
/// U-025: the reaper count is bounded by [`MAX_INFLIGHT_REAPERS`] - once that
/// many hooks are simultaneously stuck, further Ctrl+C stops are dropped so a
/// wedged peer can't drive unbounded thread/child growth. We deliberately keep
/// the `{}`-on-stdin contract (so the hook reads a valid empty payload) and do
/// NOT kill the hook on a deadline: a slow-but-progressing socket write is a
/// legitimate stop we don't want to interrupt.
#[cfg(any(unix, windows))]
pub(crate) fn send_interrupt_stop(hook_path: &Path, tool: &str) {
    use std::sync::atomic::Ordering;

    // Reserve a reaper slot up front; back out (and drop this stop) if the
    // ceiling is already reached. fetch_add returns the prior value.
    if INFLIGHT_REAPERS.fetch_add(1, Ordering::AcqRel) >= MAX_INFLIGHT_REAPERS {
        INFLIGHT_REAPERS.fetch_sub(1, Ordering::AcqRel);
        return;
    }

    let spawned = std::process::Command::new(hook_path)
        .arg("Stop")
        .env("PANEFLOW_AI_TOOL", tool)
        .env("PANEFLOW_AI_PID", std::process::id().to_string())
        .env(
            PANEFLOW_AI_EVENT_SOURCE_ENV,
            PANEFLOW_AI_EVENT_SOURCE_INTERRUPT,
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        // Spawn failed: release the reserved slot.
        INFLIGHT_REAPERS.fetch_sub(1, Ordering::AcqRel);
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"{}");
    }
    std::thread::spawn(move || {
        let _ = child.wait();
        INFLIGHT_REAPERS.fetch_sub(1, Ordering::AcqRel);
    });
}

/// EP-005 US-017 (agent-control-plane): Windows parity for the Unix `sigwait`
/// watcher. claude/codex interrupt their current TURN on Ctrl+C without exiting
/// and without firing a Stop hook, so the sidebar loader would stick forever. A
/// console-control handler emits one `ai.stop` per Ctrl+C; the agent still
/// receives `CTRL_C_EVENT` directly from the OS (every process in the console
/// group does), so its turn is interrupted as usual and the shim survives to
/// keep waiting on it. `ctrlc` wraps `SetConsoleCtrlHandler` and runs the
/// closure on a dedicated thread, so the same bounded-reaper `send_interrupt_stop`
/// the Unix path uses is reused verbatim.
///
/// Not built into the host (Linux) binary; Linux and macOS take the `sigwait` /
/// parent-death paths. Compile-verified by `cargo check --target
/// x86_64-pc-windows-msvc`, but a Windows RUNTIME smoke test (Ctrl+C an agent
/// mid-turn, confirm the loader clears) is still required before it is trusted.
#[cfg(windows)]
pub(crate) fn install_ctrl_c_handler(tool: &str) {
    let tool = tool.to_owned();
    let Some(hook_path) = locate_sibling_hook_binary() else {
        return;
    };
    // `set_handler` installs once per process; the shim calls this once per run
    // before the agent spawns. The closure runs on every subsequent Ctrl+C.
    let _ = ctrlc::set_handler(move || {
        send_interrupt_stop(&hook_path, &tool);
    });
}

/// EP-005 US-017 (agent-control-plane): macOS parity for the Linux
/// `PR_SET_PDEATHSIG` guard (the `exec.rs` stub). When Paneflow is hard-killed
/// (`kill -9`, bypassing every graceful Drop), the shim is reparented to
/// `launchd` and the agent it spawned would otherwise keep streaming and burn
/// the user's API tokens. kqueue `NOTE_EXIT` is the textbook idiom, but the AC
/// allows "ou équivalent": a tiny thread that polls `getppid()` is the same
/// reparent-detection the Linux US-037 race-close already uses (`getppid() !=
/// captured parent`), with no fragile `kevent` struct FFI. `parent_pid` is
/// captured BEFORE the spawn. On a detected reparent the agent is `SIGKILL`ed;
/// the loop also exits once the agent is already gone, so the thread never
/// outlives the work.
///
/// Not built into the host (Linux) binary. Compile-verified by `cargo check
/// --target x86_64-apple-darwin`, but a macOS RUNTIME smoke test (`kill -9`
/// Paneflow, confirm no orphaned agent survives) is still required before it is
/// trusted.
#[cfg(target_os = "macos")]
pub(crate) fn spawn_parent_death_guard(
    child_pid: u32,
    parent_pid: u32,
    child_reaped: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            // Once `run_real` has reaped the agent (`child.wait()` returned), the
            // child PID is recyclable by the OS. Bail before probing or signalling
            // it: otherwise a `kill(child_pid, SIGKILL)` could hit an unrelated
            // same-UID process, or the liveness probe could see a recycled-live
            // PID and never return (a leaked thread per agent run). The PARENT-pid
            // reparent check is immune to this (a parent is never our child's
            // recycled PID), but the CHILD-pid probe is not - hence this guard.
            if child_reaped.load(Ordering::Acquire) {
                return;
            }
            // SAFETY: `getppid`/`kill` are async-signal-safe, argument-free or
            // scalar-argument syscalls with no pointer aliasing.
            //
            // A changed parent means Paneflow exited and the kernel reparented
            // us to launchd (PID 1) - never a recycled PID, since reparenting
            // always targets the subreaper. The agent is not yet reaped (checked
            // above; `run_real` is still blocked in `child.wait()` on the live
            // orphan), so `child_pid` is unambiguously the agent: SIGKILL + stop.
            let reparented = unsafe { libc::getppid() } as u32 != parent_pid;
            if reparented {
                unsafe {
                    libc::kill(child_pid as libc::pid_t, libc::SIGKILL);
                }
                return;
            }
            // `kill(pid, 0)` probes liveness without signalling; a non-zero
            // return (ESRCH) means the agent already exited, so stop polling.
            let agent_gone = unsafe { libc::kill(child_pid as libc::pid_t, 0) } != 0;
            if agent_gone {
                return;
            }
        }
    });
}
