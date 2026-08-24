use super::*;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

/// Helper: write a valid minimal config file.
fn write_valid_config(path: &PathBuf) {
    fs::write(path, r#"{"default_shell": "/bin/bash", "commands": []}"#).unwrap();
}

/// Helper: write an updated config file with a different shell.
fn write_updated_config(path: &PathBuf) {
    fs::write(path, r#"{"default_shell": "/bin/zsh", "commands": []}"#).unwrap();
}

/// Helper: write invalid JSON to the config path.
fn write_invalid_config(path: &PathBuf) {
    fs::write(path, "this is not valid json {{{").unwrap();
}

/// Poll `condition` every 50ms until it returns `true` or `timeout` elapses.
/// Why: macOS FSEvents on CI runners can take >1s to deliver file events vs
/// near-instant inotify on Linux; a fixed sleep is inherently flaky across
/// platforms, so we poll instead.
fn wait_for<F: FnMut() -> bool>(mut condition: F, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    condition()
}

#[test]
fn test_config_watcher_new_with_path() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    let cb = Arc::new(|_: PaneFlowConfig| {});
    let watcher = ConfigWatcher::new_with_path(path.clone(), cb);
    assert_eq!(watcher.config_path, path);
}

#[test]
fn test_is_relevant_event() {
    use notify::event::*;

    assert!(is_relevant_event(&EventKind::Create(CreateKind::File)));
    assert!(is_relevant_event(&EventKind::Modify(ModifyKind::Data(
        DataChange::Content
    ))));
    assert!(is_relevant_event(&EventKind::Remove(RemoveKind::File)));
    assert!(!is_relevant_event(&EventKind::Access(AccessKind::Read)));
    assert!(!is_relevant_event(&EventKind::Other));
}

#[test]
fn test_event_targets_config() {
    let config_path = PathBuf::from("/tmp/paneflow/paneflow.json");

    let matching_event = Event {
        kind: EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Content,
        )),
        paths: vec![PathBuf::from("/tmp/paneflow/paneflow.json")],
        attrs: Default::default(),
    };
    assert!(event_targets_config(&matching_event, &config_path));

    let non_matching_event = Event {
        kind: EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Content,
        )),
        paths: vec![PathBuf::from("/tmp/paneflow/other.json")],
        attrs: Default::default(),
    };
    assert!(!event_targets_config(&non_matching_event, &config_path));
}

#[test]
fn test_attempt_reload_missing_file_keeps_old_config() {
    let path = PathBuf::from("/nonexistent/path/config.json");
    let mut current = PaneFlowConfig {
        default_shell: Some("/bin/bash".to_string()),
        ..Default::default()
    };
    let called = Arc::new(Mutex::new(false));
    let called_clone = Arc::clone(&called);
    let cb: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> =
        Arc::new(move |_| *called_clone.lock().unwrap() = true);

    attempt_reload(&path, &mut current, &cb);

    assert!(!*called.lock().unwrap(), "callback should not be called");
    assert_eq!(
        current.default_shell,
        Some("/bin/bash".to_string()),
        "old config should be preserved"
    );
}

#[test]
fn test_attempt_reload_invalid_json_keeps_old_config() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    write_invalid_config(&path);

    let mut current = PaneFlowConfig {
        default_shell: Some("/bin/bash".to_string()),
        ..Default::default()
    };
    let called = Arc::new(Mutex::new(false));
    let called_clone = Arc::clone(&called);
    let cb: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> =
        Arc::new(move |_| *called_clone.lock().unwrap() = true);

    attempt_reload(&path, &mut current, &cb);

    assert!(
        !*called.lock().unwrap(),
        "callback should not be called for invalid JSON"
    );
    assert_eq!(
        current.default_shell,
        Some("/bin/bash".to_string()),
        "old config should be preserved"
    );
}

