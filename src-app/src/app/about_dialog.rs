use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Div, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, ObjectFit, ParentElement, Pixels, Stateful, Styled, StyledImage,
    Window, accesskit::Role, div, img, prelude::*, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::{
    MODAL_PADDING, ModalKey, menu_panel, menu_row, modal_backdrop, modal_card, modal_footer,
    modal_key, secondary_button,
};
use crate::theme::UiColors;
use crate::ui_primitives::{BODY, LABEL_SM};

const DIALOG_WIDTH: Pixels = px(400.);
const DIALOG_TITLE: Pixels = px(16.);
const APP_ICON_SIZE: Pixels = px(64.);
const CARD_RADIUS: Pixels = crate::app::constants::SETTINGS_CARD_RADIUS;
const VERSION_CHIP_RADIUS: Pixels = px(6.);
const LINK_ICON_SIZE: Pixels = px(14.);
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy)]
enum AboutLink {
    Website,
    Source,
    ReleaseNotes,
}

const ABOUT_LINKS: [AboutLink; 3] = [
    AboutLink::Website,
    AboutLink::Source,
    AboutLink::ReleaseNotes,
];

impl AboutLink {
    fn label(self) -> &'static str {
        match self {
            Self::Website => "Website",
            Self::Source => "Source code",
            Self::ReleaseNotes => "Release notes",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Website => "icons/world.svg",
            Self::Source => "icons/brand-github.svg",
            Self::ReleaseNotes => "icons/file-text.svg",
        }
    }

    fn url(self) -> String {
        match self {
            Self::Website => env!("CARGO_PKG_HOMEPAGE").to_string(),
            Self::Source => env!("CARGO_PKG_REPOSITORY").to_string(),
            Self::ReleaseNotes => crate::update::release_notes::changelog_url(VERSION),
        }
    }
}

fn display_url(url: &str) -> &str {
    url.strip_prefix("https://").unwrap_or(url)
}

pub(crate) struct AboutDialog {
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    selected: Option<usize>,
}

impl PaneFlowApp {
    pub(crate) fn open_about_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.about_dialog.is_some() {
            return;
        }
        let return_focus = window.focused(cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.about_dialog = Some(AboutDialog {
            focus,
            return_focus,
            selected: None,
        });
        cx.notify();
    }

    fn close_about_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.about_dialog.take() else {
            return;
        };
        if let Some(return_focus) = dialog.return_focus {
            return_focus.focus(window, cx);
        }
        cx.notify();
    }

    fn open_about_link(&mut self, link: AboutLink, cx: &mut Context<Self>) {
        self.open_help_url(&link.url(), cx);
    }

    fn handle_about_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.about_dialog.as_mut() else {
            return;
        };
        let count = ABOUT_LINKS.len();
        match event.keystroke.key.as_str() {
            "down" => {
                dialog.selected = Some(dialog.selected.map_or(0, |index| (index + 1) % count));
                cx.notify();
            }
            "up" => {
                dialog.selected = Some(
                    dialog
                        .selected
                        .map_or(count - 1, |index| (index + count - 1) % count),
                );
                cx.notify();
            }
            _ => match modal_key(event) {
                Some(ModalKey::Dismiss) => self.close_about_dialog(window, cx),
                Some(ModalKey::Confirm) => {
                    match dialog.selected.and_then(|index| ABOUT_LINKS.get(index)) {
                        Some(&link) => self.open_about_link(link, cx),
                        None => self.close_about_dialog(window, cx),
                    }
                }
                None => return,
            },
        }
        cx.stop_propagation();
    }

    pub(crate) fn render_about_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.about_dialog.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();

        let header = div()
            .flex()
            .flex_col()
            .items_center()
            .px(MODAL_PADDING)
            .pt(px(28.))
            .child(
                img("icons/paneflow.png")
                    .size(APP_ICON_SIZE)
                    .flex_none()
                    .object_fit(ObjectFit::Contain),
            )
            .child(
                div()
                    .pt(px(12.))
                    .text_size(DIALOG_TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui.text)
                    .child("Paneflow"),
            )
            .child(
                div()
                    .pt(px(2.))
                    .text_size(BODY)
                    .text_color(ui.muted)
                    .child(crate::app::welcome::WELCOME_TAGLINE),
            )
            .child(
                div()
                    .mt(px(12.))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(VERSION_CHIP_RADIUS)
                    .bg(ui.subtle)
                    .font_family("Geist Mono")
                    .text_size(LABEL_SM)
                    .text_color(ui.text)
                    .child(format!("Version {VERSION}")),
            );

        let links = div()
            .px(MODAL_PADDING)
            .pt(px(24.))
            .child(
                menu_panel(div(), ui).children(ABOUT_LINKS.iter().enumerate().map(
                    |(index, &link)| {
                        about_link_row(ui, index, link, dialog.selected == Some(index)).on_click(
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.open_about_link(link, cx);
                                cx.stop_propagation();
                            }),
                        )
                    },
                )),
            );

        let footer = modal_footer()
            .justify_between()
            .gap(px(16.))
            .child(
                div()
                    .min_w_0()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child(concat!("© Arthur Jean · ", env!("CARGO_PKG_LICENSE"))),
            )
            .child(secondary_button(
                "about-close",
                "Close",
                ui,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_about_dialog(window, cx);
                    cx.stop_propagation();
                }),
            ));

        let card = modal_card(
            "about-dialog",
            DIALOG_WIDTH,
            CARD_RADIUS,
            ui,
            div().child(header).child(links).child(footer),
        )
        .role(Role::Dialog)
        .aria_label("About Paneflow")
        .track_focus(&dialog.focus)
        .on_key_down(cx.listener(Self::handle_about_key_down));

        modal_backdrop(
            "about-dialog-backdrop",
            card,
            cx.listener(|this, _, window, cx| {
                this.close_about_dialog(window, cx);
            }),
        )
    }
}

fn about_link_row(ui: UiColors, index: usize, link: AboutLink, selected: bool) -> Stateful<Div> {
    let url = link.url();
    menu_row(("about-link", index), selected, ui)
        .w_full()
        .gap(px(10.))
        .px(px(10.))
        .cursor(CursorStyle::PointingHand)
        .role(Role::Link)
        .aria_label(link.label())
        .child(
            svg()
                .size(LINK_ICON_SIZE)
                .flex_none()
                .path(link.icon())
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_none()
                .text_size(BODY)
                .text_color(ui.text)
                .child(link.label()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_right()
                .text_size(LABEL_SM)
                .text_color(ui.muted)
                .child(display_url(&url).to_string()),
        )
}
