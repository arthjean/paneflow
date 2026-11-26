use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext, Global};
use serde::Serialize;
use serde::de::DeserializeOwned;

const SAVE_DELAY: Duration = Duration::from_secs(2);

pub(crate) trait Kept:
    Clone + Default + Serialize + DeserializeOwned + Send + Sync + 'static
{
    const FILE_NAME: &'static str;
}

type Edit<T> = Box<dyn Fn(&mut T) -> bool>;

pub(crate) struct Store<T> {
    value: Arc<T>,
    loaded: bool,
    started: bool,
    dirty: bool,
    saving: bool,
    before_load: Vec<Edit<T>>,
}

impl<T: Kept> Default for Store<T> {
    fn default() -> Self {
        Self {
            value: Arc::default(),
            loaded: false,
            started: false,
            dirty: false,
            saving: false,
            before_load: Vec::new(),
        }
    }
}

impl<T: Kept> Global for Store<T> {}

#[cfg(not(test))]
fn path<T: Kept>() -> Option<PathBuf> {
    paneflow_home::paneflow_home()
        .map(|home| paneflow_home::browser_dir_in(&home).join(T::FILE_NAME))
}

#[cfg(test)]
fn path<T: Kept>() -> Option<PathBuf> {
    None
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn read<T: Kept>(path: &Path) -> T {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return T::default();
    };
    serde_json::from_str(&raw).unwrap_or_else(|error| {
        log::warn!(
            "browser: {} is not valid ({error}), starting empty",
            path.display()
        );
        T::default()
    })
}

fn write<T: Kept>(path: &Path, value: &T) {
    let Ok(json) = serde_json::to_vec(value) else {
        return;
    };
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        log::warn!("browser: could not create {}: {error}", parent.display());
        return;
    }
    let staging = path.with_extension("json.tmp");
    let written = std::fs::write(&staging, json).and_then(|()| std::fs::rename(&staging, path));
    if let Err(error) = written {
        log::warn!("browser: could not write {}: {error}", path.display());
    }
}

pub(crate) fn start<T: Kept>(cx: &mut App) {
    if cx
        .try_global::<Store<T>>()
        .is_some_and(|store| store.started)
    {
        return;
    }
    let store = cx.default_global::<Store<T>>();
    store.started = true;
    let Some(path) = path::<T>() else {
        store.loaded = true;
        return;
    };
    let read_task = cx.background_spawn(async move { read::<T>(&path) });
    cx.spawn(async move |cx| {
        let stored = read_task.await;
        cx.update(|cx| {
            let store = cx.default_global::<Store<T>>();
            let edits = std::mem::take(&mut store.before_load);
            store.value = Arc::new(settle(stored, &edits));
            store.loaded = true;
        })
    })
    .detach();
}

fn settle<T: Kept>(stored: T, edits: &[Edit<T>]) -> T {
    let mut value = stored;
    for edit in edits {
        edit(&mut value);
    }
    value
}

pub(crate) fn snapshot<T: Kept>(cx: &App) -> Arc<T> {
    cx.try_global::<Store<T>>()
        .map(|store| Arc::clone(&store.value))
        .unwrap_or_default()
}

pub(crate) fn loaded<T: Kept>(cx: &App) -> bool {
    cx.try_global::<Store<T>>()
        .is_some_and(|store| store.loaded)
}

pub(crate) fn change<T: Kept>(cx: &mut App, edit: impl Fn(&mut T) -> bool + 'static) {
    start::<T>(cx);
    let store = cx.default_global::<Store<T>>();
    if !edit(Arc::make_mut(&mut store.value)) {
        return;
    }
    if !store.loaded {
        store.before_load.push(Box::new(edit));
    }
    store.dirty = true;
    if store.saving {
        return;
    }
    store.saving = true;
    cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        let Some((path, value)) = cx.update(|cx| {
            let store = cx.default_global::<Store<T>>();
            store.saving = false;
            if !store.loaded || !store.dirty {
                return None;
            }
            store.dirty = false;
            path::<T>().map(|path| (path, Arc::clone(&store.value)))
        }) else {
            return;
        };
        cx.background_spawn(async move { write(&path, &*value) })
            .await;
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::omnibox::History;

    #[test]
    fn a_kept_value_written_to_disk_reads_back_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("browser").join(History::FILE_NAME);
        let mut history = History::default();
        history.record("https://vercel.com/", Some("Vercel"), true, 1_800_000_000);
        write(&path, &history);
        assert_eq!(read::<History>(&path), history);
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(read::<History>(&path), History::default());
    }

    #[test]
    fn edits_made_before_the_file_loaded_are_replayed_on_it_in_order() {
        let mut stored = History::default();
        stored.record("https://kept.example/", None, true, 1_800_000_000);
        let edits: Vec<Edit<History>> = vec![
            Box::new(|history: &mut History| history.clear_since(0)),
            Box::new(|history: &mut History| {
                history.record("https://after.example/", None, false, 1_800_000_100);
                true
            }),
        ];
        let settled = settle(stored, &edits);
        assert_eq!(
            settled.len(),
            1,
            "a clear made early is not undone by the file"
        );
        assert_eq!(settled.kept_since(1_800_000_050), 1);
    }
}