#[test]
fn test_attempt_reload_non_object_root_keeps_old_config() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    fs::write(&path, "[]").unwrap();

    let mut current = PaneFlowConfig {
        default_shell: Some("/bin/bash".to_string()),
        ..Default::default()
    };
    let called = Arc::new(Mutex::new(false));
    let called_clone = Arc::clone(&called);
    let cb: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> =
        Arc::new(move |_| *called_clone.lock().unwrap() = true);

    attempt_reload(&path, &mut current, &cb);

    assert!(!*called.lock().unwrap());
    assert_eq!(current.default_shell.as_deref(), Some("/bin/bash"));
}

#[test]
fn test_running_watcher_drop_releases_callback() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    write_valid_config(&path);
    let callback: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> = Arc::new(|_| {});

    let watcher = ConfigWatcher::new_with_path(path, Arc::clone(&callback));
    let running = watcher.start().expect("watcher should start");
    assert!(Arc::strong_count(&callback) > 1);
    drop(running);

    assert_eq!(Arc::strong_count(&callback), 1);
}

#[test]
fn test_attempt_reload_valid_config_calls_callback() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    write_valid_config(&path);

    let mut current = PaneFlowConfig::default();
    let received = Arc::new(Mutex::new(None::<PaneFlowConfig>));
    let received_clone = Arc::clone(&received);
    let cb: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> =
        Arc::new(move |cfg| *received_clone.lock().unwrap() = Some(cfg));

    attempt_reload(&path, &mut current, &cb);

    let received_cfg = received
        .lock()
        .unwrap()
        .clone()
        .expect("callback should be called");
    assert_eq!(received_cfg.default_shell, Some("/bin/bash".to_string()));
    assert_eq!(current.default_shell, Some("/bin/bash".to_string()));
}

#[test]
fn test_attempt_reload_unchanged_config_skips_callback() {
    // US-029: a reload whose parsed result equals the current config must
    // NOT fire the callback (a touch / whitespace-only save).
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    write_valid_config(&path);
    let mut current = load_config_from_path(&path);

    let called = Arc::new(Mutex::new(false));
    let called_clone = Arc::clone(&called);
    let cb: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> =
        Arc::new(move |_| *called_clone.lock().unwrap() = true);

    attempt_reload(&path, &mut current, &cb);
    assert!(
        !*called.lock().unwrap(),
        "an unchanged config must not fire the callback"
    );
}

#[test]
fn test_attempt_reload_oversize_file_rejected() {
    // US-029 negative test: the hot reload path now applies the same
    // oversize guard as the cold loader (previously absent), so a runaway
    // file is rejected before allocating and the previous config is kept.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    let big = format!(
        r#"{{"default_shell": "/bin/zsh", "_pad": "{}"}}"#,
        "x".repeat(1_100_000) // > MAX_CONFIG_SIZE_BYTES (1 MiB)
    );
    fs::write(&path, big).unwrap();

    let mut current = PaneFlowConfig {
        default_shell: Some("/bin/bash".to_string()),
        ..Default::default()
    };
    let called = Arc::new(Mutex::new(false));
    let called_clone = Arc::clone(&called);
    let cb: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> =
        Arc::new(move |_| *called_clone.lock().unwrap() = true);

    attempt_reload(&path, &mut current, &cb);
    assert!(
        !*called.lock().unwrap(),
        "an oversize file must be rejected without firing the callback"
    );
    assert_eq!(
        current.default_shell,
        Some("/bin/bash".to_string()),
        "previous config kept"
    );
}

#[test]
fn test_watcher_detects_file_change() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    write_valid_config(&path);

    let received = Arc::new(Mutex::new(Vec::<PaneFlowConfig>::new()));
    let received_clone = Arc::clone(&received);
    let cb: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> =
        Arc::new(move |cfg| received_clone.lock().unwrap().push(cfg));

    let watcher = ConfigWatcher::new_with_path(path.clone(), cb);
    let _running = watcher.start().expect("watcher should start");

    // Give the watcher time to initialize.
    thread::sleep(Duration::from_millis(100));

    write_updated_config(&path);

    let received_poll = Arc::clone(&received);
    let fired = wait_for(
        move || !received_poll.lock().unwrap().is_empty(),
        Duration::from_secs(5),
    );
    assert!(fired, "callback should have been invoked at least once");

    let configs = received.lock().unwrap();
    let last = configs.last().unwrap();
    assert_eq!(last.default_shell, Some("/bin/zsh".to_string()));
}

