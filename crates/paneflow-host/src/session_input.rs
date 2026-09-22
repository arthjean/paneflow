use std::time::{Duration, SystemTime};

use paneflow_config::schema::SessionGeneration;

pub const ESCAPE_SETTLE: Duration = Duration::from_millis(150);

const ESC: u8 = 0x1b;

const MAX_SEQUENCE_BYTES: usize = 128;

const MAX_PENDING_SIGNALS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSignal {
    Cancelled(SystemTime),
    Submitted(SystemTime),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturedSignal {
    pub generation: SessionGeneration,
    pub signal: InputSignal,
}

#[derive(Debug, Default)]
pub struct SessionInput {
    buffer: Vec<u8>,
    lone_escape_at: Option<SystemTime>,
    bracketed_paste: bool,
    generation: Option<SessionGeneration>,
    pending: Vec<CapturedSignal>,
}

impl SessionInput {
    pub fn observe(&mut self, bytes: &[u8], now: SystemTime, generation: SessionGeneration) {
        if self.generation != Some(generation) {
            self.clear();
            self.generation = Some(generation);
        }
        self.settle(now);
        self.buffer.extend_from_slice(bytes);
        self.consume(now);
    }

    pub fn settle(&mut self, now: SystemTime) {
        let Some(escape_at) = self.lone_escape_at else {
            return;
        };
        if now.duration_since(escape_at).unwrap_or_default() < ESCAPE_SETTLE {
            return;
        }
        self.lone_escape_at = None;
        self.buffer.clear();
        self.push(InputSignal::Cancelled(escape_at));
    }

    pub fn take_pending(&mut self) -> Vec<CapturedSignal> {
        std::mem::take(&mut self.pending)
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.lone_escape_at = None;
        self.pending.clear();
    }

    fn push(&mut self, signal: InputSignal) {
        let Some(generation) = self.generation else {
            return;
        };
        if self.pending.len() < MAX_PENDING_SIGNALS {
            self.pending.push(CapturedSignal { generation, signal });
        }
    }

    fn consume(&mut self, now: SystemTime) {
        let mut consumed = 0;
        while consumed < self.buffer.len() {
            let rest = &self.buffer[consumed..];
            if rest[0] != ESC {
                if !self.bracketed_paste && matches!(rest[0], b'\r' | b'\n') {
                    self.push(InputSignal::Submitted(now));
                }
                consumed += 1;
                continue;
            }
            match classify(rest) {
                Escape::Incomplete if rest.len() >= MAX_SEQUENCE_BYTES => consumed += 1,
                Escape::Incomplete => break,
                Escape::Sequence { length, effect } => {
                    consumed += length;
                    match effect {
                        Effect::None => {}
                        Effect::PasteStart => self.bracketed_paste = true,
                        Effect::PasteEnd => self.bracketed_paste = false,
                        Effect::Cancel => self.push(InputSignal::Cancelled(now)),
                    }
                }
            }
        }
        self.buffer.drain(..consumed);
        self.lone_escape_at = match self.buffer.as_slice() {
            [ESC] => self.lone_escape_at.or(Some(now)),
            _ => None,
        };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    None,
    PasteStart,
    PasteEnd,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    Incomplete,
    Sequence { length: usize, effect: Effect },
}

fn classify(bytes: &[u8]) -> Escape {
    let Some(introducer) = bytes.get(1) else {
        return Escape::Incomplete;
    };
    match introducer {
        b'[' => classify_control_sequence(bytes),
        b'O' => match bytes.len() {
            0..=2 => Escape::Incomplete,
            _ => Escape::Sequence {
                length: 3,
                effect: Effect::None,
            },
        },
        _ => Escape::Sequence {
            length: 2,
            effect: Effect::None,
        },
    }
}

fn classify_control_sequence(bytes: &[u8]) -> Escape {
    let Some(offset) = bytes[2..]
        .iter()
        .position(|byte| (0x40..=0x7e).contains(byte))
    else {
        return Escape::Incomplete;
    };
    let final_byte = bytes[2 + offset];
    let parameters = &bytes[2..2 + offset];
    Escape::Sequence {
        length: 3 + offset,
        effect: control_sequence_effect(parameters, final_byte),
    }
}

fn control_sequence_effect(parameters: &[u8], final_byte: u8) -> Effect {
    match (parameters, final_byte) {
        (b"200", b'~') => Effect::PasteStart,
        (b"201", b'~') => Effect::PasteEnd,
        (_, b'u') if kitty_escape_press(parameters) => Effect::Cancel,
        _ => Effect::None,
    }
}

fn kitty_escape_press(parameters: &[u8]) -> bool {
    let Ok(text) = str::from_utf8(parameters) else {
        return false;
    };
    let mut fields = text.split(';');
    let key = fields.next().unwrap_or_default();
    if key.split(':').next().unwrap_or_default() != "27" {
        return false;
    }
    let modifiers = fields.next().unwrap_or("1");
    let mut parts = modifiers.split(':');
    let mask = parts.next().unwrap_or("1");
    let event = parts.next().unwrap_or("1");
    matches!(mask, "" | "1") && event != "3"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    const GEN: SessionGeneration = SessionGeneration::FIRST;

    fn captured(signal: InputSignal) -> CapturedSignal {
        CapturedSignal {
            generation: GEN,
            signal,
        }
    }

    fn at(milliseconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(milliseconds)
    }

    #[test]
    fn a_bare_escape_settles_into_a_cancellation_only_after_the_quiet_window() {
        let mut input = SessionInput::default();
        input.observe(b"\x1b", at(1_000), GEN);
        assert!(input.take_pending().is_empty());

        input.settle(at(1_100));
        assert!(
            input.take_pending().is_empty(),
            "a fragmented sequence still has time to arrive"
        );

        input.settle(at(1_150));
        assert_eq!(
            input.take_pending(),
            vec![captured(InputSignal::Cancelled(at(1_000)))]
        );
        assert!(input.take_pending().is_empty());
    }

    #[test]
    fn an_escape_that_opens_a_sequence_is_never_a_cancellation() {
        for sequence in [
            &b"\x1b[A"[..],
            &b"\x1b[1;5A"[..],
            &b"\x1bOP"[..],
            &b"\x1ba"[..],
            &b"\x1b[27;1:3u"[..],
            &b"\x1b[27;5u"[..],
        ] {
            let mut input = SessionInput::default();
            input.observe(sequence, at(1_000), GEN);
            input.settle(at(2_000));
            assert!(
                input.take_pending().is_empty(),
                "{sequence:?} is not a bare escape"
            );
        }
    }

    #[test]
    fn a_sequence_split_across_two_writes_never_settles_as_an_escape() {
        let mut input = SessionInput::default();
        input.observe(b"\x1b", at(1_000), GEN);
        input.observe(b"[A", at(1_050), GEN);
        input.settle(at(3_000));
        assert!(input.take_pending().is_empty());
    }

    #[test]
    fn a_kitty_escape_press_cancels_and_its_release_does_not() {
        let mut input = SessionInput::default();
        input.observe(b"\x1b[27u", at(1_000), GEN);
        assert_eq!(
            input.take_pending(),
            vec![captured(InputSignal::Cancelled(at(1_000)))]
        );

        input.observe(b"\x1b[27;1u", at(1_200), GEN);
        assert_eq!(
            input.take_pending(),
            vec![captured(InputSignal::Cancelled(at(1_200)))]
        );

        input.observe(b"\x1b[27;1:3u", at(1_400), GEN);
        assert!(input.take_pending().is_empty());
    }

    #[test]
    fn a_submission_is_a_carriage_return_outside_bracketed_paste() {
        let mut input = SessionInput::default();
        input.observe(b"hello\r", at(1_000), GEN);
        assert_eq!(
            input.take_pending(),
            vec![captured(InputSignal::Submitted(at(1_000)))]
        );

        input.observe(b"\x1b[200~line\rmore\r\x1b[201~", at(2_000), GEN);
        assert!(
            input.take_pending().is_empty(),
            "a pasted newline is content, not a submission"
        );

        input.observe(b"\r", at(3_000), GEN);
        assert_eq!(
            input.take_pending(),
            vec![captured(InputSignal::Submitted(at(3_000)))]
        );
    }

    #[test]
    fn an_escape_then_an_enter_records_both_in_order() {
        let mut input = SessionInput::default();
        input.observe(b"\x1b", at(1_000), GEN);
        input.settle(at(1_200));
        input.observe(b"retry\r", at(1_400), GEN);
        assert_eq!(
            input.take_pending(),
            vec![
                captured(InputSignal::Cancelled(at(1_000))),
                captured(InputSignal::Submitted(at(1_400))),
            ]
        );
    }

    #[test]
    fn an_escape_followed_by_a_late_key_settles_before_the_key_is_read() {
        let mut input = SessionInput::default();
        input.observe(b"\x1b", at(1_000), GEN);
        input.observe(b"a", at(1_400), GEN);
        assert_eq!(
            input.take_pending(),
            vec![captured(InputSignal::Cancelled(at(1_000)))]
        );
    }

    #[test]
    fn an_unterminated_sequence_is_dropped_instead_of_growing_without_bound() {
        let mut input = SessionInput::default();
        input.observe(b"\x1b[", at(1_000), GEN);
        input.observe(&vec![b'0'; MAX_SEQUENCE_BYTES * 2], at(1_050), GEN);
        input.observe(b"\r", at(1_100), GEN);
        assert_eq!(
            input.take_pending(),
            vec![captured(InputSignal::Submitted(at(1_100)))]
        );
        assert!(input.buffer.len() < MAX_SEQUENCE_BYTES);
    }
}
