use std::sync::Arc;

use gpui::App;

use super::History;
use crate::browser::kept::{self, Kept};

pub(crate) use crate::browser::kept::now;

impl Kept for History {
    const FILE_NAME: &'static str = "history.json";
}

pub(crate) fn start(cx: &mut App) {
    kept::start::<History>(cx);
}

pub(crate) fn history_snapshot(cx: &App) -> Arc<History> {
    kept::snapshot::<History>(cx)
}

pub(crate) fn loaded(cx: &App) -> bool {
    kept::loaded::<History>(cx)
}

pub(crate) fn record_visit(url: &str, title: Option<&str>, typed: bool, cx: &mut App) {
    let now = now();
    let (url, title) = (url.to_string(), title.map(str::to_string));
    kept::change::<History>(cx, move |history| {
        history.record(&url, title.as_deref(), typed, now);
        true
    });
}

pub(crate) fn retitle(url: &str, title: &str, cx: &mut App) {
    let (url, title) = (url.to_string(), title.to_string());
    kept::change::<History>(cx, move |history| history.retitle(&url, &title));
}

pub(crate) fn forget(url: &str, cx: &mut App) {
    let url = url.to_string();
    kept::change::<History>(cx, move |history| history.remove(&url));
}

pub(crate) fn clear_since(since: u64, cx: &mut App) {
    kept::change::<History>(cx, move |history| history.clear_since(since));
}
