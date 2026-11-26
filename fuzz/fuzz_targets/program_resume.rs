#![no_main]

use libfuzzer_sys::fuzz_target;
use paneflow_program_resume::{
    Action, Admission, CwdSource, Outcome, RecordCwd, RecordStore, Token, classify, parse,
    sequence_body, sign,
};

const NOW_SECS: u64 = 1_795_543_200;

fuzz_target!(|data: &[u8]| {
    let token = Token::new("dummy-token-replace-me").expect("the specification token is valid");
    let _ = parse(data);
    let _ = sequence_body(data);
    let mut store = RecordStore::new();
    let unsigned = classify(data, &token, NOW_SECS, store.replay_guard());
    let _ = store.apply(unsigned, |_| None, NOW_SECS * 1000);

    let mut signed = data.to_vec();
    signed.extend_from_slice(b":mac=");
    signed.extend_from_slice(sign(data, &token).as_bytes());
    let outcome = classify(&signed, &token, NOW_SECS, store.replay_guard());
    if let Outcome::Accepted(report) = &outcome {
        assert!(report.pid > 0);
        assert!(report.time.abs_diff(NOW_SECS) <= 60);
        if let Action::Argv(argv) = &report.action {
            let program = argv.first().expect("an accepted argv has a program");
            assert!(!program.is_empty() && !program.starts_with('-'));
            assert!(!program.contains('/') || program.starts_with('/'));
            assert!(argv.iter().all(|arg| !arg.chars().any(char::is_control)));
        }
        assert!(report.cwd.as_deref().is_none_or(|cwd| cwd.starts_with('/')));
    }
    let admission = Admission {
        started_at: 1,
        ancestors: Vec::new(),
        first_process: false,
        fallback_cwd: RecordCwd {
            path: "/".to_owned(),
            source: CwdSource::Spawn,
        },
        sandbox: None,
    };
    let _ = store.apply(outcome, |_| Some(admission), NOW_SECS * 1000);
    assert!(store.records().len() <= 1);
});
