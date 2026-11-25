use gpui::{
    AppContext, Bounds, Context, Entity, Focusable, IntoElement, MouseButton, ParentElement,
    Pixels, Render, Styled, WeakEntity, Window, WindowBounds, WindowControlArea, WindowDecorations,
    WindowOptions, div, point, prelude::*, px, size,
};

use crate::pane::{Pane, PaneEvent};
use crate::{PaneFlowApp, ToggleDetachedPane};

use super::DetachedPanePlacement;

const MIN_WIDTH: f32 = 420.;
const MIN_HEIGHT: f32 = 280.;
const TITLE_BAR_HEIGHT: Pixels = px(44.);

pub(crate) struct DetachedPaneWindow {
    pane: Entity<Pane>,
    owner: WeakEntity<PaneFlowApp>,
    should_move: bool,
    initial_focus: bool,
    bounds_save: Option<gpui::Task<()>>,
    orphaned: bool,
    reveal_tabs: bool,
    background_style: paneflow_config::schema::BackgroundStyle,
    #[cfg(target_os = "windows")]
    backdrop_light: Option<bool>,
    #[cfg(target_os = "windows")]
    backdrop_acrylic: Option<bool>,
    #[cfg(target_os = "windows")]
    system_backdrop: bool,
    #[cfg(target_os = "macos")]
    macos_appearance: crate::window_chrome::system_blur::AppliedAppearance,
}

impl PaneFlowApp {
    pub(super) fn detach_pane(
        &mut self,
        pane: Entity<Pane>,
        source: &mut Window,
        restored_bounds: Option<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if pane.read(cx).is_detached() || !self.owns_pane(&pane) {
            return Ok(());
        }
        if self.serialize_detached_panes(cx).len()
            >= paneflow_config::schema::MAX_DETACHED_PANE_WINDOWS
        {
            anyhow::bail!("Detached window limit reached");
        }
        let display = restored_bounds
            .and_then(|bounds| {
                cx.displays()
                    .into_iter()
                    .find(|display| display.bounds().contains(&bounds.center()))
            })
            .or_else(|| source.display(cx))
            .or_else(|| cx.primary_display());
        let visible = display
            .as_ref()
            .map(|display| display.visible_bounds())
            .unwrap_or_else(|| source.window_bounds().get_bounds());
        let proposed = restored_bounds
            .unwrap_or_else(|| Bounds::centered_at(visible.center(), size(px(800.), px(600.))));
        let bounds = fit_bounds(proposed, visible);
        let owner = cx.weak_entity();
        let content = pane.clone();
        let config = self.cached_config.clone();
        let handle = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(MIN_WIDTH), px(MIN_HEIGHT))),
                display_id: display.map(|display| display.id()),
                window_decorations: Some(
                    if config.window_decorations.as_deref() == Some("server") {
                        WindowDecorations::Server
                    } else {
                        WindowDecorations::Client
                    },
                ),
                #[cfg_attr(target_os = "macos", allow(clippy::needless_update))]
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    #[cfg(target_os = "macos")]
                    traffic_light_position: Some(
                        crate::window_chrome::traffic_lights::origin_centered_in(TITLE_BAR_HEIGHT),
                    ),
                    ..Default::default()
                }),
                app_owns_titlebar_drag: true,
                app_id: Some("paneflow".into()),
                window_background: crate::app::constants::initial_window_background(&config),
                focus: restored_bounds.is_none(),
                icon: crate::app_icon::window_icon(config.resolved_app_icon()),
                ..crate::frame_policy::paced_window_options()
            },
            move |window, cx| {
                crate::app_icon::apply_to_window(window, config.resolved_app_icon());
                #[cfg(target_os = "windows")]
                if crate::app::constants::window_backdrop_uses_mica(
                    config.window_backdrop.as_deref(),
                ) {
                    crate::window_chrome::backdrop::apply_system_backdrop(
                        window,
                        crate::theme::active_theme().background.l > 0.5,
                        crate::window_chrome::window_backdrop(&config, window).is_blended(),
                    );
                }
                #[cfg(target_os = "linux")]
                crate::window_chrome::linux_backdrop::sync_window_backdrop(
                    window,
                    crate::window_chrome::window_backdrop(&config, window).is_blended(),
                    cx,
                );
                let window_id = window.window_handle().window_id();
                crate::agents::notifications::set_window_active(
                    window_id,
                    window.is_window_active(),
                );
                let view = cx.new(|cx| {
                    cx.observe(&content, |_, _, cx| cx.notify()).detach();
                    if let Some(owner) = owner.upgrade() {
                        cx.observe(&owner, |this: &mut DetachedPaneWindow, owner, cx| {
                            let owner = owner.read(cx);
                            this.background_style = owner.cached_config.resolved_background_style();
                            if !owner.owns_pane(&this.pane) {
                                this.orphaned = true;
                            }
                            cx.notify();
                        })
                        .detach();
                    }
                    cx.observe_window_activation(
                        window,
                        |this: &mut DetachedPaneWindow, window, cx| {
                            crate::agents::notifications::set_window_active(
                                window.window_handle().window_id(),
                                window.is_window_active(),
                            );
                            crate::terminal::blink::show_cursor(cx);
                            if window.is_window_active()
                                && let Some(owner) = this.owner.upgrade()
                            {
                                let pane = this.pane.clone();
                                owner.update(cx, |owner, cx| {
                                    owner.probe_pane_workspace_git_now(&pane, cx)
                                });
                            }
                            cx.notify();
                        },
                    )
                    .detach();
                    DetachedPaneWindow {
                        pane: content.clone(),
                        owner: owner.clone(),
                        should_move: false,
                        initial_focus: restored_bounds.is_none(),
                        bounds_save: None,
                        orphaned: false,
                        reveal_tabs: true,
                        background_style: config.resolved_background_style(),
                        #[cfg(target_os = "windows")]
                        backdrop_light: None,
                        #[cfg(target_os = "windows")]
                        backdrop_acrylic: Some(
                            crate::window_chrome::window_backdrop(&config, window).is_blended(),
                        ),
                        #[cfg(target_os = "windows")]
                        system_backdrop: crate::app::constants::window_backdrop_uses_mica(
                            config.window_backdrop.as_deref(),
                        ),
                        #[cfg(target_os = "macos")]
                        macos_appearance: Default::default(),
                    }
                });
                let close_owner = owner.clone();
                let close_pane = content.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    close_owner
                        .update(cx, |owner, cx| owner.reattach_pane(&close_pane, cx))
                        .is_err()
                });
                view.update(cx, |_, cx| {
                    cx.observe_window_bounds(window, |this, window, cx| {
                        this.reveal_tabs = true;
                        cx.notify();
                        if let Some(bounds) = crate::window_state::windowed_bounds(window) {
                            this.pane.update(cx, |pane, _| {
                                if let Some(placement) = &mut pane.detached {
                                    placement.bounds = bounds;
                                }
                            });
                            this.save_bounds_later(cx);
                        }
                    })
                    .detach();
                });
                view
            },
        )?;
        self.prepare_pane_transfer(&pane, cx);
        pane.update(cx, |pane, cx| {
            pane.detached = Some(DetachedPanePlacement {
                window: handle.into(),
                bounds,
            });
            cx.notify();
        });
        self.set_detached_through_model(&pane, true, cx);
        if restored_bounds.is_none() {
            source.blur(cx);
            if let Some(root) = self.nav_root() {
                root.focus_first(source, cx);
            }
        }
        cx.notify();
        Ok(())
    }
}

