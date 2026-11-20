use gpui::{AppContext as _, TestAppContext, px};

use super::assert_isolated_home;
use crate::PaneFlowApp;
use crate::app::bootstrap::Startup;
use crate::app::fake_host::FakeHost;
use crate::terminal::TerminalView;
use crate::terminal::view::DisplayOnlyTerminals;
use crate::workspace::{Workspace, next_workspace_id_in};

pub(super) const PINS: usize = 1;

const MIN_RENAME_FIELD_WIDTH: f32 = 120.;

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_the_tab_rename_field_spans_its_row_and_takes_the_typed_title(cx: &mut TestAppContext) {
    assert_isolated_home();
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
