use std::path::Path;

use super::install::{self, Readiness};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Matched,
    Mixed(String),
    Absent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallSite {
    pub live_hosts: usize,
    pub runtime: RuntimeState,
    pub required_bytes: u64,
    pub free_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallPlan {
    Ready,
    Resume(String),
    Blocked(String),
}

pub fn plan(site: &InstallSite) -> InstallPlan {
    if site.live_hosts > 0 {
        return InstallPlan::Blocked(
            "Close the open browser pages before updating - Windows cannot replace the browser runtime while its host holds those files".to_string(),
        );
    }
    if let Some(free) = site.free_bytes.filter(|free| *free < site.required_bytes) {
        return InstallPlan::Blocked(format!(
            "The browser runtime needs {} MB free on the installation drive and {} MB are available; the installed version and its profiles are kept",
            site.required_bytes / (1024 * 1024),
            free / (1024 * 1024)
        ));
    }
    match &site.runtime {
        RuntimeState::Mixed(reason) => InstallPlan::Resume(format!(
            "Resuming an interrupted browser installation: {reason}. The update replaces the whole payload; your profiles are preserved."
        )),
        RuntimeState::Matched | RuntimeState::Absent => InstallPlan::Ready,
    }
}

pub fn runtime_state() -> RuntimeState {
    match install::detect() {
        Readiness::Ready(..) => RuntimeState::Matched,
        Readiness::Unusable(reason) => RuntimeState::Mixed(reason),
        Readiness::Absent => RuntimeState::Absent,
    }
}

pub fn free_bytes(directory: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = directory
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut available = 0_u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(available)
}

pub fn site(live_hosts: usize) -> InstallSite {
    let install_directory = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    InstallSite {
        live_hosts,
        runtime: runtime_state(),
        required_bytes: install::required_bytes(),
        free_bytes: install_directory.as_deref().and_then(free_bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> InstallSite {
        InstallSite {
            live_hosts: 0,
            runtime: RuntimeState::Matched,
            required_bytes: 512 * 1024 * 1024,
            free_bytes: Some(4 * 1024 * 1024 * 1024),
        }
    }

    #[test]
    fn a_live_host_retains_the_runtime_files_and_blocks_the_replacement() {
        let blocked = plan(&InstallSite {
            live_hosts: 1,
            ..site()
        });
        let InstallPlan::Blocked(reason) = blocked else {
            panic!("a live browser host must block the MSI replacement");
        };
        assert!(reason.contains("Close the open browser pages"));
    }

    #[test]
    fn an_interrupted_installation_is_resumable_and_keeps_the_previous_data() {
        let mixed = RuntimeState::Mixed(
            "the installed Windows browser runtime was verified against manifest a, but this build expects b".to_string(),
        );
        let InstallPlan::Resume(reason) = plan(&InstallSite {
            runtime: mixed.clone(),
            ..site()
        }) else {
            panic!("a version mix must be reported as a resumable installation");
        };
        assert!(reason.contains("interrupted"));
        assert!(reason.contains("profiles are preserved"));
        assert_eq!(
            plan(&InstallSite {
                live_hosts: 1,
                runtime: mixed,
                ..site()
            }),
            plan(&InstallSite {
                live_hosts: 1,
                ..site()
            })
        );
    }

    #[test]
    fn insufficient_space_keeps_the_installed_version() {
        let InstallPlan::Blocked(reason) = plan(&InstallSite {
            free_bytes: Some(64 * 1024 * 1024),
            ..site()
        }) else {
            panic!("an installation drive without room must block the replacement");
        };
        assert!(reason.contains("512 MB free"));
        assert!(reason.contains("64 MB are available"));
        assert!(reason.contains("profiles are kept"));
    }

    #[test]
    fn an_unreadable_drive_does_not_invent_a_space_verdict() {
        assert_eq!(
            plan(&InstallSite {
                free_bytes: None,
                ..site()
            }),
            InstallPlan::Ready
        );
    }

    #[test]
    fn a_terminal_only_installation_updates_without_the_browser_payload() {
        assert_eq!(
            plan(&InstallSite {
                runtime: RuntimeState::Absent,
                ..site()
            }),
            InstallPlan::Ready
        );
        assert_eq!(plan(&site()), InstallPlan::Ready);
    }

    #[test]
    fn the_manifest_declares_the_payload_the_installation_drive_must_hold() {
        assert!(install::required_bytes() > 128 * 1024 * 1024);
    }
}
