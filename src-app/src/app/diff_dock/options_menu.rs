use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton,
    MouseUpEvent, ParentElement, Pixels, StatefulInteractiveElement, Styled, canvas, deferred, div,
    prelude::FluentBuilder, px, svg,
};

use super::model::{DiffChrome, DiffOptionsSubmenu};
use crate::PaneFlowApp;
use crate::diff::{ComparisonPolicy, DiffOptions, HighlightPolicy};
use crate::settings::components::{menu_divider_color, menu_surface, select_item};
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};

const MENU_WIDTH: f32 = 232.0;
const SUBMENU_WIDTH: f32 = 150.0;

#[derive(Clone, Copy)]
pub(crate) enum OptionChoice {
    Layout(bool),
    Diff(DiffOptions),
}

#[derive(Clone, Copy)]
pub(crate) struct DiffOptionsMenuState {
    pub(crate) split: bool,
    pub(crate) options: DiffOptions,
    pub(crate) submenu: Option<DiffOptionsSubmenu>,
    pub(crate) all_collapsed: Option<bool>,
}

pub(crate) type MenuAction<T> = Rc<dyn Fn(T, &mut App)>;

type SubmenuBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

#[derive(Clone)]
pub(crate) struct DiffOptionsMenuActions {
    pub(crate) toggle_submenu: MenuAction<DiffOptionsSubmenu>,
    pub(crate) choose: MenuAction<OptionChoice>,
    pub(crate) set_all_collapsed: MenuAction<bool>,
    pub(crate) refresh: Rc<dyn Fn(&mut App)>,
    pub(crate) dismiss: Rc<dyn Fn(&mut App)>,
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
        trigger.child(render_dock_options_menu(chrome, ui, cx))
    })
    .into_any_element()
}

fn render_dock_options_menu(
    chrome: &DiffChrome<'_>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let loaded = chrome
        .data
        .as_ref()
        .filter(|d| d.has_rows() && d.file_count > 0);
    let paths = loaded.map(|data| data.paths()).unwrap_or_default();
    let cwd = chrome.cwd.clone();
    let app = cx.weak_entity();
    let state = DiffOptionsMenuState {
        split: chrome.split,
        options: chrome.options,
        submenu: chrome.options_submenu,
        all_collapsed: loaded.map(|data| data.all_collapsed(chrome.collapsed)),
    };
    let actions = DiffOptionsMenuActions {
        toggle_submenu: {
            let app = app.clone();
            Rc::new(move |submenu, cx| {
                let _ = app.update(cx, |this, cx| this.toggle_diff_options_submenu(submenu, cx));
            })
        },
        choose: {
            let app = app.clone();
            Rc::new(move |choice, cx| {
                let _ = app.update(cx, |this, cx| this.apply_diff_option_choice(choice, cx));
            })
        },
        set_all_collapsed: {
            let app = app.clone();
            Rc::new(move |collapse, cx| {
                let _ = app.update(cx, |this, cx| {
                    this.set_all_diff_collapsed(&paths, collapse, cx);
                });
            })
        },
        refresh: {
            let app = app.clone();
            Rc::new(move |cx| {
                let _ = app.update(cx, |this, cx| {
                    this.refresh_diff_dock(cwd.clone(), cx);
                });
            })
        },
        dismiss: Rc::new(move |cx| {
            let _ = app.update(cx, |this, cx| this.close_diff_options_menu(cx));
        }),
    };
    render_diff_options_menu(32., state, actions, ui)
}

fn layout_entries(split: bool) -> Vec<OptionEntry> {
    vec![
        OptionEntry {
            id: "diff-options-layout-split",
            label: "Split",
            selected: split,
            choice: OptionChoice::Layout(true),
        },
        OptionEntry {
            id: "diff-options-layout-unified",
            label: "Unified",
            selected: !split,
            choice: OptionChoice::Layout(false),
        },
    ]
}

