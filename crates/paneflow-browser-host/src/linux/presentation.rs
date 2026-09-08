use std::cell::RefCell;
use std::collections::BTreeMap;
use std::os::fd::{AsFd, BorrowedFd};
use std::time::Instant;

use cef::*;
use paneflow_browser_protocol::{
    BrowserId, BrowserPresentation, BufferLayout, DirtyRect, Document, FrameAck, FrameChannel,
    FrameFailure, FrameFormat, FrameLedger, FrameMessage, InputEvent, KeyKind, MouseButton,
    PlaneLayout, POOL_BUFFERS, RETIRE_DEADLINE_MS,
};
use serde_json::json;

use super::gpu_contract::GpuContract;
use super::vulkan::{Engine, ExportedImage, Failure, ImportSource};
use super::{emit, now_ns, HOST};

const DEFAULT_WINDOWLESS_FRAME_RATE: i32 = 60;
const MAX_CONSECUTIVE_FAILURES: u32 = 3;
const POOL_INITIALIZATION_TIMEOUT_MS: u64 = 3000;
const NO_REPLACEMENT_RANGE: Range = Range {
    from: u32::MAX,
    to: u32::MAX,
};
const POOL_REFRESH_RETRY_DELAY_MS: i64 = 50;
const POOL_REFRESH_RETRIES: u32 = 16;
const STALE_REFRESH_LIMIT: u32 = 120;
const STALE_REFRESH_DELAY_MS: i64 = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Free,
    Consumer,
}

#[derive(Clone, Copy)]
enum Retirement {
    Active,
    AwaitingReplacement,
    AwaitingRelease(Instant),
}

impl Retirement {
    fn superseded(self) -> bool {
        !matches!(self, Self::Active)
    }

    fn begin_release(&mut self, now: Instant) -> bool {
        if matches!(self, Self::AwaitingReplacement) {
            *self = Self::AwaitingRelease(now);
            true
        } else {
            false
        }
    }

    fn expired(self, now: Instant) -> bool {
        matches!(self, Self::AwaitingRelease(since) if now.duration_since(since).as_millis() >= u128::from(RETIRE_DEADLINE_MS))
    }
}

struct Pool {
    document: Document,
    generation: u64,
    width: u32,
    height: u32,
    format: FrameFormat,
    images: Vec<ExportedImage>,
    slots: [Slot; POOL_BUFFERS as usize],
    retirement: Retirement,
    initialization_started: Option<Instant>,
    painted: bool,
}

#[derive(Clone, Copy)]
struct View {
    width: u32,
    height: u32,
    scale_percent: u32,
    visible: bool,
}

impl View {
    fn physical_size(&self) -> (u32, u32) {
        (
            (self.width * self.scale_percent).div_ceil(100),
            (self.height * self.scale_percent).div_ceil(100),
        )
    }
}

struct Presenter {
    channel: FrameChannel,
    engine: Engine,
    gpu_contract: GpuContract,
    document: Option<Document>,
    view: View,
    pools: Vec<Pool>,
    ledger: FrameLedger,
    sequence: u64,
    pool_generation: u64,
    sent: u64,
    dropped: u64,
    failures: u32,
    stale_refreshes: u32,
    resize_epoch: u64,
    base_frame_rate: i32,
    resize_rate_until: Option<Instant>,
    resize_pending: bool,
    resize_capture_pending: bool,
    slot_starved: bool,
    pool_starved: bool,
    disabled: Option<FrameFailure>,
}

type PresentationConfig = (FrameChannel, Option<(u32, u32)>);

thread_local! {
    static PRESENTERS: RefCell<BTreeMap<BrowserId, Presenter>> = const { RefCell::new(BTreeMap::new()) };
    static CONFIG: RefCell<Option<PresentationConfig>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Presenter) -> R) -> Option<R> {
    let document = super::current_document()?;
    PRESENTERS.with(|state| state.borrow_mut().get_mut(&document.browser).map(f))
}

fn diagnostic_trace_enabled() -> bool {
    HOST.with(|state| {
        state
            .try_borrow()
            .ok()
            .is_some_and(|host| host.as_ref().is_some_and(|host| host.tracing))
    })
}

pub(super) fn active() -> bool {
    CONFIG.with(|state| state.borrow().is_some())
}

pub(super) fn install(
    channel: FrameChannel,
    expected_gpu: Option<(u32, u32)>,
) -> Result<(), String> {
    let engine = Engine::new(expected_gpu).map_err(|failure| failure.detail)?;
    GpuContract::load((engine.vendor_id, engine.device_id))?;
    let reader = channel.try_clone().map_err(|error| error.to_string())?;
    emit(json!({
        "native": "presentation_ready",
        "device": engine.device_name,
        "vendor_id": engine.vendor_id,
        "device_id": engine.device_id,
        "extensions": engine.extensions,
    }));
    CONFIG.with(|state| state.replace(Some((channel, expected_gpu))));
    std::thread::spawn(move || loop {
        match reader.recv::<FrameAck>() {
            Ok(Some((ack, fds))) if fds.is_empty() => {
                let received_ns = now_ns();
                post_task(ThreadId::UI, Some(&mut ReleaseTask::new(ack, received_ns)));
            }
            Ok(Some(_)) | Ok(None) => {
                post_task(ThreadId::UI, Some(&mut ChannelClosed::new()));
                return;
            }
            Err(error) => {
                eprintln!("frame channel rejected: {error}");
                post_task(ThreadId::UI, Some(&mut ChannelClosed::new()));
                return;
            }
        }
    });
    Ok(())
}

pub(super) fn uninstall() {
    let presenters = PRESENTERS.with(|state| std::mem::take(&mut *state.borrow_mut()));
    CONFIG.with(|state| state.borrow_mut().take());
    for (_, mut presenter) in presenters {
        if presenter.pools.iter().any(|pool| {
            pool.initialization_started.is_some() || pool.slots.contains(&Slot::Consumer)
        }) {
            std::mem::forget(presenter);
            continue;
        }
        for pool in std::mem::take(&mut presenter.pools) {
            destroy_pool(&presenter.engine, pool);
        }
    }
}

