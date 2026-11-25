use clap::Subcommand;
use serde_json::{Map, Value, json};

use super::router::{self, Endpoints};
use super::{CliError, EXIT_OK};

#[derive(clap::Args, Debug)]
pub struct Tab {
    #[arg(help = "The tab id, as paneflow browser list prints it")]
    tab: u64,
    #[arg(long, help = "Print the method result as JSON")]
    json: bool,
}

#[derive(clap::Args, Debug)]
pub struct History {
    #[arg(help = "The tab id, as paneflow browser list prints it")]
    tab: u64,
    #[arg(
        long,
        help = "Return the entries after this sequence number (default 0)"
    )]
    cursor: Option<u64>,
    #[arg(
        long,
        help = "Return at most this many entries (default 200, max 1000)"
    )]
    limit: Option<u64>,
    #[arg(long, help = "Print the method result as JSON")]
    json: bool,
}

#[derive(Subcommand, Debug)]
pub enum BrowserCommand {
    #[command(
        about = "Report whether the browser engine runs, its version, the sandbox state and every tab",
        long_about = "Report whether the browser engine runs, its version, the sandbox state and, for every tab, its owner and presentation path.\n\nExamples:\n  paneflow browser status\n  paneflow browser status --json"
    )]
    Status {
        #[arg(
            long,
            help = "List the tabs of every workspace, not only the caller's (needs the reach gate from a pane)"
        )]
        all: bool,
        #[arg(long, help = "Print the browser.status result as JSON")]
        json: bool,
    },
    #[command(
        about = "Check that the browser engine is installed",
        long_about = "Check that the browser engine is installed and say where it is expected.\n\nExamples:\n  paneflow browser install\n  paneflow browser install --json"
    )]
    Install {
        #[arg(long, help = "Print the browser.install result as JSON")]
        json: bool,
    },
    #[command(
        about = "Open an http(s) URL in a new tab and wait for it to load",
        long_about = "Open an http(s) URL in a new tab owned by the caller and wait for it to load. Leaving loopback asks the user first.\n\nExamples:\n  paneflow browser open http://127.0.0.1:5173/\n  paneflow browser open http://localhost:3000/ --json"
    )]
    Open {
        #[arg(help = "The http or https URL to open")]
        url: String,
        #[arg(
            long,
            help = "The durable id of the workspace whose profile the tab uses (outside a pane; default: the active workspace)"
        )]
        workspace: Option<String>,
        #[arg(long, help = "Print the browser.open result as JSON")]
        json: bool,
    },
    #[command(
        about = "List the tabs with their owner, state and whether the caller may drive them",
        long_about = "List the tabs with their owner, state and whether the caller may drive them. From a pane, only that workspace's tabs are listed.\n\nExamples:\n  paneflow browser list\n  paneflow browser list --json"
    )]
    List {
        #[arg(
            long,
            help = "List the tabs of every workspace (needs the reach gate from a pane)"
        )]
        all: bool,
        #[arg(long, help = "Print the browser.list result as JSON")]
        json: bool,
    },
    #[command(
        about = "Take ownership of an unclaimed tab",
        long_about = "Take ownership of an unclaimed tab, one whose agent session ended.\n\nExamples:\n  paneflow browser claim 3"
    )]
    Claim(Tab),
    #[command(
        about = "Close a tab the caller owns",
        long_about = "Close a tab the caller owns.\n\nExamples:\n  paneflow browser close 3"
    )]
    Close(Tab),
    #[command(
        about = "Load an http(s) URL in a tab and wait for it",
        long_about = "Load an http(s) URL in a tab the caller owns and wait for it to load.\n\nExamples:\n  paneflow browser navigate 3 http://127.0.0.1:5173/settings"
    )]
    Navigate {
        #[arg(help = "The tab id")]
        tab: u64,
        #[arg(help = "The http or https URL to load")]
        url: String,
        #[arg(long, help = "Print the browser.navigate result as JSON")]
        json: bool,
    },
    #[command(
        about = "Reload a tab and wait for it",
        long_about = "Reload a tab the caller owns and wait for it to load.\n\nExamples:\n  paneflow browser reload 3"
    )]
    Reload(Tab),
    #[command(
        about = "Go back one page in a tab",
        long_about = "Go back one page in a tab the caller owns.\n\nExamples:\n  paneflow browser back 3"
    )]
    Back(Tab),
    #[command(
        about = "Go forward one page in a tab",
        long_about = "Go forward one page in a tab the caller owns.\n\nExamples:\n  paneflow browser forward 3"
    )]
    Forward(Tab),
    #[command(
        about = "Print a tab as an accessibility tree with element references",
        long_about = "Print a tab as an accessibility tree whose interactive elements carry references (e1, e2, ...) for browser act, followed by the console entries, requests and dialogs recorded since the previous snapshot.\n\nExamples:\n  paneflow browser snapshot 3\n  paneflow browser snapshot 3 --json"
    )]
    Snapshot(Tab),
    #[command(
        about = "Click, type, press, hover, select or scroll the element a reference names",
        long_about = "Act on the element a snapshot reference names: click, type (--text), press (--key), hover, select (--value) or scroll (--delta-y).\n\nExamples:\n  paneflow browser act 3 e4 click\n  paneflow browser act 3 e2 type --text ada@example.com\n  paneflow browser act 3 e2 press --key Enter\n  paneflow browser act 3 e5 select --value Japan"
    )]
    Act {
        #[arg(help = "The tab id")]
        tab: u64,
        #[arg(
            value_name = "REF",
            help = "The element reference from the latest snapshot, e.g. e4"
        )]
        reference: String,
        #[arg(value_parser = ["click", "type", "press", "hover", "select", "scroll"], help = "What to do to the element")]
        action: String,
        #[arg(long, help = "The text to type (type)")]
        text: Option<String>,
        #[arg(
            long,
            help = "The key to press, e.g. Enter, Tab, Escape, ArrowDown or a character (press)"
        )]
        key: Option<String>,
        #[arg(long, help = "The option value or label to choose (select)")]
        value: Option<String>,
        #[arg(
            long,
            allow_hyphen_values = true,
            help = "Pixels to scroll, negative upward (scroll, default 400)"
        )]
        delta_y: Option<i64>,
        #[arg(long, help = "Print the browser.act result as JSON")]
        json: bool,
    },
    #[command(
        name = "wait-for",
        about = "Wait until a text shows, an element is visible or the network is idle",
        long_about = "Wait until exactly one condition holds: a text shows on the page, an element reference is visible, or no request is in flight. It fails with the condition named when the timeout passes.\n\nExamples:\n  paneflow browser wait-for 3 --text \"Order placed\"\n  paneflow browser wait-for 3 --ref e7\n  paneflow browser wait-for 3 --network-idle --timeout-ms 30000"
    )]
    WaitFor {
        #[arg(help = "The tab id")]
        tab: u64,
        #[arg(long, help = "Wait for this text to show on the page")]
        text: Option<String>,
        #[arg(
            long = "ref",
            value_name = "REF",
            help = "Wait for this element reference to be visible"
        )]
        reference: Option<String>,
        #[arg(long, help = "Wait for no request in flight for 500 ms")]
        network_idle: bool,
        #[arg(
            long,
            help = "Give up after this many milliseconds (default 10000, max 60000)"
        )]
        timeout_ms: Option<u64>,
        #[arg(long, help = "Print the browser.wait_for result as JSON")]
        json: bool,
    },
    #[command(
        about = "Page through a tab's console history",
        long_about = "Page through a tab's console history after a cursor.\n\nExamples:\n  paneflow browser console 3\n  paneflow browser console 3 --cursor 40 --limit 20 --json"
    )]
    Console(History),
    #[command(
        about = "Page through a tab's request history",
        long_about = "Page through a tab's request history after a cursor: method, URL, status, resource type and failure, never a body, with credential headers removed.\n\nExamples:\n  paneflow browser network 3\n  paneflow browser network 3 --cursor 12 --json"
    )]
    Network(History),
    #[command(
        about = "Capture a tab's viewport as a PNG",
        long_about = "Capture a tab's viewport as a PNG and write it to a file.\n\nExamples:\n  paneflow browser screenshot 3 --out page.png\n  paneflow browser screenshot 3 --json"
    )]
    Screenshot {
        #[arg(help = "The tab id")]
        tab: u64,
        #[arg(
            long,
            value_name = "PATH",
            help = "Write the PNG to this file (default: screenshot-<tab>.png)"
        )]
        out: Option<std::path::PathBuf>,
        #[arg(
            long,
            help = "Print the browser.screenshot result, image data included, as JSON"
        )]
        json: bool,
    },
    #[command(
        about = "Run a JavaScript expression in a tab",
        long_about = "Run a JavaScript expression in a tab and print its value. An agent's first evaluation in a workspace asks the user.\n\nExamples:\n  paneflow browser evaluate 3 document.title\n  paneflow browser evaluate 3 \"[...document.querySelectorAll('a')].length\" --json"
    )]
    Evaluate {
        #[arg(help = "The tab id")]
        tab: u64,
        #[arg(help = "The JavaScript expression to run")]
        script: String,
        #[arg(long, help = "Print the browser.evaluate result as JSON")]
        json: bool,
    },
}

