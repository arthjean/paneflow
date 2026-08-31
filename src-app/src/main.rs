// Test-only allow for the CLAUDE.md-mandated clippy restrictions. These
// lints are also demoted to `allow` at crate level in `src-app/Cargo.toml`
// for pre-existing GPUI UI-code unwraps (US-007 "or equivalent" escape),
// so today this belt is effectively redundant - but it stays in place so
// that when the eventual cleanup story re-promotes the Cargo.toml lints
// to `warn`, tests continue to pass without another edit here.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]
// Windows deliberately stays a console-subsystem binary. PowerShell/cmd do not
// wait for GUI-subsystem executables, so `paneflow ls` would otherwise return
// immediately with no stdout/stderr and a misleading success code. GUI launches
// still shed the auto-created one-process console at startup; see
// `detach_lonely_windows_console_for_gui_launch`.
//! PaneFlow - native terminal workspace for coding agents.
//!
//! App shell with sidebar workspace list, terminal panes, agent surfaces, and
//! diff/review workflows.

mod agent_launcher;
mod agent_sessions;
mod agents;
mod ai_hooks;
mod ai_types;
mod app;
mod assets;
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

// Re-export action types at the crate root so existing `crate::SplitHorizontally`
// references in sibling modules keep compiling without a crate-wide import churn.
pub use app::actions::*;
// US-002: items extracted out of `main.rs` are re-exported at crate root
// so callers like `crate::TOAST_HOLD_MS` keep resolving without an
// import-rewrite churn across the workspace.
pub(crate) use app::constants::{
    MAX_CLOSED_PANE_SCROLLBACK_BYTES, MAX_CLOSED_PANES, RESIZE_BORDER, SIDEBAR_WIDTH, TOAST_HOLD_MS,
};
// `TOAST_ENTER_MS` and `TOAST_EXIT_MS` are used only by the toast
// renderer inside `app::notifications`; not re-exported at crate root.
pub(crate) use app::drag::{TabDrag, WorkspaceDrag, WorkspaceDragPreview};
pub(crate) use app::notifications::{Toast, ToastAction};
// Free helpers extracted to bootstrap.rs but still callable as
// `crate::system_package_update_command` etc. from sibling modules.
#[cfg(target_os = "macos")]
pub(crate) use app::bootstrap::{
    install_macos_menu_action_fallbacks, install_macos_menu_bar, warn_if_rosetta_translated,
};
pub(crate) use app::bootstrap::{system_package_update_command, warn_if_legacy_run_install};

// Terminal-routing helpers (`find_first_terminal`, `find_terminal_by_surface_id`)
// live in `app::ipc_handler` - its only consumer.

// ---------------------------------------------------------------------------
// Root application view
// ---------------------------------------------------------------------------

/// A page in the embedded settings experience (Codex-style: grouped nav on the
/// left rail, the section body on the right). `General` is the landing page.
/// One source of truth - replaces the old 2-variant inline enum *and* the
/// standalone window's copy, now that settings render inline (`settings::chrome`).
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
    /// Whether the page renders its own scroll container instead of riding the
    /// page-level one.
    ///
    /// Only Shortcuts does: it is the one page long enough (~90 rows) that
    /// rendering every row each frame is what made the settings surface lag,
    /// so it virtualizes with `gpui::list`. A virtualized list needs a bounded
    /// viewport to virtualize against, which a page-level scroll container -
    /// which hands its child unbounded height - cannot give it.
    pub(crate) fn owns_its_scroll(self) -> bool {
        matches!(self, SettingsSection::Shortcuts)
    }
}

/// Light / dark / system selector shown at the top of the Themes settings page.
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

    /// The bundled theme this mode resolves to *within* `preset`. Mode and
    /// preset are orthogonal: the preset owns the identity, the mode picks
    /// which of its two variants runs.
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

/// Which Terminal-page enum dropdown is currently open (only one at a time).
/// `None` = all closed. Distinct from `font_dropdown_open` (the Terminal page's
/// searchable font picker) so only one popover is active at a time.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum TerminalDropdown {
    CursorShape,
    CursorColor,
    FontWeight,
}

/// Which General-page select dropdown is currently open (only one at a time).
/// `None` = all closed. Mirrors `TerminalDropdown` so navigating away or opening
/// the other select never leaves a ghost popover.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum GeneralDropdown {
    Editor,
    Shell,
}

/// Which Workspaces-page dropdown is currently open.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum WorkspaceTemplateDropdown {
    Layout,
}

#[derive(Clone, Copy)]
pub(crate) struct WorkspaceContextMenu {
    pub(crate) idx: usize,
    pub(crate) position: Point<Pixels>,
}

/// Right-click menu for a sidebar tab row (US-010). Identifies its target by
/// workspace *and* tab index; both are re-validated before the menu paints, so
/// a tab closed under an open menu simply dismisses it.
#[derive(Clone, Copy)]
pub(crate) struct TabContextMenu {
    pub(crate) ws_idx: usize,
    pub(crate) tab_idx: usize,
    pub(crate) position: Point<Pixels>,
}

/// Right-click menu for a pane, anchored on its header (EP-002 US-007). A pane
/// is mono-surface, so the menu identifies its target by the pane alone - there
/// is no tab index, and no cross-pane move entry.
#[derive(Clone)]
pub(crate) struct PaneContextMenu {
    pub(crate) pane: Entity<Pane>,
    pub(crate) position: Point<Pixels>,
}

/// Open right-click menu for a Files-sidebar row (PRD files-tree EP-003
/// US-009). Carries the row's absolute path and the click anchor; "Copy
/// relative path" resolves the workspace root at render/action time.
#[derive(Clone)]
pub(crate) struct FilesContextMenu {
    pub(crate) path: std::path::PathBuf,
    pub(crate) position: Point<Pixels>,
}

/// Captured state of a closed pane for undo-close-pane (US-014).
pub(crate) enum ClosedSurfaceRecord {
    Terminal {
        cwd: Option<std::path::PathBuf>,
        /// The closed screen and its recent history, as the VT sequences that
        /// replay it. Styling, modes, and the cursor survive, which plain
        /// text loses. In-process only: these bytes never reach disk, and
        /// never come from it.
        replay: Option<Vec<u8>>,
        custom_name: Option<String>,
        font_size: Option<f32>,
    },
    Markdown {
        path: std::path::PathBuf,
    },
}

pub(crate) struct ClosedPaneRecord {
    /// EP-002 US-004: a pane holds exactly one surface, so an undo record
    /// captures exactly one surface and no selection cursor.
    pub(crate) surface: ClosedSurfaceRecord,
    pub(crate) workspace_idx: usize,
}

