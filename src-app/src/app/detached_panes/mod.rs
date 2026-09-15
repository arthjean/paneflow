mod persistence;
mod window;

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, Entity, Focusable, IntoElement,
    ParentElement, Pixels, Styled, Window, div, px,
};

use crate::layout::LayoutTree;
use crate::pane::{Pane, PaneEvent};
use crate::{PaneFlowApp, ToggleDetachedPane};

#[derive(Clone, Copy)]
pub(crate) struct DetachedPanePlacement {
    pub(crate) window: AnyWindowHandle,
    pub(crate) bounds: Bounds<Pixels>,
}

impl PaneFlowApp {
    pub(crate) fn detached_window_closed(&mut self, id: gpui::WindowId, cx: &mut Context<Self>) {
        let panes = self
            .workspaces
            .iter()
            .flat_map(|ws| ws.collect_panes())
            .collect::<Vec<_>>();
        let mut changed = false;
        for pane in panes {
            if pane
                .read(cx)
                .detached
                .is_some_and(|placement| placement.window.window_id() == id)
            {
                pane.update(cx, |pane, cx| {
                    pane.detached = None;
                    cx.notify();
                });
                changed = true;
            }
        }
        if changed {
            self.cancel_pane_layout_resize();
            self.save_session(cx);
            cx.notify();
        }
    }

    pub(crate) fn handle_toggle_detached_pane(
        &mut self,
        _: &ToggleDetachedPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane) = self
            .nav_root()
            .and_then(|root| root.focused_pane(window, cx))
        {
            self.toggle_detached_pane(pane, window, cx);
        }
    }

    pub(crate) fn toggle_detached_pane(
        &mut self,
        pane: Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if pane.read(cx).is_detached() {
            self.reattach_pane(&pane, cx);
        } else if let Err(error) = self.detach_pane(pane, window, None, cx) {
            log::warn!("Could not detach pane: {error:#}");
            self.show_toast(
                "Could not open a window. The pane remains in its workspace.",
                cx,
            );
        }
    }

    fn owns_pane(&self, pane: &Entity<Pane>) -> bool {
        self.workspaces
            .iter()
            .any(|ws| ws.tab_for_pane(pane).is_some())
    }

    fn cancel_pane_layout_resize(&self) {
        for ws in &self.workspaces {
            for tab in ws.tabs() {
                for root in [tab.root.as_ref(), tab.saved_layout.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    root.cancel_resize();
                }
            }
        }
    }

    fn prepare_pane_transfer(&mut self, pane: &Entity<Pane>, cx: &mut Context<Self>) {
        self.cancel_pane_layout_resize();
        self.cancel_swap_mode(cx);
        self.dismiss_transient_surfaces();
        for ws in &mut self.workspaces {
            if let Some(tab_idx) = ws.tab_index_containing_pane(pane)
                && let Some(tab) = ws.tab_mut(tab_idx)
            {
                tab.exit_zoom(cx);
                break;
            }
        }
        pane.update(cx, |pane, cx| {
            pane.zoomed = false;
            pane.reveal_active_surface();
            pane.set_dimmed(false, cx);
            for terminal in pane.terminals() {
                terminal.update(cx, |_, cx| cx.notify());
            }
            cx.notify();
        });
    }

    pub(crate) fn reattach_pane(&mut self, pane: &Entity<Pane>, cx: &mut Context<Self>) {
        let placement = pane.read(cx).detached;
        let Some(placement) = placement else { return };
        self.prepare_pane_transfer(pane, cx);
        pane.update(cx, |pane, cx| {
            pane.detached = None;
            cx.notify();
        });
        if self.owns_pane(pane) {
            self.settings_section = None;
            if let Some((ws_idx, tab_idx)) = self
                .workspaces
                .iter()
                .enumerate()
                .find_map(|(idx, ws)| ws.tab_index_containing_pane(pane).map(|tab| (idx, tab)))
            {
                self.activate_workspace_without_window(ws_idx, cx);
                self.workspaces[ws_idx].set_active_tab(tab_idx);
            }
            self.pending_pane_focus = Some(pane.clone());
        }
        let pane = pane.clone();
        cx.defer(move |cx| {
            let _ = placement
                .window
                .update(cx, |_, window, _| window.remove_window());
            for handle in cx.windows() {
                if let Some(main) = handle.downcast::<PaneFlowApp>() {
                    let _ = main.update(cx, |_, window, cx| {
                        window.activate_window();
                        pane.read(cx).focus_handle(cx).focus(window, cx);
                        window.refresh();
                    });
                    break;
                }
            }
        });
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn focus_pane_window(pane: Entity<Pane>, cx: &mut App) -> bool {
        let Some(placement) = pane.read(cx).detached else {
            return false;
        };
        cx.defer(move |cx| {
            let _ = placement.window.update(cx, |_, window, cx| {
                window.activate_window();
                pane.read(cx).focus_handle(cx).focus(window, cx);
                window.refresh();
            });
        });
        true
    }
}

pub(crate) fn render_detached_placeholder(tree: &LayoutTree, cx: &App) -> AnyElement {
    let ui = crate::theme::ui_colors();
    let rows = tree.collect_leaves().into_iter().filter_map(|pane| {
        let placement = pane.read(cx).detached?;
        let title = pane.read(cx).window_title(cx);
        let show = pane.clone();
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(div().min_w_0().flex_1().text_ellipsis().child(title))
                .child(crate::settings::components::secondary_button(
                    format!("show-detached-pane-{}", pane.entity_id()),
                    "Show window",
                    ui,
                    move |_, _, cx| {
                        PaneFlowApp::focus_pane_window(show.clone(), cx);
                    },
                ))
                .child(crate::settings::components::secondary_button(
                    format!("return-detached-pane-{}", pane.entity_id()),
                    "Return here",
                    ui,
                    move |_, _, cx| {
                        pane.update(cx, |_, cx| {
                            cx.emit(PaneEvent::ToggleDetached {
                                window: placement.window,
                            })
                        });
                    },
                )),
        )
    });
    div()
        .size_full()
        .flex()
        .flex_col()
        .justify_center()
        .items_center()
        .gap(px(16.))
        .text_color(ui.text)
        .text_size(px(13.))
        .child("Panes are open in separate windows")
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .max_w_full()
                .children(rows),
        )
        .into_any_element()
}
