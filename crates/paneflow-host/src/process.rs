use serde::{Deserialize, Serialize};
use std::io;
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
            return ProcessVerdict::Unverifiable;
        };
        match process_start_time(self.pid) {
            Some(observed) if observed == recorded => {
                if process_is_running(self.pid) {
                    ProcessVerdict::Live
                } else {
                    ProcessVerdict::Gone
                }
            }
            Some(_) => ProcessVerdict::Unverifiable,
            None => ProcessVerdict::Gone,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessVerdict {
    Live,
    Gone,
    Unverifiable,
}

impl ProcessVerdict {
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Gone => "gone",
            Self::Unverifiable => "unverifiable",
        }
    }
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut code = 0u32;
    let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
    unsafe { CloseHandle(handle) };
    ok != 0 && code == STILL_ACTIVE as u32
}

#[cfg(target_os = "linux")]
fn process_is_running(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next().map(str::to_owned))
        })
        .is_some_and(|state| state != "Z" && state != "X")
}

#[cfg(target_os = "macos")]
fn process_is_running(pid: u32) -> bool {
    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 0);
    let Some(pid) = pid else {
        return false;
    };
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
    written == size && info.pbi_status != libc::SZOMB
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn process_is_running(_pid: u32) -> bool {
    false
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
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
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
        return None;
    }
    Some(info.pbi_start_tvsec.saturating_mul(1_000_000) + info.pbi_start_tvusec)
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
pub const UNIX_SHUTDOWN_GRACE: Duration = Duration::from_millis(100);

