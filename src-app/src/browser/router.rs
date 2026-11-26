use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use gpui::{App, Global, Task, WeakEntity};
use paneflow_browser_control::SurfaceEvent;
use paneflow_browser_proto::TabId;

use super::view::BrowserView;

enum Signal {
    Frame(TabId),
    Surface(SurfaceEvent),
}

struct Router {
    views: HashMap<TabId, WeakEntity<BrowserView>>,
    pending: Arc<Mutex<HashSet<TabId>>>,
    changed: Arc<Mutex<HashSet<TabId>>>,
    _pump: Task<()>,
}

impl Global for Router {}

pub(crate) fn install(cx: &mut App) -> Result<(), String> {
    if cx.has_global::<Router>() {
        return Ok(());
    }
    let runtime = crate::browser_control::runtime()?;
    let (sender, receiver) = smol::channel::unbounded::<Signal>();
    let pending: Arc<Mutex<HashSet<TabId>>> = Arc::default();
    let frames = sender.clone();
    let waiting = Arc::clone(&pending);
    runtime.client.set_frame_waker(Some(Arc::new(move |tab| {
        let first = waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(tab);
        if first {
            let _ = frames.try_send(Signal::Frame(tab));
        }
    })));
    let events = runtime.control.subscribe();
    let changed: Arc<Mutex<HashSet<TabId>>> = Arc::default();
    let unsent = Arc::clone(&changed);
    std::thread::Builder::new()
        .name("browser-surface-events".into())
        .spawn(move || {
            for event in events {
                if let SurfaceEvent::Changed(tab) = event
                    && !first_since_delivery(&unsent, tab)
                {
                    continue;
                }
                if sender.try_send(Signal::Surface(event)).is_err() {
                    return;
                }
            }
        })
        .map_err(|error| format!("cannot follow browser events: {error}"))?;
    let pump = cx.spawn(async move |cx| {
        while let Ok(signal) = receiver.recv().await {
            cx.update(|cx| deliver(signal, cx));
        }
    });
    cx.set_global(Router {
        views: HashMap::new(),
        pending,
        changed,
        _pump: pump,
    });
    Ok(())
}

pub(crate) fn register(tab: TabId, view: WeakEntity<BrowserView>, cx: &mut App) {
    if cx.has_global::<Router>() {
        cx.global_mut::<Router>().views.insert(tab, view);
    }
}

pub(crate) fn unregister(tab: TabId, view: &WeakEntity<BrowserView>, cx: &mut App) {
    if cx.has_global::<Router>() {
        let router = cx.global_mut::<Router>();
        if router.views.get(&tab).is_some_and(|held| held == view) {
            router.views.remove(&tab);
        }
    }
}

pub(crate) fn view_for(tab: TabId, cx: &App) -> Option<gpui::Entity<BrowserView>> {
    cx.try_global::<Router>()?.views.get(&tab)?.upgrade()
}

fn first_since_delivery(waiting: &Mutex<HashSet<TabId>>, tab: TabId) -> bool {
    waiting
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(tab)
}

fn delivered(waiting: &Mutex<HashSet<TabId>>, tab: TabId) {
    waiting
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&tab);
}

fn deliver(signal: Signal, cx: &mut App) {
    if let Signal::Surface(SurfaceEvent::Changed(tab)) = &signal
        && let Some(router) = cx.try_global::<Router>()
    {
        delivered(&router.changed, *tab);
    }
    match signal {
        Signal::Frame(tab) => {
            if let Some(router) = cx.try_global::<Router>() {
                router
                    .pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&tab);
            }
            if let Some(view) = view_for(tab, cx) {
                view.update(cx, |view, cx| view.frame_ready(cx));
            }
        }
        Signal::Surface(SurfaceEvent::Host) => {
            let views: Vec<_> = cx
                .try_global::<Router>()
                .map(|router| {
                    router
                        .views
                        .values()
                        .filter_map(WeakEntity::upgrade)
                        .collect()
                })
                .unwrap_or_default();
            for view in views {
                view.update(cx, |view, cx| view.refresh(cx));
            }
        }
        Signal::Surface(event) => {
            let tab = match &event {
                SurfaceEvent::Changed(tab)
                | SurfaceEvent::Closed(tab)
                | SurfaceEvent::Cursor { tab, .. }
                | SurfaceEvent::ImeCaret { tab, .. }
                | SurfaceEvent::Dialog { tab, .. }
                | SurfaceEvent::FileChooser { tab, .. }
                | SurfaceEvent::Annotation { tab, .. } => *tab,
                SurfaceEvent::Opened { opener, .. } => *opener,
                SurfaceEvent::Host => return,
            };
            if let Some(view) = view_for(tab, cx) {
                view.update(cx, |view, cx| view.surface_event(event, cx));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_of_changes_for_one_tab_reaches_the_window_once_until_it_is_delivered() {
        let waiting = Mutex::new(HashSet::new());
        assert!(first_since_delivery(&waiting, TabId(1)));
        assert!(!first_since_delivery(&waiting, TabId(1)));
        assert!(first_since_delivery(&waiting, TabId(2)));
        delivered(&waiting, TabId(1));
        assert!(first_since_delivery(&waiting, TabId(1)));
    }
}
