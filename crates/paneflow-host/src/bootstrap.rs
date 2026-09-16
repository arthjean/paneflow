use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::client::{HostClient, HostClientError};
use crate::endpoint::host_endpoint_path;
use crate::protocol::{ClientHello, HostIdentity};

pub const HOST_EXECUTABLE_FILE_NAME: &str = if cfg!(windows) {
    "paneflow-host.exe"
} else {
    "paneflow-host"
};
pub const OWNER_LOCK_FILE_NAME: &str = "owner.lock";
pub const BOOTSTRAP_LOCK_FILE_NAME: &str = "bootstrap.lock";
pub const HOST_LOG_FILE_NAME: &str = "host.log";
pub const STARTUP_WAIT: Duration = Duration::from_secs(10);
const BOOTSTRAP_LOCK_WAIT: Duration = Duration::from_secs(15);
const LOCK_RETRY: Duration = Duration::from_millis(25);
const STARTUP_POLL: Duration = Duration::from_millis(50);
const MAX_INSTANCE_RECORD_BYTES: u64 = 64 * 1024;

#[cfg(windows)]
const ERROR_ACCESS_DENIED: i32 = 5;

#[derive(Debug, thiserror::Error)]
pub enum OwnerLockError {
    #[error("another paneflow-host already owns this state home (lock {0})")]
    Held(PathBuf),
    #[error("cannot take the host owner lock: {0}")]
    Io(#[from] io::Error),
}

pub struct OwnerLock {
    _file: File,
    path: PathBuf,
}

impl OwnerLock {
    pub fn acquire(home: &Path) -> Result<Self, OwnerLockError> {
        let path = paneflow_home::host_dir_in(home).join(OWNER_LOCK_FILE_NAME);
        let file = open_lock_file(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file, path }),
            Err(TryLockError::WouldBlock) => Err(OwnerLockError::Held(path)),
            Err(TryLockError::Error(error)) => Err(OwnerLockError::Io(error)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

struct BootstrapLock {
    _file: File,
}

impl BootstrapLock {
    fn acquire(home: &Path) -> Result<Self, BootstrapError> {
        let path = paneflow_home::host_dir_in(home).join(BOOTSTRAP_LOCK_FILE_NAME);
        let file = open_lock_file(&path).map_err(BootstrapError::Lock)?;
        let deadline = Instant::now() + BOOTSTRAP_LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(LOCK_RETRY);
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(BootstrapError::BootstrapBusy(path));
                }
                Err(TryLockError::Error(error)) => return Err(BootstrapError::Lock(error)),
            }
        }
    }
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[derive(Debug)]
pub enum Probe {
    Running(Box<HostIdentity>),
    Unreachable(io::Error),
    Incompatible(String),
    Faulted(String),
}

pub fn probe(home: &Path, endpoint: &Path, hello: &ClientHello) -> Probe {
    match HostClient::connect(endpoint, hello) {
        Ok(client) => {
            let identity = client.identity().clone();
            if paneflow_home::home_fingerprint(Path::new(&identity.home))
                != paneflow_home::home_fingerprint(home)
            {
                return Probe::Faulted(format!(
                    "the host on {} serves {} instead of {}",
                    endpoint.display(),
                    identity.home,
                    home.display()
                ));
            }
            Probe::Running(Box::new(identity))
        }
        Err(HostClientError::Unreachable { source, .. }) => Probe::Unreachable(source),
        Err(HostClientError::Incompatible(message)) => Probe::Incompatible(message),
        Err(other) => Probe::Faulted(other.to_string()),
    }
}

pub fn read_instance_record(home: &Path) -> Option<HostIdentity> {
    use std::io::Read;
    let path = paneflow_home::host_instance_record_path_in(home);
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_INSTANCE_RECORD_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_INSTANCE_RECORD_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("the host bootstrap lock {0} stayed busy; another controller is still starting a host")]
    BootstrapBusy(PathBuf),
    #[error("cannot take the host bootstrap lock: {0}")]
    Lock(io::Error),
    #[error("the controller executable {0} has no parent directory")]
    NoControllerDirectory(PathBuf),
    #[error("no host executable at {0}; build it with `cargo build -p paneflow-host`")]
    HostExecutableMissing(PathBuf),
    #[error(
        "the current Job Object denies breakaway, so a detached host cannot be started from this process ({0})"
    )]
    BreakawayDenied(io::Error),
    #[error("cannot start {executable}: {source}")]
    SpawnFailed {
        executable: PathBuf,
        source: io::Error,
    },
    #[error("the host at {0} is incompatible with this controller: {1}")]
    Incompatible(PathBuf, String),
    #[error("the endpoint {0} answered with something that is not a compatible host: {1}")]
    EndpointFaulted(PathBuf, String),
    #[error("the host exited with {status} before serving {endpoint}; see {log}")]
    ExitedEarly {
        status: ExitStatus,
        endpoint: PathBuf,
        log: PathBuf,
    },
    #[error("the host did not serve {endpoint} within {waited:?}; see {log}")]
    StartupTimeout {
        endpoint: PathBuf,
        waited: Duration,
        log: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAdoption {
    pub identity: HostIdentity,
    pub started: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
}

pub fn resolve_host_executable(controller_exe: &Path) -> Result<PathBuf, BootstrapError> {
    let dir = controller_exe
        .parent()
        .ok_or_else(|| BootstrapError::NoControllerDirectory(controller_exe.to_path_buf()))?;
    let candidate = dir.join(HOST_EXECUTABLE_FILE_NAME);
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(BootstrapError::HostExecutableMissing(candidate))
    }
}

