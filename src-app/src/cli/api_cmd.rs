use clap::Subcommand;
use paneflow_api::{Catalog, Owner};
use paneflow_ipc_client::host_control::{ControlTarget, HostTransport};
use paneflow_ipc_client::{IpcClient, IpcTransport};
use serde_json::{Map, Value, json};

use super::router::{self, Endpoint, Endpoints};
use super::{CLIENT_NAME, CliError, EXIT_OK};

#[derive(Subcommand, Debug)]
pub(super) enum ApiCommand {
    #[command(about = "List every method the running processes serve, with its owner and summary")]
    List {
        #[arg(long, help = "Print the full method descriptors as JSON")]
        json: bool,
    },
    #[command(about = "Show a method's parameters, result fields and an example invocation")]
    Describe {
        #[arg(help = "Method name, such as surface.read")]
        method: String,
        #[arg(long, help = "Print the descriptor as JSON")]
        json: bool,
    },
    #[command(
        about = "Call a method on the process that owns it and print the JSON result",
        long_about = "Call a method on the process that owns it and print the JSON result.\n\nParameters are `key=value` pairs typed from the method's schema (`lines=50` is an integer, `fenced=true` a boolean), or raw JSON with `--args '{...}'` or `--args @file.json`. Pairs override keys from `--args`."
    )]
    Call {
        #[arg(help = "Method name, such as surface.list")]
        method: String,
        #[arg(help = "Parameters as key=value pairs")]
        pairs: Vec<String>,
        #[arg(
            long,
            value_name = "JSON",
            help = "Raw JSON params object, or @path to read one from a file"
        )]
        args: Option<String>,
    },
}

enum Source {
    Window(std::path::PathBuf),
    Local,
}

struct Reachable {
    source: Source,
    owners: Vec<Owner>,
}

fn host_answers(endpoint: &std::path::Path) -> Result<(), CliError> {
    HostTransport::connect(endpoint, CLIENT_NAME)
        .map(drop)
        .map_err(|error| CliError::runtime(format!("{error}; start it with `paneflow host start`")))
}

fn reachable(endpoints: &Endpoints) -> Result<Reachable, CliError> {
    let worker = router::worker_listens(endpoints);
    let mut owners = Vec::new();
    let source = match &endpoints.control {
        Some(ControlTarget::Controller(socket)) => {
            owners.push(Owner::Window);
            if endpoints
                .host
                .as_deref()
                .is_some_and(|endpoint| host_answers(endpoint).is_ok())
            {
                owners.push(Owner::Host);
            }
            Source::Window(socket.clone())
        }
        Some(ControlTarget::Host(endpoint)) => match host_answers(endpoint) {
            Ok(()) => {
                owners.push(Owner::Host);
                Source::Local
            }
            Err(_) if worker => Source::Local,
            Err(error) => return Err(error),
        },
        None if worker => Source::Local,
        None => return Err(router::no_instance()),
    };
    if worker {
        owners.push(Owner::Worker);
    }
    Ok(Reachable { source, owners })
}

fn descriptions(catalog: &Catalog, source: &Source) -> Result<Vec<Value>, CliError> {
    match source {
        Source::Window(socket) => {
            let described = IpcClient::new(socket.clone())
                .call("system.describe", json!({}))
                .map_err(CliError::runtime)?;
            Ok(described["methods"].as_array().cloned().unwrap_or_default())
        }
        Source::Local => Ok(catalog.iter().map(|d| d.describe_from(false)).collect()),
    }
}

fn owner_of(description: &Value) -> Option<Owner> {
    match description["owner"].as_str()? {
        "window" => Some(Owner::Window),
        "host" => Some(Owner::Host),
        "worker" => Some(Owner::Worker),
        _ => None,
    }
}

fn served(description: &Value, owners: &[Owner]) -> bool {
    owner_of(description).is_some_and(|owner| owners.contains(&owner))
        || (description["fallback"] == "host" && owners.contains(&Owner::Host))
}

pub(crate) fn list_rows(catalog: &Catalog, endpoints: &Endpoints) -> Result<Vec<Value>, CliError> {
    let reachable = reachable(endpoints)?;
    Ok(descriptions(catalog, &reachable.source)?
        .into_iter()
        .filter(|description| served(description, &reachable.owners))
        .collect())
}

