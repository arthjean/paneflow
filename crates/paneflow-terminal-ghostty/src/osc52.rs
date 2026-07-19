use base64::Engine as _;

const MAX_CLIPBOARD_BYTES: usize = 100 * 1024;
const MAX_ENCODED_BYTES: usize = MAX_CLIPBOARD_BYTES.div_ceil(3) * 4;
pub(crate) const MAX_OSC_SEQUENCE_BYTES: usize = MAX_ENCODED_BYTES + 64;

#[derive(Debug, Default)]
enum BoundedState {
    #[default]
    Ground,
    Escape,
    Collect(Vec<u8>),
    Discard,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ParserState {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    String,
}

fn next_parser_state(state: ParserState, byte: u8, raw_c1: bool) -> ParserState {
    match (state, byte, raw_c1) {
        (_, b'\x1b', _) => ParserState::Escape,
        (_, b'\x18' | b'\x1a', _) => ParserState::Ground,
        (ParserState::Osc, b'\x07', _) => ParserState::Ground,
        (ParserState::Osc, _, _) => ParserState::Osc,
        (_, b'\x90' | b'\x98' | b'\x9e' | b'\x9f', true) => ParserState::String,
        (_, b'\x9b', true) => ParserState::Csi,
        (_, b'\x9d', true) => ParserState::Osc,
        (_, b'\x80'..=b'\x8f' | b'\x91'..=b'\x97' | b'\x99' | b'\x9a' | b'\x9c', true) => {
            ParserState::Ground
        }
        (ParserState::Ground, _, _) => ParserState::Ground,
        (ParserState::Escape, b'\x20'..=b'\x2f', _) => ParserState::EscapeIntermediate,
        (ParserState::Escape, b'[', _) => ParserState::Csi,
        (ParserState::Escape, b']', _) => ParserState::Osc,
        (ParserState::Escape, b'P' | b'X' | b'^' | b'_', _) => ParserState::String,
        (ParserState::Escape, b'\x30'..=b'\x7e', _) => ParserState::Ground,
        (ParserState::Escape, _, _) => ParserState::Escape,
        (ParserState::EscapeIntermediate, b'\x30'..=b'\x7e', _) => ParserState::Ground,
        (ParserState::EscapeIntermediate, _, _) => ParserState::EscapeIntermediate,
        (ParserState::Csi, b'\x40'..=b'\x7e', _) => ParserState::Ground,
        (ParserState::Csi, _, _) => ParserState::Csi,
        (ParserState::String, _, _) => ParserState::String,
    }
}

#[derive(Debug, Default)]
pub(crate) struct BoundedOscFilter {
    state: BoundedState,
    parser_state: ParserState,
}

impl BoundedOscFilter {
    pub(crate) fn feed(&mut self, bytes: &[u8], emit: &mut impl FnMut(&[u8])) {
        let mut output = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            let raw_c1 = self.is_raw_c1(byte);
            let c1_osc = raw_c1 && byte == b'\x9d';
            let state = std::mem::take(&mut self.state);
            self.state = match state {
                BoundedState::Ground if c1_osc => BoundedState::Collect(vec![b'\x9d']),
                BoundedState::Ground if byte == b'\x1b' => BoundedState::Escape,
                BoundedState::Ground => {
                    self.push_byte(&mut output, byte, raw_c1);
                    BoundedState::Ground
                }
                BoundedState::Escape if c1_osc => {
                    self.push_bytes(&mut output, b"\x1b");
                    BoundedState::Collect(vec![b'\x9d'])
                }
                BoundedState::Escape if byte == b']' => BoundedState::Collect(vec![b'\x1b', b']']),
                BoundedState::Escape if byte == b'\x1b' => {
                    self.push_bytes(&mut output, b"\x1b");
                    BoundedState::Escape
                }
                BoundedState::Escape => {
                    self.push_bytes(&mut output, &[b'\x1b', byte]);
                    BoundedState::Ground
                }
                BoundedState::Collect(buffer) if byte == b'\x1b' => {
                    self.push_bytes(&mut output, &buffer);
                    BoundedState::Escape
                }
                BoundedState::Collect(mut buffer) => {
                    buffer.push(byte);
                    if buffer.len() > MAX_OSC_SEQUENCE_BYTES {
                        BoundedState::Discard
                    // Pinned Ghostty treats raw C1 ST (0x9c) as OSC payload.
                    // Only BEL, CAN/SUB, or the ESC-based ST can end it.
                    } else if matches!(byte, b'\x07' | b'\x18' | b'\x1a') {
                        self.push_bytes(&mut output, &buffer);
                        BoundedState::Ground
                    } else {
                        BoundedState::Collect(buffer)
                    }
                }
                BoundedState::Discard if byte == b'\x1b' => {
                    self.push_bytes(&mut output, b"\x18");
                    BoundedState::Escape
                }
                BoundedState::Discard if matches!(byte, b'\x07' | b'\x18' | b'\x1a') => {
                    self.push_bytes(&mut output, b"\x18");
                    BoundedState::Ground
                }
                BoundedState::Discard => BoundedState::Discard,
            };
        }
        if !output.is_empty() {
            emit(&output);
        }
    }

