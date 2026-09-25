use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    Prompt,
    Command,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawMark {
    pub kind: MarkKind,
}

#[derive(Debug, Clone, Copy)]
pub struct CommandMark {
    pub kind: MarkKind,
    pub abs_line: i64,
}

pub const MAX_MARKS: usize = 1_000;
pub type SharedMarkRing = Arc<Mutex<MarkRing>>;

#[derive(Default)]
pub struct MarkRing {
    marks: VecDeque<CommandMark>,
    prompt_starts: u64,
}

impl MarkRing {
    pub fn push(&mut self, mark: CommandMark) {
        if mark.kind == MarkKind::Prompt {
            self.prompt_starts = self.prompt_starts.wrapping_add(1);
        }
        if self.marks.len() == MAX_MARKS {
            self.marks.pop_front();
        }
        self.marks.push_back(mark);
    }

    pub fn prompt_start_seq(&self) -> u64 {
        self.prompt_starts
    }

    pub fn retain_at_or_below(&mut self, max_abs_line: i64) {
        self.marks.retain(|mark| mark.abs_line <= max_abs_line);
    }

    pub fn prompt_before(&self, abs_line: i64) -> Option<i64> {
        self.marks
            .iter()
            .rev()
            .filter(|mark| mark.kind == MarkKind::Prompt)
            .map(|mark| mark.abs_line)
            .find(|line| *line < abs_line)
    }

    pub fn prompt_after(&self, abs_line: i64) -> Option<i64> {
        self.marks
            .iter()
            .filter(|mark| mark.kind == MarkKind::Prompt)
            .map(|mark| mark.abs_line)
            .find(|line| *line > abs_line)
    }
}

const PAYLOAD_CAP: usize = 16;
const OSC_PREFIX: &[u8] = b"133;";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanState {
    Ground,
    Esc,
    Prefix(u8),
    Payload,
    SkipToTerminator,
    PayloadEsc,
    SkipEsc,
}

pub struct Osc133Scanner {
    state: ScanState,
    payload: [u8; PAYLOAD_CAP],
    payload_len: usize,
}

impl Default for Osc133Scanner {
    fn default() -> Self {
        Self {
            state: ScanState::Ground,
            payload: [0; PAYLOAD_CAP],
            payload_len: 0,
        }
    }
}

impl Osc133Scanner {
    pub fn feed(&mut self, bytes: &[u8], on_mark: &mut impl FnMut(RawMark)) {
        let mut index = 0;
        while index < bytes.len() {
            match self.state {
                ScanState::Ground => {
                    let Some(offset) = find_escape(&bytes[index..]) else {
                        return;
                    };
                    index += offset + 1;
                    self.state = ScanState::Esc;
                    continue;
                }
                ScanState::Esc => {
                    self.state = if bytes[index] == b']' {
                        ScanState::Prefix(0)
                    } else {
                        ScanState::Ground
                    };
                }
                ScanState::Prefix(matched) => {
                    let byte = bytes[index];
                    if byte == OSC_PREFIX[matched as usize] {
                        let next = matched + 1;
                        self.state = if next as usize == OSC_PREFIX.len() {
                            self.payload_len = 0;
                            ScanState::Payload
                        } else {
                            ScanState::Prefix(next)
                        };
                    } else if matches!(byte, 0x07 | 0x18 | 0x1a) {
                        self.state = ScanState::Ground;
                    } else if byte == 0x1b {
                        self.state = ScanState::SkipEsc;
                    } else {
                        self.state = ScanState::SkipToTerminator;
                    }
                }
                ScanState::Payload => match bytes[index] {
                    0x07 => {
                        self.emit(on_mark);
                        self.state = ScanState::Ground;
                    }
                    0x1b => self.state = ScanState::PayloadEsc,
                    0x18 | 0x1a => self.state = ScanState::Ground,
                    byte => {
                        if self.payload_len < PAYLOAD_CAP {
                            self.payload[self.payload_len] = byte;
                            self.payload_len += 1;
                        } else {
                            self.state = ScanState::SkipToTerminator;
                        }
                    }
                },
                ScanState::PayloadEsc => {
                    self.state = match bytes[index] {
                        b'\\' => {
                            self.emit(on_mark);
                            ScanState::Ground
                        }
                        0x1b => ScanState::Esc,
                        _ => ScanState::Ground,
                    };
                }
                ScanState::SkipToTerminator => match bytes[index] {
                    0x07 | 0x18 | 0x1a => self.state = ScanState::Ground,
                    0x1b => self.state = ScanState::SkipEsc,
                    _ => {}
                },
                ScanState::SkipEsc => {
                    self.state = match bytes[index] {
                        b'\\' => ScanState::Ground,
                        b']' => ScanState::Prefix(0),
                        0x1b => ScanState::Esc,
                        _ => ScanState::Ground,
                    };
                }
            }
            index += 1;
        }
    }

