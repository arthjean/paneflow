use super::*;
use crate::pricing;

fn session_cost(s: &SessionMeta) -> Option<f64> {
    let usage = s.usage.as_ref()?;
    let model = s.model.as_deref()?;
    pricing::estimate_cost(model, usage)
}

impl DiffView {
    pub fn attribution_lines(&self) -> Vec<SharedString> {
        let sessions = &self.column.attribution;
        if sessions.is_empty() {
            return Vec::new();
        }
        let mut lines: Vec<SharedString> = Vec::new();
        lines.push(
            format!(
                "Attributed to {} session{}",
                sessions.len(),
                if sessions.len() == 1 { "" } else { "s" }
            )
            .into(),
        );
        for s in sessions {
            let when = crate::agent_sessions::format_relative_time(&s.timestamp);
            let model = s.model.as_deref().unwrap_or("unknown model");
            let cost_str = match session_cost(s) {
                Some(c) => pricing::format_cost(c),
                None if s.usage.is_some() => "unpriced model".to_string(),
                None => "no usage".to_string(),
            };
            lines.push(format!("{} · {model} · {when} · {cost_str}", s.agent.label()).into());
        }
        let mut agg = crate::agent_sessions::AssistantUsage::default();
        for s in sessions {
            if let Some(u) = s.usage.as_ref() {
                agg.add(u);
            }
        }
        if !agg.is_empty() {
            lines.push(
                format!(
                    "tokens: {} in · {} out · {} cache",
                    agg.input,
                    agg.output,
                    agg.cache_read.saturating_add(agg.cache_creation)
                )
                .into(),
            );
        }
        lines.push(format!("estimated · prices v{}", pricing::PRICING_TABLE_VERSION).into());
        lines
    }
}
