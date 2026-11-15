pub(crate) const ABOUT: &str = "icons/menu/about.svg";
pub(crate) const ADD: &str = "icons/menu/add.svg";
pub(crate) const AUTOMATIONS: &str = "icons/menu/automations.svg";
pub(crate) const CHECK: &str = "icons/menu/check.svg";
pub(crate) const CLOSE: &str = "icons/menu/close.svg";
pub(crate) const COLLAPSE_ALL: &str = "icons/menu/collapse-all.svg";
pub(crate) const COMPARE: &str = "icons/menu/compare.svg";
pub(crate) const COPY: &str = "icons/menu/copy.svg";
pub(crate) const DETACH: &str = "icons/menu/detach.svg";
pub(crate) const DOCK: &str = "icons/menu/dock.svg";
pub(crate) const DOCUMENTATION: &str = "icons/menu/documentation.svg";
pub(crate) const EXPAND_ALL: &str = "icons/menu/expand-all.svg";
pub(crate) const FILE_CODE: &str = "icons/menu/file-code.svg";
pub(crate) const FOLDER_OPEN: &str = "icons/menu/folder-open.svg";
pub(crate) const FORK: &str = "icons/menu/fork.svg";
pub(crate) const GIT_BRANCH: &str = "icons/menu/git-branch.svg";
pub(crate) const GLOBE: &str = "icons/menu/globe.svg";
pub(crate) const GROUP_BY: &str = "icons/menu/group-by.svg";
pub(crate) const HIGHLIGHT: &str = "icons/menu/highlight.svg";
pub(crate) const LAYOUT: &str = "icons/menu/layout.svg";
pub(crate) const LINK: &str = "icons/menu/link.svg";
pub(crate) const MARK_READ: &str = "icons/menu/mark-read.svg";
pub(crate) const MINIMAP: &str = "icons/menu/minimap.svg";
pub(crate) const MOVE_TO_SPACE: &str = "icons/menu/move-to-space.svg";
pub(crate) const NEW_WORKSPACE: &str = "icons/menu/new-workspace.svg";
pub(crate) const NOTIFICATION: &str = "icons/menu/notification.svg";
pub(crate) const NOTIFICATION_OFF: &str = "icons/menu/notification-off.svg";
pub(crate) const PLAY: &str = "icons/menu/play.svg";
pub(crate) const PREFERENCES: &str = "icons/menu/preferences.svg";
pub(crate) const REFRESH: &str = "icons/menu/refresh.svg";
pub(crate) const RENAME: &str = "icons/menu/rename.svg";
pub(crate) const RESET_NAME: &str = "icons/menu/reset-name.svg";
pub(crate) const SCROLLBAR: &str = "icons/menu/scrollbar.svg";
pub(crate) const SERVER: &str = "icons/menu/server.svg";
pub(crate) const SETTINGS: &str = "icons/menu/settings.svg";
pub(crate) const SHOW: &str = "icons/menu/show.svg";
pub(crate) const SYSTEM_INFO: &str = "icons/menu/system-info.svg";
pub(crate) const TERMINAL: &str = "icons/menu/terminal.svg";
pub(crate) const TRASH: &str = "icons/menu/trash.svg";
pub(crate) const TROUBLESHOOTING: &str = "icons/menu/troubleshooting.svg";
pub(crate) const UPDATE: &str = "icons/menu/update.svg";
pub(crate) const VSCODE: &str = "icons/menu/vscode.svg";
pub(crate) const WEBSTORM: &str = "icons/menu/webstorm.svg";
pub(crate) const WHATS_NEW: &str = "icons/menu/whats-new.svg";
pub(crate) const WHITESPACE: &str = "icons/menu/whitespace.svg";

#[cfg(test)]
mod tests {
    #[test]
    fn every_menu_glyph_is_embedded() {
        for path in [
            super::ABOUT,
            super::ADD,
            super::AUTOMATIONS,
            super::CHECK,
            super::CLOSE,
            super::COLLAPSE_ALL,
            super::COMPARE,
            super::COPY,
            super::DETACH,
            super::DOCK,
            super::DOCUMENTATION,
            super::EXPAND_ALL,
            super::FILE_CODE,
            super::FOLDER_OPEN,
            super::FORK,
            super::GIT_BRANCH,
            super::GLOBE,
            super::GROUP_BY,
            super::HIGHLIGHT,
            super::LAYOUT,
            super::LINK,
            super::MARK_READ,
            super::MINIMAP,
            super::MOVE_TO_SPACE,
            super::NEW_WORKSPACE,
            super::NOTIFICATION,
            super::NOTIFICATION_OFF,
            super::PLAY,
            super::PREFERENCES,
            super::REFRESH,
            super::RENAME,
            super::RESET_NAME,
            super::SCROLLBAR,
            super::SERVER,
            super::SETTINGS,
            super::SHOW,
            super::SYSTEM_INFO,
            super::TERMINAL,
            super::TRASH,
            super::TROUBLESHOOTING,
            super::UPDATE,
            super::VSCODE,
            super::WEBSTORM,
            super::WHATS_NEW,
            super::WHITESPACE,
        ] {
            assert!(
                crate::assets::Assets::get(path).is_some(),
                "missing menu glyph {path}"
            );
        }
    }
}
