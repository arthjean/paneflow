use std::path::Path;

use crate::tables::{LANGUAGE_KEYS, TOKEN_TYPES};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Language(u8);

impl Language {
    pub fn from_id(id: u8) -> Option<Self> {
        (id != 0 && LANGUAGE_KEYS.iter().any(|(_, known)| *known == id)).then_some(Self(id))
    }

    pub fn id(self) -> u8 {
        self.0
    }

    pub fn name(self) -> &'static str {
        LANGUAGE_KEYS
            .iter()
            .find(|(_, id)| *id == self.0)
            .map_or("plain", |(key, _)| *key)
    }

    pub fn all() -> impl Iterator<Item = Self> {
        let max = LANGUAGE_KEYS.iter().map(|(_, id)| *id).max().unwrap_or(0);
        (1..=max).filter_map(Self::from_id)
    }
}

pub fn language_for_key(key: &str) -> Option<Language> {
    let key = key.to_ascii_lowercase();
    LANGUAGE_KEYS
        .iter()
        .find(|(known, _)| *known == key)
        .and_then(|(_, id)| Language::from_id(*id))
}

pub fn language_for_path(path: &Path) -> Option<Language> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .and_then(language_for_key)
        .or_else(|| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(language_for_key)
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TokenId(u8);

impl TokenId {
    pub const NONE: Self = Self(0);

    pub(crate) fn from_raw(raw: u32) -> Self {
        u8::try_from(raw)
            .ok()
            .filter(|id| usize::from(*id) < TOKEN_TYPES.len())
            .map_or(Self::NONE, Self)
    }

    pub fn get(self) -> u8 {
        self.0
    }

    pub fn name(self) -> &'static str {
        TOKEN_TYPES
            .get(usize::from(self.0))
            .copied()
            .unwrap_or("none")
    }
}

pub fn token_types() -> &'static [&'static str] {
    TOKEN_TYPES
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_of(key: &str) -> Option<u8> {
        LANGUAGE_KEYS
            .iter()
            .find(|(known, _)| *known == key)
            .map(|(_, id)| *id)
    }

    #[test]
    fn a_path_resolves_by_extension_to_the_language_of_that_key() {
        for (path, key) in [
            ("foo.zig", "zig"),
            ("build.zig.zon", "zon"),
            ("main.kt", "kt"),
            ("init.lua", "lua"),
            ("query.sql", "sql"),
            ("src/Lib.RS", "rs"),
        ] {
            assert_eq!(
                language_for_path(Path::new(path)).map(Language::id),
                id_of(key),
                "{path}"
            );
        }
        assert_eq!(id_of("zig"), id_of("zon"));
        assert!(id_of("zig").is_some());
    }

    #[test]
    fn a_dockerfile_and_a_makefile_resolve_by_their_lowercased_file_name() {
        assert_eq!(
            language_for_path(Path::new("Dockerfile")).map(Language::id),
            Some(35)
        );
        assert_eq!(
            language_for_path(Path::new("deploy/Makefile")).map(Language::id),
            Some(86)
        );
        assert_eq!(
            Language::from_id(35).map(Language::name),
            Some("containerfile")
        );
    }

    #[test]
    fn an_unknown_extension_and_plain_text_resolve_to_no_language() {
        assert_eq!(language_for_path(Path::new("README.unknownext")), None);
        assert_eq!(language_for_path(Path::new("notes.txt")), None);
        assert_eq!(language_for_path(Path::new("LICENSE")), None);
        assert_eq!(Language::from_id(0), None);
    }

    #[test]
    fn every_lexer_id_from_1_to_152_is_reachable_from_a_key() {
        let reachable: Vec<u8> = Language::all().map(Language::id).collect();
        assert_eq!(reachable, (1..=152).collect::<Vec<u8>>());
        for language in Language::all() {
            assert_eq!(
                language_for_key(language.name()),
                Some(language),
                "{}",
                language.name()
            );
        }
    }

    #[test]
    fn token_ids_resolve_to_the_names_of_token_types_js_in_order() {
        assert_eq!(TOKEN_TYPES.first(), Some(&"none"));
        assert_eq!(TokenId::from_raw(17).name(), "keyword");
        assert_eq!(TokenId::from_raw(61).name(), "emphasis.strong.italic");
        assert_eq!(TokenId::from_raw(67).name(), "markup.strikethrough");
        assert_eq!(TokenId::from_raw(10_000), TokenId::NONE);
        assert_eq!(TOKEN_TYPES.len(), 75);
    }
}
