use super::{
    AgentPanelConfig, AgentsConfig, CommandDefinition, CursorBlinkConfig, CursorShapeConfig,
    Osc52ClipboardConfig, TelemetryConfig, TerminalConfig,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneFlowConfig {
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub shortcuts: HashMap<String, String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub default_shell: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub theme: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub theme_mode: Option<String>,
    #[serde(default, deserialize_with = "lenient_commands")]
    pub commands: Vec<CommandDefinition>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub window_decorations: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub window_backdrop: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub windows_terminal_material: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub windows_chrome_material: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub macos_chrome_material: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub linux_terminal_material: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub linux_chrome_material: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub unfocused_pane_opacity: Option<f32>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub reduce_motion: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub sidebar_show: SidebarShow,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub editor: EditorDisplayConfig,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub automation: AutomationConfig,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub worktrees: WorktreesConfig,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub line_height: Option<f32>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub cell_width: Option<f32>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub font_family: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub font_fallbacks: Option<Vec<String>>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub font_size: Option<f32>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub font_weight: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub option_as_meta: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub shell_integration: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub submit_paste_delay_ms: Option<u64>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub external_editor: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub claude_code_bypass_permissions: Option<bool>,
    #[serde(default, deserialize_with = "lenient_opt_bool")]
    pub ai_unrestricted: Option<bool>,
    #[serde(default, deserialize_with = "lenient_opt_bool")]
    pub ai_injection_fence: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub on_quit: Option<OnQuit>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub sidebar_ended_sessions: Option<u8>,
    #[serde(flatten, deserialize_with = "agent_button_visibility")]
    pub agent_button_visibility: BTreeMap<String, bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub telemetry: Option<TelemetryConfig>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub terminal: Option<TerminalConfig>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agent_panel: Option<AgentPanelConfig>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agents: Option<AgentsConfig>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient_value_or_default"
    )]
    pub agent_profiles: Vec<AgentProfileConfig>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarShow {
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub branch: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub diffstat: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub pr: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub indent_guide: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorDisplayConfig {
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub minimap: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub scrollbar: Option<bool>,
}

impl EditorDisplayConfig {
    pub fn minimap_enabled(&self) -> bool {
        self.minimap.unwrap_or(false)
    }

    pub fn scrollbar_enabled(&self) -> bool {
        self.scrollbar.unwrap_or(true)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomationConfig {
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub tab_auto_naming: Option<bool>,
}

impl AutomationConfig {
    pub fn tab_auto_naming_enabled(&self) -> bool {
        self.tab_auto_naming.unwrap_or(false)
    }
}

pub const WORKTREES_KEEP_LIMIT_DEFAULT: u32 = 15;
pub const WORKTREES_KEEP_LIMIT_MAX: u32 = 200;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreesConfig {
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub dir: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub auto_remove: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub keep_limit: Option<u32>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub for_new_branches: Option<bool>,
}

impl WorktreesConfig {
    pub fn new_branches_use_worktrees(&self) -> bool {
        self.for_new_branches.unwrap_or(true)
    }

    pub fn dir_path(&self) -> Option<std::path::PathBuf> {
        let raw = self.dir.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        let expanded =
            if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
                dirs::home_dir()?.join(rest)
            } else if raw == "~" {
                dirs::home_dir()?
            } else {
                std::path::PathBuf::from(raw)
            };
        expanded.is_absolute().then_some(expanded)
    }

    pub fn auto_remove_enabled(&self) -> bool {
        self.auto_remove.unwrap_or(true)
    }

    pub fn keep_limit(&self) -> u32 {
        self.keep_limit
            .unwrap_or(WORKTREES_KEEP_LIMIT_DEFAULT)
            .min(WORKTREES_KEEP_LIMIT_MAX)
    }
}

impl SidebarShow {
    pub fn branch_enabled(&self) -> bool {
        self.branch.unwrap_or(false)
    }

    pub fn diffstat_enabled(&self) -> bool {
        self.diffstat.unwrap_or(false)
    }

    pub fn pr_enabled(&self) -> bool {
        self.pr.unwrap_or(false)
    }

    pub fn indent_guide_enabled(&self) -> bool {
        self.indent_guide.unwrap_or(false)
    }

    pub fn any_enabled(&self) -> bool {
        self.branch_enabled() || self.diffstat_enabled()
    }
}

pub const AGENT_BUTTON_VISIBILITY_SUFFIX: &str = "_button_visible";

fn agent_button_visibility<'de, D>(d: D) -> Result<BTreeMap<String, bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let entries = BTreeMap::<String, serde_json::Value>::deserialize(d)?;
    Ok(entries
        .into_iter()
        .filter(|(key, _)| key.ends_with(AGENT_BUTTON_VISIBILITY_SUFFIX))
        .filter_map(|(key, value)| value.as_bool().map(|visible| (key, visible)))
        .collect())
}

impl PaneFlowConfig {
    pub const DEFAULT_UNFOCUSED_PANE_OPACITY: f32 = 1.0;
    pub const MIN_UNFOCUSED_PANE_OPACITY: f32 = 0.15;
    pub const MAX_UNFOCUSED_PANE_OPACITY: f32 = 1.0;

    pub const DEFAULT_SUBMIT_PASTE_DELAY_MS: u64 = 70;
    pub const MIN_SUBMIT_PASTE_DELAY_MS: u64 = 10;
    pub const MAX_SUBMIT_PASTE_DELAY_MS: u64 = 5_000;

    pub fn agent_button_visible(&self, key: &str) -> Option<bool> {
        self.agent_button_visibility.get(key).copied()
    }

    pub fn windows_terminal_material_enabled(&self) -> bool {
        cfg!(target_os = "windows") && self.windows_terminal_material.unwrap_or(false)
    }

    pub fn linux_terminal_material_enabled(&self) -> bool {
        cfg!(target_os = "linux") && self.linux_terminal_material.unwrap_or(false)
    }

    pub fn terminal_material_enabled(&self) -> bool {
        self.windows_terminal_material_enabled() || self.linux_terminal_material_enabled()
    }

    pub fn native_material_requested(&self) -> bool {
        self.cockpit_chrome_material_enabled() || self.terminal_material_enabled()
    }

    fn window_backdrop_disables_chrome_material(&self) -> bool {
        self.window_backdrop.as_deref().is_some_and(|value| {
            let value = value.trim();
            value.eq_ignore_ascii_case("opaque") || value.eq_ignore_ascii_case("off")
        })
    }

    pub fn macos_chrome_material_enabled(&self) -> bool {
        !self.window_backdrop_disables_chrome_material()
            && !self
                .window_backdrop
                .as_deref()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("transparent"))
            && self.macos_chrome_material.unwrap_or(false)
    }

    pub const INTERFACE_STYLE_KEYS: &'static [&'static str] = if cfg!(target_os = "windows") {
        &["windows_chrome_material", "windows_terminal_material"]
    } else if cfg!(target_os = "macos") {
        &["macos_chrome_material"]
    } else if cfg!(target_os = "linux") {
        &["linux_chrome_material", "linux_terminal_material"]
    } else {
        &[]
    };

    pub fn interface_style(&self) -> Option<InterfaceStyle> {
        let dual = [
            self.cockpit_chrome_material_enabled(),
            self.terminal_material_enabled(),
        ];
        let macos = [self.macos_chrome_material_enabled()];
        let materials: &[bool] = if cfg!(any(target_os = "windows", target_os = "linux")) {
            &dual
        } else if cfg!(target_os = "macos") {
            &macos
        } else {
            &[]
        };
        InterfaceStyle::from_materials(materials)
    }

    pub fn cockpit_chrome_material_enabled(&self) -> bool {
        if self.window_backdrop_disables_chrome_material() {
            return false;
        }

        if cfg!(target_os = "windows") {
            self.windows_chrome_material.unwrap_or(false)
        } else if cfg!(target_os = "macos") {
            self.macos_chrome_material_enabled()
        } else if cfg!(target_os = "linux") {
            self.linux_chrome_material.unwrap_or(false)
        } else {
            false
        }
    }

    pub fn reduce_motion_enabled(&self) -> bool {
        self.reduce_motion.unwrap_or(false)
    }

    pub fn resolved_submit_paste_delay_ms(&self) -> u64 {
        let raw = self
            .submit_paste_delay_ms
            .unwrap_or(Self::DEFAULT_SUBMIT_PASTE_DELAY_MS);
        let clamped = raw.clamp(
            Self::MIN_SUBMIT_PASTE_DELAY_MS,
            Self::MAX_SUBMIT_PASTE_DELAY_MS,
        );
        if clamped != raw {
            tracing::warn!(
                target: "paneflow_config::submit",
                requested = raw,
                clamped,
                "submit_paste_delay_ms out of range [{min}, {max}], clamped",
                min = Self::MIN_SUBMIT_PASTE_DELAY_MS,
                max = Self::MAX_SUBMIT_PASTE_DELAY_MS,
            );
        }
        clamped
    }

    pub fn resolved_unfocused_pane_dim_alpha(&self) -> f32 {
        let raw = self
            .unfocused_pane_opacity
            .filter(|value| value.is_finite())
            .unwrap_or(Self::DEFAULT_UNFOCUSED_PANE_OPACITY);
        let clamped = raw.clamp(
            Self::MIN_UNFOCUSED_PANE_OPACITY,
            Self::MAX_UNFOCUSED_PANE_OPACITY,
        );
        if clamped != raw {
            tracing::warn!(
                target: "paneflow_config::appearance",
                requested = raw,
                clamped,
                "unfocused_pane_opacity out of range [{min}, {max}], clamped",
                min = Self::MIN_UNFOCUSED_PANE_OPACITY,
                max = Self::MAX_UNFOCUSED_PANE_OPACITY,
            );
        }
        1.0 - clamped
    }

    pub fn ai_unrestricted_enabled(&self) -> bool {
        self.ai_unrestricted.unwrap_or(false)
    }

    pub fn ai_injection_fence_enabled(&self) -> bool {
        self.ai_injection_fence.unwrap_or(true)
    }

    pub fn resolved_on_quit(&self) -> OnQuit {
        self.on_quit.unwrap_or_default()
    }

    pub fn resolved_sidebar_ended_sessions(&self) -> u8 {
        self.sidebar_ended_sessions
            .filter(|cap| ENDED_SESSION_CAPS.contains(cap))
            .unwrap_or(DEFAULT_ENDED_SESSION_CAP)
    }
}

