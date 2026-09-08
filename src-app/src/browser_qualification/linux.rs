use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    fs::OpenOptions,
    io::{BufWriter, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
};

use gpui::{App, Window};
use paneflow_browser_protocol::Document;

use crate::browser::presentation::{FrameIdentity, FrameTiming};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use serde::Serialize;
use serde_json::{Value, json};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_output, wl_registry, wl_surface},
};
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};

const MAX_PENDING_INPUTS: usize = 32;
const MAX_TERMINALS: usize = 8;
const MAX_FEEDBACKS: u64 = 16;

static LOGGER: OnceLock<Option<Arc<Logger>>> = OnceLock::new();

thread_local! {
    static TRACKER: RefCell<Tracker> = RefCell::new(Tracker::default());
    static BROWSER_TRACKER: RefCell<BrowserTracker> = RefCell::new(BrowserTracker::default());
}

pub(crate) fn enabled() -> bool {
    logger().is_some()
}

fn input_only() -> bool {
    std::env::var("PANEFLOW_M1_INPUT_ONLY").is_ok_and(|value| value == "1")
}

pub(crate) fn now_ns() -> u64 {
    clock_ns(libc::CLOCK_MONOTONIC).unwrap_or(0)
}

fn clock_ns(clock: libc::clockid_t) -> Option<u64> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(clock, &mut value) } != 0 {
        return None;
    }
    u64::try_from(value.tv_sec)
        .ok()?
        .checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(value.tv_nsec).ok()?)
}

struct M1RendererTimingObserver {
    logger: Arc<Logger>,
    role: &'static str,
}

impl gpui::RendererTimingObserver for M1RendererTimingObserver {
    fn now_ns(&self) -> Option<u64> {
        clock_ns(libc::CLOCK_MONOTONIC)
    }

    fn record(&self, timing: gpui::RendererTiming) {
        self.logger.emit(json!({
            "event":"renderer_stage", "role":self.role, "pid":std::process::id(),
            "clock":"CLOCK_MONOTONIC", "window_id":timing.identity.window_id.to_string(),
            "wl_surface_id":timing.identity.wl_surface_id,
            "wl_display_address":timing.identity.wl_display_address.map(|address| format!("0x{address:x}")),
            "surface_epoch":timing.surface_epoch.to_string(), "attempt":timing.attempt.to_string(),
            "stage":timing.stage, "start_ns":timing.start_ns.map(|value| value.to_string()),
            "end_ns":timing.end_ns.map(|value| value.to_string()), "outcome":timing.outcome,
            "present_mode":timing.present_mode, "requested_present_mode":timing.requested_present_mode,
            "supported_present_modes_mask":timing.supported_present_modes_mask,
            "desired_maximum_frame_latency":timing.desired_maximum_frame_latency,
            "width":timing.width, "height":timing.height,
        }));
    }
}

struct Logger {
    sender: SyncSender<Value>,
    dropped: Arc<AtomicU64>,
}

impl Logger {
    fn emit(&self, value: Value) {
        if self.sender.try_send(value).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn fatal(&self, reason: impl std::fmt::Display) {
        self.emit(json!({"event":"fatal","at_ns":now_ns(),"reason":reason.to_string()}));
    }
}

fn logger() -> Option<&'static Arc<Logger>> {
    LOGGER.get_or_init(|| {
        let path = PathBuf::from(std::env::var_os("PANEFLOW_M1_LOG")?);
        if !path.is_absolute() {
            eprintln!("PANEFLOW_M1_LOG must be an absolute path");
            return None;
        }
        let (sender, receiver) = sync_channel::<Value>(8192);
        let dropped = Arc::new(AtomicU64::new(0));
        let logger = Arc::new(Logger { sender, dropped: dropped.clone() });
        let result = std::thread::Builder::new().name("m1-evidence".into()).spawn(move || {
            let file = match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
                Ok(file) => file,
                Err(error) => {
                    eprintln!("M1 evidence cannot open {}: {error}", path.display());
                    return;
                }
            };
            let mut writer = BufWriter::new(file);
            while let Ok(value) = receiver.recv() {
                let lost = dropped.swap(0, Ordering::Relaxed);
                if lost != 0 {
                    let failure = json!({"event":"fatal","reason":"evidence queue overflow","dropped":lost});
                    if serde_json::to_writer(&mut writer, &failure).is_err() || writeln!(writer).is_err() {
                        break;
                    }
                }
                if serde_json::to_writer(&mut writer, &value).is_err()
                    || writeln!(writer).is_err() || writer.flush().is_err() {
                    eprintln!("M1 evidence write failed");
                    break;
                }
            }
        });
        if let Err(error) = result {
            eprintln!("M1 evidence thread failed: {error}");
            return None;
        }
        logger.emit(json!({"event":"started","schema_version":1,"at_ns":now_ns(),
            "clock":"CLOCK_MONOTONIC","profile":if cfg!(debug_assertions) {"debug"} else {"release"},
            "input_boundary":"GPUI TerminalView input handler entry",
            "presentation_boundary":"wp_presentation_feedback.presented for painted echo marker"}));
        Some(logger)
    }).as_ref()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowEvidence {
    role: &'static str,
    fullscreen: bool,
    wl_surface_id: Option<u32>,
    display_id: Option<u64>,
    display_uuid: Option<uuid::Uuid>,
}

