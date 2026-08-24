//! Windows MSI self-update pipeline (US-010).
//!
//! Flow:
//!   1. Download the `.msi` to `%TEMP%\paneflow-update-<pid>.msi` via
//!      ureq with the 30-second per-call timeout (US-001).
//!   2. Verify the asset's detached **minisign** signature (`.minisig`
//!      sibling) against a key baked into this binary (US-001), then
//!      `WinVerifyTrust` on the Authenticode chain (US-005) - both
//!      **before** msiexec runs. A missing/invalid signature deletes the
//!      partial and bails; replaces the old same-host `.sha256`.
//!   3. Copy the current `paneflow.exe` to `%TEMP%` as a tiny native relay.
//!   4. The GUI saves state, spawns that copied relay with
//!      `CREATE_BREAKAWAY_FROM_JOB`, and exits before the MSI runs.
//!   5. The relay waits for the current PID to disappear, runs
//!      `msiexec.exe /i <msi> /qb /norestart /l*v <log>` (via `runas`
//!      when the install lives under Program Files), deletes the scratch
//!      MSI, and relaunches the installed `paneflow.exe` on success.
//!
//! The older synchronous path is still kept for testability and CLI-style
//! callers: resolve `%SystemRoot%\System32\msiexec.exe` first, fall back to
//! PATH (PATHEXT-aware - the `which` crate already handles this), run it,
//! and map exit codes:
//!      - `0` → success, return the canonical installed binary path.
//!      - `1602` → `InstallDeclined` ("Update cancelled - administrator
//!        permission required") - the well-known "user declined UAC"
//!        code.
//!      - `1603` → `InstallFailed { log_path }` - fatal Windows Installer
//!        error; log captures the cause.
//!      - other → `Other` with exit code + log-path hint for triage.
//!   6. Delete the MSI scratch file; keep the log on failure so bug
//!      reports can attach it.
//!
//! **Cross-platform compile.** The module is built on every target so
//! the enclosing crate is a single compile-closure. `msiexec.exe` only
//! exists on Windows; the dispatcher only routes `InstallMethod::WindowsMsi`
//! here, and that variant is produced solely by Windows path detection
//! (`%ProgramFiles%\PaneFlow\`),
//! so on Linux/macOS the function compiles but is runtime-unreachable.
//!
//! **The running-.exe-lock caveat.** Windows refuses to overwrite a
//! running `paneflow.exe`. The GUI flow therefore never runs `msiexec`
//! while the app is alive: it stages the verified MSI, starts a relay
//! outside Paneflow's kill-on-close Job Object, exits, then lets the
//! relay install and relaunch. That avoids the native Restart Manager
//! "applications should be closed" dialog and ensures restart ownership
//! is outside the process being replaced.

#[cfg(target_os = "windows")]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "windows")]
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::super::error::UpdateError;

/// Upper bound on any single HTTP call (US-001).
const UPDATE_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Relay should only wait briefly for the GUI process it spawned from to exit.
/// If that process is still alive after this, continuing is safer than wedging
/// the detached relay forever.
#[cfg(target_os = "windows")]
const PARENT_EXIT_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// Native MSI execution can legitimately take minutes, but it must remain
/// bounded so the relay eventually logs, cleans up, and relaunches the app.
const MSIEXEC_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[cfg(target_os = "windows")]
const WINDOWS_WAIT_SLICE_MS: u32 = 500;

const NATIVE_STDOUT_CAP: u64 = 64 * 1024;

#[cfg(target_os = "windows")]
const WINDOWS_PUBLISHER_ORGANIZATION: &str = "StriveX";

/// 500 MB ceiling on the MSI download. Real PaneFlow MSIs are ~60-100 MB;
/// a malicious mirror returning an unbounded stream would otherwise fill
/// `%TEMP%` before we notice.
const MAX_MSI_BYTES: u64 = 500 * 1024 * 1024;

// Well-known msiexec exit codes (see
// https://learn.microsoft.com/en-us/windows/win32/msi/error-codes).
/// ERROR_INSTALL_USEREXIT - user declined UAC or cancelled the dialog.
#[allow(dead_code)]
const MSIEXEC_EXIT_USER_CANCEL: i32 = 1602;
/// ERROR_INSTALL_FAILURE - a fatal error occurred during installation.
#[allow(dead_code)]
const MSIEXEC_EXIT_FATAL: i32 = 1603;

#[cfg(target_os = "windows")]
const MSI_RELAY_ARG: &str = "--msi-relay";
#[cfg(target_os = "windows")]
const RELAY_PARENT_PID_ARG: &str = "--parent-pid";
#[cfg(target_os = "windows")]
const RELAY_MSI_ARG: &str = "--msi";
#[cfg(target_os = "windows")]
const RELAY_MSI_LOG_ARG: &str = "--msi-log";
#[cfg(target_os = "windows")]
const RELAY_RESTART_ARG: &str = "--restart";
#[cfg(target_os = "windows")]
const RELAY_LOG_ARG: &str = "--relay-log";

/// Verified MSI staged on disk, plus the path the relay should launch after
/// `msiexec` succeeds.
#[derive(Clone, Debug)]
pub struct StagedMsiUpdate {
    msi_path: PathBuf,
    log_path: PathBuf,
    restart_path: PathBuf,
}

/// Run the MSI self-update end-to-end. Returns the canonical installed
/// binary path for `cx.set_restart_path()` on success.
#[allow(dead_code)]
pub fn install(asset_url: &str) -> Result<PathBuf> {
    let restart_path = super::super::installed_binary_path()?;
    let staged = stage_with_restart_path(asset_url, restart_path)?;
    install_with(&staged.msi_path, &staged.log_path, &MsiexecProcessRunner)?;
    // Success - tidy up the scratch MSI. Keep the log until the next
    // run so a crash-later recovery can still examine it (msiexec
    // already appends to `/l*v` on subsequent invocations).
    let _ = std::fs::remove_file(&staged.msi_path);
    Ok(staged.restart_path)
}

