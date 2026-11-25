use crate::ui_primitives::AccessibleControlExt;

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, Context, Decorations, EventEmitter, IntoElement, MouseButton, Render, Styled,
    Window, WindowControlArea, div, prelude::*, px, svg,
};

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
    pub diff_dock_open: Option<bool>,
    pub left_rail_width: f32,
    pub files_menu_open: bool,
    pub help_menu_open: bool,
    pub ipc_state: crate::ipc::IpcState,
    pub update_pill: Option<UpdatePill>,
    pub cockpit: bool,
    pub cockpit_material_active: bool,
    pub detached: bool,
    pub brand_extent: Rc<Cell<f32>>,
    pub trailing_origin: Rc<Cell<f32>>,
    files_trigger_center: Rc<Cell<f32>>,
    help_trigger_center: Rc<Cell<f32>>,
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
            | UpdatePill::Restarting => Some(comet_spinner(px(12.), ink)),
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
                .on_activate(move |_, window, cx| {
                    cx.stop_propagation();
                    window.dispatch_action(Box::new(crate::StartSelfUpdate), cx);
                })
                .into_any_element(),
            UpdatePill::CheckFailed => element
                .role(gpui::accesskit::Role::Button)
                .aria_label(label)
                .on_activate(move |_, window, cx| {
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
            diff_dock_open: None,
            left_rail_width: SIDEBAR_WIDTH,
            files_menu_open: false,
            help_menu_open: false,
            ipc_state: crate::ipc::IpcState::Online,
            update_pill: None,
            cockpit: false,
            cockpit_material_active: !cfg!(target_os = "windows"),
            detached: false,
            brand_extent: Rc::default(),
            trailing_origin: Rc::default(),
            files_trigger_center: Rc::default(),
            help_trigger_center: Rc::default(),
        }
    }

    pub(crate) fn render_brand(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let height = (1.75 * window.rem_size()).max(TITLE_BAR_MIN_HEIGHT);
        let ui = crate::theme::ui_colors();
        let brand_pl = if cfg!(target_os = "macos") && !window.is_fullscreen() {
            gpui::px(80.0)
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
        let sidebar_tooltip: gpui::SharedString = if self.sidebar_visible {
            "Hide sidebar"
        } else {
            "Show sidebar"
        }
        .into();
        let brand = div()
            .map(|brand| {
                if self.detached {
                    brand.flex_none()
                } else {
                    brand.flex_1().min_w_0().overflow_x_hidden()
                }
            })
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .pl(brand_pl)
            .pr(px(4.))
            .child(title_bar_toggle(
                "toggle-primary-sidebar",
                sidebar_tooltip,
                "icons/sidebar.svg",
                (toggle_sidebar_resting_bg, control_hover_bg),
                ui.muted,
                move |cx| {
                    if let Some(entity) = toggle_sidebar_handle.upgrade() {
                        entity.update(cx, |_this, cx| {
                            cx.emit(TitleBarEvent::ToggleSidebar);
                        });
                    }
                },
            ))
            .child(title_bar_menu_trigger(
                "title-bar-files-menu-trigger",
                FILES_MENU_LABEL,
                self.files_menu_open,
                self.files_trigger_center.clone(),
                height,
                {
                    let was_open = self.files_menu_open;
                    move |anchor, cx| {
                        if let Some(entity) = toggle_files_menu_handle.upgrade() {
                            entity.update(cx, |_this, cx| {
                                cx.emit(TitleBarEvent::ToggleFilesMenu { anchor, was_open });
                            });
                        }
                    }
                },
            ))
            .child(title_bar_menu_trigger(
                "title-bar-help-menu-trigger",
                HELP_MENU_LABEL,
                self.help_menu_open,
                self.help_trigger_center.clone(),
                height,
                {
                    let was_open = self.help_menu_open;
                    move |anchor, cx| {
                        if let Some(entity) = toggle_help_menu_handle.upgrade() {
                            entity.update(cx, |_this, cx| {
                                cx.emit(TitleBarEvent::ToggleHelpMenu { anchor, was_open });
                            });
                        }
                    }
                },
            ));
        brand
            .child(position_probe(self.brand_extent.clone()))
            .into_any_element()
    }

    pub(crate) fn render_trailing(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let height = (1.75 * window.rem_size()).max(TITLE_BAR_MIN_HEIGHT);
        let is_csd = matches!(
            crate::window_chrome::csd::decorations(window),
            Decorations::Client { .. }
        );
        let ui = crate::theme::ui_colors();
        let control_hover_bg = crate::app::constants::sidebar_tab_active_background();
        let is_maximized = window.is_maximized();
        let supported = window.window_controls();

        let close_handle = cx.entity().downgrade();
        let on_close = move |_window: &mut Window, cx: &mut gpui::App| {
            if let Some(entity) = close_handle.upgrade() {
                entity.update(cx, |_this, cx| cx.emit(TitleBarEvent::CloseRequested));
            }
        };

        let render_controls = !window.is_fullscreen() && (is_csd || cfg!(target_os = "windows"));

        let right_controls = if render_controls {
            super::csd::render_button_group("r", is_maximized, height, &supported, on_close)
        } else {
            None
        };
        let right_controls_present = right_controls.is_some();

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

        let diff_dock_toggle = self.diff_dock_open.map(|open| {
            let toggle_diff_dock_handle = cx.entity().downgrade();
            let resting_bg = if open {
                control_hover_bg
            } else {
                control_hover_bg.opacity(0.0)
            };
            let tooltip: gpui::SharedString = if open {
                "Hide Changes dock"
            } else {
                "Show Changes dock"
            }
            .into();
            let toggle = title_bar_toggle(
                "toggle-diff-dock",
                tooltip,
                "icons/layout-sidebar-right.svg",
                (resting_bg, control_hover_bg),
                ui.muted,
                move |cx| {
                    if let Some(entity) = toggle_diff_dock_handle.upgrade() {
                        entity.update(cx, |_this, cx| {
                            cx.emit(TitleBarEvent::ToggleDiffDock);
                        });
                    }
                },
            );
            #[cfg(test)]
            let toggle = toggle.debug_selector(|| "toggle-diff-dock".into());
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(TITLE_BAR_EDGE_INSET)
                .child(toggle)
                .when(cfg!(target_os = "windows"), |trailing| {
                    if right_controls_present {
                        trailing.child(div().flex_none().w(px(1.)).h(px(16.)).bg(ui.border))
                    } else {
                        trailing.mr(TITLE_BAR_EDGE_INSET)
                    }
                })
        });

        div()
            .flex_none()
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .when(
                !cfg!(target_os = "windows") && !right_controls_present,
                |trailing| trailing.pr(TITLE_BAR_EDGE_INSET),
            )
            .child(position_probe(self.trailing_origin.clone()))
            .children(update_pill)
            .children(ipc_pill)
            .children(diff_dock_toggle)
            .children(right_controls)
            .into_any_element()
    }
}