pub(super) fn create(url: &str) -> bool {
    let frame_rate = match std::env::var("PANEFLOW_BROWSER_FRAME_RATE") {
        Ok(value) => match value.parse::<i32>() {
            Ok(rate @ (60 | 120)) => rate,
            _ => {
                emit(
                    json!({ "native": "create_failed", "reason": "browser frame rate must be 60 or 120" }),
                );
                return false;
            }
        },
        Err(std::env::VarError::NotPresent) => DEFAULT_WINDOWLESS_FRAME_RATE,
        Err(_) => {
            emit(
                json!({ "native": "create_failed", "reason": "browser frame rate is not Unicode" }),
            );
            return false;
        }
    };
    with(|presenter| presenter.base_frame_rate = frame_rate);
    emit(
        json!({ "native": "capture_frame_rate", "fps": frame_rate, "at_ns": now_ns(), "reason": "initial" }),
    );
    let window_info = WindowInfo {
        windowless_rendering_enabled: 1,
        shared_texture_enabled: 1,
        external_begin_frame_enabled: 0,
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    };
    let mut extra = dictionary_value_create();
    if super::devtools::is_inspector() {
        if let Some(info) = extra.as_mut() {
            info.set_bool(Some(&super::devtools::MARKER.into()), 1);
        }
    }
    browser_host_create_browser(
        Some(&window_info),
        Some(&mut super::handlers::WitnessClient::new(
            super::current_document(),
            super::devtools::is_inspector(),
        )),
        Some(&url.into()),
        Some(&BrowserSettings {
            windowless_frame_rate: frame_rate,
            ..Default::default()
        }),
        extra.as_mut(),
        None,
    ) == 1
}

fn browser_host() -> Option<BrowserHost> {
    super::current_browser().and_then(|browser| browser.host())
}

pub(super) fn set_document(document: &Document) -> Result<(), String> {
    if !active() {
        return Ok(());
    }
    if !PRESENTERS.with(|state| state.borrow().contains_key(&document.browser)) {
        let (channel, expected_gpu) = CONFIG.with(|state| {
            let state = state.borrow();
            let (channel, expected_gpu) = state.as_ref().ok_or("missing frame channel")?;
            Ok::<_, String>((
                channel.try_clone().map_err(|error| error.to_string())?,
                *expected_gpu,
            ))
        })?;
        let engine = Engine::new(expected_gpu).map_err(|failure| failure.detail)?;
        let gpu_contract = GpuContract::load((engine.vendor_id, engine.device_id))?;
        let presenter = Presenter {
            channel,
            engine,
            gpu_contract,
            document: None,
            view: View {
                width: 1,
                height: 1,
                scale_percent: 100,
                visible: false,
            },
            pools: Vec::new(),
            ledger: FrameLedger::default(),
            sequence: 0,
            pool_generation: 0,
            sent: 0,
            dropped: 0,
            failures: 0,
            stale_refreshes: 0,
            resize_epoch: 0,
            base_frame_rate: DEFAULT_WINDOWLESS_FRAME_RATE,
            resize_rate_until: None,
            resize_pending: false,
            resize_capture_pending: false,
            slot_starved: false,
            pool_starved: false,
            disabled: None,
        };
        PRESENTERS.with(|state| {
            state
                .borrow_mut()
                .insert(document.browser.clone(), presenter)
        });
    }
    let retire = with(|presenter| {
        let changed = presenter
            .document
            .as_ref()
            .is_some_and(|current| current.generation != document.generation);
        presenter.document = Some(document.clone());
        changed
    })
    .unwrap_or(false);
    if retire {
        retire_active_pool();
    }
    Ok(())
}

pub(super) fn detach() {
    unmount();
    retire_active_pool();
    reap();
}

fn reap() {
    let alive = HOST.with(|state| {
        state
            .borrow()
            .as_ref()
            .and_then(|host| host.pages.get(&super::current_document()?.browser))
            .is_some_and(|page| page.browser.is_some())
    });
    if !alive && with(|presenter| presenter.pools.is_empty()) == Some(true) {
        if let Some(document) = super::current_document() {
            PRESENTERS.with(|state| state.borrow_mut().remove(&document.browser));
        }
    }
}

pub(super) fn unmount() {
    with(|presenter| {
        presenter.view.visible = false;
    });
    if let Some(host) = browser_host() {
        host.was_hidden(1);
    }
}

pub(super) fn present(presentation: &BrowserPresentation) {
    let previous = with(|presenter| {
        let previous = presenter.view;
        presenter.view = View {
            width: presentation.width.max(1),
            height: presentation.height.max(1),
            scale_percent: presentation.scale_percent,
            visible: presentation.visible && presentation.mounted,
        };
        presenter.stale_refreshes = 0;
        presenter.resize_epoch = presenter.resize_epoch.wrapping_add(1);
        presenter.resize_pending = previous.width != presenter.view.width
            || previous.height != presenter.view.height
            || previous.scale_percent != presenter.view.scale_percent;
        presenter.resize_capture_pending = presenter.resize_pending;
        previous
    });
    let (Some(previous), Some(host)) = (previous, browser_host()) else {
        return;
    };
    let current = with(|presenter| presenter.view).unwrap_or(previous);
    if current.scale_percent != previous.scale_percent {
        host.notify_screen_info_changed();
    }
    if current.width != previous.width
        || current.height != previous.height
        || current.scale_percent != previous.scale_percent
    {
        if std::env::var_os("PANEFLOW_BROWSER_BENCH").is_some() {
            emit(
                json!({"native":"resize_host", "at_ns":now_ns(), "generation":presentation.generation, "width":current.width, "height":current.height}),
            );
        }
        let boost = with(|presenter| {
            if !current.visible || presenter.base_frame_rate >= 120 {
                return false;
            }
            let start = presenter.resize_rate_until.is_none();
            presenter.resize_rate_until =
                Some(Instant::now() + std::time::Duration::from_millis(200));
            start
        })
        .unwrap_or(false);
        if boost {
            host.set_windowless_frame_rate(120);
            emit(
                json!({"native":"capture_frame_rate", "fps":120, "at_ns":now_ns(), "reason":"resize"}),
            );
            post_delayed_task(
                ThreadId::UI,
                Some(&mut RestoreFrameRate::new(super::current_document())),
                200,
            );
        }
        host.was_resized();
        if let Some(Err(failure)) = with(Presenter::prepare_resize_pool) {
            fail(failure.reason, failure.detail);
            return;
        }
        if std::env::var("PANEFLOW_BROWSER_RESIZE_REFRESH").as_deref() != Ok("0") {
            if let Some(epoch) = with(|presenter| presenter.resize_epoch) {
                post_task(
                    ThreadId::UI,
                    Some(&mut ResizeRefreshTask::new(
                        super::current_document(),
                        epoch,
                        16,
                    )),
                );
            }
        }
    }
    if current.visible != previous.visible {
        host.was_hidden(i32::from(!current.visible));
        if current.visible {
            host.invalidate(PaintElementType::VIEW);
        }
    }
}

