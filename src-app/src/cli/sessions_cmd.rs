use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Subcommand;
use paneflow_serve::controller::{
    Bootstrap, Controller, ControllerError, FollowFrame, FollowSession,
};
use paneflow_serve::protocol::CAPABILITY_SESSION_RUNTIME_RESUME;
use serde_json::Value;

use super::{CliError, EXIT_OK, EXIT_RUNTIME};

const FRAME_WAIT: Duration = Duration::from_millis(250);

#[derive(Subcommand, Debug)]
pub enum SessionsCommand {
    #[command(
        about = "Resume a session's runtime through the worker (refused unless the worker advertises session.runtime.resume)"
    )]
    Resume {
        #[arg(help = "Session id as the worker publishes it")]
        session: String,
    },
    #[command(about = "Lower the unread flag the worker raised on one or more finished sessions")]
    Ack {
        #[arg(help = "Session ids as the worker publishes them", required = true)]
        sessions: Vec<String>,
    },
}

pub fn run(
    command: Option<SessionsCommand>,
    follow: bool,
    json_out: bool,
) -> Result<i32, CliError> {
    let endpoint = endpoint()?;
    match command {
        Some(SessionsCommand::Resume { session }) => resume(&endpoint, &session),
        Some(SessionsCommand::Ack { sessions }) => ack(&endpoint, &sessions, json_out),
        None if follow => stream(&endpoint, json_out),
        None => list(&endpoint, json_out),
    }
}

fn endpoint() -> Result<PathBuf, CliError> {
    paneflow_home::serve_endpoint_path_for_current_home().ok_or_else(|| {
        CliError::runtime("cannot resolve the Paneflow state home; set PANEFLOW_HOME")
    })
}

pub fn cli_error(error: ControllerError) -> CliError {
    match error {
        ControllerError::PermissionDenied(path) => CliError {
            code: EXIT_RUNTIME,
            message: format!(
                "permission denied: {} is owned by another user account",
                path.display()
            ),
        },
        ControllerError::Unreachable { endpoint, reason } => CliError::runtime(format!(
            "the paneflow worker is not reachable at {} ({reason}); start it with `paneflow serve start`",
            endpoint.display()
        )),
        other => CliError::runtime(other.to_string()),
    }
}

fn connect(endpoint: &Path) -> Result<Controller, CliError> {
    Controller::connect(endpoint).map_err(cli_error)
}

fn list(endpoint: &Path, json_out: bool) -> Result<i32, CliError> {
    let mut controller = connect(endpoint)?;
    let bootstrap = controller.snapshot().map_err(cli_error)?;
    if json_out {
        print_line(&bootstrap.to_value())?;
    } else {
        print_bootstrap(&bootstrap);
    }
    Ok(EXIT_OK)
}

fn resume(endpoint: &Path, session: &str) -> Result<i32, CliError> {
    let controller = connect(endpoint)?;
    controller
        .require(CAPABILITY_SESSION_RUNTIME_RESUME)
        .map_err(cli_error)?;
    Err(CliError::runtime(format!(
        "the worker advertises {CAPABILITY_SESSION_RUNTIME_RESUME} but this CLI cannot drive it yet \
         (session {session})"
    )))
}

