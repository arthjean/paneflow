//! Window chrome - title bar and CSD (client-side decoration) helpers.
//!
//! Groups the window-controls-and-resize-edge code that used to live as
//! sibling files at the crate root. Callers reach into the submodules
//! directly via `window_chrome::csd::…` and `window_chrome::title_bar::…`.

#[cfg(target_os = "windows")]
pub mod backdrop;
pub mod csd;
#[cfg(target_os = "linux")]
pub mod linux_backdrop;
#[cfg(target_os = "macos")]
pub mod macos_backdrop;
pub mod title_bar;

#[cfg(target_os = "linux")]
fn terminal_material_active_for_support(
    config: &paneflow_config::schema::PaneFlowConfig,
    native_blur_supported: bool,
) -> bool {
    native_blur_supported && config.linux_terminal_material_enabled()
}

pub(crate) fn terminal_material_active(config: &paneflow_config::schema::PaneFlowConfig) -> bool {
    #[cfg(target_os = "linux")]
    {
        terminal_material_active_for_support(config, linux_backdrop::terminal_material_available())
    }

    #[cfg(not(target_os = "linux"))]
    {
        config.terminal_material_enabled()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_terminal_material_requires_native_blur() {
        let mut config = paneflow_config::schema::PaneFlowConfig {
            linux_terminal_material: Some(true),
            ..Default::default()
        };

        assert!(!super::terminal_material_active_for_support(&config, false));
        assert!(super::terminal_material_active_for_support(&config, true));

        config.linux_terminal_material = Some(false);
        assert!(!super::terminal_material_active_for_support(&config, true));
        assert!(!super::terminal_material_active_for_support(&config, false));
    }
}
