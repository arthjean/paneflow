#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use paneflow_api::methods::prompt::{PromptAccess, PromptAsker, prompt_access};
use paneflow_api::policy::{Policy, Section, Switches};
use paneflow_api::{Principal, authorize};
use serde_json::json;

const TABLE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/decision_table.txt");

fn principals() -> Vec<Principal> {
    vec![
        Principal::Window,
        Principal::Worker,
        Principal::Pane {
            session: "s-caller".into(),
            workspace: Some("w-caller".into()),
        },
        Principal::External,
        Principal::Unverified {
            reason: "the peer exited".into(),
        },
    ]
}

fn policies() -> [(&'static str, Policy); 2] {
    [
        ("closed", Policy::default()),
        (
            "open",
            Policy {
                section: Section::Set(Switches::OPEN),
                ..Policy::default()
            },
        ),
    ]
}

pub fn rendered_table() -> String {
    let catalog = paneflow_api::host_catalog().expect("the catalog builds");
    let mut lines = vec![
        "# method owner principal closed open (params: {\"scope_session\": \"s-claimed\"})"
            .to_string(),
    ];
    let params = json!({"scope_session": "s-claimed"});
    for descriptor in catalog.iter() {
        for principal in principals() {
            let decisions: Vec<String> = policies()
                .iter()
                .map(|(_, policy)| authorize(&principal, descriptor, &params, policy).label())
                .collect();
            lines.push(format!(
                "{} {} {} {}",
                descriptor.name(),
                descriptor.spec.owner.label(),
                principal.kind(),
                decisions.join(" ")
            ));
        }
    }
    lines.push(
        "# prompt.follow and prompt.reply against the prompt's asker: method principal asker decision"
            .to_string(),
    );
    for method in ["prompt.follow", "prompt.reply"] {
        for principal in principals() {
            for asker in PromptAsker::ALL {
                let access = prompt_access(&principal, asker);
                let decision = match (method, access) {
                    ("prompt.follow", PromptAccess::Hidden) => "hidden",
                    ("prompt.follow", _) => "shown_with_a_token",
                    (_, PromptAccess::Hidden) => "refused:not_pending",
                    (_, PromptAccess::AnswersWithItsToken) => "allowed:with_its_token",
                    (_, PromptAccess::Answers) => "allowed",
                };
                lines.push(format!(
                    "{method} {} {} {decision}",
                    principal.kind(),
                    asker.label()
                ));
            }
        }
    }
    lines.join("\n") + "\n"
}

#[test]
fn every_method_and_principal_decides_as_the_checked_in_table_says() {
    let rendered = rendered_table();
    if std::env::var("PANEFLOW_DECISION_TABLE").as_deref() == Ok("write") {
        std::fs::write(TABLE_PATH, &rendered).expect("the table is writable");
    }
    let checked_in = std::fs::read_to_string(TABLE_PATH).unwrap_or_default();
    assert!(
        checked_in == rendered,
        "crates/paneflow-api/tests/decision_table.txt differs from what authorize decides. \
         If the change is intended, regenerate it with \
         `PANEFLOW_DECISION_TABLE=write cargo test -p paneflow-api --test decision_table` and review its diff."
    );
}
