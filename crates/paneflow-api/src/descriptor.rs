use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Owner {
    Window,
    Worker,
    Host,
}

impl Owner {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Worker => "worker",
            Self::Host => "host",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Open,
    Scripting { refusal: &'static str },
    OrchestrationWhenSpawning,
}

impl Gate {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Open => "none",
            Self::Scripting { .. } => "scripting",
            Self::OrchestrationWhenSpawning => "orchestration",
        }
    }

    pub const fn note(self) -> Option<&'static str> {
        match self {
            Self::Open => None,
            Self::Scripting { .. } => Some(
                "Refused with -32601 unless PANEFLOW_IPC_SCRIPTING=1 or ai_unrestricted is on.",
            ),
            Self::OrchestrationWhenSpawning => Some(
                "A request that sets a command, a prompt or an env is refused with -32601 unless \
                 PANEFLOW_IPC_ORCHESTRATION=1 or PANEFLOW_IPC_SCRIPTING=1.",
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Inline,
    Ready,
    Deferred,
    Async,
    Stream,
}

impl Reply {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Ready => "ready",
            Self::Deferred => "deferred",
            Self::Async => "async",
            Self::Stream => "stream",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spec {
    pub name: &'static str,
    pub summary: &'static str,
    pub owner: Owner,
    pub fallback: Option<Owner>,
    pub gate: Gate,
    pub reply: Reply,
    pub capabilities: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Example {
    pub params: Value,
    pub result: Value,
}

pub trait Method: 'static {
    type Params: DeserializeOwned + JsonSchema;
    type Result: Serialize + DeserializeOwned + JsonSchema;
    const SPEC: Spec;
    fn examples() -> Vec<Example>;
}

#[derive(Clone, Copy)]
pub struct Descriptor {
    pub spec: Spec,
    params: fn() -> Schema,
    result: fn() -> Schema,
    examples: fn() -> Vec<Example>,
}

impl std::fmt::Debug for Descriptor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Descriptor")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}

fn params_schema<T: JsonSchema>() -> Schema {
    SchemaSettings::draft2020_12()
        .for_deserialize()
        .into_generator()
        .into_root_schema_for::<T>()
}

fn result_schema<T: JsonSchema>() -> Schema {
    SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<T>()
}

impl Descriptor {
    pub fn of<M: Method>() -> Self {
        Self {
            spec: M::SPEC,
            params: params_schema::<M::Params>,
            result: result_schema::<M::Result>,
            examples: M::examples,
        }
    }

    pub const fn name(&self) -> &'static str {
        self.spec.name
    }

    pub const fn is_stream(&self) -> bool {
        matches!(self.spec.reply, Reply::Stream)
    }

    pub fn params_schema(&self) -> Value {
        (self.params)().to_value()
    }

    pub fn result_schema(&self) -> Value {
        (self.result)().to_value()
    }

    pub fn examples(&self) -> Vec<Example> {
        (self.examples)()
    }

    pub fn describe_from(&self, callable: bool) -> Value {
        let mut description = self.describe();
        if let Some(fields) = description.as_object_mut() {
            fields.insert("callable".to_string(), Value::Bool(callable));
        }
        description
    }

    pub fn describe(&self) -> Value {
        let spec = &self.spec;
        json!({
            "name": spec.name,
            "summary": spec.summary,
            "owner": spec.owner.label(),
            "fallback": spec.fallback.map(Owner::label),
            "gate": spec.gate.label(),
            "gate_note": spec.gate.note(),
            "reply": spec.reply.label(),
            "stream": self.is_stream(),
            "in_capabilities": spec.capabilities,
            "params": self.params_schema(),
            "result": self.result_schema(),
            "examples": self
                .examples()
                .into_iter()
                .map(|example| json!({"params": example.params, "result": example.result}))
                .collect::<Vec<_>>(),
        })
    }
}
