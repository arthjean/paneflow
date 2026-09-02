const MAX_SIDEBAR_TITLE_CHARS: usize = 240;

const PROMPT_TITLE_WORDS: usize = 6;

const PROMPT_TITLE_MAX_CHARS: usize = 48;

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

fn is_title_meaningful_lead(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '"' | '\''
                | '`'
                | '\u{201C}'
                | '\u{201D}'
                | '\u{2018}'
                | '\u{2019}'
                | '\u{00AB}'
                | '\u{00BB}'
                | '('
                | '['
                | '{'
                | '#'
                | '@'
                | '_'
                | '/'
                | '\\'
                | '~'
                | '.'
                | '-'
                | '+'
                | '='
                | '$'
                | '\u{2013}'
                | '\u{2014}'
                | '\u{2212}'
                | '\u{00A3}'
                | '\u{00A5}'
                | '\u{20AC}'
        )
}

pub fn tab_title_from_prompt(prompt: &str) -> Option<String> {
    let cleaned = clean_sidebar_title(prompt)?;
    let mut title = String::new();
    for word in cleaned.split_whitespace().take(PROMPT_TITLE_WORDS) {
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

    #[test]
    fn a_multiline_prompt_becomes_one_line() {
        let title = tab_title_from_prompt("update the changelog\n\n  - add EP-004\n  - bump")
            .expect("a usable title");
        assert_eq!(title, "update the changelog - add EP-004");
        assert!(!title.contains('\n'));
    }

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

    #[test]
    fn a_single_overlong_word_is_elided_rather_than_dropped() {
        let word = "x".repeat(PROMPT_TITLE_MAX_CHARS + 20);
        let title = tab_title_from_prompt(&word).expect("a usable title");
        assert_eq!(title.chars().count(), PROMPT_TITLE_MAX_CHARS + 1);
        assert!(title.ends_with('…'));
    }

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