fn list(catalog: &Catalog, json_output: bool) -> Result<i32, CliError> {
    let rows = list_rows(catalog, &Endpoints::from_environment())?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        );
        return Ok(EXIT_OK);
    }
    let width = rows
        .iter()
        .filter_map(|row| row["name"].as_str().map(str::len))
        .max()
        .unwrap_or(0);
    for row in rows {
        println!(
            "{:<width$}  {:<6}  {}",
            row["name"].as_str().unwrap_or_default(),
            row["owner"].as_str().unwrap_or_default(),
            row["summary"].as_str().unwrap_or_default(),
        );
    }
    Ok(EXIT_OK)
}

fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix('#'))
        .and_then(|pointer| root.pointer(pointer))
        .map_or(schema, |target| resolve(root, target))
}

fn branches<'a>(root: &'a Value, schema: &'a Value) -> Vec<&'a Value> {
    let schema = resolve(root, schema);
    match schema.get("anyOf").or_else(|| schema.get("oneOf")) {
        Some(Value::Array(forms)) => forms
            .iter()
            .map(|form| resolve(root, form))
            .filter(|form| form.get("type").and_then(Value::as_str) != Some("null"))
            .collect(),
        _ => vec![schema],
    }
}

fn type_label(root: &Value, schema: &Value) -> String {
    let forms: Vec<String> = branches(root, schema)
        .into_iter()
        .map(|form| {
            if let Some(values) = form.get("enum").and_then(Value::as_array) {
                return values
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("|");
            }
            let names: Vec<&str> = match form.get("type") {
                Some(Value::String(name)) => vec![name.as_str()],
                Some(Value::Array(names)) => names
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|name| *name != "null")
                    .collect(),
                _ => vec!["any"],
            };
            names
                .into_iter()
                .map(|name| match name {
                    "array" => format!(
                        "array of {}",
                        form.get("items")
                            .map_or("any".to_string(), |items| type_label(root, items))
                    ),
                    "integer"
                        if form
                            .get("minimum")
                            .and_then(Value::as_f64)
                            .is_some_and(|minimum| minimum >= 0.0) =>
                    {
                        "integer >= 0".to_string()
                    }
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect();
    forms.join("|")
}

fn properties(schema: &Value) -> (Vec<(String, Value)>, Vec<String>) {
    let root = schema;
    let object = branches(root, schema)
        .into_iter()
        .find(|form| form.get("properties").is_some())
        .unwrap_or(schema);
    let fields = object
        .get("properties")
        .and_then(Value::as_object)
        .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let required = object
        .get("required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    (fields, required)
}

fn example_invocation(name: &str, params: &Value) -> String {
    let pairs: Vec<String> = params
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| match value {
                    Value::String(text) if !text.contains(char::is_whitespace) => {
                        format!("{key}={text}")
                    }
                    Value::String(_) | Value::Array(_) | Value::Object(_) => {
                        format!("{key}='{value}'")
                    }
                    other => format!("{key}={other}"),
                })
                .collect()
        })
        .unwrap_or_default();
    let nested = params
        .as_object()
        .is_some_and(|map| map.values().any(|v| v.is_array() || v.is_object()));
    if nested {
        return format!("paneflow api call {name} --args '{params}'");
    }
    std::iter::once(format!("paneflow api call {name}"))
        .chain(pairs)
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_description(description: &Value) -> String {
    let name = description["name"].as_str().unwrap_or_default();
    let mut out = format!(
        "{name}\n  {}\n\n  owner: {}{}   gate: {}   reply: {}\n",
        description["summary"].as_str().unwrap_or_default(),
        description["owner"].as_str().unwrap_or_default(),
        description["fallback"]
            .as_str()
            .map(|fallback| format!(" (falls back to the {fallback})"))
            .unwrap_or_default(),
        description["gate"].as_str().unwrap_or_default(),
        description["reply"].as_str().unwrap_or_default(),
    );
    if let Some(note) = description["gate_note"].as_str() {
        out.push_str(&format!("  {note}\n"));
    }
    let params = &description["params"];
    let (fields, required) = properties(params);
    out.push_str("\nParameters:\n");
    if fields.is_empty() {
        out.push_str("  none\n");
    }
    let width = fields.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    for (field, schema) in &fields {
        let resolved = resolve(params, schema);
        let presence = if required.contains(field) {
            "required"
        } else {
            "optional"
        };
        let default = schema
            .get("default")
            .or_else(|| resolved.get("default"))
            .map_or("-".to_string(), Value::to_string);
        let about = schema
            .get("description")
            .or_else(|| resolved.get("description"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        out.push_str(&format!(
            "  {field:<width$}  {:<20}  {presence:<8}  default {default:<8}  {about}\n",
            type_label(params, schema)
        ));
    }
    let result = &description["result"];
    let (fields, _) = properties(result);
    out.push_str("\nResult fields:\n");
    if fields.is_empty() {
        out.push_str(&format!("  {}\n", type_label(result, result)));
    }
    let width = fields.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    for (field, schema) in &fields {
        out.push_str(&format!(
            "  {field:<width$}  {}\n",
            type_label(result, schema)
        ));
    }
    if let Some(example) = description["examples"].as_array().and_then(|e| e.first()) {
        out.push_str(&format!(
            "\nExample:\n  {}\n",
            example_invocation(name, &example["params"])
        ));
    }
    out
}

pub(crate) fn describe_value(
    catalog: &Catalog,
    endpoints: &Endpoints,
    method: &str,
) -> Result<Value, CliError> {
    let reachable = reachable(endpoints)?;
    match &reachable.source {
        Source::Window(socket) => IpcClient::new(socket.clone())
            .call("system.describe", json!({"method": method}))
            .map_err(CliError::runtime),
        Source::Local => Ok(catalog
            .get(method)
            .ok_or_else(|| router::unknown_method(method))?
            .describe_from(false)),
    }
}

fn describe(catalog: &Catalog, method: &str, json_output: bool) -> Result<i32, CliError> {
    let description = describe_value(catalog, &Endpoints::from_environment(), method)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&description).unwrap_or_default()
        );
    } else {
        print!("{}", render_description(&description));
    }
    Ok(EXIT_OK)
}

fn property_schema<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    branches(root, root)
        .into_iter()
        .find_map(|form| form.get("properties").and_then(|fields| fields.get(key)))
}

