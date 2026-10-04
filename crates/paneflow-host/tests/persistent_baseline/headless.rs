use super::*;

use paneflow_host::work_counters::{CounterSample, Reading, Window};

pub(super) const SPIKE_RUNS: usize = 5;
pub(super) const SPIKE_WINDOW: Duration = Duration::from_secs(30);
pub(super) const FIRST_FRAME_LIMIT: Duration = Duration::from_secs(90);
pub(super) const UNSTABLE_CV: f64 = 0.10;

pub(super) fn spike_windows(
    read: impl Fn() -> CounterSample,
    runs: usize,
    window: Duration,
) -> Vec<Result<BTreeMap<String, Reading>, String>> {
    (0..runs)
        .map(|_| {
            let before = read();
            std::thread::sleep(window);
            match paneflow_host::work_counters::window(&before, &read()) {
                Window::Deltas(deltas) => Ok(deltas),
                Window::Invalid(reason) => Err(reason),
            }
        })
        .collect()
}

pub(super) fn counter_statistics(runs: &[Result<BTreeMap<String, Reading>, String>]) -> Value {
    let mut names: Vec<&String> = runs.iter().flatten().flat_map(BTreeMap::keys).collect();
    names.sort();
    names.dedup();
    let invalid: Vec<&String> = runs.iter().filter_map(|run| run.as_ref().err()).collect();
    let mut counters = serde_json::Map::new();
    for name in names {
        let readings: Vec<&Reading> = runs
            .iter()
            .flatten()
            .filter_map(|run| run.get(name))
            .collect();
        let values: Vec<f64> = readings
            .iter()
            .filter_map(|reading| match reading {
                Reading::Measured(value) => Some(*value as f64),
                Reading::Pending(_) => None,
            })
            .collect();
        let pending = readings.iter().find_map(|reading| match reading {
            Reading::Pending(reason) => Some(reason.clone()),
            Reading::Measured(_) => None,
        });
        let entry = match (pending, invalid.is_empty(), values.len() == runs.len()) {
            (Some(reason), _, _) => json!({"pending": reason}),
            (None, false, _) | (None, true, false) => {
                json!({"pending": format!("{} of {} windows were invalidated by a restart", invalid.len(), runs.len())})
            }
            (None, true, true) => {
                let mean = values.iter().sum::<f64>() / values.len() as f64;
                let variance =
                    values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
                let cv = (mean > 0.0).then(|| variance.sqrt() / mean);
                json!({
                    "values": values,
                    "mean": mean,
                    "cv": cv,
                    "max_deviation": values.iter().map(|v| (v - mean).abs()).fold(0.0, f64::max),
                })
            }
        };
        counters.insert(name.clone(), entry);
    }
    Value::Object(counters)
}

pub(super) fn unstable_counters(states: &Value) -> Vec<String> {
    let mut unstable = Vec::new();
    for (state, counters) in states.as_object().into_iter().flatten() {
        for (name, statistics) in counters["counters"].as_object().into_iter().flatten() {
            if statistics["cv"].as_f64().is_some_and(|cv| cv > UNSTABLE_CV) {
                unstable.push(format!("{state}.{name}"));
            }
        }
    }
    unstable
}

pub(super) fn conclusion(first_frame: Option<Duration>, states: &Value) -> (bool, Vec<String>) {
    let mut reasons = Vec::new();
    match first_frame {
        Some(elapsed) if elapsed < FIRST_FRAME_LIMIT => {}
        Some(elapsed) => reasons.push(format!(
            "first frame after {:.1} s, limit {} s",
            elapsed.as_secs_f64(),
            FIRST_FRAME_LIMIT.as_secs()
        )),
        None => reasons.push("the desktop never presented a frame".to_string()),
    }
    let idle = &states["idle_4_panes"]["counters"]["root_renders"];
    match idle["max_deviation"].as_f64() {
        Some(deviation) if deviation <= 1.0 => {}
        Some(deviation) => reasons.push(format!(
            "idle root_renders deviates by {deviation:.1} frames per window, limit 1"
        )),
        None => reasons.push(format!("idle root_renders was not measured: {idle}")),
    }
    (reasons.is_empty(), reasons)
}

