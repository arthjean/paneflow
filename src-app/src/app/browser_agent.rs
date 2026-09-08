use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{Context, Entity};
use paneflow_browser_protocol::{
    BrowserError, BrowserId, Event, HistoryDirection, InputEvent, MAX_AGENT_ACTION_MS,
    MAX_AGENT_LEASE_MS, MAX_AGENT_TYPING_BYTES, OperationId, exported_origin, exported_url,
    redact_headers,
};
use serde_json::{Map, Value, json};

use crate::PaneFlowApp;
use crate::app::ipc_handler::JsonRpcError;
use crate::browser::agent::{AgentError, AgentLease, AgentOperationInfo};
use crate::browser::authority::BrowserAuthority;
use crate::browser::view::BrowserView;

struct AgentActionOptions {
    mutation: bool,
    text_bytes: usize,
    deadline_ms: u64,
    deferred: bool,
}

impl PaneFlowApp {
    pub(crate) fn handle_browser_agent(
        &mut self,
        method: &str,
        params: &Value,
        scope_workspace_id: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Value {
        let workspace_id = match BrowserAuthority::agent_scope(scope_workspace_id) {
            Ok(workspace_id) => workspace_id,
            Err(error) => return agent_error(error),
        };
        match method {
            "browser.list" => self.browser_agent_list(workspace_id, cx),
            "browser.state" => self.browser_agent_state(workspace_id, params, cx),
            "browser.operation" => self.browser_agent_operation(workspace_id, params, cx),
            "browser.snapshot" => self.browser_agent_snapshot(workspace_id, params, cx),
            "browser.screenshot" => self.browser_agent_screenshot(workspace_id, params, cx),
            "browser.renew" => self.browser_agent_renew(workspace_id, params, cx),
            "browser.console" => self.browser_agent_diagnostics(workspace_id, params, false, cx),
            "browser.network" => self.browser_agent_diagnostics(workspace_id, params, true, cx),
            "browser.navigate" => self.browser_agent_navigate(workspace_id, params, cx),
            "browser.back" => {
                self.browser_agent_history(workspace_id, params, HistoryDirection::Back, cx)
            }
            "browser.forward" => {
                self.browser_agent_history(workspace_id, params, HistoryDirection::Forward, cx)
            }
            "browser.reload" => self.browser_agent_reload(workspace_id, params, cx),
            "browser.click" => self.browser_agent_click(workspace_id, params, cx),
            "browser.type" => self.browser_agent_type(workspace_id, params, cx),
            "browser.scroll" => self.browser_agent_scroll(workspace_id, params, cx),
            "browser.permission" => JsonRpcError::method_not_enabled(
                "browser permissions are controlled by the human in the Browser menu",
            )
            .into_value(),
            _ => JsonRpcError::method_not_found(format!("unknown browser method: {method}"))
                .into_value(),
        }
    }

    fn browser_agent_list(&self, workspace_id: u64, cx: &gpui::App) -> Value {
        let Some(authority) = cx.try_global::<BrowserAuthority>() else {
            return agent_error(AgentError::Cancelled);
        };
        if let Err(error) = authority.agent().require_read(workspace_id) {
            return agent_error(error);
        }
        let timestamp_ms = now_ms();
        let pages = self
            .quota_browser_views()
            .into_iter()
            .filter_map(|view| {
                let page = view.read(cx);
                (page.owner_ids().0 == workspace_id).then(|| browser_metadata(page, timestamp_ms))
            })
            .collect::<Vec<_>>();
        json!({
            "workspace_id": workspace_id,
            "pages": pages,
            "untrusted": true,
        })
    }

    fn browser_agent_state(&self, workspace_id: u64, params: &Value, cx: &gpui::App) -> Value {
        if let Err(error) = self.browser_agent_read_access(workspace_id, cx) {
            return agent_error(error);
        }
        let view = match self.browser_agent_target(workspace_id, params, cx) {
            Ok(view) => view,
            Err(error) => return error.into_value(),
        };
        let page = view.read(cx);
        json!({
            "workspace_id": workspace_id,
            "page": browser_metadata(page, now_ms()),
            "untrusted": true,
        })
    }

    fn browser_agent_operation(
        &mut self,
        workspace_id: u64,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Value {
        let Some(operation) = params.get("operation_id").and_then(Value::as_str) else {
            return JsonRpcError::invalid_params("missing 'operation_id'").into_value();
        };
        let operation = match OperationId::try_from(operation.to_owned()) {
            Ok(operation) => operation,
            Err(_) => return JsonRpcError::invalid_params("invalid 'operation_id'").into_value(),
        };
        let offset = match params.get("offset") {
            None => 0,
            Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                Some(offset) => offset,
                None => return JsonRpcError::invalid_params("invalid 'offset'").into_value(),
            },
        };
        let limit = match params.get("limit") {
            None => paneflow_browser_protocol::MAX_AGENT_CAPTURE_CHUNK_BYTES,
            Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                Some(limit) if limit > 0 => limit,
                _ => return JsonRpcError::invalid_params("invalid 'limit'").into_value(),
            },
        };
        if !cx.has_global::<BrowserAuthority>() {
            return agent_error(AgentError::Cancelled);
        }
        let (info, chunk) = {
            let agent = cx.global_mut::<BrowserAuthority>().agent_mut();
            let info = match agent.operation(workspace_id, &operation) {
                Ok(info) => info,
                Err(error) => return agent_error(error),
            };
            let chunk = match agent.capture_chunk(workspace_id, &operation, offset, limit) {
                Ok(chunk) => chunk,
                Err(error) => return agent_error(error),
            };
            (info, chunk)
        };
        self.browser_agent_operation_value(info, chunk, cx)
    }

