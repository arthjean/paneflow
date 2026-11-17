use log::{Level, Log, Metadata, Record};

pub(crate) const EXPECTED_PROBE_REJECTION: &str =
    "expected while a throwaway connection tests whether the compositor takes this GPU's buffers";

pub(crate) struct ProbeAwareLogger<L> {
    inner: L,
    probe_running: fn() -> bool,
}

impl<L: Log> ProbeAwareLogger<L> {
    pub(crate) fn new(inner: L, probe_running: fn() -> bool) -> Self {
        Self {
            inner,
            probe_running,
        }
    }
}

fn is_expected_probe_rejection(record: &Record, probe_running: bool) -> bool {
    probe_running
        && record.level() == Level::Error
        && record.target().starts_with("wayland_backend")
}

impl<L: Log> Log for ProbeAwareLogger<L> {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if !is_expected_probe_rejection(record, (self.probe_running)()) {
            self.inner.log(record);
            return;
        }
        let args = format_args!("{EXPECTED_PROBE_REJECTION}: {}", record.args());
        let downgraded = Record::builder()
            .args(args)
            .level(Level::Info)
            .target(record.target())
            .module_path(record.module_path())
            .file(record.file())
            .line(record.line())
            .build();
        if self.inner.enabled(downgraded.metadata()) {
            self.inner.log(&downgraded);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn presentation_probe_running() -> bool {
    gpui_wgpu::PresentationProbeScope::running()
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn presentation_probe_running() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(Level, String, String)>>);

    impl Log for Recorder {
        fn enabled(&self, metadata: &Metadata) -> bool {
            metadata.level() <= Level::Info
        }

        fn log(&self, record: &Record) {
            self.0.lock().unwrap().push((
                record.level(),
                record.target().to_string(),
                record.args().to_string(),
            ));
        }

        fn flush(&self) {}
    }

    fn log_through(logger: &ProbeAwareLogger<Recorder>, level: Level, target: &str) {
        logger.log(
            &Record::builder()
                .args(format_args!("Protocol error 7 on object @0"))
                .level(level)
                .target(target)
                .build(),
        );
    }

    #[test]
    fn a_wayland_error_during_the_presentation_probe_is_logged_as_expected_info() {
        let logger = ProbeAwareLogger::new(Recorder::default(), || true);
        log_through(&logger, Level::Error, "wayland_backend::sys::client_impl");
        let records = logger.inner.0.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].0, Level::Info);
        assert_eq!(records[0].1, "wayland_backend::sys::client_impl");
        assert_eq!(
            records[0].2,
            format!("{EXPECTED_PROBE_REJECTION}: Protocol error 7 on object @0")
        );
    }

    #[test]
    fn a_wayland_error_outside_the_probe_stays_an_error() {
        let logger = ProbeAwareLogger::new(Recorder::default(), || false);
        log_through(&logger, Level::Error, "wayland_backend::sys::client_impl");
        let records = logger.inner.0.lock().unwrap();
        assert_eq!(records[0].0, Level::Error);
        assert_eq!(records[0].2, "Protocol error 7 on object @0");
    }

    #[test]
    fn other_errors_during_the_probe_stay_errors() {
        let logger = ProbeAwareLogger::new(Recorder::default(), || true);
        log_through(&logger, Level::Error, "paneflow::launch");
        log_through(&logger, Level::Warn, "wayland_backend::sys::client_impl");
        let records = logger.inner.0.lock().unwrap();
        assert_eq!(records[0].0, Level::Error);
        assert_eq!(records[1].0, Level::Warn);
    }
}
