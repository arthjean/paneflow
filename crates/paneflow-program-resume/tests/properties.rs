#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use paneflow_program_resume::{
    Action, MAX_CONTENT_BYTES, Outcome, ReplayGuard, Token, classify, parse, sequence_body, sign,
};
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;

const NOW: u64 = 1_795_543_200;

fn token() -> Token {
    Token::new("dummy-token-replace-me").expect("the specification token is valid")
}

fn config() -> ProptestConfig {
    ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

fn element() -> impl Strategy<Value = String> {
    vec(
        any::<char>().prop_filter("reportable text has no control character", |char| {
            !char.is_control()
        }),
        0..12,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

fn encode(text: &str, padded: bool) -> String {
    if padded {
        STANDARD.encode(text)
    } else {
        STANDARD_NO_PAD.encode(text)
    }
}

fn valid_keys() -> impl Strategy<Value = String> {
    (
        1u32..=u32::MAX,
        0u64..=120,
        "[a-z][a-z0-9_.-]{0,15}",
        vec(element(), 0..8),
        option::of("(/[a-zA-Z0-9 ._-]{1,12}){1,4}"),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(pid, offset, program, args, cwd, clear, padded)| {
            let time = NOW - 60 + offset;
            if clear {
                return format!("pid={pid}:time={time}:clear=1");
            }
            let argv: Vec<String> = std::iter::once(program)
                .chain(args)
                .map(|arg| encode(&arg, padded))
                .collect();
            let mut keys = format!("time={time}:argv={}:pid={pid}", argv.join(","));
            if let Some(cwd) = cwd {
                keys.push_str(&format!(":cwd={}", encode(&cwd, padded)));
            }
            keys
        })
}

fn has_spec_control(text: &str) -> bool {
    text.chars()
        .any(|char| matches!(char, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}'))
}

fn spec_valid_program(program: &str) -> bool {
    if program.contains('/') {
        program.starts_with('/')
    } else {
        !program.is_empty() && !program.starts_with('-')
    }
}

fn candidate_report() -> impl Strategy<Value = (u32, u64, String, Vec<String>, Option<String>, bool)>
{
    let program = prop_oneof![
        "[a-z][a-z0-9_.]{0,8}",
        "-[a-z0-9]{0,4}",
        "\\.{1,2}/[a-z]{1,4}",
        "[a-z]{1,4}/[a-z]{1,4}",
        "(/[a-z0-9 ._-]{1,4}){1,3}",
        Just(String::new()),
        "[a-z]{1,3}[\\x00-\\x1f\\x7f\\x{80}-\\x{9f}][a-z]{0,2}",
    ];
    let arg = prop_oneof![
        element(),
        "[a-z ]{0,3}[\\x00-\\x1f\\x7f\\x{80}-\\x{9f}][a-z]{0,3}"
    ];
    let cwd = option::of(prop_oneof![
        "(/[a-zA-Z0-9 ._-]{1,6}){1,3}",
        "[a-z]{1,6}(/[a-z]{1,4})?",
        Just(String::new()),
        "/[a-z]{1,3}[\\x00-\\x1f\\x7f\\x{80}-\\x{9f}][a-z]{0,2}",
    ]);
    (
        1u32..=u32::MAX,
        0u64..=120,
        program,
        vec(arg, 0..5),
        cwd,
        any::<bool>(),
    )
        .prop_map(|(pid, offset, program, args, cwd, padded)| {
            (pid, NOW - 60 + offset, program, args, cwd, padded)
        })
}

fn with_mac(keys: &[u8], token: &Token) -> Vec<u8> {
    let mut body = keys.to_vec();
    body.extend_from_slice(b":mac=");
    body.extend_from_slice(sign(keys, token).as_bytes());
    body
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn arbitrary_bytes_never_panic_the_parser_or_the_classifier(
        bytes in vec(any::<u8>(), 0..=MAX_CONTENT_BYTES),
    ) {
        let token = token();
        let guard = ReplayGuard::default();
        let _ = parse(&bytes);
        let _ = sequence_body(&bytes);
        let _ = classify(&bytes, &token, NOW, &guard);
        let _ = classify(&with_mac(&bytes, &token), &token, NOW, &guard);
    }

    #[test]
    fn signed_reports_are_accepted_exactly_when_their_command_is_valid(
        (pid, time, program, args, cwd, padded) in candidate_report(),
    ) {
        let token = token();
        let mut keys = format!("pid={pid}:time={time}:argv=");
        let argv: Vec<String> = std::iter::once(program.clone()).chain(args).collect();
        keys.push_str(&argv.iter().map(|arg| encode(arg, padded)).collect::<Vec<_>>().join(","));
        if let Some(cwd) = &cwd {
            keys.push_str(&format!(":cwd={}", encode(cwd, padded)));
        }
        let outcome = classify(&with_mac(keys.as_bytes(), &token), &token, NOW, &ReplayGuard::default());
        let valid = spec_valid_program(&program)
            && argv.iter().all(|arg| !has_spec_control(arg))
            && cwd.as_deref().is_none_or(|cwd| cwd.starts_with('/') && !has_spec_control(cwd));
        match outcome {
            Outcome::Accepted(report) => {
                prop_assert!(valid, "{keys}");
                prop_assert_eq!(report.action, Action::Argv(argv));
                prop_assert_eq!(report.cwd, cwd);
            }
            Outcome::Rejected { owner, .. } => {
                prop_assert!(!valid, "{keys}");
                prop_assert_eq!(owner, Some(pid));
            }
            Outcome::Ignored(reason) => {
                return Err(TestCaseError::fail(format!("{keys}: ignored as {reason:?}")));
            }
        }
    }

    #[test]
    fn generated_valid_reports_are_accepted(keys in valid_keys()) {
        let token = token();
        let outcome = classify(&with_mac(keys.as_bytes(), &token), &token, NOW, &ReplayGuard::default());
        prop_assert!(matches!(outcome, Outcome::Accepted(_)), "{keys}: {outcome:?}");
    }

    #[test]
    fn one_mutated_byte_before_the_mac_is_never_accepted(
        keys in valid_keys(),
        position in any::<prop::sample::Index>(),
        replacement in any::<u8>(),
    ) {
        let token = token();
        let original = with_mac(keys.as_bytes(), &token);
        let index = position.index(keys.len());
        let mut mutated = original.clone();
        mutated[index] = if replacement == original[index] {
            replacement.wrapping_add(1)
        } else {
            replacement
        };
        let outcome = classify(&mutated, &token, NOW, &ReplayGuard::default());
        prop_assert!(!matches!(outcome, Outcome::Accepted(_)), "{outcome:?}");
    }
}
