use std::fmt;

pub const HEADER: &str = "paneflow-resume 1";
pub const MAX_HANDOFF_BYTES: usize = 512 * 1024;
pub const ATTEMPT_HEX_LEN: usize = 32;
pub const LAUNCHER_STEM: &str = "paneflow-resume";
pub const HANDOFF_ENV: &str = "PANEFLOW_RESUME_HANDOFF";

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CwdSource {
    Report,
    Owner,
    Spawn,
}

impl CwdSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Report => "report",
            Self::Owner => "owner",
            Self::Spawn => "spawn",
        }
    }

    fn parse(label: &str) -> Option<Self> {
        match label {
            "report" => Some(Self::Report),
            "owner" => Some(Self::Owner),
            "spawn" => Some(Self::Spawn),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    pub attempt: String,
    pub cwd: String,
    pub cwd_source: CwdSource,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub attempt: Option<String>,
    pub reason: &'static str,
}

impl fmt::Display for Invalid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason)
    }
}

impl std::error::Error for Invalid {}

pub fn is_attempt_id(value: &str) -> bool {
    value.len() == ATTEMPT_HEX_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn encode(handoff: &Handoff) -> String {
    let mut text = String::new();
    text.push_str(HEADER);
    text.push('\n');
    text.push_str("attempt ");
    text.push_str(&handoff.attempt);
    text.push('\n');
    text.push_str("cwd ");
    text.push_str(&encode_base64(handoff.cwd.as_bytes()));
    text.push(' ');
    text.push_str(handoff.cwd_source.label());
    text.push('\n');
    for arg in &handoff.argv {
        text.push_str("arg ");
        text.push_str(&encode_base64(arg.as_bytes()));
        text.push('\n');
    }
    text
}

pub fn decode(text: &str) -> Result<Handoff, Invalid> {
    let attempt = text
        .split('\n')
        .find_map(|line| line.strip_prefix("attempt "))
        .filter(|id| is_attempt_id(id))
        .map(str::to_owned);
    let fail = |reason| Invalid {
        attempt: attempt.clone(),
        reason,
    };
    if text.len() > MAX_HANDOFF_BYTES {
        return Err(fail("the handoff is larger than its limit"));
    }
    let Some(body) = text.strip_suffix('\n') else {
        return Err(fail("the handoff does not end with a line feed"));
    };
    let mut lines = body.split('\n');
    if lines.next() != Some(HEADER) {
        return Err(fail("the handoff does not start with its version line"));
    }
    let mut found_attempt = None;
    let mut cwd = None;
    let mut argv = Vec::new();
    for line in lines {
        let (field, value) = line
            .split_once(' ')
            .ok_or_else(|| fail("a handoff line has no value"))?;
        match field {
            "attempt" if found_attempt.is_none() && is_attempt_id(value) => {
                found_attempt = Some(value.to_owned());
            }
            "cwd" if cwd.is_none() => {
                let (path, source) = value
                    .split_once(' ')
                    .ok_or_else(|| fail("the cwd line has no source"))?;
                let source =
                    CwdSource::parse(source).ok_or_else(|| fail("the cwd source is unknown"))?;
                let path = decode_text(path).ok_or_else(|| fail("the cwd is not valid base64"))?;
                cwd = Some((path, source));
            }
            "arg" => argv
                .push(decode_text(value).ok_or_else(|| fail("an argument is not valid base64"))?),
            _ => return Err(fail("the handoff holds an unknown or repeated field")),
        }
    }
    let attempt = found_attempt.ok_or_else(|| fail("the handoff names no attempt"))?;
    let (cwd, cwd_source) = cwd.ok_or_else(|| fail("the handoff names no directory"))?;
    if argv.is_empty() {
        return Err(fail("the handoff names no program"));
    }
    Ok(Handoff {
        attempt,
        cwd,
        cwd_source,
        argv,
    })
}

fn decode_text(value: &str) -> Option<String> {
    String::from_utf8(decode_base64(value)?).ok()
}

pub fn encode_base64(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk.first().copied().unwrap_or(0);
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let group = (u32::from(first) << 16) | (u32::from(second) << 8) | u32::from(third);
        let symbols = [18u32, 12, 6, 0].map(|shift| ALPHABET[((group >> shift) & 0x3f) as usize]);
        let kept = chunk.len() + 1;
        for (index, symbol) in symbols.iter().enumerate() {
            encoded.push(if index < kept {
                char::from(*symbol)
            } else {
                '='
            });
        }
    }
    encoded
}

