#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub(crate) use linux::{
    BrowserFrame, browser_intake, browser_painted, browser_prepare, cpu_finished, cpu_started,
    enabled, input_key, input_text, now_ns, paint_failed, painted,
};

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub(crate) use windows::*;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) fn enabled() -> bool {
    false
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) fn now_ns() -> u64 {
    0
}