pub(super) fn typed_value(
    root: &Value,
    schema: Option<&Value>,
    raw: &str,
) -> Result<Value, String> {
    let Some(schema) = schema else {
        return Ok(Value::String(raw.to_string()));
    };
    let mut last_error = None;
    for form in branches(root, schema) {
        let names: Vec<&str> = match form.get("type") {
            Some(Value::String(name)) => vec![name.as_str()],
            Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if raw == "null" && names.contains(&"null") {
            return Ok(Value::Null);
        }
        for name in names {
            let parsed = match name {
                "integer" => raw
                    .parse::<i64>()
                    .map(Value::from)
                    .map_err(|_| "an integer"),
                "number" => raw
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number)
                    .ok_or("a number"),
                "boolean" => match raw {
                    "true" => Ok(Value::Bool(true)),
                    "false" => Ok(Value::Bool(false)),
                    _ => Err("true or false"),
                },
                "string" => Ok(Value::String(raw.to_string())),
                "array" | "object" => serde_json::from_str(raw).map_err(|_| "JSON"),
                _ => continue,
            };
            match parsed {
                Ok(value) => return Ok(value),
                Err(expected) => last_error = Some(expected),
            }
        }
        if form.get("enum").is_some() || form.get("type").is_none() {
            return Ok(serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string())));
        }
    }
    Err(format!(
        "expected {}",
        last_error.unwrap_or("a valid value")
    ))
}

fn read_args(raw: &str) -> Result<Map<String, Value>, CliError> {
    let text = match raw.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|error| CliError::runtime(format!("cannot read {path}: {error}")))?,
        None => raw.to_string(),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(CliError::runtime("--args must be a JSON object")),
        Err(error) => Err(CliError::runtime(format!(
            "--args is not valid JSON: {error}"
        ))),
    }
}

pub(super) fn build_params(
    schema: &Value,
    pairs: &[String],
    args: Option<&str>,
) -> Result<Value, CliError> {
    let mut params = match args {
        Some(raw) => read_args(raw)?,
        None => Map::new(),
    };
    for pair in pairs {
        let (key, raw) = pair
            .split_once('=')
            .ok_or_else(|| CliError::runtime(format!("'{pair}' is not a key=value pair")))?;
        let value = typed_value(schema, property_schema(schema, key), raw)
            .map_err(|error| CliError::runtime(format!("'{key}': {error}")))?;
        params.insert(key.to_string(), value);
    }
    Ok(Value::Object(params))
}

