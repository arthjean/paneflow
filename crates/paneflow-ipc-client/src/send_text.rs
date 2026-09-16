pub const BRACKETED_PASTE_START: &str = "\x1b[200~";

pub const BRACKETED_PASTE_END: &str = "\x1b[201~";

pub fn resolve_paste_mode(
    paste_param: Option<bool>,
    submit: bool,
    is_agent: bool,
    bracketed_paste_enabled: bool,
) -> bool {
    paste_param.unwrap_or(submit && (is_agent || bracketed_paste_enabled))
}

pub fn text_contains_submit_byte(text: &str) -> bool {
    text.contains('\r') || text.contains('\n')
}

pub fn resolve_send_text_body_mode(
    text: &str,
    paste_param: Option<bool>,
    resolved_paste: bool,
    bracketed_paste_enabled: bool,
) -> Result<bool, &'static str> {
    if !text_contains_submit_byte(text) {
        return Ok(resolved_paste);
    }

    let paste = if paste_param.is_none() && bracketed_paste_enabled {
        true
    } else {
        resolved_paste
    };

    if paste && bracketed_paste_enabled {
        Ok(paste)
    } else {
        Err("text contains CR or LF; multiline surface.send_text requires active bracketed paste")
    }
}

pub fn normalize_paste_text(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .chars()
        .filter(|&c| c != '\x1b' && !(('\u{0080}'..='\u{009f}').contains(&c)))
        .collect()
}

pub fn bracketed_paste_frame(text: &str) -> String {
    format!(
        "{BRACKETED_PASTE_START}{}{BRACKETED_PASTE_END}",
        normalize_paste_text(text)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_paste_mode_auto_targets_agents_or_bracketed_tuis() {
        assert!(resolve_paste_mode(None, true, true, false));
        assert!(resolve_paste_mode(None, true, false, true));
        assert!(!resolve_paste_mode(None, true, false, false));
        assert!(!resolve_paste_mode(None, false, true, true));
        assert!(!resolve_paste_mode(None, false, false, true));
        assert!(resolve_paste_mode(Some(true), false, false, false));
        assert!(!resolve_paste_mode(Some(false), true, true, true));
    }

    #[test]
    fn send_text_body_mode_rejects_crlf_without_active_bracketed_paste() {
        assert_eq!(
            resolve_send_text_body_mode("one line", None, false, false),
            Ok(false)
        );
        assert!(
            resolve_send_text_body_mode("line one\nline two", None, false, false).is_err(),
            "bare multiline writes can smuggle a submit"
        );
        assert!(
            resolve_send_text_body_mode("line one\rline two", Some(true), true, false).is_err(),
            "explicit paste is still unsafe until the terminal enabled bracketed paste"
        );
        assert_eq!(
            resolve_send_text_body_mode("line one\nline two", None, false, true),
            Ok(true),
            "an active bracketed paste mode upgrades an implicit multiline write to a paste"
        );
    }

    #[test]
    fn a_paste_frame_is_normalized_and_bracketed() {
        assert_eq!(
            bracketed_paste_frame("a\r\nb\x1b[201~c"),
            "\x1b[200~a\nb[201~c\x1b[201~"
        );
        assert_eq!(normalize_paste_text("hello world"), "hello world");
        assert_eq!(normalize_paste_text("a\x1b[201~b\u{0085}c"), "a[201~bc");
    }
}
