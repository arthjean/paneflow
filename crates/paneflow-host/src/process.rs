use serde::{Deserialize, Serialize};
use std::io;
#[cfg(windows)]
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
}

impl ProcessIdentity {
    pub fn capture(pid: u32) -> Self {
        Self {
            pid,
            started_at: process_start_time(pid),
        }
    }

    pub fn is_provably_live(&self) -> bool {
        matches!(self.verify(), ProcessVerdict::Live)
    }

    pub fn verify(&self) -> ProcessVerdict {
        let Some(recorded) = self.started_at else {
            return if process_is_absent(self.pid) {
                ProcessVerdict::Gone
            } else {
                ProcessVerdict::Unverifiable
            };
        };
        match process_start_time(self.pid) {
            Some(observed) if observed == recorded => match process_is_running(self.pid) {
                Some(true) => ProcessVerdict::Live,
                Some(false) => ProcessVerdict::Gone,
                None => ProcessVerdict::Unverifiable,
            },
            Some(_) => ProcessVerdict::Unverifiable,
            None if process_is_absent(self.pid) => ProcessVerdict::Gone,
            None => ProcessVerdict::Unverifiable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessVerdict {
    Live,
    Gone,
    Unverifiable,
}

#[cfg(windows)]
fn process_is_absent(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        io::Error::last_os_error().raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32)
    } else {
        unsafe {
            CloseHandle(handle);
        }
        false
    }
}

#[cfg(unix)]
fn process_is_absent(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    pid > 0
        && unsafe { libc::kill(pid, 0) } != 0
        && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(not(any(windows, unix)))]
fn process_is_absent(_pid: u32) -> bool {
    false
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> Option<bool> {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut code = 0u32;
    let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
    unsafe { CloseHandle(handle) };
    (ok != 0).then_some(code == STILL_ACTIVE as u32)
}

#[cfg(target_os = "linux")]
fn process_is_running(pid: u32) -> Option<bool> {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next().map(str::to_owned))
        })
        .map(|state| state != "Z" && state != "X")
}

#[cfg(target_os = "macos")]
fn process_is_running(pid: u32) -> Option<bool> {
    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 0);
    let pid = pid?;
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast::<libc::c_void>(),
            size,
        )
    };
    (written == size).then_some(info.pbi_status != libc::SZOMB)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn process_is_running(_pid: u32) -> Option<bool> {
    None
}

