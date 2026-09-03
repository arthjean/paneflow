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
    pub left_rail_width: f32,
    pub files_menu_open: bool,
    pub help_menu_open: bool,
    pub ipc_state: crate::ipc::IpcState,
    pub update_available: Option<UpdateInfo>,
    pub cockpit: bool,
    pub cockpit_material_active: bool,
    button_layout_observer: Option<gpui::Subscription>,
}

#[derive(Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub kind: UpdatePillKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UpdatePillKind {
    InApp(SelfUpdatePillState),
    SystemManaged(SystemPackageKind),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SelfUpdatePillState {
    Idle,
    Downloading,
    Installing,
    ReadyToRestart,
    Errored,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SystemPackageKind {
    RpmOstree,
    Other,
}

#[derive(Clone, Copy)]
enum PillStyle {
    Clickable,
    Busy,
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
        if self.button_layout_observer.is_none() {
            self.button_layout_observer =
                Some(cx.observe_button_layout_changed(window, |_, _, cx| cx.notify()));
        }

        let height = (1.75 * window.rem_size()).max(TITLE_BAR_MIN_HEIGHT);
        let decorations = window.window_decorations();
        let is_csd = matches!(decorations, Decorations::Client { .. });

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

        let layout = cx.button_layout().unwrap_or_else(default_button_layout);
        let is_maximized = window.is_maximized();
        let supported = window.window_controls();

        let close_handle = cx.entity().downgrade();
        let on_close = move |_window: &mut Window, cx: &mut gpui::App| {
            if let Some(entity) = close_handle.upgrade() {
                entity.update(cx, |_this, cx| cx.emit(TitleBarEvent::CloseRequested));
            }
        };

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

        let ui = crate::theme::ui_colors();
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

        let update_pill_visible = !self.cockpit;
        let update_pill = update_pill_visible
            .then(|| self.update_available.clone())
            .flatten()
            .map(|info| {
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
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(|_, window, cx| {
                                cx.stop_propagation();
                                window.dispatch_action(Box::new(crate::DismissUpdate), cx);
                            })
                            .child("×"),
                    );
                }
                match style {
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
            .bg(chrome_bg)
            .when(
                !cfg!(target_os = "windows") && !right_controls_present,
                |d| d.pr(TITLE_BAR_EDGE_INSET),
            );

        bar.on_mouse_down(
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
