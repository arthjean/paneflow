//! Shared Windows identity used by the installer, taskbar, and notifications.

#[cfg(any(target_os = "windows", test))]
pub(crate) const PANEFLOW_WINDOWS_AUMID: &str = "Strivex.PaneFlow";

#[cfg(target_os = "windows")]
pub(crate) fn ensure_process_app_user_model_id() -> Result<(), String> {
    let app_id = windows_wide_null(PANEFLOW_WINDOWS_AUMID);
    let result = unsafe {
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr())
    };
    if result < 0 {
        Err(format!(
            "SetCurrentProcessExplicitAppUserModelID({PANEFLOW_WINDOWS_AUMID}) returned HRESULT 0x{:08X}",
            result as u32
        ))
    } else {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn windows_wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_aumid_matches_wix_shortcut_identity() {
        let wix = include_str!("../../packaging/wix/main.wxs");
        let shortcut_identity =
            format!("Key='System.AppUserModel.ID' Value='{PANEFLOW_WINDOWS_AUMID}'");

        assert!(
            wix.contains(&shortcut_identity),
            "Windows shortcut identity must match the process AUMID"
        );
    }

    #[test]
    fn wix_shortcut_uses_target_exe_icon() {
        let wix = include_str!("../../packaging/wix/main.wxs");
        let shortcut = wix
            .split("<Shortcut Id='ApplicationStartMenuShortcut'")
            .nth(1)
            .and_then(|rest| rest.split("</Shortcut>").next())
            .expect("ApplicationStartMenuShortcut block should exist");

        assert!(
            shortcut.contains("Target='[APPLICATIONFOLDER]paneflow.exe'"),
            "Start Menu shortcut should target the installed exe"
        );
        assert!(
            !shortcut.contains("Icon='"),
            "Start Menu shortcut must use the exe icon, not an MSI icon-table path"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_wide_null_is_null_terminated() {
        let wide = windows_wide_null(PANEFLOW_WINDOWS_AUMID);

        assert_eq!(wide.last(), Some(&0));
        assert_eq!(
            wide.iter().filter(|unit| **unit == 0).count(),
            1,
            "AUMID should contain a single trailing nul"
        );
    }
}