pub(super) fn lenient_opt_bool<'de, D>(d: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "boolean config toggle")
}

pub(super) fn lenient_opt_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "string config value")
}

pub(super) fn lenient_opt_usize<'de, D>(d: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "positive integer config value")
}

pub(super) fn lenient_opt_f32<'de, D>(d: D) -> Result<Option<f32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "number config value")
}

pub(super) fn lenient_opt_cursor_shape<'de, D>(d: D) -> Result<Option<CursorShapeConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "terminal cursor shape")
}

pub(super) fn lenient_opt_cursor_blink<'de, D>(d: D) -> Result<Option<CursorBlinkConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "terminal cursor blink mode")
}

pub(super) fn lenient_opt_osc52_clipboard<'de, D>(
    d: D,
) -> Result<Option<Osc52ClipboardConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "terminal OSC 52 clipboard policy")
}

pub(super) fn lenient_opt_string_map<'de, D>(
    d: D,
) -> Result<Option<HashMap<String, String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "string map config value")
}

fn lenient_opt_value<'de, D, T>(d: D, expected: &'static str) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned,
{
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(match v {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => match serde_json::from_value::<T>(value.clone()) {
            Ok(parsed) => Some(parsed),
            Err(_) => {
                tracing::warn!(
                    target: "paneflow_config",
                    value = %value,
                    expected,
                    "config value has an unexpected type, ignoring value and using resolver default",
                );
                None
            }
        },
    })
}

