use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock, OnceLock};

use paneflow_api::{Descriptor, Method, Owner, Reply};
use paneflow_ipc_client::line_wire::Wire;
use serde_json::{Value, json};

use crate::layout_authority::ConnectionId;
use crate::protocol::{ERR_INTERNAL, ERR_INVALID_PARAMS, ERR_METHOD_NOT_FOUND};
use crate::server::{Flow, Worker};

#[derive(Debug, Clone, PartialEq)]
pub struct ServeError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl ServeError {
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: ERR_INVALID_PARAMS,
            message: message.into(),
            data: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: ERR_INTERNAL,
            message: message.into(),
            data: None,
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: ERR_METHOD_NOT_FOUND,
            message: format!("method not handled by the worker: {method}"),
            data: None,
        }
    }
}

pub(crate) struct Call<'a, P> {
    pub(crate) params: P,
    pub(crate) raw: &'a Value,
    pub(crate) connection: ConnectionId,
}

pub(crate) struct Stream<'a, P> {
    pub(crate) wire: &'a mut Wire,
    pub(crate) worker: &'a Arc<Worker>,
    pub(crate) shutdown: &'a AtomicBool,
    pub(crate) id: &'a Value,
    pub(crate) params: P,
}

type Handler<P> = fn(&Arc<Worker>, Call<'_, P>) -> Result<Value, ServeError>;

type ReadyHandler =
    Box<dyn Fn(&Arc<Worker>, &Value, ConnectionId) -> Result<Value, ServeError> + Send + Sync>;

type StreamHandler =
    Box<dyn Fn(&mut Wire, &Arc<Worker>, &AtomicBool, &Value, &Value) -> Flow + Send + Sync>;

enum Serve {
    Ready(ReadyHandler),
    Closing(ReadyHandler, fn(&Worker)),
    Stream(StreamHandler),
}

pub(crate) struct Entry {
    descriptor: Descriptor,
    schema: OnceLock<Value>,
    serve: Serve,
}

pub(crate) enum Served {
    Reply(Result<Value, ServeError>),
    ReplyThenClose(Result<Value, ServeError>, fn(&Worker)),
    Streamed(Flow),
}

impl Entry {
    pub(crate) fn name(&self) -> &'static str {
        self.descriptor.name()
    }

    fn validate(&self, params: &Value) -> Result<(), ServeError> {
        let schema = self.schema.get_or_init(|| self.descriptor.params_schema());
        paneflow_api::schema::validate(schema, params)
            .map_err(|violation| ServeError::invalid_params(violation.message))
    }
}

