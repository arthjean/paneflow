use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, MouseUpEvent,
    ParentElement, StatefulInteractiveElement, Styled, deferred, div, prelude::FluentBuilder, px,
    svg,
};

use super::model::{DiffChrome, DiffOptionsSubmenu};
use crate::PaneFlowApp;
use crate::diff::{ComparisonPolicy, DiffOptions, HighlightPolicy};
use crate::settings::components::{menu_divider_color, menu_surface, select_item};
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};

const MENU_WIDTH: f32 = 232.0;
const SUBMENU_WIDTH: f32 = 150.0;

#[derive(Clone, Copy)]
enum OptionChoice {
    Layout(bool),
    Diff(DiffOptions),
}

struct OptionEntry {
    id: &'static str,
    label: &'static str,
    selected: bool,
    choice: OptionChoice,
}

impl PaneFlowApp {
    fn set_diff_options_menu(&mut self, open: bool, cx: &mut Context<Self>) {
        self.diff_dock.diff_options_menu_open = open;
        if !open {
            self.diff_dock.diff_options_submenu = None;
        }
        cx.notify();
    }

    pub(crate) fn close_diff_options_menu(&mut self, cx: &mut Context<Self>) {
        if self.diff_dock.diff_options_menu_open {
            self.diff_dock.diff_options_menu_open = false;
            self.diff_dock.diff_options_submenu = None;
            cx.notify();
        }
    }

    fn toggle_diff_options_submenu(&mut self, submenu: DiffOptionsSubmenu, cx: &mut Context<Self>) {
        self.diff_dock.diff_options_submenu =
            if self.diff_dock.diff_options_submenu == Some(submenu) {
                None
            } else {
                Some(submenu)
            };
        cx.notify();
    }

    fn apply_diff_option_choice(&mut self, choice: OptionChoice, cx: &mut Context<Self>) {
        match choice {
            OptionChoice::Layout(split) => self.set_diff_dock_split(split, cx),
            OptionChoice::Diff(options) => self.set_diff_dock_options(options, cx),
        }
        self.close_diff_options_menu(cx);
    }
}

pub(super) fn render_diff_options_button(
    chrome: &DiffChrome<'_>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let open = chrome.options_open;
    let hover = crate::app::constants::sidebar_tab_hover_background();

    squircle_skin(
        div()
            .id("diff-dock-options")
            .flex_none()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center(),
        "diff-dock-options-group",
        ROW_RADIUS,
        open.then_some(hover),
        Some(hover),
    )
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
        this.set_diff_options_menu(!open, cx);
    }))
    .child(
        svg()
            .size(px(16.))
            .flex_none()
            .path("icons/dots.svg")
            .text_color(ui.muted),
    )
    .when(open, |trigger| {
        trigger.child(render_diff_options_menu(chrome, ui, cx))
    })
    .into_any_element()
}

fn layout_entries(split: bool) -> Vec<OptionEntry> {
    vec![
        OptionEntry {
            id: "diff-dock-layout-split",
            label: "Split",
            selected: split,
            choice: OptionChoice::Layout(true),
        },
        OptionEntry {
            id: "diff-dock-layout-unified",
            label: "Unified",
            selected: !split,
            choice: OptionChoice::Layout(false),
        },
    ]
}

fn highlight_entries(options: DiffOptions) -> Vec<OptionEntry> {
    [
        ("diff-dock-highlight-words", "Words", HighlightPolicy::Words),
        ("diff-dock-highlight-lines", "Lines", HighlightPolicy::Lines),
        ("diff-dock-highlight-none", "None", HighlightPolicy::None),
    ]
    .into_iter()
    .map(|(id, label, highlight)| OptionEntry {
        id,
        label,
        selected: options.highlight == highlight,
        choice: OptionChoice::Diff(DiffOptions {
            highlight,
            ..options
        }),
    })
    .collect()
}

