use std::io;
use std::path::Path;
use std::time::Duration;

use paneflow_ipc_client::ai_hook::AiHookFrame;
use paneflow_ipc_client::host_control::{HostControl, METHOD_AGENT_EVENT};

const HOST_CLIENT_NAME: &str = "paneflow-ai-hook";
const HOOK_REQUEST_DEADLINE: Duration = Duration::from_millis(350);

pub(crate) fn send_agent_event(
    endpoint: &Path,
    session: &str,
    frame: &AiHookFrame,
) -> io::Result<()> {
    let mut control =
        HostControl::connect_with_deadline(endpoint, HOST_CLIENT_NAME, HOOK_REQUEST_DEADLINE)
            .map_err(|error| io::Error::new(io::ErrorKind::ConnectionRefused, error))?;
    let accepted = control
        .request_with_deadline(
            METHOD_AGENT_EVENT,
            frame.to_agent_event_params(session),
            HOOK_REQUEST_DEADLINE,
        )
        .map_err(io::Error::other)?;
    if accepted.get("accepted").and_then(|value| value.as_bool()) == Some(false) {
        let reason = accepted
            .get("reason")
            .and_then(|value| value.as_str())
            .unwrap_or("the local host refused the event");
        return Err(io::Error::new(io::ErrorKind::InvalidData, reason));
    }
    Ok(())
}
