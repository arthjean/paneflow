use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Context, Decorations, EventEmitter, IntoElement,
    MouseButton, Render, Styled, Transformation, Window, WindowControlArea, div, percentage,
    prelude::*, px, svg,
};

use super::csd::default_button_layout;

pub struct TitleBar {
    should_move: bool,
    pub workspace_name: Option<String>,
    pub sidebar_visible: bool,
    pub rosetta_enabled: bool,
    pub rosetta_button_enabled: bool,
    pub rosetta_surface_open: bool,
    pub files_menu_open: bool,
    pub help_menu_open: bool,
    pub ipc_state: crate::ipc::IpcState,
    /// Set by PaneFlowApp when a newer version is detected.
    pub update_available: Option<UpdateInfo>,
    /// US-010 (prd-agents-ui-codex-redesign-2026-Q3.md): the brand slot's
    /// primary text in Agents mode (current thread/chat title, or a neutral
    /// "Agents"/project label in the picker state). `None` in Cli/Diff leaves
    /// the brand slot empty. PUSHED by `PaneFlowApp::render` only on the Agents
    /// arm; `TitleBar` never reads `AppMode` - the render branch tests the
    /// presence of this field, not the mode (push-only contract).
    pub agents_thread_title: Option<String>,
    /// US-010: the secondary "· context" text (project name for a project
    /// thread, "Chat" for a free chat). `None` in the picker state and in
    /// Cli/Diff.
    pub agents_context_label: Option<String>,
    /// US-011: render the `⋯` overflow button. `true` only when a concrete
    /// thread/chat is selected (the menu acts on the current target);
    /// `false` in the picker state and in Cli/Diff.
    pub agents_overflow: bool,
    /// Codex cockpit polish: in Agents mode the right area is a floating
    /// rounded panel, so the title bar drops its 1px bottom divider for a
    /// seamless chrome. `false` in Cli/Diff keeps the divider (diff visuel
    /// nul). PUSHED by `PaneFlowApp::render`; `TitleBar` never reads `AppMode`.
    pub is_agents: bool,
    /// Cockpit chrome for the Cli mode: paint the rail `#141414` and
    /// drop the bottom divider so the title bar + sidebar read as one
    /// continuous surface, matching the Agents cockpit. `false` in Diff keeps
    /// the themed chrome + divider (Diff stays frozen). Mutually exclusive with
    /// `is_agents` (Agents paints nothing). PUSHED by `PaneFlowApp::render`;
    /// `TitleBar` never reads `AppMode`.
    pub cockpit: bool,
    /// Whether cockpit chrome should let the native material show through.
    /// Pushed by `PaneFlowApp::render` so the Windows Appearance switch can
    /// control title bar transparency independently from terminal cells.
    pub cockpit_material_active: bool,
    /// #10: subscription that repaints the title bar when the desktop
    /// environment relocates the window-control buttons (e.g. GNOME left↔right).
    /// Registered lazily on the first `render` (where `window` is available, as
    /// `new` has none); `None` until then. Dropping it on `TitleBar` drop
    /// unregisters the observer.
    button_layout_observer: Option<gpui::Subscription>,
}

