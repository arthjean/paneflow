use crate::ui_primitives::TooltipDelayExt;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Context, Decorations, EventEmitter, IntoElement,
    MouseButton, Render, Styled, Transformation, Window, WindowControlArea, div, percentage,
    prelude::*, px, svg,
};

use super::csd::default_button_layout;
use crate::{
    app::constants::{
        SIDEBAR_WIDTH, TITLE_BAR_CONTROL_SIZE, TITLE_BAR_EDGE_INSET, TITLE_BAR_MIN_HEIGHT,
    },
    ui_primitives::{AnimatedHoverExt, lerp_color},
};

pub struct TitleBar {
    should_move: bool,
    pub workspace_name: Option<String>,
    pub sidebar_visible: bool,
    /// Stable expanded width of the active left rail. The body can animate to
    /// zero independently, while title-bar controls remain stationary and
    /// align with the open rail in CLI, Agents, Diff, and Settings.
    pub left_rail_width: f32,
    pub files_menu_open: bool,
    pub help_menu_open: bool,
    pub ipc_state: crate::ipc::IpcState,
    /// Set by PaneFlowApp when a newer version is detected.
    pub update_available: Option<UpdateInfo>,
    /// Cockpit chrome: paint the rail `#141414` and drop the bottom divider so
    /// the title bar + sidebar read as one continuous surface. PUSHED by
    /// `PaneFlowApp::render`; `TitleBar` never reads `AppMode`.
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
    /// Default accent pill with a hover fade. Dispatches the in-app update
    /// action.
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
            left_rail_width: SIDEBAR_WIDTH,
            files_menu_open: false,
            help_menu_open: false,
            ipc_state: crate::ipc::IpcState::Online,
            update_available: None,
            cockpit: false,
            cockpit_material_active: !cfg!(target_os = "windows"),
            button_layout_observer: None,
        }
    }
}

pub enum TitleBarEvent {
    CloseRequested,
    ToggleSidebar,
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

        let height = (1.75 * window.rem_size()).max(TITLE_BAR_MIN_HEIGHT);
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

        // The parent window shell owns the active/inactive tint. This child is
        // transparent so blur is composed once and cannot refill rounded CSD
        // corner pixels with a rectangular background.
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
        let render_controls = !window.is_fullscreen() && (is_csd || cfg!(target_os = "windows"));

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
        let left_controls_present = left_controls.is_some();
        let right_controls_present = right_controls.is_some();

