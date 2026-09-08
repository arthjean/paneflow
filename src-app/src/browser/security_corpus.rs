#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof {
    Automated(&'static str),
    Native(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub struct Case {
    pub id: &'static str,
    pub negative_input: &'static str,
    pub required_result: &'static str,
    pub entry_point: &'static str,
    pub proof: Proof,
}

pub const CORPUS: [Case; 8] = [
    Case {
        id: "SEC-01",
        negative_input: "a document asking the host for a private endpoint",
        required_result: "no command runs and no capability is exposed",
        entry_point: "paneflow-browser-host qualification::fixture_report, the only page-to-host text surface",
        proof: Proof::Automated(
            "paneflow-browser-host page_reports_are_bounded_before_conversion_and_have_a_fixed_schema",
        ),
    },
    Case {
        id: "SEC-02",
        negative_input: "the same cookie written in two workspace profiles",
        required_result: "each profile reads back its own value",
        entry_point: "browser::profile::ProfileStore and the per-profile host",
        proof: Proof::Native("two live workspaces on a qualified runtime"),
    },
    Case {
        id: "SEC-03",
        negative_input: "a top-level redirect to a forbidden scheme",
        required_result: "refused per P1, with no OS execution",
        entry_point: "browser::origin_of and browser::address normalization",
        proof: Proof::Automated(
            "forbidden_schemes_are_refused_before_any_navigation_or_os_handoff",
        ),
    },
    Case {
        id: "SEC-04",
        negative_input: "an invalid certificate or an origin change during a permission prompt",
        required_result: "no permission is granted to the new document",
        entry_point: "the host permission handler and its document generation",
        proof: Proof::Native("a live navigation against an invalid certificate"),
    },
    Case {
        id: "SEC-05",
        negative_input: "a gestureless popup trying to cover a GPUI overlay",
        required_result: "the popup is blocked and the overlay keeps priority",
        entry_point: "the host popup handler and the dock overlay order",
        proof: Proof::Native("a rendered session with an open overlay"),
    },
    Case {
        id: "SEC-06",
        negative_input: "a truncated, oversized or uncapable control message",
        required_result: "a bounded error, with no engine access",
        entry_point: "paneflow_browser_protocol::wire decoding",
        proof: Proof::Automated("malformed_control_messages_are_refused_within_their_bound"),
    },
    Case {
        id: "SEC-07",
        negative_input: "a callback from a closed browser aimed at a new generation",
        required_result: "the event is rejected and the new page is unchanged",
        entry_point: "paneflow_browser_protocol::Controller::dispatch",
        proof: Proof::Automated("a_callback_for_a_closed_browser_never_reaches_a_new_generation"),
    },
    Case {
        id: "SEC-08",
        negative_input: "a locked profile or an invalid runtime checksum",
        required_result: "no concurrent opening and no execution of the invalid runtime",
        entry_point: "browser::profile::ProfileStore::open and browser::supervisor::verify_runtime",
        proof: Proof::Automated("a_locked_profile_and_a_foreign_runtime_stamp_are_both_refused"),
    },
];

pub fn report() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "release": "R2",
        "cases": CORPUS
            .iter()
            .map(|case| {
                let (proof, detail) = match case.proof {
                    Proof::Automated(name) => ("automated", name),
                    Proof::Native(requirement) => ("native", requirement),
                };
                serde_json::json!({
                    "id": case.id,
                    "negative_input": case.negative_input,
                    "required_result": case.required_result,
                    "entry_point": case.entry_point,
                    "proof": proof,
                    "detail": detail,
                })
            })
            .collect::<Vec<_>>(),
        "automated": CORPUS.iter().filter(|case| matches!(case.proof, Proof::Automated(_))).count(),
        "native": CORPUS.iter().filter(|case| matches!(case.proof, Proof::Native(_))).count(),
    })
}