impl WindowEvidence {
    fn read(window: &Window, role: &'static str, cx: &App) -> Self {
        let wl_surface_id = HasWindowHandle::window_handle(window)
            .ok()
            .and_then(|handle| {
                let RawWindowHandle::Wayland(handle) = handle.as_raw() else {
                    return None;
                };
                unsafe {
                    wayland_client::backend::ObjectId::from_ptr(
                        wl_surface::WlSurface::interface(),
                        handle.surface.as_ptr().cast(),
                    )
                }
                .ok()
                .map(|id| id.protocol_id())
            });
        let display = window.display(cx);
        Self {
            role,
            fullscreen: window.is_fullscreen(),
            wl_surface_id,
            display_id: display.as_ref().map(|display| u64::from(display.id())),
            display_uuid: display.and_then(|display| display.uuid().ok()),
        }
    }

    fn attach(self, fields: &mut Value) {
        fields["role"] = json!(self.role);
        fields["fullscreen"] = json!(self.fullscreen);
        fields["wl_surface_id"] = json!(self.wl_surface_id);
        fields["display_id"] = json!(self.display_id);
        fields["display_uuid"] = json!(self.display_uuid);
    }
}

#[derive(Clone)]
pub(crate) struct BrowserFrame {
    pub document: Document,
    pub identity: FrameIdentity,
    pub timing: FrameTiming,
    pub intake_ns: u64,
}

impl BrowserFrame {
    fn fields(&self) -> Value {
        json!({"document":self.document,"pool_generation":self.identity.pool_generation,
            "buffer":self.identity.buffer,"frame_sequence":self.identity.sequence,
            "callback_ns":self.timing.callback_ns,"ready_ns":self.timing.ready_ns,
            "capture_timestamp_us":self.timing.capture_timestamp_us,
            "capture_counter":self.timing.capture_counter,"intake_ns":self.intake_ns})
    }
}

#[derive(Default)]
struct BrowserTracker {
    native: Option<NativeObserver>,
    native_attempted: bool,
    last_frame: Option<(Document, FrameIdentity)>,
    feedback_sequence: u64,
    viewport: Option<(u32, u32, u32, WindowEvidence)>,
}

pub(crate) fn browser_intake(frame: &BrowserFrame, outstanding: usize, pools: usize) {
    if let Some(logger) = logger() {
        let mut event = frame.fields();
        event["event"] = json!("browser_intake");
        event["at_ns"] = json!(now_ns());
        event["outstanding"] = json!(outstanding);
        event["pools"] = json!(pools);
        logger.emit(event);
    }
}

impl BrowserTracker {
    fn prepare(&mut self, window: &Window, logger: &Arc<Logger>, cx: &App) {
        let scale = window.scale_factor();
        let size = window.viewport_size();
        let viewport = (
            (f32::from(size.width) * scale).round() as u32,
            (f32::from(size.height) * scale).round() as u32,
            scale.to_bits(),
            WindowEvidence::read(window, "browser", cx),
        );
        if self.viewport != Some(viewport) {
            self.viewport = Some(viewport);
            let mut event = json!({"event":"browser_viewport","width_px":viewport.0,
                "height_px":viewport.1,"scale":scale,"at_ns":now_ns()});
            viewport.3.attach(&mut event);
            logger.emit(event);
        }
        if !self.native_attempted {
            self.native_attempted = true;
            match NativeObserver::start(window, logger.clone(), "browser") {
                Ok(native) => self.native = Some(native),
                Err(error) if input_only() => logger.emit(json!({
                    "event":"presentation_unavailable","role":"browser","at_ns":now_ns(),
                    "reason":error,"scope":"input_isolation_only"
                })),
                Err(error) => logger.fatal(error),
            }
        }
    }
}

