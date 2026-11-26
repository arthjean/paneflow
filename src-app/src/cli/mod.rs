use clap::{Parser, Subcommand, ValueEnum};
use paneflow_ipc_client::host_control::{ControlTarget, HostTransport, resolve_control_target};
use paneflow_ipc_client::{IpcClient, IpcTransport};
use serde_json::Value;

pub(crate) mod api_cmd;
mod config_cmd;
mod control_cmds;
mod do_cmd;
mod events_cmd;
mod flow_cmd;
mod flow_spec;
mod grants_cmd;
mod headless;
mod host_cmd;
mod read_cmds;
pub(crate) mod router;
mod selector;
mod send_cmd;
mod serve_cmd;
mod sessions_cmd;
mod surface_read;
mod up_cmd;
mod wait_cmd;
mod watch_cmd;
mod worker_state;
mod workspace_spec;

pub const EXIT_OK: i32 = 0;
pub const EXIT_RUNTIME: i32 = 1;
pub const EXIT_TARGET: i32 = 3;
pub const EXIT_TIMEOUT: i32 = 4;

const VERBS: &[&str] = &[
    "api",
    "do",
    "actions",
    "config",
    "grants",
    "ls",
    "read",
    "search",
    "ps",
    "status",
    "new",
    "select",
    "split",
    "send",
    "up",
    "wait",
    "watch",
    "focus",
    "key",
    "flow",
    "host",
    "serve",
    "sessions",
    "events",
    "agent",
    "list_panes",
    "read_pane",
    "search_pane",
];

pub fn is_cli_verb(arg: Option<&str>) -> bool {
    matches!(arg, Some(v) if VERBS.contains(&v))
}

const HELP_VERB: &str = "help";

const EXTERNAL_VERBS: &[(&str, &str)] = &[
    (
        "mcp",
        "Install, inspect or remove the MCP bridge (install | status | uninstall)",
    ),
    (
        "integrations",
        "List, install or remove agent integrations (list | install | remove)",
    ),
    (
        "hooks",
        "Set up, inspect or remove agent status hooks (setup | status | uninstall)",
    ),
];

pub fn is_help_verb(arg: Option<&str>) -> bool {
    arg == Some(HELP_VERB)
}

pub fn looks_like_unknown_verb(arg: Option<&str>) -> bool {
    matches!(arg, Some(v) if !v.is_empty() && !v.starts_with('-') && !VERBS.contains(&v) && v != HELP_VERB)
}

pub fn unknown_verb_message(verb: &str) -> String {
    format!("paneflow: unknown verb '{verb}'; see `paneflow --help` for the verb list")
}

fn verb_rows() -> Vec<(String, String)> {
    use clap::CommandFactory;

    let mut rows: Vec<(String, String)> = Cli::command()
        .get_subcommands()
        .filter(|command| command.get_name() != HELP_VERB)
        .map(|command| {
            let names = std::iter::once(command.get_name())
                .chain(command.get_all_aliases())
                .collect::<Vec<_>>()
                .join(", ");
            let about = command
                .get_about()
                .map(|about| about.to_string())
                .unwrap_or_default();
            (names, about)
        })
        .collect();
    rows.extend(
        EXTERNAL_VERBS
            .iter()
            .map(|(name, about)| ((*name).to_string(), (*about).to_string())),
    );
    rows
}

