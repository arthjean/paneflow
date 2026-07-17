//! "Notifications" settings page for OS-native agent notifications.

use gpui::{
    ClickEvent, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, div,
    prelude::*, px,
};
use paneflow_config::schema::NotifyWhenAgentWaiting;
use serde_json::Value;

use crate::PaneFlowApp;
use crate::settings::components::{section_header, setting_card, setting_text, toggle_pill};

impl PaneFlowApp {
    pub(crate) fn render_notifications_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let config = &self.cached_config;
        let native_notifications = config.agent_panel.as_ref().is_some_and(|agent_panel| {
            agent_panel.resolved_notify_when_agent_waiting() != NotifyWhenAgentWaiting::Never
        });
        let channels_card = setting_card(ui).child(agent_panel_toggle_row(
                "row-native-notifications",
                "Native OS notifications",
                "Send system notifications when agents need attention or finish while Paneflow is unfocused.",
                native_notifications,
                "notify_when_agent_waiting",
                if native_notifications {
                    Value::String("Never".to_string())
                } else {
                    Value::String("PrimaryScreen".to_string())
                },
                ui,
                cx,
            ));

        div()
            .flex()
            .flex_col()
            .child(section_header(ui, "Channels"))
            .child(channels_card)
            .child(div().h(px(180.)).flex_none())
    }
}

#[allow(clippy::too_many_arguments)]
fn agent_panel_toggle_row(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    current: bool,
    config_key: &'static str,
    target_value: Value,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> impl IntoElement {
    toggle_row(
        id,
        title,
        description,
        ui,
        div()
            .id(SharedString::from(id))
            .flex_shrink_0()
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.persist_agent_panel_setting(config_key, target_value.clone(), cx);
            }))
            .child(toggle_pill(current, ui)),
    )
}

fn toggle_row(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    ui: crate::theme::UiColors,
    toggle: impl IntoElement,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("{id}-row")))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(16.))
        .px(px(12.))
        .py(px(10.))
        .child(setting_text(ui, title, description))
        .child(toggle)
}