fn whitespace_entries(options: DiffOptions) -> Vec<OptionEntry> {
    [
        (
            "diff-dock-whitespace-default",
            "Default",
            ComparisonPolicy::Default,
        ),
        (
            "diff-dock-whitespace-trim",
            "Trim",
            ComparisonPolicy::TrimWhitespaces,
        ),
        (
            "diff-dock-whitespace-ignore",
            "Ignore",
            ComparisonPolicy::IgnoreWhitespaces,
        ),
    ]
    .into_iter()
    .map(|(id, label, whitespace)| OptionEntry {
        id,
        label,
        selected: options.whitespace == whitespace,
        choice: OptionChoice::Diff(DiffOptions {
            whitespace,
            ..options
        }),
    })
    .collect()
}

fn highlight_label(highlight: HighlightPolicy) -> &'static str {
    match highlight {
        HighlightPolicy::Words => "Words",
        HighlightPolicy::Lines => "Lines",
        HighlightPolicy::None => "None",
    }
}

fn whitespace_label(whitespace: ComparisonPolicy) -> &'static str {
    match whitespace {
        ComparisonPolicy::Default => "Default",
        ComparisonPolicy::TrimWhitespaces => "Trim",
        ComparisonPolicy::IgnoreWhitespaces => "Ignore",
    }
}

fn render_diff_options_menu(
    chrome: &DiffChrome<'_>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let loaded = chrome
        .data
        .as_ref()
        .filter(|d| d.has_rows() && d.file_count > 0);
    let submenu = chrome.options_submenu;
    let options = chrome.options;
    let cwd = chrome.cwd.clone();

    let mut menu = menu_surface(div().id("diff-dock-options-menu"), ui)
        .flex()
        .flex_col()
        .gap(px(1.))
        .p(px(4.))
        .w(px(MENU_WIDTH))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                this.close_diff_options_menu(cx);
            }),
        )
        .child(render_submenu_row(
            DiffOptionsSubmenu::Layout,
            "Layout",
            if chrome.split { "Split" } else { "Unified" },
            submenu,
            layout_entries(chrome.split),
            ui,
            cx,
        ))
        .child(render_submenu_row(
            DiffOptionsSubmenu::Highlight,
            "Highlight",
            highlight_label(options.highlight),
            submenu,
            highlight_entries(options),
            ui,
            cx,
        ))
        .child(render_submenu_row(
            DiffOptionsSubmenu::Whitespace,
            "Whitespace",
            whitespace_label(options.whitespace),
            submenu,
            whitespace_entries(options),
            ui,
            cx,
        ));

    menu = menu.child(
        div()
            .flex_none()
            .h(px(1.))
            .my(px(4.))
            .mx(px(4.))
            .bg(menu_divider_color(ui)),
    );

    if let Some(data) = loaded {
        let all_collapsed = data.all_collapsed(chrome.collapsed);
        let label = if all_collapsed {
            "Expand All"
        } else {
            "Collapse All"
        };
        let next_collapse = !all_collapsed;
        let paths = data.paths();
        menu = menu.child(
            select_item("diff-dock-options-collapse", false, ui)
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.set_all_diff_collapsed(&paths, next_collapse, cx);
                    this.close_diff_options_menu(cx);
                }))
                .child(menu_label(label, ui)),
        );
    }

    menu = menu.child(
        select_item("diff-dock-options-refresh", false, ui)
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.refresh_diff_dock(cwd.clone(), cx);
                this.close_diff_options_menu(cx);
            }))
            .child(menu_label("Refresh Changes", ui)),
    );

    deferred(
        div()
            .absolute()
            .top(px(32.))
            .right(px(0.))
            .occlude()
            .child(menu),
    )
    .with_priority(3)
    .into_any_element()
}

