use gpui::{
    AnyElement, ClickEvent, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, StatefulInteractiveElement, Styled, Window, div, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::{keycap, keycap_tones, select_item};

const LIST_MAX_WIDTH: f32 = 320.0;
const LIST_PADDING: f32 = 24.0;
const ROW_HEIGHT: f32 = 32.0;
const ROW_GAP: f32 = 2.0;
const ROW_CONTENT_GAP: f32 = 10.0;
const ROW_ICON_SIZE: f32 = 16.0;
const TITLE_GAP: f32 = 12.0;
const HEADER_HEIGHT: f32 = 40.0;
const PICKER_TITLE: &str = "Open a surface";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DiffDockSurface {
    Changes,
    Terminal,
    File,
    Browser,
}

impl DiffDockSurface {
    const ALL: [Self; 4] = [Self::Changes, Self::Terminal, Self::File, Self::Browser];

    fn id(self) -> &'static str {
        match self {
            Self::Changes => "diff-dock-picker-changes",
            Self::Terminal => "diff-dock-picker-terminal",
            Self::File => "diff-dock-picker-file",
            Self::Browser => "diff-dock-picker-browser",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Changes => "Changes",
            Self::Terminal => "Terminal",
            Self::File => "File",
            Self::Browser => "Browser",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Changes => "icons/dock/git-compare.svg",
            Self::Terminal => "icons/dock/computer-terminal-01.svg",
            Self::File => "icons/dock/file-code.svg",
            Self::Browser => "icons/world.svg",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Changes => "c",
            Self::Terminal => "t",
            Self::File => "f",
            Self::Browser => "b",
        }
    }

    fn keycap(self) -> &'static str {
        match self {
            Self::Changes => "C",
            Self::Terminal => "T",
            Self::File => "F",
            Self::Browser => "B",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PickerKey {
    Open(DiffDockSurface),
    Highlight(usize),
    Dismiss,
    Ignore,
}

fn picker_key(key: &str, highlighted: Option<usize>) -> PickerKey {
    let count = DiffDockSurface::ALL.len();
    match key {
        "escape" => PickerKey::Dismiss,
        "down" => PickerKey::Highlight(highlighted.map_or(0, |index| (index + 1) % count)),
        "up" => {
            PickerKey::Highlight(highlighted.map_or(count - 1, |index| (index + count - 1) % count))
        }
        "home" => PickerKey::Highlight(0),
        "end" => PickerKey::Highlight(count - 1),
        "enter" | "space" => highlighted
            .and_then(|index| DiffDockSurface::ALL.get(index).copied())
            .map_or(PickerKey::Ignore, PickerKey::Open),
        letter => DiffDockSurface::ALL
            .into_iter()
            .find(|surface| surface.key() == letter)
            .map_or(PickerKey::Ignore, PickerKey::Open),
    }
}

impl PaneFlowApp {
    pub(crate) fn diff_dock_picker_shown(&self) -> bool {
        self.diff_dock_visible() && (self.diff_dock.picker || self.diff_dock.diff_tabs.is_empty())
    }

    pub(crate) fn choose_diff_dock_surface(
        &mut self,
        surface: DiffDockSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.diff_dock.picker_highlight = None;
        match surface {
            DiffDockSurface::Changes => self.open_diff_changes_tab(cx),
            DiffDockSurface::Terminal => self.open_diff_terminal_tab(window, cx),
            DiffDockSurface::File => self.open_diff_file_picker(window, cx),
            DiffDockSurface::Browser => self.open_diff_browser_tab(window, cx),
        }
    }

    fn handle_diff_dock_picker_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return;
        }
        match picker_key(
            event.keystroke.key.as_str(),
            self.diff_dock.picker_highlight,
        ) {
            PickerKey::Open(surface) => self.choose_diff_dock_surface(surface, window, cx),
            PickerKey::Highlight(index) => {
                self.diff_dock.picker_highlight = Some(index);
                cx.notify();
            }
            PickerKey::Dismiss => self.dismiss_diff_dock_panel(cx),
            PickerKey::Ignore => return,
        }
        cx.stop_propagation();
    }

    pub(crate) fn settle_diff_dock_picker_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = self.diff_dock_picker_shown();
        if std::mem::take(&mut self.diff_dock.picker_focus_pending) && shown {
            self.diff_dock
                .picker_focus_return
                .get_or_insert_with(Default::default)
                .capture_once(window, cx);
            window.focus(&self.diff_dock.picker_focus, cx);
            return;
        }
        let focused = self.diff_dock.picker_focus.is_focused(window);
        if shown {
            if !focused && self.diff_dock.picker_highlight.take().is_some() {
                cx.notify();
            }
            return;
        }
        self.diff_dock.picker_highlight = None;
        if let Some(origin) = self.diff_dock.picker_focus_return.take()
            && focused
        {
            self.return_focus(&origin, window, cx);
        }
    }
}