fn ack(endpoint: &Path, sessions: &[String], json_out: bool) -> Result<i32, CliError> {
    let mut controller = connect(endpoint)?;
    let answered = controller.acknowledge(sessions).map_err(cli_error)?;
    if json_out {
        print_line(&answered)?;
    } else {
        let lowered = answered["acknowledged"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        println!("acknowledged {lowered} session(s)");
    }
    Ok(EXIT_OK)
}

fn stream(endpoint: &Path, json_out: bool) -> Result<i32, CliError> {
    let _ = ctrlc::set_handler(|| std::process::exit(EXIT_OK));
    let mut stream = FollowSession::open(endpoint).map_err(cli_error)?;
    loop {
        match stream.next(FRAME_WAIT) {
            FollowFrame::Bootstrap(bootstrap) => {
                if json_out {
                    print_line(&bootstrap.to_value())?;
                } else {
                    print_bootstrap(&bootstrap);
                }
            }
            FollowFrame::Event(event) => {
                if json_out {
                    print_line(&event)?;
                } else {
                    println!("{}", human_event(&event));
                }
            }
            FollowFrame::Disconnected(reason) => {
                eprintln!("paneflow: the worker stream dropped ({reason}); reconnecting");
            }
            FollowFrame::Idle => {}
        }
    }
}

fn print_line(value: &Value) -> Result<(), CliError> {
    let rendered = serde_json::to_string(value)
        .map_err(|error| CliError::runtime(format!("failed to render JSON: {error}")))?;
    println!("{rendered}");
    Ok(())
}

fn print_bootstrap(bootstrap: &Bootstrap) {
    println!(
        "worker {} protocol {} pid {} core {} capabilities {}",
        bootstrap.identity.version,
        bootstrap.identity.protocol,
        bootstrap.identity.pid,
        if bootstrap.core_connected {
            "connected"
        } else {
            "disconnected"
        },
        bootstrap.identity.capabilities.join(","),
    );
    for session in &bootstrap.fresh {
        println!("{}", human_event(session));
    }
}

fn human_event(value: &Value) -> String {
    let session = value["session"].as_str().unwrap_or("?");
    let status = value["status"].as_str().unwrap_or("idle");
    let source = value["activity_source"].as_str().unwrap_or("none");
    let runtime = value["runtime_id"].as_str().unwrap_or("-");
    let unread = if value["unread"].as_bool().unwrap_or_default() {
        " unread"
    } else {
        ""
    };
    let notify = value["notify"]["kind"]
        .as_str()
        .map(|kind| format!(" notify:{kind}"))
        .unwrap_or_default();
    format!("{session}  {status:<9} {source:<7} {runtime}{unread}{notify}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_capability_the_worker_never_advertises_is_refused_before_any_request() {
        let home = tempfile::tempdir().expect("a temporary home");
        let running = paneflow_serve::open(home.path()).expect("a worker takes the home");
        let endpoint = paneflow_home::serve_endpoint_path(home.path());

        let refused = resume(&endpoint, "01JABCDEFGHJKMNPQRSTVWXYZ")
            .expect_err("the CLI refuses a capability the worker does not advertise");
        assert_eq!(
            refused.message,
            format!("capability not advertised: {CAPABILITY_SESSION_RUNTIME_RESUME}")
        );
        assert_eq!(refused.code, EXIT_RUNTIME);

        assert_eq!(list(&endpoint, true).expect("the snapshot prints"), EXIT_OK);
        running.stop();
    }

    #[test]
    fn an_unreachable_worker_names_the_endpoint_and_a_denial_names_the_account() {
        let home = tempfile::tempdir().expect("a temporary home");
        let endpoint = paneflow_home::serve_endpoint_path(home.path());
        let unreachable = list(&endpoint, true).expect_err("no worker serves the home");
        assert!(
            unreachable.message.contains("paneflow serve start"),
            "unexpected message: {}",
            unreachable.message
        );

        let denied = cli_error(ControllerError::PermissionDenied(endpoint));
        assert!(
            denied.message.starts_with("permission denied"),
            "unexpected message: {}",
            denied.message
        );
        assert_eq!(denied.code, EXIT_RUNTIME);
    }

    #[test]
    fn the_human_line_names_the_state_its_source_and_its_attention() {
        let line = human_event(&json!({
            "session": "01JABC",
            "status": "idle",
            "activity_source": "hooks",
            "runtime_id": "com.anthropic.claude-code",
            "unread": true,
            "notify": {"kind": "finished"},
        }));
        assert!(line.contains("01JABC"));
        assert!(line.contains("idle"));
        assert!(line.contains("hooks"));
        assert!(line.contains("com.anthropic.claude-code"));
        assert!(line.contains("unread"));
        assert!(line.contains("notify:finished"));
    }
}
