pub struct PageConfig {
    pub benchmark_id: String,
    pub host_binary: std::path::PathBuf,
    pub runtime_root: std::path::PathBuf,
    pub stage_root: std::path::PathBuf,
    pub profile_dir: std::path::PathBuf,
    pub origin: String,
    pub owner: paneflow_browser_protocol::Owner,
    #[cfg(target_os = "windows")]
    pub runtime_check: crate::browser::supervisor::RuntimeCheck,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    pub scale_percent: u32,
}

#[cfg_attr(
    target_os = "windows",
    allow(
        dead_code,
        reason = "agent console/network diagnostics and the renderer clipboard round trip stay Linux-only until their own stories"
    )
)]
#[derive(Clone, Debug)]
pub enum PageSignal {
    ExternalOpen(String),
    CloseCancelled,
    Fullscreen(bool),
    FindResult {
        count: i32,
        active: i32,
    },
    Transfer(serde_json::Value),
    WebDialog {
        document: paneflow_browser_protocol::Document,
        request: u64,
        kind: String,
        origin: String,
        message: String,
        default_text: String,
    },
    WebDialogClosed {
        request: u64,
    },
    Accessibility {
        document: paneflow_browser_protocol::Document,
        kind: String,
        value: serde_json::Value,
    },
    Console {
        document: paneflow_browser_protocol::Document,
        value: serde_json::Value,
    },
    Network {
        document: paneflow_browser_protocol::Document,
        value: serde_json::Value,
    },
    OperationCompleted {
        operation: paneflow_browser_protocol::OperationId,
        result: Result<paneflow_browser_protocol::Event, paneflow_browser_protocol::BrowserError>,
    },
    PopupRequested(String),
    Ready,
    Created,
    State(paneflow_browser_protocol::BrowserSession),
    Closed,
    Refused(paneflow_browser_protocol::BrowserError),
    Loading {
        loading: bool,
        can_go_back: bool,
        can_go_forward: bool,
    },
    Title(String),
    Address(String),
    Loaded,
    LoadFailed(String),
    Clipboard {
        request: u64,
        text: String,
    },
    ContextMenu {
        request: u64,
        x: i32,
        y: i32,
        items: Vec<ContextMenuItem>,
    },
    ContextMenuClosed {
        request: u64,
    },
    Cursor(gpui::CursorStyle),
    ImeSelection(super::ime::ImeSnapshot),
    ImeBounds(super::ime::ImeSnapshot),
    Repaint,
    Lost(String),
    Fatal(String),
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct ContextMenuItem {
    pub command: i32,
    pub label: String,
    pub enabled: bool,
    pub checked: bool,
}

#[cfg(target_os = "linux")]
pub use linux::LivePage;

#[cfg(target_os = "windows")]
pub use windows::LivePage;

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
pub use stub::LivePage;

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use gpui::{App, AppContext, ExternalSurface, Window};
    use gpui_platform::gpui_wgpu::{ExternalSurfaceContext, wgpu};
    use paneflow_browser_protocol::{
        BrowserError, BrowserPresentation, BrowserSession, Command, Document, Event, FrameAck,
        MAX_AGENT_CAPTURE_BYTES, MAX_AGENT_CAPTURE_CHUNK_BYTES, OperationId, SessionState,
    };
    use serde_json::Value;

    use super::{Geometry, PageConfig, PageSignal};
    use crate::browser::linux::{DmabufImporter, external_sync};
    use crate::browser::presentation::{FrameConsumer, Intake, PoolLayout, TextureImporter};
    use crate::browser::prototype::wait_for_gpu_completion;
    use crate::browser::supervisor::{
        FrameAcknowledger, HostConfig, HostEvent, Ozone, RuntimeCheck,
    };

    struct PoolImportCompletion {
        document: Document,
        pool_generation: u64,
        result: Result<Vec<ExternalSurface>, String>,
    }

    struct ScreenshotAssembly {
        document: Document,
        mime: String,
        width: u32,
        height: u32,
        chunks: Vec<Option<String>>,
        encoded_bytes: usize,
    }

    fn screenshot_failure_error(reason: &str) -> BrowserError {
        match reason {
            "native screenshot dimensions are invalid" | "native screenshot exceeds its bound" => {
                BrowserError::TooLarge
            }
            "native screenshot result is invalid"
            | "native screenshot data is missing"
            | "native screenshot data is not base64"
            | "native screenshot chunk is invalid" => BrowserError::InvalidMessage,
            _ => BrowserError::Unavailable,
        }
    }

    pub struct GpuCompletion {
        acknowledger: Option<FrameAcknowledger>,
        result: Result<Vec<FrameAck>, String>,
        imported: Option<PoolImportCompletion>,
    }

    impl Drop for GpuCompletion {
        fn drop(&mut self) {
            let Some(acknowledger) = &self.acknowledger else {
                return;
            };
            if let Ok(acks) = &self.result {
                for ack in acks {
                    let _ = acknowledger.ack(ack.clone());
                }
            }
            if let Some(imported) = &self.imported {
                let _ = acknowledger.ack(FrameAck::PoolRejected {
                    document: imported.document.clone(),
                    pool_generation: imported.pool_generation,
                });
            }
        }
    }

    pub struct PageStart {
        pub page: LivePage,
        pub host_events: smol::channel::Receiver<HostEvent>,
        pub gpu_completions: smol::channel::Receiver<GpuCompletion>,
    }

    pub struct LivePage {
        host: Arc<crate::browser::profile_host::ProfileHost>,
        browser: paneflow_browser_protocol::BrowserId,
        context: Arc<ExternalSurfaceContext>,
        importer: DmabufImporter,
        consumer: FrameConsumer<ExternalSurface>,
        completions: smol::channel::Sender<GpuCompletion>,
        session: Option<BrowserSession>,
        last_document: Option<Document>,
        presented: Option<(Geometry, bool)>,
        presentation_generation: u64,
        resize_sent: Option<(u64, Instant)>,
        benchmark_id: String,
        screenshots: BTreeMap<OperationId, ScreenshotAssembly>,
    }

    fn resize_may_be_sent(live_pools: usize, pending: bool) -> bool {
        live_pools <= 1 && !pending
    }

    impl LivePage {
        pub fn start(window: &Window, cx: &App, page: PageConfig) -> Result<PageStart, String> {
            let PageConfig {
                benchmark_id,
                host_binary,
                runtime_root,
                stage_root,
                profile_dir,
                origin,
                owner,
            } = page;
            let context = window
                .external_surface_context()
                .and_then(|context| context.downcast::<ExternalSurfaceContext>().ok())
                .ok_or("this window exposes no external surface context")?;
            let importer = DmabufImporter::new(context.clone())?;
            let ozone = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                Ozone::Wayland
            } else {
                Ozone::X11
            };
            let config = HostConfig {
                tracing: std::env::var_os("PANEFLOW_BROWSER_TRACE_DIR").is_some(),
                host_binary,
                runtime_root,
                stage_root,
                profile_dir,
                origin: origin.clone(),
                owner,
                ozone,
                gpu: Some(importer.gpu()),
                render_node: importer.render_node.clone(),
                frames: true,
                check: RuntimeCheck::Manifest,
            };
            let browser = paneflow_browser_protocol::BrowserId::try_from(benchmark_id.clone())
                .map_err(str::to_owned)?;
            let (host, host_events, new_host) =
                crate::browser::profile_host::ProfileHost::subscribe(config, browser.clone())?;
            let (completions, gpu_completions) = smol::channel::bounded(64);
            if new_host && let Some(runtime) = cx.try_global::<crate::browser::BrowserRuntime>() {
                runtime.register_page(host.supervisor.clone());
            }
            Ok(PageStart {
                page: Self {
                    host,
                    browser,
                    context,
                    importer,
                    consumer: FrameConsumer::default(),
                    completions,
                    session: None,
                    last_document: None,
                    presented: None,
                    presentation_generation: 0,
                    resize_sent: None,
                    benchmark_id,
                    screenshots: BTreeMap::new(),
                },
                host_events,
                gpu_completions,
            })
        }

        pub fn document(&self) -> Option<&Document> {
            self.session.as_ref().map(|session| &session.document)
        }

        pub fn surface(&self) -> Option<ExternalSurface> {
            self.consumer
                .current()
                .map(|(surface, _, _)| surface.clone())
        }

        pub fn send(&self, command: Command) -> Result<OperationId, BrowserError> {
            self.host.send(&self.browser, command)
        }

        pub fn send_to_document(
            &self,
            build: impl FnOnce(Document) -> Command,
        ) -> Result<OperationId, BrowserError> {
            let document = self.document().cloned().ok_or(BrowserError::Unavailable)?;
            self.send(build(document))
        }

        pub fn agent_screenshot(&self) -> Result<OperationId, BrowserError> {
            self.send_to_document(|document| Command::Screenshot { document })
        }

