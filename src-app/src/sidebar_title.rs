//! Normalization of CLI-written titles before they reach the sidebar.
//!
//! Coding agents bake status decoration into the OSC / session titles they
//! emit (a leading `●` while answering, a `✓` or a zero-width character once
//! done). Every title that ends up on a sidebar row goes through
//! [`clean_sidebar_title`] first, so a label is what the human wrote and
//! nothing else.

/// Longest title a sidebar row keeps; the rest is elided with `…`.
const MAX_SIDEBAR_TITLE_CHARS: usize = 240;

/// Words a tab title keeps from the prompt that opened the session.
///
/// Enough to tell "fix the flaky worktree test" from "add the release
/// checksum job", short enough that a rail full of them stays scannable.
const PROMPT_TITLE_WORDS: usize = 6;

/// And a character ceiling for those words, since six of them can still be
/// six identifiers. Well under [`MAX_SIDEBAR_TITLE_CHARS`]: this one is about
/// what a person reads at a glance, not about bounding a buffer.
const PROMPT_TITLE_MAX_CHARS: usize = 48;

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

/// A tab title made from the first prompt of a session.
///
/// The opening words of what was asked, which is what tells one agent tab
/// from another - the alternative being a rail of identical "Claude Code"
/// rows. Deliberately not a summary: no model runs here, so naming costs
/// nothing and cannot be slow, wrong, or unavailable.
///
/// `prompt` is UNTRUSTED text from the agent's hook payload. It goes through
/// the same [`clean_sidebar_title`] every other CLI-written label does, which
/// neutralizes control and bidi characters, folds the newlines a pasted
/// prompt carries, and strips leading decoration.
///
/// Returns `None` when nothing usable remains - the caller leaves the tab's
/// current title alone rather than blanking it.
pub fn tab_title_from_prompt(prompt: &str) -> Option<String> {
    let cleaned = clean_sidebar_title(prompt)?;
    let mut title = String::new();
    for word in cleaned.split_whitespace().take(PROMPT_TITLE_WORDS) {
        // Keep whole words: a title cut mid-identifier reads as corruption
        // rather than as an abbreviation. The first word is kept whatever its
        // length, so a single very long one still names the tab.
        if !title.is_empty()
            && title.chars().count() + 1 + word.chars().count() > PROMPT_TITLE_MAX_CHARS
        {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
    }
    if title.is_empty() {
        return None;
    }
    if title.chars().count() > PROMPT_TITLE_MAX_CHARS {
        title = title.chars().take(PROMPT_TITLE_MAX_CHARS).collect();
        title.push('…');
    }
    Some(title)
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

    #[test]
    fn a_prompt_title_keeps_the_opening_words() {
        assert_eq!(
            tab_title_from_prompt("fix the flaky worktree test").as_deref(),
            Some("fix the flaky worktree test")
        );
        assert_eq!(
            tab_title_from_prompt(
                "rewrite the session restore path so a legacy snapshot keeps its titles"
            )
            .as_deref(),
            Some("rewrite the session restore path so"),
            "six words, and no seventh"
        );
    }

    /// A pasted prompt arrives with newlines and indentation; a tab row is one
    /// line.
    #[test]
    fn a_multiline_prompt_becomes_one_line() {
        let title = tab_title_from_prompt("update the changelog\n\n  - add EP-004\n  - bump")
            .expect("a usable title");
        assert_eq!(title, "update the changelog - add EP-004");
        assert!(!title.contains('\n'));
    }

    /// Six words can still be six identifiers, and a rail is not a paragraph.
    #[test]
    fn a_prompt_title_stays_within_its_character_budget() {
        let title = tab_title_from_prompt(
            "refactor TerminalSessionBackend AbstractSyntaxTreeVisitor ConfigurationLoader now",
        )
        .expect("a usable title");
        assert!(
            title.chars().count() <= PROMPT_TITLE_MAX_CHARS + 1,
            "{title:?} overruns the budget"
        );
        assert!(
            !title.contains("ConfigurationLoader"),
            "{title:?} should have stopped at a word boundary"
        );
    }

    /// One word longer than the whole budget still names the tab - refusing
    /// would leave the row on its "Tab 3" fallback for no good reason.
    #[test]
    fn a_single_overlong_word_is_elided_rather_than_dropped() {
        let word = "x".repeat(PROMPT_TITLE_MAX_CHARS + 20);
        let title = tab_title_from_prompt(&word).expect("a usable title");
        assert_eq!(title.chars().count(), PROMPT_TITLE_MAX_CHARS + 1);
        assert!(title.ends_with('…'));
    }

    /// The prompt is untrusted text on its way to a label: the same
    /// neutralization every other CLI-written title gets.
    #[test]
    fn a_prompt_title_is_sanitized_like_any_other_label() {
        let title =
            tab_title_from_prompt("● \u{202E}drop the table\u{200B} now").expect("a usable title");
        assert_eq!(title, "drop the table now");
    }

    #[test]
    fn a_prompt_with_nothing_usable_names_nothing() {
        assert_eq!(tab_title_from_prompt(""), None);
        assert_eq!(tab_title_from_prompt("   \n\t "), None);
        assert_eq!(tab_title_from_prompt("●  \u{200B}"), None);
    }
}
