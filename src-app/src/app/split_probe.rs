use std::time::Instant;

use gpui::TestAppContext;

use crate::app::layout_pins::{Rig, isolated_child};
use crate::bench_harness::{Metric, from_samples};
use crate::{ClosePane, SplitVertically};

const PROBE_TEST: &str = "app::split_probe::split_request_to_updated_layout";
const SAMPLE_MARKER: &str = "PANEFLOW_SPLIT_PROBE_NS";
const WARMUP: usize = 20;
const ITERS: usize = 300;

#[gpui::test]
#[ignore = "run by the terminal benchmark in its own home"]
fn split_request_to_updated_layout(cx: &mut TestAppContext) {
    let mut rig = Rig::mount(cx);
    rig.workspace("bench");
    rig.focus("a");
    let mut samples = Vec::with_capacity(ITERS);
    for iteration in 0..WARMUP + ITERS {
        let started = Instant::now();
        rig.act(|app, window, cx| app.handle_split_v(&SplitVertically, window, cx));
        let elapsed = started.elapsed();
        if iteration >= WARMUP {
            samples.push(elapsed.as_nanos());
        }
        rig.act(|app, window, cx| app.handle_close_pane(&ClosePane, window, cx));
    }
    let line: Vec<String> = samples.iter().map(u128::to_string).collect();
    println!("{SAMPLE_MARKER} {}", line.join(" "));
}

pub(crate) fn split_latency_metric() -> Metric {
    const NAME: &str = "layout_split_request_to_updated_layout";
    const NOTE: &str = "a keyboard split on a one-pane tab, from the action handler to the layout updated and the effects queued, display-only terminals and a fake host, then the pane is closed untimed";
    let Ok(home) = tempfile::tempdir() else {
        return Metric::unavailable(NAME, "ns", NOTE);
    };
    let output = isolated_child(
        home.path(),
        &[
            "--ignored",
            "--exact",
            "--nocapture",
            "--test-threads=1",
            PROBE_TEST,
        ],
    )
    .output();
    let Ok(output) = output else {
        return Metric::unavailable(NAME, "ns", NOTE);
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut samples: Vec<std::time::Duration> = stdout
        .lines()
        .find_map(|line| line.split_once(SAMPLE_MARKER).map(|(_, rest)| rest))
        .map(|rest| {
            rest.split_whitespace()
                .filter_map(|value| value.parse::<u64>().ok())
                .map(std::time::Duration::from_nanos)
                .collect()
        })
        .unwrap_or_default();
    if !output.status.success() || samples.is_empty() {
        return Metric::unavailable(NAME, "ns", NOTE);
    }
    let total = samples.iter().sum();
    let iters = samples.len();
    let mut metric = from_samples(NAME, NOTE, &mut samples, total, (0, 0), iters);
    metric.alloc_bytes_per_iter = None;
    metric.allocs_per_iter = None;
    metric
}