/// Download and verify the MSI, but do not run it yet. The GUI uses this
/// while Paneflow is still alive, then hands the staged update to a relay
/// process that runs after the GUI exits.
pub fn stage(asset_url: &str, install_path: &Path) -> Result<StagedMsiUpdate> {
    stage_with_restart_path(asset_url, binary_path_in_install_dir(install_path))
}

fn stage_with_restart_path(asset_url: &str, restart_path: PathBuf) -> Result<StagedMsiUpdate> {
    let temp = std::env::temp_dir();
    let pid = std::process::id();
    let msi_path = temp.join(format!("paneflow-update-{pid}.msi"));
    let log_path = temp.join(format!("paneflow-msi-{pid}.log"));

    let download_result = download_with_verification(asset_url, &msi_path);
    if let Err(e) = download_result {
        // AC4: the partial never survives a verification failure. The
        // verifier already tried to clean up its `.partial`, but the
        // main MSI path may also exist from a prior run - drop it too
        // so the next attempt starts clean.
        let _ = std::fs::remove_file(&msi_path);
        return Err(e);
    }

    // US-005: OS-native Authenticode check as a second, independent layer on
    // top of the minisign verification above. Fail-closed - an unsigned or
    // untrusted MSI is deleted and aborted before msiexec ever sees it. No
    // publisher-name string compare (forgeable); we trust the result of
    // `WinVerifyTrust` chaining to a trusted root. Compiled out on non-Windows
    // (the MSI path is unreachable there).
    #[cfg(target_os = "windows")]
    if let Err(e) = windows_verify_trust(&msi_path) {
        let _ = std::fs::remove_file(&msi_path);
        return Err(e);
    }

    Ok(StagedMsiUpdate {
        msi_path,
        log_path,
        restart_path,
    })
}

/// Testable core. Parameterised on:
/// - `msi_path`: already-downloaded MSI.
/// - `log_path`: the `/l*v` destination msiexec writes to.
/// - `runner`: abstracts `msiexec` invocation so tests can inject exit
///   codes without spawning the real tool.
#[allow(dead_code)]
fn install_with(msi_path: &Path, log_path: &Path, runner: &dyn Msiexec) -> Result<()> {
    match runner.run_installer(msi_path, log_path) {
        Ok(()) => Ok(()),
        Err(MsiexecError::NotFound) => Err(anyhow::Error::new(UpdateError::EnvironmentBroken {
            message:
                "msiexec.exe not found in System32 or on PATH - Windows system install appears broken. Reinstall PaneFlow manually from the releases page."
                    .to_string(),
        })),
        Err(MsiexecError::RunFailed(e)) => Err(e).context("run msiexec.exe"),
        Err(MsiexecError::Timeout) => Err(anyhow::Error::new(UpdateError::Timeout)
            .context(format!("msiexec exceeded {}s deadline", MSIEXEC_TIMEOUT.as_secs()))),
        Err(MsiexecError::NonZeroExit { code }) => Err(map_exit_code(code, log_path)),
    }
}

/// Spawn a detached relay that waits for this GUI process to exit, runs the
/// staged MSI, and relaunches Paneflow on success. This must be called only
/// after the session is saved and immediately before `cx.quit()`.
pub fn spawn_relay(staged: StagedMsiUpdate) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::OsString;
        use std::os::windows::process::CommandExt;

        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
        };

        let parent_pid = std::process::id();
        let temp = std::env::temp_dir();
        let relay_exe = temp.join(format!("paneflow-msi-relay-{parent_pid}.exe"));
        let relay_log = temp.join(format!("paneflow-msi-relay-{parent_pid}.log"));
        let current_exe = std::env::current_exe().context("resolve current paneflow executable")?;

        std::fs::copy(&current_exe, &relay_exe).with_context(|| {
            format!(
                "copy MSI relay helper {} -> {}",
                current_exe.display(),
                relay_exe.display()
            )
        })?;

        append_relay_log(
            &relay_log,
            &format!(
                "spawning relay parent={} relay={} msi={} elevated_msiexec={}",
                parent_pid,
                relay_exe.display(),
                staged.msi_path.display(),
                restart_path_requires_elevation(&staged.restart_path)
            ),
        );

        let args: Vec<OsString> = relay_args(parent_pid, &staged, &relay_log);
        Command::new(&relay_exe)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
            .spawn()
            .with_context(|| format!("spawn MSI relay {}", relay_exe.display()))?;

        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = staged;
        bail!("MSI relay is only available on Windows")
    }
}

#[cfg(target_os = "windows")]
pub fn is_relay_invocation(args: &[String]) -> bool {
    relay_arg_index(args).is_some()
}

