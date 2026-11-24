use std::rc::Rc;

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Styled, deferred, div, prelude::FluentBuilder, px, svg,
};

use super::model::DiffChrome;
use crate::PaneFlowApp;
use crate::app::menu_icons as icon;
use crate::diff::{ComparisonPolicy, DiffOptions, HighlightPolicy};
use crate::ui_primitives::AccessibleControlExt as _;
use crate::ui_primitives::action_menu::{ActionMenu, MenuHandlers, MenuItem};
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};

struct OptionEntry {
    id: &'static str,
    label: &'static str,
    selected: bool,
    options: DiffOptions,
}

impl PaneFlowApp {
    fn set_diff_options_menu(&mut self, open: bool, cx: &mut Context<Self>) {
        self.diff_dock.diff_options_menu_open = open;
        cx.notify();
    }

    pub(crate) fn close_diff_options_menu(&mut self, cx: &mut Context<Self>) {
        if self.diff_dock.diff_options_menu_open {
            self.diff_dock.diff_options_menu_open = false;
            cx.notify();
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
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center(),
        "diff-dock-options-group",
        ROW_RADIUS,
        open.then_some(hover),
        Some(hover),
    )
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    .on_activate(cx.listener(move |this, _: &ClickEvent, _w, cx| {
        this.set_diff_options_menu(!open, cx);
    }))
    .child(
        svg()
            .size(px(14.))
            .flex_none()
            .path("icons/dock/more-horizontal.svg")
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
    let handlers = MenuHandlers {
        activate: Rc::new(cx.listener(|this, options: &DiffOptions, _, cx| {
            this.set_diff_dock_options(*options, cx);
        })),
        dismiss: Rc::new(cx.listener(|this, _: &(), _, cx| this.close_diff_options_menu(cx))),
    };
    let menu = diff_options_menu(chrome.options);

    deferred(crate::ui_primitives::menu_reveal(
        "diff-options-menu-reveal",
        div()
            .absolute()
            .top(px(28.))
            .right(px(0.))
            .occlude()
            .child(menu.element("diff-options-menu", handlers, ui)),
    ))
    .with_priority(3)
    .into_any_element()
}

fn diff_options_menu(options: DiffOptions) -> ActionMenu<DiffOptions> {
    ActionMenu::new()
        .item(
            MenuItem::submenu("Highlight", choices(highlight_entries(options)))
                .id("diff-options-highlight")
                .icon(icon::HIGHLIGHT)
                .detail(highlight_label(options.highlight))
                .mnemonic('h'),
        )
        .item(
            MenuItem::submenu("Whitespace", choices(whitespace_entries(options)))
                .id("diff-options-whitespace")
                .icon(icon::WHITESPACE)
                .detail(whitespace_label(options.whitespace))
                .mnemonic('w'),
        )
}

fn choices(entries: Vec<OptionEntry>) -> ActionMenu<DiffOptions> {
    ActionMenu::new().items(entries.into_iter().map(|entry| {
        let item = MenuItem::new(entry.label, entry.options)
            .id(entry.id)
            .radio(entry.selected)
            .keep_open();
        match entry.label.chars().next() {
            Some(initial) => item.mnemonic(initial),
            None => item,
        }
    }))
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
        options: DiffOptions {
            highlight,
            ..options
        },
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
        options: DiffOptions {
            whitespace,
            ..options
        },
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
            assert_eq!(
                entry.options.whitespace,
                ComparisonPolicy::IgnoreWhitespaces
            );
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
