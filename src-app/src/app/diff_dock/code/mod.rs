pub(crate) mod base;
#[cfg(test)]
pub(crate) mod bench_corpus;
pub(crate) mod controls;
pub(crate) mod cursor;
pub(crate) mod document;
pub(crate) mod edit;
pub(crate) mod element;
pub(crate) mod highlight;
pub(crate) mod load;
pub(crate) mod markers;
mod minimap;
mod navigation;
mod navigation_paint;
#[cfg(test)]
mod perf_bench;
pub(crate) mod save;
pub(crate) mod view;

pub(crate) fn spawn_blocking_then<V, T, W, A>(cx: &mut gpui::Context<V>, work: W, apply: A)
where
    V: 'static,
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    A: FnOnce(&mut V, T, &mut gpui::Context<V>) + 'static,
{
    cx.spawn(
        async move |this: gpui::WeakEntity<V>, cx: &mut gpui::AsyncApp| {
            #[cfg(not(test))]
            let output = smol::unblock(work).await;
            #[cfg(test)]
            let output = gpui::AppContext::background_spawn(cx, async move { work() }).await;
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut V, cx: &mut gpui::Context<V>| {
                    apply(view, output, cx);
                });
            });
        },
    )
    .detach();
}
