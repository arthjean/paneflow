use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::{Duration, Instant};

use clap::Parser;
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, KeyDownEvent, KeyUpEvent, Modifiers, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, ScrollDelta, ScrollWheelEvent, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};
use gpui_platform::gpui_wgpu::{ExternalSurfaceContext, wgpu};
use paneflow_browser_protocol::{
    BrowserId, BrowserPresentation, BrowserSession, Command, Document, Event, FrameAck, InputEvent,
    KeyKind, MODIFIER_LEFT_MOUSE, MouseButton, Owner, ProfileId, SessionId, SessionState,
    WorkspaceId,
};
use serde_json::{Value, json};

use super::input::{button_modifier, key_code, modifiers, mouse_button};
use super::linux::{DmabufImporter, PatternProducer};
use super::presentation::{FrameConsumer, Intake};
use super::supervisor::{
    FrameAcknowledger, HOST_ENV, HostConfig, HostEvent, HostState, HostSupervisor, Ozone,
    RUNTIME_ENV, RuntimeCheck, SHUTDOWN_GRACE,
};
use super::{BrowserRuntime, origin_of};
use crate::browser_qualification::{self, BrowserFrame, now_ns};

const TICK: Duration = Duration::from_millis(16);
const STEP_TIMEOUT: Duration = Duration::from_secs(20);
const RUN_TIMEOUT: Duration = Duration::from_secs(240);
const TRACE_EXPORT_TIMEOUT: Duration = Duration::from_secs(60);
const RESIZE_INSET: f32 = 96.0;

struct GpuCompletion {
    acknowledger: Option<FrameAcknowledger>,
    result: Result<Vec<FrameAck>, String>,
}

#[derive(Parser, Debug)]
#[command(
    name = "paneflow browser-prototype",
    about = "Present a page through the GPUI external-surface path with a separate CEF host (Linux)"
)]
struct Options {
    #[arg(long, help = "Page to open: a loopback http origin or an https origin")]
    url: Option<String>,
    #[arg(
        long,
        default_value = "cef",
        help = "cef: frames from the external host; pattern: frames produced on the window device"
    )]
    source: String,
    #[arg(
        long,
        help = "Run the host with the X11 Ozone platform instead of Wayland"
    )]
    x11: bool,
    #[arg(
        long,
        help = "Comma-separated steps executed in order: input,resize,scale,host-loss"
    )]
    scenario: Option<String>,
    #[arg(long, help = "JSON Lines diagnostics log; absolute path, created new")]
    log: Option<PathBuf>,
    #[arg(
        long,
        default_value_t = 5,
        help = "Seconds to keep presenting after the last step"
    )]
    hold: u64,
    #[arg(long, default_value_t = 1280)]
    width: u32,
    #[arg(long, default_value_t = 800)]
    height: u32,
    #[arg(skip)]
    m1: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Input,
    Resize,
    Scale,
    HostLoss,
}

impl Step {
    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "input" => Some(Self::Input),
            "resize" => Some(Self::Resize),
            "scale" => Some(Self::Scale),
            "host-loss" => Some(Self::HostLoss),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Resize => "resize",
            Self::Scale => "scale",
            Self::HostLoss => "host-loss",
        }
    }
}

#[derive(Clone)]
struct Logger {
    sender: Option<SyncSender<Value>>,
}