    fn is_raw_c1(&self, byte: u8) -> bool {
        (!matches!(self.state, BoundedState::Ground) || self.parser_state != ParserState::Ground)
            && matches!(byte, 0x80..=0x9f)
    }

    fn push_bytes(&mut self, output: &mut Vec<u8>, bytes: &[u8]) {
        for &byte in bytes {
            self.push_byte(output, byte, matches!(byte, 0x80..=0x9f));
        }
    }

    fn push_byte(&mut self, output: &mut Vec<u8>, byte: u8, raw_c1: bool) {
        output.push(byte);
        self.parser_state = next_parser_state(self.parser_state, byte, raw_c1);
    }
}

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
    parser_state: ParserState,
    payload: Vec<u8>,
}

impl Osc52Scanner {
    pub(crate) fn feed(&mut self, bytes: &[u8], emit: &mut impl FnMut(String)) {
        for &byte in bytes {
            let raw_c1 = self.parser_state != ParserState::Ground && matches!(byte, 0x80..=0x9f);
            let starts_osc = (self.parser_state == ParserState::Escape && byte == b']')
                || (raw_c1 && byte == b'\x9d' && self.parser_state != ParserState::Osc);
            self.advance(byte, starts_osc, emit);
            self.parser_state = next_parser_state(self.parser_state, byte, raw_c1);
        }
    }

