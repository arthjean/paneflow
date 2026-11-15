use gpui::{AnyElement, ClipboardItem, Context, SharedString};

use crate::app::files_tree;
use crate::app::menu_icons as icon;
use crate::app::sidebar::context_menu::render_context_menu;
use crate::ui_primitives::action_menu::{ActionMenu, MenuItem};
use crate::{FilesContextMenu, PaneFlowApp};

#[derive(Clone, Copy)]
enum FilesMenuAction {
    CopyPath,
    CopyRelativePath,
}

impl PaneFlowApp {
    pub(crate) fn render_files_context_menu(
        &self,
        menu: FilesContextMenu,
        _ui: crate::theme::UiColors,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let actions = ActionMenu::new()
            .item(
                MenuItem::new("Copy Path", FilesMenuAction::CopyPath)
                    .icon(icon::COPY)
                    .mnemonic('p'),
            )
            .item(
                MenuItem::new("Copy Relative Path", FilesMenuAction::CopyRelativePath)
                    .icon(icon::LINK)
                    .mnemonic('r'),
            );
        let id = SharedString::from(format!("files-context-menu-{}", menu.path.display()));
        let target = menu.clone();
        let handlers = self.menu_handlers(
            cx,
            move |this, action: &FilesMenuAction, _, cx| {
                this.files_menu_open = None;
                let (value, toast) = match action {
                    FilesMenuAction::CopyPath => {
                        (target.path.to_string_lossy().into_owned(), "Copied path")
                    }
                    FilesMenuAction::CopyRelativePath => (
                        files_tree::workspace_relative_path(&target.root, &target.path),
                        "Copied relative path",
                    ),
                };
                cx.write_to_clipboard(ClipboardItem::new_string(value));
                this.show_toast(toast, cx);
            },
            |this| this.files_menu_open = None,
        );
        render_context_menu(id, actions, menu.position, handlers, window, cx)
    }
}
