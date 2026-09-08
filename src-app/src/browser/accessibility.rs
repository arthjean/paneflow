use std::collections::{BTreeMap, BTreeSet};

use gpui::{
    A11ySubtreeBuilder,
    accesskit::{Node, Role},
};
use paneflow_browser_protocol::Document;
use serde_json::Value;

const MAX_NODES: usize = 4096;
const MAX_BYTES: usize = 512 * 1024;

#[derive(Clone, Default)]
pub(super) struct AccessibilityTree {
    document: Option<Document>,
    tree_id: String,
    root: Option<i64>,
    focus: Option<i64>,
    nodes: BTreeMap<i64, Value>,
    available: bool,
}

impl AccessibilityTree {
    pub(super) fn reset(&mut self, document: Option<Document>) {
        *self = Self {
            document,
            ..Self::default()
        };
    }

    pub(super) fn apply(&mut self, document: &Document, kind: &str, value: &Value) -> bool {
        if self.document.as_ref() != Some(document) {
            return false;
        }
        let value = if value.get("native").is_some() {
            if value["native"].as_str() != Some(kind)
                || serde_json::from_value::<Document>(value["document"].clone())
                    .ok()
                    .as_ref()
                    != Some(document)
            {
                return false;
            }
            &value["value"]
        } else {
            value
        };
        if kind == "accessibility_unavailable" || value.to_string().len() > MAX_BYTES {
            self.nodes.clear();
            self.root = None;
            self.available = false;
            return true;
        }
        let mut candidate = self.clone();
        if candidate.update(kind, value).is_err() {
            self.nodes.clear();
            self.root = None;
            self.available = false;
        } else {
            *self = candidate;
        }
        true
    }