fn call(method: &str, params: Value) -> Result<Value, CliError> {
    let catalog = paneflow_api::catalog().map_err(|error| CliError::runtime(error.to_string()))?;
    let endpoints = Endpoints::from_environment();
    let endpoint = router::route(&catalog, &endpoints, method)?;
    router::send(&catalog, &endpoint, method, params)
}

fn text(value: &Value, key: &str) -> String {
    match &value[key] {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn owner(value: &Value) -> String {
    match value["kind"].as_str() {
        Some("agent") => format!(
            "agent {} ({})",
            value["agent"].as_str().unwrap_or("unnamed"),
            text(value, "session")
        ),
        Some(kind) => kind.to_string(),
        None => String::new(),
    }
}

pub(super) fn tab_line(tab: &Value) -> String {
    format!(
        "{}  {}  {}  {}{}  presentation {}",
        text(tab, "tab"),
        text(tab, "state"),
        text(tab, "url"),
        owner(&tab["owner"]),
        if tab["drivable"] == Value::Bool(true) {
            "  drivable"
        } else {
            ""
        },
        text(tab, "presentation"),
    )
}

fn entry_lines(entries: &Value, line: impl Fn(&Value) -> String) -> Vec<String> {
    entries
        .as_array()
        .map(|entries| entries.iter().map(line).collect())
        .unwrap_or_default()
}

pub(super) fn console_line(entry: &Value) -> String {
    let place = match (entry["source"].as_str(), entry["line"].as_u64()) {
        (Some(source), Some(line)) => format!("  {source}:{line}"),
        (Some(source), None) => format!("  {source}"),
        _ => String::new(),
    };
    format!(
        "#{} {} {}{place}",
        text(entry, "seq"),
        text(entry, "level"),
        text(entry, "text")
    )
}

pub(super) fn network_line(entry: &Value) -> String {
    let outcome = entry["failure"]
        .as_str()
        .map(str::to_string)
        .or_else(|| entry["status"].as_u64().map(|status| status.to_string()))
        .unwrap_or_else(|| "pending".to_string());
    format!(
        "#{} {} {} {outcome} {}",
        text(entry, "seq"),
        text(entry, "method"),
        text(entry, "url"),
        text(entry, "resource_type")
    )
}

fn navigated(value: &Value) -> String {
    let status = value["http_status"]
        .as_u64()
        .map(|status| format!(" (HTTP {status})"))
        .unwrap_or_default();
    format!(
        "tab {} {} {}{status}",
        text(value, "tab"),
        text(value, "state"),
        text(value, "url")
    )
}

pub(super) fn human(method: &str, value: &Value) -> Vec<String> {
    match method {
        "browser.status" => {
            let mut lines = vec![
                format!(
                    "engine: {}",
                    if value["running"] == Value::Bool(true) {
                        format!(
                            "running (pid {}) {}",
                            text(value, "host_pid"),
                            text(value, "runtime_version")
                        )
                    } else if value["installed"] == Value::Bool(true) {
                        "installed, not running".to_string()
                    } else {
                        format!("not installed ({})", text(value, "binary"))
                    }
                ),
                format!("sandbox: {}", text(value, "sandbox")),
            ];
            lines.extend(entry_lines(&value["tabs"], tab_line));
            lines
        }
        "browser.install" => vec![text(value, "message")],
        "browser.list" => {
            let lines = entry_lines(&value["tabs"], tab_line);
            if lines.is_empty() {
                vec!["no tab is open".to_string()]
            } else {
                lines
            }
        }
        "browser.claim" => vec![tab_line(value)],
        "browser.close" => vec![format!(
            "closed tab {} ({})",
            text(value, "tab"),
            text(value, "url")
        )],
        "browser.snapshot" => {
            let mut lines = vec![
                format!(
                    "tab {} {}{}  generation {}",
                    text(value, "tab"),
                    text(value, "url"),
                    if value["loading"] == Value::Bool(true) {
                        "  (still loading)"
                    } else {
                        ""
                    },
                    text(value, "generation")
                ),
                text(value, "tree"),
            ];
            if value["dropped_bytes"].as_u64().unwrap_or(0) > 0 {
                lines.push(format!(
                    "... {} bytes of the tree not shown",
                    text(value, "dropped_bytes")
                ));
            }
            let console = entry_lines(&value["console"], console_line);
            if !console.is_empty() {
                lines.push("console:".to_string());
                lines.extend(console);
            }
            let network = entry_lines(&value["network"], network_line);
            if !network.is_empty() {
                lines.push("network:".to_string());
                lines.extend(network);
            }
            lines.extend(entry_lines(&value["dialogs"], |dialog| {
                format!(
                    "dialog {} {:?} dismissed",
                    text(dialog, "kind"),
                    text(dialog, "message")
                )
            }));
            lines
        }
        "browser.act" => {
            let mut lines = vec![format!(
                "{} done on tab {}{}: {}",
                text(value, "action"),
                text(value, "tab"),
                if value["navigated"] == Value::Bool(true) {
                    ", navigated"
                } else {
                    ""
                },
                text(value, "url")
            )];
            lines.extend(entry_lines(&value["dialogs"], |dialog| {
                format!(
                    "dialog {} {:?} dismissed",
                    text(dialog, "kind"),
                    text(dialog, "message")
                )
            }));
            lines
        }
        "browser.wait_for" => vec![format!(
            "{} held after {} ms",
            text(value, "condition"),
            text(value, "waited_ms")
        )],
        "browser.console" => {
            let mut lines = entry_lines(&value["entries"], console_line);
            lines.push(format!(
                "next cursor {}, {} dropped",
                text(value, "cursor"),
                text(value, "dropped")
            ));
            lines
        }
        "browser.network" => {
            let mut lines = entry_lines(&value["entries"], network_line);
            lines.push(format!(
                "next cursor {}, {} dropped",
                text(value, "cursor"),
                text(value, "dropped")
            ));
            lines
        }
        "browser.evaluate" => match value["exception"].as_str() {
            Some(exception) => vec![format!("exception: {exception}")],
            None => vec![text(value, "value")],
        },
        _ => vec![navigated(value)],
    }
}

fn scoped(all: bool) -> Value {
    if all {
        json!({"scope": "all"})
    } else {
        json!({})
    }
}

fn optional(params: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        params.insert(key.into(), value);
    }
}

fn request(command: &BrowserCommand) -> (&'static str, Value, bool) {
    use BrowserCommand as C;
    match command {
        C::Status { all, json } => ("browser.status", scoped(*all), *json),
        C::Install { json } => ("browser.install", json!({}), *json),
        C::Open {
            url,
            workspace,
            json,
        } => {
            let mut params = Map::new();
            params.insert("url".into(), json!(url));
            optional(
                &mut params,
                "workspace",
                workspace.as_ref().map(|w| json!(w)),
            );
            ("browser.open", Value::Object(params), *json)
        }
        C::List { all, json } => ("browser.list", scoped(*all), *json),
        C::Claim(tab) => ("browser.claim", json!({"tab": tab.tab}), tab.json),
        C::Close(tab) => ("browser.close", json!({"tab": tab.tab}), tab.json),
        C::Navigate { tab, url, json } => {
            ("browser.navigate", json!({"tab": tab, "url": url}), *json)
        }
        C::Reload(tab) => ("browser.reload", json!({"tab": tab.tab}), tab.json),
        C::Back(tab) => ("browser.back", json!({"tab": tab.tab}), tab.json),
        C::Forward(tab) => ("browser.forward", json!({"tab": tab.tab}), tab.json),
        C::Snapshot(tab) => ("browser.snapshot", json!({"tab": tab.tab}), tab.json),
        C::Act {
            tab,
            reference,
            action,
            text,
            key,
            value,
            delta_y,
            json,
        } => {
            let mut params = Map::new();
            params.insert("tab".into(), json!(tab));
            params.insert("ref".into(), json!(reference));
            params.insert("action".into(), json!(action));
            optional(&mut params, "text", text.as_ref().map(|v| json!(v)));
            optional(&mut params, "key", key.as_ref().map(|v| json!(v)));
            optional(&mut params, "value", value.as_ref().map(|v| json!(v)));
            optional(&mut params, "delta_y", delta_y.map(|v| json!(v)));
            ("browser.act", Value::Object(params), *json)
        }
        C::WaitFor {
            tab,
            text,
            reference,
            network_idle,
            timeout_ms,
            json,
        } => {
            let mut params = Map::new();
            params.insert("tab".into(), json!(tab));
            optional(&mut params, "text", text.as_ref().map(|v| json!(v)));
            optional(&mut params, "ref", reference.as_ref().map(|v| json!(v)));
            optional(
                &mut params,
                "network_idle",
                network_idle.then_some(json!(true)),
            );
            optional(&mut params, "timeout_ms", timeout_ms.map(|v| json!(v)));
            ("browser.wait_for", Value::Object(params), *json)
        }
        C::Console(history) | C::Network(history) => {
            let method = if matches!(command, C::Console(_)) {
                "browser.console"
            } else {
                "browser.network"
            };
            let mut params = Map::new();
            params.insert("tab".into(), json!(history.tab));
            optional(&mut params, "cursor", history.cursor.map(|v| json!(v)));
            optional(&mut params, "limit", history.limit.map(|v| json!(v)));
            (method, Value::Object(params), history.json)
        }
        C::Screenshot { tab, json, .. } => ("browser.screenshot", json!({"tab": tab}), *json),
        C::Evaluate { tab, script, json } => (
            "browser.evaluate",
            json!({"tab": tab, "script": script}),
            *json,
        ),
    }
}

pub(super) fn run(command: BrowserCommand) -> Result<i32, CliError> {
    let (method, params, json) = request(&command);
    let value = call(method, params)?;
    if json {
        super::print_json(&value)?;
        return Ok(EXIT_OK);
    }
    if let BrowserCommand::Screenshot { tab, out, .. } = &command {
        let path = out
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from(format!("screenshot-{tab}.png")));
        let bytes = value["data"]
            .as_str()
            .and_then(paneflow_browser_control::decode_base64)
            .ok_or_else(|| CliError::runtime("the window sent no PNG data"))?;
        std::fs::write(&path, bytes).map_err(|error| {
            CliError::runtime(format!("cannot write {}: {error}", path.display()))
        })?;
        println!(
            "wrote {} ({}x{}, scale {})",
            path.display(),
            text(&value, "width"),
            text(&value, "height"),
            text(&value, "device_scale_factor")
        );
        return Ok(EXIT_OK);
    }
    for line in human(method, &value) {
        println!("{}", printable(&line));
    }
    Ok(EXIT_OK)
}