fn aligned_rows(rows: &[(String, String)]) -> String {
    let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
    rows.iter()
        .map(|(label, about)| format!("  {label:<width$}  {about}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn global_help(version: &str, macos: bool) -> String {
    let (secondary, alt) = if macos {
        ("Cmd", "Option")
    } else {
        ("Ctrl", "Alt")
    };
    let verbs = aligned_rows(&verb_rows());
    let keybindings = aligned_rows(&[
        (
            format!("{secondary}+Shift+D/E"),
            "Split horizontal/vertical".to_string(),
        ),
        (format!("{secondary}+Shift+W"), "Close pane".to_string()),
        (format!("{alt}+Arrow"), "Focus adjacent pane".to_string()),
        (format!("{secondary}+Shift+N"), "New workspace".to_string()),
        ("Ctrl+Tab".to_string(), "Next workspace".to_string()),
        (
            format!("{secondary}+1-9"),
            "Switch to workspace N".to_string(),
        ),
    ]);
    format!(
        "Paneflow {version} - native terminal workspace for coding agents\n\
         \n\
         Usage: paneflow [OPTIONS]\n\
         \x20      paneflow <VERB> [ARGS]\n\
         \x20      paneflow help\n\
         \n\
         Verbs:\n\
         {verbs}\n\
         \n\
         Run `paneflow <VERB> --help` for a verb's options.\n\
         \n\
         Options:\n\
         \x20 -h, --help         Print this help message\n\
         \x20 -v, --version      Print version\n\
         \x20 --update-and-exit  Check for an update and exit (CI harness)\n\
         \n\
         Agent workflow:\n\
         \x20 Launch Claude Code, Codex, opencode, Pi, or any CLI agent in panes\n\
         \x20 Use `paneflow mcp install` so capable agents can read pane output\n\
         \n\
         Keybindings:\n\
         {keybindings}\n\
         \n\
         Config paths and IPC endpoints are documented in the README.\n\
         https://github.com/arthjean/paneflow"
    )
}

#[derive(Parser, Debug)]
#[command(
    name = "paneflow",
    version,
    about = "Drive a running Paneflow instance from the shell",
    subcommand_required = false,
    arg_required_else_help = false
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(
        subcommand,
        about = "List, describe and call every Paneflow method on the process that owns it"
    )]
    Api(api_cmd::ApiCommand),
    #[command(
        about = "Run a Lua script against Paneflow: a file, -e '<chunk>' or - for stdin",
        long_about = "Run a Lua 5.4 script in this process against Paneflow. The script reaches every method as paneflow.<namespace>.<method>{...}, reacts to events with paneflow.on and paneflow.wait, sets timers with paneflow.after and paneflow.every, and runs programs with paneflow.exec.\n\nkey=value pairs after the script fill paneflow.args, typed by spelling (n=2 is a number, on=true a boolean). A returned value is printed as one line of JSON. An error prints `Error: <message>` and exits 1; paneflow.stop(n) exits n; Ctrl-C exits 130.\n\nExamples:\n  paneflow do ./split.lua\n  paneflow do -e 'return paneflow.surface.list()'\n  echo 'return paneflow.args.n * 2' | paneflow do - n=21"
    )]
    Do(do_cmd::DoArgs),
    #[command(
        about = "List the actions init.lua declares, as the worker loaded them",
        long_about = "List the actions init.lua declares, as the worker loaded them: name, title, category and where each is declared. Run one with `paneflow do <name> key=value`.\n\nExamples:\n  paneflow actions\n  paneflow actions --json"
    )]
    Actions {
        #[arg(long, help = "Print the action.list result as JSON")]
        json: bool,
    },
    #[command(
        subcommand,
        about = "Reload or check init.lua, the Lua configuration the worker runs"
    )]
    Config(config_cmd::ConfigCommand),
    #[command(
        subcommand,
        about = "List or revoke the grants agents in panes hold over Paneflow"
    )]
    Grants(grants_cmd::GrantsCommand),
    #[command(
        alias = "list_panes",
        about = "List the panes of the active workspace, framed as untrusted output when printed to a program (--raw for plain output)"
    )]
    Ls {
        #[arg(long, help = "Human-readable table instead of the default JSON")]
        human: bool,
        #[arg(
            long,
            conflicts_with = "human",
            help = "Print the listing as JSON, the default"
        )]
        json: bool,
        #[arg(
            long,
            help = "Never wrap the listing in the `<untrusted_terminal_output>` fence it gets when standard output is not a terminal"
        )]
        raw: bool,
        #[arg(
            long,
            value_parser = ["workspace", "all"],
            default_value = "workspace",
            help = "From inside a Paneflow pane, reads stay in that pane's workspace. `all` reaches every workspace and needs `ipc.orchestration` set to true in paneflow.json"
        )]
        scope: String,
    },
    #[command(
        alias = "read_pane",
        about = "Print a pane's scrollback, framed as untrusted output by default (--raw for plain text)"
    )]
    Read {
        #[arg(help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`")]
        target: String,
        #[arg(long, help = "Number of trailing lines (server clamps to 1..4000)")]
        lines: Option<u64>,
        #[arg(long, help = "Offset from the end of the buffer")]
        offset: Option<u64>,
        #[arg(
            long,
            help = "Emit the `{text, lines, total_lines, eof}` envelope as JSON"
        )]
        json: bool,
        #[arg(
            long,
            help = "Return raw scrollback, bypassing the anti-injection fence that otherwise wraps the output as `<untrusted_terminal_output>` (the fence is on by default; see the ai_injection_fence setting)"
        )]
        raw: bool,
        #[arg(
            long,
            value_parser = ["workspace", "all"],
            default_value = "workspace",
            help = "From inside a Paneflow pane, reads stay in that pane's workspace. `all` reaches every workspace and needs `ipc.orchestration` set to true in paneflow.json"
        )]
        scope: String,
    },
    #[command(
        alias = "search_pane",
        about = "Search a pane's scrollback for a substring/pattern, framed as untrusted output by default (--raw for plain lines)"
    )]
    Search {
        #[arg(help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`")]
        target: String,
        #[arg(help = "Pattern to search for")]
        pattern: String,
        #[arg(long, help = "Cap the number of matches (server clamps to 1..1000)")]
        max: Option<u64>,
        #[arg(
            long,
            help = "Print `<line>: <text>` lines when the server sends no fenced text"
        )]
        human: bool,
        #[arg(
            long,
            conflicts_with = "human",
            help = "Print the whole `{matches, truncated, text}` reply as JSON"
        )]
        json: bool,
        #[arg(
            long,
            help = "Return raw `<line>: <text>` lines, bypassing the anti-injection fence that otherwise wraps the matches as `<untrusted_terminal_output>` (see the ai_injection_fence setting)"
        )]
        raw: bool,
        #[arg(
            long,
            value_parser = ["workspace", "all"],
            default_value = "workspace",
            help = "From inside a Paneflow pane, reads stay in that pane's workspace. `all` reaches every workspace and needs `ipc.orchestration` set to true in paneflow.json"
        )]
        scope: String,
    },
    #[command(about = "List running agents across the fleet (pid, tool, state, pane)")]
    Ps {
        #[arg(
            long,
            help = "Emit the `{agents:[…]}` envelope as JSON instead of a table"
        )]
        json: bool,
    },
    #[command(about = "Read one surface's agent state (thinking / waiting / idle / errored / …)")]
    Status {
        #[arg(help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`")]
        target: String,
        #[arg(
            long,
            help = "Emit the status envelope as JSON instead of a one-line summary"
        )]
        json: bool,
    },
    #[command(about = "Create a new workspace")]
    New {
        #[arg(long, help = "Workspace title")]
        name: Option<String>,
        #[arg(long, help = "Working directory for the first pane (must exist)")]
        cwd: Option<String>,
        #[arg(
            long,
            help = "Space to create the workspace in, by name (any case) or id; defaults to the active space"
        )]
        space: Option<String>,
    },
    #[command(about = "Select a workspace by index")]
    Select {
        #[arg(help = "Zero-based workspace index")]
        index: u64,
    },
    #[command(about = "Split the active pane horizontally or vertically")]
    Split {
        #[arg(help = "`h`/`horizontal` (panes stacked) or `v`/`vertical` (side by side)")]
        direction: SplitDir,
        #[arg(
            long,
            help = "Split the pane hosting this target instead of the first leaf. Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`"
        )]
        target: Option<String>,
    },
    #[command(
        about = "Inject text into a pane WITHOUT submitting it (human-in-loop)",
        long_about = "Inject text into a pane WITHOUT submitting it (human-in-loop).\n\nRequires `ipc.scripting` set to true in paneflow.json (or a write grant); the text is written verbatim with no trailing newline so the user/agent reviews and presses Enter themselves - unless `--submit` is passed explicitly."
    )]
    Send {
        #[arg(help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`")]
        target: String,
        #[arg(help = "Text to inject (no trailing carriage return is added by default)")]
        text: String,
        #[arg(
            long,
            help = "Send to EVERY pane matching the target (a multi-match selector is an error without this flag). Prints a `{sent, failed}` report"
        )]
        broadcast: bool,
        #[arg(
            long,
            help = "Submit the text (append a carriage return). Explicit opt-in: this is the ONLY way the CLI ever submits on the user's behalf, and it still requires the instance-side scripting gate"
        )]
        submit: bool,
        #[arg(
            long,
            help = "Force bracketed-paste delivery: the text is wrapped in `ESC[200~`/`ESC[201~` and, with `--submit`, the carriage return is sent separately after a calibrated delay so a TUI agent does not swallow it (EP-001). `--submit` toward an agent pane enables this automatically; pass `--paste` to force it (e.g. toward a shell) or `--paste` alone to wrap a non-submitted inject"
        )]
        paste: bool,
        #[arg(
            long,
            value_name = "PATH",
            help = "Ask the agent to write its complete result to this file and print `REPORT_DONE <path>` after the file is fully written. The path is resolved relative to the caller's current directory"
        )]
        report_file: Option<String>,
        #[arg(
            long,
            help = "Write even when the target agent waits for a decision (permission, menu, question) or no longer runs in the foreground. The forced write is logged by the instance"
        )]
        force: bool,
        #[arg(
            long,
            value_parser = ["workspace", "all"],
            default_value = "workspace",
            help = "From inside a Paneflow pane, writes stay in that pane's workspace. `all` reaches every workspace and needs `ipc.orchestration` set to true in paneflow.json"
        )]
        scope: String,
    },
    #[command(about = "Give a targeted surface the keyboard focus")]
    Focus {
        #[arg(help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`")]
        target: String,
    },
    #[command(
        about = "Send a named keystroke (e.g. `escape`, `ctrl-c`, `tab`) to a pane",
        long_about = "Send a named keystroke (e.g. `escape`, `ctrl-c`, `tab`) to a pane.\n\nRequires `ipc.scripting` set to true in paneflow.json (or a write grant). Keystrokes that would submit a line (`enter`, `ctrl-m`, `ctrl-j`) are refused - submission is exclusive to `send --submit`."
    )]
    Key {
        #[arg(help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`")]
        target: String,
        #[arg(help = "Dash-separated keystroke description (\"escape\", \"ctrl-c\", \"alt-f\")")]
        keystroke: String,
        #[arg(
            long,
            value_parser = ["workspace", "all"],
            default_value = "workspace",
            help = "From inside a Paneflow pane, keystrokes stay in that pane's workspace. `all` reaches every workspace and needs `ipc.orchestration` set to true in paneflow.json"
        )]
        scope: String,
    },
    #[command(
        subcommand,
        about = "Run a declarative agent DAG from a `flow.toml` (orchestration engine)"
    )]
    Flow(FlowCommand),
    #[command(
        about = "Spawn a declarative agent workspace from a TOML file (\"compose for agents\")"
    )]
    Up {
        #[arg(help = "Path to a `paneflow.workspace.toml` spec")]
        file: String,
        #[arg(
            long,
            help = "Validate + print the resolved plan without touching the instance"
        )]
        dry_run: bool,
    },
    #[command(
        about = "Block until a pane goes idle, or a regex appears in its output (orchestration)"
    )]
    Wait {
        #[arg(
            long = "match",
            value_name = "SELECTOR",
            help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`. Note: `cmdline:` matches the full argv on Linux but only the executable basename on macOS/Windows; prefer `cwd:` or a name for a portable selector"
        )]
        selector: String,
        #[arg(
            long,
            required_unless_present = "idle",
            help = "Regex to wait for in the pane's recent scrollback. Only output that arrives after the wait starts can match, so a line already on screen does not end it. Required unless `--idle` is set. With `--idle` it is an optional sentinel: it is checked on each new output and EITHER signal (pattern match OR going idle) returns first (EP-003 US-008)"
        )]
        pattern: Option<String>,
        #[arg(
            long,
            help = "Wait until the pane's agent ends its turn: for an agent that declares its state through OSC 7501, return on its next idle or attention state (at once if it already waits). Other panes wait for their output to go quiet (no `output_generation` change for `--for` ms) on the push stream. Single-target"
        )]
        idle: bool,
        #[arg(
            long = "for",
            value_name = "MS",
            help = "With `--idle`: the quiescence window in milliseconds (default 1000). The pane must produce no new output for this long to count as idle"
        )]
        for_ms: Option<u64>,
        #[arg(long, help = "Max seconds to wait before giving up (default 300)")]
        timeout: Option<u64>,
        #[arg(
            long,
            conflicts_with = "all",
            help = "Succeed as soon as ANY matching pane matches (selector may hit several). `--pattern` mode only; ignored with `--idle`"
        )]
        any: bool,
        #[arg(
            long,
            help = "Require ALL matching panes to match the pattern. `--pattern` mode only"
        )]
        all: bool,
    },
    #[command(
        subcommand,
        about = "Start, inspect or stop the detached local host that owns terminal sessions for this PANEFLOW_HOME (no running GUI needed)"
    )]
    Host(host_cmd::HostCommand),
    #[command(
        subcommand,
        about = "Start, inspect or stop the per-home worker that owns agent activity and serves Controllers"
    )]
    Serve(serve_cmd::ServeCommand),
    #[command(
        about = "Read the worker's session projection as a Controller: a bootstrap carrying its advertised capabilities, then one frame per reduced transition"
    )]
    Sessions {
        #[command(subcommand)]
        command: Option<sessions_cmd::SessionsCommand>,
        #[arg(
            long,
            help = "Stay connected and print one frame per transition, reconnecting to a worker that restarts"
        )]
        follow: bool,
        #[arg(long, help = "Emit JSON lines instead of a human table")]
        json: bool,
    },
    #[command(
        about = "Follow the worker's numbered event log: one line per event with its seq, type and ids, reconnecting across a worker restart"
    )]
    Events {
        #[arg(long, help = "Print one JSON object per event instead of a line")]
        json: bool,
        #[arg(
            long,
            value_name = "TYPES",
            help = "Only these event types, comma-separated: exact names or a family such as terminal.*"
        )]
        types: Vec<String>,
        #[arg(
            long,
            value_name = "SEQ",
            help = "Replay the events after this seq first"
        )]
        since: Option<u64>,
        #[arg(
            long,
            value_name = "ID",
            help = "Only events about this layout workspace id"
        )]
        workspace_id: Option<u64>,
    },
    #[command(about = "Stream lifecycle events from the running instance as JSONL (EP-002)")]
    Watch {
        #[arg(
            long,
            help = "Only stream events for this pane (selector). Omit for all panes"
        )]
        surface: Option<String>,
        #[arg(
            long = "type",
            value_name = "TYPE",
            help = "Only stream these event types (repeatable). Omit for all types"
        )]
        types: Vec<String>,
        #[arg(
            long,
            help = "Hide subscription protocol frames and print user events only"
        )]
        events_only: bool,
    },
}

#[derive(Subcommand, Debug)]
enum FlowCommand {
    #[command(
        about = "Execute (or validate with --dry-run) a flow file against the running instance. Spawns panes, waits on `ready` barriers, feeds steps - submission only with explicit `submit = true` + the scripting gate"
    )]
    Run {
        #[arg(help = "Path to a `flow.toml`")]
        file: String,
        #[arg(
            long,
            help = "Validate + print the resolved plan without touching the instance"
        )]
        dry_run: bool,
        #[arg(
            long,
            help = "Final machine-readable report on stdout (live transitions move to stderr)"
        )]
        json: bool,
        #[arg(
            long,
            value_parser = ["workspace", "all"],
            default_value = "workspace",
            help = "From inside a Paneflow pane, `send` steps that target a pane the flow did not spawn stay in that pane's workspace. `all` reaches every workspace and needs `ipc.orchestration` set to true in paneflow.json"
        )]
        scope: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
enum SplitDir {
    #[value(name = "horizontal", alias = "h")]
    Horizontal,
    #[value(name = "vertical", alias = "v")]
    Vertical,
}

impl SplitDir {
    fn as_ipc(self) -> &'static str {
        match self {
            SplitDir::Horizontal => "horizontal",
            SplitDir::Vertical => "vertical",
        }
    }
}

#[derive(Debug)]
pub struct CliError {
    pub code: i32,
    pub message: String,
}

impl CliError {
    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_RUNTIME,
            message: message.into(),
        }
    }

    pub fn target(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_TARGET,
            message: message.into(),
        }
    }
}

pub fn run() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };

    let Some(command) = cli.command else {
        return EXIT_OK;
    };

    if let Commands::Api(command) = command {
        return match api_cmd::run(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("paneflow: {}", e.message);
                e.code
            }
        };
    }

    if let Commands::Do(args) = command {
        return do_cmd::run(args);
    }

    if let Commands::Actions { json } = command {
        return match config_cmd::actions(json) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("paneflow: {}", e.message);
                e.code
            }
        };
    }

    if let Commands::Config(command) = command {
        return match config_cmd::run(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("paneflow: {}", e.message);
                e.code
            }
        };
    }

    if let Commands::Grants(command) = command {
        return match grants_cmd::run(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("paneflow: {}", e.message);
                e.code
            }
        };
    }

    if let Commands::Host(command) = command {
        return match host_cmd::run(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("paneflow: {}", e.message);
                e.code
            }
        };
    }

    if let Commands::Serve(command) = command {
        return match serve_cmd::run(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("paneflow: {}", e.message);
                e.code
            }
        };
    }

    if let Commands::Events {
        json,
        types,
        since,
        workspace_id,
    } = command
    {
        return match events_cmd::run(&types, since, workspace_id, json) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("paneflow: {}", e.message);
                e.code
            }
        };
    }

    if let Commands::Sessions {
        command,
        follow,
        json,
    } = command
    {
        return match sessions_cmd::run(command, follow, json) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("paneflow: {}", e.message);
                e.code
            }
        };
    }

    let client = match connect() {
        Ok(client) => client,
        Err(message) => {
            eprintln!("{message}");
            return EXIT_RUNTIME;
        }
    };

    match dispatch(command, &client) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("paneflow: {}", e.message);
            e.code
        }
    }
}

pub(super) enum CliTransport {
    Controller(IpcClient),
    Host(Box<HostTransport>),
}

impl CliTransport {
    pub(super) fn controller_socket(&self) -> Option<std::path::PathBuf> {
        match self {
            Self::Controller(_) => paneflow_home::ipc_endpoint().map(|endpoint| endpoint.path),
            Self::Host(_) => None,
        }
    }

    pub(super) fn no_controller(action: &str) -> CliError {
        CliError::runtime(format!(
            "{action} needs an open Paneflow window; the local host answers session and terminal \
             operations, not machine-local window actions"
        ))
    }
}

fn worker_owned(method: &str) -> bool {
    paneflow_api::catalog().ok().is_some_and(|catalog| {
        catalog
            .get(method)
            .is_some_and(|descriptor| descriptor.spec.owner == paneflow_api::Owner::Worker)
    })
}

fn worker_lacks(error: &str, method: &str) -> bool {
    error.ends_with(&format!("method not handled by the worker: {method}"))
}

const PANE_ENDPOINT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

fn worker_endpoint_for(
    home: &std::path::Path,
    pane_endpoint: Option<&std::ffi::OsStr>,
) -> (std::path::PathBuf, bool) {
    match paneflow_home::pane_endpoint_from(home, paneflow_home::PaneServer::Worker, pane_endpoint)
    {
        Some(endpoint) => (endpoint, true),
        None => (paneflow_home::serve_endpoint_path(home), false),
    }
}

fn wait_for_pane_endpoint(endpoint: &std::path::Path) {
    use paneflow_ipc_client::host_control::{HostControl, REQUEST_DEADLINE};
    let deadline = std::time::Instant::now() + PANE_ENDPOINT_WAIT;
    while std::time::Instant::now() < deadline
        && HostControl::open(endpoint, CLIENT_NAME, REQUEST_DEADLINE).is_err()
    {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn worker_call(method: &str, params: Value) -> Result<Value, String> {
    use paneflow_ipc_client::host_control::{HostControl, REQUEST_DEADLINE};
    let home = paneflow_home::paneflow_home()
        .ok_or("cannot resolve the Paneflow state home; set PANEFLOW_HOME")?;
    let (endpoint, sandboxed) = worker_endpoint_for(
        &home,
        std::env::var_os(paneflow_home::PANE_SERVE_ENDPOINT_ENV).as_deref(),
    );
    let unreachable = |reason: String| {
        format!("{method} is served by the Paneflow worker, which is not reachable: {reason}")
    };
    if sandboxed {
        wait_for_pane_endpoint(&endpoint);
    } else if HostControl::open(&endpoint, CLIENT_NAME, REQUEST_DEADLINE).is_err() {
        let controller = std::env::current_exe().map_err(|error| {
            unreachable(format!("cannot locate the paneflow executable: {error}"))
        })?;
        paneflow_serve::ensure_worker_running(&home, &controller)
            .map_err(|error| unreachable(error.to_string()))?;
    }
    let mut control = HostControl::open(&endpoint, CLIENT_NAME, REQUEST_DEADLINE)
        .map_err(|error| unreachable(format!("{} ({error:?})", endpoint.display())))?;
    control.request_with_deadline(
        method,
        params,
        paneflow_api::methods::config::reply_deadline(method, REQUEST_DEADLINE),
    )
}

impl IpcTransport for CliTransport {
    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        if worker_owned(method) {
            let answer = match (worker_call(method, params.clone()), self) {
                (Err(error), Self::Controller(client)) if worker_lacks(&error, method) => {
                    client.call(method, params)
                }
                (answer, _) => answer,
            };
            return answer.map(|result| worker_state::with_worker_state(method, result));
        }
        match self {
            Self::Controller(client) => client
                .call(method, params)
                .map(|result| worker_state::with_worker_state(method, result)),
            Self::Host(transport) => match headless::current_worker_layout() {
                Some(layout) => windowless_call(transport, &layout, method, params),
                None => transport
                    .call(method, params)
                    .map(|result| worker_state::with_worker_state(method, result)),
            },
        }
    }
}

fn windowless_call(
    transport: &HostTransport,
    layout: &headless::WorkerLayout,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let requested = headless::requested_layout_surface(&params, layout);
    let params = headless::name_sessions(params, layout);
    match method {
        "surface.list" => transport.call(method, params).map(|listed| {
            headless::with_layout_ids(listed, layout, &paneflow_config::loader::load_config())
        }),
        "fleet.list" => {
            let listed = transport.call("surface.list", serde_json::json!({}))?;
            let listed = headless::with_layout_ids(
                listed,
                layout,
                &paneflow_config::schema::PaneFlowConfig::default(),
            );
            let projections = worker_state::worker_projections().unwrap_or_default();
            Ok(headless::fleet(
                layout,
                &headless::surface_names(&listed),
                &projections,
            ))
        }
        "surface.status" => transport.call(method, params).map(|status| {
            let projections = worker_state::worker_projections().unwrap_or_default();
            let status = headless::with_requested_surface(status, requested);
            worker_state::with_projections(
                method,
                headless::agent_status(status, &projections),
                &projections,
            )
        }),
        _ => transport
            .call(method, params)
            .map(|result| headless::with_requested_surface(result, requested)),
    }
}

const CLIENT_NAME: &str = "paneflow-cli";

fn connect() -> Result<CliTransport, String> {
    match resolve_control_target(
        paneflow_home::ipc_endpoint(),
        paneflow_host::endpoint::host_endpoint_path_for_current_home(),
        paneflow_home::reserved_host_endpoint(),
    ) {
        Some(ControlTarget::Controller(socket)) => {
            Ok(CliTransport::Controller(IpcClient::new(socket)))
        }
        Some(ControlTarget::Host(endpoint)) => HostTransport::connect(&endpoint, CLIENT_NAME)
            .map(|transport| CliTransport::Host(Box::new(transport)))
            .map_err(|error| format!("paneflow: {error}; start it with `paneflow host start`")),
        None => Err(
            "paneflow: cannot locate the IPC socket; is Paneflow running? \
         (set PANEFLOW_SOCKET_PATH if you launched the CLI outside a Paneflow pane)"
                .to_string(),
        ),
    }
}

fn dispatch(command: Commands, client: &CliTransport) -> Result<i32, CliError> {
    match command {
        Commands::Ls {
            human,
            json: _,
            raw,
            scope,
        } => read_cmds::ls(
            client,
            read_cmds::ListOptions {
                format: if human {
                    read_cmds::ListFormat::Table
                } else {
                    read_cmds::ListFormat::Json
                },
                reach: selector::Reach::from_scope_all(scope == "all"),
                raw,
            },
        ),
        Commands::Read {
            target,
            lines,
            offset,
            json,
            raw,
            scope,
        } => read_cmds::read(
            client,
            &target,
            &read_cmds::ReadOptions {
                lines,
                offset,
                json,
                raw,
                reach: selector::Reach::from_scope_all(scope == "all"),
            },
        ),
        Commands::Search {
            target,
            pattern,
            max,
            human,
            json,
            raw,
            scope,
        } => read_cmds::search(
            client,
            &target,
            &pattern,
            &read_cmds::SearchOptions {
                max,
                format: if json {
                    read_cmds::SearchFormat::Json
                } else if human {
                    read_cmds::SearchFormat::Lines
                } else {
                    read_cmds::SearchFormat::Default
                },
                raw,
                reach: selector::Reach::from_scope_all(scope == "all"),
            },
        ),
        Commands::Ps { json } => read_cmds::ps(client, json),
        Commands::Status { target, json } => read_cmds::status(client, &target, json),
        Commands::New { name, cwd, space } => {
            control_cmds::new_workspace(client, name.as_deref(), cwd.as_deref(), space.as_deref())
        }
        Commands::Select { index } => control_cmds::select(client, index),
        Commands::Split { direction, target } => {
            control_cmds::split(client, direction.as_ipc(), target.as_deref())
        }
        Commands::Send {
            target,
            text,
            broadcast,
            submit,
            paste,
            report_file,
            force,
            scope,
        } => send_cmd::send(
            client,
            &target,
            &text,
            send_cmd::SendOptions {
                broadcast,
                submit,
                paste,
                force,
                scope_all: scope == "all",
            },
            report_file.as_deref(),
        ),
        Commands::Focus { target } => control_cmds::focus(client, &target),
        Commands::Key {
            target,
            keystroke,
            scope,
        } => send_cmd::key(client, &target, &keystroke, scope == "all"),
        Commands::Flow(FlowCommand::Run {
            file,
            dry_run,
            json,
            scope,
        }) => flow_cmd::run(client, &file, dry_run, json, scope == "all"),
        Commands::Up { file, dry_run } => up_cmd::up(client, &file, dry_run),
        Commands::Wait {
            selector,
            pattern,
            idle,
            for_ms,
            timeout,
            any,
            all,
        } => {
            if idle {
                wait_cmd::wait_idle(
                    client,
                    client.controller_socket().as_deref(),
                    &selector,
                    for_ms,
                    timeout,
                    pattern.as_deref(),
                )
            } else {
                let Some(pattern) = pattern else {
                    return Err(CliError::runtime(
                        "wait requires --pattern <regex> unless --idle is set",
                    ));
                };
                let mode = if all {
                    wait_cmd::MatchMode::All
                } else if any {
                    wait_cmd::MatchMode::Any
                } else {
                    wait_cmd::MatchMode::Single
                };
                wait_cmd::wait(client, &selector, &pattern, timeout, mode)
            }
        }
        Commands::Watch {
            surface,
            types,
            events_only,
        } => match client.controller_socket() {
            Some(socket) => {
                watch_cmd::watch(client, &socket, surface.as_deref(), &types, events_only)
            }
            None => Err(CliTransport::no_controller("paneflow watch")),
        },
        Commands::Api(command) => api_cmd::run(command),
        Commands::Do(args) => Ok(do_cmd::run(args)),
        Commands::Actions { json } => config_cmd::actions(json),
        Commands::Config(command) => config_cmd::run(command),
        Commands::Grants(command) => grants_cmd::run(command),
        Commands::Host(command) => host_cmd::run(command),
        Commands::Serve(command) => serve_cmd::run(command),
        Commands::Sessions {
            command,
            follow,
            json,
        } => sessions_cmd::run(command, follow, json),
        Commands::Events {
            json,
            types,
            since,
            workspace_id,
        } => events_cmd::run(&types, since, workspace_id, json),
    }
}

pub(super) fn print_json(value: &Value) -> Result<(), CliError> {
    let rendered = serde_json::to_string_pretty(value)
        .map_err(|e| CliError::runtime(format!("failed to render JSON: {e}")))?;
    println!("{rendered}");
    Ok(())
}

pub(super) fn reject_legacy_error(result: Value) -> Result<Value, CliError> {
    if let Some(message) = result.get("error").and_then(Value::as_str) {
        return Err(CliError::runtime(message.to_string()));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_worker_owned_call_from_a_sandboxed_pane_goes_to_the_pane_endpoint() {
        let home = std::path::Path::new("/home/someone/.paneflow-dev");
        let pane = paneflow_home::pane_endpoint_path(
            home,
            "0b5f3c1e-7d1a-4f0e-9a4b-2c8d6e1f3a5b",
            paneflow_home::PaneServer::Worker,
        );
        assert_eq!(
            worker_endpoint_for(home, Some(pane.as_os_str())),
            (pane, true)
        );
        let shared = paneflow_home::serve_endpoint_path(home);
        assert_eq!(worker_endpoint_for(home, None), (shared.clone(), false));
        assert_eq!(
            worker_endpoint_for(home, Some("/run/user/1000/elsewhere.sock".as_ref())),
            (shared, false),
            "a path outside the home's panes directory is not a pane endpoint"
        );
    }

    #[test]
    fn only_a_worker_that_lacks_the_method_sends_the_call_to_the_window() {
        assert!(worker_lacks(
            "paneflow error -32601: method not handled by the worker: surface.split",
            "surface.split"
        ));
        assert!(!worker_lacks(
            "paneflow error -32601: method not handled by the worker: surface.split",
            "workspace.up"
        ));
        assert!(!worker_lacks(
            "paneflow error -32601: surface.split orchestration disabled; set PANEFLOW_IPC_ORCHESTRATION=1 \
             or PANEFLOW_IPC_SCRIPTING=1 to enable command, prompt, or env",
            "surface.split"
        ));
    }

    #[test]
    fn global_help_lists_every_verb() {
        for macos in [false, true] {
            let help = global_help("0.0.0", macos);
            for verb in VERBS
                .iter()
                .copied()
                .chain(EXTERNAL_VERBS.iter().map(|(verb, _)| *verb))
            {
                assert!(
                    help.lines().any(|line| line.starts_with("  ")
                        && line.split([',', ' ']).any(|word| word == verb)),
                    "`{verb}` missing from --help:\n{help}"
                );
            }
            assert!(help.contains("paneflow help"));
        }
    }

    #[test]
    fn every_clap_verb_is_dispatched_as_a_cli_verb() {
        use clap::CommandFactory;

        for command in Cli::command().get_subcommands() {
            for name in std::iter::once(command.get_name()).chain(command.get_all_aliases()) {
                assert!(is_cli_verb(Some(name)), "`{name}` is not routed to the CLI");
            }
        }
    }

    #[test]
    fn help_points_at_the_paneflow_json_gates_and_the_framed_read() {
        use clap::CommandFactory;

        let mut cli = Cli::command();
        let mut rendered = String::new();
        for name in ["send", "key", "flow", "read"] {
            let mut found = cli.find_subcommand_mut(name).expect(name).clone();
            for sub in found.get_subcommands_mut() {
                rendered.push_str(&sub.render_long_help().to_string());
            }
            rendered.push_str(&found.render_long_help().to_string());
        }
        rendered.push_str(&global_help("0.0.0", cfg!(target_os = "macos")));
        assert!(!rendered.contains("PANEFLOW_IPC_"), "{rendered}");
        assert!(rendered.contains("ipc.scripting"), "{rendered}");
        assert!(rendered.contains("ipc.orchestration"), "{rendered}");
        assert!(!rendered.contains("raw text by default"), "{rendered}");
    }

    #[test]
    fn global_help_uses_the_platform_secondary_key() {
        let linux = global_help("0.0.0", false);
        assert!(linux.contains("Ctrl+Shift+D/E"));
        assert!(linux.contains("Ctrl+1-9"));
        assert!(linux.contains("Alt+Arrow"));
        assert!(!linux.contains("Cmd+"));

        let macos = global_help("0.0.0", true);
        assert!(macos.contains("Cmd+Shift+D/E"));
        assert!(macos.contains("Cmd+Shift+W"));
        assert!(macos.contains("Cmd+1-9"));
        assert!(macos.contains("Option+Arrow"));
        assert!(macos.contains("Ctrl+Tab"));
        assert!(!macos.contains("Ctrl+Shift"));
    }

    #[test]
    fn an_unknown_verb_points_to_a_help_that_lists_the_verbs() {
        assert!(looks_like_unknown_verb(Some("sned")));
        let message = unknown_verb_message("sned");
        assert!(message.contains("`paneflow --help`"), "{message}");
        let help = global_help("0.0.0", cfg!(target_os = "macos"));
        assert!(help.contains("\nVerbs:\n"));
        assert!(
            help.lines()
                .any(|line| line.trim_start().starts_with("send "))
        );
        assert!(!looks_like_unknown_verb(Some("help")));
        assert!(is_help_verb(Some("help")));
    }

    #[test]
    fn a_window_only_action_names_what_the_host_cannot_do() {
        let error = CliTransport::no_controller("paneflow watch");
        assert_eq!(error.code, EXIT_RUNTIME);
        assert!(
            error.message.contains("paneflow watch"),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("Paneflow window"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_host_transport_offers_no_controller_socket() {
        let socket = std::path::PathBuf::from("/tmp/paneflow-host.sock");
        assert_eq!(
            paneflow_ipc_client::host_control::choose_control_target(None, false, Some(socket)),
            Some(ControlTarget::Host(std::path::PathBuf::from(
                "/tmp/paneflow-host.sock"
            ))),
            "with no window listening the CLI targets the local host"
        );
    }

    #[test]
    fn is_cli_verb_matches_known_verbs() {
        assert!(is_cli_verb(Some("ls")));
        assert!(is_cli_verb(Some("send")));
        assert!(is_cli_verb(Some("focus")));
        assert!(is_cli_verb(Some("key")));
        assert!(!is_cli_verb(Some("mcp")));
        assert!(!is_cli_verb(Some("--version")));
        assert!(!is_cli_verb(None));
    }

    #[test]
    fn mcp_tool_names_alias_to_their_verbs() {
        assert!(is_cli_verb(Some("search_pane")));
        assert!(is_cli_verb(Some("read_pane")));
        assert!(is_cli_verb(Some("list_panes")));
        let cli = Cli::try_parse_from(["paneflow", "search_pane", "backend", "needle"])
            .expect("parse search_pane");
        assert!(matches!(cli.command, Some(Commands::Search { .. })));
        let cli =
            Cli::try_parse_from(["paneflow", "read_pane", "backend"]).expect("parse read_pane");
        assert!(matches!(cli.command, Some(Commands::Read { .. })));
        let cli = Cli::try_parse_from(["paneflow", "list_panes"]).expect("parse list_panes");
        assert!(matches!(cli.command, Some(Commands::Ls { .. })));
    }

    #[test]
    fn unknown_verb_detected_but_bare_and_flags_are_not() {
        assert!(looks_like_unknown_verb(Some("blah")));
        assert!(looks_like_unknown_verb(Some("searh")));
        assert!(!looks_like_unknown_verb(Some("search")));
        assert!(!looks_like_unknown_verb(Some("search_pane")));
        assert!(!looks_like_unknown_verb(Some("ls")));
        assert!(!looks_like_unknown_verb(None));
        assert!(!looks_like_unknown_verb(Some("")));
        assert!(!looks_like_unknown_verb(Some("--help")));
        assert!(!looks_like_unknown_verb(Some("-v")));
        assert!(!looks_like_unknown_verb(Some("--update-and-exit")));
    }

    #[test]
    fn ps_parses_with_optional_json_flag() {
        let cli = Cli::try_parse_from(["paneflow", "ps", "--json"]).expect("parse");
        assert!(matches!(cli.command, Some(Commands::Ps { json: true })));
        let cli = Cli::try_parse_from(["paneflow", "ps"]).expect("parse");
        assert!(matches!(cli.command, Some(Commands::Ps { json: false })));
    }

    #[test]
    fn status_requires_a_target() {
        let err = Cli::try_parse_from(["paneflow", "status"]).expect_err("usage");
        assert_eq!(err.exit_code(), 2);
        let cli = Cli::try_parse_from(["paneflow", "status", "backend"]).expect("parse");
        assert!(matches!(cli.command, Some(Commands::Status { .. })));
    }

    #[test]
    fn send_flags_default_off() {
        let cli = Cli::try_parse_from(["paneflow", "send", "backend", "hi"]).expect("parse");
        assert!(matches!(
            cli.command,
            Some(Commands::Send {
                broadcast: false,
                submit: false,
                paste: false,
                ..
            })
        ));
        let cli = Cli::try_parse_from(["paneflow", "send", "--broadcast", "--submit", "sh", "go"])
            .expect("parse");
        assert!(matches!(
            cli.command,
            Some(Commands::Send {
                broadcast: true,
                submit: true,
                paste: false,
                ..
            })
        ));
        let cli =
            Cli::try_parse_from(["paneflow", "send", "--paste", "agent", "hi"]).expect("parse");
        assert!(matches!(
            cli.command,
            Some(Commands::Send { paste: true, .. })
        ));
        let cli = Cli::try_parse_from([
            "paneflow",
            "send",
            "--report-file",
            "reports/out.md",
            "agent",
            "hi",
        ])
        .expect("parse");
        assert!(
            matches!(cli.command, Some(Commands::Send { report_file: Some(p), .. }) if p == "reports/out.md")
        );
    }

    #[test]
    fn split_target_is_optional() {
        let cli = Cli::try_parse_from(["paneflow", "split", "v"]).expect("parse");
        assert!(matches!(
            cli.command,
            Some(Commands::Split { target: None, .. })
        ));
        let cli =
            Cli::try_parse_from(["paneflow", "split", "v", "--target", "backend"]).expect("parse");
        assert!(
            matches!(cli.command, Some(Commands::Split { target: Some(t), .. }) if t == "backend")
        );
    }

    #[test]
    fn key_requires_target_and_keystroke() {
        let err = Cli::try_parse_from(["paneflow", "key", "backend"]).expect_err("usage");
        assert_eq!(err.exit_code(), 2);
        let cli = Cli::try_parse_from(["paneflow", "key", "backend", "escape"]).expect("parse");
        assert!(matches!(cli.command, Some(Commands::Key { .. })));
    }

    #[test]
    fn wait_idle_and_pattern_parsing() {
        let cli = Cli::try_parse_from([
            "paneflow", "wait", "--match", "agent", "--idle", "--for", "500",
        ])
        .expect("parse");
        assert!(matches!(
            cli.command,
            Some(Commands::Wait {
                idle: true,
                pattern: None,
                for_ms: Some(500),
                ..
            })
        ));
        let cli = Cli::try_parse_from([
            "paneflow",
            "wait",
            "--match",
            "a",
            "--idle",
            "--pattern",
            "DONE",
        ])
        .expect("parse");
        assert!(
            matches!(cli.command, Some(Commands::Wait { idle: true, pattern: Some(p), .. }) if p == "DONE")
        );
        let cli = Cli::try_parse_from(["paneflow", "wait", "--match", "a", "--pattern", "DONE"])
            .expect("parse");
        assert!(matches!(
            cli.command,
            Some(Commands::Wait {
                idle: false,
                pattern: Some(_),
                ..
            })
        ));
        let err = Cli::try_parse_from(["paneflow", "wait", "--match", "a"]).expect_err("usage");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn watch_parses_optional_surface_and_repeatable_types() {
        let cli = Cli::try_parse_from(["paneflow", "watch"]).expect("parse");
        assert!(matches!(
            cli.command,
            Some(Commands::Watch { surface: None, .. })
        ));
        let cli = Cli::try_parse_from([
            "paneflow",
            "watch",
            "--surface",
            "backend",
            "--type",
            "ai.stop",
            "--type",
            "ai.notification",
        ])
        .expect("parse");
        match cli.command {
            Some(Commands::Watch {
                surface,
                types,
                events_only,
            }) => {
                assert_eq!(surface.as_deref(), Some("backend"));
                assert_eq!(types, vec!["ai.stop", "ai.notification"]);
                assert!(!events_only);
            }
            other => panic!("expected Watch, got {other:?}"),
        }
    }

    #[test]
    fn watch_parses_events_only() {
        let cli = Cli::try_parse_from(["paneflow", "watch", "--events-only"]).expect("parse");
        assert!(matches!(
            cli.command,
            Some(Commands::Watch {
                events_only: true,
                ..
            })
        ));
    }

    #[test]
    fn host_verbs_parse_and_route_before_the_gui_socket() {
        assert!(is_cli_verb(Some("host")));
        for verb in ["start", "status", "stop"] {
            let cli = Cli::try_parse_from(["paneflow", "host", verb]).expect("parse");
            assert!(matches!(cli.command, Some(Commands::Host(_))), "{verb}");
        }
        let err = Cli::try_parse_from(["paneflow", "host"]).expect_err("usage");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn serve_verbs_parse_and_route_before_the_gui_socket() {
        assert!(is_cli_verb(Some("serve")));
        for verb in ["start", "status", "stop"] {
            let cli = Cli::try_parse_from(["paneflow", "serve", verb]).expect("parse");
            assert!(matches!(cli.command, Some(Commands::Serve(_))), "{verb}");
        }
        let cli = Cli::try_parse_from(["paneflow", "serve", "run"]).expect("parse run");
        assert!(matches!(cli.command, Some(Commands::Serve(_))));
        let err = Cli::try_parse_from(["paneflow", "serve"]).expect_err("usage");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn the_sessions_verb_routes_to_the_worker_before_the_gui_socket() {
        assert!(is_cli_verb(Some("sessions")));
        let cli = Cli::try_parse_from(["paneflow", "sessions", "--follow", "--json"])
            .expect("parse the Controller stream");
        assert!(matches!(
            cli.command,
            Some(Commands::Sessions {
                command: None,
                follow: true,
                json: true,
            })
        ));
        let bare = Cli::try_parse_from(["paneflow", "sessions"]).expect("parse the snapshot");
        assert!(matches!(
            bare.command,
            Some(Commands::Sessions {
                command: None,
                follow: false,
                json: false,
            })
        ));
        let resume = Cli::try_parse_from(["paneflow", "sessions", "resume", "01JABC"])
            .expect("parse the resume");
        assert!(matches!(
            resume.command,
            Some(Commands::Sessions {
                command: Some(sessions_cmd::SessionsCommand::Resume { .. }),
                ..
            })
        ));
        let ack =
            Cli::try_parse_from(["paneflow", "sessions", "ack", "01JABC"]).expect("parse ack");
        assert!(matches!(
            ack.command,
            Some(Commands::Sessions {
                command: Some(sessions_cmd::SessionsCommand::Ack { .. }),
                ..
            })
        ));
        let err = Cli::try_parse_from(["paneflow", "sessions", "ack"]).expect_err("usage");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn cli_parses_a_verb_with_flags() {
        let cli = Cli::try_parse_from(["paneflow", "ls", "--human"]).expect("parse");
        assert!(matches!(
            cli.command,
            Some(Commands::Ls { human: true, .. })
        ));
    }

    #[test]
    fn split_accepts_short_aliases() {
        let cli = Cli::try_parse_from(["paneflow", "split", "h"]).expect("parse");
        assert!(matches!(
            cli.command,
            Some(Commands::Split {
                direction: SplitDir::Horizontal,
                ..
            })
        ));
    }

    #[test]
    fn read_requires_a_target() {
        let err = Cli::try_parse_from(["paneflow", "read"]).expect_err("usage");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn no_subcommand_parses_to_none() {
        let cli = Cli::try_parse_from(["paneflow"]).expect("parse");
        assert!(cli.command.is_none());
    }
}
