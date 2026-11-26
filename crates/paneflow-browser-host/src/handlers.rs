use cef::*;
use paneflow_browser_proto::surface::{DialogKind, FileMode};
use paneflow_browser_proto::{Event, ProfileName, Rect as ViewRect, TabId};

use crate::host::{Effect, console_level, media_permissions, origin_or_url, prompt_permissions};
use crate::ui::{dispatch, view_of, with_host};

fn text(value: Option<&CefString>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn frame_url(frame: &Frame) -> String {
    CefString::from(&frame.url()).to_string()
}

fn emit(event: Event) {
    dispatch(|_| vec![Effect::Emit(event)]);
}

fn emit_page(event: Event) {
    dispatch(|host| host.page_event(event));
}

wrap_render_handler! {
    struct TabRender {
        tab: TabId,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let view = view_of(self.tab);
                rect.x = 0;
                rect.y = 0;
                rect.width = i32::try_from(view.width).unwrap_or(1);
                rect.height = i32::try_from(view.height).unwrap_or(1);
            }
        }

        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> ::std::os::raw::c_int {
            let Some(info) = screen_info else { return 0 };
            let view = view_of(self.tab);
            let rect = Rect {
                x: 0,
                y: 0,
                width: i32::try_from(view.width).unwrap_or(1),
                height: i32::try_from(view.height).unwrap_or(1),
            };
            info.device_scale_factor = view.scale;
            info.depth = 24;
            info.depth_per_component = 8;
            info.is_monochrome = 0;
            info.rect = rect.clone();
            info.available_rect = rect;
            1
        }

        fn screen_point(
            &self,
            _browser: Option<&mut Browser>,
            view_x: ::std::os::raw::c_int,
            view_y: ::std::os::raw::c_int,
            screen_x: Option<&mut ::std::os::raw::c_int>,
            screen_y: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            match (screen_x, screen_y) {
                (Some(x), Some(y)) => {
                    *x = view_x;
                    *y = view_y;
                    1
                }
                _ => 0,
            }
        }

        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: ::std::os::raw::c_int) {
            let tab = self.tab;
            with_host(|host| host.popup_shown(tab, show == 1));
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            let Some(rect) = rect else { return };
            let rect = ViewRect { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
            let tab = self.tab;
            with_host(|host| host.popup_moved(tab, rect));
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
                return;
            };
            let Some(len) = paneflow_browser_proto::surface::frame_bytes(width, height) else {
                return;
            };
            if buffer.is_null() || len == 0 {
                return;
            }
            let pixels = unsafe { std::slice::from_raw_parts(buffer, len) };
            let popup = type_ == PaintElementType::POPUP;
            let tab = self.tab;
            with_host(|host| host.painted(tab, popup, pixels, width, height));
        }

        fn on_ime_composition_range_changed(
            &self,
            _browser: Option<&mut Browser>,
            _selected_range: Option<&Range>,
            character_bounds: Option<&[Rect]>,
        ) {
            let Some(last) = character_bounds.and_then(|bounds| bounds.last()) else {
                return;
            };
            emit(Event::ImeCaret {
                tab: self.tab,
                caret: ViewRect { x: last.x, y: last.y, width: last.width, height: last.height },
            });
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
                emit_page(Event::LoadStarted { tab: self.tab, url: frame_url(frame) });
            }
        }

        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            emit_page(Event::LoadingState {
                tab: self.tab,
                loading: is_loading == 1,
                can_go_back: can_go_back == 1,
                can_go_forward: can_go_forward == 1,
            });
        }

        fn on_load_end(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            http_status_code: ::std::os::raw::c_int,
        ) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() == 1) {
                emit_page(Event::Loaded {
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
                emit_page(Event::LoadFailed {
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
        fn on_loading_progress_change(&self, _browser: Option<&mut Browser>, progress: f64) {
            emit_page(Event::LoadProgress { tab: self.tab, progress });
        }

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

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            emit_page(Event::TitleChanged { tab: self.tab, title: text(title) });
        }

        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            if frame.is_some_and(|frame| frame.is_main() == 1) {
                emit_page(Event::AddressChanged { tab: self.tab, url: text(url) });
            }
        }

        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: ::std::os::raw::c_ulong,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> ::std::os::raw::c_int {
            emit(Event::Cursor { tab: self.tab, cursor: crate::input::cursor(type_) });
            1
        }
    }
}

wrap_jsdialog_handler! {
    struct TabJsDialog {
        tab: TabId,
    }

    impl JsdialogHandler {
        fn on_jsdialog(
            &self,
            _browser: Option<&mut Browser>,
            origin_url: Option<&CefString>,
            dialog_type: JsdialogType,
            message_text: Option<&CefString>,
            default_prompt_text: Option<&CefString>,
            callback: Option<&mut JsdialogCallback>,
            _suppress_message: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else { return 0 };
            let kind = if dialog_type == JsdialogType::CONFIRM {
                DialogKind::Confirm
            } else if dialog_type == JsdialogType::PROMPT {
                DialogKind::Prompt
            } else {
                DialogKind::Alert
            };
            let tab = self.tab;
            let callback = callback.clone();
            let Some(dialog) = with_host(|host| host.dialog_opened(tab, callback)).flatten() else {
                return 0;
            };
            emit(Event::DialogOpened {
                tab,
                dialog,
                kind,
                origin: origin_or_url(&text(origin_url)),
                message: text(message_text),
                default_text: text(default_prompt_text),
            });
            1
        }

        fn on_reset_dialog_state(&self, _browser: Option<&mut Browser>) {
            let tab = self.tab;
            with_host(|host| host.dialogs_reset(tab));
        }
    }
}

wrap_dialog_handler! {
    struct TabFileDialog {
        tab: TabId,
    }

    impl DialogHandler {
        fn on_file_dialog(
            &self,
            _browser: Option<&mut Browser>,
            mode: FileDialogMode,
            title: Option<&CefString>,
            _default_file_path: Option<&CefString>,
            accept_filters: Option<&mut CefStringList>,
            _accept_extensions: Option<&mut CefStringList>,
            _accept_descriptions: Option<&mut CefStringList>,
            callback: Option<&mut FileDialogCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else { return 0 };
            let mode = if mode == FileDialogMode::OPEN_MULTIPLE {
                FileMode::OpenMultiple
            } else if mode == FileDialogMode::OPEN_FOLDER {
                FileMode::OpenFolder
            } else if mode == FileDialogMode::SAVE {
                FileMode::Save
            } else {
                FileMode::Open
            };
            let accept: Vec<String> = accept_filters
                .map(|list| list.clone().into_iter().collect())
                .unwrap_or_default();
            let tab = self.tab;
            let held = callback.clone();
            let Some(request) =
                with_host(|host| host.file_chooser_opened(tab, held)).flatten()
            else {
                callback.cancel();
                return 1;
            };
            emit(Event::FileChooserOpened {
                tab,
                request,
                mode,
                title: text(title),
                accept,
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
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            let url = text(target_url);
            if !url.is_empty() {
                emit(Event::NewWindowRequested {
                    tab: self.tab,
                    url,
                    user_gesture: user_gesture == 1,
                });
            }
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

#[derive(Clone)]
struct TabHandlers {
    render: RenderHandler,
    load: LoadHandler,
    display: DisplayHandler,
    request: RequestHandler,
    permission: PermissionHandler,
    download: DownloadHandler,
    life_span: LifeSpanHandler,
    jsdialog: JsdialogHandler,
    dialog: DialogHandler,
}

wrap_client! {
    struct TabClient {
        handlers: TabHandlers,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.handlers.render.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.handlers.load.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.handlers.display.clone())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.handlers.request.clone())
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(self.handlers.permission.clone())
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(self.handlers.download.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.handlers.life_span.clone())
        }

        fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
            Some(self.handlers.jsdialog.clone())
        }

        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(self.handlers.dialog.clone())
        }
    }
}

pub fn tab_client(tab: TabId) -> Client {
    TabClient::new(TabHandlers {
        render: TabRender::new(tab),
        load: TabLoad::new(tab),
        display: TabDisplay::new(tab),
        request: TabRequest::new(tab),
        permission: TabPermission::new(tab),
        download: TabDownload::new(tab),
        life_span: TabLifeSpan::new(tab),
        jsdialog: TabJsDialog::new(tab),
        dialog: TabFileDialog::new(tab),
    })
}

wrap_request_context_handler! {
    struct ProfileContext {
        profile: ProfileName,
    }

    impl RequestContextHandler {
        fn on_request_context_initialized(&self, request_context: Option<&mut RequestContext>) {
            let outcome = match request_context {
                Some(context) => crate::host::block_permissions(context),
                None => Err("the engine initialised the profile without a context".to_string()),
            };
            let profile = self.profile.clone();
            crate::ui::on_ui(move || {
                dispatch(|host| host.profile_initialized(&profile, outcome));
            });
        }
    }
}

pub fn profile_handler(profile: ProfileName) -> RequestContextHandler {
    ProfileContext::new(profile)
}

wrap_dev_tools_message_observer! {
    struct TabDevTools {
        tab: TabId,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(
            &self,
            _browser: Option<&mut Browser>,
            message: Option<&[u8]>,
        ) -> ::std::os::raw::c_int {
            let Some(message) = message else { return 0 };
            let tab = self.tab;
            match with_host(|host| host.session_message(tab, message)).flatten() {
                Some(effects) => {
                    crate::host::run_effects(effects);
                    1
                }
                None => 0,
            }
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
            let tab = self.tab;
            let method = text(method);
            dispatch(|host| host.devtools_event(tab, method, params));
        }
    }
}

pub fn devtools_observer(tab: TabId) -> DevToolsMessageObserver {
    TabDevTools::new(tab)
}
