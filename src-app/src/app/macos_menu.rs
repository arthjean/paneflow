use gpui::Context;

#[cfg(target_os = "macos")]
pub(crate) fn install_macos_menu_bar(cx: &mut gpui::App) {
    use gpui::{Menu, MenuItem, OsAction};

    use crate::{
        About, CheckForUpdates, CloseWorkspace, Copy, NewWorkspace, NextWorkspace, OpenHelp,
        OpenSettings, Paste, Quit, SelectAll, ShowSystemInfo,
    };

    cx.set_menus(vec![
        Menu::new("PaneFlow").items(vec![
            MenuItem::action("About PaneFlow", About),
            MenuItem::action("Check for Updates…", CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Quit PaneFlow", Quit),
        ]),
        Menu::new("Edit").items(vec![
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::separator(),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("Window").items(vec![
            MenuItem::action("New Workspace", NewWorkspace),
            MenuItem::action("Close Workspace", CloseWorkspace),
            MenuItem::separator(),
            MenuItem::action("Next Workspace", NextWorkspace),
        ]),
        Menu::new("Help").items(vec![
            MenuItem::action("PaneFlow Help", OpenHelp),
            MenuItem::separator(),
            MenuItem::action("System Info…", ShowSystemInfo),
        ]),
    ]);
}

#[cfg(target_os = "macos")]
pub(crate) fn install_macos_menu_action_fallbacks(cx: &mut gpui::App) {
    use crate::{
        About, CheckForUpdates, CloseWorkspace, Copy, NewWorkspace, NextWorkspace, OpenHelp,
        OpenSettings, PaneFlowApp, Paste, Quit, SelectAll, ShowSystemInfo, TerminalCopy,
        TerminalPaste, TerminalSelectAll,
    };

    fn paneflow_menu_window(cx: &gpui::App) -> Option<gpui::WindowHandle<PaneFlowApp>> {
        menu_target_window(cx.active_window(), cx.windows(), |window| {
            window.downcast::<PaneFlowApp>()
        })
    }

    fn with_active_paneflow_window(
        cx: &mut gpui::App,
        f: impl FnOnce(&mut PaneFlowApp, &mut gpui::Window, &mut Context<PaneFlowApp>),
    ) {
        let Some(window) = paneflow_menu_window(cx) else {
            return;
        };
        if let Err(err) = window.update(cx, f) {
            log::debug!("macOS menu fallback: PaneFlow window unavailable: {err}");
        }
    }

    cx.on_action(|_: &Quit, cx| {
        if paneflow_menu_window(cx).is_none() {
            cx.quit();
            return;
        }
        with_active_paneflow_window(cx, |app, _window, cx| {
            app.request_quit(cx);
        });
    });

    cx.on_action(|_: &About, cx| {
        with_active_paneflow_window(cx, |app, window, cx| {
            app.open_about_dialog(window, cx);
        });
    });

    cx.on_action(|_: &Copy, cx| cx.dispatch_action(&TerminalCopy));
    cx.on_action(|_: &Paste, cx| cx.dispatch_action(&TerminalPaste));
    cx.on_action(|_: &SelectAll, cx| cx.dispatch_action(&TerminalSelectAll));

    cx.on_action(|_: &NewWorkspace, cx| {
        with_active_paneflow_window(cx, |app, window, cx| {
            app.create_workspace_with_picker(window, cx);
        });
    });
    cx.on_action(|_: &CloseWorkspace, cx| {
        with_active_paneflow_window(cx, |app, window, cx| {
            app.close_workspace_at(app.active_idx, window, cx);
        });
    });
    cx.on_action(|_: &NextWorkspace, cx| {
        with_active_paneflow_window(cx, |app, window, cx| {
            if !app.workspaces.is_empty() {
                let next = (app.active_idx + 1) % app.workspaces.len();
                app.select_workspace(next, window, cx);
            }
        });
    });

    cx.on_action(|_: &ShowSystemInfo, cx| {
        with_active_paneflow_window(cx, |app, window, cx| {
            app.open_system_info_dialog(window, cx);
        });
    });

    cx.on_action(|_: &OpenHelp, cx| {
        with_active_paneflow_window(cx, |app, _window, cx| {
            app.open_documentation(cx);
        });
    });
    cx.on_action(|_: &OpenSettings, cx| {
        with_active_paneflow_window(cx, |app, window, cx| {
            app.open_settings_window(window, cx);
        });
    });
    cx.on_action(|_: &CheckForUpdates, cx| {
        with_active_paneflow_window(cx, |app, _window, cx| {
            app.request_update_check(cx);
        });
    });
}

#[cfg(any(target_os = "macos", test))]
fn menu_target_window<W, T>(
    active: Option<W>,
    windows: impl IntoIterator<Item = W>,
    downcast: impl Fn(W) -> Option<T>,
) -> Option<T> {
    active
        .and_then(&downcast)
        .or_else(|| windows.into_iter().find_map(downcast))
}

#[cfg(test)]
mod tests {
    use super::menu_target_window;

    fn paneflow_only(window: u32) -> Option<u32> {
        (window % 2 == 0).then_some(window)
    }

    #[test]
    fn menu_actions_use_the_active_paneflow_window() {
        assert_eq!(
            menu_target_window(Some(4), [2, 4, 6], paneflow_only),
            Some(4)
        );
    }

    #[test]
    fn a_minimized_window_falls_back_to_the_first_paneflow_window() {
        assert_eq!(
            menu_target_window(None, [1, 3, 6, 8], paneflow_only),
            Some(6)
        );
        assert_eq!(menu_target_window(Some(5), [1, 8], paneflow_only), Some(8));
        assert_eq!(menu_target_window(None, [1, 3], paneflow_only), None);
    }
}