pub fn ensure_host_running(
    home: &Path,
    controller_exe: &Path,
    client_name: &str,
) -> Result<HostAdoption, BootstrapError> {
    let endpoint = host_endpoint_path(home);
    let hello = ClientHello::local(client_name);
    let _lock = BootstrapLock::acquire(home)?;
    match probe(home, &endpoint, &hello) {
        Probe::Running(identity) => {
            return Ok(HostAdoption {
                identity: *identity,
                started: false,
                executable: None,
            });
        }
        Probe::Incompatible(message) => {
            return Err(BootstrapError::Incompatible(endpoint, message));
        }
        Probe::Faulted(message) => return Err(BootstrapError::EndpointFaulted(endpoint, message)),
        Probe::Unreachable(_) => {}
    }
    let executable = resolve_host_executable(controller_exe)?;
    let log = paneflow_home::host_dir_in(home).join(HOST_LOG_FILE_NAME);
    let mut child = spawn_detached_host(&executable, home, &log)?;
    let started = Instant::now();
    loop {
        match probe(home, &endpoint, &hello) {
            Probe::Running(identity) => {
                release_child(child);
                return Ok(HostAdoption {
                    identity: *identity,
                    started: true,
                    executable: Some(executable.display().to_string()),
                });
            }
            Probe::Incompatible(message) => {
                release_child(child);
                return Err(BootstrapError::Incompatible(endpoint, message));
            }
            Probe::Faulted(message) => {
                release_child(child);
                return Err(BootstrapError::EndpointFaulted(endpoint, message));
            }
            Probe::Unreachable(_) => {}
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(BootstrapError::ExitedEarly {
                status,
                endpoint,
                log,
            });
        }
        if started.elapsed() >= STARTUP_WAIT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(BootstrapError::StartupTimeout {
                endpoint,
                waited: STARTUP_WAIT,
                log,
            });
        }
        std::thread::sleep(STARTUP_POLL);
    }
}

#[cfg(unix)]
fn release_child(mut child: HostChild) {
    let _ = std::thread::Builder::new()
        .name("paneflow-host-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        });
}

#[cfg(windows)]
fn release_child(child: HostChild) {
    drop(child);
}

fn host_log_file(log: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(log)
}

#[cfg(unix)]
struct HostChild(std::process::Child);

#[cfg(unix)]
impl HostChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.0.try_wait()
    }

    fn kill(&mut self) -> io::Result<()> {
        self.0.kill()
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        self.0.wait()
    }
}

