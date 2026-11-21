use std::borrow::Cow;
use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::descriptor::{Descriptor, Owner};

macro_rules! method {
    (
        $ty:ident {
            name: $name:literal,
            summary: $summary:literal,
            owner: $owner:expr,
            fallback: $fallback:expr,
            gate: $gate:expr,
            reply: $reply:expr,
            $(capabilities: $capabilities:expr,)?
            params: $params:ty,
            result: $result:ty,
            examples: [$(($example_params:tt => $example_result:tt)),+ $(,)?] $(,)?
        }
    ) => {
        pub struct $ty;

        impl $crate::descriptor::Method for $ty {
            type Params = $params;
            type Result = $result;
            const SPEC: $crate::descriptor::Spec = $crate::descriptor::Spec {
                name: $name,
                summary: $summary,
                owner: $owner,
                fallback: $fallback,
                gate: $gate,
                reply: $reply,
                capabilities: method!(@capabilities $($capabilities)?),
            };

            fn examples() -> Vec<$crate::descriptor::Example> {
                vec![$($crate::descriptor::Example {
                    params: serde_json::json!($example_params),
                    result: serde_json::json!($example_result),
                }),+]
            }
        }
    };
    (@capabilities) => {
        false
    };
    (@capabilities $capabilities:expr) => {
        $capabilities
    };
}

pub mod config;
pub mod events;
pub mod host;
pub mod prompt;
pub mod surface;
pub mod system;
pub mod tmux;
pub mod worker;
pub mod workspace;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpaceRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayoutDocument(pub Map<String, Value>);

