// US-018: Hot-reload via file watcher

use crate::loader::{
    config_path, load_config_from_path, read_config_string, try_parse_and_validate,
};
use crate::schema::PaneFlowConfig;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Debounce window: accumulate file events for this duration before reloading.
const DEBOUNCE_DURATION: Duration = Duration::from_millis(300);

/// US-029: hard ceiling on how long the debounce may keep postponing a reload.
/// Each event pushes the 300ms deadline forward; a source touching the watched
/// directory faster than 300ms (FSEvents batches on macOS, multi-event saves on
/// Windows) would otherwise starve the reload indefinitely. Once events have
/// been arriving for this long, the reload fires regardless (leading+trailing
/// debounce with a max-wait).
const MAX_DEBOUNCE: Duration = Duration::from_secs(1);

/// Watches the PaneFlow config file for changes and triggers hot-reload.
///
/// The watcher monitors the parent directory (not the file directly) so that
/// editor save patterns involving delete+recreate (atomic saves) are captured.
/// File events are debounced at 300ms to coalesce rapid sequences of writes.
pub struct ConfigWatcher {
    callback: Arc<dyn Fn(PaneFlowConfig) + Send + Sync>,
    config_path: PathBuf,
}

enum WatcherMessage {
    Event(notify::Result<Event>),
    Stop,
}

/// Active config-watch resource. Dropping it stops the event loop, releases
/// the OS watcher, and joins the background thread.
pub struct RunningConfigWatcher {
    stop_tx: mpsc::Sender<WatcherMessage>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for RunningConfigWatcher {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(WatcherMessage::Stop);
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                warn!("config watcher thread panicked while stopping");
            }
        }
    }
}

impl ConfigWatcher {
    /// Creates a new `ConfigWatcher` that will invoke `callback` with the new
    /// configuration whenever the config file is successfully reloaded.
    ///
    /// Uses `config_path()` to determine which file to watch. Returns `None`
    /// when the platform config directory cannot be resolved, letting the app
    /// keep running with cold-loaded defaults and hot reload disabled.
    pub fn new(callback: Arc<dyn Fn(PaneFlowConfig) + Send + Sync>) -> Option<Self> {
        let Some(config_path) = config_path() else {
            warn!("could not determine config path; config hot-reload disabled");
            return None;
        };
        Some(Self {
            callback,
            config_path,
        })
    }

    /// Creates a `ConfigWatcher` targeting a specific path - useful for testing.
    #[cfg(test)]
    fn new_with_path(path: PathBuf, callback: Arc<dyn Fn(PaneFlowConfig) + Send + Sync>) -> Self {
        Self {
            callback,
            config_path: path,
        }
    }

    /// Starts watching the config file's parent directory for changes.
    ///
    /// Spawns a background thread that:
    /// 1. Receives raw file-system events from `notify::RecommendedWatcher`
    /// 2. Debounces them over a 300ms window
    /// 3. Reloads and validates the config file
    /// 4. Calls the callback on success, or logs a warning on failure
    ///
    /// Returns an owned running handle once the watcher is installed. Dropping
    /// that handle stops the background thread and releases the OS watcher.
    pub fn start(self) -> Result<RunningConfigWatcher, notify::Error> {
        // Invariant: `self.config_path` is always a file path built from
        // `config_path()` (e.g., `/home/u/.config/paneflow/paneflow.json`),
        // so `.parent()` is guaranteed to be `Some`. `expect` is correct
        // here - documented invariant per CLAUDE.md.
        #[allow(clippy::expect_used)]
        let watch_dir = self
            .config_path
            .parent()
            .expect("config path has no parent directory")
            .to_path_buf();

        // notify can't watch a directory that doesn't exist yet - create it
        // on first run so hot-reload works even before the user writes a config.
        if !watch_dir.exists() {
            std::fs::create_dir_all(&watch_dir).map_err(notify::Error::io)?;
        }

        let config_path = self.config_path.clone();
        let callback = Arc::clone(&self.callback);

        // Channel for notify -> processing thread.
        let (tx, rx) = mpsc::channel::<WatcherMessage>();
        let stop_tx = tx.clone();

        // Create the OS file watcher. It sends events through `tx`.
        let mut watcher = RecommendedWatcher::new(
            move |res| {
                // Best-effort send; if the receiver is gone the watcher is being dropped.
                let _ = tx.send(WatcherMessage::Event(res));
            },
            notify::Config::default(),
        )?;

        // Watch the parent directory (non-recursive) to catch delete+recreate.
        watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;

        // Spawn the event-processing loop in a background thread.
        // The thread owns `watcher` to keep it alive.
        let thread = thread::spawn(move || {
            event_loop(rx, &config_path, &callback, &watcher);
        });

        info!(
            path = %self.config_path.display(),
            "config watcher started"
        );

        Ok(RunningConfigWatcher {
            stop_tx,
            thread: Some(thread),
        })
    }
}