/// US-053: in-app self-update flow state, extracted from the `PaneFlowApp`
/// god-struct. Grouped: the background-check slot/result, the live flow
/// status, the detected install method, and the consecutive-failure counter.
struct SelfUpdateState {
    /// Shared slot for the background update checker result.
    pending_update: update::checker::SharedUpdateSlot,
    /// Resolved update status (set once the background check completes).
    update_status: Option<update::checker::UpdateStatus>,
    /// Live state of the in-app self-update flow (download → install → restart).
    self_update_status: update::SelfUpdateStatus,
    /// How the running binary was installed. Detected once at startup -
    /// drives the update pill's label/click behaviour (US-012) and the
    /// in-app updater's branch selection.
    install_method: update::install_method::InstallMethod,
    /// Count of consecutive in-app update failures since process start
    /// (US-013). Bumped on every classified error; after 3 failures the
    /// 4th click skips the network and shows the "download manually"
    /// escape hatch toast.
    ///
    /// Never decremented. The only success path for an update calls
    /// `cx.restart()`, which replaces this process - the fresh
    /// `PaneFlowApp::new` initializes the counter back to 0. So "failures
    /// since last success" and "failures since process start" coincide by
    /// construction; the PRD's "three consecutive failures" requirement
    /// holds without an explicit reset.
    update_attempt_count: u32,
    /// Monotonic token identifying the current `Downloading` attempt (EP-002,
    /// U-015). Bumped each time the flow enters `Downloading`; the per-attempt
    /// watchdog captures the value and only fires if it still matches - so a
    /// stale watchdog from a superseded attempt can't reset a newer one.
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

/// Whether the native window material has to stand down for this frame.
///
/// macOS native fullscreen moves the window onto its own Space with a black
/// backdrop, so AppKit's behind-window material has no desktop left to sample
/// and the blur collapses into a flat tint. Falling back to the opaque theme
/// chrome keeps fullscreen readable instead of showing a dead material.
///
/// Tiled and maximized windows stay in the desktop Space and keep a live blur
/// (verified on macOS 15), so this keys off `is_fullscreen`, never
/// `is_maximized`.
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

/// US-053: docked agent-sessions sidebar state (visibility, per-agent
/// scanned session lists, the originating pane/cwd, and group UI flags),
/// extracted from the `PaneFlowApp` god-struct.
struct AgentSessionsState {
    /// Whether the docked agent-sessions right sidebar is visible
    /// (PRD `prd-agent-sessions-sidebar-2026-Q3`, EP-001). Toggled by the
    /// tab-bar sessions button; the sidebar renders as a layout child of the
    /// root row, not a `deferred()` overlay.
    sessions_sidebar_open: bool,
    /// Width animation for opening/closing the docked right sidebar. Reuses
    /// the same duration and easing as the primary left sidebar.
    sessions_sidebar_animation: Option<SidebarWidthAnimation>,
    /// Cwd-scoped sessions per supported CLI, indexed by
    /// [`agent_sessions::SessionAgent::index`]. Filled asynchronously by
    /// per-agent background scans.
    sessions_by_agent: [Vec<agent_sessions::SessionMeta>; agent_sessions::SESSION_AGENT_COUNT],
    /// Per-agent count of older sessions omitted by the EP-003 sidebar memory
    /// cap, indexed by `agent_index()`.
    sessions_omitted: [usize; agent_sessions::SESSION_AGENT_COUNT],
    /// Working directory the sidebar was opened for. Used to filter stale
    /// scan results and as the compact wayfinding label inside the sidebar
    /// header.
    sessions_cwd: Option<String>,
    /// Surface id of the terminal whose tab-bar button opened the sidebar.
    /// Resume commands are sent back only if that exact terminal still exists.
    sessions_surface_id: Option<u64>,
    /// Scroll state for the sessions list. Re-created on every open so a fresh
    /// sidebar starts at offset 0.
    sessions_scroll: gpui::ScrollHandle,
    /// Incremented on every open/retarget/close. Async scans must carry the
    /// generation they were spawned under so a stale result for the same cwd
    /// cannot overwrite a newer open.
    sessions_scan_generation: u64,
    /// Keyboard-selected visible session row. The index is over visible rows
    /// only, not group headers or empty/loading states.
    sessions_selected: usize,
    /// Focus target for keyboard navigation inside the docked sidebar.
    sessions_focus: FocusHandle,
    /// Per-agent sidebar group state, indexed by
    /// [`agent_sessions::SessionAgent::index`]. All reset on close/open.
    /// `collapsed`: the group's caret has hidden its rows (EP-002 US-006).
    sessions_group_collapsed: [bool; agent_sessions::SESSION_AGENT_COUNT],
    /// `show_all`: the group is past its 5-row cap via "Show more" (US-005).
    sessions_group_show_all: [bool; agent_sessions::SESSION_AGENT_COUNT],
    /// `scanning`: a background scan for this agent is in flight, so an empty
    /// list should read as "loading" not "none" (US-004).
    sessions_scanning: [bool; agent_sessions::SESSION_AGENT_COUNT],
}

/// US-053: Git Diff mode state (mounted single/multi-repo views + their
/// caches, the worktree/scope/project pickers, and the file-tree filter),
/// extracted from the `PaneFlowApp` god-struct.
struct DiffModeState {
    /// US-005 (prd-git-diff-mode-2026-Q3.md): the mounted Git Diff mode
    /// view, when `mode == AppMode::Diff`. Lazily (re)built by
    /// `rebuild_diff_view` on mode entry and on workspace switch;
    /// `None` when no git repo backs the active workspace. Dropping it
    /// releases the DiffView's filesystem watchers.
    diff_view: Option<gpui::Entity<crate::diff::DiffView>>,
    /// US-014 (prd-git-diff-mode-2026-Q3.md): the Multi-project host,
    /// mounted when `diff_scope == MultiProject`. Separate from
    /// `diff_view` (the single-repo host for Project / Worktree).
    multi_diff_view: Option<gpui::Entity<crate::diff::MultiRepoDiffView>>,
    /// US-016 warm-resume: cache of mounted single-repo `DiffView` entities
    /// (Project / Worktree scopes), keyed by repo + scope + worktree set. A
    /// CLI↔Diff toggle (or a workspace switch back to a visited repo) reuses
    /// the cached entity instead of cold-rebuilding it, so the diff shows in
    /// one frame with its computed rows instead of flashing "Computing diff…".
    /// Non-displayed entries are suspended (watchers released - US-016), so at
    /// most one diff entity ever holds live watchers. Mirrors the
    /// `agents_terminal_view_cache` pointer/owner split; bounded by
    /// `DIFF_VIEW_CACHE_CAP` and pruned to open repos on workspace close.
    diff_view_cache: std::collections::HashMap<
        crate::app::diff_view_actions::DiffViewKey,
        gpui::Entity<crate::diff::DiffView>,
    >,
    /// US-016: the cache key the current `diff_view` pointer is bound to (which
    /// cache entry it clones). `None` outside Diff mode, in Multi-project scope,
    /// or when no git repo backs the active workspace.
    diff_view_key: Option<crate::app::diff_view_actions::DiffViewKey>,
    /// US-016: retained Multi-project host + the signature of the repo-group set
    /// it was built for. Reused across CLI↔Diff toggles while the open project
    /// set is unchanged; rebuilt when projects open/close. `multi_diff_view` is
    /// the display pointer into this slot.
    multi_diff_view_retained: Option<(u64, gpui::Entity<crate::diff::MultiRepoDiffView>)>,
    /// Diff sidebar: branch sections (keyed by branch name) the user has
    /// collapsed in the multi-branch changed-files panel. Ephemeral UI state
    /// (resets on remount), so a `HashSet` of names is enough.
    diff_collapsed_branches: std::collections::HashSet<String>,
    /// `true` while the Worktree-scope on-disk worktree discovery
    /// (`spawn_worktree_discovery`) is in flight, so the diff sidebar can show a
    /// "Discovering worktrees…" note instead of looking like columns are missing
    /// during the brief cold-mount window.
    diff_discovering: bool,
    /// Repo root for the active worktree-discovery task. Prevents a stale task
    /// from clearing a newer repo's spinner while still letting its own spinner
    /// clear after the user leaves Worktree scope.
    diff_discovering_root: Option<std::path::PathBuf>,
    /// Worktree-scope branch curation: per repo, the set of worktree paths (raw
    /// path strings) the user explicitly chose to show as columns. NO entry for a
    /// repo ⇒ show ALL its worktrees (the default). An entry ⇒ build columns for
    /// exactly those worktrees, so branches the user didn't pick are never diffed
    /// (not merely hidden). Edited by the branches picker; in-memory per session.
    diff_chosen_worktrees:
        std::collections::HashMap<std::path::PathBuf, std::collections::HashSet<String>>,
    /// Whether the Worktree-scope branches multi-select popover is open.
    diff_worktree_picker_open: bool,
    /// All worktrees of `diff_available_repo`, fetched off-thread for the branches
    /// picker so it can offer branches not currently shown. Populated lazily when
    /// the picker opens.
    diff_available_worktrees: Vec<crate::diff::DiffWorktree>,
    /// The repo [`Self::diff_available_worktrees`] was fetched for (guards against
    /// showing a stale list after a workspace/repo switch).
    diff_available_repo: Option<std::path::PathBuf>,
    /// US-011: the active Git Diff view scope (Project / Multi-project /
    /// Worktree). Defaults to Project; `rebuild_diff_view` branches on it.
    diff_scope: crate::diff::DiffScope,
    /// US-012: whether the scope-selector popover is open.
    diff_scope_picker_open: bool,
    /// Whether the project-selector popover (Project / Worktree scopes) is
    /// open. Lets the user pick which open workspace's repo the single-repo
    /// diff follows, without leaving Diff mode.
    diff_project_picker_open: bool,
    /// US-008 (prd-git-diff-mode-2026-Q3.md): path of the file row
    /// selected in the diff git panel (presentation-only until the
    /// scroll-to-file wiring lands). `None` = nothing selected.
    diff_selected_file: Option<String>,
    /// US-008: whether the git panel's "Changes" section is collapsed.
    diff_files_collapsed: bool,
    /// Changed-files panel layout: `false` = flat list (default), `true` =
    /// collapsible directory tree (compact-folder chains merged). Toggled from
    /// the "Changes" header.
    diff_files_tree: bool,
    /// Collapsed directory nodes in tree mode, keyed `col_idx\0<dir path>` so a
    /// directory present in two branch sections collapses independently.
    diff_collapsed_dirs: std::collections::HashSet<String>,
    /// US-008: persistent type-to-filter field for the diff changed-files
    /// panel. Observed at construction so each keystroke re-renders the
    /// sidebar (which recomputes the visible matches by path substring).
    diff_file_filter: gpui::Entity<crate::widgets::text_input::TextInput>,
}

/// State of the CLI cockpit's right-docked git diff panel: what it shows,
/// how it is laid out, and which workspace currently owns it.
///
/// Extracted from the `PaneFlowApp` god-struct (US-053).
struct DiffDockState {
    /// Whether the Codex-style git diff dock is open on the right of the CLI
    /// pane grid (toggled from a pane header, closed from its own
    /// header).
    pub(crate) open: bool,
    /// The diff snapshot rendered by the dock, computed off-thread for the
    /// active thread's cwd. Retained while hidden so same-cwd reopen is warm.
    pub(crate) data: Option<crate::app::diff_dock::DiffDockData>,
    /// Paths of files folded shut in the diff dock, so a fold survives re-renders.
    pub(crate) collapsed: std::collections::HashSet<String>,
    /// Stable fold keys for collapsed unchanged regions opened in the dock.
    /// Mirrors Review's fold-marker interaction without re-shelling git.
    pub(crate) expanded_folds: std::collections::HashSet<String>,
    /// Diff dock view mode: `false` = unified (inline), `true` = split (old left,
    /// new right). Defaults to split; toggled from the header.
    pub(crate) split: bool,
    /// Monotonic token for same-cwd diff builds. Completion must match this
    /// generation so an older refresh cannot overwrite a newer snapshot.
    pub(crate) generation: u64,
    /// Vertical scroll handle for the diff dock's [`crate::diff::DiffElement`]
    /// (hosted in an `overflow_y_scroll` div, the same render path as the Review
    /// view's columns). Survives ordinary repaints so scroll position is kept.
    pub(crate) scroll: gpui::ScrollHandle,
    /// Whether the diff dock's `...` overflow menu (layout, expand-all, refresh)
    /// is open, and whether its "Layout" side submenu is unfolded inside it.
    pub(crate) diff_options_menu_open: bool,
    pub(crate) diff_layout_submenu_open: bool,
    /// Whether the tab strip's `+` menu (which surface a new tab opens) is up.
    pub(crate) diff_new_tab_menu_open: bool,
    /// Whether the dock is currently showing its surface picker instead of a
    /// tab (see `diff_dock::surface_picker`). Set by the pane-header toggle on
    /// the workspace's first open of the dock.
    pub(crate) picker: bool,
    /// Whether the picker has been answered at least once *for the session
    /// that owns the live dock*. Once it has, opening the dock there restores
    /// the last active tab rather than asking again.
    pub(crate) picked: bool,
    /// Which session (workspace tab id) the live dock fields above describe.
    /// `None` until the dock is first opened.
    /// [`PaneFlowApp::sync_diff_dock_session`] parks and swaps them whenever
    /// this drifts from the visible session.
    pub(crate) owner: Option<u64>,
    /// Dock state parked per session id, for every session that is not
    /// [`Self::owner`]. The dock is detached per session: opening it in one tab
    /// leaves its siblings, and the next workspace, untouched.
    pub(crate) parked: std::collections::HashMap<u64, crate::app::cli_diff_dock::DiffDockSlot>,
    /// The dock's tabs. Index 0 is always the permanent `Changes` diff; the
    /// rest are terminals opened from the `+` menu, closable from their tab.
    pub(crate) diff_tabs: Vec<crate::app::diff_dock::DiffDockTab>,
    /// Index into `diff_tabs` of the tab whose body the dock renders.
    pub(crate) diff_active_tab: usize,
    /// Index of the modified file tab whose close button is armed, i.e. waiting
    /// for the confirming second press (US-017). Cleared by any tab selection,
    /// open or close, so the confirmation never outlives its gesture.
    pub(crate) diff_tab_close_armed: Option<usize>,
    /// Open branch picker anchored to the diff dock's toolbar chip; `None` when
    /// closed. Holds the branch list, the search field and the focus to restore.
    pub(crate) diff_branch_menu: Option<crate::app::diff_dock::DiffBranchMenuState>,
    /// Width in px of the diff dock; user-resizable by dragging its left edge.
    /// Clamped to `[DIFF_DOCK_PANEL_MIN_WIDTH, DIFF_DOCK_PANEL_MAX_WIDTH]`.
    pub(crate) width: f32,
    /// Live drag anchor `(cursor_x, width_at_grab, max_width)` while the dock's
    /// left edge is being dragged to resize; `None` when not resizing. The
    /// ceiling is captured at grab time because it depends on the panel the
    /// dock is docked in, which the drag math does not see.
    pub(crate) resize: Option<(f32, f32, f32)>,
    /// Live horizontal-scrollbar drag inside the dock's shared diff body.
    pub(crate) h_scroll_drag: Option<crate::app::diff_dock::DiffDockHScrollDrag>,
    /// Per-file horizontal scroll offsets (px) for the diff dock, indexed by
    /// stable file position. Driven by Shift+wheel / trackpad horizontal gestures
    /// (`apply_diff_dock_hwheel`) and applied per file by `DiffElement`; lazily
    /// resized to the file count at render (collapse/split never change the
    /// count, so offsets stay aligned).
    pub(crate) h_offsets: std::rc::Rc<Vec<f32>>,
}

struct PaneFlowApp {
    workspaces: Vec<Workspace>,
    active_idx: usize,
    /// US-010: sidebar tab being renamed inline, as `(workspace_idx, tab_idx)`.
    ///
    /// A tab is the ONLY thing the sidebar renames. A workspace takes its name
    /// from the folder it holds (`Workspace::title`, derived from the directory
    /// at open time), so letting it drift from that folder would leave the rail
    /// claiming a name nothing on disk answers to, with no way back.
    renaming_tab: Option<(usize, usize)>,
    /// Issue #32: the inline rename box is a real `TextInput`, not a text
    /// mirror fed by the row's `on_key_down`. GPUI dispatches key events along
    /// the focus path only, and a sidebar row owns no focus handle, so the old
    /// mirror never saw a keystroke on any platform.
    rename_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    /// Whether `rename_input` held the focus as of the last frame. `render`
    /// compares it against the live rename state to focus the field when an
    /// edit opens, commit when focus leaves it, and hand focus back to the
    /// active pane when the edit ends.
    rename_focus_live: bool,
    /// Shared slot for config changes from the background `ConfigWatcher` thread.
    /// The watcher writes `Some(config)` on every successful reload; the main
    /// thread `take()`s it in the 50ms poll loop to apply keybindings + theme.
    pending_config:
        std::sync::Arc<std::sync::Mutex<Option<paneflow_config::schema::PaneFlowConfig>>>,
    /// US-011: monotonic save-coalescing token. Every `save_session` bumps it
    /// and the off-thread writer skips its disk write when a newer save has
    /// been scheduled meanwhile, collapsing a burst (e.g. closing 20
    /// workspaces) into a single write - none of it on the render thread.
    save_seq: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// US-014: parsed `paneflow.json` cached on the main thread so render paths
    /// never call the blocking `load_config()` (fs read + JSON parse) per frame.
    /// Hydrated at startup, invalidated in [`Self::process_config_changes`] when
    /// the background `ConfigWatcher` reports a reload. Render code reads this;
    /// click handlers that must observe a config write *they just made* still
    /// read fresh from disk (the cache lags the write by the watcher debounce).
    cached_config: paneflow_config::schema::PaneFlowConfig,
    ipc_rx: std::sync::mpsc::Receiver<ipc::IpcRequest>,
    ipc_status: ipc::IpcStatus,
    /// EP-002 (agent-control-plane): outbound event bus shared with the IPC
    /// server. `broadcast` is called from the render thread (non-blocking).
    event_bus: std::sync::Arc<ipc_events::EventBus>,
    /// EP-002 US-006: last `output_generation` broadcast per surface, so the
    /// 50 ms sweep emits `surface_changed` only on an actual change (debounce).
    last_broadcast_gen: std::collections::HashMap<u64, u64>,
    title_bar: Entity<title_bar::TitleBar>,
    /// Visibility of the primary left rail shared by CLI, Agents, and Diff.
    /// Ephemeral by design: each launch starts with navigation visible.
    primary_sidebar_visible: bool,
    /// Transient width interpolation for the primary rail. The boolean above is
    /// the target state; this keeps the rail mounted while its layout width
    /// eases open or closed.
    primary_sidebar_animation: Option<SidebarWidthAnimation>,
    /// Anchor for the `Files` menu in the custom title bar.
    title_bar_files_menu_open: Option<Point<Pixels>>,
    /// Anchor for the `Help` menu in the custom title bar.
    title_bar_help_menu_open: Option<Point<Pixels>>,
    /// File watcher for `.git/HEAD` and `.git/index` across all workspaces.
    /// `None` if the OS watcher could not be created (graceful degradation).
    git_watcher: Option<notify::RecommendedWatcher>,
    /// Receiver for raw notify events from the git file watcher.
    git_event_rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    /// Refcount for watched `.git` directories (multiple workspaces may share a repo).
    git_watch_counts: std::collections::HashMap<std::path::PathBuf, usize>,
    /// Last state read out of Claude Code's session registry, per agent PID.
    ///
    /// Claude Code writes that file on transition but Paneflow reads it on a
    /// clock, so without a watermark the same state would be re-applied every
    /// tick and keep resetting the stall clock on a session that has not
    /// moved. See `app::agent_status`.
    claude_registry_seen: crate::app::agent_status::RegistryWatermark,
    /// Active settings section, or `None` if settings is closed.
    settings_section: Option<SettingsSection>,
    /// Scroll state for the inline settings page.
    settings_scroll: gpui::ScrollHandle,
    settings_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
    /// Codex settings nav search box (filters the section list). A real
    /// single-line `TextInput`, observed so each keystroke re-renders the nav.
    settings_search_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    /// Codex settings: which Terminal-page dropdown is open (`None` = closed).
    terminal_dropdown: Option<TerminalDropdown>,
    /// Codex settings: which General-page select is open (`None` = closed).
    general_dropdown: Option<GeneralDropdown>,
    /// Codex settings: which Workspaces-page select is open (`None` = closed).
    workspace_template_dropdown: Option<WorkspaceTemplateDropdown>,
    /// Selected `cached_config.commands` index for the Workspaces page.
    workspace_template_selected: Option<usize>,
    /// Whether the Workspaces page is showing the selected template detail.
    workspace_template_detail_open: bool,
    /// Selected flattened pane index inside the selected workspace template.
    workspace_template_selected_pane: usize,
    /// Last Workspaces-page action result or validation message.
    workspace_template_status: Option<String>,
    /// Workspaces-page text fields. They are synced from the selected template
    /// and explicitly saved by the page actions.
    workspace_template_name_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    workspace_template_project_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    workspace_pane_name_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    workspace_pane_cwd_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    workspace_pane_command_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    workspace_pane_prompt_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    /// Codex settings: cached MCP-bridge status snapshot, refreshed off-thread
    /// so the MCP page never does config I/O during a frame.
    mcp_status: Option<Vec<paneflow_mcp_install::StatusReport>>,
    /// Codex settings: result of the last MCP-bridge install (per-agent recap,
    /// or a wholesale refusal message).
    mcp_install: Option<Result<Vec<paneflow_mcp_install::InstallReport>, String>>,
    /// Codex settings: an MCP-bridge install is running.
    mcp_busy: bool,
    /// Scroll state for the persistent sidebar workspace list.
    /// Driven by GPUI's `overflow_y_scroll + track_scroll`; the
    /// visible scroll bar has been removed but the handle is still
    /// useful so the list keeps a stable wheel-scroll offset across
    /// re-renders.
    sidebar_scroll: gpui::ScrollHandle,
    /// Effective keybindings (defaults merged with user overrides) for settings display.
    effective_shortcuts: Vec<keybindings::ShortcutEntry>,
    /// Index of the shortcut row currently being recorded (`None` = not recording).
    recording_shortcut_idx: Option<usize>,
    /// Free-text filter for the Shortcuts page. Matches the action description
    /// *and* the rendered keystroke, so "ctrl+shift" and "workspace" both work.
    shortcut_search_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    /// Whether the Shortcuts page is in key-capture mode: the pressed chord is
    /// written into the search field instead of reaching the app, so the page
    /// can answer "what already owns this key?" - which a text search cannot,
    /// unless the user already knows how the chord is spelled.
    ///
    /// While this is set, [`Self::intercept_shortcut_keystroke`] swallows every
    /// chord before GPUI can dispatch its action.
    shortcut_capture_active: bool,
    /// Whether "Reset to defaults" on the Shortcuts page is awaiting
    /// confirmation. The reset rewrites every binding in `paneflow.json` with
    /// no undo, so the button asks before it fires.
    shortcut_reset_pending: bool,
    /// Sections collapsed on the Shortcuts page. Empty = everything expanded.
    collapsed_shortcut_groups: std::collections::HashSet<keybindings::ShortcutGroup>,
    /// The Shortcuts page, flattened: section headers interleaved with the
    /// bindings that survived the filter.
    ///
    /// Cached rather than derived in `render`, because the page is virtualized
    /// and its item *count* has to be known before any item is rendered.
    /// Rebuilt by `PaneFlowApp::rebuild_shortcut_rows` whenever the query, the
    /// fold state or `effective_shortcuts` changes - never once per frame.
    shortcut_rows: Vec<crate::settings::tabs::shortcuts::ShortcutListRow>,
    /// Virtualized-list state for [`Self::shortcut_rows`]. Owns the page's
    /// scroll position and the measured item heights.
    shortcut_list: gpui::ListState,
    /// Thumb-drag state for the Shortcuts list scrollbar.
    shortcut_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
    /// Focus handle for the settings page (receives key events during recording/font search).
    settings_focus: FocusHandle,
    /// Cached list of monospace font family names from the system.
    mono_font_names: Vec<String>,
    /// Whether the font family dropdown is open.
    font_dropdown_open: bool,
    /// Filter text for the font dropdown.
    font_search: String,
    /// Whether the Themes page's bundled-theme select is open.
    theme_dropdown_open: bool,
    /// Selected segment on the Themes page (Light/Dark/System). UI state for
    /// now - highlights the active segment, ready to drive theme resolution
    /// once the light theme lands.
    theme_mode: ThemeMode,
    /// Workflow action menu currently open in the sidebar (`None` = closed).
    workspace_menu_open: Option<WorkspaceContextMenu>,
    /// US-010: right-click menu on a sidebar tab row (Rename / Close).
    tab_menu_open: Option<TabContextMenu>,
    /// Pane header context menu (EP-002 US-007), or `None` when closed.
    pane_menu_open: Option<PaneContextMenu>,
    /// Pane to focus on the next render (EP-003 US-009). Set by the drop and
    /// split handlers - which run in a subscription callback without a
    /// `Window` - and consumed in `render`, which has one. One-shot.
    pending_pane_focus: Option<Entity<Pane>>,
    /// Profile menu currently open at the right of the title bar.
    /// Stores the click position so the menu can anchor near the profile
    /// button. `None` = closed.
    profile_menu_open: Option<Point<Pixels>>,
    /// US-053: agent-sessions sidebar state (see `AgentSessionsState`).
    agent_sessions: AgentSessionsState,
    /// Whether the docked Files right sidebar is up (PRD
    /// `prd-files-tree-sidebar-2026-Q3`, EP-001). Mutually exclusive with
    /// `sessions_sidebar_open`. Never persisted - always `false` on launch.
    ///
    /// Live mirror of the visible session's `Tab::files_sidebar_open`, which is
    /// where the desire is actually owned; `sync_files_sidebar_session`
    /// reconciles the two. Being up is still not being on screen: Review and
    /// Settings unmount the rail while this stays set
    /// (`files_sidebar_host_visible`).
    files_sidebar_open: bool,
    /// Width animation for opening/closing the docked Files right sidebar.
    /// Matches the agent-sessions sidebar animation.
    files_sidebar_animation: Option<SidebarWidthAnimation>,
    /// In-memory tree state for the open Files sidebar (root + expanded set +
    /// lazily-cached directory listings). Empty when the sidebar is closed.
    files_tree: app::files_tree::FilesTreeState,
    /// Scroll state for the Files tree body. Re-created on every open so a
    /// fresh sidebar starts at offset 0.
    files_tree_scroll: gpui::ScrollHandle,
    /// Keyboard-selected visible Files row. The index is over visible rows only.
    files_selected: usize,
    /// US-020: the Files sidebar type-to-filter needle. A real single-line
    /// `TextInput`, observed at bootstrap so each keystroke re-renders.
    pub(crate) files_filter_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    /// Focus target for keyboard navigation inside the docked Files sidebar.
    files_focus: FocusHandle,
    /// Surface id of the terminal that opened the Files sidebar. Markdown
    /// rows open into the pane that still owns this surface, falling back to
    /// the active focused pane if that surface is gone.
    files_surface_id: Option<u64>,
    /// Recursive `notify` watcher on the Files tree root (EP-002 US-005).
    /// `None` when the sidebar is closed or the watch could not be installed
    /// (US-006 graceful degradation - the tree then refreshes on expand).
    files_watcher: Option<notify::RecommendedWatcher>,
    /// Receiver for raw watch events, drained + debounced by the background
    /// loop in `bootstrap`. `Some` only while a watcher is installed.
    files_event_rx: Option<std::sync::mpsc::Receiver<notify::Result<notify::Event>>>,
    /// Open right-click context menu for a Files-sidebar row (EP-003 US-009),
    /// or `None` when closed. Mutually exclusive with the other popovers.
    files_menu_open: Option<FilesContextMenu>,
    /// Ephemeral bottom-right toast.
    toast: Option<Toast>,
    /// Pending toasts waiting for the active one to finish. Runtime bursts
    /// should not overwrite user-visible messages.
    toast_queue: std::collections::VecDeque<Toast>,
    /// Dismiss timer for the active toast - dropped on new toast to cancel the old timer.
    _toast_task: Option<gpui::Task<()>>,
    /// Last light/dark value applied to the Windows native backdrop.
    #[cfg(target_os = "windows")]
    windows_backdrop_light: Option<bool>,
    /// US-019 (orchestration-v2): the surface last visited by
    /// `JumpNextWaiting`, so repeated presses cycle through the waiting
    /// agents instead of bouncing on the first one.
    jump_cursor: Option<u64>,
    /// Source pane for swap mode, or `None` if not in swap mode.
    swap_source: Option<Entity<crate::pane::Pane>>,
    /// LIFO stack of recently closed panes for undo-close (US-014).
    closed_panes: Vec<ClosedPaneRecord>,
    /// Whether the "About PaneFlow" dialog is visible.
    show_about_dialog: bool,
    /// Issue #37: the System Info modal, absent when closed. `Collecting`
    /// while the background probes run so the click feels instant.
    system_info_dialog: Option<crate::app::system_info_dialog::SystemInfoDialog>,
    /// Whether the command-palette-style theme picker is visible.
    show_theme_picker: bool,
    /// Typeahead filter for the theme picker (case-insensitive substring).
    theme_picker_query: String,
    /// Index into the *filtered* theme list for the currently highlighted row.
    theme_picker_selected_idx: usize,
    /// Focus handle routing key events to the theme picker while it's open.
    theme_picker_focus: FocusHandle,
    /// Scroll state for the theme picker list (visible scrollbar overlay).
    theme_picker_scroll: gpui::ScrollHandle,
    theme_picker_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
    /// EP-001 US-001/US-003 (cli-cockpit): live Composer session, `None` =
    /// closed. The target pane renders the pushed slot snapshot.
    composer: Option<app::composer::ComposerState>,
    /// EP-001 US-002/US-003 (cli-cockpit): broadcast groups + active index +
    /// per-terminal queued-prompt buffers. Volatile by design (v1).
    broadcast: app::broadcast::BroadcastState,
    /// Broadcast-group picker modal (theme-picker scaffold): visibility,
    /// name-input buffer (create/rename), keyboard cursor, in-place rename
    /// target, inline validation error, and the key-routing focus handle.
    broadcast_picker_open: bool,
    broadcast_picker_query: String,
    broadcast_picker_selected: usize,
    broadcast_picker_renaming: Option<usize>,
    broadcast_picker_error: Option<String>,
    broadcast_picker_focus: FocusHandle,
    /// EP-002 US-004 (cli-cockpit): Attention Queue overlay - visibility,
    /// keyboard cursor, key-routing focus handle. Rows are derived live
    /// from `agent_sessions` on every render, never stored.
    attention_queue_open: bool,
    attention_queue_selected: usize,
    attention_queue_focus: FocusHandle,
    /// EP-006 US-018 (cli-cockpit): fleet-grep overlay state, `None` =
    /// closed. Results are a bounded snapshot (counts + names, never the
    /// match vectors); the fan-out is generation-guarded.
    fleet_search: Option<app::fleet_search::FleetSearchState>,
    fleet_search_generation: u64,
    fleet_search_focus: FocusHandle,
    /// Deferred focus for the fleet overlay (opened from an event handler
    /// that has no `Window` - consumed in `render`, like
    /// `pending_pane_focus`).
    fleet_search_pending_focus: bool,
    /// Keyboard focus for the Agents environment branch picker so its Codex-style
    /// search field captures typing (live filter + new-branch name). Focused on
    /// open; focus returns to the active thread terminal on close.
    /// EP-002 US-005 (cli-cockpit): Launch Pad modal state, `None` = closed.
    launch_pad: Option<app::launch_pad::LaunchPadState>,
    launch_pad_focus: FocusHandle,
    /// EP-005 US-014 (cli-tab-hierarchy): « New pane » preset palette,
    /// `None` = closed. Opened by `New tab` and by the sidebar folder row's
    /// hover `+`.
    pane_palette: Option<app::pane_palette::PanePaletteState>,
    pane_palette_focus: FocusHandle,
    /// Set when the palette is opened from a path that has no `Window` (the
    /// pane-event subscriber); the next render claims focus for it, mirroring
    /// `pending_pane_focus`.
    pending_palette_focus: bool,
    /// US-053: self-update flow state (see `SelfUpdateState`).
    self_update: SelfUpdateState,
    /// State of the "Custom Buttons" management modal opened from the
    /// workspace context menu. `None` = closed.
    custom_buttons_modal: Option<app::custom_buttons_modal::CustomButtonsModal>,
    /// Focus handle routing key events to the custom-buttons modal while open.
    custom_buttons_modal_focus: FocusHandle,
    /// Live telemetry handle (US-012/US-013). Disabled when consent is missing
    /// or `PANEFLOW_NO_TELEMETRY` is set - every `capture`/`flush` call is a
    /// no-op in that state, so callers never branch on consent.
    telemetry: std::sync::Arc<crate::telemetry::client::TelemetryClient>,
    /// Monotonic clock at process start, used to compute
    /// `session_duration_seconds` for the `app_exited` event. Wall-clock-change
    /// proof - a system clock jump mid-session never produces a negative value.
    launch_instant: std::time::Instant,
    /// Last observed `config.telemetry.enabled` value, cached so the config
    /// watcher's reconcile path can detect a transition (US-014) without
    /// re-reading the file.
    telemetry_enabled_last: Option<bool>,
    /// US-006: shared "theme file changed" signal flipped by the theme
    /// watcher's debounce thread (event-driven invalidation). The 50 ms
    /// IPC poll loop in `process_config_changes` drains this flag and
    /// calls `cx.notify()` so the next render picks up the new theme.
    /// `Arc<AtomicBool>` - Send + Sync, lock-free.
    theme_changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// US-053: Git Diff mode state (see `DiffModeState`).
    diff_mode: DiffModeState,
    /// Top-level UI mode: `Cli` = the terminal cockpit, `Diff` = the
    /// full-screen Review surface. Toggled from the sidebar footer and
    /// persisted to / restored from `session.json`.
    pub(crate) mode: paneflow_config::schema::AppMode,
    /// US-053: right-docked git diff panel state, extracted from the
    /// god-struct.
    pub(crate) diff_dock: DiffDockState,
    /// US-048: memoized sidebar display order (worktree grouping). Recomputed
    /// only when the workspace set / order / repo roots change, keyed by a
    /// cheap content signature - `render_sidebar` runs on every app `notify()`,
    /// so the old per-frame `HashMap` + `Vec` rebuild was pure waste. Interior
    /// mutability because the render fn borrows `&self`.
    pub(crate) sidebar_order_cache: std::cell::RefCell<crate::app::sidebar::SidebarOrderCache>,
}

/// Global flag for swap mode, checked by TerminalView to intercept Escape.
/// A process-global `AtomicBool` (rather than threading state through every
/// `TerminalView`) because the check sits on the keystroke hot path.
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

