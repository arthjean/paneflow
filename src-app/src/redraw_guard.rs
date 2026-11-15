use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::VisualTestContext;

pub(crate) const GUARD_FRAMES: usize = 60;
pub(crate) const GUARD_FRAME_STEP: Duration = Duration::from_millis(16);

#[derive(Clone)]
pub(crate) struct WatchedView {
    pub(crate) name: &'static str,
    pub(crate) renders: Rc<Cell<usize>>,
}

impl WatchedView {
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            name,
            renders: Rc::new(Cell::new(0)),
        }
    }

    pub(crate) fn rendered(&self) {
        self.renders.set(self.renders.get() + 1);
    }
}

pub(crate) fn frames_rendered(cx: &mut VisualTestContext, watched: &[WatchedView]) -> Vec<usize> {
    let mut frames_with_render = vec![0; watched.len()];
    for _ in 0..GUARD_FRAMES {
        let before: Vec<usize> = watched.iter().map(|view| view.renders.get()).collect();
        cx.executor().advance_clock(GUARD_FRAME_STEP);
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        cx.run_until_parked();
        for (index, view) in watched.iter().enumerate() {
            if view.renders.get() > before[index] {
                frames_with_render[index] += 1;
            }
        }
    }
    frames_with_render
}

pub(crate) fn assert_no_redraw_loop(cx: &mut VisualTestContext, watched: &[WatchedView]) {
    let frames = frames_rendered(cx, watched);
    let looping: Vec<&str> = watched
        .iter()
        .zip(&frames)
        .filter(|(_, rendered)| **rendered == GUARD_FRAMES)
        .map(|(view, _)| view.name)
        .collect();
    assert!(
        looping.is_empty(),
        "redraw loop: {} re-rendered on every one of {GUARD_FRAMES} frames",
        looping.join(", ")
    );
}

#[cfg(test)]
mod tests {
    use gpui::{
        AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, StyleRefinement,
        Styled as _, TestAppContext, Window, div, px,
    };

    use super::*;

    struct Ticker;

    struct LoopingReader {
        ticker: Entity<Ticker>,
        watched: WatchedView,
        _ticks: gpui::Subscription,
    }

    impl Render for LoopingReader {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.watched.rendered();
            let _ = self.ticker.read(cx);
            let ticker = self.ticker.clone();
            window.on_next_frame(move |_, cx| ticker.update(cx, |_, cx| cx.notify()));
            div().size_full()
        }
    }

    struct QuietReader {
        ticker: Entity<Ticker>,
        watched: WatchedView,
    }

    impl Render for QuietReader {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.watched.rendered();
            let _ = self.ticker.read(cx);
            div().size_full()
        }
    }

    struct CachedHost {
        child: gpui::AnyView,
    }

    impl Render for CachedHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                self.child
                    .clone()
                    .cached(StyleRefinement::default().w(px(200.)).h_full()),
            )
        }
    }

    #[gpui::test]
    #[should_panic(expected = "redraw loop: LoopingReader re-rendered on every one of 60 frames")]
    fn an_entity_notifying_every_frame_under_a_cached_view_names_the_looping_view(
        cx: &mut TestAppContext,
    ) {
        let watched = WatchedView::new("LoopingReader");
        let (_, cx) = cx.add_window_view({
            let watched = watched.clone();
            move |_, cx| {
                let ticker = cx.new(|_| Ticker);
                let child = cx.new(|cx| LoopingReader {
                    _ticks: cx.observe(&ticker, |_, _, cx| cx.notify()),
                    ticker,
                    watched,
                });
                CachedHost {
                    child: child.into(),
                }
            }
        });
        cx.run_until_parked();
        assert_no_redraw_loop(cx, &[watched]);
    }

    #[gpui::test]
    fn a_settled_cached_view_passes_the_guard(cx: &mut TestAppContext) {
        let watched = WatchedView::new("QuietReader");
        let (_, cx) = cx.add_window_view({
            let watched = watched.clone();
            move |_, cx| {
                let ticker = cx.new(|_| Ticker);
                let child = cx.new(|_| QuietReader { ticker, watched });
                CachedHost {
                    child: child.into(),
                }
            }
        });
        cx.run_until_parked();
        assert_no_redraw_loop(cx, std::slice::from_ref(&watched));
        assert_eq!(watched.renders.get(), 1);
    }
}
