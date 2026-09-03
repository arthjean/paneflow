pub(crate) fn language_icon_path(basename: &str) -> &'static str {
    let basename = basename.trim().to_ascii_lowercase();
    match basename.as_str() {
        "angular.json" => return "icons/languages/angular.svg",
        "dockerfile" | "containerfile" => return "icons/languages/docker.svg",
        "makefile" => return "icons/languages/makefile.svg",
        _ => {}
    }

    if matches!(
        basename.as_str(),
        name if name.ends_with(".native.js")
            || name.ends_with(".native.jsx")
            || name.ends_with(".native.ts")
            || name.ends_with(".native.tsx")
            || name.ends_with(".ios.js")
            || name.ends_with(".ios.jsx")
            || name.ends_with(".ios.ts")
            || name.ends_with(".ios.tsx")
            || name.ends_with(".android.js")
            || name.ends_with(".android.jsx")
            || name.ends_with(".android.ts")
            || name.ends_with(".android.tsx")
    ) {
        return "icons/languages/react-native.svg";
    }

    let Some(ext) = basename.rsplit('.').next().filter(|ext| *ext != basename) else {
        return "icons/languages/file.svg";
    };

    match ext {
        "css" => "icons/languages/css.svg",
        "go" => "icons/languages/go.svg",
        "apng" | "avif" | "bmp" | "gif" | "heic" | "heif" | "ico" | "jpe" | "jpeg" | "jpg"
        | "png" | "svg" | "tif" | "tiff" | "webp" => "icons/languages/image.svg",
        "json" => "icons/languages/json.svg",
        "jsx" | "tsx" => "icons/languages/react.svg",
        "log" => "icons/languages/log.svg",
        "markdown" | "md" | "mdx" => "icons/languages/markdown.svg",
        "py" | "pyi" | "pyw" => "icons/languages/python.svg",
        "rb" | "rake" => "icons/languages/ruby.svg",
        "rs" => "icons/languages/rust-small.svg",
        "swift" => "icons/languages/swift.svg",
        "txt" => "icons/languages/text.svg",
        "toml" => "icons/languages/toml.svg",
        "cts" | "mts" | "ts" => "icons/languages/typescript.svg",
        _ => "icons/languages/file.svg",
    }
}

#[cfg(test)]
mod tests {
    use super::language_icon_path;

    #[test]
    fn full_names_win_over_extensions() {
        assert_eq!(
            language_icon_path("Dockerfile"),
            "icons/languages/docker.svg"
        );
        assert_eq!(
            language_icon_path("angular.json"),
            "icons/languages/angular.svg"
        );
        assert_eq!(
            language_icon_path("Makefile"),
            "icons/languages/makefile.svg"
        );
    }

    #[test]
    fn platform_suffixes_win_over_the_base_extension() {
        assert_eq!(
            language_icon_path("Button.ios.tsx"),
            "icons/languages/react-native.svg"
        );
        assert_eq!(
            language_icon_path("Button.tsx"),
            "icons/languages/react.svg"
        );
    }

    #[test]
    fn dotfiles_and_unknown_names_fall_back() {
        assert_eq!(language_icon_path(".gitignore"), "icons/languages/file.svg");
        assert_eq!(language_icon_path("LICENSE"), "icons/languages/file.svg");
        assert_eq!(
            language_icon_path("main.rs"),
            "icons/languages/rust-small.svg"
        );
    }
}
