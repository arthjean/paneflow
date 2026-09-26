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
mod auto_naming;
#[cfg(test)]
mod bench_harness;
mod claude_sessions;
mod cli;
mod codex_sessions;
mod command_sessions;
mod config_writer;
mod diff;
mod editor;
mod env_expand;
mod external_open;
mod file_icons;
mod fonts;
mod host_bootstrap;
mod ipc;
mod ipc_events;
mod keybindings;
mod keys;
mod launch;
mod launch_cwd;
mod layout;
mod limits;
mod login_shell_env;
mod markdown;
mod opencode_sessions;
mod pane;
mod pane_drag;
mod pi_sessions;
mod runtime_paths;
mod search;
mod settings;
mod sidebar_title;
#[cfg(test)]
mod startup_bench;
mod startup_trace;
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
mod worker_bootstrap;
mod workspace;

use crate::window_chrome::title_bar;

use gpui::{
    App, Context, CursorStyle, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    Pixels, Point, Render, Styled, Window, WindowBounds, WindowDecorations, WindowOptions, div,
    prelude::*, px,
};
use gpui_platform::application;

use crate::pane::{Pane, PaneSurface};
use crate::terminal::TerminalView;
use crate::workspace::Workspace;
use launch::run;
use window_chrome::{
    PanelCorner, native_backdrop_material_active, native_material_suppressed_by_fullscreen,
    panel_corner_mask,
};

pub use app::actions::*;
pub(crate) use app::bootstrap::warn_if_legacy_run_install;
pub(crate) use app::constants::{
    MAX_CLOSED_PANE_SCROLLBACK_BYTES, MAX_CLOSED_PANES, RESIZE_BORDER, SIDEBAR_WIDTH, TOAST_HOLD_MS,
};
pub(crate) use app::drag::{TabDrag, WorkspaceDrag, WorkspaceDragPreview};
#[cfg(target_os = "macos")]
pub(crate) use app::macos_menu::{install_macos_menu_action_fallbacks, install_macos_menu_bar};
pub(crate) use app::notifications::{Toast, ToastAction};
pub(crate) use app::render::{PRIMARY_SIDEBAR_MIN_ANIMATION_DELTA, SidebarWidthAnimation};
pub(crate) use app::window::*;

struct SelfUpdateState {
    pending_update: update::checker::SharedUpdateSlot,
    check_trigger: update::checker::UpdateCheckTrigger,
    manual_check: Option<crate::app::self_update_flow::ManualUpdateCheck>,
    update_status: Option<update::checker::UpdateStatus>,
    self_update_status: update::SelfUpdateStatus,
    install_method: update::install_method::InstallMethod,
    update_attempt_count: u32,
    download_generation: u64,
    dismissed_version: Option<String>,
    staged_msi: Option<update::windows::msi::StagedMsiUpdate>,
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
    pub(crate) maximized: Option<Option<gpui::FocusHandle>>,
    pub(crate) maximize_animation: Option<SidebarWidthAnimation>,
    pub(crate) reveal_animation: Option<SidebarWidthAnimation>,
    pub(crate) pane_grid_width: std::rc::Rc<std::cell::Cell<f32>>,
    pub(crate) resize: Option<(f32, f32, f32)>,
    pub(crate) h_scroll_drag: Option<crate::app::diff_dock::DiffDockHScrollDrag>,
    pub(crate) vertical_scrollbar: crate::widgets::editor_scrollbar::EditorScrollbar,
    pub(crate) h_offsets: std::rc::Rc<Vec<f32>>,
    pub(crate) hover: Option<crate::app::diff_dock::DiffHover>,
}

