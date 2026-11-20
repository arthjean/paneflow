use super::{
    AgentPanelConfig, AgentsConfig, CommandDefinition, CursorBlinkConfig, CursorShapeConfig,
    OpenLinksInConfig, Osc52ClipboardConfig, SpaceConfig, SpaceId, TelemetryConfig, TerminalConfig,
    UsageConfig,
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
    #[serde(default, deserialize_with = "lenient_background_style")]
    pub background_style: Option<BackgroundStyle>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub window_backdrop: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub windows_terminal_material: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub windows_chrome_material: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub windows_material_veil: Option<bool>,
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
    pub comfortable_density: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub interface_style: Option<InterfaceStyle>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub app_icon: Option<AppIcon>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub sidebar_show: SidebarShow,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub sidebar_group_by: Option<SidebarGroupBy>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub workspace_icon: Option<WorkspaceIcon>,
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
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub usage: Option<UsageConfig>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient_value_or_default"
    )]
    pub agent_profiles: Vec<AgentProfileConfig>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "super::space::lenient_spaces"
    )]
    pub spaces: Vec<SpaceConfig>,
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
    pub status: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub indent_guide: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agents: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub usage: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agent_logo: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agent_workspace: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub shimmer: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agent_shimmer: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agent_all_spaces: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorDisplayConfig {
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub minimap: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub scrollbar: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub word_wrap: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub markdown_rendered: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub csv_table: Option<bool>,
}

impl EditorDisplayConfig {
    pub fn minimap_enabled(&self) -> bool {
        self.minimap.unwrap_or(false)
    }

    pub fn scrollbar_enabled(&self) -> bool {
        self.scrollbar.unwrap_or(true)
    }

    pub fn word_wrap_enabled(&self) -> bool {
        self.word_wrap.unwrap_or(false)
    }

    pub fn markdown_rendered_enabled(&self) -> bool {
        self.markdown_rendered.unwrap_or(false)
    }

