use std::time::Duration;

use gpui::{
    AnyView, Context, Div, Entity, IntoElement, ParentElement, Pixels, Render, StyleRefinement,
    Styled, Subscription, Task, WeakEntity, Window, div, px,
};

use crate::PaneFlowApp;
use crate::worker_bootstrap::WorkerBoot;

use super::SpinnerClock;

const WORKER_BOOT_POLL: Duration = Duration::from_millis(250);

pub(crate) struct SidebarView {
    app: WeakEntity<PaneFlowApp>,
    spinner_clock: SpinnerClock,
    worker_boot_watch: Option<Task<()>>,
    _app_changes: Subscription,
}

impl SidebarView {
    pub(crate) fn new(app: &Entity<PaneFlowApp>, cx: &mut Context<Self>) -> Self {
        Self {
            app: app.downgrade(),
            spinner_clock: SpinnerClock::default(),
            worker_boot_watch: None,
            _app_changes: cx.observe(app, |_, _, cx| cx.notify()),
        }
    }

    pub(crate) fn cached_style(width: f32) -> StyleRefinement {
        StyleRefinement::default()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
    }

    pub(crate) fn faded_column(
        sidebar: AnyView,
        sidebar_width: f32,
        column_width: f32,
        opacity: f32,
        top: Pixels,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .h_full()
            .w(px(column_width))
            .flex_shrink_0()
            .overflow_hidden()
            .opacity(opacity)
            .pt(top)
            .child(sidebar.cached(Self::cached_style(sidebar_width)))
    }

