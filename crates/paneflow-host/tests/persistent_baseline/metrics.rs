use super::*;

#[derive(Debug, Clone)]
pub(super) struct ThreadCpu {
    name: String,
    cpu_ns: u64,
}

#[allow(dead_code)]
pub(super) enum Attribution {
    Exact(Vec<ThreadCpu>),
    Prefix15(Vec<ThreadCpu>),
    Pending(String),
}

impl Attribution {
    fn samples(&self) -> Option<&[ThreadCpu]> {
        match self {
            Self::Exact(samples) | Self::Prefix15(samples) => Some(samples),
            Self::Pending(_) => None,
        }
    }

    fn quality(&self) -> Value {
        match self {
            Self::Exact(_) => json!("exact thread names"),
            Self::Prefix15(_) => {
                json!("thread names truncated to 15 bytes by procfs comm; prefixes may merge")
            }
            Self::Pending(reason) => json!({"pending": reason}),
        }
    }
}

#[cfg(windows)]
pub(super) fn thread_cpu(pid: u32) -> Attribution {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{
        GetThreadDescription, GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
    };

    fn filetime_ns(time: FILETIME) -> u64 {
        ((u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)) * 100
    }

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Attribution::Pending("CreateToolhelp32Snapshot failed".to_string());
    }
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut samples = Vec::new();
    let mut unavailable = 0usize;
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            let handle =
                unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, entry.th32ThreadID) };
            if !handle.is_null() {
                let mut creation: FILETIME = unsafe { std::mem::zeroed() };
                let mut exit: FILETIME = unsafe { std::mem::zeroed() };
                let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
                let mut user: FILETIME = unsafe { std::mem::zeroed() };
                let timed = unsafe {
                    GetThreadTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user)
                };
                let mut description: *mut u16 = std::ptr::null_mut();
                let named = unsafe { GetThreadDescription(handle, &mut description) };
                let name = if named >= 0 && !description.is_null() {
                    let text =
                        unsafe { widestring::U16CStr::from_ptr_str(description) }.to_string_lossy();
                    unsafe {
                        LocalFree(description.cast());
                    }
                    text
                } else {
                    String::new()
                };
                unsafe {
                    CloseHandle(handle);
                }
                if timed != 0 {
                    samples.push(ThreadCpu {
                        name: if name.is_empty() {
                            format!("unnamed-{}", entry.th32ThreadID)
                        } else {
                            name
                        },
                        cpu_ns: filetime_ns(kernel) + filetime_ns(user),
                    });
                } else {
                    unavailable += 1;
                }
            } else {
                unavailable += 1;
            }
        }
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe {
        CloseHandle(snapshot);
    }
    if unavailable > 0 || samples.is_empty() {
        Attribution::Pending(format!(
            "thread CPU incomplete: {} measured, {unavailable} unavailable",
            samples.len()
        ))
    } else {
        Attribution::Exact(samples)
    }
}

#[cfg(target_os = "linux")]
pub(super) fn thread_cpu(pid: u32) -> Attribution {
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks_per_second <= 0 {
        return Attribution::Pending("sysconf(_SC_CLK_TCK) failed".to_string());
    }
    let ns_per_tick = 1_000_000_000u64 / ticks_per_second as u64;
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return Attribution::Pending(format!("/proc/{pid}/task is not readable"));
    };
    let mut samples = Vec::new();
    for task in tasks.flatten() {
        let dir = task.path();
        let name = std::fs::read_to_string(dir.join("comm"))
            .map(|comm| comm.trim().to_string())
            .unwrap_or_default();
        let Ok(stat) = std::fs::read_to_string(dir.join("stat")) else {
            continue;
        };
        let Some(after_name) = stat.rfind(')').map(|index| &stat[index + 1..]) else {
            continue;
        };
        let fields: Vec<&str> = after_name.split_whitespace().collect();
        let utime: u64 = fields.get(11).and_then(|v| v.parse().ok()).unwrap_or(0);
        let stime: u64 = fields.get(12).and_then(|v| v.parse().ok()).unwrap_or(0);
        samples.push(ThreadCpu {
            name,
            cpu_ns: (utime + stime) * ns_per_tick,
        });
    }
    Attribution::Prefix15(samples)
}

#[cfg(target_os = "macos")]
const PROC_PIDLISTTHREADS: libc::c_int = 6;

