use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::{Parser, Subcommand};
use paneflow_host::protocol::ClientHello;
use paneflow_host::{HostClient, HostClientError, HostError};
use serde_json::{Value, json};

const EXIT_OK: i32 = 0;
const EXIT_RUNTIME: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_UNREACHABLE: i32 = 3;
const CLIENT_NAME: &str = "paneflow-host-cli";

#[derive(Parser)]
#[command(
    name = "paneflow-host",
    version,
    about = "GPU-free local host that owns Paneflow terminal sessions"
)]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Paneflow state home to serve (defaults to PANEFLOW_HOME or ~/.paneflow)"
    )]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Serve the local host endpoint in the foreground until `paneflow host stop`")]
    Serve {
        #[arg(long, help = "Override the local endpoint derived from the state home")]
        endpoint: Option<PathBuf>,
    },
    #[command(about = "Print the protocol and terminal engine identity this executable offers")]
    Identity,
    #[command(
        subcommand,
        about = "Manage hosted sessions over the running host's endpoint (JSON output)"
    )]
    Session(SessionCommand),
}

#[derive(Subcommand)]
enum SessionCommand {
    #[command(about = "List sessions known to the host, live or not")]
    List {
        #[arg(long, help = "Only sessions bound to this workspace id")]
        workspace: Option<String>,
    },
    #[command(about = "Create a hosted shell session and print its manifest")]
    Create {
        #[arg(
            long,
            help = "Working directory (defaults to the user's home directory)"
        )]
        cwd: Option<String>,
        #[arg(
            long,
            help = "Shell executable (defaults to default_shell in paneflow.json)"
        )]
        shell: Option<String>,
        #[arg(long, help = "Human title recorded on the manifest")]
        title: Option<String>,
        #[arg(long, help = "Workspace id to bind the session to")]
        workspace: Option<String>,
        #[arg(long, default_value_t = 80, help = "Initial columns")]
        cols: u16,
        #[arg(long, default_value_t = 24, help = "Initial rows")]
        rows: u16,
        #[arg(
            last = true,
            help = "Arguments passed to the shell, after `--`; never replayed on restart"
        )]
        args: Vec<String>,
    },
    #[command(about = "Print one session's manifest, liveness and reconnection state")]
    Inspect {
        #[arg(help = "Durable session id")]
        session: String,
    },
    #[command(about = "Terminate one session's owned process tree and record its exit")]
    Stop {
        #[arg(help = "Durable session id")]
        session: String,
        #[arg(
            long,
            help = "Only stop this generation; refuse if the session moved on"
        )]
        generation: Option<u64>,
    },
    #[command(
        about = "Start a new generation of an exited or lost session as an ordinary shell (no command replay)"
    )]
    Restart {
        #[arg(help = "Durable session id")]
        session: String,
        #[arg(long, help = "Only restart from this generation")]
        generation: Option<u64>,
    },
    #[command(
        about = "Delete a non-live session's manifest; refused while the session is still running"
    )]
    Remove {
        #[arg(help = "Durable session id")]
        session: String,
    },
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let home = cli
        .home
        .or_else(paneflow_home::paneflow_home)
        .unwrap_or_else(|| {
            eprintln!("paneflow-host: no state home could be resolved; set PANEFLOW_HOME");
            std::process::exit(EXIT_USAGE);
        });
    let code = match cli.command {
        Command::Identity => identity(&home),
        Command::Serve { endpoint } => serve(&home, endpoint),
        Command::Session(command) => session(&home, command),
    };
    std::process::exit(code);
}

fn identity(home: &Path) -> i32 {
    let identity = json!({
        "name": "paneflow-host",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": paneflow_host::HOST_PROTOCOL_VERSION,
        "engine": paneflow_host::protocol::local_engine_identity(),
        "home": home.display().to_string(),
        "endpoint": paneflow_host::endpoint::host_endpoint_path(home).display().to_string(),
    });
    print_json(&identity)
}