#[cfg(windows)]
pub fn process_start_time(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return None;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut creation: FILETIME = unsafe { std::mem::zeroed() };
    let mut exit: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    unsafe { CloseHandle(handle) };
    (ok != 0)
        .then(|| (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

#[cfg(target_os = "linux")]
pub fn process_start_time(pid: u32) -> Option<u64> {
    parse_proc_stat_starttime(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

#[cfg(any(target_os = "linux", test))]
fn parse_proc_stat_starttime(stat: &str) -> Option<u64> {
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "macos")]
pub fn process_start_time(pid: u32) -> Option<u64> {
    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast::<libc::c_void>(),
            size,
        )
    };
    if written != size {
        return unreaped_start_time(pid);
    }
    Some(info.pbi_start_tvsec.saturating_mul(1_000_000) + info.pbi_start_tvusec)
}

#[cfg(target_os = "macos")]
fn unreaped_start_time(pid: i32) -> Option<u64> {
    const KINFO_PROC_BYTES: usize = 648;
    const P_PID_OFFSET: usize = 40;
    let mut name = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    let mut buffer = [0u8; KINFO_PROC_BYTES];
    let mut length = buffer.len();
    let status = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            name.len() as libc::c_uint,
            buffer.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 || length != KINFO_PROC_BYTES {
        return None;
    }
    let recorded_pid = i32::from_ne_bytes(buffer[P_PID_OFFSET..P_PID_OFFSET + 4].try_into().ok()?);
    if recorded_pid != pid {
        return None;
    }
    let seconds = u64::try_from(i64::from_ne_bytes(buffer[0..8].try_into().ok()?)).ok()?;
    let micros = u64::try_from(i32::from_ne_bytes(buffer[8..12].try_into().ok()?)).ok()?;
    Some(seconds.saturating_mul(1_000_000) + micros)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn process_start_time(_pid: u32) -> Option<u64> {
    None
}

#[cfg(unix)]
pub fn verified_process_group(child_pid: u32) -> Option<i32> {
    let pid = i32::try_from(child_pid).ok().filter(|pid| *pid > 0)?;
    (unsafe { libc::getpgid(pid) } == pid).then_some(pid)
}

#[cfg(unix)]
pub(crate) struct UnixProcessTreeOwner {
    root: ProcessIdentity,
    group: Option<i32>,
    descendants: Vec<ProcessIdentity>,
    snapshot_failed: bool,
    #[cfg(target_os = "macos")]
    root_reaped: bool,
}

#[cfg(target_os = "linux")]
fn unix_process_entries() -> io::Result<Vec<(ProcessIdentity, u32, i32)>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let Some((_, stat)) = stat.rsplit_once(')') else {
            continue;
        };
        let fields: Vec<_> = stat.split_whitespace().collect();
        if fields.len() < 20 || fields[0] == "Z" || fields[0] == "X" {
            continue;
        }
        if let (Ok(parent), Ok(group), Ok(started)) =
            (fields[1].parse(), fields[2].parse(), fields[19].parse())
        {
            entries.push((
                ProcessIdentity {
                    pid,
                    started_at: Some(started),
                },
                parent,
                group,
            ));
        }
    }
    Ok(entries)
}

#[cfg(target_os = "macos")]
fn unix_process_entries() -> io::Result<Vec<(ProcessIdentity, u32, i32)>> {
    let bytes = unsafe { libc::proc_listpids(1, 0, std::ptr::null_mut(), 0) };
    if bytes <= 0 {
        return Err(io::Error::last_os_error());
    }
    let mut pids = vec![0i32; bytes as usize / std::mem::size_of::<i32>() + 1024];
    let written = unsafe {
        libc::proc_listpids(
            1,
            0,
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(pids.as_slice()) as i32,
        )
    };
    if written <= 0 {
        return Err(io::Error::last_os_error());
    }
    let mut entries = Vec::new();
    for pid in pids
        .into_iter()
        .take(written as usize / std::mem::size_of::<i32>())
        .filter(|pid| *pid > 0)
    {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let written = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        if written == size && info.pbi_status != libc::SZOMB {
            entries.push((
                ProcessIdentity {
                    pid: pid as u32,
                    started_at: Some(
                        info.pbi_start_tvsec.saturating_mul(1_000_000) + info.pbi_start_tvusec,
                    ),
                },
                info.pbi_ppid,
                info.pbi_pgid as i32,
            ));
        }
    }
    Ok(entries)
}

#[cfg(unix)]
impl UnixProcessTreeOwner {
    pub(crate) fn new(root: ProcessIdentity) -> Self {
        Self {
            root,
            group: verified_process_group(root.pid),
            descendants: Vec::new(),
            snapshot_failed: false,
            #[cfg(target_os = "macos")]
            root_reaped: false,
        }
    }

    pub(crate) fn root_is_running(&self) -> bool {
        self.root.is_provably_live()
    }

    pub(crate) fn discover(&mut self) {
        if self.root.pid == 0 || self.root.started_at.is_none() {
            self.snapshot_failed = true;
            return;
        }
        let entries = match unix_process_entries() {
            Ok(entries) => entries,
            Err(_) => {
                self.snapshot_failed = true;
                return;
            }
        };
        self.snapshot_failed = false;
        let root_retained = self.root.started_at.is_some()
            && process_start_time(self.root.pid) == self.root.started_at;
        let group_retained = root_retained
            || self.descendants.iter().any(|known| {
                known.verify() == ProcessVerdict::Live
                    && self.group == Some(unsafe { libc::getpgid(known.pid as i32) })
            });
        if !group_retained {
            self.group = None;
        }
        loop {
            let before = self.descendants.len();
            for (identity, parent, group) in &entries {
                if identity.pid == self.root.pid || self.descendants.contains(identity) {
                    continue;
                }
                let related = (root_retained && *parent == self.root.pid)
                    || self.group == Some(*group)
                    || self.descendants.iter().any(|known| {
                        known.pid == *parent && known.verify() == ProcessVerdict::Live
                    });
                if related
                    && !matches!((self.root.started_at, identity.started_at), (Some(root), Some(child)) if child < root)
                {
                    self.descendants.push(*identity);
                }
            }
            if self.descendants.len() == before {
                break;
            }
        }
        if self.root.verify() == ProcessVerdict::Gone
            && !entries
                .iter()
                .any(|(_, _, group)| self.group == Some(*group))
        {
            self.group = None;
        }
    }

    pub(crate) fn unresolved(&mut self) -> usize {
        self.descendants
            .retain(|identity| identity.verify() != ProcessVerdict::Gone);
        self.descendants.len() + usize::from(self.snapshot_failed)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn root_reaped(&mut self) {
        self.root_reaped = true;
    }

    pub(crate) fn signal(&mut self, force: bool) {
        self.discover();
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        for identity in self
            .descendants
            .iter()
            .rev()
            .chain(std::iter::once(&self.root))
        {
            if identity.verify() == ProcessVerdict::Live {
                #[cfg(target_os = "linux")]
                signal_verified_unix(*identity, signal);
                #[cfg(target_os = "macos")]
                if *identity == self.root && !self.root_reaped {
                    if unsafe { libc::kill(identity.pid as i32, signal) } != 0 {
                        log::warn!(
                            "paneflow-host: retained root pid={} signal failed: {}",
                            identity.pid,
                            io::Error::last_os_error()
                        );
                    }
                } else if let Err(error) =
                    MacSignalTarget::acquire(*identity).and_then(|target| target.signal(signal))
                {
                    log::warn!(
                        "paneflow-host: descendant pid={} signal is unresolved: {error}",
                        identity.pid
                    );
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn signal_verified_unix(identity: ProcessIdentity, signal: i32) {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid, 0) };
    if fd < 0 {
        return;
    }
    let handle = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    if identity.verify() == ProcessVerdict::Live {
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                handle.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            );
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn await_exit_unreaped(pid: u32) -> bool {
    let id = libc::id_t::from(pid);
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let waited =
            unsafe { libc::waitid(libc::P_PID, id, &mut info, libc::WEXITED | libc::WNOWAIT) };
        if waited == 0 {
            return true;
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return false;
        }
    }
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct MacProcessUniqueInfo {
    uuid: [u8; 16],
    unique_id: u64,
    parent_unique_id: u64,
    pid_version: i32,
    reserved: [u32; 5],
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct MacProcessBsdUniqueInfo {
    bsd: libc::proc_bsdinfo,
    unique: MacProcessUniqueInfo,
}

#[cfg(target_os = "macos")]
struct MacSignalTarget {
    audit_token: [u32; 8],
}

#[cfg(target_os = "macos")]
impl MacSignalTarget {
    fn acquire(identity: ProcessIdentity) -> io::Result<Self> {
        const PROC_PIDT_BSDINFOWITHUNIQID: i32 = 18;
        let pid = i32::try_from(identity.pid)
            .ok()
            .filter(|pid| *pid > 0)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        let mut info: MacProcessBsdUniqueInfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<MacProcessBsdUniqueInfo>() as i32;
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                PROC_PIDT_BSDINFOWITHUNIQID,
                0,
                (&raw mut info).cast(),
                size,
            )
        };
        if written != size {
            return Err(io::Error::last_os_error());
        }
        let started_at =
            info.bsd.pbi_start_tvsec.saturating_mul(1_000_000) + info.bsd.pbi_start_tvusec;
        if info.bsd.pbi_pid != identity.pid
            || identity.started_at != Some(started_at)
            || info.bsd.pbi_status == libc::SZOMB
        {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        let mut audit_token = [0; 8];
        audit_token[5] = identity.pid;
        audit_token[7] = info.unique.pid_version as u32;
        Ok(Self { audit_token })
    }

    fn signal(&self, signal: i32) -> io::Result<()> {
        type SignalWithAuditToken = unsafe extern "C" fn(*const [u32; 8], i32) -> i32;
        static SIGNAL: std::sync::OnceLock<Option<SignalWithAuditToken>> =
            std::sync::OnceLock::new();
        let signal_with_token = SIGNAL
            .get_or_init(|| {
                let address = unsafe {
                    libc::dlsym(libc::RTLD_DEFAULT, c"proc_signal_with_audittoken".as_ptr())
                };
                (!address.is_null()).then(|| unsafe {
                    std::mem::transmute::<*mut libc::c_void, SignalWithAuditToken>(address)
                })
            })
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOSYS))?;
        let status = unsafe { signal_with_token(&raw const self.audit_token, signal) };
        if status == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status))
        }
    }
}

#[cfg(windows)]
pub const WINDOWS_PROCESS_TREE_TERMINATION_BUDGET: Duration = Duration::from_secs(5);

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsProcessTreeTerminationResult {
    pub targeted: usize,
    pub terminate_requested: usize,
    pub already_exited: usize,
    pub failures: usize,
    pub timed_out: usize,
    pub deadline_exhausted: bool,
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
}

#[cfg(windows)]
pub fn windows_process_entries_named() -> io::Result<Vec<WindowsProcessEntry>> {
    use std::mem;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }

    let mut entries: Vec<WindowsProcessEntry> = Vec::with_capacity(256);
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    if unsafe { Process32FirstW(snap, &mut entry) } == 0 {
        let error = io::Error::last_os_error();
        unsafe { CloseHandle(snap) };
        return Err(error);
    }
    loop {
        let end = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        entries.push(WindowsProcessEntry {
            pid: entry.th32ProcessID,
            parent_pid: entry.th32ParentProcessID,
            name: String::from_utf16_lossy(&entry.szExeFile[..end]),
        });
        if unsafe { Process32NextW(snap, &mut entry) } == 0 {
            break;
        }
    }
    unsafe { CloseHandle(snap) };
    Ok(entries)
}

