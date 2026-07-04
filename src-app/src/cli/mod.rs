//! `paneflow <verb>` scriptable CLI (EP-001, prd-cli-agent-orchestration).
//!
//! Talks to a RUNNING Paneflow instance over the existing IPC JSON-RPC socket
//! (`paneflow-ipc-client`) and exits before any GPUI init. `main.rs` dispatches
//! here only when `argv[1]` names a known verb ([`is_cli_verb`]) - mirroring the
//! `paneflow mcp …` intercept - so every other invocation (no args, unknown
//! args, `--help`/`--version`/`--update-and-exit`) is left untouched and the GUI
//! launch path is preserved. clap therefore never has to own the "no subcommand
//! => launch the GUI" default, and never eats the manually-parsed top-level
//! flags handled above it.

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

/// Process exit codes. Kept distinct so scripts can branch on the failure
/// kind. clap owns `2` for its own usage/parse errors (and `0` for
/// `--help`/`--version`), so the runtime codes start at `1` and avoid `2`.
pub const EXIT_OK: i32 = 0;
pub const EXIT_RUNTIME: i32 = 1;
pub const EXIT_TARGET: i32 = 3;
/// `wait` reached its deadline without the pattern appearing. Distinct from
/// EXIT_TARGET (no/ambiguous match) and EXIT_RUNTIME (instance down / pane
/// closed) so scripts can tell a timeout apart from a hard failure.
pub const EXIT_TIMEOUT: i32 = 4;

/// The verbs this CLI owns. `main.rs` gates the whole CLI dispatch (and the
/// manual `--help`/`--version` scans) on membership here so the GUI launch
/// path stays byte-for-byte unchanged for any other `argv[1]`.
///
/// EP-005 US-011: the trailing `list_panes`/`read_pane`/`search_pane` are the
/// `paneflow` MCP tool names, accepted as CLI aliases (clap maps each to its
/// canonical subcommand via `#[command(alias = ...)]`) so a conductor that types
/// the tool name reaches the matching verb instead of tripping the GUI
/// single-instance guard. This list only gates the `main.rs` intercept.
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

/// True when `argv[1]` names one of our subcommands (including the MCP-tool
/// aliases above).
pub fn is_cli_verb(arg: Option<&str>) -> bool {
    matches!(arg, Some(v) if VERBS.contains(&v))
}

/// True when `argv[1]` is shaped like a subcommand (present, non-empty, and not
/// a `-`/`--` flag) but is NOT one this CLI owns. `main.rs` calls this only
/// AFTER the `mcp`/`hooks`/known-verb intercepts have each had their chance and
/// exited, so a `true` here is an unmistakable typo (`paneflow blah`, a
/// mistyped `paneflow searh`): it prints an actionable "unknown verb" error and
/// exits non-zero instead of falling through to the GUI launch, which would
/// otherwise trip the single-instance guard with no message (EP-005 US-011).
/// A bare `paneflow` (argv[1] is `None`) returns `false` so the GUI still
/// launches, and a leading-`-` token is a flag, not a verb, so it stays on the
/// GUI/global-flag path.
pub fn looks_like_unknown_verb(arg: Option<&str>) -> bool {
    matches!(arg, Some(v) if !v.is_empty() && !v.starts_with('-') && !VERBS.contains(&v))
}

