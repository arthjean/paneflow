//! The side dock's surface picker: what the dock shows the first time it is
//! opened, before the user has said what they want in it.
//!
//! The pane header's dock button used to drop straight into the git diff, which
//! made "open the side panel" and "review changes" the same gesture even when
//! the user wanted a shell or a file. The picker separates the two: opening the
//! dock asks, and the answer is remembered for the workspace that gave it, so
//! only that project's first open costs a click (Cursor / Codex both behave this
//! way). The dock is detached per workspace ([`crate::app::cli_diff_dock`]), so
//! the next project starts from the same question rather than inheriting an
//! answer given for another repository.
//!
//! Three surfaces, matching the tabs the dock can actually host
//! ([`super::model::DiffDockTab`]): Changes, Terminal, File. Nothing here opens
//! a surface itself - each card routes to the exact entry point the tab strip's
//! `+` menu uses, so the two doors into the dock cannot drift apart.

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px, svg,
};

use super::render::render_diff_header_icon_button;
use crate::PaneFlowApp;
use crate::settings::components::with_alpha;
use crate::ui_primitives::squircle::squircle_border;
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};

/// Card side. Two cards plus their gap must clear the dock's minimum width
/// (`AGENTS_DIFF_PANEL_MIN_WIDTH`, 360 px) with the grid's own padding, which is
/// what the guard test below pins.
const CARD_WIDTH: f32 = 108.0;
const CARD_HEIGHT: f32 = 92.0;
const CARD_GAP: f32 = 12.0;
/// Padding around the wrapping grid, so a card never touches the dock edge.
const GRID_PADDING: f32 = 16.0;

/// One surface the dock can be opened onto.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DiffDockSurface {
    Changes,
    Terminal,
    File,
}

impl PaneFlowApp {
    /// Answer the picker: dismiss it, remember that this workspace has chosen
    /// once (so its later opens restore the last tab instead of asking again),
    /// then route to the surface.
    pub(crate) fn choose_diff_dock_surface(
        &mut self,
        surface: DiffDockSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agents_view.diff_dock_picker = false;
        self.agents_view.diff_dock_picked = true;
        match surface {
            // `Changes` is the permanent tab 0, so choosing it opens nothing -
            // it just selects the tab the dismissed picker was covering.
            DiffDockSurface::Changes => self.select_diff_tab(0, cx),
            DiffDockSurface::Terminal => self.open_diff_terminal_tab(window, cx),
            // Same as the `+` menu's File row: the Files tree is the picker, and
            // a row there opens the document as a dock tab.
            DiffDockSurface::File => self.open_diff_file_picker(window, cx),
        }
        cx.notify();
    }
}

/// The picker's header: the same 40 px band the tab strip occupies, carrying
/// only the dock's close button. Without it the picker would be a dead end -
/// there is no tab strip to close the dock from while it is up.
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
            "agents-diff-picker-close",
            "icons/close.svg",
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.close_agents_diff_panel(cx);
            }),
            ui.muted,
        ))
        .into_any_element()
}

/// The card grid, centered in the dock body. Wraps rather than fixing a column
/// count: at the dock's minimum width two cards fit per row, and a widened dock
/// puts all three on one line.
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
                    "agents-diff-picker-changes",
                    "icons/git-pull-request.svg",
                    "Changes",
                    DiffDockSurface::Changes,
                    ui,
                    cx,
                ))
                .child(card(
                    "agents-diff-picker-terminal",
                    "icons/terminal.svg",
                    "Terminal",
                    DiffDockSurface::Terminal,
                    ui,
                    cx,
                ))
                .child(card(
                    "agents-diff-picker-file",
                    "icons/file-text.svg",
                    "File",
                    DiffDockSurface::File,
                    ui,
                    cx,
                )),
        )
        .into_any_element()
}

/// One card: glyph over label, on the app's superellipse skin (a resting tint
/// plus a hover tint) with a hairline over the top - the same silhouette the
/// dock itself and the panes are drawn with.
fn card(
    id: &'static str,
    icon: &'static str,
    label: &'static str,
    surface: DiffDockSurface,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    squircle_skin(
        div()
            .id(id)
            .flex_none()
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .cursor(gpui::CursorStyle::PointingHand),
        SharedString::from(format!("{id}-group")),
        ROW_RADIUS,
        Some(with_alpha(ui.text, 0.03)),
        Some(with_alpha(ui.text, 0.07)),
    )
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
        this.choose_diff_dock_surface(surface, window, cx);
    }))
    .child(
        svg()
            .size(px(18.))
            .flex_none()
            .path(icon)
            .text_color(ui.muted),
    )
    .child(
        div()
            .whitespace_nowrap()
            .text_size(px(12.))
            .text_color(ui.text)
            .child(label),
    )
    .child(squircle_border(ROW_RADIUS, px(1.), ui.border))
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agents_diff::model::AGENTS_DIFF_PANEL_MIN_WIDTH;

    #[test]
    fn two_cards_fit_the_narrowest_dock() {
        // The grid wraps, so the layout only holds while at least two cards
        // still fit the dock at its minimum width. One card per row would read
        // as a list that forgot its labels, and a card clipped by the dock edge
        // would swallow half its own hit target.
        let two_cards = 2. * CARD_WIDTH + CARD_GAP + 2. * GRID_PADDING;
        assert!(
            two_cards <= AGENTS_DIFF_PANEL_MIN_WIDTH,
            "{two_cards}px of cards overflow a {AGENTS_DIFF_PANEL_MIN_WIDTH}px dock"
        );
    }
}