pub(super) fn render_diff_picker_header() -> AnyElement {
    div().h(px(HEADER_HEIGHT)).flex_none().into_any_element()
}

pub(super) fn render_diff_surface_picker(
    highlighted: Option<usize>,
    focus: &FocusHandle,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let (cap_fill, cap_edge, _) = keycap_tones(ui);
    let rows = DiffDockSurface::ALL
        .into_iter()
        .enumerate()
        .map(|(index, surface)| {
            select_item(surface.id(), highlighted == Some(index), ui)
                .w_full()
                .h(px(ROW_HEIGHT))
                .gap(px(ROW_CONTENT_GAP))
                .role(gpui::accesskit::Role::Button)
                .aria_label(surface.label())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.choose_diff_dock_surface(surface, window, cx);
                }))
                .child(
                    svg()
                        .size(px(ROW_ICON_SIZE))
                        .flex_none()
                        .path(surface.icon())
                        .text_color(ui.muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ui.text)
                        .child(surface.label()),
                )
                .child(keycap(surface.keycap(), cap_fill, cap_edge, ui.muted).flex_none())
        })
        .collect::<Vec<_>>();

    div()
        .id("diff-dock-picker")
        .track_focus(focus)
        .on_key_down(cx.listener(PaneFlowApp::handle_diff_dock_picker_key_down))
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .px(px(LIST_PADDING))
        .pb(px(HEADER_HEIGHT))
        .child(
            div()
                .w_full()
                .max_w(px(LIST_MAX_WIDTH))
                .flex()
                .flex_col()
                .child(
                    div()
                        .pb(px(TITLE_GAP))
                        .flex()
                        .justify_center()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(ui.text)
                        .child(PICKER_TITLE),
                )
                .child(div().flex().flex_col().gap(px(ROW_GAP)).children(rows)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::diff_dock::model::{DIFF_DOCK_PANEL_MIN_WIDTH, DIFF_DOCK_PANEL_WIDTH};

    #[test]
    fn each_surface_opens_on_its_own_letter() {
        assert_eq!(
            picker_key("c", None),
            PickerKey::Open(DiffDockSurface::Changes)
        );
        assert_eq!(
            picker_key("t", Some(0)),
            PickerKey::Open(DiffDockSurface::Terminal)
        );
        assert_eq!(
            picker_key("f", None),
            PickerKey::Open(DiffDockSurface::File)
        );
        assert_eq!(
            picker_key("b", None),
            PickerKey::Open(DiffDockSurface::Browser)
        );
        assert_eq!(picker_key("x", None), PickerKey::Ignore);
    }

    #[test]
    fn every_keycap_names_the_key_that_opens_its_row() {
        for surface in DiffDockSurface::ALL {
            assert_eq!(surface.keycap().to_lowercase(), surface.key());
            assert_eq!(picker_key(surface.key(), None), PickerKey::Open(surface));
        }
    }

    #[test]
    fn the_arrows_wrap_through_the_rows() {
        assert_eq!(picker_key("down", None), PickerKey::Highlight(0));
        assert_eq!(picker_key("down", Some(3)), PickerKey::Highlight(0));
        assert_eq!(picker_key("up", None), PickerKey::Highlight(3));
        assert_eq!(picker_key("up", Some(0)), PickerKey::Highlight(3));
        assert_eq!(picker_key("home", Some(2)), PickerKey::Highlight(0));
        assert_eq!(picker_key("end", Some(0)), PickerKey::Highlight(3));
    }

    #[test]
    fn enter_opens_the_highlighted_row_only() {
        assert_eq!(picker_key("enter", None), PickerKey::Ignore);
        assert_eq!(
            picker_key("enter", Some(1)),
            PickerKey::Open(DiffDockSurface::Terminal)
        );
        assert_eq!(
            picker_key("space", Some(2)),
            PickerKey::Open(DiffDockSurface::File)
        );
        assert_eq!(picker_key("escape", Some(1)), PickerKey::Dismiss);
    }

    #[test]
    fn the_list_keeps_its_full_width_in_a_default_dock_and_fits_the_narrowest() {
        let full_list = LIST_MAX_WIDTH + 2. * LIST_PADDING;
        assert!(
            full_list <= DIFF_DOCK_PANEL_WIDTH,
            "{full_list}px of list overflow a {DIFF_DOCK_PANEL_WIDTH}px dock"
        );
        let narrowest_list = DIFF_DOCK_PANEL_MIN_WIDTH - 2. * LIST_PADDING;
        assert!(
            narrowest_list >= 240.,
            "{narrowest_list}px cannot hold a label and its keycap"
        );
    }
}
