use std::path::PathBuf;

use gpui::{AnyElement, ClipboardItem, Context, Pixels, Point, SharedString, Window};

use super::model::DiffDockTab;
use super::tabs::TabBatch;
use crate::PaneFlowApp;
use crate::app::agent_send::{AgentTarget, send_to_agent_item};
use crate::app::files_tree::workspace_relative_path;
use crate::app::menu_icons as icon;
use crate::ui_primitives::action_menu::{ActionMenu, MenuItem};

pub(crate) const REOPEN_CLOSED_TAB: &str = "Reopen Closed Tab";

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DiffTabMenu {
    pub(crate) target: Option<usize>,
    pub(crate) position: Point<Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TabMenuAction {
    CopyPath,
    CopyRelativePath,
    SendToAgent(u64),
    Close,
    CloseOthers,
    CloseToTheRight,
    CloseAll,
    Reopen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TabMenuTarget {
    File,
    Other,
    Strip,
}

pub(super) fn tab_menu_items(
    target: TabMenuTarget,
    can_reopen: bool,
    agents: &[AgentTarget],
) -> ActionMenu<TabMenuAction> {
    let mut menu = ActionMenu::new();
    if target == TabMenuTarget::File {
        menu = menu
            .item(
                MenuItem::new("Copy Path", TabMenuAction::CopyPath)
                    .icon(icon::COPY)
                    .mnemonic('p'),
            )
            .item(
                MenuItem::new("Copy Relative Path", TabMenuAction::CopyRelativePath)
                    .icon(icon::LINK)
                    .mnemonic('r'),
            )
            .item(send_to_agent_item(agents, TabMenuAction::SendToAgent))
            .separator();
    }
    if target != TabMenuTarget::Strip {
        menu = menu
            .item(
                MenuItem::new("Close", TabMenuAction::Close)
                    .icon(icon::CLOSE)
                    .mnemonic('c'),
            )
            .item(MenuItem::new("Close Others", TabMenuAction::CloseOthers).mnemonic('o'))
            .item(
                MenuItem::new("Close to the Right", TabMenuAction::CloseToTheRight).mnemonic('t'),
            );
    }
    menu = menu
        .item(MenuItem::new("Close All", TabMenuAction::CloseAll).mnemonic('a'))
        .separator();
    menu.item(reopen_item(can_reopen, TabMenuAction::Reopen))
}

pub(super) fn reopen_item<A>(can_reopen: bool, action: A) -> MenuItem<A> {
    let item = if can_reopen {
        MenuItem::new(REOPEN_CLOSED_TAB, action)
    } else {
        MenuItem::disabled(REOPEN_CLOSED_TAB)
    };
    item.icon(icon::REFRESH).mnemonic('e')
}

impl PaneFlowApp {
    pub(crate) fn open_diff_tab_menu(
        &mut self,
        target: Option<usize>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_transient_surfaces();
        self.close_diff_new_tab_menu(cx);
        self.diff_dock.crumb_menu = None;
        self.diff_dock.tab_menu = Some(DiffTabMenu { target, position });
        cx.notify();
    }

    pub(crate) fn diff_tab_root(&self) -> PathBuf {
        self.diff_dock
            .data
            .as_ref()
            .map(|data| data.cwd.clone())
            .filter(|cwd| !cwd.is_empty())
            .or_else(|| self.active_workspace().map(|ws| ws.cwd.clone()))
            .map(PathBuf::from)
            .unwrap_or_default()
    }

    pub(crate) fn render_diff_tab_menu(
        &self,
        menu: DiffTabMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = menu
            .target
            .and_then(|index| match self.diff_dock.diff_tabs.get(index) {
                Some(DiffDockTab::File(view)) => Some(view.read(cx).path().to_path_buf()),
                _ => None,
            });
        let target = match (menu.target, &path) {
            (Some(_), Some(_)) => TabMenuTarget::File,
            (Some(_), None) => TabMenuTarget::Other,
            (None, _) => TabMenuTarget::Strip,
        };
        let actions = tab_menu_items(
            target,
            !self.diff_dock.closed_files.is_empty(),
            &self.agent_targets(cx),
        );
        let root = self.diff_tab_root();
        let index = menu.target;
        let handlers = self.menu_handlers(
            cx,
            move |this, action: &TabMenuAction, window, cx| {
                this.diff_dock.tab_menu = None;
                match (action, index) {
                    (TabMenuAction::CopyPath, _) => {
                        if let Some(path) = &path {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                path.to_string_lossy().into_owned(),
                            ));
                            this.show_toast("Copied path", cx);
                        }
                    }
                    (TabMenuAction::CopyRelativePath, _) => {
                        if let Some(path) = &path {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                workspace_relative_path(&root, path),
                            ));
                            this.show_toast("Copied relative path", cx);
                        }
                    }
                    (TabMenuAction::SendToAgent(surface_id), _) => {
                        if let Some(path) = &path {
                            this.send_file_to_agent(*surface_id, &root, path, window, cx);
                        }
                    }
                    (TabMenuAction::Close, Some(index)) => {
                        this.request_close_diff_tab(index, window, cx)
                    }
                    (TabMenuAction::CloseOthers, Some(index)) => {
                        this.close_diff_tab_batch(TabBatch::Others(index), window, cx)
                    }
                    (TabMenuAction::CloseToTheRight, Some(index)) => {
                        this.close_diff_tab_batch(TabBatch::ToTheRight(index), window, cx)
                    }
                    (TabMenuAction::CloseAll, _) => {
                        this.close_diff_tab_batch(TabBatch::All, window, cx)
                    }
                    (TabMenuAction::Reopen, _) => this.reopen_closed_diff_tab(window, cx),
                    _ => {}
                }
            },
            |this| this.diff_dock.tab_menu = None,
        );
        crate::app::sidebar::context_menu::render_context_menu(
            SharedString::from("diff-dock-tab-menu"),
            actions,
            menu.position,
            handlers,
            window,
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_primitives::action_menu::MenuEntry;

    fn labels(menu: &ActionMenu<TabMenuAction>) -> Vec<String> {
        menu.entries()
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(item.label_for_test().to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_file_tab_menu_lists_the_copy_and_close_actions() {
        let menu = tab_menu_items(TabMenuTarget::File, true, &[]);
        assert_eq!(
            labels(&menu),
            [
                "Copy Path",
                "Copy Relative Path",
                crate::app::agent_send::NO_AGENT,
                "Close",
                "Close Others",
                "Close to the Right",
                "Close All",
                REOPEN_CLOSED_TAB,
            ]
        );
    }

    #[test]
    fn reopening_is_disabled_while_the_history_is_empty() {
        let menu = tab_menu_items(TabMenuTarget::Strip, false, &[]);
        assert_eq!(labels(&menu), ["Close All", REOPEN_CLOSED_TAB]);
        let last = menu.entries().len() - 1;
        assert!(!menu.enabled_at_for_test(last));
        let menu = tab_menu_items(TabMenuTarget::Strip, true, &[]);
        assert!(menu.enabled_at_for_test(menu.entries().len() - 1));
        assert_eq!(
            menu.action_at(menu.entries().len() - 1),
            Some(TabMenuAction::Reopen)
        );
    }

    #[test]
    fn a_non_file_tab_has_no_path_to_copy() {
        let menu = tab_menu_items(TabMenuTarget::Other, false, &[]);
        assert_eq!(labels(&menu)[0], "Close");
        assert!(!labels(&menu).iter().any(|label| label.starts_with("Copy")));
    }
}