    fn update(&mut self, kind: &str, value: &Value) -> Result<(), ()> {
        if kind == "accessibility_location" {
            for update in value.as_array().ok_or(())? {
                if update["ax_tree_id"].as_str() != Some(self.tree_id.as_str()) {
                    continue;
                }
                if let Some(node) = update["id"].as_i64().and_then(|id| self.nodes.get_mut(&id)) {
                    node["location"] = update["new_location"]["bounds"].clone();
                    node["offset_container_id"] =
                        update["new_location"]["offset_container_id"].clone();
                    node["transform"] = update["new_location"]["transform"].clone();
                }
            }
            if self
                .nodes
                .values()
                .map(|node| node.to_string().len())
                .sum::<usize>()
                > MAX_BYTES
            {
                return Err(());
            }
            return Ok(());
        }
        if kind != "accessibility_tree" {
            return Err(());
        }
        let tree_id = value["ax_tree_id"].as_str().ok_or(())?;
        if self.tree_id.is_empty() {
            self.tree_id = tree_id.to_owned();
        }
        if tree_id != self.tree_id {
            return Ok(());
        }
        let updates = match value.get("updates") {
            Some(updates) => Some(updates.as_array().ok_or(())?),
            None => None,
        };
        for update in updates.into_iter().flatten() {
            if let Some(focus) = update["tree_data"]["focus_id"].as_i64() {
                self.focus = Some(focus);
            }
            if let Some(id) = update["node_id_to_clear"].as_i64().filter(|id| *id > 0) {
                let mut pending = vec![id];
                let mut seen = BTreeSet::new();
                while let Some(id) = pending.pop() {
                    if !seen.insert(id) {
                        return Err(());
                    }
                    if let Some(node) = self.nodes.remove(&id) {
                        pending.extend(children(&node)?);
                    }
                }
            }
            if let Some(root) = update["root_id"].as_i64() {
                self.root = Some(root);
            }
            for node in update["nodes"].as_array().ok_or(())? {
                let id = node["id"].as_i64().ok_or(())?;
                self.nodes.insert(id, node.clone());
                if self.nodes.len() > MAX_NODES {
                    return Err(());
                }
            }
        }
        if self
            .nodes
            .values()
            .map(|node| node.to_string().len())
            .sum::<usize>()
            > MAX_BYTES
        {
            return Err(());
        }
        let Some(root) = self.root else {
            return Err(());
        };
        let mut pending = vec![root];
        let mut reachable = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !reachable.insert(id) {
                return Err(());
            }
            pending.extend(children(self.nodes.get(&id).ok_or(())?)?);
        }
        self.nodes.retain(|id, _| reachable.contains(id));
        self.available = true;
        Ok(())
    }

    pub(super) fn append(&self, builder: &mut A11ySubtreeBuilder<'_>) {
        if !self.available {
            return;
        }
        let generation = self.document.as_ref().map(|document| document.generation);
        let id_for = |builder: &A11ySubtreeBuilder<'_>, id| {
            builder.synthetic_node_id((generation, &self.tree_id, id))
        };
        let original = builder.parent_node().children().to_vec();
        for (id, value) in &self.nodes {
            let mut node = accessible_node(value);
            node.set_children(
                children(value)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|child| id_for(builder, child))
                    .collect::<Vec<_>>(),
            );
            builder.push_child(id_for(builder, *id), node);
        }
        let mut roots = original;
        if let Some(root) = self.root {
            roots.push(id_for(builder, root));
        }
        builder.parent_node().set_children(roots);
        if let Some(focus) = self.focus.filter(|id| self.nodes.contains_key(id)) {
            builder.set_active_descendant(id_for(builder, focus));
        }
    }

    pub(super) fn agent_snapshot(&self) -> Option<Value> {
        if !self.available {
            return None;
        }
        let root = self.root?;
        let mut truncated = false;
        let mut nodes = Vec::new();
        for (&id, value) in &self.nodes {
            if hidden_or_protected(value) {
                continue;
            }
            if nodes.len() >= paneflow_browser_protocol::MAX_AGENT_SNAPSHOT_NODES {
                truncated = true;
                break;
            }
            let attributes = value.get("attributes").and_then(Value::as_object);
            let mut node = serde_json::Map::new();
            node.insert("id".to_string(), Value::from(id));
            node.insert(
                "role".to_string(),
                value.get("role").cloned().unwrap_or(Value::Null),
            );
            for key in ["name", "description", "value"] {
                if let Some(text) = attributes
                    .and_then(|attributes| attributes.get(key))
                    .and_then(Value::as_str)
                {
                    let (text, was_truncated) = paneflow_browser_protocol::cap_text(text, 4096);
                    truncated |= was_truncated;
                    node.insert(key.to_string(), Value::String(text));
                }
            }
            if let Some(bounds) = bounds(value) {
                node.insert("bounds".to_string(), bounds);
            }
            let children = children(value)
                .unwrap_or_default()
                .into_iter()
                .filter(|child| {
                    self.nodes
                        .get(child)
                        .is_some_and(|node| !hidden_or_protected(node))
                })
                .map(Value::from)
                .collect::<Vec<_>>();
            if !children.is_empty() {
                node.insert("children".to_string(), Value::Array(children));
            }
            nodes.push(Value::Object(node));
        }
        let snapshot = serde_json::json!({
            "root": root,
            "nodes": nodes,
            "truncated": truncated,
        });
        if snapshot.to_string().len() > paneflow_browser_protocol::MAX_AGENT_SNAPSHOT_BYTES {
            return Some(serde_json::json!({
                "root": root,
                "nodes": [],
                "truncated": true,
            }));
        }
        Some(snapshot)
    }

    pub(super) fn agent_selection_at(
        &self,
        x: i32,
        y: i32,
    ) -> Option<crate::browser::agent::BrowserContextSelection> {
        if !self.available {
            return None;
        }
        let document = self.document.clone()?;
        self.nodes
            .iter()
            .filter(|(_, value)| !hidden_or_protected(value))
            .filter_map(|(&node_id, value)| {
                let [left, top, width, height] = bounds_array(&bounds(value)?)?;
                let right = left.saturating_add(width);
                let bottom = top.saturating_add(height);
                (x >= left && x <= right && y >= top && y <= bottom).then(|| {
                    let attributes = value.get("attributes").and_then(Value::as_object);
                    let role = value
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or("element");
                    let name = attributes
                        .and_then(|attributes| attributes.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let summary = if name.is_empty() {
                        role.to_string()
                    } else {
                        format!("{role}: {name}")
                    };
                    let summary = paneflow_browser_protocol::cap_text(&summary, 512).0;
                    let text = attributes
                        .and_then(|attributes| attributes.get("value"))
                        .and_then(Value::as_str)
                        .or_else(|| {
                            attributes
                                .and_then(|attributes| attributes.get("name"))
                                .and_then(Value::as_str)
                        })
                        .map(|text| paneflow_browser_protocol::cap_text(text, 8192).0)
                        .filter(|text| !text.is_empty());
                    crate::browser::agent::BrowserContextSelection {
                        document: document.clone(),
                        node_id,
                        url: None,
                        rect: [left, top, width, height],
                        summary,
                        text,
                    }
                })
            })
            .min_by_key(|selection| {
                i64::from(selection.rect[2].max(0)) * i64::from(selection.rect[3].max(0))
            })
    }

    pub(super) fn agent_selection_is_current(
        &self,
        selection: &crate::browser::agent::BrowserContextSelection,
    ) -> bool {
        self.document.as_ref() == Some(&selection.document)
            && self
                .nodes
                .get(&selection.node_id)
                .and_then(|node| {
                    if hidden_or_protected(node) {
                        None
                    } else {
                        bounds(node).and_then(|bounds| bounds_array(&bounds))
                    }
                })
                .is_some_and(|bounds| bounds == selection.rect)
    }
}

