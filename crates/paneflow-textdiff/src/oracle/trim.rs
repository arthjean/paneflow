use crate::text::{is_alpha, is_continuous_script, is_punctuation};

use super::{CH_MAN, CH_SMILE};

const PUNCTUATION: &str = "(){}[],./?`~!@#$%^&*-=+|\\;:'\"<>";

#[test]
fn punctuation() {
    for code in 0..=0xFFFFu32 {
        let Some(c) = char::from_u32(code) else {
            continue;
        };
        assert_eq!(PUNCTUATION.contains(c), is_punctuation(c), "'{c}' - {code}");
    }
}

#[test]
fn continuous_script() {
    assert_is_alpha("!? \t\n+~", false);
    assert_is_alpha("\r", false);

    assert_is_alpha("AB12", true);
    assert_is_alpha("АБВ汉语日ひรไ", true);
    assert_is_alpha("óèäñĀ", true);
    assert_is_alpha("_\u{0001}", true);
    assert_is_alpha(&format!("{CH_SMILE}{CH_MAN}"), true);

    assert_is_continuous("12_ABZ", false);
    assert_is_continuous("АБВ", false);
    assert_is_continuous("ʁit", false);
    assert_is_continuous("음훈", false);
    assert_is_continuous("óèäñĀ", false);
    assert_is_continuous("\r_\u{0001}", false);

    assert_is_continuous("象形文字", true);
    assert_is_continuous("อักษรไ", true);
    assert_is_continuous("ひらがなカタカナ日本語", true);
    assert_is_continuous("汉语漢語", true);
    assert_is_continuous("☺♥", true);
    assert_is_continuous(&format!("{CH_SMILE}{CH_MAN}"), true);
    assert_is_continuous("\u{200e}\u{200f}\u{061c}", true);
}

fn assert_is_alpha(text: &str, expected: bool) {
    for c in text.chars() {
        assert_eq!(expected, is_alpha(c), "{} - {c:?}", c as u32);
    }
}

fn assert_is_continuous(text: &str, expected: bool) {
    for c in text.chars() {
        assert_eq!(expected, is_continuous_script(c), "{} - {c:?}", c as u32);
    }
}
