use std::sync::{LazyLock, OnceLock};

use gpui::Context;
use paneflow_api::{Descriptor, Gate, Method, Owner, Reply, Spec};
use serde_json::Value;

use super::{IpcReply, JsonRpcError, ipc_orchestration_enabled, ipc_scripting_enabled};
use crate::PaneFlowApp;

pub(crate) struct Call<'a, P> {
    pub(crate) params: P,
    pub(crate) raw: &'a Value,
    pub(crate) caller_pid: Option<i64>,
}

type AppHandler = Box<
    dyn Fn(&mut PaneFlowApp, &Value, Option<i64>, &mut Context<PaneFlowApp>) -> IpcReply
        + Send
        + Sync,
>;

type InlineHandler = Box<dyn Fn(&Value) -> Value + Send + Sync>;

enum Serve {
    Inline(InlineHandler),
    App(AppHandler),
    Stream,
}

pub(crate) struct Entry {
    descriptor: Descriptor,
    schema: OnceLock<Value>,
    serve: Serve,
}

pub(crate) enum Route<'a> {
    Inline(&'a Entry),
    Stream(&'a Entry),
    App,
}

impl Entry {
    pub(crate) fn spec(&self) -> &Spec {
        &self.descriptor.spec
    }

    pub(crate) fn validate(&self, params: &Value) -> Result<(), JsonRpcError> {
        let schema = self.schema.get_or_init(|| self.descriptor.params_schema());
        paneflow_api::schema::validate(schema, params)
            .map_err(|violation| JsonRpcError::invalid_params(violation.message))
    }

    pub(crate) fn answer_inline(&self, params: &Value) -> Value {
        if let Err(error) = self.validate(params) {
            return error.into_value();
        }
        match &self.serve {
            Serve::Inline(answer) => answer(params),
            Serve::App(_) | Serve::Stream => JsonRpcError::method_not_found(format!(
                "{} is not answered inline",
                self.spec().name
            ))
            .into_value(),
        }
    }
}

fn decode<P: serde::de::DeserializeOwned>(raw: &Value) -> Result<P, JsonRpcError> {
    P::deserialize(raw)
        .map_err(|error| JsonRpcError::invalid_params(format!("invalid params: {error}")))
}

pub(crate) fn gate_refusal(gate: Gate, ai_unrestricted: bool) -> Option<JsonRpcError> {
    match gate {
        Gate::Scripting { refusal }
            if !super::send_text_gate_open(ipc_scripting_enabled(), ai_unrestricted) =>
        {
            Some(JsonRpcError::method_not_enabled(refusal))
        }
        Gate::Open | Gate::Scripting { .. } | Gate::OrchestrationWhenSpawning => None,
    }
}

pub(crate) fn spawn_gate_open(spec: &Spec) -> bool {
    !matches!(spec.gate, Gate::OrchestrationWhenSpawning) || ipc_orchestration_enabled()
}

pub(crate) fn spawn_gate_refusal(spec: &Spec) -> JsonRpcError {
    super::orchestration_disabled_error(spec.name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RegistryError {
    Undeclared(&'static str),
    Duplicate(&'static str),
    WrongReply {
        method: &'static str,
        declared: Reply,
        registered: Reply,
    },
    Unserved(&'static str),
    GatedOffTheAppThread(&'static str),
}

pub(crate) struct Registry {
    entries: Vec<Entry>,
}

#[derive(Default)]
pub(crate) struct RegistryBuilder {
    entries: Vec<(Entry, Reply)>,
}

impl RegistryBuilder {
    fn push<M: Method>(mut self, registered: Reply, serve: Serve) -> Self {
        self.entries.push((
            Entry {
                descriptor: Descriptor::of::<M>(),
                schema: OnceLock::new(),
                serve,
            },
            registered,
        ));
        self
    }

    pub(crate) fn inline<M: Method>(self, answer: fn(Call<'_, M::Params>) -> Value) -> Self {
        self.push::<M>(
            Reply::Inline,
            Serve::Inline(Box::new(move |raw| match decode::<M::Params>(raw) {
                Ok(params) => answer(Call {
                    params,
                    raw,
                    caller_pid: None,
                }),
                Err(error) => error.into_value(),
            })),
        )
    }

    fn app<M: Method>(
        self,
        registered: Reply,
        handler: fn(&mut PaneFlowApp, Call<'_, M::Params>, &mut Context<PaneFlowApp>) -> IpcReply,
    ) -> Self {
        self.push::<M>(
            registered,
            Serve::App(Box::new(move |app, raw, caller_pid, cx| {
                match decode::<M::Params>(raw) {
                    Ok(params) => handler(
                        app,
                        Call {
                            params,
                            raw,
                            caller_pid,
                        },
                        cx,
                    ),
                    Err(error) => IpcReply::Ready(error.into_value()),
                }
            })),
        )
    }

    pub(crate) fn ready<M: Method>(
        self,
        handler: fn(&mut PaneFlowApp, Call<'_, M::Params>, &mut Context<PaneFlowApp>) -> Value,
    ) -> Self {
        self.push::<M>(
            Reply::Ready,
            Serve::App(Box::new(move |app, raw, caller_pid, cx| {
                IpcReply::Ready(match decode::<M::Params>(raw) {
                    Ok(params) => handler(
                        app,
                        Call {
                            params,
                            raw,
                            caller_pid,
                        },
                        cx,
                    ),
                    Err(error) => error.into_value(),
                })
            })),
        )
    }

    pub(crate) fn deferred<M: Method>(
        self,
        handler: fn(&mut PaneFlowApp, Call<'_, M::Params>, &mut Context<PaneFlowApp>) -> IpcReply,
    ) -> Self {
        self.app::<M>(Reply::Deferred, handler)
    }

    pub(crate) fn asynchronous<M: Method>(
        self,
        handler: fn(&mut PaneFlowApp, Call<'_, M::Params>, &mut Context<PaneFlowApp>) -> IpcReply,
    ) -> Self {
        self.app::<M>(Reply::Async, handler)
    }

    pub(crate) fn stream<M: Method>(self) -> Self {
        self.push::<M>(Reply::Stream, Serve::Stream)
    }

    pub(crate) fn build(self) -> Result<Registry, RegistryError> {
        let catalog =
            paneflow_api::catalog().map_err(|duplicate| RegistryError::Duplicate(duplicate.0))?;
        let mut entries: Vec<Entry> = Vec::with_capacity(self.entries.len());
        for (entry, registered) in self.entries {
            let name = entry.spec().name;
            if catalog.get(name).is_none() {
                return Err(RegistryError::Undeclared(name));
            }
            if entries.iter().any(|held| held.spec().name == name) {
                return Err(RegistryError::Duplicate(name));
            }
            let gate_is_checked = matches!(entry.serve, Serve::App(_));
            if !gate_is_checked && entry.spec().gate != Gate::Open {
                return Err(RegistryError::GatedOffTheAppThread(name));
            }
            if entry.spec().reply != registered {
                return Err(RegistryError::WrongReply {
                    method: name,
                    declared: entry.spec().reply,
                    registered,
                });
            }
            entries.push(entry);
        }
        let order = catalog.names();
        for name in catalog.owned_by(Owner::Window).map(Descriptor::name) {
            if !entries.iter().any(|entry| entry.spec().name == name) {
                return Err(RegistryError::Unserved(name));
            }
        }
        entries.sort_by_key(|entry| order.iter().position(|name| *name == entry.spec().name));
        Ok(Registry { entries })
    }
}

impl Registry {
    pub(crate) fn builder() -> RegistryBuilder {
        RegistryBuilder::default()
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.spec().name == name)
    }

    pub(crate) fn route(&self, name: &str) -> Route<'_> {
        match self.get(name) {
            Some(entry) => match entry.serve {
                Serve::Inline(_) => Route::Inline(entry),
                Serve::Stream => Route::Stream(entry),
                Serve::App(_) => Route::App,
            },
            None => Route::App,
        }
    }

    #[cfg(test)]
    pub(crate) fn method_names(&self) -> Vec<&'static str> {
        self.entries.iter().map(|entry| entry.spec().name).collect()
    }

    pub(crate) fn capability_names(&self) -> Vec<&'static str> {
        self.entries
            .iter()
            .filter(|entry| entry.spec().capabilities)
            .map(|entry| entry.spec().name)
            .collect()
    }

    pub(crate) fn describe(&self, method: Option<&str>) -> Value {
        let catalog = match paneflow_api::catalog() {
            Ok(catalog) => catalog,
            Err(duplicate) => {
                return JsonRpcError::invalid_params(duplicate.to_string()).into_value();
            }
        };
        let describe = |descriptor: &Descriptor| {
            descriptor.describe_from(self.get(descriptor.name()).is_some())
        };
        match method {
            Some(name) => match catalog.get(name) {
                Some(descriptor) => describe(descriptor),
                None => JsonRpcError::invalid_params(format!(
                    "unknown method '{name}'; system.describe lists every method"
                ))
                .into_value(),
            },
            None => serde_json::json!({
                "methods": catalog.iter().map(describe).collect::<Vec<_>>(),
            }),
        }
    }

    pub(crate) fn serve(
        &self,
        app: &mut PaneFlowApp,
        method: &str,
        params: &Value,
        caller_pid: Option<i64>,
        granted: bool,
        cx: &mut Context<PaneFlowApp>,
    ) -> IpcReply {
        let Some(entry) = self.get(method) else {
            return IpcReply::Ready(
                JsonRpcError::method_not_found(format!("Method not found: {method}")).into_value(),
            );
        };
        if let Err(error) = entry.validate(params) {
            return IpcReply::Ready(error.into_value());
        }
        if let Some(refusal) = gate_refusal(
            entry.spec().gate,
            app.cached_config.ai_unrestricted_enabled() || granted,
        ) {
            return IpcReply::Ready(refusal.into_value());
        }
        match &entry.serve {
            Serve::App(handler) => handler(app, params, caller_pid, cx),
            Serve::Inline(answer) => IpcReply::Ready(answer(params)),
            Serve::Stream => IpcReply::Ready(
                JsonRpcError::method_not_found(format!(
                    "{method} is a stream; open it on its own connection"
                ))
                .into_value(),
            ),
        }
    }
}

static REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
    super::register_methods(Registry::builder())
        .build()
        .expect("every app handler matches its paneflow-api declaration")
});

pub(crate) fn registry() -> &'static Registry {
    &REGISTRY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_registry_builds_and_serves_every_window_method() {
        let built = super::super::register_methods(Registry::builder()).build();
        let registry = built.expect("every registration matches its declaration");
        let catalog = paneflow_api::catalog().expect("catalog");
        let window: Vec<&str> = catalog
            .names()
            .into_iter()
            .filter(|name| {
                catalog
                    .get(name)
                    .is_some_and(|descriptor| descriptor.spec.owner == Owner::Window)
                    || paneflow_api::methods::WINDOW_ANSWERS_TOO.contains(name)
            })
            .collect();
        assert_eq!(registry.method_names(), window);
        for name in paneflow_api::methods::WINDOW_ANSWERS_TOO {
            assert_eq!(
                catalog.get(name).map(|descriptor| descriptor.spec.owner),
                Some(Owner::Worker),
                "{name} is owned by the worker and still answered by the window"
            );
        }
    }

    #[test]
    fn a_handler_registered_with_another_reply_kind_is_refused() {
        let error = Registry::builder()
            .stream::<paneflow_api::methods::system::Ping>()
            .build()
            .err()
            .expect("system.ping is declared inline");
        assert_eq!(
            error,
            RegistryError::WrongReply {
                method: "system.ping",
                declared: Reply::Inline,
                registered: Reply::Stream,
            }
        );
    }

    #[test]
    fn a_gated_method_served_where_no_gate_is_read_is_refused() {
        let error = Registry::builder()
            .stream::<paneflow_api::methods::surface::SendText>()
            .build()
            .err()
            .expect("a stream is opened without reading the scripting gate");
        assert_eq!(
            error,
            RegistryError::GatedOffTheAppThread("surface.send_text")
        );
    }

    #[test]
    fn a_method_registered_twice_is_refused() {
        let error = Registry::builder()
            .stream::<paneflow_api::methods::events::Subscribe>()
            .stream::<paneflow_api::methods::events::Subscribe>()
            .build()
            .err()
            .expect("twice");
        assert_eq!(error, RegistryError::Duplicate("events.subscribe"));
    }

    #[test]
    fn a_window_method_with_no_handler_is_refused() {
        let error = Registry::builder()
            .stream::<paneflow_api::methods::events::Subscribe>()
            .build()
            .err()
            .expect("the other window methods have no handler");
        assert!(matches!(error, RegistryError::Unserved(_)), "{error:?}");
    }

    #[test]
    fn a_mistyped_parameter_is_refused_naming_it_and_an_extra_one_is_accepted() {
        let entry = registry().get("surface.read").expect("surface.read");
        let error = entry
            .validate(&serde_json::json!({"lines": "50"}))
            .expect_err("a string is not a line count");
        assert_eq!(error.code, JsonRpcError::INVALID_PARAMS);
        assert!(error.message.contains("'lines'"), "{}", error.message);
        assert!(
            entry
                .validate(&serde_json::json!({"lines": 50, "from_a_newer_client": true}))
                .is_ok()
        );
    }

    #[test]
    fn the_scripting_gate_refusal_comes_from_the_declaration() {
        let spec = paneflow_api::methods::surface::SendKeystroke::SPEC;
        let Gate::Scripting { refusal } = spec.gate else {
            panic!("surface.send_keystroke is scripting gated");
        };
        let error = gate_refusal(spec.gate, false);
        if ipc_scripting_enabled() {
            assert!(error.is_none());
        } else {
            let error = error.expect("closed gate");
            assert_eq!(error.code, -32601);
            assert_eq!(error.message, refusal);
        }
        assert!(
            gate_refusal(spec.gate, true).is_none(),
            "free access opens it"
        );
    }
}

#[cfg(test)]
mod source_guards {
    const DISPATCH: &str = include_str!("mod.rs");
    const GATES: &str = include_str!("gates.rs");

    fn compares_a_method_name(line: &str) -> bool {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix('"') else {
            return trimmed.contains("starts_with(\"surface.")
                || trimmed.contains("starts_with(\"workspace.");
        };
        let Some((name, after)) = rest.split_once('"') else {
            return false;
        };
        name.contains('.')
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_')
            && (after.trim_start().starts_with("=>") || after.trim_start().starts_with('|'))
    }

    #[test]
    fn the_dispatch_module_holds_no_match_on_method_names() {
        let offenders: Vec<&str> = DISPATCH
            .lines()
            .filter(|line| compares_a_method_name(line))
            .collect();
        assert!(offenders.is_empty(), "{offenders:#?}");
        assert!(compares_a_method_name(
            r#"            "surface.read" => self.read(),"#
        ));
        assert!(compares_a_method_name(
            r#"            m if m.starts_with("workspace.") => {"#
        ));
    }

    #[test]
    fn capabilities_list_no_method_by_hand() {
        let body = GATES
            .split("fn capabilities_value")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .expect("capabilities_value is defined in gates.rs");
        assert!(!body.contains("\"system."), "{body}");
        assert!(!body.contains("\"surface."), "{body}");
        assert!(body.contains("registry().capability_names()"), "{body}");
    }
}
