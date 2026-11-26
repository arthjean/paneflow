use clap::Args;
use paneflow_ipc_client::IpcTransport;
use serde_json::{Value, json};

use super::selector::{Reach, resolve_target_in};
use super::{CliError, EXIT_OK, EXIT_RUNTIME};

pub(super) const NO_CALLING_PANE: &str =
    "paneflow write needs the session of the calling pane; run it from a Paneflow pane";

#[derive(Args, Debug)]
pub(super) struct WriteArgs {
    #[arg(help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`")]
    target: String,
    #[arg(
        help = "Message for the agent in the target pane; Paneflow prefixes it with a line naming this pane"
    )]
    text: String,
    #[arg(
        long,
        help = "Submit the message once it is written, as if Enter were pressed"
    )]
    submit: bool,
    #[arg(
        long,
        value_parser = ["workspace", "all"],
        default_value = "workspace",
        help = "Writes stay in the calling pane's workspace. `all` reaches every workspace and needs `ipc.orchestration` set to true in paneflow.json"
    )]
    scope: String,
    #[arg(long, help = "Print the whole pane.write reply as JSON")]
    json: bool,
}

pub(super) fn write_params(
    source: &str,
    session: &str,
    text: &str,
    submit: bool,
    scope_all: bool,
) -> Value {
    let mut params = json!({
        "source_session": source,
        "session": session,
        "text": text,
        "submit": submit,
    });
    if scope_all {
        params["scope"] = json!("all");
    }
    params
}

pub(super) fn session_of(client: &impl IpcTransport, surface_id: u64) -> Result<String, CliError> {
    let status = client
        .call("surface.status", json!({"surface_id": surface_id}))
        .map_err(CliError::runtime)?;
    if let Some(error) = status.get("error").and_then(Value::as_str) {
        return Err(CliError::runtime(error.to_string()));
    }
    status
        .get("session")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            CliError::runtime(format!(
                "surface {surface_id} is not backed by a Paneflow host session, so it cannot be written to"
            ))
        })
}

fn request_label(request: &Value) -> String {
    request
        .as_str()
        .map_or_else(|| request.to_string(), str::to_string)
}

pub(super) fn printed(reply: &Value, as_json: bool) -> Result<(String, i32), CliError> {
    let code = match reply["status"].as_str() {
        Some("written" | "approval_pending") => EXIT_OK,
        Some("no_window") => EXIT_RUNTIME,
        other => {
            return Err(CliError::runtime(format!(
                "pane.write returned an unknown status {}",
                other.unwrap_or("(none)")
            )));
        }
    };
    if as_json {
        let rendered = serde_json::to_string_pretty(reply)
            .map_err(|error| CliError::runtime(format!("failed to render JSON: {error}")))?;
        return Ok((rendered, code));
    }
    let line = match reply["status"].as_str() {
        Some("approval_pending") => {
            format!("approval_pending {}", request_label(&reply["request"]))
        }
        Some(status) => status.to_string(),
        None => String::new(),
    };
    Ok((line, code))
}

pub(super) fn run(args: &WriteArgs) -> Result<i32, CliError> {
    let source =
        super::send_cmd::caller_session().ok_or_else(|| CliError::runtime(NO_CALLING_PANE))?;
    let client = super::connect().map_err(|message| {
        CliError::runtime(message.trim_start_matches("paneflow: ").to_string())
    })?;
    let scope_all = args.scope == "all";
    let surface_id = resolve_target_in(&client, &args.target, Reach::from_scope_all(scope_all))
        .map_err(|error| CliError::runtime(error.message))?;
    let session = session_of(&client, surface_id)?;
    let reply = super::router::Owners::from_environment()?.call(
        "pane.write",
        write_params(&source, &session, &args.text, args.submit, scope_all),
    )?;
    let (line, code) = printed(&reply, args.json)?;
    println!("{line}");
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_status_prints_its_line_and_exit_code() {
        assert_eq!(
            printed(&json!({"status": "written", "bytes": 40}), false).unwrap(),
            ("written".to_string(), EXIT_OK)
        );
        assert_eq!(
            printed(&json!({"status": "approval_pending", "request": 7}), false).unwrap(),
            ("approval_pending 7".to_string(), EXIT_OK)
        );
        assert_eq!(
            printed(&json!({"status": "no_window", "request": 8}), false).unwrap(),
            ("no_window".to_string(), EXIT_RUNTIME)
        );
        let (rendered, code) =
            printed(&json!({"status": "written", "submitted": true}), true).unwrap();
        assert_eq!(code, EXIT_OK);
        assert_eq!(
            serde_json::from_str::<Value>(&rendered).unwrap(),
            json!({"status": "written", "submitted": true})
        );
        assert!(printed(&json!({"status": "lost"}), false).is_err());
    }

    #[test]
    fn the_request_carries_the_calling_session_submit_and_scope() {
        assert_eq!(
            write_params("a", "b", "hi", true, true),
            json!({"source_session": "a", "session": "b", "text": "hi", "submit": true, "scope": "all"})
        );
        assert!(
            write_params("a", "b", "hi", false, false)
                .get("scope")
                .is_none()
        );
    }
}
