use std::time::Duration;

use paneflow_serve::controller::{EventFrame, EventRequest, EventStream};
use serde_json::{Value, json};

use super::{CliError, EXIT_OK};

const FRAME_WAIT: Duration = Duration::from_millis(250);

const ID_FIELDS: &[&str] = &["workspace_id", "tab_id", "pane_id", "surface_id", "session"];

pub(super) fn request(
    types: &[String],
    since: Option<u64>,
    workspace: Option<u64>,
) -> EventRequest {
    EventRequest {
        types: types
            .iter()
            .flat_map(|entry| entry.split(','))
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect(),
        workspace_id: workspace,
        since,
        epoch: None,
    }
}

pub(super) fn human_line(event: &Value) -> String {
    let mut line = format!(
        "{} {}",
        event["seq"].as_u64().unwrap_or_default(),
        event["type"].as_str().unwrap_or_default()
    );
    for field in ID_FIELDS {
        match &event[*field] {
            Value::Null => {}
            Value::String(text) => line.push_str(&format!(" {field}={text}")),
            other => line.push_str(&format!(" {field}={other}")),
        }
    }
    line
}

pub(super) fn restarted_line(seq: u64, epoch: &str, json_out: bool) -> String {
    if json_out {
        json!({"type": "restarted", "seq": seq, "epoch": epoch}).to_string()
    } else {
        format!(
            "-- the worker restarted: the event sequence restarted at seq {seq} (epoch {epoch})"
        )
    }
}

pub(super) fn run(
    types: &[String],
    since: Option<u64>,
    workspace: Option<u64>,
    json_out: bool,
) -> Result<i32, CliError> {
    let endpoint = paneflow_home::serve_endpoint_path_for_current_home().ok_or_else(|| {
        CliError::runtime("cannot resolve the Paneflow state home; set PANEFLOW_HOME")
    })?;
    let mut stream = EventStream::open(&endpoint, request(types, since, workspace))
        .map_err(super::sessions_cmd::cli_error)?;
    let _ = ctrlc::set_handler(|| std::process::exit(EXIT_OK));
    loop {
        match stream.next(FRAME_WAIT) {
            EventFrame::Following {
                seq,
                epoch,
                restarted: true,
            } => println!("{}", restarted_line(seq, &epoch, json_out)),
            EventFrame::Following { .. } | EventFrame::Idle => {}
            EventFrame::Event(event) => {
                if json_out {
                    println!("{event}");
                } else {
                    println!("{}", human_line(&event));
                }
            }
            EventFrame::Disconnected(reason) => {
                eprintln!("paneflow: the worker event stream dropped ({reason}); reconnecting");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_and_since_map_to_the_follow_parameters() {
        let built = request(
            &[
                "terminal.*,pane.closed".to_string(),
                " layout.changed ".to_string(),
            ],
            Some(41),
            Some(7),
        );
        assert_eq!(built.types, ["terminal.*", "pane.closed", "layout.changed"]);
        assert_eq!(built.since, Some(41));
        assert_eq!(built.workspace_id, Some(7));
        assert_eq!(built.epoch, None);
    }

    #[test]
    fn a_human_line_carries_the_seq_the_type_and_every_id() {
        let line = human_line(&json!({
            "seq": 42,
            "ts": 1,
            "type": "pane.created",
            "workspace_id": 1,
            "tab_id": 2,
            "pane_id": 5,
            "data": {},
        }));
        assert_eq!(line, "42 pane.created workspace_id=1 tab_id=2 pane_id=5");
        let line =
            human_line(&json!({"seq": 3, "type": "terminal.bell", "session": "s-1", "data": {}}));
        assert_eq!(line, "3 terminal.bell session=s-1");
    }
}