#[cfg(windows)]
fn windows_process_entries() -> io::Result<Vec<(u32, u32)>> {
    Ok(windows_process_entries_named()?
        .into_iter()
        .map(|entry| (entry.pid, entry.parent_pid))
        .collect())
}

#[cfg(windows)]
fn windows_root_exit_time_bounds_adoption(
    root_started_at: Option<u64>,
    root_exited_at: Option<u64>,
    child_started_at: Option<u64>,
) -> bool {
    match (root_started_at, child_started_at) {
        (Some(_), Some(child)) if root_exited_at.is_some_and(|exit| child > exit) => false,
        (Some(parent), Some(child)) => child >= parent,
        _ => true,
    }
}

#[cfg(all(windows, test))]
fn windows_descendants_postorder(root_pid: u32, entries: &[(u32, u32)]) -> Vec<u32> {
    fn visit(
        pid: u32,
        entries: &[(u32, u32)],
        seen: &mut std::collections::HashSet<u32>,
        out: &mut Vec<u32>,
    ) -> bool {
        if !seen.insert(pid) {
            return false;
        }
        let mut children: Vec<u32> = entries
            .iter()
            .filter_map(|(child, parent)| (*parent == pid).then_some(*child))
            .collect();
        children.sort_unstable();
        for child in children {
            if visit(child, entries, seen, out) {
                out.push(child);
            }
        }
        true
    }

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let _ = visit(root_pid, entries, &mut seen, &mut out);
    out
}