    fn watch_worker_boot(&mut self, boot: &WorkerBoot, cx: &mut Context<Self>) {
        if *boot != WorkerBoot::Pending {
            self.worker_boot_watch = None;
            return;
        }
        if self.worker_boot_watch.is_some() {
            return;
        }
        self.worker_boot_watch = Some(cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(WORKER_BOOT_POLL).await;
                if crate::worker_bootstrap::state() != WorkerBoot::Pending {
                    let _ = view.update(cx, |_, cx| cx.notify());
                    break;
                }
            }
        }));
    }
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self
            .app
            .update(cx, |app, cx| {
                app.prune_sidebar_selection();
                app.render_sidebar(window, cx).into_any_element()
            })
            .unwrap_or_else(|_| div().into_any_element());
        self.spinner_clock.sync(cx);
        self.watch_worker_boot(&crate::worker_bootstrap::state(), cx);
        sidebar
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{AppContext as _, TestAppContext, canvas, deferred};

    use super::*;
    use crate::redraw_guard::{WatchedView, frames_rendered};

    const SIDEBAR_WIDTH: f32 = 200.;

    struct CountedSidebar {
        watched: WatchedView,
    }

    impl Render for CountedSidebar {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.watched.rendered();
            div().size_full().bg(gpui::red())
        }
    }

    struct FadingRoot {
        sidebar: Entity<CountedSidebar>,
        opacity: f32,
    }

    impl Render for FadingRoot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().child(SidebarView::faded_column(
                self.sidebar.clone().into(),
                SIDEBAR_WIDTH,
                SIDEBAR_WIDTH * self.opacity,
                self.opacity,
                px(0.),
            ))
        }
    }

    #[gpui::test]
    fn a_collapsing_sidebar_repaints_its_cached_view_at_the_new_opacity_on_that_frame(
        cx: &mut TestAppContext,
    ) {
        let watched = WatchedView::new("SidebarView");
        let (root, cx) = cx.add_window_view({
            let watched = watched.clone();
            move |_, cx| FadingRoot {
                sidebar: cx.new(|_| CountedSidebar { watched }),
                opacity: 1.,
            }
        });
        cx.run_until_parked();
        assert_eq!(watched.renders.get(), 1);

        root.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert_eq!(
            watched.renders.get(),
            1,
            "an unchanged opacity reuses the cached sidebar"
        );

        for (step, opacity) in [0.75, 0.5, 0.25].into_iter().enumerate() {
            root.update(cx, |root, cx| {
                root.opacity = opacity;
                cx.notify();
            });
            cx.run_until_parked();
            assert_eq!(
                watched.renders.get(),
                2 + step,
                "the cached sidebar repaints in the frame that changes its opacity to {opacity}"
            );
            assert_eq!(
                cx.update(|window, cx| window.simulate_next_frame(cx)),
                0,
                "the repaint needs no follow-up frame"
            );
        }
    }

    struct Label(&'static str);

    struct MenuContent {
        label: Entity<Label>,
        shown: Rc<Cell<&'static str>>,
        _label_changes: Subscription,
    }

    impl Render for MenuContent {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let text = self.label.read(cx).0;
            self.shown.set(text);
            div().w(px(80.)).h(px(20.)).child(text)
        }
    }

    struct SidebarWithMenu {
        menu: Entity<MenuContent>,
        badge: Entity<Label>,
        painted_badge: Rc<Cell<&'static str>>,
        watched: WatchedView,
    }

    impl Render for SidebarWithMenu {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.watched.rendered();
            let badge = self.badge.clone();
            let painted_badge = self.painted_badge.clone();
            let panel = div()
                .absolute()
                .left(px(20.))
                .top(px(20.))
                .child(self.menu.clone())
                .child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, _, cx| painted_badge.set(badge.read(cx).0),
                    )
                    .size(px(10.)),
                );
            div()
                .size_full()
                .child(deferred(crate::ui_primitives::menu_reveal("menu", panel)).priority(3))
        }
    }

    struct MenuRoot {
        sidebar: Entity<SidebarWithMenu>,
        watched: WatchedView,
    }

    impl Render for MenuRoot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.watched.rendered();
            div().size_full().flex().child(SidebarView::faded_column(
                self.sidebar.clone().into(),
                SIDEBAR_WIDTH,
                SIDEBAR_WIDTH,
                1.,
                px(0.),
            ))
        }
    }

    #[gpui::test]
    fn an_entity_read_only_inside_a_deferred_menu_reaches_it_after_the_sidebar_is_reused(
        cx: &mut TestAppContext,
    ) {
        let sidebar_watch = WatchedView::new("SidebarView");
        let root_watch = WatchedView::new("root");
        let shown = Rc::new(Cell::new(""));
        let painted_badge = Rc::new(Cell::new(""));
        let label = cx.new(|_| Label("Rename"));
        let badge = cx.new(|_| Label("idle"));
        let (root, cx) = cx.add_window_view({
            let (sidebar_watch, root_watch) = (sidebar_watch.clone(), root_watch.clone());
            let (shown, painted_badge) = (shown.clone(), painted_badge.clone());
            let (label, badge) = (label.clone(), badge.clone());
            move |_, cx| {
                let menu = cx.new(|cx| MenuContent {
                    _label_changes: cx.observe(&label, |_, _, cx| cx.notify()),
                    label,
                    shown,
                });
                MenuRoot {
                    sidebar: cx.new(|_| SidebarWithMenu {
                        menu,
                        badge,
                        painted_badge,
                        watched: sidebar_watch,
                    }),
                    watched: root_watch,
                }
            }
        });
        cx.run_until_parked();
        frames_rendered(cx, &[]);
        assert_eq!(shown.get(), "Rename");
        assert_eq!(painted_badge.get(), "idle");

        let sidebar_renders = sidebar_watch.renders.get();
        for _ in 0..2 {
            root.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
        }
        assert_eq!(
            sidebar_watch.renders.get(),
            sidebar_renders,
            "the sidebar is drawn from its cache"
        );

        label.update(cx, |label, cx| {
            label.0 = "Rename space";
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(
            shown.get(),
            "Rename space",
            "the deferred menu shows the new value on the next frame"
        );

        let root_renders = root_watch.renders.get();
        badge.update(cx, |badge, cx| {
            badge.0 = "thinking";
            cx.notify();
        });
        cx.run_until_parked();
        assert!(
            root_watch.renders.get() > root_renders,
            "an entity read only while painting the deferred menu still wakes the window after a cache reuse"
        );
    }
}
