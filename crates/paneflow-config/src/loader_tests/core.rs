#[path = "core_config.rs"]
mod config;
#[path = "core_layout.rs"]
mod layout;
#[path = "core_roundtrip.rs"]
mod roundtrip;
#[path = "core_validation.rs"]
mod validation;

#[cfg(unix)]
#[test]
fn a_fifo_config_falls_back_to_defaults_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paneflow.json");
    let made = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .is_ok_and(|status| status.success());
    assert!(made);
    let started = std::time::Instant::now();
    let loaded = super::load_config_from_path(&path);
    assert!(started.elapsed() < std::time::Duration::from_millis(100));
    assert_eq!(loaded, crate::schema::PaneFlowConfig::default());
    assert!(matches!(
        super::read_config_string(&path),
        Err(super::ConfigError::NotRegularFile { .. })
    ));
}