#[cfg(all(windows, test))]
fn windows_process_tree_targets(
    root_pid: u32,
    entries: &[(u32, u32)],
    include_root: bool,
) -> Vec<u32> {
    let mut targets = windows_descendants_postorder(root_pid, entries);
    if include_root && root_pid != 0 {
        targets.push(root_pid);
    }
    targets
}

#[cfg(windows)]
fn windows_wait_timeout_ms(remaining: Duration) -> Option<u32> {
    const MAX_FINITE_WAIT_MS: u32 = u32::MAX - 1;
    let milliseconds = remaining.as_millis().min(u128::from(MAX_FINITE_WAIT_MS)) as u32;
    (milliseconds != 0).then_some(milliseconds)
}

#[cfg(windows)]
struct WindowsTerminationHandle {
    identity: ProcessIdentity,
    handle: std::os::windows::io::OwnedHandle,
}

#[cfg(windows)]
impl WindowsTerminationHandle {
    fn open(identity: ProcessIdentity) -> io::Result<Self> {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        };
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | 0x0010_0000,
                0,
                identity.pid,
            )
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let handle = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle) };
        let process = Self { identity, handle };
        if process.started_at() != identity.started_at || identity.started_at.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "process identity changed before its termination handle was acquired",
            ));
        }
        Ok(process)
    }

    fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        use std::os::windows::io::AsRawHandle;
        self.handle.as_raw_handle()
    }

    fn times(&self) -> Option<(u64, u64)> {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::GetProcessTimes;
        let mut creation: FILETIME = unsafe { std::mem::zeroed() };
        let mut exit: FILETIME = unsafe { std::mem::zeroed() };
        let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
        let mut user: FILETIME = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            GetProcessTimes(self.raw(), &mut creation, &mut exit, &mut kernel, &mut user)
        };
        (ok != 0).then(|| {
            (
                (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime),
                (u64::from(exit.dwHighDateTime) << 32) | u64::from(exit.dwLowDateTime),
            )
        })
    }

    fn started_at(&self) -> Option<u64> {
        self.times().map(|(creation, _)| creation)
    }

    fn exited_at(&self) -> Option<u64> {
        if !self.exited() {
            return None;
        }
        self.times().map(|(_, exit)| exit).filter(|exit| *exit != 0)
    }

    fn exited(&self) -> bool {
        (unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(self.raw(), 0) })
            == windows_sys::Win32::Foundation::WAIT_OBJECT_0
    }
}

#[cfg(windows)]
pub(crate) struct WindowsProcessTreeOwner {
    root: ProcessIdentity,
    handles: Vec<WindowsTerminationHandle>,
    unresolved: Vec<ProcessIdentity>,
    snapshot_failed: bool,
    inherited_links: Vec<(u32, u32)>,
}

#[cfg(windows)]
impl WindowsProcessTreeOwner {
    pub(crate) fn new(root: ProcessIdentity) -> Self {
        let mut owner = Self {
            root,
            handles: Vec::new(),
            unresolved: Vec::new(),
            snapshot_failed: false,
            inherited_links: Vec::new(),
        };
        owner.retain(root);
        if let Ok(entries) = windows_process_entries() {
            owner.record_inherited_links(root.pid, &entries);
        }
        owner
    }

    fn record_inherited_links(&mut self, parent: u32, entries: &[(u32, u32)]) {
        self.inherited_links.extend(
            entries
                .iter()
                .copied()
                .filter(|&(child, linked)| linked == parent && child != parent),
        );
    }

