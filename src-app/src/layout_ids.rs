use std::cell::{Cell, RefCell};
use std::sync::LazyLock;

use paneflow_layout::IdSource;

use crate::terminal::host_link::LaunchRoute;

static IDS: LazyLock<IdSource> = LazyLock::new(IdSource::default);

thread_local! {
    static REQUESTED_SURFACE: Cell<Option<u64>> = const { Cell::new(None) };
    static LAUNCH_ROUTE: RefCell<Option<LaunchRoute>> = const { RefCell::new(None) };
}

pub(crate) fn source() -> IdSource {
    IDS.clone()
}

pub(crate) fn allocate() -> u64 {
    IDS.allocate()
}

pub(crate) fn claim(id: u64) {
    IDS.claim(id);
}

pub(crate) fn assigning_surface<R>(id: u64, build: impl FnOnce() -> R) -> R {
    claim(id);
    let previous = REQUESTED_SURFACE.with(|requested| requested.replace(Some(id)));
    let built = build();
    REQUESTED_SURFACE.with(|requested| requested.set(previous));
    built
}

pub(crate) fn launching_through<R>(route: Option<LaunchRoute>, build: impl FnOnce() -> R) -> R {
    let previous = LAUNCH_ROUTE.with(|current| current.replace(route));
    let built = build();
    LAUNCH_ROUTE.with(|current| current.replace(previous));
    built
}

pub(crate) fn launch_route_for_new_terminal() -> Option<LaunchRoute> {
    LAUNCH_ROUTE.with(|current| current.borrow_mut().take())
}

pub(crate) fn surface_id_for_new_terminal() -> u64 {
    REQUESTED_SURFACE.with(Cell::take).unwrap_or_else(allocate)
}