        pub fn on_host_event(&mut self, event: HostEvent, cx: &mut App) -> Vec<PageSignal> {
            let bench = crate::browser::benchmark::enabled();
            if bench
                && let HostEvent::Frame(
                    paneflow_browser_protocol::FrameMessage::Frame {
                        sequence,
                        callback_ns,
                        ready_ns,
                        ..
                    },
                    _,
                ) = &event
            {
                crate::browser::benchmark::record(
                    &self.benchmark_id,
                    "frame_received",
                    serde_json::json!({"sequence": sequence, "callback_ns": callback_ns, "ready_ns": ready_ns}),
                );
            }
            match event {
                HostEvent::Ready(info) => {
                    crate::browser::benchmark::record(
                        &self.benchmark_id,
                        "host_ready",
                        serde_json::json!({"pid":info.pid}),
                    );
                    vec![PageSignal::Ready]
                }
                HostEvent::Reply(reply) => {
                    let operation = reply.operation;
                    let result = reply.result;
                    let mut signals = match &result {
                        Ok(Event::State { session }) | Ok(Event::NavigationStarted { session }) => {
                            if self
                                .session
                                .as_ref()
                                .is_some_and(|current| current.document != session.document)
                            {
                                self.screenshots.clear();
                            }
                            self.last_document = Some(session.document.clone());
                            self.consumer.set_document(session.document.clone());
                            if !session.presentation.mounted {
                                self.presented = None;
                                self.resize_sent = None;
                            }
                            self.session = Some(session.clone());
                            vec![PageSignal::State(session.clone())]
                        }
                        Ok(Event::Closed { .. }) => {
                            self.session = None;
                            vec![PageSignal::Closed]
                        }
                        Ok(_) => Vec::new(),
                        Err(error) => vec![PageSignal::Refused(*error)],
                    };
                    signals.push(PageSignal::OperationCompleted { operation, result });
                    signals
                }
                HostEvent::Native(value) => self.on_native(&value),
                HostEvent::Frame(message, fds) => {
                    if let paneflow_browser_protocol::FrameMessage::PoolCreated {
                        document,
                        pool_generation,
                        width,
                        height,
                        format,
                        modifier,
                        buffers,
                    } = message
                    {
                        let layout = PoolLayout {
                            generation: pool_generation,
                            width,
                            height,
                            format,
                            modifier,
                            buffers,
                        };
                        if let Err(outcome) =
                            self.consumer
                                .reserve_pool(document.clone(), layout.clone(), true)
                        {
                            return self.on_intake(outcome, cx);
                        }
                        let acknowledger = match self.acknowledger() {
                            Ok(acknowledger) => acknowledger,
                            Err(error) => return vec![PageSignal::Fatal(error)],
                        };
                        let mut importer = self.importer.pool_importer();
                        let context = self.context.clone();
                        let sender = self.completions.clone();
                        let benchmark_id = self.benchmark_id.clone();
                        cx.background_spawn(async move {
                            let (result, import_duration_ns) = smol::unblock(move || {
                                let started = if bench { crate::browser::benchmark::now_ns() } else { 0 };
                                let result = importer.import(&layout, fds);
                                let duration = if bench { crate::browser::benchmark::now_ns().saturating_sub(started) } else { 0 };
                                (result, duration)
                            }).await;
                            if bench {
                                crate::browser::benchmark::record(
                                    &benchmark_id,
                                    "pool_import",
                                    serde_json::json!({"duration_ns": import_duration_ns}),
                                );
                            }
                            let initialization_started = if bench { crate::browser::benchmark::now_ns() } else { 0 };
                            let result = match result {
                                Ok(surfaces) if surfaces.len() != usize::from(paneflow_browser_protocol::POOL_BUFFERS) => {
                                    Err("pool import returned an incomplete buffer set".into())
                                }
                                Ok(surfaces) => match external_sync::initialize(context, surfaces.clone()) {
                                    Ok(initialization) => initialization.await.map(|()| surfaces),
                                    Err(error) => Err(error),
                                },
                                Err(error) => Err(error),
                            };
                            if bench {
                                crate::browser::benchmark::record(
                                    &benchmark_id,
                                    "pool_initialize",
                                    serde_json::json!({"duration_ns": crate::browser::benchmark::now_ns().saturating_sub(initialization_started)}),
                                );
                            }
                            let _ = sender.send(GpuCompletion {
                                acknowledger: Some(acknowledger),
                                result: Ok(Vec::new()),
                                imported: Some(PoolImportCompletion { document, pool_generation, result }),
                            }).await;
                        }).detach();
                        return Vec::new();
                    }
                    let outcome = self.consumer.intake(message, fds, &mut self.importer);
                    self.on_intake(outcome, cx)
                }
                HostEvent::Lost(reason) => {
                    self.session = None;
                    self.presented = None;
                    self.resize_sent = None;
                    vec![PageSignal::Lost(reason)]
                }
                HostEvent::Stopped => Vec::new(),
            }
        }