pub(crate) fn call_value(
    catalog: &Catalog,
    endpoints: &Endpoints,
    method: &str,
    pairs: &[String],
    args: Option<&str>,
) -> Result<Value, CliError> {
    let descriptor = catalog
        .get(method)
        .ok_or_else(|| router::unknown_method(method))?;
    if descriptor.is_stream() {
        return Err(CliError::runtime(format!(
            "{method} is a stream; `paneflow api call` answers one request (use `paneflow watch` or `paneflow sessions --follow`)"
        )));
    }
    let schema = descriptor.params_schema();
    let params = build_params(&schema, pairs, args)?;
    paneflow_api::schema::validate(&schema, &params)
        .map_err(|violation| CliError::runtime(violation.message))?;
    let endpoint: Endpoint = router::route(catalog, endpoints, method)?;
    router::send(catalog, &endpoint, method, params)
}

fn call(
    catalog: &Catalog,
    method: &str,
    pairs: &[String],
    args: Option<&str>,
) -> Result<i32, CliError> {
    let result = call_value(catalog, &Endpoints::from_environment(), method, pairs, args)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(EXIT_OK)
}

pub(super) fn run(command: ApiCommand) -> Result<i32, CliError> {
    let catalog = paneflow_api::catalog().map_err(|error| CliError::runtime(error.to_string()))?;
    match command {
        ApiCommand::List { json } => list(&catalog, json),
        ApiCommand::Describe { method, json } => describe(&catalog, &method, json),
        ApiCommand::Call {
            method,
            pairs,
            args,
        } => call(&catalog, &method, &pairs, args.as_deref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_of(method: &str) -> Value {
        paneflow_api::catalog()
            .expect("catalog")
            .get(method)
            .expect(method)
            .params_schema()
    }

    #[test]
    fn pairs_are_typed_from_the_method_schema() {
        let params = build_params(
            &schema_of("surface.read"),
            &["lines=50".into(), "fenced=true".into(), "name=build".into()],
            None,
        )
        .expect("typed");
        assert_eq!(
            params,
            json!({"lines": 50, "fenced": true, "name": "build"})
        );
    }

    #[test]
    fn a_numeric_string_stays_a_string_where_the_schema_wants_one() {
        let params =
            build_params(&schema_of("surface.rename"), &["name=2024".into()], None).expect("typed");
        assert_eq!(params, json!({"name": "2024"}));
    }

    #[test]
    fn a_pair_the_schema_cannot_type_is_refused_naming_the_key() {
        let error = build_params(&schema_of("surface.read"), &["lines=many".into()], None)
            .expect_err("not an integer");
        assert!(error.message.contains("'lines'"), "{}", error.message);
    }

    #[test]
    fn raw_args_and_files_are_merged_under_the_pairs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("params.json");
        std::fs::write(&file, r#"{"lines": 3, "offset": 1}"#).expect("write");
        let from_file = format!("@{}", file.display());
        let params = build_params(
            &schema_of("surface.read"),
            &["lines=7".into()],
            Some(&from_file),
        )
        .expect("merged");
        assert_eq!(params, json!({"lines": 7, "offset": 1}));
        let inline = build_params(
            &schema_of("surface.split"),
            &[],
            Some(r#"{"direction":"vertical","env":{"A":"1"}}"#),
        )
        .expect("inline");
        assert_eq!(inline["env"]["A"], "1");
    }

    #[test]
    fn enums_and_nested_objects_are_typed_too() {
        let params = build_params(
            &schema_of("surface.split"),
            &["direction=vertical".into(), r#"env={"PORT":"8080"}"#.into()],
            None,
        )
        .expect("typed");
        assert_eq!(
            params,
            json!({"direction": "vertical", "env": {"PORT": "8080"}})
        );
    }

    #[test]
    fn a_description_lists_parameters_results_and_an_example() {
        let catalog = paneflow_api::catalog().expect("catalog");
        let rendered = render_description(&catalog.get("surface.read").expect("read").describe());
        assert!(rendered.contains("lines"), "{rendered}");
        assert!(rendered.contains("integer >= 0"), "{rendered}");
        assert!(rendered.contains("optional"), "{rendered}");
        assert!(rendered.contains("total_lines"), "{rendered}");
        assert!(
            rendered.contains("paneflow api call surface.read"),
            "{rendered}"
        );
        let focus = render_description(&catalog.get("surface.focus").expect("focus").describe());
        assert!(focus.contains("required"), "{focus}");
    }
}