    fn retain(&mut self, identity: ProcessIdentity) {
        if self
            .handles
            .iter()
            .any(|process| process.identity == identity)
            || self.unresolved.contains(&identity)
        {
            return;
        }
        match WindowsTerminationHandle::open(identity) {
            Ok(process) => self.handles.push(process),
            Err(_) => self.unresolved.push(identity),
        }
    }

    pub(crate) fn root_is_running(&self) -> bool {
        self.root.is_provably_live()
    }

    pub(crate) fn discover(&mut self) {
        match windows_process_entries() {
            Ok(entries) => self.discover_in(&entries),
            Err(_) => self.snapshot_failed = true,
        }
    }

    fn discover_in(&mut self, entries: &[(u32, u32)]) {
        self.snapshot_failed = false;
        let mut index = 0;
        while index < self.handles.len() {
            let root = self.handles[index].identity;
            let root_exited_at = self.handles[index].exited_at();
            index += 1;
            for (pid, parent) in entries {
                if *parent != root.pid || *pid == root.pid {
                    continue;
                }
                let identity = ProcessIdentity::capture(*pid);
                if self.handles.iter().any(|known| known.identity == identity)
                    || self.unresolved.contains(&identity)
                {
                    continue;
                }
                if !windows_root_exit_time_bounds_adoption(
                    root.started_at,
                    root_exited_at,
                    identity.started_at,
                ) {
                    continue;
                }
                match (root.started_at, identity.started_at) {
                    (Some(_), Some(_)) => {}
                    _ => {
                        if !self.inherited_links.contains(&(*pid, root.pid))
                            && identity.verify() != ProcessVerdict::Gone
                        {
                            self.unresolved.push(identity);
                        }
                        continue;
                    }
                }
                match WindowsTerminationHandle::open(identity) {
                    Ok(process) => match windows_process_entries() {
                        Ok(current) if current.contains(&(*pid, root.pid)) => {
                            self.handles.push(process);
                            self.record_inherited_links(*pid, entries);
                        }
                        Ok(_) => {}
                        Err(_) => {
                            self.snapshot_failed = true;
                        }
                    },
                    Err(_) => match windows_process_entries() {
                        Ok(current)
                            if current.contains(&(*pid, root.pid))
                                && identity.verify() == ProcessVerdict::Live =>
                        {
                            self.unresolved.push(identity)
                        }
                        Ok(_) => {
                            if identity.verify() == ProcessVerdict::Unverifiable {
                                self.unresolved.push(identity);
                            }
                        }
                        Err(_) => self.snapshot_failed = true,
                    },
                }
            }
        }
    }

    pub(crate) fn unresolved(&mut self) -> usize {
        self.unresolved
            .retain(|identity| identity.verify() != ProcessVerdict::Gone);
        self.handles
            .iter()
            .filter(|process| process.identity != self.root && !process.exited())
            .count()
            + self.unresolved.len()
            + usize::from(self.snapshot_failed)
    }

    pub(crate) fn terminate(
        &mut self,
        deadline: std::time::Instant,
    ) -> WindowsProcessTreeTerminationResult {
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};
        self.discover();
        let retry = std::mem::take(&mut self.unresolved);
        for identity in retry {
            if identity.verify() != ProcessVerdict::Gone {
                self.retain(identity);
            }
        }
        let mut result = WindowsProcessTreeTerminationResult::default();
        for process in self.handles.iter().rev() {
            result.targeted += 1;
            if process.exited() {
                result.already_exited += 1;
            } else if process.started_at() != process.identity.started_at {
                result.failures += 1;
            } else if unsafe { TerminateProcess(process.raw(), 1) } != 0 {
                result.terminate_requested += 1;
            } else if !process.exited() {
                result.failures += 1;
            }
        }
        for process in &self.handles {
            if process.exited() {
                continue;
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let timeout = windows_wait_timeout_ms(remaining).unwrap_or(0);
            if unsafe { WaitForSingleObject(process.raw(), timeout) } != WAIT_OBJECT_0 {
                result.timed_out += 1;
            }
        }
        result.failures += self.unresolved.len() + usize::from(self.snapshot_failed);
        result.deadline_exhausted = result.timed_out > 0 && std::time::Instant::now() >= deadline;
        result
    }
}

#[cfg(windows)]
pub fn terminate_windows_process_tree(
    root_pid: u32,
    deadline: std::time::Instant,
) -> WindowsProcessTreeTerminationResult {
    WindowsProcessTreeOwner::new(ProcessIdentity::capture(root_pid)).terminate(deadline)
}

#[cfg(target_os = "linux")]
pub fn process_argv(pid: u32) -> Option<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let argv = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect::<Vec<_>>();
    (!argv.is_empty()).then_some(argv)
}

#[cfg(target_os = "macos")]
pub fn process_argv(pid: u32) -> Option<Vec<String>> {
    parse_procargs2(&kernel_process_arguments(pid)?)
}

