use serde_json::{Map, Value};

pub const META_SCHEMA: &str = "https://json-schema.org/draft/2020-12/schema";

const ANNOTATIONS: &[&str] = &[
    "$schema",
    "$defs",
    "$id",
    "$comment",
    "title",
    "description",
    "default",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
    "format",
];

const ASSERTIONS: &[&str] = &[
    "$ref",
    "type",
    "enum",
    "const",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "minItems",
    "maxItems",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "anyOf",
    "oneOf",
    "allOf",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Violation {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Key(String),
    Index(usize),
}

fn render(path: &[Segment]) -> String {
    let mut rendered = String::new();
    for segment in path {
        match segment {
            Segment::Key(key) if rendered.is_empty() => rendered.push_str(key),
            Segment::Key(key) => {
                rendered.push('.');
                rendered.push_str(key);
            }
            Segment::Index(index) => rendered.push_str(&format!("[{index}]")),
        }
    }
    rendered
}

fn violation(path: &[Segment], expectation: &str) -> Violation {
    let rendered = render(path);
    let message = if rendered.is_empty() {
        format!("params must be {expectation}")
    } else {
        format!("'{rendered}' must be {expectation}")
    };
    Violation {
        path: rendered,
        message,
    }
}

fn missing(path: &[Segment], key: &str) -> Violation {
    let mut full = path.to_vec();
    full.push(Segment::Key(key.to_string()));
    let rendered = render(&full);
    Violation {
        message: format!("missing '{rendered}' parameter"),
        path: rendered,
    }
}

fn resolve<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix('#')?;
    root.pointer(pointer)
}

fn type_names(schema: &Map<String, Value>) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn article(name: &str) -> &'static str {
    match name {
        "integer" | "array" | "object" => "an",
        _ => "a",
    }
}

fn describe(root: &Value, schema: &Value) -> String {
    let Some(map) = schema.as_object() else {
        return "a valid value".to_string();
    };
    if let Some(target) = map
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| resolve(root, reference))
    {
        return describe(root, target);
    }
    if let Some(values) = map.get("enum").and_then(Value::as_array) {
        let listed: Vec<String> = values.iter().map(Value::to_string).collect();
        return format!("one of {}", listed.join(", "));
    }
    if let Some(value) = map.get("const") {
        return value.to_string();
    }
    if let Some(branches) = map
        .get("anyOf")
        .or_else(|| map.get("oneOf"))
        .and_then(Value::as_array)
    {
        let forms: Vec<String> = branches
            .iter()
            .filter(|branch| branch.get("type").and_then(Value::as_str) != Some("null"))
            .map(|branch| describe(root, branch))
            .collect();
        return forms.join(" or ");
    }
    let non_negative = map
        .get("minimum")
        .and_then(Value::as_f64)
        .is_some_and(|minimum| minimum >= 0.0);
    let names: Vec<&str> = type_names(map)
        .into_iter()
        .filter(|name| *name != "null")
        .collect();
    let forms: Vec<String> = names
        .iter()
        .map(|name| match *name {
            "integer" if non_negative => "a non-negative integer".to_string(),
            name => format!("{} {name}", article(name)),
        })
        .collect();
    if forms.is_empty() {
        "a valid value".to_string()
    } else {
        forms.join(" or ")
    }
}