#[cfg(target_os = "macos")]
fn proc_list<T: Copy>(pid: libc::c_int, flavor: libc::c_int, empty: T) -> Option<Vec<T>> {
    let mut entries = vec![empty; 256];
    loop {
        let capacity = libc::c_int::try_from(entries.len() * std::mem::size_of::<T>()).ok()?;
        let returned = unsafe {
            libc::proc_pidinfo(
                pid,
                flavor,
                0,
                entries.as_mut_ptr().cast::<libc::c_void>(),
                capacity,
            )
        };
        if returned <= 0 {
            return None;
        }
        let count = returned as usize / std::mem::size_of::<T>();
        if count < entries.len() {
            entries.truncate(count);
            return Some(entries);
        }
        entries.resize(entries.len() * 2, empty);
    }
}

#[cfg(target_os = "macos")]
fn task_info(pid: u32) -> Option<libc::proc_taskinfo> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    let returned = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast::<libc::c_void>(),
            size,
        )
    };
    (returned == size).then(|| unsafe { info.assume_init() })
}

#[cfg(target_os = "macos")]
pub(super) fn thread_cpu(pid: u32) -> Attribution {
    let Some(handles) = libc::c_int::try_from(pid)
        .ok()
        .and_then(|pid| proc_list(pid, PROC_PIDLISTTHREADS, 0u64))
    else {
        return Attribution::Pending(format!(
            "proc_pidinfo(PROC_PIDLISTTHREADS) failed for {pid}"
        ));
    };
    let mut samples = Vec::new();
    let mut unavailable = 0usize;
    for handle in handles {
        let mut info = std::mem::MaybeUninit::<libc::proc_threadinfo>::uninit();
        let size = std::mem::size_of::<libc::proc_threadinfo>() as libc::c_int;
        let returned = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTHREADINFO,
                handle,
                info.as_mut_ptr().cast::<libc::c_void>(),
                size,
            )
        };
        if returned != size {
            unavailable += 1;
            continue;
        }
        let info = unsafe { info.assume_init() };
        let name: Vec<u8> = info
            .pth_name
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect();
        samples.push(ThreadCpu {
            name: String::from_utf8_lossy(&name).into_owned(),
            cpu_ns: info.pth_user_time + info.pth_system_time,
        });
    }
    if unavailable > 0 || samples.is_empty() {
        Attribution::Pending(format!(
            "thread CPU incomplete: {} measured, {unavailable} unavailable",
            samples.len()
        ))
    } else {
        Attribution::Exact(samples)
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(super) fn thread_cpu(_pid: u32) -> Attribution {
    Attribution::Pending(
        "per-thread CPU attribution on this platform needs proc_pidinfo(PROC_PIDTHREADINFO); not implemented"
            .to_string(),
    )
}

#[cfg(windows)]
pub(super) fn resident_bytes(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let queried = unsafe { K32GetProcessMemoryInfo(handle, &mut counters, counters.cb) };
    unsafe {
        CloseHandle(handle);
    }
    (queried != 0).then_some(counters.WorkingSetSize as u64)
}

#[cfg(target_os = "linux")]
pub(super) fn resident_bytes(pid: u32) -> Option<u64> {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|kib| kib.parse::<u64>().ok())
        .map(|kib| kib * 1024)
}

#[cfg(target_os = "macos")]
pub(super) fn resident_bytes(pid: u32) -> Option<u64> {
    task_info(pid).map(|info| info.pti_resident_size)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(super) fn resident_bytes(_pid: u32) -> Option<u64> {
    None
}

#[cfg(windows)]
pub(super) fn process_counters(pid: u32) -> (Option<u64>, Option<u64>) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessHandleCount, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let handles = {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            None
        } else {
            let mut count = 0u32;
            let queried = unsafe { GetProcessHandleCount(handle, &mut count) };
            unsafe {
                CloseHandle(handle);
            }
            (queried != 0).then_some(u64::from(count))
        }
    };
    let threads = {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            None
        } else {
            let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut count = 0u64;
            let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    count += 1;
                }
                more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
            }
            unsafe {
                CloseHandle(snapshot);
            }
            Some(count)
        }
    };
    (threads, handles)
}

#[cfg(target_os = "linux")]
pub(super) fn process_counters(pid: u32) -> (Option<u64>, Option<u64>) {
    let threads = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("Threads:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|count| count.parse::<u64>().ok())
        });
    let fds = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .ok()
        .map(|entries| entries.count() as u64);
    (threads, fds)
}