        fn on_native(&mut self, value: &Value) -> Vec<PageSignal> {
            let kind = value.get("native").and_then(Value::as_str).unwrap_or("");
            if kind == "agent_navigation_committed" {
                let Some(operation) = value
                    .get("operation")
                    .and_then(Value::as_str)
                    .and_then(|operation| OperationId::try_from(operation.to_owned()).ok())
                else {
                    return Vec::new();
                };
                let Some(session) = value
                    .get("session")
                    .cloned()
                    .and_then(|session| serde_json::from_value::<BrowserSession>(session).ok())
                    .filter(|session| {
                        session.document.browser == self.browser
                            && self.session.as_ref().is_some_and(|current| {
                                session.document.owner == current.document.owner
                                    && session.document.generation > current.document.generation
                            })
                    })
                else {
                    return Vec::new();
                };
                self.screenshots.clear();
                self.last_document = Some(session.document.clone());
                self.consumer.set_document(session.document.clone());
                self.presented = None;
                self.resize_sent = None;
                self.session = Some(session.clone());
                return vec![
                    PageSignal::State(session),
                    PageSignal::OperationCompleted {
                        operation: operation.clone(),
                        result: Ok(Event::Completed { operation }),
                    },
                ];
            }
            if matches!(
                kind,
                "agent_navigation_failed" | "agent_navigation_cancelled"
            ) {
                let Some(_document) = value
                    .get("document")
                    .cloned()
                    .and_then(|document| serde_json::from_value::<Document>(document).ok())
                    .filter(|document| Some(document) == self.document())
                else {
                    return Vec::new();
                };
                let Some(operation) = value
                    .get("operation")
                    .and_then(Value::as_str)
                    .and_then(|operation| OperationId::try_from(operation.to_owned()).ok())
                else {
                    return Vec::new();
                };
                let result = Err(BrowserError::Unavailable);
                return vec![
                    PageSignal::LoadFailed(
                        value
                            .get("reason")
                            .and_then(Value::as_str)
                            .filter(|reason| reason.len() <= 256)
                            .unwrap_or("agent navigation was cancelled")
                            .to_owned(),
                    ),
                    PageSignal::OperationCompleted { operation, result },
                ];
            }
            if kind == "screenshot_chunk" {
                return self.on_screenshot_chunk(value);
            }
            if kind == "screenshot_failed" {
                let Some(_document) = value
                    .get("document")
                    .cloned()
                    .and_then(|document| serde_json::from_value::<Document>(document).ok())
                    .filter(|document| Some(document) == self.document())
                else {
                    return Vec::new();
                };
                let Some(operation) = value
                    .get("operation")
                    .and_then(Value::as_str)
                    .and_then(|operation| OperationId::try_from(operation.to_owned()).ok())
                else {
                    return Vec::new();
                };
                self.screenshots.remove(&operation);
                return vec![PageSignal::OperationCompleted {
                    operation,
                    result: Err(screenshot_failure_error(
                        value
                            .get("reason")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )),
                }];
            }
            if matches!(
                kind,
                "external_open" | "renderer_crashed" | "certificate_error"
            ) {
                let document = value
                    .get("document")
                    .and_then(|value| serde_json::from_value::<Document>(value.clone()).ok());
                if document.is_none() || document.as_ref() != self.document() {
                    return Vec::new();
                }
                return match kind {
                    "renderer_crashed" => vec![PageSignal::Lost(
                        "Renderer crashed; reload the page to recover".into(),
                    )],
                    "certificate_error" => vec![PageSignal::LoadFailed(
                        "The page certificate could not be verified".into(),
                    )],
                    _ => value
                        .get("url")
                        .and_then(Value::as_str)
                        .filter(|url| {
                            !url.is_empty()
                                && url.len() <= paneflow_browser_protocol::MAX_URL_BYTES
                                && !url.contains('\0')
                        })
                        .map(|url| PageSignal::ExternalOpen(url.to_owned()))
                        .into_iter()
                        .collect(),
                };
            }

            if matches!(kind, "close_cancelled" | "fullscreen" | "find_result") {
                let document = value
                    .get("document")
                    .and_then(|value| serde_json::from_value::<Document>(value.clone()).ok());
                if document.is_none() || document.as_ref() != self.document() {
                    return Vec::new();
                }
                return match kind {
                    "close_cancelled" => vec![PageSignal::CloseCancelled],
                    "fullscreen" => value
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .map(PageSignal::Fullscreen)
                        .into_iter()
                        .collect(),
                    _ => value
                        .get("count")
                        .and_then(Value::as_i64)
                        .and_then(|count| i32::try_from(count).ok())
                        .zip(
                            value
                                .get("active")
                                .and_then(Value::as_i64)
                                .and_then(|active| i32::try_from(active).ok()),
                        )
                        .map(|(count, active)| PageSignal::FindResult { count, active })
                        .into_iter()
                        .collect(),
                };
            }

            if matches!(
                kind,
                "file_picker" | "download_destination" | "download_progress"
            ) {
                let document = value
                    .get("document")
                    .and_then(|value| serde_json::from_value::<Document>(value.clone()).ok());
                if document.is_none()
                    || document.as_ref() != self.document()
                    || value
                        .get("request")
                        .and_then(Value::as_u64)
                        .is_none_or(|request| request == 0)
                {
                    return Vec::new();
                }
                if value.get("suggested_name").is_some_and(|name| {
                    name.as_str()
                        .is_none_or(|name| name.len() > 1024 || name.contains('\0'))
                }) {
                    return Vec::new();
                }
                return vec![PageSignal::Transfer(value.clone())];
            }

            if matches!(
                kind,
                "web_dialog"
                    | "web_dialog_closed"
                    | "accessibility_tree"
                    | "accessibility_location"
                    | "accessibility_unavailable"
                    | "popup_requested"
            ) {
                let Some(document) = value
                    .get("document")
                    .and_then(|value| serde_json::from_value::<Document>(value.clone()).ok())
                    .filter(|document| Some(document) == self.document())
                else {
                    return Vec::new();
                };
                if kind == "web_dialog_closed" {
                    return value
                        .get("request")
                        .and_then(serde_json::Value::as_u64)
                        .filter(|request| *request > 0)
                        .map(|request| PageSignal::WebDialogClosed { request })
                        .into_iter()
                        .collect();
                }
                if kind.starts_with("accessibility_") {
                    return vec![PageSignal::Accessibility {
                        document,
                        kind: kind.to_owned(),
                        value: value.clone(),
                    }];
                }
                if kind == "popup_requested" {
                    return value
                        .get("url")
                        .and_then(Value::as_str)
                        .filter(|url| paneflow_browser_protocol::validate_url(url).is_ok())
                        .map(|url| PageSignal::PopupRequested(url.to_owned()))
                        .into_iter()
                        .collect();
                }
                let bounded_text = |key: &str| {
                    value
                        .get(key)
                        .and_then(Value::as_str)
                        .filter(|text| text.len() <= 8192 && !text.contains('\0'))
                        .map(str::to_owned)
                };
                if let (
                    Some(request),
                    Some(kind),
                    Some(origin),
                    Some(message),
                    Some(default_text),
                ) = (
                    value
                        .get("request")
                        .and_then(Value::as_u64)
                        .filter(|request| *request > 0),
                    bounded_text("kind"),
                    bounded_text("origin"),
                    bounded_text("message"),
                    bounded_text("default_text"),
                ) {
                    return vec![PageSignal::WebDialog {
                        document,
                        request,
                        kind,
                        origin,
                        message,
                        default_text,
                    }];
                }
                return Vec::new();
            }

            if matches!(kind, "agent_console" | "agent_network") {
                let Some(document) = value
                    .get("document")
                    .and_then(|value| serde_json::from_value::<Document>(value.clone()).ok())
                    .filter(|document| Some(document) == self.document())
                else {
                    return Vec::new();
                };
                let payload = value.get("value").cloned().unwrap_or_else(|| value.clone());
                return if kind == "agent_console" {
                    vec![PageSignal::Console {
                        document,
                        value: payload,
                    }]
                } else {
                    vec![PageSignal::Network {
                        document,
                        value: payload,
                    }]
                };
            }

            if matches!(
                kind,
                "pool_ready_accepted"
                    | "pool_ready_rejected"
                    | "frame_ack_received"
                    | "drop_enter"
                    | "drop_ack"
                    | "drop_dispatched"
                    | "drop_timeout"
            ) {
                crate::browser::benchmark::record(&self.benchmark_id, kind, value.clone());
            }
            if matches!(kind, "resize_host" | "resize_capture" | "resize_pool") {
                crate::browser::benchmark::record(
                    &self.benchmark_id,
                    kind,
                    serde_json::json!({"host_at_ns":value.get("at_ns"), "generation":value.get("generation"), "width":value.get("width"), "height":value.get("height")}),
                );
            }
            if crate::browser::benchmark::enabled()
                && matches!(
                    kind,
                    "loading" | "loaded" | "load_failed" | "created" | "closed"
                )
            {
                crate::browser::benchmark::record(
                    &self.benchmark_id,
                    kind,
                    serde_json::json!({"loading": value.get("is_loading")}),
                );
            }
            let text = |key: &str| {
                value
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let flag = |key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);
            if matches!(kind, "ime_selection" | "ime_bounds") {
                let document = value
                    .get("document")
                    .and_then(|v| serde_json::from_value::<Document>(v.clone()).ok());
                if document.as_ref() != self.document() || document.is_none() {
                    return Vec::new();
                }
                let snapshot = value.get("snapshot").and_then(|v| {
                    serde_json::from_value::<super::super::ime::ImeSnapshot>(v.clone()).ok()
                });
                return snapshot
                    .filter(|s| s.valid())
                    .map(|s| {
                        vec![if kind == "ime_selection" {
                            PageSignal::ImeSelection(s)
                        } else {
                            PageSignal::ImeBounds(s)
                        }]
                    })
                    .unwrap_or_default();
            }
            match value.get("native").and_then(Value::as_str) {
                Some("clipboard") => {
                    if let (Some(request), Some(text)) = (
                        value.get("request").and_then(Value::as_u64),
                        value.get("text").and_then(Value::as_str),
                    ) && request != 0
                        && paneflow_browser_protocol::clipboard_text_is_valid(text)
                    {
                        return vec![PageSignal::Clipboard {
                            request,
                            text: text.to_owned(),
                        }];
                    }
                    return Vec::new();
                }
                Some("context_menu") => {
                    let parsed = (|| {
                        let request = value
                            .get("request")?
                            .as_u64()
                            .filter(|request| *request != 0)?;
                        let x = i32::try_from(value.get("x")?.as_i64()?).ok()?;
                        let y = i32::try_from(value.get("y")?.as_i64()?).ok()?;
                        let raw = value.get("items")?.as_array()?;
                        if raw.len() > 64 {
                            return None;
                        }
                        let items: Vec<super::ContextMenuItem> =
                            serde_json::from_value(value.get("items")?.clone()).ok()?;
                        if items.iter().any(|item| {
                            item.command < 0
                                || item.label.chars().count() > 256
                                || item.label.chars().any(char::is_control)
                        }) {
                            return None;
                        }
                        Some(PageSignal::ContextMenu {
                            request,
                            x,
                            y,
                            items,
                        })
                    })();
                    return parsed.into_iter().collect();
                }
                Some("context_menu_closed") => {
                    return value
                        .get("request")
                        .and_then(Value::as_u64)
                        .map(|request| PageSignal::ContextMenuClosed { request })
                        .into_iter()
                        .collect();
                }
                _ => {}
            }
            match kind {
                "created" => vec![PageSignal::Created],
                "loading" => vec![PageSignal::Loading {
                    loading: flag("is_loading"),
                    can_go_back: flag("can_go_back"),
                    can_go_forward: flag("can_go_forward"),
                }],
                "cursor" => value
                    .get("style")
                    .cloned()
                    .and_then(|style| serde_json::from_value(style).ok())
                    .map(PageSignal::Cursor)
                    .into_iter()
                    .collect(),
                "title" => vec![PageSignal::Title(text("title"))],
                "address" => vec![PageSignal::Address(text("url"))],
                "loaded" => vec![PageSignal::Loaded],
                "load_failed" if value.get("main").and_then(Value::as_bool) != Some(false) => {
                    let detail = text("error_text");
                    vec![PageSignal::LoadFailed(if detail.is_empty() {
                        "The page failed to load".to_string()
                    } else {
                        detail
                    })]
                }
                _ => Vec::new(),
            }
        }

