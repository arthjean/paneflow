//! Process-lifetime Windows timer resolution.
//!
//! Windows delivers timer expirations on the system clock tick, 15.625 ms by
//! default, and since Windows 10 2004 that default is per-process: a process
//! that never asks stays on 15.6 ms no matter what else is running. Every
//! short timeout in the terminal loop rounds up to it. `smol::Timer` resolves
//! through `polling`, which calls `GetQueuedCompletionStatusEx` with a
//! millisecond timeout, so the 4 ms wakeup batch window in
//! `terminal/view.rs` really waits ~15.6 ms. `Condvar::wait_timeout` behind
//! `RuntimeMailbox::recv_timeout` rounds the same way, which stretches
//! `RUNTIME_IDLE_TICK` and `MIN_PUBLISH_INTERVAL` past their budgets.
//!
//! GPUI raises the resolution too, but only for the duration of a blocking
//! `block_with_timeout` (`gpui::platform_scheduler`), which does not cover the
//! terminal loops. Holding it for the process lifetime is what browsers and
//! media players do, and is the documented remedy.
//!
//! The cost is real but small and bounded: a higher tick rate raises idle
//! power draw. Paneflow is a foreground developer tool whose whole value is
//! keystroke latency, so the trade is worth making while a window is open.

/// Raise the process timer resolution to 1 ms until the returned guard drops.
///
/// The guard is deliberately `#[must_use]`: dropping it immediately restores
/// the 15.6 ms tick and undoes the point of the call.
#[cfg(windows)]
#[must_use = "dropping the guard restores the default 15.6 ms timer tick"]
pub fn high_resolution_timer() -> TimerResolutionGuard {
    // SAFETY: `timeBeginPeriod` takes a plain millisecond count and is safe to
    // call from any thread. 1 ms is the value every supported Windows release
    // accepts; a rejected period returns TIMERR_NOCANDO, which the guard's
    // matching `timeEndPeriod` tolerates.
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

/// Restores the default timer tick on drop.
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
        // SAFETY: pairs with the `timeBeginPeriod(1)` that succeeded above.
        unsafe { windows_sys::Win32::Media::timeEndPeriod(1) };
    }
}

/// No-op outside Windows: Linux and macOS already deliver timer expirations at
/// the resolution the caller asked for.
#[cfg(not(windows))]
pub struct TimerResolutionGuard;

#[cfg(not(windows))]
pub fn high_resolution_timer() -> TimerResolutionGuard {
    TimerResolutionGuard
}