struct PaneFlowApp {
    pending_detached_panes: Vec<paneflow_config::schema::DetachedPaneSession>,
    workspaces: Vec<Workspace>,
    active_idx: usize,
    renaming_tab: Option<(usize, usize)>,
    sidebar_focus: FocusHandle,
    sidebar_cursor: Option<app::sidebar::keyboard::SidebarCursor>,
    rename_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    sidebar_filter_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    sidebar_filter_hovered: bool,
    sidebar_row_motion: std::cell::RefCell<app::sidebar::SidebarRowMotion>,
    rename_focus_live: bool,
    pending_config:
        std::sync::Arc<std::sync::Mutex<Option<paneflow_config::schema::PaneFlowConfig>>>,
    save_seq: std::sync::Arc<std::sync::atomic::AtomicU64>,
    session_restore_failed: bool,
    session_exit_pending: bool,
    session_save_error_shown: bool,
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
    settings_section: Option<SettingsSection>,
    settings_scroll: gpui::ScrollHandle,
    settings_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
    settings_search_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    settings_search_motion: std::rc::Rc<std::cell::RefCell<crate::settings::search::SearchMotion>>,
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
    agent_profile_editor: Option<crate::settings::tabs::agents::AgentProfileEditor>,
    agents_list_expanded: bool,
    agents_list_animation: Option<SidebarWidthAnimation>,
    integration_status: Option<Vec<paneflow_mcp_install::IntegrationStatus>>,
    integration_busy: Option<String>,
    integration_errors: std::collections::HashMap<String, String>,
    agent_profile_name_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    agent_profile_args_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    mcp_status: Option<Vec<paneflow_mcp_install::StatusReport>>,
    mcp_install: Option<Result<Vec<paneflow_mcp_install::InstallReport>, String>>,
    mcp_busy: bool,
    sidebar_scroll: gpui::ScrollHandle,
    effective_shortcuts: Vec<keybindings::ShortcutEntry>,
    recording_shortcut_idx: Option<usize>,
    shortcut_search_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    shortcut_capture_active: bool,
    shortcut_reset_pending: bool,
    shortcut_conflict: Option<crate::settings::tabs::shortcuts::ShortcutConflict>,
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
    session_menu_open: Option<SessionContextMenu>,
    pub(crate) worktree_states: crate::app::tab_worktree::WorktreeStates,
    pub(crate) branch_checkout_pending: Option<String>,
    pub(crate) pr_states: crate::app::pull_request::PrStates,
    pub(crate) sidebar_customize_menu_open: bool,
    pub(crate) sidebar_show_submenu_open: bool,
    tab_menu_open: Option<TabContextMenu>,
    pane_menu_open: Option<PaneContextMenu>,
    pending_pane_focus: Option<Entity<Pane>>,
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
    closed_panes: Vec<crate::app::workspace_ops::ClosedRecord>,
    owned_sessions: crate::app::hosted_sessions::OwnedSessions,
    resume_batch: Option<crate::app::hosted_sessions::ResumeBatch>,
    close_dialog: Option<crate::app::close_policy::CloseDialog>,
    close_dialog_focus: FocusHandle,
    worktree_remove_dialog: Option<crate::app::worktree_remove::WorktreeRemoveDialog>,
    worktree_remove_focus: FocusHandle,
    host_agents: crate::app::host_agents::HostAgentView,
    about_dialog: Option<crate::app::about_dialog::AboutDialog>,
    system_info_dialog: Option<crate::app::system_info_dialog::SystemInfoDialog>,
    quit_dialog: Option<crate::app::quit_dialog::QuitDialog>,
    quit_dialog_focus: FocusHandle,
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
    branch_prompt: Option<app::branch_prompt::BranchPromptState>,
    branch_prompt_focus: FocusHandle,
    recent_workspaces: Vec<app::recents::RecentWorkspace>,
    welcome_focus: FocusHandle,
    clone_repo: Option<app::clone_repo::CloneRepoState>,
    clone_repo_focus: FocusHandle,
    command_palette_open: bool,
    command_palette_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    command_palette_selected: usize,
    command_palette_query_seen: String,
    command_palette_scroll: gpui::UniformListScrollHandle,
    command_palette_scope: Option<crate::app::command_palette::Scope>,
    command_palette_context: crate::app::command_palette::PaletteContext,
    command_palette_restore_focus: Option<FocusHandle>,
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
    pub(crate) diff_dock: DiffDockState,
    pub(crate) sidebar_order_cache: std::cell::RefCell<crate::app::sidebar::SidebarOrderCache>,
}

pub static SWAP_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn main() {
    run();
}