pub(super) fn input(input: InputEvent) {
    let Some(host) = browser_host() else {
        return;
    };
    if matches!(
        &input,
        InputEvent::MouseButton { down: true, .. }
            | InputEvent::Key { .. }
            | InputEvent::Edit { .. }
    ) {
        host.set_focus(1);
    }
    match input {
        InputEvent::MouseMove { x, y, modifiers } => {
            host.send_mouse_move_event(Some(&MouseEvent { x, y, modifiers }), 0);
        }
        InputEvent::MouseLeave { x, y, modifiers } => {
            host.send_mouse_move_event(Some(&MouseEvent { x, y, modifiers }), 1);
        }
        InputEvent::MouseButton {
            x,
            y,
            button,
            down,
            clicks,
            modifiers,
        } => {
            let kind = match button {
                MouseButton::Left => MouseButtonType::LEFT,
                MouseButton::Middle => MouseButtonType::MIDDLE,
                MouseButton::Right => MouseButtonType::RIGHT,
            };
            host.send_mouse_click_event(
                Some(&MouseEvent { x, y, modifiers }),
                kind,
                i32::from(!down),
                i32::from(clicks),
            );
        }
        InputEvent::MouseWheel {
            x,
            y,
            delta_x,
            delta_y,
            modifiers,
        } => {
            host.send_mouse_wheel_event(Some(&MouseEvent { x, y, modifiers }), delta_x, delta_y);
        }
        InputEvent::Key {
            kind,
            key_code,
            native_key_code,
            character,
            unmodified_character,
            modifiers,
        } => {
            let type_ = match kind {
                KeyKind::RawDown => KeyEventType::RAWKEYDOWN,
                KeyKind::Down => KeyEventType::KEYDOWN,
                KeyKind::Up => KeyEventType::KEYUP,
                KeyKind::Char => KeyEventType::CHAR,
            };
            host.send_key_event(Some(&KeyEvent {
                type_,
                modifiers,
                windows_key_code: key_code,
                native_key_code,
                is_system_key: 0,
                character,
                unmodified_character,
                focus_on_editable_field: 0,
                ..Default::default()
            }));
        }
        InputEvent::Focus { focused } => {
            if !focused {
                super::editing::clear();
            }
            host.set_focus(i32::from(focused));
        }
        InputEvent::ClipboardWritten { request } => super::clipboard::written(request),
        InputEvent::Edit { action, request } => super::editing::edit(action, request),
        InputEvent::ContextMenu { request, command } => super::editing::choose(request, command),
        response @ InputEvent::WebResponse { .. } => {
            if let Some(document) = super::current_document() {
                super::web_interactions::handle(&document, &response);
                super::permissions::handle(&document, &response);
                super::external_protocols::handle(&document, &response);
            }
        }
        response @ (InputEvent::TransferResponse { .. } | InputEvent::CancelDownload { .. }) => {
            if let Some(document) = super::current_document() {
                super::transfers::handle(&document, &response);
            }
        }
        InputEvent::DropFiles { paths, x, y } => super::transfers::drop_files(&host, &paths, x, y),
        InputEvent::CaptureLost => host.send_capture_lost_event(),
        InputEvent::ImeComposition {
            text,
            cursor,
            selection_start,
            replacement,
        } => {
            let selection = Range {
                from: selection_start.unwrap_or(cursor),
                to: cursor,
            };
            let replacement = replacement
                .map(|[from, to]| Range { from, to })
                .unwrap_or(NO_REPLACEMENT_RANGE);
            host.ime_set_composition(
                Some(&CefString::from(text.as_str())),
                None,
                Some(&replacement),
                Some(&selection),
            );
        }
        InputEvent::ImeCommit { text, replacement } => {
            let replacement = replacement
                .map(|[from, to]| Range { from, to })
                .unwrap_or(NO_REPLACEMENT_RANGE);
            host.ime_commit_text(Some(&CefString::from(text.as_str())), Some(&replacement), 0);
        }
        InputEvent::ImeCancel => host.ime_cancel_composition(),
        InputEvent::ImeFinish => host.ime_finish_composing_text(1),
    }
}

pub(super) fn view_rect() -> Rect {
    let view = with(|presenter| presenter.view);
    Rect {
        x: 0,
        y: 0,
        width: view.map_or(1, |view| view.width as i32),
        height: view.map_or(1, |view| view.height as i32),
    }
}

pub(super) fn screen_info() -> ScreenInfo {
    let rect = view_rect();
    let scale = with(|presenter| presenter.view.scale_percent).unwrap_or(100);
    ScreenInfo {
        device_scale_factor: scale as f32 / 100.0,
        depth: 24,
        depth_per_component: 8,
        is_monochrome: 0,
        rect: rect.clone(),
        available_rect: rect,
        ..Default::default()
    }
}

pub(super) fn software_paint(width: i32, height: i32) {
    let first = with(|presenter| {
        let first = presenter.disabled.is_none();
        presenter.disabled = Some(FrameFailure::UnsupportedFormat);
        presenter.dropped += 1;
        first
    })
    .unwrap_or(false);
    if first {
        fail(
            FrameFailure::UnsupportedFormat,
            format!(
                "CEF fell back to software painting ({width}x{height}); the CPU path is refused"
            ),
        );
    }
}

