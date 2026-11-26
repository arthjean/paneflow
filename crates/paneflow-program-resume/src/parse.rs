use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};

pub const OSC_NUMBER: &[u8] = b"7577";
pub const MAX_CONTENT_BYTES: usize = 4096;
pub const MAX_BODY_BYTES: usize = MAX_CONTENT_BYTES - OSC_NUMBER.len() - 1;
pub(crate) const MAC_KEY: &[u8] = b"mac";
pub(crate) const MAC_HEX_LEN: usize = 64;
const IMPLEMENTED_KEYS: [&[u8]; 7] = [b"pid", b"time", b"mac", b"argv", b"cwd", b"clear", b"need"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub pid: u32,
    pub time: u64,
    pub action: Action,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Argv(Vec<String>),
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Malformation {
    Reserved,
    MalformedPair,
    MissingMac,
    MacNotLast,
    MalformedMac,
    DuplicateKey,
    MissingPid,
    BadPid,
    MissingTime,
    BadTime,
    MalformedNeed,
    UnsupportedNeed,
    ArgvAndClear,
    MissingArgv,
    BadClear,
    BadBase64,
    BadUtf8,
    ControlCharacter,
    EmptyArgv,
    EmptyProgram,
    DashProgram,
    RelativeProgram,
    RelativeCwd,
}

impl Malformation {
    pub fn leaves_the_mac_unverifiable(self) -> bool {
        matches!(
            self,
            Self::Reserved | Self::MissingMac | Self::MacNotLast | Self::MalformedMac
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed {
    pub kind: Malformation,
    pub pid: Option<u32>,
    pub time: Option<u64>,
}

impl Report {
    pub fn argv(&self) -> Option<&[String]> {
        match &self.action {
            Action::Argv(argv) => Some(argv),
            Action::Clear => None,
        }
    }
}

pub fn sequence_body(content: &[u8]) -> Option<&[u8]> {
    let (number, body) = match content.iter().position(|byte| *byte == b';') {
        Some(split) => (&content[..split], &content[split + 1..]),
        None => (content, &content[content.len()..]),
    };
    (number == OSC_NUMBER).then_some(body)
}

pub fn parse(body: &[u8]) -> Result<Report, Malformed> {
    let segments: Vec<&[u8]> = body.split(|byte| *byte == b':').collect();
    let pid = agreed_value(&segments, b"pid", parse_pid);
    let time = agreed_value(&segments, b"time", parse_time);
    let fail = |kind| Malformed { kind, pid, time };

    if body == b"?" {
        return Err(fail(Malformation::Reserved));
    }
    let pairs = segments
        .iter()
        .map(|segment| strict_pair(segment))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| fail(Malformation::MalformedPair))?;
    check_mac_position(&pairs).map_err(fail)?;
    if has_duplicate_key(&pairs) {
        return Err(fail(Malformation::DuplicateKey));
    }
    let value = |key: &[u8]| {
        pairs
            .iter()
            .find(|(candidate, _)| *candidate == key)
            .map(|(_, value)| *value)
    };

    let pid = match value(b"pid") {
        None => return Err(fail(Malformation::MissingPid)),
        Some(raw) => parse_pid(raw).ok_or_else(|| fail(Malformation::BadPid))?,
    };
    let time = match value(b"time") {
        None => return Err(fail(Malformation::MissingTime)),
        Some(raw) => parse_time(raw).ok_or_else(|| fail(Malformation::BadTime))?,
    };
    if let Some(need) = value(b"need") {
        check_need(need).map_err(fail)?;
    }
    let action = match (value(b"argv"), value(b"clear")) {
        (Some(_), Some(_)) => return Err(fail(Malformation::ArgvAndClear)),
        (None, None) => return Err(fail(Malformation::MissingArgv)),
        (None, Some(b"1")) => Action::Clear,
        (None, Some(_)) => return Err(fail(Malformation::BadClear)),
        (Some(raw), None) => Action::Argv(decode_argv(raw).map_err(fail)?),
    };
    let cwd = value(b"cwd")
        .map(|raw| decode_cwd(raw).map_err(fail))
        .transpose()?;
    Ok(Report {
        pid,
        time,
        action,
        cwd,
    })
}

fn agreed_value<T: PartialEq + Copy>(
    segments: &[&[u8]],
    key: &[u8],
    parse_value: fn(&[u8]) -> Option<T>,
) -> Option<T> {
    let mut agreed = None;
    for segment in segments {
        let Some(split) = segment.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        if &segment[..split] != key {
            continue;
        }
        let parsed = parse_value(&segment[split + 1..])?;
        if agreed.is_some_and(|earlier| earlier != parsed) {
            return None;
        }
        agreed = Some(parsed);
    }
    agreed
}

fn strict_pair(segment: &[u8]) -> Option<(&[u8], &[u8])> {
    let split = segment.iter().position(|byte| *byte == b'=')?;
    let (key, value) = (&segment[..split], &segment[split + 1..]);
    let key_fits = !key.is_empty() && key.iter().all(u8::is_ascii_lowercase);
    let value_fits = value.iter().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'.' | b',' | b'+' | b'/' | b'=' | b'-')
    });
    (key_fits && value_fits).then_some((key, value))
}