#[cfg(any(target_os = "macos", test))]
fn parse_procargs2(buffer: &[u8]) -> Option<Vec<String>> {
    if buffer.len() < 4 {
        return None;
    }
    let argc = i32::from_ne_bytes(buffer[..4].try_into().ok()?);
    if argc < 1 {
        return None;
    }
    let rest = &buffer[4..];
    let executable_end = rest.iter().position(|byte| *byte == 0)?;
    let mut cursor = executable_end;
    while cursor < rest.len() && rest[cursor] == 0 {
        cursor += 1;
    }
    let mut argv = Vec::with_capacity((argc as usize).min(rest.len()));
    for _ in 0..argc {
        let suffix = rest.get(cursor..)?;
        let end = suffix.iter().position(|byte| *byte == 0)?;
        if end == 0 {
            return None;
        }
        argv.push(String::from_utf8_lossy(&suffix[..end]).into_owned());
        cursor = cursor.checked_add(end + 1)?;
    }
    Some(argv)
}

#[cfg(target_os = "macos")]
fn kernel_process_arguments(pid: u32) -> Option<Vec<u8>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut size: libc::size_t = 0;
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || size == 0
    {
        return None;
    }
    let mut buffer = vec![0u8; size];
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast::<libc::c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    buffer.truncate(size);
    Some(buffer)
}

#[cfg(windows)]
pub fn process_argv(pid: u32) -> Option<Vec<String>> {
    let command_line = command_line(pid)?;
    let argv = command_line_to_argv(&command_line);
    (!argv.is_empty()).then_some(argv)
}

