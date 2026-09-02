use serde_json::{json, Map, Value};

pub(crate) const MAX_EVENT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TelemetryVersion(String);

impl TelemetryVersion {
    pub fn parse(value: &str) -> Option<Self> {
        if value.len() > 64 {
            return None;
        }
        let version = semver::Version::parse(value).ok()?;
        Some(Self(format!(
            "{}.{}.{}",
            version.major, version.minor, version.patch
        )))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperatingSystem {
    Linux,
    MacOs,
    Windows,
    Other,
}

impl OperatingSystem {
    pub const fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::MacOs => "macos",
            Self::Windows => "windows",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Architecture {
    X86_64,
    Aarch64,
    Other,
}

impl Architecture {
    pub const fn current() -> Self {
        if cfg!(target_arch = "x86_64") {
            Self::X86_64
        } else if cfg!(target_arch = "aarch64") {
            Self::Aarch64
        } else {
            Self::Other
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionErrorCategory {
    Io,
    Syntax,
    Data,
    Eof,
    Oversize,
    NonRegular,
    UnsupportedVersion,
}

impl SessionErrorCategory {
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "io" => Some(Self::Io),
            "syntax" => Some(Self::Syntax),
            "data" => Some(Self::Data),
            "eof" => Some(Self::Eof),
            "oversize" => Some(Self::Oversize),
            "non_regular" => Some(Self::NonRegular),
            "unsupported_version" => Some(Self::UnsupportedVersion),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Io => "io",
            Self::Syntax => "syntax",
            Self::Data => "data",
            Self::Eof => "eof",
            Self::Oversize => "oversize",
            Self::NonRegular => "non_regular",
            Self::UnsupportedVersion => "unsupported_version",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallMethod {
    Deb,
    Rpm,
    RpmOstree,
    Other,
    AppImage,
    TarGz,
    Dmg,
    Msi,
    ExternallyManaged,
    Unknown,
}

impl InstallMethod {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::RpmOstree => "rpm-ostree",
            Self::Other => "other",
            Self::AppImage => "appimage",
            Self::TarGz => "tar.gz",
            Self::Dmg => "dmg",
            Self::Msi => "msi",
            Self::ExternallyManaged => "externally-managed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateErrorCategory {
    Network,
    Signature,
    Disk,
    Unknown,
}

impl UpdateErrorCategory {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Signature => "signature",
            Self::Disk => "disk",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAssetFormat {
    Deb,
    Rpm,
    AppImage,
    TarGz,
    Dmg,
    Msi,
}

impl UpdateAssetFormat {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::AppImage => "appimage",
            Self::TarGz => "targz",
            Self::Dmg => "dmg",
            Self::Msi => "msi",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TelemetryEvent {
    name: &'static str,
    properties: Map<String, Value>,
}

impl TelemetryEvent {
    pub fn session_corrupted(
        error: SessionErrorCategory,
        file_size: u64,
        file_age_seconds: Option<u64>,
        backup_written: bool,
    ) -> Self {
        Self::new(
            "session_corrupted",
            properties([
                ("error", json!(error.as_str())),
                ("file_size", json!(file_size)),
                ("file_age_seconds", json!(file_age_seconds)),
                ("backup_written", json!(backup_written)),
            ]),
        )
    }

    pub fn app_started(
        os: OperatingSystem,
        arch: Architecture,
        app_version: TelemetryVersion,
        install_method: InstallMethod,
        is_first_run: bool,
    ) -> Self {
        Self::new(
            "app_started",
            properties([
                ("os", json!(os.as_str())),
                ("arch", json!(arch.as_str())),
                ("app_version", version_value(app_version)),
                ("install_method", json!(install_method.as_str())),
                ("is_first_run", json!(is_first_run)),
            ]),
        )
    }

    pub fn app_exited(session_duration_seconds: u64) -> Self {
        Self::new(
            "app_exited",
            properties([("session_duration_seconds", json!(session_duration_seconds))]),
        )
    }

    pub fn update_installed(
        from_version: TelemetryVersion,
        to_version: Option<TelemetryVersion>,
        install_method: InstallMethod,
    ) -> Self {
        Self::new(
            "update_installed",
            properties([
                ("from_version", version_value(from_version)),
                ("to_version", reported_version_value(to_version)),
                ("install_method", json!(install_method.as_str())),
                ("success", json!(true)),
            ]),
        )
    }

    pub fn update_install_failed(
        from_version: TelemetryVersion,
        to_version: Option<TelemetryVersion>,
        install_method: InstallMethod,
        error_category: UpdateErrorCategory,
    ) -> Self {
        Self::new(
            "update_installed",
            properties([
                ("from_version", version_value(from_version)),
                ("to_version", reported_version_value(to_version)),
                ("install_method", json!(install_method.as_str())),
                ("success", json!(false)),
                ("error_category", json!(error_category.as_str())),
            ]),
        )
    }

    pub fn update_check_started(current_version: TelemetryVersion) -> Self {
        Self::new(
            "update_check_started",
            properties([
                ("trigger", json!("auto")),
                ("current_version", version_value(current_version)),
            ]),
        )
    }

    pub fn update_available(
        from_version: TelemetryVersion,
        to_version: TelemetryVersion,
        asset_format: UpdateAssetFormat,
    ) -> Self {
        Self::new(
            "update_available",
            properties([
                ("from_version", version_value(from_version)),
                ("to_version", version_value(to_version)),
                ("asset_format", json!(asset_format.as_str())),
            ]),
        )
    }

    pub fn update_dismissed(
        from_version: TelemetryVersion,
        to_version: Option<TelemetryVersion>,
    ) -> Self {
        Self::new(
            "update_dismissed",
            properties([
                ("from_version", version_value(from_version)),
                ("to_version", reported_version_value(to_version)),
                ("reason", json!("user_dismissed")),
            ]),
        )
    }

    pub fn telemetry_reenabled() -> Self {
        Self::new("telemetry_reenabled", Map::new())
    }

    pub(crate) fn name(&self) -> &'static str {
        self.name
    }

    pub(crate) fn encoded_len_if_safe(&self) -> Option<usize> {
        if !self.name.bytes().all(is_event_name_byte) || !properties_are_safe(&self.properties) {
            return None;
        }
        let encoded = serde_json::to_vec(&self.properties).ok()?;
        (encoded.len() <= MAX_EVENT_BYTES).then_some(encoded.len())
    }

    pub(crate) fn into_parts(self) -> (&'static str, Map<String, Value>) {
        (self.name, self.properties)
    }

    fn new(name: &'static str, properties: Map<String, Value>) -> Self {
        Self { name, properties }
    }
}

fn properties<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn version_value(version: TelemetryVersion) -> Value {
    Value::String(version.0)
}

fn reported_version_value(version: Option<TelemetryVersion>) -> Value {
    version.map_or_else(|| json!("unknown"), version_value)
}

fn properties_are_safe(properties: &Map<String, Value>) -> bool {
    properties.iter().all(|(key, value)| {
        !key.is_empty()
            && key.len() <= 64
            && key.bytes().all(is_property_name_byte)
            && value_is_safe(value)
    })
}

fn value_is_safe(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(value) => {
            !value.is_empty()
                && value.len() <= 64
                && value.bytes().all(is_low_cardinality_value_byte)
        }
        Value::Array(values) => values.iter().all(value_is_safe),
        Value::Object(values) => properties_are_safe(values),
    }
}

fn is_event_name_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
}

fn is_property_name_byte(byte: u8) -> bool {
    is_event_name_byte(byte)
}

fn is_low_cardinality_value_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(value: &str) -> TelemetryVersion {
        TelemetryVersion::parse(value).unwrap()
    }

    fn canonical_events() -> [TelemetryEvent; 9] {
        [
            TelemetryEvent::session_corrupted(SessionErrorCategory::Syntax, 42, Some(3), true),
            TelemetryEvent::app_started(
                OperatingSystem::Linux,
                Architecture::X86_64,
                version("0.8.2"),
                InstallMethod::TarGz,
                true,
            ),
            TelemetryEvent::app_exited(12),
            TelemetryEvent::update_installed(
                version("0.8.1"),
                Some(version("0.8.2")),
                InstallMethod::AppImage,
            ),
            TelemetryEvent::update_install_failed(
                version("0.8.1"),
                None,
                InstallMethod::Deb,
                UpdateErrorCategory::Network,
            ),
            TelemetryEvent::update_check_started(version("0.8.2")),
            TelemetryEvent::update_available(
                version("0.8.1"),
                version("0.8.2"),
                UpdateAssetFormat::TarGz,
            ),
            TelemetryEvent::update_dismissed(version("0.8.1"), Some(version("0.8.2"))),
            TelemetryEvent::telemetry_reenabled(),
        ]
    }

    #[test]
    fn canonical_events_pass_runtime_validation() {
        assert!(canonical_events()
            .iter()
            .all(|event| event.encoded_len_if_safe().is_some()));
    }

    #[test]
    fn event_names_and_property_shapes_are_exact() {
        let events = canonical_events();
        assert_eq!(events[0].name, "session_corrupted");
        assert_eq!(events[0].properties["error"], "syntax");
        assert_eq!(events[1].properties["install_method"], "tar.gz");
        assert_eq!(events[4].properties["success"], false);
        assert_eq!(events[6].properties["asset_format"], "targz");
        assert_eq!(events[7].properties["reason"], "user_dismissed");
        assert!(events[8].properties.is_empty());
    }

    #[test]
    fn versions_reject_non_semver_and_strip_free_form_metadata() {
        for value in ["arthur", "arthur-mbp", "192.168.1.2", "/home/arthur"] {
            assert!(
                TelemetryVersion::parse(value).is_none(),
                "accepted {value:?}"
            );
        }
        assert_eq!(
            TelemetryVersion::parse("1.2.3-arthur-mbp").unwrap().0,
            "1.2.3"
        );
        assert_eq!(
            TelemetryVersion::parse("1.2.3+ArthurJean").unwrap().0,
            "1.2.3"
        );

        let event = TelemetryEvent::update_installed(version("0.8.1"), None, InstallMethod::Deb);
        assert_eq!(event.properties["to_version"], "unknown");
    }

    #[test]
    fn reserved_properties_are_rejected() {
        let event = TelemetryEvent::new(
            "unsafe",
            properties([("$process_person_profile", json!(true))]),
        );
        assert!(event.encoded_len_if_safe().is_none());
    }

    #[test]
    fn session_error_category_rejects_unknown_tags() {
        assert_eq!(
            SessionErrorCategory::from_tag("syntax"),
            Some(SessionErrorCategory::Syntax)
        );
        assert_eq!(SessionErrorCategory::from_tag("arthur"), None);
    }
}