    /// Add a workspace's `.git` directory to the file watcher.
    /// Uses refcounting so multiple workspaces sharing a repo don't conflict.
    /// Silently skipped if the workspace is not in a git repo or watcher is unavailable.
    fn watch_git_dir(&mut self, ws: &Workspace) {
        if let Some(ref git_dir) = ws.git_dir {
            let current = self.git_watch_counts.get(git_dir).copied().unwrap_or(0);
            if current == 0 {
                // First workspace watching this git dir - register with OS.
                // U-018: only commit the refcount when `watch()` succeeds. The
                // old form incremented to 1 before checking, so a transient
                // failure pinned the count at 1 and every later workspace
                // sharing the repo saw count>1 and never retried the
                // registration - the dir stayed permanently unwatched. On
                // failure we return without recording the entry so a later
                // workspace re-attempts the watch.
                if let Some(ref mut watcher) = self.git_watcher
                    && let Err(e) = watcher.watch(git_dir, notify::RecursiveMode::NonRecursive)
                {
                    log::warn!("git watcher: failed to watch {}: {e}", git_dir.display());
                    return;
                }
            }
            *self.git_watch_counts.entry(git_dir.clone()).or_insert(0) += 1;
        }
    }

    /// Remove a workspace's `.git` directory from the file watcher.
    /// Only unwatches when the last workspace using this git dir is removed.
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

