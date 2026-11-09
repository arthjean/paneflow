use std::time::Duration;

#[allow(dead_code, reason = "each example reads only the fields it reports")]
pub(crate) struct ChildRun {
    pub(crate) success: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) cpu: Duration,
    pub(crate) peak_bytes: u64,
}

#[cfg(unix)]
mod imp {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use super::ChildRun;

    #[cfg(target_os = "macos")]
    const MAXRSS_UNIT: u64 = 1;

    #[cfg(not(target_os = "macos"))]
    const MAXRSS_UNIT: u64 = 1024;

    fn timeval(value: libc::timeval) -> Duration {
        Duration::from_secs(value.tv_sec as u64) + Duration::from_micros(value.tv_usec as u64)
    }

    fn usage(who: libc::c_int) -> libc::rusage {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(who, &mut usage) };
        usage
    }

    pub(crate) fn self_cpu() -> Duration {
        let usage = usage(libc::RUSAGE_SELF);
        timeval(usage.ru_utime) + timeval(usage.ru_stime)
    }

    pub(crate) fn self_peak_bytes() -> u64 {
        usage(libc::RUSAGE_SELF).ru_maxrss as u64 * MAXRSS_UNIT
    }

    pub(crate) fn run(mut command: Command) -> std::io::Result<ChildRun> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(mut pipe) = child.stdout.take() {
            pipe.read_to_end(&mut stdout)?;
        }
        if let Some(mut pipe) = child.stderr.take() {
            pipe.read_to_end(&mut stderr)?;
        }
        let mut status = 0;
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        let pid = child.id() as libc::pid_t;
        let waited = unsafe { libc::wait4(pid, &mut status, 0, &mut usage) };
        if waited != pid {
            return Err(std::io::Error::last_os_error());
        }
        Ok(ChildRun {
            success: libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            stdout,
            stderr,
            cpu: timeval(usage.ru_utime) + timeval(usage.ru_stime),
            peak_bytes: usage.ru_maxrss as u64 * MAXRSS_UNIT,
        })
    }
}

#[cfg(windows)]
mod imp {
    use std::io::Read;
    use std::os::windows::io::AsRawHandle;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{FILETIME, HANDLE};
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    use super::ChildRun;

    const FILETIME_TICK_NANOS: u64 = 100;

    fn ticks(time: FILETIME) -> u64 {
        (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
    }

    fn cpu(handle: HANDLE) -> Duration {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
        Duration::from_nanos((ticks(kernel) + ticks(user)) * FILETIME_TICK_NANOS)
    }

    fn peak_bytes(handle: HANDLE) -> u64 {
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        counters.cb = size;
        unsafe { GetProcessMemoryInfo(handle, &mut counters, size) };
        counters.PeakWorkingSetSize as u64
    }

    pub(crate) fn self_cpu() -> Duration {
        cpu(unsafe { GetCurrentProcess() })
    }

    pub(crate) fn self_peak_bytes() -> u64 {
        peak_bytes(unsafe { GetCurrentProcess() })
    }

    pub(crate) fn run(mut command: Command) -> std::io::Result<ChildRun> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(mut pipe) = child.stdout.take() {
            pipe.read_to_end(&mut stdout)?;
        }
        if let Some(mut pipe) = child.stderr.take() {
            pipe.read_to_end(&mut stderr)?;
        }
        let status = child.wait()?;
        let handle = child.as_raw_handle() as HANDLE;
        Ok(ChildRun {
            success: status.success(),
            stdout,
            stderr,
            cpu: cpu(handle),
            peak_bytes: peak_bytes(handle),
        })
    }
}

pub(crate) use imp::{run, self_cpu, self_peak_bytes};
