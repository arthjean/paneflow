use crate::theme::ui_colors;
use gpui::{
    AnyElement, FontWeight, Hsla, IntoElement, ParentElement, SharedString, Styled, div, hsla, px,
    svg,
};

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CalloutSeverity {
    Info,
    Warning,
    Error,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub(crate) enum CalloutIcon {
    Info,
    XCircle,
    TriangleAlert,
}

pub(crate) struct Callout {
    severity: CalloutSeverity,
    icon: Option<CalloutIcon>,
    title: SharedString,
    description: Option<SharedString>,
    description_slot: Option<AnyElement>,
    actions_slot: Option<AnyElement>,
}

impl Callout {
    pub(crate) fn new(severity: CalloutSeverity, title: impl Into<SharedString>) -> Self {
        Self {
            severity,
            icon: None,
            title: title.into(),
            description: None,
            description_slot: None,
            actions_slot: None,
        }
    }

    pub(crate) fn icon(mut self, icon: CalloutIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    #[allow(dead_code)]
    pub(crate) fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[allow(dead_code)]
    pub(crate) fn description_slot(mut self, slot: AnyElement) -> Self {
        self.description_slot = Some(slot);
        self
    }

    #[allow(dead_code)]
    pub(crate) fn actions_slot(mut self, slot: AnyElement) -> Self {
        self.actions_slot = Some(slot);
        self
    }

    pub(crate) fn render(self) -> AnyElement {
        let ui = ui_colors();
        let accent: Hsla = match self.severity {
            CalloutSeverity::Info => ui.accent,
            CalloutSeverity::Warning => hsla(40.0 / 360.0, 0.85, 0.55, 1.0),
            CalloutSeverity::Error => hsla(0.0, 0.62, 0.56, 1.0),
        };

        let icon_element = self.icon.map(|i| {
            let path = match i {
                CalloutIcon::Info => "icons/bulb.svg",
                CalloutIcon::XCircle => "icons/x_circle.svg",
                CalloutIcon::TriangleAlert => "icons/triangle-alert.svg",
            };
            svg()
                .size(px(16.))
                .flex_none()
                .path(path)
                .text_color(accent)
                .into_any_element()
        });

        let title_element = div()
            .text_size(px(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ui.text)
            .child(self.title);

        let description_element = match (self.description_slot, self.description) {
            (Some(slot), _) => Some(slot),
            (None, Some(text)) => Some(
                div()
                    .text_size(px(13.))
                    .text_color(ui.muted)
                    .child(text)
                    .into_any_element(),
            ),
            _ => None,
        };

        let mut content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .gap(px(6.))
            .child(title_element);
        if let Some(desc) = description_element {
            content = content.child(desc);
        }

        let mut row = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(10.))
            .w_full()
            .max_w(px(560.))
            .px(px(16.))
            .py(px(14.))
            .rounded(px(8.))
            .border_1()
            .border_color(accent)
            .bg(ui.surface);
        if let Some(icon) = icon_element {
            row = row.child(div().mt(px(2.)).child(icon));
        }
        row = row.child(content);
        if let Some(actions) = self.actions_slot {
            row = row.child(div().flex().flex_none().items_center().child(actions));
        }
        row.into_any_element()
    }
}