fn hidden_or_protected(value: &Value) -> bool {
    let attributes = value.get("attributes");
    value
        .get("role")
        .and_then(Value::as_str)
        .is_some_and(|role| role.eq_ignore_ascii_case("passwordField"))
        || attributes
            .and_then(|attributes| attributes.get("restriction"))
            .and_then(Value::as_str)
            .is_some_and(|restriction| restriction.eq_ignore_ascii_case("protected"))
        || attributes
            .and_then(|attributes| attributes.get("hidden"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || attributes
            .and_then(|attributes| attributes.get("invisible"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn bounds(value: &Value) -> Option<Value> {
    let value = value
        .get("location")?
        .get("bounds")
        .or_else(|| value.get("location"))?;
    bounds_array(value).map(|bounds| Value::Array(bounds.into_iter().map(Value::from).collect()))
}

fn bounds_array(value: &Value) -> Option<[i32; 4]> {
    let array = value.as_array()?;
    if array.len() != 4 {
        return None;
    }
    Some([
        i32::try_from(array[0].as_i64()?).ok()?,
        i32::try_from(array[1].as_i64()?).ok()?,
        i32::try_from(array[2].as_i64()?).ok()?,
        i32::try_from(array[3].as_i64()?).ok()?,
    ])
}

fn accessible_node(value: &Value) -> Node {
    let mut node = Node::new(role(value["role"].as_str().unwrap_or_default()));
    for (key, setter) in [
        ("name", Node::set_label as fn(&mut Node, String)),
        ("description", Node::set_description),
        ("value", Node::set_value),
    ] {
        if let Some(text) = value["attributes"][key].as_str() {
            setter(&mut node, text.to_owned());
        }
    }
    if matches!(value["role"].as_str(), Some("staticText" | "inlineTextBox")) {
        node.set_value(
            value["attributes"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        );
    }
    if value["role"] == "inlineTextBox" {
        let text = value["attributes"]["name"].as_str().unwrap_or_default();
        node.set_character_lengths(
            text.chars()
                .map(|ch| ch.len_utf8() as u8)
                .collect::<Vec<_>>(),
        );
    }
    if value["attributes"]["restriction"] == "disabled" {
        node.set_disabled();
    }
    if value["attributes"]["restriction"] == "readOnly" {
        node.set_read_only();
    }
    node
}

fn children(node: &Value) -> Result<Vec<i64>, ()> {
    match node.get("child_ids") {
        None => Ok(Vec::new()),
        Some(value) => value
            .as_array()
            .ok_or(())?
            .iter()
            .map(|id| id.as_i64().ok_or(()))
            .collect(),
    }
}

fn role(value: &str) -> Role {
    match value {
        "rootWebArea" | "webArea" => Role::RootWebArea,
        "button" | "toggleButton" => Role::Button,
        "link" => Role::Link,
        "textField" | "textFieldWithComboBox" => Role::TextInput,
        "checkBox" => Role::CheckBox,
        "radioButton" => Role::RadioButton,
        "heading" => Role::Heading,
        "image" => Role::Image,
        "list" => Role::List,
        "listItem" => Role::ListItem,
        "staticText" => Role::Label,
        "inlineTextBox" => Role::TextRun,
        "table" => Role::Table,
        "row" => Role::Row,
        "cell" => Role::Cell,
        "slider" => Role::Slider,
        _ => Role::GenericContainer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document() -> Document {
        serde_json::from_value(
            json!({"owner":{"workspace":"w", "session":"s"}, "browser":"b", "generation":1}),
        )
        .unwrap()
    }

    #[test]
    fn cef_text_nodes_supply_valid_utf8_text_run_metadata() {
        for role in ["staticText", "inlineTextBox"] {
            for (attributes, expected) in [(json!({"name":"a😀b"}), "a😀b"), (json!({}), "")] {
                let node = accessible_node(&json!({"role":role,"attributes":attributes}));
                assert_eq!(
                    node.role(),
                    if role == "inlineTextBox" {
                        Role::TextRun
                    } else {
                        Role::Label
                    }
                );
                if role == "inlineTextBox" {
                    assert_eq!(
                        node.character_lengths()
                            .iter()
                            .map(|x| *x as usize)
                            .sum::<usize>(),
                        expected.len()
                    );
                }
                assert_eq!(node.value(), Some(expected));
            }
        }
    }

    #[test]
    fn cef_focus_updates_follow_the_live_document() {
        let document = document();
        let mut tree = AccessibilityTree::default();
        tree.reset(Some(document.clone()));
        tree.apply(&document, "accessibility_tree", &json!({"ax_tree_id":"t","updates":[{"root_id":1,"tree_data":{"focus_id":2},"nodes":[{"id":1,"child_ids":[2]},{"id":2,"role":"button"}]}]}));
        assert_eq!(tree.focus, Some(2));
        tree.reset(None);
        assert_eq!(tree.focus, None);
    }

    #[test]
    fn accepts_native_transport_envelope_from_page_signal() {
        let document = document();
        let mut tree = AccessibilityTree::default();
        tree.reset(Some(document.clone()));
        let event = json!({"native":"accessibility_tree", "document":document, "value":{"ax_tree_id":"t","updates":[{"root_id":1,"nodes":[{"id":1,"role":"rootWebArea","child_ids":[2]},{"id":2,"role":"button","attributes":{"name":"Submit"}}]}]}});
        assert!(tree.apply(&document, "accessibility_tree", &event));
        assert!(tree.available);
        assert_eq!(tree.nodes[&2]["attributes"]["name"], "Submit");
        let mut stale = event.clone();
        stale["document"]["generation"] = json!(2);
        assert!(!tree.apply(&document, "accessibility_tree", &stale));
        assert!(tree.available);
        let unavailable = json!({"native":"accessibility_unavailable", "document":document, "reason":"tree_update_limit"});
        assert!(tree.apply(&document, "accessibility_unavailable", &unavailable));
        assert!(!tree.available);
    }

    #[test]
    fn rejects_missing_children_and_malformed_nodes() {
        let document = document();
        for nodes in [
            json!([{"id":1,"child_ids":[2]}]),
            json!([{"id":1,"child_ids":"invalid"}]),
            json!([{"role":"button"}]),
        ] {
            let mut tree = AccessibilityTree::default();
            tree.reset(Some(document.clone()));
            tree.apply(
                &document,
                "accessibility_tree",
                &json!({"ax_tree_id":"t","updates":[{"root_id":1,"nodes":nodes}]}),
            );
            assert!(!tree.available);
            assert!(tree.nodes.is_empty());
        }
    }

    #[test]
    fn lost_update_requires_a_new_root_and_reset_drops_old_document() {
        let document = document();
        let mut tree = AccessibilityTree::default();
        tree.reset(Some(document.clone()));
        let full = json!({"ax_tree_id":"t","updates":[{"root_id":1,"nodes":[{"id":1,"role":"rootWebArea"}]}]});
        tree.apply(&document, "accessibility_tree", &full);
        assert!(tree.available);
        assert_eq!(role("rootWebArea"), Role::RootWebArea);
        tree.apply(&document, "accessibility_unavailable", &Value::Null);
        tree.apply(
            &document,
            "accessibility_tree",
            &json!({"ax_tree_id":"t","updates":[{"nodes":[{"id":1}]}]}),
        );
        assert!(!tree.available);
        tree.apply(&document, "accessibility_tree", &full);
        assert!(tree.available);
        tree.reset(None);
        assert!(!tree.apply(&document, "accessibility_tree", &full));
        assert!(tree.nodes.is_empty());
    }

    #[test]
    fn rejects_malformed_update_container_after_valid_tree() {
        let document = document();
        let mut tree = AccessibilityTree::default();
        tree.reset(Some(document.clone()));
        tree.apply(
            &document,
            "accessibility_tree",
            &json!({"ax_tree_id":"t","updates":[{"root_id":1,"nodes":[{"id":1}]}]}),
        );
        assert!(tree.available);
        tree.apply(
            &document,
            "accessibility_tree",
            &json!({"ax_tree_id":"t","updates":"invalid"}),
        );
        assert!(!tree.available);
    }

    #[test]
    fn rejects_oversized_updates() {
        let document = document();
        let mut tree = AccessibilityTree::default();
        tree.reset(Some(document.clone()));
        tree.apply(&document, "accessibility_tree", &json!({"ax_tree_id":"t", "updates":[{"root_id":1,"nodes":[{"id":1,"attributes":{"name":"x".repeat(MAX_BYTES)}}]}]}));
        assert!(!tree.available);
        assert!(tree.nodes.is_empty());
    }

    #[test]
    fn rejects_stale_and_cyclic_updates() {
        let document = document();
        let mut tree = AccessibilityTree::default();
        tree.reset(Some(document.clone()));
        let valid = json!({"ax_tree_id":"t", "updates":[{"root_id":1,"nodes":[{"id":1,"role":"rootWebArea","child_ids":[2]},{"id":2,"role":"button","attributes":{"name":"Go"}}]}]});
        assert!(tree.apply(&document, "accessibility_tree", &valid));
        assert!(tree.available);
        let mut stale = document.clone();
        stale.generation = 2;
        assert!(!tree.apply(&stale, "accessibility_unavailable", &Value::Null));
        assert!(tree.available);
        tree.apply(
            &document,
            "accessibility_tree",
            &json!({"ax_tree_id":"t", "updates":[{"nodes":[{"id":2,"child_ids":[1]}]}]}),
        );
        assert!(!tree.available);
        assert!(tree.nodes.is_empty());
    }

    #[test]
    fn agent_selection_excludes_protected_nodes_and_invalidates_removed_nodes() {
        let document = document();
        let mut tree = AccessibilityTree::default();
        tree.reset(Some(document.clone()));
        let update = json!({
            "ax_tree_id": "t",
            "updates": [{
                "root_id": 1,
                "nodes": [
                    {"id": 1, "role": "rootWebArea", "child_ids": [2, 3, 4], "location": {"bounds": [0, 0, 100, 100]}},
                    {"id": 2, "role": "button", "attributes": {"name": "Continue", "value": "go"}, "location": {"bounds": [10, 10, 20, 20]}},
                    {"id": 3, "role": "passwordField", "attributes": {"name": "Secret"}, "location": {"bounds": [10, 10, 20, 20]}},
                    {"id": 4, "role": "button", "attributes": {"name": "Hidden", "hidden": true}, "location": {"bounds": [10, 10, 20, 20]}}
                ]
            }]
        });
        assert!(tree.apply(&document, "accessibility_tree", &update));
        let selection = tree.agent_selection_at(15, 15).expect("visible selection");
        assert_eq!(selection.node_id, 2);
        assert_eq!(selection.summary, "button: Continue");
        assert!(tree.agent_selection_is_current(&selection));

        assert!(tree.apply(
            &document,
            "accessibility_tree",
            &json!({"ax_tree_id":"t", "updates":[{"node_id_to_clear":2, "nodes":[]}]}),
        ));
        assert!(!tree.agent_selection_is_current(&selection));
    }
}