fn fit_bounds(proposed: Bounds<Pixels>, visible: Bounds<Pixels>) -> Bounds<Pixels> {
    let width = proposed
        .size
        .width
        .as_f32()
        .clamp(MIN_WIDTH, visible.size.width.as_f32().max(MIN_WIDTH));
    let height = proposed
        .size
        .height
        .as_f32()
        .clamp(MIN_HEIGHT, visible.size.height.as_f32().max(MIN_HEIGHT));
    Bounds::new(
        point(
            px(proposed.origin.x.as_f32().clamp(
                visible.left().as_f32(),
                (visible.right().as_f32() - width).max(visible.left().as_f32()),
            )),
            px(proposed.origin.y.as_f32().clamp(
                visible.top().as_f32(),
                (visible.bottom().as_f32() - height).max(visible.top().as_f32()),
            )),
        ),
        size(px(width), px(height)),
    )
}

impl DetachedPaneWindow {
    fn save_bounds_later(&mut self, cx: &mut Context<Self>) {
        self.bounds_save = Some(cx.spawn(async move |this, cx| {
            smol::Timer::after(std::time::Duration::from_millis(300)).await;
            let _ = this.update(cx, |this, cx| {
                let _ = this.owner.update(cx, |owner, cx| owner.save_session(cx));
            });
        }));
    }
}

