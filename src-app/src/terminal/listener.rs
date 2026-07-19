//! Alacritty event-listener bridge + grid dimensions adapter.
//!
//! `ZedListener` carries `AlacEvent`s from the VTE thread to the GPUI main
//! thread via a `futures::mpsc` channel. `SpikeTermSize` adapts our
//! `(columns, screen_lines)` pair to alacritty's `Dimensions` trait.
//!
//! Extracted from `terminal.rs` per US-011 of the src-app refactor PRD.

use alacritty_terminal::event::{Event as AlacEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use futures::channel::mpsc::UnboundedSender;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::limits::MAX_OSC52_BYTES;

pub struct SpikeTermSize {
    pub columns: usize,
    pub screen_lines: usize,
}

impl Dimensions for SpikeTermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

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

    fn allows_load(&self) -> bool {
        let required = Self::FOCUSED | Self::LOAD_ALLOWED;
        self.state.load(Ordering::Acquire) & required == required
    }
}

#[derive(Clone)]
pub struct ZedListener {
    events: UnboundedSender<AlacEvent>,
    clipboard_gate: Option<Arc<ClipboardGate>>,
}

impl ZedListener {
    #[cfg(test)]
    pub(super) fn new(events: UnboundedSender<AlacEvent>) -> Self {
        Self {
            events,
            clipboard_gate: None,
        }
    }

    pub(super) fn with_clipboard_gate(
        events: UnboundedSender<AlacEvent>,
        clipboard_gate: Arc<ClipboardGate>,
    ) -> Self {
        Self {
            events,
            clipboard_gate: Some(clipboard_gate),
        }
    }
}

impl EventListener for ZedListener {
    fn send_event(&self, event: AlacEvent) {
        if let Some(gate) = &self.clipboard_gate {
            match &event {
                AlacEvent::ClipboardStore(_, text) if text.len() > MAX_OSC52_BYTES => return,
                AlacEvent::ClipboardStore(..) if !gate.allows_store() => return,
                AlacEvent::ClipboardLoad(..) if !gate.allows_load() => return,
                _ => {}
            }
        }
        let _ = self.events.unbounded_send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::ClipboardType;
    use futures::channel::mpsc::unbounded;

    #[test]
    fn clipboard_events_are_filtered_at_the_vte_source() {
        let (events_tx, mut events_rx) = unbounded();
        let gate = Arc::new(ClipboardGate::default());
        let listener = ZedListener::with_clipboard_gate(events_tx, gate.clone());

        listener.send_event(AlacEvent::ClipboardStore(
            ClipboardType::Clipboard,
            "unfocused".into(),
        ));
        assert!(events_rx.try_recv().is_err());

        gate.set_policy(true, false);
        gate.set_focused(true);
        listener.send_event(AlacEvent::ClipboardStore(
            ClipboardType::Clipboard,
            "focused".into(),
        ));
        assert!(matches!(
            events_rx.try_recv(),
            Ok(AlacEvent::ClipboardStore(_, text)) if text == "focused"
        ));

        listener.send_event(AlacEvent::ClipboardStore(
            ClipboardType::Clipboard,
            "x".repeat(MAX_OSC52_BYTES + 1),
        ));
        assert!(events_rx.try_recv().is_err());

        gate.set_focused(false);
        listener.send_event(AlacEvent::ClipboardStore(
            ClipboardType::Clipboard,
            "lost-focus".into(),
        ));
        assert!(events_rx.try_recv().is_err());
    }
}