fn highlight_entries(options: DiffOptions) -> Vec<OptionEntry> {
    [
        (
            "diff-options-highlight-words",
            "Words",
            HighlightPolicy::Words,
        ),
        (
            "diff-options-highlight-lines",
            "Lines",
            HighlightPolicy::Lines,
        ),
        ("diff-options-highlight-none", "None", HighlightPolicy::None),
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
            "diff-options-whitespace-default",
            "Default",
            ComparisonPolicy::Default,
        ),
        (
            "diff-options-whitespace-trim",
            "Trim",
            ComparisonPolicy::TrimWhitespaces,
        ),
        (
            "diff-options-whitespace-ignore",
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

pub(crate) fn render_diff_options_menu(
    top: f32,
    state: DiffOptionsMenuState,
    actions: DiffOptionsMenuActions,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let dismiss = actions.dismiss.clone();
    let submenu_bounds: SubmenuBounds = Rc::default();
    let dismiss_bounds = submenu_bounds.clone();
    let mut menu = menu_surface(div().id("diff-options-menu"), ui)
        .flex()
        .flex_col()
        .gap(px(1.))
        .p(px(4.))
        .w(px(MENU_WIDTH))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up_out(MouseButton::Left, move |event: &MouseUpEvent, _w, cx| {
            if dismiss_bounds
                .get()
                .is_some_and(|bounds| bounds.contains(&event.position))
            {
                return;
            }
            dismiss(cx);
        })
        .child(render_submenu_row(
            SubmenuRow {
                submenu: DiffOptionsSubmenu::Layout,
                label: "Layout",
                value: if state.split { "Split" } else { "Unified" },
                entries: layout_entries(state.split),
            },
            state.submenu,
            &actions,
            &submenu_bounds,
            ui,
        ))
        .child(render_submenu_row(
            SubmenuRow {
                submenu: DiffOptionsSubmenu::Highlight,
                label: "Highlight",
                value: highlight_label(state.options.highlight),
                entries: highlight_entries(state.options),
            },
            state.submenu,
            &actions,
            &submenu_bounds,
            ui,
        ))
        .child(render_submenu_row(
            SubmenuRow {
                submenu: DiffOptionsSubmenu::Whitespace,
                label: "Whitespace",
                value: whitespace_label(state.options.whitespace),
                entries: whitespace_entries(state.options),
            },
            state.submenu,
            &actions,
            &submenu_bounds,
            ui,
        ));

    menu = menu.child(
        div()
            .flex_none()
            .h(px(1.))
            .my(px(4.))
            .mx(px(4.))
            .bg(menu_divider_color(ui)),
    );

    if let Some(all_collapsed) = state.all_collapsed {
        let label = if all_collapsed {
            "Expand All"
        } else {
            "Collapse All"
        };
        let next_collapse = !all_collapsed;
        let set_all_collapsed = actions.set_all_collapsed.clone();
        menu = menu.child(
            select_item("diff-options-collapse", false, ui)
                .on_click(move |_: &ClickEvent, _w, cx| set_all_collapsed(next_collapse, cx))
                .child(menu_label(label, ui)),
        );
    }

    let refresh = actions.refresh.clone();
    menu = menu.child(
        select_item("diff-options-refresh", false, ui)
            .on_click(move |_: &ClickEvent, _w, cx| refresh(cx))
            .child(menu_label("Refresh Changes", ui)),
    );

    deferred(
        div()
            .absolute()
            .top(px(top))
            .right(px(0.))
            .occlude()
            .child(menu),
    )
    .with_priority(3)
    .into_any_element()
}

fn submenu_ids(submenu: DiffOptionsSubmenu) -> (&'static str, &'static str) {
    match submenu {
        DiffOptionsSubmenu::Layout => ("diff-options-layout", "diff-options-layout-submenu"),
        DiffOptionsSubmenu::Highlight => {
            ("diff-options-highlight", "diff-options-highlight-submenu")
        }
        DiffOptionsSubmenu::Whitespace => {
            ("diff-options-whitespace", "diff-options-whitespace-submenu")
        }
    }
}

struct SubmenuRow {
    submenu: DiffOptionsSubmenu,
    label: &'static str,
    value: &'static str,
    entries: Vec<OptionEntry>,
}

fn render_submenu_row(
    row: SubmenuRow,
    open_submenu: Option<DiffOptionsSubmenu>,
    actions: &DiffOptionsMenuActions,
    submenu_bounds: &SubmenuBounds,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let SubmenuRow {
        submenu,
        label,
        value,
        entries,
    } = row;
    let open = open_submenu == Some(submenu);
    let (row_id, submenu_id) = submenu_ids(submenu);
    let toggle_submenu = actions.toggle_submenu.clone();

    select_item(row_id, open, ui)
        .relative()
        .on_click(move |_: &ClickEvent, _w, cx| toggle_submenu(submenu, cx))
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
            row.child(render_submenu(
                submenu_id,
                entries,
                &actions.choose,
                submenu_bounds.clone(),
                ui,
            ))
        })
        .into_any_element()
}

fn render_submenu(
    id: &'static str,
    entries: Vec<OptionEntry>,
    choose: &MenuAction<OptionChoice>,
    submenu_bounds: SubmenuBounds,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let mut menu = menu_surface(div().id(id), ui)
        .flex()
        .flex_col()
        .gap(px(1.))
        .p(px(4.))
        .w(px(SUBMENU_WIDTH))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            canvas(
                move |bounds, _, _| submenu_bounds.set(Some(bounds)),
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
    for entry in entries {
        menu = menu.child(render_option_entry(entry, choose.clone(), ui));
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
    choose: MenuAction<OptionChoice>,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let choice = entry.choice;
    select_item(entry.id, entry.selected, ui)
        .on_click(move |_: &ClickEvent, _w, cx| choose(choice, cx))
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
