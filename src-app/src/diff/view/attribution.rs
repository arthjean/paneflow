use super::*;
use crate::pricing;
use crate::ui_primitives::TooltipDelayExt;
use gpui::Hsla;

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

fn session_cost(s: &SessionMeta) -> Option<f64> {
    let usage = s.usage.as_ref()?;
    let model = s.model.as_deref()?;
    pricing::estimate_cost(model, usage)
}

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
                div()
                    .when(i > 0, |d| d.text_color(ui.muted))
                    .child(line.clone())
            }))
    }
}

impl DiffView {
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

    pub(super) fn render_attribution_badge(
        &self,
        col: &Column,
        ui: crate::theme::UiColors,
    ) -> Option<AnyElement> {
        let top = col.attribution.first()?;
        let cost = Self::column_cost(col);

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
        if let Some(model) = top.model.as_deref() {
            pill = pill.child(
                div()
                    .flex_none()
                    .text_color(ui.text)
                    .child(short_model(model)),
            );
        }
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