fn check_mac_position(pairs: &[(&[u8], &[u8])]) -> Result<(), Malformation> {
    match pairs.last() {
        Some((key, value)) if *key == MAC_KEY => {
            if is_mac_hex(value) {
                Ok(())
            } else {
                Err(Malformation::MalformedMac)
            }
        }
        _ if pairs.iter().any(|(key, _)| *key == MAC_KEY) => Err(Malformation::MacNotLast),
        _ => Err(Malformation::MissingMac),
    }
}

pub(crate) fn is_mac_hex(value: &[u8]) -> bool {
    value.len() == MAC_HEX_LEN
        && value
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn has_duplicate_key(pairs: &[(&[u8], &[u8])]) -> bool {
    let mut keys: Vec<&[u8]> = pairs.iter().map(|(key, _)| *key).collect();
    keys.sort_unstable();
    keys.windows(2).any(|window| window[0] == window[1])
}

fn decimal(raw: &[u8]) -> Option<&str> {
    let fits = raw
        .first()
        .is_some_and(|first| (b'1'..=b'9').contains(first))
        && raw.iter().all(u8::is_ascii_digit);
    fits.then(|| std::str::from_utf8(raw).ok()).flatten()
}

fn parse_pid(raw: &[u8]) -> Option<u32> {
    decimal(raw)?.parse().ok()
}

fn parse_time(raw: &[u8]) -> Option<u64> {
    decimal(raw)?.parse().ok()
}

fn check_need(raw: &[u8]) -> Result<(), Malformation> {
    for key in raw.split(|byte| *byte == b',') {
        if key.is_empty() || !key.iter().all(u8::is_ascii_lowercase) {
            return Err(Malformation::MalformedNeed);
        }
        if !IMPLEMENTED_KEYS.contains(&key) {
            return Err(Malformation::UnsupportedNeed);
        }
    }
    Ok(())
}

fn decode_text(raw: &[u8]) -> Result<String, Malformation> {
    let engine = if raw.contains(&b'=') {
        &STANDARD
    } else {
        &STANDARD_NO_PAD
    };
    let bytes = engine.decode(raw).map_err(|_| Malformation::BadBase64)?;
    let text = String::from_utf8(bytes).map_err(|_| Malformation::BadUtf8)?;
    if text.chars().any(char::is_control) {
        return Err(Malformation::ControlCharacter);
    }
    Ok(text)
}

fn decode_argv(raw: &[u8]) -> Result<Vec<String>, Malformation> {
    if raw.is_empty() {
        return Err(Malformation::EmptyArgv);
    }
    let argv = raw
        .split(|byte| *byte == b',')
        .map(decode_text)
        .collect::<Result<Vec<_>, _>>()?;
    match argv.first() {
        Some(program) => check_program(program)?,
        None => return Err(Malformation::EmptyArgv),
    }
    Ok(argv)
}

fn decode_cwd(raw: &[u8]) -> Result<String, Malformation> {
    let cwd = decode_text(raw)?;
    if is_absolute(&cwd) {
        Ok(cwd)
    } else {
        Err(Malformation::RelativeCwd)
    }
}

fn check_program(program: &str) -> Result<(), Malformation> {
    if program.is_empty() {
        Err(Malformation::EmptyProgram)
    } else if !names_a_path(program) {
        if program.starts_with('-') {
            Err(Malformation::DashProgram)
        } else {
            Ok(())
        }
    } else if is_absolute(program) {
        Ok(())
    } else {
        Err(Malformation::RelativeProgram)
    }
}

fn names_a_path(program: &str) -> bool {
    program
        .chars()
        .any(|char| char == '/' || std::path::is_separator(char) || (cfg!(windows) && char == ':'))
}

fn is_absolute(path: &str) -> bool {
    path.starts_with('/') || Path::new(path).is_absolute()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEOVIM: &str = "pid=74877:time=1795543200:argv=bnZpbQ==,LVM=,L2hvbWUvdXNlci8ubG9jYWwvc3RhdGUvbnZpbS9zZXNzaW9ucy84ZjJjMWQudmlt:mac=9dbb99bfb856749a87563839d7ce9062818d78cf25909db93f309e0cd3b1909b";
    const LAZYGIT: &str = "pid=74877:time=1795543200:argv=bGF6eWdpdA==:cwd=L2hvbWUvdXNlci9zcmMvcHJvamVjdA==:mac=35cb909805c380336efd3689a125acc774a9dec8422a1d21492ebec06ca758e1";
    const ZERO_MAC: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn with_mac(keys: &str) -> String {
        format!("{keys}:mac={ZERO_MAC}")
    }

    fn refusal(body: &str) -> Malformed {
        parse(body.as_bytes()).expect_err("the body must be refused")
    }

    fn encoded(text: &str) -> String {
        STANDARD.encode(text.as_bytes())
    }

    #[test]
    fn the_neovim_example_parses_into_its_argv_without_a_cwd() {
        let report = parse(NEOVIM.as_bytes()).expect("the Neovim example parses");
        assert_eq!(report.pid, 74877);
        assert_eq!(report.time, 1_795_543_200);
        assert_eq!(
            report.action,
            Action::Argv(vec![
                "nvim".to_owned(),
                "-S".to_owned(),
                "/home/user/.local/state/nvim/sessions/8f2c1d.vim".to_owned(),
            ])
        );
        assert_eq!(report.cwd, None);
    }

    #[test]
    fn the_lazygit_example_carries_its_cwd() {
        let report = parse(LAZYGIT.as_bytes()).expect("the lazygit example parses");
        assert_eq!(report.argv(), Some(&["lazygit".to_owned()][..]));
        assert_eq!(report.cwd.as_deref(), Some("/home/user/src/project"));
    }

    #[test]
    fn the_clear_example_parses_into_a_clear() {
        let report = parse(b"pid=74877:time=1795543200:clear=1:mac=c30603369f98fef581a4098be889175b0c74f0641e49f8b25f4326cb38f97c93")
            .expect("the clear example parses");
        assert_eq!(report.action, Action::Clear);
        assert_eq!(report.argv(), None);
    }

    #[test]
    fn a_body_whose_last_key_is_not_mac_is_refused_as_mac_not_last() {
        let body = format!("mac={ZERO_MAC}:pid=74877:time=1795543200:argv=bnZpbQ==");
        assert_eq!(refusal(&body).kind, Malformation::MacNotLast);
    }

    #[test]
    fn a_body_without_mac_is_refused_as_missing_mac() {
        assert_eq!(
            refusal("pid=74877:time=1795543200:argv=bnZpbQ==").kind,
            Malformation::MissingMac
        );
    }

    #[test]
    fn a_mac_that_is_not_64_lowercase_hex_digits_is_malformed() {
        let upper = format!("pid=1:time=2:clear=1:mac={}", ZERO_MAC.replace('0', "A"));
        assert_eq!(refusal(&upper).kind, Malformation::MalformedMac);
        assert_eq!(
            refusal("pid=1:time=2:clear=1:mac=00").kind,
            Malformation::MalformedMac
        );
    }

    #[test]
    fn a_question_mark_body_is_refused_as_reserved() {
        assert_eq!(refusal("?").kind, Malformation::Reserved);
    }

    #[test]
    fn a_repeated_key_is_refused_as_duplicate_key_with_the_parsed_pid() {
        let refused = refusal(&with_mac(
            "pid=74877:time=1795543200:argv=bnZpbQ==:argv=bnZpbQ==",
        ));
        assert_eq!(refused.kind, Malformation::DuplicateKey);
        assert_eq!(refused.pid, Some(74877));
        assert_eq!(refused.time, Some(1_795_543_200));
    }

    #[test]
    fn a_repeated_pid_with_two_values_leaves_no_owner() {
        let refused = refusal(&with_mac("pid=1:pid=2:time=1795543200:argv=bnZpbQ=="));
        assert_eq!(refused.kind, Malformation::DuplicateKey);
        assert_eq!(refused.pid, None);
    }

    #[test]
    fn a_relative_program_is_refused() {
        let refused = refusal(&with_mac("pid=74877:time=1795543200:argv=Li9ydW4uc2g="));
        assert_eq!(refused.kind, Malformation::RelativeProgram);
        assert_eq!(refused.pid, Some(74877));
    }

    #[test]
    fn a_program_starting_with_a_dash_is_refused() {
        let body = with_mac(&format!("pid=74877:time=1795543200:argv={}", encoded("-x")));
        assert_eq!(refusal(&body).kind, Malformation::DashProgram);
    }

    #[test]
    fn an_absolute_program_parses() {
        let body = with_mac(&format!(
            "pid=74877:time=1795543200:argv={}",
            encoded("/usr/bin/nvim")
        ));
        let report = parse(body.as_bytes()).expect("an absolute program parses");
        assert_eq!(report.argv(), Some(&["/usr/bin/nvim".to_owned()][..]));
    }

    #[test]
    fn argv_and_clear_together_are_refused() {
        assert_eq!(
            refusal(&with_mac("pid=74877:time=1795543200:argv=bnZpbQ==:clear=1")).kind,
            Malformation::ArgvAndClear
        );
    }

    #[test]
    fn a_report_without_argv_or_clear_is_refused() {
        assert_eq!(
            refusal(&with_mac("pid=74877:time=1795543200")).kind,
            Malformation::MissingArgv
        );
    }

    #[test]
    fn a_clear_value_other_than_one_is_refused() {
        assert_eq!(
            refusal(&with_mac("pid=74877:time=1795543200:clear=2")).kind,
            Malformation::BadClear
        );
    }

    #[test]
    fn a_decoded_element_with_a_c1_control_character_is_refused() {
        let body = with_mac(&format!(
            "pid=74877:time=1795543200:argv=bnZpbQ==,{}",
            encoded("a\u{85}b")
        ));
        assert_eq!(refusal(&body).kind, Malformation::ControlCharacter);
    }

    #[test]
    fn a_decoded_element_with_a_newline_or_delete_is_refused() {
        for text in ["a\nb", "a\u{7f}b", "\u{0}"] {
            let body = with_mac(&format!(
                "pid=74877:time=1795543200:argv=bnZpbQ==,{}",
                encoded(text)
            ));
            assert_eq!(refusal(&body).kind, Malformation::ControlCharacter);
        }
    }

    #[test]
    fn a_right_to_left_override_is_allowed_in_an_element() {
        let body = with_mac(&format!(
            "pid=74877:time=1795543200:argv=bnZpbQ==,{}",
            encoded("a\u{202e}b")
        ));
        let report = parse(body.as_bytes()).expect("U+202E is not a control character");
        assert_eq!(report.argv().map(<[String]>::len), Some(2));
    }

    #[test]
    fn padding_of_the_wrong_length_is_refused_as_bad_base64() {
        for argv in ["bnZpbQ=", "bnZpbQ===", "bn=pbQ==", "bnZp_Q=="] {
            let body = with_mac(&format!("pid=74877:time=1795543200:argv={argv}"));
            assert_eq!(refusal(&body).kind, Malformation::BadBase64, "{argv}");
        }
    }

    #[test]
    fn unpadded_base64_parses() {
        let report = parse(with_mac("pid=74877:time=1795543200:argv=bnZpbQ").as_bytes())
            .expect("padding is optional");
        assert_eq!(report.argv(), Some(&["nvim".to_owned()][..]));
    }

    #[test]
    fn bytes_that_are_not_utf8_are_refused() {
        let body = with_mac(&format!(
            "pid=74877:time=1795543200:argv=bnZpbQ==,{}",
            STANDARD.encode([0xff, 0xfe])
        ));
        assert_eq!(refusal(&body).kind, Malformation::BadUtf8);
    }

    #[test]
    fn need_naming_an_unimplemented_key_is_refused() {
        assert_eq!(
            refusal(&with_mac(
                "pid=74877:time=1795543200:argv=bnZpbQ==:need=later"
            ))
            .kind,
            Malformation::UnsupportedNeed
        );
    }

    #[test]
    fn need_naming_implemented_keys_parses() {
        let report = parse(
            with_mac("pid=74877:time=1795543200:argv=bnZpbQ==:cwd=Lw==:need=cwd,argv").as_bytes(),
        )
        .expect("need lists keys Paneflow implements");
        assert_eq!(report.cwd.as_deref(), Some("/"));
    }

    #[test]
    fn a_malformed_need_value_is_refused() {
        for need in ["", "cwd,", "CWD"] {
            let body = with_mac(&format!(
                "pid=74877:time=1795543200:argv=bnZpbQ==:need={need}"
            ));
            assert_eq!(refusal(&body).kind, Malformation::MalformedNeed, "{need}");
        }
    }

    #[test]
    fn an_unknown_key_without_need_is_dropped() {
        let report = parse(with_mac("pid=74877:time=1795543200:foo=bar:argv=bnZpbQ==").as_bytes())
            .expect("an unknown key is ignored");
        assert_eq!(report.argv(), Some(&["nvim".to_owned()][..]));
    }

    #[test]
    fn an_empty_argv_is_refused() {
        assert_eq!(
            refusal(&with_mac("pid=74877:time=1795543200:argv=")).kind,
            Malformation::EmptyArgv
        );
    }

    #[test]
    fn an_empty_first_element_is_refused() {
        assert_eq!(
            refusal(&with_mac("pid=74877:time=1795543200:argv=,bnZpbQ==")).kind,
            Malformation::EmptyProgram
        );
    }

    #[test]
    fn an_empty_element_after_the_first_is_an_empty_argument() {
        let report = parse(with_mac("pid=74877:time=1795543200:argv=bnZpbQ==,,LVM=,").as_bytes())
            .expect("empty arguments are allowed after the program");
        assert_eq!(
            report.argv(),
            Some(
                &[
                    "nvim".to_owned(),
                    String::new(),
                    "-S".to_owned(),
                    String::new()
                ][..]
            )
        );
    }

    #[test]
    fn a_relative_cwd_is_refused() {
        assert_eq!(
            refusal(&with_mac(
                "pid=74877:time=1795543200:argv=bnZpbQ==:cwd=cHJvamVjdA=="
            ))
            .kind,
            Malformation::RelativeCwd
        );
        assert_eq!(
            refusal(&with_mac("pid=74877:time=1795543200:argv=bnZpbQ==:cwd=")).kind,
            Malformation::RelativeCwd
        );
    }

    #[test]
    fn a_missing_or_malformed_pid_leaves_no_owner() {
        let missing = refusal(&with_mac("time=1795543200:argv=bnZpbQ=="));
        assert_eq!(
            (missing.kind, missing.pid),
            (Malformation::MissingPid, None)
        );
        for pid in ["0", "012", "4294967296", "-1"] {
            let refused = refusal(&with_mac(&format!(
                "pid={pid}:time=1795543200:argv=bnZpbQ=="
            )));
            assert_eq!(refused.kind, Malformation::BadPid, "{pid}");
            assert_eq!(refused.pid, None, "{pid}");
        }
    }

    #[test]
    fn a_time_too_large_for_u64_is_refused_without_a_time() {
        let refused = refusal(&with_mac(
            "pid=74877:time=18446744073709551616:argv=bnZpbQ==",
        ));
        assert_eq!(refused.kind, Malformation::BadTime);
        assert_eq!(refused.time, None);
        assert_eq!(refused.pid, Some(74877));
    }

    #[test]
    fn a_missing_time_is_refused() {
        assert_eq!(
            refusal(&with_mac("pid=74877:argv=bnZpbQ==")).kind,
            Malformation::MissingTime
        );
    }

    #[test]
    fn a_malformed_pair_keeps_the_pid_and_time_it_could_read() {
        for keys in [
            "pid=74877:time=1795543200:argv=bnZpbQ==:bad pair",
            "pid=74877:time=1795543200:argv=bnZpbQ==::x=1",
            "pid=74877:time=1795543200:argv=bnZpbQ==:Key=1",
            "pid=74877:time=1795543200:argv=bnZpbQ==:novalue",
            "pid=74877:time=1795543200:argv=bnZpbQ==:x=a;b",
        ] {
            let refused = refusal(&with_mac(keys));
            assert_eq!(refused.kind, Malformation::MalformedPair, "{keys}");
            assert_eq!(refused.pid, Some(74877), "{keys}");
            assert_eq!(refused.time, Some(1_795_543_200), "{keys}");
        }
    }

    #[test]
    fn sequence_body_keeps_only_osc_7577() {
        assert_eq!(sequence_body(b"7577;pid=1"), Some(&b"pid=1"[..]));
        assert_eq!(sequence_body(b"7577"), Some(&b""[..]));
        assert_eq!(sequence_body(b"7577;"), Some(&b""[..]));
        assert_eq!(sequence_body(b"75770;pid=1"), None);
        assert_eq!(sequence_body(b"7400;x"), None);
        assert_eq!(sequence_body(b""), None);
    }

    #[test]
    fn the_body_cap_leaves_room_for_the_osc_number() {
        assert_eq!(MAX_BODY_BYTES + b"7577;".len(), MAX_CONTENT_BYTES);
    }
}
