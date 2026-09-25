use std::path::{Path, PathBuf};

use clap::Subcommand;
use paneflow_serve::bootstrap::{self, Probe};
use serde_json::{Value, json};

use super::{CliError, EXIT_OK};

#[derive(Subcommand, Debug)]
pub enum ServeCommand {
    #[command(
        about = "Start the detached worker for this PANEFLOW_HOME, or adopt the one already serving it"
    )]
    Start,
    #[command(
        about = "Print the worker pid, protocol version, home, session count and advertised capabilities"
    )]
    Status,
    #[command(about = "Stop the worker; terminals are untouched")]
    Stop {
        #[arg(
            long,
            default_value_t = 5_000,
            help = "Milliseconds to wait for the worker to exit before giving up"
        )]
        drain_ms: u64,
    },
    #[command(
        hide = true,
        about = "Run the worker in this process; started by the app"
    )]
    Run {
        #[arg(long, help = "State home this worker serves")]
        home: Option<PathBuf>,
    },
}

pub fn run(command: ServeCommand) -> Result<i32, CliError> {
    match command {
        ServeCommand::Run { home } => run_worker(home),
        ServeCommand::Start => start(&resolve_home()?),
        ServeCommand::Status => status(&resolve_home()?),
        ServeCommand::Stop { drain_ms } => {
            stop(&resolve_home()?, std::time::Duration::from_millis(drain_ms))
        }
    }
}

fn resolve_home() -> Result<PathBuf, CliError> {
    paneflow_home::paneflow_home().ok_or_else(|| {
        CliError::runtime("cannot resolve the Paneflow state home; set PANEFLOW_HOME")
    })
}

fn run_worker(home: Option<PathBuf>) -> Result<i32, CliError> {
    let home = match home {
        Some(home) => home,
        None => resolve_home()?,
    };
    match paneflow_serve::run(&home) {
        Ok(()) => Ok(EXIT_OK),
        Err(paneflow_serve::WorkerError::AlreadyRunning(lock)) => {
            eprintln!("paneflow serve: already running (owner lock {lock})");
            Ok(EXIT_OK)
        }
        Err(error) => Err(CliError::runtime(error.to_string())),
    }
}

fn start(home: &Path) -> Result<i32, CliError> {
    let endpoint = paneflow_home::serve_endpoint_path(home);
    if let Probe::Running(identity) = bootstrap::probe(home, &endpoint) {
        super::print_json(&json!({
            "home": home.display().to_string(),
            "started": false,
            "state": "already running",
            "identity": *identity,
        }))?;
        return Ok(EXIT_OK);
    }
    let controller = std::env::current_exe()
        .map_err(|e| CliError::runtime(format!("cannot locate the paneflow executable: {e}")))?;
    let adoption = bootstrap::ensure_worker_running(home, &controller)
        .map_err(|e| CliError::runtime(e.to_string()))?;
    super::print_json(&json!({
        "home": home.display().to_string(),
        "started": adoption.started,
        "state": if adoption.started { "started" } else { "already running" },
        "replaced": adoption.replaced,
        "identity": adoption.identity,
    }))?;
    Ok(EXIT_OK)
}

fn status(home: &Path) -> Result<i32, CliError> {
    let endpoint = paneflow_home::serve_endpoint_path(home);
    let report = match bootstrap::probe(home, &endpoint) {
        Probe::Running(identity) => {
            let sessions = worker_status(&endpoint)
                .map(|status| status["session_count"].clone())
                .unwrap_or(Value::Null);
            json!({
                "state": "running",
                "pid": identity.pid,
                "build_id": identity.build_id,
                "protocol": identity.protocol,
                "required_core_protocol": identity.required_core_protocol,
                "home": identity.home,
                "endpoint": identity.endpoint,
                "version": identity.version,
                "session_count": sessions,
                "capabilities": identity.capabilities,
            })
        }
        Probe::Unreachable(error) => json!({
            "state": "unreachable",
            "home": home.display().to_string(),
            "endpoint": endpoint.display().to_string(),
            "error": error,
            "last_instance_record": bootstrap::read_instance_record(home),
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

fn worker_status(endpoint: &Path) -> Option<Value> {
    use paneflow_ipc_client::host_control::HostControl;
    let mut control = HostControl::connect(endpoint, "paneflow-cli").ok()?;
    control
        .request(paneflow_serve::protocol::METHOD_WORKER_STATUS, json!({}))
        .ok()
}

fn stop(home: &Path, drain: std::time::Duration) -> Result<i32, CliError> {
    let endpoint = paneflow_home::serve_endpoint_path(home);
    if matches!(bootstrap::probe(home, &endpoint), Probe::Unreachable(_)) {
        super::print_json(&json!({
            "home": home.display().to_string(),
            "stopped": true,
            "state": "not running",
        }))?;
        return Ok(EXIT_OK);
    }
    let stopped = bootstrap::stop_worker(home, drain);
    super::print_json(&json!({
        "home": home.display().to_string(),
        "endpoint": endpoint.display().to_string(),
        "stopped": stopped,
        "drain_ms": drain.as_millis() as u64,
    }))?;
    Ok(if stopped {
        EXIT_OK
    } else {
        super::EXIT_TIMEOUT
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_start_adopts_the_running_worker_and_status_prints_its_capabilities() {
        let home = tempfile::tempdir().expect("home");
        let worker = paneflow_serve::open(home.path()).expect("a worker takes the home");

        assert_eq!(
            start(home.path()).expect("start answers"),
            EXIT_OK,
            "a second start exits 0 instead of racing the running worker"
        );

        let endpoint = paneflow_home::serve_endpoint_path(home.path());
        let reported = worker_status(&endpoint).expect("worker.status answers the CLI");
        assert_eq!(
            reported["pid"].as_u64(),
            Some(u64::from(std::process::id()))
        );
        assert_eq!(
            reported["protocol"].as_u64(),
            Some(u64::from(paneflow_serve::WORKER_PROTOCOL_VERSION))
        );
        assert_eq!(reported["home"], home.path().display().to_string());
        assert!(reported["session_count"].as_u64().is_some());
        assert_eq!(
            reported["capabilities"]
                .as_array()
                .map(|entries| entries.len()),
            Some(paneflow_serve::advertised_capabilities().len())
        );
        assert_eq!(status(home.path()).expect("status answers"), EXIT_OK);

        assert_eq!(
            stop(home.path(), paneflow_serve::DRAIN_WAIT).expect("stop answers"),
            EXIT_OK
        );
        worker.stop();
        assert_eq!(
            stop(home.path(), std::time::Duration::from_millis(100)).expect("stop answers"),
            EXIT_OK,
            "stopping a worker that is not running is not an error"
        );
    }
}