#[cfg(unix)]
pub fn terminate_process_group(group: i32, grace: Duration) {
    unsafe {
        libc::kill(-group, libc::SIGTERM);
    }
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline {
        let group_exists = unsafe { libc::kill(-group, 0) == 0 }
            || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if !group_exists {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe {
        libc::kill(-group, libc::SIGKILL);
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
pub(crate) struct WindowsProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
}

#[cfg(windows)]
pub(crate) fn windows_process_entries_named() -> io::Result<Vec<WindowsProcessEntry>> {
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

#[cfg(windows)]
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
    pid: u32,
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl Drop for WindowsTerminationHandle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
fn request_windows_pid_termination(
    pid: u32,
    result: &mut WindowsProcessTreeTerminationResult,
) -> Option<WindowsTerminationHandle> {
    use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    };
    const SYNCHRONIZE: u32 = 0x0010_0000;

    let handle = unsafe { OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        let os_error = io::Error::last_os_error().raw_os_error();
        result.failures = result.failures.saturating_add(1);
        log::debug!(
            "paneflow-host: process cleanup could not open pid={pid} (os_error={os_error:?})"
        );
        return None;
    }
    let process = WindowsTerminationHandle { pid, handle };

    match unsafe { WaitForSingleObject(process.handle, 0) } {
        WAIT_OBJECT_0 => {
            result.already_exited = result.already_exited.saturating_add(1);
            return None;
        }
        WAIT_TIMEOUT => {}
        WAIT_FAILED => {
            let os_error = io::Error::last_os_error().raw_os_error();
            result.failures = result.failures.saturating_add(1);
            log::warn!(
                "paneflow-host: process cleanup precheck failed for pid={pid} (os_error={os_error:?})"
            );
        }
        status => {
            result.failures = result.failures.saturating_add(1);
            log::warn!(
                "paneflow-host: process cleanup precheck returned status={status:#x} for pid={pid}"
            );
        }
    }

    if unsafe { TerminateProcess(process.handle, 1) } == 0 {
        let terminate_error = io::Error::last_os_error().raw_os_error();
        let exited = unsafe { WaitForSingleObject(process.handle, 0) } == WAIT_OBJECT_0;
        if exited {
            result.already_exited = result.already_exited.saturating_add(1);
        } else {
            result.failures = result.failures.saturating_add(1);
            log::debug!(
                "paneflow-host: process cleanup could not terminate pid={pid} (os_error={terminate_error:?})"
            );
        }
        return None;
    }

    result.terminate_requested = result.terminate_requested.saturating_add(1);
    Some(process)
}

#[cfg(windows)]
fn wait_for_windows_terminations(
    handles: Vec<WindowsTerminationHandle>,
    deadline: std::time::Instant,
    result: &mut WindowsProcessTreeTerminationResult,
) {
    use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    if handles.is_empty() {
        return;
    }

    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if windows_wait_timeout_ms(remaining).is_none() {
        result.deadline_exhausted = true;
        result.timed_out = result.timed_out.saturating_add(handles.len());
        return;
    }

    let mut pending = Vec::with_capacity(handles.len());
    for process in handles {
        match unsafe { WaitForSingleObject(process.handle, 0) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => pending.push(process),
            WAIT_FAILED => {
                let os_error = io::Error::last_os_error().raw_os_error();
                result.failures = result.failures.saturating_add(1);
                log::warn!(
                    "paneflow-host: process cleanup wait failed for pid={} (os_error={os_error:?})",
                    process.pid
                );
            }
            status => {
                result.failures = result.failures.saturating_add(1);
                log::warn!(
                    "paneflow-host: process cleanup wait returned status={status:#x} for pid={}",
                    process.pid
                );
            }
        }
    }

    let pending_count = pending.len();
    for (index, process) in pending.into_iter().enumerate() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(timeout_ms) = windows_wait_timeout_ms(remaining) else {
            result.deadline_exhausted = true;
            result.timed_out = result
                .timed_out
                .saturating_add(pending_count.saturating_sub(index));
            break;
        };

        match unsafe { WaitForSingleObject(process.handle, timeout_ms) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => {
                result.deadline_exhausted = true;
                result.timed_out = result
                    .timed_out
                    .saturating_add(pending_count.saturating_sub(index));
                break;
            }
            WAIT_FAILED => {
                let os_error = io::Error::last_os_error().raw_os_error();
                result.failures = result.failures.saturating_add(1);
                log::warn!(
                    "paneflow-host: process cleanup wait failed for pid={} (os_error={os_error:?})",
                    process.pid
                );
            }
            status => {
                result.failures = result.failures.saturating_add(1);
                log::warn!(
                    "paneflow-host: process cleanup wait returned status={status:#x} for pid={}",
                    process.pid
                );
            }
        }
    }
}

#[cfg(windows)]
pub fn terminate_windows_process_tree(
    root_pid: u32,
    deadline: std::time::Instant,
) -> WindowsProcessTreeTerminationResult {
    let mut result = WindowsProcessTreeTerminationResult::default();
    if root_pid == 0 {
        return result;
    }

    const KILL_PASSES: usize = 3;
    let mut targeted = std::collections::HashSet::new();
    let mut handles = Vec::new();
    for pass in 0..KILL_PASSES {
        if pass > 0 && std::time::Instant::now() >= deadline {
            result.deadline_exhausted = true;
            break;
        }

        let (entries, snapshot_failed) = match windows_process_entries() {
            Ok(entries) => (entries, false),
            Err(error) => {
                result.failures = result.failures.saturating_add(1);
                log::warn!(
                    "paneflow-host: process cleanup snapshot failed for root_pid={root_pid} (os_error={:?})",
                    error.raw_os_error()
                );
                (Vec::new(), true)
            }
        };
        let targets = windows_process_tree_targets(root_pid, &entries, pass == 0);
        let had_descendants = targets.iter().any(|pid| *pid != root_pid);
        for pid in targets {
            if !targeted.insert(pid) {
                continue;
            }
            result.targeted = result.targeted.saturating_add(1);
            if let Some(handle) = request_windows_pid_termination(pid, &mut result) {
                handles.push(handle);
            }
        }

        if pass > 0 && !snapshot_failed && !had_descendants {
            break;
        }
    }

    wait_for_windows_terminations(handles, deadline, &mut result);
    if result.failures != 0 || result.timed_out != 0 {
        log::warn!(
            "paneflow-host: process cleanup incomplete (root_pid={root_pid}, targeted={}, terminate_requested={}, already_exited={}, failures={}, timed_out={}, deadline_exhausted={})",
            result.targeted,
            result.terminate_requested,
            result.already_exited,
            result.failures,
            result.timed_out,
            result.deadline_exhausted
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

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
