use base64::Engine as _;

use crate::osc7::working_directory_from_ghostty;
use crate::{BackendEvent, Rgb, TerminalAppearance};

const MAX_CLIPBOARD_BYTES: usize = 100 * 1024;
const MAX_ENCODED_CLIPBOARD_BYTES: usize = MAX_CLIPBOARD_BYTES.div_ceil(3) * 4;
const MAX_METADATA_OSC_BYTES: usize = 4 * 1024;
pub(crate) const MAX_OSC_SEQUENCE_BYTES: usize = MAX_ENCODED_CLIPBOARD_BYTES + 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StreamState {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
    Discard,
    DiscardEscape,
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
        (ParserState::Osc, b'\x9c', true) => ParserState::Ground,
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

pub(crate) struct OscRouter {
    state: StreamState,
    parser_state: ParserState,
    sequence: Vec<u8>,
    appearance: TerminalAppearance,
    last_working_directory: Option<String>,
}

impl OscRouter {
    pub(crate) fn new(appearance: TerminalAppearance) -> Self {
        Self {
            state: StreamState::Ground,
            parser_state: ParserState::Ground,
            sequence: Vec::new(),
            appearance,
            last_working_directory: None,
        }
    }

    pub(crate) fn set_appearance(&mut self, appearance: TerminalAppearance) {
        self.appearance = appearance;
    }

    pub(crate) fn reset(&mut self) {
        self.state = StreamState::Ground;
        self.parser_state = ParserState::Ground;
        self.sequence.clear();
        self.last_working_directory = None;
    }

    pub(crate) fn feed(
        &mut self,
        bytes: &[u8],
        forward: &mut impl FnMut(&[u8]),
        emit: &mut impl FnMut(BackendEvent),
    ) {
        let mut output = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            self.advance(byte, &mut output, forward, emit);
        }
        self.flush(&mut output, forward);
    }

    fn advance(
        &mut self,
        byte: u8,
        output: &mut Vec<u8>,
        forward: &mut impl FnMut(&[u8]),
        emit: &mut impl FnMut(BackendEvent),
    ) {
        let mut current = Some(byte);
        while let Some(byte) = current.take() {
            let raw_c1 = (!matches!(self.state, StreamState::Ground)
                || self.parser_state != ParserState::Ground)
                && matches!(byte, 0x80..=0x9f);
            match self.state {
                StreamState::Ground if raw_c1 && byte == b'\x9d' => self.start_c1_osc(),
                StreamState::Ground if byte == b'\x1b' => self.state = StreamState::Escape,
                StreamState::Ground => self.push_output(output, byte),
                StreamState::Escape if raw_c1 && byte == b'\x9d' => {
                    self.push_output(output, b'\x1b');
                    self.start_c1_osc();
                }
                StreamState::Escape if byte == b']' => {
                    self.sequence.clear();
                    self.sequence.extend_from_slice(b"\x1b]");
                    self.state = StreamState::Osc;
                }
                StreamState::Escape if byte == b'\x1b' => self.push_output(output, b'\x1b'),
                StreamState::Escape => {
                    self.push_output(output, b'\x1b');
                    self.push_output(output, byte);
                    self.state = StreamState::Ground;
                }
                StreamState::Osc if byte == b'\x1b' => self.state = StreamState::OscEscape,
                StreamState::Osc if raw_c1 && byte == b'\x9c' => {
                    self.sequence.push(byte);
                    self.finish_sequence(false, output, forward, emit);
                }
                StreamState::Osc => {
                    self.sequence.push(byte);
                    if (is_osc7(&self.sequence) && self.sequence.len() > MAX_METADATA_OSC_BYTES)
                        || self.sequence.len() > MAX_OSC_SEQUENCE_BYTES
                    {
                        self.sequence.clear();
                        self.state = StreamState::Discard;
                    } else if byte == b'\x07' {
                        self.finish_sequence(false, output, forward, emit);
                    } else if matches!(byte, b'\x18' | b'\x1a') {
                        self.finish_sequence(true, output, forward, emit);
                    }
                }
                StreamState::OscEscape if byte == b'\\' => {
                    self.sequence.extend_from_slice(b"\x1b\\");
                    self.finish_sequence(false, output, forward, emit);
                }
                StreamState::OscEscape => {
                    let sequence = std::mem::take(&mut self.sequence);
                    self.push_outputs(output, &sequence);
                    self.sequence = sequence;
                    self.sequence.clear();
                    self.state = StreamState::Escape;
                    current = Some(byte);
                }
                StreamState::Discard if byte == b'\x1b' => {
                    self.state = StreamState::DiscardEscape;
                }
                StreamState::Discard
                    if (raw_c1 && byte == b'\x9c')
                        || matches!(byte, b'\x07' | b'\x18' | b'\x1a') =>
                {
                    self.push_output(output, b'\x18');
                    self.state = StreamState::Ground;
                }
                StreamState::Discard => {}
                StreamState::DiscardEscape if byte == b'\\' => {
                    self.push_output(output, b'\x18');
                    self.state = StreamState::Ground;
                }
                StreamState::DiscardEscape => {
                    self.push_output(output, b'\x18');
                    self.state = StreamState::Escape;
                    current = Some(byte);
                }
            }
        }
    }

    fn start_c1_osc(&mut self) {
        self.sequence.clear();
        self.sequence.push(b'\x9d');
        self.state = StreamState::Osc;
    }

    fn finish_sequence(
        &mut self,
        cancelled: bool,
        output: &mut Vec<u8>,
        forward: &mut impl FnMut(&[u8]),
        emit: &mut impl FnMut(BackendEvent),
    ) {
        let sequence = std::mem::take(&mut self.sequence);
        if cancelled || !self.dispatch(&sequence, output, forward, emit) {
            self.push_outputs(output, &sequence);
        }
        self.sequence = sequence;
        self.sequence.clear();
        self.state = StreamState::Ground;
    }

    /// Returns true only when the sequence is intercepted instead of being
    /// forwarded to libghostty.
    fn dispatch(
        &mut self,
        sequence: &[u8],
        output: &mut Vec<u8>,
        forward: &mut impl FnMut(&[u8]),
        emit: &mut impl FnMut(BackendEvent),
    ) -> bool {
        let Some((body, terminator)) = sequence_body(sequence) else {
            return false;
        };
        match body {
            b"10;?" => self.emit_color_reply(
                10,
                self.appearance.foreground,
                terminator,
                output,
                forward,
                emit,
            ),
            b"11;?" => self.emit_color_reply(
                11,
                self.appearance.background,
                terminator,
                output,
                forward,
                emit,
            ),
            b"12;?" => self.emit_color_reply(
                12,
                self.appearance.cursor,
                terminator,
                output,
                forward,
                emit,
            ),
            _ => {
                if let Some(raw) = body.strip_prefix(b"7;") {
                    self.emit_working_directory(raw, emit);
                } else if let Some(raw) = body.strip_prefix(b"52;") {
                    emit_clipboard(raw, emit);
                }
                false
            }
        }
    }

    fn emit_color_reply(
        &mut self,
        code: u8,
        color: Rgb,
        terminator: &[u8],
        output: &mut Vec<u8>,
        forward: &mut impl FnMut(&[u8]),
        emit: &mut impl FnMut(BackendEvent),
    ) -> bool {
        self.flush(output, forward);
        let component = |value: u8| u16::from(value) * 0x101;
        let mut reply = format!(
            "\x1b]{code};rgb:{:04x}/{:04x}/{:04x}",
            component(color.r),
            component(color.g),
            component(color.b),
        )
        .into_bytes();
        reply.extend_from_slice(terminator);
        emit(BackendEvent::WritePty(reply));
        true
    }

    fn emit_working_directory(&mut self, raw: &[u8], emit: &mut impl FnMut(BackendEvent)) {
        let Ok(raw) = std::str::from_utf8(raw) else {
            return;
        };
        let Some(cwd) = working_directory_from_ghostty(raw) else {
            return;
        };
        if self.last_working_directory.as_deref() != Some(&cwd) {
            self.last_working_directory = Some(cwd.clone());
            emit(BackendEvent::WorkingDirectory(cwd));
        }
    }

    fn flush(&mut self, output: &mut Vec<u8>, forward: &mut impl FnMut(&[u8])) {
        if !output.is_empty() {
            forward(output);
            output.clear();
        }
    }

    fn push_outputs(&mut self, output: &mut Vec<u8>, bytes: &[u8]) {
        for &byte in bytes {
            self.push_output(output, byte);
        }
    }

    fn push_output(&mut self, output: &mut Vec<u8>, byte: u8) {
        let raw_c1 = self.parser_state != ParserState::Ground && matches!(byte, 0x80..=0x9f);
        output.push(byte);
        self.parser_state = next_parser_state(self.parser_state, byte, raw_c1);
    }
}