fn position_probe(position: Rc<Cell<f32>>) -> impl IntoElement {
    gpui::canvas(
        move |bounds, window, _| {
            let x = f32::from(bounds.origin.x);
            if position.replace(x) != x {
                window.request_animation_frame();
            }
        },
        |_, _, _, _| {},
    )
    .flex_none()
    .w(px(0.))
    .h(px(0.))
}

pub enum TitleBarEvent {
    CloseRequested,
    ToggleSidebar,
    ToggleDiffDock,
    ToggleFilesMenu {
        anchor: gpui::Point<gpui::Pixels>,
        was_open: bool,
    },
    ToggleHelpMenu {
        anchor: gpui::Point<gpui::Pixels>,
        was_open: bool,
    },
}

impl EventEmitter<TitleBarEvent> for TitleBar {}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = (1.75 * window.rem_size()).max(TITLE_BAR_MIN_HEIGHT);
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

        let ui = crate::theme::ui_colors();
        let left_rail = div()
            .flex_none()
            .w(px(self.left_rail_width))
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .overflow_x_hidden()
            .children((!self.detached).then(|| self.render_brand(window, cx)));

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

        let bar = div()
            .id("title-bar")
            .window_control_area(WindowControlArea::Drag)
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(height)
            .bg(chrome_bg);
        let supported = window.window_controls();
        let trailing = (!self.detached).then(|| self.render_trailing(window, cx));

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
        .children(trailing)
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

pub(crate) const FILES_MENU_LABEL: &str = "Files";
pub(crate) const HELP_MENU_LABEL: &str = "Help";