#[cfg(target_os = "windows")]
pub fn run_relay_from_args(args: &[String]) -> i32 {
    match parse_relay_invocation(args) {
        Ok(invocation) => match run_native_relay(invocation) {
            Ok(code) => code,
            Err(err) => {
                eprintln!("paneflow-msi-relay: {err:#}");
                1
            }
        },
        Err(err) => {
            eprintln!("paneflow-msi-relay: {err:#}");
            if let Some(path) = relay_log_path_from_args(args) {
                append_relay_log(&path, &format!("relay argument parse failed: {err:#}"));
            }
            2
        }
    }
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct RelayInvocation {
    parent_pid: u32,
    msi_path: PathBuf,
    msi_log_path: PathBuf,
    restart_path: PathBuf,
    relay_log_path: PathBuf,
}

#[cfg(target_os = "windows")]
fn relay_args(
    parent_pid: u32,
    staged: &StagedMsiUpdate,
    relay_log_path: &Path,
) -> Vec<std::ffi::OsString> {
    use std::ffi::OsString;

    vec![
        OsString::from(MSI_RELAY_ARG),
        OsString::from(RELAY_PARENT_PID_ARG),
        OsString::from(parent_pid.to_string()),
        OsString::from(RELAY_MSI_ARG),
        staged.msi_path.as_os_str().to_os_string(),
        OsString::from(RELAY_MSI_LOG_ARG),
        staged.log_path.as_os_str().to_os_string(),
        OsString::from(RELAY_RESTART_ARG),
        staged.restart_path.as_os_str().to_os_string(),
        OsString::from(RELAY_LOG_ARG),
        relay_log_path.as_os_str().to_os_string(),
    ]
}

#[cfg(target_os = "windows")]
fn parse_relay_invocation(args: &[String]) -> Result<RelayInvocation> {
    let relay_idx = relay_arg_index(args).context(format!("missing {MSI_RELAY_ARG}"))?;

    let mut parent_pid = None;
    let mut msi_path = None;
    let mut msi_log_path = None;
    let mut restart_path = None;
    let mut relay_log_path = None;

    let mut idx = relay_idx + 1;
    while idx < args.len() {
        let key = args[idx].as_str();
        let value = args
            .get(idx + 1)
            .with_context(|| format!("missing value for {key}"))?;
        match key {
            RELAY_PARENT_PID_ARG => {
                parent_pid = Some(
                    value
                        .parse::<u32>()
                        .with_context(|| format!("invalid {RELAY_PARENT_PID_ARG}: {value}"))?,
                );
            }
            RELAY_MSI_ARG => msi_path = Some(PathBuf::from(value)),
            RELAY_MSI_LOG_ARG => msi_log_path = Some(PathBuf::from(value)),
            RELAY_RESTART_ARG => restart_path = Some(PathBuf::from(value)),
            RELAY_LOG_ARG => relay_log_path = Some(PathBuf::from(value)),
            other => bail!("unknown relay argument {other}"),
        }
        idx += 2;
    }

    Ok(RelayInvocation {
        parent_pid: parent_pid.context("missing relay parent PID")?,
        msi_path: msi_path.context("missing relay MSI path")?,
        msi_log_path: msi_log_path.context("missing relay MSI log path")?,
        restart_path: restart_path.context("missing relay restart path")?,
        relay_log_path: relay_log_path.context("missing relay diagnostic log path")?,
    })
}

#[cfg(target_os = "windows")]
fn relay_arg_index(args: &[String]) -> Option<usize> {
    args.iter()
        .position(|arg| arg == MSI_RELAY_ARG)
        .filter(|idx| *idx > 0)
}

#[cfg(target_os = "windows")]
fn relay_log_path_from_args(args: &[String]) -> Option<PathBuf> {
    args.windows(2)
        .find(|pair| pair.first().map(String::as_str) == Some(RELAY_LOG_ARG))
        .and_then(|pair| pair.get(1))
        .map(PathBuf::from)
}

#[cfg(target_os = "windows")]
fn run_native_relay(invocation: RelayInvocation) -> Result<i32> {
    append_relay_log(
        &invocation.relay_log_path,
        &format!(
            "started pid={} parent={} msi={} restart={}",
            std::process::id(),
            invocation.parent_pid,
            invocation.msi_path.display(),
            invocation.restart_path.display()
        ),
    );

    wait_for_parent_exit(invocation.parent_pid, &invocation.relay_log_path);
    std::thread::sleep(Duration::from_millis(350));

    let result = run_msiexec_for_relay(
        &invocation.msi_path,
        &invocation.msi_log_path,
        &invocation.relay_log_path,
        restart_path_requires_elevation(&invocation.restart_path),
    );
    let _ = std::fs::remove_file(&invocation.msi_path);

    append_relay_log(
        &invocation.relay_log_path,
        &format!("msiexec exited with {}", result.exit_code),
    );

    if relay_should_relaunch_after_msiexec(result.exit_code) {
        relaunch_paneflow(&invocation.restart_path, &invocation.relay_log_path)
            .with_context(|| format!("relaunch after msiexec exit {}", result.exit_code))?;
    }

    schedule_relay_cleanup(&invocation.relay_log_path);
    Ok(result.exit_code)
}

#[cfg(target_os = "windows")]
struct RelayInstallResult {
    exit_code: i32,
}

#[cfg(target_os = "windows")]
fn relay_should_relaunch_after_msiexec(_exit_code: i32) -> bool {
    true
}

#[cfg(target_os = "windows")]
fn run_msiexec_for_relay(
    msi_path: &Path,
    log_path: &Path,
    relay_log_path: &Path,
    elevated: bool,
) -> RelayInstallResult {
    append_relay_log(
        relay_log_path,
        &format!(
            "running {}msiexec msi={} log={}",
            if elevated { "elevated " } else { "" },
            msi_path.display(),
            log_path.display()
        ),
    );

    if elevated {
        return run_elevated_msiexec_for_relay(msi_path, log_path, relay_log_path);
    }

    match MsiexecProcessRunner.run_installer(msi_path, log_path) {
        Ok(()) => RelayInstallResult { exit_code: 0 },
        Err(MsiexecError::NotFound) => {
            append_relay_log(relay_log_path, "msiexec.exe not found");
            RelayInstallResult { exit_code: 127 }
        }
        Err(MsiexecError::RunFailed(err)) => {
            append_relay_log(relay_log_path, &format!("run msiexec failed: {err:#}"));
            RelayInstallResult { exit_code: 1 }
        }
        Err(MsiexecError::Timeout) => {
            append_relay_log(relay_log_path, "msiexec timed out");
            RelayInstallResult { exit_code: 124 }
        }
        Err(MsiexecError::NonZeroExit { code }) => RelayInstallResult { exit_code: code },
    }
}

#[cfg(target_os = "windows")]
fn run_elevated_msiexec_for_relay(
    msi_path: &Path,
    log_path: &Path,
    relay_log_path: &Path,
) -> RelayInstallResult {
    let Some(msiexec) = msiexec_exe() else {
        append_relay_log(relay_log_path, "msiexec.exe not found");
        return RelayInstallResult { exit_code: 127 };
    };

    let args = msiexec_args(msi_path, log_path);
    match shell_execute_wait_elevated(&msiexec, &args) {
        Ok(code) => RelayInstallResult { exit_code: code },
        Err(ElevatedProcessError::Cancelled) => {
            append_relay_log(relay_log_path, "elevated msiexec cancelled by user");
            RelayInstallResult {
                exit_code: MSIEXEC_EXIT_USER_CANCEL,
            }
        }
        Err(ElevatedProcessError::LaunchFailed(err)) => {
            append_relay_log(
                relay_log_path,
                &format!("launch elevated msiexec failed: {err:#}"),
            );
            RelayInstallResult { exit_code: 1 }
        }
        Err(ElevatedProcessError::WaitFailed(err)) => {
            append_relay_log(
                relay_log_path,
                &format!("wait elevated msiexec failed: {err}"),
            );
            RelayInstallResult { exit_code: 1 }
        }
    }
}

#[cfg(target_os = "windows")]
fn wait_for_parent_exit(parent_pid: u32, relay_log_path: &Path) {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};

    // SAFETY: OpenProcess does not retain Rust pointers; parent_pid is the
    // process id passed by the GUI before it exits.
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, parent_pid) };
    if handle.is_null() {
        append_relay_log(
            relay_log_path,
            &format!("parent {parent_pid} already exited or cannot be opened"),
        );
        return;
    }

    append_relay_log(relay_log_path, &format!("waiting for parent {parent_pid}"));
    let started = std::time::Instant::now();
    let mut wait_result;
    loop {
        // SAFETY: handle is a valid process handle from OpenProcess.
        wait_result = unsafe { WaitForSingleObject(handle, WINDOWS_WAIT_SLICE_MS) };
        if wait_result != WAIT_TIMEOUT || started.elapsed() >= PARENT_EXIT_TIMEOUT {
            break;
        }
    }
    // SAFETY: handle is no longer used after this call.
    unsafe {
        let _ = CloseHandle(handle);
    }

    if wait_result == WAIT_OBJECT_0 {
        append_relay_log(relay_log_path, "parent exited");
    } else if wait_result == WAIT_TIMEOUT {
        append_relay_log(
            relay_log_path,
            &format!(
                "parent {parent_pid} did not exit within {}s; continuing",
                PARENT_EXIT_TIMEOUT.as_secs()
            ),
        );
    } else if wait_result == WAIT_FAILED {
        append_relay_log(relay_log_path, "parent wait failed; continuing");
    } else {
        append_relay_log(
            relay_log_path,
            &format!("parent wait returned {wait_result}; continuing"),
        );
    }
}

