pub struct PageConfig {
    pub benchmark_id: String,
    pub host_binary: std::path::PathBuf,
    pub runtime_root: std::path::PathBuf,
    pub stage_root: std::path::PathBuf,
    pub profile_dir: std::path::PathBuf,
    pub origin: String,
    pub owner: paneflow_browser_protocol::Owner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    pub scale_percent: u32,
}

#[derive(Clone, Debug)]
pub enum PageSignal {
    Ready,
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

#[cfg(not(target_os = "linux"))]
pub use stub::LivePage;

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use gpui::{App, AppContext, ExternalSurface, Window};
    use gpui_platform::gpui_wgpu::{ExternalSurfaceContext, wgpu};
    use paneflow_browser_protocol::{
        BrowserError, BrowserPresentation, BrowserSession, Command, Document, Event, FrameAck,
        OperationId, SessionState,
    };
    use serde_json::Value;

    use super::{Geometry, PageConfig, PageSignal};
    use crate::browser::linux::{DmabufImporter, external_sync};
    use crate::browser::presentation::{FrameConsumer, Intake, PoolLayout, TextureImporter};
    use crate::browser::prototype::wait_for_gpu_completion;
    use crate::browser::supervisor::{
        FrameAcknowledger, HostConfig, HostEvent, HostSupervisor, Ozone, RuntimeCheck,
        SHUTDOWN_GRACE,
    };

    struct PoolImportCompletion {
        document: Document,
        pool_generation: u64,
        result: Result<Vec<ExternalSurface>, String>,
    }

    pub struct GpuCompletion {
        acknowledger: Option<FrameAcknowledger>,
        result: Result<Vec<FrameAck>, String>,
        imported: Option<PoolImportCompletion>,
    }

    pub struct PageStart {
        pub page: LivePage,
        pub host_events: smol::channel::Receiver<HostEvent>,
        pub gpu_completions: smol::channel::Receiver<GpuCompletion>,
    }

    pub struct LivePage {
        supervisor: HostSupervisor,
        context: Arc<ExternalSurfaceContext>,
        importer: DmabufImporter,
        consumer: FrameConsumer<ExternalSurface>,
        completions: smol::channel::Sender<GpuCompletion>,
        session: Option<BrowserSession>,
        presented: Option<(Geometry, bool)>,
        presentation_generation: u64,
        resize_sent: Option<(u64, Instant)>,
        benchmark_id: String,
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
            let (supervisor, host_events) = HostSupervisor::channel();
            let (completions, gpu_completions) = smol::channel::bounded(64);
            if let Some(runtime) = cx.try_global::<crate::browser::BrowserRuntime>() {
                runtime.register_page(supervisor.clone());
            }
            supervisor.activate(config);
            Ok(PageStart {
                page: Self {
                    supervisor,
                    context,
                    importer,
                    consumer: FrameConsumer::default(),
                    completions,
                    session: None,
                    presented: None,
                    presentation_generation: 0,
                    resize_sent: None,
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
            self.consumer
                .current()
                .map(|(surface, _, _)| surface.clone())
        }

        pub fn send(&self, command: Command) -> Result<OperationId, BrowserError> {
            self.supervisor.send(command)
        }

        pub fn send_to_document(
            &self,
            build: impl FnOnce(Document) -> Command,
        ) -> Result<OperationId, BrowserError> {
            let document = self.document().cloned().ok_or(BrowserError::Unavailable)?;
            self.send(build(document))
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
                HostEvent::Reply(reply) => match reply.result {
                    Ok(Event::State { session }) => {
                        self.consumer.set_document(session.document.clone());
                        if !session.presentation.mounted {
                            self.presented = None;
                            self.resize_sent = None;
                        }
                        self.session = Some(session.clone());
                        vec![PageSignal::State(session)]
                    }
                    Ok(Event::Closed { .. }) => {
                        self.session = None;
                        vec![PageSignal::Closed]
                    }
                    Ok(_) => Vec::new(),
                    Err(error) => vec![PageSignal::Refused(error)],
                },
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
                    self.consumer.host_lost();
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

        fn acknowledger(&self) -> Result<FrameAcknowledger, String> {
            self.supervisor
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

        pub fn on_gpu_completion(&mut self, completion: GpuCompletion) -> Result<(), String> {
            if let Some(acknowledger) = &completion.acknowledger
                && !self.supervisor.has_frame_connection(acknowledger)
            {
                return Ok(());
            }
            let mut acks = completion.result?;
            if let Some(PoolImportCompletion {
                document,
                pool_generation,
                result,
            }) = completion.imported
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
                .ok_or("GPU acknowledgement has no host connection")?;
            for ack in acks {
                if let FrameAck::PoolReady {
                    document,
                    pool_generation,
                } = &ack
                {
                    self.consumer.initialized(document, *pool_generation)?;
                }
                acknowledger
                    .ack(ack)
                    .map_err(|error| format!("GPU acknowledgement not delivered: {error:?}"))?;
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

        pub fn shutdown(self) {
            let supervisor = self.supervisor;
            let grace = if std::env::var_os("PANEFLOW_BROWSER_TRACE_DIR").is_some() {
                Duration::from_secs(60)
            } else {
                SHUTDOWN_GRACE
            };
            std::thread::Builder::new()
                .name("browser-page-shutdown".into())
                .spawn(move || supervisor.shutdown(grace))
                .ok();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::resize_may_be_sent;
        #[test]
        fn resize_requires_both_a_completed_transition_and_a_free_pool_budget() {
            assert!(resize_may_be_sent(0, false));
            assert!(resize_may_be_sent(1, false));
            assert!(!resize_may_be_sent(1, true));
            assert!(!resize_may_be_sent(2, false));
            assert!(!resize_may_be_sent(2, true));
        }
    }
}

#[cfg(not(target_os = "linux"))]
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

        pub fn surface(&self) -> Option<gpui::ExternalSurface> {
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
