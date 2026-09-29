use std::time::Duration;

use paneflow_ipc_client::{IpcCallError, IpcTransport};
use serde_json::{Value, json};

pub(super) const READ_WINDOW_LINES: u64 = 500;

pub(super) const TRANSIENT_RETRIES: u32 = 3;

#[derive(Clone, Debug)]
pub(super) struct ReadSnapshot {
    pub(super) text: String,
    pub(super) output_generation: Option<u64>,
}

#[derive(Clone, Debug)]
pub(super) enum SurfaceRead {
    Snapshot(ReadSnapshot),
    Gone,
    Skipped,
}

pub(super) fn is_surface_gone_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("not found") || lower.contains("-32602")
}

pub(super) fn transient_backoff(attempt: u32) -> Duration {
    Duration::from_millis(250u64.saturating_mul(4u64.saturating_pow(attempt)))
        .min(Duration::from_secs(2))
}

fn read_surface_once(
    client: &impl IpcTransport,
    surface_id: u64,
    lines: u64,
) -> Result<SurfaceRead, IpcCallError> {
    match client.try_call(
        "surface.read",
        json!({ "surface_id": surface_id, "lines": lines, "fenced": false }),
    ) {
        Ok(result) => Ok(SurfaceRead::Snapshot(ReadSnapshot {
            text: result
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            output_generation: result.get("output_generation").and_then(Value::as_u64),
        })),
        Err(IpcCallError::Failed(message)) if is_surface_gone_error(&message) => {
            Ok(SurfaceRead::Gone)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn read_surface(
    client: &impl IpcTransport,
    surface_id: u64,
    lines: u64,
) -> Result<SurfaceRead, IpcCallError> {
    let mut attempt = 0;
    loop {
        match read_surface_once(client, surface_id, lines) {
            Err(error) if error.is_transient() => {
                if attempt == TRANSIENT_RETRIES {
                    return Ok(SurfaceRead::Skipped);
                }
                std::thread::sleep(transient_backoff(attempt));
                attempt += 1;
            }
            outcome => return outcome,
        }
    }
}

pub(super) fn read_baseline(
    client: &impl IpcTransport,
    surface_id: u64,
    lines: u64,
) -> Result<Option<ReadSnapshot>, String> {
    let mut attempt = 0;
    loop {
        match read_surface(client, surface_id, lines) {
            Ok(SurfaceRead::Snapshot(snapshot)) => return Ok(Some(snapshot)),
            Ok(SurfaceRead::Gone) => return Ok(None),
            Ok(SurfaceRead::Skipped) => {
                return Err(format!(
                    "could not read the baseline of surface {surface_id}: Paneflow stayed busy"
                ));
            }
            Err(error)
                if matches!(error, IpcCallError::Unreachable(_))
                    || attempt == TRANSIENT_RETRIES =>
            {
                return Err(format!(
                    "could not read the baseline of surface {surface_id}: {error}"
                ));
            }
            Err(_) => {
                std::thread::sleep(transient_backoff(attempt));
                attempt += 1;
            }
        }
    }
}

pub(super) fn text_after_baseline(
    baseline: &ReadSnapshot,
    current: &ReadSnapshot,
) -> Option<String> {
    if matches!(
        (current.output_generation, baseline.output_generation),
        (Some(current), Some(previous)) if current <= previous
    ) {
        return None;
    }
    Some(new_text_since_baseline(&baseline.text, &current.text))
}

fn new_text_since_baseline(baseline: &str, current: &str) -> String {
    if current == baseline {
        return String::new();
    }
    if let Some(rest) = current.strip_prefix(baseline) {
        return rest.to_string();
    }
    let old: Vec<&str> = baseline.lines().collect();
    let new: Vec<&str> = current.lines().collect();
    let width = new.len() + 1;
    let mut common = vec![0u32; (old.len() + 1) * width];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            common[i * width + j] = if old[i] == new[j] {
                common[(i + 1) * width + j + 1] + 1
            } else {
                common[(i + 1) * width + j].max(common[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut added = Vec::new();
    while j < new.len() {
        if i < old.len() && old[i] == new[j] {
            i += 1;
            j += 1;
        } else if i < old.len() && common[(i + 1) * width + j] >= common[i * width + j + 1] {
            i += 1;
        } else {
            added.push(new[j]);
            j += 1;
        }
    }
    added.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_diff_ignores_prompt_echo_sentinel() {
        let base = "please print RENDER_AUDIT_DONE when complete\n";
        let current = "please print RENDER_AUDIT_DONE when complete\nactual work\n";
        assert_eq!(new_text_since_baseline(base, current), "actual work\n");

        let shifted = "actual work\nplease print RENDER_AUDIT_DONE when complete\nnew DONE\n";
        assert_eq!(
            new_text_since_baseline(base, shifted),
            "actual work\nnew DONE"
        );
    }

    #[test]
    fn a_second_identical_line_in_the_same_pane_is_new_text() {
        let base = "build 1\nDONE\n";
        let current = "build 1\nDONE\nbuild 2\nDONE";
        assert_eq!(new_text_since_baseline(base, current), "build 2\nDONE");

        let scrolled = "DONE\nbuild 2\nDONE";
        assert_eq!(
            new_text_since_baseline(base, scrolled),
            "build 2\nDONE",
            "a window that scrolled past the baseline head still aligns in order"
        );
    }

    #[test]
    fn transient_backoff_spans_250_ms_to_2_s() {
        let delays: Vec<Duration> = (0..TRANSIENT_RETRIES).map(transient_backoff).collect();
        assert_eq!(
            delays,
            [250, 1000, 2000].map(Duration::from_millis).to_vec()
        );
        assert_eq!(transient_backoff(9), Duration::from_secs(2));
    }
}
