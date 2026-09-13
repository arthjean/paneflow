use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use gpui::Window;

pub(crate) const OUTPUT_PATH_ENV: &str = "PANEFLOW_STARTUP_TRACE";
const SCHEMA_VERSION: u32 = 1;

static ORIGIN: OnceLock<Instant> = OnceLock::new();
static MARKS: Mutex<Vec<Mark>> = Mutex::new(Vec::new());
static FIRST_RENDER_SEEN: AtomicBool = AtomicBool::new(false);
static FIRST_RENDER_BUILT: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mark {
    pub(crate) name: &'static str,
    pub(crate) at_us: u64,
}

pub(crate) fn begin() {
    ORIGIN.get_or_init(Instant::now);
}

pub(crate) fn mark(name: &'static str) {
    if output_path().is_some() {
        record(name);
    }
}

fn record(name: &'static str) {
    let Some(origin) = ORIGIN.get() else {
        return;
    };
    let at_us = origin.elapsed().as_micros().min(u64::MAX as u128) as u64;
    MARKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(Mark { name, at_us });
}

pub(crate) fn output_path() -> Option<&'static PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        std::env::var_os(OUTPUT_PATH_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    })
    .as_ref()
}

pub(crate) fn on_app_render(window: &mut Window) {
    if output_path().is_none() || FIRST_RENDER_SEEN.swap(true, Ordering::SeqCst) {
        return;
    }
    mark("first_render");
    window.on_next_frame(|_, cx| {
        mark("first_frame");
        if let Some(path) = output_path() {
            let marks = MARKS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Err(error) = std::fs::write(path, report_json(&marks)) {
                log::error!("startup trace: failed to write {}: {error}", path.display());
            }
        }
        cx.quit();
    });
}

pub(crate) fn on_app_render_built() {
    if output_path().is_none() || FIRST_RENDER_BUILT.swap(true, Ordering::SeqCst) {
        return;
    }
    mark("first_render_built");
}

pub(crate) fn report_json(marks: &[Mark]) -> String {
    let mut previous_us = 0;
    let steps: Vec<serde_json::Value> = marks
        .iter()
        .map(|mark| {
            let step_us = mark.at_us.saturating_sub(previous_us);
            previous_us = mark.at_us;
            serde_json::json!({
                "name": mark.name,
                "at_us": mark.at_us,
                "step_us": step_us,
            })
        })
        .collect();
    let total_us = marks.last().map(|mark| mark.at_us).unwrap_or(0);
    let document = serde_json::json!({
        "schema": SCHEMA_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "os": std::env::consts::OS,
        "total_us": total_us,
        "marks": steps,
    });
    serde_json::to_string_pretty(&document).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{Mark, begin, record, report_json};

    #[test]
    fn marks_are_monotonic_from_the_origin() {
        begin();
        record("first");
        std::thread::sleep(std::time::Duration::from_millis(2));
        record("second");
        let marks = super::MARKS.lock().unwrap().clone();
        let first = marks.iter().position(|m| m.name == "first").unwrap();
        let second = marks.iter().position(|m| m.name == "second").unwrap();
        assert!(first < second);
        assert!(marks[second].at_us >= marks[first].at_us + 2_000);
    }

    #[test]
    fn report_carries_absolute_and_step_durations() {
        let marks = [
            Mark {
                name: "home_migrated",
                at_us: 1_500,
            },
            Mark {
                name: "first_frame",
                at_us: 9_000,
            },
        ];
        let document: serde_json::Value = serde_json::from_str(&report_json(&marks)).unwrap();
        assert_eq!(document["schema"], 1);
        assert_eq!(document["total_us"], 9_000);
        assert_eq!(document["marks"][0]["step_us"], 1_500);
        assert_eq!(document["marks"][1]["name"], "first_frame");
        assert_eq!(document["marks"][1]["step_us"], 7_500);
        assert!(document["profile"].is_string());
    }

    #[test]
    fn empty_report_is_still_valid_json() {
        let document: serde_json::Value = serde_json::from_str(&report_json(&[])).unwrap();
        assert_eq!(document["total_us"], 0);
        assert_eq!(document["marks"].as_array().unwrap().len(), 0);
    }
}
