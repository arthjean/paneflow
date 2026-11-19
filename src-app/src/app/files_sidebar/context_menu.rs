use gpui::{AnyElement, ClipboardItem, Context, SharedString};

use super::open_with::{self, OPEN_WITH_DEFAULT_LABEL, OPEN_WITH_LABEL};
use crate::app::agent_send::{AgentTarget, send_to_agent_item};
use crate::app::files_tree;
use crate::app::menu_icons as icon;
use crate::app::sidebar::context_menu::{EditorMenuEntry, installed_editors, render_context_menu};
use crate::app::window::FilesMenuScope;
use crate::ui_primitives::action_menu::{ActionMenu, MenuItem};
use crate::{FilesContextMenu, PaneFlowApp};

#[derive(Clone, Copy)]
pub(crate) enum FilesMenuAction {
    OpenDefault,
    Reveal,
    OpenIn(&'static EditorMenuEntry),
    CopyPath,
    CopyRelativePath,
    SendToAgent(u64),
}

pub(crate) fn open_with_actions(
    editors: &'static [&'static EditorMenuEntry],
) -> ActionMenu<FilesMenuAction> {
    ActionMenu::new().items(editors.iter().map(|entry| {
        MenuItem::new(
            entry.label.trim_start_matches("Open in "),
            FilesMenuAction::OpenIn(entry),
        )
        .icon(entry.icon)
        .mnemonic(entry.mnemonic)
    }))
}

pub(crate) fn files_menu_actions(
    editors: &'static [&'static EditorMenuEntry],
    agents: &[AgentTarget],
) -> ActionMenu<FilesMenuAction> {
    let open_with = if editors.is_empty() {
        MenuItem::disabled(OPEN_WITH_LABEL).icon(icon::FILE_CODE)
    } else {
        MenuItem::submenu(OPEN_WITH_LABEL, open_with_actions(editors)).icon(icon::FILE_CODE)
    };
    ActionMenu::new()
        .item(
            MenuItem::new(OPEN_WITH_DEFAULT_LABEL, FilesMenuAction::OpenDefault)
                .icon(icon::GLOBE)
                .mnemonic('o'),
        )
        .item(
            MenuItem::new(open_with::reveal_label(), FilesMenuAction::Reveal)
                .icon(icon::FOLDER_OPEN)
                .mnemonic('e'),
        )
        .item(open_with)
        .separator()
        .item(
            MenuItem::new("Copy Path", FilesMenuAction::CopyPath)
                .icon(icon::COPY)
                .mnemonic('p'),
        )
        .item(
            MenuItem::new("Copy Relative Path", FilesMenuAction::CopyRelativePath)
                .icon(icon::LINK)
                .mnemonic('r'),
        )
        .separator()
        .item(send_to_agent_item(agents, FilesMenuAction::SendToAgent))
}

impl PaneFlowApp {
    pub(crate) fn render_files_context_menu(
        &self,
        menu: FilesContextMenu,
        _ui: crate::theme::UiColors,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let actions = match menu.scope {
            FilesMenuScope::Full => {
                files_menu_actions(installed_editors(), &self.agent_targets(cx))
            }
            FilesMenuScope::OpenWith => open_with_actions(installed_editors()),
        };
        let id = SharedString::from(format!("files-context-menu-{}", menu.path.display()));
        let target = menu.clone();
        let handlers = self.menu_handlers(
            cx,
            move |this, action: &FilesMenuAction, window, cx| {
                this.files_menu_open = None;
                this.run_files_menu_action(*action, &target, window, cx);
                cx.notify();
            },
            |this| this.files_menu_open = None,
        );
        render_context_menu(id, actions, menu.position, handlers, window, cx)
    }

    fn run_files_menu_action(
        &mut self,
        action: FilesMenuAction,
        target: &FilesContextMenu,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let path = target.path.clone();
        let launch = match action {
            FilesMenuAction::SendToAgent(surface_id) => {
                self.send_file_to_agent(surface_id, &target.root, &path, window, cx);
                return;
            }
            FilesMenuAction::CopyPath => {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    path.to_string_lossy().into_owned(),
                ));
                self.show_toast("Copied path", cx);
                return;
            }
            FilesMenuAction::CopyRelativePath => {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    files_tree::workspace_relative_path(&target.root, &path),
                ));
                self.show_toast("Copied relative path", cx);
                return;
            }
            FilesMenuAction::OpenDefault => cx
                .background_executor()
                .spawn(open_with::open_with_default_app(path)),
            FilesMenuAction::Reveal => cx
                .background_executor()
                .spawn(open_with::reveal_in_file_manager(path)),
            FilesMenuAction::OpenIn(entry) => cx
                .background_executor()
                .spawn(open_with::open_in_editor(path, entry.command, entry.label)),
        };
        cx.spawn(async move |this, cx| {
            if let Err(message) = launch.await {
                log::warn!("{message}");
                let _ = this.update(cx, |app, cx| app.show_toast(message, cx));
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::sidebar::context_menu::EDITOR_MENU_ENTRIES;

    fn labels(menu: &ActionMenu<FilesMenuAction>) -> Vec<String> {
        menu.entries()
            .iter()
            .filter_map(|entry| match entry {
                crate::ui_primitives::action_menu::MenuEntry::Item(item) => {
                    Some(item.label_for_test().to_string())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_tree_menu_lists_its_entries_in_order() {
        static ONE: [&EditorMenuEntry; 1] = [&EDITOR_MENU_ENTRIES[0]];
        let menu = files_menu_actions(&ONE, &[]);
        assert_eq!(
            labels(&menu),
            vec![
                "Open with Default App",
                open_with::reveal_label(),
                "Open With",
                "Copy Path",
                "Copy Relative Path",
                crate::app::agent_send::NO_AGENT,
            ]
        );
        assert!(menu.enabled_at_for_test(2));
        let last = menu.entries().len() - 1;
        assert!(!menu.enabled_at_for_test(last));
    }

    #[test]
    fn the_tree_menu_sends_to_the_only_agent_of_the_workspace() {
        let agents = [AgentTarget {
            surface_id: 5,
            tool: crate::agent_launcher::TerminalAgent::ClaudeCode,
            title: "review".into(),
        }];
        let menu = files_menu_actions(&[], &agents);
        let last = menu.entries().len() - 1;
        assert_eq!(
            labels(&menu).last().map(String::as_str),
            Some(crate::app::agent_send::SEND_TO_AGENT)
        );
        assert!(matches!(
            menu.action_at(last),
            Some(FilesMenuAction::SendToAgent(5))
        ));
    }

    #[test]
    fn the_open_with_scope_lists_only_the_installed_editors() {
        static ONE: [&EditorMenuEntry; 1] = [&EDITOR_MENU_ENTRIES[0]];
        let menu = open_with_actions(&ONE);
        assert_eq!(
            labels(&menu),
            vec![EDITOR_MENU_ENTRIES[0].label.trim_start_matches("Open in ")]
        );
    }

    #[test]
    fn open_with_is_disabled_when_no_editor_is_installed() {
        let menu = files_menu_actions(&[], &[]);
        assert_eq!(labels(&menu)[2], "Open With");
        assert!(!menu.enabled_at_for_test(2));
        assert!(menu.enabled_at_for_test(0));
    }
}
