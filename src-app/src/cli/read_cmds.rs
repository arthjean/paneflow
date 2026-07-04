//! Read surface of the CLI: `ls` / `read` / `search` (US-004).
//!
//! Thin wrappers over the existing `surface.list` / `surface.read` /
//! `surface.search` IPC methods. Introspection (`ls`, `search`) emits JSON by
//! default for scripts; `read` prints the scrollback text (wrapped in the
//! anti-injection `<untrusted_terminal_output>` fence when the
//! `ai_injection_fence` setting is on, the default; pass `--raw` to bypass it
//! for a human piping into `grep`) and only wraps it in the
//! `{text, lines, total_lines, eof}` envelope under `--json`.

use paneflow_ipc_client::IpcTransport;
use serde_json::{Value, json};

use super::selector::resolve_target;
use super::{CliError, EXIT_OK};

/// `paneflow ls [--human]` - list terminal surfaces.
pub fn ls(client: &impl IpcTransport, human: bool) -> Result<i32, CliError> {
    let result = super::reject_legacy_error(
        client
            .call("surface.list", json!({}))
            .map_err(CliError::runtime)?,
    )?;
    if human {
        print_surfaces_table(&result);
    } else {
        super::print_json(&result)?;
    }
    Ok(EXIT_OK)
}

/// `paneflow read <target> [--lines N] [--offset N] [--json] [--raw]`.
pub fn read(
    client: &impl IpcTransport,
    target: &str,
    lines: Option<u64>,
    offset: Option<u64>,
    json_out: bool,
    raw: bool,
) -> Result<i32, CliError> {
    let surface_id = resolve_target(client, target)?;
    let mut params = json!({ "surface_id": surface_id });
    if let Some(lines) = lines {
        params["lines"] = json!(lines);
    }
    if let Some(offset) = offset {
        params["offset"] = json!(offset);
    }
    // EP-003 US-011 (agent-control-plane): surface.read fences its output as
    // untrusted by default (the `ai_injection_fence` setting). `--raw` forces
    // the historical unwrapped output for a human piping into `grep` etc.
    if raw {
        params["fenced"] = json!(false);
    }
    let result = super::reject_legacy_error(
        client
            .call("surface.read", params)
            .map_err(CliError::runtime)?,
    )?;
    if json_out {
        super::print_json(&result)?;
    } else {
        // Raw scrollback: print verbatim (it carries its own newlines). No
        // trailing newline is appended so the output round-trips exactly.
        let text = result.get("text").and_then(Value::as_str).unwrap_or("");
        print!("{text}");
    }
    Ok(EXIT_OK)
}

/// `paneflow search <target> <pattern> [--max N] [--human]`.
pub fn search(
    client: &impl IpcTransport,
    target: &str,
    pattern: &str,
    max: Option<u64>,
    human: bool,
) -> Result<i32, CliError> {
    let surface_id = resolve_target(client, target)?;
    let mut params = json!({ "surface_id": surface_id, "pattern": pattern });
    if let Some(max) = max {
        params["max_matches"] = json!(max);
    }
    let result = super::reject_legacy_error(
        client
            .call("surface.search", params)
            .map_err(CliError::runtime)?,
    )?;
    if human {
        if let Some(matches) = result.get("matches").and_then(Value::as_array) {
            for m in matches {
                let line = m.get("line").and_then(Value::as_u64).unwrap_or(0);
                let text = m.get("text").and_then(Value::as_str).unwrap_or("");
                println!("{line}: {text}");
            }
        }
    } else {
        super::print_json(&result)?;
    }
    Ok(EXIT_OK)
}

fn print_surfaces_table(result: &Value) {
    let surfaces = result.get("surfaces").and_then(Value::as_array);
    let Some(surfaces) = surfaces else {
        println!("(no surfaces)");
        return;
    };
    if surfaces.is_empty() {
        println!("(no surfaces)");
        return;
    }
    println!("{:>4}  {:<20}  {:<24}  CWD", "ID", "NAME", "CMD");
    for s in surfaces {
        let id = s.get("surface_id").and_then(Value::as_u64).unwrap_or(0);
        let name = s.get("name").and_then(Value::as_str).unwrap_or("");
        let cmd = s.get("cmd").and_then(Value::as_str).unwrap_or("");
        let cwd = s.get("cwd").and_then(Value::as_str).unwrap_or("");
        println!("{id:>4}  {name:<20}  {cmd:<24}  {cwd}");
    }
}

/// `paneflow ps [--json]` - list running agents across the fleet (EP-001
/// US-001). Defaults to a human table (like Unix `ps`); `--json` emits the
/// `{agents:[…]}` envelope for scripts.
pub fn ps(client: &impl IpcTransport, json_out: bool) -> Result<i32, CliError> {
    let result = super::reject_legacy_error(
        client
            .call("fleet.list", json!({}))
            .map_err(CliError::runtime)?,
    )?;
    if json_out {
        super::print_json(&result)?;
    } else {
        print_agents_table(&result);
    }
    Ok(EXIT_OK)
}

/// `paneflow status <target> [--json]` - read one surface's agent state (EP-001
/// US-002). Defaults to a one-line summary; `--json` emits the full envelope.
pub fn status(client: &impl IpcTransport, target: &str, json_out: bool) -> Result<i32, CliError> {
    let surface_id = resolve_target(client, target)?;
    let result = super::reject_legacy_error(
        client
            .call("surface.status", json!({ "surface_id": surface_id }))
            .map_err(CliError::runtime)?,
    )?;
    if json_out {
        super::print_json(&result)?;
    } else {
        let state = result
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        match result.get("tool").and_then(Value::as_str) {
            Some(tool) => println!("{state} ({tool})"),
            None => println!("{state}"),
        }
        if let Some(msg) = result.get("message").and_then(Value::as_str) {
            println!("{msg}");
        }
    }
    Ok(EXIT_OK)
}

fn print_agents_table(result: &Value) {
    let agents = result
        .get("agents")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty());
    let Some(agents) = agents else {
        println!("(no agents)");
        return;
    };
    println!(
        "{:>7}  {:<10}  {:<18}  {:>2}  PANE",
        "PID", "TOOL", "STATE", "WS"
    );
    for a in agents {
        let pid = a
            .get("pid")
            .and_then(Value::as_u64)
            .map_or_else(|| "-".to_string(), |p| p.to_string());
        let tool = a.get("tool").and_then(Value::as_str).unwrap_or("");
        let state = a.get("state").and_then(Value::as_str).unwrap_or("");
        let ws = a.get("workspace").and_then(Value::as_u64).unwrap_or(0);
        let pane = a.get("surface_name").and_then(Value::as_str).unwrap_or("-");
        println!("{pid:>7}  {tool:<10}  {state:<18}  {ws:>2}  {pane}");
    }
}
