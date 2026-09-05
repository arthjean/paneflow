#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

mod agent_launcher;
mod agent_sessions;
mod agents;
mod ai_hooks;
mod ai_types;
mod app;
mod assets;
#[cfg(test)]
mod bench_harness;
mod claude_session_registry;
mod claude_sessions;
mod cli;
mod codex_sessions;
mod command_sessions;
mod config_writer;
mod diff;
mod editor;
mod external_open;
mod file_icons;
mod fonts;
mod ipc;
mod ipc_events;
mod keybindings;
mod keys;
mod launch_cwd;
mod layout;
mod limits;
mod login_shell_env;
mod markdown;
mod opencode_sessions;
mod pane;
mod pane_drag;
mod pi_sessions;
mod pricing;
mod runtime_paths;
mod search;
mod settings;
mod sidebar_title;
mod system_info;
mod telemetry;
mod terminal;
pub mod theme;
mod ui_primitives;
mod update;
mod widgets;
mod window_chrome;
mod window_state;
mod windows_app_identity;
mod workspace;

use crate::window_chrome::title_bar;

use gpui::{
    Animation, AnimationExt, App, Context, CursorStyle, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, PathBuilder, Pixels, Point, Render, SharedString, Styled,
    Window, WindowBounds, WindowDecorations, WindowOptions, canvas, div, point, prelude::*, px,
};
use gpui_platform::application;
use notify::Watcher;

use crate::pane::{Pane, PaneSurface};
use crate::terminal::TerminalView;
use crate::workspace::Workspace;

pub use app::actions::*;
#[cfg(target_os = "macos")]
pub(crate) use app::bootstrap::{
    install_macos_menu_action_fallbacks, install_macos_menu_bar, warn_if_rosetta_translated,
};
pub(crate) use app::bootstrap::{system_package_update_command, warn_if_legacy_run_install};
pub(crate) use app::constants::{
    MAX_CLOSED_PANE_SCROLLBACK_BYTES, MAX_CLOSED_PANES, RESIZE_BORDER, SIDEBAR_WIDTH, TOAST_HOLD_MS,
};
pub(crate) use app::drag::{TabDrag, WorkspaceDrag, WorkspaceDragPreview};
pub(crate) use app::notifications::{Toast, ToastAction};

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SettingsSection {
    General,
    Appearance,
    Shortcuts,
    Terminal,
    AiAgent,
    McpServers,
    Workspaces,
}

impl SettingsSection {
    pub(crate) fn owns_its_scroll(self) -> bool {
        matches!(self, SettingsSection::Shortcuts)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ThemeMode {
    Light,
    Dark,
    System,
}

impl ThemeMode {
    pub(crate) fn from_config(mode: Option<&str>, theme_name: Option<&str>) -> Self {
        match mode.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("light") => Self::Light,
            Some("dark") => Self::Dark,
            Some("system") => Self::System,
            _ => Self::from_theme_name(theme_name.unwrap_or(crate::theme::DEFAULT_THEME)),
        }
    }

    pub(crate) fn from_theme_name(name: &str) -> Self {
        if crate::theme::theme_name_is_light(name) {
            Self::Light
        } else {
            Self::Dark
        }
    }

    pub(crate) fn as_config_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }

    pub(crate) fn resolved_theme_name(
        self,
        preset: &crate::theme::ThemePreset,
        appearance: gpui::WindowAppearance,
    ) -> &'static str {
        preset.variant(self.is_light(appearance))
    }

    fn is_light(self, appearance: gpui::WindowAppearance) -> bool {
        match self {
            Self::Light => true,
            Self::Dark => false,
            Self::System => Self::appearance_is_light(appearance),
        }
    }

    pub(crate) fn appearance_is_light(appearance: gpui::WindowAppearance) -> bool {
        matches!(
            appearance,
            gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum TerminalDropdown {
    CursorShape,
    CursorColor,
    FontWeight,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum GeneralDropdown {
    Editor,
    Shell,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum WorkspaceTemplateDropdown {
    Layout,
}

#[derive(Clone, Copy)]
pub(crate) struct WorkspaceContextMenu {
    pub(crate) idx: usize,
    pub(crate) position: Point<Pixels>,
}

#[derive(Clone, Copy)]
pub(crate) struct TabContextMenu {
    pub(crate) ws_idx: usize,
    pub(crate) tab_idx: usize,
    pub(crate) position: Point<Pixels>,
}

#[derive(Clone)]
pub(crate) struct PaneContextMenu {
    pub(crate) pane: Entity<Pane>,
    pub(crate) position: Point<Pixels>,
}

#[derive(Clone)]
pub(crate) struct FilesContextMenu {
    pub(crate) root: std::path::PathBuf,
    pub(crate) path: std::path::PathBuf,
    pub(crate) position: Point<Pixels>,
}

pub(crate) enum ClosedSurfaceRecord {
    Terminal {
        cwd: Option<std::path::PathBuf>,
        replay: Option<Vec<u8>>,
        custom_name: Option<String>,
        font_size: Option<f32>,
    },
    Markdown {
        path: std::path::PathBuf,
    },
}

pub(crate) struct ClosedPaneRecord {
    pub(crate) surface: ClosedSurfaceRecord,
    pub(crate) workspace_idx: usize,
}

struct SelfUpdateState {
    pending_update: update::checker::SharedUpdateSlot,
    update_status: Option<update::checker::UpdateStatus>,
    self_update_status: update::SelfUpdateStatus,
    install_method: update::install_method::InstallMethod,
    update_attempt_count: u32,
    download_generation: u64,
}

const PRIMARY_SIDEBAR_ANIMATION_MS: u64 = 280;
const PRIMARY_SIDEBAR_MIN_ANIMATION_DELTA: f32 = 0.5;
const STARTUP_SPLASH_TEXT_WIDTH: f32 = 198.;
const STARTUP_SPLASH_TEXT: [&str; 8] = ["P", "a", "n", "e", "f", "l", "o", "w"];
const STARTUP_SPLASH_LETTER_COUNT: f32 = STARTUP_SPLASH_TEXT.len() as f32;
const STARTUP_SPLASH_TEXT_ALPHA: f32 = 0.54;
const STARTUP_SPLASH_SHIMMER_ALPHA: f32 = 0.82;
const STARTUP_SPLASH_SHIMMER_MS: u64 = 2600;
const STARTUP_SPLASH_MIN_VISIBLE_MS: u64 = 900;

#[derive(Clone, Copy)]
struct SidebarWidthAnimation {
    from_width: f32,
    to_width: f32,
    started_at: std::time::Instant,
}

struct StartupSplashView {
    mount_scheduled: bool,
    native_material_active: bool,
}

impl StartupSplashView {
    fn new(_: &mut Context<Self>) -> Self {
        let config = paneflow_config::loader::load_config();
        Self {
            mount_scheduled: false,
            native_material_active: config.cockpit_chrome_material_enabled()
                || config.windows_terminal_material_enabled(),
        }
    }
}

fn native_backdrop_material_active(
    mode: paneflow_config::schema::AppMode,
    settings_open: bool,
    terminal_material_active: bool,
    chrome_material_active: bool,
) -> bool {
    chrome_material_active
        || (!settings_open
            && matches!(mode, paneflow_config::schema::AppMode::Cli)
            && terminal_material_active)
}

fn should_load_login_shell_env_for_startup(
    is_msi_relay: bool,
    is_mcp_subcommand: bool,
    is_cli_subcommand: bool,
    is_hooks_subcommand: bool,
    is_update_and_exit: bool,
    is_unknown_verb: bool,
) -> bool {
    !(is_msi_relay
        || is_mcp_subcommand
        || is_cli_subcommand
        || is_hooks_subcommand
        || is_update_and_exit
        || is_unknown_verb)
}

fn should_extract_mcp_bridge_for_cli(args: &[String]) -> bool {
    args.get(1).map(String::as_str) == Some("mcp")
        && args.get(2).map(String::as_str) == Some("install")
        && args.len() == 3
}

fn native_material_suppressed_by_fullscreen(is_fullscreen: bool) -> bool {
    cfg!(target_os = "macos") && is_fullscreen
}

#[cfg(test)]
mod native_material_tests {
    use super::{
        native_backdrop_material_active, native_material_suppressed_by_fullscreen,
        should_extract_mcp_bridge_for_cli, should_load_login_shell_env_for_startup,
    };
    use paneflow_config::schema::AppMode;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn fullscreen_suppresses_the_native_material_on_macos_only() {
        assert!(!native_material_suppressed_by_fullscreen(false));
        assert_eq!(
            native_material_suppressed_by_fullscreen(true),
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn terminal_material_can_activate_backdrop_without_chrome_material() {
        assert!(native_backdrop_material_active(
            AppMode::Cli,
            false,
            true,
            false
        ));
    }

    #[test]
    fn terminal_material_only_applies_to_visible_cli_terminal() {
        assert!(!native_backdrop_material_active(
            AppMode::Cli,
            true,
            true,
            false
        ));
        assert!(!native_backdrop_material_active(
            AppMode::Diff,
            false,
            true,
            false
        ));
    }

    #[test]
    fn chrome_material_activates_backdrop_independently() {
        assert!(native_backdrop_material_active(
            AppMode::Diff,
            true,
            false,
            true
        ));
    }

    #[test]
    fn login_shell_env_capture_only_runs_for_gui_launches() {
        assert!(should_load_login_shell_env_for_startup(
            false, false, false, false, false, false
        ));
        assert!(!should_load_login_shell_env_for_startup(
            false, true, false, false, false, false
        ));
        assert!(!should_load_login_shell_env_for_startup(
            false, false, true, false, false, false
        ));
        assert!(!should_load_login_shell_env_for_startup(
            false, false, false, true, false, false
        ));
        assert!(!should_load_login_shell_env_for_startup(
            false, false, false, false, true, false
        ));
        assert!(!should_load_login_shell_env_for_startup(
            false, false, false, false, false, true
        ));
    }

    #[test]
    fn mcp_bridge_extraction_only_runs_for_exact_install_command() {
        assert!(should_extract_mcp_bridge_for_cli(&args(&[
            "paneflow", "mcp", "install"
        ])));
        assert!(!should_extract_mcp_bridge_for_cli(&args(&[
            "paneflow", "mcp", "status"
        ])));
        assert!(!should_extract_mcp_bridge_for_cli(&args(&[
            "paneflow",
            "mcp",
            "uninstall"
        ])));
        assert!(!should_extract_mcp_bridge_for_cli(&args(&[
            "paneflow", "mcp", "install", "--help"
        ])));
    }
}

#[derive(Clone, Copy)]
enum PanelCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

fn panel_corner_mask(corner: PanelCorner, background: gpui::Hsla) -> impl IntoElement {
    const KAPPA: f32 = 0.552_284_8;

    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let left = bounds.left();
            let right = bounds.right();
            let top = bounds.top();
            let bottom = bounds.bottom();
            let radius = bounds.size.width.min(bounds.size.height);
            let k = radius * KAPPA;

            let mut builder = PathBuilder::fill();
            match corner {
                PanelCorner::TopLeft => {
                    builder.move_to(point(left, top));
                    builder.line_to(point(right, top));
                    builder.cubic_bezier_to(
                        point(left, bottom),
                        point(right - k, top),
                        point(left, bottom - k),
                    );
                    builder.line_to(point(left, top));
                }
                PanelCorner::TopRight => {
                    builder.move_to(point(left, top));
                    builder.line_to(point(right, top));
                    builder.line_to(point(right, bottom));
                    builder.cubic_bezier_to(
                        point(left, top),
                        point(right, bottom - k),
                        point(left + k, top),
                    );
                }
                PanelCorner::BottomLeft => {
                    builder.move_to(point(left, bottom));
                    builder.line_to(point(right, bottom));
                    builder.cubic_bezier_to(
                        point(left, top),
                        point(right - k, bottom),
                        point(left, top + k),
                    );
                    builder.line_to(point(left, bottom));
                }
                PanelCorner::BottomRight => {
                    builder.move_to(point(left, bottom));
                    builder.line_to(point(right, bottom));
                    builder.line_to(point(right, top));
                    builder.cubic_bezier_to(
                        point(left, bottom),
                        point(right, top + k),
                        point(left + k, bottom),
                    );
                }
            }
            builder.close();

            if let Ok(path) = builder.build() {
                window.paint_path(path, background);
            }
        },
    )
    .size_full()
}

fn startup_splash_letter(
    label: &'static str,
    index: usize,
    base_color: gpui::Hsla,
) -> gpui::AnyElement {
    div()
        .text_size(px(34.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(base_color)
        .child(label)
        .with_animation(
            SharedString::from(format!("startup-splash-shimmer-letter-{index}")),
            Animation::new(std::time::Duration::from_millis(STARTUP_SPLASH_SHIMMER_MS)).repeat(),
            move |letter, delta| {
                let color = startup_splash_shimmer_color(base_color, index, delta);
                letter.text_color(color)
            },
        )
        .into_any_element()
}

fn startup_splash_shimmer_color(base_color: gpui::Hsla, index: usize, delta: f32) -> gpui::Hsla {
    let active_delta = if delta < 0.78 {
        delta / 0.78
    } else {
        return base_color;
    };
    let center = -1.8 + active_delta * (STARTUP_SPLASH_LETTER_COUNT + 3.6);
    let distance = (index as f32 - center).abs();
    let sigma = 0.86;
    let strength = (-(distance * distance) / (2. * sigma * sigma)).exp();
    let lightness = (base_color.l + (1. - base_color.l) * strength * 0.86).min(0.97);
    let saturation = base_color.s * (1. - strength * 0.85).max(0.);
    let alpha = base_color.a + (STARTUP_SPLASH_SHIMMER_ALPHA - base_color.a) * strength;

    gpui::hsla(base_color.h, saturation, lightness, alpha)
}

impl Render for StartupSplashView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.mount_scheduled {
            self.mount_scheduled = true;
            cx.spawn_in(window, async move |_, cx| {
                smol::Timer::after(std::time::Duration::from_millis(
                    STARTUP_SPLASH_MIN_VISIBLE_MS,
                ))
                .await;
                let _ = cx.update(|window, cx| {
                    mount_paneflow_app(window, cx);
                });
            })
            .detach();
        }

        let ui = crate::theme::ui_colors();
        let splash_text_color = gpui::Hsla {
            a: STARTUP_SPLASH_TEXT_ALPHA,
            ..ui.muted
        };
        let theme = crate::theme::active_theme();
        let is_window_active = window.is_window_active();
        let shell_color = if is_window_active {
            theme.title_bar_background
        } else {
            theme.title_bar_inactive_background
        };
        let background = crate::app::constants::cockpit_backdrop_background(
            shell_color,
            is_window_active,
            self.native_material_active,
        );
        let content = div()
            .font_family("Geist")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w(px(STARTUP_SPLASH_TEXT_WIDTH))
                    .h(px(58.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(
                        STARTUP_SPLASH_TEXT
                            .iter()
                            .enumerate()
                            .map(|(index, label)| {
                                startup_splash_letter(label, index, splash_text_color)
                            }),
                    ),
            );
        crate::window_chrome::csd::client_side_window_shell(content, window, background, ui.border)
    }
}

impl SidebarWidthAnimation {
    fn width_at(self, now: std::time::Instant) -> f32 {
        let duration = std::time::Duration::from_millis(PRIMARY_SIDEBAR_ANIMATION_MS);
        let progress = (now.duration_since(self.started_at).as_secs_f32() / duration.as_secs_f32())
            .clamp(0., 1.);
        let eased = 1. - (1. - progress).powi(3);
        self.from_width + (self.to_width - self.from_width) * eased
    }

    fn is_finished(self, now: std::time::Instant) -> bool {
        now.duration_since(self.started_at)
            >= std::time::Duration::from_millis(PRIMARY_SIDEBAR_ANIMATION_MS)
    }
}

struct AgentSessionsState {
    sessions_sidebar_open: bool,
    sessions_sidebar_animation: Option<SidebarWidthAnimation>,
    sessions_by_agent: [Vec<agent_sessions::SessionMeta>; agent_sessions::SESSION_AGENT_COUNT],
    sessions_omitted: [usize; agent_sessions::SESSION_AGENT_COUNT],
    sessions_cwd: Option<String>,
    sessions_surface_id: Option<u64>,
    sessions_scroll: gpui::ScrollHandle,
    sessions_scan_generation: u64,
    sessions_selected: usize,
    sessions_focus: FocusHandle,
    sessions_group_collapsed: [bool; agent_sessions::SESSION_AGENT_COUNT],
    sessions_group_show_all: [bool; agent_sessions::SESSION_AGENT_COUNT],
    sessions_scanning: [bool; agent_sessions::SESSION_AGENT_COUNT],
}

struct DiffModeState {
    diff_view: Option<gpui::Entity<crate::diff::DiffView>>,
    multi_diff_view: Option<gpui::Entity<crate::diff::MultiRepoDiffView>>,
    diff_view_cache: std::collections::HashMap<
        crate::app::diff_view_actions::DiffViewKey,
        gpui::Entity<crate::diff::DiffView>,
    >,
    diff_view_key: Option<crate::app::diff_view_actions::DiffViewKey>,
    multi_diff_view_retained: Option<(u64, gpui::Entity<crate::diff::MultiRepoDiffView>)>,
    diff_collapsed_branches: std::collections::HashSet<String>,
    diff_discovering: bool,
    diff_discovering_root: Option<std::path::PathBuf>,
    diff_chosen_worktrees:
        std::collections::HashMap<std::path::PathBuf, std::collections::HashSet<String>>,
    diff_worktree_picker_open: bool,
    diff_available_worktrees: Vec<crate::diff::DiffWorktree>,
    diff_available_repo: Option<std::path::PathBuf>,
    diff_scope: crate::diff::DiffScope,
    diff_scope_picker_open: bool,
    diff_project_picker_open: bool,
    diff_selected_file: Option<String>,
    diff_files_collapsed: bool,
    diff_files_tree: bool,
    diff_collapsed_dirs: std::collections::HashSet<String>,
    diff_file_filter: gpui::Entity<crate::widgets::text_input::TextInput>,
}

struct DiffDockState {
    pub(crate) open: bool,
    pub(crate) data: Option<crate::app::diff_dock::DiffDockData>,
    pub(crate) collapsed: std::collections::HashSet<String>,
    pub(crate) expanded_folds: std::collections::HashSet<String>,
    pub(crate) split: bool,
    pub(crate) generation: u64,
    pub(crate) scroll: gpui::ScrollHandle,
    pub(crate) diff_options_menu_open: bool,
    pub(crate) diff_options_submenu: Option<crate::app::diff_dock::DiffOptionsSubmenu>,
    pub(crate) diff_options: crate::diff::DiffOptions,
    pub(crate) diff_new_tab_menu_open: bool,
    pub(crate) picker: bool,
    pub(crate) picked: bool,
    pub(crate) owner: Option<u64>,
    pub(crate) parked: std::collections::HashMap<u64, crate::app::cli_diff_dock::DiffDockSlot>,
    pub(crate) diff_tabs: Vec<crate::app::diff_dock::DiffDockTab>,
    pub(crate) diff_active_tab: usize,
    pub(crate) diff_tab_close_armed: Option<usize>,
    pub(crate) diff_branch_menu: Option<crate::app::diff_dock::DiffBranchMenuState>,
    pub(crate) width: f32,
    pub(crate) resize: Option<(f32, f32, f32)>,
    pub(crate) h_scroll_drag: Option<crate::app::diff_dock::DiffDockHScrollDrag>,
    pub(crate) vertical_scrollbar: crate::widgets::editor_scrollbar::EditorScrollbar,
    pub(crate) h_offsets: std::rc::Rc<Vec<f32>>,
    pub(crate) hover: Option<crate::app::diff_dock::DiffHover>,
}

struct PaneFlowApp {
    workspaces: Vec<Workspace>,
    active_idx: usize,
    renaming_tab: Option<(usize, usize)>,
    rename_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    rename_focus_live: bool,
    pending_config:
        std::sync::Arc<std::sync::Mutex<Option<paneflow_config::schema::PaneFlowConfig>>>,
    save_seq: std::sync::Arc<std::sync::atomic::AtomicU64>,
    cached_config: paneflow_config::schema::PaneFlowConfig,
    ipc_rx: std::sync::mpsc::Receiver<ipc::IpcRequest>,
    ipc_status: ipc::IpcStatus,
    event_bus: std::sync::Arc<ipc_events::EventBus>,
    last_broadcast_gen: std::collections::HashMap<u64, u64>,
    title_bar: Entity<title_bar::TitleBar>,
    primary_sidebar_visible: bool,
    primary_sidebar_animation: Option<SidebarWidthAnimation>,
    title_bar_files_menu_open: Option<Point<Pixels>>,
    title_bar_help_menu_open: Option<Point<Pixels>>,
    git_watcher: Option<notify::RecommendedWatcher>,
    git_event_rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    git_watch_counts: std::collections::HashMap<std::path::PathBuf, usize>,
    claude_registry_seen: crate::app::agent_status::RegistryWatermark,
    settings_section: Option<SettingsSection>,
    settings_scroll: gpui::ScrollHandle,
    settings_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
    settings_search_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    terminal_dropdown: Option<TerminalDropdown>,
    general_dropdown: Option<GeneralDropdown>,
    workspace_template_dropdown: Option<WorkspaceTemplateDropdown>,
    workspace_template_selected: Option<usize>,
    workspace_template_detail_open: bool,
    workspace_template_selected_pane: usize,
    workspace_template_status: Option<String>,
    workspace_template_name_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    workspace_template_project_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    workspace_pane_name_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    workspace_pane_cwd_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    workspace_pane_command_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    workspace_pane_prompt_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    mcp_status: Option<Vec<paneflow_mcp_install::StatusReport>>,
    mcp_install: Option<Result<Vec<paneflow_mcp_install::InstallReport>, String>>,
    mcp_busy: bool,
    sidebar_scroll: gpui::ScrollHandle,
    effective_shortcuts: Vec<keybindings::ShortcutEntry>,
    recording_shortcut_idx: Option<usize>,
    shortcut_search_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    shortcut_capture_active: bool,
    shortcut_reset_pending: bool,
    collapsed_shortcut_groups: std::collections::HashSet<keybindings::ShortcutGroup>,
    shortcut_rows: Vec<crate::settings::tabs::shortcuts::ShortcutListRow>,
    shortcut_list: gpui::ListState,
    shortcut_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
    settings_focus: FocusHandle,
    mono_font_names: Vec<String>,
    font_dropdown_open: bool,
    font_search: String,
    theme_dropdown_open: bool,
    theme_mode: ThemeMode,
    workspace_menu_open: Option<WorkspaceContextMenu>,
    pub(crate) worktree_states: crate::app::tab_worktree::WorktreeStates,
    pub(crate) branch_checkout_pending: Option<String>,
    pub(crate) pr_states: crate::app::pull_request::PrStates,
    pub(crate) sidebar_customize_menu_open: bool,
    pub(crate) sidebar_show_submenu_open: bool,
    tab_menu_open: Option<TabContextMenu>,
    pane_menu_open: Option<PaneContextMenu>,
    pending_pane_focus: Option<Entity<Pane>>,
    profile_menu_open: Option<Point<Pixels>>,
    agent_sessions: AgentSessionsState,
    files_sidebar_open: bool,
    files_sidebar_animation: Option<SidebarWidthAnimation>,
    files_sidebar: Entity<app::files_sidebar::FilesSidebar>,
    files_sidebar_root: Option<std::path::PathBuf>,
    files_sidebar_workspace: Option<u64>,
    files_menu_open: Option<FilesContextMenu>,
    toast: Option<Toast>,
    toast_queue: std::collections::VecDeque<Toast>,
    _toast_task: Option<gpui::Task<()>>,
    #[cfg(target_os = "windows")]
    windows_backdrop_light: Option<bool>,
    jump_cursor: Option<u64>,
    swap_source: Option<Entity<crate::pane::Pane>>,
    closed_panes: Vec<ClosedPaneRecord>,
    show_about_dialog: bool,
    system_info_dialog: Option<crate::app::system_info_dialog::SystemInfoDialog>,
    show_theme_picker: bool,
    theme_picker_query: String,
    theme_picker_selected_idx: usize,
    theme_picker_focus: FocusHandle,
    theme_picker_scroll: gpui::ScrollHandle,
    theme_picker_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
    composer: Option<app::composer::ComposerState>,
    broadcast: app::broadcast::BroadcastState,
    broadcast_picker_open: bool,
    broadcast_picker_query: String,
    broadcast_picker_selected: usize,
    broadcast_picker_renaming: Option<usize>,
    broadcast_picker_error: Option<String>,
    broadcast_picker_focus: FocusHandle,
    attention_queue_open: bool,
    attention_queue_selected: usize,
    attention_queue_focus: FocusHandle,
    fleet_search: Option<app::fleet_search::FleetSearchState>,
    fleet_search_generation: u64,
    fleet_search_focus: FocusHandle,
    fleet_search_pending_focus: bool,
    launch_pad: Option<app::launch_pad::LaunchPadState>,
    launch_pad_focus: FocusHandle,
    pane_palette: Option<app::pane_palette::PanePaletteState>,
    pane_palette_focus: FocusHandle,
    pending_palette_focus: bool,
    self_update: SelfUpdateState,
    custom_buttons_modal: Option<app::custom_buttons_modal::CustomButtonsModal>,
    custom_buttons_modal_focus: FocusHandle,
    telemetry: std::sync::Arc<crate::telemetry::client::TelemetryClient>,
    launch_instant: std::time::Instant,
    telemetry_enabled_last: Option<bool>,
    theme_changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    diff_mode: DiffModeState,
    pub(crate) mode: paneflow_config::schema::AppMode,
    pub(crate) diff_dock: DiffDockState,
    pub(crate) sidebar_order_cache: std::cell::RefCell<crate::app::sidebar::SidebarOrderCache>,
}

pub static SWAP_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl PaneFlowApp {
    fn primary_sidebar_expanded_width(&self) -> f32 {
        if self.settings_section.is_some() {
            crate::settings::chrome::SETTINGS_NAV_WIDTH
        } else {
            match self.mode {
                paneflow_config::schema::AppMode::Diff => {
                    crate::app::diff_view_actions::DIFF_SIDEBAR_WIDTH
                }
                paneflow_config::schema::AppMode::Cli => SIDEBAR_WIDTH,
            }
        }
    }

    fn primary_sidebar_width_at(&self, now: std::time::Instant) -> f32 {
        if self.settings_section.is_some() {
            return crate::settings::chrome::SETTINGS_NAV_WIDTH;
        }
        if let Some(animation) = self.primary_sidebar_animation {
            animation.width_at(now)
        } else if self.primary_sidebar_visible {
            self.primary_sidebar_expanded_width()
        } else {
            0.
        }
    }

    fn rendered_primary_sidebar_width(&mut self, window: &mut Window) -> f32 {
        if self.settings_section.is_some() {
            self.primary_sidebar_animation = None;
            return crate::settings::chrome::SETTINGS_NAV_WIDTH;
        }

        let now = std::time::Instant::now();
        if let Some(animation) = self.primary_sidebar_animation {
            if animation.is_finished(now) {
                self.primary_sidebar_animation = None;
                animation.to_width
            } else {
                window.request_animation_frame();
                animation.width_at(now)
            }
        } else if self.primary_sidebar_visible {
            self.primary_sidebar_expanded_width()
        } else {
            0.
        }
    }

    pub(crate) fn toggle_primary_sidebar(&mut self, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let from_width = self.primary_sidebar_width_at(now);
        self.primary_sidebar_visible = !self.primary_sidebar_visible;

        if self.settings_section.is_some() {
            self.primary_sidebar_animation = None;
            cx.notify();
            return;
        }

        let to_width = if self.primary_sidebar_visible {
            self.primary_sidebar_expanded_width()
        } else {
            0.
        };

        self.primary_sidebar_animation = if !crate::ui_primitives::reduce_motion()
            && (from_width - to_width).abs() > PRIMARY_SIDEBAR_MIN_ANIMATION_DELTA
        {
            Some(SidebarWidthAnimation {
                from_width,
                to_width,
                started_at: now,
            })
        } else {
            None
        };
        cx.notify();
    }

    fn watch_git_dir(&mut self, ws: &Workspace) {
        if let Some(ref git_dir) = ws.git_dir {
            let current = self.git_watch_counts.get(git_dir).copied().unwrap_or(0);
            if current == 0
                && let Some(ref mut watcher) = self.git_watcher
                && let Err(e) = watcher.watch(git_dir, notify::RecursiveMode::NonRecursive)
            {
                log::warn!("git watcher: failed to watch {}: {e}", git_dir.display());
                return;
            }
            *self.git_watch_counts.entry(git_dir.clone()).or_insert(0) += 1;
        }
    }

    fn unwatch_git_dir(&mut self, git_dir: &std::path::Path) {
        if let Some(count) = self.git_watch_counts.get_mut(git_dir) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.git_watch_counts.remove(git_dir);
                if let Some(ref mut watcher) = self.git_watcher {
                    let _ = watcher.unwatch(git_dir);
                }
            }
        }
    }

    fn create_pane(
        &mut self,
        terminal: Entity<TerminalView>,
        workspace_id: u64,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        cx.subscribe(&terminal, Self::handle_terminal_event)
            .detach();
        let pane = cx.new(|cx| Pane::new(terminal, workspace_id, cx));
        cx.subscribe(&pane, Self::handle_pane_event).detach();
        pane
    }

    pub(crate) fn create_pane_with_existing_surface(
        &mut self,
        surface: PaneSurface,
        workspace_id: u64,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        let pane = cx.new(|cx| Pane::new_with_surface(surface, workspace_id, cx));
        cx.subscribe(&pane, Self::handle_pane_event).detach();
        pane
    }

    pub(crate) fn record_update_failure(
        &mut self,
        context: &str,
        err: &anyhow::Error,
        cx: &mut Context<Self>,
    ) {
        log::error!("self-update/{context}: {err:#}");
        let tag = update::UpdateError::classify(err);
        self.emit_update_failure(&tag);
        self.self_update.self_update_status = update::SelfUpdateStatus::Errored(tag.clone());
        self.self_update.update_attempt_count =
            self.self_update.update_attempt_count.saturating_add(1);
        self.show_update_error_toast(&tag, cx);
        cx.notify();
    }
}

impl Render for PaneFlowApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();
        #[cfg(target_os = "windows")]
        {
            let is_light = theme.background.l > 0.5;
            if self.windows_backdrop_light != Some(is_light) {
                crate::window_chrome::backdrop::sync_wallpaper_mica_theme(window, is_light);
                self.windows_backdrop_light = Some(is_light);
            }
        }
        let chrome_material_suppressed =
            native_material_suppressed_by_fullscreen(window.is_fullscreen());
        #[cfg(target_os = "macos")]
        crate::window_chrome::macos_backdrop::sync_subtle_sidebar_material(
            theme.background.l > 0.5,
            self.cached_config.macos_chrome_material_enabled() && !chrome_material_suppressed,
        );
        let title_bar_h =
            (1.75 * window.rem_size()).max(crate::app::constants::TITLE_BAR_MIN_HEIGHT);
        let settings_open = self.settings_section.is_some();
        let sessions_sidebar_width = self.rendered_sessions_sidebar_width(window);
        let sessions_sidebar_mounted = self.agent_sessions.sessions_sidebar_open
            || self.agent_sessions.sessions_sidebar_animation.is_some();
        let sessions_sidebar_opacity = (sessions_sidebar_width
            / crate::app::sessions_sidebar::SESSIONS_SIDEBAR_WIDTH.max(1.))
        .clamp(0., 1.);
        let secondary_sidebar_open = sessions_sidebar_mounted;
        let right_rail_width = if sessions_sidebar_mounted {
            sessions_sidebar_width
        } else {
            0.
        };
        let terminal_material_active = self.cached_config.windows_terminal_material_enabled();
        let chrome_material_active =
            self.cached_config.cockpit_chrome_material_enabled() && !chrome_material_suppressed;
        let terminal_surface_mounted = self
            .active_workspace()
            .is_some_and(|ws| ws.active_tab().root.is_some());
        let terminal_material_visible = !settings_open
            && matches!(self.mode, paneflow_config::schema::AppMode::Cli)
            && terminal_surface_mounted
            && terminal_material_active;
        let native_material_active = native_backdrop_material_active(
            self.mode,
            settings_open,
            terminal_material_active,
            chrome_material_active,
        );
        let is_window_active = window.is_window_active();
        let shell_color = if is_window_active {
            theme.title_bar_background
        } else {
            theme.title_bar_inactive_background
        };
        let opaque_shell_bg = gpui::Hsla {
            a: 1.,
            ..shell_color
        };
        let app_backdrop_bg = crate::app::constants::cockpit_backdrop_background(
            shell_color,
            is_window_active,
            native_material_active,
        );
        let panel_bg = if settings_open {
            ui.base
        } else {
            match self.mode {
                paneflow_config::schema::AppMode::Cli => gpui::transparent_black(),
                paneflow_config::schema::AppMode::Diff => ui.base,
            }
        };
        let panel_corner_mask_bg = crate::app::constants::cockpit_backdrop_background(
            shell_color,
            is_window_active,
            chrome_material_active,
        );
        let panel_top = title_bar_h;
        let primary_sidebar_width = self.rendered_primary_sidebar_width(window);
        let title_bar_rail_width = self.primary_sidebar_expanded_width();
        let primary_sidebar_mounted = self.settings_section.is_some()
            || self.primary_sidebar_visible
            || self.primary_sidebar_animation.is_some();
        let primary_sidebar_opacity = if self.settings_section.is_some() {
            1.
        } else {
            (primary_sidebar_width / self.primary_sidebar_expanded_width().max(1.)).clamp(0., 1.)
        };
        let panel_edge_share = 1. - primary_sidebar_opacity;
        let main_panel_left_inset = crate::app::constants::PANEL_INSET * panel_edge_share;
        let pane_grid_left_gutter = crate::layout::PANE_GUTTER_PX * panel_edge_share;
        let main_panel_corner_mask_bg = panel_corner_mask_bg;
        let main_panel_width = f32::from(window.viewport_size().width)
            - primary_sidebar_width
            - right_rail_width
            - main_panel_left_inset
            - crate::app::constants::PANEL_INSET;
        #[cfg(target_os = "linux")]
        {
            crate::window_chrome::linux_backdrop::set_chrome_geometry(
                crate::window_chrome::linux_backdrop::ChromeGeometry {
                    left_sidebar_width: primary_sidebar_width,
                    right_sidebar_width: right_rail_width,
                    title_bar_height: f32::from(title_bar_h),
                    title_bar_spans_window: true,
                },
            );
            crate::window_chrome::linux_backdrop::refresh_blur_region(window);
        }

        if let Some(pane) = self.pending_pane_focus.take() {
            pane.read(cx).focus_handle(cx).focus(window, cx);
        }
        if std::mem::take(&mut self.pending_palette_focus) {
            window.focus(&self.pane_palette_focus, cx);
        }
        let rename_focus = self.rename_input.read(cx).focus_handle.clone();
        let rename_live = self.renaming_tab.is_some();
        if rename_live != self.rename_focus_live {
            self.rename_focus_live = rename_live;
            if rename_live {
                window.focus(&rename_focus, cx);
            } else if rename_focus.is_focused(window)
                && let Some(ws) = self.workspaces.get(self.active_idx)
            {
                ws.focus_first(window, cx);
            }
        } else if rename_live && !rename_focus.is_focused(window) {
            self.commit_rename(cx);
            self.rename_focus_live = false;
        }
        self.prune_stale_split_palette(cx);
        let main_content = if self.settings_section.is_some() {
            self.render_settings_content_panel(cx).into_any_element()
        } else if matches!(self.mode, paneflow_config::schema::AppMode::Diff) {
            self.render_diff_main(cx)
        } else if let Some(ws) = self.active_workspace() {
            if let Some(root) = &ws.active_tab().root {
                let app_weak = cx.weak_entity();
                let on_resize_end = std::rc::Rc::new(move |cx: &mut App| {
                    let _ = app_weak.update(cx, |app, cx| app.save_session(cx));
                });
                root.sync_unfocused_dim(window, cx);
                let outer = div()
                    .flex()
                    .size_full()
                    .pl(px(pane_grid_left_gutter))
                    .pr(px(crate::layout::PANE_GUTTER_PX))
                    .pt(px(crate::layout::PANE_GUTTER_PX))
                    .pb(px(crate::layout::PANE_GUTTER_PX));
                let preview = self.pending_split_palette().map(|(target, direction)| {
                    crate::layout::SplitPreview {
                        target,
                        direction,
                        element: std::cell::RefCell::new(Some(self.render_pane_palette(cx))),
                    }
                });
                outer
                    .child(root.render_with_preview(
                        window,
                        cx,
                        Some(on_resize_end),
                        preview.as_ref(),
                    ))
                    .into_any_element()
            } else if self
                .pane_palette
                .as_ref()
                .is_some_and(|palette| palette.ws_id == ws.id)
            {
                div()
                    .flex()
                    .size_full()
                    .pl(px(pane_grid_left_gutter))
                    .pr(px(crate::layout::PANE_GUTTER_PX))
                    .pt(px(crate::layout::PANE_GUTTER_PX))
                    .pb(px(crate::layout::PANE_GUTTER_PX))
                    .child(self.render_pane_palette(cx))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .child(div().text_color(ui.text).child("No terminal panes open"))
                    .into_any_element()
            }
        } else {
            div()
                .flex()
                .items_center()
                .justify_center()
                .size_full()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .text_center()
                        .gap(px(10.))
                        .w(px(460.))
                        .px(px(24.))
                        .child(
                            div()
                                .text_color(ui.text)
                                .text_size(px(20.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Welcome to PaneFlow"),
                        )
                        .child(
                            div()
                                .text_color(ui.muted)
                                .text_size(px(13.))
                                .child(
                                    "The next-generation IDE for the AI era - \
                                     a GPU-native terminal with workspace-aware panes, \
                                     live git status, and first-class support for Claude Code and Codex.",
                                ),
                        )
                        .child(
                            div()
                                .mt(px(6.))
                                .text_color(ui.muted)
                                .text_size(px(12.))
                                .child("Click + in the sidebar to create your first workspace."),
                        ),
                )
                .into_any_element()
        };
        let main_content = self.wrap_cli_diff_dock(main_content, main_panel_width, window, cx);
        let ws_name = if self.settings_section.is_some() {
            None
        } else {
            self.active_workspace().map(|ws| ws.title.clone())
        };
        let update_info = self.update_pill_info();
        self.title_bar.update(cx, |tb, _| {
            tb.workspace_name = ws_name;
            tb.sidebar_visible = self.primary_sidebar_visible;
            tb.left_rail_width = title_bar_rail_width;
            tb.files_menu_open = self.title_bar_files_menu_open.is_some();
            tb.help_menu_open = self.title_bar_help_menu_open.is_some();
            tb.update_available = update_info;
            tb.ipc_state = self.ipc_status.state();
            tb.cockpit = true;
            tb.cockpit_material_active = chrome_material_active;
        });

        let mut app_content = div()
            .font_family("Geist")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .cursor(CursorStyle::Arrow)
            .on_action(cx.listener(Self::handle_split_h))
            .on_action(cx.listener(Self::handle_split_v))
            .on_action(cx.listener(Self::handle_close_pane))
            .on_action(cx.listener(Self::handle_new_tab))
            .on_action(cx.listener(Self::handle_close_tab))
            .on_action(cx.listener(Self::handle_next_tab))
            .on_action(cx.listener(Self::handle_previous_tab))
            .on_action(cx.listener(Self::handle_focus_left))
            .on_action(cx.listener(Self::handle_focus_right))
            .on_action(cx.listener(Self::handle_focus_up))
            .on_action(cx.listener(Self::handle_focus_down))
            .on_action(cx.listener(Self::handle_jump_next_waiting))
            .on_action(cx.listener(Self::handle_new_workspace))
            .on_action(cx.listener(Self::handle_close_workspace))
            .on_action(cx.listener(Self::handle_copy_workspace_path))
            .on_action(cx.listener(Self::handle_reveal_workspace_in_file_manager))
            .on_action(cx.listener(Self::handle_open_workspace_in_zed))
            .on_action(cx.listener(Self::handle_open_workspace_in_cursor))
            .on_action(cx.listener(Self::handle_open_workspace_in_vscode))
            .on_action(cx.listener(Self::handle_open_workspace_in_windsurf))
            .on_action(cx.listener(Self::handle_next_workspace))
            .on_action(cx.listener(Self::handle_toggle_zoom))
            .on_action(cx.listener(Self::handle_layout_even_h))
            .on_action(cx.listener(Self::handle_layout_even_v))
            .on_action(cx.listener(Self::handle_layout_main_v))
            .on_action(cx.listener(Self::handle_layout_tiled))
            .on_action(cx.listener(Self::handle_split_equalize))
            .on_action(cx.listener(Self::handle_swap_pane))
            .on_action(cx.listener(Self::handle_undo_close_pane))
            .on_action(cx.listener(Self::handle_open_multi_diff))
            .on_action(cx.listener(Self::handle_open_diff_view))
            .on_action(cx.listener(Self::handle_ws1))
            .on_action(cx.listener(Self::handle_ws2))
            .on_action(cx.listener(Self::handle_ws3))
            .on_action(cx.listener(Self::handle_ws4))
            .on_action(cx.listener(Self::handle_ws5))
            .on_action(cx.listener(Self::handle_ws6))
            .on_action(cx.listener(Self::handle_ws7))
            .on_action(cx.listener(Self::handle_ws8))
            .on_action(cx.listener(Self::handle_ws9))
            .on_action(cx.listener(|this: &mut Self, _: &Quit, _window, cx| {
                this.save_session_blocking(cx);
                this.emit_app_exited_and_flush();
                cx.quit();
            }))
            .on_action(cx.listener(|this: &mut Self, _: &About, _window, cx| {
                this.show_about_dialog = true;
                cx.notify();
            }))
            .on_action(cx.listener(|_this: &mut Self, _: &Copy, _window, cx| {
                cx.dispatch_action(&TerminalCopy);
            }))
            .on_action(cx.listener(|_this: &mut Self, _: &Paste, _window, cx| {
                cx.dispatch_action(&TerminalPaste);
            }))
            .on_action(
                cx.listener(|_this: &mut Self, _: &SelectAll, _window, _cx| {
                    log::debug!("Edit > Select All dispatched (terminal select-all not yet wired)");
                }),
            )
            .on_action(
                cx.listener(|this: &mut Self, _: &ShowSystemInfo, window, cx| {
                    this.open_system_info_dialog(window, cx);
                }),
            )
            .on_action(cx.listener(|_this: &mut Self, _: &OpenHelp, _window, _cx| {
                if let Err(e) =
                    crate::external_open::open_url("https://github.com/arthjean/paneflow#readme")
                {
                    log::warn!("Help > PaneFlow Help: could not open browser: {e}");
                }
            }))
            .on_action(cx.listener(Self::handle_start_self_update))
            .on_action(cx.listener(Self::handle_dismiss_update))
            .on_action(cx.listener(Self::handle_toggle_files_sidebar))
            .on_action(cx.listener(Self::handle_open_composer))
            .on_action(cx.listener(Self::handle_toggle_broadcast_member))
            .on_action(cx.listener(Self::handle_open_broadcast_groups))
            .on_action(cx.listener(Self::handle_open_attention_queue))
            .on_action(cx.listener(Self::handle_open_launch_pad))
            .on_action(cx.listener(Self::handle_diff_new_file_tab))
            .on_action(cx.listener(Self::handle_diff_new_terminal_tab))
            .capture_key_down(cx.listener(|_this, e: &gpui::KeyDownEvent, window, cx| {
                if cx.has_active_drag() && e.keystroke.key == "escape" {
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_move(|_e, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .overflow_hidden()
                    .relative()
                    .when(
                        terminal_material_visible && primary_sidebar_mounted,
                        |row| {
                            row.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(px(primary_sidebar_width))
                                    .bg(panel_corner_mask_bg),
                            )
                        },
                    )
                    .when(terminal_material_visible && secondary_sidebar_open, |row| {
                        row.child(
                            div()
                                .absolute()
                                .right_0()
                                .top_0()
                                .bottom_0()
                                .w(px(sessions_sidebar_width))
                                .bg(panel_corner_mask_bg),
                        )
                    })
                    .when(primary_sidebar_mounted, |row| {
                        if self.settings_section.is_some() {
                            return row.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .h_full()
                                    .w(px(primary_sidebar_width))
                                    .flex_shrink_0()
                                    .overflow_hidden()
                                    .pt(title_bar_h)
                                    .child(self.render_settings_nav(window, cx))
                                    .into_any_element(),
                            );
                        }
                        row.child(match self.mode {
                            paneflow_config::schema::AppMode::Diff => div()
                                .flex()
                                .flex_col()
                                .h_full()
                                .w(px(primary_sidebar_width))
                                .flex_shrink_0()
                                .overflow_hidden()
                                .opacity(primary_sidebar_opacity)
                                .pt(title_bar_h)
                                .child(self.render_diff_sidebar(window, cx))
                                .into_any_element(),
                            paneflow_config::schema::AppMode::Cli => div()
                                .flex()
                                .flex_col()
                                .h_full()
                                .w(px(primary_sidebar_width))
                                .flex_shrink_0()
                                .overflow_hidden()
                                .opacity(primary_sidebar_opacity)
                                .pt(title_bar_h)
                                .child(self.render_sidebar(window, cx))
                                .into_any_element(),
                        })
                    })
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .relative()
                            .flex()
                            .flex_col()
                            .child(div().h(title_bar_h).flex_none())
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .relative()
                                    .flex()
                                    .flex_col()
                                    .overflow_hidden()
                                    .bg(panel_bg)
                                    .ml(px(main_panel_left_inset))
                                    .mr(px(crate::app::constants::PANEL_INSET))
                                    .mb(px(crate::app::constants::PANEL_INSET))
                                    .rounded(crate::app::constants::PANEL_CORNER_RADIUS)
                                    .capture_any_mouse_down(cx.listener(
                                        |this, event: &gpui::MouseDownEvent, _window, cx| {
                                            if event.button == gpui::MouseButton::Left
                                                && this.settings_section.is_none()
                                                && matches!(
                                                    this.mode,
                                                    paneflow_config::schema::AppMode::Cli
                                                )
                                            {
                                                this.acknowledge_visible_completions(cx);
                                            }
                                        },
                                    ))
                                    .child(main_content),
                            )
                            .when(terminal_material_visible, |panel_shell| {
                                panel_shell
                                    .child(
                                        div()
                                            .absolute()
                                            .right_0()
                                            .top(panel_top)
                                            .bottom_0()
                                            .w(px(crate::app::constants::PANEL_INSET))
                                            .bg(opaque_shell_bg),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .left_0()
                                            .right_0()
                                            .bottom_0()
                                            .h(px(crate::app::constants::PANEL_INSET))
                                            .bg(opaque_shell_bg),
                                    )
                                    .when(main_panel_left_inset > 0., |panel_shell| {
                                        panel_shell.child(
                                            div()
                                                .absolute()
                                                .left_0()
                                                .top(panel_top)
                                                .bottom_0()
                                                .w(px(main_panel_left_inset))
                                                .bg(opaque_shell_bg),
                                        )
                                    })
                            })
                            .child(
                                div()
                                    .absolute()
                                    .left(px(main_panel_left_inset))
                                    .top(panel_top)
                                    .size(crate::app::constants::PANEL_CORNER_RADIUS)
                                    .child(panel_corner_mask(
                                        PanelCorner::TopLeft,
                                        main_panel_corner_mask_bg,
                                    )),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .right(px(crate::app::constants::PANEL_INSET))
                                    .top(panel_top)
                                    .size(crate::app::constants::PANEL_CORNER_RADIUS)
                                    .child(panel_corner_mask(
                                        PanelCorner::TopRight,
                                        main_panel_corner_mask_bg,
                                    )),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(main_panel_left_inset))
                                    .bottom(px(crate::app::constants::PANEL_INSET))
                                    .size(crate::app::constants::PANEL_CORNER_RADIUS)
                                    .child(panel_corner_mask(
                                        PanelCorner::BottomLeft,
                                        main_panel_corner_mask_bg,
                                    )),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .right(px(crate::app::constants::PANEL_INSET))
                                    .bottom(px(crate::app::constants::PANEL_INSET))
                                    .size(crate::app::constants::PANEL_CORNER_RADIUS)
                                    .child(panel_corner_mask(
                                        PanelCorner::BottomRight,
                                        main_panel_corner_mask_bg,
                                    )),
                            ),
                    )
                    .when(sessions_sidebar_mounted, |row| {
                        row.child(
                            div()
                                .flex()
                                .flex_col()
                                .h_full()
                                .w(px(sessions_sidebar_width))
                                .flex_shrink_0()
                                .overflow_hidden()
                                .opacity(sessions_sidebar_opacity)
                                .pt(title_bar_h)
                                .child(self.render_sessions_sidebar(window, cx))
                                .into_any_element(),
                        )
                    }),
            );

        {
            app_content = app_content.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .overflow_hidden()
                    .child(self.title_bar.clone()),
            );
        }

        if let Some(toast) = &self.toast {
            app_content = app_content.child(self.render_toast(toast, ui));
        }

        if let Some(anchor) = self.title_bar_files_menu_open {
            app_content = app_content.child(self.render_title_bar_files_menu(anchor, window, cx));
        }

        if let Some(anchor) = self.title_bar_help_menu_open {
            app_content = app_content.child(self.render_title_bar_help_menu(anchor, window, cx));
        }

        if let Some(anchor) = self.profile_menu_open {
            app_content = app_content.child(self.render_profile_menu(anchor, window, cx));
        }

        if self.show_theme_picker {
            app_content = app_content.child(self.render_theme_picker(cx));
        }

        if self.broadcast_picker_open {
            app_content = app_content.child(self.render_broadcast_picker(cx));
        }

        let in_cli_mode = matches!(self.mode, paneflow_config::schema::AppMode::Cli);
        if self.attention_queue_open && in_cli_mode {
            app_content = app_content.child(self.render_attention_queue(cx));
        }
        if self.launch_pad.is_some() && in_cli_mode {
            app_content = app_content.child(self.render_launch_pad(cx));
        }
        if self.fleet_search.is_some() && in_cli_mode {
            if std::mem::take(&mut self.fleet_search_pending_focus) {
                self.fleet_search_focus.focus(window, cx);
            }
            app_content = app_content.child(self.render_fleet_search(cx));
        }

        if self.custom_buttons_modal.is_some() {
            app_content = app_content.child(self.render_custom_buttons_modal(cx));
        }

        if self.show_about_dialog {
            app_content = app_content.child(self.render_about_dialog(cx));
        }

        if self.system_info_dialog.is_some() {
            app_content = app_content.child(self.render_system_info_dialog(cx));
        }

        if let Some(menu) = self.workspace_menu_open
            && menu.idx < self.workspaces.len()
        {
            app_content =
                app_content.child(self.render_workspace_context_menu(menu, ui, window, cx));
        }

        if let Some(menu) = self.tab_menu_open
            && self
                .workspaces
                .get(menu.ws_idx)
                .is_some_and(|ws| menu.tab_idx < ws.tab_count())
        {
            app_content = app_content.child(self.render_tab_context_menu(menu, ui, window, cx));
        }

        if let Some(menu) = self.pane_menu_open.clone() {
            app_content = app_content.child(self.render_pane_context_menu(menu, ui, window, cx));
        }

        if let Some(menu) = self.files_menu_open.clone() {
            app_content = app_content.child(self.render_files_context_menu(menu, ui, window, cx));
        }

        crate::window_chrome::csd::client_side_window_shell(
            app_content,
            window,
            app_backdrop_bg,
            if terminal_material_visible {
                gpui::transparent_black()
            } else {
                ui.border
            },
        )
    }
}