    /// Create a new pane wrapping a terminal, and subscribe to its events.
    /// When the pane emits `PaneEvent::Remove` (last tab closed), the pane
    /// is removed from the split tree - following Zed's EventEmitter pattern.
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

    /// Create a pane around an existing surface and subscribe to pane-level
    /// events. Terminal surfaces passed here have already been wired to
    /// app-level terminal events by their original owner; re-subscribing would
    /// duplicate CWD, port-scan, and exit handling.
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

    /// Centralised bookkeeping for a failed update attempt (US-013):
    /// classify the error, log it, update state, show the retry toast,
    /// and bump the attempt counter (which gates the 4th-click escape
    /// hatch).
    pub(crate) fn record_update_failure(
        &mut self,
        context: &str,
        err: &anyhow::Error,
        cx: &mut Context<Self>,
    ) {
        log::error!("self-update/{context}: {err:#}");
        let tag = update::UpdateError::classify(err);
        // US-013 AC #4 - single choke-point for the failure telemetry: the
        // classified `UpdateError` collapses into a canonical
        // `error_category` label so no message string ever leaves the
        // machine. Called before `show_update_error_toast` so the event is
        // queued even if toast rendering panics.
        self.emit_update_failure(&tag);
        self.self_update.self_update_status = update::SelfUpdateStatus::Errored(tag.clone());
        self.self_update.update_attempt_count =
            self.self_update.update_attempt_count.saturating_add(1);
        self.show_update_error_toast(&tag, cx);
        cx.notify();
    }

