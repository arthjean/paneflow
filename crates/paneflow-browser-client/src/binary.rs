use std::ffi::OsString;
use std::path::{Path, PathBuf};

use paneflow_browser_proto::HOST_BINARY_NAME;

pub const HOST_BINARY_ENV: &str = "PANEFLOW_BROWSER_HOST";

pub const CURRENT_RUNTIME_DIR_NAME: &str = "current";

fn executable_name() -> String {
    format!("{HOST_BINARY_NAME}{}", std::env::consts::EXE_SUFFIX)
}

pub fn dev_build_output() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(HOST_BINARY_NAME)
        .join("target")
        .join("debug")
        .join(executable_name())
}

pub fn installed_runtime_binary(home: &Path) -> PathBuf {
    paneflow_home::browser_runtime_dir_in(home)
        .join(CURRENT_RUNTIME_DIR_NAME)
        .join(executable_name())
}

pub fn resolve_host_binary(home: &Path) -> PathBuf {
    resolve_host_binary_with(
        home,
        std::env::var_os(HOST_BINARY_ENV),
        cfg!(debug_assertions),
    )
}

pub fn resolve_host_binary_with(
    home: &Path,
    override_path: Option<OsString>,
    debug_build: bool,
) -> PathBuf {
    if debug_build {
        return override_path
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(dev_build_output);
    }
    installed_runtime_binary(home)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_debug_build_takes_the_override_first() {
        let home = Path::new("/home/u/.paneflow-dev");
        let resolved = resolve_host_binary_with(home, Some(OsString::from("/opt/host/bin")), true);
        assert_eq!(resolved, PathBuf::from("/opt/host/bin"));
    }

    #[test]
    fn a_debug_build_without_override_uses_the_host_crate_build_output() {
        let home = Path::new("/home/u/.paneflow-dev");
        for unset in [None, Some(OsString::new())] {
            let resolved = resolve_host_binary_with(home, unset, true);
            assert_eq!(resolved, dev_build_output());
            assert!(resolved.ends_with(
                Path::new("paneflow-browser-host/target/debug").join(executable_name())
            ));
            assert!(!resolved.starts_with(home));
        }
    }

    #[test]
    fn a_debug_build_never_resolves_into_an_installed_home() {
        for home in ["/home/u/.paneflow", "/home/u/.paneflow-dev"] {
            let resolved = resolve_host_binary_with(Path::new(home), None, true);
            assert!(!resolved.starts_with(home), "{}", resolved.display());
        }
    }

    #[test]
    fn a_release_build_uses_the_installed_runtime_and_ignores_the_override() {
        let home = Path::new("/home/u/.paneflow");
        let resolved =
            resolve_host_binary_with(home, Some(OsString::from("/tmp/evil-host")), false);
        assert_eq!(
            resolved,
            home.join("browser/runtime/current").join(executable_name())
        );
    }
}