fn run_update_and_exit() -> i32 {
    use crate::update::checker::{UpdateStatus, check_github_release};
    use crate::update::install_method::{self, InstallMethod};

    let method = install_method::detect();
    log::info!("--update-and-exit: install method = {method:?}");

    let null_telemetry = crate::telemetry::client::TelemetryClient::disabled();
    let status = check_github_release(&null_telemetry);
    let (version, asset_url) = match status {
        UpdateStatus::Available {
            version,
            asset_url: Some(url),
            ..
        } => (version, url),
        UpdateStatus::Available {
            asset_url: None, ..
        } => {
            eprintln!("paneflow-update: no asset matched the install method - nothing to install");
            return 5;
        }
        UpdateStatus::UpToDate => {
            eprintln!("paneflow-update: already up to date");
            return 2;
        }
        UpdateStatus::Failed => {
            eprintln!(
                "paneflow-update: feed unreachable at {} - check PANEFLOW_UPDATE_FEED_URL",
                crate::update::checker::update_feed_url()
            );
            return 3;
        }
        UpdateStatus::Checking => {
            eprintln!("paneflow-update: checker returned Checking - should never happen");
            return 1;
        }
    };

    log::info!("--update-and-exit: installing v{version} from {asset_url}");

    match method {
        InstallMethod::TarGz { .. } => match crate::update::linux::targz::run_update(&asset_url) {
            Ok(new_bin) => {
                println!("paneflow-update: ok new={}", new_bin.display());
                0
            }
            Err(err) => {
                let classified = crate::update::error::UpdateError::classify(&err);
                if matches!(
                    classified,
                    crate::update::error::UpdateError::IntegrityMismatch { .. }
                ) {
                    eprintln!("paneflow-update: hash mismatch - {err}");
                    return 4;
                }
                eprintln!("paneflow-update: install failed - {err}");
                1
            }
        },
        InstallMethod::AppImage { source_path, .. } => {
            match crate::update::linux::appimage::run_update(&source_path, &asset_url) {
                Ok(new_bin) => {
                    println!("paneflow-update: ok new={}", new_bin.display());
                    0
                }
                Err(err) => {
                    eprintln!("paneflow-update: AppImage install failed - {err}");
                    1
                }
            }
        }
        other => {
            eprintln!(
                "paneflow-update: --update-and-exit does not support install method {other:?}"
            );
            5
        }
    }
}

