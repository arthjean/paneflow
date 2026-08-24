use std::fmt;
use std::ops::Range;

use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsoncError(String);

impl JsoncError {
    fn invalid(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for JsoncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for JsoncError {}

/// Parse JSONC while accepting comments and trailing commas.
pub fn parse(input: &str) -> Result<Value, JsoncError> {
    Parser::new(input).parse_document()?;
    let without_comments = strip_comments(input)?;
    let normalized = remove_trailing_commas(&without_comments)?;
    serde_json::from_str(&normalized)
        .map_err(|error| JsoncError::invalid(format!("invalid JSONC: {error}")))
}

/// Upsert one nested object member without reserializing the surrounding file.
///
/// `Ok(None)` means the semantic value was already current. Otherwise only the
/// target value, or the minimum insertion needed for it, changes in the source.
pub fn upsert_entry(
    input: &str,
    container_key: &str,
    entry_key: &str,
    entry_value: &Value,
) -> Result<Option<String>, JsoncError> {
    let semantic = parse(input)?;
    let root = semantic
        .as_object()
        .ok_or_else(|| JsoncError::invalid("config root must be a JSON object"))?;
    if let Some(container) = root.get(container_key) {
        let container = container.as_object().ok_or_else(|| {
            JsoncError::invalid(format!("config key `{container_key}` must be an object"))
        })?;
        if container.get(entry_key) == Some(entry_value) {
            return Ok(None);
        }
    }

    let document = Parser::new(input).parse_document()?;
    let root_object = document
        .object
        .as_ref()
        .ok_or_else(|| JsoncError::invalid("config root must be a JSON object"))?;
    let updated = if let Some(container_index) = member_index(root_object, container_key) {
        let container = root_object.members[container_index]
            .value
            .object
            .as_ref()
            .ok_or_else(|| {
                JsoncError::invalid(format!("config key `{container_key}` must be an object"))
            })?;
        if let Some(entry_index) = member_index(container, entry_key) {
            replace_value(input, &container.members[entry_index].value, entry_value)?
        } else {
            insert_member(input, container, entry_key, entry_value)?
        }
    } else {
        let mut container = serde_json::Map::new();
        container.insert(entry_key.to_string(), entry_value.clone());
        insert_member(input, root_object, container_key, &Value::Object(container))?
    };
    parse(&updated)?;
    Ok(Some(updated))
}

/// Remove one nested object member without reserializing the surrounding file.
pub fn remove_entry(
    input: &str,
    container_key: &str,
    entry_key: &str,
) -> Result<Option<String>, JsoncError> {
    let semantic = parse(input)?;
    let root = semantic
        .as_object()
        .ok_or_else(|| JsoncError::invalid("config root must be a JSON object"))?;
    let Some(container) = root.get(container_key) else {
        return Ok(None);
    };
    let container = container.as_object().ok_or_else(|| {
        JsoncError::invalid(format!("config key `{container_key}` must be an object"))
    })?;
    if !container.contains_key(entry_key) {
        return Ok(None);
    }

    let document = Parser::new(input).parse_document()?;
    let root_object = document
        .object
        .as_ref()
        .ok_or_else(|| JsoncError::invalid("config root must be a JSON object"))?;
    let container_index = member_index(root_object, container_key).ok_or_else(|| {
        JsoncError::invalid(format!("could not locate config key `{container_key}`"))
    })?;
    let container = root_object.members[container_index]
        .value
        .object
        .as_ref()
        .ok_or_else(|| {
            JsoncError::invalid(format!("config key `{container_key}` must be an object"))
        })?;
    let entry_index = member_index(container, entry_key)
        .ok_or_else(|| JsoncError::invalid(format!("could not locate entry `{entry_key}`")))?;
    let updated = remove_member(input, container, entry_index);
    parse(&updated)?;
    Ok(Some(updated))
}

fn replace_value(input: &str, node: &Node, value: &Value) -> Result<String, JsoncError> {
    let serialized = serde_json::to_string(value)
        .map_err(|error| JsoncError::invalid(format!("serialize JSON value failed: {error}")))?;
    Ok(apply_edits(input, vec![(node.start..node.end, serialized)]))
}

fn insert_member(
    input: &str,
    object: &ObjectNode,
    key: &str,
    value: &Value,
) -> Result<String, JsoncError> {
    let key = serde_json::to_string(key)
        .map_err(|error| JsoncError::invalid(format!("serialize JSON key failed: {error}")))?;
    let value = serde_json::to_string(value)
        .map_err(|error| JsoncError::invalid(format!("serialize JSON value failed: {error}")))?;
    let close_line_start = line_start(input, object.close);
    let close_indent = &input[close_line_start..object.close];
    let close_is_indented = close_indent
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'));

    if close_is_indented && input[object.open..object.close].contains('\n') {
        let newline = if input.contains("\r\n") { "\r\n" } else { "\n" };
        let child_indent = object
            .members
            .first()
            .and_then(|member| indentation_at(input, member.start))
            .map(str::to_string)
            .unwrap_or_else(|| format!("{close_indent}  "));
        let preserves_trailing_comma = object
            .members
            .last()
            .is_some_and(|member| member.comma.is_some());
        let trailing_comma = if preserves_trailing_comma { "," } else { "" };
        let insertion = format!("{child_indent}{key}: {value}{trailing_comma}{newline}");
        let mut edits = vec![(close_line_start..close_line_start, insertion)];
        if let Some(last) = object.members.last() {
            if last.comma.is_none() {
                edits.push((last.value.end..last.value.end, ",".to_string()));
            }
        }
        return Ok(apply_edits(input, edits));
    }

    let insertion = match object.members.last() {
        None => format!("{key}: {value}"),
        Some(last) if last.comma.is_some() => format!(" {key}: {value},"),
        Some(_) => format!(", {key}: {value}"),
    };
    Ok(apply_edits(
        input,
        vec![(object.close..object.close, insertion)],
    ))
}

fn remove_member(input: &str, object: &ObjectNode, index: usize) -> String {
    let member = &object.members[index];
    let range = if let Some(comma) = &member.comma {
        member.start..comma.end
    } else if index > 0 {
        let previous = &object.members[index - 1];
        previous
            .comma
            .as_ref()
            .map_or(member.start..member.value.end, |comma| {
                comma.start..member.value.end
            })
    } else {
        member.start..member.value.end
    };
    apply_edits(input, vec![(range, String::new())])
}

fn apply_edits(input: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0.start));
    let mut output = input.to_string();
    for (range, replacement) in edits {
        output.replace_range(range, &replacement);
    }
    output
}