pub(crate) fn browser_prepare(window: &Window, cx: &App) {
    let Some(logger) = logger() else { return };
    BROWSER_TRACKER.with_borrow_mut(|tracker| tracker.prepare(window, logger, cx));
}

pub(crate) fn browser_painted(frame: BrowserFrame, window: &Window, cx: &App) {
    let Some(logger) = logger() else { return };
    BROWSER_TRACKER.with_borrow_mut(|tracker| {
        tracker.prepare(window, logger, cx);
        let key = (frame.document.clone(), frame.identity);
        if tracker.last_frame.as_ref() == Some(&key) {
            return;
        }
        let Some(native) = tracker.native.as_mut() else {
            return;
        };
        if !native.ready() {
            let mut event = frame.fields();
            event["event"] = json!("browser_paint_unobserved");
            event["at_ns"] = json!(now_ns());
            event["reason"] = json!("presentation observer is initializing");
            WindowEvidence::read(window, "browser", cx).attach(&mut event);
            logger.emit(event);
            return;
        }
        tracker.feedback_sequence += 1;
        native.request(
            Feedback::Browser {
                frame,
                id: tracker.feedback_sequence,
            },
            logger,
            window,
            cx,
        );
        tracker.last_frame = Some(key);
    });
}

#[derive(Default)]
struct Tracker {
    terminals: HashMap<u64, TerminalInput>,
    native: Option<NativeObserver>,
    native_attempted: bool,
    feedback_sequence: u64,
    viewport: Option<(u32, u32, u32, WindowEvidence)>,
}

#[derive(Default)]
struct TerminalInput {
    line: String,
    first_ns: u64,
    key_ns: Option<u64>,
    pending: VecDeque<Input>,
    geometry: Option<(usize, usize)>,
}

#[derive(Clone)]
struct Input {
    surface_id: u64,
    tick: u64,
    input_ns: u64,
    completed_ns: u64,
}

pub(crate) fn input_key(surface_id: u64, key: &str) {
    let Some(logger) = logger() else { return };
    let timestamp = now_ns();
    TRACKER.with_borrow_mut(|tracker| {
        if !tracker.terminals.contains_key(&surface_id) && tracker.terminals.len() >= MAX_TERMINALS
        {
            logger.fatal("too many instrumented terminals");
            return;
        }
        let terminal = tracker.terminals.entry(surface_id).or_default();
        terminal.key_ns = Some(timestamp);
        if key != "enter" {
            return;
        }
        let line = std::mem::take(&mut terminal.line);
        let Some(tick) = line
            .strip_prefix('i')
            .and_then(|value| value.parse::<u64>().ok())
        else {
            terminal.first_ns = 0;
            return;
        };
        let input = Input {
            surface_id,
            tick,
            input_ns: terminal.first_ns,
            completed_ns: timestamp,
        };
        logger.emit(json!({"event":"input","surface_id":surface_id,"tick":tick,
            "input_ns":input.input_ns,"completed_ns":timestamp}));
        terminal.first_ns = 0;
        if terminal.pending.len() >= MAX_PENDING_INPUTS {
            logger.fatal("unpresented input queue overflow");
            return;
        }
        terminal.pending.push_back(input);
    });
}

pub(crate) fn input_text(surface_id: u64, text: &str) {
    let Some(logger) = logger() else { return };
    let timestamp = now_ns();
    TRACKER.with_borrow_mut(|tracker| {
        if !tracker.terminals.contains_key(&surface_id) && tracker.terminals.len() >= MAX_TERMINALS
        {
            logger.fatal("too many instrumented terminals");
            return;
        }
        let terminal = tracker.terminals.entry(surface_id).or_default();
        if terminal.line.is_empty() {
            terminal.first_ns = terminal.key_ns.take().unwrap_or(timestamp);
        }
        if terminal.line.len() + text.len() > 32 {
            logger.fatal("qualification input exceeds marker limit");
            terminal.line.clear();
            terminal.first_ns = 0;
            return;
        }
        terminal.line.push_str(text);
    });
}

pub(crate) struct CpuStart {
    monotonic_ns: u64,
    thread_cpu_ns: u64,
}