fn decode<P: serde::de::DeserializeOwned>(raw: &Value) -> Result<P, ServeError> {
    P::deserialize(raw)
        .map_err(|error| ServeError::invalid_params(format!("invalid params: {error}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    Undeclared(&'static str),
    Duplicate(&'static str),
    WrongReply {
        method: &'static str,
        declared: Reply,
        registered: Reply,
    },
    Unserved(&'static str),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
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

    fn ready_handler<M: Method>(handler: Handler<M::Params>) -> ReadyHandler {
        Box::new(move |worker, raw, connection| {
            let params = decode::<M::Params>(raw)?;
            handler(
                worker,
                Call {
                    params,
                    raw,
                    connection,
                },
            )
        })
    }

    pub(crate) fn ready<M: Method>(self, handler: Handler<M::Params>) -> Self {
        self.push::<M>(
            Reply::Ready,
            Serve::Ready(Self::ready_handler::<M>(handler)),
        )
    }

    pub(crate) fn asynchronous<M: Method>(self, handler: Handler<M::Params>) -> Self {
        self.push::<M>(
            Reply::Async,
            Serve::Ready(Self::ready_handler::<M>(handler)),
        )
    }

    pub(crate) fn inline<M: Method>(self, handler: Handler<M::Params>) -> Self {
        self.push::<M>(
            Reply::Inline,
            Serve::Ready(Self::ready_handler::<M>(handler)),
        )
    }

    pub(crate) fn closing<M: Method>(
        self,
        handler: Handler<M::Params>,
        after: fn(&Worker),
    ) -> Self {
        self.push::<M>(
            Reply::Ready,
            Serve::Closing(Self::ready_handler::<M>(handler), after),
        )
    }

    pub(crate) fn stream<M: Method>(self, handler: fn(Stream<'_, M::Params>) -> Flow) -> Self {
        self.push::<M>(
            Reply::Stream,
            Serve::Stream(Box::new(
                move |wire, worker, shutdown, id, raw| match decode::<M::Params>(raw) {
                    Ok(params) => handler(Stream {
                        wire,
                        worker,
                        shutdown,
                        id,
                        params,
                    }),
                    Err(error) => crate::server::write_error(wire, id, &error),
                },
            )),
        )
    }

    pub(crate) fn build(self) -> Result<Registry, RegistryError> {
        let catalog =
            paneflow_api::catalog().map_err(|duplicate| RegistryError::Duplicate(duplicate.0))?;
        let mut entries: Vec<Entry> = Vec::with_capacity(self.entries.len());
        for (entry, registered) in self.entries {
            let name = entry.name();
            if catalog.get(name).is_none() {
                return Err(RegistryError::Undeclared(name));
            }
            if entries.iter().any(|held| held.name() == name) {
                return Err(RegistryError::Duplicate(name));
            }
            if entry.descriptor.spec.reply != registered {
                return Err(RegistryError::WrongReply {
                    method: name,
                    declared: entry.descriptor.spec.reply,
                    registered,
                });
            }
            entries.push(entry);
        }
        for name in catalog.owned_by(Owner::Worker).map(Descriptor::name) {
            if !entries.iter().any(|entry| entry.name() == name) {
                return Err(RegistryError::Unserved(name));
            }
        }
        let order = catalog.names();
        entries.sort_by_key(|entry| order.iter().position(|name| *name == entry.name()));
        Ok(Registry { entries })
    }
}

pub(crate) struct Registry {
    entries: Vec<Entry>,
}

impl Registry {
    pub(crate) fn builder() -> RegistryBuilder {
        RegistryBuilder::default()
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.name() == name)
    }

    #[cfg(test)]
    pub(crate) fn method_names(&self) -> Vec<&'static str> {
        self.entries.iter().map(Entry::name).collect()
    }

    pub(crate) fn describe(&self, method: Option<&str>) -> Result<Value, ServeError> {
        let catalog = paneflow_api::catalog()
            .map_err(|duplicate| ServeError::invalid_params(duplicate.to_string()))?;
        match method {
            Some(name) => catalog
                .get(name)
                .map(|descriptor| descriptor.describe_from(self.get(name).is_some()))
                .ok_or_else(|| {
                    ServeError::invalid_params(format!(
                        "unknown method '{name}'; system.describe lists every method"
                    ))
                }),
            None => Ok(json!({
                "methods": catalog
                    .owned_by(Owner::Worker)
                    .map(|descriptor| descriptor.describe_from(self.get(descriptor.name()).is_some()))
                    .collect::<Vec<_>>(),
            })),
        }
    }

    pub(crate) fn call(
        &self,
        worker: &Arc<Worker>,
        method: &str,
        params: &Value,
    ) -> Result<Value, ServeError> {
        let entry = self
            .get(method)
            .ok_or_else(|| ServeError::method_not_found(method))?;
        entry.validate(params)?;
        match &entry.serve {
            Serve::Ready(handler) => handler(worker, params, ConnectionId::INTERNAL),
            Serve::Closing(handler, after) => {
                let answer = handler(worker, params, ConnectionId::INTERNAL);
                after(worker);
                answer
            }
            Serve::Stream(_) => Err(ServeError::invalid_params(format!(
                "{method} is a stream and answers no single call"
            ))),
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "a served request carries its wire, worker, shutdown flag, id, method, params and connection"
    )]
    pub(crate) fn serve(
        &self,
        wire: &mut Wire,
        worker: &Arc<Worker>,
        shutdown: &AtomicBool,
        id: &Value,
        method: &str,
        params: &Value,
        connection: ConnectionId,
    ) -> Served {
        let Some(entry) = self.get(method) else {
            return Served::Reply(Err(ServeError::method_not_found(method)));
        };
        if let Err(error) = entry.validate(params) {
            return Served::Reply(Err(error));
        }
        match &entry.serve {
            Serve::Ready(handler) => Served::Reply(handler(worker, params, connection)),
            Serve::Closing(handler, after) => {
                Served::ReplyThenClose(handler(worker, params, connection), *after)
            }
            Serve::Stream(handler) => Served::Streamed(handler(wire, worker, shutdown, id, params)),
        }
    }
}

static REGISTRY: LazyLock<Result<Registry, RegistryError>> =
    LazyLock::new(|| crate::server::register_methods(Registry::builder()).build());

pub(crate) fn registry() -> Result<&'static Registry, ServeError> {
    REGISTRY.as_ref().map_err(|error| ServeError {
        code: ERR_INTERNAL,
        message: format!("the worker's method registry does not match paneflow-api: {error}"),
        data: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_registry_builds_and_serves_every_worker_method_and_describe() {
        let registry = crate::server::register_methods(Registry::builder())
            .build()
            .expect("every worker handler matches its paneflow-api declaration");
        let catalog = paneflow_api::catalog().expect("catalog");
        let mut expected: Vec<&str> = catalog
            .owned_by(Owner::Worker)
            .map(Descriptor::name)
            .collect();
        expected.push("system.describe");
        let mut served = registry.method_names();
        served.sort_unstable();
        expected.sort_unstable();
        assert_eq!(served, expected);
    }

    #[test]
    fn a_worker_method_with_no_handler_is_refused() {
        let error = Registry::builder()
            .ready::<paneflow_api::methods::worker::Status>(|_, _| Ok(Value::Null))
            .build()
            .err()
            .expect("the other worker methods have no handler");
        assert!(matches!(error, RegistryError::Unserved(_)), "{error:?}");
    }

    #[test]
    fn a_handler_registered_with_another_reply_kind_is_refused() {
        let error = Registry::builder()
            .stream::<paneflow_api::methods::worker::Status>(|_| Flow::Close)
            .build()
            .err()
            .expect("worker.status is declared ready");
        assert_eq!(
            error,
            RegistryError::WrongReply {
                method: "worker.status",
                declared: Reply::Ready,
                registered: Reply::Stream,
            }
        );
    }

    #[test]
    fn a_method_registered_twice_is_refused() {
        let error = Registry::builder()
            .ready::<paneflow_api::methods::worker::Status>(|_, _| Ok(Value::Null))
            .ready::<paneflow_api::methods::worker::Status>(|_, _| Ok(Value::Null))
            .build()
            .err()
            .expect("twice");
        assert_eq!(error, RegistryError::Duplicate("worker.status"));
    }
}
