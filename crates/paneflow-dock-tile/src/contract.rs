use std::path::Path;

pub const APP_ICON_KEY: &str = "PaneflowAppIcon";
pub const PLUGIN_BUNDLE: &str = "PaneflowDockTile.docktileplugin";
const NOTIFICATION_SUFFIX: &str = ".app-icon-changed";
const ICON_NAME_MAX_BYTES: usize = 64;

pub fn notification_name(host_identifier: &str) -> String {
    format!("{host_identifier}{NOTIFICATION_SUFFIX}")
}

pub fn host_app(plugin: &Path) -> Option<&Path> {
    let plugins = plugin.parent()?;
    let contents = plugins.parent()?;
    let app = contents.parent()?;
    let laid_out = plugins.file_name()? == "PlugIns"
        && contents.file_name()? == "Contents"
        && app.extension()? == "app";
    laid_out.then_some(app)
}

pub fn bundle_of_executable(executable: &Path) -> Option<&Path> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let laid_out = macos.file_name()? == "MacOS" && contents.file_name()? == "Contents";
    laid_out.then_some(contents.parent()?)
}

pub fn is_icon_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= ICON_NAME_MAX_BYTES
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_app_is_three_folders_above_the_plugin() {
        let plugin = Path::new("/Applications/Paneflow.app/Contents/PlugIns").join(PLUGIN_BUNDLE);
        assert_eq!(
            host_app(&plugin),
            Some(Path::new("/Applications/Paneflow.app"))
        );
        assert_eq!(
            host_app(Path::new("/tmp/Contents/PlugIns/x.docktileplugin")),
            None
        );
        assert_eq!(
            host_app(Path::new("/A.app/Contents/Resources/x.docktileplugin")),
            None
        );
        assert_eq!(host_app(Path::new("x.docktileplugin")), None);
    }

    #[test]
    fn the_bundle_is_two_folders_above_its_executable() {
        let bundle = Path::new("/A.app/Contents/PlugIns").join(PLUGIN_BUNDLE);
        assert_eq!(
            bundle_of_executable(&bundle.join("Contents/MacOS/PaneflowDockTile")),
            Some(bundle.as_path())
        );
        assert_eq!(
            bundle_of_executable(Path::new("/target/release/examples/probe")),
            None
        );
        assert_eq!(bundle_of_executable(Path::new("probe")), None);
    }

    #[test]
    fn only_plain_asset_names_are_accepted() {
        for name in ["amethyst", "black-solid", "midnight-matte", "a1"] {
            assert!(is_icon_name(name), "{name}");
        }
        for name in [
            "",
            "../etc",
            "Black",
            "a/b",
            "chip.png",
            "-x",
            &"a".repeat(65),
        ] {
            assert!(!is_icon_name(name), "{name}");
        }
    }

    #[test]
    fn the_notification_belongs_to_the_host() {
        assert_eq!(
            notification_name("io.github.arthurdev44.paneflow"),
            "io.github.arthurdev44.paneflow.app-icon-changed"
        );
    }
}
