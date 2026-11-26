use std::time::Duration;

use gpui::{
    AnyElement, AppContext, Context, DispatchPhase, Entity, InteractiveElement, IntoElement,
    MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Styled, Window, canvas, div,
    prelude::FluentBuilder, px,
};
use paneflow_browser_proto::{Reply, TabId};
use serde_json::json;

use super::router;
use super::view::BrowserView;
use crate::settings::kit::Ink;

const MIN_WIDTH: f32 = 280.;
const PAGE_MIN_WIDTH: f32 = 240.;
const DEFAULT_SHARE: f32 = 0.42;
const HANDLE_WIDTH: f32 = 6.;
const PANEL_ATTEMPTS: usize = 150;
const PANEL_RETRY: Duration = Duration::from_millis(100);
const CALL_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DevToolsKey {
    Toggle,
    Console,
    Inspect,
}

fn panel_script(key: DevToolsKey) -> Option<&'static str> {
    match key {
        DevToolsKey::Toggle => None,
        DevToolsKey::Console => Some(concat!(
            "(() => { const all = [];",
            " const walk = (root) => { for (const el of root.querySelectorAll('*')) { all.push(el); if (el.shadowRoot) walk(el.shadowRoot); } };",
            " walk(document);",
            " const tab = all.find((el) => el.id === 'tab-console');",
            " if (!tab || !globalThis.InspectorFrontendAPI) return false;",
            " if (tab.getAttribute('aria-selected') === 'true') return true;",
            " InspectorFrontendAPI.showPanel('console'); return false; })()",
        )),
        DevToolsKey::Inspect => Some(concat!(
            "(() => { const all = [];",
            " const walk = (root) => { for (const el of root.querySelectorAll('*')) { all.push(el); if (el.shadowRoot) walk(el.shadowRoot); } };",
            " walk(document);",
            " const picker = all.find((el) => el.tagName === 'BUTTON' && el.hasAttribute('aria-pressed') && /select an element/i.test(el.getAttribute('aria-label') || ''));",
            " if (!picker || !globalThis.InspectorFrontendAPI) return false;",
            " if (picker.getAttribute('aria-pressed') === 'true') return true;",
            " InspectorFrontendAPI.enterInspectElementMode(); return false; })()",
        )),
    }
}

pub(crate) fn docked_width(wanted: Option<f32>, total: f32) -> f32 {
    let ceiling = (total - PAGE_MIN_WIDTH - HANDLE_WIDTH).max(MIN_WIDTH);
    wanted
        .unwrap_or(total * DEFAULT_SHARE)
        .clamp(MIN_WIDTH, ceiling)
}

pub(crate) struct Docked {
    pub(crate) view: Entity<BrowserView>,
    pub(crate) inspects: TabId,
    width: Option<f32>,
    row_width: f32,
    drag: Option<(f32, f32)>,
}

impl BrowserView {
    pub(crate) fn inspecting(
        workspace_id: u64,
        inspector: TabId,
        page: TabId,
        displayed: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(workspace_id, cx);
        view.inspects = Some(page);
        view.tab = Some(inspector);
        view.set_shown(displayed);
        router::register(inspector, cx.weak_entity(), cx);
        view
    }

    pub(crate) fn devtools_shortcut(
        &mut self,
        key: DevToolsKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (key, self.docked.is_some()) {
            (DevToolsKey::Toggle, true) => self.close_inspector(window, cx),
            (_, true) => self.run_in_inspector(key, window, cx),
            (_, false) => self.open_inspector(key, window, cx),
        }
    }