        fn on_screenshot_chunk(&mut self, value: &Value) -> Vec<PageSignal> {
            let Some(document) = value
                .get("document")
                .cloned()
                .and_then(|document| serde_json::from_value::<Document>(document).ok())
                .filter(|document| Some(document) == self.document())
            else {
                return Vec::new();
            };
            let Some(operation) = value
                .get("operation")
                .and_then(Value::as_str)
                .and_then(|operation| OperationId::try_from(operation.to_owned()).ok())
            else {
                return Vec::new();
            };
            let Some(index) = value
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|index| usize::try_from(index).ok())
            else {
                return Vec::new();
            };
            let Some(count) = value
                .get("count")
                .and_then(Value::as_u64)
                .and_then(|count| usize::try_from(count).ok())
                .filter(|count| *count > 0)
            else {
                return Vec::new();
            };
            let Some(data) = value.get("data").and_then(Value::as_str) else {
                return Vec::new();
            };
            let width = value
                .get("width")
                .and_then(Value::as_u64)
                .and_then(|width| u32::try_from(width).ok())
                .filter(|width| *width > 0);
            let height = value
                .get("height")
                .and_then(Value::as_u64)
                .and_then(|height| u32::try_from(height).ok())
                .filter(|height| *height > 0);
            if count
                > ((MAX_AGENT_CAPTURE_BYTES * 4).div_ceil(3) + 4)
                    .div_ceil(MAX_AGENT_CAPTURE_CHUNK_BYTES)
                || index >= count
                || data.len() > MAX_AGENT_CAPTURE_CHUNK_BYTES
                || !data.is_ascii()
                || width.is_none()
                || height.is_none()
                || value.get("final").and_then(Value::as_bool).is_none()
            {
                self.screenshots.remove(&operation);
                return vec![PageSignal::OperationCompleted {
                    operation: operation.clone(),
                    result: Err(BrowserError::TooLarge),
                }];
            }
            let mime = value
                .get("mime")
                .and_then(Value::as_str)
                .filter(|mime| *mime == "image/png")
                .unwrap_or_default()
                .to_owned();
            if mime.is_empty() {
                self.screenshots.remove(&operation);
                return vec![PageSignal::OperationCompleted {
                    operation: operation.clone(),
                    result: Err(BrowserError::InvalidMessage),
                }];
            }
            let assembly = self
                .screenshots
                .entry(operation.clone())
                .or_insert_with(|| ScreenshotAssembly {
                    document: document.clone(),
                    mime: mime.clone(),
                    width: width.unwrap_or_default(),
                    height: height.unwrap_or_default(),
                    chunks: vec![None; count],
                    encoded_bytes: 0,
                });
            if assembly.document != document
                || assembly.mime != mime
                || assembly.width != width.unwrap_or_default()
                || assembly.height != height.unwrap_or_default()
                || assembly.chunks.len() != count
            {
                self.screenshots.remove(&operation);
                return vec![PageSignal::OperationCompleted {
                    operation: operation.clone(),
                    result: Err(BrowserError::StaleGeneration),
                }];
            }
            if let Some(previous) = &assembly.chunks[index] {
                if previous != data {
                    self.screenshots.remove(&operation);
                    return vec![PageSignal::OperationCompleted {
                        operation: operation.clone(),
                        result: Err(BrowserError::InvalidMessage),
                    }];
                }
            } else {
                assembly.encoded_bytes = assembly.encoded_bytes.saturating_add(data.len());
                if assembly.encoded_bytes > (MAX_AGENT_CAPTURE_BYTES * 4).div_ceil(3) + 4 {
                    self.screenshots.remove(&operation);
                    return vec![PageSignal::OperationCompleted {
                        operation: operation.clone(),
                        result: Err(BrowserError::TooLarge),
                    }];
                }
                assembly.chunks[index] = Some(data.to_owned());
            }
            let final_chunk = value.get("final").and_then(Value::as_bool).unwrap_or(false);
            if !final_chunk || assembly.chunks.iter().any(Option::is_none) {
                return Vec::new();
            }
            let Some(assembly) = self.screenshots.remove(&operation) else {
                return Vec::new();
            };
            let mut data = String::with_capacity(assembly.encoded_bytes);
            for chunk in assembly.chunks {
                let Some(chunk) = chunk else {
                    return vec![PageSignal::OperationCompleted {
                        operation: operation.clone(),
                        result: Err(BrowserError::InvalidMessage),
                    }];
                };
                data.push_str(&chunk);
            }
            vec![PageSignal::OperationCompleted {
                operation: operation.clone(),
                result: Ok(Event::Screenshot {
                    mime: assembly.mime,
                    width: assembly.width,
                    height: assembly.height,
                    data,
                }),
            }]
        }

        fn acknowledger(&self) -> Result<FrameAcknowledger, String> {
            self.host
                .supervisor
                .frame_acknowledger()
                .map_err(|error| format!("no host connection for GPU acknowledgement: {error:?}"))
        }

        fn on_intake(&mut self, outcome: Intake, cx: &mut App) -> Vec<PageSignal> {
            match outcome {
                Intake::PoolRejected {
                    document,
                    pool_generation,
                } => match self.acknowledger().and_then(|acknowledger| {
                    acknowledger
                        .ack(FrameAck::PoolRejected {
                            document,
                            pool_generation,
                        })
                        .map_err(|error| format!("pool rejection not delivered: {error:?}"))
                }) {
                    Ok(()) => Vec::new(),
                    Err(error) => vec![PageSignal::Fatal(error)],
                },
                Intake::PoolImported {
                    document,
                    pool_generation,
                } => {
                    let acknowledger = match self.acknowledger() {
                        Ok(acknowledger) => acknowledger,
                        Err(error) => return vec![PageSignal::Fatal(error)],
                    };
                    let Some(surfaces) = self
                        .consumer
                        .pool_textures(&document, pool_generation)
                        .map(<[_]>::to_vec)
                    else {
                        return vec![PageSignal::Fatal(
                            "pool initialization lost its textures".into(),
                        )];
                    };
                    let initialization =
                        match external_sync::initialize(self.context.clone(), surfaces) {
                            Ok(initialization) => initialization,
                            Err(error) => return vec![PageSignal::Fatal(error)],
                        };
                    let sender = self.completions.clone();
                    cx.background_spawn(async move {
                        let result = initialization.await.map(|()| {
                            vec![FrameAck::PoolReady {
                                document,
                                pool_generation,
                            }]
                        });
                        let _ = sender
                            .send(GpuCompletion {
                                acknowledger: Some(acknowledger),
                                result,
                                imported: None,
                            })
                            .await;
                    })
                    .detach();
                    Vec::new()
                }
                Intake::Presented(identity) => {
                    if let Some((geometry, _)) = self.presented {
                        let expected = (
                            (geometry.width * geometry.scale_percent).div_ceil(100),
                            (geometry.height * geometry.scale_percent).div_ceil(100),
                        );
                        if self.consumer.current_size() == Some(expected)
                            && let Some((_, started)) = self.resize_sent.take()
                        {
                            crate::browser::benchmark::record(
                                &self.benchmark_id,
                                "resize_ready",
                                serde_json::json!({"generation": self.presentation_generation, "duration_ns": started.elapsed().as_nanos() as u64, "width": geometry.width, "height": geometry.height}),
                            );
                        }
                    }
                    let Some((surface, _, _)) = self.consumer.current() else {
                        return vec![PageSignal::Fatal(
                            "frame acquisition lost its texture".into(),
                        )];
                    };
                    match self.importer.acquire(identity, surface.clone()) {
                        Ok(()) => vec![PageSignal::Repaint],
                        Err(error) => vec![PageSignal::Fatal(error)],
                    }
                }
                Intake::Ignored(reason) => {
                    if reason == "pool imported" {
                        vec![PageSignal::Repaint]
                    } else {
                        Vec::new()
                    }
                }
                Intake::Fatal(reason) => vec![PageSignal::Fatal(reason)],
            }
        }

        pub fn on_gpu_completion(&mut self, mut completion: GpuCompletion) -> Result<(), String> {
            if let Some(acknowledger) = &completion.acknowledger
                && !self.host.supervisor.has_frame_connection(acknowledger)
            {
                return Ok(());
            }
            let mut acks = std::mem::replace(&mut completion.result, Ok(Vec::new()))?;
            if let Some(PoolImportCompletion {
                document,
                pool_generation,
                result,
            }) = completion.imported.take()
            {
                match self
                    .consumer
                    .complete_pool(&document, pool_generation, result, false)
                {
                    Intake::PoolImported {
                        document,
                        pool_generation,
                    } => {
                        acks.push(FrameAck::PoolReady {
                            document,
                            pool_generation,
                        });
                    }
                    Intake::PoolRejected {
                        document,
                        pool_generation,
                    } => {
                        acks.push(FrameAck::PoolRejected {
                            document,
                            pool_generation,
                        });
                    }
                    Intake::Ignored(_) => return Ok(()),
                    Intake::Fatal(error) => return Err(error),
                    Intake::Presented(_) => return Err("pool import returned a frame".into()),
                }
            }
            let acknowledger = completion
                .acknowledger
                .take()
                .ok_or("GPU acknowledgement has no host connection")?;
            for ack in acks {
                if let FrameAck::PoolReady {
                    document,
                    pool_generation,
                } = &ack
                {
                    self.consumer.initialized(document, *pool_generation)?;
                }
                let benchmark_ack =
                    crate::browser::benchmark::enabled().then(|| serde_json::json!(&ack));
                acknowledger
                    .ack(ack)
                    .map_err(|error| format!("GPU acknowledgement not delivered: {error:?}"))?;
                if let Some(ack) = benchmark_ack {
                    crate::browser::benchmark::record(&self.benchmark_id, "gpu_ack_sent", ack);
                }
            }
            Ok(())
        }

        pub fn schedule_releases(
            &mut self,
            window: &mut Window,
            cx: &mut App,
        ) -> Result<(), String> {
            if !self.consumer.has_pending_releases() {
                return Ok(());
            }
            let acknowledger = self.acknowledger()?;
            let acks = self.consumer.take_releases();
            let surfaces = self.importer.release_surfaces(&acks);
            let context = self.context.clone();
            let device = context.device.clone();
            let queue = context.queue.clone();
            let sender = self.completions.clone();
            window.defer(cx, move |_window, cx| {
                if !surfaces.is_empty() {
                    match external_sync::barrier(
                        &context,
                        &surfaces,
                        external_sync::Transfer::Release,
                    ) {
                        Ok(command) => {
                            queue.submit([command]);
                        }
                        Err(error) => {
                            cx.background_spawn(async move {
                                let _ = sender
                                    .send(GpuCompletion {
                                        acknowledger: Some(acknowledger),
                                        result: Err(error),
                                        imported: None,
                                    })
                                    .await;
                            })
                            .detach();
                            return;
                        }
                    }
                }
                let (done, completed) = std::sync::mpsc::channel::<()>();
                queue.on_submitted_work_done(move || {
                    drop(surfaces);
                    let _ = done.send(());
                });
                cx.background_spawn(async move {
                    let completion = smol::unblock(move || {
                        wait_for_gpu_completion(
                            || {
                                device
                                    .poll(wgpu::PollType::Poll)
                                    .map(|_| ())
                                    .map_err(|error| error.to_string())
                            },
                            completed,
                            Duration::from_secs(2),
                        )
                    })
                    .await;
                    let _ = sender
                        .send(GpuCompletion {
                            acknowledger: Some(acknowledger),
                            result: completion.map(|()| acks),
                            imported: None,
                        })
                        .await;
                })
                .detach();
            });
            Ok(())
        }

        pub fn present_if_needed(
            &mut self,
            geometry: Geometry,
            visible: bool,
        ) -> Result<bool, BrowserError> {
            let Some(session) = &self.session else {
                return Ok(false);
            };
            if session.state == SessionState::Dormant {
                return Ok(false);
            }
            if self.presented == Some((geometry, visible)) {
                return Ok(true);
            }
            let resize = self
                .presented
                .is_none_or(|(previous, _)| previous != geometry);
            if resize
                && visible
                && !resize_may_be_sent(self.consumer.pool_count(), self.resize_sent.is_some())
            {
                return Ok(false);
            }
            let generation = self.presentation_generation + 1;
            let presentation = BrowserPresentation {
                mounted: true,
                visible,
                width: geometry.width.max(1),
                height: geometry.height.max(1),
                generation,
                scale_percent: geometry.scale_percent,
            };
            let document = session.document.clone();
            self.send(Command::Present {
                document,
                presentation,
            })?;
            self.presentation_generation = generation;
            self.presented = Some((geometry, visible));
            if resize {
                self.resize_sent = Some((self.consumer.stats().frames_received, Instant::now()));
                crate::browser::benchmark::record(
                    &self.benchmark_id,
                    "resize_sent",
                    serde_json::json!({"generation": generation, "width": geometry.width, "height": geometry.height, "scale": geometry.scale_percent}),
                );
            }
            Ok(true)
        }

        pub fn shutdown(mut self) {
            let document = self.document().cloned();
            let mut acks = self.consumer.take_releases();
            if let (Some(document), Some((_, identity, _))) =
                (&self.last_document, self.consumer.current())
            {
                acks.push(FrameAck::Release {
                    document: document.clone(),
                    pool_generation: identity.pool_generation,
                    buffer: identity.buffer,
                    sequence: identity.sequence,
                });
            }
            let surfaces = self.importer.release_surfaces(&acks);
            let context = self.context.clone();
            let acknowledger = self.acknowledger().ok();
            let released = if surfaces.is_empty() {
                true
            } else {
                external_sync::barrier(&context, &surfaces, external_sync::Transfer::Release)
                    .map(|command| {
                        context.queue.submit([command]);
                    })
                    .is_ok()
            };
            let retiring_host = self.host.supervisor.clone();
            context.queue.on_submitted_work_done(move || {
                if released && let Some(acknowledger) = acknowledger {
                    for ack in acks {
                        let _ = acknowledger.ack(ack);
                    }
                }
                drop(surfaces);
                self.host.clone().unsubscribe(&self.browser, document);
                drop(self);
            });
            std::thread::Builder::new()
                .name("browser-page-gpu-retire".into())
                .spawn(move || {
                    if let Err(error) = context.device.poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: Some(Duration::from_millis(
                            paneflow_browser_protocol::RETIRE_DEADLINE_MS,
                        )),
                    }) {
                        log::error!(
                            "browser GPU retirement timed out; resources remain retained: {error}"
                        );
                        retiring_host.terminate();
                    }
                })
                .ok();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::resize_may_be_sent;
        use super::screenshot_failure_error;
        use paneflow_browser_protocol::BrowserError;
        #[test]
        fn resize_requires_both_a_completed_transition_and_a_free_pool_budget() {
            assert!(resize_may_be_sent(0, false));
            assert!(resize_may_be_sent(1, false));
            assert!(!resize_may_be_sent(1, true));
            assert!(!resize_may_be_sent(2, false));
            assert!(!resize_may_be_sent(2, true));
        }

        #[test]
        fn screenshot_failures_preserve_bounded_error_categories() {
            assert_eq!(
                screenshot_failure_error("native screenshot exceeds its bound"),
                BrowserError::TooLarge
            );
            assert_eq!(
                screenshot_failure_error("native screenshot result is invalid"),
                BrowserError::InvalidMessage
            );
            assert_eq!(
                screenshot_failure_error("native capture adapter unavailable"),
                BrowserError::Unavailable
            );
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::sync::Arc;

    use gpui::{App, AppContext, ExternalSurface, Window};
    use gpui_platform::gpui_windows::ExternalSurfaceContext;
    use paneflow_browser_protocol::{
        BrowserError, BrowserPresentation, BrowserSession, Command, Document, Event, FrameAck,
        FrameMessage, OperationId, SessionState,
    };
    use serde_json::Value;

    use super::{Geometry, PageConfig, PageSignal};
    use crate::browser::presentation::{FrameConsumer, Intake, PoolLayout, TextureImporter};
    use crate::browser::supervisor::{FrameAcknowledger, HostConfig, HostEvent};
    use crate::browser::windows::D3dImporter;

    struct PoolImportCompletion {
        document: Document,
        pool_generation: u64,
        result: Result<Vec<ExternalSurface>, String>,
    }

    pub struct GpuCompletion {
        acknowledger: Option<FrameAcknowledger>,
        imported: Option<PoolImportCompletion>,
        release_error: Option<String>,
    }

    impl Drop for GpuCompletion {
        fn drop(&mut self) {
            let Some(acknowledger) = &self.acknowledger else {
                return;
            };
            if let Some(imported) = &self.imported {
                let _ = acknowledger.ack(FrameAck::PoolRejected {
                    document: imported.document.clone(),
                    pool_generation: imported.pool_generation,
                });
            }
        }
    }

    pub struct PageStart {
        pub page: LivePage,
        pub host_events: smol::channel::Receiver<HostEvent>,
        pub gpu_completions: smol::channel::Receiver<GpuCompletion>,
    }

    pub struct LivePage {
        host: Arc<crate::browser::profile_host::ProfileHost>,
        browser: paneflow_browser_protocol::BrowserId,
        context: Arc<ExternalSurfaceContext>,
        importer: D3dImporter,
        consumer: FrameConsumer<ExternalSurface>,
        completions: smol::channel::Sender<GpuCompletion>,
        session: Option<BrowserSession>,
        presented: Option<(Geometry, bool)>,
        presentation_generation: u64,
        benchmark_id: String,
    }

    fn resize_may_be_sent(live_pools: usize, pending: bool) -> bool {
        live_pools <= 1 && !pending
    }

    fn bounded_text(value: &Value, key: &str) -> Option<String> {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| text.len() <= 8192 && !text.contains('\0'))
            .map(str::to_owned)
    }

    fn context_menu_signals(value: &Value, kind: &str) -> Vec<PageSignal> {
        let Some(request) = value
            .get("request")
            .and_then(Value::as_u64)
            .filter(|request| *request != 0)
        else {
            return Vec::new();
        };
        if kind == "context_menu_closed" {
            return vec![PageSignal::ContextMenuClosed { request }];
        }
        let parsed = (|| {
            let x = i32::try_from(value.get("x")?.as_i64()?).ok()?;
            let y = i32::try_from(value.get("y")?.as_i64()?).ok()?;
            let items: Vec<super::ContextMenuItem> =
                serde_json::from_value(value.get("items")?.clone()).ok()?;
            if items.len() > 64
                || items.iter().any(|item| {
                    item.command < 0
                        || item.label.chars().count() > 256
                        || item.label.chars().any(char::is_control)
                })
            {
                return None;
            }
            Some(PageSignal::ContextMenu {
                request,
                x,
                y,
                items,
            })
        })();
        parsed.into_iter().collect()
    }

    fn transfer_signals(value: &Value) -> Vec<PageSignal> {
        if value
            .get("request")
            .and_then(Value::as_u64)
            .is_none_or(|request| request == 0)
        {
            return Vec::new();
        }
        if value.get("suggested_name").is_some_and(|name| {
            name.as_str()
                .is_none_or(|name| name.len() > 1024 || name.contains('\0'))
        }) {
            return Vec::new();
        }
        vec![PageSignal::Transfer(value.clone())]
    }

    fn web_signals(value: &Value, kind: &str, document: Document) -> Vec<PageSignal> {
        if kind == "web_dialog_closed" {
            return value
                .get("request")
                .and_then(Value::as_u64)
                .filter(|request| *request > 0)
                .map(|request| PageSignal::WebDialogClosed { request })
                .into_iter()
                .collect();
        }
        if kind.starts_with("accessibility_") {
            return vec![PageSignal::Accessibility {
                document,
                kind: kind.to_owned(),
                value: value.clone(),
            }];
        }
        if kind == "popup_requested" {
            return value
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| paneflow_browser_protocol::validate_url(url).is_ok())
                .map(|url| PageSignal::PopupRequested(url.to_owned()))
                .into_iter()
                .collect();
        }
        let (Some(request), Some(dialog_kind), Some(origin), Some(message), Some(default_text)) = (
            value
                .get("request")
                .and_then(Value::as_u64)
                .filter(|request| *request > 0),
            bounded_text(value, "kind"),
            bounded_text(value, "origin"),
            bounded_text(value, "message"),
            bounded_text(value, "default_text"),
        ) else {
            return Vec::new();
        };
        vec![PageSignal::WebDialog {
            document,
            request,
            kind: dialog_kind,
            origin,
            message,
            default_text,
        }]
    }

    fn native_signals(value: &Value, live: Option<&Document>) -> Vec<PageSignal> {
        let kind = value.get("native").and_then(Value::as_str).unwrap_or("");
        let scoped = matches!(
            kind,
            "context_menu"
                | "context_menu_closed"
                | "ime_selection"
                | "ime_bounds"
                | "file_picker"
                | "download_destination"
                | "download_progress"
                | "web_dialog"
                | "web_dialog_closed"
                | "accessibility_tree"
                | "accessibility_location"
                | "accessibility_unavailable"
                | "popup_requested"
                | "external_open"
                | "renderer_crashed"
                | "certificate_error"
                | "close_cancelled"
                | "fullscreen"
                | "find_result"
        );
        let document = value
            .get("document")
            .and_then(|value| serde_json::from_value::<Document>(value.clone()).ok())
            .filter(|document| Some(document) == live);
        let Some(document) = document else {
            return if scoped {
                Vec::new()
            } else {
                unscoped_signals(value, kind)
            };
        };
        match kind {
            "context_menu" | "context_menu_closed" => context_menu_signals(value, kind),
            "ime_selection" | "ime_bounds" => value
                .get("snapshot")
                .and_then(|value| {
                    serde_json::from_value::<super::super::ime::ImeSnapshot>(value.clone()).ok()
                })
                .filter(|snapshot| snapshot.valid())
                .map(|snapshot| {
                    vec![if kind == "ime_selection" {
                        PageSignal::ImeSelection(snapshot)
                    } else {
                        PageSignal::ImeBounds(snapshot)
                    }]
                })
                .unwrap_or_default(),
            "file_picker" | "download_destination" | "download_progress" => transfer_signals(value),
            "web_dialog"
            | "web_dialog_closed"
            | "accessibility_tree"
            | "accessibility_location"
            | "accessibility_unavailable"
            | "popup_requested" => web_signals(value, kind, document),
            "renderer_crashed" => vec![PageSignal::Lost(
                "Renderer crashed; reload the page to recover".into(),
            )],
            "certificate_error" => vec![PageSignal::LoadFailed(
                "The page certificate could not be verified".into(),
            )],
            "external_open" => value
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| {
                    !url.is_empty()
                        && url.len() <= paneflow_browser_protocol::MAX_URL_BYTES
                        && !url.contains('\0')
                })
                .map(|url| PageSignal::ExternalOpen(url.to_owned()))
                .into_iter()
                .collect(),
            "close_cancelled" => vec![PageSignal::CloseCancelled],
            "fullscreen" => value
                .get("enabled")
                .and_then(Value::as_bool)
                .map(PageSignal::Fullscreen)
                .into_iter()
                .collect(),
            "find_result" => value
                .get("count")
                .and_then(Value::as_i64)
                .and_then(|count| i32::try_from(count).ok())
                .zip(
                    value
                        .get("active")
                        .and_then(Value::as_i64)
                        .and_then(|active| i32::try_from(active).ok()),
                )
                .map(|(count, active)| PageSignal::FindResult { count, active })
                .into_iter()
                .collect(),
            _ => unscoped_signals(value, kind),
        }
    }

    fn unscoped_signals(value: &Value, kind: &str) -> Vec<PageSignal> {
        match kind {
            "cursor" => value
                .get("style")
                .cloned()
                .and_then(|style| serde_json::from_value(style).ok())
                .map(PageSignal::Cursor)
                .into_iter()
                .collect(),
            "created" => vec![PageSignal::Created],
            "loading" => vec![PageSignal::Loading {
                loading: value
                    .get("is_loading")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                can_go_back: value
                    .get("can_go_back")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                can_go_forward: value
                    .get("can_go_forward")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            }],
            "title" => vec![PageSignal::Title(
                value
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )],
            "address" => vec![PageSignal::Address(
                value
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )],
            "loaded" => vec![PageSignal::Loaded],
            "load_failed" => vec![PageSignal::LoadFailed(
                value
                    .get("error_text")
                    .and_then(Value::as_str)
                    .unwrap_or("The page failed to load")
                    .to_string(),
            )],
            _ => Vec::new(),
        }
    }

    impl LivePage {
        pub fn start(window: &Window, cx: &App, page: PageConfig) -> Result<PageStart, String> {
            let PageConfig {
                benchmark_id,
                host_binary,
                runtime_root,
                stage_root,
                profile_dir,
                origin,
                owner,
                runtime_check,
            } = page;
            crate::browser_qualification::observe(window);
            let context = window
                .external_surface_context()
                .and_then(|context| context.downcast::<ExternalSurfaceContext>().ok())
                .ok_or("this window exposes no DirectX external surface context")?;
            let importer = D3dImporter::new(context.clone());
            let browser = paneflow_browser_protocol::BrowserId::try_from(benchmark_id.clone())
                .map_err(str::to_owned)?;
            let config = HostConfig {
                tracing: std::env::var_os("PANEFLOW_BROWSER_TRACE_DIR").is_some(),
                host_binary,
                runtime_root,
                stage_root,
                profile_dir,
                origin,
                owner,
                frames: true,
                check: runtime_check,
            };
            let (host, host_events, new_host) =
                crate::browser::profile_host::ProfileHost::subscribe(config, browser.clone())?;
            if new_host && let Some(runtime) = cx.try_global::<crate::browser::BrowserRuntime>() {
                runtime.register_page(host.supervisor.clone());
            }
            let (completions, gpu_completions) = smol::channel::bounded(32);
            Ok(PageStart {
                page: Self {
                    host,
                    browser,
                    context,
                    importer,
                    consumer: FrameConsumer::default(),
                    completions,
                    session: None,
                    presented: None,
                    presentation_generation: 0,
                    benchmark_id,
                },
                host_events,
                gpu_completions,
            })
        }

        pub fn document(&self) -> Option<&Document> {
            self.session.as_ref().map(|session| &session.document)
        }

        pub fn surface(&self) -> Option<ExternalSurface> {
            self.consumer.current().map(|(surface, identity, timing)| {
                crate::browser_qualification::record(
                    "browser_scene",
                    serde_json::json!({
                        "page":self.benchmark_id,"pool_generation":identity.pool_generation,
                        "buffer":identity.buffer,"sequence":identity.sequence,
                        "callback_ns":timing.callback_ns,"ready_ns":timing.ready_ns
                    }),
                );
                surface.clone()
            })
        }

        pub fn send(&self, command: Command) -> Result<OperationId, BrowserError> {
            self.host.send(&self.browser, command)
        }

        pub fn send_to_document(
            &self,
            build: impl FnOnce(Document) -> Command,
        ) -> Result<OperationId, BrowserError> {
            let document = self.document().cloned().ok_or(BrowserError::Unavailable)?;
            self.send(build(document))
        }

        pub fn agent_screenshot(&self) -> Result<OperationId, BrowserError> {
            self.send_to_document(|document| Command::Screenshot { document })
        }

        pub fn on_host_event(&mut self, event: HostEvent, cx: &mut App) -> Vec<PageSignal> {
            match event {
                HostEvent::Ready(info) => {
                    crate::browser::benchmark::record(
                        &self.benchmark_id,
                        "host_ready",
                        serde_json::json!({
                            "pid": info.pid,
                            "availability": format!("{:?}", info.availability),
                            "contract_version": info.contract_version,
                            "presentation": info.presentation,
                            "initialized": info.initialized,
                        }),
                    );
                    vec![PageSignal::Ready]
                }
                HostEvent::Reply(reply) => {
                    let operation = reply.operation;
                    let result = reply.result;
                    let signals = match &result {
                        Ok(Event::State { session }) | Ok(Event::NavigationStarted { session }) => {
                            self.consumer.set_document(session.document.clone());
                            if !session.presentation.mounted {
                                self.presented = None;
                            }
                            self.session = Some(session.clone());
                            vec![PageSignal::State(session.clone())]
                        }
                        Ok(Event::Closed { .. }) => {
                            self.session = None;
                            vec![PageSignal::Closed]
                        }
                        Ok(_) => Vec::new(),
                        Err(error) => vec![PageSignal::Refused(*error)],
                    };
                    let mut signals = signals;
                    signals.push(PageSignal::OperationCompleted { operation, result });
                    signals
                }
                HostEvent::Native(value) => self.on_native(&value),
                HostEvent::Frame(message, handles) => match message {
                    FrameMessage::PoolCreated {
                        document,
                        pool_generation,
                        width,
                        height,
                        format,
                        modifier,
                        buffers,
                        ..
                    } => {
                        let layout = PoolLayout {
                            generation: pool_generation,
                            width,
                            height,
                            format,
                            modifier,
                            buffers,
                        };
                        if let Err(outcome) =
                            self.consumer
                                .reserve_pool(document.clone(), layout.clone(), true)
                        {
                            D3dImporter::close_handles(handles);
                            return self.on_intake(outcome);
                        }
                        let acknowledger = match self.acknowledger() {
                            Ok(acknowledger) => acknowledger,
                            Err(error) => {
                                D3dImporter::close_handles(handles);
                                return vec![PageSignal::Fatal(error)];
                            }
                        };
                        let sender = self.completions.clone();
                        let mut importer = D3dImporter::new(self.context.clone());
                        cx.background_spawn(async move {
                            let result =
                                smol::unblock(move || importer.import(&layout, handles)).await;
                            let _ = sender
                                .send(GpuCompletion {
                                    acknowledger: Some(acknowledger),
                                    release_error: None,
                                    imported: Some(PoolImportCompletion {
                                        document,
                                        pool_generation,
                                        result,
                                    }),
                                })
                                .await;
                        })
                        .detach();
                        Vec::new()
                    }
                    message => {
                        let outcome = self.consumer.intake(message, handles, &mut self.importer);
                        self.on_intake(outcome)
                    }
                },
                HostEvent::Lost(reason) => {
                    self.session = None;
                    self.consumer.host_lost();
                    self.presented = None;
                    vec![PageSignal::Lost(reason)]
                }
                HostEvent::Stopped => Vec::new(),
            }
        }

        fn on_native(&self, value: &Value) -> Vec<PageSignal> {
            native_signals(value, self.document())
        }

        fn acknowledger(&self) -> Result<FrameAcknowledger, String> {
            self.host
                .supervisor
                .frame_acknowledger()
                .map_err(|error| format!("no host connection for GPU acknowledgement: {error:?}"))
        }

        fn on_intake(&mut self, outcome: Intake) -> Vec<PageSignal> {
            match outcome {
                Intake::PoolRejected {
                    document,
                    pool_generation,
                } => match self.acknowledger().and_then(|acknowledger| {
                    acknowledger
                        .ack(FrameAck::PoolRejected {
                            document,
                            pool_generation,
                        })
                        .map_err(|error| format!("pool rejection not delivered: {error:?}"))
                }) {
                    Ok(()) => Vec::new(),
                    Err(error) => vec![PageSignal::Fatal(error)],
                },
                Intake::Presented(_) | Intake::Ignored("pool imported") => {
                    vec![PageSignal::Repaint]
                }
                Intake::Ignored(_) => Vec::new(),
                Intake::PoolImported { .. } => Vec::new(),
                Intake::Fatal(reason) => vec![PageSignal::Fatal(reason)],
            }
        }

        pub fn on_gpu_completion(&mut self, mut completion: GpuCompletion) -> Result<(), String> {
            if let Some(acknowledger) = &completion.acknowledger
                && !self.host.supervisor.has_frame_connection(acknowledger)
            {
                return Ok(());
            }
            if let Some(error) = completion.release_error.take() {
                return Err(error);
            }
            let Some(imported) = completion.imported.take() else {
                return Ok(());
            };
            let outcome = self.consumer.complete_pool(
                &imported.document,
                imported.pool_generation,
                imported.result,
                false,
            );
            let acknowledger = completion
                .acknowledger
                .take()
                .ok_or("GPU acknowledgement has no host connection")?;
            match outcome {
                Intake::PoolImported {
                    document,
                    pool_generation,
                } => {
                    self.consumer.initialized(&document, pool_generation)?;
                    acknowledger
                        .ack(FrameAck::PoolReady {
                            document,
                            pool_generation,
                        })
                        .map_err(|error| format!("GPU acknowledgement not delivered: {error:?}"))
                }
                Intake::PoolRejected {
                    document,
                    pool_generation,
                } => acknowledger
                    .ack(FrameAck::PoolRejected {
                        document,
                        pool_generation,
                    })
                    .map_err(|error| format!("pool rejection not delivered: {error:?}")),
                Intake::Ignored("pool imported") => Ok(()),
                Intake::Ignored(_) => Ok(()),
                Intake::Fatal(error) => Err(error),
                Intake::Presented(_) => Err("pool import returned a frame".into()),
            }
        }

        pub fn schedule_releases(
            &mut self,
            window: &mut Window,
            cx: &mut App,
        ) -> Result<(), String> {
            if !self.consumer.has_pending_releases() {
                return Ok(());
            }
            let acknowledger = self.acknowledger()?;
            let releases = self.consumer.take_releases();
            let sender = self.completions.clone();
            window.defer(cx, move |_, cx| {
                cx.background_spawn(async move {
                    for ack in releases {
                        if let Err(error) = acknowledger.ack(ack) {
                            let _ = sender
                                .send(GpuCompletion {
                                    acknowledger: Some(acknowledger),
                                    imported: None,
                                    release_error: Some(format!(
                                        "frame release not delivered: {error:?}"
                                    )),
                                })
                                .await;
                            break;
                        }
                    }
                })
                .detach();
            });
            Ok(())
        }

        pub fn present_if_needed(
            &mut self,
            geometry: Geometry,
            visible: bool,
        ) -> Result<bool, BrowserError> {
            let Some(session) = &self.session else {
                return Ok(false);
            };
            if session.state == SessionState::Dormant {
                return Ok(false);
            }
            if self.presented == Some((geometry, visible)) {
                return Ok(true);
            }
            let resize = self
                .presented
                .is_none_or(|(previous, _)| previous != geometry);
            if resize && visible && !resize_may_be_sent(self.consumer.pool_count(), false) {
                return Ok(false);
            }
            let generation = self.presentation_generation + 1;
            self.send(Command::Present {
                document: session.document.clone(),
                presentation: BrowserPresentation {
                    mounted: true,
                    visible,
                    width: geometry.width.max(1),
                    height: geometry.height.max(1),
                    generation,
                    scale_percent: geometry.scale_percent,
                },
            })?;
            self.presentation_generation = generation;
            self.presented = Some((geometry, visible));
            Ok(true)
        }

        pub fn shutdown(mut self) {
            if let Ok(acknowledger) = self.acknowledger() {
                for ack in self.consumer.take_releases() {
                    let _ = acknowledger.ack(ack);
                }
            }
            let document = self.document().cloned();
            self.host.unsubscribe(&self.browser, document);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{native_signals, resize_may_be_sent};
        use crate::browser::accessibility::AccessibilityTree;
        use crate::browser::page::PageSignal;
        use paneflow_browser_protocol::Document;
        use serde_json::{Value, json};

        fn document(generation: u64) -> Document {
            serde_json::from_value(json!({
                "owner": {"workspace": "w", "session": "s"},
                "browser": "b",
                "generation": generation,
            }))
            .unwrap()
        }

        fn signals(value: &Value) -> Vec<PageSignal> {
            native_signals(value, Some(&document(1)))
        }

        #[test]
        fn resize_requires_both_a_completed_transition_and_a_free_pool_budget() {
            assert!(resize_may_be_sent(0, false));
            assert!(resize_may_be_sent(1, false));
            assert!(!resize_may_be_sent(1, true));
            assert!(!resize_may_be_sent(2, false));
        }

        #[test]
        fn accessibility_updates_reach_the_shared_tree_for_the_live_document() {
            let live = document(1);
            let update = json!({
                "native": "accessibility_tree",
                "document": live,
                "value": {
                    "ax_tree_id": "tree",
                    "updates": [{
                        "root_id": 1,
                        "tree_data": {"focus_id": 1},
                        "nodes": [{"id": 1, "role": "rootWebArea", "child_ids": []}],
                    }],
                },
            });
            let produced = signals(&update);
            let Some(PageSignal::Accessibility {
                document: signalled,
                kind,
                value,
            }) = produced.into_iter().next()
            else {
                panic!("the Windows host event produced no accessibility signal");
            };
            assert_eq!(signalled, live);
            assert_eq!(kind, "accessibility_tree");
            let mut tree = AccessibilityTree::default();
            tree.reset(Some(live.clone()));
            assert!(tree.apply(&live, &kind, &value));
            assert!(tree.agent_snapshot().is_some());
        }

        #[test]
        fn accessibility_updates_for_a_replaced_document_are_dropped() {
            let stale = json!({
                "native": "accessibility_tree",
                "document": document(2),
                "value": {"ax_tree_id": "tree"},
            });
            assert!(signals(&stale).is_empty());
            let unavailable = json!({
                "native": "accessibility_unavailable",
                "document": document(1),
                "reason": "tree_update_limit",
            });
            assert!(matches!(
                signals(&unavailable).as_slice(),
                [PageSignal::Accessibility { kind, .. }] if kind == "accessibility_unavailable"
            ));
        }

        #[test]
        fn permission_and_script_dialogs_carry_their_origin_and_close_by_request() {
            let request = json!({
                "native": "web_dialog",
                "document": document(1),
                "request": 9,
                "kind": "permission",
                "origin": "https://example.com/",
                "message": "Autoriser la géolocalisation pour ce document ?",
                "default_text": "",
            });
            assert!(matches!(
                signals(&request).as_slice(),
                [PageSignal::WebDialog { request: 9, kind, origin, .. }]
                    if kind == "permission" && origin == "https://example.com/"
            ));
            let closed = json!({
                "native": "web_dialog_closed",
                "document": document(1),
                "request": 9,
            });
            assert!(matches!(
                signals(&closed).as_slice(),
                [PageSignal::WebDialogClosed { request: 9 }]
            ));
            let stale = json!({
                "native": "web_dialog",
                "document": document(2),
                "request": 9,
                "kind": "permission",
                "origin": "https://example.com/",
                "message": "m",
                "default_text": "",
            });
            assert!(signals(&stale).is_empty());
        }

        #[test]
        fn file_pickers_and_downloads_require_a_bounded_request_and_name() {
            for kind in ["file_picker", "download_destination", "download_progress"] {
                let event = json!({
                    "native": kind,
                    "document": document(1),
                    "request": 3,
                    "suggested_name": "report.pdf",
                });
                assert!(matches!(
                    signals(&event).as_slice(),
                    [PageSignal::Transfer(_)]
                ));
                assert!(
                    signals(&json!({
                        "native": kind,
                        "document": document(1),
                        "request": 0,
                    }))
                    .is_empty()
                );
                assert!(
                    signals(&json!({
                        "native": kind,
                        "document": document(1),
                        "request": 3,
                        "suggested_name": "a".repeat(1025),
                    }))
                    .is_empty()
                );
            }
        }

        #[test]
        fn external_protocols_popups_and_certificates_refuse_unsupported_targets() {
            assert!(matches!(
                signals(&json!({
                    "native": "external_open",
                    "document": document(1),
                    "url": "mailto:user@example.com",
                }))
                .as_slice(),
                [PageSignal::ExternalOpen(url)] if url == "mailto:user@example.com"
            ));
            assert!(
                signals(&json!({
                    "native": "external_open",
                    "document": document(1),
                    "url": "",
                }))
                .is_empty()
            );
            assert!(matches!(
                signals(&json!({
                    "native": "popup_requested",
                    "document": document(1),
                    "url": "https://example.com/",
                }))
                .as_slice(),
                [PageSignal::PopupRequested(_)]
            ));
            assert!(
                signals(&json!({
                    "native": "popup_requested",
                    "document": document(1),
                    "url": "file:///etc/passwd",
                }))
                .is_empty()
            );
            assert!(matches!(
                signals(&json!({
                    "native": "certificate_error",
                    "document": document(1),
                    "error_code": -202,
                }))
                .as_slice(),
                [PageSignal::LoadFailed(_)]
            ));
            assert!(matches!(
                signals(&json!({
                    "native": "renderer_crashed",
                    "document": document(1),
                    "status": 2,
                    "error_code": 0,
                }))
                .as_slice(),
                [PageSignal::Lost(_)]
            ));
        }

        #[test]
        fn dock_controls_reach_the_view_from_the_live_document_only() {
            assert!(matches!(
                signals(&json!({
                    "native": "find_result",
                    "document": document(1),
                    "count": 4,
                    "active": 2,
                    "final": true,
                }))
                .as_slice(),
                [PageSignal::FindResult {
                    count: 4,
                    active: 2
                }]
            ));
            assert!(matches!(
                signals(&json!({
                    "native": "fullscreen",
                    "document": document(1),
                    "enabled": true,
                }))
                .as_slice(),
                [PageSignal::Fullscreen(true)]
            ));
            assert!(matches!(
                signals(&json!({
                    "native": "close_cancelled",
                    "document": document(1),
                }))
                .as_slice(),
                [PageSignal::CloseCancelled]
            ));
            assert!(
                signals(&json!({
                    "native": "find_result",
                    "document": document(2),
                    "count": 4,
                    "active": 2,
                }))
                .is_empty()
            );
            assert!(matches!(
                signals(&json!({"native": "cursor", "style": "PointingHand"})).as_slice(),
                [PageSignal::Cursor(_)]
            ));
        }
    }
}

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
mod stub {
    use gpui::{App, Window};
    use paneflow_browser_protocol::{BrowserError, Command, Document, OperationId};

