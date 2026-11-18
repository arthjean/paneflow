mod tables;

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use tables::{
    BUILT_IN_EXTENSIONS, BUILT_IN_NAMES, COMPLETE_EXTENSION_OVERRIDES, T3CODE_EXTENSIONS,
    T3CODE_NAMES, TOKENS,
};

pub(crate) const DEFAULT_TOKEN: &str = "default";
pub(crate) const FILE_ICON_ROOT: &str = "icons/files/";

const PANEFLOW_NAMES: &[(&str, &str)] = &[
    ("angular.json", "angular"),
    ("containerfile", "docker"),
    ("gnumakefile", "makefile"),
    ("makefile", "makefile"),
];

const PANEFLOW_EXTENSIONS: &[(&str, &str)] = &[
    ("android.js", "react-native"),
    ("android.jsx", "react-native"),
    ("android.ts", "react-native"),
    ("android.tsx", "react-native"),
    ("ios.js", "react-native"),
    ("ios.jsx", "react-native"),
    ("ios.ts", "react-native"),
    ("ios.tsx", "react-native"),
    ("log", "log"),
    ("native.js", "react-native"),
    ("native.jsx", "react-native"),
    ("native.ts", "react-native"),
    ("native.tsx", "react-native"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IconVariant {
    Light,
    Dark,
}

impl IconVariant {
    pub(crate) fn current() -> Self {
        Self::for_ui(crate::theme::ui_colors())
    }

    pub(crate) fn for_ui(ui: crate::theme::UiColors) -> Self {
        if ui.base.l > 0.5 {
            Self::Light
        } else {
            Self::Dark
        }
    }

    fn dir(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

static PATHS: LazyLock<HashMap<&'static str, [&'static str; 2]>> = LazyLock::new(|| {
    let available: HashSet<String> = crate::assets::Assets::iter()
        .filter(|path| path.starts_with(FILE_ICON_ROOT))
        .map(|path| path.into_owned())
        .collect();
    TOKENS
        .iter()
        .filter_map(|token| {
            let paths = [IconVariant::Light, IconVariant::Dark]
                .map(|variant| format!("{FILE_ICON_ROOT}{}/{token}.svg", variant.dir()));
            if paths.iter().any(|path| !available.contains(path)) {
                log::warn!("file icon {token} is missing a variant; the generic icon stands in");
                return None;
            }
            Some((*token, paths.map(|path| &*Box::leak(path.into_boxed_str()))))
        })
        .collect()
});

pub(crate) fn warm_file_icons() {
    LazyLock::force(&PATHS);
}

pub(crate) fn file_icon_path(basename: &str) -> &'static str {
    icon_path_for(file_icon_token(basename), IconVariant::current())
}

pub(crate) fn icon_path_for(token: &str, variant: IconVariant) -> &'static str {
    let slot = usize::from(variant == IconVariant::Dark);
    PATHS
        .get(token)
        .or_else(|| PATHS.get(DEFAULT_TOKEN))
        .map_or("icons/file-text.svg", |paths| paths[slot])
}

pub(crate) fn is_file_icon_path(path: &str) -> bool {
    path.starts_with(FILE_ICON_ROOT)
}

pub(crate) fn file_icon_token(basename: &str) -> &'static str {
    let name = basename.trim().to_lowercase();
    if name.is_empty() {
        return DEFAULT_TOKEN;
    }
    if let Some(token) = find(PANEFLOW_NAMES, &name).or_else(|| find(T3CODE_NAMES, &name)) {
        return token;
    }
    for extension in extension_candidates(&name) {
        if let Some(token) =
            find(PANEFLOW_EXTENSIONS, extension).or_else(|| find(T3CODE_EXTENSIONS, extension))
        {
            return token;
        }
    }
    if let Some(token) = find(BUILT_IN_NAMES, &name) {
        return token;
    }
    for extension in extension_candidates(&name) {
        if let Some(token) = find(COMPLETE_EXTENSION_OVERRIDES, extension)
            .or_else(|| find(BUILT_IN_EXTENSIONS, extension))
        {
            return token;
        }
    }
    DEFAULT_TOKEN
}

pub(crate) fn extension_candidates(lowered: &str) -> impl Iterator<Item = &str> {
    lowered
        .match_indices('.')
        .map(move |(index, _)| &lowered[index + 1..])
}