#[derive(Clone)]
pub struct UpdateInfo {
    pub version: String,
    /// Which pill to render - the in-app flow or the system-package hint.
    pub kind: UpdatePillKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UpdatePillKind {
    /// In-app self-update flow (AppImage / tar.gz / unknown fallback).
    InApp(SelfUpdatePillState),
    /// Managed by the host's package manager (US-012). Clicking the pill
    /// never downloads - it shows a toast with the exact upgrade command.
    SystemManaged(SystemPackageKind),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SelfUpdatePillState {
    Idle,
    Downloading,
    Installing,
    /// Background install completed; the next click only invokes
    /// `cx.restart()`. Mirrors Zed's "Restart to Update" CTA - the heavy
    /// work happened while the user was busy doing something else, so the
    /// click→restart latency is bounded by GPUI's relauncher only (~100 ms).
    ReadyToRestart,
    Errored,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SystemPackageKind {
    /// Immutable Fedora variants (Silverblue / Kinoite / Bazzite) -
    /// detected via `/run/ostree-booted`. The pill surfaces a
    /// `rpm-ostree upgrade` hint rather than the usual `dnf`/`apt` copy.
    RpmOstree,
    /// `SystemPackage` was detected but neither apt, dnf, zypper, nor ostree
    /// markers were present (e.g., `eopkg` on Solus, `xbps` on Void).
    /// Apt/Dnf are intentionally absent: they route through the in-app
    /// pkexec installer (UpdatePillKind::InApp), not SystemManaged.
    Other,
}

/// Internal visual/interaction mode for the update pill.
#[derive(Clone, Copy)]
enum PillStyle {
    /// Default accent pill; pointer cursor + hover fade. Dispatches the
    /// in-app update action.
    Clickable,
    /// De-emphasized, non-interactive (download/install in flight).
    Busy,
    /// System-managed install: de-emphasized, default cursor, still clickable
    /// to reveal the package-manager hint toast.
    SystemHint,
}

impl TitleBar {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            should_move: false,
            workspace_name: None,
            sidebar_visible: true,
            rosetta_enabled: true,
            rosetta_button_enabled: true,
            rosetta_surface_open: false,
            files_menu_open: false,
            help_menu_open: false,
            ipc_state: crate::ipc::IpcState::Online,
            update_available: None,
            agents_thread_title: None,
            agents_context_label: None,
            agents_overflow: false,
            is_agents: false,
            cockpit: false,
            cockpit_material_active: !cfg!(target_os = "windows"),
            button_layout_observer: None,
        }
    }
}

pub enum TitleBarEvent {
    CloseRequested,
    ToggleSidebar,
    ToggleRosettaSurface,
    ToggleFilesMenu(gpui::Point<gpui::Pixels>),
    ToggleHelpMenu(gpui::Point<gpui::Pixels>),
}

impl EventEmitter<TitleBarEvent> for TitleBar {}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // #10: repaint when the desktop environment relocates the window-control
        // buttons (GNOME left↔right) so `cx.button_layout()` below is never
        // stale until some unrelated repaint forces a frame. Registered once
        // here (not in `new`, which has no `Window`); the `Subscription` lives
        // in `self`. Mirrors Zed (`title_bar.rs:488`).
        if self.button_layout_observer.is_none() {
            self.button_layout_observer =
                Some(cx.observe_button_layout_changed(window, |_, _, cx| cx.notify()));
        }

        let height = (1.75 * window.rem_size()).max(px(34.));
        let decorations = window.window_decorations();
        let is_csd = matches!(decorations, Decorations::Client { .. });
        // #9: under real server-side decorations (`window_decorations: server`,
        // opt-in; e.g. KDE Plasma) the compositor draws its own caption bar AND
        // this custom bar renders below it - they double up. We can't simply
        // drop this bar under SSD: it carries app chrome the compositor caption
        // does NOT (sidebar toggle, Files/Help menus, workspace tabs). The
        // min/max/close pill IS gated on `is_csd` below so those don't double;
        // the brand/menus row is best-effort under SSD. The default `client`
        // (CSD) path - which PaneFlow uses everywhere it can - avoids this
        // entirely, which is why it is the default.

        // --- Title bar background from theme, switching on window focus ---
        let theme = crate::theme::active_theme();
        let is_window_active = window.is_window_active();
        let bg_color = if is_window_active {
            theme.title_bar_background
        } else {
            theme.title_bar_inactive_background
        };
        let chrome_bg = crate::app::constants::cockpit_chrome_background(
            bg_color,
            is_window_active,
            self.cockpit_material_active,
        );

        // --- Read DE button layout ---
        let layout = cx.button_layout().unwrap_or_else(default_button_layout);
        let is_maximized = window.is_maximized();
        let supported = window.window_controls();

