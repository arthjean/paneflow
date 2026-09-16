use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::{Parser, Subcommand};

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
    #[command(about = "Serve the local host endpoint in the foreground until interrupted")]
    Serve {
        #[arg(long, help = "Override the local endpoint derived from the state home")]
        endpoint: Option<PathBuf>,
    },
    #[command(about = "Print the protocol and terminal engine identity this executable offers")]
    Identity,
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let home = cli
        .home
        .or_else(paneflow_home::paneflow_home)
        .unwrap_or_else(|| {
            eprintln!("paneflow-host: no state home could be resolved; set PANEFLOW_HOME");
            std::process::exit(2);
        });
    match cli.command {
        Command::Identity => {
            let identity = serde_json::json!({
                "name": "paneflow-host",
                "version": env!("CARGO_PKG_VERSION"),
                "protocol": paneflow_host::HOST_PROTOCOL_VERSION,
                "engine": paneflow_host::protocol::local_engine_identity(),
                "home": home.display().to_string(),
                "endpoint": paneflow_host::endpoint::host_endpoint_path(&home).display().to_string(),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&identity).unwrap_or_default()
            );
        }
        Command::Serve { endpoint } => {
            let endpoint =
                endpoint.unwrap_or_else(|| paneflow_host::endpoint::host_endpoint_path(&home));
            let host = match paneflow_host::SessionHost::open(&home, &endpoint) {
                Ok(host) => host,
                Err(error) => {
                    eprintln!("paneflow-host: cannot open {}: {error}", home.display());
                    std::process::exit(1);
                }
            };
            let shutdown = Arc::new(AtomicBool::new(false));
            if let Err(error) = paneflow_host::serve(host, &endpoint, shutdown) {
                eprintln!("paneflow-host: serve failed: {error}");
                std::process::exit(1);
            }
        }
    }
}
