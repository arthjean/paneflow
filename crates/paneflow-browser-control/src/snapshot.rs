use std::collections::{HashMap, HashSet};

use serde_json::Value;

pub const HIDDEN_VALUE: &str = "<hidden>";

const MAX_NAME_CHARS: usize = 200;

const INTERACTIVE: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "checkbox",
    "radio",
    "combobox",
    "listbox",
    "option",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "switch",
    "slider",
    "spinbutton",
    "treeitem",
    "textField",
    "PopUpButton",
];

const VALUED: &[&str] = &[
    "textbox",
    "searchbox",
    "combobox",
    "spinbutton",
    "slider",
    "textField",
    "PopUpButton",
];

const SKIPPED: &[&str] = &["InlineTextBox", "LineBreak"];

const TRANSPARENT_WITHOUT_NAME: &[&str] = &[
    "generic",
    "none",
    "presentation",
    "LabelText",
    "paragraph",
    "group",
    "div",
    "Section",
    "MenuListPopup",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub tree: String,
    pub dropped_bytes: u64,
    pub refs: HashMap<String, i64>,
    pub title: Option<String>,
}

pub fn password_candidates(nodes: &[Value]) -> Vec<i64> {
    nodes
        .iter()
        .filter(|node| node["ignored"].as_bool() != Some(true))
        .filter(|node| VALUED.contains(&role(node)))
        .filter(|node| !node_value(node).is_empty())
        .filter_map(|node| node["backendDOMNodeId"].as_i64())
        .collect()
}

pub fn render(nodes: &[Value], passwords: &HashSet<i64>, bound: usize) -> Rendered {
    let by_id: HashMap<&str, &Value> = nodes
        .iter()
        .filter_map(|node| node["nodeId"].as_str().map(|id| (id, node)))
        .collect();
    let root = nodes
        .iter()
        .find(|node| node.get("parentId").is_none_or(Value::is_null))
        .or_else(|| nodes.first());
    let mut out = Rendered {
        tree: String::new(),
        dropped_bytes: 0,
        refs: HashMap::new(),
        title: None,
    };
    let Some(root) = root else {
        return out;
    };
    if role(root) == "RootWebArea" {
        out.title = Some(clean(&text_of(&root["name"]))).filter(|title| !title.is_empty());
    }
    let mut next_ref = 1usize;
    let mut stack: Vec<(&Value, usize)> = vec![(root, 0)];
    let mut visited: HashSet<&str> = HashSet::new();
    while let Some((node, depth)) = stack.pop() {
        if let Some(id) = node["nodeId"].as_str()
            && !visited.insert(id)
        {
            continue;
        }
        let node_role = role(node);
        if SKIPPED.contains(&node_role) {
            continue;
        }
        let name = clean(&text_of(&node["name"]));
        let ignored = node["ignored"].as_bool() == Some(true);
        let transparent = ignored
            || (name.is_empty() && TRANSPARENT_WITHOUT_NAME.contains(&node_role))
            || (node_role == "StaticText" && name.is_empty());
        let child_depth = if transparent { depth } else { depth + 1 };
        let children: Vec<&Value> = node["childIds"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .filter_map(|id| by_id.get(id).copied())
                    .collect()
            })
            .unwrap_or_default();
        for child in children.into_iter().rev() {
            stack.push((child, child_depth));
        }
        if transparent {
            continue;
        }
        let fits = out.dropped_bytes == 0;
        let interactive = INTERACTIVE.contains(&node_role)
            || (property(node, "focusable") == Some(&Value::Bool(true))
                && !matches!(node_role, "RootWebArea" | "generic" | "StaticText"));
        let reference = (interactive && fits).then(|| {
            let reference = format!("e{next_ref}");
            next_ref += 1;
            reference
        });
        let line = line(
            node,
            node_role,
            &name,
            depth,
            reference.as_deref(),
            passwords,
        );
        if fits && out.tree.len() + line.len() < bound {
            if let (Some(reference), Some(backend)) = (reference, node["backendDOMNodeId"].as_i64())
            {
                out.refs.insert(reference, backend);
            }
            out.tree.push_str(&line);
            out.tree.push('\n');
        } else {
            out.dropped_bytes += line.len() as u64 + 1;
        }
    }
    if out.tree.ends_with('\n') {
        out.tree.pop();
    }
    out
}

fn line(
    node: &Value,
    node_role: &str,
    name: &str,
    depth: usize,
    reference: Option<&str>,
    passwords: &HashSet<i64>,
) -> String {
    let mut line = " ".repeat(depth * 2);
    let shown_role = if node_role == "StaticText" {
        "text"
    } else {
        node_role
    };
    line.push_str(shown_role);
    if !name.is_empty() {
        let shown = if node_role != "ListMarker" && is_masked(name) {
            HIDDEN_VALUE
        } else {
            name
        };
        line.push_str(&format!(" \"{shown}\""));
    }
    if let Some(level) = property(node, "level").and_then(Value::as_u64) {
        line.push_str(&format!(" [level={level}]"));
    }
    if VALUED.contains(&node_role) {
        let value = node_value(node);
        let secret = node["backendDOMNodeId"]
            .as_i64()
            .is_some_and(|backend| passwords.contains(&backend))
            || is_masked(&value);
        if secret {
            line.push_str(&format!(" value={HIDDEN_VALUE}"));
        } else if !value.is_empty() && value != name {
            line.push_str(&format!(" value=\"{}\"", clean(&value)));
        }
    }
    for flag in ["disabled", "expanded", "selected", "required", "invalid"] {
        match property(node, flag) {
            Some(Value::Bool(true)) => line.push_str(&format!(" [{flag}]")),
            Some(Value::String(token)) if flag == "invalid" && token != "false" => {
                line.push_str(" [invalid]")
            }
            _ => {}
        }
    }
    match property(node, "checked") {
        Some(Value::String(state)) if state == "true" => line.push_str(" [checked]"),
        Some(Value::String(state)) if state == "mixed" => line.push_str(" [checked=mixed]"),
        _ => {}
    }
    if let Some(reference) = reference {
        line.push_str(&format!(" [ref={reference}]"));
    }
    line
}

