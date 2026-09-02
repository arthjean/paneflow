use gpui::{
    AnyElement, ClickEvent, Context, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, StatefulInteractiveElement, Styled, Window, div, px, rgb, svg,
};

use super::render::render_diff_header_icon_button;
use crate::PaneFlowApp;
use crate::settings::components::with_alpha;

const CARD_WIDTH: f32 = 122.0;
const CARD_HEIGHT: f32 = 98.0;
const CARD_GAP: f32 = 12.0;
const CARD_RADIUS: f32 = 10.0;
const CARD_ICON_GAP: f32 = 8.0;
const GRID_PADDING: f32 = 16.0;

fn card_ink(ui: crate::theme::UiColors) -> (Hsla, Hsla, Hsla) {
    if ui.base.l > 0.5 {
        (ui.border, ui.muted, ui.text)
    } else {
        (
            rgb(0x2c2c2c).into(),
            rgb(0x8b8b8b).into(),
            rgb(0xb9b9b9).into(),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DiffDockSurface {
    Changes,
    Terminal,
    File,
}

impl PaneFlowApp {
    pub(crate) fn choose_diff_dock_surface(
        &mut self,
        surface: DiffDockSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.diff_dock.picker = false;
        self.diff_dock.picked = true;
        match surface {
            DiffDockSurface::Changes => self.select_diff_tab(0, cx),
            DiffDockSurface::Terminal => self.open_diff_terminal_tab(window, cx),
            DiffDockSurface::File => self.open_diff_file_picker(window, cx),
        }
        cx.notify();
    }
}

pub(super) fn render_diff_picker_header(
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    div()
        .h(px(40.))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .px(px(8.))
        .child(div().flex_1().min_w_0())
        .child(render_diff_header_icon_button(
            "diff-dock-picker-close",
            "icons/close.svg",
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.close_diff_dock_panel(cx);
            }),
            ui.muted,
        ))
        .into_any_element()
}

pub(super) fn render_diff_surface_picker(
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .p(px(GRID_PADDING))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .justify_center()
                .gap(px(CARD_GAP))
                .child(card(
                    "diff-dock-picker-changes",
                    "icons/git-pull-request.svg",
                    "Changes",
                    DiffDockSurface::Changes,
                    ui,
                    cx,
                ))
                .child(card(
                    "diff-dock-picker-terminal",
                    "icons/terminal.svg",
                    "Terminal",
                    DiffDockSurface::Terminal,
                    ui,
                    cx,
                ))
                .child(card(
                    "diff-dock-picker-file",
                    "icons/file-text.svg",
                    "File",
                    DiffDockSurface::File,
                    ui,
                    cx,
                )),
        )
        .into_any_element()
}

fn card(
    id: &'static str,
    icon: &'static str,
    label: &'static str,
    surface: DiffDockSurface,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let (border, glyph, ink) = card_ink(ui);
    div()
        .id(id)
        .flex_none()
        .w(px(CARD_WIDTH))
        .h(px(CARD_HEIGHT))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(CARD_ICON_GAP))
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(border)
        .hover(|style| style.bg(with_alpha(ink, 0.05)))
        .cursor(gpui::CursorStyle::PointingHand)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.choose_diff_dock_surface(surface, window, cx);
        }))
        .child(svg().size(px(18.)).flex_none().path(icon).text_color(glyph))
        .child(
            div()
                .whitespace_nowrap()
                .text_size(px(12.))
                .text_color(ink)
                .child(label),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::diff_dock::model::DIFF_DOCK_PANEL_MIN_WIDTH;

    #[test]
    fn two_cards_fit_the_narrowest_dock() {
        let two_cards = 2. * CARD_WIDTH + CARD_GAP + 2. * GRID_PADDING;
        assert!(
            two_cards <= DIFF_DOCK_PANEL_MIN_WIDTH,
            "{two_cards}px of cards overflow a {DIFF_DOCK_PANEL_MIN_WIDTH}px dock"
        );
    }
}