        // --- Left section: brand slot, fixed width aligned with sidebar ---
        let ui = crate::theme::ui_colors();
        // US-011: on macOS, reserve the leftmost ~80px of the custom titlebar
        // for the native red/yellow/green traffic lights (positioned at
        // x=12,y=12 by WindowOptions::titlebar::traffic_light_position in
        // main.rs). Linux control groups own the shared 8px edge inset;
        // adding another brand inset would duplicate that spacing.
        //
        // In macOS fullscreen AppKit hides the traffic lights, so the 80px
        // reservation would leave a dead gap before the brand cluster - drop
        // back to the shared 8px inset there.
        let brand_pl = if cfg!(target_os = "macos") && !window.is_fullscreen() {
            gpui::px(80.0)
        } else if left_controls_present {
            gpui::px(0.)
        } else {
            TITLE_BAR_EDGE_INSET
        };
        let toggle_sidebar_handle = cx.entity().downgrade();
        let toggle_files_menu_handle = cx.entity().downgrade();
        let toggle_help_menu_handle = cx.entity().downgrade();
        let control_hover_bg = crate::app::constants::sidebar_tab_active_background();
        let toggle_sidebar_resting_bg = if self.sidebar_visible {
            control_hover_bg.opacity(0.0)
        } else {
            control_hover_bg
        };
        let files_menu_resting_bg = if self.files_menu_open {
            control_hover_bg
        } else {
            control_hover_bg.opacity(0.0)
        };
        let files_menu_resting_text = if self.files_menu_open {
            ui.text
        } else {
            ui.muted
        };
        let help_menu_resting_bg = if self.help_menu_open {
            control_hover_bg
        } else {
            control_hover_bg.opacity(0.0)
        };
        let help_menu_resting_text = if self.help_menu_open {
            ui.text
        } else {
            ui.muted
        };
        let sidebar_tooltip: gpui::SharedString = if self.sidebar_visible {
            "Hide sidebar"
        } else {
            "Show sidebar"
        }
        .into();
        let brand = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .pl(brand_pl)
            .pr(px(4.))
            .overflow_x_hidden()
            .child(
                div()
                    .id("toggle-primary-sidebar")
                    .flex_none()
                    .size(TITLE_BAR_CONTROL_SIZE)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .animated_hover(move |style, delta| {
                        style.bg(lerp_color(
                            toggle_sidebar_resting_bg,
                            control_hover_bg,
                            delta,
                        ));
                    })
                    .delayed_tooltip(move |_window, cx| {
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
                    .id("title-bar-files-menu-trigger")
                    .flex_none()
                    .h(TITLE_BAR_CONTROL_SIZE)
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .rounded(px(8.))
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(files_menu_resting_text)
                    .animated_hover(move |style, delta| {
                        style
                            .bg(lerp_color(files_menu_resting_bg, control_hover_bg, delta))
                            .text_color(lerp_color(files_menu_resting_text, ui.text, delta));
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
                    .h(TITLE_BAR_CONTROL_SIZE)
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .rounded(px(8.))
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(help_menu_resting_text)
                    .animated_hover(move |style, delta| {
                        style
                            .bg(lerp_color(help_menu_resting_bg, control_hover_bg, delta))
                            .text_color(lerp_color(help_menu_resting_text, ui.text, delta));
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
        let left_rail = div()
            .flex_none()
            .w(px(self.left_rail_width))
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .overflow_x_hidden()
            .children(left_controls)
            .child(brand);

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
        let update_pill_visible = !self.cockpit;
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
                // region declared on the parent. Its nested mouse-down handlers
                // stop propagation so interaction with the pill does not trigger
                // a window drag while the rest of the title bar remains draggable.
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
                            .animated_hover(move |style, delta| {
                                style.text_color(lerp_color(muted, text, delta));
                            })
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
                    PillStyle::Clickable => pill
                        .animated_hover(move |style, delta| {
                            style
                                .bg(lerp_color(ui.subtle, ui.surface, delta))
                                .border_color(lerp_color(ui.border, ui.muted, delta));
                        })
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            cx.stop_propagation();
                            window.dispatch_action(Box::new(crate::StartSelfUpdate), cx);
                        })
                        .into_any_element(),
                    PillStyle::Busy => pill.opacity(0.7).into_any_element(),
                    // SystemHint copies the upgrade command to the clipboard
                    // through a toast. It remains clickable with the default
                    // cursor, consistent with the Review/Diff chrome.
                    PillStyle::SystemHint => pill
                        .opacity(0.8)
                        .animated_hover(move |style, delta| {
                            style
                                .bg(lerp_color(ui.subtle, ui.surface, delta))
                                .border_color(lerp_color(ui.border, ui.muted, delta))
                                .opacity(0.8 + 0.2 * delta);
                        })
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            cx.stop_propagation();
                            window.dispatch_action(Box::new(crate::StartSelfUpdate), cx);
                        })
                        .into_any_element(),
                }
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

        let bar = div()
            .id("title-bar")
            .window_control_area(WindowControlArea::Drag)
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(height)
            // The transparent fill reveals either the themed shell or the
            // platform material selected by the parent window.
            .bg(chrome_bg)
            // Windows remains flush for native caption hit targets. Linux
            // right-side controls already own their 8px edge inset; macOS
            // and layouts without right controls keep the bar-level inset.
            .when(
                !cfg!(target_os = "windows") && !right_controls_present,
                |d| d.pr(TITLE_BAR_EDGE_INSET),
            );

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
            .child(left_rail)
            .child(content)
            .children(ipc_pill)
            .children(update_pill)
            .children(right_controls)
            .when(!self.cockpit, |this| {
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