    fn open_inspector(&mut self, key: DevToolsKey, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(page), None) = (self.tab, self.inspects) else {
            return;
        };
        let (Ok(runtime), Ok(_)) = (crate::browser_control::runtime(), router::install(cx)) else {
            return;
        };
        let (inspector, ticket) = runtime.client.inspect(page);
        runtime.control.adopt_inspector(inspector);
        let workspace_id = self.workspace_id;
        let displayed = self.shown();
        let view =
            cx.new(|cx| BrowserView::inspecting(workspace_id, inspector, page, displayed, cx));
        self.docked = Some(Docked {
            view: view.clone(),
            inspects: page,
            width: None,
            row_width: 0.,
            drag: None,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let opened = smol::unblock(move || ticket.wait(CALL_DEADLINE * 5)).await;
            if let Err(error) = opened {
                log::warn!("developer tools did not open: {error}");
                let _ = this.update(cx, |page, cx| {
                    if page
                        .docked
                        .as_ref()
                        .is_some_and(|docked| docked.view == view)
                    {
                        page.docked = None;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        self.run_in_inspector(key, window, cx);
    }

    pub(crate) fn close_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.docked.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn run_in_inspector(&mut self, key: DevToolsKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(docked) = self.docked.as_ref() else {
            return;
        };
        let (inspector, focus) = {
            let view = docked.view.read(cx);
            (view.tab, view.focus.clone())
        };
        window.focus(&focus, cx);
        let (Some(script), Some(inspector), Ok(runtime)) = (
            panel_script(key),
            inspector,
            crate::browser_control::runtime(),
        ) else {
            return;
        };
        let client = std::sync::Arc::clone(&runtime.client);
        cx.background_spawn(async move {
            for _ in 0..PANEL_ATTEMPTS {
                let ticket = client.devtools(
                    inspector,
                    "Runtime.evaluate",
                    json!({ "expression": script, "returnByValue": true }),
                );
                let answer = smol::unblock(move || ticket.wait(CALL_DEADLINE)).await;
                match answer {
                    Ok(Reply::DevTools(value))
                        if value.pointer("/result/value") == Some(&json!(true)) =>
                    {
                        return;
                    }
                    Ok(_) => smol::Timer::after(PANEL_RETRY).await,
                    Err(_) => return,
                };
            }
        })
        .detach();
    }

    pub(crate) fn drop_inspector_unless(&mut self, page: Option<TabId>) {
        if self
            .docked
            .as_ref()
            .is_some_and(|docked| Some(docked.inspects) != page)
        {
            self.docked = None;
        }
    }

    fn drag_inspector(&mut self, at: Pixels, cx: &mut Context<Self>) {
        let Some(docked) = self.docked.as_mut() else {
            return;
        };
        let Some((from, width)) = docked.drag else {
            return;
        };
        let moved = f32::from(at) - from;
        docked.width = Some(docked_width(Some(width - moved), docked.row_width));
        cx.notify();
    }

    fn end_inspector_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(docked) = self.docked.as_mut()
            && docked.drag.take().is_some()
        {
            cx.notify();
        }
    }

    pub(crate) fn render_docked(
        &mut self,
        page: AnyElement,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(docked) = self.docked.as_ref() else {
            return page;
        };
        let dragging = docked.drag.is_some();
        let width = docked_width(docked.width, docked.row_width);
        let entity = cx.entity();
        let handle = div()
            .id("inspector-resize")
            .flex_none()
            .w(px(HANDLE_WIDTH))
            .h_full()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .flex()
            .justify_center()
            .child(div().w(px(1.)).h_full().bg(ink.edge))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    if let Some(docked) = this.docked.as_mut() {
                        docked.drag = Some((f32::from(event.position.x), width));
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        div()
            .id("browser-docked")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_row()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |view, cx| {
                            let measured = f32::from(bounds.size.width);
                            if let Some(docked) = view.docked.as_mut()
                                && docked.row_width != measured
                            {
                                docked.row_width = measured;
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(page),
            )
            .child(handle)
            .child(
                div()
                    .flex_none()
                    .w(px(width))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(docked.view.clone()),
            )
            .when(dragging, |row| {
                let entity = cx.entity();
                row.child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let moving = entity.clone();
                            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                                if phase == DispatchPhase::Capture {
                                    moving.update(cx, |view, cx| {
                                        view.drag_inspector(event.position.x, cx)
                                    });
                                }
                            });
                            let releasing = entity.clone();
                            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                                if phase == DispatchPhase::Capture
                                    && event.button == MouseButton::Left
                                {
                                    releasing.update(cx, |view, cx| view.end_inspector_drag(cx));
                                }
                            });
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_docked_width_starts_at_a_share_and_leaves_room_for_the_page() {
        assert_eq!(docked_width(None, 1000.), 420.);
        assert_eq!(docked_width(Some(100.), 1000.), MIN_WIDTH);
        assert_eq!(
            docked_width(Some(900.), 1000.),
            1000. - PAGE_MIN_WIDTH - HANDLE_WIDTH
        );
        assert_eq!(docked_width(Some(500.), 400.), MIN_WIDTH);
    }
}
