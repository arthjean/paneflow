use std::cell::RefCell;

use cef::*;
use serde_json::json;

use super::{emit, HOST};

wrap_window_delegate! {
    struct WitnessWindow {
        browser_view: RefCell<Option<BrowserView>>,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            Size { width: 1920, height: 1080 }
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut Window>) {
            let view = self.browser_view.borrow();
            if let (Some(window), Some(view)) = (window, view.as_ref()) {
                window.set_title(Some(&"Paneflow CEF qualification".into()));
                window.add_child_view(Some(&mut View::from(view)));
                window.set_size(Some(&Size { width: 1920, height: 1080 }));
                window.show();
                if std::env::var("PANEFLOW_M1_FULLSCREEN").as_deref() == Ok("1") {
                    window.set_fullscreen(1);
                }
            }
        }

        fn on_window_fullscreen_transition(&self, window: Option<&mut Window>, is_completed: i32) {
            if let Some(window) = window {
                let bounds = window.bounds();
                emit(json!({ "native": "window_fullscreen", "completed": is_completed != 0,
                    "fullscreen": window.is_fullscreen() != 0,
                    "width": bounds.width, "height": bounds.height }));
            }
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            *self.browser_view.borrow_mut() = None;
            emit(json!({ "native": "window_destroyed" }));
        }

        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            self.browser_view.borrow().as_ref().and_then(|view| view.browser())
                .and_then(|browser| browser.host()).map_or(1, |host| host.try_close_browser())
        }

        fn is_frameless(&self, _window: Option<&mut Window>) -> i32 { 1 }
        fn window_runtime_style(&self) -> RuntimeStyle { RuntimeStyle::ALLOY }
    }
}

wrap_browser_view_delegate! {
    struct WitnessView {}

    impl ViewDelegate {}

    impl BrowserViewDelegate {
        fn browser_runtime_style(&self) -> RuntimeStyle { RuntimeStyle::ALLOY }
    }
}

pub(super) fn create(url: &str) -> bool {
    let view = browser_view_create(
        Some(&mut super::handlers::WitnessClient::new()),
        Some(&url.into()),
        Some(&BrowserSettings::default()),
        None,
        None,
        Some(&mut WitnessView::new()),
    );
    if view.is_none() {
        return false;
    }
    let window = window_create_top_level(Some(&mut WitnessWindow::new(RefCell::new(view))));
    let created = window.is_some();
    HOST.with(|state| {
        if let Some(host) = state.borrow_mut().as_mut() {
            host.window = window;
        }
    });
    created
}
