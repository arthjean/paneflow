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

const DEBOUNCE_DURATION: Duration = Duration::from_millis(300);

const MAX_DEBOUNCE: Duration = Duration::from_secs(1);

pub struct ConfigWatcher {
    callback: Arc<dyn Fn(PaneFlowConfig) + Send + Sync>,
    config_path: PathBuf,
}

enum WatcherMessage {
    Event(notify::Result<Event>),
    Stop,
}

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

    #[cfg(test)]
    fn new_with_path(path: PathBuf, callback: Arc<dyn Fn(PaneFlowConfig) + Send + Sync>) -> Self {
        Self {
            callback,
            config_path: path,
        }
    }

    pub fn start(self) -> Result<RunningConfigWatcher, notify::Error> {
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

        let (tx, rx) = mpsc::channel::<WatcherMessage>();
        let stop_tx = tx.clone();

        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(WatcherMessage::Event(res));
            },
            notify::Config::default(),
        )?;

        watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;

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

fn is_relevant_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn event_targets_config(event: &Event, config_path: &Path) -> bool {
    let target_name = config_path.file_name();
    target_name.is_some() && event.paths.iter().any(|p| p.file_name() == target_name)
}

fn event_loop(
    rx: mpsc::Receiver<WatcherMessage>,
    config_path: &Path,
    callback: &Arc<dyn Fn(PaneFlowConfig) + Send + Sync>,
    _watcher: &RecommendedWatcher,
) {
    let mut current_config = load_config_from_path(config_path);
    let mut pending_reload: Option<Instant> = None;
    let mut first_event_at: Option<Instant> = None;

    loop {
        let event_result = if let Some(deadline) = pending_reload {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                pending_reload = None;
                first_event_at = None;
                attempt_reload(config_path, &mut current_config, callback);
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
            Ok(WatcherMessage::Event(Ok(event))) => {
                if is_relevant_event(&event.kind) && event_targets_config(&event, config_path) {
                    let now = Instant::now();
                    let burst_start = *first_event_at.get_or_insert(now);
                    let deadline = (now + DEBOUNCE_DURATION).min(burst_start + MAX_DEBOUNCE);
                    pending_reload = Some(deadline);
                }
            }
            Ok(WatcherMessage::Event(Err(e))) => {
                warn!("file watcher error: {e}");
            }
            Ok(WatcherMessage::Stop) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                pending_reload = None;
                first_event_at = None;
                attempt_reload(config_path, &mut current_config, callback);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }
}

fn attempt_reload(
    config_path: &Path,
    current_config: &mut PaneFlowConfig,
    callback: &Arc<dyn Fn(PaneFlowConfig) + Send + Sync>,
) {
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
