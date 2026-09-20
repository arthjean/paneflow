use std::path::PathBuf;

use paneflow_mcp_install::IntegrationBinaries;

const EXE_SUFFIX: &str = if cfg!(windows) { ".exe" } else { "" };

pub fn helper_path(home: &std::path::Path, stem: &str) -> PathBuf {
    home.join("bin").join(format!("{stem}{EXE_SUFFIX}"))
}

pub fn resolve_binaries() -> Option<IntegrationBinaries> {
    let home = paneflow_home::paneflow_home()?;
    let hook_binary = helper_path(&home, "paneflow-ai-hook");
    let bridge_binary = helper_path(&home, "paneflow-mcp");
    if !hook_binary.is_file() || !bridge_binary.is_file() {
        return None;
    }
    Some(IntegrationBinaries {
        hook_binary,
        bridge_binary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_paths_stay_inside_the_paneflow_home_bin_directory() {
        let home = std::path::Path::new("/state/.paneflow");
        let hook = helper_path(home, "paneflow-ai-hook");
        assert!(hook.starts_with(home.join("bin")));
        assert!(
            hook.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("paneflow-ai-hook"))
        );
    }

    #[test]
    fn a_home_with_no_staged_helpers_refuses_to_refresh_integrations() {
        let home = tempfile::tempdir().unwrap();
        let previous = std::env::var_os(paneflow_home::HOME_ENV);
        unsafe { std::env::set_var(paneflow_home::HOME_ENV, home.path()) };
        assert!(
            resolve_binaries().is_none(),
            "a refresh never runs against helpers that are not staged"
        );
        match previous {
            Some(value) => unsafe { std::env::set_var(paneflow_home::HOME_ENV, value) },
            None => unsafe { std::env::remove_var(paneflow_home::HOME_ENV) },
        }
    }
}
