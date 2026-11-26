use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::parse::{Action, Report};
use crate::token::Token;
use crate::verify::sign;

pub fn signed_body(report: &Report, token: &Token) -> String {
    let mut keys = format!("pid={}:time={}", report.pid, report.time);
    match &report.action {
        Action::Argv(argv) => {
            let encoded: Vec<String> = argv
                .iter()
                .map(|element| STANDARD.encode(element.as_bytes()))
                .collect();
            keys.push_str(":argv=");
            keys.push_str(&encoded.join(","));
        }
        Action::Clear => keys.push_str(":clear=1"),
    }
    if let Some(cwd) = &report.cwd {
        keys.push_str(":cwd=");
        keys.push_str(&STANDARD.encode(cwd.as_bytes()));
    }
    let mac = sign(keys.as_bytes(), token);
    format!("{keys}:mac={mac}")
}

pub fn osc_sequence(body: &str) -> String {
    format!("\x1b]7577;{body}\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::{Outcome, ReplayGuard, classify};

    const NOW: u64 = 1_795_543_200;

    fn token() -> Token {
        Token::new("dummy-token-replace-me").expect("the specification token is valid")
    }

    #[test]
    fn the_encoder_reproduces_the_specification_neovim_report() {
        let report = Report {
            pid: 74877,
            time: NOW,
            action: Action::Argv(vec![
                "nvim".into(),
                "-S".into(),
                "/home/user/.local/state/nvim/sessions/8f2c1d.vim".into(),
            ]),
            cwd: None,
        };
        assert_eq!(
            signed_body(&report, &token()),
            "pid=74877:time=1795543200:argv=bnZpbQ==,LVM=,L2hvbWUvdXNlci8ubG9jYWwvc3RhdGUvbnZpbS9zZXNzaW9ucy84ZjJjMWQudmlt:mac=9dbb99bfb856749a87563839d7ce9062818d78cf25909db93f309e0cd3b1909b"
        );
    }

    #[test]
    fn an_encoded_report_with_a_cwd_and_a_clear_classifies_back_to_itself() {
        for report in [
            Report {
                pid: 42,
                time: NOW,
                action: Action::Argv(vec!["lazygit".into(), String::new(), "two words".into()]),
                cwd: Some("/home/user/src/project".into()),
            },
            Report {
                pid: 42,
                time: NOW,
                action: Action::Clear,
                cwd: None,
            },
        ] {
            let body = signed_body(&report, &token());
            assert_eq!(
                classify(body.as_bytes(), &token(), NOW, &ReplayGuard::default()),
                Outcome::Accepted(report)
            );
        }
    }

    #[test]
    fn the_sequence_wraps_the_body_in_osc_7577_ended_by_st() {
        assert_eq!(osc_sequence("pid=1"), "\x1b]7577;pid=1\x1b\\");
    }
}