        // Close handler: emit CloseRequested so `PaneFlowApp` can intercept
        // (e.g., session save) before the window is removed.
        let close_handle = cx.entity().downgrade();
        let on_close = move |_window: &mut Window, cx: &mut gpui::App| {
            if let Some(entity) = close_handle.upgrade() {
                entity.update(cx, |_this, cx| cx.emit(TitleBarEvent::CloseRequested));
            }
        };

        // Paint our own window controls under CSD (Linux) and always on
        // Windows, where the transparent titlebar (`appears_transparent: true`)
        // hides the native caption buttons while gpui still reports
        // `Decorations::Server` - so `is_csd` is false and, without this guard,
        // the minimize/maximize/close buttons vanish entirely on Windows.
        // macOS keeps its native traffic lights, so it stays gated on `is_csd`
        // (false there). Mirrors the settings title bar (settings/window.rs).
        let render_controls = is_csd || cfg!(target_os = "windows");

        let left_controls = if render_controls {
            super::csd::render_button_group(
                "l",
                &layout.left,
                is_maximized,
                height,
                &supported,
                on_close.clone(),
            )
        } else {
            None
        };

        let right_controls = if render_controls {
            super::csd::render_button_group(
                "r",
                &layout.right,
                is_maximized,
                height,
                &supported,
                on_close,
            )
        } else {
            None
        };

