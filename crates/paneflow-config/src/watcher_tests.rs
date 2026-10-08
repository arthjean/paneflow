use super::*;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

fn write_valid_config(path: &PathBuf) {
    fs::write(path, r#"{"default_shell": "/bin/bash", "commands": []}"#).unwrap();
}

fn write_updated_config(path: &PathBuf) {
    fs::write(path, r#"{"default_shell": "/bin/zsh", "commands": []}"#).unwrap();
}

fn write_invalid_config(path: &PathBuf) {
    fs::write(path, "this is not valid json {{{").unwrap();
}

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
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    let big = format!(
        r#"{{"default_shell": "/bin/zsh", "_pad": "{}"}}"#,
        "x".repeat(1_100_000)
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

    write_invalid_config(&path);

    thread::sleep(Duration::from_millis(800));

    let configs = received.lock().unwrap();
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

    for i in 0..5 {
        let shell = format!("/bin/shell{i}");
        let json = format!(r#"{{"default_shell": "{shell}", "commands": []}}"#);
        fs::write(&path, json).unwrap();
        thread::sleep(Duration::from_millis(50));
    }

    let call_count_poll = Arc::clone(&call_count);
    let fired = wait_for(
        move || *call_count_poll.lock().unwrap() >= 1,
        Duration::from_secs(5),
    );
    assert!(fired, "at least one reload should have occurred");

    thread::sleep(Duration::from_secs(1));

    let count = *call_count.lock().unwrap();
    assert!(
        count <= 2,
        "debounce should coalesce rapid writes, got {count} callbacks for 5 writes"
    );
}

fn link_to(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(target, link).expect("symlink");
}

fn start_recording(path: &Path) -> (RunningConfigWatcher, Arc<Mutex<Vec<PaneFlowConfig>>>) {
    let received = Arc::new(Mutex::new(Vec::<PaneFlowConfig>::new()));
    let received_clone = Arc::clone(&received);
    let cb: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> =
        Arc::new(move |cfg| received_clone.lock().unwrap().push(cfg));
    let running = ConfigWatcher::new_with_path(path.to_path_buf(), cb)
        .start()
        .expect("watcher should start");
    thread::sleep(Duration::from_millis(100));
    (running, received)
}

fn last_shell(received: &Arc<Mutex<Vec<PaneFlowConfig>>>) -> Option<String> {
    received
        .lock()
        .unwrap()
        .last()
        .and_then(|config| config.default_shell.clone())
}

#[test]
fn test_watcher_reloads_within_a_second_when_the_symlink_target_changes() {
    let dir = TempDir::new().unwrap();
    let dotfiles = dir.path().join("dotfiles");
    let home = dir.path().join("home");
    fs::create_dir_all(&dotfiles).unwrap();
    fs::create_dir_all(&home).unwrap();
    let target = dotfiles.join("paneflow.json");
    write_valid_config(&target);
    let path = home.join("paneflow.json");
    link_to(&target, &path);
    let (_running, received) = start_recording(&path);

    let written_at = Instant::now();
    write_updated_config(&target);
    let fired = wait_for(
        || last_shell(&received).as_deref() == Some("/bin/zsh"),
        Duration::from_secs(1),
    );

    assert!(
        fired,
        "no reload within 1 s of editing the symlink target ({:?})",
        written_at.elapsed()
    );
}

#[test]
fn test_watcher_follows_a_retargeted_symlink() {
    let dir = TempDir::new().unwrap();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    let home = dir.path().join("home");
    for folder in [&first, &second, &home] {
        fs::create_dir_all(folder).unwrap();
    }
    write_valid_config(&first.join("paneflow.json"));
    let moved = second.join("paneflow.json");
    write_updated_config(&moved);
    let path = home.join("paneflow.json");
    link_to(&first.join("paneflow.json"), &path);
    let (_running, received) = start_recording(&path);

    fs::remove_file(&path).unwrap();
    link_to(&moved, &path);
    assert!(wait_for(
        || last_shell(&received).as_deref() == Some("/bin/zsh"),
        Duration::from_secs(5),
    ));

    fs::write(&moved, r#"{"default_shell": "/bin/fish", "commands": []}"#).unwrap();
    assert!(
        wait_for(
            || last_shell(&received).as_deref() == Some("/bin/fish"),
            Duration::from_secs(5),
        ),
        "an edit of the new target must reload"
    );
}

#[test]
fn test_attempt_reload_out_of_range_checksum_extension_keeps_old_value() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("paneflow.json");
    fs::write(&path, r#"{"terminal": {"xt_checksum_extension": 32}}"#).unwrap();

    let mut current = PaneFlowConfig {
        terminal: Some(crate::schema::TerminalConfig {
            xt_checksum_extension: Some(4),
            ..Default::default()
        }),
        ..Default::default()
    };
    let called = Arc::new(Mutex::new(false));
    let called_clone = Arc::clone(&called);
    let cb: Arc<dyn Fn(PaneFlowConfig) + Send + Sync> =
        Arc::new(move |_| *called_clone.lock().unwrap() = true);

    attempt_reload(&path, &mut current, &cb);

    assert!(!*called.lock().unwrap());
    assert_eq!(
        current
            .terminal
            .as_ref()
            .map(crate::schema::TerminalConfig::resolved_xt_checksum_extension),
        Some(4)
    );
}
