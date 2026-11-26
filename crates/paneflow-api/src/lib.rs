#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod authorize;
pub mod descriptor;
pub mod grants;
#[macro_use]
pub mod methods;
pub mod policy;
pub mod principal;
pub mod schema;

pub use authorize::{Decision, Refusal, Scope, authorize, authorize_with};
pub use descriptor::{Audience, Descriptor, Example, Gate, Method, Owner, Reply, ScopeRule, Spec};
pub use methods::{Catalog, DuplicateMethod, catalog, declarations, host_catalog};
pub use policy::{Gates, Policy};
pub use principal::Principal;

pub const SNAPSHOT_VERSION: u32 = 1;

fn with_sorted_keys(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> = map.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, with_sorted_keys(value)))
                    .collect(),
            )
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(with_sorted_keys).collect())
        }
        other => other,
    }
}

pub fn snapshot() -> Result<serde_json::Value, DuplicateMethod> {
    let catalog = catalog()?;
    Ok(with_sorted_keys(serde_json::json!({
        "api": SNAPSHOT_VERSION,
        "description": "Every Paneflow method, generated from crates/paneflow-api. A test fails when this file and the declarations differ; regenerate it with PANEFLOW_API_SNAPSHOT=write cargo test -p paneflow-api snapshot.",
        "methods": catalog.iter().map(Descriptor::describe).collect::<Vec<_>>(),
    })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_registry_crate_links_no_ui_no_lua_and_no_app_module() {
        let manifest = include_str!("../Cargo.toml");
        let dependencies = manifest
            .split("[dependencies]")
            .nth(1)
            .and_then(|rest| rest.split("\n[").next())
            .expect("a dependency table");
        for banned in ["gpui", "mlua", "paneflow-app"] {
            assert!(
                !dependencies
                    .lines()
                    .any(|line| line.trim_start().starts_with(banned)),
                "{banned} in {dependencies}"
            );
        }
    }
}

#[cfg(test)]
mod audience_tests {
    const MACRO: &str = include_str!("methods/mod.rs");
    const DESCRIPTOR: &str = include_str!("descriptor.rs");

    #[test]
    fn a_declaration_must_state_its_audience_to_compile() {
        let rule = MACRO
            .split("macro_rules! method")
            .nth(1)
            .and_then(|rest| rest.split("=> {").next())
            .expect("the method! rule");
        let line = rule
            .lines()
            .find(|line| line.contains("audience:"))
            .expect("the rule names an audience");
        assert_eq!(line.trim(), "audience: $audience:expr,");
        assert!(
            !rule.contains("$(audience"),
            "the audience is not optional: {rule}"
        );
        let spec = DESCRIPTOR
            .split("pub struct Spec")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("the Spec struct");
        assert!(spec.contains("pub audience: Audience,"), "{spec}");
        let derives = DESCRIPTOR
            .split("pub struct Spec")
            .next()
            .and_then(|before| before.lines().rev().find(|line| line.contains("derive")))
            .expect("the derive line of Spec");
        assert!(!derives.contains("Default"), "{derives}");
    }

    #[test]
    fn describe_lists_the_audience_and_the_gate_of_every_method() {
        let catalog = super::host_catalog().expect("the catalog builds");
        for descriptor in catalog.iter() {
            let described = descriptor.describe();
            assert!(described["audience"].is_string(), "{}", descriptor.name());
            assert!(described["gate"].is_string(), "{}", descriptor.name());
            assert!(described["scope"].is_string(), "{}", descriptor.name());
        }
        let audience = |name: &str| catalog.get(name).expect(name).spec.audience;
        for name in [
            "layout.launch",
            "layout.sweep",
            "approval.follow",
            "approval.decide",
            "session.runtime.bind",
            "host.shutdown",
            "session.workspace",
            "session.prepare_resume",
            "session.discard_resume",
        ] {
            assert_eq!(audience(name), super::Audience::PaneflowOnly, "{name}");
        }
        assert_eq!(
            audience("session.resume_attempt"),
            super::Audience::PaneOnly
        );
        for name in ["layout.attach", "peer.classify"] {
            assert_eq!(audience(name), super::Audience::NotPane, "{name}");
        }
        for name in [
            "session.input",
            "session.stop",
            "session.restart",
            "session.remove",
            "session.reset",
            "session.clear_history",
            "session.resize",
            "session.appearance",
        ] {
            assert_eq!(
                catalog.get(name).expect(name).spec.scope,
                super::ScopeRule::Workspace,
                "{name}"
            );
        }
    }

    #[test]
    fn every_method_the_host_dispatches_has_a_descriptor() {
        let catalog = super::host_catalog().expect("the catalog builds");
        for name in [
            "system.whoami",
            "peer.classify",
            "session.attach",
            "session.output",
            "approval.follow",
            "approval.decide",
            "agent.follow",
            "host.shutdown",
            "agent.event",
            "host.hello",
            "host.status",
            "session.list",
            "session.create",
            "session.inspect",
            "session.stop",
            "session.restart",
            "session.prepare_resume",
            "session.discard_resume",
            "session.resume_attempt",
            "session.remove",
            "session.input",
            "session.runtime.bind",
            "session.resize",
            "session.appearance",
            "session.workspace",
            "session.clear_history",
            "session.reset",
            "session.text",
            "agent.snapshot",
            "pane.write",
        ] {
            assert!(catalog.get(name).is_some(), "{name}");
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    const SNAPSHOT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../protocol/api-v1.json");

    #[test]
    fn the_checked_in_snapshot_matches_the_declarations() {
        let generated = super::snapshot().expect("the catalog builds");
        let rendered = format!(
            "{}\n",
            serde_json::to_string_pretty(&generated).expect("the snapshot renders")
        );
        if std::env::var("PANEFLOW_API_SNAPSHOT").as_deref() == Ok("write") {
            std::fs::write(SNAPSHOT_PATH, &rendered).expect("protocol/api-v1.json is writable");
        }
        let checked_in = std::fs::read_to_string(SNAPSHOT_PATH).unwrap_or_default();
        assert!(
            checked_in == rendered,
            "protocol/api-v1.json differs from the method declarations in crates/paneflow-api. \
             If the API change is intended, regenerate the file with \
             `PANEFLOW_API_SNAPSHOT=write cargo test -p paneflow-api snapshot` and review its diff."
        );
    }
}