#[derive(Parser, Debug)]
#[command(
    name = "paneflow",
    version,
    about = "Drive a running Paneflow instance from the shell",
    // The GUI launch (no subcommand) is handled in main.rs, never here, so a
    // bare `paneflow` never reaches clap. `Option<Commands>` keeps clap from
    // forcing `subcommand_required` / `arg_required_else_help` regardless.
    subcommand_required = false,
    arg_required_else_help = false
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List terminal surfaces.
    // EP-005 US-011: `list_panes` is the MCP tool name; accept it as a hidden
    // alias so a conductor can type either.
    #[command(alias = "list_panes")]
    Ls {
        /// Human-readable table instead of the default JSON.
        #[arg(long)]
        human: bool,
    },
    /// Print a pane's scrollback (raw text by default).
    #[command(alias = "read_pane")]
    Read {
        /// Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`.
        target: String,
        /// Number of trailing lines (server clamps to 1..4000).
        #[arg(long)]
        lines: Option<u64>,
        /// Offset from the end of the buffer.
        #[arg(long)]
        offset: Option<u64>,
        /// Emit the `{text, lines, total_lines, eof}` envelope as JSON.
        #[arg(long)]
        json: bool,
        /// Return raw scrollback, bypassing the anti-injection fence that
        /// otherwise wraps the output as `<untrusted_terminal_output>` (the
        /// fence is on by default; see the ai_injection_fence setting).
        #[arg(long)]
        raw: bool,
    },
    /// Search a pane's scrollback for a substring/pattern.
    #[command(alias = "search_pane")]
    Search {
        /// Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`.
        target: String,
        /// Pattern to search for.
        pattern: String,
        /// Cap the number of matches (server clamps to 1..1000).
        #[arg(long)]
        max: Option<u64>,
        /// Human-readable lines instead of the default JSON.
        #[arg(long)]
        human: bool,
    },
    /// List running agents across the fleet (pid, tool, state, pane).
    Ps {
        /// Emit the `{agents:[…]}` envelope as JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Read one surface's agent state (thinking / waiting / idle / errored / …).
    Status {
        /// Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`.
        target: String,
        /// Emit the status envelope as JSON instead of a one-line summary.
        #[arg(long)]
        json: bool,
    },
    /// Create a new workspace.
    New {
        /// Workspace title.
        #[arg(long)]
        name: Option<String>,
        /// Working directory for the first pane (must exist).
        #[arg(long)]
        cwd: Option<String>,
    },
    /// Select a workspace by index.
    Select {
        /// Zero-based workspace index.
        index: u64,
    },
    /// Split the active pane horizontally or vertically.
    Split {
        /// `h`/`horizontal` (panes stacked) or `v`/`vertical` (side by side).
        direction: SplitDir,
        /// Split the pane hosting this target instead of the first leaf.
        /// Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`.
        #[arg(long)]
        target: Option<String>,
    },
    /// Inject text into a pane WITHOUT submitting it (human-in-loop).
    ///
    /// Requires `PANEFLOW_IPC_SCRIPTING=1` on the running instance; the text is
    /// written verbatim with no trailing newline so the user/agent reviews and
    /// presses Enter themselves - unless `--submit` is passed explicitly.
    Send {
        /// Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`.
        target: String,
        /// Text to inject (no trailing carriage return is added by default).
        text: String,
        /// Send to EVERY pane matching the target (a multi-match selector is
        /// an error without this flag). Prints a `{sent, failed}` report.
        #[arg(long)]
        broadcast: bool,
        /// Submit the text (append a carriage return). Explicit opt-in: this
        /// is the ONLY way the CLI ever submits on the user's behalf, and it
        /// still requires the instance-side scripting gate.
        #[arg(long)]
        submit: bool,
        /// Force bracketed-paste delivery: the text is wrapped in
        /// `ESC[200~`/`ESC[201~` and, with `--submit`, the carriage return is
        /// sent separately after a calibrated delay so a TUI agent does not
        /// swallow it (EP-001). `--submit` toward an agent pane enables this
        /// automatically; pass `--paste` to force it (e.g. toward a shell) or
        /// `--paste` alone to wrap a non-submitted inject.
        #[arg(long)]
        paste: bool,
        /// Ask the agent to write its complete result to this file and print
        /// `REPORT_DONE <path>` after the file is fully written. The path is
        /// resolved relative to the caller's current directory.
        #[arg(long, value_name = "PATH")]
        report_file: Option<String>,
    },
    /// Give a targeted surface the keyboard focus.
    Focus {
        /// Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`.
        target: String,
    },
    /// Send a named keystroke (e.g. `escape`, `ctrl-c`, `tab`) to a pane.
    ///
    /// Requires `PANEFLOW_IPC_SCRIPTING=1` on the running instance. Keystrokes
    /// that would submit a line (`enter`, `ctrl-m`, `ctrl-j`) are refused -
    /// submission is exclusive to `send --submit`.
    Key {
        /// Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`.
        target: String,
        /// Dash-separated keystroke description ("escape", "ctrl-c", "alt-f").
        keystroke: String,
    },
    /// Run a declarative agent DAG from a `flow.toml` (orchestration engine).
    #[command(subcommand)]
    Flow(FlowCommand),
    /// Spawn a declarative agent workspace from a TOML file ("compose for agents").
    Up {
        /// Path to a `paneflow.workspace.toml` spec.
        file: String,
        /// Validate + print the resolved plan without touching the instance.
        #[arg(long)]
        dry_run: bool,
    },
    /// Block until a pane goes idle, or a regex appears in its output (orchestration).
    Wait {
        /// Target: surface id, name, `cmdline:<substr>`, or `cwd:<path>`.
        /// Note: `cmdline:` matches the full argv on Linux but only the
        /// executable basename on macOS/Windows; prefer `cwd:` or a name for a
        /// portable selector.
        #[arg(long = "match", value_name = "SELECTOR")]
        selector: String,
        /// Regex to wait for in the pane's recent scrollback. Required unless
        /// `--idle` is set. With `--idle` it is an optional sentinel: it is
        /// checked on each new output and EITHER signal (pattern match OR going
        /// idle) returns first (EP-003 US-008).
        #[arg(long, required_unless_present = "idle")]
        pattern: Option<String>,
        /// Wait until the pane's output goes quiet (no `output_generation`
        /// change for `--for` ms) by subscribing to the push stream - zero
        /// client-side polling (EP-003 US-007). Single-target.
        #[arg(long)]
        idle: bool,
        /// With `--idle`: the quiescence window in milliseconds (default 1000).
        /// The pane must produce no new output for this long to count as idle.
        #[arg(long = "for", value_name = "MS")]
        for_ms: Option<u64>,
        /// Max seconds to wait before giving up (default 300).
        #[arg(long)]
        timeout: Option<u64>,
        /// Succeed as soon as ANY matching pane matches (selector may hit
        /// several). `--pattern` mode only; ignored with `--idle`.
        #[arg(long, conflicts_with = "all")]
        any: bool,
        /// Require ALL matching panes to match the pattern. `--pattern` mode only.
        #[arg(long)]
        all: bool,
    },
    /// Stream lifecycle events from the running instance as JSONL (EP-002).
    Watch {
        /// Only stream events for this pane (selector). Omit for all panes.
        #[arg(long)]
        surface: Option<String>,
        /// Only stream these event types (repeatable). Omit for all types.
        #[arg(long = "type", value_name = "TYPE")]
        types: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum FlowCommand {
    /// Execute (or validate with --dry-run) a flow file against the running
    /// instance. Spawns panes, waits on `ready` barriers, feeds steps -
    /// submission only with explicit `submit = true` + the scripting gate.
    Run {
        /// Path to a `flow.toml`.
        file: String,
        /// Validate + print the resolved plan without touching the instance.
        #[arg(long)]
        dry_run: bool,
        /// Final machine-readable report on stdout (live transitions move to
        /// stderr).
        #[arg(long)]
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
    /// The `direction` string the `surface.split` IPC method expects.
    fn as_ipc(self) -> &'static str {
        match self {
            SplitDir::Horizontal => "horizontal",
            SplitDir::Vertical => "vertical",
        }
    }
}

/// A CLI failure carrying the process exit code to surface for it.
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

/// Entry point invoked by `main.rs` when `argv[1]` is a known verb. Parses the
/// args with clap, opens a client to the running instance, dispatches, and
/// returns the process exit code.
pub fn run() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // clap prints `--help`/`--version` (exit 0) and usage errors (exit 2)
        // itself; we just relay its code rather than letting `parse()` abort
        // the process from inside a GUI binary.
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };

    // `command` is always `Some` here: main.rs only calls `run` when argv[1]
    // is a known verb. The `None` arm is unreachable in practice.
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

/// Resolve the socket path and build a client. The path is resolved eagerly
/// (honoring `PANEFLOW_SOCKET_PATH`), but a missing instance only surfaces as
/// an "unreachable … is Paneflow running?" error on the first `call`, so a
/// resolvable-but-dead socket is not a `connect` failure.
fn connect() -> Result<IpcClient, String> {
    let socket = paneflow_ipc_client::resolve_socket_path().ok_or_else(|| {
        "paneflow: cannot locate the IPC socket; is Paneflow running? \
         (set PANEFLOW_SOCKET_PATH if you launched the CLI outside a Paneflow pane)"
            .to_string()
    })?;
    Ok(IpcClient::new(socket))
}

/// Route a parsed subcommand to its handler. Handlers land per story:
/// `read`/`search` + the target selector (US-003/US-004), `new`/`select`/`split`
/// (US-005), `send` (US-006). The scaffold (US-002) wires the surface and the
/// transport; each arm returns an explicit "not yet implemented" runtime error
/// until its story fills it in.
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
                // EP-003 US-007: push-based quiescence; optional sentinel (US-008).
                wait_cmd::wait_idle(client, &selector, for_ms, timeout, pattern.as_deref())
            } else {
                // Pattern-only poll path (unchanged). clap's
                // `required_unless_present="idle"` guarantees `Some` here; guard
                // defensively so a future flag change can't smuggle an empty
                // (match-everything) regex through.
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
        Commands::Watch { surface, types } => watch_cmd::watch(client, surface.as_deref(), &types),
    }
}

