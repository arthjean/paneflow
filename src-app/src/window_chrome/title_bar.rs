use crate::ui_primitives::AccessibleControlExt;

use gpui::{
    AnyElement, Context, Decorations, EventEmitter, IntoElement, MouseButton, Render, Styled,
    Window, WindowControlArea, div, prelude::*, px, svg,
};

use super::csd::default_button_layout;
use crate::{
    app::constants::{
        SIDEBAR_WIDTH, TITLE_BAR_CONTROL_SIZE, TITLE_BAR_EDGE_INSET, TITLE_BAR_MIN_HEIGHT,
    },
    ui_primitives::{AnimatedHoverExt, ROW_RADIUS, comet_spinner, lerp_color, squircle_skin},
};

pub struct TitleBar {
    should_move: bool,
    pub workspace_name: Option<String>,
    pub sidebar_visible: bool,
    pub left_rail_width: f32,
    pub files_menu_open: bool,
    pub help_menu_open: bool,
    pub ipc_state: crate::ipc::IpcState,
    pub update_pill: Option<UpdatePill>,
    pub cockpit: bool,
    pub cockpit_material_active: bool,
    button_layout_observer: Option<gpui::Subscription>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemPackageKind {
    RpmOstree,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdatePill {
    Checking,
    UpToDate,
    CheckFailed,
    Available(String),
    Downloading,
    Installing,
    ReadyToRestart,
    Restarting,
    InstallFailed,
    SystemManaged(SystemPackageKind),
}

const UPDATE_AVAILABLE_BLUE: u32 = 0x1a6ff6;

impl TitleBar {
    fn render_update_pill(&self, ui: crate::theme::UiColors) -> Option<AnyElement> {
        let pill = self.update_pill.clone()?;
        let white = gpui::white();
        let blue = gpui::Hsla::from(gpui::rgb(UPDATE_AVAILABLE_BLUE));
        let (label, fill, ink): (String, gpui::Hsla, gpui::Hsla) = match &pill {
            UpdatePill::Checking => ("Checking for updates…".to_string(), ui.subtle, ui.text),
            UpdatePill::UpToDate => (
                "Paneflow is up to date".to_string(),
                ui.vc_added.opacity(0.12),
                ui.vc_added,
            ),
            UpdatePill::CheckFailed => (
                "Retry update check".to_string(),
                ui.vc_deleted.opacity(0.12),
                ui.vc_deleted,
            ),
            UpdatePill::Available(version) => (format!("v{version} available"), blue, white),
            UpdatePill::Downloading => ("Downloading update…".to_string(), ui.subtle, ui.text),
            UpdatePill::Installing => ("Installing update…".to_string(), ui.subtle, ui.text),
            UpdatePill::ReadyToRestart | UpdatePill::Restarting => {
                ("Restart to update".to_string(), blue, white)
            }
            UpdatePill::InstallFailed => (
                "Retry update".to_string(),
                ui.vc_deleted.opacity(0.12),
                ui.vc_deleted,
            ),
            UpdatePill::SystemManaged(SystemPackageKind::RpmOstree) => {
                ("Update via rpm-ostree".to_string(), ui.subtle, ui.text)
            }
            UpdatePill::SystemManaged(SystemPackageKind::Other) => {
                ("Update via package manager".to_string(), ui.subtle, ui.text)
            }
        };

        let hovered = match &pill {
            UpdatePill::Available(_) | UpdatePill::ReadyToRestart => Some(gpui::Hsla {
                l: (fill.l - 0.05).max(0.0),
                ..fill
            }),
            UpdatePill::CheckFailed | UpdatePill::InstallFailed => Some(ui.vc_deleted.opacity(0.2)),
            UpdatePill::SystemManaged(_) => Some(ui.surface),
            UpdatePill::Checking
            | UpdatePill::UpToDate
            | UpdatePill::Downloading
            | UpdatePill::Installing
            | UpdatePill::Restarting => None,
        };
        let shell = div()
            .id("update-pill")
            .ml_auto()
            .mr_2()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(5.))
            .px(px(10.))
            .h(px(24.))
            .text_color(ink)
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        #[cfg(test)]
        let shell = shell.debug_selector(|| "update-pill".into());
        let mut element =
            squircle_skin(shell, "update-pill-group", ROW_RADIUS, Some(fill), hovered);

        let glyph = match &pill {
            UpdatePill::Checking
            | UpdatePill::Downloading
            | UpdatePill::Installing
            | UpdatePill::Restarting => Some(comet_spinner("update-pill-spinner", px(12.), ink)),
            UpdatePill::Available(_) => Some(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path("icons/download.svg")
                    .text_color(white)
                    .into_any_element(),
            ),
            UpdatePill::ReadyToRestart => Some(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path("icons/refresh.svg")
                    .text_color(white)
                    .into_any_element(),
            ),
            UpdatePill::SystemManaged(_) => Some(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path("icons/tool.svg")
                    .text_color(ui.muted)
                    .into_any_element(),
            ),
            UpdatePill::CheckFailed | UpdatePill::InstallFailed => Some(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path("icons/triangle-alert.svg")
                    .text_color(ink)
                    .into_any_element(),
            ),
            UpdatePill::UpToDate => None,
        };
        let label = gpui::SharedString::from(label);
        element = element.children(glyph).child(label.clone());

        let element = match pill {
            UpdatePill::Available(_)
            | UpdatePill::ReadyToRestart
            | UpdatePill::InstallFailed
            | UpdatePill::SystemManaged(_) => element
                .role(gpui::accesskit::Role::Button)
                .aria_label(label)
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    window.dispatch_action(Box::new(crate::StartSelfUpdate), cx);
                })
                .into_any_element(),
            UpdatePill::CheckFailed => element
                .role(gpui::accesskit::Role::Button)
                .aria_label(label)
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    window.dispatch_action(Box::new(crate::CheckForUpdates), cx);
                })
                .into_any_element(),
            UpdatePill::Checking | UpdatePill::Downloading | UpdatePill::Installing => {
                element.opacity(0.7).into_any_element()
            }
            UpdatePill::UpToDate | UpdatePill::Restarting => element.into_any_element(),
        };
        Some(element)
    }

    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            should_move: false,
            workspace_name: None,
            sidebar_visible: true,
            left_rail_width: SIDEBAR_WIDTH,
            files_menu_open: false,
            help_menu_open: false,
            ipc_state: crate::ipc::IpcState::Online,
            update_pill: None,
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
                    .accessible_control(gpui::accesskit::Role::Button, sidebar_tooltip)
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

        let ipc_pill =
            (!self.cockpit && self.ipc_state == crate::ipc::IpcState::Disabled).then(|| {
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

        let update_pill = self.render_update_pill(ui);

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
        .children(update_pill)
        .children(ipc_pill)
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AvailableSpace, point, size};

    #[gpui::test]
    fn the_update_pill_paints_in_the_cockpit_shell(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let bar = cx.new(|cx| {
            let mut bar = TitleBar::new(cx);
            bar.cockpit = true;
            bar.update_pill = Some(UpdatePill::Available("9.0.0".into()));
            bar
        });
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(1400.)),
                AvailableSpace::Definite(px(40.)),
            ),
            move |_, _| div().size_full().child(bar.clone()),
        );
        assert!(cx.debug_bounds("update-pill").is_some());
    }
}
