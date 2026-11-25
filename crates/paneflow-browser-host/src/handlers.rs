use cef::*;
use paneflow_browser_proto::{Event, TabId};

use crate::host::{Effect, console_level, media_permissions, origin_or_url, prompt_permissions};
use crate::ui::{dispatch, with_host};

const VIEW_WIDTH: i32 = 1280;
const VIEW_HEIGHT: i32 = 800;

fn text(value: Option<&CefString>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn frame_url(frame: &Frame) -> String {
    CefString::from(&frame.url()).to_string()
}

fn emit(event: Event) {
    dispatch(|_| vec![Effect::Emit(event)]);
}

wrap_render_handler! {
    struct TabRender {
        tab: TabId,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                rect.width = VIEW_WIDTH;
                rect.height = VIEW_HEIGHT;
            }
        }
    }
}

wrap_load_handler! {
    struct TabLoad {
        tab: TabId,
    }

    impl LoadHandler {
        fn on_load_start(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _transition_type: TransitionType,
        ) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() == 1) {
                emit(Event::LoadStarted { tab: self.tab, url: frame_url(frame) });
            }
        }

        fn on_load_end(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            http_status_code: ::std::os::raw::c_int,
        ) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() == 1) {
                emit(Event::Loaded {
                    tab: self.tab,
                    url: frame_url(frame),
                    http_status: http_status_code,
                });
            }
        }

        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            if frame.is_some_and(|frame| frame.is_main() == 1) {
                emit(Event::LoadFailed {
                    tab: self.tab,
                    url: text(failed_url),
                    error_code: error_code.get_raw(),
                    error_text: text(error_text),
                });
            }
        }
    }
}

wrap_display_handler! {
    struct TabDisplay {
        tab: TabId,
    }

    impl DisplayHandler {
        fn on_console_message(
            &self,
            _browser: Option<&mut Browser>,
            level: LogSeverity,
            message: Option<&CefString>,
            source: Option<&CefString>,
            line: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            emit(Event::Console {
                tab: self.tab,
                level: console_level(level),
                text: text(message),
                source: text(source),
                line,
            });
            1
        }
    }
}

wrap_request_handler! {
    struct TabRequest {
        tab: TabId,
    }

    impl RequestHandler {
        fn on_before_browse(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: ::std::os::raw::c_int,
            _is_redirect: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let Some(request) = request else { return 1 };
            let url = CefString::from(&request.url()).to_string();
            let top_level = frame.is_none_or(|frame| frame.is_main() == 1);
            let tab = self.tab;
            let decided = with_host(|host| host.before_browse(tab, url, top_level));
            let (cancel, effects) = decided.unwrap_or((true, Vec::new()));
            crate::host::run_effects(effects);
            i32::from(cancel)
        }

        fn on_render_process_terminated(
            &self,
            _browser: Option<&mut Browser>,
            status: TerminationStatus,
            error_code: ::std::os::raw::c_int,
            error_string: Option<&CefString>,
        ) {
            let reason = format!(
                "the page's renderer process ended ({status:?}, code {error_code}{})",
                error_string
                    .map(|value| format!(", {value}"))
                    .unwrap_or_default()
            );
            let tab = self.tab;
            dispatch(|host| host.tab_crashed(tab, reason));
        }
    }
}

wrap_permission_handler! {
    struct TabPermission {
        tab: TabId,
    }

    impl PermissionHandler {
        fn on_request_media_access_permission(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> ::std::os::raw::c_int {
            if let Some(callback) = callback {
                callback.cancel();
            }
            emit(Event::PermissionDenied {
                tab: self.tab,
                origin: origin_or_url(&text(requesting_origin)),
                permissions: media_permissions(requested_permissions),
            });
            1
        }

        fn on_show_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            _prompt_id: u64,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> ::std::os::raw::c_int {
            if let Some(callback) = callback {
                callback.cont(PermissionRequestResult::DENY);
            }
            emit(Event::PermissionDenied {
                tab: self.tab,
                origin: origin_or_url(&text(requesting_origin)),
                permissions: prompt_permissions(requested_permissions),
            });
            1
        }
    }
}

wrap_download_handler! {
    struct TabDownload {
        tab: TabId,
    }

    impl DownloadHandler {
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            emit(Event::DownloadCancelled { tab: self.tab, url: text(url) });
            0
        }

        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            _download_item: Option<&mut DownloadItem>,
            _suggested_name: Option<&CefString>,
            _callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            _download_item: Option<&mut DownloadItem>,
            callback: Option<&mut DownloadItemCallback>,
        ) {
            if let Some(callback) = callback {
                callback.cancel();
            }
        }
    }
}

wrap_life_span_handler! {
    struct TabLifeSpan {
        tab: TabId,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            _target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            1
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            let browser = browser.clone();
            let tab = self.tab;
            dispatch(|host| host.tab_created(tab, browser));
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            let tab = self.tab;
            dispatch(|host| host.tab_closed(tab));
        }
    }
}

wrap_client! {
    struct TabClient {
        render: RenderHandler,
        load: LoadHandler,
        display: DisplayHandler,
        request: RequestHandler,
        permission: PermissionHandler,
        download: DownloadHandler,
        life_span: LifeSpanHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.request.clone())
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(self.permission.clone())
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(self.download.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }
    }
}

pub fn tab_client(tab: TabId) -> Client {
    TabClient::new(
        TabRender::new(tab),
        TabLoad::new(tab),
        TabDisplay::new(tab),
        TabRequest::new(tab),
        TabPermission::new(tab),
        TabDownload::new(tab),
        TabLifeSpan::new(tab),
    )
}

wrap_dev_tools_message_observer! {
    struct TabDevTools {
        tab: TabId,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(
            &self,
            _browser: Option<&mut Browser>,
            _message: Option<&[u8]>,
        ) -> ::std::os::raw::c_int {
            0
        }

        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            let tab = self.tab;
            let result = result.unwrap_or_default().to_vec();
            dispatch(|host| host.devtools_result(tab, message_id, success == 1, &result));
        }

        fn on_dev_tools_event(
            &self,
            _browser: Option<&mut Browser>,
            method: Option<&CefString>,
            params: Option<&[u8]>,
        ) {
            let params = params
                .and_then(|params| serde_json::from_slice(params).ok())
                .unwrap_or(serde_json::Value::Null);
            emit(Event::DevTools { tab: self.tab, method: text(method), params });
        }
    }
}

pub fn devtools_observer(tab: TabId) -> DevToolsMessageObserver {
    TabDevTools::new(tab)
}