#[cfg(windows)]
fn should_detach_windows_console(
    is_scriptable_invocation: bool,
    console_process_count: u32,
) -> bool {
    !is_scriptable_invocation && console_process_count == 1
}

#[cfg(windows)]
fn detach_lonely_windows_console_for_gui_launch(is_scriptable_invocation: bool) {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};

    let mut processes = [0_u32; 2];
    let count = unsafe { GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) };
    if should_detach_windows_console(is_scriptable_invocation, count) {
        unsafe {
            let _ = FreeConsole();
        }
    }
}

#[cfg(all(test, windows))]
mod windows_startup_console_tests {
    use super::should_detach_windows_console;

    #[test]
    fn gui_launch_detaches_only_a_lonely_console() {
        assert!(should_detach_windows_console(false, 1));
        assert!(!should_detach_windows_console(false, 0));
        assert!(!should_detach_windows_console(false, 2));
    }

    #[test]
    fn scriptable_invocation_keeps_console_even_when_lonely() {
        assert!(!should_detach_windows_console(true, 1));
    }
}

fn mount_paneflow_app(window: &mut Window, cx: &mut App) -> Entity<PaneFlowApp> {
    let view = window.replace_root(cx, |_, cx| PaneFlowApp::new(cx));
    view.update(cx, |_, cx| {
        let weak = cx.weak_entity();
        cx.intercept_keystrokes(move |event, window, cx| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            if app.read(cx).settings_section != Some(SettingsSection::Shortcuts) {
                return;
            }
            let consumed = app.update(cx, |this, cx| {
                this.intercept_shortcut_keystroke(&event.keystroke, window, cx)
            });
            if consumed {
                cx.stop_propagation();
            }
        })
        .detach();
        let subscription = cx.observe_window_bounds(window, |this, window, cx| {
            crate::window_state::record_windowed_size(window);
            #[cfg(target_os = "linux")]
            crate::window_chrome::linux_backdrop::refresh_blur_region(window);
            if this.settings_section.is_some() {
                this.reset_settings_scroll();
                cx.notify();
                cx.on_next_frame(window, |this, _window, cx| {
                    if this.settings_section.is_some() {
                        cx.notify();
                    }
                });
            } else {
                cx.notify();
            }
        });
        subscription.detach();
    });
    window.on_window_should_close(cx, {
        let view = view.clone();
        move |_window, cx| {
            let app = view.read(cx);
            app.save_session_blocking(cx);
            app.emit_app_exited_and_flush();
            #[cfg(target_os = "linux")]
            crate::window_chrome::linux_backdrop::clear_subtle_chrome_material();
            cx.quit();
            false
        }
    });
    view.update(cx, |_, cx| {
        let subscription = cx.observe_window_activation(window, |_, window, cx| {
            crate::agents::notifications::set_window_active(window.is_window_active());
            #[cfg(target_os = "linux")]
            crate::window_chrome::linux_backdrop::refresh_blur_region(window);
            cx.notify();
        });
        subscription.detach();
    });
    crate::agents::notifications::set_window_active(window.is_window_active());

    view.update(cx, |app, cx| {
        app.sync_system_theme_from_window(window, cx);
        let subscription = cx.observe_window_appearance(window, |this, window, cx| {
            this.sync_system_theme_from_window(window, cx);
            cx.notify();
        });
        subscription.detach();
    });

    view.update(cx, |app, cx| {
        if let Some(ws) = app.workspaces.get(app.active_idx) {
            ws.focus_first(window, cx);
        }
    });
    view
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(unix)]
    if args.get(1).map(String::as_str) == Some(agents::parent_guard::PTY_GUARD_SUBCOMMAND) {
        std::process::exit(agents::parent_guard::run_pty_guard_from_args(&args));
    }
    #[cfg(target_os = "windows")]
    if external_open::is_open_url_helper_invocation(&args) {
        std::process::exit(external_open::run_open_url_helper_from_args(&args));
    }
    #[cfg(windows)]
    let is_msi_relay = update::windows::msi::is_relay_invocation(&args);
    #[cfg(not(windows))]
    let is_msi_relay = false;
    let is_mcp_subcommand = args.get(1).map(String::as_str) == Some("mcp");
    let is_cli_subcommand = cli::is_cli_verb(args.get(1).map(String::as_str));
    let is_hooks_subcommand = args.get(1).map(String::as_str) == Some("hooks");
    let is_global_help = !is_msi_relay
        && !is_mcp_subcommand
        && !is_cli_subcommand
        && !is_hooks_subcommand
        && args.iter().any(|a| a == "--help" || a == "-h");
    let is_global_version = !is_msi_relay
        && !is_mcp_subcommand
        && !is_cli_subcommand
        && !is_hooks_subcommand
        && args.iter().any(|a| a == "--version" || a == "-v");
    let is_update_and_exit = !is_msi_relay
        && !is_mcp_subcommand
        && !is_cli_subcommand
        && !is_hooks_subcommand
        && args.iter().any(|a| a == "--update-and-exit");
    let is_unknown_verb = args
        .get(1)
        .is_some_and(|verb| cli::looks_like_unknown_verb(Some(verb.as_str())));

    #[cfg(windows)]
    detach_lonely_windows_console_for_gui_launch(
        is_msi_relay
            || is_mcp_subcommand
            || is_cli_subcommand
            || is_hooks_subcommand
            || is_global_help
            || is_global_version
            || is_update_and_exit
            || is_unknown_verb,
    );

    #[cfg(windows)]
    if is_msi_relay {
        std::process::exit(update::windows::msi::run_relay_from_args(&args));
    }

    if is_global_help {
        println!(
            "PaneFlow {version} - native terminal workspace for coding agents\n\
             \n\
             Usage: paneflow [OPTIONS]\n\
             \x20      paneflow mcp <install|status|uninstall>\n\
             \n\
             Options:\n\
             \x20 -h, --help       Print this help message\n\
             \x20 -v, --version    Print version\n\
             \x20 --update-and-exit  Check for an update and exit (CI harness)\n\
             \n\
             Agent workflow:\n\
             \x20 Launch Claude Code, Codex, opencode, Pi, or any CLI agent in panes\n\
             \x20 Use `paneflow mcp install` so capable agents can read pane output\n\
             \n\
             Keybindings:\n\
             \x20 Ctrl+Shift+D/E   Split horizontal/vertical\n\
             \x20 Ctrl+Shift+W     Close pane\n\
             \x20 Alt+Arrow        Focus adjacent pane\n\
             \x20 Ctrl+Shift+N     New workspace\n\
             \x20 Ctrl+Tab         Next workspace\n\
             \x20 Ctrl+1-9         Switch to workspace N\n\
             \n\
             Config paths and IPC endpoints are documented in the README.\n\
             https://github.com/arthjean/paneflow",
            version = env!("CARGO_PKG_VERSION")
        );
        return;
    }
    if is_global_version {
        println!("paneflow {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    unsafe { agents::parent_guard::scrub_claudecode_env_before_threads() };

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(
        "warn,wgpu_hal=off,naga=warn,gpui_macos::text_system=error,zbus=warn,zbus::proxy=error,tracing::span=warn",
    ))
    .init();

    match agents::parent_guard::install_process_job() {
        Ok(agents::parent_guard::ParentGuardStatus::Installed) => {}
        Ok(agents::parent_guard::ParentGuardStatus::Unsupported) => {
            log::debug!(
                "parent_guard: process-wide job guard unsupported on Unix; PTY shells use per-PTY guards and shim-wrapped agents use shim guards"
            );
        }
        Err(err) => {
            log::warn!(
                "parent_guard: failed to install Job Object; kill -9 of Paneflow may orphan agent CLIs ({err})"
            );
        }
    }

    if should_load_login_shell_env_for_startup(
        is_msi_relay,
        is_mcp_subcommand,
        is_cli_subcommand,
        is_hooks_subcommand,
        is_update_and_exit,
        is_unknown_verb,
    ) {
        login_shell_env::load_login_shell_env();
    }

    runtime_paths::augment_path_for_gui_launch();

    if is_update_and_exit {
        std::process::exit(run_update_and_exit());
    }

    if args.get(1).map(String::as_str) == Some("mcp") {
        let bridge_path = if should_extract_mcp_bridge_for_cli(&args) {
            match ai_hooks::extract::ensure_bridge_extracted() {
                Ok(p) => Some(p),
                Err(e) => {
                    log::warn!("paneflow mcp: bridge extraction failed ({e:#})");
                    runtime_paths::bridge_binary_path()
                }
            }
        } else {
            runtime_paths::bridge_binary_path()
        };
        std::process::exit(paneflow_mcp_install::run_cli(&args[2..], bridge_path));
    }

    if is_hooks_subcommand {
        let hook_path = match ai_hooks::extract::ensure_ai_hook_extracted() {
            Ok(p) => Some(p),
            Err(e) => {
                log::warn!("paneflow hooks: ai-hook extraction failed ({e:#})");
                runtime_paths::ai_hook_binary_path()
            }
        };
        std::process::exit(paneflow_mcp_install::run_hooks_cli(&args[2..], hook_path));
    }

    if is_cli_subcommand {
        std::process::exit(cli::run());
    }

    if is_unknown_verb && let Some(verb) = args.get(1) {
        eprintln!("paneflow: unknown verb '{verb}'; see `paneflow --help` for the verb list");
        std::process::exit(2);
    }

    warn_if_legacy_run_install();
    #[cfg(target_os = "macos")]
    warn_if_rosetta_translated();

    match ai_hooks::extract::ensure_bridge_extracted() {
        Ok(path) => log::info!("paneflow: MCP bridge ready at {}", path.display()),
        Err(e) => log::warn!(
            "paneflow: MCP bridge extraction failed ({e:#}); `paneflow mcp install` will be unavailable until resolved"
        ),
    }

    #[cfg(target_os = "windows")]
    if let Err(err) = windows_app_identity::ensure_process_app_user_model_id() {
        log::warn!("paneflow: Windows app identity setup failed: {err}");
    }

    let _timer_resolution = app::win_timer::high_resolution_timer();

    application()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            let config = paneflow_config::loader::load_config();
            cx.set_text_rendering_mode(gpui::TextRenderingMode::Grayscale);
            keybindings::apply_keybindings(cx, &config.shortcuts);

            if let Err(e) = assets::Assets.load_fonts(cx) {
                log::warn!(
                    "Assets::load_fonts failed: {e}; text rendering may fail on \
                     systems without a system monospace font"
                );
            }

            #[cfg(target_os = "macos")]
            {
                install_macos_menu_bar(cx);
                install_macos_menu_action_fallbacks(cx);
            }

            let bounds = crate::window_state::initial_bounds(cx);
            let decorations = match config.window_decorations.as_deref() {
                Some("server") => WindowDecorations::Server,
                Some("client") | None => WindowDecorations::Client,
                Some(other) => {
                    log::warn!(
                        "Invalid window_decorations value '{}', using 'client'",
                        other
                    );
                    WindowDecorations::Client
                }
            };

            #[cfg_attr(target_os = "macos", allow(clippy::needless_update))]
            let titlebar_options = gpui::TitlebarOptions {
                title: None,
                appears_transparent: true,
                #[cfg(target_os = "macos")]
                traffic_light_position: Some(point(px(12.0), px(10.0))),
                ..Default::default()
            };

            let window_result = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(crate::window_state::minimum_size()),
                    window_decorations: Some(decorations),
                    titlebar: Some(titlebar_options),
                    window_background: crate::app::constants::window_background_appearance(
                        config.window_backdrop.as_deref(),
                    ),
                    app_id: Some("paneflow".into()),
                    ..Default::default()
                },
                |window, cx| {
                    #[cfg(target_os = "windows")]
                    if crate::app::constants::window_backdrop_uses_mica(
                        config.window_backdrop.as_deref(),
                    ) {
                        crate::window_chrome::backdrop::apply_wallpaper_mica(
                            window,
                            crate::theme::active_theme().background.l > 0.5,
                        );
                    }
                    #[cfg(target_os = "macos")]
                    if crate::app::constants::macos_sidebar_material_enabled(
                        config.window_backdrop.as_deref(),
                    ) {
                        crate::window_chrome::macos_backdrop::apply_subtle_sidebar_material(
                            window,
                            crate::theme::active_theme().background.l > 0.5,
                            config.macos_chrome_material_enabled(),
                        );
                    }
                    #[cfg(target_os = "linux")]
                    crate::window_chrome::linux_backdrop::apply_subtle_chrome_material(window);

                    cx.new(StartupSplashView::new)
                },
            );

            match window_result {
                Ok(_) => cx.activate(true),
                Err(e) => {
                    log::error!("Failed to open PaneFlow window: {e}");
                    #[cfg(target_os = "linux")]
                    eprintln!(
                        "Error: PaneFlow requires a GPU with Vulkan support.\n\n\
                         Install mesa-vulkan-drivers (AMD/Intel) or your GPU's proprietary driver.\n\n\
                         Install commands:\n\
                         \x20 Debian/Ubuntu:  sudo apt install mesa-vulkan-drivers\n\
                         \x20 Fedora/RHEL:    sudo dnf install mesa-vulkan-drivers\n\
                         \x20 Arch:           sudo pacman -S vulkan-radeon vulkan-intel or nvidia-utils\n\n\
                         Run `vulkaninfo` to verify Vulkan support.\n\
                         If drivers are already installed, run with RUST_LOG=error for details.\n\n\
                         Underlying error: {e}"
                    );
                    #[cfg(target_os = "windows")]
                    eprintln!(
                        "Error: PaneFlow could not create its GPU-backed window on Windows.\n\n\
                         Update your GPU driver from NVIDIA, AMD, Intel, or your PC vendor, then restart Paneflow.\n\
                         If this started after enabling a native backdrop, launch once with:\n\
                         \x20 PANEFLOW_WINDOW_BACKDROP=off\n\n\
                         Underlying error: {e}"
                    );
                    #[cfg(target_os = "macos")]
                    eprintln!(
                        "Error: PaneFlow could not create its GPU-backed window on macOS.\n\n\
                         Update macOS and restart Paneflow. If this started after enabling a native backdrop, launch once with:\n\
                         \x20 PANEFLOW_WINDOW_BACKDROP=off\n\n\
                         Underlying error: {e}"
                    );
                    std::process::exit(1);
                }
            }
        });
}
