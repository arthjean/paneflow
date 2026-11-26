use std::path::PathBuf;

use paneflow_agent_integrations::IntegrationBinaries;

const EXE_SUFFIX: &str = if cfg!(windows) { ".exe" } else { "" };
pub const HOOK_HELPER: &str = "paneflow-ai-hook";

pub fn helper_path(home: &std::path::Path, stem: &str) -> PathBuf {
    home.join("bin").join(format!("{stem}{EXE_SUFFIX}"))
}

pub fn resolve_binaries(home: &std::path::Path) -> Result<IntegrationBinaries, PathBuf> {
    let hook_binary = helper_path(home, HOOK_HELPER);
    if !hook_binary.is_file() {
        return Err(hook_binary);
    }
    Ok(IntegrationBinaries { hook_binary })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_paths_stay_inside_the_paneflow_home_bin_directory() {
        let home = std::path::Path::new("/state/.paneflow");
        let hook = helper_path(home, HOOK_HELPER);
        assert!(hook.starts_with(home.join("bin")));
        assert!(
            hook.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(HOOK_HELPER))
        );
    }

    #[test]
    fn the_refresh_needs_only_the_hook_helper_and_names_it_when_it_is_missing() {
        let home = tempfile::tempdir().unwrap();
        let hook = helper_path(home.path(), HOOK_HELPER);
        assert_eq!(resolve_binaries(home.path()).unwrap_err(), hook);

        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, b"helper").unwrap();
        assert!(!helper_path(home.path(), "paneflow-mcp").exists());
        assert_eq!(resolve_binaries(home.path()).unwrap().hook_binary, hook);
    }

    #[test]
    fn the_refresh_resolves_helpers_in_the_home_it_was_given_not_the_process_home() {
        let given = tempfile::tempdir().unwrap();
        let process_home = tempfile::tempdir().unwrap();
        let helper = helper_path(process_home.path(), HOOK_HELPER);
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::fs::write(&helper, b"helper").unwrap();
        let previous = std::env::var_os(paneflow_home::HOME_ENV);
        unsafe { std::env::set_var(paneflow_home::HOME_ENV, process_home.path()) };
        let unstaged = resolve_binaries(given.path());
        let helper = helper_path(given.path(), HOOK_HELPER);
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::fs::write(&helper, b"helper").unwrap();
        let staged = resolve_binaries(given.path());
        match previous {
            Some(value) => unsafe { std::env::set_var(paneflow_home::HOME_ENV, value) },
            None => unsafe { std::env::remove_var(paneflow_home::HOME_ENV) },
        }
        assert!(
            unstaged.is_err(),
            "the process home's helpers never stand in for the given home's"
        );
        let staged = staged.expect("the given home's helpers");
        assert!(staged.hook_binary.starts_with(given.path()));
    }
}
