use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::parse::{MAC_HEX_LEN, MAX_BODY_BYTES, Malformation, Report, is_mac_hex, parse};
use crate::token::Token;

pub const TIME_WINDOW_SECS: u64 = 60;
const MAC_SEPARATOR: &[u8] = b":mac=";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Accepted(Report),
    Rejected {
        owner: Option<u32>,
        reason: Malformation,
    },
    Ignored(IgnoreReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IgnoreReason {
    TooLong,
    Reserved,
    NoMac,
    MacMismatch,
    NoTime,
    OutsideWindow,
    OlderThanAccepted,
}

impl Outcome {
    pub fn owner(&self) -> Option<u32> {
        match self {
            Self::Accepted(report) => Some(report.pid),
            Self::Rejected { owner, .. } => *owner,
            Self::Ignored(_) => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayGuard {
    last_accepted: BTreeMap<u32, u64>,
    by_time: BTreeSet<(u64, u32)>,
}

impl ReplayGuard {
    pub fn last_accepted(&self, pid: u32) -> Option<u64> {
        self.last_accepted.get(&pid).copied()
    }

    pub(crate) fn note(&mut self, pid: u32, time: u64, now_secs: u64) {
        let horizon = now_secs.saturating_sub(TIME_WINDOW_SECS);
        while let Some(&(last, stale)) = self.by_time.first()
            && last < horizon
        {
            self.by_time.pop_first();
            self.last_accepted.remove(&stale);
        }
        let last = self.last_accepted.entry(pid).or_insert(time);
        let previous = *last;
        *last = previous.max(time);
        let latest = *last;
        self.by_time.remove(&(previous, pid));
        self.by_time.insert((latest, pid));
    }
}

pub fn sign(signed: &[u8], token: &Token) -> String {
    digest(signed, token)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn classify(body: &[u8], token: &Token, now_secs: u64, guard: &ReplayGuard) -> Outcome {
    if body.len() > MAX_BODY_BYTES {
        return Outcome::Ignored(IgnoreReason::TooLong);
    }
    if body == b"?" {
        return Outcome::Ignored(IgnoreReason::Reserved);
    }
    let Some((signed, received)) = split_mac(body) else {
        return Outcome::Ignored(IgnoreReason::NoMac);
    };
    if !constant_time_eq(&digest(signed, token), &received) {
        return Outcome::Ignored(IgnoreReason::MacMismatch);
    }
    let parsed = parse(body);
    let (pid, time) = match &parsed {
        Ok(report) => (Some(report.pid), Some(report.time)),
        Err(malformed) => (malformed.pid, malformed.time),
    };
    let Some(time) = time else {
        return Outcome::Ignored(IgnoreReason::NoTime);
    };
    if time.abs_diff(now_secs) > TIME_WINDOW_SECS {
        return Outcome::Ignored(IgnoreReason::OutsideWindow);
    }
    if let Some(pid) = pid
        && guard.last_accepted(pid).is_some_and(|last| time < last)
    {
        return Outcome::Ignored(IgnoreReason::OlderThanAccepted);
    }
    match parsed {
        Ok(report) => Outcome::Accepted(report),
        Err(malformed) if malformed.kind.leaves_the_mac_unverifiable() => {
            Outcome::Ignored(IgnoreReason::NoMac)
        }
        Err(malformed) => Outcome::Rejected {
            owner: malformed.pid,
            reason: malformed.kind,
        },
    }
}

fn split_mac(body: &[u8]) -> Option<(&[u8], [u8; 32])> {
    let start = body
        .windows(MAC_SEPARATOR.len())
        .rposition(|window| window == MAC_SEPARATOR)?;
    let hex = &body[start + MAC_SEPARATOR.len()..];
    if hex.len() != MAC_HEX_LEN || !is_mac_hex(hex) {
        return None;
    }
    let mut mac = [0u8; 32];
    let (pairs, _) = hex.as_chunks::<2>();
    for (byte, pair) in mac.iter_mut().zip(pairs) {
        *byte = (hex_digit(pair[0]) << 4) | hex_digit(pair[1]);
    }
    Some((&body[..start], mac))
}

fn hex_digit(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        _ => byte - b'a' + 10,
    }
}

fn digest(signed: &[u8], token: &Token) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(signed);
    hasher.update(b":");
    hasher.update(token.expose().as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    let difference = left
        .iter()
        .zip(right.iter())
        .fold(0u8, |acc, (left, right)| acc | (left ^ right));
    std::hint::black_box(difference) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::Action;

    const NOW: u64 = 1_795_543_200;
    const NEOVIM: &str = "pid=74877:time=1795543200:argv=bnZpbQ==,LVM=,L2hvbWUvdXNlci8ubG9jYWwvc3RhdGUvbnZpbS9zZXNzaW9ucy84ZjJjMWQudmlt:mac=9dbb99bfb856749a87563839d7ce9062818d78cf25909db93f309e0cd3b1909b";
    const NEOVIM_CLEAR: &str = "pid=74877:time=1795543200:clear=1:mac=c30603369f98fef581a4098be889175b0c74f0641e49f8b25f4326cb38f97c93";
    const LAZYGIT: &str = "pid=74877:time=1795543200:argv=bGF6eWdpdA==:cwd=L2hvbWUvdXNlci9zcmMvcHJvamVjdA==:mac=35cb909805c380336efd3689a125acc774a9dec8422a1d21492ebec06ca758e1";
    const STANDARD_ENCODED_24_BYTES: &str = "YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFh";
    const RESUME_WRAP: &str = "pid=90412:time=1795543200:argv=cmVzdW1lLXdyYXA=,c3No,ZGV2Ym94:mac=e76149d0ba54a83570ffab0583867dacdfa23928ba9d18604f0badea88b62b65";

    fn token() -> Token {
        Token::new("dummy-token-replace-me").expect("the specification token is valid")
    }

    fn other_token() -> Token {
        Token::new("another-token-entirely-0123").expect("valid token")
    }

    fn signed_with(keys: &str, token: &Token) -> String {
        format!("{keys}:mac={}", sign(keys.as_bytes(), token))
    }

    fn signed(keys: &str) -> String {
        signed_with(keys, &token())
    }

    fn classify_now(body: &str) -> Outcome {
        classify(body.as_bytes(), &token(), NOW, &ReplayGuard::default())
    }

    #[test]
    fn the_four_valid_specification_reports_are_accepted() {
        for body in [NEOVIM, NEOVIM_CLEAR, LAZYGIT, RESUME_WRAP] {
            assert!(matches!(classify_now(body), Outcome::Accepted(_)), "{body}");
        }
    }

    #[test]
    fn the_resume_wrap_example_decodes_to_its_three_elements() {
        let Outcome::Accepted(report) = classify_now(RESUME_WRAP) else {
            unreachable!("covered by the_four_valid_specification_reports_are_accepted");
        };
        assert_eq!(report.pid, 90412);
        assert_eq!(
            report.action,
            Action::Argv(vec!["resume-wrap".into(), "ssh".into(), "devbox".into()])
        );
    }

    #[test]
    fn the_clear_example_mac_is_the_specification_hash() {
        assert_eq!(
            sign(b"pid=74877:time=1795543200:clear=1", &token()),
            "c30603369f98fef581a4098be889175b0c74f0641e49f8b25f4326cb38f97c93"
        );
    }

    #[test]
    fn a_report_signed_with_another_token_is_ignored_without_an_owner() {
        let body = signed_with("pid=74877:time=1795543200:argv=bnZpbQ==", &other_token());
        let outcome = classify_now(&body);
        assert_eq!(outcome, Outcome::Ignored(IgnoreReason::MacMismatch));
        assert_eq!(outcome.owner(), None);
    }

    #[test]
    fn a_duplicate_key_signed_with_another_token_is_ignored_not_rejected() {
        let body = signed_with(
            "pid=74877:time=1795543200:argv=bnZpbQ==:argv=bnZpbQ==",
            &other_token(),
        );
        assert_eq!(
            classify_now(&body),
            Outcome::Ignored(IgnoreReason::MacMismatch)
        );
    }

    #[test]
    fn a_signed_duplicate_key_inside_the_window_is_rejected_with_its_owner() {
        let body = signed("pid=74877:time=1795543200:argv=bnZpbQ==:argv=bnZpbQ==");
        assert_eq!(
            classify_now(&body),
            Outcome::Rejected {
                owner: Some(74877),
                reason: Malformation::DuplicateKey,
            }
        );
    }

    #[test]
    fn mac_not_last_and_reserved_bodies_are_ignored() {
        let mac_first = format!(
            "mac={}:pid=74877:time=1795543200:argv=bnZpbQ==",
            sign(b"pid=74877:time=1795543200:argv=bnZpbQ==", &token())
        );
        assert_eq!(
            classify_now(&mac_first),
            Outcome::Ignored(IgnoreReason::NoMac)
        );
        assert_eq!(classify_now("?"), Outcome::Ignored(IgnoreReason::Reserved));
    }

    #[test]
    fn a_report_61_seconds_old_is_ignored() {
        let body = signed(&format!("pid=74877:time={}:argv=bnZpbQ==", NOW - 61));
        assert_eq!(
            classify_now(&body),
            Outcome::Ignored(IgnoreReason::OutsideWindow)
        );
    }

    #[test]
    fn a_report_61_seconds_ahead_is_ignored() {
        let body = signed(&format!("pid=74877:time={}:argv=bnZpbQ==", NOW + 61));
        assert_eq!(
            classify_now(&body),
            Outcome::Ignored(IgnoreReason::OutsideWindow)
        );
    }

    #[test]
    fn a_report_at_the_edge_of_the_window_is_accepted() {
        for time in [NOW - 60, NOW + 60] {
            let body = signed(&format!("pid=74877:time={time}:argv=bnZpbQ=="));
            assert!(
                matches!(classify_now(&body), Outcome::Accepted(_)),
                "{time}"
            );
        }
    }

    #[test]
    fn a_report_older_than_the_last_accepted_one_for_its_pid_is_ignored() {
        let mut guard = ReplayGuard::default();
        guard.note(74877, NOW, NOW);
        let earlier = signed(&format!("pid=74877:time={}:clear=1", NOW - 1));
        assert_eq!(
            classify(earlier.as_bytes(), &token(), NOW, &guard),
            Outcome::Ignored(IgnoreReason::OlderThanAccepted)
        );
        let same_time = signed(&format!("pid=74877:time={NOW}:clear=1"));
        assert!(matches!(
            classify(same_time.as_bytes(), &token(), NOW, &guard),
            Outcome::Accepted(_)
        ));
        let other_pid = signed(&format!("pid=74878:time={}:clear=1", NOW - 1));
        assert!(matches!(
            classify(other_pid.as_bytes(), &token(), NOW, &guard),
            Outcome::Accepted(_)
        ));
    }

    #[test]
    fn an_older_malformed_report_is_ignored_not_rejected() {
        let mut guard = ReplayGuard::default();
        guard.note(74877, NOW, NOW);
        let body = signed(&format!("pid=74877:time={}:argv=Li9ydW4uc2g=", NOW - 1));
        assert_eq!(
            classify(body.as_bytes(), &token(), NOW, &guard),
            Outcome::Ignored(IgnoreReason::OlderThanAccepted)
        );
    }

    #[test]
    fn the_replay_guard_forgets_times_that_left_the_window() {
        let mut guard = ReplayGuard::default();
        guard.note(1, NOW, NOW);
        guard.note(2, NOW + 61, NOW + 61);
        assert_eq!(guard.last_accepted(1), None);
        assert_eq!(guard.last_accepted(2), Some(NOW + 61));
    }

    #[test]
    fn the_replay_guard_keeps_its_time_index_in_step_with_its_pids() {
        let mut guard = ReplayGuard::default();
        for pid in 1..=1_000u32 {
            guard.note(pid, NOW + u64::from(pid), NOW + u64::from(pid));
        }
        guard.note(1_000, NOW + 900, NOW + 1_000);
        assert_eq!(guard.last_accepted(1_000), Some(NOW + 1_000));
        assert_eq!(guard.last_accepted(939), None);
        assert_eq!(guard.last_accepted(940), Some(NOW + 940));
        assert_eq!(guard.last_accepted.len(), 61);
        assert_eq!(guard.by_time.len(), guard.last_accepted.len());
        assert!(
            guard
                .by_time
                .iter()
                .all(|(time, pid)| guard.last_accepted(*pid) == Some(*time))
        );
    }

    #[test]
    fn a_time_too_large_for_u64_is_ignored() {
        let body = signed("pid=74877:time=18446744073709551616:argv=bnZpbQ==");
        assert_eq!(classify_now(&body), Outcome::Ignored(IgnoreReason::NoTime));
    }

    #[test]
    fn a_mac_differing_in_its_last_digit_is_ignored() {
        let mut body = NEOVIM.to_owned();
        let last = body.pop().expect("the body ends with a hex digit");
        body.push(if last == '0' { '1' } else { '0' });
        assert_eq!(
            classify_now(&body),
            Outcome::Ignored(IgnoreReason::MacMismatch)
        );
    }

    #[test]
    fn every_invalid_specification_example_has_its_documented_result() {
        let zero_mac = format!("pid=74877:time={NOW}:argv=bnZpbQ==:mac={}", "0".repeat(64));
        assert_eq!(
            classify_now(&zero_mac),
            Outcome::Ignored(IgnoreReason::MacMismatch)
        );
        assert_eq!(
            classify_now(&format!("pid=74877:time={NOW}:argv=bnZpbQ==")),
            Outcome::Ignored(IgnoreReason::NoMac)
        );
        let keys = format!("pid=74877:time={NOW}:argv=bnZpbQ==");
        let mac_first = format!("mac={}:{keys}", sign(keys.as_bytes(), &token()));
        assert_eq!(
            classify_now(&mac_first),
            Outcome::Ignored(IgnoreReason::NoMac)
        );
        assert_eq!(
            classify_now(&signed("pid=74877:time=1000000000:argv=bnZpbQ==")),
            Outcome::Ignored(IgnoreReason::OutsideWindow)
        );
        assert_eq!(
            classify_now(&signed("pid=74877:argv=bnZpbQ==")),
            Outcome::Ignored(IgnoreReason::NoTime)
        );
        for (keys, reason) in [
            ("argv=Li9ydW4uc2g=", Malformation::RelativeProgram),
            ("argv=bnZpbQ==:clear=1", Malformation::ArgvAndClear),
            ("argv=bnZpbQ==:cwd=cHJvamVjdA==", Malformation::RelativeCwd),
            ("argv=bnZpbQ==:need=later", Malformation::UnsupportedNeed),
        ] {
            assert_eq!(
                classify_now(&signed(&format!("pid=74877:time={NOW}:{keys}"))),
                Outcome::Rejected {
                    owner: Some(74877),
                    reason,
                },
                "{keys}"
            );
        }
    }

    #[test]
    fn a_signed_malformed_report_without_a_usable_pid_is_rejected_without_an_owner() {
        let body = signed(&format!("pid=4294967296:time={NOW}:argv=bnZpbQ=="));
        let outcome = classify_now(&body);
        assert_eq!(
            outcome,
            Outcome::Rejected {
                owner: None,
                reason: Malformation::BadPid,
            }
        );
        assert_eq!(outcome.owner(), None);
    }

    #[test]
    fn a_body_longer_than_the_parser_holds_is_ignored() {
        let padding = "a".repeat(MAX_BODY_BYTES);
        let body = signed(&format!("pid=74877:time={NOW}:argv=bnZpbQ==:x={padding}"));
        assert_eq!(classify_now(&body), Outcome::Ignored(IgnoreReason::TooLong));
    }

    #[test]
    fn a_body_at_the_specification_minimum_limits_is_accepted() {
        let element = STANDARD_ENCODED_24_BYTES;
        let elements = vec![element; 63].join(",");
        let body = signed(&format!("pid=74877:time={NOW}:argv=bnZpbQ==,{elements}"));
        assert!(body.len() >= 2048, "{}", body.len());
        let report = match classify_now(&body) {
            Outcome::Accepted(report) => report,
            other => unreachable!("2048 bytes and 64 elements must be accepted, got {other:?}"),
        };
        assert_eq!(report.argv().map(<[String]>::len), Some(64));
    }

    #[test]
    fn the_longest_body_the_parser_holds_is_accepted_and_one_more_byte_is_ignored() {
        let prefix = format!("pid=74877:time={NOW}:argv=bnZpbQ==:x=");
        let keys_len = MAX_BODY_BYTES - ":mac=".len() - MAC_HEX_LEN;
        let longest = signed(&format!("{prefix}{}", "a".repeat(keys_len - prefix.len())));
        assert_eq!(longest.len(), MAX_BODY_BYTES);
        assert!(matches!(classify_now(&longest), Outcome::Accepted(_)));
        let too_long = signed(&format!(
            "{prefix}{}",
            "a".repeat(keys_len - prefix.len() + 1)
        ));
        assert_eq!(too_long.len(), MAX_BODY_BYTES + 1);
        assert_eq!(
            classify_now(&too_long),
            Outcome::Ignored(IgnoreReason::TooLong)
        );
    }
}
