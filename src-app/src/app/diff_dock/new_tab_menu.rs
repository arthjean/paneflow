use std::rc::Rc;

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    deferred, div, px,
};

use crate::PaneFlowApp;
use crate::app::menu_icons as icon;
use crate::ui_primitives::action_menu::{ActionMenu, MenuHandlers, MenuItem};

#[derive(Clone, Copy)]
enum NewTabAction {
    Changes,
    File,
    Terminal,
}

impl PaneFlowApp {
    pub(crate) fn toggle_diff_new_tab_menu(&mut self, open: bool, cx: &mut Context<Self>) {
        self.diff_dock.diff_new_tab_menu_open = open;
        cx.notify();
    }

    pub(crate) fn close_diff_new_tab_menu(&mut self, cx: &mut Context<Self>) {
        if self.diff_dock.diff_new_tab_menu_open {
            self.diff_dock.diff_new_tab_menu_open = false;
            cx.notify();
        }
    }
}

pub(super) fn render_diff_new_tab_menu(
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let shortcut = |key: &str| {
        Some(SharedString::from(crate::keybindings::format_keystroke(
            key,
        )))
    };
    let menu = ActionMenu::new()
        .item(
            MenuItem::new("Changes", NewTabAction::Changes)
                .id("diff-dock-new-tab-changes")
                .icon(icon::COMPARE)
                .mnemonic('c'),
        )
        .item(
            MenuItem::new("File", NewTabAction::File)
                .id("diff-dock-new-tab-file")
                .icon(icon::FILE_CODE)
                .mnemonic('f')
                .shortcut(shortcut("secondary-g")),
        )
        .item(
            MenuItem::new("Terminal", NewTabAction::Terminal)
                .id("diff-dock-new-tab-terminal")
                .icon(icon::TERMINAL)
                .mnemonic('t')
                .shortcut(shortcut("secondary-j")),
        );
    let handlers = MenuHandlers {
        activate: Rc::new(cx.listener(|this, action: &NewTabAction, window, cx| {
            this.close_diff_new_tab_menu(cx);
            match action {
                NewTabAction::Changes => this.open_diff_changes_tab(cx),
                NewTabAction::File => this.open_diff_file_picker(window, cx),
                NewTabAction::Terminal => this.open_diff_terminal_tab(window, cx),
            }
        })),
        dismiss: Rc::new(cx.listener(|this, _: &(), _, cx| this.close_diff_new_tab_menu(cx))),
    };

    deferred(crate::ui_primitives::menu_reveal(
        "diff-dock-new-tab-menu-reveal",
        div()
            .absolute()
            .top(px(30.))
            .right(px(0.))
            .occlude()
            .child(menu.element("diff-dock-new-tab-menu", handlers, ui)),
    ))
    .with_priority(3)
    .into_any_element()
}
