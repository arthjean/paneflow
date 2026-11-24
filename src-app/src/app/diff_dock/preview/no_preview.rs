use std::path::Path;

use gpui::{
    AnyElement, App, ClickEvent, CursorStyle, FontWeight, InteractiveElement, IntoElement,
    ParentElement, SharedString, Styled, Window, div, img, prelude::FluentBuilder, px,
};

use super::{NoPreview, format_size};
use crate::app::files_sidebar::open_with::{
    OPEN_WITH_DEFAULT_LABEL, OPEN_WITH_LABEL, reveal_label,
};
use crate::ui_primitives::AccessibleControlExt;

pub(crate) const NO_PREVIEW_TITLE: &str = "No preview for this file";

pub(crate) type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

pub(crate) struct NoPreviewActions {
    pub(crate) open_default: ClickHandler,
    pub(crate) reveal: ClickHandler,
    pub(crate) open_with: Option<ClickHandler>,
}

pub(crate) fn no_preview_labels() -> [&'static str; 3] {
    [OPEN_WITH_DEFAULT_LABEL, reveal_label(), OPEN_WITH_LABEL]
}

pub(crate) fn detail_line(screen: &NoPreview) -> Option<String> {
    match (screen.size, screen.reason.as_deref()) {
        (Some(size), Some(reason)) => Some(format!("{} · {reason}", format_size(size))),
        (Some(size), None) => Some(format_size(size)),
        (None, Some(reason)) => Some(reason.to_string()),
        (None, None) => None,
    }
}

fn action_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    ui: crate::theme::UiColors,
    on_click: Option<ClickHandler>,
) -> AnyElement {
    let enabled = on_click.is_some();
    let button = div()
        .id(id)
        .flex_none()
        .h(px(26.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(ui.border)
        .when(primary, |button| button.bg(ui.surface))
        .accessible_control(gpui::accesskit::Role::Button, label)
        .text_size(crate::ui_primitives::BODY)
        .text_color(if enabled {
            ui.text
        } else {
            ui.muted.opacity(0.6)
        })
        .child(label);
    #[cfg(test)]
    let button = button.debug_selector(|| id.to_string());
    match on_click {
        Some(on_click) => button
            .cursor(CursorStyle::PointingHand)
            .hover(|style| style.bg(ui.subtle))
            .on_activate(move |event, window, cx| on_click(event, window, cx))
            .into_any_element(),
        None => button.into_any_element(),
    }
}

pub(crate) fn render_no_preview(
    path: &Path,
    screen: &NoPreview,
    actions: NoPreviewActions,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let icon = crate::file_icons::icon_path_for(
        crate::file_icons::file_icon_token(&name),
        crate::file_icons::IconVariant::for_ui(ui),
    );
    let [default_label, reveal, open_with] = no_preview_labels();
    div()
        .id("no-preview")
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .p(px(16.))
        .child(img(icon).size(px(32.)).flex_none())
        .child(
            div()
                .id("no-preview-title")
                .text_size(crate::ui_primitives::TITLE)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ui.text)
                .child(NO_PREVIEW_TITLE),
        )
        .children(detail_line(screen).map(|line| {
            div()
                .max_w(px(420.))
                .text_size(crate::ui_primitives::BODY)
                .text_color(ui.muted)
                .text_center()
                .child(SharedString::from(line))
        }))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .justify_center()
                .gap(px(6.))
                .pt(px(4.))
                .child(action_button(
                    "no-preview-open-default",
                    default_label,
                    true,
                    ui,
                    Some(actions.open_default),
                ))
                .child(action_button(
                    "no-preview-reveal",
                    reveal,
                    false,
                    ui,
                    Some(actions.reveal),
                ))
                .child(action_button(
                    "no-preview-open-with",
                    open_with,
                    false,
                    ui,
                    actions.open_with,
                )),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screen_offers_the_three_tree_actions_in_order() {
        assert_eq!(
            no_preview_labels(),
            [
                "Open with Default App",
                crate::app::files_sidebar::open_with::reveal_label(),
                "Open With"
            ]
        );
    }

    #[test]
    fn the_detail_line_joins_the_size_and_the_reason() {
        assert_eq!(
            detail_line(&NoPreview::new(Some(2048), "Binary.")),
            Some("2 KB · Binary.".to_string())
        );
        assert_eq!(
            detail_line(&NoPreview {
                size: Some(3),
                reason: None
            }),
            Some("3 bytes".to_string())
        );
        assert_eq!(
            detail_line(&NoPreview::special()).as_deref(),
            NoPreview::special().reason.as_deref()
        );
    }
}