impl Logger {
    fn new(path: Option<&PathBuf>) -> Result<Self, String> {
        let Some(path) = path else {
            return Ok(Self { sender: None });
        };
        if !path.is_absolute() {
            return Err("--log must be an absolute path".to_string());
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let (sender, receiver) = sync_channel::<Value>(4096);
        std::thread::Builder::new()
            .name("browser-prototype-log".into())
            .spawn(move || {
                let mut writer = BufWriter::new(file);
                while let Ok(value) = receiver.recv() {
                    if serde_json::to_writer(&mut writer, &value).is_err()
                        || writeln!(writer).is_err()
                        || writer.flush().is_err()
                    {
                        return;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            sender: Some(sender),
        })
    }

    fn emit_releases(&self, event: &str, acks: &[FrameAck], stage_ns: u64) {
        self.emit(
            event,
            json!({ "clock": "CLOCK_MONOTONIC", "stage_ns": stage_ns, "acks": acks }),
        );
    }

    fn emit(&self, event: &str, mut fields: Value) {
        fields["event"] = json!(event);
        fields["at_ns"] = json!(now_ns());
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(fields);
        } else if let Ok(text) = serde_json::to_string(&fields) {
            eprintln!("{text}");
        }
    }
}

enum Source {
    Cef { importer: Option<DmabufImporter> },
    Pattern { producer: Option<PatternProducer> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Starting,
    Ready,
    Lost,
    Finished,
}

struct Geometry {
    width: u32,
    height: u32,
    scale_percent: u32,
}

struct PrototypeView {
    options: Arc<Options>,
    log: Logger,
    exit: Arc<AtomicI32>,
    focus: FocusHandle,
    source: Source,
    context: Option<Arc<ExternalSurfaceContext>>,
    consumer: FrameConsumer<gpui::ExternalSurface>,
    supervisor: Option<HostSupervisor>,
    config: Option<HostConfig>,
    owner: Owner,
    session: Option<BrowserSession>,
    presentation_generation: u64,
    presented: Option<Geometry>,
    scale_override: Option<u32>,
    inset: bool,
    phase: Phase,
    loaded: bool,
    fixture: Option<Value>,
    intake_frames: u64,
    current_frame: Option<BrowserFrame>,
    first_frame_ns: Option<u64>,
    generations_seen: u64,
    steps: VecDeque<Step>,
    step_started: Option<Instant>,
    step_stage: u8,
    step_marker: u64,
    hold_until: Option<Instant>,
    started: Instant,
    acks: smol::channel::Sender<GpuCompletion>,
    host_pids: Vec<u32>,
    failure: Option<String>,
    trace_finished: Option<smol::channel::Sender<Result<(), String>>>,
}

fn position(point: gpui::Point<Pixels>, inset: bool) -> (i32, i32) {
    let offset = if inset { RESIZE_INSET } else { 0.0 };
    (
        (f32::from(point.x) - offset).round() as i32,
        (f32::from(point.y) - offset).round() as i32,
    )
}

impl PrototypeView {
    fn new(
        options: Arc<Options>,
        log: Logger,
        exit: Arc<AtomicI32>,
        acks: smol::channel::Sender<GpuCompletion>,
        cx: &mut Context<Self>,
    ) -> Self {
        let steps = options
            .scenario
            .as_deref()
            .map(|list| {
                list.split(',')
                    .filter(|item| !item.trim().is_empty())
                    .filter_map(Step::parse)
                    .collect()
            })
            .unwrap_or_default();
        let source = if options.source == "pattern" {
            Source::Pattern { producer: None }
        } else {
            Source::Cef { importer: None }
        };
        let owner = Owner {
            workspace: WorkspaceId::try_from("prototype".to_string())
                .unwrap_or_else(|_| unreachable!()),
            session: SessionId::try_from("browser".to_string()).unwrap_or_else(|_| unreachable!()),
        };
        Self {
            options,
            log,
            exit,
            focus: cx.focus_handle(),
            source,
            context: None,
            consumer: FrameConsumer::default(),
            supervisor: None,
            config: None,
            owner,
            session: None,
            presentation_generation: 0,
            presented: None,
            scale_override: None,
            inset: false,
            phase: Phase::Idle,
            loaded: false,
            fixture: None,
            intake_frames: 0,
            current_frame: None,
            first_frame_ns: None,
            generations_seen: 0,
            steps,
            step_started: None,
            step_stage: 0,
            step_marker: 0,
            hold_until: None,
            started: Instant::now(),
            acks,
            host_pids: Vec::new(),
            failure: None,
            trace_finished: None,
        }
    }

    fn fail(&mut self, reason: String, cx: &mut Context<Self>) {
        if self.phase == Phase::Finished {
            if self.trace_finished.is_some() {
                self.log.emit("fatal", json!({ "reason": reason }));
                self.failure = Some(reason.clone());
                self.exit.store(1, Ordering::SeqCst);
                self.complete_trace(Err(reason));
            }
            return;
        }
        self.log.emit("fatal", json!({ "reason": reason }));
        self.failure = Some(reason);
        self.finish(cx);
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        if self.phase == Phase::Finished {
            return;
        }
        self.phase = Phase::Finished;
        let stats = self.consumer.stats();
        self.log.emit(
            "summary",
            json!({
                "status": if self.failure.is_some() { "FAILED" } else { "COMPLETED" },
                "failure": self.failure,
                "source": self.options.source,
                "ozone": if self.options.x11 { "x11" } else { "wayland" },
                "presented_frames": self.intake_frames,
                "intake_frames": self.intake_frames,
                "counter_boundary": "accepted external surface, not compositor presentation",
                "pool_generations": self.generations_seen,
                "first_frame_after_ms": self.first_frame_ns.map(|ns| ns.saturating_sub(self.started_ns()) / 1_000_000),
                "consumer": {
                    "outstanding": self.consumer.outstanding(),
                    "pools_created": stats.pools_created,
                    "pools_retired": stats.pools_retired,
                    "frames_received": stats.frames_received,
                    "frames_presented": stats.frames_presented,
                    "frames_ignored": stats.frames_ignored,
                    "releases_sent": stats.releases_sent,
                    "failures": stats.failures,
                },
                "host_pids": self.host_pids,
                "remaining_steps": self.steps.iter().map(|step| step.name()).collect::<Vec<_>>(),
            }),
        );
        self.exit
            .store(i32::from(self.failure.is_some()), Ordering::SeqCst);
        let trace_export = if self.options.m1
            && self.failure.is_none()
            && self.supervisor.is_some()
            && self.session.is_some()
        {
            let (sender, receiver) = smol::channel::bounded(1);
            self.trace_finished = Some(sender);
            Some(receiver)
        } else {
            None
        };
        if let (Some(supervisor), Some(session)) = (&self.supervisor, &self.session) {
            let result = supervisor.send(Command::Close {
                document: session.document.clone(),
            });
            if let Err(error) = result {
                self.complete_trace(Err(format!("host refused trace finalization: {error:?}")));
            }
        }
        let supervisor = self.supervisor.clone();
        let keep_application = self.options.m1;
        let log = self.log.clone();
        let exit = self.exit.clone();
        cx.spawn(async move |_this, cx| {
            if let Some(receiver) = trace_export
                && let Err(reason) = await_trace_export(receiver, TRACE_EXPORT_TIMEOUT).await
            {
                log.emit("fatal", json!({ "reason": reason }));
                exit.store(1, Ordering::SeqCst);
            }
            if let Some(supervisor) = supervisor {
                smol::unblock(move || supervisor.shutdown(SHUTDOWN_GRACE)).await;
            }
            if !keep_application {
                cx.update(|cx| cx.quit());
            }
        })
        .detach();
    }

    fn complete_trace(&mut self, result: Result<(), String>) {
        if let Some(sender) = self.trace_finished.take() {
            let _ = sender.try_send(result);
        }
    }

    fn started_ns(&self) -> u64 {
        now_ns().saturating_sub(self.started.elapsed().as_nanos() as u64)
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.context.is_some() || self.phase != Phase::Idle {
            return;
        }
        let Some(context) = window
            .external_surface_context()
            .and_then(|context| context.downcast::<ExternalSurfaceContext>().ok())
        else {
            self.fail(
                "browser unavailable: this window exposes no external surface context".to_string(),
                cx,
            );
            return;
        };
        let info = context.adapter.get_info();
        self.log.emit(
            "window",
            json!({
                "backend": format!("{:?}", info.backend),
                "adapter": info.name,
                "driver": info.driver,
                "driver_info": info.driver_info,
                "vendor_id": info.vendor,
                "device_id": info.device,
                "scale": window.scale_factor(),
                "wayland": std::env::var_os("WAYLAND_DISPLAY").is_some(),
                "display": std::env::var_os("DISPLAY").is_some(),
            }),
        );
        self.context = Some(context.clone());
        match &mut self.source {
            Source::Pattern { producer } => {
                let document = Document {
                    owner: self.owner.clone(),
                    browser: BrowserId::try_from("pattern".to_string())
                        .unwrap_or_else(|_| unreachable!()),
                    generation: 1,
                };
                self.consumer.set_document(document.clone());
                *producer = Some(PatternProducer::new(context, document));
                self.phase = Phase::Ready;
                self.loaded = true;
                self.log
                    .emit("source_ready", json!({ "source": "pattern" }));
            }
            Source::Cef { importer } => {
                let imported = match DmabufImporter::new(context) {
                    Ok(importer) => importer,
                    Err(error) => {
                        self.fail(format!("browser unavailable: {error}"), cx);
                        return;
                    }
                };
                self.log.emit(
                    "importer_ready",
                    json!({
                        "device": imported.device_name,
                        "vendor_id": imported.vendor_id,
                        "device_id": imported.device_id,
                        "extensions": imported.extensions,
                        "render_node": imported.render_node,
                    }),
                );
                let gpu = imported.gpu();
                let render_node = imported.render_node.clone();
                *importer = Some(imported);
                match self.host_config(gpu, render_node) {
                    Ok(config) => {
                        let runtime = cx.global::<BrowserRuntime>();
                        let supervisor = runtime.supervisor().clone();
                        let Some(receiver) = runtime.take_events() else {
                            self.fail(
                                "the browser runtime events are already consumed".to_string(),
                                cx,
                            );
                            return;
                        };
                        self.log.emit(
                            "host_config",
                            json!({
                                "ozone": format!("{:?}", config.ozone),
                                "gpu": config.gpu,
                                "render_node": config.render_node,
                                "host_stderr": config.profile_dir().join("host.stderr"),
                            }),
                        );
                        self.config = Some(config.clone());
                        self.supervisor = Some(supervisor.clone());
                        self.phase = Phase::Starting;
                        let state = supervisor.activate(config);
                        self.log
                            .emit("activate", json!({ "state": format!("{state:?}") }));
                        cx.spawn(async move |this, cx| {
                            while let Ok(event) = receiver.recv().await {
                                if this
                                    .update(cx, |view, cx| view.on_host_event(event, cx))
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        })
                        .detach();
                    }
                    Err(error) => self.fail(format!("browser unavailable: {error}"), cx),
                }
            }
        }
    }

    fn host_config(
        &self,
        gpu: (u32, u32),
        render_node: Option<PathBuf>,
    ) -> Result<HostConfig, String> {
        let url = self
            .options
            .url
            .as_deref()
            .ok_or("--url is required for the cef source")?;
        let origin = origin_of(url)?;
        let host_binary = match std::env::var_os(HOST_ENV) {
            Some(path) => PathBuf::from(path),
            None => std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("paneflow-browser-host")))
                .ok_or("cannot locate paneflow-browser-host next to the executable")?,
        };
        let runtime_root = std::env::var_os(RUNTIME_ENV)
            .map(PathBuf::from)
            .ok_or(format!(
                "{RUNTIME_ENV} must point at a verified CEF runtime"
            ))?;
        let stage_root = crate::runtime_paths::data_dir()
            .ok_or("no writable data directory for the browser host")?
            .join("browser");
        let profile_dir = HostConfig::witness_profile_dir(&stage_root, &origin);
        Ok(HostConfig {
            tracing: true,
            host_binary,
            runtime_root,
            stage_root,
            profile_dir,
            origin,
            owner: self.owner.clone(),
            ozone: if self.options.x11 {
                Ozone::X11
            } else {
                Ozone::Wayland
            },
            gpu: Some(gpu),
            render_node,
            frames: true,
            check: RuntimeCheck::Manifest,
        })
    }

    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        let Some(supervisor) = &self.supervisor else {
            return;
        };
        let label = match &command {
            Command::Create { .. } => "create",
            Command::Start { .. } => "start",
            Command::Present { .. } => "present",
            Command::Input { .. } => "input",
            Command::Close { .. } => "close",
            _ => "other",
        };
        match supervisor.send(command) {
            Ok(operation) => {
                if label != "input" {
                    self.log.emit(
                        "command",
                        json!({ "command": label, "operation": format!("{operation:?}") }),
                    );
                }
            }
            Err(error) => {
                if label != "input" {
                    self.fail(format!("host refused {label}: {error:?}"), cx);
                }
            }
        }
    }

    fn on_host_event(&mut self, event: HostEvent, cx: &mut Context<Self>) {
        match event {
            HostEvent::Ready(info) => {
                self.phase = Phase::Ready;
                self.host_pids.push(info.pid);
                self.log.emit(
                    "host_ready",
                    json!({
                        "pid": info.pid,
                        "availability": format!("{:?}", info.availability),
                        "contract_version": info.contract_version,
                        "presentation": info.presentation,
                        "initialized": info.initialized,
                    }),
                );
                let url = self.options.url.clone().unwrap_or_default();
                self.send(
                    Command::Create {
                        owner: self.owner.clone(),
                        browser: BrowserId::try_from("prototype".to_string())
                            .unwrap_or_else(|_| unreachable!()),
                        profile: ProfileId::try_from("prototype".to_string())
                            .unwrap_or_else(|_| unreachable!()),
                        url,
                        title: "Prototype".to_string(),
                    },
                    cx,
                );
            }
            HostEvent::Reply(reply) => match reply.result {
                Ok(Event::State { session }) => {
                    self.consumer.set_document(session.document.clone());
                    let state = session.state;
                    let mounted = session.presentation.mounted;
                    self.session = Some(session);
                    self.log.emit(
                        "state",
                        json!({ "state": format!("{state:?}"), "mounted": mounted }),
                    );
                    match state {
                        SessionState::Dormant => {
                            let document = self
                                .session
                                .as_ref()
                                .map(|session| session.document.clone());
                            if let Some(document) = document {
                                self.send(Command::Start { document }, cx);
                            }
                        }
                        _ if !mounted => {
                            self.presented = None;
                            cx.notify();
                        }
                        _ => (),
                    }
                }
                Ok(Event::Closed { .. }) => self.log.emit("closed", json!({})),
                Ok(_) => (),
                Err(error) => {
                    self.log
                        .emit("reply_error", json!({ "error": format!("{error:?}") }));
                    if self.session.is_none() {
                        self.fail(format!("host refused the page: {error:?}"), cx);
                    }
                }
            },
            HostEvent::Native(value) => {
                let kind = value.get("native").and_then(Value::as_str).unwrap_or("");
                match kind {
                    "loaded" => self.loaded = true,
                    "fixture_state" => self.fixture = value.get("state").cloned(),
                    "pool_created" => self.generations_seen += 1,
                    "trace_completed" => self.complete_trace(Ok(())),
                    "trace_failed" => self.complete_trace(Err("CEF trace export failed".into())),
                    _ => (),
                }
                if kind != "fixture_state"
                    || self.step_started.is_some()
                    || browser_qualification::enabled()
                {
                    self.log.emit("native", json!({ "native": value }));
                }
                if kind == "load_failed" {
                    self.fail("the page failed to load".to_string(), cx);
                }
            }
            HostEvent::Frame(message, fds) => {
                let intake_ns = now_ns();
                let Source::Cef {
                    importer: Some(importer),
                } = &mut self.source
                else {
                    return;
                };
                let outcome = self.consumer.intake(message, fds, importer);
                self.on_intake(outcome, intake_ns, cx);
            }
            HostEvent::Lost(reason) => {
                self.complete_trace(Err(format!("host lost before trace export: {reason}")));
                self.log.emit(
                    "host_lost",
                    json!({ "reason": reason, "surface_cleared": true }),
                );
                self.consumer.host_lost();
                self.session = None;
                self.loaded = false;
                self.fixture = None;
                self.presented = None;
                self.current_frame = None;
                self.phase = Phase::Lost;
                cx.notify();
                if self.steps.front() != Some(&Step::HostLoss) || self.step_started.is_none() {
                    self.fail(
                        format!("host lost outside the host-loss step: {reason}"),
                        cx,
                    );
                }
            }
            HostEvent::Stopped => {
                self.complete_trace(Err("host stopped before trace export".into()));
                self.log.emit("host_stopped", json!({}));
            }
        }
    }

    fn on_intake(&mut self, outcome: Intake, intake_ns: u64, cx: &mut Context<Self>) {
        match outcome {
            Intake::PoolRejected {
                document,
                pool_generation,
            } => {
                let acknowledger = self
                    .supervisor
                    .as_ref()
                    .and_then(|supervisor| supervisor.frame_acknowledger().ok());
                if let Err(error) = self.apply_acks(
                    vec![FrameAck::PoolRejected {
                        document,
                        pool_generation,
                    }],
                    acknowledger,
                ) {
                    self.fail(error, cx);
                }
            }
            Intake::PoolImported {
                document,
                pool_generation,
            } => {
                let Some(context) = self.context.clone() else {
                    self.fail("pool initialization has no GPU context".into(), cx);
                    return;
                };
                let Some(acknowledger) = self
                    .supervisor
                    .as_ref()
                    .and_then(|supervisor| supervisor.frame_acknowledger().ok())
                else {
                    self.fail("pool initialization has no host connection".into(), cx);
                    return;
                };
                let Some(surfaces) = self
                    .consumer
                    .pool_textures(&document, pool_generation)
                    .map(<[_]>::to_vec)
                else {
                    self.fail("pool initialization lost its textures".into(), cx);
                    return;
                };
                let initialization =
                    match super::linux::external_sync::initialize(context, surfaces) {
                        Ok(initialization) => initialization,
                        Err(error) => {
                            self.fail(error, cx);
                            return;
                        }
                    };
                self.log.emit("pool_init_submitted", json!({ "document": document, "pool_generation": pool_generation, "at_ns": now_ns() }));
                let sender = self.acks.clone();
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
                        })
                        .await;
                })
                .detach();
            }
            Intake::Presented(identity) => {
                if let Source::Cef {
                    importer: Some(importer),
                } = &mut self.source
                {
                    let Some((surface, _, _)) = self.consumer.current() else {
                        self.fail("frame acquisition lost its texture".into(), cx);
                        return;
                    };
                    if let Err(error) = importer.acquire(identity, surface.clone()) {
                        self.fail(error, cx);
                        return;
                    }
                }
                self.intake_frames += 1;
                if self.first_frame_ns.is_none() {
                    self.first_frame_ns = Some(intake_ns);
                }
                if let (Some(session), Some((_, identity, timing))) =
                    (&self.session, self.consumer.current())
                {
                    let frame = BrowserFrame {
                        document: session.document.clone(),
                        identity,
                        timing,
                        intake_ns,
                    };
                    browser_qualification::browser_intake(
                        &frame,
                        self.consumer.outstanding(),
                        self.consumer.pool_count(),
                    );
                    self.current_frame = Some(frame);
                }
                let verbose = browser_qualification::enabled()
                    || self.intake_frames <= 5
                    || self.intake_frames.is_multiple_of(30)
                    || self.step_started.is_some();
                if let Some((_, _, timing)) = self.consumer.current().filter(|_| verbose) {
                    self.log.emit(
                        "frame",
                        json!({
                            "pool_generation": identity.pool_generation,
                            "buffer": identity.buffer,
                            "sequence": identity.sequence,
                            "callback_ns": timing.callback_ns,
                            "ready_ns": timing.ready_ns,
                            "capture_timestamp_us": timing.capture_timestamp_us,
                            "capture_counter": timing.capture_counter,
                            "intake_ns": intake_ns,
                            "copy_us": timing.ready_ns.saturating_sub(timing.callback_ns) / 1000,
                            "transfer_us": intake_ns.saturating_sub(timing.ready_ns) / 1000,
                        }),
                    );
                }
                cx.notify();
            }
            Intake::Ignored(reason) => {
                if reason != "pool imported" {
                    self.log.emit("frame_ignored", json!({ "reason": reason }));
                } else {
                    self.log.emit(
                        "pool_imported",
                        json!({ "generation": self.consumer.active_generation(), "pools": self.consumer.pool_count() }),
                    );
                    cx.notify();
                }
            }
            Intake::Fatal(reason) => {
                self.fail(format!("browser error: {reason}"), cx);
            }
        }
    }

    fn on_gpu_completion(&mut self, completion: GpuCompletion, cx: &mut Context<Self>) {
        if let Some(acknowledger) = &completion.acknowledger
            && !self
                .supervisor
                .as_ref()
                .is_some_and(|supervisor| supervisor.has_frame_connection(acknowledger))
        {
            self.log.emit("stale_gpu_completion", json!({}));
            return;
        }
        match completion.result {
            Ok(acks) => {
                for ack in &acks {
                    if let FrameAck::PoolReady {
                        document,
                        pool_generation,
                    } = ack
                    {
                        if let Err(error) = self.consumer.initialized(document, *pool_generation) {
                            self.fail(error, cx);
                            return;
                        }
                        self.log.emit("pool_init_completed", json!({ "document": document, "pool_generation": pool_generation, "at_ns": now_ns() }));
                    }
                }
                if let Err(error) = self.apply_acks(acks, completion.acknowledger) {
                    self.fail(error, cx);
                }
            }
            Err(error) => self.fail(error, cx),
        }
    }

    fn apply_acks(
        &mut self,
        acks: Vec<FrameAck>,
        acknowledger: Option<FrameAcknowledger>,
    ) -> Result<(), String> {
        match &mut self.source {
            Source::Cef { .. } => {
                if let Some(acknowledger) = acknowledger {
                    for ack in acks {
                        let diagnostic_ack = (browser_qualification::enabled()
                            && self.log.sender.is_some())
                        .then(|| ack.clone());
                        let enqueue_start_ns = diagnostic_ack.as_ref().map(|_| now_ns());
                        match acknowledger.ack(ack) {
                            Ok(()) => {
                                if let Some(ack) = diagnostic_ack {
                                    let enqueue_end_ns = now_ns();
                                    self.log.emit(
                                        match &ack {
                                            FrameAck::PoolReady { .. } => "pool_ready_enqueued",
                                            FrameAck::PoolRejected { .. } => {
                                                "pool_rejected_enqueued"
                                            }
                                            FrameAck::Release { .. } => "release_ack_enqueued",
                                        },
                                        json!({
                                            "clock": "CLOCK_MONOTONIC",
                                            "enqueue_start_ns": enqueue_start_ns,
                                            "enqueue_end_ns": enqueue_end_ns,
                                            "ack": ack,
                                        }),
                                    );
                                }
                            }
                            Err(error) => {
                                let mut fields = json!({ "error": format!("{error:?}") });
                                if let Some(ack) = diagnostic_ack {
                                    fields["ack"] = json!(ack);
                                }
                                self.log.emit("ack_failed", fields);
                                return Err(format!(
                                    "GPU acknowledgement could not reach its host: {error:?}"
                                ));
                            }
                        }
                    }
                } else {
                    return Err("GPU acknowledgement has no host connection".into());
                }
            }
            Source::Pattern {
                producer: Some(producer),
            } => {
                for ack in &acks {
                    producer.release(ack);
                }
            }
            Source::Pattern { producer: None } => (),
        }
        Ok(())
    }

    fn schedule_releases(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.consumer.has_pending_releases() {
            return;
        }
        let acknowledger = if matches!(self.source, Source::Cef { .. }) {
            match self
                .supervisor
                .as_ref()
                .and_then(|supervisor| supervisor.frame_acknowledger().ok())
            {
                Some(acknowledger) => Some(acknowledger),
                None => {
                    self.fail("cannot bind GPU completion to its browser host".into(), cx);
                    return;
                }
            }
        } else {
            None
        };
        let Some(context) = &self.context else {
            return;
        };
        let acks = self.consumer.take_releases();
        let surfaces = match &mut self.source {
            Source::Cef {
                importer: Some(importer),
            } => importer.release_surfaces(&acks),
            _ => Vec::new(),
        };
        let release_context = context.clone();
        let diagnostics = (browser_qualification::enabled() && self.log.sender.is_some())
            .then(|| (self.log.clone(), acks.clone()));
        if let Some((log, releases)) = &diagnostics {
            log.emit_releases("releases_dequeued", releases, now_ns());
        }
        let device = context.device.clone();
        let queue = context.queue.clone();
        let sender = self.acks.clone();
        window.defer(cx, move |_window, cx| {
            if let Some((log, releases)) = &diagnostics {
                log.emit_releases("release_scene_replaced", releases, now_ns());
            }
            if !surfaces.is_empty() {
                match super::linux::external_sync::barrier(
                    &release_context,
                    &surfaces,
                    super::linux::external_sync::Transfer::Release,
                ) {
                    Ok(command) => {
                        queue.submit([command]);
                    }
                    Err(error) => {
                        cx.background_spawn(async move {
                            let _ = sender
                                .send(GpuCompletion {
                                    acknowledger,
                                    result: Err(error),
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
                if let Some((log, releases)) = diagnostics {
                    log.emit_releases("release_gpu_completed", &releases, now_ns());
                }
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
                        acknowledger,
                        result: completion.map(|()| acks),
                    })
                    .await;
            })
            .detach();
        });
    }

    fn geometry(&self, window: &Window) -> Geometry {
        let viewport = window.viewport_size();
        let inset = if self.inset { RESIZE_INSET * 2.0 } else { 0.0 };
        let scale = self
            .scale_override
            .unwrap_or((window.scale_factor() * 100.0).round() as u32)
            .clamp(50, 400);
        Geometry {
            width: (f32::from(viewport.width) - inset).max(1.0).round() as u32,
            height: (f32::from(viewport.height) - inset).max(1.0).round() as u32,
            scale_percent: scale,
        }
    }

    fn present_if_needed(&mut self, window: &Window, cx: &mut Context<Self>) {
        let geometry = self.geometry(window);
        let changed = self.presented.as_ref().is_none_or(|current| {
            current.width != geometry.width
                || current.height != geometry.height
                || current.scale_percent != geometry.scale_percent
        });
        if !changed {
            return;
        }
        match &mut self.source {
            Source::Pattern {
                producer: Some(producer),
            } => {
                let physical_width = geometry.width * geometry.scale_percent / 100;
                let physical_height = geometry.height * geometry.scale_percent / 100;
                let messages = producer.resize(physical_width, physical_height);
                self.presented = Some(geometry);
                self.log.emit("present", json!({ "width": physical_width, "height": physical_height, "source": "pattern" }));
                for message in messages {
                    let Source::Pattern {
                        producer: Some(producer),
                    } = &mut self.source
                    else {
                        return;
                    };
                    if matches!(
                        message,
                        paneflow_browser_protocol::FrameMessage::PoolCreated { .. }
                    ) {
                        self.generations_seen += 1;
                    }
                    let outcome = self.consumer.intake(message, Vec::new(), producer);
                    self.on_intake(outcome, now_ns(), cx);
                }
            }
            Source::Cef { .. } => {
                let Some(session) = &self.session else {
                    return;
                };
                if session.state == SessionState::Dormant {
                    return;
                }
                self.presentation_generation += 1;
                let presentation = BrowserPresentation {
                    mounted: true,
                    visible: true,
                    width: geometry.width,
                    height: geometry.height,
                    generation: self.presentation_generation,
                    scale_percent: geometry.scale_percent,
                };
                let document = session.document.clone();
                self.log.emit(
                    "present",
                    json!({
                        "width": geometry.width,
                        "height": geometry.height,
                        "scale_percent": geometry.scale_percent,
                        "generation": self.presentation_generation,
                    }),
                );
                self.presented = Some(geometry);
                self.send(
                    Command::Present {
                        document,
                        presentation,
                    },
                    cx,
                );
            }
            Source::Pattern { producer: None } => (),
        }
    }

    fn input(&mut self, input: InputEvent, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let document = session.document.clone();
        self.send(Command::Input { document, input }, cx);
    }

    fn page_ready(&self) -> bool {
        self.loaded && self.consumer.current().is_some()
    }

    fn fixture_counter(&self, name: &str) -> u64 {
        self.fixture
            .as_ref()
            .and_then(|state| state.get(name))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }

    fn fixture_scale(&self) -> f64 {
        self.fixture
            .as_ref()
            .and_then(|state| state.get("scale"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    }

    fn fixture_width(&self) -> u64 {
        self.fixture
            .as_ref()
            .and_then(|state| state.get("width"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.phase == Phase::Finished {
            return;
        }
        if self.started.elapsed() > RUN_TIMEOUT {
            self.fail("prototype run exceeded its time budget".to_string(), cx);
            return;
        }
        let produced = match &mut self.source {
            Source::Pattern {
                producer: Some(producer),
            } => producer.produce(now_ns()),
            _ => None,
        };
        if let Some(message) = produced {
            let Source::Pattern {
                producer: Some(producer),
            } = &mut self.source
            else {
                return;
            };
            let outcome = self.consumer.intake(message, Vec::new(), producer);
            self.on_intake(outcome, now_ns(), cx);
        }
        if let Some(until) = self.hold_until {
            if Instant::now() >= until && !self.options.m1 {
                self.finish(cx);
            }
            return;
        }
        if !self.page_ready() {
            if self.step_started.is_none()
                && self.started.elapsed() > STEP_TIMEOUT * 2
                && self.phase != Phase::Lost
            {
                self.fail(
                    "no frame was presented within the startup budget".to_string(),
                    cx,
                );
            }
            if self.steps.front() != Some(&Step::HostLoss) || self.step_started.is_none() {
                return;
            }
        }
        let Some(step) = self.steps.front().copied() else {
            self.hold_until = Some(Instant::now() + Duration::from_secs(self.options.hold));
            self.log
                .emit("hold", json!({ "seconds": self.options.hold }));
            return;
        };
        if self.step_started.is_none() {
            self.step_started = Some(Instant::now());
            self.step_stage = 0;
            self.log
                .emit("step_started", json!({ "step": step.name() }));
        }
        let elapsed = self.step_started.map(|at| at.elapsed()).unwrap_or_default();
        if elapsed > STEP_TIMEOUT {
            self.fail(format!("step {} timed out", step.name()), cx);
            return;
        }
        let done = match step {
            Step::Input => self.step_input(window, cx),
            Step::Resize => self.step_resize(cx),
            Step::Scale => self.step_scale(cx),
            Step::HostLoss => self.step_host_loss(cx),
        };
        if done {
            self.log.emit(
                "step_completed",
                json!({ "step": step.name(), "elapsed_ms": elapsed.as_millis() as u64 }),
            );
            self.steps.pop_front();
            self.step_started = None;
            self.step_stage = 0;
        }
    }

    fn step_input(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if matches!(self.source, Source::Pattern { .. }) {
            self.log.emit(
                "step_skipped",
                json!({ "step": "input", "reason": "pattern source has no page" }),
            );
            return true;
        }
        if self.step_stage == 0 {
            self.step_marker = self.fixture_counter("clicks") + self.fixture_counter("keys");
            let geometry = self.geometry(window);
            let (x, y) = ((geometry.width / 2) as i32, (geometry.height / 2) as i32);
            self.input(InputEvent::Focus { focused: true }, cx);
            self.input(InputEvent::MouseMove { x, y, modifiers: 0 }, cx);
            self.input(
                InputEvent::MouseButton {
                    x,
                    y,
                    button: MouseButton::Left,
                    down: true,
                    clicks: 1,
                    modifiers: MODIFIER_LEFT_MOUSE,
                },
                cx,
            );
            self.input(
                InputEvent::MouseButton {
                    x,
                    y,
                    button: MouseButton::Left,
                    down: false,
                    clicks: 1,
                    modifiers: 0,
                },
                cx,
            );
            self.input(
                InputEvent::Key {
                    kind: KeyKind::RawDown,
                    key_code: 65,
                    native_key_code: 38,
                    character: 0,
                    unmodified_character: 0,
                    modifiers: 0,
                },
                cx,
            );
            self.input(
                InputEvent::Key {
                    kind: KeyKind::Char,
                    key_code: 65,
                    native_key_code: 38,
                    character: u16::from(b'a'),
                    unmodified_character: u16::from(b'a'),
                    modifiers: 0,
                },
                cx,
            );
            self.input(
                InputEvent::Key {
                    kind: KeyKind::Up,
                    key_code: 65,
                    native_key_code: 38,
                    character: 0,
                    unmodified_character: 0,
                    modifiers: 0,
                },
                cx,
            );
            self.log.emit(
                "input_sent",
                json!({ "x": x, "y": y, "keys": ["a"], "clicks": 1 }),
            );
            self.step_stage = 1;
            return false;
        }
        let clicks = self.fixture_counter("clicks");
        let keys = self.fixture_counter("keys");
        if clicks >= 1 && keys >= 1 {
            self.log.emit(
                "input_observed",
                json!({ "clicks": clicks, "keys": keys, "fixture": self.fixture }),
            );
            return true;
        }
        false
    }

    fn step_resize(&mut self, cx: &mut Context<Self>) -> bool {
        if self.step_stage == 0 {
            self.step_marker = self.generations_seen;
            self.inset = true;
            cx.notify();
            self.step_stage = 1;
            return false;
        }
        let resized =
            self.generations_seen > self.step_marker && self.presented_generation_matches();
        let settled = match &self.source {
            Source::Pattern { .. } => true,
            Source::Cef { .. } => self
                .presented
                .as_ref()
                .is_some_and(|geometry| self.fixture_width() == u64::from(geometry.width)),
        };
        if resized && settled {
            self.log.emit("resize_observed", json!({ "pool_generations": self.generations_seen, "fixture": self.fixture, "presented": self.presented.as_ref().map(|g| json!({ "width": g.width, "height": g.height })) }));
            return true;
        }
        false
    }

    fn presented_generation_matches(&self) -> bool {
        let Some(geometry) = &self.presented else {
            return false;
        };
        match self
            .consumer
            .active_generation()
            .and_then(|generation| self.consumer.pool_layout(generation))
        {
            Some(layout) => {
                let expected_width = geometry.width * geometry.scale_percent / 100;
                layout.width.abs_diff(expected_width) <= 2
                    && self.consumer.current().is_some_and(|(_, identity, _)| {
                        identity.pool_generation == layout.generation
                    })
            }
            None => false,
        }
    }

    fn step_scale(&mut self, cx: &mut Context<Self>) -> bool {
        match self.step_stage {
            0 => {
                self.step_marker = self.generations_seen;
                self.scale_override = Some(200);
                cx.notify();
                self.step_stage = 1;
                false
            }
            1 => {
                let scaled =
                    self.generations_seen > self.step_marker && self.presented_generation_matches();
                let page = matches!(self.source, Source::Pattern { .. })
                    || (self.fixture_scale() - 2.0).abs() < 0.01;
                if scaled && page {
                    self.log.emit(
                        "scale_observed",
                        json!({ "scale_percent": 200, "fixture": self.fixture }),
                    );
                    self.step_marker = self.generations_seen;
                    self.scale_override = None;
                    cx.notify();
                    self.step_stage = 2;
                }
                false
            }
            _ => {
                let restored =
                    self.generations_seen > self.step_marker && self.presented_generation_matches();
                let page = matches!(self.source, Source::Pattern { .. })
                    || (self.fixture_scale() - 1.0).abs() < 0.01;
                if restored && page {
                    self.log
                        .emit("scale_restored", json!({ "fixture": self.fixture }));
                }
                restored && page
            }
        }
    }

    fn step_host_loss(&mut self, cx: &mut Context<Self>) -> bool {
        match self.step_stage {
            0 => {
                let Some(supervisor) = &self.supervisor else {
                    self.fail("host-loss step needs the cef source".to_string(), cx);
                    return false;
                };
                let pid = supervisor.pid();
                let killed = supervisor.terminate();
                self.log
                    .emit("host_killed", json!({ "pid": pid, "signalled": killed }));
                self.step_stage = 1;
                false
            }
            1 => {
                if self.phase != Phase::Lost {
                    return false;
                }
                let state = self.supervisor.as_ref().map(HostSupervisor::state);
                let failed = matches!(state, Some(HostState::Failed(_)));
                let cleared = self.consumer.current().is_none() && self.consumer.pool_count() == 0;
                self.log.emit("host_loss_observed", json!({ "state": format!("{state:?}"), "surface_cleared": cleared, "auto_restart": false }));
                if !failed || !cleared {
                    self.fail(
                        "host loss did not leave the browser unavailable with a cleared surface"
                            .to_string(),
                        cx,
                    );
                    return false;
                }
                let (Some(supervisor), Some(config)) =
                    (self.supervisor.clone(), self.config.clone())
                else {
                    return false;
                };
                self.phase = Phase::Starting;
                self.intake_frames = 0;
                let state = supervisor.retry(config);
                self.log
                    .emit("host_retry", json!({ "state": format!("{state:?}") }));
                self.step_stage = 2;
                false
            }
            _ => self.page_ready() && self.host_pids.len() >= 2,
        }
    }
}

impl Render for PrototypeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        browser_qualification::browser_prepare(window, cx);
        self.initialize(window, cx);
        if self.phase != Phase::Finished {
            self.present_if_needed(window, cx);
        }
        self.schedule_releases(window, cx);
        let inset = self.inset;
        let surface = self
            .consumer
            .current()
            .map(|(surface, _, _)| surface.clone());
        let mut root = div()
            .id("browser-prototype")
            .size_full()
            .bg(gpui::rgb(0x1a1a1a))
            .track_focus(&self.focus)
            .on_mouse_move(cx.listener(move |view, event: &MouseMoveEvent, _, cx| {
                let (x, y) = position(event.position, inset);
                let held = event.pressed_button.map(button_modifier).unwrap_or(0);
                view.input(
                    InputEvent::MouseMove {
                        x,
                        y,
                        modifiers: modifiers(&event.modifiers) | held,
                    },
                    cx,
                );
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    window.focus(&view.focus, cx);
                    view.mouse_button(
                        event.button,
                        event.position,
                        true,
                        event.click_count,
                        &event.modifiers,
                        cx,
                    );
                }),
            )
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |view, event: &MouseDownEvent, _, cx| {
                    view.mouse_button(
                        event.button,
                        event.position,
                        true,
                        event.click_count,
                        &event.modifiers,
                        cx,
                    );
                }),
            )
            .on_mouse_down(
                gpui::MouseButton::Middle,
                cx.listener(move |view, event: &MouseDownEvent, _, cx| {
                    view.mouse_button(
                        event.button,
                        event.position,
                        true,
                        event.click_count,
                        &event.modifiers,
                        cx,
                    );
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(move |view, event: &MouseUpEvent, _, cx| {
                    view.mouse_button(
                        event.button,
                        event.position,
                        false,
                        event.click_count,
                        &event.modifiers,
                        cx,
                    );
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Right,
                cx.listener(move |view, event: &MouseUpEvent, _, cx| {
                    view.mouse_button(
                        event.button,
                        event.position,
                        false,
                        event.click_count,
                        &event.modifiers,
                        cx,
                    );
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Middle,
                cx.listener(move |view, event: &MouseUpEvent, _, cx| {
                    view.mouse_button(
                        event.button,
                        event.position,
                        false,
                        event.click_count,
                        &event.modifiers,
                        cx,
                    );
                }),
            )
            .on_scroll_wheel(cx.listener(move |view, event: &ScrollWheelEvent, _, cx| {
                let (x, y) = position(event.position, inset);
                let (delta_x, delta_y) = match event.delta {
                    ScrollDelta::Pixels(delta) => (
                        f32::from(delta.x).round() as i32,
                        f32::from(delta.y).round() as i32,
                    ),
                    ScrollDelta::Lines(delta) => (
                        (delta.x * 40.0).round() as i32,
                        (delta.y * 40.0).round() as i32,
                    ),
                };
                view.input(
                    InputEvent::MouseWheel {
                        x,
                        y,
                        delta_x,
                        delta_y,
                        modifiers: modifiers(&event.modifiers),
                    },
                    cx,
                );
            }))
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                let keystroke = &event.keystroke;
                let code = key_code(&keystroke.key);
                let flags = modifiers(&keystroke.modifiers);
                view.input(
                    InputEvent::Key {
                        kind: KeyKind::RawDown,
                        key_code: code,
                        native_key_code: 0,
                        character: 0,
                        unmodified_character: 0,
                        modifiers: flags,
                    },
                    cx,
                );
                if let Some(character) = keystroke
                    .key_char
                    .as_deref()
                    .and_then(|text| text.chars().next())
                {
                    let mut units = [0_u16; 2];
                    let encoded = character.encode_utf16(&mut units);
                    if encoded.len() == 1 {
                        view.input(
                            InputEvent::Key {
                                kind: KeyKind::Char,
                                key_code: code,
                                native_key_code: 0,
                                character: encoded[0],
                                unmodified_character: encoded[0],
                                modifiers: flags,
                            },
                            cx,
                        );
                    }
                } else if keystroke.key == "enter"
                    || keystroke.key == "tab"
                    || keystroke.key == "space"
                {
                    let character = match keystroke.key.as_str() {
                        "enter" => 13,
                        "tab" => 9,
                        _ => 32,
                    };
                    view.input(
                        InputEvent::Key {
                            kind: KeyKind::Char,
                            key_code: code,
                            native_key_code: 0,
                            character,
                            unmodified_character: character,
                            modifiers: flags,
                        },
                        cx,
                    );
                }
            }))
            .on_key_up(cx.listener(|view, event: &KeyUpEvent, _, cx| {
                let keystroke = &event.keystroke;
                view.input(
                    InputEvent::Key {
                        kind: KeyKind::Up,
                        key_code: key_code(&keystroke.key),
                        native_key_code: 0,
                        character: 0,
                        unmodified_character: 0,
                        modifiers: modifiers(&keystroke.modifiers),
                    },
                    cx,
                );
            }));
        if inset {
            root = root.p(px(RESIZE_INSET));
        }
        if let Some(surface) = surface {
            let frame = self.current_frame.clone();
            root = root.child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        window.paint_external_surface(bounds, surface);
                        if let Some(frame) = frame {
                            browser_qualification::browser_painted(frame, window, cx);
                        }
                    },
                )
                .size_full(),
            );
        }
        root
    }
}

impl PrototypeView {
    fn mouse_button(
        &mut self,
        button: gpui::MouseButton,
        point: gpui::Point<Pixels>,
        down: bool,
        click_count: usize,
        held: &Modifiers,
        cx: &mut Context<Self>,
    ) {
        let Some(button) = mouse_button(button) else {
            return;
        };
        let (x, y) = position(point, self.inset);
        let clicks = click_count.clamp(1, 3) as u8;
        self.input(
            InputEvent::MouseButton {
                x,
                y,
                button,
                down,
                clicks,
                modifiers: modifiers(held),
            },
            cx,
        );
    }
}

pub fn run(args: &[String]) -> i32 {
    let mut argv: Vec<String> = vec!["paneflow browser-prototype".to_string()];
    argv.extend(args.iter().skip(2).cloned());
    let options = match Options::try_parse_from(argv) {
        Ok(options) => Arc::new(options),
        Err(error) => {
            let _ = error.print();
            return 2;
        }
    };
    if options.source != "cef" && options.source != "pattern" {
        eprintln!("--source must be cef or pattern");
        return 2;
    }
    if options.source == "cef" && options.url.is_none() {
        eprintln!("--url is required for the cef source");
        return 2;
    }
    let log = match Logger::new(options.log.as_ref()) {
        Ok(log) => log,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    log.emit(
        "started",
        json!({
            "schema_version": 1,
            "pid": std::process::id(),
            "source": options.source,
            "url": options.url,
            "ozone": if options.x11 { "x11" } else { "wayland" },
            "scenario": options.scenario,
            "clock": "CLOCK_MONOTONIC",
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        }),
    );
    let exit = Arc::new(AtomicI32::new(1));
    let exit_for_app = exit.clone();
    gpui_platform::application().run(move |cx: &mut App| {
        BrowserRuntime::install(cx);
        match open_window(options, log, exit_for_app, None, cx) {
            Ok(()) => cx.activate(true),
            Err(error) => {
                eprintln!("browser prototype window failed: {error}");
                cx.quit();
            }
        }
    });
    exit.load(Ordering::SeqCst)
}

pub(crate) fn configure_m1_window(
    options: &mut WindowOptions,
    role: &str,
    cx: &App,
) -> Result<(), String> {
    if !browser_qualification::enabled() {
        return Ok(());
    }
    let fullscreen = match std::env::var("PANEFLOW_M1_FULLSCREEN") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Err(std::env::VarError::NotPresent) => false,
        _ => return Err("PANEFLOW_M1_FULLSCREEN must be 0 or 1".into()),
    };
    let variable = match role {
        "terminal" => "PANEFLOW_M1_TERMINAL_OUTPUT",
        "browser" => "PANEFLOW_M1_BROWSER_OUTPUT",
        _ => return Err("unknown M1 window role".into()),
    };
    let name = match std::env::var(variable) {
        Ok(name) if !name.is_empty() && name.len() <= 256 => name,
        Err(std::env::VarError::NotPresent) if !fullscreen => return Ok(()),
        _ => return Err(format!("{variable} requires an exact Wayland output name")),
    };
    let uuid = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, name.as_bytes());
    let mut matches = cx
        .displays()
        .into_iter()
        .filter(|display| display.uuid().ok() == Some(uuid));
    let display = matches
        .next()
        .ok_or_else(|| format!("M1 output {name} is unavailable in GPUI"))?;
    if matches.next().is_some() {
        return Err(format!("M1 output {name} is ambiguous in GPUI"));
    }
    options.display_id = Some(display.id());
    if fullscreen {
        options.window_bounds = Some(WindowBounds::Fullscreen(display.bounds()));
    }
    Ok(())
}

fn m1_outputs_ready(cx: &App) -> Result<(), String> {
    configure_m1_window(&mut WindowOptions::default(), "terminal", cx)?;
    if std::env::var_os("PANEFLOW_M1_BROWSER_URL").is_some() {
        configure_m1_window(&mut WindowOptions::default(), "browser", cx)?;
    }
    Ok(())
}

pub(crate) fn launch_m1_when_outputs_ready(launch: impl FnOnce(&mut App) + 'static, cx: &mut App) {
    if m1_outputs_ready(cx).is_ok() {
        launch(cx);
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(TICK).await;
            match cx.update(|cx| m1_outputs_ready(cx)) {
                Ok(()) => {
                    cx.update(launch);
                    return;
                }
                Err(error) if Instant::now() >= deadline => {
                    log::error!("M1 output bootstrap timed out after 5 seconds: {error}");
                    eprintln!("M1 output bootstrap timed out after 5 seconds: {error}");
                    cx.update(|cx| cx.quit());
                    return;
                }
                Err(_) => {}
            }
        }
    })
    .detach();
}