pub(crate) fn cpu_started() -> CpuStart {
    CpuStart {
        monotonic_ns: now_ns(),
        thread_cpu_ns: clock_ns(libc::CLOCK_THREAD_CPUTIME_ID).unwrap_or(0),
    }
}

pub(crate) fn cpu_finished(surface_id: u64, phase: &str, start: CpuStart) {
    let thread_cpu_end_ns = clock_ns(libc::CLOCK_THREAD_CPUTIME_ID).unwrap_or(0);
    let end_ns = now_ns();
    if let Some(logger) = logger() {
        if start.thread_cpu_ns == 0 || thread_cpu_end_ns < start.thread_cpu_ns {
            logger.fatal("thread CPU clock could not be measured");
            return;
        }
        logger.emit(json!({"event":"cpu","surface_id":surface_id,"phase":phase,
            "start_ns":start.monotonic_ns,"end_ns":end_ns,
            "thread_cpu_start_ns":start.thread_cpu_ns,"thread_cpu_end_ns":thread_cpu_end_ns}));
    }
}

pub(crate) fn paint_failed(reason: impl std::fmt::Display) {
    if let Some(logger) = logger() {
        logger.fatal(format!("terminal glyph paint failed: {reason}"));
    }
}

pub(crate) fn painted<'a>(
    surface_id: u64,
    runs: impl Iterator<Item = (i32, usize, &'a str)>,
    columns: usize,
    rows: usize,
    window: &Window,
    cx: &App,
) {
    let Some(logger) = logger() else { return };
    TRACKER.with_borrow_mut(|tracker| {
        let scale = window.scale_factor();
        let size = window.viewport_size();
        let viewport = ((f32::from(size.width) * scale).round() as u32,
            (f32::from(size.height) * scale).round() as u32, scale.to_bits(),
            WindowEvidence::read(window, "terminal", cx));
        if tracker.viewport != Some(viewport) {
            tracker.viewport = Some(viewport);
            let mut event = json!({"event":"viewport","width_px":viewport.0,"height_px":viewport.1,"scale":scale,"at_ns":now_ns()});
            viewport.3.attach(&mut event);
            logger.emit(event);
        }
        if !tracker.terminals.contains_key(&surface_id) && tracker.terminals.len() >= MAX_TERMINALS {
            logger.fatal("too many instrumented terminals");
            return;
        }
        let terminal = tracker.terminals.entry(surface_id).or_default();
        if terminal.geometry != Some((columns, rows)) {
            terminal.geometry = Some((columns, rows));
            logger.emit(json!({"event":"geometry","surface_id":surface_id,"columns":columns,"rows":rows,"at_ns":now_ns()}));
        }
        if !tracker.native_attempted {
            tracker.native_attempted = true;
            match NativeObserver::start(window, logger.clone(), "terminal") {
                Ok(native) => tracker.native = Some(native),
                Err(error) if input_only() => logger.emit(json!({
                    "event":"presentation_unavailable","role":"terminal","at_ns":now_ns(),
                    "reason":error,"scope":"input_isolation_only"
                })),
                Err(error) => logger.fatal(error),
            }
        }
        let Some(native) = tracker.native.as_mut() else { return };
        if !native.ready() { return; }
        let Some(terminal) = tracker.terminals.get_mut(&surface_id) else { return };
        if terminal.pending.is_empty() { return; }
        let mut visible = vec![vec![' '; columns.min(512)]; rows.min(256)];
        for (line, column, text) in runs {
            let Ok(line) = usize::try_from(line) else { continue };
            if let Some(row) = visible.get_mut(line) {
                for (offset, character) in text.chars().enumerate() {
                    if let Some(cell) = row.get_mut(column + offset) { *cell = character; }
                }
            }
        }
        let markers: Vec<(u64, u64)> = visible.into_iter().filter_map(|row| {
            let row: String = row.into_iter().collect();
            let marker = row.split_once("pf-input:")?.1;
            let (terminal, tick) = marker.split_once(':')?;
            let tick = tick.split(|character: char| !character.is_ascii_digit()).next()?;
            Some((terminal.parse().ok()?, tick.parse().ok()?))
        }).collect();
        for (terminal_index, tick) in markers {
            let Some(index) = terminal.pending.iter().position(|input| input.tick == tick) else { continue };
            let Some(input) = terminal.pending.remove(index) else { continue };
            tracker.feedback_sequence += 1;
            native.request(Feedback::Terminal { input, terminal: terminal_index, id: tracker.feedback_sequence }, logger, window, cx);
        }
    });
}