#[cfg(windows)]
fn command_line(pid: u32) -> Option<String> {
    use std::mem;
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, UNICODE_STRING};
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PEB, PROCESS_BASIC_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_VM_READ, RTL_USER_PROCESS_PARAMETERS,
    };

    const MAX_COMMAND_LINE_BYTES: usize = 64 * 1024;

    if pid == 0 {
        return None;
    }

    unsafe fn read_remote<T: Copy>(handle: HANDLE, ptr: *const T) -> Option<T> {
        let mut value: T = unsafe { mem::zeroed() };
        let mut read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                handle,
                ptr.cast(),
                (&mut value as *mut T).cast(),
                mem::size_of::<T>(),
                &mut read,
            )
        };
        (ok != 0 && read == mem::size_of::<T>()).then_some(value)
    }

    unsafe fn read_unicode_string(handle: HANDLE, value: UNICODE_STRING) -> Option<String> {
        let len = value.Length as usize;
        if len == 0
            || len > MAX_COMMAND_LINE_BYTES
            || !len.is_multiple_of(2)
            || value.Buffer.is_null()
        {
            return None;
        }
        let mut bytes = vec![0u16; len / 2];
        let mut read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                handle,
                value.Buffer.cast(),
                bytes.as_mut_ptr().cast(),
                len,
                &mut read,
            )
        };
        (ok != 0 && read == len).then(|| String::from_utf16_lossy(&bytes))
    }

    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let result = (|| {
        let mut info: PROCESS_BASIC_INFORMATION = unsafe { mem::zeroed() };
        let status = unsafe {
            NtQueryInformationProcess(
                handle,
                ProcessBasicInformation,
                (&mut info as *mut PROCESS_BASIC_INFORMATION).cast(),
                mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if status < 0 || info.PebBaseAddress.is_null() {
            return None;
        }
        let peb: PEB = unsafe { read_remote(handle, info.PebBaseAddress.cast())? };
        if peb.ProcessParameters.is_null() {
            return None;
        }
        let parameters: RTL_USER_PROCESS_PARAMETERS =
            unsafe { read_remote(handle, peb.ProcessParameters.cast())? };
        unsafe { read_unicode_string(handle, parameters.CommandLine) }
    })();
    unsafe { CloseHandle(handle) };
    result
}

#[cfg(windows)]
fn command_line_to_argv(command_line: &str) -> Vec<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

    let mut wide: Vec<u16> = command_line
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut argc = 0i32;
    let argv = unsafe { CommandLineToArgvW(wide.as_mut_ptr(), &mut argc) };
    if argv.is_null() || argc <= 0 {
        return command_line
            .split_whitespace()
            .map(str::to_string)
            .collect();
    }
    let mut args = Vec::with_capacity(argc as usize);
    let slice = unsafe { std::slice::from_raw_parts(argv, argc as usize) };
    for &ptr in slice {
        if ptr.is_null() {
            continue;
        }
        let mut len = 0usize;
        unsafe {
            while *ptr.add(len) != 0 {
                len += 1;
            }
            args.push(String::from_utf16_lossy(std::slice::from_raw_parts(
                ptr, len,
            )));
        }
    }
    unsafe { LocalFree(argv.cast()) };
    args
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn process_argv(_pid: u32) -> Option<Vec<String>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn a_child_born_after_the_root_exit_belongs_to_a_recycled_pid_not_to_the_root() {
        assert!(windows_root_exit_time_bounds_adoption(
            Some(100),
            None,
            Some(150)
        ));
        assert!(windows_root_exit_time_bounds_adoption(
            Some(100),
            Some(200),
            Some(150)
        ));
        assert!(!windows_root_exit_time_bounds_adoption(
            Some(100),
            Some(200),
            Some(201)
        ));
        assert!(!windows_root_exit_time_bounds_adoption(
            Some(100),
            None,
            Some(50)
        ));
        assert!(windows_root_exit_time_bounds_adoption(
            None,
            Some(200),
            Some(500)
        ));
        assert!(windows_root_exit_time_bounds_adoption(
            Some(100),
            Some(200),
            None
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn an_exited_root_keeps_its_identity_and_group_until_it_is_reaped() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 0.2; exit 0"])
            .spawn()
            .expect("shell spawns");
        let pid = child.id();
        let started = process_start_time(pid);
        assert!(started.is_some(), "the live root has a start time");
        assert!(
            await_exit_unreaped(pid),
            "the exit is observed without reaping"
        );
        assert_eq!(
            process_start_time(pid),
            started,
            "the unreaped root still reports its start time"
        );
        assert!(child.wait().expect("reap").success());
        assert_eq!(process_start_time(pid), None, "the reaped root is gone");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_signals_check_kernel_pid_versions_without_a_task_control_port() {
        use std::os::unix::process::ExitStatusExt;

        assert_eq!(std::mem::size_of::<MacProcessUniqueInfo>(), 56);
        for signal in [libc::SIGTERM, libc::SIGKILL] {
            let mut child = std::process::Command::new("/bin/sleep")
                .arg("30")
                .spawn()
                .unwrap();
            let actual = ProcessIdentity::capture(child.id());
            let stale = ProcessIdentity {
                started_at: actual.started_at.map(|time| time.wrapping_add(1)),
                ..actual
            };
            assert!(MacSignalTarget::acquire(stale).is_err());
            let target = MacSignalTarget::acquire(actual).unwrap();
            let mut stale_token = MacSignalTarget {
                audit_token: target.audit_token,
            };
            stale_token.audit_token[7] = stale_token.audit_token[7].wrapping_add(1);
            assert_eq!(
                stale_token.signal(signal).unwrap_err().raw_os_error(),
                Some(libc::ESRCH)
            );
            assert!(actual.is_provably_live());
            target.signal(signal).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            assert_eq!(status.signal(), Some(signal));
            assert_eq!(actual.verify(), ProcessVerdict::Gone);
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_mismatched_identity_never_signals_the_process_behind_the_pid() {
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/D", "/Q", "/C", "pause"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let actual = ProcessIdentity::capture(child.id());
        let stale = ProcessIdentity {
            started_at: actual.started_at.map(|time| time.wrapping_add(1)),
            ..actual
        };
        let mut owner = WindowsProcessTreeOwner::new(stale);
        let result = owner.terminate(std::time::Instant::now() + Duration::from_millis(20));
        assert_eq!(result.terminate_requested, 0);
        assert!(owner.unresolved() > 0);
        assert!(actual.is_provably_live());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_stale_parent_link_without_a_start_time_is_never_adopted() {
        let system = ProcessIdentity::capture(4);
        let mut root = std::process::Command::new("cmd.exe")
            .args(["/D", "/Q", "/C", "pause"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let identity = ProcessIdentity::capture(root.id());
        let stale_link = [(system.pid, identity.pid)];

        let mut inherited = WindowsProcessTreeOwner::new(identity);
        inherited.record_inherited_links(identity.pid, &stale_link);
        inherited.discover_in(&stale_link);
        assert_eq!(
            inherited.unresolved(),
            0,
            "a process that listed this pid as its parent before the root existed is not its child"
        );

        let mut late = WindowsProcessTreeOwner::new(identity);
        late.discover_in(&stale_link);
        if system.started_at.is_none() {
            assert_eq!(
                late.unresolved(),
                1,
                "a link that appears after the root exists stays unresolved without a start time"
            );
        }

        root.kill().unwrap();
        root.wait().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn retained_descendant_handles_settle_after_the_root_has_exited() {
        let mut root = std::process::Command::new("cmd.exe")
            .args(["/D", "/Q", "/C", "pause"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/D", "/Q", "/C", "pause"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut owner = WindowsProcessTreeOwner::new(ProcessIdentity::capture(root.id()));
        let descendant = ProcessIdentity::capture(child.id());
        owner.retain(descendant);
        root.kill().unwrap();
        root.wait().unwrap();
        assert_eq!(owner.unresolved(), 1);
        assert!(descendant.is_provably_live());
        owner.terminate(std::time::Instant::now() + Duration::from_secs(2));
        assert_eq!(owner.unresolved(), 0);
        assert!(!descendant.is_provably_live());
        child.wait().unwrap();
    }

    #[test]
    fn the_current_process_proves_its_own_identity() {
        let identity = ProcessIdentity::capture(std::process::id());
        assert!(
            identity.started_at.is_some(),
            "the host needs kernel start-time identity on every shipping target"
        );
        assert!(identity.is_provably_live());
        let unknown = ProcessIdentity {
            pid: std::process::id(),
            started_at: None,
        };
        assert!(
            !unknown.is_provably_live(),
            "a pid without a start time never proves ownership"
        );
        let stale = ProcessIdentity {
            pid: std::process::id(),
            started_at: identity.started_at.map(|t| t.wrapping_add(1)),
        };
        assert!(!stale.is_provably_live());
    }

    #[test]
    fn proc_stat_starttime_survives_hostile_comm_names() {
        let plain = "1234 (zsh) S 1 1234 1234 0 -1 4194304 0 0 0 0 5 3 0 0 20 0 11 0 9876543 123 456 18446744073709551615";
        assert_eq!(parse_proc_stat_starttime(plain), Some(9876543));
        let hostile = "1234 (next-server (v15)) S 1 1234 1234 0 -1 4194304 0 0 0 0 5 3 0 0 20 0 11 0 424242 123 456";
        assert_eq!(parse_proc_stat_starttime(hostile), Some(424242));
        let split_comm =
            "4321 (evil) (x) S 1 4321 4321 0 -1 4194304 0 0 0 0 5 3 0 0 20 0 11 0 777777 123 456";
        assert_eq!(parse_proc_stat_starttime(split_comm), Some(777777));
        assert_eq!(parse_proc_stat_starttime("1234 (zsh) S 1 1234"), None);
        assert_eq!(parse_proc_stat_starttime(""), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_start_time_matches_field_22_of_proc_stat() {
        let pid = std::process::id();
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let field_22 = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(19)
            .unwrap();
        assert_eq!(process_start_time(pid), Some(field_22.parse().unwrap()));
    }

    #[test]
    fn parse_procargs2_extracts_argv_after_exec_path() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&2i32.to_ne_bytes());
        buf.extend_from_slice(b"/usr/local/bin/node\0\0\0\0");
        buf.extend_from_slice(b"node\0/repo/node_modules/.bin/vite\0");
        buf.extend_from_slice(b"PATH=/usr/bin\0");
        assert_eq!(
            parse_procargs2(&buf),
            Some(vec![
                "node".to_string(),
                "/repo/node_modules/.bin/vite".to_string()
            ])
        );
        assert_eq!(parse_procargs2(&[]), None);
        assert_eq!(parse_procargs2(&[1, 0, 0]), None);
        assert_eq!(parse_procargs2(&0i32.to_ne_bytes()), None);
    }

    #[test]
    fn the_current_process_exposes_its_argv() {
        let argv = process_argv(std::process::id());
        if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
            assert!(argv.is_some_and(|argv| !argv.is_empty()));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_command_line_argv_splits_quoted_node_frontend() {
        let args = command_line_to_argv(
            r#""C:\Program Files\nodejs\node.exe" "C:\repo\node_modules\.bin\vite" --host"#,
        );
        assert_eq!(
            args,
            vec![
                r"C:\Program Files\nodejs\node.exe".to_string(),
                r"C:\repo\node_modules\.bin\vite".to_string(),
                "--host".to_string(),
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_descendants_postorder_places_children_before_parent() {
        let entries = vec![(10, 1), (11, 10), (12, 10), (13, 12), (20, 1)];
        assert_eq!(
            windows_descendants_postorder(10, &entries),
            vec![11, 13, 12]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_process_tree_targets_stay_scoped_and_put_root_last() {
        let entries = vec![(10, 1), (11, 10), (12, 10), (13, 12), (20, 1)];
        assert_eq!(
            windows_process_tree_targets(10, &entries, true),
            vec![11, 13, 12, 10]
        );
        assert_eq!(
            windows_process_tree_targets(10, &entries, false),
            vec![11, 13, 12]
        );
        let cyclic_entries = vec![(10, 11), (11, 10), (20, 1)];
        assert_eq!(
            windows_process_tree_targets(10, &cyclic_entries, true),
            vec![11, 10]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_wait_timeout_never_rounds_past_global_budget() {
        assert_eq!(windows_wait_timeout_ms(Duration::ZERO), None);
        assert_eq!(windows_wait_timeout_ms(Duration::from_micros(999)), None);
        assert_eq!(windows_wait_timeout_ms(Duration::from_millis(1)), Some(1));
        assert_eq!(
            windows_wait_timeout_ms(Duration::from_micros(1_999)),
            Some(1)
        );
        assert_eq!(
            windows_wait_timeout_ms(Duration::from_millis(u64::from(u32::MAX) + 1)),
            Some(u32::MAX - 1)
        );
    }
}