fn role(node: &Value) -> &str {
    node["role"]["value"].as_str().unwrap_or("unknown")
}

fn property<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node["properties"]
        .as_array()?
        .iter()
        .find(|property| property["name"].as_str() == Some(name))
        .map(|property| &property["value"]["value"])
}

fn node_value(node: &Value) -> String {
    text_of(&node["value"])
}

fn text_of(value: &Value) -> String {
    match &value["value"] {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn is_masked(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| matches!(c, '\u{2022}' | '\u{25cf}' | '*'))
}

fn clean(text: &str) -> String {
    let collapsed: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('"', "\\\"");
    if collapsed.chars().count() > MAX_NAME_CHARS {
        let cut: String = collapsed.chars().take(MAX_NAME_CHARS).collect();
        format!("{cut}...")
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(id: &str, parent: Option<&str>, role: &str, name: &str, children: &[&str]) -> Value {
        let mut node = json!({
            "nodeId": id,
            "ignored": false,
            "role": {"type": "role", "value": role},
            "name": {"type": "computedString", "value": name},
            "childIds": children,
            "backendDOMNodeId": id.parse::<i64>().unwrap() + 100,
        });
        if let Some(parent) = parent {
            node["parentId"] = json!(parent);
        }
        node
    }

    fn page() -> Vec<Value> {
        let mut password = node("5", Some("3"), "textbox", "Password", &[]);
        password["value"] = json!({"type": "string", "value": "••••••"});
        let mut email = node("4", Some("3"), "textbox", "Email", &[]);
        email["value"] = json!({"type": "string", "value": "a@b.c"});
        let mut disabled = node("6", Some("3"), "checkbox", "Remember", &[]);
        disabled["properties"] =
            json!([{"name": "disabled", "value": {"type": "boolean", "value": true}}]);
        vec![
            node("1", None, "RootWebArea", "Checkout form", &["2", "3"]),
            node("2", Some("1"), "heading", "Checkout", &[]),
            node("3", Some("1"), "generic", "", &["4", "5", "6", "7"]),
            email,
            password,
            disabled,
            node("7", Some("3"), "button", "Place order", &[]),
        ]
    }

    #[test]
    fn interactive_nodes_get_a_reference_and_containers_are_flattened() {
        let rendered = render(&page(), &HashSet::new(), 32 * 1024);
        assert_eq!(
            rendered.tree,
            "RootWebArea \"Checkout form\"\n  heading \"Checkout\"\n  textbox \"Email\" value=\"a@b.c\" [ref=e1]\n  textbox \"Password\" value=<hidden> [ref=e2]\n  checkbox \"Remember\" [disabled] [ref=e3]\n  button \"Place order\" [ref=e4]"
        );
        assert_eq!(rendered.refs["e1"], 104);
        assert_eq!(rendered.refs["e4"], 107);
        assert_eq!(rendered.title.as_deref(), Some("Checkout form"));
    }

    #[test]
    fn a_list_bullet_is_not_mistaken_for_a_masked_password() {
        let nodes = vec![
            node("1", None, "RootWebArea", "List", &["2"]),
            node("2", Some("1"), "ListMarker", "\u{2022} ", &[]),
        ];
        let rendered = render(&nodes, &HashSet::new(), 1024);
        assert!(
            rendered.tree.contains("ListMarker \"\u{2022}\""),
            "{}",
            rendered.tree
        );
    }

    #[test]
    fn a_password_field_never_shows_its_value_even_unmasked() {
        let mut nodes = page();
        nodes[4]["value"] = json!({"type": "string", "value": "hunter2-secret"});
        let rendered = render(&nodes, &[105].into_iter().collect(), 32 * 1024);
        assert!(!rendered.tree.contains("hunter2"), "{}", rendered.tree);
        assert!(
            rendered
                .tree
                .contains("textbox \"Password\" value=<hidden>")
        );
    }

    #[test]
    fn a_tree_over_the_bound_is_cut_at_a_line_and_states_what_was_dropped() {
        let mut nodes = vec![node("1", None, "RootWebArea", "Big", &[])];
        let children: Vec<String> = (2..400).map(|id| id.to_string()).collect();
        nodes[0]["childIds"] = json!(children);
        for id in &children {
            nodes.push(node(
                id,
                Some("1"),
                "button",
                &format!("Button number {id}"),
                &[],
            ));
        }
        let full = render(&nodes, &HashSet::new(), usize::MAX);
        let bounded = render(&nodes, &HashSet::new(), 1024);
        assert!(bounded.tree.len() <= 1024);
        assert!(bounded.dropped_bytes > 0);
        assert!(full.tree.starts_with(&bounded.tree));
        assert!(bounded.tree.len() as u64 + bounded.dropped_bytes <= full.tree.len() as u64 + 1);
        assert!(bounded.refs.len() < full.refs.len());
    }
}
