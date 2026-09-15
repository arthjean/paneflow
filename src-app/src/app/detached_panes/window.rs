use gpui::{
    AppContext, Bounds, Context, Entity, Focusable, IntoElement, MouseButton, ParentElement,
    Pixels, Render, SharedString, Styled, WeakEntity, Window, WindowBounds, WindowControlArea,
    WindowDecorations, WindowOptions, div, point, prelude::*, px, size,
};

use crate::pane::{Pane, PaneEvent};
use crate::ui_primitives::TooltipDelayExt;
use crate::{PaneFlowApp, ToggleDetachedPane};

use super::DetachedPanePlacement;

const MIN_WIDTH: f32 = 420.;
const MIN_HEIGHT: f32 = 280.;

pub(crate) struct DetachedPaneWindow {
    pane: Entity<Pane>,
    owner: WeakEntity<PaneFlowApp>,
    should_move: bool,
    initial_focus: bool,
    bounds_save: Option<gpui::Task<()>>,
    orphaned: bool,
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
                #[allow(clippy::needless_update)]
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    #[cfg(target_os = "macos")]
                    traffic_light_position: Some(point(px(12.), px(12.))),
                    ..Default::default()
                }),
                app_owns_titlebar_drag: true,
                app_id: Some("paneflow".into()),
                window_background: gpui::WindowBackgroundAppearance::Opaque,
                focus: restored_bounds.is_none(),
                ..Default::default()
            },
            move |window, cx| {
                let window_id = window.window_handle().window_id();
                crate::agents::notifications::set_window_active(
                    window_id,
                    window.is_window_active(),
                );
                let view = cx.new(|cx| {
                    cx.observe(&content, |_, _, cx| cx.notify()).detach();
                    if let Some(owner) = owner.upgrade() {
                        cx.observe(&owner, |this: &mut DetachedPaneWindow, owner, cx| {
                            if !owner.read(cx).owns_pane(&this.pane) {
                                this.orphaned = true;
                                cx.notify();
                            }
                        })
                        .detach();
                    }
                    cx.observe_window_activation(window, |_, window, cx| {
                        crate::agents::notifications::set_window_active(
                            window.window_handle().window_id(),
                            window.is_window_active(),
                        );
                        cx.notify();
                    })
                    .detach();
                    DetachedPaneWindow {
                        pane: content.clone(),
                        owner: owner.clone(),
                        should_move: false,
                        initial_focus: restored_bounds.is_none(),
                        bounds_save: None,
                        orphaned: false,
                    }
                });
                let close_owner = owner.clone();
                let close_pane = content.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    if close_owner
                        .update(cx, |owner, cx| owner.reattach_pane(&close_pane, cx))
                        .is_err()
                    {
                        return true;
                    }
                    false
                });
                view.update(cx, |_, cx| {
                    cx.observe_window_bounds(window, |this, window, cx| {
                        if !window.is_maximized() && !window.is_fullscreen() {
                            let bounds = window.window_bounds().get_bounds();
                            this.pane.update(cx, |pane, _| {
                                if let Some(placement) = &mut pane.detached {
                                    placement.bounds = bounds;
                                }
                            });
                            this.bounds_save = Some(cx.spawn(async move |this, cx| {
                                smol::Timer::after(std::time::Duration::from_millis(300)).await;
                                let _ = this.update(cx, |this, cx| {
                                    let _ =
                                        this.owner.update(cx, |owner, cx| owner.save_session(cx));
                                });
                            }));
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
        if restored_bounds.is_none() {
            source.blur();
            if let Some(root) = self.nav_root() {
                root.focus_first(source, cx);
            }
        }
        self.save_session(cx);
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

impl Render for DetachedPaneWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.orphaned || self.owner.upgrade().is_none() {
            window.defer(cx, |window, _| window.remove_window());
            return div().into_any_element();
        }
        if self.initial_focus {
            self.initial_focus = false;
            self.pane.read(cx).focus_handle(cx).focus(window, cx);
        }
        let title = self.pane.read(cx).window_title(cx);
        window.set_window_title(&format!("{title} - Paneflow"));
        let ui = crate::theme::ui_colors();
        let pane = self.pane.clone();
        let close = move |window: &mut Window, cx: &mut gpui::App| {
            pane.update(cx, |pane, cx| pane.toggle_detached(window, cx));
        };
        let controls = cx
            .button_layout()
            .unwrap_or_else(crate::window_chrome::csd::default_button_layout);
        let render_controls = !cfg!(target_os = "macos")
            && !window.is_fullscreen()
            && (matches!(
                window.window_decorations(),
                gpui::Decorations::Client { .. }
            ) || cfg!(target_os = "windows"));
        let left = render_controls
            .then(|| {
                crate::window_chrome::csd::render_button_group(
                    "detached-left",
                    &controls.left,
                    window.is_maximized(),
                    px(40.),
                    &window.window_controls(),
                    close.clone(),
                )
            })
            .flatten();
        let right = render_controls
            .then(|| {
                crate::window_chrome::csd::render_button_group(
                    "detached",
                    &controls.right,
                    window.is_maximized(),
                    px(40.),
                    &window.window_controls(),
                    close.clone(),
                )
            })
            .flatten();
        let back = div()
            .id("reattach-pane-tooltip")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .delayed_tooltip(crate::ui_primitives::text_tooltip(
                "Return this pane without stopping its terminal",
            ))
            .child(crate::settings::components::secondary_button(
                "reattach-pane",
                "Return to workspace",
                ui,
                move |_, window, cx| close(window, cx),
            ));
        let titlebar = div()
            .id("detached-titlebar")
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .items_center()
            .h(px(40.))
            .flex_none()
            .gap(px(8.))
            .pl(px(if cfg!(target_os = "macos") { 80. } else { 12. }))
            .children(left)
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_ellipsis()
                    .text_size(px(13.))
                    .child(SharedString::from(title)),
            )
            .child(back)
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
                let _ = this.owner.update(cx, |owner, cx| {
                    owner.save_session_blocking(cx);
                    owner.emit_app_exited_and_flush();
                    cx.quit();
                });
            }))
            .on_action(|_: &crate::OpenSettings, _, cx| {
                forward_to_workspace(crate::OpenSettings, cx)
            })
            .on_action(|_: &crate::OpenCommandPalette, _, cx| {
                forward_to_workspace(crate::OpenCommandPalette, cx)
            })
            .child(titlebar)
            .child(div().flex_1().min_h_0().child(self.pane.clone()));
        crate::window_chrome::csd::client_side_window_shell(
            content,
            window,
            crate::theme::active_theme().background,
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
