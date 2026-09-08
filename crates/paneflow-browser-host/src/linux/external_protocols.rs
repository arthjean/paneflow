use std::{cell::RefCell, collections::BTreeMap};

use paneflow_browser_protocol::{Document, InputEvent};
use serde_json::json;

struct Pending {
    document: Document,
    url: String,
}

struct State {
    next: u64,
    pending: BTreeMap<u64, Pending>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            next: 1 << 62,
            pending: BTreeMap::new(),
        }
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn scheme(url: &str) -> Option<&str> {
    if url.is_empty()
        || url.len() > 8192
        || url.chars().any(|character| {
            character.is_control() || character.is_whitespace() || character == '\\'
        })
    {
        return None;
    }
    let (scheme, remainder) = url.split_once(':')?;
    if remainder.is_empty() || !matches!(scheme, "mailto" | "tel" | "sms" | "magnet") {
        return None;
    }
    if let Some(authority) = remainder.strip_prefix("//") {
        let authority = authority.split(['/', '?', '#']).next()?;
        if authority.contains('@') || authority.contains('%') {
            return None;
        }
    }
    let bytes = url.as_bytes();
    for index in 0..bytes.len() {
        if bytes[index] != b'%' {
            continue;
        }
        let high = *bytes.get(index + 1)?;
        let low = *bytes.get(index + 2)?;
        let decoded = (char::from(high).to_digit(16)? * 16 + char::from(low).to_digit(16)?) as u8;
        if decoded.is_ascii_control() || decoded == b'\\' {
            return None;
        }
    }
    Some(scheme)
}

pub(super) fn request(document: &Document, origin: &str, url: &str) -> bool {
    let Some(scheme) = scheme(url) else {
        return false;
    };
    let Some(document) = super::latest_document(document).filter(|latest| latest == document)
    else {
        return false;
    };
    let request = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.pending.len() >= 16
            || state
                .pending
                .values()
                .any(|pending| pending.document == document)
            || state.next >= (1 << 63)
        {
            return None;
        }
        let request = state.next;
        state.next += 1;
        state.pending.insert(
            request,
            Pending {
                document: document.clone(),
                url: url.to_owned(),
            },
        );
        Some(request)
    });
    let Some(request) = request else {
        return false;
    };
    let origin = origin
        .chars()
        .filter(|character| !character.is_control())
        .take(2048)
        .collect::<String>();
    super::emit(
        json!({"native":"web_dialog", "document":document, "request":request, "kind":"external_protocol", "origin":origin, "message":format!("Ouvrir le protocole {scheme} dans une application externe ?\n{url}"), "default_text":""}),
    );
    true
}

pub(super) fn handle(document: &Document, input: &InputEvent) {
    let InputEvent::WebResponse {
        request, accept, ..
    } = input
    else {
        return;
    };
    let pending = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state
            .pending
            .get(request)
            .is_some_and(|pending| &pending.document == document)
        {
            state.pending.remove(request)
        } else {
            None
        }
    });
    if let Some(pending) =
        pending.filter(|_| *accept && super::latest_document(document).as_ref() == Some(document))
    {
        super::emit(json!({"native":"external_open", "document":document, "url":pending.url}));
    }
}

pub(super) fn clear(document: &Document) {
    STATE.with(|state| {
        state.borrow_mut().pending.retain(|_, pending| {
            pending.document.browser != document.browser || pending.document.owner != document.owner
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_only_explicit_non_privileged_schemes() {
        for url in [
            "mailto:user@example.com",
            "tel:+33123456789",
            "sms:+33123456789",
            "magnet:?xt=urn:btih:abcd",
        ] {
            assert!(scheme(url).is_some(), "{url}");
        }
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/plain,x",
            "chrome:settings",
            "devtools:inspect",
            "command:run",
            "paneflow:run",
            "unknown:action",
            "https://example.com",
            "mailto:",
        ] {
            assert!(scheme(url).is_none(), "{url}");
        }
    }

    #[test]
    fn rejects_userinfo_controls_and_oversized_urls() {
        for url in [
            "mailto://user@host/path",
            "tel://user%40host/path",
            "mailto:a\nb",
            "mailto:a%0db",
            "mailto:a%7fb",
            "mailto:a%00b",
            "mailto:a%zz",
            "mailto:a\\b",
            "mailto:a b",
        ] {
            assert!(scheme(url).is_none(), "{url}");
        }
        assert!(scheme(&format!("mailto:{}", "a".repeat(8192))).is_none());
    }
}
