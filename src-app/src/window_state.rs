use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use gpui::{App, Bounds, Pixels, Size, Window, px, size};
use serde::{Deserialize, Serialize};

const DEFAULT_WINDOW_WIDTH: f32 = 1200.;
const DEFAULT_WINDOW_HEIGHT: f32 = 800.;
const MIN_WINDOW_WIDTH: f32 = 800.;
const MIN_WINDOW_HEIGHT: f32 = 500.;
const FALLBACK_MAX_WINDOW_WIDTH: f32 = 3840.;
const FALLBACK_MAX_WINDOW_HEIGHT: f32 = 2160.;
const MAX_WINDOW_STATE_BYTES: u64 = 4096;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct PersistedWindowSize {
    width: f32,
    height: f32,
}

static LAST_WINDOWED_SIZE: Mutex<Option<PersistedWindowSize>> = Mutex::new(None);

pub(crate) fn initial_bounds(cx: &App) -> Bounds<Pixels> {
    let persisted = load().unwrap_or(PersistedWindowSize {
        width: DEFAULT_WINDOW_WIDTH,
        height: DEFAULT_WINDOW_HEIGHT,
    });
    *last_windowed_size_guard() = Some(persisted);
    let visible_bounds = cx.primary_display().map(|display| display.visible_bounds());
    let display_size = visible_bounds.map(|bounds| bounds.size).unwrap_or_else(|| {
        size(
            px(FALLBACK_MAX_WINDOW_WIDTH),
            px(FALLBACK_MAX_WINDOW_HEIGHT),
        )
    });
    let max_width = f32::from(display_size.width).max(MIN_WINDOW_WIDTH);
    let max_height = f32::from(display_size.height).max(MIN_WINDOW_HEIGHT);
    let restored_size = size(
        px(persisted.width.clamp(MIN_WINDOW_WIDTH, max_width)),
        px(persisted.height.clamp(MIN_WINDOW_HEIGHT, max_height)),
    );

    visible_bounds
        .map(|bounds| Bounds::centered_at(bounds.center(), restored_size))
        .unwrap_or_else(|| Bounds::centered(None, restored_size, cx))
}

pub(crate) fn minimum_size() -> Size<Pixels> {
    size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))
}

pub(crate) fn record_windowed_size(window: &Window) {
    if window.is_maximized() || window.is_fullscreen() {
        return;
    }
    let size = window.window_bounds().get_bounds().size;
    let state = PersistedWindowSize {
        width: size.width.into(),
        height: size.height.into(),
    };
    if !is_valid_size(state) {
        log::warn!(
            "window state: refusing to record invalid size {}x{}",
            state.width,
            state.height
        );
        return;
    }
    *last_windowed_size_guard() = Some(state);
}

pub(crate) fn save() {
    let Some(state) = *last_windowed_size_guard() else {
        return;
    };

    let Some(path) = state_path() else {
        return;
    };
    if let Err(error) = write_state(&path, &state) {
        log::warn!("window state: failed to persist: {error}");
    }
}

fn write_state(path: &std::path::Path, state: &PersistedWindowSize) -> std::io::Result<()> {
    let mut json = serde_json::to_vec_pretty(state)?;
    json.push(b'\n');
    paneflow_home::write_atomically(path, &json)
}

fn load() -> Option<PersistedWindowSize> {
    load_from(&state_path()?)
}

fn load_from(path: &std::path::Path) -> Option<PersistedWindowSize> {
    let contents = match paneflow_home::read_regular_string_capped(path, MAX_WINDOW_STATE_BYTES) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            log::warn!("window state: rejected {}: {error}", path.display());
            return None;
        }
    };
    match serde_json::from_str::<PersistedWindowSize>(&contents) {
        Ok(state) if state.width.is_finite() && state.height.is_finite() => Some(state),
        Ok(_) => {
            log::warn!(
                "window state: rejected non-finite size at {}",
                path.display()
            );
            None
        }
        Err(error) => {
            log::warn!("window state: invalid JSON at {}: {error}", path.display());
            None
        }
    }
}

fn state_path() -> Option<PathBuf> {
    paneflow_home::window_state_path()
}

fn is_valid_size(state: PersistedWindowSize) -> bool {
    state.width.is_finite()
        && state.height.is_finite()
        && state.width >= MIN_WINDOW_WIDTH
        && state.height >= MIN_WINDOW_HEIGHT
}

fn last_windowed_size_guard() -> MutexGuard<'static, Option<PersistedWindowSize>> {
    LAST_WINDOWED_SIZE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mkfifo(path: &std::path::Path) {
        let made = std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .is_ok_and(|status| status.success());
        assert!(made, "mkfifo {}", path.display());
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_window_state_is_refused_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("window-state.json");
        mkfifo(&path);
        let started = std::time::Instant::now();
        assert_eq!(load_from(&path), None);
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
    }

    fn link_to(target: &std::path::Path, link: &std::path::Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).expect("symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, link).expect("symlink");
    }

    fn is_link(path: &std::path::Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
    }

    #[test]
    fn the_window_state_is_written_through_a_symlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("real.json");
        std::fs::write(&target, "{}").expect("seed");
        let link = dir.path().join("window-state.json");
        link_to(&target, &link);

        write_state(
            &link,
            &PersistedWindowSize {
                width: 1300.,
                height: 900.,
            },
        )
        .expect("write");

        assert!(is_link(&link));
        let written: PersistedWindowSize =
            serde_json::from_str(&std::fs::read_to_string(&target).expect("read")).expect("json");
        assert_eq!((written.width, written.height), (1300., 900.));
    }
}