    // --- Sidebar rendering ---
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
        // Fullscreen kills the behind-window material, so resolve it once here
        // and keep the AppKit view and the shell tint in agreement.
        let chrome_material_suppressed =
            native_material_suppressed_by_fullscreen(window.is_fullscreen());
        #[cfg(target_os = "macos")]
        crate::window_chrome::macos_backdrop::sync_subtle_sidebar_material(
            theme.background.l > 0.5,
            self.cached_config.macos_chrome_material_enabled() && !chrome_material_suppressed,
        );
        // Every mode is cockpit now (Agents first, then Cli, then Diff): the
        // title bar floats above the full window and the right panel reserves
        // a matching strip so content clears window controls.
        let title_bar_h =
            (1.75 * window.rem_size()).max(crate::app::constants::TITLE_BAR_MIN_HEIGHT);
        let settings_open = self.settings_section.is_some();
        let sessions_sidebar_width = self.rendered_sessions_sidebar_width(window);
        let sessions_sidebar_mounted = self.agent_sessions.sessions_sidebar_open
            || self.agent_sessions.sessions_sidebar_animation.is_some();
        let sessions_sidebar_opacity = (sessions_sidebar_width
            / crate::app::sessions_sidebar::SESSIONS_SIDEBAR_WIDTH.max(1.))
        .clamp(0., 1.);
        // The Files rail belongs to the CLI cockpit only. Review and Settings
        // unmount it (`files_sidebar_host_visible`) while `files_sidebar_open`
        // survives, so returning to the cockpit brings the same tree back. The
        // width animation is still advanced off-screen so an in-flight close
        // finishes and releases the tree state instead of freezing mid-way.
        let files_sidebar_host_visible = self.files_sidebar_host_visible();
        // The rail follows the session on screen, so every tab switch, tab
        // close and cross-workspace tab move is reconciled here instead of in
        // each of those mutations. Off the cockpit the rail is unmounted
        // anyway: leave the live state alone and reconcile on the way back.
        if files_sidebar_host_visible {
            self.sync_files_sidebar_session(cx);
        }
        let animated_files_sidebar_width = self.rendered_files_sidebar_width(window);
        let files_sidebar_width = if files_sidebar_host_visible {
            animated_files_sidebar_width
        } else {
            0.
        };
        let files_sidebar_mounted = files_sidebar_host_visible
            && (self.files_sidebar_open || self.files_sidebar_animation.is_some());
        let files_sidebar_opacity = (files_sidebar_width
            / crate::app::files_sidebar::FILES_SIDEBAR_WIDTH.max(1.))
        .clamp(0., 1.);
        let secondary_sidebar_open = sessions_sidebar_mounted || files_sidebar_mounted;
        // The two right rails are mutually exclusive layout children, so the
        // main panel loses exactly one of their widths. Bound once: the Linux
        // blur geometry and the diff dock's width ceiling describe the same
        // strip and must not drift apart.
        let right_rail_width = if sessions_sidebar_mounted {
            sessions_sidebar_width
        } else if files_sidebar_mounted {
            files_sidebar_width
        } else {
            0.
        };
        // Every mode now renders the right area as ONE top-rounded clipped panel
        // (`panel_bg` fill + 16px rail-side top radius + 5px inset), replacing the
        // old Cli/Diff corner-mask trick. GPUI clips the panel's bg fill to the
        // radius, so the window backdrop shows in the corner notch - a clean
        // radius on every platform (Linux, macOS, Windows Mica), where a solid
        // mask would read as a square patch. The 5px inset keeps opaque content
        // (terminal cells, diff rows, settings cards) off the arc, since GPUI
        // does NOT clip children to the radius. The Cli pane grid normally
        // keeps the terminal background; on Windows terminal material it lets
        // the native backdrop show through. Diff / Agents / Settings use the
        // #181818 surface.
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
                // CLI panes are self-contained cards carrying their own fill,
                // so the panel behind them stays empty and lets the window
                // shell read through the gutter between them.
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
        // The primary rail is a flat region of the window shell: no inset
        // card, no border, no isolating mask. Native material therefore spans
        // the rail like any other chrome.
        //
        // Air on the panel's left edge is redundant while the rail is open:
        // the workspace rows already stop `SIDEBAR_ROW_MARGIN_X` short of the
        // rail edge, and stacking the panel inset plus the grid gutter on top
        // of it left the sidebar with 8px of breathing room on its left and
        // 20px on its right (Arthur). Both fade back in as the rail closes, so
        // the panel never jumps at either end of the animation.
        let panel_edge_share = 1. - primary_sidebar_opacity;
        let main_panel_left_inset = crate::app::constants::PANEL_INSET * panel_edge_share;
        let pane_grid_left_gutter = crate::layout::PANE_GUTTER_PX * panel_edge_share;
        let main_panel_corner_mask_bg = panel_corner_mask_bg;
        // Room the main panel actually has between the rails, for children
        // sized in absolute px (the CLI diff dock). The window shell's own
        // client-side inset is not subtracted here: the dock's reserve for the
        // pane grid is an order of magnitude larger than it.
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

