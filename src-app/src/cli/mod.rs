use clap::{Parser, Subcommand, ValueEnum};
use paneflow_ipc_client::IpcClient;
use serde_json::Value;

mod control_cmds;
mod flow_cmd;
mod flow_spec;
mod read_cmds;
mod selector;
mod send_cmd;
mod up_cmd;
mod wait_cmd;
mod watch_cmd;
mod workspace_spec;

pub const EXIT_OK: i32 = 0;
pub const EXIT_RUNTIME: i32 = 1;
pub const EXIT_TARGET: i32 = 3;
pub const EXIT_TIMEOUT: i32 = 4;

const VERBS: &[&str] = &[
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
    "list_panes",
    "read_pane",
    "search_pane",
];

pub fn is_cli_verb(arg: Option<&str>) -> bool {
    matches!(arg, Some(v) if VERBS.contains(&v))
}

pub fn looks_like_unknown_verb(arg: Option<&str>) -> bool {
    matches!(arg, Some(v) if !v.is_empty() && !v.starts_with('-') && !VERBS.contains(&v))
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
    #[command(alias = "list_panes", about = "List terminal surfaces")]
    Ls {
        #[arg(long, help = "Human-readable table instead of the default JSON")]
        human: bool,
    },
    #[command(
        alias = "read_pane",
        about = "Print a pane's scrollback (raw text by default)"
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
    },
    #[command(
        alias = "search_pane",
        about = "Search a pane's scrollback for a substring/pattern"
    )]
    Search {
        #[arg(help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`")]
        target: String,
        #[arg(help = "Pattern to search for")]
        pattern: String,
        #[arg(long, help = "Cap the number of matches (server clamps to 1..1000)")]
        max: Option<u64>,
        #[arg(long, help = "Human-readable lines instead of the default JSON")]
        human: bool,
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
        long_about = "Inject text into a pane WITHOUT submitting it (human-in-loop).\n\nRequires `PANEFLOW_IPC_SCRIPTING=1` on the running instance; the text is written verbatim with no trailing newline so the user/agent reviews and presses Enter themselves - unless `--submit` is passed explicitly."
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
    },
    #[command(about = "Give a targeted surface the keyboard focus")]
    Focus {
        #[arg(help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`")]
        target: String,
    },
    #[command(
        about = "Send a named keystroke (e.g. `escape`, `ctrl-c`, `tab`) to a pane",
        long_about = "Send a named keystroke (e.g. `escape`, `ctrl-c`, `tab`) to a pane.\n\nRequires `PANEFLOW_IPC_SCRIPTING=1` on the running instance. Keystrokes that would submit a line (`enter`, `ctrl-m`, `ctrl-j`) are refused - submission is exclusive to `send --submit`."
    )]
    Key {
        #[arg(help = "Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`")]
        target: String,
        #[arg(help = "Dash-separated keystroke description (\"escape\", \"ctrl-c\", \"alt-f\")")]
        keystroke: String,
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
            help = "Regex to wait for in the pane's recent scrollback. Required unless `--idle` is set. With `--idle` it is an optional sentinel: it is checked on each new output and EITHER signal (pattern match OR going idle) returns first (EP-003 US-008)"
        )]
        pattern: Option<String>,
        #[arg(
            long,
            help = "Wait until the pane's output goes quiet (no `output_generation` change for `--for` ms) by subscribing to the push stream - zero client-side polling (EP-003 US-007). Single-target"
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

fn connect() -> Result<IpcClient, String> {
    let socket = paneflow_ipc_client::resolve_socket_path().ok_or_else(|| {
        "paneflow: cannot locate the IPC socket; is Paneflow running? \
         (set PANEFLOW_SOCKET_PATH if you launched the CLI outside a Paneflow pane)"
            .to_string()
    })?;
    Ok(IpcClient::new(socket))
}

fn dispatch(command: Commands, client: &IpcClient) -> Result<i32, CliError> {
    match command {
        Commands::Ls { human } => read_cmds::ls(client, human),
        Commands::Read {
            target,
            lines,
            offset,
            json,
            raw,
        } => read_cmds::read(client, &target, lines, offset, json, raw),
        Commands::Search {
            target,
            pattern,
            max,
            human,
        } => read_cmds::search(client, &target, &pattern, max, human),
        Commands::Ps { json } => read_cmds::ps(client, json),
        Commands::Status { target, json } => read_cmds::status(client, &target, json),
        Commands::New { name, cwd } => {
            control_cmds::new_workspace(client, name.as_deref(), cwd.as_deref())
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
        } => send_cmd::send(
            client,
            &target,
            &text,
            broadcast,
            submit,
            paste,
            report_file.as_deref(),
        ),
        Commands::Focus { target } => control_cmds::focus(client, &target),
        Commands::Key { target, keystroke } => send_cmd::key(client, &target, &keystroke),
        Commands::Flow(FlowCommand::Run {
            file,
            dry_run,
            json,
        }) => flow_cmd::run(client, &file, dry_run, json),
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
                wait_cmd::wait_idle(client, &selector, for_ms, timeout, pattern.as_deref())
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
        } => watch_cmd::watch(client, surface.as_deref(), &types, events_only),
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
    fn cli_parses_a_verb_with_flags() {
        let cli = Cli::try_parse_from(["paneflow", "ls", "--human"]).expect("parse");
        assert!(matches!(cli.command, Some(Commands::Ls { human: true })));
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
