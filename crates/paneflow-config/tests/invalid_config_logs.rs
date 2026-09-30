#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Mutex;

struct CapturingLogger {
    lines: Mutex<Vec<String>>,
}

impl log::Log for CapturingLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        self.lines
            .lock()
            .unwrap()
            .push(format!("{} {}", record.level(), record.args()));
    }

    fn flush(&self) {}
}

static LOGGER: CapturingLogger = CapturingLogger {
    lines: Mutex::new(Vec::new()),
};

#[test]
fn an_invalid_paneflow_json_logs_a_using_defaults_line_through_the_log_facade() {
    log::set_logger(&LOGGER).expect("the only logger of this test binary");
    log::set_max_level(log::LevelFilter::Trace);
    let home = tempfile::tempdir().expect("home");
    let path = home.path().join("paneflow.json");
    std::fs::write(&path, "{ \"theme\": ").expect("invalid config");

    let config = paneflow_config::loader::load_config_from_path(&path);

    assert_eq!(config, paneflow_config::schema::PaneFlowConfig::default());
    let lines = LOGGER.lines.lock().unwrap();
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("WARN") && line.ends_with("using defaults")),
        "no `using defaults` warning reached the log facade: {lines:?}"
    );
}
