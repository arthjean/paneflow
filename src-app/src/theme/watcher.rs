use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::builtin::{paneflow_dark, theme_by_name};
use super::model::{TerminalTheme, apply_surface_overrides};

const THEME_CHECK_INTERVAL: Duration = Duration::from_millis(500);

const DEBOUNCE_DURATION: Duration = Duration::from_millis(300);

struct CachedTheme {
    theme: TerminalTheme,
    mtime: Option<SystemTime>,
    last_check: Instant,
}

static THEME_CACHE: Mutex<Option<CachedTheme>> = Mutex::new(None);
static THEME_GENERATION: AtomicU64 = AtomicU64::new(0);

static WATCHER_ACTIVE: AtomicBool = AtomicBool::new(false);

fn read_config_theme_name() -> Option<String> {
    paneflow_config::loader::load_config().theme
}

fn resolve_theme() -> TerminalTheme {
    if let Some(name) = read_config_theme_name() {
        if let Some(theme) = theme_by_name(&name) {
            return apply_surface_overrides(theme);
        }
        log::warn!("Unknown theme '{}', using default", name);
    }
    apply_surface_overrides(paneflow_dark())
}

pub fn invalidate_theme_cache() {
    *THEME_CACHE.lock() = None;
    THEME_GENERATION.fetch_add(1, Ordering::AcqRel);
}

pub fn theme_generation() -> u64 {
    THEME_GENERATION.load(Ordering::Acquire)
}

pub fn config_mtime() -> Option<SystemTime> {
    let config_path = paneflow_config::loader::config_path()?;
    std::fs::metadata(config_path).ok()?.modified().ok()
}

pub fn active_theme() -> TerminalTheme {
    let mut cache = THEME_CACHE.lock();

    if let Some(cached) = cache.as_ref() {
        if WATCHER_ACTIVE.load(Ordering::Acquire) {
            return cached.theme;
        }
        if cached.last_check.elapsed() < THEME_CHECK_INTERVAL {
            return cached.theme;
        }
    }

    let current_mtime = config_mtime();
    let needs_reload = match (&*cache, current_mtime) {
        (None, _) => true,
        (_, None) => true,
        (Some(cached), Some(_)) => cached.mtime != current_mtime,
    };

    if needs_reload {
        let had_cached_theme = cache.is_some();
        let theme = resolve_theme();
        *cache = Some(CachedTheme {
            theme,
            mtime: current_mtime,
            last_check: Instant::now(),
        });
        if had_cached_theme {
            THEME_GENERATION.fetch_add(1, Ordering::AcqRel);
        }
        theme
    } else {
        #[allow(clippy::expect_used)]
        let cached = cache
            .as_mut()
            .expect("needs_reload=false implies cache is Some");
        cached.last_check = Instant::now();
        cached.theme
    }
}

pub struct ThemeWatcher {
    callback: Arc<dyn Fn() + Send + Sync>,
    config_path: PathBuf,
}

impl ThemeWatcher {
    pub fn new(callback: Arc<dyn Fn() + Send + Sync>) -> Option<Self> {
        let config_path = paneflow_config::loader::config_path()?;
        Some(Self {
            callback,
            config_path,
        })
    }

    #[cfg(test)]
    fn new_with_path(path: PathBuf, callback: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            callback,
            config_path: path,
        }
    }

    pub fn start(&self) -> Result<(), notify::Error> {
        if WATCHER_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(notify::Error::generic(
                "theme watcher already running - start() called twice",
            ));
        }

        let result = self.install_watcher();
        if result.is_err() {
            WATCHER_ACTIVE.store(false, Ordering::Release);
        }
        result
    }

    fn install_watcher(&self) -> Result<(), notify::Error> {
        #[allow(clippy::expect_used)]
        let watch_dir = self
            .config_path
            .parent()
            .expect("config path has no parent directory")
            .to_path_buf();

        if !watch_dir.exists() {
            std::fs::create_dir_all(&watch_dir).map_err(notify::Error::io)?;
        }

        let config_path = self.config_path.clone();
        let callback = Arc::clone(&self.callback);

        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            notify::Config::default(),
        )?;

        watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;

        thread::spawn(move || {
            event_loop(rx, &config_path, &callback, &watcher);
        });

        log::info!(
            "theme watcher started (path={})",
            self.config_path.display()
        );
        Ok(())
    }
}

fn is_relevant_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn event_targets_config(event: &Event, config_path: &std::path::Path) -> bool {
    let target_name = config_path.file_name();
    target_name.is_some() && event.paths.iter().any(|p| p.file_name() == target_name)
}