#[cfg(target_os = "macos")]
pub(super) fn process_counters(pid: u32) -> (Option<u64>, Option<u64>) {
    let threads = task_info(pid).and_then(|info| u64::try_from(info.pti_threadnum).ok());
    let empty = libc::proc_fdinfo {
        proc_fd: 0,
        proc_fdtype: 0,
    };
    let fds = libc::c_int::try_from(pid)
        .ok()
        .and_then(|pid| proc_list(pid, libc::PROC_PIDLISTFDS, empty))
        .map(|entries| entries.len() as u64);
    (threads, fds)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(super) fn process_counters(_pid: u32) -> (Option<u64>, Option<u64>) {
    (None, None)
}

fn counters_json(pid: u32) -> Value {
    let (threads, handles) = process_counters(pid);
    json!({
        "threads": threads,
        "handles_or_fds": handles,
        "note": if threads.is_none() || handles.is_none() { "unavailable on this platform sampler; never zero" } else { "Windows handles, or Linux and macOS descriptors, plus thread count" },
    })
}

fn role_of(thread_name: &str) -> &'static str {
    let roles = [
        ("paneflow-host-session", "host.session"),
        ("paneflow-host-pty-reader", "host.pty_reader"),
        ("paneflow-host-pty-writer", "host.pty_writer"),
        ("paneflow-host-viewport", "host.viewport_scan"),
        ("paneflow-host-cancellation", "host.cancellation_scan"),
        ("paneflow-host-conn", "host.ipc_connection"),
        ("paneflow-host-accept", "host.ipc_accept"),
        ("paneflow-host-launch-owner", "host.launch_owner"),
        ("paneflow-serve-core", "worker.core_link"),
        ("paneflow-ghostty-runtime", "desktop.runtime"),
        ("paneflow-ghostty-attached", "desktop.attached_runtime"),
        ("paneflow-ghostty-display", "desktop.display"),
        ("paneflow-ghostty-follower", "desktop.follower"),
        ("baseline-follower", "headless.follower"),
    ];
    if let Some((_, role)) = roles
        .iter()
        .find(|(prefix, _)| thread_name.starts_with(prefix))
    {
        return role;
    }
    let mut truncated = roles
        .iter()
        .filter(|(prefix, _)| prefix.starts_with(thread_name) && thread_name.len() >= 15);
    if let Some((_, role)) = truncated.next() {
        return if truncated.next().is_some() {
            "merged_truncated_names"
        } else {
            role
        };
    }
    if thread_name == "main" || thread_name.is_empty() {
        return "main";
    }
    "other"
}

fn attribute(before: &[ThreadCpu], after: &[ThreadCpu], window: Duration) -> Value {
    let mut by_role: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut raw: BTreeMap<String, u64> = BTreeMap::new();
    let aggregate = |samples: &[ThreadCpu]| {
        let mut names = BTreeMap::<String, u64>::new();
        for sample in samples {
            *names.entry(sample.name.clone()).or_default() += sample.cpu_ns;
        }
        names
    };
    let baseline = aggregate(before);
    for (name, cpu_ns) in aggregate(after) {
        let started = baseline.get(&name).copied().unwrap_or(0);
        let delta = cpu_ns.saturating_sub(started);
        *by_role.entry(role_of(&name)).or_default() += delta;
        raw.insert(name, delta);
    }
    let window_ns = window.as_nanos() as f64;
    let percent = |ns: u64| (ns as f64 / window_ns) * 100.0;
    json!({
        "by_role_cpu_percent": by_role.iter().map(|(role, ns)| (role.to_string(), percent(*ns))).collect::<BTreeMap<_, _>>(),
        "by_thread_cpu_ns": raw,
        "total_cpu_percent": percent(by_role.values().sum()),
    })
}

pub(super) fn process_sample(
    pid: u32,
    before: &Attribution,
    after: &Attribution,
    window: Duration,
) -> Value {
    let cpu = match (before.samples(), after.samples()) {
        (Some(before), Some(after)) => attribute(before, after, window),
        _ => json!({"pending": after.quality()}),
    };
    json!({
        "pid": pid,
        "resident_bytes": resident_bytes(pid),
        "counters": counters_json(pid),
        "cpu": cpu,
        "attribution": after.quality(),
    })
}
