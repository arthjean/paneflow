#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub(crate) use linux::{
    BrowserFrame, browser_intake, browser_painted, browser_prepare, cpu_finished, cpu_started,
    enabled, input_key, input_text, now_ns, paint_failed, painted,
};

#[cfg(not(target_os = "linux"))]
pub(crate) fn enabled() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn now_ns() -> u64 {
    0
}