    pub fn csv_table_enabled(&self) -> bool {
        self.csv_table.unwrap_or(true)
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

    pub fn status_enabled(&self) -> bool {
        self.status.unwrap_or(true)
    }

    pub fn indent_guide_enabled(&self) -> bool {
        self.indent_guide.unwrap_or(false)
    }

    pub fn agents_enabled(&self) -> bool {
        self.agents.unwrap_or(true)
    }

    pub fn usage_enabled(&self) -> bool {
        self.usage.unwrap_or(true)
    }

    pub fn agent_logo_enabled(&self) -> bool {
        self.agent_logo.unwrap_or(true)
    }

    pub fn agent_workspace_enabled(&self) -> bool {
        self.agent_workspace.unwrap_or(true)
    }

    pub fn shimmer_enabled(&self) -> bool {
        self.shimmer.unwrap_or(true)
    }

    pub fn agent_shimmer_enabled(&self) -> bool {
        self.agent_shimmer.unwrap_or(true)
    }

    pub fn agent_all_spaces_enabled(&self) -> bool {
        self.agent_all_spaces.unwrap_or(false)
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

    pub const LEGACY_BACKGROUND_STYLE_KEYS: [&'static str; 7] = [
        "window_backdrop",
        "windows_terminal_material",
        "windows_chrome_material",
        "windows_material_veil",
        "macos_chrome_material",
        "linux_terminal_material",
        "linux_chrome_material",
    ];

    pub fn resolved_background_style(&self) -> BackgroundStyle {
        self.background_style
            .unwrap_or_else(|| self.legacy_background_style(HostPlatform::CURRENT))
    }

    pub fn set_background_style(&mut self, style: BackgroundStyle) {
        self.background_style = Some(style);
        self.window_backdrop = None;
        self.windows_terminal_material = None;
        self.windows_chrome_material = None;
        self.windows_material_veil = None;
        self.macos_chrome_material = None;
        self.linux_terminal_material = None;
        self.linux_chrome_material = None;
    }

    fn legacy_background_style(&self, platform: HostPlatform) -> BackgroundStyle {
        let backdrop_disables_materials = self.window_backdrop.as_deref().is_some_and(|value| {
            let value = value.trim();
            value.eq_ignore_ascii_case("opaque")
                || value.eq_ignore_ascii_case("off")
                || (platform == HostPlatform::Macos && value.eq_ignore_ascii_case("transparent"))
        });
        if backdrop_disables_materials {
            return BackgroundStyle::Themed;
        }
        let materials = match platform {
            HostPlatform::Windows => [self.windows_chrome_material, self.windows_terminal_material],
            HostPlatform::Macos => [self.macos_chrome_material, None],
            HostPlatform::Linux => [self.linux_chrome_material, self.linux_terminal_material],
            HostPlatform::Other => [None, None],
        };
        if materials.iter().any(|material| material.unwrap_or(false)) {
            BackgroundStyle::Blended
        } else {
            BackgroundStyle::Themed
        }
    }

    fn blended(&self) -> bool {
        self.resolved_background_style() == BackgroundStyle::Blended
    }

    pub fn windows_terminal_material_enabled(&self) -> bool {
        cfg!(target_os = "windows") && self.blended()
    }

    pub fn linux_terminal_material_enabled(&self) -> bool {
        cfg!(target_os = "linux") && self.blended()
    }

    pub fn terminal_material_enabled(&self) -> bool {
        self.windows_terminal_material_enabled() || self.linux_terminal_material_enabled()
    }

    pub fn native_material_requested(&self) -> bool {
        self.cockpit_chrome_material_enabled() || self.terminal_material_enabled()
    }

    pub fn macos_chrome_material_enabled(&self) -> bool {
        cfg!(target_os = "macos") && self.blended()
    }

    pub fn material_veiled(&self) -> bool {
        if cfg!(target_os = "linux") {
            true
        } else if cfg!(target_os = "windows") {
            self.blended()
        } else {
            false
        }
    }

    pub fn cockpit_chrome_material_enabled(&self) -> bool {
        cfg!(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "linux"
        )) && self.blended()
    }

    pub fn reduce_motion_enabled(&self) -> bool {
        self.reduce_motion.unwrap_or(false)
    }

    pub fn comfortable_density_enabled(&self) -> bool {
        self.comfortable_density.unwrap_or(true)
    }

    pub fn resolved_interface_style(&self) -> InterfaceStyle {
        self.interface_style.unwrap_or_default()
    }

    pub fn resolved_app_icon(&self) -> AppIcon {
        self.app_icon.unwrap_or_default()
    }

    pub fn resolved_sidebar_group_by(&self) -> SidebarGroupBy {
        self.sidebar_group_by.unwrap_or_default()
    }

    pub fn resolved_workspace_icon(&self) -> WorkspaceIcon {
        self.workspace_icon.unwrap_or_default()
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

    pub fn space(&self, id: &SpaceId) -> Option<&SpaceConfig> {
        self.spaces.iter().find(|space| &space.id == id)
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

pub(super) fn lenient_opt_open_links_in<'de, D>(d: D) -> Result<Option<OpenLinksInConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "terminal link target")
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InterfaceStyle {
    #[default]
    Classic,
    #[serde(alias = "paneflow")]
    Rounded,
}

impl InterfaceStyle {
    pub const ALL: &'static [Self] = &[Self::Classic, Self::Rounded];

    pub fn label(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::Rounded => "Rounded",
        }
    }

    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Rounded => "rounded",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppIcon {
    #[default]
    Glass,
    Black,
    White,
    BlackSolid,
    WhiteSolid,
    Aubergine,
    Paper,
    MidnightMatte,
    Polar,
    Melon,
    Honeydew,
    Berry,
    Rainbow,
    Solarwave,
    Neon,
    Phosphor,
    Holo,
    Amethyst,
    Blueprint,
    Chip,
    Nostalgia,
}

impl AppIcon {
    pub const ALL: &'static [Self] = &[
        Self::Glass,
        Self::Black,
        Self::White,
        Self::BlackSolid,
        Self::WhiteSolid,
        Self::Aubergine,
        Self::Paper,
        Self::MidnightMatte,
        Self::Polar,
        Self::Melon,
        Self::Honeydew,
        Self::Berry,
        Self::Rainbow,
        Self::Solarwave,
        Self::Neon,
        Self::Phosphor,
        Self::Holo,
        Self::Amethyst,
        Self::Blueprint,
        Self::Chip,
        Self::Nostalgia,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Glass => "Glass",
            Self::Black => "Black",
            Self::White => "White",
            Self::BlackSolid => "Black solid",
            Self::WhiteSolid => "White solid",
            Self::Aubergine => "Aubergine",
            Self::Paper => "Paper",
            Self::MidnightMatte => "Midnight matte",
            Self::Polar => "Polar",
            Self::Melon => "Melon",
            Self::Honeydew => "Honeydew",
            Self::Berry => "Berry",
            Self::Rainbow => "Rainbow",
            Self::Solarwave => "Solarwave",
            Self::Neon => "Neon",
            Self::Phosphor => "Phosphor",
            Self::Holo => "Holo",
            Self::Amethyst => "Amethyst",
            Self::Blueprint => "Blueprint",
            Self::Chip => "Chip",
            Self::Nostalgia => "Nostalgia",
        }
    }

    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Glass => "glass",
            Self::Black => "black",
            Self::White => "white",
            Self::BlackSolid => "black-solid",
            Self::WhiteSolid => "white-solid",
            Self::Aubergine => "aubergine",
            Self::Paper => "paper",
            Self::MidnightMatte => "midnight-matte",
            Self::Polar => "polar",
            Self::Melon => "melon",
            Self::Honeydew => "honeydew",
            Self::Berry => "berry",
            Self::Rainbow => "rainbow",
            Self::Solarwave => "solarwave",
            Self::Neon => "neon",
            Self::Phosphor => "phosphor",
            Self::Holo => "holo",
            Self::Amethyst => "amethyst",
            Self::Blueprint => "blueprint",
            Self::Chip => "chip",
            Self::Nostalgia => "nostalgia",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarGroupBy {
    #[default]
    Folder,
    Status,
}

impl SidebarGroupBy {
    pub const ALL: &'static [Self] = &[Self::Folder, Self::Status];

    pub fn label(self) -> &'static str {
        match self {
            Self::Folder => "Folder",
            Self::Status => "Status",
        }
    }

    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Status => "status",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceIcon {
    #[default]
    Default,
    Standard,
    Metal,
    Windows,
    Windows95,
    Pixel,
}

impl WorkspaceIcon {
    pub const ALL: &'static [Self] = &[
        Self::Default,
        Self::Standard,
        Self::Metal,
        Self::Windows,
        Self::Windows95,
        Self::Pixel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Standard => "Standard",
            Self::Metal => "Metal",
            Self::Windows => "Windows",
            Self::Windows95 => "Windows 95",
            Self::Pixel => "Pixel",
        }
    }

    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Standard => "standard",
            Self::Metal => "metal",
            Self::Windows => "windows",
            Self::Windows95 => "windows95",
            Self::Pixel => "pixel",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundStyle {
    #[default]
    Themed,
    Blended,
}

impl BackgroundStyle {
    pub const ALL: &'static [Self] = &[Self::Themed, Self::Blended];

    pub fn label(self) -> &'static str {
        match self {
            Self::Themed => "Themed",
            Self::Blended => "Blended",
        }
    }

    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Themed => "themed",
            Self::Blended => "blended",
        }
    }

    fn from_wire(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|style| style.wire_str().eq_ignore_ascii_case(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostPlatform {
    Windows,
    Macos,
    Linux,
    Other,
}

impl HostPlatform {
    const CURRENT: Self = if cfg!(target_os = "windows") {
        Self::Windows
    } else if cfg!(target_os = "macos") {
        Self::Macos
    } else if cfg!(target_os = "linux") {
        Self::Linux
    } else {
        Self::Other
    };
}

fn lenient_background_style<'de, D>(d: D) -> Result<Option<BackgroundStyle>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(d)?;
    if value.is_null() {
        return Ok(None);
    }
    let style = value
        .as_str()
        .and_then(BackgroundStyle::from_wire)
        .unwrap_or_else(|| {
            tracing::warn!(
                target: "paneflow_config",
                value = %value,
                "unknown background_style value; using themed",
            );
            BackgroundStyle::Themed
        });
    Ok(Some(style))
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
    fn interface_style_defaults_to_classic_and_tolerates_garbage() {
        let config: PaneFlowConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.resolved_interface_style(), InterfaceStyle::Classic);
        let config: PaneFlowConfig =
            serde_json::from_str(r#"{"interface_style": "rounded"}"#).unwrap();
        assert_eq!(config.resolved_interface_style(), InterfaceStyle::Rounded);
        for retired in ["crisp", "cursor", "claude"] {
            let json = serde_json::json!({ "interface_style": retired });
            let config: PaneFlowConfig = serde_json::from_value(json).unwrap();
            assert_eq!(config.resolved_interface_style(), InterfaceStyle::Classic);
        }
        let config: PaneFlowConfig =
            serde_json::from_str(r#"{"interface_style": "brutalist"}"#).unwrap();
        assert_eq!(config.resolved_interface_style(), InterfaceStyle::Classic);
        let config: PaneFlowConfig = serde_json::from_str(r#"{"interface_style": 2}"#).unwrap();
        assert_eq!(config.resolved_interface_style(), InterfaceStyle::Classic);
    }

    #[test]
    fn the_former_name_of_the_rounded_style_still_selects_it() {
        let config: PaneFlowConfig =
            serde_json::from_str(r#"{"interface_style": "paneflow"}"#).unwrap();
        assert_eq!(config.resolved_interface_style(), InterfaceStyle::Rounded);
        assert_eq!(
            serde_json::to_value(InterfaceStyle::Rounded).unwrap(),
            serde_json::json!("rounded")
        );
    }

    #[test]
    fn every_interface_style_round_trips_through_its_wire_name() {
        for &style in InterfaceStyle::ALL {
            let json = serde_json::json!({ "interface_style": style.wire_str() });
            let config: PaneFlowConfig = serde_json::from_value(json).unwrap();
            assert_eq!(config.interface_style, Some(style));
            assert_eq!(
                serde_json::to_value(style).unwrap(),
                serde_json::Value::String(style.wire_str().to_string())
            );
        }
    }

    #[test]
    fn app_icon_defaults_to_glass_and_tolerates_garbage() {
        let config: PaneFlowConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.resolved_app_icon(), AppIcon::Glass);
        let config: PaneFlowConfig = serde_json::from_str(r#"{"app_icon": "black"}"#).unwrap();
        assert_eq!(config.resolved_app_icon(), AppIcon::Black);
        let config: PaneFlowConfig =
            serde_json::from_str(r#"{"app_icon": "white-solid"}"#).unwrap();
        assert_eq!(config.resolved_app_icon(), AppIcon::WhiteSolid);
        let config: PaneFlowConfig =
            serde_json::from_str(r#"{"app_icon": "midnight-matte"}"#).unwrap();
        assert_eq!(config.resolved_app_icon(), AppIcon::MidnightMatte);
        let config: PaneFlowConfig = serde_json::from_str(r#"{"app_icon": "plaid"}"#).unwrap();
        assert_eq!(config.resolved_app_icon(), AppIcon::Glass);
        let config: PaneFlowConfig = serde_json::from_str(r#"{"app_icon": 2}"#).unwrap();
        assert_eq!(config.resolved_app_icon(), AppIcon::Glass);
    }

    #[test]
    fn every_app_icon_round_trips_through_its_wire_name() {
        for &icon in AppIcon::ALL {
            let json = serde_json::json!({ "app_icon": icon.wire_str() });
            let config: PaneFlowConfig = serde_json::from_value(json).unwrap();
            assert_eq!(config.app_icon, Some(icon));
            assert_eq!(
                serde_json::to_value(icon).unwrap(),
                serde_json::Value::String(icon.wire_str().to_string())
            );
        }
    }

    #[test]
    fn sidebar_group_by_defaults_to_folder_and_tolerates_garbage() {
        let config: PaneFlowConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.resolved_sidebar_group_by(), SidebarGroupBy::Folder);
        let config: PaneFlowConfig =
            serde_json::from_str(r#"{"sidebar_group_by": "status"}"#).unwrap();
        assert_eq!(config.resolved_sidebar_group_by(), SidebarGroupBy::Status);
        let config: PaneFlowConfig =
            serde_json::from_str(r#"{"theme": "One Dark", "sidebar_group_by": "date"}"#).unwrap();
        assert_eq!(config.resolved_sidebar_group_by(), SidebarGroupBy::Folder);
        assert_eq!(config.theme.as_deref(), Some("One Dark"));
        let config: PaneFlowConfig = serde_json::from_str(r#"{"sidebar_group_by": 1}"#).unwrap();
        assert_eq!(config.resolved_sidebar_group_by(), SidebarGroupBy::Folder);
    }

    #[test]
    fn workspace_icon_defaults_to_the_paneflow_folder_and_tolerates_garbage() {
        let config: PaneFlowConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.resolved_workspace_icon(), WorkspaceIcon::Default);
        let config: PaneFlowConfig =
            serde_json::from_str(r#"{"workspace_icon": "windows95"}"#).unwrap();
        assert_eq!(config.resolved_workspace_icon(), WorkspaceIcon::Windows95);
        let config: PaneFlowConfig =
            serde_json::from_str(r#"{"theme": "One Dark", "workspace_icon": "vista"}"#).unwrap();
        assert_eq!(config.resolved_workspace_icon(), WorkspaceIcon::Default);
        assert_eq!(config.theme.as_deref(), Some("One Dark"));
        let config: PaneFlowConfig = serde_json::from_str(r#"{"workspace_icon": 95}"#).unwrap();
        assert_eq!(config.resolved_workspace_icon(), WorkspaceIcon::Default);
    }

    #[test]
    fn every_workspace_icon_round_trips_through_its_wire_name() {
        for &icon in WorkspaceIcon::ALL {
            let json = serde_json::json!({ "workspace_icon": icon.wire_str() });
            let config: PaneFlowConfig = serde_json::from_value(json).unwrap();
            assert_eq!(config.workspace_icon, Some(icon));
            assert_eq!(
                serde_json::to_value(icon).unwrap(),
                serde_json::Value::String(icon.wire_str().to_string())
            );
        }
    }

    #[test]
    fn every_sidebar_grouping_round_trips_through_its_wire_name() {
        for &group_by in SidebarGroupBy::ALL {
            let json = serde_json::json!({ "sidebar_group_by": group_by.wire_str() });
            let config: PaneFlowConfig = serde_json::from_value(json).unwrap();
            assert_eq!(config.sidebar_group_by, Some(group_by));
            assert_eq!(
                serde_json::to_value(group_by).unwrap(),
                serde_json::Value::String(group_by.wire_str().to_string())
            );
        }
    }

    fn config(json: serde_json::Value) -> PaneFlowConfig {
        serde_json::from_value(json).unwrap()
    }

    const PLATFORMS: [HostPlatform; 4] = [
        HostPlatform::Windows,
        HostPlatform::Macos,
        HostPlatform::Linux,
        HostPlatform::Other,
    ];

    fn platform_material_keys(platform: HostPlatform) -> &'static [&'static str] {
        match platform {
            HostPlatform::Windows => &["windows_chrome_material", "windows_terminal_material"],
            HostPlatform::Macos => &["macos_chrome_material"],
            HostPlatform::Linux => &["linux_chrome_material", "linux_terminal_material"],
            HostPlatform::Other => &[],
        }
    }

    const ALL_MATERIAL_KEYS: [&str; 5] = [
        "windows_chrome_material",
        "windows_terminal_material",
        "macos_chrome_material",
        "linux_chrome_material",
        "linux_terminal_material",
    ];

    fn resolved_for(config: &PaneFlowConfig, platform: HostPlatform) -> BackgroundStyle {
        config
            .background_style
            .unwrap_or_else(|| config.legacy_background_style(platform))
    }

    #[test]
    fn an_explicit_background_style_wins_over_every_legacy_key() {
        let mut legacy = serde_json::Map::new();
        for key in ALL_MATERIAL_KEYS {
            legacy.insert(key.to_string(), serde_json::json!(false));
        }
        legacy.insert("window_backdrop".into(), serde_json::json!("opaque"));
        legacy.insert("windows_material_veil".into(), serde_json::json!(false));
        legacy.insert("background_style".into(), serde_json::json!("blended"));
        let blended = config(serde_json::Value::Object(legacy.clone()));
        assert_eq!(
            blended.resolved_background_style(),
            BackgroundStyle::Blended
        );
        for platform in PLATFORMS {
            assert_eq!(resolved_for(&blended, platform), BackgroundStyle::Blended);
        }

        for key in ALL_MATERIAL_KEYS {
            legacy.insert(key.to_string(), serde_json::json!(true));
        }
        legacy.insert("window_backdrop".into(), serde_json::json!("blurred"));
        legacy.insert("background_style".into(), serde_json::json!("themed"));
        let themed = config(serde_json::Value::Object(legacy));
        assert_eq!(themed.resolved_background_style(), BackgroundStyle::Themed);
    }

    #[test]
    fn any_material_key_of_the_running_platform_resolves_to_blended() {
        for platform in PLATFORMS {
            for &key in platform_material_keys(platform) {
                let legacy = config(serde_json::json!({ key: true }));
                assert_eq!(
                    resolved_for(&legacy, platform),
                    BackgroundStyle::Blended,
                    "{platform:?} {key}"
                );
                let with_backdrop =
                    config(serde_json::json!({ key: true, "window_backdrop": "auto" }));
                assert_eq!(
                    resolved_for(&with_backdrop, platform),
                    BackgroundStyle::Blended
                );
            }
        }
        let mica = config(serde_json::json!({
            "windows_chrome_material": true,
            "windows_terminal_material": true,
            "windows_material_veil": false,
        }));
        assert_eq!(
            resolved_for(&mica, HostPlatform::Windows),
            BackgroundStyle::Blended
        );
        let custom = config(serde_json::json!({
            "linux_chrome_material": true,
            "linux_terminal_material": false,
        }));
        assert_eq!(
            resolved_for(&custom, HostPlatform::Linux),
            BackgroundStyle::Blended
        );
    }

    #[test]
    fn an_opaque_or_off_backdrop_keeps_the_legacy_config_themed() {
        let mut materials = serde_json::Map::new();
        for key in ALL_MATERIAL_KEYS {
            materials.insert(key.to_string(), serde_json::json!(true));
        }
        for backdrop in ["opaque", "off", " OFF ", "Opaque"] {
            let mut json = materials.clone();
            json.insert("window_backdrop".into(), serde_json::json!(backdrop));
            let legacy = config(serde_json::Value::Object(json));
            for platform in PLATFORMS {
                assert_eq!(
                    resolved_for(&legacy, platform),
                    BackgroundStyle::Themed,
                    "{platform:?} {backdrop}"
                );
            }
        }
        let mut json = materials;
        json.insert("window_backdrop".into(), serde_json::json!("transparent"));
        let transparent = config(serde_json::Value::Object(json));
        assert_eq!(
            resolved_for(&transparent, HostPlatform::Macos),
            BackgroundStyle::Themed
        );
        assert_eq!(
            resolved_for(&transparent, HostPlatform::Linux),
            BackgroundStyle::Blended
        );
    }

    #[test]
    fn a_config_without_any_backdrop_key_is_themed() {
        let empty = config(serde_json::json!({}));
        assert_eq!(empty.resolved_background_style(), BackgroundStyle::Themed);
        for platform in PLATFORMS {
            assert_eq!(resolved_for(&empty, platform), BackgroundStyle::Themed);
        }
        assert_eq!(
            PaneFlowConfig::default().resolved_background_style(),
            BackgroundStyle::Themed
        );
    }

    #[test]
    fn material_keys_of_another_platform_only_stay_themed() {
        for platform in PLATFORMS {
            let mut json = serde_json::Map::new();
            for other in PLATFORMS.into_iter().filter(|other| *other != platform) {
                for &key in platform_material_keys(other) {
                    json.insert(key.to_string(), serde_json::json!(true));
                }
            }
            json.insert("windows_material_veil".into(), serde_json::json!(true));
            let foreign = config(serde_json::Value::Object(json));
            assert_eq!(
                resolved_for(&foreign, platform),
                BackgroundStyle::Themed,
                "{platform:?}"
            );
        }
    }

    #[test]
    fn an_unknown_background_style_string_loads_as_themed() {
        for value in ["mica", "acrylic", "", "blurred"] {
            let unknown = config(serde_json::json!({
                "background_style": value,
                "linux_chrome_material": true,
                "windows_chrome_material": true,
                "macos_chrome_material": true,
                "theme": "One Dark",
            }));
            assert_eq!(unknown.background_style, Some(BackgroundStyle::Themed));
            assert_eq!(unknown.resolved_background_style(), BackgroundStyle::Themed);
            assert_eq!(unknown.theme.as_deref(), Some("One Dark"));
        }
        let loose = config(serde_json::json!({ "background_style": " Blended " }));
        assert_eq!(loose.resolved_background_style(), BackgroundStyle::Blended);
    }

    #[test]
    fn a_non_string_background_style_loads_as_themed_and_keeps_the_rest_of_the_file() {
        for value in [
            serde_json::json!(3),
            serde_json::json!(true),
            serde_json::json!(["blended"]),
            serde_json::json!({"style": "blended"}),
        ] {
            let malformed = config(serde_json::json!({
                "background_style": value,
                "linux_chrome_material": true,
                "theme": "One Dark",
                "font_size": 15.0,
            }));
            assert_eq!(
                malformed.resolved_background_style(),
                BackgroundStyle::Themed
            );
            assert_eq!(malformed.theme.as_deref(), Some("One Dark"));
            assert_eq!(malformed.font_size, Some(15.0));
        }
        let null = config(serde_json::json!({
            "background_style": null,
            "linux_chrome_material": true,
        }));
        assert_eq!(null.background_style, None);
        assert_eq!(
            resolved_for(&null, HostPlatform::Linux),
            BackgroundStyle::Blended
        );
    }

    #[test]
    fn background_style_has_two_variants_on_every_platform() {
        assert_eq!(
            BackgroundStyle::ALL,
            &[BackgroundStyle::Themed, BackgroundStyle::Blended]
        );
        for &style in BackgroundStyle::ALL {
            match style {
                BackgroundStyle::Themed | BackgroundStyle::Blended => {}
            }
            let json = serde_json::json!({ "background_style": style.wire_str() });
            assert_eq!(config(json).background_style, Some(style));
            assert_eq!(
                serde_json::to_value(style).unwrap(),
                serde_json::Value::String(style.wire_str().to_string())
            );
        }
    }

    #[test]
    fn setting_a_background_style_clears_every_legacy_field() {
        let mut legacy = serde_json::Map::new();
        for key in PaneFlowConfig::LEGACY_BACKGROUND_STYLE_KEYS {
            legacy.insert(key.to_string(), serde_json::json!(true));
        }
        legacy.insert("window_backdrop".into(), serde_json::json!("blurred"));
        legacy.insert("theme".into(), serde_json::json!("One Dark"));
        let mut cfg = config(serde_json::Value::Object(legacy));
        cfg.set_background_style(BackgroundStyle::Themed);
        assert_eq!(cfg.resolved_background_style(), BackgroundStyle::Themed);
        let written = serde_json::to_value(&cfg).unwrap();
        for key in PaneFlowConfig::LEGACY_BACKGROUND_STYLE_KEYS {
            assert!(written[key].is_null(), "{key} survived: {written}");
        }
        assert_eq!(written["background_style"], serde_json::json!("themed"));
        assert_eq!(cfg.theme.as_deref(), Some("One Dark"));
    }

    #[test]
    fn the_derived_material_accessors_follow_the_resolved_style() {
        let blended = config(serde_json::json!({ "background_style": "blended" }));
        let themed = config(serde_json::json!({
            "background_style": "themed",
            "linux_chrome_material": true,
            "windows_chrome_material": true,
            "macos_chrome_material": true,
        }));
        assert!(!themed.cockpit_chrome_material_enabled());
        assert!(!themed.terminal_material_enabled());
        assert!(!themed.native_material_requested());
        assert!(!themed.macos_chrome_material_enabled());
        assert_eq!(
            blended.cockpit_chrome_material_enabled(),
            cfg!(any(
                target_os = "windows",
                target_os = "macos",
                target_os = "linux"
            ))
        );
        assert_eq!(
            blended.terminal_material_enabled(),
            cfg!(any(target_os = "windows", target_os = "linux"))
        );
        assert_eq!(
            blended.macos_chrome_material_enabled(),
            cfg!(target_os = "macos")
        );
        assert_eq!(
            blended.material_veiled(),
            cfg!(any(target_os = "windows", target_os = "linux"))
        );
    }
}
