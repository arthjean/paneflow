use base64::Engine as _;

const MAX_CLIPBOARD_BYTES: usize = 100 * 1024;
const MAX_ENCODED_BYTES: usize = MAX_CLIPBOARD_BYTES.div_ceil(3) * 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Osc,
    Osc5,
    Osc52,
    Selection,
    Payload,
    PayloadEscape,
    Discard,
    DiscardEscape,
}

#[derive(Default)]
pub(crate) struct Osc52Scanner {
    state: State,
    utf8_continuations: u8,
    payload: Vec<u8>,
}

impl Osc52Scanner {
    pub(crate) fn feed(&mut self, bytes: &[u8], emit: &mut impl FnMut(String)) {
        for &byte in bytes {
            if self.state == State::Ground && self.consume_utf8_text_byte(byte) {
                continue;
            }
            self.advance(byte, emit);
        }
    }

    fn consume_utf8_text_byte(&mut self, byte: u8) -> bool {
        if self.utf8_continuations > 0 {
            if byte & 0b1100_0000 == 0b1000_0000 {
                self.utf8_continuations -= 1;
                return true;
            }
            self.utf8_continuations = 0;
        }

        self.utf8_continuations = match byte {
            0xc2..=0xdf => 1,
            0xe0..=0xef => 2,
            0xf0..=0xf4 => 3,
            _ => 0,
        };
        self.utf8_continuations > 0
    }

    fn advance(&mut self, byte: u8, emit: &mut impl FnMut(String)) {
        self.state = match self.state {
            State::Ground => match byte {
                b'\x1b' => State::Escape,
                b'\x9d' => State::Osc,
                _ => State::Ground,
            },
            State::Escape => match byte {
                b']' => State::Osc,
                b'\x1b' => State::Escape,
                _ => State::Ground,
            },
            State::Osc => match byte {
                b'5' => State::Osc5,
                b'\x1b' => State::Escape,
                _ => State::Discard,
            },
            State::Osc5 => match byte {
                b'2' => State::Osc52,
                b'\x1b' => State::Escape,
                _ => State::Discard,
            },
            State::Osc52 => match byte {
                b';' => State::Selection,
                b'\x1b' => State::Escape,
                _ => State::Discard,
            },
            State::Selection => match byte {
                b'c' | b'p' | b's' => State::Payload,
                _ => State::Discard,
            },
            State::Payload if self.payload.is_empty() && byte == b';' => State::Payload,
            State::Payload => match byte {
                b'\x07' | b'\x9c' => {
                    self.finish(emit);
                    State::Ground
                }
                b'\x18' | b'\x1a' => {
                    self.reset_payload();
                    State::Ground
                }
                b'\x1b' => State::PayloadEscape,
                _ if self.payload.len() < MAX_ENCODED_BYTES => {
                    self.payload.push(byte);
                    State::Payload
                }
                _ => {
                    self.reset_payload();
                    State::Discard
                }
            },
            State::PayloadEscape if byte == b'\\' => {
                self.finish(emit);
                State::Ground
            }
            State::PayloadEscape => {
                self.reset_payload();
                State::Discard
            }
            State::Discard => match byte {
                b'\x07' | b'\x9c' | b'\x18' | b'\x1a' => State::Ground,
                b'\x1b' => State::DiscardEscape,
                _ => State::Discard,
            },
            State::DiscardEscape => match byte {
                b'\\' => State::Ground,
                b']' => State::Osc,
                b'\x1b' => State::Escape,
                _ => State::Ground,
            },
        };
    }

    fn finish(&mut self, emit: &mut impl FnMut(String)) {
        if self.payload != b"?"
            && let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&self.payload)
            && decoded.len() <= MAX_CLIPBOARD_BYTES
            && let Ok(text) = String::from_utf8(decoded)
        {
            emit(text);
        }
        self.reset_payload();
    }

    fn reset_payload(&mut self) {
        self.payload.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<String> {
        let mut scanner = Osc52Scanner::default();
        let mut output = Vec::new();
        for chunk in chunks {
            scanner.feed(chunk, &mut |text| output.push(text));
        }
        output
    }

    #[test]
    fn clipboard_store_survives_arbitrary_chunk_boundaries() {
        assert_eq!(
            scan(&[b"\x1b]", b"52;c;c3lu", b"dGhldGlj\x1b", b"\\"]),
            ["synthetic"]
        );
    }

    #[test]
    fn queries_and_malformed_payloads_emit_nothing() {
        assert!(scan(&[b"\x1b]52;c;?\x07\x1b]52;c;%%%\x07"]).is_empty());
    }

    #[test]
    fn foreign_osc_cannot_smuggle_clipboard_and_scanner_recovers() {
        assert_eq!(
            scan(&[b"\x1b]0;title52;c;b3duZWQ=\x07\x1b]52;c;b2s=\x07"]),
            ["ok"]
        );
    }

    #[test]
    fn utf8_continuation_cannot_start_c1_osc() {
        let mut bytes = "\u{045d}52;c;b3duZWQ=".as_bytes().to_vec();
        bytes.extend_from_slice(b"\x07\x1b]52;c;b2s=\x07");
        assert_eq!(scan(&[&bytes]), ["ok"]);
    }

    #[test]
    fn oversized_payload_is_discarded_and_scanner_recovers() {
        let mut oversized = b"\x1b]52;c;".to_vec();
        oversized.extend(std::iter::repeat_n(b'A', MAX_ENCODED_BYTES + 1));
        oversized.extend_from_slice(b"\x07\x1b]52;c;b2s=\x07");
        assert_eq!(scan(&[&oversized]), ["ok"]);
    }
}
