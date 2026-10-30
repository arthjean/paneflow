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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct AgentsSectionState {
    pub(crate) height: Option<f32>,
    pub(crate) collapsed: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct PersistedWindowState {
    #[serde(flatten)]
    size: PersistedWindowSize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agents_section_height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agents_section_collapsed: Option<bool>,
}

impl PersistedWindowState {
    fn new(size: PersistedWindowSize, agents: AgentsSectionState) -> Self {
        Self {
            size,
            agents_section_height: agents
                .height
                .filter(|height| is_valid_section_height(*height)),
            agents_section_collapsed: agents.collapsed.then_some(true),
        }
    }

    fn agents_section(&self) -> AgentsSectionState {
        AgentsSectionState {
            height: self
                .agents_section_height
                .filter(|height| is_valid_section_height(*height)),
            collapsed: self.agents_section_collapsed.unwrap_or(false),
        }
    }
}

static LAST_WINDOWED_SIZE: Mutex<Option<PersistedWindowSize>> = Mutex::new(None);
static AGENTS_SECTION: Mutex<AgentsSectionState> = Mutex::new(AgentsSectionState {
    height: None,
    collapsed: false,
});

pub(crate) fn initial_bounds(cx: &App) -> Bounds<Pixels> {
    let loaded = load();
    let persisted = loaded.map_or(
        PersistedWindowSize {
            width: DEFAULT_WINDOW_WIDTH,
            height: DEFAULT_WINDOW_HEIGHT,
        },
        |state| state.size,
    );
    *last_windowed_size_guard() = Some(persisted);
    *agents_section_guard() = loaded
        .map(|state| state.agents_section())
        .unwrap_or_default();
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

pub(crate) fn agents_section() -> AgentsSectionState {
    *agents_section_guard()
}

pub(crate) fn record_agents_section(state: AgentsSectionState) {
    *agents_section_guard() = state;
}

pub(crate) fn save() {
    let Some(size) = *last_windowed_size_guard() else {
        return;
    };
    let state = PersistedWindowState::new(size, *agents_section_guard());

    let Some(path) = state_path() else {
        return;
    };
    if let Err(error) = write_state(&path, &state) {
        log::warn!("window state: failed to persist: {error}");
    }
}

fn write_state(path: &std::path::Path, state: &PersistedWindowState) -> std::io::Result<()> {
    let mut json = serde_json::to_vec_pretty(state)?;
    json.push(b'\n');
    paneflow_home::write_atomically(path, &json)
}

fn load() -> Option<PersistedWindowState> {
    load_from(&state_path()?)
}

fn load_from(path: &std::path::Path) -> Option<PersistedWindowState> {
    let contents = match paneflow_home::read_regular_string_capped(path, MAX_WINDOW_STATE_BYTES) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            log::warn!("window state: rejected {}: {error}", path.display());
            return None;
        }
    };
    match serde_json::from_str::<PersistedWindowState>(&contents) {
        Ok(state) if state.size.width.is_finite() && state.size.height.is_finite() => Some(state),
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

fn is_valid_section_height(height: f32) -> bool {
    height.is_finite() && height > 0.
}

fn last_windowed_size_guard() -> MutexGuard<'static, Option<PersistedWindowSize>> {
    LAST_WINDOWED_SIZE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn agents_section_guard() -> MutexGuard<'static, AgentsSectionState> {
    AGENTS_SECTION
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
        assert!(load_from(&path).is_none());
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
            &PersistedWindowState::new(size(), AgentsSectionState::default()),
        )
        .expect("write");

        assert!(is_link(&link));
        let written: PersistedWindowSize =
            serde_json::from_str(&std::fs::read_to_string(&target).expect("read")).expect("json");
        assert_eq!((written.width, written.height), (1300., 900.));
    }

    fn size() -> PersistedWindowSize {
        PersistedWindowSize {
            width: 1300.,
            height: 900.,
        }
    }

    fn read_json(path: &std::path::Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json")
    }

    #[test]
    fn the_agents_section_height_and_fold_round_trip_through_the_window_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("window-state.json");
        let chosen = AgentsSectionState {
            height: Some(240.),
            collapsed: true,
        };

        write_state(&path, &PersistedWindowState::new(size(), chosen)).expect("write");

        let raw = read_json(&path);
        assert_eq!(raw["agents_section_height"], 240.);
        assert_eq!(raw["agents_section_collapsed"], true);
        let loaded = load_from(&path).expect("loaded");
        assert_eq!(loaded.agents_section(), chosen);
        assert_eq!((loaded.size.width, loaded.size.height), (1300., 900.));
    }

    #[test]
    fn the_default_agents_section_writes_neither_optional_field() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("window-state.json");

        write_state(
            &path,
            &PersistedWindowState::new(size(), AgentsSectionState::default()),
        )
        .expect("write");

        let raw = read_json(&path);
        assert_eq!(raw.as_object().map(|object| object.len()), Some(2), "{raw}");
    }

    #[test]
    fn an_older_window_state_loads_with_the_agents_section_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("window-state.json");
        std::fs::write(&path, r#"{"width": 1400.0, "height": 950.0}"#).expect("seed");

        let loaded = load_from(&path).expect("loaded");

        assert_eq!((loaded.size.width, loaded.size.height), (1400., 950.));
        assert_eq!(loaded.agents_section(), AgentsSectionState::default());
    }

    #[test]
    fn an_unusable_section_height_falls_back_to_the_content_height() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("window-state.json");
        std::fs::write(
            &path,
            r#"{"width": 1400.0, "height": 950.0, "agents_section_height": -3.0}"#,
        )
        .expect("seed");

        assert_eq!(
            load_from(&path).expect("loaded").agents_section(),
            AgentsSectionState::default()
        );
    }

    #[test]
    fn a_corrupt_or_oversized_window_state_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let corrupt = dir.path().join("corrupt.json");
        std::fs::write(&corrupt, r#"{"width": 1400.0, "agents_section_height": "#).expect("seed");
        assert!(load_from(&corrupt).is_none());

        let oversized = dir.path().join("oversized.json");
        let padding = " ".repeat(MAX_WINDOW_STATE_BYTES as usize);
        std::fs::write(
            &oversized,
            format!(
                r#"{{"width": 1400.0, "height": 950.0, "agents_section_height": 200.0{padding}}}"#
            ),
        )
        .expect("seed");
        assert!(load_from(&oversized).is_none());
    }
}