fn title_bar_menu_trigger(
    id: &'static str,
    label: &'static str,
    open: bool,
    center_x: Rc<Cell<f32>>,
    anchor_y: gpui::Pixels,
    on_toggle: impl Fn(gpui::Point<gpui::Pixels>, &mut gpui::App) + 'static,
) -> crate::ui_primitives::AnimatedHover {
    let ui = crate::theme::ui_colors();
    let hover_bg = crate::app::constants::sidebar_tab_active_background();
    let resting_bg = if open {
        hover_bg
    } else {
        hover_bg.opacity(0.0)
    };
    let resting_text = if open { ui.text } else { ui.muted };
    let on_toggle = Rc::new(on_toggle);
    let pointer = on_toggle.clone();
    let probe = center_x.clone();
    div()
        .id(id)
        .relative()
        .flex_none()
        .h(TITLE_BAR_CONTROL_SIZE)
        .px(px(6.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::NORMAL)
        .text_color(resting_text)
        .role(gpui::accesskit::Role::Button)
        .aria_label(label)
        .aria_expanded(open)
        .animated_hover(move |style, delta| {
            style
                .bg(lerp_color(resting_bg, hover_bg, delta))
                .text_color(lerp_color(resting_text, ui.text, delta));
        })
        .aria_has_popup(gpui::accesskit::HasPopup::Menu)
        .on_mouse_down(MouseButton::Left, move |event, _, cx| {
            cx.stop_propagation();
            pointer(gpui::point(event.position.x, anchor_y), cx);
        })
        .on_a11y_action(gpui::accesskit::Action::Click, move |_, _, cx| {
            on_toggle(gpui::point(px(center_x.get()), anchor_y), cx);
        })
        .child(center_probe(probe))
        .child(label)
}

fn center_probe(center: Rc<Cell<f32>>) -> impl IntoElement {
    gpui::canvas(
        move |bounds, _, _| center.set(f32::from(bounds.center().x)),
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

fn title_bar_toggle(
    id: &'static str,
    label: gpui::SharedString,
    icon: &'static str,
    (resting_bg, hover_bg): (gpui::Hsla, gpui::Hsla),
    icon_color: gpui::Hsla,
    on_press: impl Fn(&mut gpui::App) + 'static,
) -> crate::ui_primitives::AnimatedHover {
    div()
        .id(id)
        .flex_none()
        .size(TITLE_BAR_CONTROL_SIZE)
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .animated_hover(move |style, delta| {
            style.bg(lerp_color(resting_bg, hover_bg, delta));
        })
        .accessible_control(gpui::accesskit::Role::Button, label)
        .on_pointer_press(move |_, _, cx| on_press(cx))
        .child(svg().size(px(14.)).path(icon).text_color(icon_color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AvailableSpace, point, size};

    #[test]
    fn the_sidebar_and_changes_dock_toggles_answer_an_accessibility_press() {
        for (id, label) in [
            ("toggle-primary-sidebar", "Hide sidebar"),
            ("toggle-diff-dock", "Show Changes dock"),
        ] {
            let ui = crate::theme::ui_colors();
            let toggle = title_bar_toggle(
                id,
                label.into(),
                "icons/sidebar.svg",
                (ui.subtle, ui.subtle),
                ui.muted,
                |_| {},
            );
            let role = gpui::Element::a11y_role(&toggle);
            assert_eq!(role, Some(gpui::accesskit::Role::Button), "{label}");
            let mut node = gpui::accesskit::Node::new(gpui::accesskit::Role::Button);
            gpui::Element::write_a11y_info(&toggle, &mut node);
            assert_eq!(node.label(), Some(label));
            assert!(
                node.supports_action(gpui::accesskit::Action::Click),
                "{label} must advertise an accessibility press"
            );
        }
    }

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

    struct TrailingHost {
        bar: gpui::Entity<TitleBar>,
    }

    impl Render for TrailingHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let trailing = self
                .bar
                .update(cx, |bar, cx| bar.render_trailing(window, cx));
            div()
                .size_full()
                .flex()
                .flex_row()
                .child(div().w(px(300.)).h_full())
                .child(trailing)
        }
    }

    fn draw_bar(cx: &mut gpui::VisualTestContext, diff_dock_open: Option<bool>) {
        let bar = cx.new(|cx| {
            let mut bar = TitleBar::new(cx);
            bar.cockpit = true;
            bar.diff_dock_open = diff_dock_open;
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
    }

    #[gpui::test]
    fn the_diff_dock_toggle_sits_at_the_right_of_the_bar(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        draw_bar(cx, Some(false));
        let toggle = cx
            .debug_bounds("toggle-diff-dock")
            .expect("the toggle paints while a workspace is shown");
        assert_eq!(toggle.size, size(px(20.), px(20.)));
        assert!(toggle.origin.x > px(700.), "{toggle:?}");
    }

    #[gpui::test]
    fn the_diff_dock_toggle_is_absent_without_a_workspace_view(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        draw_bar(cx, None);
        assert!(cx.debug_bounds("toggle-diff-dock").is_none());
    }

    #[gpui::test]
    fn detached_trailing_controls_paint_where_the_host_places_them(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let bar = cx.new(|cx| {
            let mut bar = TitleBar::new(cx);
            bar.cockpit = true;
            bar.diff_dock_open = Some(true);
            bar.detached = true;
            bar
        });
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(1400.)),
                AvailableSpace::Definite(px(40.)),
            ),
            {
                let bar = bar.clone();
                move |_, _| div().size_full().child(bar.clone())
            },
        );
        assert!(
            cx.debug_bounds("toggle-diff-dock").is_none(),
            "the bar leaves its trailing controls to the host"
        );

        let host_bar = bar.clone();
        let host = cx.new(|_| TrailingHost { bar });
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(1400.)),
                AvailableSpace::Definite(px(40.)),
            ),
            move |_, _| div().size_full().child(host.clone()),
        );
        let toggle = cx
            .debug_bounds("toggle-diff-dock")
            .expect("the host paints the trailing controls");
        assert_eq!(toggle.origin.x, px(300.));
        assert_eq!(toggle.size, size(px(20.), px(20.)));
        assert_eq!(
            cx.read(|cx| host_bar.read(cx).trailing_origin.get()),
            300.,
            "the bar reports where its trailing controls start"
        );
    }

    #[test]
    fn the_files_and_help_triggers_are_menu_buttons_that_report_their_menu() {
        use gpui::accesskit::{Action, HasPopup, Node, Role};
        for label in [FILES_MENU_LABEL, HELP_MENU_LABEL] {
            for open in [false, true] {
                let trigger = title_bar_menu_trigger(
                    "trigger",
                    label,
                    open,
                    Rc::default(),
                    px(28.),
                    |_, _| {},
                );
                let role = gpui::Element::a11y_role(&trigger);
                assert_eq!(role, Some(Role::Button), "{label}");
                let mut node = Node::new(Role::Button);
                gpui::Element::write_a11y_info(&trigger, &mut node);
                assert_eq!(node.label(), Some(label));
                assert_eq!(node.is_expanded(), Some(open), "{label}");
                assert_eq!(node.has_popup(), Some(HasPopup::Menu), "{label}");
                assert!(
                    node.supports_action(Action::Click),
                    "{label} must advertise an accessibility press"
                );
            }
        }
    }

    struct MenuHost {
        bar: gpui::Entity<TitleBar>,
        files_center: Rc<Cell<f32>>,
        presses: Rc<std::cell::RefCell<Vec<gpui::Point<gpui::Pixels>>>>,
    }

    impl Render for MenuHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let brand = self.bar.update(cx, |bar, cx| bar.render_brand(window, cx));
            let presses = self.presses.clone();
            div()
                .size_full()
                .flex()
                .flex_row()
                .items_start()
                .child(brand)
                .child(
                    div()
                        .debug_selector(|| "files-probe".into())
                        .child(title_bar_menu_trigger(
                            "title-bar-files-menu-probe",
                            FILES_MENU_LABEL,
                            false,
                            self.files_center.clone(),
                            px(28.),
                            move |anchor, _| presses.borrow_mut().push(anchor),
                        )),
                )
        }
    }

    #[gpui::test]
    fn a_menu_trigger_knows_where_it_is_drawn_so_a_press_anchors_its_menu_there(
        cx: &mut gpui::TestAppContext,
    ) {
        let files_center = Rc::new(Cell::new(0.));
        let (host, cx) = cx.add_window_view({
            let files_center = files_center.clone();
            move |_, cx| MenuHost {
                bar: cx.new(TitleBar::new),
                files_center,
                presses: Rc::default(),
            }
        });
        cx.simulate_resize(size(px(1400.), px(40.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let bar = cx.read(|cx| host.read(cx).bar.clone());
        let drawn = cx
            .debug_bounds("files-probe")
            .expect("the probe trigger paints");
        assert!(drawn.size.width > px(0.));
        assert_eq!(files_center.get(), f32::from(drawn.center().x));
        cx.simulate_mouse_down(
            drawn.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        assert_eq!(
            cx.read(|cx| host.read(cx).presses.borrow().clone()),
            [point(drawn.center().x, px(28.))],
            "a pointer press still anchors the menu under the pointer"
        );
        let (files, help) = cx.read(|cx| {
            let bar = bar.read(cx);
            (
                bar.files_trigger_center.get(),
                bar.help_trigger_center.get(),
            )
        });
        assert!(
            files > 0. && help > files,
            "the brand records both trigger centers, Files before Help: {files} {help}"
        );
    }
}