    fn emit(&mut self, on_mark: &mut impl FnMut(RawMark)) {
        if let Some(mark) = parse_payload(&self.payload[..self.payload_len]) {
            on_mark(mark);
        }
        self.payload_len = 0;
    }
}

fn find_escape(bytes: &[u8]) -> Option<usize> {
    const ESCAPES: u64 = u64::from_ne_bytes([0x1b; 8]);
    const LOW_BITS: u64 = u64::from_ne_bytes([0x01; 8]);
    const HIGH_BITS: u64 = u64::from_ne_bytes([0x80; 8]);

    let (chunks, remainder) = bytes.as_chunks::<8>();
    for (chunk_index, chunk) in chunks.iter().enumerate() {
        let word = u64::from_ne_bytes(*chunk);
        let candidates = word ^ ESCAPES;
        if candidates.wrapping_sub(LOW_BITS) & !candidates & HIGH_BITS != 0 {
            return chunk
                .iter()
                .position(|byte| *byte == 0x1b)
                .map(|offset| chunk_index * 8 + offset);
        }
    }

    let tail_start = bytes.len() - remainder.len();
    remainder
        .iter()
        .position(|byte| *byte == 0x1b)
        .map(|offset| tail_start + offset)
}

fn parse_payload(payload: &[u8]) -> Option<RawMark> {
    let kind = match payload.first()? {
        b'A' => MarkKind::Prompt,
        b'B' => MarkKind::Command,
        b'C' => MarkKind::Output,
        _ => return None,
    };
    Some(RawMark { kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<RawMark> {
        let mut scanner = Osc133Scanner::default();
        let mut marks = Vec::new();
        for chunk in chunks {
            scanner.feed(chunk, &mut |mark| marks.push(mark));
        }
        marks
    }

    #[test]
    fn recognizes_the_read_kinds_and_both_terminators() {
        let marks = scan(&[b"\x1b]133;A\x07\x1b]133;B\x1b\\\x1b]133;C\x07\x1b]133;D;7\x07"]);
        assert_eq!(
            marks,
            vec![
                RawMark {
                    kind: MarkKind::Prompt
                },
                RawMark {
                    kind: MarkKind::Command
                },
                RawMark {
                    kind: MarkKind::Output
                },
            ]
        );
    }

    #[test]
    fn accepts_every_chunk_boundary() {
        let sequence = b"\x1b]133;A;cl=m\x1b\\";
        for split in 1..sequence.len() {
            assert_eq!(
                scan(&[&sequence[..split], &sequence[split..]]),
                vec![RawMark {
                    kind: MarkKind::Prompt
                }],
                "split at {split}"
            );
        }
    }

    #[test]
    fn drops_hostile_payload_and_recovers() {
        let payload = vec![b'x'; 64 * 1024];
        let marks = scan(&[b"\x1b]133;D;", &payload, b"\x07\x1b]133;A\x07"]);
        assert_eq!(
            marks,
            vec![RawMark {
                kind: MarkKind::Prompt
            }]
        );
    }

    #[test]
    fn ring_is_bounded_and_navigable() {
        let mut ring = MarkRing::default();
        for line in 0..MAX_MARKS + 10 {
            ring.push(CommandMark {
                kind: MarkKind::Prompt,
                abs_line: line as i64,
            });
        }
        assert_eq!(ring.prompt_before(10), None);
        assert_eq!(ring.prompt_before(20), Some(19));
        assert_eq!(ring.prompt_after(20), Some(21));
    }

    #[test]
    fn escape_search_covers_word_boundaries_and_tail() {
        for offset in 0..24 {
            let mut bytes = vec![b'x'; 24];
            bytes[offset] = 0x1b;
            assert_eq!(find_escape(&bytes), Some(offset));
        }
        assert_eq!(find_escape(&[b'x'; 24]), None);
    }
}
