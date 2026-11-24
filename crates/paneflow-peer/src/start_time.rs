#[cfg(windows)]
pub fn process_start_time(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    if pid == 0 {
        return None;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let created = crate::windows::creation_time(handle);
    unsafe { CloseHandle(handle) };
    created
}

#[cfg(target_os = "linux")]
pub fn process_start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    crate::linux::parse_stat(&stat).map(|stat| stat.started_at)
}

#[cfg(target_os = "macos")]
pub fn process_start_time(pid: u32) -> Option<u64> {
    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
    match crate::macos::bsd_info(pid) {
        Ok(info) => Some(crate::macos::started_at(&info)),
        Err(_) => crate::macos::kernel_process(pid)
            .ok()
            .map(|process| process.started_at),
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn process_start_time(_pid: u32) -> Option<u64> {
    None
}

#[cfg(unix)]
pub fn process_session(pid: u32) -> Option<u32> {
    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
    let session = unsafe { libc::getsid(pid) };
    u32::try_from(session).ok().filter(|session| *session > 0)
}

#[cfg(unix)]
pub fn parent_process() -> Option<(u32, u64)> {
    let parent = u32::try_from(unsafe { libc::getppid() })
        .ok()
        .filter(|pid| *pid > 0)?;
    Some((parent, process_start_time(parent)?))
}

#[cfg(windows)]
pub fn parent_process() -> Option<(u32, u64)> {
    crate::windows::parent_process()
}

#[cfg(not(any(unix, windows)))]
pub fn parent_process() -> Option<(u32, u64)> {
    None
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_a_stable_start_time_and_a_session() {
        let pid = std::process::id();
        let first = process_start_time(pid).expect("our own start time");
        assert_eq!(process_start_time(pid), Some(first));
        assert!(process_session(pid).is_some());
    }

    #[test]
    fn the_parent_of_this_process_is_read_with_a_start_time_no_later_than_ours() {
        let (parent, parent_started) = parent_process().expect("a test process has a parent");
        assert_eq!(parent, u32::try_from(unsafe { libc::getppid() }).unwrap());
        let ours = process_start_time(std::process::id()).unwrap();
        assert!(parent_started <= ours, "{parent_started} > {ours}");
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
}
