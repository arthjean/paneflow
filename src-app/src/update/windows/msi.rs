#[cfg(target_os = "windows")]
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::process::Command;
#[cfg(target_os = "windows")]
use std::process::Stdio;
use std::time::Duration;

#[cfg(target_os = "windows")]
use anyhow::Context;
use anyhow::{Result, bail};

const UPDATE_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(target_os = "windows")]
const PARENT_EXIT_TIMEOUT: Duration = Duration::from_secs(2 * 60);

#[cfg(target_os = "windows")]
const MSIEXEC_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[cfg(target_os = "windows")]
const WINDOWS_WAIT_SLICE_MS: u32 = 500;

#[cfg(target_os = "windows")]
const NATIVE_STDOUT_CAP: u64 = 64 * 1024;

#[cfg(target_os = "windows")]
const WINDOWS_PUBLISHER_ORGANIZATION: &str = "StriveX";

const MAX_MSI_BYTES: u64 = 500 * 1024 * 1024;

#[cfg(target_os = "windows")]
const MSIEXEC_EXIT_USER_CANCEL: i32 = 1602;

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

#[derive(Clone, Debug)]
pub struct StagedMsiUpdate {
    msi_path: PathBuf,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    log_path: PathBuf,
    restart_path: PathBuf,
}

impl StagedMsiUpdate {
    pub fn is_staged(&self) -> bool {
        std::fs::File::open(&self.msi_path)
            .and_then(|file| file.metadata())
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
            && self
                .restart_path
                .parent()
                .is_some_and(std::path::Path::is_dir)
    }
}

#[cfg(target_os = "windows")]
fn installed_host_is_replaceable(restart_path: &Path) -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let directory = restart_path
        .parent()
        .context("installed application has no directory")?;
    let host = directory.join(paneflow_host::bootstrap::HOST_EXECUTABLE_FILE_NAME);
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ)
        .open(&host)
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("{} is in use or cannot be replaced; keep the current version and retry after its sessions stop", host.display())),
    }
}

#[cfg(target_os = "windows")]
const HOST_RELEASE_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(target_os = "windows")]
const RELAY_EXIT_HOST_STILL_SERVING: i32 = 3;

#[cfg(target_os = "windows")]
fn host_still_serving(relay_log_path: &Path) -> Option<String> {
    let home = paneflow_home::paneflow_home()?;
    let endpoint = paneflow_host::endpoint::host_endpoint_path(&home);
    let hello = paneflow_host::ClientHello::control("paneflow-msi-relay");
    let started = std::time::Instant::now();
    loop {
        match paneflow_host::probe(&home, &endpoint, &hello) {
            paneflow_host::Probe::Unreachable(_) => return None,
            paneflow_host::Probe::Running(identity) => {
                if started.elapsed() >= HOST_RELEASE_TIMEOUT {
                    return Some(format!(
                        "the session host pid={} still serves {}",
                        identity.pid,
                        endpoint.display()
                    ));
                }
            }
            paneflow_host::Probe::Incompatible(message)
            | paneflow_host::Probe::Faulted(message) => {
                if started.elapsed() >= HOST_RELEASE_TIMEOUT {
                    return Some(format!(
                        "something still answers on {}: {message}",
                        endpoint.display()
                    ));
                }
            }
        }
        append_relay_log(
            relay_log_path,
            &format!(
                "waiting for the session host to release {}",
                endpoint.display()
            ),
        );
        std::thread::sleep(Duration::from_millis(u64::from(WINDOWS_WAIT_SLICE_MS)));
    }
}

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
        let _ = std::fs::remove_file(&msi_path);
        return Err(e);
    }

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

    if let Some(reason) = host_still_serving(&invocation.relay_log_path).or_else(|| {
        installed_host_is_replaceable(&invocation.restart_path)
            .err()
            .map(|error| format!("{error:#}"))
    }) {
        append_relay_log(
            &invocation.relay_log_path,
            &format!(
                "update deferred: {reason}; msiexec is not run because the host binary is in use; relaunching the current version"
            ),
        );
        relaunch_paneflow(&invocation.restart_path, &invocation.relay_log_path)
            .context("relaunch after deferring the update")?;
        schedule_relay_cleanup(&invocation.relay_log_path);
        return Ok(RELAY_EXIT_HOST_STILL_SERVING);
    }

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

    relaunch_paneflow(&invocation.restart_path, &invocation.relay_log_path)
        .with_context(|| format!("relaunch after msiexec exit {}", result.exit_code))?;

    schedule_relay_cleanup(&invocation.relay_log_path);
    Ok(result.exit_code)
}

#[cfg(target_os = "windows")]
struct RelayInstallResult {
    exit_code: i32,
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