        // --- Left section: brand slot, fixed width aligned with sidebar ---
        let ui = crate::theme::ui_colors();
        // US-011: on macOS, reserve the leftmost ~80px of the custom titlebar
        // for the native red/yellow/green traffic lights (positioned at
        // x=12,y=12 by WindowOptions::titlebar::traffic_light_position in
        // main.rs). On Linux the window controls are rendered elsewhere,
        // so the brand keeps the historical `pl_3()` (12px) padding.
        //
        // In macOS fullscreen AppKit hides the traffic lights, so the 80px
        // reservation would leave a dead gap before the brand cluster - drop
        // back to 12px there (matches Zed's `is_fullscreen()` gate).
        let brand_pl = if cfg!(target_os = "macos") && !window.is_fullscreen() {
            gpui::px(80.0)
        } else {
            gpui::px(12.0)
        };
        let toggle_sidebar_handle = cx.entity().downgrade();
        let toggle_rosetta_handle = cx.entity().downgrade();
        let toggle_files_menu_handle = cx.entity().downgrade();
        let toggle_help_menu_handle = cx.entity().downgrade();
        let sidebar_tooltip: gpui::SharedString = if self.sidebar_visible {
            "Hide sidebar"
        } else {
            "Show sidebar"
        }
        .into();
        let rosetta_enabled = self.rosetta_enabled;
        let rosetta_button_enabled = rosetta_enabled && self.rosetta_button_enabled;
        let rosetta_open = rosetta_button_enabled && self.rosetta_surface_open;
        let rosetta_tooltip: gpui::SharedString = if !rosetta_enabled {
            "Rosetta disabled"
        } else if !rosetta_button_enabled {
            "Available in CLI and Agents"
        } else if rosetta_open {
            "Hide Rosetta"
        } else {
            "Show Rosetta"
        }
        .into();
        let rosetta_icon = if rosetta_enabled {
            "icons/bell.svg"
        } else {
            "icons/bell-off.svg"
        };
        let mut brand = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .pl(brand_pl)
            .pr(px(2.))
            .overflow_x_hidden()
            .child(
                div()
                    .id("toggle-primary-sidebar")
                    .flex_none()
                    .w(px(24.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(!self.sidebar_visible, |d| {
                        d.bg(crate::app::constants::sidebar_tab_active_background())
                    })
                    .hover(|s| s.bg(crate::app::constants::sidebar_tab_hover_background()))
                    .tooltip(move |_window, cx| {
                        let label = sidebar_tooltip.clone();
                        cx.new(|_| crate::app::sidebar::SidebarTooltip { label })
                            .into()
                    })
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        if let Some(entity) = toggle_sidebar_handle.upgrade() {
                            entity.update(cx, |_this, cx| {
                                cx.emit(TitleBarEvent::ToggleSidebar);
                            });
                        }
                    })
                    .child(
                        svg()
                            .size(px(14.))
                            .path("icons/sidebar.svg")
                            .text_color(ui.muted),
                    ),
            )
            .child(
                div()
                    .id("toggle-rosetta-surface")
                    .flex_none()
                    .w(px(24.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .when(rosetta_button_enabled, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(crate::app::constants::sidebar_tab_hover_background()))
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                cx.stop_propagation();
                                if let Some(entity) = toggle_rosetta_handle.upgrade() {
                                    entity.update(cx, |_this, cx| {
                                        cx.emit(TitleBarEvent::ToggleRosettaSurface);
                                    });
                                }
                            })
                    })
                    .when(rosetta_open, |d| {
                        d.bg(crate::app::constants::sidebar_tab_active_background())
                    })
                    .tooltip(move |_window, cx| {
                        let label = rosetta_tooltip.clone();
                        cx.new(|_| crate::app::sidebar::SidebarTooltip { label })
                            .into()
                    })
                    .child(
                        svg()
                            .size(px(14.))
                            .path(rosetta_icon)
                            .text_color(if rosetta_open { ui.text } else { ui.muted }),
                    ),
            )
            .child(
                div()
                    .id("title-bar-files-menu-trigger")
                    .flex_none()
                    .h(px(24.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(if self.files_menu_open {
                        ui.text
                    } else {
                        ui.muted
                    })
                    .when(self.files_menu_open, |d| {
                        d.bg(crate::app::constants::sidebar_tab_active_background())
                    })
                    .hover(|s| {
                        s.bg(crate::app::constants::sidebar_tab_hover_background())
                            .text_color(ui.text)
                    })
                    .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                        cx.stop_propagation();
                        if let Some(entity) = toggle_files_menu_handle.upgrade() {
                            let anchor = gpui::point(event.position.x, height);
                            entity.update(cx, |_this, cx| {
                                cx.emit(TitleBarEvent::ToggleFilesMenu(anchor));
                            });
                        }
                    })
                    .child("Files"),
            )
            .child(
                div()
                    .id("title-bar-help-menu-trigger")
                    .flex_none()
                    .h(px(24.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(if self.help_menu_open {
                        ui.text
                    } else {
                        ui.muted
                    })
                    .when(self.help_menu_open, |d| {
                        d.bg(crate::app::constants::sidebar_tab_active_background())
                    })
                    .hover(|s| {
                        s.bg(crate::app::constants::sidebar_tab_hover_background())
                            .text_color(ui.text)
                    })
                    .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                        cx.stop_propagation();
                        if let Some(entity) = toggle_help_menu_handle.upgrade() {
                            let anchor = gpui::point(event.position.x, height);
                            entity.update(cx, |_this, cx| {
                                cx.emit(TitleBarEvent::ToggleHelpMenu(anchor));
                            });
                        }
                    })
                    .child("Help"),
            );
        if self.is_agents {
            // Agents: no brand text in the chrome - the thread/chat name is
            // already shown in the rail (active row) and in the terminal
            // itself, and the rail-width title bar would only clip it. Keep the
            // slot empty so just the window controls remain top-left.
        } else if let Some(title) = self.agents_thread_title.clone() {
            // US-010: contextual brand in Agents mode - `thread title · context`.
            // The title truncates first; the context label and the `⋯` button
            // stay pinned.
            let mut label_row = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(ui.text)
                        .text_sm()
                        .font_weight(gpui::FontWeight::BOLD)
                        .truncate()
                        .child(title),
                );
            if let Some(ctx) = self.agents_context_label.clone() {
                label_row = label_row
                    .child(
                        div()
                            .flex_none()
                            .text_color(ui.muted)
                            .text_size(px(12.))
                            .child("·"),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(110.))
                            .text_color(ui.muted)
                            .text_size(px(12.))
                            .truncate()
                            .child(ctx),
                    );
            }
            brand = brand.child(label_row);
            if self.agents_overflow {
                // US-011: `⋯` overflow button. on_mouse_down + stop_propagation
                // (NOT on_click) - same Wayland first-press / drag race the
                // update pill documents (title_bar.rs ~354). Dispatches a typed
                // action; `PaneFlowApp` resolves the current target and opens
                // the shared thread context menu.
                brand = brand.child(
                    div()
                        .id("agents-overflow-btn")
                        .flex_none()
                        .w(px(22.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .cursor_pointer()
                        .text_color(ui.muted)
                        .text_size(px(15.))
                        .hover(|s| {
                            s.bg(crate::app::constants::sidebar_tab_hover_background())
                                .text_color(ui.text)
                        })
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            cx.stop_propagation();
                            window.dispatch_action(Box::new(crate::OpenAgentsThreadMenu), cx);
                        })
                        .child("⋯"),
                );
            }
        }
        let brand = brand;

        // --- Center section: workspace name breadcrumb (muted) ---
        // Takes the remaining flex space and centers the current workspace
        // name. Acts as drag area when the workspace is unnamed / unset.
        // Cockpit (Cli): the breadcrumb is dropped - the workspace name already
        // anchors the sidebar, so the title bar centre stays a clean drag area.
        // Diff keeps it.
        let mut content = div()
            .flex_1()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .px(px(12.))
            .min_w_0();
        if !self.cockpit
            && let Some(name) = self.workspace_name.as_ref()
        {
            content = content.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .child(
                        div()
                            .w(px(3.))
                            .h(px(3.))
                            .rounded_full()
                            .bg(ui.muted)
                            .flex_none(),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(ui.muted)
                            .truncate()
                            .child(name.clone()),
                    ),
            );
        }

        // --- Update available pill ---
        // Cockpit modes (Agents + Cli): the bar is a rail-confined overlay
        // entirely filled by the brand slot, so the pill would never be
        // visible - its cockpit home is the sidebar update banner
        // (`render_sidebar_update_banner`). Diff keeps the title-bar pill.
        let update_pill_visible = !self.is_agents && !self.cockpit;
        let update_pill = update_pill_visible
            .then(|| self.update_available.clone())
            .flatten()
            .map(|info| {
                // Decide label + visual style per install method. The click handler
                // always dispatches `StartSelfUpdate`; the action handler in
                // `PaneFlowApp` decides whether to download (in-app), trigger
                // an instant restart (ReadyToRestart), or show the
                // package-manager hint toast (system-managed).
                let (label, style): (String, PillStyle) = match info.kind {
                    UpdatePillKind::InApp(state) => match state {
                        SelfUpdatePillState::Idle => {
                            (format!("v{} available", info.version), PillStyle::Clickable)
                        }
                        SelfUpdatePillState::Downloading => {
                            ("Downloading update…".to_string(), PillStyle::Busy)
                        }
                        SelfUpdatePillState::Installing => {
                            ("Installing update…".to_string(), PillStyle::Busy)
                        }
                        SelfUpdatePillState::ReadyToRestart => {
                            ("Restart Paneflow".to_string(), PillStyle::Clickable)
                        }
                        SelfUpdatePillState::Errored => {
                            ("Update failed".to_string(), PillStyle::Clickable)
                        }
                    },
                    UpdatePillKind::SystemManaged(kind) => {
                        let label = match kind {
                            SystemPackageKind::RpmOstree => "Update via rpm-ostree".to_string(),
                            SystemPackageKind::Other => "Update via package manager".to_string(),
                        };
                        (label, PillStyle::SystemHint)
                    }
                };

                // Leading icon. Clickable renders `download.svg` for the
                // pre-install CTA and `refresh.svg` for the post-install
                // "Restart for vX" CTA so the user has a visual cue that the
                // heavy work is already done; Busy renders a `loader-circle.svg`
                // arc continuously rotating via GPUI's declarative
                // Animation+Transformation API (one full revolution per second,
                // repeat forever). Pattern mirrors
                // `crates/gpui/examples/animation.rs` in the upstream Zed repo.
                let is_ready_to_restart = matches!(
                    info.kind,
                    UpdatePillKind::InApp(SelfUpdatePillState::ReadyToRestart)
                );
                let leading_icon: AnyElement = match style {
                    PillStyle::Busy => svg()
                        .size(px(11.))
                        .flex_none()
                        .path("icons/loader-circle.svg")
                        .text_color(ui.muted)
                        .with_animation(
                            "update-pill-spinner",
                            Animation::new(Duration::from_secs(1)).repeat(),
                            |svg, delta| {
                                svg.with_transformation(Transformation::rotate(percentage(delta)))
                            },
                        )
                        .into_any_element(),
                    PillStyle::Clickable => svg()
                        .size(px(11.))
                        .flex_none()
                        .path(if is_ready_to_restart {
                            "icons/refresh.svg"
                        } else {
                            "icons/download.svg"
                        })
                        .text_color(ui.muted)
                        .into_any_element(),
                    PillStyle::SystemHint => svg()
                        .size(px(11.))
                        .flex_none()
                        .path("icons/tool.svg")
                        .text_color(ui.muted)
                        .into_any_element(),
                };

                // The pill sits inside the title bar's `WindowControlArea::Drag`
                // region declared on the parent. GPUI's hit-testing picks the
                // most-specific element on hover, so `cursor_pointer()` on the
                // pill itself overrides the parent's cursor without needing
                // to detach from the drag area; the `cx.stop_propagation()`
                // mouse-down further down ensures the click doesn't trigger
                // a window drag. Same idiom Zed's `ButtonLike` relies on.
                // US-007 AC3: a small `×` dismiss affordance on the
                // non-busy states. We deliberately omit it during
                // Downloading/Installing/ReadyToRestart - those have a
                // user-perceivable side effect already in flight (or
                // sitting one click away from `cx.restart()`); a stray
                // dismiss there would be jarring. Errored remains
                // dismissable so a user with a chronic install failure
                // can hide the pill without having to bounce the app.
                let pill_dismissable = matches!(
                    info.kind,
                    UpdatePillKind::InApp(SelfUpdatePillState::Idle | SelfUpdatePillState::Errored)
                        | UpdatePillKind::SystemManaged(_)
                );

                let mut pill = div()
                    .id("update-pill")
                    .ml_auto()
                    .mr_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(5.))
                    .px(px(8.))
                    .h(px(24.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(ui.border)
                    .bg(ui.subtle)
                    .text_color(ui.text)
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(leading_icon)
                    .child(label);

                if pill_dismissable {
                    let muted = ui.muted;
                    let text = ui.text;
                    pill = pill.child(
                        div()
                            .id("update-pill-dismiss")
                            .ml(px(2.))
                            .px(px(4.))
                            .text_color(muted)
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .cursor_pointer()
                            .hover(move |s| s.text_color(text))
                            // stop_propagation on BOTH mouse-down and click
                            // so the click never reaches the parent pill's
                            // `on_click` handler that dispatches
                            // `StartSelfUpdate` - otherwise hitting the `×`
                            // would (a) dismiss the pill (b) immediately
                            // start the update we just dismissed.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(|_, window, cx| {
                                cx.stop_propagation();
                                window.dispatch_action(Box::new(crate::DismissUpdate), cx);
                            })
                            .child("×"),
                    );
                }
                match style {
                    // Dispatch on mouse-DOWN, not on click (mouse-up). At cold
                    // start the update check resolves before the user has
                    // touched the window, so the very first press on the pill
                    // happens against a window the compositor still considers
                    // inactive (Wayland focus-stealing prevention often
                    // rejects `cx.activate(true)`) and a focus chain that
                    // isn't yet initialized. In that state, `on_click`
                    // (which needs a matched press+release pair routed
                    // through the focus chain) silently drops the first
                    // interaction; the user has to click elsewhere to wake
                    // the chain, then re-click. Press-based dispatch avoids
                    // both races and matches the title-bar button idiom in
                    // Zed/VS Code/Discord. The pkexec modal confirms the
                    // action, so we don't lose "drag-out to cancel".
                    PillStyle::Clickable => {
                        pill = pill
                            .cursor_pointer()
                            .hover(|s| {
                                let ui = crate::theme::ui_colors();
                                s.bg(ui.surface).border_color(ui.muted)
                            })
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                cx.stop_propagation();
                                window.dispatch_action(Box::new(crate::StartSelfUpdate), cx);
                            });
                    }
                    PillStyle::Busy => {
                        pill = pill.opacity(0.7);
                    }
                    // SystemHint is a button that copies the upgrade command
                    // to the clipboard via a toast. It's still a button, so
                    // the cursor must hint that - `cursor_pointer()` matches
                    // every other clickable surface in the chrome.
                    PillStyle::SystemHint => {
                        pill = pill
                            .opacity(0.8)
                            .cursor_pointer()
                            .hover(|s| {
                                let ui = crate::theme::ui_colors();
                                s.bg(ui.surface).border_color(ui.muted).opacity(1.0)
                            })
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                cx.stop_propagation();
                                window.dispatch_action(Box::new(crate::StartSelfUpdate), cx);
                            });
                    }
                }
                pill
            });
        // Cockpit modes: same rail-confinement story as the update pill - the
        // notice lives in the sidebar (`render_sidebar_ipc_banner`). Diff
        // keeps the title-bar pill.
        let ipc_pill = (update_pill_visible && self.ipc_state == crate::ipc::IpcState::Disabled)
            .then(|| {
                div()
                    .id("ipc-offline-pill")
                    .mr_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(5.))
                    .px(px(8.))
                    .h(px(24.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(ui.border)
                    .bg(ui.subtle)
                    .text_color(ui.text)
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(
                        svg()
                            .size(px(11.))
                            .flex_none()
                            .path("icons/triangle-alert.svg")
                            .text_color(ui.muted),
                    )
                    .child("IPC offline")
            });

        let csd_rounding = px(10.);

        let mut bar = div()
            .id("title-bar")
            .window_control_area(WindowControlArea::Drag)
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(height)
            // Windows/macOS use GPUI's native blurred material through this
            // translucent fill. Linux keeps the same color fully opaque.
            .bg(chrome_bg)
            // Windows: drop the right padding so the native-style caption
            // buttons sit flush in the top-right corner (Fitts's-law target).
            // Linux/macOS keep the 12px inset for the compact pill controls.
            .when(!cfg!(target_os = "windows"), |d| d.pr(px(12.)));

        // CSD rounded corners with tiling awareness. Skipped in the cockpit
        // (Agents + Cli): the title bar is a confined overlay there, so its
        // corner fill + border would draw a stray frame (and a mid-window
        // rounded top-right corner) over the rail + panel.
        if !self.is_agents
            && !self.cockpit
            && let Decorations::Client { tiling } = decorations
        {
            if !(tiling.top || tiling.left) {
                bar = bar.rounded_tl(csd_rounding);
            }
            if !(tiling.top || tiling.right) {
                bar = bar.rounded_tr(csd_rounding);
            }
            // 1px border + negative margins fill transparent gap at rounded
            // corners. Match the cockpit gray in Cli so the corner fill blends
            // with the painted strip instead of showing the themed chrome.
            bar = bar
                .mt(px(-1.))
                .mb(px(-1.))
                .border(px(1.))
                .border_color(chrome_bg);
        }

        bar
            // Drag-to-move state machine
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.should_move = true;
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.should_move = false;
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| {
                this.should_move = false;
            }))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.should_move {
                    this.should_move = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.zoom_window();
                }
            })
            // Right-click opens the DE's native window menu
            .when(supported.window_menu, |bar| {
                bar.on_mouse_down(MouseButton::Right, |ev, window, _| {
                    window.show_window_menu(ev.position);
                })
            })
            .children(left_controls)
            .child(brand)
            .child(content)
            .children(ipc_pill)
            .children(update_pill)
            .children(right_controls)
            .when(!self.is_agents && !self.cockpit, |this| {
                // Codex cockpit: Agents + Cli drop the bottom divider so the
                // chrome reads as one seamless surface (Cli fuses with its
                // #141414 sidebar); Diff keeps it (diff visuel nul).
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(1.))
                        .bg(ui.border),
                )
            })
    }
}
