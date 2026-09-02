use paneflow_ipc_client::IpcTransport;
use serde_json::json;

use super::selector::resolve_target;
use super::{CliError, EXIT_OK};

pub fn new_workspace(
    client: &impl IpcTransport,
    name: Option<&str>,
    cwd: Option<&str>,
) -> Result<i32, CliError> {
    let mut params = json!({});
    if let Some(name) = name {
        params["name"] = json!(name);
    }
    if let Some(cwd) = cwd {
        params["cwd"] = json!(cwd);
    }
    let result = super::reject_legacy_error(
        client
            .call("workspace.create", params)
            .map_err(CliError::runtime)?,
    )?;
    super::print_json(&result)?;
    Ok(EXIT_OK)
}

pub fn select(client: &impl IpcTransport, index: u64) -> Result<i32, CliError> {
    let result = super::reject_legacy_error(
        client
            .call("workspace.select", json!({ "index": index }))
            .map_err(CliError::runtime)?,
    )?;
    super::print_json(&result)?;
    Ok(EXIT_OK)
}

pub fn split(
    client: &impl IpcTransport,
    direction: &str,
    target: Option<&str>,
) -> Result<i32, CliError> {
    let mut params = json!({ "direction": direction });
    if let Some(target) = target {
        params["surface_id"] = json!(resolve_target(client, target)?);
    }
    let result = super::reject_legacy_error(
        client
            .call("surface.split", params)
            .map_err(CliError::runtime)?,
    )?;
    super::print_json(&result)?;
    Ok(EXIT_OK)
}

pub fn focus(client: &impl IpcTransport, target: &str) -> Result<i32, CliError> {
    let surface_id = resolve_target(client, target)?;
    let result = super::reject_legacy_error(
        client
            .call("surface.focus", json!({ "surface_id": surface_id }))
            .map_err(CliError::runtime)?,
    )?;
    super::print_json(&result)?;
    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::cell::RefCell;

    struct FakeTransport(Value);
    impl IpcTransport for FakeTransport {
        fn call(&self, _method: &str, _params: Value) -> Result<Value, String> {
            Ok(self.0.clone())
        }
    }

    struct RoutedTransport {
        calls: RefCell<Vec<(String, Value)>>,
        reply: Value,
    }
    impl RoutedTransport {
        fn new(reply: Value) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                reply,
            }
        }
    }
    impl IpcTransport for RoutedTransport {
        fn call(&self, method: &str, params: Value) -> Result<Value, String> {
            if method == "surface.list" {
                return Ok(json!({ "surfaces": [
                    { "surface_id": 12, "name": "backend" },
                    { "surface_id": 18, "name": "frontend" },
                ]}));
            }
            self.calls
                .borrow_mut()
                .push((method.to_string(), params.clone()));
            Ok(self.reply.clone())
        }
    }

    #[test]
    fn focus_resolves_selector_then_calls_surface_focus() {
        let fake = RoutedTransport::new(json!({ "focused": true }));
        assert_eq!(focus(&fake, "backend").expect("ok"), EXIT_OK);
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "surface.focus");
        assert_eq!(calls[0].1["surface_id"], 12);
    }

    #[test]
    fn focus_ambiguous_prefix_is_target_error_without_ipc_mutation() {
        let fake = RoutedTransport::new(json!({ "focused": true }));
        let err = focus(&fake, "zzz").expect_err("no match");
        assert_eq!(err.code, crate::cli::EXIT_TARGET);
        assert!(
            fake.calls.borrow().is_empty(),
            "no mutation on resolve fail"
        );
    }

    #[test]
    fn split_with_target_passes_resolved_surface_id() {
        let fake = RoutedTransport::new(json!({ "split": true, "panes": 3 }));
        assert_eq!(
            split(&fake, "vertical", Some("frontend")).expect("ok"),
            EXIT_OK
        );
        let calls = fake.calls.borrow();
        assert_eq!(calls[0].0, "surface.split");
        assert_eq!(calls[0].1["surface_id"], 18);
        assert_eq!(calls[0].1["direction"], "vertical");
    }

    #[test]
    fn split_without_target_omits_surface_id() {
        let fake = RoutedTransport::new(json!({ "split": true, "panes": 2 }));
        assert_eq!(split(&fake, "horizontal", None).expect("ok"), EXIT_OK);
        let calls = fake.calls.borrow();
        assert_eq!(calls[0].0, "surface.split");
        assert!(calls[0].1.get("surface_id").is_none());
    }

    #[test]
    fn split_at_cap_legacy_error_is_nonzero_exit() {
        let fake = FakeTransport(json!({ "error": "Maximum pane count reached" }));
        let err = split(&fake, "horizontal", None).expect_err("legacy error must be non-zero exit");
        assert_eq!(err.code, crate::cli::EXIT_RUNTIME);
        assert!(err.message.contains("Maximum pane"), "got: {}", err.message);
    }

    #[test]
    fn select_out_of_range_legacy_error_is_nonzero_exit() {
        let fake = FakeTransport(json!({ "error": "Index out of bounds" }));
        let err = select(&fake, 999).expect_err("legacy error must be non-zero exit");
        assert_eq!(err.code, crate::cli::EXIT_RUNTIME);
    }

    #[test]
    fn select_success_returns_ok() {
        let fake = FakeTransport(json!({ "selected": 2 }));
        assert_eq!(select(&fake, 2).expect("ok"), EXIT_OK);
    }
}