#[derive(Clone)]
enum Feedback {
    Terminal {
        input: Input,
        terminal: u64,
        id: u64,
    },
    Browser {
        frame: BrowserFrame,
        id: u64,
    },
}

impl Feedback {
    fn id(&self) -> u64 {
        match self {
            Self::Terminal { id, .. } | Self::Browser { id, .. } => *id,
        }
    }

    fn fields(&self, event: &str) -> Value {
        match self {
            Self::Terminal {
                input,
                terminal,
                id,
            } => json!({"event":event,
                "surface_id":input.surface_id,"terminal":terminal,"tick":input.tick,
                "feedback_id":id,"input_ns":input.input_ns,"completed_ns":input.completed_ns}),
            Self::Browser { frame, id } => {
                let mut fields = frame.fields();
                fields["event"] = json!(format!("browser_{event}"));
                fields["feedback_id"] = json!(id);
                fields
            }
        }
    }
}

struct PaintedFeedback {
    payload: Feedback,
    window: WindowEvidence,
}

impl PaintedFeedback {
    fn fields(&self, event: &str) -> Value {
        let mut fields = self.payload.fields(event);
        self.window.attach(&mut fields);
        fields
    }
}

struct PresentationHandle {
    presentation: wp_presentation::WpPresentation,
    queue: QueueHandle<PresentationState>,
}

struct NativeObserver {
    role: &'static str,
    connection: Connection,
    surface: wl_surface::WlSurface,
    ready_receiver: Receiver<PresentationHandle>,
    handle: Option<PresentationHandle>,
    pending: Arc<AtomicU64>,
}