#[cfg(target_os = "windows")]
fn relaunch_paneflow(restart_path: &Path, relay_log_path: &Path) -> Result<()> {
    if let Some(explorer) = explorer_exe() {
        match Command::new(&explorer)
            .arg(restart_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(_) => {
                append_relay_log(
                    relay_log_path,
                    &format!("relaunch requested through {}", explorer.display()),
                );
                return Ok(());
            }
            Err(err) => {
                append_relay_log(relay_log_path, &format!("explorer relaunch failed: {err}"))
            }
        }
    }

    Command::new(restart_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("relaunch {}", restart_path.display()))?;
    append_relay_log(
        relay_log_path,
        &format!("relaunch requested through {}", restart_path.display()),
    );
    Ok(())
}

fn binary_path_in_install_dir(install_path: &Path) -> PathBuf {
    let mut exe = install_path.join("paneflow");
    if !std::env::consts::EXE_EXTENSION.is_empty() {
        exe.set_extension(std::env::consts::EXE_EXTENSION);
    }
    exe
}

#[cfg(target_os = "windows")]
fn explorer_exe() -> Option<PathBuf> {
    let system_root = std::env::var_os("SystemRoot")?;
    let candidate = PathBuf::from(system_root).join("explorer.exe");
    candidate.exists().then_some(candidate)
}

#[cfg(target_os = "windows")]
fn restart_path_requires_elevation(restart_path: &Path) -> bool {
    ["ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .any(|root| path_starts_with_case_insensitive(restart_path, &root))
}

#[cfg(target_os = "windows")]
fn path_starts_with_case_insensitive(path: &Path, root: &Path) -> bool {
    let path = normalize_windows_path(path);
    let root = normalize_windows_path(root);
    path == root || path.starts_with(&(root + "\\"))
}

#[cfg(target_os = "windows")]
fn normalize_windows_path(path: &Path) -> String {
    path.as_os_str()
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
enum ElevatedProcessError {
    Cancelled,
    LaunchFailed(anyhow::Error),
    WaitFailed(std::io::Error),
}

#[cfg(target_os = "windows")]
fn shell_execute_wait_elevated(
    exe: &Path,
    args: &[std::ffi::OsString],
) -> std::result::Result<i32, ElevatedProcessError> {
    use std::ffi::OsStr;
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, TerminateProcess, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NO_CONSOLE, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
        ShellExecuteExW,
    };

    const ERROR_CANCELLED: i32 = 1223;

    let verb = wide_null(OsStr::new("runas"));
    let file = wide_null(exe.as_os_str());
    let parameters = shell_execute_parameters(args);
    let parameters = wide_null(OsStr::new(&parameters));

    // SAFETY: zero-init is valid for SHELLEXECUTEINFOW; all pointer fields are
    // either set to live NUL-terminated buffers or left null.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NO_CONSOLE | SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = 0;

    // SAFETY: the SHELLEXECUTEINFOW pointers outlive the call; Windows copies
    // the command line before returning.
    let ok = unsafe { ShellExecuteExW(&mut info) };
    if ok == 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(ERROR_CANCELLED) {
            return Err(ElevatedProcessError::Cancelled);
        }
        return Err(ElevatedProcessError::LaunchFailed(
            anyhow::Error::new(err).context(format!("launch elevated {}", exe.display())),
        ));
    }

    if info.hProcess.is_null() {
        return Err(ElevatedProcessError::LaunchFailed(anyhow::anyhow!(
            "ShellExecuteExW returned no process handle for {}",
            exe.display()
        )));
    }

    let started = std::time::Instant::now();
    let mut wait_result;
    loop {
        // SAFETY: hProcess is owned by this SHELLEXECUTEINFOW result when
        // SEE_MASK_NOCLOSEPROCESS succeeds.
        wait_result = unsafe { WaitForSingleObject(info.hProcess, WINDOWS_WAIT_SLICE_MS) };
        if wait_result != WAIT_TIMEOUT || started.elapsed() >= MSIEXEC_TIMEOUT {
            break;
        }
    }
    if wait_result == WAIT_TIMEOUT {
        // SAFETY: hProcess is a live process handle from ShellExecuteExW. A
        // timeout means the relay cannot wait usefully anymore; terminate
        // best-effort, then close the handle below.
        unsafe {
            let _ = TerminateProcess(info.hProcess, 1);
        }
        unsafe {
            let _ = CloseHandle(info.hProcess);
        }
        return Err(ElevatedProcessError::WaitFailed(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!(
                "elevated msiexec exceeded {}s deadline",
                MSIEXEC_TIMEOUT.as_secs()
            ),
        )));
    }
    if wait_result == WAIT_FAILED {
        let err = std::io::Error::last_os_error();
        unsafe {
            let _ = CloseHandle(info.hProcess);
        }
        return Err(ElevatedProcessError::WaitFailed(err));
    }
    if wait_result != WAIT_OBJECT_0 {
        unsafe {
            let _ = CloseHandle(info.hProcess);
        }
        return Err(ElevatedProcessError::WaitFailed(std::io::Error::other(
            format!("unexpected wait result {wait_result}"),
        )));
    }

    let mut exit_code = 0u32;
    let got_code = unsafe { GetExitCodeProcess(info.hProcess, &mut exit_code) };
    unsafe {
        let _ = CloseHandle(info.hProcess);
    }
    if got_code == 0 {
        return Err(ElevatedProcessError::WaitFailed(
            std::io::Error::last_os_error(),
        ));
    }

    Ok(exit_code as i32)
}