fn desktop_failure(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|text| text.to_string()))
        .unwrap_or_else(|| "the desktop failed to start".to_string())
}

pub(super) fn start_desktop(
    home: &Path,
    sessions: &[SessionId],
    marker: bool,
) -> Result<DesktopProcess, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        DesktopProcess::start_ready(
            home,
            sessions,
            marker.then_some("fixture idle"),
            FIRST_FRAME_LIMIT,
        )
    }))
    .map_err(desktop_failure)
}

pub(super) fn start_desktop_in(
    home: &Path,
    cwd: &Path,
    sessions: &[SessionId],
) -> Result<DesktopProcess, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        DesktopProcess::start_ready_in(home, cwd, sessions, Some("fixture idle"), FIRST_FRAME_LIMIT)
    }))
    .map_err(desktop_failure)
}

pub(super) fn submit_prompt(endpoint: &Path, session: &SessionId) -> Value {
    HostClient::connect(endpoint, &ClientHello::control("headless-spike-ai-hook"))
        .and_then(|mut hook| {
            hook.call(
                paneflow_host::protocol::METHOD_AGENT_EVENT,
                json!({
                    "session": session,
                    "runtime_generation": 1,
                    "kind": "ai.prompt_submit",
                    "tool": "claude",
                    "hook_payload": {"hook_event_name": "UserPromptSubmit", "session_id": "headless-spike"},
                }),
            )
        })
        .unwrap_or_else(|error| json!({"error": error.to_string()}))
}

pub(super) fn measure_state(desktop: &DesktopProcess, runs: usize, window: Duration) -> Value {
    let windows = spike_windows(|| active::desktop_counters(desktop), runs, window);
    json!({
        "runs": runs,
        "window_s": window.as_secs_f64(),
        "invalid_windows": windows.iter().filter_map(|run| run.as_ref().err()).collect::<Vec<_>>(),
        "counters": counter_statistics(&windows),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(root_renders: u64) -> Result<BTreeMap<String, Reading>, String> {
        Ok(BTreeMap::from([
            ("root_renders".to_string(), Reading::Measured(root_renders)),
            (
                "session_list_calls".to_string(),
                Reading::Pending("old desktop".to_string()),
            ),
        ]))
    }

    #[test]
    fn statistics_report_mean_and_cv_and_keep_pending_counters_pending() {
        let stats = counter_statistics(&[run(2), run(3), run(2), run(3), run(2)]);
        assert_eq!(stats["root_renders"]["mean"], json!(2.4));
        assert!(stats["root_renders"]["cv"].as_f64().unwrap() > 0.2);
        assert_eq!(stats["session_list_calls"]["pending"], "old desktop");
        let restarted = counter_statistics(&[run(2), Err("desktop restarted".to_string())]);
        assert!(
            restarted["root_renders"]["pending"]
                .as_str()
                .unwrap()
                .contains("1 of 2")
        );
    }

    #[test]
    fn the_spike_is_validated_only_with_a_frame_in_time_and_a_stable_idle() {
        let states = |values: [u64; 5]| json!({"idle_4_panes": {"counters": counter_statistics(&values.map(run))}});
        let stable = states([2, 3, 2, 3, 2]);
        assert_eq!(
            conclusion(Some(Duration::from_secs(20)), &stable),
            (true, vec![])
        );
        let (validated, reasons) =
            conclusion(Some(Duration::from_secs(20)), &states([0, 9, 0, 0, 0]));
        assert!(!validated);
        assert!(reasons[0].contains("deviates"), "{reasons:?}");
        let (validated, reasons) = conclusion(None, &stable);
        assert!(!validated);
        assert_eq!(
            reasons,
            vec!["the desktop never presented a frame".to_string()]
        );
        assert_eq!(
            unstable_counters(&states([0, 9, 0, 0, 0])),
            vec!["idle_4_panes.root_renders".to_string()]
        );
    }
}
