use std::time::Duration;

use gpui::{Entity, Global};

pub const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);

pub struct BlinkPhase {
    pub visible: bool,
}

impl Default for BlinkPhase {
    fn default() -> Self {
        Self { visible: true }
    }
}

pub struct BlinkPhaseGlobal(pub Entity<BlinkPhase>);

impl Global for BlinkPhaseGlobal {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_visible() {
        assert!(BlinkPhase::default().visible);
    }
}