fn matches_type(name: &str, value: &Value) -> bool {
    match name {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "string" => value.is_string(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

fn depth(violation: &Violation) -> usize {
    violation.path.matches(['.', '[']).count() + usize::from(!violation.path.is_empty())
}

fn check_branches(
    root: &Value,
    schema: &Value,
    branches: &[Value],
    value: &Value,
    path: &mut Vec<Segment>,
    exactly_one: bool,
) -> Result<(), Violation> {
    let mut passed = 0usize;
    let mut deepest: Option<Violation> = None;
    for branch in branches {
        match check(root, branch, value, path) {
            Ok(()) => passed += 1,
            Err(failure) => {
                let deeper = deepest
                    .as_ref()
                    .is_none_or(|held| depth(&failure) > depth(held));
                if deeper {
                    deepest = Some(failure);
                }
            }
        }
    }
    match passed {
        0 => match deepest {
            Some(failure) if depth(&failure) > path.len() => Err(failure),
            _ => Err(violation(path, &describe(root, schema))),
        },
        1 => Ok(()),
        _ if exactly_one => Err(violation(path, "exactly one of its allowed forms")),
        _ => Ok(()),
    }
}

fn check(
    root: &Value,
    schema: &Value,
    value: &Value,
    path: &mut Vec<Segment>,
) -> Result<(), Violation> {
    let map = match schema {
        Value::Bool(true) => return Ok(()),
        Value::Bool(false) => return Err(violation(path, "absent")),
        Value::Object(map) => map,
        _ => return Ok(()),
    };
    if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
        let target = resolve(root, reference).ok_or_else(|| Violation {
            path: render(path),
            message: format!("the schema reference {reference} does not resolve"),
        })?;
        check(root, target, value, path)?;
    }
    if let Some(all) = map.get("allOf").and_then(Value::as_array) {
        for branch in all {
            check(root, branch, value, path)?;
        }
    }
    if let Some(any) = map.get("anyOf").and_then(Value::as_array) {
        check_branches(root, schema, any, value, path, false)?;
    }
    if let Some(one) = map.get("oneOf").and_then(Value::as_array) {
        check_branches(root, schema, one, value, path, true)?;
    }
    let names = type_names(map);
    if !names.is_empty() && !names.iter().any(|name| matches_type(name, value)) {
        return Err(violation(path, &describe(root, schema)));
    }
    if let Some(allowed) = map.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        return Err(violation(path, &describe(root, schema)));
    }
    if let Some(expected) = map.get("const")
        && expected != value
    {
        return Err(violation(path, &describe(root, schema)));
    }
    if let Some(number) = value.as_f64() {
        let bound = |key: &str| map.get(key).and_then(Value::as_f64);
        let out_of_range = bound("minimum").is_some_and(|minimum| number < minimum)
            || bound("maximum").is_some_and(|maximum| number > maximum)
            || bound("exclusiveMinimum").is_some_and(|minimum| number <= minimum)
            || bound("exclusiveMaximum").is_some_and(|maximum| number >= maximum);
        if out_of_range {
            return Err(violation(path, &describe_range(root, schema, map)));
        }
    }
    if let Some(text) = value.as_str() {
        let length = text.chars().count() as u64;
        let below = map
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|minimum| length < minimum);
        let above = map
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|maximum| length > maximum);
        if below || above {
            return Err(violation(path, &describe_length(map, "characters")));
        }
    }
    if let Some(items) = value.as_array() {
        let count = items.len() as u64;
        let below = map
            .get("minItems")
            .and_then(Value::as_u64)
            .is_some_and(|minimum| count < minimum);
        let above = map
            .get("maxItems")
            .and_then(Value::as_u64)
            .is_some_and(|maximum| count > maximum);
        if below || above {
            return Err(violation(path, &describe_items(map)));
        }
        if let Some(item_schema) = map.get("items") {
            for (index, item) in items.iter().enumerate() {
                path.push(Segment::Index(index));
                let checked = check(root, item_schema, item, path);
                path.pop();
                checked?;
            }
        }
    }
    if let Some(object) = value.as_object() {
        check_object(root, map, object, path)?;
    }
    Ok(())
}

fn check_object(
    root: &Value,
    map: &Map<String, Value>,
    object: &Map<String, Value>,
    path: &mut Vec<Segment>,
) -> Result<(), Violation> {
    if let Some(required) = map.get("required").and_then(Value::as_array) {
        for key in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(key) {
                return Err(missing(path, key));
            }
        }
    }
    let properties = map.get("properties").and_then(Value::as_object);
    for (key, field) in object {
        let declared = properties.and_then(|properties| properties.get(key));
        let field_schema = match (declared, map.get("additionalProperties")) {
            (Some(schema), _) => schema,
            (None, Some(extra)) => extra,
            (None, None) => continue,
        };
        path.push(Segment::Key(key.clone()));
        let checked = match (declared, field_schema) {
            (None, Value::Bool(false)) => Err(Violation {
                path: render(path),
                message: format!("'{}' is not a known field", render(path)),
            }),
            _ => check(root, field_schema, field, path),
        };
        path.pop();
        checked?;
    }
    Ok(())
}

fn describe_range(root: &Value, schema: &Value, map: &Map<String, Value>) -> String {
    let base = describe(root, schema);
    match (
        map.get("minimum").and_then(Value::as_f64),
        map.get("maximum").and_then(Value::as_f64),
    ) {
        (Some(minimum), Some(maximum)) => format!("{base} between {minimum} and {maximum}"),
        (Some(minimum), None) if minimum != 0.0 => format!("{base} of at least {minimum}"),
        (None, Some(maximum)) => format!("{base} of at most {maximum}"),
        _ => base,
    }
}

fn describe_length(map: &Map<String, Value>, unit: &str) -> String {
    match (
        map.get("minLength").and_then(Value::as_u64),
        map.get("maxLength").and_then(Value::as_u64),
    ) {
        (Some(minimum), Some(maximum)) => format!("a string of {minimum} to {maximum} {unit}"),
        (Some(minimum), None) => format!("a string of at least {minimum} {unit}"),
        (None, Some(maximum)) => format!("a string of at most {maximum} {unit}"),
        (None, None) => "a string".to_string(),
    }
}

fn describe_items(map: &Map<String, Value>) -> String {
    match (
        map.get("minItems").and_then(Value::as_u64),
        map.get("maxItems").and_then(Value::as_u64),
    ) {
        (Some(minimum), Some(maximum)) => format!("an array of {minimum} to {maximum} items"),
        (Some(minimum), None) => format!("an array of at least {minimum} items"),
        (None, Some(maximum)) => format!("an array of at most {maximum} items"),
        (None, None) => "an array".to_string(),
    }
}

