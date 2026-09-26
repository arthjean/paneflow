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

    fn with_active_paneflow_window(
        cx: &mut gpui::App,
        f: impl FnOnce(&mut PaneFlowApp, &mut gpui::Window, &mut Context<PaneFlowApp>),
    ) {
        let Some(window) = cx.active_window() else {
            return;
        };
        let Some(window) = window.downcast::<PaneFlowApp>() else {
            return;
        };
        if let Err(err) = window.update(cx, f) {
            log::debug!("macOS menu fallback: active PaneFlow window unavailable: {err}");
        }
    }

    cx.on_action(|_: &Quit, cx| {
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