    use super::{Geometry, PageConfig, PageSignal};

    pub struct GpuCompletion;

    pub struct PageStart {
        pub page: LivePage,
        pub host_events: smol::channel::Receiver<()>,
        pub gpu_completions: smol::channel::Receiver<GpuCompletion>,
    }

    pub struct LivePage {}

    impl LivePage {
        pub fn start(_window: &Window, _cx: &App, _page: PageConfig) -> Result<PageStart, String> {
            Err("the browser has a Linux adapter only".to_string())
        }

        pub fn document(&self) -> Option<&Document> {
            None
        }

        pub fn surface(&self) -> Option<()> {
            None
        }

        pub fn send(&self, _command: Command) -> Result<OperationId, BrowserError> {
            Err(BrowserError::Unavailable)
        }

        pub fn send_to_document(
            &self,
            _build: impl FnOnce(Document) -> Command,
        ) -> Result<OperationId, BrowserError> {
            Err(BrowserError::Unavailable)
        }

        pub fn agent_screenshot(&self) -> Result<OperationId, BrowserError> {
            Err(BrowserError::Unavailable)
        }

        pub fn on_host_event(&mut self, _event: (), _cx: &mut App) -> Vec<PageSignal> {
            Vec::new()
        }

        pub fn on_gpu_completion(&mut self, _completion: GpuCompletion) -> Result<(), String> {
            Ok(())
        }

        pub fn schedule_releases(
            &mut self,
            _window: &mut Window,
            _cx: &mut App,
        ) -> Result<(), String> {
            Ok(())
        }

        pub fn present_if_needed(
            &mut self,
            _geometry: Geometry,
            _visible: bool,
        ) -> Result<bool, BrowserError> {
            Ok(false)
        }

        pub fn shutdown(self) {}
    }
}
