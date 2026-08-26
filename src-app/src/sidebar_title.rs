//! Normalization of CLI-written titles before they reach the sidebar.
//!
//! Coding agents bake status decoration into the OSC / session titles they
//! emit (a leading `●` while answering, a `✓` or a zero-width character once
//! done). Every title that ends up on a sidebar row goes through
//! [`clean_sidebar_title`] first, so a label is what the human wrote and
//! nothing else.

/// Longest title a sidebar row keeps; the rest is elided with `…`.
const MAX_SIDEBAR_TITLE_CHARS: usize = 240;

/// Strip leading decoration glyphs and invisible characters that CLI
/// agents (Claude Code, Codex, OpenCode, Pi, Amp) bake into their
/// session / OSC titles to indicate status. Without this:
/// - During response: "● Project overview" sits in the sidebar with
///   a literal dot in front of the label.
/// - After response: a completion glyph (`✓`, `⚡`, …) or a
///   zero-width character (`U+200B`, `U+FEFF`, …) takes its place
///   and shows as a phantom margin -- `trim()` doesn't strip these
///   because they aren't whitespace per the Unicode standard, yet
///   most fonts render them with non-zero advance width.
///
/// Implementation strategy: whitelist what *can* legitimately lead a
/// human-written title (letters, digits, common opening punctuation)
/// and strip everything else from the front in one pass. That covers
/// the entire CLI-status-decoration family in a future-proof way --
/// new spinner glyphs or completion icons get caught without code
/// changes. Trailing whitespace is also normalized.
///
/// Returns `None` when nothing meaningful remains after stripping
/// (the caller treats that the same as an empty title -- the row
/// keeps its previous label rather than flashing blank).
pub fn clean_sidebar_title(raw: &str) -> Option<String> {
    let normalized: String = raw
        .chars()
        .map(|c| {
            if is_title_invisible_or_control(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let stripped = normalized
        .trim_start_matches(|c: char| !is_title_meaningful_lead(c))
        .trim();
    if stripped.is_empty() {
        None
    } else {
        let collapsed = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
        Some(cap_sidebar_title(&collapsed))
    }
}

fn cap_sidebar_title(title: &str) -> String {
    let mut chars = title.chars();
    let mut capped: String = chars.by_ref().take(MAX_SIDEBAR_TITLE_CHARS).collect();
    if chars.next().is_some() {
        capped.push('…');
    }
    capped
}

fn is_title_invisible_or_control(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200B}'
                | '\u{200C}'
                | '\u{200D}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

/// Whitelist of characters that can legitimately *start* a sidebar
/// title written by a human. Everything else (CLI status
/// glyphs, emoji, zero-width characters, format/control codepoints)
/// is treated as decoration and stripped by [`clean_sidebar_title`].
fn is_title_meaningful_lead(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            // Quotes -- ASCII + Unicode opening forms
            '"' | '\'' | '`'
            | '\u{201C}' | '\u{201D}'  // "" curly double
            | '\u{2018}' | '\u{2019}'  // '' curly single
            | '\u{00AB}' | '\u{00BB}'  // « » guillemets
            // Opening brackets / parens
            | '(' | '[' | '{'
            // Common title leads (hashtag, mention, code identifier)
            | '#' | '@' | '_'
            // Path / namespace separators
            | '/' | '\\' | '~' | '.'
            // Math / numeric leads
            | '-' | '+' | '=' | '$'
            | '\u{2013}' | '\u{2014}'  // - -
            | '\u{2212}'               // − minus sign
            // Currency
            | '\u{00A3}' | '\u{00A5}' | '\u{20AC}' // £ ¥ €
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_titles_are_cleaned_and_bounded() {
        let long = "x".repeat(MAX_SIDEBAR_TITLE_CHARS + 8);
        let raw = format!("● hello\n\u{202E}world {long}");

        let cleaned = clean_sidebar_title(&raw).expect("title should survive cleanup");

        assert!(cleaned.starts_with("hello world "));
        assert!(!cleaned.contains('\n'));
        assert!(!cleaned.contains('\u{202E}'));
        assert_eq!(cleaned.chars().count(), MAX_SIDEBAR_TITLE_CHARS + 1);
        assert!(cleaned.ends_with('…'));
    }

    #[test]
    fn pure_decoration_leaves_nothing() {
        assert_eq!(clean_sidebar_title("●  \u{200B}"), None);
    }
}
