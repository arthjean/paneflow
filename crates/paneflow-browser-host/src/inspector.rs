use std::collections::HashSet;

use paneflow_browser_proto::TabId;
use serde_json::{Map, Value};

pub const FRONTEND_URL: &str =
    "devtools://devtools/bundled/devtools_app.html?ws=paneflow.invalid/inspector";
pub const FRONTEND_LOCALE: &str = "en-US";
pub const BINDING: &str = "__paneflowInspectorSend";
pub const SHIM: &str = include_str!("inspector_shim.js");

pub fn is_frontend_page(url: &str) -> bool {
    url.starts_with("devtools://devtools/")
}

pub struct Bridge {
    pub inspected: TabId,
    session: Option<String>,
    children: HashSet<String>,
    queued: Vec<Map<String, Value>>,
}

impl Bridge {
    pub fn new(inspected: TabId) -> Self {
        Self {
            inspected,
            session: None,
            children: HashSet::new(),
            queued: Vec::new(),
        }
    }

    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    pub fn attached(&mut self, session: String) -> Vec<Vec<u8>> {
        self.session = Some(session);
        std::mem::take(&mut self.queued)
            .into_iter()
            .filter_map(|message| self.addressed(message))
            .collect()
    }

    pub fn outgoing(&mut self, payload: &str) -> Option<Vec<u8>> {
        let Ok(Value::Object(message)) = serde_json::from_str(payload) else {
            return None;
        };
        if message.get("id").and_then(Value::as_i64).is_none()
            || message.get("method").and_then(Value::as_str).is_none()
        {
            return None;
        }
        if self.session.is_none() && !message.contains_key("sessionId") {
            self.queued.push(message);
            return None;
        }
        self.addressed(message)
    }

    fn addressed(&self, mut message: Map<String, Value>) -> Option<Vec<u8>> {
        match message.get("sessionId").and_then(Value::as_str) {
            Some(child) if self.children.contains(child) => {}
            Some(_) => return None,
            None => {
                message.insert("sessionId".to_string(), Value::from(self.session.clone()?));
            }
        }
        serde_json::to_vec(&Value::Object(message)).ok()
    }

    pub fn incoming(&mut self, message: &[u8]) -> Option<String> {
        let Ok(Value::Object(mut message)) = serde_json::from_slice(message) else {
            return None;
        };
        let session = message
            .get("sessionId")
            .and_then(Value::as_str)?
            .to_string();
        let ours = self.session.as_deref() == Some(session.as_str());
        if !ours && !self.children.contains(&session) {
            return None;
        }
        let child = message
            .get("params")
            .and_then(|params| params.get("sessionId"))
            .and_then(Value::as_str)
            .map(str::to_string);
        match (message.get("method").and_then(Value::as_str), child) {
            (Some("Target.attachedToTarget"), Some(child)) => {
                self.children.insert(child);
            }
            (Some("Target.detachedFromTarget"), Some(child)) => {
                self.children.remove(&child);
            }
            _ => {}
        }
        if ours {
            message.remove("sessionId");
        }
        let text = Value::Object(message).to_string();
        Some(format!(
            "window.__paneflowInspectorReceive({})",
            Value::from(text)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivered(script: &str) -> Value {
        let literal = script
            .strip_prefix("window.__paneflowInspectorReceive(")
            .and_then(|rest| rest.strip_suffix(')'))
            .unwrap();
        let text: String = serde_json::from_str(literal).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn the_frontend_talks_through_its_own_session_and_waits_for_it() {
        let mut bridge = Bridge::new(TabId(1));
        let early = r#"{"id":1,"method":"DOM.enable","params":{}}"#;
        assert_eq!(bridge.outgoing(early), None, "queued until attached");
        assert_eq!(bridge.outgoing("not json"), None);
        assert_eq!(bridge.outgoing(r#"{"method":"Page.enable"}"#), None);
        let flushed = bridge.attached("S1".to_string());
        assert_eq!(flushed.len(), 1);
        let sent: Value = serde_json::from_slice(&flushed[0]).unwrap();
        assert_eq!(sent["sessionId"], "S1");
        assert_eq!(sent["method"], "DOM.enable");
        let later = bridge
            .outgoing(r#"{"id":2,"method":"Runtime.evaluate","params":{"expression":"1"}}"#)
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&later).unwrap()["sessionId"],
            "S1"
        );
    }

    #[test]
    fn answers_reach_the_frontend_without_its_session_and_child_sessions_pass_through() {
        let mut bridge = Bridge::new(TabId(1));
        bridge.attached("S1".to_string());
        assert_eq!(
            bridge.incoming(br#"{"id":3,"result":{}}"#),
            None,
            "the window's own answers stay out"
        );
        assert_eq!(
            bridge.incoming(br#"{"id":3,"result":{},"sessionId":"OTHER"}"#),
            None
        );
        let answer = delivered(
            &bridge
                .incoming(br#"{"id":3,"result":{"ok":true},"sessionId":"S1"}"#)
                .unwrap(),
        );
        assert_eq!(answer["result"]["ok"], true);
        assert!(answer.get("sessionId").is_none());
        let attached =
            br#"{"method":"Target.attachedToTarget","params":{"sessionId":"C1"},"sessionId":"S1"}"#;
        bridge.incoming(attached).unwrap();
        let child = delivered(
            &bridge
                .incoming(br#"{"method":"Runtime.consoleAPICalled","params":{},"sessionId":"C1"}"#)
                .unwrap(),
        );
        assert_eq!(child["sessionId"], "C1");
        let to_child = bridge
            .outgoing(r#"{"id":4,"method":"Runtime.enable","sessionId":"C1"}"#)
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&to_child).unwrap()["sessionId"],
            "C1"
        );
        assert_eq!(
            bridge.outgoing(r#"{"id":5,"method":"Runtime.enable","sessionId":"FORGED"}"#),
            None
        );
    }

    #[test]
    fn page_text_cannot_break_out_of_the_delivery_call() {
        let mut bridge = Bridge::new(TabId(1));
        bridge.attached("S1".to_string());
        let hostile = br#"{"method":"Runtime.consoleAPICalled","params":{"args":[{"value":"\u2028');alert(1)//</script>"}]},"sessionId":"S1"}"#;
        let script = bridge.incoming(hostile).unwrap();
        let back = delivered(&script);
        assert_eq!(
            back["params"]["args"][0]["value"],
            "\u{2028}');alert(1)//</script>"
        );
    }
}