    fn advance(&mut self, byte: u8, starts_osc: bool, emit: &mut impl FnMut(String)) {
        if starts_osc {
            self.reset_payload();
            self.state = State::Osc;
            return;
        }
        self.state = match self.state {
            State::Ground => match byte {
                b'\x1b' => State::Escape,
                _ => State::Ground,
            },
            State::Escape => match byte {
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
                b'\x07' => {
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
                b'\x07' | b'\x18' | b'\x1a' => State::Ground,
                b'\x1b' => State::DiscardEscape,
                _ => State::Discard,
            },
            State::DiscardEscape => match byte {
                b'\\' => State::Ground,
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
    fn ground_c1_osc_cannot_emit_clipboard_store() {
        let mut bytes = b"\x9d52;c;b3duZWQ=\x07".to_vec();
        bytes.extend_from_slice(b"\x1b]52;c;b2s=\x07");
        assert_eq!(scan(&[&bytes]), ["ok"]);
    }

    #[test]
    fn non_ground_c1_osc_emits_clipboard_store() {
        assert_eq!(scan(&[b"\x1b[", b"\x9d52;c;", b"b2s=\x07"]), ["ok"]);
    }

    #[test]
    fn new_osc_recovers_after_an_interrupted_clipboard_payload() {
        assert_eq!(
            scan(&[b"\x1b]52;c;b3duZWQ=\x1b[", b"\x9d52;c;b2s=\x07"]),
            ["ok"]
        );
        assert_eq!(scan(&[b"\x1b]52;c;b3duZWQ=\x1b]52;c;b2s=\x07"]), ["ok"]);
    }

    #[test]
    fn oversized_payload_is_discarded_and_scanner_recovers() {
        let mut oversized = b"\x1b]52;c;".to_vec();
        oversized.extend(std::iter::repeat_n(b'A', MAX_ENCODED_BYTES + 1));
        oversized.extend_from_slice(b"\x07\x1b]52;c;b2s=\x07");
        assert_eq!(scan(&[&oversized]), ["ok"]);
    }

    #[test]
    fn c1_st_byte_does_not_finish_pinned_ghostty_osc52() {
        assert!(scan(&[b"\x1b]52;c;b2s=\x9cjunk\x07"]).is_empty());
    }

    #[test]
    fn bounded_filter_preserves_valid_fragmented_osc() {
        let mut filter = BoundedOscFilter::default();
        let mut output = Vec::new();
        for chunk in [
            b"before\x1b]52;c;SGV".as_slice(),
            b"sbG8=\x1b".as_slice(),
            b"\\after".as_slice(),
        ] {
            filter.feed(chunk, &mut |bytes| output.extend_from_slice(bytes));
        }
        assert_eq!(output, b"before\x1b]52;c;SGVsbG8=\x1b\\after");
    }

    #[test]
    fn bounded_filter_drops_oversized_osc_before_native_parser() {
        let mut filter = BoundedOscFilter::default();
        let mut output = Vec::new();
        filter.feed(b"before\x1b]52;c;", &mut |bytes| {
            output.extend_from_slice(bytes)
        });
        filter.feed(&vec![b'A'; MAX_OSC_SEQUENCE_BYTES + 1], &mut |bytes| {
            output.extend_from_slice(bytes)
        });
        filter.feed(b"\x07after", &mut |bytes| output.extend_from_slice(bytes));

        assert_eq!(output, b"before\x18after");
        assert!(matches!(filter.state, BoundedState::Ground));
    }

    #[test]
    fn bounded_filter_drops_fragmented_c1_osc_from_csi_state() {
        let mut filter = BoundedOscFilter::default();
        let mut output = Vec::new();
        filter.feed(b"\x1b[\xd1", &mut |bytes| output.extend_from_slice(bytes));
        filter.feed(b"\x9d0;", &mut |bytes| output.extend_from_slice(bytes));
        filter.feed(&vec![b'A'; MAX_OSC_SEQUENCE_BYTES + 1], &mut |bytes| {
            output.extend_from_slice(bytes)
        });
        filter.feed(b"\x9cafter", &mut |bytes| output.extend_from_slice(bytes));

        assert_eq!(output, b"\x1b[\xd1");
        assert!(matches!(filter.state, BoundedState::Discard));

        filter.feed(&vec![b'B'; MAX_OSC_SEQUENCE_BYTES + 1], &mut |bytes| {
            output.extend_from_slice(bytes)
        });
        filter.feed(b"\x07after", &mut |bytes| output.extend_from_slice(bytes));

        assert_eq!(output, b"\x1b[\xd1\x18after");
        assert!(matches!(filter.state, BoundedState::Ground));
        assert_eq!(filter.parser_state, ParserState::Ground);
    }

    #[test]
    fn oversized_osc_discards_nested_c1_osc_until_the_real_terminator() {
        let mut filter = BoundedOscFilter::default();
        let mut output = Vec::new();
        filter.feed(b"\x1b]0;", &mut |bytes| output.extend_from_slice(bytes));
        filter.feed(&vec![b'A'; MAX_OSC_SEQUENCE_BYTES + 1], &mut |bytes| {
            output.extend_from_slice(bytes)
        });
        filter.feed(b"\x9d52;c;b3duZWQ=\x07after", &mut |bytes| {
            output.extend_from_slice(bytes)
        });

        assert_eq!(output, b"\x18after");
        assert!(matches!(filter.state, BoundedState::Ground));
        assert_eq!(filter.parser_state, ParserState::Ground);
    }

    #[test]
    fn bounded_filter_preserves_c1_bytes_inside_ground_state_utf8() {
        let text = "beforeѝќafter";
        let mut filter = BoundedOscFilter::default();
        let mut output = Vec::new();
        for chunk in text.as_bytes().chunks(1) {
            filter.feed(chunk, &mut |bytes| output.extend_from_slice(bytes));
        }
        assert_eq!(output, text.as_bytes());
    }

    #[test]
    fn bounded_filter_preserves_ground_c1_for_ghostty_utf8_decoder() {
        let input = b"\x9d52;c;b3duZWQ=\x07";
        let mut filter = BoundedOscFilter::default();
        let mut output = Vec::new();
        filter.feed(input, &mut |bytes| output.extend_from_slice(bytes));
        assert_eq!(output, input);
        assert_eq!(filter.parser_state, ParserState::Ground);
    }

    #[test]
    fn executable_c1_returns_parser_to_ground_before_utf8_text() {
        let mut input = b"\x1b[\x85".to_vec();
        input.extend_from_slice("ѝtext".as_bytes());
        let mut filter = BoundedOscFilter::default();
        let mut output = Vec::new();
        for chunk in input.chunks(1) {
            filter.feed(chunk, &mut |bytes| output.extend_from_slice(bytes));
        }
        assert_eq!(output, input);
        assert_eq!(filter.parser_state, ParserState::Ground);
    }

    #[test]
    fn c1_bytes_remain_payload_inside_osc_before_ground_utf8() {
        let mut input = b"\x1b]0;".to_vec();
        input.push(b'\x9b');
        input.push(b'\x07');
        input.extend_from_slice("ѝtext".as_bytes());
        let mut filter = BoundedOscFilter::default();
        let mut output = Vec::new();
        for chunk in input.chunks(1) {
            filter.feed(chunk, &mut |bytes| output.extend_from_slice(bytes));
        }
        assert_eq!(output, input);
        assert_eq!(filter.parser_state, ParserState::Ground);
    }
}
