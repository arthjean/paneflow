use cef::*;
use paneflow_browser_protocol::Document;
use serde_json::json;

use super::emit;

const MAX_UPDATE_BYTES: usize = 240 * 1024;

wrap_accessibility_handler! {
    pub struct Accessibility {
        document: Document,
    }

    impl AccessibilityHandler {
        fn on_accessibility_tree_change(&self, value: Option<&mut Value>) {
            forward(&self.document, "accessibility_tree", value);
        }

        fn on_accessibility_location_change(&self, value: Option<&mut Value>) {
            forward(&self.document, "accessibility_location", value);
        }
    }
}

fn forward(document: &Document, kind: &str, value: Option<&mut Value>) {
    let Some(document) = super::latest_document(document) else {
        return;
    };
    let serialized = CefString::from(&write_json(value, JsonWriterOptions::default())).to_string();
    if serialized.len() >= MAX_UPDATE_BYTES {
        emit(
            json!({"native": "accessibility_unavailable", "document": document, "reason": "tree_update_limit"}),
        );
        return;
    }
    match serde_json::from_str::<serde_json::Value>(&serialized) {
        Ok(value) => {
            let event = json!({"native": kind, "document": document, "value": value});
            if event.to_string().len() < MAX_UPDATE_BYTES {
                emit(event);
            } else {
                emit(
                    json!({"native": "accessibility_unavailable", "document": document, "reason": "tree_update_limit"}),
                );
            }
        }
        Err(_) => emit(
            json!({"native": "accessibility_unavailable", "document": document, "reason": "invalid_tree_update"}),
        ),
    }
}
