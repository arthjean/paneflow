use paneflow_ipc_client::IpcTransport;
use serde_json::{Value, json};

use super::selector::resolve_target;
use super::{CliError, EXIT_OK};

pub fn ls(client: &impl IpcTransport, human: bool) -> Result<i32, CliError> {
    let result = active_workspace_panes(client)?;
    if human {
        print_surfaces_table(&result);
    } else {
        super::print_json(&result)?;
    }
    Ok(EXIT_OK)
}

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
        let text = result.get("text").and_then(Value::as_str).unwrap_or("");
        print!("{text}");
    }
    Ok(EXIT_OK)
}

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
                let line = m.get("line").and_then(Value::as_i64).unwrap_or(0);
                let text = m.get("text").and_then(Value::as_str).unwrap_or("");
                println!("{line}: {text}");
            }
        }
    } else {
        super::print_json(&result)?;
    }
    Ok(EXIT_OK)
}

fn active_workspace_panes(client: &impl IpcTransport) -> Result<Value, CliError> {
    let listed = super::reject_legacy_error(
        client
            .call("surface.list", json!({}))
            .map_err(CliError::runtime)?,
    )?;
    Ok(active_workspace_listing(listed))
}

fn active_workspace_listing(mut listed: Value) -> Value {
    if !listed["scope_workspace_id"].is_null() {
        return listed;
    }
    let Some(active) = listed["workspace"].as_u64() else {
        return listed;
    };
    if let Some(surfaces) = listed["surfaces"].as_array_mut() {
        surfaces.retain(|surface| surface["workspace"].as_u64() == Some(active));
    }
    listed
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
    println!(
        "{:>4}  {:<20}  {:<16}  {:<24}  CWD",
        "ID", "NAME", "SPACE", "CMD"
    );
    for s in surfaces {
        let id = s.get("surface_id").and_then(Value::as_u64).unwrap_or(0);
        let name = s.get("name").and_then(Value::as_str).unwrap_or("");
        let space = surface_space_label(s);
        let cmd = s.get("cmd").and_then(Value::as_str).unwrap_or("");
        let cwd = s.get("cwd").and_then(Value::as_str).unwrap_or("");
        println!("{id:>4}  {name:<20}  {space:<16}  {cmd:<24}  {cwd}");
    }
}

fn surface_space_label(surface: &Value) -> &str {
    surface
        .get("space")
        .and_then(|space| space.get("name"))
        .and_then(Value::as_str)
        .unwrap_or(paneflow_config::schema::DEFAULT_SPACE_NAME)
}

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
            .unwrap_or("no state");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_human_listing_names_the_default_space_for_a_surface_without_one() {
        assert_eq!(
            surface_space_label(&json!({ "space": { "id": "x", "name": "Work" } })),
            "Work"
        );
        assert_eq!(surface_space_label(&json!({ "space": null })), "Default");
        assert_eq!(surface_space_label(&json!({})), "Default");
    }

    fn surface_ids(listed: &Value) -> Vec<u64> {
        listed["surfaces"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|surface| surface["surface_id"].as_u64())
            .collect()
    }

    struct ListingTransport(Value);

    impl IpcTransport for ListingTransport {
        fn call(&self, method: &str, _params: Value) -> Result<Value, String> {
            assert_eq!(method, "surface.list");
            Ok(self.0.clone())
        }
    }

    #[test]
    fn ls_keeps_only_the_surfaces_of_the_workspace_the_listing_describes() {
        let listed = json!({
            "pane_count": 2,
            "workspace": 0,
            "scope_workspace_id": null,
            "surfaces": [
                {"surface_id": 1026, "workspace": 0},
                {"surface_id": 2053, "workspace": 0},
                {"surface_id": 5137, "workspace": 1}
            ]
        });
        let shown = active_workspace_panes(&ListingTransport(listed)).expect("listed");
        assert_eq!(surface_ids(&shown), vec![1026, 2053]);
        assert_eq!(shown["pane_count"], 2);
        assert_eq!(shown["workspace"], 0);
    }

    #[test]
    fn ls_lists_every_host_session_when_no_workspace_is_known() {
        let listed = json!({
            "pane_count": 2,
            "workspace": null,
            "surfaces": [{"surface_id": 1}, {"surface_id": 2}]
        });
        let shown = active_workspace_panes(&ListingTransport(listed)).expect("listed");
        assert_eq!(surface_ids(&shown), vec![1, 2]);
    }
}