pub(super) fn accelerated_paint(
    type_: PaintElementType,
    dirty_rects: Option<&[Rect]>,
    info: Option<&AcceleratedPaintInfo>,
) {
    let callback_ns = now_ns();
    if type_ != PaintElementType::VIEW {
        return;
    }
    let Some(info) = info else {
        return;
    };
    let outcome = with(|presenter| presenter.paint(info, dirty_rects, callback_ns));
    match outcome {
        Some(Ok(Some(message))) => send(message, &[]),
        Some(Ok(None)) => (),
        Some(Err(failure)) => {
            let disabled = with(|presenter| {
                presenter.failures += 1;
                presenter.dropped += 1;
                if presenter.failures >= MAX_CONSECUTIVE_FAILURES {
                    presenter.disabled = Some(failure.reason);
                }
                presenter.disabled
            })
            .flatten();
            fail(failure.reason, failure.detail);
            if disabled.is_some() {
                emit(json!({ "native": "presentation_disabled", "reason": disabled }));
            }
        }
        None => (),
    }
}

fn fail(reason: FrameFailure, detail: String) {
    let document = with(|presenter| presenter.document.clone()).flatten();
    emit(json!({ "native": "frame_failed", "reason": reason, "detail": detail }));
    if let Some(document) = document {
        let detail: String = detail.chars().take(1024).collect();
        send(
            FrameMessage::Failed {
                document,
                reason,
                detail,
            },
            &[],
        );
    }
}