        // EP-003 US-009: focus the pane created by a drop-to-split. Deferred
        // here from the `DropSplit` subscription handler (no `Window` there).
        if let Some(pane) = self.pending_pane_focus.take() {
            pane.read(cx).focus_handle(cx).focus(window, cx);
        }
        // EP-005: same deferral for a split picker opened from `PaneEvent::Split`,
        // and drop one whose slot left the screen since the last frame.
        if std::mem::take(&mut self.pending_palette_focus) {
            window.focus(&self.pane_palette_focus, cx);
        }
        // Issue #32: focus routing for the sidebar inline rename. The field
        // only receives keys while it holds the focus, and the pane only gets
        // its keys back once the edit is over, so both transitions are driven
        // here rather than from the row listeners (which have no `Window`).
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
            // Focus moved elsewhere while an edit was live: commit it instead
            // of leaving an orphan box behind, and leave the focus where the
            // click put it.
            self.commit_rename(cx);
            self.rename_focus_live = false;
        }
        self.prune_stale_split_palette(cx);
        let main_content = if self.settings_section.is_some() {
            // Embedded settings take precedence over the mode screen: the left
            // rail becomes the settings nav (below) and this panel shows the
            // active section body. Checked first so Settings opens correctly
            // from Diff mode too.
            self.render_settings_content_panel(cx).into_any_element()
        } else if matches!(self.mode, paneflow_config::schema::AppMode::Diff) {
            // US-003 (prd-git-diff-mode-2026-Q3.md). NOTE: this site is
            // an `if matches!`, not a `match`, so the compiler does NOT
            // force a Diff arm - it must be added by hand or the diff
            // mode would silently fall through to the terminal view.
            self.render_diff_main(cx)
        } else if let Some(ws) = self.active_workspace() {
            if let Some(root) = &ws.active_tab().root {
                let app_weak = cx.weak_entity();
                let on_resize_end = std::rc::Rc::new(move |cx: &mut App| {
                    let _ = app_weak.update(cx, |app, cx| app.save_session(cx));
                });
                // Project focus onto the per-pane unfocused dim before the
                // tree paints. GPUI repaints the window on every focus change
                // and each write is idempotent, so a steady frame is a no-op.
                root.sync_unfocused_dim(window, cx);
                // Gutter around the outermost panes. Applied here rather than
                // inside the recursive tree render, which would compound the
                // gap at every nesting level.
                let outer = div()
                    .flex()
                    .size_full()
                    .pl(px(pane_grid_left_gutter))
                    .pr(px(crate::layout::PANE_GUTTER_PX))
                    .pt(px(crate::layout::PANE_GUTTER_PX))
                    .pb(px(crate::layout::PANE_GUTTER_PX));
                // EP-005: when a split button asked *what* to launch, the
                // picker is injected into the tree at the target pane's slot,
                // so it occupies exactly the half the split will create -
                // wherever that pane sits in the layout. Nothing is split until
                // a preset is picked, so the tree below is still the current
                // layout.
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
                // EP-005 US-014: an empty tab opened by `New tab` / the
                // sidebar `+` shows the preset palette as its surface, inside
                // the same gutter the pane grid uses - it is a pane-sized card,
                // not a floating menu.
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
        // The right diff dock rides beside the CLI pane grid, opened from a
        // pane header. A no-op in every other mode.
        let main_content = self.wrap_cli_diff_dock(main_content, main_panel_width, cx);
        // Update title bar with current workspace name.
        let ws_name = if self.settings_section.is_some() {
            // Settings open: the title-bar center is left empty (the section
            // title lives in the content panel), matching the Codex reference.
            None
        } else {
            self.active_workspace().map(|ws| ws.title.clone())
        };
        // Update CTA state - extracted to `update_pill_info()` so the CLI
        // sidebar banner and the Diff title-bar pill share one source.
        let update_info = self.update_pill_info();
        self.title_bar.update(cx, |tb, _| {
            tb.workspace_name = ws_name;
            tb.sidebar_visible = self.primary_sidebar_visible;
            tb.left_rail_width = title_bar_rail_width;
            tb.files_menu_open = self.title_bar_files_menu_open.is_some();
            tb.help_menu_open = self.title_bar_help_menu_open.is_some();
            tb.update_available = update_info;
            tb.ipc_state = self.ipc_status.state();
            // Cockpit chrome (#141414 + no divider) for both Cli and Diff.
            tb.cockpit = true;
            tb.cockpit_material_active = chrome_material_active;
        });

        // The inner app content (title bar + sidebar + main). UI tree
        // uses Geist (bundled, registered at boot via
        // `Assets::load_fonts`). TerminalElement resolves its own
        // monospace family from `paneflow.json#font_family`, so the
        // terminal output is unaffected.
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
            // US-012: macOS menu-bar actions. `About` opens the in-app About
            // dialog. `Copy` / `Paste` delegate to the existing terminal
            // clipboard actions so Edit > Copy works when a terminal pane is
            // focused (matches the ⌘C keybinding from US-010). `SelectAll` is
            // a no-op until the terminal exposes a select-all action.
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
            // Issue #37: Help > System Info. The collection needs the window
            // (GPU adapter) and pushes its blocking probes to the background
            // executor - see `crate::system_info`.
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
            // EP-001 (cli-cockpit): Composer + broadcast groups.
            .on_action(cx.listener(Self::handle_open_composer))
            .on_action(cx.listener(Self::handle_toggle_broadcast_member))
            .on_action(cx.listener(Self::handle_open_broadcast_groups))
            // EP-002 (cli-cockpit): Attention Queue + Launch Pad.
            .on_action(cx.listener(Self::handle_open_attention_queue))
            .on_action(cx.listener(Self::handle_open_launch_pad))
            .on_action(cx.listener(Self::handle_diff_new_file_tab))
            .on_action(cx.listener(Self::handle_diff_new_terminal_tab))
            // EP-001 US-003: Escape cancels an in-flight tab drag. Capture
            // phase runs ancestor-before-descendant, so this pre-empts the
            // focused terminal's own Escape->PTY forwarding - but only while a
            // drag is active; otherwise we leave the key untouched so normal
            // terminal Escape behaviour is unaffected. Drop-outside-target is
            // handled by GPUI itself (it clears the active drag on mouse-up
            // over a non-target), so no extra wiring is needed there.
            .capture_key_down(cx.listener(|_this, e: &gpui::KeyDownEvent, window, cx| {
                if cx.has_active_drag() && e.keystroke.key == "escape" {
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_move(|_e, _, cx| cx.stop_propagation())
            // Sidebar + main content area. US-008: branch on the
            // top-level UI mode so the CLI sidebar (workspace list) and
            // the Review sidebar swap atomically with the main content.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .overflow_hidden()
                    .relative()
                    // The rounded window surface owns the backdrop. Keeping
                    // this row transparent is load-bearing: GPUI clips child
                    // overflow to a rectangle, so a row fill would repaint the
                    // transparent pixels outside the surface's corner radius.
                    // Sidebar content fades during width animations. Keep a
                    // stable chrome fill behind it so terminal-only material
                    // never exposes unblurred desktop pixels in the rail area.
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
                                .w(px(if sessions_sidebar_mounted {
                                    sessions_sidebar_width
                                } else {
                                    files_sidebar_width
                                }))
                                .bg(panel_corner_mask_bg),
                        )
                    })
                    // While settings is open the left rail becomes the Codex
                    // settings nav (kept visible even if the user had hidden the
                    // primary rail, so the back button is always reachable).
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
                                    // Clear the transparent title-bar overlay so the
                                    // settings header sits below the floating controls.
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
                                // Clear the transparent title-bar overlay so the
                                // first sidebar row sits below the floating
                                // window controls (mirrors the other rails).
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
                                // Clear the transparent title-bar overlay so the
                                // first workspace card sits below the floating
                                // window controls (mirrors the Agents rail).
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
                            // Anchor the absolutely-positioned border contour (below).
                            .relative()
                            .flex()
                            .flex_col()
                            // Every mode renders the right area with the same
                            // 10px corner language and 4px side/bottom inset.
                            // The content remains flush at the top; corner masks
                            // below preserve all four arcs because GPUI does not
                            // clip children to the radius.
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
                                            // Clicking into the pane area is
                                            // "I am looking at this", so it
                                            // answers the visible tab's marks
                                            // only - a sibling tab's dot is
                                            // not something this click saw.
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
                            // Windows Acrylic spans the host window. Cover the
                            // panel's outer insets so native material cannot
                            // continue past the four rounded corner wedges.
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
                            // GPUI clips overflow with a rectangular content
                            // mask, so rounded panel children can still paint
                            // square backgrounds in the corners. These masks
                            // restore the visual radius with surrounding chrome.
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
                    // Docked agent-sessions sidebar (right edge). A layout child
                    // - not an overlay - so it reflows the content and persists
                    // while the user works (PRD agent-sessions-sidebar EP-001).
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
                                // Keep the right rail below the full-width
                                // title bar, aligned with the main panel.
                                .pt(title_bar_h)
                                .child(self.render_sessions_sidebar(window, cx))
                                .into_any_element(),
                        )
                    })
                    // Docked Files sidebar (right edge) - same layout child as
                    // the sessions sidebar, mutually exclusive with it (PRD
                    // files-tree EP-001).
                    .when(files_sidebar_mounted && !sessions_sidebar_mounted, |row| {
                        row.child(
                            div()
                                .flex()
                                .flex_col()
                                .h_full()
                                .w(px(files_sidebar_width))
                                .flex_shrink_0()
                                .overflow_hidden()
                                .opacity(files_sidebar_opacity)
                                // Keep the right rail below the full-width
                                // title bar, aligned with the main panel.
                                .pt(title_bar_h)
                                .child(self.render_files_sidebar(window, cx))
                                .into_any_element(),
                        )
                    }),
            );

        {
            // Codex cockpit: title bar floats as an overlay above the rail and
            // panel. It still owns window drag and custom controls where the
            // platform needs them.
            app_content = app_content.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    // Supported desktop platforms span the full window. The
                    // right panel reserves a matching top strip, so content
                    // clears native or custom window controls.
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

        // EP-001 US-002 (cli-cockpit): broadcast-group picker modal.
        if self.broadcast_picker_open {
            app_content = app_content.child(self.render_broadcast_picker(cx));
        }

        // EP-002 (cli-cockpit): Attention Queue overlay + Launch Pad modal.
        // Mode-gated (review R3): a mode switch while a launch runs in the
        // background must not paint cockpit chrome over Agents/Diff - the
        // modal reappears (or finishes) back in Cli mode.
        let in_cli_mode = matches!(self.mode, paneflow_config::schema::AppMode::Cli);
        if self.attention_queue_open && in_cli_mode {
            app_content = app_content.child(self.render_attention_queue(cx));
        }
        if self.launch_pad.is_some() && in_cli_mode {
            app_content = app_content.child(self.render_launch_pad(cx));
        }
        // EP-006 US-018: fleet-grep results overlay (same mode gate). The
        // deferred focus (the trigger event has no Window) lands here.
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

        // US-010 (prd-cli-tab-hierarchy): sidebar tab row context menu.
        if let Some(menu) = self.tab_menu_open
            && self
                .workspaces
                .get(menu.ws_idx)
                .is_some_and(|ws| menu.tab_idx < ws.tab_count())
        {
            app_content = app_content.child(self.render_tab_context_menu(menu, ui, window, cx));
        }

        // EP-002 US-007: pane header context menu.
        if let Some(menu) = self.pane_menu_open.clone() {
            app_content = app_content.child(self.render_pane_context_menu(menu, ui, window, cx));
        }

        // files-tree EP-003 US-009: per-file copy-path context menu.
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

// ---------------------------------------------------------------------------
// `--update-and-exit` (US-005 e2e auto-update harness)
// ---------------------------------------------------------------------------

/// Synchronous self-update entry point invoked by the e2e harness
/// (`scripts/test-update-e2e.sh`). Mirrors the GUI flow's check + per-format
/// install steps but never initializes GPUI - so it runs cleanly in headless
/// CI containers without Xvfb. Honours `PANEFLOW_UPDATE_FEED_URL`
/// ([`update::checker::update_feed_url`]) so the harness can point the
/// checker at a localhost fixture.
///
/// Returns the process exit code (see `--update-and-exit` doc-comment in
/// `main` for the full table). The split between exit-3 (feed unreachable)
/// and exit-1 (other) satisfies AC6 - the harness asserts a specific code,
/// not a substring of the generic "update failed" toast.
fn run_update_and_exit() -> i32 {
    use crate::update::checker::{UpdateStatus, check_github_release};
    use crate::update::install_method::{self, InstallMethod};

    let method = install_method::detect();
    log::info!("--update-and-exit: install method = {method:?}");

    // The harness MUST NOT emit telemetry - the test runs are not user
    // sessions and would skew funnels. Use a disabled client (no-op
    // capture, no HTTP).
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
            // The checker logs whether the failure was DNS/HTTP/parse via
            // `log::warn!`; we can't easily distinguish here without a
            // structured error, so print the explicit feed-unreachable
            // hint per AC6 - the dominant failure mode the harness
            // exercises (kill miniserve before invocation).
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
            // AC3a deferred: appimageupdatetool isn't part of the default
            // CI image, and it has no in-process SHA verify path (the tool
            // fetches via embedded zsync metadata). The tar.gz path covers
            // the same regression surface (download + SHA verify + atomic
            // swap + restart-path). Leaving the wiring in place so a
            // follow-up can opt in by installing the tool.
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
        // SystemPackage (.deb/.rpm/dnf/apt) updates need pkexec + a
        // running polkit agent - neither belongs in `--update-and-exit`,
        // which is designed to be deterministic and non-interactive.
        // AppBundle/WindowsMsi: the e2e harness is Linux-only (US-014
        // covers Windows e2e separately).
        other => {
            eprintln!(
                "paneflow-update: --update-and-exit does not support install method {other:?}"
            );
            5
        }
    }
}

