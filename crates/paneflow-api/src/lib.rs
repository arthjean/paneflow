pub mod descriptor;
#[macro_use]
pub mod methods;
pub mod schema;

pub use descriptor::{Descriptor, Example, Gate, Method, Owner, Reply, Spec};
pub use methods::{Catalog, DuplicateMethod, catalog, declarations};

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