fn line_start(input: &str, position: usize) -> usize {
    input[..position].rfind('\n').map_or(0, |index| index + 1)
}

fn indentation_at(input: &str, position: usize) -> Option<&str> {
    let start = line_start(input, position);
    let indentation = &input[start..position];
    indentation
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'))
        .then_some(indentation)
}

fn member_index(object: &ObjectNode, key: &str) -> Option<usize> {
    object.members.iter().position(|member| member.key == key)
}

#[derive(Debug)]
struct Node {
    start: usize,
    end: usize,
    object: Option<ObjectNode>,
}

#[derive(Debug)]
struct ObjectNode {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

#[derive(Debug)]
struct Member {
    key: String,
    start: usize,
    value: Node,
    comma: Option<Range<usize>>,
}

struct Parser<'a> {
    input: &'a str,
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            bytes: input.as_bytes(),
            position: 0,
        }
    }

    fn parse_document(mut self) -> Result<Node, JsoncError> {
        self.skip_trivia()?;
        let node = self.parse_value()?;
        self.skip_trivia()?;
        if self.position != self.bytes.len() {
            return Err(self.error("unexpected content after root value"));
        }
        Ok(node)
    }

    fn parse_value(&mut self) -> Result<Node, JsoncError> {
        self.skip_trivia()?;
        let start = self.position;
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => {
                self.parse_string()?;
                Ok(Node {
                    start,
                    end: self.position,
                    object: None,
                })
            }
            Some(_) => self.parse_primitive(),
            None => Err(self.error("expected a JSON value")),
        }
    }

    fn parse_object(&mut self) -> Result<Node, JsoncError> {
        let start = self.position;
        self.expect(b'{')?;
        let open = start;
        let mut members: Vec<Member> = Vec::new();
        loop {
            self.skip_trivia()?;
            if self.peek() == Some(b'}') {
                let close = self.position;
                self.position += 1;
                return Ok(Node {
                    start,
                    end: self.position,
                    object: Some(ObjectNode {
                        open,
                        close,
                        members,
                    }),
                });
            }

            let member_start = self.position;
            let key_range = self.parse_string()?;
            let key: String = serde_json::from_str(&self.input[key_range.clone()])
                .map_err(|error| self.error(format!("invalid object key: {error}")))?;
            if members.iter().any(|member| member.key == key) {
                return Err(self.error(format!("duplicate object key `{key}` is ambiguous")));
            }
            self.skip_trivia()?;
            self.expect(b':')?;
            let value = self.parse_value()?;
            self.skip_trivia()?;
            let comma = if self.peek() == Some(b',') {
                let comma = self.position..self.position + 1;
                self.position += 1;
                Some(comma)
            } else {
                None
            };
            members.push(Member {
                key,
                start: member_start,
                value,
                comma,
            });
            self.skip_trivia()?;
            if members.last().is_some_and(|member| member.comma.is_none())
                && self.peek() != Some(b'}')
            {
                return Err(self.error("expected `,` or `}` after object member"));
            }
        }
    }

    fn parse_array(&mut self) -> Result<Node, JsoncError> {
        let start = self.position;
        self.expect(b'[')?;
        loop {
            self.skip_trivia()?;
            if self.peek() == Some(b']') {
                self.position += 1;
                return Ok(Node {
                    start,
                    end: self.position,
                    object: None,
                });
            }
            self.parse_value()?;
            self.skip_trivia()?;
            if self.peek() == Some(b',') {
                self.position += 1;
                continue;
            }
            if self.peek() != Some(b']') {
                return Err(self.error("expected `,` or `]` after array value"));
            }
        }
    }

    fn parse_string(&mut self) -> Result<Range<usize>, JsoncError> {
        let start = self.position;
        self.expect(b'"')?;
        let mut escaped = false;
        while let Some(byte) = self.peek() {
            self.position += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                return Ok(start..self.position);
            }
        }
        Err(self.error("unterminated JSON string"))
    }

    fn parse_primitive(&mut self) -> Result<Node, JsoncError> {
        let start = self.position;
        while let Some(byte) = self.peek() {
            if byte.is_ascii_whitespace() || matches!(byte, b',' | b']' | b'}') {
                break;
            }
            if byte == b'/' && matches!(self.bytes.get(self.position + 1), Some(b'/') | Some(b'*'))
            {
                break;
            }
            self.position += 1;
        }
        if self.position == start {
            return Err(self.error("expected a JSON value"));
        }
        Ok(Node {
            start,
            end: self.position,
            object: None,
        })
    }

    fn skip_trivia(&mut self) -> Result<(), JsoncError> {
        loop {
            while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
                self.position += 1;
            }
            if self.bytes.get(self.position..self.position + 2) == Some(b"//") {
                self.position += 2;
                while self.peek().is_some_and(|byte| byte != b'\n') {
                    self.position += 1;
                }
                continue;
            }
            if self.bytes.get(self.position..self.position + 2) == Some(b"/*") {
                self.position += 2;
                let mut closed = false;
                while self.position + 1 < self.bytes.len() {
                    if self.bytes.get(self.position..self.position + 2) == Some(b"*/") {
                        self.position += 2;
                        closed = true;
                        break;
                    }
                    self.position += 1;
                }
                if !closed {
                    return Err(self.error("unterminated block comment"));
                }
                continue;
            }
            return Ok(());
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), JsoncError> {
        if self.peek() == Some(expected) {
            self.position += 1;
            Ok(())
        } else {
            Err(self.error(format!("expected `{}`", char::from(expected))))
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn error(&self, message: impl Into<String>) -> JsoncError {
        JsoncError::invalid(format!("{} at byte {}", message.into(), self.position))
    }
}

fn strip_comments(input: &str) -> Result<String, JsoncError> {
    let bytes = input.as_bytes();
    let mut output = bytes.to_vec();
    let mut position = 0;
    let mut in_string = false;
    let mut escaped = false;

    while position < bytes.len() {
        let byte = bytes[position];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            position += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            position += 1;
            continue;
        }
        if bytes.get(position..position + 2) == Some(b"//") {
            output[position] = b' ';
            output[position + 1] = b' ';
            position += 2;
            while position < bytes.len() && bytes[position] != b'\n' {
                output[position] = b' ';
                position += 1;
            }
            continue;
        }
        if bytes.get(position..position + 2) == Some(b"/*") {
            output[position] = b' ';
            output[position + 1] = b' ';
            position += 2;
            let mut closed = false;
            while position < bytes.len() {
                if bytes.get(position..position + 2) == Some(b"*/") {
                    output[position] = b' ';
                    output[position + 1] = b' ';
                    position += 2;
                    closed = true;
                    break;
                }
                if bytes[position] != b'\n' && bytes[position] != b'\r' {
                    output[position] = b' ';
                }
                position += 1;
            }
            if !closed {
                return Err(JsoncError::invalid("unterminated block comment"));
            }
            continue;
        }
        position += 1;
    }
    String::from_utf8(output)
        .map_err(|error| JsoncError::invalid(format!("JSONC must remain UTF-8: {error}")))
}

