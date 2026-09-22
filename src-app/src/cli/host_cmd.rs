use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Subcommand;
use paneflow_host::protocol::{ClientHello, ERR_SESSION_LIVE};
use paneflow_host::{HostClient, HostClientError, Probe};
use serde_json::{Value, json};

use super::{CliError, EXIT_OK};

const CLIENT_NAME: &str = "paneflow-cli";
const STOP_WAIT: Duration = Duration::from_secs(5);
const STOP_POLL: Duration = Duration::from_millis(50);

#[derive(Subcommand, Debug)]
pub enum HostCommand {
    #[command(
        about = "Start the detached local host for this PANEFLOW_HOME, or adopt the one already serving it"
    )]
    Start,
    #[command(about = "Report the host serving this PANEFLOW_HOME and every session it knows")]
    Status,
    #[command(about = "Stop the local host; refused while live sessions remain (they are listed)")]
    Stop {
        #[arg(
            long,
            help = "End every live session first instead of refusing; their processes are terminated"
        )]
        force: bool,
    },
}

pub fn run(command: HostCommand) -> Result<i32, CliError> {
    let home = paneflow_home::paneflow_home().ok_or_else(|| {
        CliError::runtime("cannot resolve the Paneflow state home; set PANEFLOW_HOME")
    })?;
    match command {
        HostCommand::Start => start(&home),
        HostCommand::Status => status(&home),
        HostCommand::Stop { force } => stop(&home, force),
    }
}

fn start(home: &Path) -> Result<i32, CliError> {
    let controller = std::env::current_exe()
        .map_err(|e| CliError::runtime(format!("cannot locate the paneflow executable: {e}")))?;
    let adoption = paneflow_host::ensure_host_running(home, &controller, CLIENT_NAME)
        .map_err(|e| CliError::runtime(e.to_string()))?;
    super::print_json(&json!({
        "home": home.display().to_string(),
        "started": adoption.started,
        "executable": adoption.executable,
        "identity": adoption.identity,
    }))?;
    Ok(EXIT_OK)
}

fn status(home: &Path) -> Result<i32, CliError> {
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home);
    let hello = ClientHello::local(CLIENT_NAME);
    let report = match paneflow_host::probe(home, &endpoint, &hello) {
        Probe::Running(identity) => {
            let mut client = HostClient::connect(&endpoint, &hello)
                .map_err(|e| CliError::runtime(e.to_string()))?;
            let status = client.call("host.status", json!({})).unwrap_or(Value::Null);
            let helpers = status["helpers"].clone();
            let resources = status["resources"].clone();
            let listed = client
                .call("session.list", json!({}))
                .map_err(|e| CliError::runtime(e.to_string()))?;
            let sessions: Vec<Value> = listed["sessions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(session_line)
                .collect();
            let live = sessions
                .iter()
                .filter(|s| s["live"].as_bool() == Some(true))
                .count();
            json!({
                "state": "running",
                "home": home.display().to_string(),
                "endpoint": endpoint.display().to_string(),
                "identity": *identity,
                "helpers": helpers,
                "resources": resources,
                "live_sessions": live,
                "sessions": sessions,
            })
        }
        Probe::Unreachable(error) => json!({
            "state": "unreachable",
            "home": home.display().to_string(),
            "endpoint": endpoint.display().to_string(),
            "error": error.to_string(),
            "last_instance_record": paneflow_host::bootstrap::read_instance_record(home),
            "last_instance_record_verified": false,
        }),
        Probe::Incompatible(message) => json!({
            "state": "incompatible",
            "home": home.display().to_string(),
            "endpoint": endpoint.display().to_string(),
            "message": message,
        }),
        Probe::Faulted(message) => json!({
            "state": "faulted",
            "home": home.display().to_string(),
            "endpoint": endpoint.display().to_string(),
            "message": message,
        }),
    };
    super::print_json(&report)?;
    Ok(EXIT_OK)
}

fn session_line(row: Value) -> Value {
    json!({
        "session": row["session"],
        "generation": row["generation"],
        "workspace": row["workspace"],
        "title": row["title"],
        "cwd": row["cwd"],
        "shell": row["shell"],
        "pid": row["pid"],
        "lifecycle": row["lifecycle"],
        "live": row["live"],
        "owned": row["owned"],
        "reconnection": row["reconnection"],
    })
}

fn stop(home: &Path, force: bool) -> Result<i32, CliError> {
    let endpoint: PathBuf = paneflow_host::endpoint::host_endpoint_path(home);
    let hello = ClientHello::local(CLIENT_NAME);
    let mut client = match HostClient::connect(&endpoint, &hello) {
        Ok(client) => client,
        Err(HostClientError::Unreachable { .. }) => {
            return Err(CliError::target(format!(
                "no host is serving {} at {}",
                home.display(),
                endpoint.display()
            )));
        }
        Err(error) => return Err(CliError::runtime(error.to_string())),
    };
    let instance = client.identity().host_instance.clone();
    match client.call("host.shutdown", json!({"force": force})) {
        Ok(_) => {}
        Err(HostClientError::Rpc {
            code,
            message,
            data,
        }) if code == ERR_SESSION_LIVE => {
            let mut lines = vec![message];
            for summary in data
                .as_ref()
                .and_then(|d| d["live_sessions"].as_array())
                .into_iter()
                .flatten()
            {
                lines.push(format!(
                    "  {} generation {} pid {} {} ({})",
                    summary["session"].as_str().unwrap_or("?"),
                    summary["generation"],
                    summary["process"]["pid"],
                    summary["launch"]["shell"].as_str().unwrap_or("?"),
                    summary["current_cwd"]
                        .as_str()
                        .or(summary["cwd"].as_str())
                        .unwrap_or("?")
                ));
            }
            lines.push(
                "stop each session with `paneflow-host session stop <id>` first, or rerun with `--force` to end them all"
                    .to_string(),
            );
            return Err(CliError::runtime(lines.join("\n")));
        }
        Err(error) => return Err(CliError::runtime(error.to_string())),
    }
    drop(client);
    let deadline = Instant::now() + STOP_WAIT;
    let stopped = loop {
        if matches!(
            paneflow_host::probe(home, &endpoint, &hello),
            Probe::Unreachable(_)
        ) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(STOP_POLL);
    };
    super::print_json(&json!({
        "home": home.display().to_string(),
        "host_instance": instance,
        "stopped": stopped,
        "endpoint": endpoint.display().to_string(),
    }))?;
    Ok(if stopped {
        EXIT_OK
    } else {
        super::EXIT_TIMEOUT
    })
}
