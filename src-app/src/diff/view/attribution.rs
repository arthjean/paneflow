//! EP-004 (Agent Attribution & Cost) rendering for the Review view: the
//! per-column attribution badge (US-015) and the estimated-cost figures
//! (US-017). The matching + token/usage parsing happens off-thread in the
//! column-load task (see [`super::loader`]); this module is pure render over the
//! `Column::attribution` already cached on the column, so it stays O(1) per
//! frame. Cost is ALWAYS labeled estimated and an unpriced model shows tokens
//! without a fabricated number.

use super::*;
use crate::pricing;
use crate::ui_primitives::TooltipDelayExt;
use gpui::Hsla;

/// Brand accent for a session agent's glyph - mirrors the sessions sidebar /
/// launcher buttons. Multi-color logos are rendered through `img()` and ignore
/// this tint.
fn agent_brand_color(
    agent: crate::agent_sessions::SessionAgent,
    ui: crate::theme::UiColors,
) -> Hsla {
    agent
        .terminal_agent()
        .accent()
        .map(|accent| gpui::rgb(accent).into())
        .unwrap_or(ui.text)
}

/// Compact model family label for the badge (the full id lands in the tooltip).
fn short_model(model: &str) -> String {
    let lc = model.to_ascii_lowercase();
    if lc.contains("opus") {
        "Opus".into()
    } else if lc.contains("sonnet") {
        "Sonnet".into()
    } else if lc.contains("haiku") {
        "Haiku".into()
    } else if lc.contains("gpt-5") {
        "GPT-5".into()
    } else if model.chars().count() <= 16 {
        model.to_string()
    } else {
        format!("{}…", model.chars().take(15).collect::<String>())
    }
}

/// Per-session estimated cost - `Some` only when the session carries both a
/// model and usage AND the model is in the pricing table.
fn session_cost(s: &SessionMeta) -> Option<f64> {
    let usage = s.usage.as_ref()?;
    let model = s.model.as_deref()?;
    pricing::estimate_cost(model, usage)
}

/// A small multi-line tooltip body for the attribution badge: one line per
/// matched session plus an aggregate/version footer. Distinct from
/// [`crate::ui_primitives::PaneflowTooltip`] (single-line) because US-015/US-017
/// want a per-session breakdown.
pub(super) struct AttributionTooltip {
    pub(super) lines: Vec<SharedString>,
}

impl Render for AttributionTooltip {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        crate::ui_primitives::tooltip_shell()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(self.lines.iter().enumerate().map(|(i, line)| {
                // First line (the title) at full strength; the rest muted.
                div()
                    .when(i > 0, |d| d.text_color(ui.muted))
                    .child(line.clone())
            }))
    }
}

impl DiffView {
    /// Sum of estimated cost across a column's matched sessions, or `None` when
    /// none are priceable (unknown model or no usage on every match).
    fn column_cost(col: &Column) -> Option<f64> {
        let mut total = 0.0;
        let mut any = false;
        for s in &col.attribution {
            if let Some(c) = session_cost(s) {
                total += c;
                any = true;
            }
        }
        any.then_some(total)
    }

    /// EP-004 US-017: "Total ~$X.XX across N worktrees" for the toolbar - summed
    /// over visible columns that carry a cost. `None` when nothing is priced.
    pub(super) fn attribution_total(&self) -> Option<(f64, usize)> {
        let mut total = 0.0;
        let mut n = 0usize;
        for col in &self.columns {
            if !col.visible {
                continue;
            }
            if let Some(c) = Self::column_cost(col) {
                total += c;
                n += 1;
            }
        }
        (n > 0).then_some((total, n))
    }

    /// EP-004 US-015/US-017: the attribution badge for a column header - agent
    /// glyph + model + "~$X.XX (est.)" as a border-only pill, with a hover
    /// breakdown. `None` (zero-width slot, pixel-identical to no-attribution)
    /// when the column has no matched session.
    pub(super) fn render_attribution_badge(
        &self,
        col: &Column,
        ui: crate::theme::UiColors,
    ) -> Option<AnyElement> {
        let top = col.attribution.first()?;
        let cost = Self::column_cost(col);

        // Tooltip: a line per session (most-relevant first), an aggregate
        // token-tier line, then the estimated/version footer.
        let mut lines: Vec<SharedString> = Vec::new();
        lines.push(
            format!(
                "Attributed to {} session{}",
                col.attribution.len(),
                if col.attribution.len() == 1 { "" } else { "s" }
            )
            .into(),
        );
        for s in &col.attribution {
            let when = crate::agent_sessions::format_relative_time(&s.timestamp);
            let model = s.model.as_deref().unwrap_or("unknown model");
            let cost_str = match session_cost(s) {
                Some(c) => pricing::format_cost(c),
                None if s.usage.is_some() => "unpriced model".to_string(),
                None => "no usage".to_string(),
            };
            lines.push(format!("{} · {model} · {when} · {cost_str}", s.agent.label()).into());
        }
        // Aggregate token tiers across the matched sessions (US-017 breakdown).
        let mut agg = crate::agent_sessions::AssistantUsage::default();
        for s in &col.attribution {
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

        let icon = if top.agent.terminal_agent().icon_multicolor() {
            gpui::img(top.agent.icon_path())
                .size(px(11.))
                .flex_none()
                .into_any_element()
        } else {
            gpui::svg()
                .size(px(11.))
                .flex_none()
                .path(top.agent.icon_path())
                .text_color(agent_brand_color(top.agent, ui))
                .into_any_element()
        };
        let mut pill = div()
            .id("diff-attribution-badge")
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .px(px(5.))
            .py(px(1.))
            .rounded(px(4.))
            .border_1()
            .border_color(ui.border)
            .text_size(crate::ui_primitives::LABEL_XS)
            .text_color(ui.muted)
            .delayed_tooltip(move |_w, cx| {
                let lines = lines.clone();
                cx.new(|_| AttributionTooltip { lines }).into()
            })
            .child(icon);
        // Model short name (when known).
        if let Some(model) = top.model.as_deref() {
            pill = pill.child(
                div()
                    .flex_none()
                    .text_color(ui.text)
                    .child(short_model(model)),
            );
        }
        // Estimated cost (when priceable). Unpriced/usage-less → glyph + model
        // only, never a fabricated number.
        if let Some(c) = cost {
            pill = pill.child(
                div()
                    .flex_none()
                    .text_color(ui.muted)
                    .child(format!("{} (est.)", pricing::format_cost(c))),
            );
        }
        Some(pill.into_any_element())
    }
}