fn serve(home: &Path, endpoint: Option<PathBuf>) -> i32 {
    let endpoint = endpoint.unwrap_or_else(|| paneflow_host::endpoint::host_endpoint_path(home));
    let host = match paneflow_host::SessionHost::open(home, &endpoint) {
        Ok(host) => host,
        Err(HostError::OwnerBusy(message)) => {
            eprintln!("paneflow-host: {message}");
            return EXIT_UNREACHABLE;
        }
        Err(error) => {
            eprintln!("paneflow-host: cannot open {}: {error}", home.display());
            return EXIT_RUNTIME;
        }
    };
    let shutdown = Arc::new(AtomicBool::new(false));
    let served = paneflow_host::serve(Arc::clone(&host), &endpoint, shutdown);
    host.retire();
    match served {
        Ok(()) => {
            log::info!("paneflow-host: stopped serving {}", endpoint.display());
            EXIT_OK
        }
        Err(error) => {
            eprintln!("paneflow-host: serve failed: {error}");
            EXIT_RUNTIME
        }
    }
}

fn session(home: &Path, command: SessionCommand) -> i32 {
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home);
    let mut client = match HostClient::connect(&endpoint, &ClientHello::local(CLIENT_NAME)) {
        Ok(client) => client,
        Err(HostClientError::Unreachable { endpoint, .. }) => {
            eprintln!(
                "paneflow-host: no host is serving {} at {endpoint}; run `paneflow host start`",
                home.display()
            );
            return EXIT_UNREACHABLE;
        }
        Err(error) => {
            eprintln!("paneflow-host: {error}");
            return EXIT_RUNTIME;
        }
    };
    let owner = client.identity().host_instance.clone();
    let outcome = match command {
        SessionCommand::List { workspace } => {
            let mut params = json!({});
            if let Some(workspace) = workspace {
                params["workspace"] = json!(workspace);
            }
            client.call("session.list", params).map(|mut listed| {
                if let Some(sessions) = listed["sessions"].as_array_mut() {
                    for summary in sessions {
                        annotate_reconnection(summary, &owner);
                    }
                }
                listed
            })
        }
        SessionCommand::Create {
            cwd,
            shell,
            title,
            workspace,
            cols,
            rows,
            args,
        } => {
            let mut params = json!({"cols": cols, "rows": rows, "args": args});
            for (key, value) in [
                ("cwd", cwd),
                ("shell", shell),
                ("title", title),
                ("workspace", workspace),
            ] {
                if let Some(value) = value {
                    params[key] = json!(value);
                }
            }
            client.call("session.create", params)
        }
        SessionCommand::Inspect { session } => client
            .call("session.inspect", json!({"session": session}))
            .map(|mut summary| {
                annotate_reconnection(&mut summary, &owner);
                summary
            }),
        SessionCommand::Stop {
            session,
            generation,
        } => client.call(
            "session.stop",
            json!({"session": session, "generation": generation}),
        ),
        SessionCommand::Restart {
            session,
            generation,
        } => client.call(
            "session.restart",
            json!({"session": session, "generation": generation}),
        ),
        SessionCommand::Remove { session } => {
            client.call("session.remove", json!({"session": session}))
        }
    };
    match outcome {
        Ok(value) => print_json(&value),
        Err(error) => {
            eprintln!("paneflow-host: {error}");
            EXIT_RUNTIME
        }
    }
}

fn annotate_reconnection(summary: &mut Value, owner: &paneflow_host::HostInstanceToken) {
    let parsed: Result<paneflow_host::SessionSummary, _> = serde_json::from_value(summary.clone());
    if let Ok(parsed) = parsed
        && let Some(object) = summary.as_object_mut()
    {
        object.insert(
            "reconnection".to_string(),
            serde_json::to_value(parsed.reconnection(owner)).unwrap_or(Value::Null),
        );
    }
}

fn print_json(value: &Value) -> i32 {
    match serde_json::to_string_pretty(value) {
        Ok(rendered) => {
            println!("{rendered}");
            EXIT_OK
        }
        Err(error) => {
            eprintln!("paneflow-host: cannot render JSON: {error}");
            EXIT_RUNTIME
        }
    }
}
