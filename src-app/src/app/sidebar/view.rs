use std::time::Duration;

use gpui::{
    Context, Entity, IntoElement, Render, StyleRefinement, Styled, Subscription, Task, WeakEntity,
    Window, div, px,
};

use crate::worker_bootstrap::WorkerBoot;
use crate::{PaneFlowApp, SIDEBAR_WIDTH};

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

    pub(crate) fn cached_style() -> StyleRefinement {
        StyleRefinement::default()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_shrink_0()
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
                app.render_sidebar(window, cx).into_any_element()
            })
            .unwrap_or_else(|_| div().into_any_element());
        self.spinner_clock.sync(cx);
        self.watch_worker_boot(&crate::worker_bootstrap::state(), cx);
        sidebar
    }
}
