use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Default)]
pub(super) struct ClipboardGate {
    state: AtomicU8,
}

impl ClipboardGate {
    const FOCUSED: u8 = 1 << 0;
    const STORE_ALLOWED: u8 = 1 << 1;

    pub(super) fn set_focused(&self, focused: bool) {
        if focused {
            self.state.fetch_or(Self::FOCUSED, Ordering::AcqRel);
        } else {
            self.state.fetch_and(!Self::FOCUSED, Ordering::AcqRel);
        }
    }

    pub(super) fn set_policy(&self, store_allowed: bool) {
        let mut policy = 0;
        if store_allowed {
            policy |= Self::STORE_ALLOWED;
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

        gate.set_policy(true);
        assert!(!gate.allows_store());

        gate.set_focused(true);
        assert!(gate.allows_store());

        gate.set_policy(false);
        assert!(!gate.allows_store());

        gate.set_policy(true);
        gate.set_focused(false);
        assert!(!gate.allows_store());
    }
}