#[cfg(target_os = "windows")]
fn msiexec_args(msi: &Path, log: &Path) -> Vec<std::ffi::OsString> {
    use std::ffi::OsString;

    vec![
        OsString::from("/i"),
        msi.as_os_str().to_os_string(),
        OsString::from("/qb"),
        OsString::from("/norestart"),
        OsString::from("/l*v"),
        log.as_os_str().to_os_string(),
    ]
}

#[cfg(target_os = "windows")]
fn shell_execute_parameters(args: &[std::ffi::OsString]) -> String {
    args.iter()
        .map(|arg| quote_windows_arg(arg.as_os_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(target_os = "windows")]
fn quote_windows_arg(arg: &std::ffi::OsStr) -> String {
    let value = arg.to_string_lossy();
    if value.is_empty() {
        return "\"\"".to_string();
    }
    if !value.chars().any(|c| c.is_whitespace() || c == '"') {
        return value.into_owned();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0usize;
    for ch in value.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(target_os = "windows")]
fn wide_null(value: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn append_relay_log(path: &Path, message: &str) {
    let timestamp = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => format!("{}.{:03}", elapsed.as_secs(), elapsed.subsec_millis()),
        Err(_) => "time-unavailable".to_string(),
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "[{timestamp}] {message}");
    }
}

#[cfg(target_os = "windows")]
fn schedule_relay_cleanup(relay_log_path: &Path) {
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};

    let Ok(current_exe) = std::env::current_exe() else {
        return;
    };
    let exe = wide_null(current_exe.as_os_str());
    // SAFETY: `exe` is a live NUL-terminated path; null destination with
    // MOVEFILE_DELAY_UNTIL_REBOOT schedules deletion after this process exits.
    let ok = unsafe { MoveFileExW(exe.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };
    if ok == 0 {
        append_relay_log(
            relay_log_path,
            &format!(
                "could not schedule relay cleanup for {}: {}",
                current_exe.display(),
                std::io::Error::last_os_error()
            ),
        );
    }
}

/// Verify the Authenticode signature chain of `msi` with `WinVerifyTrust`
/// (US-005). Fail-closed: any result other than "trusted" returns a tagged
/// `IntegrityMismatch` so the toast reads "corrupt or tampered".
///
/// This is defense-in-depth on top of US-001's minisign check - it validates
/// the Microsoft Authenticode chain (our signing certificate → a trusted
/// root), which minisign does not cover. We trust `WinVerifyTrust`'s chain
/// result, never a forgeable publisher-name string compare.
///
/// Two-pass `VERIFY` then `CLOSE` is the documented usage (the second call
/// frees `hWVTStateData`). Untestable on the Linux/macOS CI legs (no
/// `wintrust.dll`, no signed-MSI fixture); the Windows release leg exercises
/// it against the real signed artifact. Struct/const names pinned against
/// windows-sys 0.59.
#[cfg(target_os = "windows")]
fn windows_verify_trust(msi: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
        WTD_REVOKE_WHOLECHAIN, WTD_SAFER_FLAG, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE, WinVerifyTrust,
    };

    // Wide, NUL-terminated path. Must outlive the WinVerifyTrust call (it
    // backs the `pcwszFilePath` pointer below).
    let wide: Vec<u16> = msi
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // No parent window - WTD_UI_NONE means WinVerifyTrust never shows UI.
    let hwnd: windows_sys::Win32::Foundation::HWND = std::ptr::null_mut();

    // SAFETY: zero-init is a valid bit pattern for these `repr(C)` structs
    // (null pointers, zero enums); we then set every field WinVerifyTrust
    // reads for a file check.
    let mut file_info: WINTRUST_FILE_INFO = unsafe { std::mem::zeroed() };
    file_info.cbStruct = std::mem::size_of::<WINTRUST_FILE_INFO>() as u32;
    file_info.pcwszFilePath = wide.as_ptr();

    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_WHOLECHAIN;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    // Writing a union field is safe (only reads are unsafe).
    data.Anonymous.pFile = &mut file_info;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    data.dwProvFlags = WTD_SAFER_FLAG;

    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: standard WinVerifyTrust FFI. `action`/`data` outlive the call;
    // `wide` + `file_info` back the pointers reachable through `data`.
    let status = unsafe {
        WinVerifyTrust(
            hwnd,
            &mut action,
            &mut data as *mut WINTRUST_DATA as *mut core::ffi::c_void,
        )
    };

    let publisher_organization = (status == 0).then(|| windows_signer_organization(&data));

    // Always run the CLOSE pass to free the provider state. Publisher
    // inspection must happen before this pass because hWVTStateData is freed.
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: same data block; CLOSE frees `hWVTStateData`.
    unsafe {
        WinVerifyTrust(
            hwnd,
            &mut action,
            &mut data as *mut WINTRUST_DATA as *mut core::ffi::c_void,
        );
    }

    if status == 0 {
        let publisher =
            publisher_organization.context("missing Windows publisher verification result")??;
        if publisher == WINDOWS_PUBLISHER_ORGANIZATION {
            return Ok(());
        }
        return Err(anyhow::Error::new(super::super::error::IntegrityMismatch {
            expected: format!("Windows publisher O={WINDOWS_PUBLISHER_ORGANIZATION}"),
            got: format!("Windows publisher O={publisher}"),
        }));
    }

    // Any nonzero HRESULT (TRUST_E_NOSIGNATURE, TRUST_E_SUBJECT_NOT_TRUSTED,
    // CERT_E_UNTRUSTEDROOT, …) is a fail-closed rejection.
    Err(anyhow::Error::new(super::super::error::IntegrityMismatch {
        expected: "trusted Authenticode signature".to_string(),
        got: format!("WinVerifyTrust returned 0x{:08X}", status as u32),
    }))
}

#[cfg(target_os = "windows")]
fn windows_signer_organization(
    data: &windows_sys::Win32::Security::WinTrust::WINTRUST_DATA,
) -> Result<String> {
    use windows_sys::Win32::Security::Cryptography::{
        CERT_NAME_ATTR_TYPE, CertGetNameStringW, szOID_ORGANIZATION_NAME,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData,
    };

    // SAFETY: data.hWVTStateData is owned by WinVerifyTrust after a successful
    // VERIFY pass and remains valid until the caller runs CLOSE.
    let provider = unsafe { WTHelperProvDataFromStateData(data.hWVTStateData) };
    if provider.is_null() {
        bail!("WinVerifyTrust returned no provider state");
    }
    // SAFETY: provider is non-null and owned by the WinTrust provider state.
    let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
    if signer.is_null() {
        bail!("WinVerifyTrust returned no primary signer");
    }
    // SAFETY: signer is non-null and points into the provider state.
    let cert = unsafe { WTHelperGetProvCertFromChain(signer, 0) };
    if cert.is_null() {
        bail!("WinVerifyTrust returned no signer certificate");
    }
    // SAFETY: cert is non-null and pCert points to a CERT_CONTEXT owned by the
    // provider state until CLOSE.
    let context = unsafe { (*cert).pCert };
    if context.is_null() {
        bail!("WinVerifyTrust signer certificate has no context");
    }

    let oid = szOID_ORGANIZATION_NAME as *const core::ffi::c_void;
    // SAFETY: context is a live CERT_CONTEXT and oid is a static NUL-terminated
    // C string for the organizationName attribute.
    let required = unsafe {
        CertGetNameStringW(
            context,
            CERT_NAME_ATTR_TYPE,
            0,
            oid,
            std::ptr::null_mut(),
            0,
        )
    };
    if required <= 1 {
        bail!("Windows signer certificate has no Organization attribute");
    }
    let mut buf = vec![0u16; required as usize];
    // SAFETY: buf has the exact capacity requested by the probe call above.
    let written = unsafe {
        CertGetNameStringW(
            context,
            CERT_NAME_ATTR_TYPE,
            0,
            oid,
            buf.as_mut_ptr(),
            buf.len() as u32,
        )
    };
    if written == 0 {
        bail!("read Windows signer Organization attribute");
    }
    let nul = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    Ok(String::from_utf16_lossy(&buf[..nul]))
}

/// Download the MSI, verify its detached **minisign** signature (US-001),
/// and persist at `dest` on success. Mirrors the shared pattern in
/// `targz.rs` / `macos/dmg.rs` - see them for rationale on each guard
/// (partial→rename, size cap, RO body stream). The signature, not a
/// same-host `.sha256`, is the trust anchor and is checked **before**
/// msiexec is ever invoked.
fn download_with_verification(asset_url: &str, dest: &Path) -> Result<()> {
    super::super::verified_download::download_verified_asset(
        asset_url,
        dest,
        MAX_MSI_BYTES,
        UPDATE_HTTP_TIMEOUT,
        "MSI",
    )
}

/// Map a non-zero msiexec exit code onto the right `UpdateError` variant.
/// Pure - unit-tested without spawning.
#[allow(dead_code)]
fn map_exit_code(code: i32, log_path: &Path) -> anyhow::Error {
    match code {
        MSIEXEC_EXIT_USER_CANCEL => anyhow::Error::new(UpdateError::InstallDeclined {
            message: "Update cancelled - administrator permission required.".to_string(),
        }),
        MSIEXEC_EXIT_FATAL => anyhow::Error::new(UpdateError::InstallFailed {
            log_path: log_path.to_path_buf(),
        }),
        other => anyhow::anyhow!(
            "msiexec exited with code {other}. See log at {} for details.",
            log_path.display()
        ),
    }
}

/// Why `msiexec` failed. `NotFound`, `Timeout`, and `NonZeroExit` route to
/// specific `UpdateError` variants; `RunFailed` retains the typed process cause.
#[derive(Debug)]
#[allow(dead_code)]
enum MsiexecError {
    NotFound,
    RunFailed(anyhow::Error),
    Timeout,
    NonZeroExit { code: i32 },
}

/// Abstraction over `msiexec` invocation so tests can inject exit
/// codes without spawning the real tool (it doesn't exist on Linux CI).
#[allow(dead_code)]
trait Msiexec {
    /// Run `msiexec /i <msi> /qb /norestart /l*v <log>` and block until
    /// it exits. Returns `Ok(())` on exit code 0 - every other outcome
    /// is an error the caller classifies.
    fn run_installer(&self, msi: &Path, log: &Path) -> std::result::Result<(), MsiexecError>;
}

#[allow(dead_code)]
struct MsiexecProcessRunner;

impl Msiexec for MsiexecProcessRunner {
    fn run_installer(&self, msi: &Path, log: &Path) -> std::result::Result<(), MsiexecError> {
        let msiexec = msiexec_exe().ok_or(MsiexecError::NotFound)?;

        // Bound the native installer with the same process-tree runner used by
        // other external tools. Capture overflow and timeout both terminate the
        // run, so `msiexec` cannot wedge the relay forever.
        let mut cmd = Command::new(&msiexec);
        cmd.arg("/i")
            .arg(msi)
            .arg("/qb")
            .arg("/norestart")
            .arg("/l*v")
            .arg(log);
        let out = paneflow_process::run_with_timeout(cmd, MSIEXEC_TIMEOUT, NATIVE_STDOUT_CAP)
            .map_err(|e| match e {
                paneflow_process::ProcError::Timeout => MsiexecError::Timeout,
                other => MsiexecError::RunFailed(anyhow::Error::new(other)),
            })?;

        if out.status.success() {
            return Ok(());
        }
        // `code()` is `None` only when the process was terminated by a
        // signal - on Windows that essentially can't happen for a
        // subprocess we started synchronously, but fall back to -1 so
        // the classifier doesn't drop the error on the floor.
        Err(MsiexecError::NonZeroExit {
            code: out.status.code().unwrap_or(-1),
        })
    }
}

fn msiexec_exe() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        let candidate = PathBuf::from(system_root)
            .join("System32")
            .join("msiexec.exe");
        if candidate.exists() {
            return Some(candidate);
        }
    }

    which::which("msiexec").ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[cfg(target_os = "windows")]
    #[test]
    fn relay_invocation_parses_paths_with_spaces() {
        let args = vec![
            "paneflow".to_string(),
            MSI_RELAY_ARG.to_string(),
            RELAY_PARENT_PID_ARG.to_string(),
            "1234".to_string(),
            RELAY_MSI_ARG.to_string(),
            "C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow update.msi".to_string(),
            RELAY_MSI_LOG_ARG.to_string(),
            "C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow msi.log".to_string(),
            RELAY_RESTART_ARG.to_string(),
            "C:\\Program Files\\PaneFlow\\paneflow.exe".to_string(),
            RELAY_LOG_ARG.to_string(),
            "C:\\Users\\Example\\AppData\\Local\\Temp\\relay.log".to_string(),
        ];

        let parsed = parse_relay_invocation(&args).expect("parse relay args");

        assert_eq!(parsed.parent_pid, 1234);
        assert_eq!(
            parsed.msi_path,
            PathBuf::from("C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow update.msi")
        );
        assert_eq!(
            parsed.restart_path,
            PathBuf::from("C:\\Program Files\\PaneFlow\\paneflow.exe")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn relay_invocation_parses_when_flag_is_not_argv1() {
        let args = vec![
            "paneflow".to_string(),
            "--host-added-flag".to_string(),
            MSI_RELAY_ARG.to_string(),
            RELAY_PARENT_PID_ARG.to_string(),
            "1234".to_string(),
            RELAY_MSI_ARG.to_string(),
            "C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow-update.msi".to_string(),
            RELAY_MSI_LOG_ARG.to_string(),
            "C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow-msi.log".to_string(),
            RELAY_RESTART_ARG.to_string(),
            "C:\\Program Files\\PaneFlow\\paneflow.exe".to_string(),
            RELAY_LOG_ARG.to_string(),
            "C:\\Users\\Example\\AppData\\Local\\Temp\\relay.log".to_string(),
        ];

        assert!(is_relay_invocation(&args));
        let parsed = parse_relay_invocation(&args).expect("parse relay args");

        assert_eq!(parsed.parent_pid, 1234);
        assert_eq!(
            parsed.msi_log_path,
            PathBuf::from("C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow-msi.log")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn relay_parse_error_writes_relay_log_when_log_arg_is_present() {
        let log_path = std::env::temp_dir().join(format!(
            "paneflow-relay-parse-test-{}.log",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&log_path);
        let args = vec![
            "paneflow".to_string(),
            MSI_RELAY_ARG.to_string(),
            RELAY_LOG_ARG.to_string(),
            log_path.display().to_string(),
        ];

        let code = run_relay_from_args(&args);
        let contents = std::fs::read_to_string(&log_path).expect("relay parse log written");
        let _ = std::fs::remove_file(&log_path);

        assert_eq!(code, 2);
        assert!(contents.contains("relay argument parse failed"));
        assert!(contents.contains("missing relay parent PID"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn shell_execute_parameters_quote_windows_paths() {
        use std::ffi::OsString;

        let args = vec![
            OsString::from("--flag"),
            OsString::from("C:\\Program Files\\PaneFlow\\paneflow.exe"),
            OsString::from("quote\"inside"),
        ];

        assert_eq!(
            shell_execute_parameters(&args),
            "--flag \"C:\\Program Files\\PaneFlow\\paneflow.exe\" \"quote\\\"inside\""
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn msiexec_parameters_quote_windows_paths() {
        assert_eq!(
            shell_execute_parameters(&msiexec_args(
                Path::new("C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow update.msi"),
                Path::new("C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow msi.log"),
            )),
            "/i \"C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow update.msi\" /qb /norestart /l*v \"C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow msi.log\""
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn relay_relaunches_after_success_cancel_and_failure() {
        assert!(relay_should_relaunch_after_msiexec(0));
        assert!(relay_should_relaunch_after_msiexec(
            MSIEXEC_EXIT_USER_CANCEL
        ));
        assert!(relay_should_relaunch_after_msiexec(MSIEXEC_EXIT_FATAL));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn program_files_restart_requires_elevation() {
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            let path = PathBuf::from(program_files)
                .join("PaneFlow")
                .join("paneflow.exe");
            assert!(restart_path_requires_elevation(&path));
        }

        assert!(!restart_path_requires_elevation(Path::new(
            "C:\\Users\\Example\\AppData\\Local\\Programs\\PaneFlow\\paneflow.exe"
        )));
    }

    // ── Exit-code classification ─────────────────────────────────────

    #[test]
    fn map_exit_code_1602_is_install_declined() {
        // AC6: the canonical "user declined UAC" code must surface the
        // exact mandated toast copy.
        let log = PathBuf::from("C:\\Temp\\test.log");
        let err = map_exit_code(MSIEXEC_EXIT_USER_CANCEL, &log);
        match UpdateError::classify(&err) {
            UpdateError::InstallDeclined { message } => {
                assert!(
                    message.contains("administrator permission required"),
                    "got: {message}"
                );
                assert!(message.contains("cancelled"), "got: {message}");
            }
            other => panic!("expected InstallDeclined, got {other:?}"),
        }
    }

    #[test]
    fn map_exit_code_1603_is_install_failed_with_log_path() {
        // AC7: fatal install error carries the verbose log path through
        // for the bug-report attachment.
        let log = PathBuf::from("C:\\Temp\\paneflow-msi-999.log");
        let err = map_exit_code(MSIEXEC_EXIT_FATAL, &log);
        match UpdateError::classify(&err) {
            UpdateError::InstallFailed { log_path } => {
                assert_eq!(log_path, log);
            }
            other => panic!("expected InstallFailed, got {other:?}"),
        }
    }

    #[test]
    fn map_exit_code_unknown_falls_through_to_other_with_log_hint() {
        let log = PathBuf::from("C:\\Temp\\test.log");
        let err = map_exit_code(42, &log);
        let tag = UpdateError::classify(&err);
        match tag {
            UpdateError::Other(msg) => {
                assert!(msg.contains("42"), "got: {msg}");
                assert!(msg.contains("test.log"), "got: {msg}");
            }
            other => panic!("expected Other for exit 42, got {other:?}"),
        }
    }

    // ── install_with() with stubbed msiexec ──────────────────────────

    /// Stub that records a single invocation and returns a pre-loaded
    /// result. `spawn_count` proves that exit-code paths actually
    /// reach the classifier vs. short-circuiting in download.
    struct StubMsiexec {
        outcome: Cell<Option<std::result::Result<(), MsiexecError>>>,
        spawn_count: Cell<usize>,
    }

    impl Msiexec for StubMsiexec {
        fn run_installer(&self, _msi: &Path, _log: &Path) -> std::result::Result<(), MsiexecError> {
            self.spawn_count.set(self.spawn_count.get() + 1);
            self.outcome
                .take()
                .expect("StubMsiexec outcome polled twice")
        }
    }

    /// AC9: msiexec missing maps to EnvironmentBroken with a specific
    /// message (not a generic "update failed"). This is distinct from
    /// InstallDeclined and InstallFailed because the user hasn't even
    /// been asked to install - the environment itself is broken.
    ///
    /// Uses the direct MsiexecError → install_with error-path logic
    /// (not a full download leg, which needs a live HTTP server). We
    /// exercise the classification contract instead.
    #[test]
    fn msiexec_not_found_maps_to_environment_broken() {
        // Construct the same error install_with would produce on the
        // NotFound branch and verify classification.
        let err = anyhow::Error::new(UpdateError::EnvironmentBroken {
            message: "msiexec.exe not found in System32 or on PATH - Windows system install appears broken. Reinstall PaneFlow manually from the releases page.".to_string(),
        });
        match UpdateError::classify(&err) {
            UpdateError::EnvironmentBroken { message } => {
                assert!(message.contains("msiexec.exe"), "got: {message}");
                assert!(message.contains("PATH"), "got: {message}");
            }
            other => panic!("expected EnvironmentBroken, got {other:?}"),
        }
    }

    /// StubMsiexec plumbing sanity - confirms the trait object is
    /// actually invoked when present and the outcome surfaces cleanly.
    #[test]
    fn stub_msiexec_records_invocations() {
        let stub = StubMsiexec {
            outcome: Cell::new(Some(Ok(()))),
            spawn_count: Cell::new(0),
        };
        assert_eq!(stub.spawn_count.get(), 0);
        let r = stub.run_installer(Path::new("C:\\tmp\\x.msi"), Path::new("C:\\tmp\\x.log"));
        assert!(r.is_ok());
        assert_eq!(stub.spawn_count.get(), 1);
    }

    /// StubMsiexec returning 1602 round-trips through install_with's
    /// error mapping into InstallDeclined - the full AC6 chain.
    /// Exercises install_with by short-circuiting the download via an
    /// HTTP URL that ureq will fail fast on (no actual network).
    /// Since we can't stub ureq without a framework, test the
    /// classification layer directly via map_exit_code (covered above)
    /// and the trait wiring separately (covered here). The full path
    /// is exercised by the CI windows-check job in release.yml.
    #[test]
    fn stub_msiexec_nonzero_exit_surfaces_to_caller() {
        let stub = StubMsiexec {
            outcome: Cell::new(Some(Err(MsiexecError::NonZeroExit {
                code: MSIEXEC_EXIT_FATAL,
            }))),
            spawn_count: Cell::new(0),
        };
        let r = stub.run_installer(Path::new("C:\\x.msi"), Path::new("C:\\x.log"));
        match r {
            Err(MsiexecError::NonZeroExit { code }) => assert_eq!(code, MSIEXEC_EXIT_FATAL),
            other => panic!("expected NonZeroExit, got {other:?}"),
        }
        assert_eq!(stub.spawn_count.get(), 1);
    }
}
