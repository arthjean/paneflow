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