#[cfg(unix)]
fn spawn_detached_host(
    executable: &Path,
    home: &Path,
    log: &Path,
) -> Result<HostChild, BootstrapError> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let stderr = host_log_file(log)
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null());
    let mut command = Command::new(executable);
    command
        .arg("--home")
        .arg(home)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .spawn()
        .map(HostChild)
        .map_err(|source| classify_spawn_error(executable, source))
}

#[cfg(windows)]
struct HostChild {
    process: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
unsafe impl Send for HostChild {}

#[cfg(windows)]
impl HostChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        match unsafe { WaitForSingleObject(self.process, 0) } {
            WAIT_OBJECT_0 => self.exit_status().map(Some),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn kill(&mut self) -> io::Result<()> {
        use windows_sys::Win32::System::Threading::TerminateProcess;
        if unsafe { TerminateProcess(self.process, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};
        if unsafe { WaitForSingleObject(self.process, INFINITE) } != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        self.exit_status()
    }

    fn exit_status(&self) -> io::Result<ExitStatus> {
        use std::os::windows::process::ExitStatusExt;
        use windows_sys::Win32::System::Threading::GetExitCodeProcess;
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(self.process, &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(ExitStatus::from_raw(code))
    }
}

#[cfg(windows)]
impl Drop for HostChild {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        unsafe {
            CloseHandle(self.process);
        }
    }
}

#[cfg(windows)]
struct ProcThreadAttributeList(Vec<u8>);

#[cfg(windows)]
impl ProcThreadAttributeList {
    fn with_inherited_handles(
        handles: &[windows_sys::Win32::Foundation::HANDLE],
    ) -> io::Result<Self> {
        use windows_sys::Win32::System::Threading::{
            InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            UpdateProcThreadAttribute,
        };
        let mut size = 0usize;
        unsafe {
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
        }
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0u8; size];
        let list = buffer.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let updated = unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast_mut().cast(),
                std::mem::size_of_val(handles),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if updated == 0 {
            let error = io::Error::last_os_error();
            unsafe {
                windows_sys::Win32::System::Threading::DeleteProcThreadAttributeList(list);
            }
            return Err(error);
        }
        Ok(Self(buffer))
    }

    fn as_ptr(&mut self) -> windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr().cast()
    }
}

#[cfg(windows)]
impl Drop for ProcThreadAttributeList {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::DeleteProcThreadAttributeList(self.as_ptr());
        }
    }
}

#[cfg(windows)]
fn inheritable(file: &File) -> io::Result<windows_sys::Win32::Foundation::HANDLE> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    let handle = file.as_raw_handle().cast();
    if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

#[cfg(windows)]
fn quote_windows_argument(argument: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let mut quoted = vec![u16::from(b'"')];
    let mut backslashes = 0usize;
    for unit in argument.encode_wide() {
        if unit == u16::from(b'\\') {
            backslashes += 1;
            continue;
        }
        if unit == u16::from(b'"') {
            quoted.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2 + 1));
        } else {
            quoted.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
        }
        backslashes = 0;
        quoted.push(unit);
    }
    quoted.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2));
    quoted.push(u16::from(b'"'));
    quoted
}