fn is_osc7(sequence: &[u8]) -> bool {
    sequence
        .strip_prefix(b"\x1b]")
        .or_else(|| sequence.strip_prefix(b"\x9d"))
        .is_some_and(|body| body.starts_with(b"7;"))
}

fn sequence_body(sequence: &[u8]) -> Option<(&[u8], &[u8])> {
    let body_start = if sequence.starts_with(b"\x1b]") {
        2
    } else if sequence.starts_with(b"\x9d") {
        1
    } else {
        return None;
    };
    let (body_end, terminator) = if sequence.ends_with(b"\x1b\\") {
        (sequence.len().checked_sub(2)?, b"\x1b\\".as_slice())
    } else if sequence.ends_with(b"\x07") {
        (sequence.len().checked_sub(1)?, b"\x07".as_slice())
    } else if sequence.ends_with(b"\x9c") {
        (sequence.len().checked_sub(1)?, b"\x9c".as_slice())
    } else {
        return None;
    };
    Some((sequence.get(body_start..body_end)?, terminator))
}

fn emit_clipboard(raw: &[u8], emit: &mut impl FnMut(BackendEvent)) {
    let Some(separator) = raw.iter().position(|byte| *byte == b';') else {
        return;
    };
    let selection = &raw[..separator];
    let payload = &raw[separator + 1..];
    if !matches!(selection, b"c" | b"p" | b"s") || payload == b"?" {
        return;
    }
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(payload)
        && decoded.len() <= MAX_CLIPBOARD_BYTES
        && let Ok(text) = String::from_utf8(decoded)
    {
        emit(BackendEvent::ClipboardStore(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ColorScheme;

    fn route(router: &mut OscRouter, chunks: &[&[u8]]) -> (Vec<u8>, Vec<BackendEvent>) {
        let mut input = Vec::new();
        let mut events = Vec::new();
        for chunk in chunks {
            router.feed(
                chunk,
                &mut |bytes| input.extend_from_slice(bytes),
                &mut |event| events.push(event),
            );
        }
        (input, events)
    }

    #[test]
    fn one_router_preserves_stream_order_and_dispatches_supported_osc() {
        let appearance = TerminalAppearance::new(
            Rgb {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            },
            Rgb {
                r: 0x44,
                g: 0x55,
                b: 0x66,
            },
            Rgb {
                r: 0x77,
                g: 0x88,
                b: 0x99,
            },
            ColorScheme::Dark,
        );
        let mut router = OscRouter::new(appearance);
        let (input, events) = route(
            &mut router,
            &[
                b"before\x1b]10;?\x1b\\middle\x1b]7;file:///tmp/work%20dir\x07",
                b"\x1b]52;c;b2s=\x07after",
            ],
        );

        assert_eq!(
            input,
            b"beforemiddle\x1b]7;file:///tmp/work%20dir\x07\x1b]52;c;b2s=\x07after"
        );
        assert_eq!(
            events,
            [
                BackendEvent::WritePty(b"\x1b]10;rgb:1111/2222/3333\x1b\\".to_vec()),
                BackendEvent::WorkingDirectory("/tmp/work dir".into()),
                BackendEvent::ClipboardStore("ok".into()),
            ]
        );
    }

    #[test]
    fn oversized_osc_is_replaced_by_cancel_and_router_recovers() {
        let mut router = OscRouter::new(TerminalAppearance::default());
        let oversized = vec![b'A'; MAX_OSC_SEQUENCE_BYTES + 1];
        let (input, events) = route(
            &mut router,
            &[
                b"before\x1b]52;c;",
                &oversized,
                b"\x07after\x1b]52;c;b2s=\x07",
            ],
        );

        assert_eq!(input, b"before\x18after\x1b]52;c;b2s=\x07");
        assert_eq!(events, [BackendEvent::ClipboardStore("ok".into())]);
    }

    #[test]
    fn ground_c1_bytes_stay_text_but_executable_c1_osc_is_dispatched() {
        let mut router = OscRouter::new(TerminalAppearance::default());
        let (input, events) = route(
            &mut router,
            &[b"\x9d52;c;b3duZWQ=\x07\x1b[\xd1\x9d52;c;b2s=\x07"],
        );

        assert_eq!(input, b"\x9d52;c;b3duZWQ=\x07\x1b[\xd1\x9d52;c;b2s=\x07");
        assert_eq!(events, [BackendEvent::ClipboardStore("ok".into())]);
    }

    #[test]
    fn reset_drops_every_partial_parser_state_but_keeps_appearance() {
        let mut router = OscRouter::new(TerminalAppearance::default());
        let _ = route(&mut router, &[b"\x1b]10;"]);
        router.reset();
        let (input, events) = route(&mut router, &[b"?\x07\x1b]10;?\x07"]);

        assert_eq!(input, b"?\x07");
        assert!(matches!(events.as_slice(), [BackendEvent::WritePty(_)]));
    }

    #[test]
    fn c1_st_terminates_osc_and_embedded_escape_is_forwarded_once() {
        let mut router = OscRouter::new(TerminalAppearance::default());
        let (input, events) = route(
            &mut router,
            &[b"\x1b]52;c;b2s=\x9c\x9d52;c;b2s=\x07after\x1b]0;before\x1bxafter\x07"],
        );

        assert_eq!(
            input, b"\x1b]52;c;b2s=\x9c\x9d52;c;b2s=\x07after\x1b]0;before\x1bxafter\x07",
            "the embedded escape must not be duplicated"
        );
        assert_eq!(events, [BackendEvent::ClipboardStore("ok".into())]);
    }
}