pub const ENDED_SESSION_CAPS: &[u8] = &[0, 3, 5, 10];

pub const DEFAULT_ENDED_SESSION_CAP: u8 = 5;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnQuit {
    #[default]
    Ask,
    Keep,
    Stop,
}

impl OnQuit {
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Keep => "keep",
            Self::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceStyle {
    Themed,
    Blended,
}

impl InterfaceStyle {
    pub const ALL: [Self; 2] = [Self::Themed, Self::Blended];

    pub fn label(self) -> &'static str {
        match self {
            Self::Themed => "Themed",
            Self::Blended => "Blended",
        }
    }

    fn from_materials(materials: &[bool]) -> Option<Self> {
        if materials.iter().all(|enabled| !enabled) {
            Some(Self::Themed)
        } else if materials.iter().all(|enabled| *enabled) {
            Some(Self::Blended)
        } else {
            None
        }
    }
}

fn lenient_value_or_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(d)?;
    Ok(match serde_json::from_value::<T>(value.clone()) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::warn!(
                target: "paneflow_config",
                value = %value,
                %error,
                "ignoring malformed config field and using its default",
            );
            T::default()
        }
    })
}

fn lenient_commands<'de, D>(d: D) -> Result<Vec<CommandDefinition>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(d)?;
    let Some(items) = value.as_array() else {
        tracing::warn!("ignoring config field `commands`: expected an array");
        return Ok(Vec::new());
    };

    Ok(items
        .iter()
        .enumerate()
        .filter_map(
            |(index, raw)| match serde_json::from_value::<CommandDefinition>(raw.clone()) {
                Ok(command) => Some(command),
                Err(error) => {
                    tracing::warn!("skipping invalid command entry at index {index}: {error}");
                    None
                }
            },
        )
        .collect())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AgentProfileConfig {
    pub name: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ended_session_cap_keeps_the_allowed_values_and_falls_back_to_five() {
        let config: PaneFlowConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.resolved_sidebar_ended_sessions(), 5);
        for cap in ENDED_SESSION_CAPS {
            let config: PaneFlowConfig =
                serde_json::from_str(&format!(r#"{{"sidebar_ended_sessions": {cap}}}"#)).unwrap();
            assert_eq!(config.resolved_sidebar_ended_sessions(), *cap);
        }
        for rejected in ["7", "\"many\"", "-3", "999"] {
            let config: PaneFlowConfig =
                serde_json::from_str(&format!(r#"{{"sidebar_ended_sessions": {rejected}}}"#))
                    .unwrap();
            assert_eq!(config.resolved_sidebar_ended_sessions(), 5);
        }
    }

    #[test]
    fn on_quit_defaults_to_ask_and_tolerates_garbage() {
        let config: PaneFlowConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.resolved_on_quit(), OnQuit::Ask);
        let config: PaneFlowConfig = serde_json::from_str(r#"{"on_quit": "stop"}"#).unwrap();
        assert_eq!(config.resolved_on_quit(), OnQuit::Stop);
        let config: PaneFlowConfig = serde_json::from_str(r#"{"on_quit": "later"}"#).unwrap();
        assert_eq!(config.resolved_on_quit(), OnQuit::Ask);
        let config: PaneFlowConfig = serde_json::from_str(r#"{"on_quit": 3}"#).unwrap();
        assert_eq!(config.resolved_on_quit(), OnQuit::Ask);
    }

    #[test]
    fn interface_style_is_themed_or_blended_only_when_every_material_agrees() {
        assert_eq!(
            InterfaceStyle::from_materials(&[]),
            Some(InterfaceStyle::Themed)
        );
        assert_eq!(
            InterfaceStyle::from_materials(&[false, false]),
            Some(InterfaceStyle::Themed)
        );
        assert_eq!(
            InterfaceStyle::from_materials(&[true, true]),
            Some(InterfaceStyle::Blended)
        );
        assert_eq!(InterfaceStyle::from_materials(&[true, false]), None);
    }

    #[test]
    fn interface_style_defaults_to_themed_and_its_keys_select_blended() {
        assert_eq!(
            PaneFlowConfig::default().interface_style(),
            Some(InterfaceStyle::Themed)
        );

        let blended: serde_json::Map<String, serde_json::Value> =
            PaneFlowConfig::INTERFACE_STYLE_KEYS
                .iter()
                .map(|key| ((*key).to_string(), serde_json::Value::Bool(true)))
                .collect();
        let config: PaneFlowConfig =
            serde_json::from_value(serde_json::Value::Object(blended)).unwrap();
        let expected = if PaneFlowConfig::INTERFACE_STYLE_KEYS.is_empty() {
            InterfaceStyle::Themed
        } else {
            InterfaceStyle::Blended
        };
        assert_eq!(config.interface_style(), Some(expected));
    }
}