fn find(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table
        .binary_search_by(|(candidate, _)| (*candidate).cmp(key))
        .ok()
        .map(|index| table[index].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_is_sorted_so_lookups_can_bisect() {
        for table in [
            PANEFLOW_NAMES,
            PANEFLOW_EXTENSIONS,
            T3CODE_NAMES,
            T3CODE_EXTENSIONS,
            BUILT_IN_NAMES,
            BUILT_IN_EXTENSIONS,
            COMPLETE_EXTENSION_OVERRIDES,
        ] {
            assert!(
                table.windows(2).all(|pair| pair[0].0 < pair[1].0),
                "unsorted or duplicate key near {:?}",
                table.windows(2).find(|pair| pair[0].0 >= pair[1].0)
            );
        }
    }

    #[test]
    fn every_token_ships_a_light_and_a_dark_svg() {
        let referenced: HashSet<&str> = [
            PANEFLOW_NAMES,
            PANEFLOW_EXTENSIONS,
            T3CODE_NAMES,
            T3CODE_EXTENSIONS,
            BUILT_IN_NAMES,
            BUILT_IN_EXTENSIONS,
            COMPLETE_EXTENSION_OVERRIDES,
        ]
        .into_iter()
        .flatten()
        .map(|(_, token)| *token)
        .chain(TOKENS.iter().copied())
        .collect();
        for token in referenced {
            for variant in ["light", "dark"] {
                let path = format!("{FILE_ICON_ROOT}{variant}/{token}.svg");
                assert!(
                    crate::assets::Assets::get(&path).is_some(),
                    "token {token} has no {variant} SVG at {path}"
                );
            }
            assert!(PATHS.contains_key(token), "token {token} is not resolvable");
        }
    }

    #[test]
    fn both_icon_folders_hold_at_least_seventy_five_sixteen_pixel_svgs() {
        for variant in ["light", "dark"] {
            let prefix = format!("{FILE_ICON_ROOT}{variant}/");
            let files: Vec<String> = crate::assets::Assets::iter()
                .filter(|path| path.starts_with(&prefix) && path.ends_with(".svg"))
                .map(|path| path.into_owned())
                .collect();
            assert!(files.len() >= 75, "{variant} holds {} icons", files.len());
            for file in files {
                let data = crate::assets::Assets::get(&file).expect("listed asset");
                let text = String::from_utf8_lossy(&data.data);
                let root = &text[text.find("<svg").expect("an svg root")..];
                let root = &root[..root.find('>').expect("a closed root")];
                assert!(
                    root.contains("width=\"16\"") && root.contains("height=\"16\""),
                    "{file} is not 16 by 16"
                );
            }
        }
    }

    #[test]
    fn paneflow_icons_without_an_upstream_equivalent_stay() {
        assert_eq!(file_icon_token("angular.json"), "angular");
        assert_eq!(file_icon_token("Makefile"), "makefile");
        assert_eq!(file_icon_token("Button.ios.tsx"), "react-native");
        assert_eq!(file_icon_token("app.native.ts"), "react-native");
        assert_eq!(file_icon_token("server.log"), "log");
        assert_eq!(file_icon_token("Button.tsx"), "react");
    }

    #[test]
    fn exact_names_win_over_extensions() {
        assert_eq!(file_icon_token("Dockerfile"), "docker");
        assert_eq!(file_icon_token("vite.config.ts"), "vite");
        assert_eq!(file_icon_token("package.json"), "npm");
        assert_eq!(file_icon_token("tsconfig.json"), "typescript");
        assert_eq!(file_icon_token("README.md"), "markdown");
        assert_eq!(file_icon_token(".gitignore"), "git");
    }

    #[test]
    fn compound_extensions_are_tried_from_longest_to_shortest() {
        assert_eq!(
            extension_candidates("a.test.ts").collect::<Vec<_>>(),
            vec!["test.ts", "ts"]
        );
        assert_eq!(file_icon_token("a.test.ts"), "typescript");
        assert_eq!(file_icon_token("page.mdx.tsx"), "markdown");
        assert_eq!(file_icon_token(".env.local"), "text");
        assert_eq!(file_icon_token("archive.tar.gz"), "zip");
    }

    #[test]
    fn the_common_languages_never_fall_back_to_the_default_icon() {
        for (name, token) in [
            ("index.js", "javascript"),
            ("ci.yaml", "yml"),
            ("index.html", "html"),
            ("main.c", "c"),
            ("Main.java", "java"),
            ("build.sh", "bash"),
            ("main.rs", "rust"),
            ("report.PDF", "pdf"),
            ("clip.mp4", "video"),
        ] {
            assert_eq!(file_icon_token(name), token, "{name}");
        }
    }

    #[test]
    fn unknown_empty_and_dot_only_names_resolve_to_the_default_icon() {
        for name in ["LICENSE", "", "   ", ".", "....", "unknown.zzz", "noext"] {
            assert_eq!(file_icon_token(name), DEFAULT_TOKEN, "{name:?}");
        }
    }

    #[test]
    fn a_light_theme_takes_the_light_icons_and_a_dark_theme_the_dark_ones() {
        let light = crate::theme::app_theme_by_name("Paneflow Light").expect("bundled light theme");
        let dark = crate::theme::app_theme_by_name("Paneflow Dark").expect("bundled dark theme");
        assert_eq!(
            IconVariant::for_ui(crate::theme::ui_colors_with(&light)),
            IconVariant::Light
        );
        assert_eq!(
            IconVariant::for_ui(crate::theme::ui_colors_with(&dark)),
            IconVariant::Dark
        );
    }

    #[test]
    fn a_missing_token_resolves_to_the_generic_icon_of_the_variant() {
        assert_eq!(
            icon_path_for("no-such-token", IconVariant::Dark),
            icon_path_for(DEFAULT_TOKEN, IconVariant::Dark)
        );
        assert_eq!(
            icon_path_for("rust", IconVariant::Light),
            "icons/files/light/rust.svg"
        );
        assert_eq!(
            icon_path_for("rust", IconVariant::Dark),
            "icons/files/dark/rust.svg"
        );
    }
}
