use std::collections::HashSet;

pub(super) const READ_WINDOW_LINES: u64 = 500;

#[derive(Clone, Debug)]
pub(super) struct ReadSnapshot {
    pub(super) text: String,
    pub(super) output_generation: Option<u64>,
}

pub(super) fn is_surface_gone_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("not found") || lower.contains("-32602")
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
    let old_lines: HashSet<&str> = baseline.lines().collect();
    current
        .lines()
        .filter(|line| !old_lines.contains(line))
        .collect::<Vec<_>>()
        .join("\n")
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
}
