use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use gpui::{App, Bounds, Decorations, Pixels, Size, Window, px, size};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_width: Option<f32>,
}

impl PersistedWindowState {
    fn new(
        size: PersistedWindowSize,
        agents: AgentsSectionState,
        sidebar_width: Option<f32>,
    ) -> Self {
        Self {
            size,
            agents_section_height: agents
                .height
                .filter(|height| is_valid_section_height(*height)),
            agents_section_collapsed: agents.collapsed.then_some(true),
            sidebar_width: sidebar_width.filter(|width| is_valid_section_height(*width)),
        }
    }

    fn sidebar_width(&self) -> Option<f32> {
        self.sidebar_width
            .filter(|width| is_valid_section_height(*width))
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
static SIDEBAR_WIDTH: Mutex<Option<f32>> = Mutex::new(None);

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
    *sidebar_width_guard() = loaded.and_then(|state| state.sidebar_width());
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

pub(crate) fn minimum_size(compositor: &str, decorations: gpui::WindowDecorations) -> Size<Pixels> {
    size_hint_for_visible(
        size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT)),
        compositor,
        decorations,
    )
}

pub(crate) fn size_hint_for_visible(
    visible: Size<Pixels>,
    compositor: &str,
    decorations: gpui::WindowDecorations,
) -> Size<Pixels> {
    let size_hint_counts_the_client_inset = cfg!(any(target_os = "linux", target_os = "freebsd"))
        && compositor == "X11"
        && matches!(decorations, gpui::WindowDecorations::Client);
    if size_hint_counts_the_client_inset {
        let frame = crate::RESIZE_BORDER * 2.;
        size(visible.width + frame, visible.height + frame)
    } else {
        visible
    }
}

pub(crate) fn windowed_bounds(window: &Window) -> Option<Bounds<Pixels>> {
    if window.is_maximized() || window.is_fullscreen() {
        return None;
    }
    let inner = window.inner_window_bounds().get_bounds().size;
    let size = restorable_size(inner, window.window_decorations())?;
    Some(Bounds::new(
        window.window_bounds().get_bounds().origin,
        size,
    ))
}

fn restorable_size(inner: Size<Pixels>, decorations: Decorations) -> Option<Size<Pixels>> {
    if let Decorations::Client { tiling } = decorations
        && tiling.is_tiled()
    {
        return None;
    }
    let frame = crate::window_chrome::csd::client_frame_inset(decorations) * 2.0;
    Some(size(inner.width + frame, inner.height + frame))
}

