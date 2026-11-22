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
        Err(_) => unreaped_start_time(pid),
    }
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
pub fn process_session(pid: u32) -> Option<u32> {
    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
    let session = unsafe { libc::getsid(pid) };
    u32::try_from(session).ok().filter(|session| *session > 0)
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
