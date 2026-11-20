use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::Value;

pub const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let starts = chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_');
    starts && chars.all(|c| c.is_ascii_alphanumeric() || c == '_') && !LUA_KEYWORDS.contains(&name)
}

fn quoted(name: &str) -> String {
    serde_json::to_string(name).unwrap_or_else(|_| format!("\"{name}\""))
}

fn member(base: &str, name: &str) -> String {
    if is_identifier(name) {
        format!("{base}.{name}")
    } else {
        format!("{base}[{}]", quoted(name))
    }
}

fn field_name(name: &str) -> String {
    if is_identifier(name) {
        name.to_string()
    } else {
        format!("[{}]", quoted(name))
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix('#'))
        .and_then(|pointer| root.pointer(pointer))
        .map_or(schema, |target| resolve(root, target))
}

struct Shape {
    lua: String,
    nullable: bool,
}

fn shape(root: &Value, schema: &Value, depth: usize) -> Shape {
    let schema = resolve(root, schema);
    if depth > 8 {
        return Shape {
            lua: "any".into(),
            nullable: false,
        };
    }
    if let Some(forms) = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(Value::as_array)
    {
        let mut nullable = false;
        let mut kinds = Vec::new();
        for form in forms {
            let form = resolve(root, form);
            if form.get("type").and_then(Value::as_str) == Some("null") {
                nullable = true;
                continue;
            }
            let inner = shape(root, form, depth + 1);
            nullable |= inner.nullable;
            if !kinds.contains(&inner.lua) {
                kinds.push(inner.lua);
            }
        }
        return Shape {
            lua: if kinds.is_empty() {
                "any".into()
            } else {
                kinds.join("|")
            },
            nullable,
        };
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let literals: Vec<String> = values
            .iter()
            .map(|value| match value {
                Value::String(text) => quoted(text),
                other => other.to_string(),
            })
            .collect();
        return Shape {
            lua: literals.join("|"),
            nullable: false,
        };
    }
    let names: Vec<&str> = match schema.get("type") {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let nullable = names.contains(&"null");
    let kinds: Vec<String> = names
        .into_iter()
        .filter(|name| *name != "null")
        .map(|name| match name {
            "integer" => "integer".to_string(),
            "number" => "number".to_string(),
            "string" => "string".to_string(),
            "boolean" => "boolean".to_string(),
            "array" => {
                let item = schema
                    .get("items")
                    .map_or("any".to_string(), |items| shape(root, items, depth + 1).lua);
                if item.contains('|') {
                    format!("({item})[]")
                } else {
                    format!("{item}[]")
                }
            }
            "object" => match schema.get("additionalProperties") {
                Some(values) if values.is_object() => {
                    format!("table<string, {}>", shape(root, values, depth + 1).lua)
                }
                _ => "table".to_string(),
            },
            _ => "any".to_string(),
        })
        .collect();
    Shape {
        lua: if kinds.is_empty() {
            "any".into()
        } else {
            kinds.join("|")
        },
        nullable,
    }
}

struct Field {
    name: String,
    lua: String,
    optional: bool,
    about: String,
}

fn fields(schema: &Value) -> Vec<Field> {
    let object = resolve(schema, schema);
    let forms: Vec<&Value> = match object.get("anyOf").or_else(|| object.get("oneOf")) {
        Some(Value::Array(forms)) => forms.iter().map(|form| resolve(schema, form)).collect(),
        _ => vec![object],
    };
    let mut collected: BTreeMap<String, Field> = BTreeMap::new();
    for form in forms {
        let required: Vec<&str> = form
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let Some(properties) = form.get("properties").and_then(Value::as_object) else {
            continue;
        };
        for (name, property) in properties {
            let typed = shape(schema, property, 0);
            let about = property
                .get("description")
                .or_else(|| resolve(schema, property).get("description"))
                .and_then(Value::as_str)
                .map(one_line)
                .unwrap_or_default();
            collected.entry(name.clone()).or_insert(Field {
                name: name.clone(),
                lua: typed.lua,
                optional: typed.nullable || !required.contains(&name.as_str()),
                about,
            });
        }
    }
    collected.into_values().collect()
}

fn method_names(descriptions: &[Value]) -> Vec<(&str, &Value)> {
    let mut methods: Vec<(&str, &Value)> = descriptions
        .iter()
        .filter_map(|description| Some((description["name"].as_str()?, description)))
        .collect();
    methods.sort_by(|left, right| left.0.cmp(right.0));
    methods
}

const RUNTIME_STUBS: &str = r#"---@class paneflow.ctx
---@field surface_id integer? The surface the script, handler or timer belongs to
---@field pane_id integer? The pane that holds it
---@field tab_id integer? The tab that holds it
---@field workspace_id integer? The layout workspace that holds it
---@field origin "cli"|"timer"|"event"|string What started this code

---@class paneflow.timer
local timer = {}

---Stop the timer; returns whether it was active.
---@return boolean
function timer:cancel() end

---@return boolean
function timer:active() end

---@class paneflow.exec.result
---@field code integer? The exit code, nil when a signal ended the program
---@field signal integer? The signal that ended the program, on Unix
---@field stdout string? Captured output, nil when stdout = false
---@field stderr string? Captured errors, nil when stderr = false
---@field timed_out boolean True when the timeout stopped the program
---@field ok boolean True when the code is 0 and the program did not time out

---@class paneflow.exec.options
---@field cwd string? Directory to run in
---@field env table<string, string|false>? Variables to set, false removes one
---@field clear_env boolean? Start from an empty environment
---@field stdin string|boolean? Text to write to stdin, or false for no stdin
---@field timeout number? Seconds before the process tree is terminated
---@field stdout boolean|fun(chunk: string)? false discards, a function receives output as it arrives
---@field stderr boolean|fun(chunk: string)? false discards, a function receives output as it arrives
---@field text boolean? Decode output as text and turn CRLF into LF, true by default
---@field detach boolean? Keep the program running when the scope that started it ends

---@class paneflow.exec.handle
---@field pid integer
local handle = {}

---Wait for the program to end; nil and "timeout" when seconds pass first.
---@param seconds number?
---@return paneflow.exec.result?
---@return string? message
function handle:wait(seconds) end

---@param data string
---@return boolean? ok
---@return string? message
function handle:write(data) end

---@return boolean
function handle:close_stdin() end

---@param signal string|integer? TERM by default
---@return boolean
function handle:kill(signal) end

---@return boolean
function handle:running() end

---@class paneflow.event
---@field type string
---@field seq integer
---@field session string?
---@field [string] any

---@class paneflow.layout.node
---@field type "pane"|"split"

---@class paneflow.json
paneflow.json = {}

---Encode a value; a table with keys 1 to n becomes an array, any other table an object.
---@param value any
---@param pretty boolean?
---@return string? json
---@return string? message
function paneflow.json.encode(value, pretty) end

---Decode JSON; null becomes nil and integers stay integers.
---@param text string
---@return any value
---@return string? message
function paneflow.json.decode(text) end

---@class paneflow.layout
paneflow.layout = {}

---@param spec { surfaces: table[]?, command: string?, cwd: string?, name: string?, env: table<string, string>?, focus: boolean? }?
---@return paneflow.layout.node
function paneflow.layout.pane(spec) end

---Stack two nodes, the first taking ratio of the height.
---@param ratio number
---@param first paneflow.layout.node
---@param second paneflow.layout.node
---@return paneflow.layout.node
function paneflow.layout.horizontal(ratio, first, second) end

---Put two nodes side by side, the first taking ratio of the width.
---@param ratio number
---@param first paneflow.layout.node
---@param second paneflow.layout.node
---@return paneflow.layout.node
function paneflow.layout.vertical(ratio, first, second) end

---The key=value pairs or the --args JSON given to paneflow do.
---@type table<string, any>
paneflow.args = {}

---@type paneflow.ctx
paneflow.ctx = { origin = "cli" }

---Call any method by its full name.
---@param method string
---@param params table?
---@return table? result
---@return string? message
---@return integer? code
function paneflow.call(method, params) end

---Call fn(ctx, ev) for every matching event; at the top level only. A name may be a family such as terminal.*.
---@param event string
---@param fn fun(ctx: paneflow.ctx, ev: paneflow.event)
---@return boolean
function paneflow.on(event, fn) end

---Wait for the next matching event since the script started; nil and "timeout" when seconds pass first.
---@param event string
---@param filter table|fun(ev: paneflow.event, ctx: paneflow.ctx): boolean|nil
---@param timeout number?
---@return paneflow.event? ev
---@return paneflow.ctx|string ctx_or_message
function paneflow.wait(event, filter, timeout) end

---@param seconds number
function paneflow.sleep(seconds) end

---End the script at once with this exit status.
---@param status integer?
function paneflow.stop(status) end

---@param seconds number
---@param fn fun(ctx: paneflow.ctx)
---@return paneflow.timer
function paneflow.after(seconds, fn) end

---@param seconds number
---@param fn fun(ctx: paneflow.ctx)
---@return paneflow.timer
function paneflow.every(seconds, fn) end

---@class paneflow.exec
---Run a program from an argument list, never through a shell.
---@overload fun(argv: string[], opts: paneflow.exec.options?): paneflow.exec.result?, string?
paneflow.exec = {}

---Start a program and return a handle at once.
---@param argv string[]
---@param opts paneflow.exec.options?
---@return paneflow.exec.handle? handle
---@return string? message
function paneflow.exec.start(argv, opts) end

---@param level "debug"|"info"|"warn"|"error"|any
---@param message any?
function paneflow.log(level, message) end
"#;

const RUNTIME_NAMESPACES: &[&str] = &["json", "layout", "exec", "args", "ctx"];

pub fn lua_stubs(descriptions: &[Value]) -> String {
    let mut out = String::new();
    out.push_str("---@meta paneflow\n\n");
    out.push_str("---@class paneflow\npaneflow = {}\n\n");
    out.push_str(RUNTIME_STUBS);
    let methods = method_names(descriptions);
    let mut declared: Vec<String> = RUNTIME_NAMESPACES
        .iter()
        .map(|name| format!("paneflow.{name}"))
        .collect();
    for (name, description) in methods {
        let parts: Vec<&str> = name.split('.').collect();
        let Some((leaf, namespaces)) = parts.split_last() else {
            continue;
        };
        if namespaces.is_empty() {
            continue;
        }
        let mut base = "paneflow".to_string();
        for namespace in namespaces {
            let next = member(&base, namespace);
            if !declared.contains(&next) {
                let _ = write!(out, "\n---@class {base}.{namespace}\n{next} = {{}}\n");
                declared.push(next.clone());
            }
            base = next;
        }
        let class = format!("paneflow.{name}");
        let params = fields(&description["params"]);
        let results = fields(&description["result"]);
        let _ = write!(out, "\n---@class {class}.params\n");
        for field in &params {
            let _ = writeln!(
                out,
                "---@field {}{} {}{}",
                field_name(&field.name),
                if field.optional { "?" } else { "" },
                field.lua,
                if field.about.is_empty() {
                    String::new()
                } else {
                    format!(" {}", field.about)
                }
            );
        }
        let _ = write!(out, "\n---@class {class}.result\n");
        for field in &results {
            let _ = writeln!(
                out,
                "---@field {}{} {}",
                field_name(&field.name),
                if field.optional { "?" } else { "" },
                field.lua
            );
        }
        let summary = description["summary"]
            .as_str()
            .map(one_line)
            .unwrap_or_default();
        let any_required = params.iter().any(|field| !field.optional);
        let _ = write!(out, "\n---{summary}\n");
        let _ = writeln!(
            out,
            "---@param params {class}.params{}",
            if any_required { "" } else { "?" }
        );
        let _ = writeln!(out, "---@return {class}.result? result");
        out.push_str("---@return string? message\n---@return integer? code\n");
        let target = member(&base, leaf);
        let _ = writeln!(out, "{target} = function(params) end");
    }
    out
}

fn markdown_cell(text: &str) -> String {
    one_line(text).replace('|', "\\|")
}

fn lua_literal(value: &Value) -> String {
    match value {
        Value::Null => "nil".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => quoted(text),
        Value::Array(items) => format!(
            "{{ {} }}",
            items.iter().map(lua_literal).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(fields) => {
            if fields.is_empty() {
                return "{}".to_string();
            }
            format!(
                "{{ {} }}",
                fields
                    .iter()
                    .map(|(key, value)| format!("{} = {}", field_name(key), lua_literal(value)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

pub fn markdown_reference(descriptions: &[Value]) -> String {
    let mut out = String::from(
        "# Paneflow API reference\n\nGenerated from the method registry linked into `paneflow` (`paneflow api docs`). Each method is reachable from Lua as `paneflow.<namespace>.<method>{...}` and from the shell as `paneflow api call <method>`.\n",
    );
    for (name, description) in method_names(descriptions) {
        let _ = write!(out, "\n## `{name}`\n\n");
        if let Some(summary) = description["summary"].as_str() {
            let _ = writeln!(out, "{}\n", one_line(summary));
        }
        let _ = writeln!(
            out,
            "Owner: {}. Gate: {}. Reply: {}.",
            description["owner"].as_str().unwrap_or("-"),
            description["gate"].as_str().unwrap_or("-"),
            description["reply"].as_str().unwrap_or("-"),
        );
        if let Some(note) = description["gate_note"].as_str() {
            let _ = writeln!(out, "\n{}", one_line(note));
        }
        out.push_str("\n### Parameters\n\n");
        let params = fields(&description["params"]);
        if params.is_empty() {
            out.push_str("None.\n");
        } else {
            out.push_str("| Name | Type | Required | Description |\n|---|---|---|---|\n");
            for field in params {
                let _ = writeln!(
                    out,
                    "| `{}` | `{}` | {} | {} |",
                    field.name,
                    markdown_cell(&field.lua),
                    if field.optional { "no" } else { "yes" },
                    markdown_cell(&field.about)
                );
            }
        }
        out.push_str("\n### Result\n\n");
        let results = fields(&description["result"]);
        if results.is_empty() {
            out.push_str("A value with no declared fields.\n");
        } else {
            out.push_str("| Field | Type |\n|---|---|\n");
            for field in results {
                let _ = writeln!(
                    out,
                    "| `{}` | `{}{}` |",
                    field.name,
                    markdown_cell(&field.lua),
                    if field.optional { "?" } else { "" }
                );
            }
        }
        if let Some(example) = description["examples"].as_array().and_then(|e| e.first()) {
            let call = match name.split_once('.') {
                Some(_) => {
                    let parts: Vec<&str> = name.split('.').collect();
                    let mut target = "paneflow".to_string();
                    for part in parts {
                        target = member(&target, part);
                    }
                    target
                }
                None => format!("paneflow.call({})", quoted(name)),
            };
            let params = &example["params"];
            let invocation = match params {
                Value::Object(fields) if fields.is_empty() => format!("{call}()"),
                Value::Null => format!("{call}()"),
                other => format!("{call}{}", lua_literal(other)),
            };
            let _ = write!(
                out,
                "\n### Example\n\n```lua\nlocal result = {invocation}\n```\n\n```json\n{}\n```\n",
                serde_json::to_string_pretty(&example["result"]).unwrap_or_default()
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keywords_and_non_identifiers_take_the_bracket_form() {
        assert!(is_identifier("surface"));
        assert!(!is_identifier("end"));
        assert!(!is_identifier("send-text"));
        assert!(!is_identifier("2fast"));
        assert_eq!(
            member("paneflow.surface", "end"),
            "paneflow.surface[\"end\"]"
        );
        assert_eq!(field_name("send-text"), "[\"send-text\"]");
    }

    #[test]
    fn an_optional_parameter_is_marked_optional() {
        let stubs = lua_stubs(&[json!({
            "name": "demo.end",
            "summary": "A method named with a keyword",
            "params": {
                "type": "object",
                "properties": {
                    "id": {"type": "integer"},
                    "label": {"type": ["string", "null"]},
                    "end": {"type": "boolean"},
                    "mode": {"enum": ["a", "b"]}
                },
                "required": ["id"]
            },
            "result": {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"]}
        })]);
        assert!(stubs.contains("---@field id integer\n"), "{stubs}");
        assert!(stubs.contains("---@field label? string\n"), "{stubs}");
        assert!(stubs.contains("---@field [\"end\"]? boolean\n"), "{stubs}");
        assert!(stubs.contains("---@field mode? \"a\"|\"b\"\n"), "{stubs}");
        assert!(
            stubs.contains("paneflow.demo[\"end\"] = function(params) end"),
            "{stubs}"
        );
        assert!(
            stubs.contains("---@param params paneflow.demo.end.params\n"),
            "{stubs}"
        );
    }
}
