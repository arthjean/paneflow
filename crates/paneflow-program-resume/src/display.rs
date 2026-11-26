use std::fmt::Write as _;

const BIDI_CONTROLS: [char; 12] = [
    '\u{061C}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}',
    '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
];

pub fn escape_for_display(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if needs_escape(character) {
            let _ = write!(escaped, "\\u{{{:04X}}}", u32::from(character));
        } else {
            escaped.push(character);
        }
    }
    escaped
}

fn needs_escape(character: char) -> bool {
    character.is_control()
        || BIDI_CONTROLS.contains(&character)
        || matches!(character, '\u{2028}' | '\u{2029}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bidi_overrides_and_control_characters_are_shown_as_code_points() {
        assert_eq!(
            escape_for_display("a\u{202E}b\x1b[31mc\u{85}d"),
            "a\\u{202E}b\\u{001B}[31mc\\u{0085}d"
        );
    }

    #[test]
    fn ordinary_text_including_accents_and_spaces_is_unchanged() {
        assert_eq!(escape_for_display("naïve two words *"), "naïve two words *");
    }
}
