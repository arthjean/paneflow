use paneflow_highlights::{Language, language_for_key};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FirstFrame {
    Lexical,
    Plain,
}

pub(crate) type FirstFramePolicy = &'static [(&'static str, FirstFrame)];

pub(crate) const FIRST_FRAME_POLICY: FirstFramePolicy = &[
    ("bash", FirstFrame::Lexical),
    ("c", FirstFrame::Lexical),
    ("cpp", FirstFrame::Lexical),
    ("css", FirstFrame::Lexical),
    ("go", FirstFrame::Lexical),
    ("html", FirstFrame::Lexical),
    ("java", FirstFrame::Lexical),
    ("js", FirstFrame::Lexical),
    ("json", FirstFrame::Lexical),
    ("markdown", FirstFrame::Lexical),
    ("python", FirstFrame::Lexical),
    ("ruby", FirstFrame::Lexical),
    ("rust", FirstFrame::Lexical),
    ("toml", FirstFrame::Lexical),
    ("tsx", FirstFrame::Lexical),
    ("typescript", FirstFrame::Lexical),
    ("yaml", FirstFrame::Lexical),
    ("zig", FirstFrame::Lexical),
];

pub(crate) fn first_frame_in(policy: FirstFramePolicy, language: Language) -> FirstFrame {
    policy
        .iter()
        .find(|(name, _)| language_for_key(name) == Some(language))
        .map_or(FirstFrame::Lexical, |(_, first_frame)| *first_frame)
}

#[cfg(test)]
thread_local! {
    static OVERRIDE: std::cell::Cell<Option<FirstFramePolicy>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) struct PolicyOverride(Option<FirstFramePolicy>);

#[cfg(test)]
impl Drop for PolicyOverride {
    fn drop(&mut self) {
        OVERRIDE.with(|cell| cell.set(self.0));
    }
}

#[cfg(test)]
pub(crate) fn override_policy_for_test(policy: FirstFramePolicy) -> PolicyOverride {
    PolicyOverride(OVERRIDE.with(|cell| cell.replace(Some(policy))))
}

fn active_policy() -> FirstFramePolicy {
    #[cfg(test)]
    if let Some(policy) = OVERRIDE.with(std::cell::Cell::get) {
        return policy;
    }
    FIRST_FRAME_POLICY
}

pub(crate) fn first_frame_for(language: Language) -> FirstFrame {
    first_frame_in(active_policy(), language)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use paneflow_highlights::language_for_path;

    use super::*;

    #[test]
    fn every_policy_entry_names_a_distinct_lexer_language() {
        let mut seen = BTreeSet::new();
        for (name, _) in FIRST_FRAME_POLICY {
            let language =
                language_for_key(name).unwrap_or_else(|| panic!("{name} is not a lexer language"));
            assert!(seen.insert(language), "{name} repeats a language");
        }
    }

    #[test]
    fn every_language_paired_with_a_grammar_has_an_entry() {
        for (sample, _) in crate::diff::parity_tests::CORPUS {
            let language = language_for_path(Path::new(sample)).expect("a lexer language");
            assert!(
                FIRST_FRAME_POLICY
                    .iter()
                    .any(|(name, _)| language_for_key(name) == Some(language)),
                "{sample} ({}) has no first-frame entry",
                language.name()
            );
        }
    }

    #[test]
    fn every_entry_is_lexical_until_arthur_chooses_otherwise() {
        for (name, first_frame) in FIRST_FRAME_POLICY {
            assert_eq!(*first_frame, FirstFrame::Lexical, "{name}");
        }
    }

    #[test]
    fn a_language_absent_from_the_table_is_lexical() {
        let kotlin = language_for_key("kotlin").expect("Pierre lexes Kotlin");
        assert!(
            !FIRST_FRAME_POLICY
                .iter()
                .any(|(name, _)| language_for_key(name) == Some(kotlin))
        );
        assert_eq!(
            first_frame_in(FIRST_FRAME_POLICY, kotlin),
            FirstFrame::Lexical
        );
        assert_eq!(first_frame_in(&[], kotlin), FirstFrame::Lexical);
        let rust = language_for_key("rust").expect("Pierre lexes Rust");
        assert_eq!(
            first_frame_in(&[("rs", FirstFrame::Plain)], rust),
            FirstFrame::Plain
        );
    }
}