impl NativeObserver {
    fn start(window: &Window, logger: Arc<Logger>, role: &'static str) -> Result<Self, String> {
        window.set_renderer_timing_observer(Some(Arc::new(M1RendererTimingObserver {
            logger: logger.clone(),
            role,
        })));
        let window_handle =
            HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
        let display_handle =
            HasDisplayHandle::display_handle(window).map_err(|error| error.to_string())?;
        let (RawWindowHandle::Wayland(window_handle), RawDisplayHandle::Wayland(display_handle)) =
            (window_handle.as_raw(), display_handle.as_raw())
        else {
            return Err("M1 presentation requires a native Wayland window".into());
        };
        let backend = unsafe {
            wayland_client::backend::Backend::from_foreign_display(
                display_handle.display.as_ptr().cast(),
            )
        };
        let connection = Connection::from_backend(backend);
        let surface_id = unsafe {
            wayland_client::backend::ObjectId::from_ptr(
                wl_surface::WlSurface::interface(),
                window_handle.surface.as_ptr().cast(),
            )
        }
        .map_err(|error| error.to_string())?;
        let surface = wl_surface::WlSurface::from_id(&connection, surface_id)
            .map_err(|error| error.to_string())?;
        let wl_surface_id = surface.id().protocol_id();
        let worker_connection = connection.clone();
        let (ready_sender, ready_receiver) = sync_channel(1);
        let pending = Arc::new(AtomicU64::new(0));
        let worker_pending = pending.clone();
        std::thread::Builder::new()
            .name("m1-presentation".into())
            .spawn(move || {
                let result = (|| -> Result<(), String> {
                    let (globals, mut event_queue) =
                        registry_queue_init::<PresentationState>(&worker_connection)
                            .map_err(|error| error.to_string())?;
                    let queue = event_queue.handle();
                    let presentation = globals
                        .bind::<wp_presentation::WpPresentation, _, _>(&queue, 1..=1, ())
                        .map_err(|error| error.to_string())?;
                    let mut state = PresentationState {
                        logger: logger.clone(),
                        clock_id: None,
                        pending: worker_pending,
                        role,
                        wl_surface_id,
                        outputs: HashMap::new(),
                        feedback_outputs: HashMap::new(),
                    };
                    globals.contents().with_list(|list| {
                        for global in list.iter().filter(|global| global.interface == "wl_output") {
                            state.bind_output(
                                globals.registry(),
                                &queue,
                                global.name,
                                global.version,
                            );
                        }
                    });
                    event_queue
                        .roundtrip(&mut state)
                        .map_err(|error| error.to_string())?;
                    if state.clock_id.is_none() {
                        return Err("presentation clock id was not advertised".into());
                    }
                    ready_sender
                        .send(PresentationHandle {
                            presentation,
                            queue,
                        })
                        .map_err(|error| error.to_string())?;
                    loop {
                        event_queue
                            .blocking_dispatch(&mut state)
                            .map_err(|error| error.to_string())?;
                    }
                })();
                if let Err(error) = result {
                    logger.fatal(error);
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            role,
            connection,
            surface,
            ready_receiver,
            handle: None,
            pending,
        })
    }

    fn ready(&mut self) -> bool {
        if self.handle.is_none() {
            self.handle = self.ready_receiver.try_recv().ok();
        }
        self.handle.is_some()
    }

    fn request(&self, feedback: Feedback, logger: &Logger, window: &Window, cx: &App) {
        let Some(handle) = self.handle.as_ref() else {
            return;
        };
        if self.pending.fetch_add(1, Ordering::Relaxed) >= MAX_FEEDBACKS {
            self.pending.fetch_sub(1, Ordering::Relaxed);
            logger.fatal("presentation feedback queue overflow");
            return;
        }
        let feedback = PaintedFeedback {
            payload: feedback,
            window: WindowEvidence::read(window, self.role, cx),
        };
        let mut event = feedback.fields("paint");
        event["at_ns"] = json!(now_ns());
        logger.emit(event);
        handle
            .presentation
            .feedback(&self.surface, &handle.queue, feedback);
        if let Err(error) = self.connection.flush() {
            logger.fatal(error);
        }
    }
}

#[derive(Clone, Default, Serialize)]
struct OutputInfo {
    name: String,
    width_px: i32,
    height_px: i32,
    refresh_millihz: i32,
    scale: i32,
    done_ns: u64,
    wl_output_id: u32,
    registry_global_name: u32,
}

struct OutputState {
    proxy: wl_output::WlOutput,
    pending: OutputInfo,
    current: Option<OutputInfo>,
}

#[derive(Default)]
struct FeedbackOutputs {
    named: BTreeMap<String, OutputInfo>,
    ids: BTreeSet<u32>,
    unknown: BTreeSet<u32>,
}

struct PresentationState {
    logger: Arc<Logger>,
    clock_id: Option<libc::clockid_t>,
    pending: Arc<AtomicU64>,
    role: &'static str,
    wl_surface_id: u32,
    outputs: HashMap<u32, OutputState>,
    feedback_outputs: HashMap<u64, FeedbackOutputs>,
}

impl PresentationState {
    fn bind_output(
        &mut self,
        registry: &wl_registry::WlRegistry,
        queue: &QueueHandle<Self>,
        name: u32,
        version: u32,
    ) {
        if self
            .outputs
            .values()
            .any(|output| output.pending.registry_global_name == name)
        {
            return;
        }
        if version < 4 || self.outputs.len() >= 32 {
            self.logger
                .fatal("M1 output observation requires wl_output v4 and at most 32 outputs");
            return;
        }
        let proxy = registry.bind::<wl_output::WlOutput, _, _>(name, 4, queue, name);
        let id = proxy.id().protocol_id();
        self.outputs.insert(
            id,
            OutputState {
                proxy,
                pending: OutputInfo {
                    wl_output_id: id,
                    registry_global_name: name,
                    ..Default::default()
                },
                current: None,
            },
        );
    }