pub(super) fn printable(line: &str) -> String {
    let mut shown = String::with_capacity(line.len());
    for character in line.chars() {
        if character.is_control() && !matches!(character, '\n' | '\t') {
            shown.push_str(&format!("\\u{{{:x}}}", u32::from(character)));
        } else {
            shown.push(character);
        }
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[derive(clap::Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        command: BrowserCommand,
    }

    #[test]
    fn page_text_cannot_send_escape_sequences_to_the_terminal() {
        let snapshot = json!({
            "tab": 1,
            "url": "http://127.0.0.1/",
            "generation": 1,
            "tree": "RootWebArea \"\u{1b}]52;c;cHduZWQ=\u{7}Shop\"",
            "console": [{"seq": 1, "level": "error", "text": "a\rb\u{1b}[2J\u{9b}31m"}],
        });
        let shown: Vec<String> = human("browser.snapshot", &snapshot)
            .iter()
            .map(|line| printable(line))
            .collect();
        let joined = shown.join("\n");
        assert!(
            !joined.chars().any(|c| c.is_control() && c != '\n'),
            "{joined:?}"
        );
        assert!(
            joined.contains("\\u{1b}]52;c;cHduZWQ=\\u{7}Shop"),
            "{joined}"
        );
        assert!(joined.contains("a\\u{d}b\\u{1b}[2J\\u{9b}31m"), "{joined}");
        assert_eq!(printable("tab\tok\nnext"), "tab\tok\nnext");
    }

    #[test]
    fn every_browser_method_has_a_verb_with_an_example_and_json_output() {
        let command = Probe::command();
        let verbs: Vec<String> = command
            .get_subcommands()
            .map(|verb| verb.get_name().replace('-', "_"))
            .collect();
        for method in paneflow_api::methods::browser::METHODS {
            let verb = method.trim_start_matches("browser.");
            assert!(
                verbs.iter().any(|name| name == verb),
                "{method} has no verb"
            );
        }
        for verb in command.get_subcommands() {
            let long = verb
                .get_long_about()
                .map(ToString::to_string)
                .unwrap_or_default();
            assert!(
                long.contains("Examples:\n  paneflow browser "),
                "{} has no example",
                verb.get_name()
            );
            assert!(
                verb.get_arguments()
                    .any(|argument| argument.get_id() == "json"),
                "{} has no --json",
                verb.get_name()
            );
        }
    }

    #[test]
    fn a_verb_builds_the_params_its_method_takes() {
        let parse = |args: &[&str]| {
            let mut argv = vec!["probe"];
            argv.extend_from_slice(args);
            <Probe as clap::Parser>::try_parse_from(argv)
                .expect("parses")
                .command
        };
        let (method, params, json) = request(&parse(&[
            "act",
            "3",
            "e2",
            "type",
            "--text",
            "ada@example.com",
            "--json",
        ]));
        assert_eq!(method, "browser.act");
        assert_eq!(
            params,
            json!({"tab": 3, "ref": "e2", "action": "type", "text": "ada@example.com"})
        );
        assert!(json);
        let (method, params, _) = request(&parse(&["wait-for", "3", "--network-idle"]));
        assert_eq!(method, "browser.wait_for");
        assert_eq!(params, json!({"tab": 3, "network_idle": true}));
        let (_, params, _) = request(&parse(&["act", "1", "e9", "scroll", "--delta-y", "-200"]));
        assert_eq!(params["delta_y"], -200);
        let (method, params, _) = request(&parse(&["status", "--all"]));
        assert_eq!(
            (method, params),
            ("browser.status", json!({"scope": "all"}))
        );
        let (method, _, _) = request(&parse(&["network", "2", "--cursor", "4"]));
        assert_eq!(method, "browser.network");
    }

    #[test]
    fn status_reads_as_engine_sandbox_and_one_line_per_tab() {
        let lines = human(
            "browser.status",
            &json!({
                "running": true, "host_pid": 42, "installed": true, "binary": "/b",
                "runtime_version": "CEF 156.0.31", "sandbox": "enabled",
                "tabs": [{"tab": 1, "workspace": "w", "url": "http://127.0.0.1/", "state": "loaded",
                          "owner": {"kind": "agent", "session": "s-1", "agent": "Claude Code"},
                          "drivable": false, "presentation": "none"}]
            }),
        );
        assert_eq!(lines[0], "engine: running (pid 42) CEF 156.0.31");
        assert_eq!(lines[1], "sandbox: enabled");
        assert_eq!(
            lines[2],
            "1  loaded  http://127.0.0.1/  agent Claude Code (s-1)  presentation none"
        );
    }
}