/// Returns `true` if this event kind is relevant for config reload.
fn is_relevant_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Returns `true` if any path in the event matches the config file.
///
/// Matches by file name rather than full path: platforms rewrite watched
/// paths before emitting events (macOS FSEvents canonicalizes
/// `/var/folders/...` to `/private/var/folders/...`, Windows sometimes uses
/// UNC `\\?\C:\...` prefixes) so a full-path comparison is inherently
/// fragile. Because the watcher is installed `NonRecursive` on the parent
/// directory, every event we receive already belongs to that directory -
/// basename equality is sufficient and portable.
fn event_targets_config(event: &Event, config_path: &Path) -> bool {
    let target_name = config_path.file_name();
    target_name.is_some() && event.paths.iter().any(|p| p.file_name() == target_name)
}

/// The main event-processing loop running on the background thread.
///
/// `_watcher` is kept alive by moving it into this scope - dropping it would
/// stop the OS-level file watch.
fn event_loop(
    rx: mpsc::Receiver<WatcherMessage>,
    config_path: &Path,
    callback: &Arc<dyn Fn(PaneFlowConfig) + Send + Sync>,
    _watcher: &RecommendedWatcher,
) {
    // The last config that was successfully loaded (starts as the current one).
    let mut current_config = load_config_from_path(config_path);
    let mut pending_reload: Option<Instant> = None;
    // US-029: timestamp of the first event in the current debounce burst, used
    // to cap the trailing debounce so a continuous event stream can't starve
    // the reload forever.
    let mut first_event_at: Option<Instant> = None;

    loop {
        // If we have a pending reload, wait only until the debounce window expires.
        // Otherwise block indefinitely for the next event.
        let event_result = if let Some(deadline) = pending_reload {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                // Debounce window expired - do the reload.
                pending_reload = None;
                first_event_at = None;
                attempt_reload(config_path, &mut current_config, callback);
                continue;
            }
            rx.recv_timeout(remaining)
        } else {
            // No pending reload - block for the next event.
            match rx.recv() {
                Ok(ev) => Ok(ev),
                Err(_) => break, // Channel closed - watcher was dropped.
            }
        };

        match event_result {
            Ok(WatcherMessage::Event(Ok(event))) => {
                if is_relevant_event(&event.kind) && event_targets_config(&event, config_path) {
                    let now = Instant::now();
                    let burst_start = *first_event_at.get_or_insert(now);
                    // Trailing debounce, but never pushed past the max-wait cap
                    // measured from the first event of the burst.
                    let deadline = (now + DEBOUNCE_DURATION).min(burst_start + MAX_DEBOUNCE);
                    pending_reload = Some(deadline);
                }
            }
            Ok(WatcherMessage::Event(Err(e))) => {
                warn!("file watcher error: {e}");
            }
            Ok(WatcherMessage::Stop) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Debounce window expired.
                pending_reload = None;
                first_event_at = None;
                attempt_reload(config_path, &mut current_config, callback);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break; // Channel closed.
            }
        }
    }
}

/// Attempt to reload the config file. On success, call the callback and update
/// `current_config`. On failure (file deleted or invalid), log a warning and
/// keep the old config.
fn attempt_reload(
    config_path: &Path,
    current_config: &mut PaneFlowConfig,
    callback: &Arc<dyn Fn(PaneFlowConfig) + Send + Sync>,
) {
    // US-029: read through the shared helper so the oversize guard (cheap stat
    // before allocating) applies on this hot path too - it previously read
    // with no cap, the only path a hostile/runaway file could freeze.
    let contents = match read_config_string(config_path) {
        Ok(Some(contents)) => contents,
        Ok(None) => {
            warn!(
                path = %config_path.display(),
                "config file was deleted; keeping previous config and continuing to watch"
            );
            return;
        }
        Err(error) => {
            warn!(%error, "config reload rejected; keeping previous config");
            return;
        }
    };

    // US-029: parse exactly once. A syntax error keeps the previous config
    // (never broadcast defaults on a malformed save); the old code parsed the
    // JSON twice - a syntax-guard `from_str` plus a second parse inside
    // `parse_and_validate_with_path`.
    let new_config = match try_parse_and_validate(&contents) {
        Ok(c) => c,
        Err(error) => {
            warn!(
                %error,
                "config file has validation errors; keeping previous config"
            );
            return;
        }
    };

    // US-029: a save that didn't actually change the parsed config (whitespace,
    // a `touch`, an unrelated key) shouldn't fire the callback and re-apply on
    // the GPUI thread.
    if new_config == *current_config {
        return;
    }

    info!("config reloaded successfully");
    *current_config = new_config.clone();
    callback(new_config);
}

#[cfg(test)]
#[path = "watcher_tests.rs"]
mod tests;
