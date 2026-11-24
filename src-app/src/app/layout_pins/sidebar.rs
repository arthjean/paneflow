use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext, point, px};

use super::forget_the_previous_pin;
use crate::app::bootstrap::Startup;
use crate::app::fake_host::FakeHost;
use crate::terminal::TerminalView;
use crate::terminal::view::DisplayOnlyTerminals;
use crate::workspace::{Workspace, next_workspace_id_in};
use crate::{FocusWorkspacesSidebar, PaneFlowApp, TabKey};

pub(super) const PINS: usize = 2;

const MIN_RENAME_FIELD_WIDTH: f32 = 120.;

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_the_tab_rename_field_spans_its_row_and_takes_the_typed_title(cx: &mut TestAppContext) {
    forget_the_previous_pin();
    cx.executor().allow_parking();
    cx.update(|cx| {
        cx.set_global(DisplayOnlyTerminals);
        cx.set_global(FakeHost::default());
    });
    let (app, cx) = cx.add_window_view(|_, cx| PaneFlowApp::start(Startup::Conformance, cx));
    cx.run_until_parked();
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let ws_id = next_workspace_id_in(None);
            let terminal = cx.new(|cx| TerminalView::new(ws_id, cx));
            let pane = app.create_pane(terminal, ws_id, cx);
            let idx = app.add_workspace_in(Workspace::with_id(ws_id, "alpha", pane), None, cx);
            app.select_workspace_through_model(idx, cx);
            app.begin_tab_rename(idx, 0, cx);
        });
    });
    cx.run_until_parked();
    let empty = cx
        .debug_bounds("tab-rename-field")
        .expect("the rename field is painted");
    assert!(
        empty.size.width > px(MIN_RENAME_FIELD_WIDTH),
        "an empty rename field is {:?} wide",
        empty.size.width
    );
    cx.simulate_keystrokes("m y space t a b");
    cx.run_until_parked();
    let typed = cx
        .debug_bounds("tab-rename-field")
        .expect("the rename field is painted");
    assert!(
        typed.size.width > px(MIN_RENAME_FIELD_WIDTH),
        "a filled rename field is {:?} wide",
        typed.size.width
    );
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let title = cx.update(|_, cx| app.read(cx).workspaces[0].tabs()[0].title().to_string());
    assert_eq!(title, "my tab");
}

fn open_menus(
    app: &Entity<PaneFlowApp>,
    cx: &mut VisualTestContext,
) -> (Option<TabKey>, Option<usize>) {
    cx.update(|_, cx| {
        let app = app.read(cx);
        (
            app.tab_menu_open.map(|menu| menu.tab),
            app.workspace_menu_open.map(|menu| menu.idx),
        )
    })
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_shift_f10_opens_the_menu_of_the_row_under_the_keyboard_cursor_at_that_row(
    cx: &mut TestAppContext,
) {
    forget_the_previous_pin();
    cx.executor().allow_parking();
    cx.update(|cx| {
        cx.set_global(DisplayOnlyTerminals);
        cx.set_global(FakeHost::default());
    });
    let (app, cx) = cx.add_window_view(|_, cx| PaneFlowApp::start(Startup::Conformance, cx));
    cx.run_until_parked();
    let (ws_idx, tab) = cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let ws_id = next_workspace_id_in(None);
            let terminal = cx.new(|cx| TerminalView::new(ws_id, cx));
            let pane = app.create_pane(terminal, ws_id, cx);
            let idx = app.add_workspace_in(Workspace::with_id(ws_id, "alpha", pane), None, cx);
            app.select_workspace_through_model(idx, cx);
            let tab = TabKey::of(&app.workspaces[idx], 0).expect("the workspace has a tab");
            (idx, tab)
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.handle_focus_workspaces_sidebar(&FocusWorkspacesSidebar, window, cx);
        });
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    let tab_row = cx
        .debug_bounds("sidebar-cursor-row")
        .expect("the tab row under the cursor is painted");
    cx.simulate_keystrokes("shift-f10");
    cx.run_until_parked();
    assert_eq!(open_menus(&app, cx), (Some(tab), None));
    let anchor = cx.update(|_, cx| app.read(cx).tab_menu_open.map(|menu| menu.position));
    assert_eq!(anchor, Some(point(tab_row.left(), tab_row.bottom())));

    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert_eq!(open_menus(&app, cx), (None, None));
    let renaming = cx.update(|_, cx| app.read(cx).renaming_tab);
    assert_eq!(
        renaming,
        Some(tab),
        "the arrow keys and Enter run the first item, Rename, on that tab"
    );

    cx.simulate_keystrokes("shift-f10");
    cx.run_until_parked();
    assert_eq!(
        open_menus(&app, cx),
        (None, None),
        "Shift+F10 opens nothing while a tab is being renamed"
    );
    let renaming = cx.update(|_, cx| app.read(cx).renaming_tab);
    assert_eq!(renaming, Some(tab));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.handle_focus_workspaces_sidebar(&FocusWorkspacesSidebar, window, cx);
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    let folder_row = cx
        .debug_bounds("sidebar-cursor-row")
        .expect("the folder row under the cursor is painted");
    cx.simulate_keystrokes("shift-f10");
    cx.run_until_parked();
    assert_eq!(open_menus(&app, cx), (None, Some(ws_idx)));
    let anchor = cx.update(|_, cx| app.read(cx).workspace_menu_open.map(|menu| menu.position));
    assert_eq!(anchor, Some(point(folder_row.left(), folder_row.bottom())));

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(open_menus(&app, cx), (None, None));
    let sidebar_focused = cx.update(|window, cx| app.read(cx).sidebar_focus.is_focused(window));
    assert!(sidebar_focused, "Escape returns focus to the sidebar");
}