pub fn validate(schema: &Value, value: &Value) -> Result<(), Violation> {
    check(schema, schema, value, &mut Vec::new())
}

pub fn unsupported_keywords(schema: &Value) -> Vec<String> {
    let mut found = Vec::new();
    collect_unsupported(schema, "", &mut found);
    found
}

fn collect_unsupported(schema: &Value, at: &str, found: &mut Vec<String>) {
    let Some(map) = schema.as_object() else {
        return;
    };
    for (key, child) in map {
        let here = format!("{at}/{key}");
        if !ANNOTATIONS.contains(&key.as_str()) && !ASSERTIONS.contains(&key.as_str()) {
            found.push(here.clone());
        }
        match key.as_str() {
            "properties" | "$defs" => {
                if let Some(children) = child.as_object() {
                    for (name, nested) in children {
                        collect_unsupported(nested, &format!("{here}/{name}"), found);
                    }
                }
            }
            "anyOf" | "oneOf" | "allOf" => {
                if let Some(branches) = child.as_array() {
                    for (index, branch) in branches.iter().enumerate() {
                        collect_unsupported(branch, &format!("{here}/{index}"), found);
                    }
                }
            }
            "items" | "additionalProperties" => collect_unsupported(child, &here, found),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn integer_field() -> Value {
        json!({
            "type": "object",
            "properties": {
                "surface_id": {"type": ["integer", "null"], "format": "uint64", "minimum": 0},
                "env": {"type": ["object", "null"], "additionalProperties": {"type": "string"}},
                "scope": {"anyOf": [{"$ref": "#/$defs/Scope"}, {"type": "null"}]},
                "panes": {"type": "array", "items": {"$ref": "#/$defs/Pane"}, "minItems": 1},
            },
            "required": ["panes"],
            "$defs": {
                "Scope": {"type": "string", "enum": ["workspace", "all"]},
                "Pane": {"type": "object", "properties": {"cwd": {"type": ["string", "null"]}}},
            },
        })
    }

    #[test]
    fn a_mistyped_integer_names_the_parameter_as_the_hand_written_checks_did() {
        let error = validate(
            &integer_field(),
            &json!({"panes": [{}], "surface_id": "12"}),
        )
        .expect_err("a string is not an id");
        assert_eq!(error.message, "'surface_id' must be a non-negative integer");
        assert_eq!(error.path, "surface_id");
        for bad in [json!(-1), json!(1.5), json!(true)] {
            assert!(
                validate(&integer_field(), &json!({"panes": [{}], "surface_id": bad})).is_err(),
                "{bad}"
            );
        }
        assert!(
            validate(
                &integer_field(),
                &json!({"panes": [{}], "surface_id": null})
            )
            .is_ok()
        );
    }

    #[test]
    fn nested_fields_are_named_with_their_path() {
        let error = validate(
            &integer_field(),
            &json!({"panes": [{}], "env": {"PORT": 8080}}),
        )
        .expect_err("a number is not an env value");
        assert_eq!(error.message, "'env.PORT' must be a string");
        let error = validate(&integer_field(), &json!({"panes": [{"cwd": 5}]}))
            .expect_err("a number is not a cwd");
        assert_eq!(error.message, "'panes[0].cwd' must be a string");
    }

    #[test]
    fn a_missing_required_field_and_an_enum_violation_read_plainly() {
        let error = validate(&integer_field(), &json!({})).expect_err("panes is required");
        assert_eq!(error.message, "missing 'panes' parameter");
        let error = validate(&integer_field(), &json!({"panes": []})).expect_err("empty");
        assert_eq!(
            error.message,
            "'panes' must be an array of at least 1 items"
        );
        let error = validate(&integer_field(), &json!({"panes": [{}], "scope": "galaxy"}))
            .expect_err("an unknown scope");
        assert_eq!(
            error.message,
            "'scope' must be one of \"workspace\", \"all\""
        );
    }

    #[test]
    fn unknown_fields_pass_unless_the_schema_closes_the_object() {
        assert!(validate(&integer_field(), &json!({"panes": [{}], "extra": 1})).is_ok());
        let closed = json!({"type": "object", "properties": {}, "additionalProperties": false});
        let error = validate(&closed, &json!({"extra": 1})).expect_err("closed");
        assert_eq!(error.message, "'extra' is not a known field");
    }

    #[test]
    fn an_unknown_keyword_is_reported_with_its_location() {
        let schema = json!({"type": "object", "properties": {"a": {"pattern": "x"}}});
        assert_eq!(unsupported_keywords(&schema), vec!["/properties/a/pattern"]);
        assert!(unsupported_keywords(&integer_field()).is_empty());
    }
}