#[cfg(windows)]
fn spawn_detached_host(
    executable: &Path,
    home: &Path,
    log: &Path,
) -> Result<HostChild, BootstrapError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS,
        EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    };
    let spawn_failed = |source: io::Error| BootstrapError::SpawnFailed {
        executable: executable.to_path_buf(),
        source,
    };
    let stdin = File::open("NUL").map_err(spawn_failed)?;
    let stdout = File::create("NUL").map_err(spawn_failed)?;
    let stderr = host_log_file(log)
        .or_else(|_| File::create("NUL"))
        .map_err(spawn_failed)?;
    let handles = [
        inheritable(&stdin).map_err(spawn_failed)?,
        inheritable(&stdout).map_err(spawn_failed)?,
        inheritable(&stderr).map_err(spawn_failed)?,
    ];
    let mut attributes =
        ProcThreadAttributeList::with_inherited_handles(&handles).map_err(spawn_failed)?;
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.as_ptr();
    let mut program: Vec<u16> = executable.as_os_str().encode_wide().collect();
    program.push(0);
    let mut command_line = quote_windows_argument(executable.as_os_str());
    for argument in [
        std::ffi::OsStr::new("--home"),
        home.as_os_str(),
        std::ffi::OsStr::new("serve"),
    ] {
        command_line.push(u16::from(b' '));
        command_line.extend(quote_windows_argument(argument));
    }
    command_line.push(0);
    let mut information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        CreateProcessW(
            program.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            CREATE_BREAKAWAY_FROM_JOB
                | CREATE_NEW_PROCESS_GROUP
                | DETACHED_PROCESS
                | EXTENDED_STARTUPINFO_PRESENT,
            std::ptr::null(),
            std::ptr::null(),
            &startup.StartupInfo,
            &mut information,
        )
    };
    if created == 0 {
        return Err(classify_spawn_error(executable, io::Error::last_os_error()));
    }
    unsafe {
        CloseHandle(information.hThread);
    }
    Ok(HostChild {
        process: information.hProcess,
    })
}

#[cfg(windows)]
fn classify_spawn_error(executable: &Path, source: io::Error) -> BootstrapError {
    if source.raw_os_error() == Some(ERROR_ACCESS_DENIED) {
        return BootstrapError::BreakawayDenied(source);
    }
    BootstrapError::SpawnFailed {
        executable: executable.to_path_buf(),
        source,
    }
}

#[cfg(not(windows))]
fn classify_spawn_error(executable: &Path, source: io::Error) -> BootstrapError {
    BootstrapError::SpawnFailed {
        executable: executable.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_executable_is_resolved_beside_the_controller_and_never_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let controller = dir.path().join("paneflow");
        std::fs::write(&controller, b"").unwrap();
        assert!(matches!(
            resolve_host_executable(&controller),
            Err(BootstrapError::HostExecutableMissing(path)) if path == dir.path().join(HOST_EXECUTABLE_FILE_NAME)
        ));
        let host = dir.path().join(HOST_EXECUTABLE_FILE_NAME);
        std::fs::write(&host, b"").unwrap();
        assert_eq!(resolve_host_executable(&controller).unwrap(), host);
    }

    #[test]
    fn a_second_owner_lock_on_the_same_home_is_refused_while_the_first_lives() {
        let home = tempfile::tempdir().unwrap();
        let first = OwnerLock::acquire(home.path()).unwrap();
        assert!(matches!(
            OwnerLock::acquire(home.path()),
            Err(OwnerLockError::Held(path)) if path == first.path()
        ));
        drop(first);
        OwnerLock::acquire(home.path()).unwrap();
    }

    #[test]
    fn an_unreachable_endpoint_probes_as_unreachable_and_a_missing_record_reads_as_none() {
        let home = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let endpoint = PathBuf::from(format!(
            r"\\.\pipe\paneflow-host-probe-{}",
            std::process::id()
        ));
        #[cfg(unix)]
        let endpoint = home.path().join("absent.sock");
        assert!(matches!(
            probe(home.path(), &endpoint, &ClientHello::local("probe-test")),
            Probe::Unreachable(_)
        ));
        assert!(read_instance_record(home.path()).is_none());
    }

    #[test]
    fn a_missing_host_executable_is_a_bounded_error_before_anything_is_spawned() {
        let home = tempfile::tempdir().unwrap();
        let controller = home.path().join("controller");
        std::fs::write(&controller, b"").unwrap();
        let error = ensure_host_running(home.path(), &controller, "bootstrap-test").unwrap_err();
        assert!(matches!(error, BootstrapError::HostExecutableMissing(_)));
        assert!(
            !paneflow_home::host_instance_record_path_in(home.path()).exists(),
            "no owner record appears without a running host"
        );
    }
}