    match run_msiexec(msi_path, log_path) {
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
        wait_result = unsafe { WaitForSingleObject(handle, WINDOWS_WAIT_SLICE_MS) };
        if wait_result != WAIT_TIMEOUT || started.elapsed() >= PARENT_EXIT_TIMEOUT {
            break;
        }
    }
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

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NO_CONSOLE | SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = 0;

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
        wait_result = unsafe { WaitForSingleObject(info.hProcess, WINDOWS_WAIT_SLICE_MS) };
        if wait_result != WAIT_TIMEOUT || started.elapsed() >= MSIEXEC_TIMEOUT {
            break;
        }
    }
    if wait_result == WAIT_TIMEOUT {
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
        OsString::from("MSIRESTARTMANAGERCONTROL=Disable"),
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

#[cfg(target_os = "windows")]
fn windows_verify_trust(msi: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
        WTD_REVOKE_WHOLECHAIN, WTD_SAFER_FLAG, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE, WinVerifyTrust,
    };

    let wide: Vec<u16> = msi
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let hwnd: windows_sys::Win32::Foundation::HWND = std::ptr::null_mut();

    let mut file_info: WINTRUST_FILE_INFO = unsafe { std::mem::zeroed() };
    file_info.cbStruct = std::mem::size_of::<WINTRUST_FILE_INFO>() as u32;
    file_info.pcwszFilePath = wide.as_ptr();

    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_WHOLECHAIN;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    data.Anonymous.pFile = &mut file_info;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    data.dwProvFlags = WTD_SAFER_FLAG;

    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let status = unsafe {
        WinVerifyTrust(
            hwnd,
            &mut action,
            &mut data as *mut WINTRUST_DATA as *mut core::ffi::c_void,
        )
    };

    let publisher_organization = (status == 0).then(|| windows_signer_organization(&data));

    data.dwStateAction = WTD_STATEACTION_CLOSE;
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

    let provider = unsafe { WTHelperProvDataFromStateData(data.hWVTStateData) };
    if provider.is_null() {
        bail!("WinVerifyTrust returned no provider state");
    }
    let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
    if signer.is_null() {
        bail!("WinVerifyTrust returned no primary signer");
    }
    let cert = unsafe { WTHelperGetProvCertFromChain(signer, 0) };
    if cert.is_null() {
        bail!("WinVerifyTrust returned no signer certificate");
    }
    let context = unsafe { (*cert).pCert };
    if context.is_null() {
        bail!("WinVerifyTrust signer certificate has no context");
    }

    let oid = szOID_ORGANIZATION_NAME as *const core::ffi::c_void;
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

fn download_with_verification(asset_url: &str, dest: &Path) -> Result<()> {
    super::super::verified_download::download_verified_asset(
        asset_url,
        dest,
        MAX_MSI_BYTES,
        UPDATE_HTTP_TIMEOUT,
        "MSI",
    )
}

#[cfg(target_os = "windows")]
enum MsiexecError {
    NotFound,
    RunFailed(anyhow::Error),
    Timeout,
    NonZeroExit { code: i32 },
}

#[cfg(target_os = "windows")]
fn run_msiexec(msi: &Path, log: &Path) -> std::result::Result<(), MsiexecError> {
    let msiexec = msiexec_exe().ok_or(MsiexecError::NotFound)?;

    let mut cmd = Command::new(&msiexec);
    cmd.arg("/i")
        .arg(msi)
        .arg("/qb")
        .arg("/norestart")
        .arg("MSIRESTARTMANAGERCONTROL=Disable")
        .arg("/l*v")
        .arg(log);
    let out = paneflow_process::run_with_timeout(cmd, MSIEXEC_TIMEOUT, NATIVE_STDOUT_CAP).map_err(
        |e| match e {
            paneflow_process::ProcError::Timeout => MsiexecError::Timeout,
            other => MsiexecError::RunFailed(anyhow::Error::new(other)),
        },
    )?;

    if out.status.success() {
        return Ok(());
    }
    Err(MsiexecError::NonZeroExit {
        code: out.status.code().unwrap_or(-1),
    })
}

#[cfg(target_os = "windows")]
fn msiexec_exe() -> Option<PathBuf> {
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

#[cfg(all(test, target_os = "windows"))]
mod tests {
    #[test]
    fn installation_preflight_detects_an_in_use_host_without_an_endpoint() {
        use std::os::windows::process::CommandExt;
        let directory = tempfile::tempdir().unwrap();
        let restart = directory.path().join("paneflow.exe");
        let host = directory
            .path()
            .join(paneflow_host::bootstrap::HOST_EXECUTABLE_FILE_NAME);
        let shell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("cmd.exe");
        std::fs::copy(shell, &host).unwrap();
        let bytes = std::fs::read(&host).unwrap();
        let mut retained = Command::new(&host)
            .args(["/D", "/Q", "/C", "pause"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        let busy = installed_host_is_replaceable(&restart).is_err();
        let still_live = retained.try_wait().unwrap().is_none();
        retained.kill().unwrap();
        retained.wait().unwrap();
        assert!(busy);
        assert!(still_live);
        assert_eq!(std::fs::read(&host).unwrap(), bytes);
        assert!(installed_host_is_replaceable(&restart).is_ok());
    }

    #[test]
    fn installation_preflight_accepts_a_missing_host_binary() {
        let directory = tempfile::tempdir().unwrap();
        let restart = directory.path().join("paneflow.exe");
        assert!(installed_host_is_replaceable(&restart).is_ok());
    }
    use super::*;

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

    #[test]
    fn msiexec_parameters_quote_windows_paths() {
        assert_eq!(
            shell_execute_parameters(&msiexec_args(
                Path::new("C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow update.msi"),
                Path::new("C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow msi.log"),
            )),
            "/i \"C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow update.msi\" /qb /norestart MSIRESTARTMANAGERCONTROL=Disable /l*v \"C:\\Users\\Example\\AppData\\Local\\Temp\\paneflow msi.log\""
        );
    }

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
}