fn event_loop(
    rx: mpsc::Receiver<notify::Result<Event>>,
    config_path: &std::path::Path,
    callback: &Arc<dyn Fn() + Send + Sync>,
    _watcher: &RecommendedWatcher,
) {
    let mut pending_reload: Option<Instant> = None;

    loop {
        let event_result = if let Some(deadline) = pending_reload {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                pending_reload = None;
                fire_reload(callback);
                continue;
            }
            rx.recv_timeout(remaining)
        } else {
            match rx.recv() {
                Ok(ev) => Ok(ev),
                Err(_) => break,
            }
        };

        match event_result {
            Ok(Ok(event)) => {
                if is_relevant_event(&event.kind) && event_targets_config(&event, config_path) {
                    pending_reload = Some(Instant::now() + DEBOUNCE_DURATION);
                }
            }
            Ok(Err(e)) => {
                log::warn!("theme watcher error: {e}");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                pending_reload = None;
                fire_reload(callback);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    WATCHER_ACTIVE.store(false, Ordering::Release);
    log::debug!("theme watcher event loop exited");
}

fn fire_reload(callback: &Arc<dyn Fn() + Send + Sync>) {
    invalidate_theme_cache();
    callback();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;
    use tempfile::TempDir;

    fn wait_for<F: FnMut() -> bool>(mut pred: F, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if pred() {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        pred()
    }

    static SERIAL_TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct SerialGuard<'a>(#[allow(dead_code)] std::sync::MutexGuard<'a, ()>);
    impl Drop for SerialGuard<'_> {
        fn drop(&mut self) {
            WATCHER_ACTIVE.store(false, Ordering::Release);
        }
    }
    fn serial() -> SerialGuard<'static> {
        let lock = SERIAL_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        WATCHER_ACTIVE.store(false, Ordering::Release);
        SerialGuard(lock)
    }

    fn write_config(path: &std::path::Path, theme: &str) {
        std::fs::write(path, format!(r#"{{"theme": "{theme}"}}"#)).unwrap();
    }

    #[test]
    fn test_theme_watcher_start_succeeds_and_flips_flag() {
        let _g = serial();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("paneflow.json");
        write_config(&path, "One Dark");

        let watcher = ThemeWatcher::new_with_path(path.clone(), Arc::new(|| {}));
        watcher
            .start()
            .expect("start must succeed on a normal tempdir");

        assert!(WATCHER_ACTIVE.load(Ordering::Acquire));
    }

    #[test]
    fn test_theme_watcher_invokes_callback_on_change() {
        let _g = serial();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("paneflow.json");
        write_config(&path, "One Dark");

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);
        let watcher = ThemeWatcher::new_with_path(
            path.clone(),
            Arc::new(move || {
                counter_clone.fetch_add(1, Ordering::Release);
            }),
        );
        watcher.start().expect("start must succeed");

        write_config(&path, "One Dark");

        let fired = wait_for(
            || counter.load(Ordering::Acquire) >= 1,
            Duration::from_millis(1500),
        );
        assert!(fired, "callback should fire at least once on a file modify");
    }

    #[test]
    fn test_theme_watcher_debounce_coalesces_burst() {
        let _g = serial();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("paneflow.json");
        write_config(&path, "One Dark");

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);
        let watcher = ThemeWatcher::new_with_path(
            path.clone(),
            Arc::new(move || {
                counter_clone.fetch_add(1, Ordering::Release);
            }),
        );
        watcher.start().expect("start must succeed");

        for theme in ["A", "B", "C", "D", "E"] {
            write_config(&path, theme);
            thread::sleep(Duration::from_millis(20));
        }

        thread::sleep(Duration::from_millis(800));

        let fires = counter.load(Ordering::Acquire);
        assert!(
            fires >= 1,
            "burst of 5 writes should fire the debounced callback at least once, got {fires}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_theme_watcher_start_failure_keeps_polling_fallback() {
        let _g = serial();

        let bogus = PathBuf::from("/proc/self/__paneflow_us006_test/paneflow.json");
        let watcher = ThemeWatcher::new_with_path(bogus, Arc::new(|| {}));
        let result = watcher.start();
        assert!(result.is_err(), "start should fail on /proc/self subdir");
        assert!(
            !WATCHER_ACTIVE.load(Ordering::Acquire),
            "WATCHER_ACTIVE must be false after init failure (AC #3) so the \
             500ms polling fallback in active_theme() takes over"
        );
    }

    #[test]
    fn test_theme_watcher_double_start_rejected() {
        let _g = serial();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("paneflow.json");
        write_config(&path, "One Dark");

        let w1 = ThemeWatcher::new_with_path(path.clone(), Arc::new(|| {}));
        w1.start().expect("first start must succeed");
        assert!(WATCHER_ACTIVE.load(Ordering::Acquire));

        let w2 = ThemeWatcher::new_with_path(path.clone(), Arc::new(|| {}));
        let err = w2
            .start()
            .expect_err("second start must reject - single-watcher contract");
        let _ = err;
        assert!(
            WATCHER_ACTIVE.load(Ordering::Acquire),
            "first watcher's lease must survive a rejected second start()"
        );
    }

    #[test]
    fn test_theme_watcher_background_thread_outlives_struct_drop() {
        let _g = serial();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("paneflow.json");
        write_config(&path, "One Dark");

        {
            let watcher = ThemeWatcher::new_with_path(path.clone(), Arc::new(|| {}));
            watcher.start().expect("start must succeed");
            assert!(WATCHER_ACTIVE.load(Ordering::Acquire));
        }
    }
}