pub(crate) fn record_windowed_size(window: &Window) {
    let Some(Bounds { size, .. }) = windowed_bounds(window) else {
        return;
    };
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

pub(crate) fn sidebar_width() -> Option<f32> {
    *sidebar_width_guard()
}

pub(crate) fn record_sidebar_width(width: Option<f32>) {
    *sidebar_width_guard() = width;
}

pub(crate) fn save() {
    if let Some(path) = state_path() {
        save_to(&path);
    }
}

fn save_to(path: &std::path::Path) {
    let Some(size) = *last_windowed_size_guard() else {
        return;
    };
    let state = PersistedWindowState::new(size, *agents_section_guard(), *sidebar_width_guard());
    if let Err(error) = write_state(path, &state) {
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

fn sidebar_width_guard() -> MutexGuard<'static, Option<f32>> {
    SIDEBAR_WIDTH.lock().unwrap_or_else(PoisonError::into_inner)
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

    #[test]
    fn the_restorable_size_adds_the_client_frame_whatever_inset_the_window_reports() {
        let floating = Decorations::Client {
            tiling: gpui::Tiling::default(),
        };
        let inset = f32::from(crate::window_chrome::csd::client_frame_inset(floating));
        assert!(inset > 0.0);
        let frame = gpui::size(px(1180.0), px(780.0));
        assert_eq!(
            restorable_size(frame, floating),
            Some(gpui::size(
                px(1180.0 + 2.0 * inset),
                px(780.0 + 2.0 * inset)
            ))
        );
        assert_eq!(restorable_size(frame, Decorations::Server), Some(frame));
    }

    #[test]
    fn a_tiled_window_records_no_restorable_size() {
        let tiled = Decorations::Client {
            tiling: gpui::Tiling {
                left: true,
                top: true,
                bottom: true,
                right: false,
            },
        };
        assert_eq!(
            restorable_size(gpui::size(px(720.0), px(900.0)), tiled),
            None
        );
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
            &PersistedWindowState::new(size(), AgentsSectionState::default(), None),
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

        write_state(&path, &PersistedWindowState::new(size(), chosen, None)).expect("write");

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
            &PersistedWindowState::new(size(), AgentsSectionState::default(), None),
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
    fn the_sidebar_width_round_trips_through_the_window_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("window-state.json");

        write_state(
            &path,
            &PersistedWindowState::new(size(), AgentsSectionState::default(), Some(372.)),
        )
        .expect("write");

        assert_eq!(read_json(&path)["sidebar_width"], 372.);
        assert_eq!(
            load_from(&path).expect("loaded").sidebar_width(),
            Some(372.)
        );
    }

    #[test]
    fn an_older_or_unusable_sidebar_width_loads_as_unset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("window-state.json");
        std::fs::write(&path, r#"{"width": 1400.0, "height": 950.0}"#).expect("seed");
        assert_eq!(load_from(&path).expect("loaded").sidebar_width(), None);

        std::fs::write(
            &path,
            r#"{"width": 1400.0, "height": 950.0, "sidebar_width": -3.0}"#,
        )
        .expect("seed");
        assert_eq!(load_from(&path).expect("loaded").sidebar_width(), None);
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

    struct BoundsProbe;

    impl gpui::Render for BoundsProbe {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::Empty
        }
    }

    #[gpui::test]
    fn a_fullscreen_round_trip_saves_the_windowed_size(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("window-state.json");
        let (_, cx) = cx.add_window_view(|window, cx| {
            cx.observe_window_bounds(window, |_, window, _| record_windowed_size(window))
                .detach();
            BoundsProbe
        });
        let windowed = gpui::size(px(1000.), px(700.));
        let screen = gpui::size(px(1920.), px(1080.));

        cx.simulate_resize(windowed);
        cx.run_until_parked();
        cx.update(|window, _| window.toggle_fullscreen());
        cx.run_until_parked();
        cx.simulate_resize(screen);
        cx.run_until_parked();
        cx.update(|window, _| {
            assert!(window.is_fullscreen());
            assert_eq!(windowed_bounds(window), None);
        });
        assert!(!path.exists(), "the bounds observer wrote the state file");
        let fullscreen_path = dir.path().join("fullscreen-window-state.json");
        save_to(&fullscreen_path);
        let saved = load_from(&fullscreen_path).expect("saved while fullscreen");
        assert_eq!((saved.size.width, saved.size.height), (1000., 700.));

        cx.update(|window, _| window.toggle_fullscreen());
        cx.run_until_parked();
        cx.simulate_resize(windowed);
        cx.run_until_parked();
        cx.update(|window, _| {
            assert_eq!(
                windowed_bounds(window).map(|bounds| bounds.size),
                Some(windowed)
            );
        });
        assert!(!path.exists(), "the bounds observer wrote the state file");

        save_to(&path);
        let saved = load_from(&path).expect("saved");
        assert_eq!((saved.size.width, saved.size.height), (1000., 700.));
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn an_x11_client_decorated_window_asks_for_its_inset_on_top_of_the_minimum() {
        let frame = crate::RESIZE_BORDER * 2.;
        assert_eq!(
            minimum_size("X11", gpui::WindowDecorations::Client),
            gpui::size(px(MIN_WINDOW_WIDTH) + frame, px(MIN_WINDOW_HEIGHT) + frame)
        );
        assert_eq!(
            minimum_size("X11", gpui::WindowDecorations::Server),
            gpui::size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))
        );
    }

    #[test]
    fn a_compositor_that_sizes_the_visible_window_gets_the_bare_minimum() {
        for decorations in [
            gpui::WindowDecorations::Client,
            gpui::WindowDecorations::Server,
        ] {
            assert_eq!(
                minimum_size("Wayland", decorations),
                gpui::size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))
            );
        }
    }
}
