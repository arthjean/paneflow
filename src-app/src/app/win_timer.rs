#[cfg(windows)]
#[must_use = "dropping the guard restores the default 15.6 ms timer tick"]
pub fn high_resolution_timer() -> TimerResolutionGuard {
    let granted = unsafe { windows_sys::Win32::Media::timeBeginPeriod(1) };
    if granted != 0 {
        log::warn!(
            target: "paneflow::win_timer",
            "timeBeginPeriod(1) was refused ({granted}); timers stay on the ~15.6 ms tick"
        );
    }
    TimerResolutionGuard {
        active: granted == 0,
    }
}

#[cfg(windows)]
pub struct TimerResolutionGuard {
    active: bool,
}

#[cfg(windows)]
impl Drop for TimerResolutionGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        unsafe { windows_sys::Win32::Media::timeEndPeriod(1) };
    }
}

#[cfg(not(windows))]
pub struct TimerResolutionGuard;

#[cfg(not(windows))]
pub fn high_resolution_timer() -> TimerResolutionGuard {
    TimerResolutionGuard
}