impl JsonSchema for LayoutDocument {
    fn schema_name() -> Cow<'static, str> {
        "LayoutDocument".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "description": "A layout node as workspace files declare it: {\"type\": \"pane\", \"surfaces\": [...]} or {\"type\": \"split\", \"direction\": \"horizontal\"|\"vertical\", \"ratios\": [...], \"children\": [...]}"
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceProfile {
    Normal,
    Agent,
    Cached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WriteScope {
    Workspace,
    All,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ManagedWorktreeSpec {
    pub path: Option<String>,
    pub repo_root: Option<String>,
    pub branch: Option<String>,
    pub teardown: Option<String>,
}

pub type EnvMap = BTreeMap<String, String>;

pub(crate) const WINDOW: Owner = Owner::Window;
pub(crate) const HOST_FALLBACK: Option<Owner> = Some(Owner::Host);
pub(crate) const NO_FALLBACK: Option<Owner> = None;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateMethod(pub &'static str);

impl std::fmt::Display for DuplicateMethod {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "the method {} is declared twice", self.0)
    }
}

impl std::error::Error for DuplicateMethod {}

#[derive(Debug, Clone)]
pub struct Catalog {
    descriptors: Vec<Descriptor>,
}

impl Catalog {
    pub fn build(descriptors: Vec<Descriptor>) -> Result<Self, DuplicateMethod> {
        let mut seen = std::collections::HashSet::new();
        for descriptor in &descriptors {
            if !seen.insert(descriptor.name()) {
                return Err(DuplicateMethod(descriptor.name()));
            }
        }
        Ok(Self { descriptors })
    }

    pub fn get(&self, name: &str) -> Option<&Descriptor> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.name() == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Descriptor> {
        self.descriptors.iter()
    }

    pub fn owned_by(&self, owner: Owner) -> impl Iterator<Item = &Descriptor> {
        self.descriptors
            .iter()
            .filter(move |descriptor| descriptor.spec.owner == owner)
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.descriptors.iter().map(Descriptor::name).collect()
    }
}

pub fn window_declarations() -> Vec<Descriptor> {
    vec![
        Descriptor::of::<system::Ping>(),
        Descriptor::of::<system::Capabilities>(),
        Descriptor::of::<system::Identify>(),
        Descriptor::of::<system::Counters>(),
        Descriptor::of::<workspace::List>(),
        Descriptor::of::<workspace::Create>(),
        Descriptor::of::<workspace::Select>(),
        Descriptor::of::<workspace::Close>(),
        Descriptor::of::<workspace::Current>(),
        Descriptor::of::<workspace::RestoreLayout>(),
        Descriptor::of::<workspace::Up>(),
        Descriptor::of::<surface::List>(),
        Descriptor::of::<surface::Read>(),
        Descriptor::of::<surface::Search>(),
        Descriptor::of::<surface::Rename>(),
        Descriptor::of::<surface::SendText>(),
        Descriptor::of::<surface::SendKeystroke>(),
        Descriptor::of::<surface::Split>(),
        Descriptor::of::<surface::Focus>(),
        Descriptor::of::<surface::Status>(),
        Descriptor::of::<surface::FleetList>(),
        Descriptor::of::<events::Subscribe>(),
        Descriptor::of::<tmux::Compat>(),
        Descriptor::of::<config::ListBuiltinActions>(),
        Descriptor::of::<config::DispatchAction>(),
        Descriptor::of::<system::Describe>(),
    ]
}

pub fn host_declarations() -> Vec<Descriptor> {
    vec![Descriptor::of::<host::PaneWrite>()]
}

pub fn worker_declarations() -> Vec<Descriptor> {
    vec![
        Descriptor::of::<worker::Hello>(),
        Descriptor::of::<worker::Status>(),
        Descriptor::of::<worker::Shutdown>(),
        Descriptor::of::<worker::Snapshot>(),
        Descriptor::of::<worker::Follow>(),
        Descriptor::of::<worker::ActivityLogMethod>(),
        Descriptor::of::<worker::Acknowledge>(),
        Descriptor::of::<worker::UsageSnapshotMethod>(),
        Descriptor::of::<worker::UsageRefresh>(),
        Descriptor::of::<worker::UsageImport>(),
        Descriptor::of::<events::Follow>(),
        Descriptor::of::<events::Publish>(),
        Descriptor::of::<config::Status>(),
        Descriptor::of::<config::Reload>(),
        Descriptor::of::<config::ListActions>(),
        Descriptor::of::<config::InvokeAction>(),
        Descriptor::of::<config::ListKeymap>(),
        Descriptor::of::<prompt::Request>(),
        Descriptor::of::<prompt::Wait>(),
        Descriptor::of::<prompt::ReplyTo>(),
        Descriptor::of::<prompt::Follow>(),
    ]
}

pub fn declarations() -> Vec<Descriptor> {
    let mut all = window_declarations();
    all.extend(host_declarations());
    all.extend(worker_declarations());
    all
}

pub fn catalog() -> Result<Catalog, DuplicateMethod> {
    Catalog::build(declarations())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{META_SCHEMA, unsupported_keywords, validate};

    #[test]
    fn two_declarations_with_one_name_are_refused() {
        let mut descriptors = declarations();
        descriptors.push(Descriptor::of::<system::Ping>());
        let error = Catalog::build(descriptors).expect_err("a second system.ping is refused");
        assert_eq!(error, DuplicateMethod("system.ping"));
    }

    #[test]
    fn the_shipped_catalog_builds() {
        let catalog = catalog().expect("no method is declared twice");
        assert!(catalog.get("surface.read").is_some());
        assert!(catalog.get("surface.unknown").is_none());
    }

    #[test]
    fn every_schema_is_draft_2020_12_and_uses_keywords_the_validator_reads() {
        for descriptor in declarations() {
            for schema in [descriptor.params_schema(), descriptor.result_schema()] {
                assert_eq!(schema["$schema"], META_SCHEMA, "{}", descriptor.name());
                assert_eq!(
                    unsupported_keywords(&schema),
                    Vec::<String>::new(),
                    "{}",
                    descriptor.name()
                );
            }
        }
    }

    #[test]
    fn every_method_validates_its_own_examples() {
        for descriptor in declarations() {
            let examples = descriptor.examples();
            assert!(!examples.is_empty(), "{} has no example", descriptor.name());
            for example in examples {
                let params = validate(&descriptor.params_schema(), &example.params);
                assert_eq!(params, Ok(()), "{} example params", descriptor.name());
                let result = validate(&descriptor.result_schema(), &example.result);
                assert_eq!(result, Ok(()), "{} example result", descriptor.name());
            }
        }
    }

    #[test]
    fn every_declaration_carries_a_summary() {
        for descriptor in declarations() {
            assert!(!descriptor.spec.summary.is_empty(), "{}", descriptor.name());
        }
    }
}