/// Render a JSON-RPC `result` value as pretty JSON to stdout. Shared by the
/// read and control command modules so every machine-readable output uses one
/// renderer.
pub(super) fn print_json(value: &Value) -> Result<(), CliError> {
    let rendered = serde_json::to_string_pretty(value)
        .map_err(|e| CliError::runtime(format!("failed to render JSON: {e}")))?;
    println!("{rendered}");
    Ok(())
}

/// Reject a server reply that carries a *legacy* application error.
///
/// A handful of server handlers signal cap/validation failures (split at
/// `MAX_PANES`, `select` out-of-range, `send_text` over the 64 KiB limit) with
/// an ad-hoc `{"error": "<message>"}` payload that does NOT use the
/// `_jsonrpc_error` sentinel. The dispatcher therefore promotes them under
/// `result`, so the transport's `parse_response` returns `Ok` and the command
/// would otherwise print the error and exit 0 - breaking the scriptability
/// contract (US-005 AC4 "code non-zéro", US-006 AC3). Calling this on every
/// `result` before printing maps that legacy shape to a non-zero `CliError`.
///
/// No success envelope on these verbs carries a top-level `error` string
/// (`{index,…}`, `{selected}`, `{split,…}`, `{sent,…}`, `{surfaces,…}`,
/// `{text,…}`, `{matches,…}`), so the check can't false-positive on real data.
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
        // EP-005 US-011: the MCP tool names gate the CLI dispatch in main.rs...
        assert!(is_cli_verb(Some("search_pane")));
        assert!(is_cli_verb(Some("read_pane")));
        assert!(is_cli_verb(Some("list_panes")));
        // ...and clap routes each alias to its canonical subcommand, so a
        // conductor that types the MCP name never lands on the GUI launch path.
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
        // EP-005 US-011: a verb-shaped typo is flagged so main.rs errors
        // actionably instead of launching the GUI / tripping the singleton.
        assert!(looks_like_unknown_verb(Some("blah")));
        assert!(looks_like_unknown_verb(Some("searh")));
        // Known verbs and MCP aliases are NOT unknown.
        assert!(!looks_like_unknown_verb(Some("search")));
        assert!(!looks_like_unknown_verb(Some("search_pane")));
        assert!(!looks_like_unknown_verb(Some("ls")));
        // A bare `paneflow` (None) and an empty token still launch the GUI.
        assert!(!looks_like_unknown_verb(None));
        assert!(!looks_like_unknown_verb(Some("")));
        // Flags stay on the global-flag / GUI path, never the unknown-verb error.
        assert!(!looks_like_unknown_verb(Some("--help")));
        assert!(!looks_like_unknown_verb(Some("-v")));
        assert!(!looks_like_unknown_verb(Some("--update-and-exit")));
    }

    #[test]
    fn ps_parses_with_optional_json_flag() {
        let cli = Cli::try_parse_from(["paneflow", "ps", "--json"]).expect("parse");
        assert!(matches!(cli.command, Some(Commands::Ps { json: true })));
        // Default is the human table (like Unix `ps`), JSON is opt-in.
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
        // The human-in-loop default: no broadcast, no submit, no paste unless
        // explicit (EP-001 US-002: absent `--paste`, the server auto-decides).
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
        // EP-001 US-002 AC2: `--paste` is an explicit, parseable override.
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
        // EP-003 US-007: `--idle` parses WITHOUT `--pattern` (the sentinel is
        // optional in idle mode); `--for` carries the quiescence window.
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
        // US-008: `--idle` + `--pattern` coexist (OR semantics, first to fire).
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
        // `--pattern` alone (no `--idle`) still parses (the existing poll path).
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
        // Neither `--idle` nor `--pattern` -> clap usage error (exit 2), never a
        // silent empty-regex that matches everything.
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
            Some(Commands::Watch { surface, types }) => {
                assert_eq!(surface.as_deref(), Some("backend"));
                assert_eq!(types, vec!["ai.stop", "ai.notification"]);
            }
            other => panic!("expected Watch, got {other:?}"),
        }
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
        // Missing the required positional `target` is a clap usage error (2).
        let err = Cli::try_parse_from(["paneflow", "read"]).expect_err("usage");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn no_subcommand_parses_to_none() {
        // Defensive: a bare invocation never reaches `run` (main.rs gates on a
        // known verb), but clap must not force-error on it.
        let cli = Cli::try_parse_from(["paneflow"]).expect("parse");
        assert!(cli.command.is_none());
    }
}
