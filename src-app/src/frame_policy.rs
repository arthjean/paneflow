use std::time::Duration;

use gpui::WindowOptions;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FramePolicy {
    pub(crate) inactive: Duration,
    pub(crate) low_power: Duration,
    pub(crate) thermal: Duration,
}

pub(crate) const FRAME_POLICY: FramePolicy = FramePolicy {
    inactive: Duration::from_micros(33_333),
    low_power: Duration::from_micros(33_333),
    thermal: Duration::from_micros(16_667),
};

pub(crate) fn paced_window_options() -> WindowOptions {
    WindowOptions {
        inactive_frame_interval: Some(FRAME_POLICY.inactive),
        low_power_frame_interval: Some(FRAME_POLICY.low_power),
        thermal_frame_interval: Some(FRAME_POLICY.thermal),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_caps_unfocused_and_low_power_windows_at_thirty_fps_and_thermal_pressure_at_sixty()
    {
        assert_eq!(
            FRAME_POLICY,
            FramePolicy {
                inactive: Duration::from_micros(33_333),
                low_power: Duration::from_micros(33_333),
                thermal: Duration::from_micros(16_667),
            }
        );
    }

    #[test]
    fn paced_window_options_carry_the_policy_and_leave_every_other_option_at_its_default() {
        let options = paced_window_options();
        assert_eq!(
            options.inactive_frame_interval,
            Some(Duration::from_micros(33_333))
        );
        assert_eq!(
            options.low_power_frame_interval,
            Some(Duration::from_micros(33_333))
        );
        assert_eq!(
            options.thermal_frame_interval,
            Some(Duration::from_micros(16_667))
        );
        let defaults = WindowOptions::default();
        assert_eq!(options.focus, defaults.focus);
        assert_eq!(options.show, defaults.show);
        assert_eq!(options.is_movable, defaults.is_movable);
        assert_eq!(options.is_resizable, defaults.is_resizable);
        assert_eq!(options.is_minimizable, defaults.is_minimizable);
        assert_eq!(
            options.app_owns_titlebar_drag,
            defaults.app_owns_titlebar_drag
        );
        assert!(options.window_bounds.is_none());
    }

    fn rust_sources(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("source directory is readable") {
            let path = entry.expect("source entry is readable").path();
            if path.is_dir() {
                rust_sources(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }

    #[test]
    fn every_paneflow_window_is_opened_with_the_paced_options() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let this_file = root.join("frame_policy.rs");
        let mut sources = Vec::new();
        rust_sources(&root, &mut sources);
        let mut windows_opened = 0;
        for path in sources.into_iter().filter(|path| *path != this_file) {
            let source = std::fs::read_to_string(&path).expect("source file is readable");
            let opened = source.matches(".open_window(").count();
            windows_opened += opened;
            assert_eq!(
                opened,
                source
                    .matches("..crate::frame_policy::paced_window_options()")
                    .count(),
                "{} opens a window without the frame policy",
                path.display()
            );
        }
        assert!(
            windows_opened >= 2,
            "the main window and the detached pane window are both found, got {windows_opened}"
        );
    }
}