    fn attach_outputs(&mut self, feedback: u64, event: &mut Value) {
        let outputs = self.feedback_outputs.remove(&feedback).unwrap_or_default();
        event["outputs"] = json!(outputs.named.into_values().collect::<Vec<_>>());
        event["sync_output_ids"] = json!(outputs.ids);
        event["unknown_output_ids"] = json!(outputs.unknown);
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for PresentationState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => {
                state.bind_output(registry, queue, name, version);
            }
            wl_registry::Event::GlobalRemove { name } => {
                let id = state.outputs.iter().find_map(|(id, output)| {
                    (output.pending.registry_global_name == name).then_some(*id)
                });
                if let Some(output) = id.and_then(|id| state.outputs.remove(&id)) {
                    state.logger.emit(json!({"event":"output_removed","role":state.role,
                        "wl_surface_id":state.wl_surface_id,"output":output.current,"at_ns":now_ns()}));
                    output.proxy.release();
                }
            }
            _ => (),
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for PresentationState {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.get_mut(&output.id().protocol_id()) else {
            return;
        };
        match event {
            wl_output::Event::Name { name } => output.pending.name = name,
            wl_output::Event::Mode {
                flags: wayland_client::WEnum::Value(flags),
                width,
                height,
                refresh,
            } if flags.contains(wl_output::Mode::Current) => {
                output.pending.width_px = width;
                output.pending.height_px = height;
                output.pending.refresh_millihz = refresh;
            }
            wl_output::Event::Scale { factor } => output.pending.scale = factor,
            wl_output::Event::Done => {
                output.pending.done_ns = now_ns();
                if output.pending.name.is_empty()
                    || output.pending.width_px <= 0
                    || output.pending.height_px <= 0
                    || output.pending.refresh_millihz <= 0
                    || output.pending.scale <= 0
                {
                    output.current = None;
                    state
                        .logger
                        .fatal("wl_output Done has incomplete name, current mode or scale");
                    return;
                }
                output.current = Some(output.pending.clone());
                state.logger.emit(json!({"event":"output","role":state.role,
                    "wl_surface_id":state.wl_surface_id,"output":output.current,"at_ns":now_ns()}));
            }
            _ => (),
        }
    }
}

impl Dispatch<wp_presentation::WpPresentation, ()> for PresentationState {
    fn event(
        state: &mut Self,
        _: &wp_presentation::WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.clock_id = Some(clk_id as libc::clockid_t);
            state
                .logger
                .emit(json!({"event":"presentation_ready","at_ns":now_ns(),"clock_id":clk_id,"role":state.role,"wl_surface_id":state.wl_surface_id}));
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, PaintedFeedback>
    for PresentationState
{
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        feedback: &PaintedFeedback,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::SyncOutput { output } => {
                let id = output.id().protocol_id();
                let metadata = state
                    .outputs
                    .get(&id)
                    .and_then(|output| output.current.clone());
                let outputs = state
                    .feedback_outputs
                    .entry(feedback.payload.id())
                    .or_default();
                outputs.ids.insert(id);
                if let Some(metadata) = metadata {
                    outputs.named.insert(metadata.name.clone(), metadata);
                } else {
                    outputs.unknown.insert(id);
                }
            }
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
            } => {
                state.pending.fetch_sub(1, Ordering::Relaxed);
                let callback_ns = now_ns();
                let mut event = feedback.fields("presented");
                state.attach_outputs(feedback.payload.id(), &mut event);
                let Some(clock_id) = state.clock_id else {
                    state.logger.fatal("missing presentation clock");
                    return;
                };
                let before = now_ns();
                let Some(source) = clock_ns(clock_id) else {
                    state.logger.fatal("presentation clock cannot be read");
                    return;
                };
                let after = now_ns();
                let mapped = before + (after - before) / 2;
                let native_present_ns = ((u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo))
                    * 1_000_000_000
                    + u64::from(tv_nsec);
                let present_ns =
                    i128::from(native_present_ns) + i128::from(mapped) - i128::from(source);
                let Ok(present_ns) = u64::try_from(present_ns) else {
                    state
                        .logger
                        .fatal("mapped presentation timestamp out of range");
                    return;
                };
                let timing = json!({"present_ns":present_ns,"native_present_ns":native_present_ns,
                    "callback_ns":callback_ns,"clock_id":clock_id,"refresh_ns":refresh,
                    "sequence":(u64::from(seq_hi)<<32)|u64::from(seq_lo),
                    "flags":format!("{flags:?}"),"calibration":{"source_ns":source,
                    "mapped_ns":mapped,"max_error_ns":(after-before).div_ceil(2)}});
                if let (Some(event), Some(timing)) = (event.as_object_mut(), timing.as_object()) {
                    for (key, value) in timing {
                        if key == "callback_ns"
                            && matches!(&feedback.payload, Feedback::Browser { .. })
                        {
                            event.insert("presentation_callback_ns".into(), value.clone());
                        } else {
                            event.insert(key.clone(), value.clone());
                        }
                    }
                }
                state.logger.emit(event);
            }
            wp_presentation_feedback::Event::Discarded => {
                state.pending.fetch_sub(1, Ordering::Relaxed);
                let mut event = feedback.fields("discarded");
                state.attach_outputs(feedback.payload.id(), &mut event);
                event["at_ns"] = json!(now_ns());
                state.logger.emit(event);
            }
            _ => {}
        }
    }
}