#[test]
fn test_watcher_invalid_change_keeps_old() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    write_valid_config(&path);

    let received = Arc::new(Mutex::new(Vec::<PaneFlowConfig>::new()));
    let received_clone = Arc::clone(&received);
    let cb: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> =
        Arc::new(move |cfg| received_clone.lock().unwrap().push(cfg));

    let watcher = ConfigWatcher::new_with_path(path.clone(), cb);
    let _running = watcher.start().expect("watcher should start");

    thread::sleep(Duration::from_millis(100));

    // Write invalid JSON.
    write_invalid_config(&path);

    // Wait for debounce + processing.
    thread::sleep(Duration::from_millis(800));

    let configs = received.lock().unwrap();
    // Callback should NOT have been called (invalid JSON is rejected).
    assert!(
        configs.is_empty(),
        "callback should not be invoked for invalid config"
    );
}

#[test]
fn test_watcher_survives_file_deletion_and_recreation() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    write_valid_config(&path);

    let received = Arc::new(Mutex::new(Vec::<PaneFlowConfig>::new()));
    let received_clone = Arc::clone(&received);
    let cb: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> =
        Arc::new(move |cfg| received_clone.lock().unwrap().push(cfg));

    let watcher = ConfigWatcher::new_with_path(path.clone(), cb);
    let _running = watcher.start().expect("watcher should start");

    thread::sleep(Duration::from_millis(100));

    // Delete the file, then recreate with new content. macOS FSEvents may
    // coalesce both into a single event batch, so we only wait on the
    // post-recreation callback rather than pausing between steps.
    fs::remove_file(&path).unwrap();
    write_updated_config(&path);

    let received_poll = Arc::clone(&received);
    let fired = wait_for(
        move || {
            let guard = received_poll.lock().unwrap();
            guard
                .last()
                .is_some_and(|cfg| cfg.default_shell.as_deref() == Some("/bin/zsh"))
        },
        Duration::from_secs(5),
    );
    assert!(fired, "callback should fire after file recreation");
}

#[test]
fn test_debounce_coalesces_rapid_writes() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    write_valid_config(&path);

    let call_count = Arc::new(Mutex::new(0u32));
    let call_count_clone = Arc::clone(&call_count);
    let cb: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> =
        Arc::new(move |_| *call_count_clone.lock().unwrap() += 1);

    let watcher = ConfigWatcher::new_with_path(path.clone(), cb);
    let _running = watcher.start().expect("watcher should start");

    thread::sleep(Duration::from_millis(100));

    // Rapid-fire writes within the debounce window.
    for i in 0..5 {
        let shell = format!("/bin/shell{i}");
        let json = format!(r#"{{"default_shell": "{shell}", "commands": []}}"#);
        fs::write(&path, json).unwrap();
        thread::sleep(Duration::from_millis(50));
    }

    // Wait for at least one callback to fire (up to 5s for macOS CI).
    let call_count_poll = Arc::clone(&call_count);
    let fired = wait_for(
        move || *call_count_poll.lock().unwrap() >= 1,
        Duration::from_secs(5),
    );
    assert!(fired, "at least one reload should have occurred");

    // Then settle for an extra second so any trailing debounce flushes.
    thread::sleep(Duration::from_secs(1));

    let count = *call_count.lock().unwrap();
    // With debouncing, we should see fewer callbacks than writes.
    // Typically 1 (all coalesced), but timing may cause 2.
    assert!(
        count <= 2,
        "debounce should coalesce rapid writes, got {count} callbacks for 5 writes"
    );
}
