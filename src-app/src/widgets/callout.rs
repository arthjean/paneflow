use crate::theme::ui_colors;
use gpui::{
    AnyElement, FontWeight, Hsla, IntoElement, ParentElement, SharedString, Styled, div, hsla, px,
    svg,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CalloutSeverity {
    Warning,
}

#[derive(Clone, Copy)]
pub(crate) enum CalloutIcon {
    TriangleAlert,
}

pub(crate) struct Callout {
    severity: CalloutSeverity,
    icon: Option<CalloutIcon>,
    title: SharedString,
    description: Option<SharedString>,
}

impl Callout {
    pub(crate) fn new(severity: CalloutSeverity, title: impl Into<SharedString>) -> Self {
        Self {
            severity,
            icon: None,
            title: title.into(),
            description: None,
        }
    }

    pub(crate) fn icon(mut self, icon: CalloutIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub(crate) fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub(crate) fn render(self) -> AnyElement {
        let ui = ui_colors();
        let accent: Hsla = match self.severity {
            CalloutSeverity::Warning => hsla(40.0 / 360.0, 0.85, 0.55, 1.0),
        };

        let icon_element = self.icon.map(|i| {
            let path = match i {
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

        let description_element = self
            .description
            .map(|text| div().text_size(px(13.)).text_color(ui.muted).child(text));

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
        row.into_any_element()
    }
}
