use serde_json::{Value, json};

const NAMED: &[(&str, &str, i64, Option<&str>)] = &[
    ("Enter", "Enter", 13, Some("\r")),
    ("Tab", "Tab", 9, None),
    ("Escape", "Escape", 27, None),
    ("Backspace", "Backspace", 8, None),
    ("Delete", "Delete", 46, None),
    ("Space", "Space", 32, Some(" ")),
    ("ArrowUp", "ArrowUp", 38, None),
    ("ArrowDown", "ArrowDown", 40, None),
    ("ArrowLeft", "ArrowLeft", 37, None),
    ("ArrowRight", "ArrowRight", 39, None),
    ("Home", "Home", 36, None),
    ("End", "End", 35, None),
    ("PageUp", "PageUp", 33, None),
    ("PageDown", "PageDown", 34, None),
];

pub fn key_event(name: &str) -> Option<(Value, Value)> {
    let (key, code, virtual_code, text) = NAMED
        .iter()
        .find(|(key, ..)| key.eq_ignore_ascii_case(name) || (name == " " && *key == "Space"))
        .map(|(key, code, virtual_code, text)| {
            let key = if *key == "Space" { " " } else { key };
            (
                key.to_string(),
                code.to_string(),
                *virtual_code,
                text.map(str::to_string),
            )
        })
        .or_else(|| single_character(name))?;
    let mut down = json!({
        "type": if text.is_some() { "keyDown" } else { "rawKeyDown" },
        "key": key,
        "code": code,
        "windowsVirtualKeyCode": virtual_code,
    });
    if let Some(text) = text {
        down["text"] = json!(text);
        down["unmodifiedText"] = json!(text);
    }
    let up = json!({
        "type": "keyUp",
        "key": key,
        "code": code,
        "windowsVirtualKeyCode": virtual_code,
    });
    Some((down, up))
}

fn single_character(name: &str) -> Option<(String, String, i64, Option<String>)> {
    let mut chars = name.chars();
    let character = chars.next()?;
    if chars.next().is_some() || character.is_control() {
        return None;
    }
    let upper = character.to_ascii_uppercase();
    let (code, virtual_code) = if upper.is_ascii_alphabetic() {
        (format!("Key{upper}"), i64::from(upper as u8))
    } else if character.is_ascii_digit() {
        (format!("Digit{character}"), i64::from(character as u8))
    } else {
        (String::new(), 0)
    };
    Some((
        character.to_string(),
        code,
        virtual_code,
        Some(character.to_string()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_types_a_carriage_return_and_arrows_type_nothing() {
        let (down, up) = key_event("enter").unwrap();
        assert_eq!(down["type"], "keyDown");
        assert_eq!(down["text"], "\r");
        assert_eq!(down["windowsVirtualKeyCode"], 13);
        assert_eq!(up["type"], "keyUp");
        let (down, _) = key_event("ArrowDown").unwrap();
        assert_eq!(down["type"], "rawKeyDown");
        assert!(down.get("text").is_none());
    }

    #[test]
    fn a_single_character_is_a_key_and_an_unknown_name_is_not() {
        let (down, _) = key_event("a").unwrap();
        assert_eq!(down["code"], "KeyA");
        assert_eq!(down["text"], "a");
        assert!(key_event("Hyper").is_none());
    }
}
