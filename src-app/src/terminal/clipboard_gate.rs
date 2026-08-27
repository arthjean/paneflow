//! OSC 52 clipboard gate.
//!
//! The terminal engine raises clipboard requests from whatever is running in
//! the PTY. `ClipboardGate` is the policy checked at that source: a request is
//! honored only while the pane is focused and the configured OSC 52 mode
//! allows the direction being requested.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Default)]
pub(super) struct ClipboardGate {
    state: AtomicU8,
}

impl ClipboardGate {
    const FOCUSED: u8 = 1 << 0;
    const STORE_ALLOWED: u8 = 1 << 1;
    const LOAD_ALLOWED: u8 = 1 << 2;

    pub(super) fn set_focused(&self, focused: bool) {
        if focused {
            self.state.fetch_or(Self::FOCUSED, Ordering::AcqRel);
        } else {
            self.state.fetch_and(!Self::FOCUSED, Ordering::AcqRel);
        }
    }

    pub(super) fn set_policy(&self, store_allowed: bool, load_allowed: bool) {
        let mut policy = 0;
        if store_allowed {
            policy |= Self::STORE_ALLOWED;
        }
        if load_allowed {
            policy |= Self::LOAD_ALLOWED;
        }
        let _ = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some((state & Self::FOCUSED) | policy)
            });
    }

    pub(super) fn allows_store(&self) -> bool {
        let required = Self::FOCUSED | Self::STORE_ALLOWED;
        self.state.load(Ordering::Acquire) & required == required
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_stores_need_focus_and_policy() {
        let gate = ClipboardGate::default();
        assert!(!gate.allows_store());

        gate.set_policy(true, false);
        assert!(!gate.allows_store());

        gate.set_focused(true);
        assert!(gate.allows_store());

        gate.set_policy(false, false);
        assert!(!gate.allows_store());

        gate.set_policy(true, false);
        gate.set_focused(false);
        assert!(!gate.allows_store());
    }
}