pub fn decode_base64(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut decoded = Vec::with_capacity(bytes.len() / 4 * 3);
    let groups = bytes.len() / 4;
    for (index, group) in bytes.chunks(4).enumerate() {
        let last = index + 1 == groups;
        let padding = group.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && !last) {
            return None;
        }
        let mut bits = 0u32;
        for symbol in &group[..4 - padding] {
            bits = (bits << 6) | u32::from(sextet(*symbol)?);
        }
        bits <<= 6 * padding as u32;
        let produced = 3 - padding;
        let unused = bits & ((1u32 << (8 * padding as u32)) - 1);
        if unused != 0 {
            return None;
        }
        decoded.extend_from_slice(&[(bits >> 16) as u8, (bits >> 8) as u8, bits as u8][..produced]);
    }
    Some(decoded)
}

fn sextet(symbol: u8) -> Option<u8> {
    match symbol {
        b'A'..=b'Z' => Some(symbol - b'A'),
        b'a'..=b'z' => Some(symbol - b'a' + 26),
        b'0'..=b'9' => Some(symbol - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handoff(argv: &[&str]) -> Handoff {
        Handoff {
            attempt: "0123456789abcdef0123456789abcdef".to_owned(),
            cwd: "/home/user/src".to_owned(),
            cwd_source: CwdSource::Report,
            argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        }
    }

    #[test]
    fn a_handoff_round_trips_every_byte_of_every_argument() {
        let original = handoff(&[
            "paneflow-session-fixture",
            "argv-dump",
            "$(reboot)",
            "a; b",
            "two words",
            "",
            "-n",
            "naïve",
            "*",
        ]);
        let text = encode(&original);
        assert!(
            text.starts_with("paneflow-resume 1\nattempt 0123456789abcdef0123456789abcdef\ncwd ")
        );
        assert!(!text.contains("$(reboot)"));
        assert_eq!(decode(&text).unwrap(), original);
    }

    #[test]
    fn the_format_matches_the_documented_layout() {
        let text = encode(&handoff(&["nvim", "-S"]));
        assert_eq!(
            text,
            "paneflow-resume 1\nattempt 0123456789abcdef0123456789abcdef\ncwd L2hvbWUvdXNlci9zcmM= report\narg bnZpbQ==\narg LVM=\n"
        );
    }

    #[test]
    fn base64_matches_the_standard_alphabet_with_padding() {
        for (raw, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode_base64(raw.as_bytes()), encoded);
            assert_eq!(decode_base64(encoded).unwrap(), raw.as_bytes());
        }
        assert_eq!(encode_base64(&[0xfb, 0xff]), "+/8=");
        for refused in [
            "Zg", "Zg=", "Z===", "Zh==", "Zg==Zg==", "Zm9v-A==", "Zm\n9v",
        ] {
            assert!(decode_base64(refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn a_handoff_without_its_version_line_an_arg_or_with_an_unknown_field_is_refused() {
        let valid = encode(&handoff(&["nvim"]));
        let cases = [
            valid.replacen("paneflow-resume 1", "paneflow-resume 2", 1),
            valid
                .lines()
                .filter(|line| !line.starts_with("arg "))
                .map(|line| format!("{line}\n"))
                .collect(),
            format!("{valid}env UEFUSD0v\n"),
            format!("{valid}attempt 0123456789abcdef0123456789abcdef\n"),
            valid.trim_end().to_owned(),
            valid.replacen(" report", " osc7", 1),
        ];
        for case in cases {
            let refused = decode(&case).unwrap_err();
            assert_eq!(
                refused.attempt.as_deref(),
                Some("0123456789abcdef0123456789abcdef"),
                "{case:?}"
            );
        }
        assert!(decode(&valid.replace('\n', "\r\n")).is_err());
        assert!(
            decode("paneflow-resume 1\nattempt XYZ\n")
                .unwrap_err()
                .attempt
                .is_none()
        );
    }

    #[test]
    fn an_attempt_id_is_thirty_two_lowercase_hex_characters() {
        assert!(is_attempt_id("0123456789abcdef0123456789abcdef"));
        assert!(!is_attempt_id("0123456789ABCDEF0123456789abcdef"));
        assert!(!is_attempt_id("0123456789abcdef"));
    }
}