pub(crate) fn close_m1_window(cx: &mut App) -> Result<bool, String> {
    for handle in cx.windows() {
        let Some(handle) = handle.downcast::<PrototypeView>() else {
            continue;
        };
        let requested = handle
            .update(cx, |view, _, cx| {
                if !view.options.m1 {
                    return false;
                }
                view.log.emit("m1_stop_requested", json!({}));
                view.finish(cx);
                true
            })
            .map_err(|error| error.to_string())?;
        if requested {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn open_m1_window(cx: &mut App) -> Result<(), String> {
    let Some(url) = std::env::var_os("PANEFLOW_M1_BROWSER_URL") else {
        return Ok(());
    };
    if !browser_qualification::enabled() {
        return Err("PANEFLOW_M1_BROWSER_URL requires PANEFLOW_M1_LOG".into());
    }
    let url = url.into_string().map_err(|_| "invalid M1 Browser URL")?;
    origin_of(&url)?;
    let log_path = std::env::var_os("PANEFLOW_M1_BROWSER_LOG")
        .map(PathBuf::from)
        .ok_or("PANEFLOW_M1_BROWSER_LOG is required for the combined M1 capture")?;
    let log = Logger::new(Some(&log_path))?;
    let mut options = Options::try_parse_from([
        "paneflow browser-prototype",
        "--url",
        &url,
        "--width",
        "1920",
        "--height",
        "1080",
        "--hold",
        "180",
    ])
    .map_err(|error| error.to_string())?;
    options.m1 = true;
    let coordinate = |name: &str| -> Result<f32, String> {
        match std::env::var(name) {
            Ok(value) => value
                .parse::<i32>()
                .map(|value| value as f32)
                .map_err(|_| format!("{name} must be an integer physical pixel coordinate")),
            Err(std::env::VarError::NotPresent) => Ok(0.0),
            Err(error) => Err(error.to_string()),
        }
    };
    let origin = gpui::point(
        px(coordinate("PANEFLOW_M1_BROWSER_X_PX")?),
        px(coordinate("PANEFLOW_M1_BROWSER_Y_PX")?),
    );
    log.emit(
        "started",
        json!({"schema_version":1,"pid":std::process::id(),
        "source":"cef","url":url,"ozone":"wayland","clock":"CLOCK_MONOTONIC",
        "configuration":"C","terminal_layout":"unchanged normal application window",
        "browser_physical_width":1920,"browser_physical_height":1080,
        "profile":if cfg!(debug_assertions) {"debug"} else {"release"}}),
    );
    open_window(
        Arc::new(options),
        log,
        Arc::new(AtomicI32::new(1)),
        Some(origin),
        cx,
    )
}

fn open_window(
    options: Arc<Options>,
    log: Logger,
    exit: Arc<AtomicI32>,
    physical_origin: Option<gpui::Point<Pixels>>,
    cx: &mut App,
) -> Result<(), String> {
    let (ack_sender, ack_receiver) = smol::channel::bounded::<GpuCompletion>(8);
    let scale = if options.m1 {
        cx.windows()
            .first()
            .copied()
            .and_then(|handle| handle.update(cx, |_, window, _| window.scale_factor()).ok())
            .filter(|scale| scale.is_finite() && *scale > 0.0)
            .ok_or("M1 Browser needs an existing terminal window scale")?
    } else {
        1.0
    };
    let dimensions = size(
        px(options.width as f32 / scale),
        px(options.height as f32 / scale),
    );
    let bounds = match physical_origin {
        Some(origin) => Bounds::new(origin / scale, dimensions),
        None => Bounds::centered(None, dimensions, cx),
    };
    let m1 = options.m1;
    let mut window_options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        app_id: Some(
            if m1 {
                "paneflow-m1-browser"
            } else {
                "paneflow-browser-prototype"
            }
            .into(),
        ),
        focus: !m1,
        inactive_frame_interval: None,
        ..Default::default()
    };
    configure_m1_window(&mut window_options, "browser", cx)?;
    let fullscreen_requested = matches!(
        window_options.window_bounds,
        Some(WindowBounds::Fullscreen(_))
    );
    cx.open_window(window_options, move |window, cx| {
        if m1 && !fullscreen_requested {
            let scale = window.scale_factor();
            window.resize(size(px(1920.0 / scale), px(1080.0 / scale)));
        }
        let view: Entity<PrototypeView> =
            cx.new(|cx| PrototypeView::new(options, log, exit, ack_sender, cx));
        if !m1 {
            let focus = view.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
        let ticker = view.downgrade();
        window
            .spawn(cx, async move |cx| {
                loop {
                    cx.background_executor().timer(TICK).await;
                    if ticker
                        .update_in(cx, |view, window, cx| view.tick(window, cx))
                        .is_err()
                    {
                        return;
                    }
                }
            })
            .detach();
        let acker = view.downgrade();
        cx.spawn(async move |cx| {
            while let Ok(acks) = ack_receiver.recv().await {
                if acker
                    .update(cx, |view, cx| view.on_gpu_completion(acks, cx))
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
        view
    })
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn await_trace_export(
    receiver: smol::channel::Receiver<Result<(), String>>,
    timeout: Duration,
) -> Result<(), String> {
    smol::future::or(
        async {
            receiver
                .recv()
                .await
                .unwrap_or_else(|error| Err(format!("trace completion channel closed: {error}")))
        },
        async {
            smol::Timer::after(timeout).await;
            Err("CEF trace export timed out before host shutdown".into())
        },
    )
    .await
}

pub(super) fn wait_for_gpu_completion(
    mut poll: impl FnMut() -> Result<(), String>,
    completed: std::sync::mpsc::Receiver<()>,
    timeout: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        match completed.try_recv() {
            Ok(()) => return Ok(()),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err(
                    "GPU completion callback disconnected; buffers remain unacknowledged".into(),
                );
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => (),
        }
        let poll_result = poll();
        let remaining = timeout.saturating_sub(started.elapsed());
        match completed.recv_timeout(remaining.min(Duration::from_millis(1))) {
            Ok(()) => return Ok(()),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                if poll_result.is_ok() && started.elapsed() < timeout => {}
            Err(error) => {
                return Err(format!(
                    "GPU completion was not observed; buffers remain unacknowledged: {error}; poll: {poll_result:?}"
                ));
            }
        }
    }
}

#[cfg(test)]
mod release_tests {
    use super::*;

    #[test]
    fn trace_export_requires_success_before_graceful_shutdown() {
        let (sender, receiver) = smol::channel::bounded(1);
        assert!(sender.try_send(Ok(())).is_ok());
        assert!(smol::block_on(await_trace_export(receiver, Duration::from_secs(1))).is_ok());
    }

    #[test]
    fn trace_export_failure_and_disconnect_are_not_completion() {
        let (sender, receiver) = smol::channel::bounded(1);
        assert!(sender.try_send(Err("export failed".into())).is_ok());
        assert!(smol::block_on(await_trace_export(receiver, Duration::ZERO)).is_err());
        let (sender, receiver) = smol::channel::bounded(1);
        drop(sender);
        assert!(smol::block_on(await_trace_export(receiver, Duration::ZERO)).is_err());
    }

    #[test]
    fn a_missing_trace_callback_has_a_bounded_deadline() {
        let (_sender, receiver) = smol::channel::bounded(1);
        assert!(smol::block_on(await_trace_export(receiver, Duration::ZERO)).is_err());
    }

    #[test]
    fn a_successful_poll_without_completion_cannot_release_buffers() {
        let (_sender, receiver) = std::sync::mpsc::channel();
        assert!(wait_for_gpu_completion(|| Ok(()), receiver, Duration::ZERO).is_err());
    }

    #[test]
    fn a_disconnected_callback_cannot_release_buffers() {
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(sender);
        assert!(wait_for_gpu_completion(|| Ok(()), receiver, Duration::ZERO).is_err());
    }

    #[test]
    fn an_observed_callback_does_not_poll_later_submissions() {
        let (sender, receiver) = std::sync::mpsc::channel();
        assert!(sender.send(()).is_ok());
        assert!(
            wait_for_gpu_completion(
                || panic!("completed buffers must not wait for later submissions"),
                receiver,
                Duration::ZERO,
            )
            .is_ok()
        );
    }

    #[test]
    fn nonblocking_polls_continue_until_the_callback_is_observed() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut polls = 0;
        let result = wait_for_gpu_completion(
            || {
                polls += 1;
                if polls == 2 {
                    assert!(sender.send(()).is_ok());
                }
                Ok(())
            },
            receiver,
            Duration::from_millis(100),
        );
        assert!(result.is_ok());
        assert_eq!(polls, 2);
    }

    #[test]
    fn a_callback_from_a_failed_poll_still_proves_completion() {
        let (sender, receiver) = std::sync::mpsc::channel();
        assert!(
            wait_for_gpu_completion(
                || {
                    assert!(sender.send(()).is_ok());
                    Err("poll failed after invoking completion".into())
                },
                receiver,
                Duration::ZERO,
            )
            .is_ok()
        );
    }
}