fn submenu_ids(submenu: DiffOptionsSubmenu) -> (&'static str, &'static str) {
    match submenu {
        DiffOptionsSubmenu::Layout => ("diff-dock-options-layout", "diff-dock-layout-submenu"),
        DiffOptionsSubmenu::Highlight => {
            ("diff-dock-options-highlight", "diff-dock-highlight-submenu")
        }
        DiffOptionsSubmenu::Whitespace => (
            "diff-dock-options-whitespace",
            "diff-dock-whitespace-submenu",
        ),
    }
}

fn render_submenu_row(
    submenu: DiffOptionsSubmenu,
    label: &'static str,
    value: &'static str,
    open_submenu: Option<DiffOptionsSubmenu>,
    entries: Vec<OptionEntry>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let open = open_submenu == Some(submenu);
    let (row_id, submenu_id) = submenu_ids(submenu);

    select_item(row_id, open, ui)
        .relative()
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.toggle_diff_options_submenu(submenu, cx);
        }))
        .child(menu_label(label, ui))
        .child(
            div()
                .flex_none()
                .text_size(px(12.))
                .text_color(ui.muted)
                .child(value),
        )
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path("icons/chevron-right.svg")
                .text_color(ui.muted),
        )
        .when(open, |row| {
            row.child(render_submenu(submenu_id, entries, ui, cx))
        })
        .into_any_element()
}

fn render_submenu(
    id: &'static str,
    entries: Vec<OptionEntry>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let mut menu = menu_surface(div().id(id), ui)
        .flex()
        .flex_col()
        .gap(px(1.))
        .p(px(4.))
        .w(px(SUBMENU_WIDTH))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for entry in entries {
        menu = menu.child(render_option_entry(entry, ui, cx));
    }

    deferred(
        div()
            .absolute()
            .top(px(-5.))
            .right(px(MENU_WIDTH - 12.))
            .occlude()
            .child(menu),
    )
    .with_priority(4)
    .into_any_element()
}

fn render_option_entry(
    entry: OptionEntry,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let choice = entry.choice;
    select_item(entry.id, entry.selected, ui)
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.apply_diff_option_choice(choice, cx);
        }))
        .child(menu_label(entry.label, ui))
        .child(div().w(px(14.)).flex_none().child(if entry.selected {
            svg()
                .size(px(13.))
                .path("icons/check.svg")
                .text_color(ui.text)
                .into_any_element()
        } else {
            div().size(px(13.)).into_any_element()
        }))
        .into_any_element()
}

fn menu_label(label: &'static str, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .whitespace_nowrap()
        .text_size(px(13.))
        .text_color(ui.text)
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_entries_check_the_active_policy_and_keep_whitespace() {
        let options = DiffOptions {
            highlight: HighlightPolicy::Lines,
            whitespace: ComparisonPolicy::IgnoreWhitespaces,
        };
        let entries = highlight_entries(options);
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries
                .iter()
                .filter(|e| e.selected)
                .map(|e| e.label)
                .collect::<Vec<_>>(),
            vec!["Lines"]
        );
        for entry in &entries {
            match entry.choice {
                OptionChoice::Diff(next) => {
                    assert_eq!(next.whitespace, ComparisonPolicy::IgnoreWhitespaces);
                }
                OptionChoice::Layout(_) => panic!("highlight entries never change layout"),
            }
        }
    }

    #[test]
    fn default_options_check_words_and_default() {
        let options = DiffOptions::default();
        let highlight: Vec<&str> = highlight_entries(options)
            .iter()
            .filter(|e| e.selected)
            .map(|e| e.label)
            .collect();
        let whitespace: Vec<&str> = whitespace_entries(options)
            .iter()
            .filter(|e| e.selected)
            .map(|e| e.label)
            .collect();
        assert_eq!(highlight, vec!["Words"]);
        assert_eq!(whitespace, vec!["Default"]);
        assert_eq!(
            whitespace_entries(options)
                .iter()
                .map(|e| e.label)
                .collect::<Vec<_>>(),
            vec!["Default", "Trim", "Ignore"]
        );
    }
}