fn send(message: FrameMessage, fds: &[BorrowedFd<'_>]) {
    let result = with(|presenter| presenter.channel.send(&message, fds));
    if let Some(Err(error)) = result {
        emit(json!({ "native": "frame_channel_failed", "detail": error.to_string() }));
        super::close();
    }
}

fn destroy_pool(engine: &Engine, pool: Pool) {
    for image in pool.images {
        engine.destroy_exported_image(image);
    }
}

fn retire_active_pool() {
    let retired = with(|presenter| presenter.retire_active());
    if let Some(messages) = retired {
        for message in messages {
            send(message, &[]);
        }
    }
}

impl Presenter {
    fn active_index(&self) -> Option<usize> {
        self.pools
            .iter()
            .position(|pool| !pool.retirement.superseded())
    }

    fn retire_active(&mut self) -> Vec<FrameMessage> {
        let messages = self.retire(false);
        self.begin_retirements();
        messages
    }

    fn begin_retirements(&mut self) {
        for pool in &mut self.pools {
            if pool.retirement.begin_release(Instant::now()) {
                post_delayed_task(
                    ThreadId::UI,
                    Some(&mut RetireCheck::new(
                        super::current_document(),
                        pool.generation,
                    )),
                    RETIRE_DEADLINE_MS as i64,
                );
            }
        }
    }

    fn retire(&mut self, await_replacement: bool) -> Vec<FrameMessage> {
        let mut messages = Vec::new();
        let Some(index) = self.active_index() else {
            return messages;
        };
        let outstanding = self.pools[index].initialization_started.is_some()
            || self.pools[index].slots.contains(&Slot::Consumer);
        if outstanding {
            self.pools[index].retirement = Retirement::AwaitingReplacement;
            if !await_replacement {
                self.pools[index].retirement.begin_release(Instant::now());
                post_delayed_task(
                    ThreadId::UI,
                    Some(&mut RetireCheck::new(
                        super::current_document(),
                        self.pools[index].generation,
                    )),
                    RETIRE_DEADLINE_MS as i64,
                );
            }
        } else {
            let pool = self.pools.remove(index);
            messages.push(FrameMessage::PoolRetired {
                document: pool.document.clone(),
                pool_generation: pool.generation,
            });
            destroy_pool(&self.engine, pool);
        }
        self.ledger.invalidate();
        messages
    }

    fn prepare_resize_pool(&mut self) -> Result<(), Failure> {
        if !self.view.visible || self.disabled.is_some() {
            return Ok(());
        }
        let Some(document) = self.document.clone() else {
            return Ok(());
        };
        let Some(index) = self.active_index() else {
            return Ok(());
        };
        let (width, height) = self.view.physical_size();
        let pool = &self.pools[index];
        if (pool.width, pool.height) == (width, height) {
            return Ok(());
        }
        let format = pool.format;
        let painted = pool.painted;
        if painted && self.pools.len() >= 2 {
            return Ok(());
        }
        for message in self.retire(painted) {
            self.channel.send(&message, &[]).map_err(channel_failure)?;
        }
        if self.pools.len() >= 2 {
            self.pool_starved = true;
            return Ok(());
        }
        self.create_pool(document, width, height, format)?;
        if std::env::var_os("PANEFLOW_BROWSER_BENCH").is_some() {
            emit(json!({"native":"resize_pool", "at_ns":now_ns(), "width":width, "height":height}));
        }
        Ok(())
    }

    fn paint(
        &mut self,
        info: &AcceleratedPaintInfo,
        dirty_rects: Option<&[Rect]>,
        callback_ns: u64,
    ) -> Result<Option<FrameMessage>, Failure> {
        if self.disabled.is_some() || !self.view.visible {
            self.dropped += 1;
            return Ok(None);
        }
        let Some(document) = self.document.clone() else {
            self.dropped += 1;
            return Ok(None);
        };
        if let Err(detail) = self.gpu_contract.verify() {
            self.disabled = Some(FrameFailure::CopyFailed);
            return Err(Failure {
                reason: FrameFailure::CopyFailed,
                detail,
            });
        }
        let format = match info.format {
            ColorType::BGRA_8888 => FrameFormat::Bgra8,
            ColorType::RGBA_8888 => FrameFormat::Rgba8,
            other => {
                return Err(Failure {
                    reason: FrameFailure::UnsupportedFormat,
                    detail: format!("CEF color type {other:?} is not a supported frame format"),
                });
            }
        };
        let visible = &info.extra.visible_rect;
        let coded = &info.extra.coded_size;
        let plane_count = usize::try_from(info.plane_count).unwrap_or(0);
        if visible.width <= 0
            || visible.height <= 0
            || visible.x < 0
            || visible.y < 0
            || coded.width < visible.x + visible.width
            || coded.height < visible.y + visible.height
            || plane_count == 0
            || plane_count > info.planes.len()
        {
            return Err(Failure {
                reason: FrameFailure::InvalidHandle,
                detail: format!(
                    "CEF frame geometry is inconsistent: visible {}x{}+{}+{} coded {}x{} planes {}",
                    visible.width,
                    visible.height,
                    visible.x,
                    visible.y,
                    coded.width,
                    coded.height,
                    plane_count
                ),
            });
        }
        let planes = &info.planes[..plane_count];
        if planes.iter().any(|plane| plane.fd < 0)
            || planes.iter().any(|plane| plane.fd != planes[0].fd)
        {
            return Err(Failure {
                reason: FrameFailure::InvalidHandle,
                detail: "CEF native pixmap planes must share one descriptor".to_string(),
            });
        }
        let width = visible.width as u32;
        let height = visible.height as u32;
        let expected = self.view.physical_size();
        if (width, height) != expected {
            self.dropped += 1;
            self.stale_refreshes += 1;
            if self.stale_refreshes == 1 || self.stale_refreshes == STALE_REFRESH_LIMIT {
                emit(json!({
                    "native": "frame_dropped",
                    "reason": "stale_geometry",
                    "coded": [coded.width, coded.height],
                    "visible": [visible.width, visible.height],
                    "expected": [expected.0, expected.1],
                    "scale_percent": self.view.scale_percent,
                    "refreshes": self.stale_refreshes,
                }));
            }
            if self.stale_refreshes <= STALE_REFRESH_LIMIT {
                post_delayed_task(
                    ThreadId::UI,
                    Some(&mut RefreshTask::new(super::current_document(), 0)),
                    STALE_REFRESH_DELAY_MS,
                );
            }
            return Ok(None);
        }
        self.stale_refreshes = 0;
        if self.resize_capture_pending {
            self.resize_capture_pending = false;
            if std::env::var_os("PANEFLOW_BROWSER_BENCH").is_some() {
                emit(
                    json!({"native":"resize_capture", "at_ns":now_ns(), "width":width, "height":height}),
                );
            }
        }
        let needs_pool = self.active_index().is_none_or(|index| {
            let pool = &self.pools[index];
            pool.width != width || pool.height != height || pool.format != format
        });
        if needs_pool {
            if self.pools.len() >= 2 {
                self.dropped += 1;
                self.pool_starved = true;
                return Ok(None);
            }
            for message in self.retire(true) {
                self.channel.send(&message, &[]).map_err(channel_failure)?;
            }
            self.create_pool(document.clone(), width, height, format)?;
            if std::env::var_os("PANEFLOW_BROWSER_BENCH").is_some() {
                emit(
                    json!({"native":"resize_pool", "at_ns":now_ns(), "width":width, "height":height}),
                );
            }
        }
        let index = self.active_index().ok_or_else(|| Failure {
            reason: FrameFailure::CopyFailed,
            detail: "no active pool after creation".to_string(),
        })?;
        if self.pools[index].initialization_started.is_some() {
            self.dropped += 1;
            return Ok(None);
        }
        let Some(slot) = self.pools[index]
            .slots
            .iter()
            .position(|slot| *slot == Slot::Free)
        else {
            if diagnostic_trace_enabled() {
                let pool = &self.pools[index];
                emit(json!({
                    "native": "frame_slot_starved",
                    "clock": "CLOCK_MONOTONIC",
                    "at_ns": now_ns(),
                    "callback_ns": callback_ns,
                    "document": document,
                    "pool_generation": pool.generation,
                    "capture_counter": (info.extra.has_capture_counter != 0)
                        .then_some(info.extra.capture_counter),
                    "capture_timestamp_us": info.extra.timestamp,
                    "occupied_slots": pool.slots.iter().filter(|slot| **slot == Slot::Consumer).count(),
                    "total_slots": pool.slots.len(),
                }));
            }
            self.dropped += 1;
            self.slot_starved = true;
            return Ok(None);
        };
        let layouts: Vec<PlaneLayout> = planes
            .iter()
            .map(|plane| PlaneLayout {
                stride: plane.stride,
                offset: plane.offset,
                size: plane.size,
            })
            .collect();
        let description = format!(
            "cef frame format {format:?} modifier {:#x} coded {}x{} visible {}x{}+{}+{} planes {:?} fd {} device {}",
            info.modifier,
            coded.width,
            coded.height,
            visible.width,
            visible.height,
            visible.x,
            visible.y,
            layouts,
            planes[0].fd,
            self.engine.device_name
        );
        if needs_pool {
            emit(json!({ "native": "frame_source", "detail": description }));
        }
        let source = ImportSource {
            fd: planes[0].fd,
            planes: &layouts,
            modifier: info.modifier,
            format,
            coded_width: coded.width as u32,
            coded_height: coded.height as u32,
            x: visible.x as u32,
            y: visible.y as u32,
            width,
            height,
        };
        let pool = &mut self.pools[index];
        let ready_ns = self
            .engine
            .copy_frame(&source, &pool.images[slot])
            .map_err(|failure| Failure {
                reason: failure.reason,
                detail: format!("{}; {description}", failure.detail),
            })?;
        self.sequence += 1;
        let sequence = self.sequence;
        self.ledger
            .receive(document.generation, pool.generation, slot as u8, sequence)
            .map_err(|error| Failure {
                reason: FrameFailure::CopyFailed,
                detail: format!("frame ledger refused sequence {sequence}: {error:?}"),
            })?;
        pool.slots[slot] = Slot::Consumer;
        if diagnostic_trace_enabled() {
            emit(json!({
                "native": "frame_slot_occupied",
                "clock": "CLOCK_MONOTONIC",
                "at_ns": now_ns(),
                "document": document,
                "pool_generation": pool.generation,
                "buffer": slot,
                "sequence": sequence,
                "capture_counter": (info.extra.has_capture_counter != 0)
                    .then_some(info.extra.capture_counter),
                "capture_timestamp_us": info.extra.timestamp,
                "occupied_slots": pool.slots.iter().filter(|slot| **slot == Slot::Consumer).count(),
                "total_slots": pool.slots.len(),
            }));
        }
        self.failures = 0;
        self.sent += 1;
        self.resize_pending = false;
        pool.painted = true;
        let dirty = dirty_rects.and_then(|rects| union(rects, width, height));
        let frame = FrameMessage::Frame {
            document,
            pool_generation: pool.generation,
            buffer: slot as u8,
            sequence,
            callback_ns,
            ready_ns,
            capture_timestamp_us: info.extra.timestamp,
            capture_counter: (info.extra.has_capture_counter != 0)
                .then_some(info.extra.capture_counter),
            dirty,
        };
        self.begin_retirements();
        Ok(Some(frame))
    }

    fn create_pool(
        &mut self,
        document: Document,
        width: u32,
        height: u32,
        format: FrameFormat,
    ) -> Result<(), Failure> {
        let modifier = self.engine.choose_export_modifier(format)?;
        let generation = self.pool_generation + 1;
        self.ledger
            .resize(document.generation, generation)
            .map_err(|error| Failure {
                reason: FrameFailure::RetireTimeout,
                detail: format!("frame ledger refused pool {generation}: {error:?}"),
            })?;
        let mut images = match self.engine.create_exported_images(
            width,
            height,
            format,
            modifier,
            POOL_BUFFERS as usize,
        ) {
            Ok(images) => images,
            Err(failure) => {
                self.ledger.invalidate();
                return Err(failure);
            }
        };
        self.pool_generation = generation;
        let buffers = images
            .iter()
            .enumerate()
            .map(|(slot, image)| BufferLayout {
                slot: slot as u8,
                planes: vec![image.layout],
            })
            .collect();
        let fds: Vec<std::os::fd::OwnedFd> = images
            .iter_mut()
            .filter_map(|image| image.fd.take())
            .collect();
        let borrowed: Vec<BorrowedFd<'_>> = fds.iter().map(AsFd::as_fd).collect();
        let message = FrameMessage::PoolCreated {
            document: document.clone(),
            pool_generation: generation,
            width,
            height,
            format,
            modifier,
            buffers,
        };
        if let Err(error) = self.channel.send(&message, &borrowed) {
            drop(borrowed);
            for image in images {
                self.engine.destroy_exported_image(image);
            }
            self.ledger.invalidate();
            return Err(channel_failure(error));
        }
        emit(json!({
            "native": "pool_created",
            "pool_generation": generation,
            "width": width,
            "height": height,
            "format": format,
            "modifier": modifier,
        }));
        self.pools.push(Pool {
            document,
            generation,
            width,
            height,
            format,
            images,
            slots: [Slot::Free; POOL_BUFFERS as usize],
            retirement: Retirement::Active,
            initialization_started: Some(Instant::now()),
            painted: false,
        });
        post_delayed_task(
            ThreadId::UI,
            Some(&mut RetireCheck::new(super::current_document(), generation)),
            POOL_INITIALIZATION_TIMEOUT_MS as i64,
        );
        Ok(())
    }

    fn pool_ready(
        &mut self,
        document: Document,
        generation: u64,
        received_ns: u64,
    ) -> Vec<FrameMessage> {
        let Some(index) = self
            .pools
            .iter()
            .position(|pool| pool.document == document && pool.generation == generation)
        else {
            emit(
                json!({ "native": "pool_ready_rejected", "pool_generation": generation, "reason": "unknown pool document" }),
            );
            return Vec::new();
        };
        if self.disabled.is_some() || self.pools[index].initialization_started.is_none() {
            emit(
                json!({ "native": "pool_ready_rejected", "pool_generation": generation, "reason": "duplicate or failed initialization" }),
            );
            return Vec::new();
        }
        self.pools[index].initialization_started = None;
        emit(
            json!({ "native": "pool_ready_accepted", "document": document, "pool_generation": generation, "received_ns": received_ns, "at_ns": now_ns() }),
        );
        if self.pools[index].retirement.superseded() {
            let pool = self.pools.remove(index);
            let message = FrameMessage::PoolRetired {
                document: pool.document.clone(),
                pool_generation: generation,
            };
            destroy_pool(&self.engine, pool);
            if std::mem::take(&mut self.pool_starved) {
                post_task(
                    ThreadId::UI,
                    Some(&mut RefreshTask::new(
                        super::current_document(),
                        POOL_REFRESH_RETRIES,
                    )),
                );
            }
            return vec![message];
        }
        if self.view.visible && self.document.as_ref() == Some(&document) {
            post_task(
                ThreadId::UI,
                Some(&mut RefreshTask::new(
                    super::current_document(),
                    POOL_REFRESH_RETRIES,
                )),
            );
        }
        Vec::new()
    }

    fn awaits_first_frame(&self) -> bool {
        self.disabled.is_none()
            && self.view.visible
            && self.active_index().is_some_and(|index| {
                let pool = &self.pools[index];
                pool.initialization_started.is_none() && !pool.painted
            })
    }

    fn pool_rejected(&mut self, document: Document, generation: u64) -> Vec<FrameMessage> {
        let Some(index) = self.pools.iter().position(|pool| {
            pool.document == document
                && pool.generation == generation
                && pool.initialization_started.is_some()
        }) else {
            emit(json!({ "native": "pool_rejection_refused", "pool_generation": generation }));
            return Vec::new();
        };
        let pool = self.pools.remove(index);
        self.ledger.forget_pool(document.generation, generation);
        destroy_pool(&self.engine, pool);
        emit(
            json!({ "native": "pool_rejection_accepted", "document": document, "pool_generation": generation }),
        );
        vec![FrameMessage::PoolRetired {
            document,
            pool_generation: generation,
        }]
    }

    fn release(&mut self, ack: FrameAck, received_ns: u64) -> Vec<FrameMessage> {
        let FrameAck::Release {
            document,
            pool_generation,
            buffer,
            sequence,
        } = ack
        else {
            return match ack {
                FrameAck::PoolReady {
                    document,
                    pool_generation,
                } => self.pool_ready(document, pool_generation, received_ns),
                FrameAck::PoolRejected {
                    document,
                    pool_generation,
                } => self.pool_rejected(document, pool_generation),
                FrameAck::Release { .. } => Vec::new(),
            };
        };
        if self
            .pools
            .iter()
            .all(|pool| pool.generation != pool_generation || pool.document != document)
        {
            emit(
                json!({ "native": "release_rejected", "pool_generation": pool_generation, "reason": "pool document mismatch" }),
            );
            return Vec::new();
        }
        let tracing = diagnostic_trace_enabled();
        if tracing {
            let pool = self
                .pools
                .iter()
                .find(|pool| pool.generation == pool_generation);
            emit(json!({
                "native": "frame_ack_received",
                "clock": "CLOCK_MONOTONIC",
                "received_ns": received_ns,
                "ui_dispatch_ns": now_ns(),
                "document": document,
                "pool_generation": pool_generation,
                "buffer": buffer,
                "sequence": sequence,
                "occupied_slots_at_ui_dispatch": pool.map(|pool| pool.slots.iter().filter(|slot| **slot == Slot::Consumer).count()),
                "total_slots": pool.map(|pool| pool.slots.len()),
            }));
        }
        let mut messages = Vec::new();
        if self
            .ledger
            .release(document.generation, pool_generation, buffer, sequence)
            .is_err()
        {
            emit(
                json!({ "native": "release_rejected", "pool_generation": pool_generation, "buffer": buffer, "sequence": sequence }),
            );
            return messages;
        }
        let Some(index) = self
            .pools
            .iter()
            .position(|pool| pool.generation == pool_generation)
        else {
            return messages;
        };
        self.pools[index].slots[usize::from(buffer)] = Slot::Free;
        if tracing {
            let pool = &self.pools[index];
            emit(json!({
                "native": "frame_slot_released",
                "clock": "CLOCK_MONOTONIC",
                "at_ns": now_ns(),
                "received_ns": received_ns,
                "document": document,
                "pool_generation": pool_generation,
                "buffer": buffer,
                "sequence": sequence,
                "occupied_slots": pool.slots.iter().filter(|slot| **slot == Slot::Consumer).count(),
                "total_slots": pool.slots.len(),
            }));
        }
        let retiring = self.pools[index].retirement.superseded();
        if !retiring && std::mem::take(&mut self.slot_starved) {
            post_delayed_task(
                ThreadId::UI,
                Some(&mut RefreshTask::new(super::current_document(), 0)),
                0,
            );
        }
        let drained = self.pools[index]
            .slots
            .iter()
            .all(|slot| *slot == Slot::Free);
        if retiring && drained {
            let pool = self.pools.remove(index);
            messages.push(FrameMessage::PoolRetired {
                document: pool.document.clone(),
                pool_generation: pool.generation,
            });
            destroy_pool(&self.engine, pool);
            if std::mem::take(&mut self.pool_starved) {
                post_delayed_task(
                    ThreadId::UI,
                    Some(&mut RefreshTask::new(super::current_document(), 0)),
                    0,
                );
            }
        }
        messages
    }

    fn retire_check(&mut self, generation: u64) -> Vec<FrameMessage> {
        let Some(pool) = self.pools.iter().find(|pool| pool.generation == generation) else {
            return Vec::new();
        };
        let initialization_expired = pool.initialization_started.is_some_and(|since| {
            since.elapsed().as_millis() >= u128::from(POOL_INITIALIZATION_TIMEOUT_MS)
        });
        let retirement_expired = pool.retirement.expired(Instant::now());
        if self.disabled.is_some() || (!initialization_expired && !retirement_expired) {
            return Vec::new();
        }
        self.disabled = Some(FrameFailure::RetireTimeout);
        emit(
            json!({ "native": "pool_quarantined", "pool_generation": generation, "initializing": pool.initialization_started.is_some() }),
        );
        vec![FrameMessage::Failed {
            document: pool.document.clone(),
            reason: FrameFailure::RetireTimeout,
            detail: format!(
                "pool {generation} exceeded its GPU ownership deadline; production stopped and allocations retained until host exit"
            ),
        }]
    }
}

fn channel_failure(error: std::io::Error) -> Failure {
    Failure {
        reason: FrameFailure::CopyFailed,
        detail: format!("frame channel: {error}"),
    }
}

fn union(rects: &[Rect], width: u32, height: u32) -> Option<DirtyRect> {
    let mut left = i32::MAX;
    let mut top = i32::MAX;
    let mut right = i32::MIN;
    let mut bottom = i32::MIN;
    for rect in rects {
        if rect.width <= 0 || rect.height <= 0 {
            continue;
        }
        left = left.min(rect.x.max(0));
        top = top.min(rect.y.max(0));
        right = right.max((rect.x + rect.width).min(width as i32));
        bottom = bottom.max((rect.y + rect.height).min(height as i32));
    }
    if left >= right || top >= bottom {
        return None;
    }
    Some(DirtyRect {
        x: left as u32,
        y: top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

wrap_task! {
    struct ReleaseTask {
        ack: FrameAck,
        received_ns: u64,
    }

    impl Task {
        fn execute(&self) {
            let document = match &self.ack { FrameAck::Release { document, .. } | FrameAck::PoolReady { document, .. } | FrameAck::PoolRejected { document, .. } => document.clone() };
            let _context = super::Context::enter(Some(document));
            let ack = self.ack.clone();
            if let Some(messages) = with(|presenter| presenter.release(ack, self.received_ns)) {
                for message in messages {
                    send(message, &[]);
                }
            }
            reap();
        }
    }
}

wrap_task! {
    struct RestoreFrameRate { document: Option<Document> }

    impl Task {
        fn execute(&self) {
            let _context = super::Context::enter(self.document.clone());
            let rate = with(|presenter| {
                let deadline = presenter.resize_rate_until?;
                if presenter.view.visible && presenter.disabled.is_none() && Instant::now() < deadline {
                    return None;
                }
                presenter.resize_rate_until = None;
                Some(presenter.base_frame_rate)
            }).flatten();
            if let Some(rate) = rate {
                if let Some(host) = browser_host() {
                    host.set_windowless_frame_rate(rate);
                    emit(json!({"native":"capture_frame_rate", "fps":rate, "at_ns":now_ns(), "reason":"restore"}));
                }
            } else if with(|presenter| presenter.resize_rate_until.is_some()) == Some(true) {
                post_delayed_task(ThreadId::UI, Some(&mut RestoreFrameRate::new(super::current_document())), 50);
            }
        }
    }
}

wrap_task! {
    struct ResizeRefreshTask {
        document: Option<Document>,
        epoch: u64,
        remaining: u32,
    }

    impl Task {
        fn execute(&self) {
            let _context = super::Context::enter(self.document.clone());
            let needed = with(|presenter| presenter.resize_epoch == self.epoch
                && presenter.resize_pending && presenter.view.visible
                && presenter.disabled.is_none()).unwrap_or(false);
            if std::env::var_os("PANEFLOW_BROWSER_BENCH").is_some() { emit(json!({ "native": "resize_refresh", "needed": needed, "browser_available": browser_host().is_some(), "remaining": self.remaining, "epoch": self.epoch })); }
            if !needed || self.remaining == 0 { return; }
            if let Some(host) = browser_host() {
                host.invalidate(PaintElementType::VIEW);
                post_delayed_task(ThreadId::UI,
                    Some(&mut ResizeRefreshTask::new(super::current_document(), self.epoch, self.remaining - 1)), 16);
            }
        }
    }
}

wrap_task! {
    struct RefreshTask {
        document: Option<Document>,
        retries: u32,
    }

    impl Task {
        fn execute(&self) {
            let _context = super::Context::enter(self.document.clone());
            if std::env::var_os("PANEFLOW_BROWSER_BENCH").is_some() { emit(json!({ "native": "refresh_requested", "browser_available": browser_host().is_some(), "retries": self.retries })); }
            if let Some(host) = browser_host() {
                host.invalidate(PaintElementType::VIEW);
            }
            if self.retries > 0 && with(|presenter| presenter.awaits_first_frame()) == Some(true) {
                post_delayed_task(
                    ThreadId::UI,
                    Some(&mut RefreshTask::new(super::current_document(), self.retries - 1)),
                    POOL_REFRESH_RETRY_DELAY_MS,
                );
            }
        }
    }
}

wrap_task! {
    struct RetireCheck {
        document: Option<Document>,
        generation: u64,
    }

    impl Task {
        fn execute(&self) {
            let _context = super::Context::enter(self.document.clone());
            let generation = self.generation;
            if let Some(messages) = with(|presenter| presenter.retire_check(generation)) {
                for message in messages {
                    send(message, &[]);
                }
            }
        }
    }
}

wrap_task! {
    struct ChannelClosed;

    impl Task {
        fn execute(&self) {
            if active() {
                emit(json!({ "native": "frame_channel_closed" }));
                super::close();
            }
        }
    }
}

wrap_render_handler! {
    pub struct Renderer { document: Option<Document> }

    impl RenderHandler {
        fn update_drag_cursor(&self, browser: Option<&mut Browser>, operation: DragOperationsMask) {
            let _context = super::Context::browser(browser.as_deref());
            if let Some(browser) = browser { super::transfers::update_drag_cursor(browser, operation); }
        }
        fn accessibility_handler(&self) -> Option<AccessibilityHandler> { self.document.clone().map(super::accessibility::Accessibility::new) }
        fn on_text_selection_changed(&self, _browser: Option<&mut Browser>, selected_text: Option<&CefString>, selected_range: Option<&Range>) {
            let _context = super::Context::browser(_browser.as_deref());
            let Some(range) = selected_range else { return; };
            let Some(document) = with(|presenter| presenter.document.clone()).flatten() else { return; };
            let text = selected_text.map(ToString::to_string).filter(|text| text.len() <= 65536);
            emit(json!({"native":"ime_selection", "document":document, "snapshot":{"start":range.from, "end":range.to, "text":text}}));
        }

        fn on_ime_composition_range_changed(&self, _browser: Option<&mut Browser>, selected_range: Option<&Range>, character_bounds: Option<&[Rect]>) {
            let _context = super::Context::browser(_browser.as_deref());
            let Some(range) = selected_range else { return; };
            let Some(document) = with(|presenter| presenter.document.clone()).flatten() else { return; };
            let bounds: Vec<_> = character_bounds.unwrap_or_default().iter().take(4096).map(|r| [r.x,r.y,r.width,r.height]).collect();
            emit(json!({"native":"ime_bounds", "document":document, "snapshot":{"start":range.from, "end":range.to, "bounds":bounds}}));
        }

        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            let _context = super::Context::browser(_browser.as_deref());
            if let Some(rect) = rect {
                *rect = view_rect();
            }
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, screen_info: Option<&mut ScreenInfo>) -> i32 {
            let _context = super::Context::browser(_browser.as_deref());
            if let Some(screen_info) = screen_info {
                *screen_info = self::screen_info();
                return 1;
            }
            0
        }

        fn on_paint(&self, _browser: Option<&mut Browser>, type_: PaintElementType, _dirty_rects: Option<&[Rect]>, _buffer: *const u8, width: i32, height: i32) {
            let _context = super::Context::browser(_browser.as_deref());
            if type_ == PaintElementType::VIEW {
                software_paint(width, height);
            }
        }

        fn on_accelerated_paint(&self, _browser: Option<&mut Browser>, type_: PaintElementType, dirty_rects: Option<&[Rect]>, info: Option<&AcceleratedPaintInfo>) {
            let _context = super::Context::browser(_browser.as_deref());
            accelerated_paint(type_, dirty_rects, info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displayed_pool_retirement_waits_for_replacement_then_keeps_exact_deadline() {
        let resize = Instant::now();
        let replacement = resize + std::time::Duration::from_millis(5000);
        let mut retirement = Retirement::AwaitingReplacement;
        assert!(retirement.superseded());
        assert!(!retirement.expired(replacement));
        assert!(retirement.begin_release(replacement));
        assert!(!retirement
            .expired(replacement + std::time::Duration::from_millis(RETIRE_DEADLINE_MS - 1)));
        assert!(
            retirement.expired(replacement + std::time::Duration::from_millis(RETIRE_DEADLINE_MS))
        );
        assert!(!retirement.begin_release(replacement + std::time::Duration::from_millis(100)));
    }
}
