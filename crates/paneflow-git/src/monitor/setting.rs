use std::sync::OnceLock;

pub const MONITOR_ENV: &str = "PANEFLOW_GIT_MONITOR";

pub const ACCEPTED_MONITOR_VALUES: &[&str] = &["on", "off"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorSetting {
    pub enabled: bool,
    pub warning: Option<String>,
}

pub fn parse_monitor_setting(value: Option<&str>) -> MonitorSetting {
    let Some(raw) = value else {
        return MonitorSetting {
            enabled: true,
            warning: None,
        };
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "on" => MonitorSetting {
            enabled: true,
            warning: None,
        },
        "off" => MonitorSetting {
            enabled: false,
            warning: None,
        },
        _ => MonitorSetting {
            enabled: true,
            warning: Some(format!(
                "{MONITOR_ENV}={raw:?} is not recognized, keeping the incremental git status on; accepted values: {}",
                ACCEPTED_MONITOR_VALUES.join(", ")
            )),
        },
    }
}

pub fn monitor_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let value = std::env::var(MONITOR_ENV).ok();
        let setting = parse_monitor_setting(value.as_deref());
        if let Some(warning) = &setting.warning {
            log::warn!("{warning}");
        }
        if !setting.enabled {
            log::info!("{MONITOR_ENV}=off: every git status probe takes a full snapshot");
        }
        setting.enabled
    })
}
