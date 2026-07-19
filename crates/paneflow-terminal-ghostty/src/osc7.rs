const MAX_OSC7_URI_BYTES: usize = 4096;
const MAX_OSC7_PAYLOAD_BYTES: usize = MAX_OSC7_URI_BYTES + 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
    Discard,
    DiscardEscape,
}

#[derive(Default)]
pub(crate) struct Osc7Scanner {
    state: State,
    payload: Vec<u8>,
}

impl Osc7Scanner {
    pub(crate) fn feed(&mut self, bytes: &[u8], emit: &mut impl FnMut(String)) {
        for &byte in bytes {
            match self.state {
                State::Ground => {
                    if byte == b'\x1b' {
                        self.state = State::Escape;
                    }
                }
                State::Escape => match byte {
                    b']' => {
                        self.payload.clear();
                        self.state = State::Osc;
                    }
                    b'\x1b' => {}
                    _ => self.state = State::Ground,
                },
                State::Osc => match byte {
                    b'\x07' => self.finish(emit),
                    b'\x18' | b'\x1a' => self.reset(),
                    b'\x1b' => self.state = State::OscEscape,
                    _ => {
                        self.push_payload_byte(byte);
                    }
                },
                State::OscEscape => match byte {
                    b'\\' => self.finish(emit),
                    b'\x18' | b'\x1a' => self.reset(),
                    _ => {
                        if self.push_payload_byte(b'\x1b') {
                            if byte == b'\x1b' {
                                self.state = State::OscEscape;
                            } else if self.push_payload_byte(byte) {
                                self.state = State::Osc;
                            }
                        }
                    }
                },
                State::Discard => match byte {
                    b'\x07' | b'\x18' | b'\x1a' => self.reset(),
                    b'\x1b' => self.state = State::DiscardEscape,
                    _ => {}
                },
                State::DiscardEscape => match byte {
                    b'\\' | b'\x07' | b'\x18' | b'\x1a' => self.reset(),
                    b'\x1b' => {}
                    _ => self.state = State::Discard,
                },
            }
        }
    }

    fn push_payload_byte(&mut self, byte: u8) -> bool {
        if self.payload.len() < MAX_OSC7_PAYLOAD_BYTES {
            self.payload.push(byte);
            true
        } else {
            self.payload.clear();
            self.state = State::Discard;
            false
        }
    }

    fn finish(&mut self, emit: &mut impl FnMut(String)) {
        if let Ok(payload) = std::str::from_utf8(&self.payload)
            && let Some(raw) = payload.strip_prefix("7;")
            && raw.len() <= MAX_OSC7_URI_BYTES
            && let Some(cwd) = working_directory_from_ghostty(raw)
        {
            emit(cwd);
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.state = State::Ground;
        self.payload.clear();
    }
}

pub(crate) fn working_directory_from_ghostty(raw: &str) -> Option<String> {
    let rest = raw.strip_prefix("file://")?;
    let path = if rest.starts_with('/') {
        rest.to_owned()
    } else {
        let (_, path) = rest.split_once('/')?;
        format!("/{path}")
    };
    let decoded = percent_decode_uri_path(&path)?;

    #[cfg(windows)]
    if let Some(msys_path) = msys_path_to_windows_path(&decoded) {
        return Some(msys_path);
    }

    #[cfg(windows)]
    if decoded.len() >= 3
        && decoded.as_bytes()[0] == b'/'
        && decoded.as_bytes()[1].is_ascii_alphabetic()
        && decoded.as_bytes()[2] == b':'
    {
        return Some(decoded[1..].replace('/', "\\"));
    }
    Some(decoded)
}

#[cfg(windows)]
fn msys_path_to_windows_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    if bytes.len() < 2
        || bytes[0] != b'/'
        || !bytes[1].is_ascii_alphabetic()
        || (bytes.len() > 2 && bytes[2] != b'/')
    {
        return None;
    }

    let drive = (bytes[1] as char).to_ascii_uppercase();
    if bytes.len() == 2 {
        Some(format!("{drive}:\\"))
    } else {
        Some(format!("{drive}:\\{}", path[3..].replace('/', "\\")))
    }
}

fn percent_decode_uri_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            match (
                bytes.get(index + 1).copied().and_then(hex_value),
                bytes.get(index + 2).copied().and_then(hex_value),
            ) {
                (Some(high), Some(low)) => {
                    output.push((high << 4) | low);
                    index += 3;
                }
                _ => {
                    output.push(bytes[index]);
                    index += 1;
                }
            }
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<String> {
        let mut scanner = Osc7Scanner::default();
        let mut output = Vec::new();
        for chunk in chunks {
            scanner.feed(chunk, &mut |cwd| output.push(cwd));
        }
        output
    }

    #[test]
    fn osc7_scanner_handles_split_bel_and_st() {
        assert_eq!(
            scan(&[
                b"\x1b]7;file:///tmp/path%20",
                b"one\x07\x1b]7;file:///tmp/path%20two\x1b",
                b"\\"
            ]),
            ["/tmp/path one", "/tmp/path two"]
        );
    }

    #[test]
    fn osc7_scanner_drops_oversized_payload_and_recovers() {
        let mut oversized = b"\x1b]7;file:///tmp/".to_vec();
        oversized.extend(std::iter::repeat_n(b'a', MAX_OSC7_URI_BYTES + 1));
        oversized.extend_from_slice(b"\x07\x1b]7;file:///tmp/recovered\x07");

        assert_eq!(scan(&[&oversized]), ["/tmp/recovered"]);
    }

    #[cfg(not(windows))]
    #[test]
    fn osc7_preserves_drive_like_posix_path() {
        assert_eq!(
            working_directory_from_ghostty("file:///C:/dev/path%20with%20space/%C3%A9"),
            Some("/C:/dev/path with space/é".to_owned())
        );
    }

    #[cfg(windows)]
    #[test]
    fn osc7_windows_and_msys_paths_are_decoded() {
        assert_eq!(
            working_directory_from_ghostty("file:///C:/dev/path%20with%20space/%C3%A9"),
            Some(r"C:\dev\path with space\é".to_owned())
        );
        assert_eq!(
            working_directory_from_ghostty("file://DESKTOP-123/c/dev/path%20with%20space"),
            Some(r"C:\dev\path with space".to_owned())
        );
    }
}