fn remove_trailing_commas(input: &str) -> Result<String, JsoncError> {
    let bytes = input.as_bytes();
    let mut output = bytes.to_vec();
    let mut position = 0;
    let mut in_string = false;
    let mut escaped = false;
    while position < bytes.len() {
        let byte = bytes[position];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            position += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            position += 1;
            continue;
        }
        if byte == b',' {
            let next = bytes[position + 1..]
                .iter()
                .copied()
                .find(|candidate| !candidate.is_ascii_whitespace());
            if matches!(next, Some(b'}') | Some(b']')) {
                output[position] = b' ';
                position += 1;
                continue;
            }
        }
        position += 1;
    }
    String::from_utf8(output)
        .map_err(|error| JsoncError::invalid(format!("JSONC must remain UTF-8: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SOURCE: &str = r#"{
  // this comment must survive
  "$schema": "https://example.test/schema.json",
  "mcp": {
    "weather": { "command": ["weather-mcp"] },
    // Paneflow entry comment
    "paneflow": { "command": ["/old"] },
  },
}"#;

    #[test]
    fn upsert_changes_only_managed_value() {
        let updated = upsert_entry(
            SOURCE,
            "mcp",
            "paneflow",
            &json!({ "command": ["/new"], "enabled": true }),
        )
        .unwrap()
        .unwrap();
        assert!(updated.contains("// this comment must survive"));
        assert!(updated.contains("// Paneflow entry comment"));
        assert!(updated.contains("\"weather\": { \"command\": [\"weather-mcp\"] }"));
        assert!(
            updated.contains("},\n  },"),
            "trailing commas should survive: {updated}"
        );
        assert_eq!(
            parse(&updated).unwrap()["mcp"]["paneflow"]["command"],
            json!(["/new"])
        );
    }

    #[test]
    fn insert_and_remove_preserve_comments_and_siblings() {
        let source = r#"{
  "mcp": {
    // existing provider
    "weather": { "command": ["weather-mcp"] },
  },
  "theme": "dark",
}"#;
        let inserted = upsert_entry(source, "mcp", "paneflow", &json!({ "command": ["/p"] }))
            .unwrap()
            .unwrap();
        assert!(inserted.contains("// existing provider"));
        assert!(inserted.contains("\"theme\": \"dark\""));
        let removed = remove_entry(&inserted, "mcp", "paneflow").unwrap().unwrap();
        assert!(removed.contains("// existing provider"));
        assert!(removed.contains("weather-mcp"));
        assert!(parse(&removed).unwrap()["mcp"].get("paneflow").is_none());
    }

    #[test]
    fn missing_container_is_inserted_without_reformatting_root() {
        let source = "{\r\n\t// keep CRLF and tab style\r\n\t\"theme\": \"dark\"\r\n}\r\n";
        let updated = upsert_entry(source, "mcp", "paneflow", &json!({ "command": ["/p"] }))
            .unwrap()
            .unwrap();
        assert!(updated.contains("// keep CRLF and tab style\r\n"));
        assert!(updated.contains("\r\n"));
        assert_eq!(parse(&updated).unwrap()["theme"], json!("dark"));
        assert_eq!(
            parse(&updated).unwrap()["mcp"]["paneflow"]["command"],
            json!(["/p"])
        );
    }

    #[test]
    fn semantic_noop_preserves_source_byte_for_byte() {
        let result =
            upsert_entry(SOURCE, "mcp", "paneflow", &json!({ "command": ["/old"] })).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn invalid_boundaries_and_duplicate_keys_are_refused() {
        assert!(upsert_entry("[]", "mcp", "paneflow", &json!({})).is_err());
        assert!(upsert_entry("{\"mcp\": 1}", "mcp", "paneflow", &json!({})).is_err());
        assert!(upsert_entry("{\"mcp\": {}, \"mcp\": {}}", "mcp", "paneflow", &json!({})).is_err());
        let duplicate_current =
            r#"{"mcp":{"paneflow":{"command":["/old"]},"paneflow":{"command":["/old"]}}}"#;
        assert!(parse(duplicate_current).is_err());
        assert!(upsert_entry(
            duplicate_current,
            "mcp",
            "paneflow",
            &json!({ "command": ["/old"] })
        )
        .is_err());
        assert!(remove_entry(r#"{"mcp":{},"mcp":{}}"#, "mcp", "paneflow").is_err());
        assert!(parse("{/* unterminated").is_err());
    }
}