pub fn run() -> i32 {
    match serde_json::to_string_pretty(&report()) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(error) => {
            eprintln!("security corpus: {error}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use paneflow_browser_protocol::{
        BrowserError, BrowserId, CONTRACT_VERSION, Command, Controller, Document, Envelope, Event,
        MAX_MESSAGE_BYTES, OperationId, Owner, ProfileId, SessionId, WorkspaceId,
    };

    use super::*;
    use crate::browser::profile::{ProfileError, ProfileStore};
    use crate::browser::supervisor::{RuntimeCheck, manifest_digest, verify_runtime};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "paneflow-browser-sec-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|moment| moment.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn owner(workspace: &str) -> Owner {
        Owner {
            workspace: WorkspaceId::try_from(workspace.to_string()).unwrap(),
            session: SessionId::try_from("tab-1".to_string()).unwrap(),
        }
    }

    fn envelope(sequence: u64, command: Command) -> Envelope {
        Envelope {
            version: CONTRACT_VERSION,
            operation: OperationId::try_from(format!("op-{sequence}")).unwrap(),
            command,
        }
    }

    #[test]
    fn the_corpus_covers_the_eight_release_cases_and_names_a_proof_for_each() {
        let ids: Vec<_> = CORPUS.iter().map(|case| case.id).collect();
        assert_eq!(
            ids,
            [
                "SEC-01", "SEC-02", "SEC-03", "SEC-04", "SEC-05", "SEC-06", "SEC-07", "SEC-08"
            ]
        );
        for case in CORPUS {
            let detail = match case.proof {
                Proof::Automated(name) => name,
                Proof::Native(requirement) => requirement,
            };
            assert!(
                !detail.is_empty() && !case.entry_point.is_empty(),
                "{}",
                case.id
            );
        }
        let value = report();
        assert_eq!(value["automated"], 5);
        assert_eq!(value["native"], 3);
        assert_eq!(value["cases"].as_array().map(Vec::len), Some(8));
    }

    #[test]
    fn forbidden_schemes_are_refused_before_any_navigation_or_os_handoff() {
        for url in [
            "file:///etc/passwd",
            "javascript:fetch('/')",
            "data:text/html,<script>1</script>",
            "chrome://settings",
            "http://user:token@example.org/",
        ] {
            assert!(
                crate::browser::origin_of(url).is_err(),
                "{url} must not resolve to a navigable origin"
            );
        }
        assert_eq!(
            crate::browser::origin_of("http://127.0.0.1:8080/path?query#fragment"),
            Ok("http://127.0.0.1:8080".to_string())
        );
    }

    #[test]
    fn malformed_control_messages_are_refused_within_their_bound() {
        let mut controller = Controller::new("test".to_string(), true);
        let scope = owner("ws-1");
        let oversized = controller.dispatch(
            &scope,
            envelope(
                1,
                Command::Create {
                    owner: scope.clone(),
                    browser: BrowserId::try_from("b-1".to_string()).unwrap(),
                    profile: ProfileId::try_from("p-1".to_string()).unwrap(),
                    url: "x".repeat(MAX_MESSAGE_BYTES),
                    title: String::new(),
                },
            ),
        );
        assert_eq!(oversized.result.err(), Some(BrowserError::TooLarge));

        let stale_contract = controller.dispatch(
            &scope,
            Envelope {
                version: CONTRACT_VERSION - 1,
                operation: OperationId::try_from("op-2".to_string()).unwrap(),
                command: Command::Capabilities,
            },
        );
        assert_eq!(
            stale_contract.result.err(),
            Some(BrowserError::IncompatibleVersion),
            "a message from another contract version must never reach the engine"
        );

        let unknown = controller.dispatch(
            &scope,
            envelope(
                3,
                Command::Close {
                    document: Document {
                        owner: scope.clone(),
                        browser: BrowserId::try_from("b-absent".to_string()).unwrap(),
                        generation: 1,
                    },
                },
            ),
        );
        assert_eq!(unknown.result.err(), Some(BrowserError::UnknownIdentity));
    }

    #[test]
    fn a_callback_for_a_closed_browser_never_reaches_a_new_generation() {
        let mut controller = Controller::new("test".to_string(), true);
        let scope = owner("ws-1");
        let profile = ProfileId::try_from("p-1".to_string()).unwrap();
        let create = |controller: &mut Controller, sequence: u64, id: &str| {
            let reply = controller.dispatch(
                &scope,
                envelope(
                    sequence,
                    Command::Create {
                        owner: scope.clone(),
                        browser: BrowserId::try_from(id.to_string()).unwrap(),
                        profile: profile.clone(),
                        url: paneflow_browser_protocol::BLANK_URL.to_string(),
                        title: String::new(),
                    },
                ),
            );
            match reply.result {
                Ok(Event::State { session }) => session.document,
                other => panic!("create must open a session: {other:?}"),
            }
        };

        let first = create(&mut controller, 1, "b-1");
        assert!(
            controller
                .dispatch(
                    &scope,
                    envelope(
                        2,
                        Command::Close {
                            document: first.clone()
                        }
                    )
                )
                .result
                .is_ok()
        );
        let second = create(&mut controller, 3, "b-2");
        assert_ne!(second.generation, first.generation);

        let replayed = controller.dispatch(&scope, envelope(4, Command::Close { document: first }));
        assert_eq!(
            replayed.result.err(),
            Some(BrowserError::UnknownIdentity),
            "a callback naming the closed browser must not be honored"
        );

        let stale = controller.dispatch(
            &scope,
            envelope(
                5,
                Command::Close {
                    document: Document {
                        generation: second.generation.saturating_sub(1),
                        ..second.clone()
                    },
                },
            ),
        );
        assert_eq!(
            stale.result.err(),
            Some(BrowserError::StaleGeneration),
            "a stale generation must not act on the new page"
        );

        let foreign = controller.dispatch(
            &owner("ws-2"),
            envelope(6, Command::Close { document: second }),
        );
        assert_eq!(
            foreign.result.err(),
            Some(BrowserError::AccessDenied),
            "another workspace must not reach this page"
        );
    }

    #[test]
    fn a_locked_profile_and_a_foreign_runtime_stamp_are_both_refused() {
        let root = scratch("lock");
        let owner = ProfileStore::open(root.clone()).unwrap();
        assert_eq!(
            ProfileStore::open(root.clone()).err(),
            Some(ProfileError::InUse),
            "a second instance must not open the same browser root"
        );
        drop(owner);

        let runtime = scratch("runtime");
        std::fs::create_dir_all(runtime.join("Release")).unwrap();
        std::fs::create_dir_all(runtime.join("Resources/locales")).unwrap();
        for name in [
            "Release/libcef.so",
            "Resources/icudtl.dat",
            "Resources/locales/en-US.pak",
        ] {
            std::fs::write(runtime.join(name), b"stub").unwrap();
        }
        assert!(
            verify_runtime(&runtime, RuntimeCheck::StampOnly).is_err(),
            "a runtime without a verification stamp must not be executed"
        );
        std::fs::write(
            runtime.join("verified-manifest.sha256"),
            format!("{}\n", "0".repeat(64)),
        )
        .unwrap();
        assert!(
            verify_runtime(&runtime, RuntimeCheck::StampOnly).is_err(),
            "a runtime verified against another manifest must not be executed"
        );
        std::fs::write(
            runtime.join("verified-manifest.sha256"),
            format!("{}\n", manifest_digest()),
        )
        .unwrap();
        assert!(verify_runtime(&runtime, RuntimeCheck::StampOnly).is_ok());
        assert!(
            verify_runtime(&runtime, RuntimeCheck::Manifest).is_err(),
            "stub bytes must not satisfy the pinned digests"
        );

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(runtime);
    }
}