impl Render for DetachedPaneWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.orphaned || self.owner.upgrade().is_none() {
            window.defer(cx, |window, _| window.remove_window());
            return div().into_any_element();
        }
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |owner, cx| {
                if owner.acknowledge_seen_declared_errors(cx) {
                    cx.notify();
                }
            });
        });
        if self.initial_focus {
            self.initial_focus = false;
            self.pane.read(cx).focus_handle(cx).focus(window, cx);
        }
        if std::mem::take(&mut self.reveal_tabs) {
            cx.on_next_frame(window, |this, _, cx| {
                this.pane.read(cx).reveal_active_surface();
                cx.notify();
            });
        }
        let title = self.pane.read(cx).window_title(cx);
        window.set_window_title(&format!("{title} - Paneflow"));
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();
        let backdrop = crate::window_chrome::style_backdrop(self.background_style, window);
        crate::window_chrome::veil::note_frame_backdrop(backdrop);
        #[cfg(target_os = "linux")]
        crate::window_chrome::linux_backdrop::sync_window_backdrop(
            window,
            backdrop.is_blended(),
            cx,
        );
        #[cfg(target_os = "windows")]
        if self.backdrop_light != Some(theme.background.l > 0.5) {
            crate::window_chrome::backdrop::sync_wallpaper_mica_theme(
                window,
                theme.background.l > 0.5,
            );
            self.backdrop_light = Some(theme.background.l > 0.5);
        }
        #[cfg(target_os = "windows")]
        if self.backdrop_acrylic != Some(backdrop.is_blended()) {
            if self.system_backdrop {
                crate::window_chrome::backdrop::sync_system_backdrop(window, backdrop.is_blended());
            }
            self.backdrop_acrylic = Some(backdrop.is_blended());
        }
        #[cfg(target_os = "macos")]
        self.macos_appearance.sync(window, backdrop);
        let shell_color = if window.is_window_active() {
            theme.title_bar_background
        } else {
            theme.title_bar_inactive_background
        };
        let shell_background =
            crate::app::constants::cockpit_backdrop_background(shell_color, backdrop);
        let backdrop_background =
            crate::app::constants::cockpit_shell_background(shell_color, backdrop);
        let pane = self.pane.clone();
        let close = move |window: &mut Window, cx: &mut gpui::App| {
            pane.update(cx, |pane, cx| pane.toggle_detached(window, cx));
        };
        let render_controls = !cfg!(target_os = "macos")
            && !window.is_fullscreen()
            && (matches!(
                crate::window_chrome::csd::decorations(window),
                gpui::Decorations::Client { .. }
            ) || cfg!(target_os = "windows"));
        let right = render_controls
            .then(|| {
                crate::window_chrome::csd::render_button_group(
                    "detached",
                    window.is_maximized(),
                    px(44.),
                    &window.window_controls(),
                    close,
                )
            })
            .flatten();
        let tabs = self.pane.update(cx, |pane, cx| {
            pane.render_tab_bar(Some(shell_background), cx)
        });
        let titlebar = div()
            .id("detached-titlebar")
            .bg(shell_background)
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .items_center()
            .h(TITLE_BAR_HEIGHT)
            .flex_none()
            .gap(px(12.))
            .pl(px(if cfg!(target_os = "macos") { 80. } else { 7. }))
            .child(div().min_w_0().flex_1().h_full().children(tabs))
            .when(cfg!(target_os = "windows") && right.is_some(), |titlebar| {
                titlebar.child(div().flex_none().w(px(1.)).h(px(16.)).bg(ui.border))
            })
            .children(right)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.should_move = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.should_move = false),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.should_move = false))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if std::mem::take(&mut this.should_move) {
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.zoom_window();
                }
            });
        let content = div()
            .id("detached-pane-window")
            .flex()
            .flex_col()
            .size_full()
            .text_color(ui.text)
            .on_action(cx.listener(|this, _: &ToggleDetachedPane, window, cx| {
                this.pane
                    .update(cx, |pane, cx| pane.toggle_detached(window, cx));
            }))
            .on_action(cx.listener(|this, _: &crate::ClosePane, _, cx| {
                this.pane.update(cx, |pane, cx| pane.close(cx));
            }))
            .on_action(cx.listener(|this, _: &crate::NewTab, _, cx| {
                this.pane.update(cx, |_, cx| cx.emit(PaneEvent::NewTab));
            }))
            .on_action(cx.listener(|this, _: &crate::Quit, _, cx| {
                let _ = this.owner.update(cx, |owner, cx| owner.request_quit(cx));
            }))
            .on_action(|_: &crate::OpenSettings, _, cx| {
                forward_to_workspace(crate::OpenSettings, cx)
            })
            .on_action(|_: &crate::OpenCommandPalette, _, cx| {
                forward_to_workspace(crate::OpenCommandPalette, cx)
            })
            .child(titlebar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_l(px(7.))
                    .border_r(px(7.))
                    .border_b(px(7.))
                    .border_color(shell_background)
                    .child(self.pane.clone()),
            );
        crate::window_chrome::csd::client_side_window_shell(
            content,
            window,
            backdrop_background,
            ui.border,
        )
        .into_any_element()
    }
}

fn forward_to_workspace(action: impl gpui::Action, cx: &mut gpui::App) {
    cx.defer(move |cx| {
        if let Some(handle) = cx
            .windows()
            .into_iter()
            .find_map(|handle| handle.downcast::<PaneFlowApp>())
        {
            let _ = handle.update(cx, |_, window, cx| {
                window.activate_window();
                window.dispatch_action(Box::new(action), cx);
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_window_bounds_remain_visible_on_negative_coordinate_monitors() {
        let visible = Bounds::new(point(px(-1920.), px(40.)), size(px(1920.), px(1040.)));
        let proposed = Bounds::new(point(px(-4000.), px(2000.)), size(px(1000.), px(800.)));
        let fitted = fit_bounds(proposed, visible);
        assert_eq!(fitted.origin, point(px(-1920.), px(280.)));
        assert_eq!(fitted.size, proposed.size);
    }

    #[test]
    fn detached_window_restoration_fits_oversized_bounds_to_display() {
        let visible = Bounds::new(point(px(0.), px(0.)), size(px(1280.), px(720.)));
        let proposed = Bounds::new(point(px(3000.), px(2000.)), size(px(4000.), px(3000.)));
        assert_eq!(fit_bounds(proposed, visible), visible);
    }
}