    fn browser_agent_snapshot(
        &mut self,
        workspace_id: u64,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Value {
        if let Err(error) = self.browser_agent_read_access(workspace_id, cx) {
            return agent_error(error);
        }
        let view = match self.browser_agent_target(workspace_id, params, cx) {
            Ok(view) => view,
            Err(error) => return error.into_value(),
        };
        let lease = match self.browser_agent_lease(
            workspace_id,
            &view,
            AgentActionOptions {
                mutation: false,
                text_bytes: 0,
                deadline_ms: MAX_AGENT_ACTION_MS,
                deferred: false,
            },
            cx,
        ) {
            Ok(lease) => lease,
            Err(error) => return agent_error(error),
        };
        let snapshot = view.update(cx, |view, _| view.agent_snapshot());
        match snapshot {
            Some(snapshot) => {
                self.browser_agent_complete(lease, cx)
                    .map_or_else(agent_error, |_| {
                        json!({
                            "workspace_id": workspace_id,
                            "page": browser_metadata(view.read(cx), now_ms()),
                            "snapshot": snapshot,
                            "untrusted": true,
                        })
                    })
            }
            None => {
                self.browser_agent_abort(lease, AgentError::Cancelled, cx);
                browser_error(BrowserError::InvalidFrame)
            }
        }
    }

    fn browser_agent_screenshot(
        &mut self,
        workspace_id: u64,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Value {
        if let Err(error) = self.browser_agent_read_access(workspace_id, cx) {
            return agent_error(error);
        }
        self.browser_agent_action(
            workspace_id,
            params,
            AgentActionOptions {
                mutation: false,
                text_bytes: 0,
                deadline_ms: MAX_AGENT_ACTION_MS,
                deferred: true,
            },
            cx,
            |view, _| view.agent_screenshot().map(|operation| vec![operation]),
        )
    }

    fn browser_agent_renew(
        &mut self,
        workspace_id: u64,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Value {
        let Some(operation) = params.get("operation_id").and_then(Value::as_str) else {
            return JsonRpcError::invalid_params("missing 'operation_id'").into_value();
        };
        let operation = match OperationId::try_from(operation.to_owned()) {
            Ok(operation) => operation,
            Err(_) => return JsonRpcError::invalid_params("invalid 'operation_id'").into_value(),
        };
        let extension_ms = match params.get("extension_ms") {
            None => MAX_AGENT_LEASE_MS,
            Some(value) => match value.as_u64() {
                Some(extension_ms) if extension_ms > 0 => extension_ms,
                _ => return JsonRpcError::invalid_params("invalid 'extension_ms'").into_value(),
            },
        };
        let info = match cx.global_mut::<BrowserAuthority>().agent_mut().renew(
            workspace_id,
            &operation,
            extension_ms,
        ) {
            Ok(info) => info,
            Err(error) => return agent_error(error),
        };
        self.browser_agent_operation_value(info, None, cx)
    }

    fn browser_agent_diagnostics(
        &mut self,
        workspace_id: u64,
        params: &Value,
        network: bool,
        cx: &mut Context<Self>,
    ) -> Value {
        if let Err(error) = self.browser_agent_read_access(workspace_id, cx) {
            return agent_error(error);
        }
        let view = match self.browser_agent_target(workspace_id, params, cx) {
            Ok(view) => view,
            Err(error) => return error.into_value(),
        };
        let lease = match self.browser_agent_lease(
            workspace_id,
            &view,
            AgentActionOptions {
                mutation: false,
                text_bytes: 0,
                deadline_ms: MAX_AGENT_ACTION_MS,
                deferred: false,
            },
            cx,
        ) {
            Ok(lease) => lease,
            Err(error) => return agent_error(error),
        };
        view.update(cx, |view, _| view.enable_agent_diagnostics(network));
        let (entries, truncated) = view.read(cx).agent_diagnostics(network);
        let entries = entries
            .into_iter()
            .map(|entry| sanitize_diagnostic(entry, 0))
            .collect::<Vec<_>>();
        if let Err(error) = self.browser_agent_complete(lease, cx) {
            return agent_error(error);
        }
        json!({
            "workspace_id": workspace_id,
            "page": browser_metadata(view.read(cx), now_ms()),
            "entries": entries,
            "truncated": truncated,
            "untrusted": true,
        })
    }

    fn browser_agent_navigate(
        &mut self,
        workspace_id: u64,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Value {
        let Some(url) = params.get("url").and_then(Value::as_str) else {
            return JsonRpcError::invalid_params("missing 'url'").into_value();
        };
        self.browser_agent_action(
            workspace_id,
            params,
            AgentActionOptions {
                mutation: true,
                text_bytes: url.len(),
                deadline_ms: MAX_AGENT_LEASE_MS,
                deferred: true,
            },
            cx,
            |view, cx| {
                view.agent_navigate(url, cx)
                    .map(|operation| vec![operation])
            },
        )
    }

    fn browser_agent_history(
        &mut self,
        workspace_id: u64,
        params: &Value,
        direction: HistoryDirection,
        cx: &mut Context<Self>,
    ) -> Value {
        self.browser_agent_action(
            workspace_id,
            params,
            AgentActionOptions {
                mutation: true,
                text_bytes: 0,
                deadline_ms: MAX_AGENT_ACTION_MS,
                deferred: false,
            },
            cx,
            move |view, cx| {
                view.agent_history(direction, cx)
                    .map(|operation| vec![operation])
            },
        )
    }

    fn browser_agent_reload(
        &mut self,
        workspace_id: u64,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Value {
        self.browser_agent_action(
            workspace_id,
            params,
            AgentActionOptions {
                mutation: true,
                text_bytes: 0,
                deadline_ms: MAX_AGENT_ACTION_MS,
                deferred: false,
            },
            cx,
            |view, cx| view.agent_reload(cx).map(|operation| vec![operation]),
        )
    }

    fn browser_agent_click(
        &mut self,
        workspace_id: u64,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Value {
        let Some(x) = params.get("x").and_then(Value::as_i64) else {
            return JsonRpcError::invalid_params("missing or invalid 'x'").into_value();
        };
        let Some(y) = params.get("y").and_then(Value::as_i64) else {
            return JsonRpcError::invalid_params("missing or invalid 'y'").into_value();
        };
        let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
            return JsonRpcError::invalid_params("'x' and 'y' must be 32-bit integers")
                .into_value();
        };
        self.browser_agent_action(
            workspace_id,
            params,
            AgentActionOptions {
                mutation: true,
                text_bytes: 0,
                deadline_ms: MAX_AGENT_ACTION_MS,
                deferred: false,
            },
            cx,
            move |view, _| {
                let move_operation =
                    view.agent_input(InputEvent::MouseMove { x, y, modifiers: 0 })?;
                let down_operation = view.agent_input(InputEvent::MouseButton {
                    x,
                    y,
                    button: paneflow_browser_protocol::MouseButton::Left,
                    down: true,
                    clicks: 1,
                    modifiers: 0,
                })?;
                let up_operation = view.agent_input(InputEvent::MouseButton {
                    x,
                    y,
                    button: paneflow_browser_protocol::MouseButton::Left,
                    down: false,
                    clicks: 1,
                    modifiers: 0,
                })?;
                Ok(vec![move_operation, down_operation, up_operation])
            },
        )
    }

    fn browser_agent_type(
        &mut self,
        workspace_id: u64,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Value {
        let Some(text) = params.get("text").and_then(Value::as_str) else {
            return JsonRpcError::invalid_params("missing 'text'").into_value();
        };
        if text.len() > MAX_AGENT_TYPING_BYTES || text.chars().any(char::is_control) {
            return JsonRpcError::invalid_params("text exceeds 64 KiB or contains controls")
                .into_value();
        }
        self.browser_agent_action(
            workspace_id,
            params,
            AgentActionOptions {
                mutation: true,
                text_bytes: text.len(),
                deadline_ms: MAX_AGENT_ACTION_MS,
                deferred: false,
            },
            cx,
            |view, _| {
                view.agent_input(InputEvent::ImeCommit {
                    text: text.to_string(),
                    replacement: None,
                })
                .map(|operation| vec![operation])
            },
        )
    }

    fn browser_agent_scroll(
        &mut self,
        workspace_id: u64,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Value {
        let parse = |name: &str| {
            params
                .get(name)
                .and_then(Value::as_i64)
                .and_then(|value| i32::try_from(value).ok())
        };
        let (Some(x), Some(y), Some(delta_x), Some(delta_y)) =
            (parse("x"), parse("y"), parse("delta_x"), parse("delta_y"))
        else {
            return JsonRpcError::invalid_params(
                "'x', 'y', 'delta_x', and 'delta_y' must be 32-bit integers",
            )
            .into_value();
        };
        self.browser_agent_action(
            workspace_id,
            params,
            AgentActionOptions {
                mutation: true,
                text_bytes: 0,
                deadline_ms: MAX_AGENT_ACTION_MS,
                deferred: false,
            },
            cx,
            move |view, _| {
                view.agent_input(InputEvent::MouseWheel {
                    x,
                    y,
                    delta_x,
                    delta_y,
                    modifiers: 0,
                })
                .map(|operation| vec![operation])
            },
        )
    }

    fn browser_agent_action<F>(
        &mut self,
        workspace_id: u64,
        params: &Value,
        options: AgentActionOptions,
        cx: &mut Context<Self>,
        action: F,
    ) -> Value
    where
        F: FnOnce(
            &mut BrowserView,
            &mut Context<BrowserView>,
        ) -> Result<Vec<OperationId>, BrowserError>,
    {
        let view = match self.browser_agent_target(workspace_id, params, cx) {
            Ok(view) => view,
            Err(error) => return error.into_value(),
        };
        let lease = match self.browser_agent_lease(workspace_id, &view, options, cx) {
            Ok(lease) => lease,
            Err(error) => return agent_error(error),
        };
        match view.update(cx, action) {
            Ok(transport_operations) => {
                for transport_operation in transport_operations {
                    if let Err(error) = self.browser_agent_bind(&lease, transport_operation, cx) {
                        self.browser_agent_abort(lease, error, cx);
                        return agent_error(error);
                    }
                }
                json!({
                    "workspace_id": workspace_id,
                    "operation_id": lease.id,
                    "status": "accepted",
                    "page": browser_metadata(view.read(cx), now_ms()),
                    "untrusted": true,
                })
            }
            Err(error) => {
                self.browser_agent_abort(lease, AgentError::Cancelled, cx);
                browser_error(error)
            }
        }
    }

    fn browser_agent_read_access(
        &self,
        workspace_id: u64,
        cx: &gpui::App,
    ) -> Result<(), AgentError> {
        cx.try_global::<BrowserAuthority>()
            .ok_or(AgentError::Cancelled)?
            .agent()
            .require_read(workspace_id)
            .map(|_| ())
    }

    fn browser_agent_target(
        &self,
        workspace_id: u64,
        params: &Value,
        cx: &gpui::App,
    ) -> Result<Entity<BrowserView>, JsonRpcError> {
        let browser = params
            .get("browser_id")
            .and_then(Value::as_str)
            .ok_or_else(|| JsonRpcError::invalid_params("missing 'browser_id'"))?;
        let browser = BrowserId::try_from(browser.to_string())
            .map_err(|_| JsonRpcError::invalid_params("invalid 'browser_id'"))?;
        let generation = params
            .get("generation")
            .and_then(Value::as_u64)
            .filter(|generation| *generation > 0)
            .ok_or_else(|| JsonRpcError::invalid_params("missing or invalid 'generation'"))?;
        let mut owned_identity = false;
        for view in self.quota_browser_views() {
            let page = view.read(cx);
            if page.browser_id() != &browser {
                continue;
            }
            if page.owner_ids().0 != workspace_id {
                return Err(JsonRpcError {
                    code: -32001,
                    message: "browser access denied".to_string(),
                });
            }
            owned_identity = true;
            if page.document().generation == generation {
                return Ok(view.clone());
            }
        }
        if owned_identity {
            Err(JsonRpcError::method_not_enabled(
                "browser document generation is stale",
            ))
        } else {
            Err(JsonRpcError::method_not_enabled(
                "browser target is not accessible",
            ))
        }
    }

    fn browser_agent_lease(
        &mut self,
        workspace_id: u64,
        view: &Entity<BrowserView>,
        options: AgentActionOptions,
        cx: &mut Context<Self>,
    ) -> Result<AgentLease, AgentError> {
        let page = view.read(cx);
        let browser = page.browser_id().clone();
        let generation = page.document().generation;
        if !cx.has_global::<BrowserAuthority>() {
            return Err(AgentError::Cancelled);
        }
        let agent = cx.global_mut::<BrowserAuthority>().agent_mut();
        if options.deferred {
            agent.begin_deferred(
                workspace_id,
                browser,
                generation,
                options.mutation,
                options.text_bytes,
                options.deadline_ms,
            )
        } else {
            agent.begin(
                workspace_id,
                browser,
                generation,
                options.mutation,
                options.text_bytes,
                options.deadline_ms,
            )
        }
    }

    fn browser_agent_complete(
        &mut self,
        lease: AgentLease,
        cx: &mut Context<Self>,
    ) -> Result<(), AgentError> {
        if !cx.has_global::<BrowserAuthority>() {
            return Err(AgentError::Cancelled);
        }
        cx.global_mut::<BrowserAuthority>()
            .agent_mut()
            .complete(&lease)
    }

    fn browser_agent_bind(
        &mut self,
        lease: &AgentLease,
        transport_operation: OperationId,
        cx: &mut Context<Self>,
    ) -> Result<(), AgentError> {
        if !cx.has_global::<BrowserAuthority>() {
            return Err(AgentError::Cancelled);
        }
        cx.global_mut::<BrowserAuthority>()
            .agent_mut()
            .bind_transport(lease, transport_operation)
    }

    fn browser_agent_abort(
        &mut self,
        lease: AgentLease,
        error: AgentError,
        cx: &mut Context<Self>,
    ) {
        if cx.has_global::<BrowserAuthority>() {
            let authority = cx.global_mut::<BrowserAuthority>();
            authority.agent_mut().abort(&lease, error);
        }
    }

    pub(crate) fn finish_browser_agent_operation(
        &mut self,
        browser: BrowserId,
        operation: OperationId,
        result: &Result<Event, BrowserError>,
        view: &Entity<BrowserView>,
        cx: &mut Context<Self>,
    ) {
        if !cx.has_global::<BrowserAuthority>() {
            return;
        }
        let (info, deliver) = {
            let authority = cx.global_mut::<BrowserAuthority>();
            let agent = authority.agent_mut();
            let info = agent.finish_transport(&browser, &operation, result);
            let deliver = info
                .as_ref()
                .is_some_and(|info| agent.permission(info.workspace_id).access.permits_read());
            (info, deliver)
        };
        let Some(info) = info else {
            return;
        };
        if !deliver {
            return;
        }
        let mut event = serde_json::json!({
            "type": "browser_operation",
            "workspace_id": info.workspace_id,
            "operation_id": info.id,
            "browser_id": info.browser.as_str(),
            "generation": info.generation,
            "status": info.status,
            "timestamp_ms": now_ms(),
            "untrusted": true,
        });
        if let Some(error) = info.error {
            event["error"] = serde_json::json!({"kind": format!("{error:?}")});
        }
        let page = view.read(cx);
        if page.owner_ids().0 == info.workspace_id && page.browser_id() == &info.browser {
            event["page"] = browser_metadata(page, now_ms());
        }
        self.event_bus
            .broadcast("browser_operation", None, Some(info.workspace_id), &event);
        cx.notify();
    }
}

impl PaneFlowApp {
    fn browser_agent_operation_value(
        &self,
        info: AgentOperationInfo,
        chunk: Option<crate::browser::agent::AgentCaptureChunk>,
        cx: &gpui::App,
    ) -> Value {
        let mut value = serde_json::json!({
            "workspace_id": info.workspace_id,
            "operation_id": info.id,
            "browser_id": info.browser.as_str(),
            "generation": info.generation,
            "status": info.status,
            "timestamp_ms": now_ms(),
            "untrusted": true,
        });
        if let Some(error) = info.error {
            value["error"] = serde_json::json!({"kind": format!("{error:?}")});
        }
        if let Some(capture) = info.capture {
            value["capture"] = serde_json::json!({
                "mime": capture.mime,
                "width": capture.width,
                "height": capture.height,
                "encoded_bytes": capture.encoded_bytes,
            });
        }
        if let Some(chunk) = chunk {
            value["chunk"] = serde_json::json!({
                "mime": chunk.mime,
                "width": chunk.width,
                "height": chunk.height,
                "offset": chunk.offset,
                "next_offset": chunk.next_offset,
                "total_bytes": chunk.total_bytes,
                "data": chunk.data,
                "done": chunk.done,
            });
        }
        if let Some(page) = self.quota_browser_views().into_iter().find_map(|view| {
            let page = view.read(cx);
            (page.owner_ids().0 == info.workspace_id && page.browser_id() == &info.browser)
                .then(|| browser_metadata(page, now_ms()))
        }) {
            value["page"] = page;
        }
        value
    }
}

fn browser_metadata(view: &BrowserView, timestamp_ms: u64) -> Value {
    let (workspace_id, tab_id) = view.owner_ids();
    let url = view.agent_url().and_then(exported_url);
    let origin = view.agent_url().and_then(exported_origin);
    json!({
        "owner": {
            "workspace_id": workspace_id,
            "session_id": tab_id,
        },
        "browser_id": view.browser_id().as_str(),
        "document": {
            "browser_id": view.document().browser.as_str(),
            "generation": view.document().generation,
        },
        "origin": origin,
        "url": url,
        "title": view.agent_title(),
        "state": view.session_state(),
        "parked": !view.is_visible(),
        "timestamp_ms": timestamp_ms,
    })
}

fn sanitize_diagnostic(value: Value, depth: usize) -> Value {
    if depth > 4 {
        return Value::String("[truncated]".to_string());
    }
    match value {
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| sanitize_diagnostic(value, depth + 1))
                .collect(),
        ),
        Value::Object(values) => {
            let mut output = Map::new();
            for (key, value) in values {
                let value = if key.eq_ignore_ascii_case("headers") {
                    value
                        .as_object()
                        .map(redact_headers)
                        .map(Value::Object)
                        .unwrap_or_else(|| sanitize_diagnostic(value, depth + 1))
                } else if key.eq_ignore_ascii_case("url") {
                    value
                        .as_str()
                        .and_then(exported_url)
                        .map(Value::String)
                        .unwrap_or(Value::Null)
                } else {
                    sanitize_diagnostic(value, depth + 1)
                };
                output.insert(key, value);
            }
            Value::Object(output)
        }
        other => other,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u128::from(u64::MAX)) as u64
        })
}

fn agent_error(error: AgentError) -> Value {
    let (code, message) = match error {
        AgentError::ScopeRequired => (-32602, "browser operations require a workspace scope"),
        AgentError::AccessDenied => (-32001, "browser access denied"),
        AgentError::StaleTarget => (-32004, "browser target is stale"),
        AgentError::Busy => (-32010, "browser operation capacity is busy"),
        AgentError::LimitReached => (-32010, "browser operation capacity is exhausted"),
        AgentError::Cancelled => (-32005, "browser operation cancelled"),
        AgentError::TimedOut => (-32006, "browser operation timed out"),
        AgentError::Replay => (-32007, "browser operation already completed"),
        AgentError::InvalidInput => (-32602, "browser input exceeds its bound"),
    };
    JsonRpcError {
        code,
        message: message.to_string(),
    }
    .into_value()
}

fn browser_error(error: BrowserError) -> Value {
    match error {
        BrowserError::TooLarge | BrowserError::InvalidMessage | BrowserError::InvalidUrl => {
            JsonRpcError::invalid_params("browser request is invalid").into_value()
        }
        BrowserError::AccessDenied | BrowserError::UnknownIdentity => {
            agent_error(AgentError::AccessDenied)
        }
        BrowserError::StaleGeneration => agent_error(AgentError::StaleTarget),
        BrowserError::Busy => agent_error(AgentError::Busy),
        BrowserError::LimitReached => agent_error(AgentError::LimitReached),
        BrowserError::Unavailable
        | BrowserError::EmbeddedDevToolsUnavailable
        | BrowserError::InvalidFrame
        | BrowserError::IncompatibleVersion => {
            JsonRpcError::method_not_enabled("browser page is unavailable").into_value()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_diagnostic;
    use serde_json::json;

    #[test]
    fn diagnostic_page_text_stays_data_while_urls_and_headers_are_reduced() {
        let value = sanitize_diagnostic(
            json!({
                "message": "browser.navigate https://example.invalid",
                "url": "https://example.com/path?token=secret#fragment",
                "headers": {
                    "Authorization": "Bearer secret",
                    "X-Trace": "kept"
                }
            }),
            0,
        );
        assert_eq!(value["message"], "browser.navigate https://example.invalid");
        assert_eq!(value["url"], "https://example.com/path");
        assert_eq!(value["headers"]["Authorization"], "[REDACTED]");
        assert_eq!(value["headers"]["X-Trace"], "kept");
    }
}
