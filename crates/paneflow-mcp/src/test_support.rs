use std::cell::RefCell;
use std::collections::HashMap;

use paneflow_ipc_client::IpcTransport;
use serde_json::Value;

pub struct FakeTransport {
    responses: HashMap<String, Result<Value, String>>,
    calls: RefCell<Vec<(String, Value)>>,
}

impl FakeTransport {
    pub fn new() -> Self {
        Self {
            responses: HashMap::new(),
            calls: RefCell::new(Vec::new()),
        }
    }

    pub fn with(mut self, method: &str, result: Value) -> Self {
        self.responses.insert(method.to_string(), Ok(result));
        self
    }

    pub fn with_err(mut self, method: &str, message: &str) -> Self {
        self.responses
            .insert(method.to_string(), Err(message.to_string()));
        self
    }

    pub fn last_params(&self, method: &str) -> Option<Value> {
        self.calls
            .borrow()
            .iter()
            .rev()
            .find(|(called, _)| called == method)
            .map(|(_, params)| params.clone())
    }

    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.borrow().clone()
    }
}

impl IpcTransport for FakeTransport {
    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.calls.borrow_mut().push((method.to_string(), params));
        self.responses
            .get(method)
            .cloned()
            .unwrap_or_else(|| Err(format!("no fake for {method}")))
    }
}