// ---------------------------------------------------------------------------
// App entry point
// ---------------------------------------------------------------------------

/// Whether a Windows startup console belongs only to PaneFlow and can be shed.
///
/// Console-subsystem executables launched from Explorer / Start Menu get a new
/// console before Rust code runs. When that console contains only PaneFlow, the
/// launch is a GUI launch and the console is visual noise. When it contains the
/// parent shell too, the user is running a CLI/scriptable path and stdout,
/// stderr, waiting, and exit codes must remain intact.
#[cfg(windows)]
fn should_detach_windows_console(
    is_scriptable_invocation: bool,
    console_process_count: u32,
) -> bool {
    !is_scriptable_invocation && console_process_count == 1
}

/// Detach the one-process console Windows creates for Explorer/Start launches.
#[cfg(windows)]
fn detach_lonely_windows_console_for_gui_launch(is_scriptable_invocation: bool) {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};

    let mut processes = [0_u32; 2];
    // SAFETY: GetConsoleProcessList writes at most the buffer length we pass
    // and returns the number of attached console processes. A return larger
    // than the buffer means "there are multiple processes", which is exactly
    // the keep-attached case for terminal-launched CLI paths.
    let count = unsafe { GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) };
    if should_detach_windows_console(is_scriptable_invocation, count) {
        // SAFETY: FreeConsole only detaches this process from its console. It
        // has no Rust aliasing or lifetime implications; failure is harmless
        // and simply leaves the console visible.
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
        // Shortcuts-page key capture and rebind recording have to see a chord
        // *before* GPUI matches it against the keymap, or pressing Cmd+Q to
        // find (or rebind) Quit simply quits. `intercept_keystrokes` is the
        // only hook that runs that early; a key-down listener never fires at
        // all once a binding has been dispatched and handled.
        let weak = cx.weak_entity();
        cx.intercept_keystrokes(move |event, window, cx| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            // This runs for every keystroke in the app, terminal typing
            // included, so the common answer has to be cheap. `Entity::update`
            // opens an update cycle and flushes the effect queue on exit;
            // doing that mid-key-dispatch just to read one field would put a
            // flush on the keystroke-to-pixel path AGENTS.md protects. Read
            // the gate first and only take the update when the page is open.
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
            // US-013 AC #2 - final chance to flush `app_exited` when the OS
            // close button or a keyboard shortcut closes the last window.
            app.emit_app_exited_and_flush();
            #[cfg(target_os = "linux")]
            crate::window_chrome::linux_backdrop::clear_subtle_chrome_material();
            cx.quit();
            false
        }
    });
    // US-116 (prd-agent-ui-refactor-2026-Q3.md): track window-activation state
    // for OS notification gating.
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
    // Handle --help and --version before initializing GPUI
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
    // US-038: detect the `mcp` subcommand BEFORE the global flag scans. Those
    // scans look at *every* arg, so `paneflow mcp install --help` would
    // otherwise match the global `--help` and print the top-level help instead
    // of routing to the `mcp` handler (which forwards `--help` to its own
    // subcommand parser). Gating the global scans on `!is_mcp_subcommand`
    // hands `paneflow mcp …` straight to the dispatcher below.
    let is_mcp_subcommand = args.get(1).map(String::as_str) == Some("mcp");
    // EP-001 (cli-agent-orchestration): same gating rationale as the `mcp`
    // flag. When argv[1] is a known CLI verb (`paneflow ls --help`,
    // `paneflow read … --json`), the global flag scans below must NOT fire -
    // clap owns per-subcommand `--help`/`--version`, and the CLI dispatch runs
    // after the manual intercepts.
    let is_cli_subcommand = cli::is_cli_verb(args.get(1).map(String::as_str));
    // EP-004 (cli-agent-orchestration): `paneflow hooks <cmd>` is intercepted
    // before clap (like `mcp`) and mutates agent config files offline - so the
    // global flag scans must not eat its `--help`.
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

    // US-011 (cli-hardening-followup-2026-Q3): scrub the `CLAUDECODE`
    // env var BEFORE any thread::spawn / tokio runtime / smol /
    // GPUI init reads or mutates env. Rust 1.85 made
    // `std::env::remove_var` `unsafe` precisely because it races
    // with concurrent `getenv` calls; the only race-free place to
    // mutate process env is the top of `main()` before any other
    // thread exists.
    // SAFETY: this is still before env_logger, GPUI, IPC, config watchers,
    // async executors, and any app-owned thread.
    unsafe { agents::parent_guard::scrub_claudecode_env_before_threads() };

    // Quiet by default: a plain `cargo run` (or a shipped binary) shows only
    // warnings + errors. `RUST_LOG=info` restores the startup/runtime
    // diagnostics (GPU selection, IPC, session restore, …) and `RUST_LOG=debug`
    // adds the per-operation diff/git trace - matching the documented
    // "RUST_LOG=info cargo run # with logging" workflow.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(
        "warn,wgpu_hal=off,naga=warn,gpui_macos::text_system=error,zbus=warn,zbus::proxy=error,tracing::span=warn",
    ))
    .init();

    // US-003: install the process-wide kill-on-parent-death guard BEFORE any
    // agent CLI or ConPTY spawns so children inherit the Job Object (Windows).
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

    // Adopt the user's login-shell environment only for the real GUI path
    // (Finder / Dock / `.desktop`), where the inherited launchd / systemd-user
    // PATH omits Homebrew, Nix, version managers, and `~/.zprofile` additions.
    // Scriptable CLI/MCP/hooks/update invocations must not execute an
    // interactive login shell as a side effect. Runs before the static prepend
    // below so per-user bin dirs stay first. Must run before any other thread
    // spawns - it mutates the process environment (see the module's safety note).
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

    // Patch PATH BEFORE GPUI starts so agent launch and CLI helper lookups find
    // binaries installed under `~/.bun/bin` when Paneflow is launched from a
    // `.desktop` file / Finder / Start Menu (those inherit a minimal
    // systemd-user / launchd / Explorer PATH that does not source the user's
    // shell rc). Must run before any other thread spawns - see safety note on
    // `augment_path_for_gui_launch`.
    runtime_paths::augment_path_for_gui_launch();

    // US-005: synchronous update flow for the e2e harness. Runs the same
    // checker + per-format installer the GUI calls, but without ever
    // initializing GPUI - exits with status 0 on a successful swap, 2 on
    // "no update needed", 3 on a feed-unreachable error (AC6's explicit
    // "feed unreachable" requirement vs the generic "update failed"),
    // 4 on integrity / hash mismatch, 5 on unsupported install method,
    // 1 on any other error. Pair with `PANEFLOW_UPDATE_FEED_URL` to
    // point the checker at a localhost fixture.
    // Gate the global `--update-and-exit` scan on the SAME three intercepts as
    // the `--help`/`--version` scans above, not just `mcp`. Otherwise a literal
    // `--update-and-exit` token appearing as a CLI/hooks *argument* (e.g.
    // `paneflow send <t> "--update-and-exit"`, `paneflow search x --update-and-exit`)
    // is captured by this `args.iter().any(...)` scan and hijacks the verb into
    // the self-updater (US-002: "pas de capture par un scan global").
    if is_update_and_exit {
        std::process::exit(run_update_and_exit());
    }

    // EP-002 US-004: `paneflow mcp <subcommand>` runs as a scriptable CLI
    // and exits - it never initializes GPUI / opens a window. Placed after
    // `augment_path_for_gui_launch` (so agent-CLI detection sees `~/.bun/bin`
    // etc.) and after `--update-and-exit`, before any GUI bootstrap. The
    // install engine lives in the GPU-free `paneflow-mcp-install` crate.
    // Only `install` extracts the bridge; `status` and `uninstall` must stay
    // read-only with respect to Paneflow's own data dir.
    // Diagnostics go to stderr (env_logger), the per-agent report to stdout.
    if args.get(1).map(String::as_str) == Some("mcp") {
        let bridge_path = if should_extract_mcp_bridge_for_cli(&args) {
            match ai_hooks::extract::ensure_bridge_extracted() {
                Ok(p) => Some(p),
                Err(e) => {
                    log::warn!("paneflow mcp: bridge extraction failed ({e:#})");
                    // Fall back to the resolved-but-maybe-missing path so the
                    // engine can emit the precise "binary missing at <path>"
                    // refusal rather than a vaguer "data dir unresolved".
                    runtime_paths::bridge_binary_path()
                }
            }
        } else {
            runtime_paths::bridge_binary_path()
        };
        std::process::exit(paneflow_mcp_install::run_cli(&args[2..], bridge_path));
    }

    // EP-004 (cli-agent-orchestration): `paneflow hooks <cmd>` installs the
    // persistent agent-notification hooks and exits - like `mcp`, it mutates
    // external config files offline and never initializes GPUI. Extract the
    // ai-hook callback to its stable path first so the path written into agent
    // configs is guaranteed to exist; fall back to the resolved-but-maybe-
    // missing path so the engine can emit a precise refusal.
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

    // EP-001 (cli-agent-orchestration): the `paneflow <verb>` scriptable CLI
    // drives a RUNNING instance over the existing IPC socket and exits - it
    // never initializes GPUI. Gated on a known verb in argv[1] (same pattern as
    // `mcp`) so unknown args still fall through to the GUI below. Placed after
    // the logger + PATH augmentation so the CLI inherits `RUST_LOG` and the
    // same binary-resolution environment as the GUI.
    if is_cli_subcommand {
        std::process::exit(cli::run());
    }

    // EP-005 US-011: an argv[1] shaped like a verb but not one we own
    // (`paneflow blah`, a mistyped `paneflow searh`, or the MCP tool name had
    // an alias not been wired) is a typo, not a GUI launch. The `mcp`/`hooks`/
    // known-verb intercepts above have all exited by now, so anything still
    // here is genuinely unknown: print an actionable error and exit non-zero
    // (clap's usage-error code 2) instead of falling through to the bootstrap,
    // which would silently trip the single-instance guard. A bare `paneflow`
    // (no argv[1]) and any `-`/`--` flag are NOT flagged, so the GUI and the
    // global-flag scans keep their existing behaviour.
    if is_unknown_verb && let Some(verb) = args.get(1) {
        eprintln!("paneflow: unknown verb '{verb}'; see `paneflow --help` for the verb list");
        std::process::exit(2);
    }

    warn_if_legacy_run_install();
    #[cfg(target_os = "macos")]
    warn_if_rosetta_translated();

    // EP-001 US-003: materialize the embedded `paneflow-mcp` bridge to its
    // stable, non-versioned path so a registered MCP server keeps resolving
    // across Paneflow updates. SHA-compared + atomic: a no-op when the
    // on-disk bytes already match the embedded version. Non-fatal - the GUI
    // must still open if data_dir is unwritable; `paneflow mcp install`
    // (EP-002) refuses cleanly later rather than write a dangling path.
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

    application()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            // Load config early - needed for keybindings and window decorations
            let config = paneflow_config::loader::load_config();
            // Match Windows Terminal/PowerShell-style grayscale text
            // antialiasing. GPUI's platform default can pick subpixel
            // rendering on Windows/Linux; Paneflow's dark terminal surfaces
            // read cleaner without colored LCD fringes on thin mono glyphs.
            cx.set_text_rendering_mode(gpui::TextRenderingMode::Grayscale);
            // `apply_keybindings` clears the whole registry, so it now also
            // (re-)registers the TextInput / TextArea widget bindings itself
            // (US-016: agents composer textarea included) - no separate startup
            // call is needed, and a later re-apply can no longer strip them.
            keybindings::apply_keybindings(cx, &config.shortcuts);

            // Register every embedded `.ttf` under `assets/fonts/` BEFORE
            // any window opens, so GPUI's text system can resolve the
            // `Geist Mono` family (mono) and `Geist`
            // family (sans, 4 weights) Paneflow ships as the default
            // primaries - same strategy Zed uses with `.ZedMono` /
            // `.ZedSans` (`zed/assets/settings/default.json:29,57`).
            // Picking embedded families as the **primary** instead of
            // system families (Menlo / Cascadia Mono / DejaVu) sidesteps
            // the c3e2331 failure mode: Core Text inside a signed .app
            // bundle could return valid glyph_ids for a system family
            // and rasterize them as empty bitmaps; GPUI's per-Font
            // fallback chain only walks on missing-glyph not on
            // empty-raster, so the system primary "rendered" zero glyphs
            // and nothing fell through. With Geist Mono as the registered
            // primary, GPUI owns the font tables end-to-end. Iterates
            // the rust-embed registry (Zed pattern,
            // `zed/crates/assets/src/assets.rs:42`) so adding a new font
            // face is "drop a .ttf into assets/fonts/" with no Rust
            // change needed.
            if let Err(e) = assets::Assets.load_fonts(cx) {
                log::warn!(
                    "Assets::load_fonts failed: {e}; text rendering may fail on \
                     systems without a system monospace font"
                );
            }

            // US-012: macOS native menu bar. On Linux/Windows the call is
            // elided - GPUI's non-macOS platforms don't render a menu bar
            // and AC5 forbids any Linux UI change.
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

            // US-011: reserve space on the left of the custom titlebar
            // for macOS traffic lights. The three red/yellow/green circles
            // live at x≈12-78px; the sidebar-aligned title-bar slot starts at
            // x=80 (see title_bar.rs). y=10 centers the 12px lights in the
            // 32px bar (`TITLE_BAR_MIN_HEIGHT`). `..Default::default()` is load-bearing on
            // non-macOS (GPUI's TitlebarOptions may grow platform-specific
            // fields we don't set); clippy only flags it needless under
            // target_os = "macos" where traffic_light_position makes the
            // field list complete.
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
